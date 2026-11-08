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
//! - [`window`]: a window's GPU surface and [`window::run`], which drives an [`App`] in a window.
//! - [`page_bridge`]: a web build's page and the game's scripts calling each other (`torque.call`, `callPage`), within caps.
//! - [`web_page`]: the entry web builds share: the page's calls ([`web_exports!`]), the files it fetches, its settings, and
//!   the loading stages and frame times written on the page.
//! - `serve` (the `p2p` feature, natively): a dedicated server's switches for browsers (the web beside UDP, data channels,
//!   a room) and its start-up lines.
//! - `web_join` and `web_host` (the `p2p` feature, built for the web): a browser's client joining a server, a room's host
//!   or the server in its page's worker; and that worker's server (`web_host_exports!`).
//! - [`shots`]: an [`App`] drawn offscreen, frame by frame, and pictures of it written as PNGs.
//! - [`cli`]: the command line's flags and their values.
//! - [`draw_cache`] and [`layout_fonts`]: the GUI's fonts, the system's TrueType faces where there are any.

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

use torque_client::conn::tge::TgeConn;
use torque_client::host::ClientHost;
use torque_fonts::{Fallback, FontCache, NoFallback};
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

/// The client host a game drives: TGE's connection half over a script engine as its server.
pub type Host = ClientHost<TgeConn, Server>;

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
pub fn layout_fonts() -> FontCache {
    FontCache::new("fonts", system_fallback())
}
