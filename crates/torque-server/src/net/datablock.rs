//! `SimDataBlockEvent` as the host sends it: Torque's frame (the flag, the datablock id, the class id, the position in the
//! datablock group and the group's size) around the record a game's codecs write from the host's view of the datablock
//! ([`NetCodecs::datablock_record`](super::codecs::NetCodecs::datablock_record), the class's `packData`). The host never reads
//! a record: it renders one, measures it, and packs it into a packet.
//!
//! Credit: the frame follows Torque3D's MIT-licensed `T3D/gameBase/gameConnectionEvents.cpp` (Copyright (c) 2012 GarageGames,
//! LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use std::any::Any;
use std::fmt;
use std::rc::Rc;

use torque_net::bits::{Affine, BitWriter};
use torque_net::classes::{Kind, NetClasses};
use torque_net::huffman::Table;

/// The first datablock id (`DataBlockObjectIdFirst`).
pub const DATABLOCK_ID_FIRST: u32 = 3;

/// The scratch stream one event is measured in: Torque's packet data size (`Net::MaxPacketDataSize`, 1,500 bytes).
pub const EVENT_SCRATCH_BITS: usize = 1500 * 8;

/// One value of a datablock as the host's view answers it: a field by name and element index.
#[derive(Debug, Clone, PartialEq)]
pub enum DbValue {
    /// No value (a null pointer or string, an unset field).
    Null,
    Bool(bool),
    Int(i64),
    F32(f32),
    Str(Vec<u8>),
    /// A datablock pointer resolved to the object's id; `None` is null (or an object that is gone or no datablock).
    Id(Option<u32>),
    /// A transform as Torque writes it (`writeAffineTransform`); `None` is the identity.
    Xform(Option<Affine>),
}

/// The host's getters for one datablock: a field by name and element index (0 for a scalar; the element of an array field; the
/// component of a `Point3F` or `ColorF`, an array element `i` at components `3 * i + c` or `4 * i + c`).
pub trait DatablockView {
    fn get(&self, field: &str, index: usize) -> DbValue;
}

/// Why a datablock event could not be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbPackError {
    /// The event is larger than one packet can carry.
    TooBig { bits: usize, limit: usize },
    /// The class has no id in the class table the host passed.
    UnknownClass(&'static str),
    /// A record that does not follow its class's layout.
    Malformed(&'static str),
}

impl fmt::Display for DbPackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbPackError::TooBig { bits, limit } => write!(
                f,
                "datablock event of {bits} bits does not fit the {limit} bits of one packet"
            ),
            DbPackError::UnknownClass(c) => write!(f, "{c} has no class id"),
            DbPackError::Malformed(w) => write!(f, "malformed datablock record: {w}"),
        }
    }
}

impl std::error::Error for DbPackError {}

/// A datablock's payload as a game's codecs wrote it (its `packData`).
pub trait DatablockRecord: fmt::Debug {
    /// Writes the payload (the packet's stream carries the string buffer).
    fn encode(&self, w: &mut BitWriter, table: &Table) -> Result<(), DbPackError>;
    /// The values it holds (the host charges its render by them).
    fn entries(&self) -> usize;
    fn as_any(&self) -> &dyn Any;
    /// The class id the event carries, when the record's class has none in the class table (a plugin's class, numbered past
    /// the table: `torque_ghosts::plugin`). `None` by default: the table's id of the body's class.
    fn class_id(&self) -> Option<u32> {
        None
    }
}

/// The datablock part of a `SimDataBlockEvent`, present when the connection's key is older than the datablock's.
#[derive(Debug, Clone)]
pub struct DatablockBody {
    /// The object id as the wire keeps it.
    pub id: u32,
    /// The class's name in the class table.
    pub class: &'static str,
    /// The position in the datablock group.
    pub index: u32,
    /// The group's size.
    pub total: u32,
    pub record: Rc<dyn DatablockRecord>,
}

impl DatablockBody {
    /// A body with its numbers as the wire keeps them, datablock ids being `id_bits` wide (the size one bit wider).
    pub fn new(
        id: u32,
        class: &'static str,
        index: u32,
        total: u32,
        id_bits: u8,
        record: Rc<dyn DatablockRecord>,
    ) -> DatablockBody {
        let mask = id_mask(id_bits);
        DatablockBody {
            id: (id.wrapping_sub(DATABLOCK_ID_FIRST) & mask) + DATABLOCK_ID_FIRST,
            class,
            index: index & mask,
            total: total & ((mask << 1) | 1),
            record,
        }
    }
}

fn id_mask(id_bits: u8) -> u32 {
    if id_bits >= 32 {
        u32::MAX
    } else {
        (1u32 << id_bits) - 1
    }
}

/// `SimDataBlockEvent` (server to client): `flag(modified)` then, when set, the body.
#[derive(Debug, Clone)]
pub struct DatablockEvent {
    /// `None` is the one-bit event of a datablock the connection already has (or that is gone): the client does nothing.
    pub body: Option<DatablockBody>,
    /// The width of a datablock id.
    pub id_bits: u8,
}

impl DatablockEvent {
    /// Writes the event: the flag, the id less [`DATABLOCK_ID_FIRST`], the class id (the width of the class table's datablock
    /// group), the position, the group's size (one bit wider than an id), then the record.
    pub fn pack(
        &self,
        w: &mut BitWriter,
        table: &Table,
        classes: &NetClasses,
    ) -> Result<(), DbPackError> {
        let Some(body) = &self.body else {
            w.write_flag(false);
            return Ok(());
        };
        let class_bits = classes.id_bits(Kind::DataBlock);
        let class_id = body
            .record
            .class_id()
            .or_else(|| classes.id_of(Kind::DataBlock, body.class))
            .filter(|&id| u64::from(id) < 1u64 << class_bits)
            .ok_or(DbPackError::UnknownClass(body.class))?;
        w.write_flag(true);
        w.write_ranged_u32(
            body.id,
            DATABLOCK_ID_FIRST,
            DATABLOCK_ID_FIRST + id_mask(self.id_bits),
        );
        w.write_bits(class_id, class_bits);
        w.write_bits(body.index, self.id_bits);
        w.write_bits(body.total, self.id_bits + 1);
        body.record.encode(w, table)
    }

    /// The bits the event takes on its own in a fresh packet with a string buffer, or [`DbPackError::TooBig`] when they are
    /// above `limit_bits`.
    pub fn measure(
        &self,
        table: &Table,
        classes: &NetClasses,
        limit_bits: usize,
    ) -> Result<usize, DbPackError> {
        let mut w = BitWriter::new();
        w.enable_string_buffer();
        self.pack(&mut w, table, classes)?;
        let bits = w.bits();
        if bits > limit_bits {
            return Err(DbPackError::TooBig {
                bits,
                limit: limit_bits,
            });
        }
        Ok(bits)
    }
}
