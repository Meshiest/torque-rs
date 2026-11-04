//! A singleplayer world: a menu of cube maps, then the asset kit's box figure walking and jumping on the map's cubes, a server and
//! a client of the engine joined in one process.
//!
//! - [`files`]: the kit and the game folder (the scripts, the maps).
//! - [`server`]: torque-server's main loop over TGE's game classes, with a network host on an in-memory transport.
//! - [`client`]: the client host, the server of the map being played, the link between them, and a frame for the world view.
//! - [`scene`]: the client's ghosts as the world view's objects (the cubes and the figures).
//! - [`window`]: the example in a window.
//! - [`shots`]: the example without a window, drawn offscreen: pictures of the menu and of every map.

pub mod client;
pub mod files;
pub mod scene;
pub mod server;
pub mod shots;
pub mod window;
