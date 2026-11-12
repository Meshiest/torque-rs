//! What a game made on the engine shares: its files, a server, a client, a window, the world view and offscreen pictures. The
//! umbrella crate re-exports it as `torque::app` (its `app` feature). The server half (the files, the server, the water block,
//! the dedicated runner and the plugins a dedicated server needs) is the `torque-app-server` crate, which depends on no drawing,
//! GUI, audio or window crate and is re-exported here under the same module paths.
//!
//! - [`files::GameFiles`]: a game folder read once into memory, served to the scripts as their files.
//! - [`config::GameConfig`]: `game.toml`, a game's config: entry scripts, plugins on and off, the runner.
//! - [`manifest::Manifest`]: the list of a web build's files (`files.json`: paths, sizes, hashes) and its caps.
//! - [`server::GameServer`]: the server's main loop with TGE's game classes and a network host, on a UDP socket or an in-memory
//!   link: started, ticked, called, its console drained; [`server::GameServer::run_dedicated`] paces a dedicated one.
//! - [`client::GameClient`]: a client host (script engine, GUI, action maps, TGE's connection half), its globals and keys, and the
//!   server it hosts in the same process on the same clock.
//! - [`view::WorldView`]: the per-frame world view, drawn by the renderer slot's renderer under the GUI.
//! - [`transports`]: the transports a client connects over and a server listens on, registered by plugins under ids (UDP, memory,
//!   web, rooms, relay) and picked by id.
//! - [`render`]: passes at the stages of the engine's renderer's frame, static batches and particle emitter kinds, added by
//!   plugins ([`render::EngineRender::render`]).
//! - [`renderers`]: the renderer slot: renderers registered by plugins under ids and swapped while the game runs
//!   (`setRenderer`), the engine's own renderer and its features as plugins.
//! - [`gizmos`]: debug lines, boxes, spheres, arrows, crosses and labels drawn in world space (`GizmosPlugin`, opt-in; the
//!   `gizmos` feature).
//! - [`ghosts::GhostScene`]: the client's ghosts drawn through the engine's moving shapes, with live Players, held images and
//!   projectiles.
//! - [`interiors::Interiors`]: the map's interiors (InteriorInstance ghosts) as static meshes of the view (kept on the GPU).
//! - [`hosting`]: hosting a game and joining one: the client's network mode, the address a hosted game binds and is joined by,
//!   a typed address and the join screen's natives.
//! - [`lights::WorldLights`]: the engine's spot lights and the day (TimeOfDay, ScatterSky) of a client's world, into the view.
//! - [`orbit::Orbit`]: a third-person camera that circles a point; [`orbit::Zoom`]: a camera distance the wheel moves.
//! - [`camera_blend::CameraBlend`]: a camera that eases from one mode's eye to another's on a switch.
//! - [`camera_boom::CameraBoom`]: a third-person camera's boom, pulled in fast and let out slowly by what is in the way.
//! - [`camera_behind`]: the camera behind a figure, looking where the figure looks.
//! - [`camera_modes`]: named camera modes plugins add (first person, third person, orbit, a plugin's own), the one in use and the
//!   eased switch between them, and the console functions `setCameraMode` and `getCameraMode`.
//! - [`terrain::Terrain`]: a `TerrainBlock` over a legacy `.ter`: its collision on the server and in the client's prediction,
//!   and its ground drawn.
//! - [`water::Water`]: a `WaterBlock` as water the Players swim in, on the server and in the client's prediction.
//! - [`plugins`]: [`plugins::DefaultPlugins`], the plugins that wire a server's and a client's script engine, replayed by
//!   [`server::GameServer::start_engine`] and [`client::ClientBuilder::from_engine`].
//! - [`run`]: [`run::EngineRun`] and the runners behind [`Engine::run`](torque_engine::Engine::run), each a plugin on the
//!   engine's runner slot: the window, the page's canvas, offscreen and a dedicated server.
//! - [`window`]: a window's GPU surface, the window's choices a game makes ([`window::WindowChoices`]) and [`window::run`],
//!   which drives a [`GameApp`] in a window.
//! - [`page_bridge`]: a web build's page and the game's scripts calling each other (`torque.call`, `callPage`), within caps.
//! - [`web_page`]: the entry web builds share: the page's calls ([`web_exports!`]), the files it fetches, its settings, and
//!   the loading stages and frame times written on the page.
//! - `serve` (the `p2p` feature, natively): a dedicated server's switches for browsers (the web beside UDP, data channels,
//!   a room) and its start-up lines.
//! - `web_join` and `web_host` (the `p2p` feature, built for the web): a browser's client joining a server, a room's host
//!   or the server in its page's worker; and that worker's server (`web_host_exports!`).
//! - `testing` (feature `testing`, for tests): `Rig`, a server and engine clients over the memory transport, and
//!   `TgeOnlyClient`, a client that speaks only Torque's protocol; the plugin crates' net tests share them.
//! - [`shots`]: a [`GameApp`] drawn offscreen, frame by frame, and pictures of it written as PNGs.
//! - [`cli`]: the command line's flags and their values.
//! - [`draw_cache`] and [`layout_fonts`]: the GUI's fonts, the system's TrueType faces where there are any.
//!
//! # Maintaining
//!
//! Rules a change must keep, each guarded by a test:
//! - A [`ghosts::GhostScene`] gives the Sky's wind ([`torque_render_scene::sky::wind`]) to every emitter it runs at the start
//!   of a frame (an emitter made in a frame has it from the next) and to the pool's frame, and runs the ghosted
//!   `ParticleEmitterNode`s ([`torque_render_scene::emitter_nodes::EmitterNodes`]) beside the trails and the dust, handing their
//!   particles to the pool or drawing them like the others (`the_skys_wind_reaches_every_emitter_and_the_pools_frame`).
//! - A [`ghosts::GhostScene`] draws its moving shapes under [`ghosts::GhostOptions::budget`] (4M vertices and 4 shape parses a
//!   frame by default) (`a_frames_moving_shapes_stay_within_the_vertex_budget_nearest_first` and
//!   `a_frame_parses_at_most_its_budget_of_new_shapes_and_the_rest_wait`, in `torque-render-scene`).
//! - A [`ghosts::GhostScene`] builds its moving shapes in its own shape cache and [`view::WorldView`] keeps their objects, so a
//!   ghost posed as it was is the last frame's object again
//!   (`a_ghost_posed_as_it_was_takes_its_last_objects_draws_and_converts_nothing`, in `torque-render-scene`); with
//!   [`ghosts::GhostOptions::cull`] it leaves out only what [`view::WorldView::ghost_cull`] says nothing can show from the eye
//!   about to be drawn, shadows and reflections included (`a_shape_out_of_view_whose_shadow_falls_in_view_is_kept`, in
//!   `torque-render-scene`), and nothing on the first frame, a cut's or another renderer's
//!   (`a_cut_to_a_far_view_draws_its_ghosts_on_that_frame`, in `tests/drawn_shapes.rs`).
//! - A [`ghosts::GhostScene`] that draws the moving shapes ([`ghosts::GhostOptions::select`] `Moving`) draws the resting plugin
//!   ghosts (sleeping props) as still copies of one static batch a shape (`resting.rs`; ids from [`view::RESTING_BATCHES`]),
//!   laid out again only when a shape's resting copies change, and its moving shapes leave out exactly the ghosts it drew so
//!   (`Hooks::still`): one it could not keep (past [`view::MAX_RESTING_SHAPES`], a shape that did not load, a batch the
//!   budget would cut short) is drawn every frame. Whether a ghost rests is read live (its record and the datablocks its draw
//!   reads), a copy looks as the ghost drawn every frame, and a ghost that stops resting leaves its batch the same frame
//!   (`a_resting_plugin_ghost_is_a_still_copy_that_looks_as_the_moving_one`, in `tests/drawn_shapes.rs`;
//!   `a_ghosts_shape_is_read_live_so_a_datablock_arriving_or_changing_after_it_counts`; the level demo's
//!   `the_sleeping_furniture_is_drawn_still_and_a_pushed_chair_moves_again`).
//! - The interiors hand their panes out with the last frame's draws moved over, not copied, and a frame with none changed
//!   reads no file name again (bench `interiors update 100 kept` in `benches/app.rs`, timed under the bench lock).
//! - Every runner takes a [`GameApp`], not an [`App`]: [`App`] is the game over TGE's connection half, and a blanket impl makes
//!   each one a `GameApp` with that `Conn`, so [`window::run`], [`shots::Seat`] and the runner plugins have one generic path
//!   for every connection half. [`run::EngineRun::app_for`] stores, for the game's type, the three runners (window, page,
//!   offscreen) as closures over its `make`; nothing past it is erased, so a game's frame calls are not dynamic
//!   (`app_for_names_a_game_app_and_the_offscreen_runner_draws_64_frames_of_it`, and
//!   `every_app_is_a_game_app_over_the_tge_connection`, in `run_tests.rs`). The app slot is one slot for `app` and `app_for`
//!   (`app_for_is_the_same_exclusive_slot_as_app`). A new method a runner needs from the game goes on [`GameApp`] with a
//!   default, and on [`App`] with the same default, so existing games keep compiling.
//! - The runners never draw a frame themselves. A frame of [`window::run`] or [`shots::Seat::frame`] is: [`GameApp::frame`],
//!   then [`GameApp::draw`] into the runner's target, then `count_frame` (`$Client::FrameRate`, and the frame average only
//!   when [`WindowChoices::fps_update`] is chosen: `a_frame_counts_the_frame_average_only_when_the_game_chose_it`, in
//!   `window_tests.rs`). `draw`'s default is [`WorldView::draw`], so a game that draws nothing of its own gets the view's
//!   frame pixel for pixel (`the_default_draw_is_the_views_draw_pixel_for_pixel`, in `shots_tests.rs`; the examples'
//!   shots), and one with its own `draw` paints the frame in the view's place
//!   (`a_games_own_draw_paints_the_frame_in_place_of_the_view`). [`shots::Seat::world_picture`] draws the world alone and
//!   does not call `draw`.
//! - The window's per-pass choices are the game's. `AppWindow` answers the driver's `fresh_resolution`, `frame_cap_pref`,
//!   `may_grab` and `vsync` from [`GameApp::window_choices`], read again at each call so an answer can change while the game
//!   runs; it runs [`GameApp::upkeep`] after its own (the page bridge, in a browser) and hands [`GameApp::pass_work`] each
//!   pass's time (`window_tests.rs`). The surface's present mode follows `vsync` before each frame
//!   ([`window::WindowSurface::set_vsync`] configures the surface again only on a change), and a native window's device is
//!   [`GameApp::window_gpu`]'s (a browser's canvas asks for its own). Every default is what the window does for a game that
//!   chooses nothing (`a_game_that_makes_no_choices_gets_the_windows_own`): a new value the window reads goes into
//!   [`WindowChoices`] with such a default, a new callback onto [`GameApp`] and [`App`] alike.
//! - A view that starts released ([`view::WorldView::released_with_extension`]) is `with_extension` minus what a release
//!   drops: no `HdWorld`, no warm-up thread, and a GUI pass made at the first `draw` instead of with the view (the other
//!   constructors still make both, so their start is unchanged). Its first world is built with the renderer's defaults
//!   (`Carried::Defaults`, not a released world's `HdSettings`), by `restore_gpu` or `adopt_gpu`
//!   (`a_view_that_starts_released_builds_no_world_and_draws_the_same_picture_after_a_restore`, in `tests/view_rebuild.rs`).
//! - The native window saves the pipeline cache after the first frame where [`GameApp::warming`] is false (the view's own
//!   warm-up by default; a game with its own compile worker answers for it) and again at the close; the decision is
//!   `AppWindow::cache_due` (`the_cache_is_saved_once_the_game_says_its_warm_up_is_over`, in `window_tests.rs`). A new hook
//!   goes on [`GameApp`] and [`App`] alike.
//! - A client is made on the engine's script engine and runs the engine's client entry in `exec_entry` when a plugin set them
//!   ([`slots::HostSlots`]), and over a game's own file host with [`ClientBuilder::from_engine_with_files`]; both
//!   `from_engine` constructors share one body that is not generic over the file host. With none set the client is the same as
//!   before (`host_slots_tests.rs`; the native snapshots in `tests/natives`).
//! - A client over its server ([`ClientBuilder::over_server`]) is made by the same body as a client of its own
//!   (`ClientBuilder::make`, generic over the conn game only) on the server's script engine, taken out of the server for the
//!   stages and given back in [`ClientBuilder::build`], so the client host's server host is the server and each pass ticks
//!   its main loop. Every difference between the two is a method of the private `Sharing` in `client.rs` (the replay, the
//!   server's natives listed, the build's warnings, the plugins' collision), so `make` itself has no branch on it: on a
//!   shared engine the client's stages replay with `Engine::replay_shared`, and the build's warnings, the files and the
//!   plugins' collision are the server's; a client native over a server native is listed, not printed
//!   ([`ClientBuilder::replaced_natives`]). A client of its own takes none of these branches, so its natives and bytes do
//!   not move (`client_over_server_tests.rs`; the native snapshots; the wire goldens). [`GameApp::Server`] names the server
//!   host to the runners; every [`App`] has [`Server`]. A new difference goes on `Sharing`, not into `make`.
//! - A client fails to be made with a [`ClientError`], one variant a cause (no client host, no GUI, two GUIs, a recipe off a
//!   client, the wrong conn game, RPC lanes over a shared engine, the engine's build, channels, RPCs and socket), which the
//!   tests match by variant (`a_client_needs_its_gui_and_its_host`, `a_second_gui_given_to_a_client_is_refused`, in
//!   `plugins_tests.rs`; `a_second_gui_is_refused`, in `client_over_server_tests.rs`;
//!   `conn_game_wiring_is_typed_and_the_parts_are_not`, in `torque-trace-hash/tests/conn_game.rs`). A client recipe returns
//!   the engine's `String` error, so the helpers a recipe calls ([`plugins::ClientWiring::of`],
//!   [`plugins::ClientParts::set_gui`], `client_parts`, `client_gui`) return a `ClientError` that `?` turns into its message
//!   (`From<ClientError> for String`); one raised in a recipe reaches the builder as [`ClientError::Recipe`], with the plugin
//!   and the stage. A new cause gets a variant, never a `String` (the enum is `#[non_exhaustive]`).
//! - [`plugins::ClientHostSetup::hosts_server_global`] is the one place the hosting global's name lives: `build`,
//!   `start_hosting` and `stop_hosting` set it through `GameClient::mark_hosting`, and `None` sets nothing
//!   (`the_hosting_global_follows_hosting_unless_the_host_names_none`, in `plugins_tests.rs`). [`plugins::ClientHostPlugin`]
//!   defaults it to [`client::HOSTS_SERVER_GLOBAL`], so every game on the engine's host keeps it.
//! - [`view::WorldView`] is generic over the renderer's extension (`WorldView<E = NoExtension>`): `new` stays on the default
//!   type and [`view::WorldView::with_extension`] makes any other, so every game that names no extension builds the same code
//!   as before; [`GameApp::Ext`] names it for the runners (`NoExtension` for every [`App`]). The view reaches the frame only
//!   through the scene's deref and [`ViewScene`] (mode, time, batch cells), never a field of the engine's own scene. A view
//!   with a game's extension makes its slot refuse every renderer but the engine's (`RendererSlot::attach_gpu` takes the
//!   extension's name; [`renderers::SwitchError::Extension`]), and a scene without batch cells refuses static meshes,
//!   batches and the plugins' sources with a warning naming them (both in `tests/render_ext.rs`, whose tint test also checks
//!   a game's extension with its block off draws exactly the engine's picture). The view's GPU world is its `world` field,
//!   made in `with_extension` alone; the slot's features and passes go into it in `attach_slot`/`take_adds`, and the scene's
//!   cells carry their `resident` flags. `WorldView::release_gpu` empties it and `restore_gpu` rebuilds it
//!   (`tests/view_rebuild.rs`): `world` is `None` while released, and `carried` holds the renderer's settings
//!   (`HdSettings`, from `HdWorld::settings`) for the new `HdWorld` (its `ext`, histories and exposure start over), which
//!   then gets `warm_effects`, the slot's features and `take_adds` (passes, batch sources, kinds), every scene cell's
//!   `resident` flag cleared (counted when its mesh was dropped: the game builds those again) and `statics_changed`. The
//!   scene, the quality, the particle light, the daylight, the GUI pass and the slot are not touched, so no setter runs again.
//!   Everything that reads `world` goes through the `Option`: a draw with the engine's renderer errors, `draw` leaves the
//!   world to the app (`own_look`: no clear, the GUI over it) and `renderer()` panics, so a new use of `world` handles `None`.
//!   A new GPU-held part of the view (not the scene) belongs in `release_gpu`, and a new plain renderer setting that must
//!   survive belongs in `HdSettings` (`settings`/`apply_settings` in torque-render-hd); `tests/view_rebuild.rs` compares pictures, so a part missed shows as a differing pixel.
//!   `WorldView::adopt_gpu` takes an `HdWorld<E>` built elsewhere (a worker thread, warmed there) in place of the one the view
//!   holds: it is `release_gpu` and then the end of `restore_gpu` (`install_world`) with the given world, so the two share one
//!   body and cannot differ in what they re-apply; it draws the pixels a restore does (`a_world_built_elsewhere_draws_the_picture_a_restore_does`
//!   and `adopting_a_world_replaces_the_one_a_view_holds_and_counts_the_cells_without_meshes`, in `tests/view_rebuild.rs`). The world must
//!   be the view's extension on the view's device.
//! - [`window::run`] makes its event loop and calls [`window::run_with`], which takes one the caller built, so a client run off
//!   the main thread (where `EventLoop::new` panics on Windows) builds its own with the platform's builder extension; the
//!   two are one body, so a change to the window's run goes in `run_with` (`run_takes_a_game_and_run_with_a_game_and_an_event_loop_the_caller_built`,
//!   in `window_tests.rs`, pins both signatures: no test opens a window).
//! - A page's `?room=` code passes `web_join::room_code` before it goes to the room service (`web_join::Target::from_page`;
//!   the module is built for the web only, so no native test runs it).
//! - The client recipes name no conn game: a client of any [`ConnGame`] is made by the same
//!   plugins at the same stages ([`client::ClientBuilder::from_engine_with_conn`]; the recipes call
//!   [`plugins::ClientParts::installs`], made for the client's conn game), and the only typed extension is
//!   [`plugins::ClientWiring<C>`](plugins::ClientWiring). A conn game with TGE's feed gets the engine's natives and its own,
//!   and writes the bytes a client of TGE's writes in both protocols (`conn_game_natives_are_the_engines_and_its_own`,
//!   `conn_game_writes_torques_bytes_in_both_protocols`, in `torque-trace-hash/tests/conn_game.rs`), and the runners drive a
//!   [`GameApp`] over it (`conn_game_app_for_draws_64_offscreen_frames_of_a_client_of_its_own_conn_game`, in `run_tests.rs`).
//! - The world's sounds ([`sounds`]) run at the end of each tick of TGE's connection (its `ticked` hook,
//!   `torque_client::conn::tge::TgeTicked`, which [`client::ClientBuilder::build`] sets), before the plugins' ticks, so a
//!   voice a tick starts is advanced by the audio plugin's tick of the same pass: a server `playAudio` is a voice at the
//!   object in both protocols, following it, stopped by `stopAudio`, the ghost's end or a new world
//!   (`tests/shape_sounds.rs`). The sound card is opened by [`window::run`] alone, behind the `audio-device` feature; a
//!   failure leaves the mixer on the clock and the game plays on.
//! - The QUIC link (`quic` feature, native) is the client transport `quic` ([`transports::QuicClientTransportPlugin`], for
//!   [`transports::QuicTarget`]): the socket names its server by the address dialed under [`torque_client::net::NetPolicy`]
//!   `Loopback` or `Internet`, so the engine's ticket is made for that address as on UDP, plus the link's identity and
//!   channel binding. The server's key is checked against the identity given, or trusted on first use and remembered in
//!   `known_servers` under the user's data folder ([`transports::known_servers_path`]); the link's state comes back as a
//!   `torque_net_quic::QuicWatch` in the [`transports::LinkInfo`] (`tests/quic_link.rs`: a pinned identity connects and gets
//!   its ghosts, a wrong one is refused naming both hashes, a changed key is refused, a moved socket keeps its connection).
//! - A [`ghosts::GhostScene`] keys a Player's live pose on its datablock's generation beside the shape's node count and the
//!   ghost's birth: a datablock changed under it (a shape of the same node count among them) makes the pose again, at most
//!   `MAX_REFRESHES_PER_FRAME` poses a frame, the rest drawn as they are until a later frame; the preload loads what a changed
//!   datablock names, keyed on the same generation (`a_players_datablock_change_rebuilds_its_live_pose_within_the_frames_budget`).
//!
//! **Test support.** `testing` (feature `testing`; only dev-dependencies turn it on) holds `Rig<P>`, a server and engine clients over
//! the memory transport stepped by hand, and `TgeOnlyClient`, a client that speaks only Torque's protocol. A plugin's net tests
//! implement `RigPlugin` for their plugin (its engine and start function) and keep their own files and reads.

