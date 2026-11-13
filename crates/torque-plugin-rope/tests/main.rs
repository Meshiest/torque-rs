//! The integration tests, linked as one test program: one module per file.

#[path = "pictures/common.rs"]
mod common;
#[path = "net/rig.rs"]
mod rig;

mod driver;
mod hierarchy_anchor;
mod muzzle;
mod net;
mod net_driver;
mod net_events;
mod net_hash;
mod net_pictures;
mod picture;
mod script;
mod sim;
mod strip;
mod tge_client;
