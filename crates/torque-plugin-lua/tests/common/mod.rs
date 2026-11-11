//! What the Lua tests share: an engine with the Lua extension linked in (and local mods), server and client script engines
//! made the way the engine's hosts make them, and the console.
//!
//! The web check (`tests/web/lua-check`, `just lua-web-check`) builds the same cases for a browser, where [`lua`] is the
//! page's Lua module instead (`WebLuaPlugin`). With `cfg(torque_lua_module)` (the crate building them sets it, as the
//! Wasmtime gate's example in `torque-client-script-host` does) they are a native build's cases against a Lua module too:
//! listed in `CASES`, with [`lua`] the building crate's `crate::lua_module()`.
#![allow(dead_code)]

use torque_engine::{ClientStage, Engine, EngineBuilder, ServerStage};
use torque_mods::ModsPlugin;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::{Vm, VmError};

/// Test cases: each a `#[test]` natively; on the web (or against a Lua module), functions listed in `CASES` by name, for
/// the page (or the gate) to run one by one.
#[cfg(not(any(target_arch = "wasm32", torque_lua_module)))]
#[allow(unused_macros)]
macro_rules! cases {
    ($($(#[$meta:meta])* fn $name:ident() $body:block)*) => {
        $( $(#[$meta])* #[test] fn $name() $body )*
    };
}

#[cfg(any(target_arch = "wasm32", torque_lua_module))]
#[allow(unused_macros)]
macro_rules! cases {
    ($($(#[$meta:meta])* fn $name:ident() $body:block)*) => {
        $( $(#[$meta])* pub fn $name() $body )*
        /// Every case, by name.
        pub const CASES: &[(&str, fn())] = &[$((stringify!($name), $name)),*];
    };
}

/// The Lua extension, linked in.
#[cfg(not(any(target_arch = "wasm32", torque_lua_module)))]
pub fn lua() -> torque_plugin_loader::ExtensionPlugin {
    use torque_plugin_sdk::abi::TORQUE_ABI_VERSION;
    torque_plugin_loader::ExtensionPlugin::from_init(TORQUE_ABI_VERSION, torque_lua::INIT)
        .expect("the extension starts")
}

/// The page's Lua module, over its bridge.
#[cfg(target_arch = "wasm32")]
pub fn lua() -> torque_plugin_loader::web_lua::WebLuaPlugin {
    torque_plugin_loader::web_lua::WebLuaPlugin::default()
}

/// A Lua module the crate building the cases makes (natively, the Wasmtime gate's).
#[cfg(all(not(target_arch = "wasm32"), torque_lua_module))]
pub fn lua() -> torque_plugin_loader::web_lua::WebLuaPlugin {
    crate::lua_module()
}

/// Time since it was started, in seconds (`std::time` has no clock on the web).
pub struct Stopwatch(f64);

impl Stopwatch {
    pub fn start() -> Stopwatch {
        Stopwatch(now())
    }

    pub fn seconds(&self) -> f64 {
        now() - self.0
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn now() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

#[cfg(target_arch = "wasm32")]
fn now() -> f64 {
    js_sys::Date::now() / 1000.0
}

/// An engine with local mods and the Lua extension.
pub fn engine() -> Engine {
    EngineBuilder::new()
        .add_plugins((ModsPlugin::new(), lua()))
        .build()
        .expect("the engine builds")
}

/// A server's script engine of `engine`, the way a host makes it: the files in at `Prepare`, the entry script `main.cs`
/// (when the files have one) before `Entry`.
pub fn server(engine: &Engine, files: &MemFiles) -> Vm {
    let mut vm = Vm::new();
    for &stage in ServerStage::ALL {
        if stage == ServerStage::Prepare {
            vm.set_file_host(files.clone());
        }
        if stage == ServerStage::Entry {
            vm.exec_file(b"main.cs", false)
                .expect("the entry script runs");
        }
        engine.replay(stage, &mut vm).expect("the stage replays");
    }
    vm
}

/// A client's script engine of `engine`, as [`server`].
pub fn client(engine: &Engine, files: &MemFiles) -> Vm {
    let mut vm = Vm::new();
    for &stage in ClientStage::ALL {
        if stage == ClientStage::Game {
            vm.set_file_host(files.clone());
        }
        if stage == ClientStage::Entry {
            vm.exec_file(b"main.cs", false)
                .expect("the entry script runs");
        }
        engine.replay(stage, &mut vm).expect("the stage replays");
    }
    vm
}

/// Files in memory, with an empty `main.cs`.
pub fn files(entries: &[(&str, &[u8])]) -> MemFiles {
    let mem = MemFiles::new();
    mem.insert("main.cs", b"");
    for (path, bytes) in entries {
        mem.insert(path, bytes);
    }
    mem
}

/// Writes `source` as the file `path` and runs it with `exec`.
pub fn run(vm: &mut Vm, files: &MemFiles, path: &str, source: &str) -> Result<bool, VmError> {
    files.insert(path, source.as_bytes());
    vm.exec_file(path.as_bytes(), false)
}

/// The shared test script `name` (`tests/scripts`), built in: the web check has no files to read it from.
pub fn script(name: &str) -> String {
    match name {
        "basics.lua" => include_str!("../scripts/basics.lua"),
        "sandbox.lua" => include_str!("../scripts/sandbox.lua"),
        _ => panic!("no test script {name}"),
    }
    .to_string()
}