// The server half lives in `torque-app-server` (no drawing, GUI, audio or window crates); these are its modules under this
// crate's paths.
#[cfg(all(feature = "p2p", not(target_arch = "wasm32")))]
pub use torque_app_server::serve;
pub use torque_app_server::{cli, config, files, manifest, net_game, server, slots, water};

pub mod camera_behind;
pub mod camera_blend;
pub mod camera_boom;
pub mod camera_modes;
pub mod client;
pub mod ghosts;
pub mod gizmos;
pub mod hosting;
pub mod interiors;
pub mod lights;
pub mod orbit;
pub mod page_bridge;
pub mod plugins;
pub mod render;
pub mod renderers;
mod resting;
pub mod run;
pub mod shots;
pub mod sounds;
pub mod terrain;
#[cfg(feature = "testing")]
pub mod testing;
pub mod transports;
pub mod view;
#[cfg(all(feature = "p2p", target_arch = "wasm32"))]
pub mod web_host;
#[cfg(all(feature = "p2p", target_arch = "wasm32"))]
pub mod web_join;
pub mod web_page;
pub mod window;

use std::sync::Arc;

pub use torque_render_core::Gpu;
use torque_render_core::wgpu;
use torque_render_hd::ext::{HdExtension, NoExtension, ViewScene};
pub use torque_render_scene::source::Eye;
use web_time::Duration;
use winit::window::Window;

