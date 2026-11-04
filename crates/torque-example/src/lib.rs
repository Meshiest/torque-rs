//! A dedicated server and a client built on the `torque` umbrella crate, which connect over the loopback address.
//!
//! - [`game`]: the example game's scripts (`game/example/main.cs` and its mission), loaded into a script engine with TGE's game
//!   classes declared.
//! - [`view`]: the server's script objects as the ghost layer reads them.
//! - [`server`]: the dedicated server: the transport, the handshake, and per client the event queue and the ghost manager over
//!   TGE's record codecs.
//! - [`client`]: the client: the handshake, the packets, and the client's world of ghosts over TGE's client set.
//! - `window` (feature `window`): the client in a window, its ghosts drawn by a client host's GUI.
//!
//! `cargo run -p torque-example --bin example-server` and, beside it, `cargo run -p torque-example --bin example-client` (or
//! `cargo run -p torque-example --features window --bin example-window`, which with `--local` runs the server itself).

pub mod client;
pub mod game;
pub mod server;
pub mod view;
#[cfg(feature = "window")]
pub mod window;
