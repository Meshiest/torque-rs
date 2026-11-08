//! The Lua extension's tests (`crates/torque-plugin-lua/tests/lua.rs` and the shared scripts it runs) built for a browser:
//! the same cases, with the engine's Lua being the page's Lua module (`web_lua::WebLuaPlugin`, over `web/lua.js`) instead
//! of the native extension. `web/lua-check.mjs` runs each case in a fresh instance, under Node and in headless Chrome
//! (`just lua-web-check`). Natively the crate is empty.
#![cfg(target_arch = "wasm32")]

use wasm_bindgen::prelude::*;

// Linted where it lives, with the Lua extension's tests.
#[allow(clippy::all, clippy::pedantic, unexpected_cfgs)]
#[path = "../../../crates/torque-plugin-lua/tests/lua.rs"]
mod lua;

/// A panic's message goes to the console (the module then traps, and the case has failed).
#[wasm_bindgen(start)]
fn start() {
    console_error_panic_hook::set_once();
}

/// The cases, by name, in the order they are written.
#[wasm_bindgen]
pub fn cases() -> Vec<String> {
    lua::CASES
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect()
}

/// Runs the case `name`: false when there is none; a failed case panics.
#[wasm_bindgen(js_name = runCase)]
pub fn run_case(name: &str) -> bool {
    match lua::CASES.iter().find(|(case, _)| *case == name) {
        Some((_, case)) => {
            case();
            true
        }
        None => false,
    }
}
