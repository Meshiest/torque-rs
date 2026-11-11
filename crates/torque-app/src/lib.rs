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
//! - [`window`]: a window's GPU surface and [`window::run`], which drives a [`GameApp`] in a window.
//! - [`page_bridge`]: a web build's page and the game's scripts calling each other (`torque.call`, `callPage`), within caps.
//! - [`web_page`]: the entry web builds share: the page's calls ([`web_exports!`]), the files it fetches, its settings, and
//!   the loading stages and frame times written on the page.
//! - `serve` (the `p2p` feature, natively): a dedicated server's switches for browsers (the web beside UDP, data channels,
//!   a room) and its start-up lines.
//! - `web_join` and `web_host` (the `p2p` feature, built for the web): a browser's client joining a server, a room's host
//!   or the server in its page's worker; and that worker's server (`web_host_exports!`).
//! - [`shots`]: a [`GameApp`] drawn offscreen, frame by frame, and pictures of it written as PNGs.
//! - [`cli`]: the command line's flags and their values.
//! - [`draw_cache`] and [`layout_fonts`]: the GUI's fonts, the system's TrueType faces where there are any.
//!
//! # Maintaining
//!
//! Rules a change must keep, each guarded by a test:
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
//! - A page's `?room=` code passes `web_join::room_code` before it goes to the room service (`web_join::Target::from_page`;
//!   the module is built for the web only, so no native test runs it).

// The server half lives in `torque-app-server` (no drawing, GUI, audio or window crates); these are its modules under this
// crate's paths.
#[cfg(all(feature = "p2p", not(target_arch = "wasm32")))]
pub use torque_app_server::serve;
pub use torque_app_server::{cli, config, files, manifest, server, water};

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
pub mod terrain;
pub mod transports;
pub mod view;
#[cfg(all(feature = "p2p", target_arch = "wasm32"))]
pub mod web_host;
#[cfg(all(feature = "p2p", target_arch = "wasm32"))]
pub mod web_join;
pub mod web_page;
pub mod window;

use torque_render_core::Gpu;
use torque_render_scene::source::Eye;

use torque_client::conn::ConnGame;
use torque_client::conn::tge::TgeConn;
use torque_client::host::ClientHost;
use torque_client::screen::{ScreenRules, TgeScreen};
use torque_fonts::faces::FaceCache;
use torque_fonts::{Fallback, NoFallback};
use torque_gui::{DrawCache, GuiDraws};
use torque_server::server::Server;

#[cfg(test)]
mod plugins_tests;
#[cfg(all(test, feature = "renderer"))]
mod renderers_tests;
#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod transports_tests;

pub use client::{ClientBuilder, ClientNet, ClientSetup, GameClient};
pub use files::GameFiles;
pub use server::{GameServer, Listen};
pub use view::{Daylight, SceneShape, WorldView};

/// The client host a game drives: the connection half `C` (TGE's by default) over a script engine as its server.
pub type Host<C = TgeConn> = ClientHost<C, Server>;

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
}

/// A game the runners drive over any connection half: [`App`] with the half named. Every [`App`] is a `GameApp` over
/// [`TgeConn`]; a game with its own [`ConnGame`] implements this directly and is named to the engine with
/// [`EngineRun::app_for`](run::EngineRun::app_for).
pub trait GameApp: 'static {
    /// The connection half the game's client host runs.
    type Conn: ConnGame;

    /// The game's client host, for the runners to drive.
    fn host(&mut self) -> &mut Host<Self::Conn>;

    /// The game's client host, read only.
    fn host_ref(&self) -> &Host<Self::Conn>;

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

    /// The rules the window's first size and `setScreenMode` follow (TGE 1.4's by default).
    fn screen_rules(&self) -> &dyn ScreenRules {
        &TgeScreen
    }
}

impl<A: App + 'static> GameApp for A {
    type Conn = TgeConn;

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
