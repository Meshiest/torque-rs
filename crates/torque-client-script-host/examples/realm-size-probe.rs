//! What the `wasmtime` feature adds to a release binary: built with it, this compiles, links and starts the Lua module named
//! on its command line through the whole runner; built without it, it does nothing. `realm-gate` builds both and
//! compares their sizes.

fn main() {
    #[cfg(feature = "wasmtime")]
    if let Some(path) = std::env::args().nth(1) {
        use torque_client_script_host::{GuestEngine, GuestLimits, LuaGuest};
        let run = || -> Result<(), String> {
            let bytes = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
            let engine =
                GuestEngine::new(std::time::Duration::from_millis(5)).map_err(|e| e.to_string())?;
            let guest = LuaGuest::new(&engine, &bytes).map_err(|e| e.to_string())?;
            let limits = GuestLimits {
                memory: 64 << 20,
                deadline: std::time::Duration::from_secs(1),
            };
            let module = guest.instantiate(limits).map_err(|e| e.to_string())?;
            let plugin = torque_plugin_loader::web_lua::WebLuaPlugin::new(module);
            drop(plugin);
            Ok(())
        };
        match run() {
            Ok(()) => println!("{path}: started"),
            Err(error) => eprintln!("{error}"),
        }
    }
}
