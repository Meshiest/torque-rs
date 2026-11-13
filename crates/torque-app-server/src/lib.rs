//! The server half of a game made on the engine: its files, a server, the terrain and water blocks, and the plugins and
//! runner that start a dedicated server. Nothing here draws, plays sound or opens a window, so a dedicated server builds
//! without the drawing, GUI, audio and window crates. `torque-app` re-exports every module as its own
//! (`torque_app::server`, `torque_app::files`, ...).
//!
//! - [`files::GameFiles`]: a game folder read once into memory, served to the scripts as their files.
//! - [`manifest::Manifest`]: the list of a web build's files (`files.json`: paths, sizes, hashes) and its caps.
//! - [`server::GameServer`]: the server's main loop with TGE's game classes and a network host, on a UDP socket or an in-memory
//!   link: started, ticked, called, its console drained; [`server::GameServer::run_dedicated`] paces a dedicated one.
//! - [`net_game`]: the net game a server's network host speaks (`GameServer<G>`, Torque's by default), the codecs, and
//!   [`net_game::RunServer`], what a runner drives of a server of any net game.
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
//! - [`slots`]: [`slots::ServerSlots`], the engine builder's calls for a server's seams: hooks (a list), the mission loader,
//!   the server game, the net game and the codecs (each exclusive); and [`slots::HostSlots`], what every server and client is
//!   made of and how it starts: the script engine, the server's entry and the client's (each exclusive).
//!
//! # Maintaining
//!
//! **Test support** lives in `testing` (feature `testing`, dev-dependencies only): `TestNet` and `TestConn`, the net game and
//! conn game the tests here and torque-trace-hash's wire goldens run on.
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
//!
//! **The host slots are exclusive and default to what a TGE game does** ([`slots::HostSlots`]). The script engine comes from
//! `slots::make_script_engine` (the slot's factory, else `Vm::new`) first thing in `GameServer::start_prepared` and in torque-app's
//! `ClientBuilder::from_engine_files`, so every recipe runs on it. The files reach both bodies as one `slots::FileSetter`, so the
//! constructors over any `FileHost` (`start_engine_with_files`, `from_engine_with_files`) do not copy either body per file
//! host; the `MemFiles` constructors are unchanged. The server's entry slot replaces `run_entry_script` (the entry script, the
//! `Entry` stage, the start function) and the turning on of connections; the `Entry` stage then replays after the game's entry,
//! and connections stay as the game left them: the server turns them off just before the game's entry, and
//! `slots::Connections` (a weak handle on the server's state, a no-op once it is gone or borrowed) turns them on. The client's
//! entry slot replaces only the entry script in `exec_entry`. An engine with none of them builds the same server and client as
//! before, which the wire goldens and native snapshots check. Tests are `script_engine_*`, `script_files_*` and `server_entry_*` in
//! `slots_tests.rs`, and the client's in torque-app's `host_slots_tests.rs`.
//!
//! **A net game is a type parameter, erased only where it is crossed once** ([`net_game`]). `GameServer<G>`,
//! `ServerWiring::new::<G>` and `net_game::attach::<G>` are the typed side; the engine's slot keeps the factory as
//! `Box<dyn Any>` with the type's name and a `fn` that starts `GameServer<G>` as a `Box<dyn RunServer>` (what the dedicated
//! runner calls), so nothing per packet or object is dynamic and Torque's server is the same code as before. The flow:
//! `start_prepared` resolves the factory first (`factory_for`: a wrong type is refused before a socket opens), builds the
//! untyped wiring with two closures over the typed state (`tge_game`, `attach_net`), runs the stages as before, sets the
//! codecs slot's codecs just before the `Net` stage (`apply_codecs`), and takes what `NetHostPlugin`'s recipe attached from a
//! cell it shares with the wiring. `ServerGamePlugin`'s recipe calls the server game slot's factory, else TGE's install, so
//! the recipes keep their stages and order (the native snapshots and the wire goldens check that). Invariants and tests
//! (`net_game_tests.rs`, `cargo test -p torque-app-server --lib -- net_game_`): a server of another net game than the slot's
//! is refused naming both, and an engine with no slot starts only `TorqueNet`; a second net game, codecs or server game setter
//! fails the build naming both; the factory runs once per server over the server game's parts and its payload wrapper gets
//! Torque's event layer; the dedicated runner starts the slot's net game; plugin net classes that the ghost game or codecs do
//! not carry are warned about at start. torque-trace-hash's `net_game_*` wire tests hold a net game made of Torque's parts to
//! the same goldens. Extension points: a new per-server part of the host goes on `NetGameParts` (a field with Torque's value
//! as `TorqueNetFactory`'s); a runner that must start any net game calls `net_game::start_net_game`; a method a runner needs goes
//! on `RunServer`.
//!
//! **A server can start with no network host** ([`server::Listen::Late`]) and get it from [`server::GameServer::attach_net`].
//! `start_prepared` then opens no transport: the wiring has none, so `NetHostPlugin`'s recipe marks the host as waiting
//! instead of attaching it (the rest of the `Net` stage runs as usual, the script registry's entries with it), the codecs are
//! not set, and `shared`, `transport` and `end` stay empty while `LateHost` keeps the engine, the wiring and the attached
//! cell. `attach_net` checks the server's rule first (`Server::can_attach_net`: no host yet, no datablock, a real socket only on
//! the system clock) and, in the engine's protocol, refuses a real plain socket bound outside the private addresses as a
//! start on `Listen::Udp` does (`check_plain_engine_bind`; a validated link may bind anywhere). Then it sets the codecs, runs
//! `ServerWiring::attach_late` (the net game's factory and `attach_host`) and `wire_host`, the same function `start_prepared`
//! calls after the `Net` stage, so the two paths cannot drift. `wire_host` attaches the host's passes after its last
//! fallible step, so a failure at any step leaves a server whose transport is never read or written; the host's natives and
//! extensions that a failed attach set stay, `LateHost` is kept, the recipe stays waiting, and a later `attach_net` makes the
//! host again from the factory up. It keeps `allowConnections` as the start left it, also on failure (`attach_host` turns it
//! off), and hands the gate the required mods the entry loaded. A server started on any other `Listen` takes the same path
//! as before, which the native snapshots and the wire goldens check. Tests are `late_net_*` in `late_net_tests.rs` (no host's
//! natives before, a normal server's after, each refusal, a failed step that reads nothing, a retry after a failed factory,
//! `start_with_net_files`) and torque-trace-hash's `tests/late_net.rs` (a client's session writes the same datagrams as
//! against a server started on the link, in both protocols): `cargo test -p torque-app-server --lib -- late_net_` and
//! `cargo test -p torque-trace-hash --test main late_net::`. A new part of the host goes in `wire_host` before the passes, never in
//! one of its callers.
//!
//! **A [`server::GameServer`] is a server host** (`torque_server::server::GameServer`), so torque-app's
//! `ClientBuilder::over_server` can make a client on its script engine: `run_ticks` is `Server::run_on` over the server it
//! holds, the same main loop as [`server::GameServer::tick`] with the client's work before each tick. Nothing in this crate
//! calls it, so a server alone ticks as before (torque-app's `client_over_server_tests.rs`).
//!
//! **The QUIC link is one more transport** (`quic` feature, native; [`transports::QuicServerTransportPlugin`], id `quic`,
//! for [`server::Listen::Quic`]): `torque-net-quic`'s server endpoint under the same bind policy as UDP, on the identity key
//! [`server::QuicListen::identity`] names (by default `identity.key` in the server's folder under the user's data folder,
//! [`server::identity_path`], made on first start). A public bind is allowed there, since the link is encrypted and validates
//! addresses; `plain_engine_refusal` stays what it was for plain UDP. Nothing of it reaches a browser build (torque-app's
//! `tests/quic_link.rs`; `cargo tree -p torque-trace-hash --target wasm32-unknown-unknown` has no `quinn-proto`). In the
//! engine's protocol it is what UDP means: `start_prepared` opens `server::engine_link(engine, listen)`, which turns
//! `Listen::Udp` (and UDP beside the web) into the QUIC link with `QuicListen::lan_list` on, before the plain-bind refusal
//! looks, so the refusal still holds wherever the QUIC transport is off (`transports_tests::udp_in_the_engines_protocol_is_the_quic_link_under_the_same_bind_policy`,
//! `a_public_bind_of_plain_engine_udp_is_refused`; torque-app's `quic_link` tests for both ends).

pub mod cli;
pub mod config;
pub mod files;
pub mod manifest;
pub mod net_game;
pub mod plugins;
pub mod run;
#[cfg(all(feature = "p2p", not(target_arch = "wasm32")))]
pub mod serve;
pub mod server;
pub mod slots;
pub mod terrain;
#[cfg(any(test, feature = "testing"))]
#[doc(hidden)]
pub mod testing;
pub mod transports;
pub mod uploads;
pub mod water;

#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod engine_tickets_tests;
#[cfg(test)]
mod late_net_tests;
#[cfg(test)]
mod net_game_tests;
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
