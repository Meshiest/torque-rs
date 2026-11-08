//! What the Lua tests share: an engine with the Lua extension linked in (and local mods), server and client script engines
//! made the way the engine's hosts make them, and the console.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use torque_engine::{ClientStage, Engine, EngineBuilder, ServerStage};
use torque_mods::ModsPlugin;
use torque_plugin_loader::ExtensionPlugin;
use torque_plugin_sdk::abi::TORQUE_ABI_VERSION;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::{Vm, VmError};

/// The Lua extension, linked in.
pub fn lua() -> ExtensionPlugin {
    ExtensionPlugin::from_init(TORQUE_ABI_VERSION, torque_lua::INIT).expect("the extension starts")
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

/// TorqueScript text run with `eval`, its result as text.
pub fn eval(vm: &mut Vm, code: &str) -> Result<String, VmError> {
    vm.eval(code.as_bytes())
        .map(|r| String::from_utf8_lossy(&r).into_owned())
}

/// The console's lines since the last call, one string.
pub fn console(vm: &mut Vm) -> String {
    vm.drain_log()
        .into_iter()
        .map(|l| String::from_utf8_lossy(&l.text).into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A global's value as text.
pub fn global(vm: &Vm, name: &str) -> String {
    String::from_utf8_lossy(&vm.global(name).unwrap_or_default()).into_owned()
}

/// The folder of the shared Lua test scripts (the same files a web build's tests run).
pub fn scripts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("scripts")
}

/// The shared test script `name`.
pub fn script(name: &str) -> String {
    std::fs::read_to_string(scripts().join(name)).expect("the test script is there")
}
