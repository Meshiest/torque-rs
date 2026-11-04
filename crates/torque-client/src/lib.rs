//! The client's engine half: Torque's client parts, over which a game adds its own (its sign-in, its server lists, its
//! downloads, its UI).
//!
//! | module | part |
//! |---|---|
//! | [`actionmap`] | `ActionMap` and its natives (bindings, the key strings, `save`) |
//! | [`input`] | the platform input layer (window or injected messages into the canvas and the action maps) |
//! | [`screen`] | the screen mode natives (resolutions, full screen, the display device) |
//! | [`conn`] | the client's connection to a server (the handshake, the packets, the ticks) |
//! | [`query`] | the LAN server query (the pings and info requests of the server list) |
//! | [`net`] | the client's socket and the rules on what it may reach; the move each tick ([`net::moves`]) |
//! | [`host`] | the client host: a game's server host with the GUI, the client natives' state and the input layer, run tick by tick |
//! | [`live_perf`], [`perf`] | the client's work timed by part; a frame breakdown's rolling mean, lists and memory query |
//! | [`view`] | the per-frame driver of the world's view (when the scene is built again, the lines of sight between builds, the warm-up) |
//! | `platform` | the window glue (feature `window`: keys, cursor hold, frame cap, URL opener, the window's event loop) |
//! | [`ghost_objects`], [`datablocks`] | the ghosts and the datablocks as script objects |
//! | [`predict`] | the control object's prediction (on torque-scene's container) |
//! | [`eye`], [`zoom`], [`fps`], [`hud`], [`console_log`] | the eye and the zoom, the frame rate, the HUD feed, the console log file |
//! | [`game`] | what the engine half asks the game ([`game::ClientGame`]) |
//! | [`records`] | what the engine half reads of the game's records ([`records::EngineSet`]) |
//! | [`render`] | what the engine half asks the renderer (a ghost's shape posed as the frame draws it, the resets) |
//! | [`state`] | the client natives' state: the engine's parts and one slot for the game's ([`state::GameSlot`]) |
//!
//! The game's answers reach them through [`game::ClientGame`] (a `Vm` extension: the codecs the client speaks, the key tables, the
//! install's string coder, the class table's fields and the field rules, the renderer's resets, the datablock unpack and the
//! prediction's questions [`predict::game::PredictGame`]), which a game implements, and through the connection's game
//! half ([`conn::ConnGame`], the type the client state is made over: the game's feed [`conn::Feed`] into its world, its server
//! half for a local connection, its files and its own events and natives). The eye, the HUD and the prediction read the record
//! world ([`ghost_world::ClientWorld`]) through the game's view of it ([`render::PoseView`], [`predict::game::PredictWorld`]) and
//! the game's records through [`records`]; the ghost objects and the datablocks are made from the same. TGE's own answers are here
//! too ([`conn::tge`], [`records::tge`]): a client over TGE's records needs no game crate.

pub mod actionmap;
pub mod conn;
pub mod console_log;
pub mod datablocks;
pub mod eye;
pub mod fps;
pub mod game;
pub mod ghost_objects;
pub mod host;
pub mod hud;
pub mod input;
pub mod live_perf;
pub mod net;
pub mod perf;
#[cfg(feature = "window")]
pub mod platform;
pub mod predict;
pub mod query;
pub mod records;
pub mod render;
pub mod screen;
pub mod state;
pub mod view;
pub mod zoom;

/// The object a method native was called on, if it still exists.
pub fn this(vm: &ts_vm::vm::Vm, c: &ts_vm::vm::NativeCall) -> Option<ts_vm::sim::ObjId> {
    c.this_object.filter(|id| vm.objects().contains(*id))
}

/// A native's argument `i` (empty past the last).
pub fn arg(c: &ts_vm::vm::NativeCall, i: usize) -> &[u8] {
    c.argv.get(i).map_or(&[][..], Vec::as_slice)
}