use torque_client::conn::ConnGame;
use torque_client::conn::tge::TgeConn;
use torque_client::host::ClientHost;
use torque_client::screen::{ScreenRules, TgeScreen};
use torque_fonts::faces::FaceCache;
use torque_fonts::{Fallback, NoFallback};
use torque_gui::{DrawCache, GuiDraws};
use torque_server::server::Server;

#[cfg(test)]
mod host_slots_tests;
#[cfg(test)]
mod plugins_tests;
#[cfg(all(test, feature = "renderer"))]
mod renderers_tests;
#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod shots_tests;
#[cfg(test)]
mod transports_tests;

pub use client::{ClientBuilder, ClientError, ClientNet, ClientSetup, GameClient};
pub use files::GameFiles;
pub use server::{GameServer, Listen};
pub use view::{Daylight, SceneShape, WorldView};
pub use window::WindowChoices;

/// The client host a game drives: the connection half `C` (TGE's by default) over the server host `S` of its script
/// engine: one of its own by default, or the game's server it runs over ([`ClientBuilder::over_server`]).
pub type Host<C = TgeConn, S = Server> = ClientHost<C, S>;

/// A game the window ([`window::run`]) and the pictures ([`shots`]) drive.
pub trait App {
    fn host(&mut self) -> &mut Host;

