//! The codec the ghost records are written against: ONE layout function per record, run by an encoder, a decoder, or (in the
//! tests) the encoder's logging form.
//!
//! A record's layout is a list of calls on a [`Codec`] in wire order: `c.flag("hasScale", &mut r.has_scale)?`,
//! `c.int("damageState", &mut r.state, 2)?`, ... The [`Enc`] reads the record's field and writes it; the [`Dec`] reads the stream
//! and stores it in the field. A conditional block is `if c.flag(..) { ... }` on a field the encoder takes from the record and the
//! decoder from the stream, so the two directions cannot disagree about the order, the widths or which block is present: what the
//! tests pin down is the layout itself (a hand-written list of `(field, width)` per record from its `packUpdate`, run through
//! [`Enc::logging`]), the bit totals, and the quantisers' own numbers (in the `pack` functions).
//!
//! **Hostile input.** The decoder reads from a [`BitReader`] and returns [`NetError`] on overrun or on a value the format forbids;
//! every wire count goes through [`torque_net::cap`] against the bits left and a named cap; it never panics. The encoder is total: any
//! record gives a stream (values are masked to their width, counts are cut to what their field holds).
//!
//! Strings use the packet's string context (`torque_net::strcoder`): the writer and the reader must have had
//! `enable_string_buffer` called, as a game packet's streams do, or the plain form is used.
//!
//! **Widths and the client's facts.** Every id width comes from the connection's [`NetWidths`] (the ghost index and the datablock
//! id), never from a literal. A decoder that needs to know something about the client's own datablocks or objects asks the
//! [`ClientFacts`] it was given: the engine's questions are methods (a WheeledVehicleData's wheel count), a game's own questions
//! travel through [`ClientFacts::answer`] as a value of the game's type.
//!
//! Credit: the field forms follow TGE 1.4's `core/bitStream.cc` and `sim/netConnection.cc` (`packStringHandleU`), carried into
//! the MIT-licensed Torque3D as `core/stream/bitStream.cpp` and `sim/netConnection.cpp` (each Copyright (c) 2012 GarageGames,
//! LLC); the code here is this crate's own.

use std::any::Any;

use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter, bit_length};
use torque_net::cap;
use torque_net::huffman::Table;
use torque_net::strcoder::{read_string, write_string};

use crate::{ChargeFn, pay};

/// What a codec call returns.
pub type R = Result<(), NetError>;

/// The first datablock id (`DataBlockObjectIdFirst`): a datablock reference travels as `id - 3` in the connection's datablock-id
/// width.
pub const DATABLOCK_ID_FIRST: u32 = torque_net::gameevents::DATABLOCK_ID_FIRST;

/// The id widths of a connection: the ghost index (the accept's, or the width of the ghost limit) and the datablock id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NetWidths {
    pub ghost_bits: u8,
    pub datablock_bits: u8,
}

impl NetWidths {
    pub const fn new(ghost_bits: u8, datablock_bits: u8) -> NetWidths {
        NetWidths {
            ghost_bits,
            datablock_bits,
        }
    }

    /// torque-net's defaults when a game sets nothing: the width of [`torque_net::profile::DEFAULT_GHOST_LIMIT`] and
    /// [`torque_net::profile::DEFAULT_DATABLOCK_BITS`].
    pub fn engine_default() -> NetWidths {
        NetWidths::new(
            torque_net::classes::ghost_id_bits(torque_net::profile::DEFAULT_GHOST_LIMIT),
            torque_net::profile::DEFAULT_DATABLOCK_BITS,
        )
    }

    /// The range a datablock reference travels in, `ranged(id, lo, hi)`: `lo` is [`DATABLOCK_ID_FIRST`] and `hi - lo` fills
    /// `datablock_bits` bits.
    pub fn datablock_range(self) -> (u32, u32) {
        let lo = DATABLOCK_ID_FIRST;
        (
            lo,
            lo.wrapping_add(mask_bits(u32::MAX, self.datablock_bits)),
        )
    }

    /// The wire id of a datablock pointer, as the reader gets it back: `None` (a null pointer the engine would dereference) goes
    /// out as the first id.
    pub fn wire_datablock(self, id: Option<u32>) -> u32 {
        let (lo, hi) = self.datablock_range();
        wrap_ranged(id.unwrap_or(lo), lo, hi)
    }
}

