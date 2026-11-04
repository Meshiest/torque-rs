//! Datablocks: the record a `SimDataBlockEvent` carries for one datablock (its class's `packData`), the registry of a game's
//! datablock classes ([`DatablockCodecSet`]) and the engine's own lists, TGE 1.4's ([`TgeDatablocks`]).
//!
//! A datablock's record is laid out like a ghost's: ONE layout function per class on a [`Codec`], run by the encoder, the decoder
//! (hostile input: never a panic, every count capped) or the logging encoder. A class that extends another starts with its
//! parent's part (`ShapeBaseData`, `VehicleData`); `SimDataBlock` and `GameBaseData` send nothing.
//!
//! **Records hold wire values**: a raw float's bits ([`F32`]), the integer a quantiser writes, a datablock reference after its
//! wrap into the connection's datablock-id range, a flag for each conditional block (`Option`). So `decode(encode(r)) == r`, and
//! `encode(decode(bits)) == bits` for every stream whose strings are spelt the canonical way.
//!
//! **The host's view.** A packer reads the datablock through an [`ObjectView`] (only `get` is used): each field by the name of
//! the class's member that `packData` writes (`mass`, `cameraDefaultFov`, `mCRC`, ...), an array member's element `i` at index
//! `i`, a `Point3F`'s components at `3 * i + c`, a `ColorF`'s at `4 * i + c`, a member of a nested struct as `body.friction`
//! or `state.fire`. Pointers to other datablocks come back as [`Value::Id`]. The values a host computes when
//! it loads the datablock (a shape's CRC, the image states' transition indices and sequence ids, an emitter's particle ids)
//! are fields like the others. A packer never fails: a missing value is 0, false, "" or null.
//!
//! **Caps** (each wire count is also checked against the bits left): [`MAX_PARTICLE_TEXTURES`], [`MAX_EMITTER_PARTICLES`],
//! [`MAX_EXPLOSION_KEYS`], [`MAX_SHAPE_SEQUENCES`].
//!
//! Credit: a datablock reference follows TGE 1.4's `PACK_DB_ID` in `game/gameBase.cc` (Copyright (C) GarageGames.com, Inc.),
//! whose MIT-licensed successor is Torque3D's `T3D/gameBase/gameBase.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C)
//! 2015 Faust Logic, Inc.); the empty `SimDataBlock::packData` is TGE's `console/simBase.cc` (Copyright (C) GarageGames.com,
//! Inc.), in Torque3D `console/simDatablock.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.).
//! The code here is this crate's own.

pub mod audio;
pub mod effects;
pub mod image;
pub mod misc;
pub mod shapes;
pub mod tge;
pub mod vehicles;
pub mod weather;

use std::fmt::Debug;

use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};
use torque_net::huffman::Table;
use torque_net::library;

use crate::codec::{Codec, F32, NetWidths, R, datablock_id, opt};
use crate::view::{boolean, fld, id};
use crate::{ChargeFn, ObjectView, Value};

pub use tge::{ScriptField, TgeDatablockClass, TgeDatablockRecord, TgeDatablocks};

/// The most texture names a `ParticleData` sends (`PDC_MAX_TEX`).
pub const MAX_PARTICLE_TEXTURES: usize = 50;

/// The most particle datablock ids a `ParticleEmitterData` record may carry: this crate's own safety cap (TGE has none and
/// sends its whole list behind a 32-bit count). A longer list is refused on read, and the packer sends only the first ones.
pub const MAX_EMITTER_PARTICLES: usize = 64;

/// The most size/time keys an `ExplosionData` sends (`EC_NUM_TIME_KEYS`).
pub const MAX_EXPLOSION_KEYS: usize = 4;

/// The most sequences a `TSShapeConstructor` sends (`MaxSequences`, the largest 7-bit count).
pub const MAX_SHAPE_SEQUENCES: usize = 127;

/// A set of datablock classes and their record codecs.
pub trait DatablockCodecSet {
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

    /// The class of a record.
    fn class_of(record: &Self::Record) -> Self::Class;

    /// Packs a datablock of `class` from the host's view (its `packData`).
    fn pack(class: Self::Class, view: &dyn ObjectView, widths: NetWidths) -> Self::Record;

    /// Reads one record of `class` from hostile bytes (its `unpackData`).
    fn decode(
        class: Self::Class,
        r: &mut BitReader,
        table: &Table,
        charge: &mut ChargeFn,
        widths: NetWidths,
    ) -> Result<Self::Record, NetError>;

    /// Writes a record in the connection's `widths`.
    fn encode(record: &Self::Record, w: &mut BitWriter, table: &Table, widths: NetWidths) -> R;
}