    fn host_ref(&self) -> &Host;

    /// The window's title.
    fn title(&self) -> String;

    /// The name printed in front of the window's notes.
    fn log_name(&self) -> &str;

    /// A world view on `gpu` for the game's frames.
    fn view(&self, gpu: &Gpu) -> WorldView;

    /// `ms` of the clock between two frames.
    fn advance(&mut self, ms: u32);

    /// The frame: the world into `view`, the canvas drawn (with the world's rectangle) and the eye.
    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye);

    /// Whether the game is still loading its map (a browser's page keeps its loading display up until it is not).
    fn loading(&self) -> bool {
        false
    }

    /// Draws the frame into `target`: [`GameApp::draw`], with the same default ([`WorldView::draw`]).
    fn draw(
        &mut self,
        view: &mut WorldView,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        gui: &GuiDraws,
        eye: Eye,
    ) -> Result<bool, String> {
        view.draw(gpu, target, gui, eye)
    }

    /// The window's choices: [`GameApp::window_choices`], the window's own by default.
    fn window_choices(&self) -> WindowChoices<'_> {
        WindowChoices::default()
    }

    /// The upkeep of a pass of the window's loop: [`GameApp::upkeep`], nothing by default.
    fn upkeep(&mut self) {}

    /// The wall time a pass of the window's loop took: [`GameApp::pass_work`], ignored by default.
    fn pass_work(&mut self, work: Duration) {
        let _ = work;
    }

    /// Whether the game is still compiling pipelines: [`GameApp::warming`], the view's own warm-up by default.
    fn warming(&self, view: &WorldView) -> bool {
        view.warming()
    }

    /// The window's device and surface: [`GameApp::window_gpu`], the engine's choice by default.
    fn window_gpu(
        &mut self,
        window: &Arc<Window>,
    ) -> Result<(Gpu, wgpu::Surface<'static>), String> {
        window::default_gpu(window)
    }
}

