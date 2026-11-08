//! The Wasmtime gate: whether the Lua extension's WebAssembly module (`just lua-web`, the one a browser build runs) runs
//! natively under Wasmtime, and what that costs.
//!
//! Usage: `realm-gate <torque_lua.wasm> [--cold]` (the justfile's `realm-gate` recipe).
//!
//! It runs the Lua extension's shared cases (`crates/torque-plugin-lua/tests/lua.rs`, the cases `just lua-web-check` runs
//! under Node and in Chrome) with the engine's Lua being a fresh instance of the module under Wasmtime for each, checks
//! that an import-free looping module stops by epoch interruption within twice its deadline, and prints the costs: the
//! module's compile and instantiate times, a host call across the boundary, a Luau step, and what the `wasmtime` feature
//! adds to a release binary (a small probe built with and without it in `<target>/realm-gate`). With `--cold` it
//! also builds the probe both ways from nothing and prints the difference.
//!
//! Exits with a failure when a case fails or the loop is not stopped in time.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use torque_client_script_host::{GuestEngine, GuestLimits, LuaGuest, is_deadline};
use torque_engine::{EngineBuilder, ServerStage};
use torque_mods::ModsPlugin;
use torque_plugin_loader::web_lua::WebLuaPlugin;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::{Level, Vm};
use wasmtime::{Caller, Config, Engine, Instance, Linker, Module, Store};

// Linted where it lives, with the Lua extension's tests.
#[allow(clippy::all, clippy::pedantic)]
#[path = "../../torque-plugin-lua/tests/lua.rs"]
mod lua;

/// The epoch's tick: the deadlines' resolution.
const TICK: Duration = Duration::from_millis(5);
/// The epoch check's deadline (the realm's native backstop).
const DEADLINE: Duration = Duration::from_millis(250);
/// The caps of a case's instance: the cases run realms of up to 64 MiB of Lua each, and runs of millions of steps.
const CASE_LIMITS: GuestLimits = GuestLimits {
    memory: 1 << 30,
    deadline: Duration::from_secs(60),
};
/// The stack of the thread each case runs on (a test thread's).
const CASE_STACK: usize = 2 << 20;

static GUEST: OnceLock<LuaGuest> = OnceLock::new();

/// The module, compiled once.
fn guest() -> &'static LuaGuest {
    GUEST
        .get()
        .expect("the module is compiled before the cases run")
}

