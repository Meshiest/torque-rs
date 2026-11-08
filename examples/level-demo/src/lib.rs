//! A singleplayer world: a menu of cube maps, then the asset kit's box figure walking and jumping on the map's cubes, a server and
//! a client of the engine joined in one process.
//!
//! - [`files`]: the kit and the game folder (the scripts, the maps).
//! - [`ground`]: a map's terrain and water as the client draws them (the valley's).
//! - [`levels`]: maps authored as Blockbench models, built into interiors when the example starts.
//! - [`plugin`]: the game as an engine plugin ([`plugin::LevelPlugin`]) and the engine it runs on.
//! - `ropes` (the `rope` feature): the plaza's clothesline, made by the rope plugin.
//! - [`server`]: torque-server's main loop over TGE's game classes, with a network host on an in-memory transport.
//! - [`client`]: the client host, the server of the map being played, the link between them, and a frame for the world view.
//! - [`scene`]: the client's ghosts as the world view's objects (the cubes and the figures).
//! - [`showcase`]: pictures of the world alone at the highest quality: the valley's views and the front page's pictures.
//! - [`shots`]: the example without a window, drawn offscreen: pictures of the menu and of every map.
//! - `web` (built for the web only): the example in a browser, its files fetched by the page or carried inside it.
//!
//! The window is the engine's ([`torque::app::window::run`]).

use torque::sim::player::PoseData;

pub mod client;
pub mod files;
pub mod ground;
pub mod levels;
pub mod plugin;
#[cfg(feature = "rope")]
pub mod ropes;
pub mod scene;
pub mod server;
pub mod shots;
pub mod showcase;
#[cfg(target_arch = "wasm32")]
pub mod web;

/// The figure's `swimBoundingBox` and `swimForce` (`game/server.cs`).
pub const SWIM_BOX: [f32; 3] = [1.0, 2.3, 1.8];
pub const SWIM_FORCE: f32 = 2400.0;

/// The figure's poses as the level demo's server gives them: the examples' ([`torque_example_common::figure_poses`]), and
/// swimming. A client cannot read a datablock's dynamic fields, so the client's prediction and the drawing of the other
/// figures take them from here; a test holds the two the same.
pub fn figure_poses() -> PoseData {
    PoseData {
        swim: true,
        swim_box: SWIM_BOX,
        swim_force: SWIM_FORCE,
        ..torque_example_common::figure_poses()
    }
}
