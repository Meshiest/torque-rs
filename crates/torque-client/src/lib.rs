//! The client's engine half: Torque's client parts, over which a game adds its own (its sign-in, its server lists, its
//! downloads, its UI).
//!
//! | module | part |
//! |---|---|
//! | [`actionmap`] | `ActionMap` and its natives (bindings, the key strings, `save`) |
//! | [`input`] | the platform input layer (window or injected messages into the canvas and the action maps); the keys and buttons ([`Key`], [`MouseButton`]) |
//! | [`input_source`] | devices a plugin adds ([`input_source::InputSource`]) that the client polls once per pass, as TGE's `joystick<N>` events |
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
//! | [`player_anim`] | a Player ghost's action and arm threads, as TGE's client plays them |
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
//!
//! # Maintaining
//!
//! Rules a change must keep, each guarded by a test:
//! - A tick's prediction spends at most [`predict::TICK_WORK`], about 8 ms of the reference machine (measured with
//!   `torque-sim`'s `work_units_cost`; the trace-hash scenarios' lines must not change), and a predicted Player's box is held to
//!   [`predict::MAX_PREDICTED_BOX`] (`a_predicted_players_box_is_held_to_the_largest_side_prediction_takes`).
//! - A connection keeps at most [`ghost_objects::MAX_MIRRORS`] ghost objects, whatever its ghost table
//!   (`a_wide_ghost_table_is_mirrored_up_to_the_live_cap`).
//! - A shape prediction collides with loads once, on a budget of its own that a tick's loads share
//!   ([`predict::drive::SHAPE_LOAD_WORK`]), and takes a fixed share of the tick's
//!   (`a_collision_shape_of_megabytes_loads_once_for_a_small_share_of_the_tick`,
//!   `the_loads_of_one_tick_share_one_load_budget_and_the_rest_load_on_later_ticks`); the trace-hash
//!   scenarios' busiest tick stays under half of [`predict::TICK_WORK`].
//! - Speed: a prediction world ([`predict::ClientCollision`]) finds an object by a sorted index, not a scan (every convex's
//!   transform asks), shares its plugin shapes while their sources' revisions hold, and hands out a shape's slots as one
//!   `Rc`; the bench is torque-trace-hash's `client` (a predicting client's tick with replays over a link with latency). The
//!   trace-hash lines must not move.
//! - A tick tells the plugin handlers of the ghosts that changed, not of every ghost of their class: the world lists what the
//!   applied sections created, updated and deleted (`ghost_world::world::TouchedGhosts`) and the walk of the whole class is for a
//!   list that overflowed (a reset, or more than `MAX_TOUCHED_GHOSTS`); the bench is `ghost_read`. A scene-hierarchy child
//!   moves with its parent without an update of its own, so a handler that follows where a ghost is drawn asks
//!   `ClientWorld::world_pose` each frame rather than waiting to be told; `ClientGhostLookup::position` (a plugin's view of
//!   where a ghost is) answers the same way, so a rope hung from a child follows the child's resolved pose.
//! - A connection reads the per-ghost block only when its engine's plugins fill it (`torque_ghosts::block::BlockSections`, a
//!   resource of the engine, `conn::tge::ConnectHello::blocks`), and the manifest makes the server's plugins the same.
//! - Nothing a server or an off-path sender can predict or flood is trusted: the TGE connect sequence mixes in the system's
//!   randomness but under a fixed key (the trace hashes) (`conn::tests::the_connect_sequence_is_random_outside_a_run_that_must_repeat`);
//!   a received datagram's decoders spend at most its work budget (`torque_ghosts::datagram_budget`); a profile with the
//!   handshake extension pads its query requests so a server that bounds what it reflects answers in full
//!   (`net::serverquery::tests::a_padded_request_reads_as_the_request_it_pads`); a room join refused `busy` is tried again
//!   three times, backing off (`net::room_join::tests::busy_is_tried_again_three_times_backing_off_and_nothing_else_is`).

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
pub mod input_source;
pub mod live_perf;
pub mod net;
pub mod perf;
#[cfg(feature = "window")]
pub mod platform;
pub mod player_anim;
pub mod predict;
pub mod query;
pub mod records;
pub mod render;
pub mod screen;
pub mod state;
pub mod view;
pub mod zoom;

pub use input::{Key, MouseButton};

/// The object a method native was called on, if it still exists.
pub fn this(vm: &ts_vm::vm::Vm, c: &ts_vm::vm::NativeCall) -> Option<ts_vm::sim::ObjId> {
    c.this_object.filter(|id| vm.objects().contains(*id))
}

/// A native's argument `i` (empty past the last).
pub fn arg(c: &ts_vm::vm::NativeCall, i: usize) -> &[u8] {
    c.argv.get(i).map_or(&[][..], Vec::as_slice)
}