/// What a decoder may ask about the client's own state: its datablocks (by id) and objects. The answers are the client's, so a
/// record whose layout depends on them can be read; a question with no answer makes the record an error.
pub trait ClientFacts {
    /// The wheel count of the WheeledVehicleData `datablock` (`None`: not one the client knows).
    fn wheel_count(&self, datablock: u32) -> Option<usize> {
        let _ = datablock;
        None
    }

    /// A game's own question: `question` is a value of the game's question type, which the game's facts recognise by downcasting
    /// and fill in. A question the facts do not know is left as it was (unanswered).
    fn answer(&self, question: &mut dyn Any) {
        let _ = question;
    }
}

/// A raw float as it travels: its 32 bits (so a NaN compares equal to itself and a round trip is bit exact).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct F32(pub u32);

impl F32 {
    pub fn new(x: f32) -> F32 {
        F32(x.to_bits())
    }

    pub fn get(self) -> f32 {
        f32::from_bits(self.0)
    }
}

/// A transform as `writeAffineTransform` sends it: the position, the quaternion's x, y, z (raw floats) and the flag
/// `w < 0`. The default is the identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Xf {
    pub position: [F32; 3],
    pub quat: [F32; 3],
    pub negative_w: bool,
}

impl Xf {
    pub fn from_affine(a: &torque_net::bits::Affine) -> Xf {
        Xf {
            position: [
                F32::new(a.position[0]),
                F32::new(a.position[1]),
                F32::new(a.position[2]),
            ],
            quat: [
                F32::new(a.quaternion[0]),
                F32::new(a.quaternion[1]),
                F32::new(a.quaternion[2]),
            ],
            negative_w: a.quaternion[3] < 0.0,
        }
    }
}

/// A net string as `NetConnection::packStringHandleU` (Torque3D: `packNetStringHandleU`) sends it: null, a slot of the
/// connection's string table (`ConnectionStringTable::EntryBitSize`, 5 bits), or the text itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum NetStr {
    #[default]
    Null,
    Slot(u8),
    Text(Vec<u8>),
}

