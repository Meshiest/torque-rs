//! `torque_lua_module`: the Lua extension's shared cases (crates/torque-plugin-lua/tests), which the gate example builds,
//! run against a Lua module (`web_lua::WebLuaPlugin`) instead of the linked extension, as in a browser build.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(torque_lua_module)");
    println!("cargo::rustc-cfg=torque_lua_module");
}
