//! A determinism harness: scripted 600-tick runs of the predicted paths (the figure on foot, the buggy, the client's
//! prediction and its replay of the server's corrections) hashed into one value per scenario. The value must be the same on
//! every target: native, and wasm32 (`wasm32-wasip1`), which CI builds and compares.
//!
//! - [`hash::TraceHasher`]: the hash; floats go in by their bits.
//! - [`rig::Rig`] and [`rig::Mode`]: a server (`MinimalPlugins` and a plugin under test) and a client over the in-memory link, stepped by hand.
//! - [`rapier`]: the Rapier world's scenario (bodies dropped onto a terrain and an interior; no server or client), printed
//!   among [`scenarios::physics`] and folded into the `physics` line, not `all`.
//! - [`scenarios`]: the scenarios ([`scenarios::all`]) and [`scenarios::trace_hash`], the per-plugin entry point
//!   (`trace_hash::<MyPlugin>()`).
//!
//! The `trace-hash` binary prints `name mode hash` for every scenario (the mode is the network protocol, `tge`); `just trace-hash` runs it natively and as wasm32-wasip1
//! and fails when the two differ.
//!
//! What a predicted path must not do (the engine's rules for plugins that run on it): iterate a `HashMap` or `HashSet` in an
//! order that reaches the state, read a clock, or use a random number that is not seeded from the state. The harness cannot see
//! those directly; a plugin that does one shows as a hash that changes between runs or between targets.

pub mod hash;
pub mod rapier;
pub mod rig;
pub mod scenarios;
