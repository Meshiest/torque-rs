//! Shared helpers of the mods' tests: zips, a server script engine over an engine's plugins, and a counting-free RNG.
#![allow(dead_code)]

use std::io::Write;

use torque_engine::{Engine, ServerStage};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// A zip of `(name, bytes)` entries, deflated.
pub fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        w.start_file(*name, options).expect("start a zip entry");
        w.write_all(bytes).expect("write a zip entry");
    }
    w.finish().expect("finish the zip").into_inner()
}

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

/// The console's lines so far, one string.
pub fn console(vm: &mut Vm) -> String {
    vm.drain_log()
        .into_iter()
        .map(|l| String::from_utf8_lossy(&l.text).into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

/// `eval` as text.
pub fn eval(vm: &mut Vm, code: &str) -> String {
    String::from_utf8_lossy(&vm.eval(code.as_bytes()).expect("the code runs")).into_owned()
}

/// A small seeded generator (xorshift), so a failing case can be rerun.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}
