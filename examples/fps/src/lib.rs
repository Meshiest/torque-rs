//! A small arena shooter for one player or a few on a local network.
//!
//! - [`files`]: the kit and the game folder (the scripts, the maps).
//! - [`server`]: torque-server's main loop over TGE's game classes, with a network host on a UDP socket.
//! - [`client`]: the client host on its own UDP socket, and the server it hosts.
//! - [`effects`]: the screen's hurt and hit marks.
//! - [`hitbox_view`]: a figure's hitboxes drawn as lines over a picture.
//! - [`scene`]: the client's ghosts as the world view's objects.
//! - [`style`]: the arena's own touches on the engine's ghosts (the pistol's kick and sights, the bolts' curve, the shots'
//!   flares).
//! - [`shots`]: the example without a window: review pictures drawn offscreen.
//! - [`showcase`]: a shot fired in the arena seen from the side, the world alone at the highest quality.
//! - `web` (built for the web only): the client in a browser, join-only, over a game socket to the page's server.
//!
//! The window is the engine's ([`torque::app::window::run`]).

pub mod client;
pub mod effects;
pub mod files;
pub mod hitbox_view;
pub mod scene;
pub mod server;
pub mod shots;
pub mod showcase;
pub mod style;
#[cfg(target_arch = "wasm32")]
pub mod web;
#[cfg(target_arch = "wasm32")]
pub mod web_host;
