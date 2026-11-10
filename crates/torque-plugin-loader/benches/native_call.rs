//! What a call across the C ABI costs: the sample extension's `Sample::add(2, 3)` loaded from its library and the same
//! native compiled in (the difference is the bridge's cost per call), then calls from the extension back into the
//! engine (a function, field writes and reads). Run it with `just bench-script`, or
//! `cargo bench -p torque-plugin-loader --bench native_call`.
//!
//! Each line is the best of a few runs, in nanoseconds per call.

#[path = "../tests/common/mod.rs"]
mod common;

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use common::{CompiledSample, library_file_name, loaded_engine, scratch, server_vm, user_of};
use torque_engine::EngineBuilder;
use torque_ts_vm::vm::Vm;

const CALLS: usize = 100_000;
const RUNS: usize = 5;

/// The sample extension's library built in release with its bench natives, in a build folder of its own.
fn bench_library() -> PathBuf {
    let manifest = common::workspace().join("examples/native-extension/Cargo.toml");
    let target = common::target_dir().join("native-extension-bench");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--quiet", "--release", "--features", "bench"])
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--target-dir")
        .arg(&target)
        .status()
        .expect("cargo runs");
    assert!(status.success(), "the sample extension builds");
    target
        .join("release")
        .join(library_file_name("native_sample"))
}

/// A server script engine that has the bench library loaded.
fn loaded_vm(library: &PathBuf) -> Vm {
    let dir = scratch("native-call-bench");
    std::fs::copy(library, dir.join(library_file_name("native_sample"))).expect("copies");
    std::fs::write(dir.join("native_sample.toml"), common::sample_manifest()).expect("writes");
    let engine = loaded_engine(&dir);
    let vm = server_vm(&engine);
    let _ = std::fs::remove_dir_all(user_of(&dir));
    vm
}

const PRELUDE: &str = "
function add(%a, %b) { return %a + %b; }
function viaNative(%n) { %s = 0; for (%i = 0; %i < %n; %i++) %s += Sample::add(2, 3); return %s; }
function viaScript(%n) { %s = 0; for (%i = 0; %i < %n; %i++) %s += add(2, 3); return %s; }
function backCalls(%n) { return Sample::callBack(%n); }
function fieldCalls(%o, %n) { return Sample::fieldBack(%o, %n); }
";

/// Best of `RUNS` of `vm.invoke(function, [CALLS])`, in nanoseconds per call, and the result.
fn time(vm: &mut Vm, function: &[u8], extra: &[Vec<u8>]) -> (f64, Vec<u8>) {
    let mut args = extra.to_vec();
    args.push(CALLS.to_string().into_bytes());
    let mut best = f64::MAX;
    let mut result = Vec::new();
    for _ in 0..RUNS {
        let start = Instant::now();
        result = vm.invoke(None, function, &args).expect("runs");
        best = best.min(start.elapsed().as_secs_f64() * 1e9 / CALLS as f64);
    }
    (best, result)
}

/// Stops with the console's lines when a native did not do `CALLS` calls.
fn check(vm: &mut Vm, result: &[u8]) {
    if result != CALLS.to_string().as_bytes() {
        let lines: Vec<String> = vm
            .drain_log()
            .iter()
            .take(5)
            .map(|l| String::from_utf8_lossy(&l.text).into_owned())
            .collect();
        panic!("the native stopped early: {lines:?}");
    }
}

fn main() {
    let library = bench_library();
    let mut loaded = loaded_vm(&library);
    let compiled_engine = EngineBuilder::new()
        .add_plugins(CompiledSample)
        .build()
        .expect("the engine builds");
    let mut compiled = server_vm(&compiled_engine);
    for vm in [&mut loaded, &mut compiled] {
        vm.eval(PRELUDE.as_bytes()).expect("the prelude runs");
    }

    let (script, a) = time(&mut loaded, b"viaScript", &[]);
    let (native_loaded, b) = time(&mut loaded, b"viaNative", &[]);
    let (native_compiled, c) = time(&mut compiled, b"viaNative", &[]);
    assert_eq!(a, b);
    assert_eq!(b, c);
    println!("script call (loop + add)        {script:>8.0} ns/call");
    println!("Sample::add loaded              {native_loaded:>8.0} ns/call");
    println!("Sample::add compiled in         {native_compiled:>8.0} ns/call");
    println!(
        "bridge (loaded - compiled in)   {:>8.0} ns/call",
        native_loaded - native_compiled
    );

    let (back, result) = time(&mut loaded, b"backCalls", &[]);
    check(&mut loaded, &result);
    println!("back-call add(3, 4)             {back:>8.0} ns/call");

    loaded
        .eval(b"$bench_object = new ScriptObject();")
        .expect("object");
    let id = loaded.global("$bench_object").expect("the object's id");
    let (fields, result) = time(&mut loaded, b"fieldCalls", &[id]);
    check(&mut loaded, &result);
    println!("field write + read              {fields:>8.0} ns/call");
}
