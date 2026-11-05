//! A singleplayer world: a menu of cube maps, then the asset kit's box figure walking and jumping on the map's cubes, a server and
//! a client of the engine joined in one process.
//!
//! - [`files`]: the kit and the game folder (the scripts, the maps).
//! - [`levels`]: maps authored as Blockbench models, built into interiors when the example starts.
//! - [`server`]: torque-server's main loop over TGE's game classes, with a network host on an in-memory transport.
//! - [`client`]: the client host, the server of the map being played, the link between them, and a frame for the world view.
//! - [`scene`]: the client's ghosts as the world view's objects (the cubes and the figures).
//! - [`shots`]: the example without a window, drawn offscreen: pictures of the menu and of every map.
//! - `web` (built for the web only): the example in a browser, its files carried inside it.
//!
//! The window is the engine's ([`torque::app::window::run`]).

pub mod client;
pub mod files;
pub mod levels;
pub mod scene;
pub mod server;
pub mod shots;
#[cfg(target_arch = "wasm32")]
pub mod web;