// The layout pieces

/// `write(F32)`: a raw float.
pub fn float<C: Codec>(c: &mut C, name: &'static str, v: &mut F32) -> R {
    c.raw32(name, &mut v.0)
}

/// `writeFlag(x != default)` and, when set, `write(F32)`.
pub fn opt_float<C: Codec>(c: &mut C, name: &'static str, v: &mut Option<F32>) -> R {
    opt(c, name, v, |c, x| float(c, name, x))
}

/// A datablock pointer as `packData` sends it: `flag(non-null)`, then the id `ranged` over the connection's datablock-id range.
pub fn db_ref<C: Codec>(c: &mut C, name: &'static str, v: &mut Option<u32>) -> R {
    opt(c, name, v, |c, id| datablock_id(c, name, id))
}

/// An array of datablock pointers, one [`db_ref`] each.
pub fn db_refs<C: Codec, const N: usize>(
    c: &mut C,
    name: &'static str,
    v: &mut [Option<u32>; N],
) -> R {
    v.iter_mut().try_for_each(|x| db_ref(c, name, x))
}

/// An array of strings, one `writeString` each.
pub fn strings<C: Codec, const N: usize>(c: &mut C, name: &'static str, v: &mut [Vec<u8>; N]) -> R {
    v.iter_mut().try_for_each(|s| c.string(name, s))
}

/// `write(ColorF)`: the colour converted to `ColorI`, four bytes.
pub fn color_i<C: Codec>(c: &mut C, name: &'static str, v: &mut [u8; 4]) -> R {
    v.iter_mut().try_for_each(|b| c.raw8(name, b))
}

/// `writeFloat(f, bits)` of each channel.
pub fn float_ints<C: Codec, const N: usize>(
    c: &mut C,
    name: &'static str,
    v: &mut [u32; N],
    bits: u8,
) -> R {
    v.iter_mut().try_for_each(|x| c.int(name, x, bits))
}

// The packers' pieces

/// A datablock pointer field as it travels: the id wrapped into the connection's range, `None` for null.
pub fn db(view: &dyn ObjectView, name: &str, i: usize, widths: NetWidths) -> Option<u32> {
    id(view, name, i).map(|id| widths.wire_datablock(Some(id)))
}

/// `N` datablock pointers, elements `0..N`.
pub fn dbs<const N: usize>(
    view: &dyn ObjectView,
    name: &str,
    widths: NetWidths,
) -> [Option<u32>; N] {
    std::array::from_fn(|i| db(view, name, i, widths))
}

/// `write(bool)`: one byte, 1 or 0.
pub fn byte(view: &dyn ObjectView, name: &str, i: usize) -> u8 {
    u8::from(boolean(view, name, i))
}

/// A `ColorF` field (element `i`) as `write(ColorF)` sends it.
pub fn color_bytes(view: &dyn ObjectView, name: &str, i: usize) -> [u8; 4] {
    std::array::from_fn(|k| (library::get().color_byte)(fld(view, name, 4 * i + k)))
}

/// `writeFloat(f, bits)` of the components `base..base + N` of a field.
pub fn float_ints_of<const N: usize>(
    view: &dyn ObjectView,
    name: &str,
    base: usize,
    bits: u8,
) -> [u32; N] {
    std::array::from_fn(|k| crate::motion::float_bits(fld(view, name, base + k), bits))
}

/// The text of `N` string fields, elements `0..N`.
pub fn texts<const N: usize>(view: &dyn ObjectView, name: &str) -> [Vec<u8>; N] {
    std::array::from_fn(|i| crate::view::text(view, name, i))
}

/// A float that is sent only when it differs from its default (a float compare: a NaN differs).
pub fn unless(value: f32, default: f32) -> Option<F32> {
    (value != default).then_some(F32::new(value))
}

/// A float converted as the source's `(S32)` cast does, as the low 32 bits a `(U32)` cast or `writeInt` keeps.
pub fn cast(f: f32) -> u32 {
    (library::get().to_int)(f) as u32
}

/// Whether the view has a value for element `i` of a field (a list runs until its first missing element).
pub fn present(view: &dyn ObjectView, name: &str, i: usize) -> bool {
    view.get(name, i) != Value::Null
}

/// `mClampF(val, low, high)`: `getMax(getMin(val, high), low)` with TGE's `a > b` compares (a NaN gives `low`).
pub fn clamp_f(val: f32, low: f32, high: f32) -> f32 {
    let m = if val > high { high } else { val };
    if m > low { m } else { low }
}
