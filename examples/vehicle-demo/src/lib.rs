//! A drive for one player or a few on the same network: the asset kit's box figure beside a dune buggy on green checkerboard
//! hills, a click on the buggy to take a seat (the driver's or the passenger's), the buggy driven over the hills, E to exit.
//!
//! - [`files`]: the kit and the game folder (the scripts, the map), and the map's terrain.
//! - [`seats`]: the demo's Player: a driver's jump trigger brakes the buggy, and no key of a move dismounts; [`seats::SeatsPlugin`] gives
//!   a server those rules.
//! - [`server`]: torque-server's main loop over TGE's game classes, with a network host on a UDP socket (and for
//!   `vehicle-demo-server` the web beside it, or a room).
//! - [`client`]: the client host on its own UDP socket, the server it hosts, the keys in a seat and a frame for the view.
//! - [`scene`]: the client's ghosts as the world view's objects (the figures and the buggy with its wheels).
//! - [`shots`]: the example without a window, drawn offscreen: pictures of the buggy, its cockpit, two players seated, a
//!   passenger's view, the hill and the start.
//! - [`cloud_shots`]: the sky's clouds through the day, drawn offscreen with the engine's clouds and with the demo's.
//! - [`web`]: the example in a browser, its files fetched by the page: played alone, joined to a server or a player's host,
//!   or hosted with its server in the page's worker.
//! - [`showcase`]: the buggy in a turn of the race track, the world alone at the highest quality.
//!
//! The game is made by an engine of the demo's plugins ([`client::DemoPlugins`]), which also starts the servers it hosts; the
//! window is the engine's runner ([`torque::engine::Engine::run`]).

pub mod client;
pub mod cloud_shots;
pub mod files;
pub mod scene;
pub mod seats;
pub mod server;
pub mod shots;
pub mod showcase;
pub mod web;