/// A game the runners drive over any connection half: [`App`] with the half named. Every [`App`] is a `GameApp` over
/// [`TgeConn`]; a game with its own [`ConnGame`] implements this directly and is named to the engine with
/// [`EngineRun::app_for`](run::EngineRun::app_for).
pub trait GameApp: 'static {
    /// The connection half the game's client host runs.
    type Conn: ConnGame;

    /// The server host of the client host's script engine: [`Server`] for a client on a script engine of its own, the
    /// game's server for a client that runs over it ([`ClientBuilder::over_server`]).
    type Server: torque_client::host::GameServer;

    /// The extension the world view draws the game's own static geometry with ([`view::WorldView`]'s `E`;
    /// [`NoExtension`] for the engine's plain cells).
    type Ext: HdExtension<Scene: ViewScene>;

    /// The game's client host, for the runners to drive.
    fn host(&mut self) -> &mut Host<Self::Conn, Self::Server>;

    /// The game's client host, read only.
    fn host_ref(&self) -> &Host<Self::Conn, Self::Server>;

    /// The window's title.
    fn title(&self) -> String;

    /// The name printed in front of the window's notes.
    fn log_name(&self) -> &str;

    /// A world view on `gpu` for the game's frames.
    fn view(&self, gpu: &Gpu) -> WorldView<Self::Ext>;

    /// `ms` of the clock between two frames.
    fn advance(&mut self, ms: u32);

    /// The frame: the world into `view`, the canvas drawn (with the world's rectangle) and the eye.
    fn frame(&mut self, view: &mut WorldView<Self::Ext>) -> (GuiDraws, Eye);

    /// Whether the game is still loading its map (a browser's page keeps its loading display up until it is not).
    fn loading(&self) -> bool {
        false
    }

    /// The rules the window's first size and `setScreenMode` follow (TGE 1.4's by default).
    fn screen_rules(&self) -> &dyn ScreenRules {
        &TgeScreen
    }

    /// Draws the frame into `target` (an `Rgba8Unorm` texture of the canvas's size), right after [`frame`](Self::frame) gave
    /// `gui` and `eye` (the offscreen seat's own eye in its place when it has one). Returns whether a world was drawn: the
    /// window caps its frames while none is. The window and the offscreen seat draw every frame through here and never call
    /// [`WorldView::draw`] themselves, so a game with a renderer of its own (a view released with
    /// [`WorldView::release_gpu`]) or one that layers its GUI another way draws what it likes. The default is
    /// [`WorldView::draw`]: the world in the GUI's world rectangle, then the whole GUI over it.
    fn draw(
        &mut self,
        view: &mut WorldView<Self::Ext>,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        gui: &GuiDraws,
        eye: Eye,
    ) -> Result<bool, String> {
        view.draw(gpu, target, gui, eye)
    }

    /// What the window does each pass and each frame that is the game's to choose ([`WindowChoices`]): read as the window
    /// opens and again every pass, so an answer may change while the game runs. The default is the window's own.
    fn window_choices(&self) -> WindowChoices<'_> {
        WindowChoices::default()
    }

    /// Once a pass of the window's loop, after the clock's step and before the cursor and the screen mode are looked at (the
    /// work a game's own loop does between its steps). Nothing by default.
    fn upkeep(&mut self) {}

    /// The wall time the window's loop spent in a pass, its frame aside (a game's budget for its own work). Ignored by default.
    fn pass_work(&mut self, work: Duration) {
        let _ = work;
    }

    /// Whether the game is still compiling pipelines (its own warm-up worker, beside the view's): the native window saves the
    /// device's pipeline cache once this is false after a frame, and again at the close, so a crash after the warm-up keeps
    /// what it compiled. The default is the view's own warm-up ([`WorldView::warming`]), which is over at once for a view
    /// that starts released ([`WorldView::released_with_extension`]): a game with a worker of its own overrides this to
    /// answer true until the worker is done.
    /// A browser keeps no such cache; it never asks.
    fn warming(&self, view: &WorldView<Self::Ext>) -> bool {
        view.warming()
    }

    /// The device and surface the window draws with, asked for once as a native window is made (a browser's canvas asks for
    /// its own without blocking and never calls this). The window configures the surface. The default is the engine's
    /// window order ([`torque_render_core::window::window_gpu`]: `WGPU_BACKEND`, then Vulkan first); a game picks its own
    /// backends or display device with [`torque_render_core::window::window_gpu_prefer`] or a device of its own.
    ///
    /// # Errors
    ///
    /// When no device and surface can be made: the window's loop ends with the message.
    fn window_gpu(
        &mut self,
        window: &Arc<Window>,
    ) -> Result<(Gpu, wgpu::Surface<'static>), String> {
        window::default_gpu(window)
    }
}

