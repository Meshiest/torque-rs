//! What a game made on the engine shares: its files, a server, a client, a window, the world view and offscreen pictures. The
//! umbrella crate re-exports it as `torque::app` (its `app` feature).
//!
//! - [`files::GameFiles`]: a game folder read once into memory, served to the scripts as their files.
//! - [`manifest::Manifest`]: the list of a web build's files (`files.json`: paths, sizes, hashes) and its caps.
//! - [`server::GameServer`]: the server's main loop with TGE's game classes and a network host, on a UDP socket or an in-memory
//!   link: started, ticked, called, its console drained; [`server::GameServer::run_dedicated`] paces a dedicated one.
//! - [`client::GameClient`]: a client host (script engine, GUI, action maps, TGE's connection half), its globals and keys, and the
//!   server it hosts in the same process on the same clock.
//! - [`view::WorldView`]: the per-frame world view, drawn by the enhanced renderer under the GUI.
//! - [`ghosts::GhostScene`]: the client's ghosts drawn through the engine's moving shapes, with live Players, held images and
//!   projectiles.
//! - [`interiors::Interiors`]: the map's interiors (InteriorInstance ghosts) as static meshes of the view (kept on the GPU).
//! - [`hosting`]: hosting a game and joining one: the client's network mode, the address a hosted game binds and is joined by,
//!   a typed address and the join screen's natives.
//! - [`lights::WorldLights`]: the engine's spot lights and the day (TimeOfDay, ScatterSky) of a client's world, into the view.
//! - [`orbit::Orbit`]: a third-person camera that circles a point; [`orbit::Zoom`]: a camera distance the wheel moves.
//! - [`camera_blend::CameraBlend`]: a camera that eases from one mode's eye to another's on a switch.
//! - [`terrain::Terrain`]: a `TerrainBlock` over a legacy `.ter`: its collision on the server and in the client's prediction,
//!   and its ground drawn.
//! - [`window`]: a window's GPU surface and [`window::run`], which drives an [`App`] in a window.
//! - [`web_page`]: the entry web builds share: the page's calls ([`web_exports!`]), the files it fetches, its settings, and
//!   the loading stages and frame times written on the page.
//! - [`shots`]: an [`App`] drawn offscreen, frame by frame, and pictures of it written as PNGs.
//! - [`cli`]: the command line's flags and their values.
//! - [`draw_cache`] and [`layout_fonts`]: the GUI's fonts, the system's TrueType faces where there are any.

pub mod camera_blend;
pub mod cli;
pub mod client;
pub mod files;
pub mod ghosts;
pub mod hosting;
pub mod interiors;
pub mod lights;
pub mod manifest;
pub mod orbit;
pub mod server;
pub mod shots;
pub mod terrain;
pub mod view;
pub mod web_page;
pub mod window;

use torque_render_core::Gpu;
use torque_render_scene::source::Eye;

use torque_client::conn::tge::TgeConn;
use torque_client::host::ClientHost;
use torque_fonts::{Fallback, FontCache, NoFallback};
use torque_gui::{DrawCache, GuiDraws};
use torque_server::server::Server;

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
/// face of the profile's name (Windows' fonts folder, or a browser's fonts); with neither, text is not drawn.
pub fn draw_cache() -> DrawCache {
    DrawCache::new(Box::new(layout_fonts()))
}

/// The fonts the GUI's layout measures text with: the same faces [`draw_cache`] draws with.
pub fn layout_fonts() -> FontCache {
    FontCache::new("fonts", system_fallback())
}
