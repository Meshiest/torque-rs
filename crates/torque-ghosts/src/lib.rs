//! The engine's networked-object layer: how a ghost's record is laid out, packed, written and read.
//!
//! - [`codec`]: the layout DSL. A record's layout is ONE function of calls on a [`codec::Codec`] in wire order, run by the encoder,
//!   the decoder (hostile input: never a panic, every count capped) or the logging encoder (the `(field, width)` list a test
//!   pins). The connection's id widths ([`NetWidths`]) and the client's facts a decoder may ask ([`ClientFacts`]) live here.
//! - [`view`]: the host's [`ObjectView`] of one object as one connection sees it, the [`PackCtx`], and the coercions a packer
//!   reads values with.
//! - [`set`]: [`GhostCodecSet`], a game's registry of ghost classes and their codecs.
//! - [`parts`]: the GameBase part and the [`parts::ShapeBasePart`] seam.
//! - [`motion`]: the compressed point, the velocity block and signed floats as wire values.
//! - The engine's layouts (TGE 1.4's): [`shapebase`] (the ShapeBase part, [`TgeShapeBase`]), [`markers`] (bodies over any
//!   ShapeBase part), [`shapes`] (Camera, StaticShape, ScopeAlwaysShape, Item), [`player`] (Player, AIPlayer), [`vehicle`] (the
//!   Vehicle part, WheeledVehicle, FlyingVehicle, HoverVehicle), [`projectile`], [`props`] (TSStatic, VehicleBlocker), [`fx`]
//!   (ParticleEmitterNode, fxLight), [`zone`] (PhysicalZone), [`weather`] (Lightning, Precipitation), [`sky`] (Sky, Sun),
//!   [`audio`] (AudioEmitter); and the engine's own, after Torque3D's: [`light`] (SpotLight, TimeOfDay, ScatterSky).
//! - [`tails`]: the control objects' packet data and their class-directed reader.
//! - [`tge`]: [`TgeGhosts`], the engine's default [`GhostCodecSet`], and TGE's networked class list.
//! - [`plugin`]: the classes a plugin adds ([`PluginClass`], [`PluginClasses`]), their records ([`GhostRecord`],
//!   [`DatablockCodec`], carried as a [`Record`]) and the field types the derives build them of; the closed sets carry them
//!   in their `Plugin` arms.
//! - [`datablocks`]: [`DatablockCodecSet`], a game's registry of datablock classes, and [`TgeDatablocks`], TGE's datablock
//!   records (each class's `packData`).
//!
//! **Every decoder reads hostile input.** It takes a `torque_net::bits::BitReader` and a [`ChargeFn`], and returns a value or a
//! `torque_net::NetError`; it never panics, and it never allocates from a wire count without checking it against the bits left AND a
//! named cap through `torque_net::cap`.

pub mod audio;
pub mod codec;
pub mod datablocks;
pub mod fx;
pub mod interior;
pub mod light;
pub mod markers;
pub mod motion;
pub mod parts;
pub mod player;
pub mod plugin;
pub mod projectile;
pub mod props;
pub mod set;
pub mod shapebase;
pub mod shapes;
pub mod sky;
pub mod tails;
pub mod tge;
pub mod value;
pub mod vehicle;
pub mod view;
pub mod weather;
pub mod zone;

use torque_net::NetError;

pub use codec::{
    ClientFacts, F32, Layout, NetStr, NetWidths, ObjCtx, ReadCtx, WIRE_WHEELS, Xf, decode, encode,
};
pub use datablocks::{DatablockCodecSet, TgeDatablockClass, TgeDatablockRecord, TgeDatablocks};
pub use plugin::{
    DatablockCodec, DatablockRef, GhostRecord, NetRecord, PluginClass, PluginClasses, Record,
    Replicate,
};
pub use set::GhostCodecSet;
pub use shapebase::TgeShapeBase;
pub use tails::{TailClass, TgePacketData, TgeTails};
pub use tge::{TgeClass, TgeGhosts, TgeRecord, tge_reps};
pub use value::Value;
pub use view::{Link, ObjectView, PackCtx, boolean, fld, id, int, raw, raws, text, word, xform};

/// The work callback of a decoder: it is told how many units of work the decoder is about to do (one unit is one byte of wire input,
/// and at least one for an element that takes fewer) and answers whether the per-tick network work budget can pay for them. A
/// `false` stops the decode with an error.
pub type ChargeFn<'a> = dyn FnMut(u64) -> bool + 'a;

/// Pays `units` to `charge`, or fails when the budget cannot.
pub fn pay(charge: &mut ChargeFn, units: u64) -> Result<(), NetError> {
    if charge(units) {
        Ok(())
    } else {
        Err(NetError::Invalid("work budget"))
    }
}

/// A charge callback that pays for everything: for tests and for callers with no budget.
pub fn free_charge() -> impl FnMut(u64) -> bool {
    |_| true
}

#[cfg(test)]
mod tests;