/// The shared cases' Lua (`tests/common`'s `lua()`): a fresh instance of the module.
fn lua_module() -> WebLuaPlugin {
    WebLuaPlugin::new(
        guest()
            .instantiate(CASE_LIMITS)
            .expect("the module instantiates"),
    )
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(wasm), cold) = (args.next(), args.next().as_deref() == Some("--cold")) else {
        eprintln!("usage: realm-gate <torque_lua.wasm> [--cold]");
        return ExitCode::from(2);
    };
    match run(Path::new(&wasm), cold) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("realm-gate: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(wasm: &Path, cold: bool) -> Result<bool, String> {
    let bytes = std::fs::read(wasm).map_err(|e| format!("{}: {e}", wasm.display()))?;
    let engine = GuestEngine::new(TICK).map_err(|e| e.to_string())?;
    let mut report = Vec::new();
    report.push(format!(
        "module: {} ({} KiB)",
        wasm.display(),
        bytes.len() / 1024
    ));

    // Without exception handling the module does not compile: it needs them.
    let mut plain = Config::new();
    plain.wasm_exceptions(false);
    let refused = Engine::new(&plain)
        .and_then(|e| Module::new(&e, &bytes))
        .is_err();
    report.push(format!(
        "exceptions: {}",
        if refused {
            "the module needs them (refused with them off), compiled with them on"
        } else {
            "the module compiles without them too"
        }
    ));

    let started = Instant::now();
    let module = Module::new(engine.engine(), &bytes).map_err(|e| format!("compile: {e:#}"))?;
    let compiled = started.elapsed();
    let mut imports = std::collections::BTreeMap::<String, usize>::new();
    for import in module.imports() {
        *imports.entry(import.module().to_string()).or_default() += 1;
    }
    let guest = LuaGuest::link(&engine, module).map_err(|e| format!("link: {e:#}"))?;
    report.push(format!(
        "imports: {}",
        imports
            .iter()
            .map(|(m, n)| format!("{m} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    report.push(format!("module compile: {:.0} ms", ms(compiled)));

    const INSTANCES: u32 = 20;
    let started = Instant::now();
    for _ in 0..INSTANCES {
        drop(
            guest
                .instantiate(CASE_LIMITS)
                .map_err(|e| format!("instantiate: {e:#}"))?,
        );
    }
    report.push(format!(
        "instantiate (and start): {:.2} ms",
        ms(started.elapsed()) / f64::from(INSTANCES)
    ));
    let _ = GUEST.set(guest);

    let (passed, total, failed) = run_cases();
    let cases_ok = passed == total && total > 0;
    report.push(format!(
        "cases: {passed}/{total} passed{}",
        if failed.is_empty() {
            String::new()
        } else {
            format!(" (failed: {})", failed.join(", "))
        }
    ));

    let (stopped, elapsed) = epoch_check(&engine)?;
    let epoch_ok = stopped && elapsed < 2 * DEADLINE;
    report.push(format!(
        "epoch: an import-free loop {} after {:.0} ms (deadline {} ms, limit {} ms): {}",
        if stopped {
            "stopped"
        } else {
            "was not stopped"
        },
        ms(elapsed),
        DEADLINE.as_millis(),
        (2 * DEADLINE).as_millis(),
        if epoch_ok { "pass" } else { "FAIL" }
    ));

    report.push(format!(
        "host call (wasm -> Rust and back): {:.1} ns",
        boundary(&engine)?
    ));
    let (step, round_trip) = lua_costs()?;
    report.push(format!(
        "Luau step (a numeric for loop's iteration): {step:.2} ns"
    ));
    report.push(format!(
        "ts.global from Lua (module -> engine host -> Vm and back), beyond its loop's steps: {round_trip:.0} ns"
    ));

    let work = target_dir().join("realm-gate");
    match sizes(&work.join("probe")) {
        Ok((off, on)) => report.push(format!(
            "release binary: {:.2} MiB without the feature, {:.2} MiB with it: +{:.2} MiB",
            mib(off),
            mib(on),
            mib(on.saturating_sub(off))
        )),
        Err(error) => report.push(format!("release binary: not measured ({error})")),
    }
    if cold {
        match cold_builds(&work) {
            Ok((off, on)) => report.push(format!(
                "cold release build (4 jobs): {:.0} s without the feature, {:.0} s with it: +{:.0} s",
                off.as_secs_f64(),
                on.as_secs_f64(),
                (on.saturating_sub(off)).as_secs_f64()
            )),
            Err(error) => report.push(format!("cold release build: not measured ({error})")),
        }
    } else {
        report.push("cold release build: not measured (run with --cold)".to_string());
    }

    println!("\nrealm gate");
    for line in &report {
        println!("  {line}");
    }
    let ok = cases_ok && epoch_ok;
    println!("  gate: {}", if ok { "pass" } else { "FAIL" });
    Ok(ok)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / f64::from(1 << 20)
}

/// Runs every shared case on a thread of its own: how many passed, of how many, and the names of those that failed.
fn run_cases() -> (usize, usize, Vec<&'static str>) {
    let mut failed = Vec::new();
    for &(name, case) in lua::CASES {
        let started = Instant::now();
        let outcome = std::thread::Builder::new()
            .name(name.to_string())
            .stack_size(CASE_STACK)
            .spawn(case)
            .map(|thread| thread.join().is_ok());
        let ok = outcome.unwrap_or(false);
        println!(
            "case {name}: {} ({:.0} ms)",
            if ok { "ok" } else { "FAILED" },
            ms(started.elapsed())
        );
        if !ok {
            failed.push(name);
        }
    }
    (lua::CASES.len() - failed.len(), lua::CASES.len(), failed)
}

/// Runs a module that loops for ever without an import under [`DEADLINE`]: whether the deadline stopped it, and when.
fn epoch_check(engine: &GuestEngine) -> Result<(bool, Duration), String> {
    let wasm = wat::parse_str(r#"(module (func (export "spin") (loop $again (br $again))))"#)
        .map_err(|e| e.to_string())?;
    let module = Module::new(engine.engine(), wasm).map_err(|e| e.to_string())?;
    let mut store = Store::new(engine.engine(), ());
    let instance = Instance::new(&mut store, &module, &[]).map_err(|e| e.to_string())?;
    let spin = instance
        .get_typed_func::<(), ()>(&mut store, "spin")
        .map_err(|e| e.to_string())?;
    store.set_epoch_deadline(engine.ticks(DEADLINE));
    let started = Instant::now();
    let result = spin.call(&mut store, ());
    let elapsed = started.elapsed();
    Ok((result.as_ref().is_err_and(is_deadline), elapsed))
}

/// What one call from a module to a host function costs: a loop of calls to an import that adds one.
fn boundary(engine: &GuestEngine) -> Result<f64, String> {
    const CALLS: i32 = 20_000_000;
    let wasm = wat::parse_str(
        r#"(module
            (import "host" "next" (func $next (param i32) (result i32)))
            (func (export "run") (param $n i32) (result i32)
              (local $acc i32)
              (block $done
                (loop $again
                  (br_if $done (i32.eqz (local.get $n)))
                  (local.set $acc (call $next (local.get $acc)))
                  (local.set $n (i32.sub (local.get $n) (i32.const 1)))
                  (br $again)))
              (local.get $acc)))"#,
    )
    .map_err(|e| e.to_string())?;
    let module = Module::new(engine.engine(), wasm).map_err(|e| e.to_string())?;
    let mut linker = Linker::new(engine.engine());
    linker
        .func_wrap("host", "next", |_: Caller<'_, ()>, x: i32| {
            x.wrapping_add(1)
        })
        .map_err(|e| e.to_string())?;
    let mut store = Store::new(engine.engine(), ());
    store.set_epoch_deadline(engine.ticks(Duration::from_secs(60)));
    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(|e| e.to_string())?;
    let run = instance
        .get_typed_func::<i32, i32>(&mut store, "run")
        .map_err(|e| e.to_string())?;
    run.call(&mut store, 1000).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let total = run.call(&mut store, CALLS).map_err(|e| e.to_string())?;
    let elapsed = started.elapsed();
    if total != CALLS {
        return Err(format!("the host was called {total} times, not {CALLS}"));
    }
    Ok(elapsed.as_secs_f64() * 1e9 / f64::from(CALLS))
}

/// A server's script engine of an engine with local mods and Lua (`plugin`), made as a host makes it.
fn script_engine(
    plugin: WebLuaPlugin,
    files: &MemFiles,
) -> Result<(torque_engine::Engine, Vm), String> {
    let engine = EngineBuilder::new()
        .add_plugins((ModsPlugin::new(), plugin))
        .build()
        .map_err(|e| format!("{e:?}"))?;
    let mut vm = Vm::new();
    for &stage in ServerStage::ALL {
        if stage == ServerStage::Prepare {
            vm.set_file_host(files.clone());
        }
        engine
            .replay(stage, &mut vm)
            .map_err(|e| format!("{e:?}"))?;
    }
    Ok((engine, vm))
}

/// The nanoseconds of a Luau step, and of a `ts.global` call from Lua beyond the steps of its loop.
fn lua_costs() -> Result<(f64, f64), String> {
    const ITERATIONS: u64 = 20_000_000;
    const CALLS: u64 = 1_000_000;
    let module = guest()
        .instantiate(CASE_LIMITS)
        .map_err(|e| format!("{e:#}"))?;
    let counters = module.counters();
    let files = MemFiles::new();
    let (_engine, mut vm) = script_engine(WebLuaPlugin::new(module), &files)?;
    vm.set_global("$Bench::Value", "42");
    let time = |vm: &mut Vm, name: &str, source: String| -> Result<(Duration, u64), String> {
        files.insert(name, source.as_bytes());
        vm.set_step_limit(Some(u64::MAX / 1024));
        let steps = counters.steps();
        let started = Instant::now();
        let ran = vm.exec_file(name.as_bytes(), false);
        let elapsed = started.elapsed();
        // `exec`'s own line ("Executing ...") is printed at the normal level.
        let log: Vec<_> = vm
            .drain_log()
            .into_iter()
            .filter(|l| l.level != Level::Normal)
            .collect();
        if ran != Ok(true) || !log.is_empty() {
            let log: Vec<_> = log
                .iter()
                .map(|l| String::from_utf8_lossy(&l.text).into_owned())
                .collect();
            return Err(format!(
                "{name} did not run cleanly ({ran:?}): {}",
                log.join(" | ")
            ));
        }
        Ok((elapsed, counters.steps() - steps))
    };
    // Warm up, then measure.
    time(&mut vm, "warm.lua", "for i = 1, 100000 do end".into())?;
    let (elapsed, steps) = time(
        &mut vm,
        "steps.lua",
        format!("local x = 0 for i = 1, {ITERATIONS} do x = x + i end"),
    )?;
    if steps < ITERATIONS / 2 {
        return Err(format!(
            "{ITERATIONS} iterations charged only {steps} steps"
        ));
    }
    let step = elapsed.as_secs_f64() * 1e9 / steps as f64;
    let (elapsed, steps) = time(
        &mut vm,
        "calls.lua",
        format!("local g = ts.global for i = 1, {CALLS} do g('$Bench::Value') end"),
    )?;
    let calls = (elapsed.as_secs_f64() * 1e9 - steps as f64 * step) / CALLS as f64;
    Ok((step, calls))
}

/// The build folder: CARGO_TARGET_DIR's, else the workspace's.
fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
}

/// Builds the size probe in release into `target`, with or without the `wasmtime` feature: the binary's path.
fn build_probe(target: &Path, wasmtime: bool) -> Result<PathBuf, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .args([
            "build",
            "--release",
            "--jobs",
            "4",
            "--example",
            "realm-size-probe",
        ])
        .arg("--manifest-path")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", target)
        .env_remove("RUSTC_WRAPPER");
    if wasmtime {
        command.args(["--features", "wasmtime"]);
    }
    let status = command.status().map_err(|e| format!("cargo: {e}"))?;
    if !status.success() {
        return Err(format!("the probe's build failed ({status})"));
    }
    let name = format!("realm-size-probe{}", std::env::consts::EXE_SUFFIX);
    Ok(target.join("release").join("examples").join(name))
}

/// The probe's release binary without and with the feature, in bytes.
fn sizes(target: &Path) -> Result<(u64, u64), String> {
    let size = |wasmtime| -> Result<u64, String> {
        let probe = build_probe(target, wasmtime)?;
        std::fs::metadata(&probe)
            .map(|m| m.len())
            .map_err(|e| format!("{}: {e}", probe.display()))
    };
    let off = size(false)?;
    let on = size(true)?;
    Ok((off, on))
}

/// The probe's release build from nothing, without and with the feature.
fn cold_builds(work: &Path) -> Result<(Duration, Duration), String> {
    let time = |name: &str, wasmtime| -> Result<Duration, String> {
        let target = work.join(name);
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        }
        let started = Instant::now();
        build_probe(&target, wasmtime)?;
        let elapsed = started.elapsed();
        std::fs::remove_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        Ok(elapsed)
    };
    let off = time("cold-off", false)?;
    let on = time("cold-on", true)?;
    Ok((off, on))
}
