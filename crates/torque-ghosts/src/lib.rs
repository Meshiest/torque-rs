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
//! - [`block`]: the per-ghost block the engine's own peers send beside a class record ([`block::GhostBlock`]: the scene
//!   hierarchy's link and local pose), and the mask bits it owns.
//! - [`tails`]: the control objects' packet data and their class-directed reader.
//! - `testing` (feature `testing`, for tests): `MapView`, an [`ObjectView`] over maps.
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
pub mod block;
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
#[cfg(any(test, feature = "testing"))]
pub mod testing;
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
    DatablockCodec, DatablockRef, GhostRecord, GhostRef, NetMessage, NetRecord, PluginClass,
    PluginClasses, Record, Replicate,
};
pub use set::GhostCodecSet;
pub use shapebase::TgeShapeBase;
pub use tails::{TailClass, TailFrame, TgePacketData, TgeTails};
pub use tge::{TgeClass, TgeGhosts, TgeRecord, tge_reps};
pub use value::Value;
pub use view::{Link, ObjectView, PackCtx, boolean, fld, id, int, raw, raws, text, word, xform};

pub use torque_bytes::{ChargeFn, budget_charge, cell_charge, free_charge};

/// Pays `units` to `charge`, or fails when the budget cannot.
pub fn pay(charge: &mut ChargeFn, units: u64) -> Result<(), NetError> {
    Ok(torque_bytes::pay(charge, units)?)
}

/// The work units a received datagram of `len` bytes may spend in a client's decoders: 8 a byte and a floor. An honest
/// record costs about a unit a byte; a string the shared-prefix form expands costs a unit per 8 bytes it expands to, so a
/// datagram of strings at their greatest expansion stops at this bound.
#[must_use]
pub fn datagram_budget(len: usize) -> u64 {
    u64::try_from(len)
        .unwrap_or(u64::MAX)
        .saturating_mul(8)
        .saturating_add(256)
}

#[cfg(test)]
mod tests;