impl<A: App + 'static> GameApp for A {
    type Conn = TgeConn;
    type Server = Server;
    type Ext = NoExtension;

    fn host(&mut self) -> &mut Host {
        App::host(self)
    }

    fn host_ref(&self) -> &Host {
        App::host_ref(self)
    }

    fn title(&self) -> String {
        App::title(self)
    }

    fn log_name(&self) -> &str {
        App::log_name(self)
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        App::view(self, gpu)
    }

    fn advance(&mut self, ms: u32) {
        App::advance(self, ms);
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        App::frame(self, view)
    }

    fn loading(&self) -> bool {
        App::loading(self)
    }

    fn draw(
        &mut self,
        view: &mut WorldView,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        gui: &GuiDraws,
        eye: Eye,
    ) -> Result<bool, String> {
        App::draw(self, view, gpu, target, gui, eye)
    }

    fn window_choices(&self) -> WindowChoices<'_> {
        App::window_choices(self)
    }

    fn upkeep(&mut self) {
        App::upkeep(self);
    }

    fn pass_work(&mut self, work: Duration) {
        App::pass_work(self, work);
    }

    fn warming(&self, view: &WorldView) -> bool {
        App::warming(self, view)
    }

    fn window_gpu(
        &mut self,
        window: &Arc<Window>,
    ) -> Result<(Gpu, wgpu::Surface<'static>), String> {
        App::window_gpu(self, window)
    }
}