/// The calls a record's layout makes. See the module text; `name` is the field's name (the logging encoder records it).
pub trait Codec: Sized {
    /// One bit.
    fn flag(&mut self, name: &'static str, v: &mut bool) -> R;
    /// `int(bits)`: the low `bits` bits (at most 32).
    fn int(&mut self, name: &'static str, v: &mut u32, bits: u8) -> R;
    /// A raw byte (8 bits, not a flag).
    fn raw8(&mut self, name: &'static str, v: &mut u8) -> R;
    /// A raw 32-bit word (a float's bits or an `i32`).
    fn raw32(&mut self, name: &'static str, v: &mut u32) -> R;
    /// `N` raw floats.
    fn floats<const N: usize>(&mut self, name: &'static str, v: &mut [F32; N]) -> R;
    /// A transform, 193 bits.
    fn xform(&mut self, name: &'static str, v: &mut Xf) -> R;
    /// `string(255)` through the packet's string context.
    fn string(&mut self, name: &'static str, v: &mut Vec<u8>) -> R;
    /// `ranged(v, lo, hi)`: `int(bitlen(hi - lo), v - lo)`; the record holds `v` (wrapped into the field's width on the way out).
    fn ranged(&mut self, name: &'static str, v: &mut u32, lo: u32, hi: u32) -> R;
    /// A count of `bits` bits followed by that many elements, each made by `each` (at least `min_bits_each` bits). The decoder
    /// refuses a count above `max` (a named cap) or above what the bits left could hold; the encoder cuts the vector to `max`.
    fn seq<T: Default>(
        &mut self,
        name: &'static str,
        v: &mut Vec<T>,
        bits: u8,
        max: usize,
        min_bits_each: usize,
        each: impl FnMut(&mut Self, &mut T) -> R,
    ) -> R;

    /// The id widths of the connection.
    fn widths(&self) -> NetWidths;
    /// The ghost-index width of the connection.
    fn ghost_bits(&self) -> u8 {
        self.widths().ghost_bits
    }
    /// Whether this codec reads a stream (a decoder) rather than writes a record.
    fn decoding(&self) -> bool {
        false
    }
    /// Decoder only: the client's facts the decode was given (`None` for an encoder, or a decode given none).
    fn facts(&self) -> Option<&dyn ClientFacts> {
        None
    }
    /// Decoder only: the datablock the object has now (a later update carries none).
    fn object_datablock(&self) -> Option<u32> {
        None
    }
    /// Decoder only: whether this is a new object's first update (the unpack's "object not yet added"); the record stores it.
    fn initial_update(&self) -> Option<bool> {
        None
    }
    /// The stream's compression point: the reader's for a decoder, the writer's for an encoder (a layout whose position coding
    /// depends on it reads it here; an encoder can check that the record's coding agrees with its writer).
    fn compression_point(&self) -> Option<[f32; 3]> {
        None
    }
    /// Sets the stream's compression point (`BitStream::setCompressionPoint`), as a control object's packet data does: the
    /// writer's for an encoder, the reader's for a decoder. It writes nothing.
    fn set_compression_point(&mut self, point: [f32; 3]) {
        let _ = point;
    }
    /// Decoder only: whether the object being read is this connection's control object on the client (its controlling
    /// connection is this one); a layout may leave out what the controlling client predicts itself ([`ObjCtx::controlled_here`]).
    fn controlled_here(&self) -> Option<bool> {
        None
    }
    /// Decoder only: the wheel count of the WheeledVehicleData `id` (or of the object's current datablock when `id` is `None`),
    /// from the client's facts and checked against [`WIRE_WHEELS`]. The encoder answers `None` (it uses the record's).
    fn wheel_count(&mut self, id: Option<u32>) -> Result<Option<usize>, NetError> {
        if !self.decoding() {
            return Ok(None);
        }
        let id = id
            .or(self.object_datablock())
            .ok_or(NetError::Invalid("object without a datablock"))?;
        let facts = self
            .facts()
            .ok_or(NetError::Invalid("vehicle datablock table"))?;
        let n = facts
            .wheel_count(id)
            .ok_or(NetError::Invalid("unknown wheeled vehicle datablock"))?;
        let n = cap::count("wheels", n as u64, WIRE_WHEELS, usize::MAX, 0)?;
        Ok(Some(n))
    }

    /// A net string: `flag(non-null)`, then `flag(mapped)` and `int(5)` or the text.
    fn net_string(&mut self, v: &mut NetStr) -> R {
        let mut present = !matches!(v, NetStr::Null);
        self.flag("ns.present", &mut present)?;
        if !present {
            *v = NetStr::Null;
            return Ok(());
        }
        let mut mapped = matches!(v, NetStr::Slot(_));
        self.flag("ns.mapped", &mut mapped)?;
        if mapped {
            let mut slot = match v {
                NetStr::Slot(s) => u32::from(*s),
                _ => 0,
            };
            self.int("ns.slot", &mut slot, 5)?;
            *v = NetStr::Slot(slot as u8);
        } else {
            let mut text = match std::mem::take(v) {
                NetStr::Text(t) => t,
                _ => Vec::new(),
            };
            self.string("ns.text", &mut text)?;
            *v = NetStr::Text(text);
        }
        Ok(())
    }
}

/// `flag(present)` and, when set, the block. The decoder creates the block's default and fills it.
pub fn opt<C: Codec, T: Default>(
    c: &mut C,
    name: &'static str,
    o: &mut Option<T>,
    f: impl FnOnce(&mut C, &mut T) -> R,
) -> R {
    let mut present = o.is_some();
    c.flag(name, &mut present)?;
    if present {
        let inner = o.get_or_insert_with(T::default);
        f(c, inner)
    } else {
        *o = None;
        Ok(())
    }
}

/// The most wheels of a WheeledVehicle (`WheeledVehicleData::MaxWheels`).
pub const WIRE_WHEELS: usize = 8;

/// A datablock reference, `ranged(id, lo, hi)` over the connection's datablock-id range ([`NetWidths::datablock_range`]).
pub fn datablock_id<C: Codec>(c: &mut C, name: &'static str, id: &mut u32) -> R {
    let (lo, hi) = c.widths().datablock_range();
    c.ranged(name, id, lo, hi)
}

/// A ghost index sent as `writeRangedU32(index, 0, MaxGhostCount)`: `MaxGhostCount` is `1 << ghost bits`, so the field is one
/// bit wider than the ghost index.
pub fn ghost_ranged<C: Codec>(c: &mut C, name: &'static str, index: &mut u32) -> R {
    let hi = 1u32
        .checked_shl(u32::from(c.ghost_bits()))
        .unwrap_or(u32::MAX);
    c.ranged(name, index, 0, hi)
}

/// A link to another ghost: `flag(has)`, then `flag(ghosted)` and `int(GB)`. `None` is no object, `Some(None)` an object that has no
/// ghost index yet (the packer keeps a bit in its return mask for it), `Some(Some(i))` the index.
pub fn link<C: Codec>(c: &mut C, name: &'static str, l: &mut Option<Option<u32>>) -> R {
    let bits = c.ghost_bits();
    opt(c, name, l, |c, inner| {
        let mut ok = inner.is_some();
        c.flag("link.ghosted", &mut ok)?;
        if ok {
            let mut idx = inner.unwrap_or(0);
            c.int("link.index", &mut idx, bits)?;
            *inner = Some(mask_bits(idx, bits));
        } else {
            *inner = None;
        }
        Ok(())
    })
}

/// `value` cut to its low `bits` bits.
pub fn mask_bits(value: u32, bits: u8) -> u32 {
    if bits >= 32 {
        value
    } else {
        value & ((1u32 << bits) - 1)
    }
}

/// `ranged`'s range check: the value as it comes back from the wire, `lo + ((v - lo) & mask)`.
pub fn wrap_ranged(v: u32, lo: u32, hi: u32) -> u32 {
    lo.wrapping_add(mask_bits(
        v.wrapping_sub(lo),
        bit_length(hi.wrapping_sub(lo)),
    ))
}

// The encoder

/// Writes a record. Total: nothing fails except a string (never) and the writer's own capacity (it marks itself overflowed).
pub struct Enc<'a> {
    w: &'a mut BitWriter,
    table: &'a Table,
    widths: NetWidths,
    log: Option<Vec<(&'static str, usize)>>,
}

impl<'a> Enc<'a> {
    pub fn new(w: &'a mut BitWriter, table: &'a Table, widths: NetWidths) -> Enc<'a> {
        Enc {
            w,
            table,
            widths,
            log: None,
        }
    }

    /// An encoder that also records `(field, bits written)` for every call (a string counts the bits its coding took).
    pub fn logging(w: &'a mut BitWriter, table: &'a Table, widths: NetWidths) -> Enc<'a> {
        Enc {
            log: Some(Vec::new()),
            ..Enc::new(w, table, widths)
        }
    }

    /// The log of a logging encoder.
    pub fn into_log(self) -> Vec<(&'static str, usize)> {
        self.log.unwrap_or_default()
    }

    fn note(&mut self, name: &'static str, before: usize) {
        let now = self.w.bits();
        if let Some(log) = &mut self.log {
            log.push((name, now.saturating_sub(before)));
        }
    }
}

impl Codec for Enc<'_> {
    fn flag(&mut self, name: &'static str, v: &mut bool) -> R {
        let b = self.w.bits();
        self.w.write_flag(*v);
        self.note(name, b);
        Ok(())
    }

    fn int(&mut self, name: &'static str, v: &mut u32, bits: u8) -> R {
        let b = self.w.bits();
        self.w.write_bits(*v, bits);
        self.note(name, b);
        Ok(())
    }

    fn raw8(&mut self, name: &'static str, v: &mut u8) -> R {
        let b = self.w.bits();
        self.w.write_u8(*v);
        self.note(name, b);
        Ok(())
    }

    fn raw32(&mut self, name: &'static str, v: &mut u32) -> R {
        let b = self.w.bits();
        self.w.write_u32(*v);
        self.note(name, b);
        Ok(())
    }

    fn floats<const N: usize>(&mut self, name: &'static str, v: &mut [F32; N]) -> R {
        let b = self.w.bits();
        for f in v.iter() {
            self.w.write_u32(f.0);
        }
        self.note(name, b);
        Ok(())
    }

    fn xform(&mut self, name: &'static str, v: &mut Xf) -> R {
        let b = self.w.bits();
        for f in v.position.iter().chain(v.quat.iter()) {
            self.w.write_u32(f.0);
        }
        self.w.write_flag(v.negative_w);
        self.note(name, b);
        Ok(())
    }

    fn string(&mut self, name: &'static str, v: &mut Vec<u8>) -> R {
        let b = self.w.bits();
        write_string(self.w, self.table, v, cap::NET_STRING_BYTES);
        self.note(name, b);
        Ok(())
    }

    fn ranged(&mut self, name: &'static str, v: &mut u32, lo: u32, hi: u32) -> R {
        let b = self.w.bits();
        self.w.write_ranged_u32(*v, lo, hi);
        *v = wrap_ranged(*v, lo, hi);
        self.note(name, b);
        Ok(())
    }

    fn seq<T: Default>(
        &mut self,
        name: &'static str,
        v: &mut Vec<T>,
        bits: u8,
        max: usize,
        _min_bits_each: usize,
        mut each: impl FnMut(&mut Self, &mut T) -> R,
    ) -> R {
        let limit = max.min(mask_bits(u32::MAX, bits) as usize);
        v.truncate(limit);
        let b = self.w.bits();
        self.w.write_bits(v.len() as u32, bits);
        self.note(name, b);
        for item in v.iter_mut() {
            each(self, item)?;
        }
        Ok(())
    }

    fn widths(&self) -> NetWidths {
        self.widths
    }

    fn compression_point(&self) -> Option<[f32; 3]> {
        Some(self.w.compression_point())
    }

    fn set_compression_point(&mut self, point: [f32; 3]) {
        self.w.set_compression_point(point);
    }
}

// The decoder

/// What a decode of an object's record needs to know besides the bytes. The default is an object that is not the control object
/// and has no datablock yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ObjCtx {
    /// The object is this connection's control object on the client (the server's control-object section named it).
    pub controlled_here: bool,
    /// The datablock the object has now (a later update carries none); a WheeledVehicle's wheel count comes from it.
    pub datablock: Option<u32>,
}

/// The context of one record's decode.
#[derive(Clone, Copy)]
pub struct ReadCtx<'a> {
    /// The connection's id widths.
    pub widths: NetWidths,
    /// Whether this record is the first update of a new object (the unpack's "not yet added" test).
    pub initial: bool,
    /// The client's facts the layout may ask about (`None`: a record that asks fails).
    pub facts: Option<&'a dyn ClientFacts>,
    /// The object the record is for.
    pub object: ObjCtx,
}

impl ReadCtx<'_> {
    /// A context with no facts, for an object with no datablock that is not the control object.
    pub fn new(widths: NetWidths, initial: bool) -> ReadCtx<'static> {
        ReadCtx {
            widths,
            initial,
            facts: None,
            object: ObjCtx::default(),
        }
    }
}

/// Reads a record from hostile bytes.
pub struct Dec<'a, 'b, 'c> {
    r: &'a mut BitReader<'b>,
    table: &'a Table,
    charge: &'a mut ChargeFn<'c>,
    ctx: &'a ReadCtx<'a>,
}

