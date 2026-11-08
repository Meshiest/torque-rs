//! The host of client scripts: Lua that a server sends, run on a player's client inside a WebAssembly guest, natively under
//! Wasmtime as in a browser.
//!
//! What is here today is the runner the realm will be built on, proven against the guest that exists: the Lua extension's
//! WebAssembly module (`crates/torque-plugin-lua`, built for `wasm32-wasip1` by `just lua-web`, the module `web/lua.js` runs
//! in a browser). [`WasmtimeLuaModule`] runs that module's bytes under Wasmtime, with WebAssembly exception handling on (the
//! module's Luau throws C++ exceptions as `exnref`), and is the engine's `web_lua::LuaModule`, so the engine's `LuaHost`
//! makes every check, as it does for the page's module. Its imports are answered in Rust: the module's WASI imports, all
//! denied or minimal as `web/lua.js` answers them (no files, no environment, no arguments, the monotonic clock only,
//! random bytes, standard output and error to this process's standard error), and its `torque` imports, served by the
//! `LuaHost` of the running call.
//!
//! Every call into the module runs under a wall-clock deadline (Wasmtime's epoch interruption, ticked by a thread of the
//! [`GuestEngine`]) and in linear memory capped by the store's limits; a trap of any kind leaves the module broken, and it
//! runs nothing again, as the page's module is.
//!
//! The justfile's `realm-gate` recipe runs the Lua extension's shared cases under it and prints what it costs.
//!
//! Wasmtime comes in only with the `wasmtime` feature (off by default, native only); without it the crate is empty.

#[cfg(all(feature = "wasmtime", not(target_arch = "wasm32")))]
mod guest;

#[cfg(all(feature = "wasmtime", not(target_arch = "wasm32")))]
pub use guest::{Counters, GuestEngine, GuestLimits, LuaGuest, WasmtimeLuaModule, is_deadline};
