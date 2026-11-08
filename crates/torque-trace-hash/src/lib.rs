//! A determinism harness: scripted 600-tick runs of the predicted paths (the figure on foot, the buggy, the client's
//! prediction and its replay of the server's corrections) hashed into one value per scenario. The value must be the same on
//! every target: native, and wasm32 (`wasm32-wasip1`), which CI builds and compares.
//!
//! - [`hash::TraceHasher`]: the hash; floats go in by their bits.
//! - [`rig::Rig`]: a server (`MinimalPlugins` and a plugin under test) and a client over the in-memory link, stepped by hand.
//! - [`scenarios`]: the scenarios ([`scenarios::all`]) and [`scenarios::trace_hash`], the per-plugin entry point
//!   (`trace_hash::<MyPlugin>()`).
//!
//! The `trace-hash` binary prints `name hash` for every scenario; `just trace-hash` runs it natively and as wasm32-wasip1
//! and fails when the two differ.
//!
//! What a predicted path must not do (the engine's rules for plugins that run on it): iterate a `HashMap` or `HashSet` in an
//! order that reaches the state, read a clock, or use a random number that is not seeded from the state. The harness cannot see
//! those directly; a plugin that does one shows as a hash that changes between runs or between targets.

pub mod hash;
pub mod rig;
pub mod scenarios;
