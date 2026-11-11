//! The server half of a game made on the engine: its files, a server, the terrain and water blocks, and the plugins and
//! runner that start a dedicated server. Nothing here draws, plays sound or opens a window, so a dedicated server builds
//! without the drawing, GUI, audio and window crates. `torque-app` re-exports every module as its own
//! (`torque_app::server`, `torque_app::files`, ...).
//!
//! - [`files::GameFiles`]: a game folder read once into memory, served to the scripts as their files.
//! - [`manifest::Manifest`]: the list of a web build's files (`files.json`: paths, sizes, hashes) and its caps.
//! - [`server::GameServer`]: the server's main loop with TGE's game classes and a network host, on a UDP socket or an in-memory
//!   link: started, ticked, called, its console drained; [`server::GameServer::run_dedicated`] paces a dedicated one.
//! - [`terrain::Terrain`]: a `TerrainBlock` over a legacy `.ter`: its collision on the server.
//! - [`water::Water`]: a `WaterBlock` as water the Players swim in.
//! - [`plugins`]: the plugins that wire a server's script engine, replayed by [`server::GameServer::start_engine`], and
//!   [`plugins::MinimalPlugins`], the group a dedicated server needs.
//! - [`transports`]: the transports a server listens on (UDP, memory, web, rooms, relay), a plugin each registered by id, and
//!   [`transports::ServerTransportPlugins`], which `MinimalPlugins` has.
//! - [`run`]: [`run::Dedicated`], [`run::Stop`] and [`run::DedicatedRunnerPlugin`], which runs a dedicated server for
//!   [`Engine::run`](torque_engine::Engine::run).
//! - `serve` (the `p2p` feature, natively): a dedicated server's switches for browsers (the web beside UDP, data channels,
//!   a room) and its start-up lines.
//! - [`config`]: `game.toml`, the config a game's folder carries: entry scripts, plugins on and off, the runner.
//! - [`cli`]: the command line's flags and their values.
//! - [`slots`]: [`slots::ServerSlots`], the engine builder's calls for a server's main-loop seams: hooks (a list) and the
//!   mission loader (exclusive).
//!
//! # Maintaining
//!
//! **The main-loop seams hold factories, run once per server** ([`slots`]). `GameServer::start_prepared` calls
//! `slots::make_server_hooks` and `slots::make_mission_loader` (public, so a host of its own applies the same slots) after the
//! `Net` stage and the engine's tick hooks and before the entry script, so what a factory makes can
//! read what the recipes left on the script engine. One entry sets its hooks as they are, several go in a
//! `torque_server::server::HookList` (add order; `pass_slice` is the first answer); the loader slot replaces `ExecMission`, and
//! a second setter fails the build through the engine's exclusive slot. A factory error returns from the start. An engine with
//! no entry builds a server with the plain engine's `NoHooks` and `ExecMission`, so the wire goldens and native snapshots do
//! not move. Tests are `server_hooks_*` and `mission_loader_*` in `slots_tests.rs`
//! (`cargo test -p torque-app-server --lib -- server_hooks_ mission_loader_`).

pub mod cli;
pub mod config;
pub mod files;
pub mod manifest;
pub mod plugins;
pub mod run;
#[cfg(all(feature = "p2p", not(target_arch = "wasm32")))]
pub mod serve;
pub mod server;
pub mod slots;
pub mod terrain;
pub mod transports;
pub mod uploads;
pub mod water;

#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod engine_tickets_tests;
#[cfg(test)]
mod plugins_tests;
#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod slots_tests;
#[cfg(test)]
mod tick_tests;
#[cfg(test)]
mod transports_tests;

pub use files::GameFiles;
pub use server::{GameServer, Listen};
