//! Test rigs for the plugins' net tests (the `testing` feature; only dev-dependencies turn it on): [`Rig`], a server and engine
//! clients in one process over the memory transport, and [`TgeOnlyClient`], a client that speaks only Torque's protocol.

mod rig;
mod tge_client;

pub use rig::{Link, Peer, Rig, RigPlugin, SERVER, engine, plain_engine};
pub use tge_client::{NetServer, TgeOnlyClient, connection_of, game_server, tge_eval, tge_run};
