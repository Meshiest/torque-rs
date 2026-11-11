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
//! - The connection's natives that take over a server's (registered after it on one script engine) answer for the
//!   server's objects as the server does: the methods fall back through `prev`, and `commandToServer` gives a remote
//!   party's command ([`ts_vm::vm::Vm::command_source`] other than the client's connection) to the server's own, so a
//!   server's handler sends nothing on the client's connection (torque-app's `client_over_server_tests.rs`). With no server
//!   native before it the client's is the only one, as on a client of its own.
//! - A tick's prediction spends at most [`predict::TICK_WORK`], about 8 ms of the reference machine (measured with
//!   `torque-sim`'s `work_units_cost`; the trace-hash scenarios' lines must not change), and a predicted Player's box is held to
//!   [`predict::MAX_PREDICTED_BOX`] (`a_predicted_players_box_is_held_to_the_largest_side_prediction_takes`).
//!   A part's look at the world (`TickStep::around`: the map pieces and the players and vehicles near its objects) is made on what
//!   is left of the tick's budget for parts and charged to it, with none left it looks at nothing
//!   (`what_a_part_asks_of_the_world_is_made_on_the_budget_for_parts_and_charged_to_it`).
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
//! - The prediction's collision world places a scene-hierarchy child for each tick where the server's latest poses of its
//!   parents and its last local pose put it (`ClientWorld::tick_pose_in`, through `predict::ClientCollision::new`), never at
//!   its record's stale pose or a frame's blended one, and the predicted object's own children where it is before each move
//!   (`ClientCollision::carry_children`). Each ghost a chain reaches is worked out once a container (one whose chain a refused
//!   charge cut is charged again when asked for again) and every link walked is charged [`predict::LINK_WORK`] to the tick's
//!   budget. What the moves always have is half of the budget (`ClientCollision::moves_reserve`): the ghost walk stops once the
//!   tick's work passes half of it less one ghost's step (`walk_stopped`, counted in `PredictStats::walk_stops`; the ghosts past
//!   it are not in the container that tick), the placing of children stops at three eighths of the tick's total work, the
//!   carrying of children takes only what the placement left of that (`child_cap`), and the predicted object's own frame stops
//!   short of the half, on a slice of one eighth (`frame_share`; the placement's poses are kept, so no ghost of them is charged
//!   again): everything but the moves' own work stays within `budget - moves_reserve` for a world and a tree of any size
//!   (`the_moves_keep_half_the_budget_under_a_huge_walk_a_hostile_tree_and_a_deep_frame_chain`,
//!   `children_stop_at_their_cap_of_the_whole_ticks_work_after_a_real_ghost_walk`,
//!   `a_ghost_walk_that_took_the_childrens_cap_leaves_them_nothing`,
//!   `the_frame_is_placed_past_the_childrens_cap_on_a_slice_of_its_own_and_no_ghost_is_charged_twice`). It costs nothing
//!   without links. The game's static set (`StaticIndex::set_near`) is not
//!   re-placed. torque-plugin-hierarchy's `tests/ride.rs`: a rider on a hung deck is corrected no more than on the same
//!   deck moved as a root, and not at all on one that never moves.
//! - A control Player hung on a parent is predicted in the parent's frame ([`predict::framed`]): the server's packet data
//!   says so (its frame, `records::TailFrame`) and carries its local translation, velocity and yaw there; each move places
//!   it at the parent's tick pose (from the placement's own tick poses, charged to the same budget) times its local pose,
//!   runs the move and takes the local pose back as the server's tree rounds it, so the parent's motion past its latest
//!   record never corrects it; a tick that cannot place the parent leaves its moves to the next that can. It is drawn, with its eye and camera, on the parent's drawn pose. torque-plugin-hierarchy's `tests/turret.rs`: a
//!   gunner on a driven tank is corrected no more than on a still base, and the driver no more than without the gunner.
//! - A connection reads the per-ghost block only when its engine's plugins fill it (`torque_ghosts::block::BlockSections`, a
//!   resource of the engine, `conn::tge::ConnectHello::blocks`), and the manifest makes the server's plugins the same.
//! - Nothing a server or an off-path sender can predict or flood is trusted: the TGE connect sequence mixes in the system's
//!   randomness but under a fixed key (the trace hashes) (`conn::tests::the_connect_sequence_is_random_outside_a_run_that_must_repeat`);
//!   a received datagram's decoders spend at most its work budget (`torque_ghosts::datagram_budget`); a profile with the
//!   handshake extension pads its query requests so a server that bounds what it reflects answers in full
//!   (`net::serverquery::tests::a_padded_request_reads_as_the_request_it_pads`); a room join refused `busy` is tried again
//!   three times, backing off (`net::room_join::tests::busy_is_tried_again_three_times_backing_off_and_nothing_else_is`).
//! - The engine-mode connect reads its link's kind and channel binding again every pass (`conn::Link::kind`,
//!   `conn::Link::binding`, from [`net::Datagrams::link_kind`] and [`net::Datagrams::channel_binding`];
//!   `conn::engine_wire::EngineWire::refresh`): a secure link (QUIC) learns the server's identity and derives its binding only
//!   as its handshake ends, and the ticket that answers the server's challenge is made for both. A socket without them keeps
//!   today's context bytes (torque-app's `tests/quic_link.rs`).
//! - A datablock changed mid-game refreshes the predictors in place, never by making them again: `predict::Predicted`,
//!   `predict::drive::DrivenVehicle` and the ghost vehicles keep the datablock's generation
//!   (`ghost_world::ClientWorld::datablock_generation`) and bring their tuning up to it (`refresh_tuning`, `refresh_wheels`
//!   with a generation per tire and spring) when it moved, so the tuning equals a fresh predictor's while position, velocity
//!   and state stay the same to the bit; a record of another class or an unreadable shape takes the generation and keeps the
//!   tuning; a resend moves nothing (`tests/datablock_refresh.rs`). The ghost vehicles are refreshed at most
//!   [`ghost_world::world::MAX_REFRESHES_PER_FRAME`] a tick (`Predictor::refresh_ghost_vehicles`). The script objects' side
//!   is [`datablocks::DatablockObjects`]: an event writes the fields only of an object the client made from an earlier event
//!   (on a shared engine the object is the server's own: preloaded, not written) and moves the object's generation when
//!   something changed, which a plugin's copy of the fields is keyed on (torque-plugin-props' `tests/datablock_refresh.rs`).
//! - A server cannot grow or busy a connection's datablocks without bound: the world's table and the script group take at most
//!   [`conn::tge::max_datablocks`] (`$Pref::Client::MaxDataBlocks` under the wire's id range; 1,024 in Torque's protocol,
//!   4,096 in the engine's when unset), and after the first datablocks-done message bodies spend a token of
//!   [`conn::tge::LiveBucket`] (256 a second, a burst of one id range; two seconds of refusals end the connection). Refusals
//!   count in `ClientState::datablocks_refused` and run no script (`conn::tge::tests`; torque-test-app's
//!   `server_datablock_live`). A datablocks-done message after the first runs `onDataBlocksChanged()` only when a script
//!   defines it, and a connection over a script engine shared with its server never writes the server's own datablock from an
//!   event ([`datablocks::Sharing`]).
//! - A datablock the server removed (`DataBlockRemoveEvent`, the engine's protocol: a feed over the engine's framing and a
//!   local connection of an engine in that mode read and write [`conn::tge::TgeTables::engine`]) is retired in the world
//!   (`ghost_world::ClientWorld::retire_datablock`), never deleted on the spot: a ghost naming it may still be on its way. The
//!   connection's tick purges what no ghost record, mounted image or predictor names (`conn::purge_retired_datablocks`,
//!   [`predict::Predictor::uses_datablock`]) and deletes only the script objects the client made
//!   ([`datablocks::DatablockObjects::made`]); a body for a retired id is refused and counted in `datablocks_refused`, a
//!   removal of an unknown or already retired id is counted in `ClientState::datablock_removes_ignored`, and more than
//!   `ghost_world::world::MAX_RETIRED_DATABLOCKS` retired at once end the connection with
//!   [`conn::tge::RETIRED_DATABLOCKS_REASON`] (torque-test-app's `server_datablock_remove_wire`; torque-server's
//!   `tests/datablock_remove_wire.rs` for the wire). Torque's protocol carries no removal: a stock client keeps the datablock.

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