impl<'a, 'b, 'c> Dec<'a, 'b, 'c> {
    pub fn new(
        r: &'a mut BitReader<'b>,
        table: &'a Table,
        charge: &'a mut ChargeFn<'c>,
        ctx: &'a ReadCtx<'a>,
    ) -> Dec<'a, 'b, 'c> {
        Dec {
            r,
            table,
            charge,
            ctx,
        }
    }
}

impl Codec for Dec<'_, '_, '_> {
    fn flag(&mut self, _name: &'static str, v: &mut bool) -> R {
        *v = self.r.read_flag()?;
        Ok(())
    }

    fn int(&mut self, _name: &'static str, v: &mut u32, bits: u8) -> R {
        *v = self.r.read_bits(bits)?;
        Ok(())
    }

    fn raw8(&mut self, _name: &'static str, v: &mut u8) -> R {
        *v = self.r.read_u8()?;
        Ok(())
    }

    fn raw32(&mut self, _name: &'static str, v: &mut u32) -> R {
        *v = self.r.read_u32()?;
        Ok(())
    }

    fn floats<const N: usize>(&mut self, _name: &'static str, v: &mut [F32; N]) -> R {
        for f in v.iter_mut() {
            f.0 = self.r.read_u32()?;
        }
        Ok(())
    }

    fn xform(&mut self, _name: &'static str, v: &mut Xf) -> R {
        for f in v.position.iter_mut().chain(v.quat.iter_mut()) {
            f.0 = self.r.read_u32()?;
        }
        v.negative_w = self.r.read_flag()?;
        Ok(())
    }

    fn string(&mut self, _name: &'static str, v: &mut Vec<u8>) -> R {
        pay(self.charge, 1)?;
        *v = read_string(self.r, self.table)?;
        pay(self.charge, (v.len() as u64) / 8)?;
        Ok(())
    }

    fn ranged(&mut self, _name: &'static str, v: &mut u32, lo: u32, hi: u32) -> R {
        *v = self.r.read_ranged_u32(lo, hi)?;
        Ok(())
    }

    fn seq<T: Default>(
        &mut self,
        name: &'static str,
        v: &mut Vec<T>,
        bits: u8,
        max: usize,
        min_bits_each: usize,
        mut each: impl FnMut(&mut Self, &mut T) -> R,
    ) -> R {
        let n = u64::from(self.r.read_bits(bits)?);
        let n = cap::count(name, n, max, self.r.bits_left(), min_bits_each)?;
        v.clear();
        v.reserve(n);
        for _ in 0..n {
            pay(self.charge, 1)?;
            let mut item = T::default();
            each(self, &mut item)?;
            v.push(item);
        }
        Ok(())
    }

    fn widths(&self) -> NetWidths {
        self.ctx.widths
    }

    fn decoding(&self) -> bool {
        true
    }

    fn facts(&self) -> Option<&dyn ClientFacts> {
        self.ctx.facts
    }

    fn object_datablock(&self) -> Option<u32> {
        self.ctx.object.datablock
    }

    fn initial_update(&self) -> Option<bool> {
        Some(self.ctx.initial)
    }

    fn compression_point(&self) -> Option<[f32; 3]> {
        Some(self.r.compression_point())
    }

    fn set_compression_point(&mut self, point: [f32; 3]) {
        self.r.set_compression_point(point);
    }

    fn controlled_here(&self) -> Option<bool> {
        Some(self.ctx.object.controlled_here)
    }
}

