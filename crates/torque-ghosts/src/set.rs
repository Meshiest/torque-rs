//! The class registry of a game's ghost records: a [`GhostCodecSet`] names the object classes it has a record codec for (by
//! their engine class names, the names the net class table sorts into ids) and packs, reads and writes their records. A
//! client's codec maps the class ids of the connection's class table (derived by `torque_net::classes::NetClasses`) to the
//! set's classes by name; a class the set does not have stays unsupported, by name.

use std::fmt::Debug;

use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};
use torque_net::huffman::Table;

use crate::ChargeFn;
use crate::codec::{NetWidths, R, ReadCtx};
use crate::{ObjectView, PackCtx};

/// A set of ghost classes and their record codecs.
pub trait GhostCodecSet {
    /// A class of the set.
    type Class: Copy + Eq + Debug + 'static;
    /// A record of any class of the set.
    type Record: Clone + Debug + PartialEq;

    /// Every class the set has a record codec for.
    fn classes() -> &'static [Self::Class];

    /// The engine's class name of `class` (the net class table's).
    fn name(class: Self::Class) -> &'static str;

    /// The class a net class table's name stands for (`None`: the set has no record codec for it).
    fn class_by_name(name: &str) -> Option<Self::Class> {
        Self::classes()
            .iter()
            .copied()
            .find(|&c| Self::name(c) == name)
    }

    /// The class of an object id past the end of a table of `table_len` object classes (a set's own extension; `None` by default).
    fn class_past_table(id: u32, table_len: usize) -> Option<Self::Class> {
        let _ = (id, table_len);
        None
    }

    /// The class a new-ghost record without a class id stands for (the ghost policy's implied form), if the set has one.
    fn implied() -> Option<Self::Class> {
        None
    }

    /// The class of a record.
    fn class_of(record: &Self::Record) -> Self::Class;

    /// Packs an object of `class` as one connection sees it: the record and the packer's return mask.
    fn pack(
        class: Self::Class,
        view: &dyn ObjectView,
        mask: u32,
        ctx: &PackCtx,
    ) -> (Self::Record, u32);

    /// Reads one record of `class` from hostile bytes.
    fn decode(
        class: Self::Class,
        r: &mut BitReader,
        table: &Table,
        charge: &mut ChargeFn,
        ctx: &ReadCtx,
    ) -> Result<Self::Record, NetError>;

    /// Writes a record in the connection's `widths`.
    fn encode(record: &Self::Record, w: &mut BitWriter, table: &Table, widths: NetWidths) -> R;
}
