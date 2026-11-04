//! A small arena shooter for one player or a few on a local network.
//!
//! - [`files`]: the kit and the game folder (the scripts, the maps).
//! - [`server`]: torque-server's main loop over TGE's game classes, with a network host on a UDP socket.
//! - [`client`]: the client host on its own UDP socket, and the server it hosts.
//! - [`effects`]: the screen's hurt and hit marks.
//! - [`scene`]: the client's ghosts as the world view's objects.
//! - [`shots`]: the example without a window: review pictures drawn offscreen.
//!
//! The window is the engine's ([`torque::app::window::run`]).

pub mod client;
pub mod effects;
pub mod files;
pub mod scene;
pub mod server;
pub mod shots;
