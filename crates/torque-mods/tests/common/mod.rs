//! Shared helpers of the mods' tests: zips, a server script engine over an engine's plugins.
#![allow(dead_code)]

use torque_engine::{Engine, ServerStage};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

/// Files in memory.
pub fn files(entries: &[(&str, &[u8])]) -> MemFiles {
    let mem = MemFiles::new();
    for (path, bytes) in entries {
        mem.insert(path, bytes);
    }
    mem
}

/// The server script engine the way a host makes it: the stages in order, the files in after `Classes`, the game's entry
/// script `entry` (when the files have it) before `Entry`.
pub fn server(engine: &Engine, files: MemFiles, entry: &str) -> Vm {
    let mut vm = Vm::new();
    for &stage in ServerStage::ALL {
        if stage == ServerStage::Prepare {
            vm.set_file_host(files.clone());
        }
        if stage == ServerStage::Entry {
            vm.exec_file(entry.as_bytes(), false)
                .expect("the entry script runs");
        }
        engine.replay(stage, &mut vm).expect("the stage replays");
    }
    vm
}