/// A frame drawn at real time `real_ms`, counted as the window counts it: `$Client::FrameRate` ([`Host::frame_drawn`]), and
/// the frame average too ([`Host::fps_update`]) when the game's [`WindowChoices::fps_update`] says so.
pub(crate) fn count_frame<G: GameApp>(game: &mut G, real_ms: f64) {
    let fps_update = game.window_choices().fps_update;
    let host = game.host();
    host.frame_drawn(real_ms);
    if fps_update {
        host.fps_update(real_ms);
    }
}

/// The system's fonts as the fallback for a face with no font cache in `fonts/` (none where the system has none); in a
/// browser, the page's fonts.
fn system_fallback() -> Box<dyn Fallback> {
    #[cfg(target_arch = "wasm32")]
    if let Some(browser) = torque_fonts::web::BrowserFonts::new() {
        return Box::new(browser);
    }
    match torque_fonts::system::SystemFonts::new() {
        Some(system) => Box::new(system),
        None => Box::new(NoFallback),
    }
}

/// The GUI's draw cache: fonts from `.uft` files in `fonts/` when there are any, else rasterized from the system's TrueType
/// face of the profile's name (Windows' fonts folder, the font folders of Linux or macOS with free stand-ins for TGE's common
/// faces, or a browser's fonts); with neither, text is not drawn.
pub fn draw_cache() -> DrawCache {
    DrawCache::new(Box::new(layout_fonts()))
}

/// The fonts the GUI's layout measures text with: the same faces [`draw_cache`] draws with.
pub fn layout_fonts() -> FaceCache {
    FaceCache::new(
        "fonts",
        system_fallback(),
        Box::new(torque_fonts::uft::Uft::default()),
        std::convert::identity,
    )
}