/// Anything with a layout: the record types.
pub trait Layout: Default + Clone {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R;
}

/// Writes `rec` to `w` (cloning it: the layout takes `&mut`). The stream's strings use `w`'s string buffer if it has one.
pub fn encode<L: Layout>(rec: &L, w: &mut BitWriter, table: &Table, widths: NetWidths) -> R {
    let mut copy = rec.clone();
    let mut c = Enc::new(w, table, widths);
    copy.layout(&mut c)
}

/// Reads one record.
pub fn decode<L: Layout>(
    r: &mut BitReader,
    table: &Table,
    charge: &mut ChargeFn,
    ctx: &ReadCtx,
) -> Result<L, NetError> {
    decode_into(L::default(), r, table, charge, ctx)
}

/// Reads one record into `rec` (a record whose layout depends on what it already holds, such as its class, starts from it).
pub fn decode_into<L: Layout>(
    mut rec: L,
    r: &mut BitReader,
    table: &Table,
    charge: &mut ChargeFn,
    ctx: &ReadCtx,
) -> Result<L, NetError> {
    // The record's base unit is paid before anything is read, so a spent budget stops the decode at once; the rest (one unit per
    // byte read, strings and elements as they come) follows.
    pay(charge, 1)?;
    let start = r.position();
    {
        let mut c = Dec::new(r, table, charge, ctx);
        rec.layout(&mut c)?;
    }
    let bytes = r.position().saturating_sub(start).div_ceil(8) as u64;
    pay(charge, bytes.saturating_sub(1))?;
    Ok(rec)
}
