//! Ropes over the network: a server makes a rope, every client of the engine shows it.
//!
//! - [`server`]: the `Rope` console class and the `ropeNet*` functions a server script makes and changes ropes with.
//! - [`records`]: the ghost record ([`RopeGhost`]), the datablock record ([`RopeDataRecord`]) and the field types they are made of.
//! - [`client`]: what a client does with them ([`NetRopes`]).
//!
//! The server keeps what a rope was told and ghosts it; each client makes a rope of its own world from the ghost and simulates and
//! draws it. The rope's points never cross the network, and a rope never pulls on anything: a networked rope is as visual as a
//! local one.

pub mod client;
pub mod records;
pub mod server;

pub use client::{NetRopes, RopeNet};
pub use records::{
    AnchorWire, CurveWire, LengthWire, LifeWire, LookWire, PredictWire, RopeDataRecord, RopeGhost,
};

use torque_engine::{EngineBuilder, Replicate};

use crate::data::RopeData;
use crate::natives::RopeShared;

/// The network version of the rope plugin: a client and a server must run the same.
pub const NET_VERSION: u32 = 1;

/// The name of the ghost class a networked rope is.
pub const GHOST_CLASS: &str = "Rope";

// ANCHOR: register
/// Adds the rope's network surface to an engine being built: its ghost class and datablock class (so the manifest lists them),
/// the ghost handler of the clients, and the functions of both sides.
pub(crate) fn register(engine: &mut EngineBuilder, ropes: &RopeShared, net: &RopeNet) {
    engine
        .net()
        .ghost::<RopeGhost>(GHOST_CLASS, Replicate::Interpolated)
        .datablock::<RopeDataRecord>(<RopeData as torque_engine::ScriptClass>::NAME);
    engine.insert_resource(server::FxBudget::default());
    let (shared, book) = (ropes.clone(), net.clone());
    engine
        .net()
        .on_ghost::<RopeGhost>(GHOST_CLASS, move |vm, event| {
            client::heard(vm, &shared, &book, event);
        });
    engine
        .server_script()
        .class::<RopeData>()
        .class::<server::NetRope>()
        .natives(server::declared());
    engine
        .client_script()
        .class::<server::NetRope>()
        .natives(client::declared());
}
// ANCHOR_END: register
