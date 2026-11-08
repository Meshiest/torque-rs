//! The browser build's Lua interface, tested natively: a module written in Rust stands in for the page's Luau module, and
//! the stand-in for a page without one says so.

use torque_engine::{ClientStage, Engine, EngineBuilder, ServerStage, Side};
use torque_plugin_loader::web_lua::{HostError, LuaHost, LuaModule, WebLuaPlugin};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::{Vm, VmError};

/// A request's result as the module hands it on: a refusal is the text `refused`, a limit stops the run.
fn stop(result: Result<Vec<u8>, HostError>) -> Result<Vec<u8>, VmError> {
    match result {
        Ok(text) => Ok(text),
        Err(HostError::Refused) => Ok(b"refused".to_vec()),
        Err(HostError::Stopped(error)) => Err(error),
    }
}

/// One command per line: `call <function> <args...>`, `define <name> <id>`, `read <path>`, `spin`, `realm`; results go
/// into `$Web::Result`.
struct Commands;

impl LuaModule for Commands {
    fn run_file(&self, host: &mut dyn LuaHost, _path: &str, source: &[u8]) -> Result<(), VmError> {
        for line in String::from_utf8_lossy(source).lines() {
            let mut words = line.split_whitespace();
            let result: Vec<u8> = match words.next() {
                Some("call") => {
                    let name = words.next().unwrap_or_default();
                    let args: Vec<Vec<u8>> = words.map(|w| w.as_bytes().to_vec()).collect();
                    stop(host.call_function("", name, &args))?
                }
                Some("define") => {
                    let name = words.next().unwrap_or_default();
                    let id = words.next().and_then(|w| w.parse().ok()).unwrap_or(0);
                    let package = host.package().to_string();
                    let ok = host.define_function("", name, &package, "webEcho(...)", id);
                    if ok {
                        b"defined".to_vec()
                    } else {
                        b"refused".to_vec()
                    }
                }
                Some("read") => host
                    .read_script(words.next().unwrap_or_default())?
                    .unwrap_or_else(|| b"refused".to_vec()),
                Some("spin") => loop {
                    host.charge(1000)?;
                },
                Some("realm") => {
                    let (realm, side) = host.realm();
                    format!("{side:?} {}", realm != 0).into_bytes()
                }
                _ => continue,
            };
            assert!(host.set_global("$Web::Result", &result));
        }
        Ok(())
    }

    fn call_defined(
        &self,
        host: &mut dyn LuaHost,
        id: u64,
        args: &[Vec<u8>],
    ) -> Result<Vec<u8>, VmError> {
        if id == 2 {
            // Down again through TorqueScript.
            return stop(host.call_function("", "tsDown", args));
        }
        let args: Vec<String> = args
            .iter()
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect();
        Ok(format!("module {id}: {}", args.join(",")).into_bytes())
    }
}

fn engine(plugin: WebLuaPlugin) -> Engine {
    EngineBuilder::new().add_plugins(plugin).build().unwrap()
}

fn server(engine: &Engine, files: &MemFiles) -> Vm {
    let mut vm = Vm::new();
    vm.set_file_host(files.clone());
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut vm).unwrap();
    }
    vm
}

fn result(vm: &Vm) -> String {
    String::from_utf8_lossy(&vm.global("$Web::Result").unwrap_or_default()).into_owned()
}

fn run(vm: &mut Vm, files: &MemFiles, source: &str) -> Result<bool, VmError> {
    files.insert("test.lua", source.as_bytes());
    vm.exec_file(b"test.lua", false)
}

#[test]
fn a_module_runs_lua_files_and_calls_the_engine_back() {
    let engine = engine(WebLuaPlugin::new(Commands));
    let files = MemFiles::new();
    files.insert("lib/mod.lua", b"return 1");
    files.insert("lib/mod.cs", b"script");
    let mut vm = server(&engine, &files);
    assert_eq!(run(&mut vm, &files, "call strupr abc"), Ok(true));
    assert_eq!(result(&vm), "ABC");
    assert_eq!(run(&mut vm, &files, "realm"), Ok(true));
    assert_eq!(result(&vm), format!("{:?} true", Side::Server));
    assert_eq!(run(&mut vm, &files, "define webEcho 5"), Ok(true));
    assert_eq!(vm.eval(b"return webEcho(1, 2);").unwrap(), b"module 5: 1,2");
    for (path, expected) in [
        ("lib/mod.lua", "return 1"),
        ("lib/mod.cs", "refused"),
        ("../lib/mod.lua", "refused"),
        ("lib/none.lua", "refused"),
    ] {
        assert_eq!(run(&mut vm, &files, &format!("read {path}")), Ok(true));
        assert_eq!(result(&vm), expected, "{path}");
    }
    let mut client = Vm::new();
    client.set_file_host(files.clone());
    for &stage in ClientStage::ALL {
        engine.replay(stage, &mut client).unwrap();
    }
    assert_eq!(client.exec_file(b"test.lua", false), Ok(true));
}

#[test]
fn a_module_is_held_to_the_step_limit_and_the_call_depth() {
    let engine = engine(WebLuaPlugin::new(Commands));
    let files = MemFiles::new();
    let mut vm = server(&engine, &files);
    vm.set_step_limit(Some(100_000));
    assert_eq!(run(&mut vm, &files, "spin"), Err(VmError::StepLimit));
    vm.set_step_limit(None);
    assert_eq!(run(&mut vm, &files, "spin"), Err(VmError::StepLimit));
    vm.eval(b"function tsDown(%n) { if (%n <= 0) return \"bottom\"; return webDown(%n - 1); }")
        .unwrap();
    assert_eq!(run(&mut vm, &files, "define webDown 2"), Ok(true));
    assert_eq!(vm.eval(b"return tsDown(5);").unwrap(), b"bottom");
    assert_eq!(vm.eval(b"return tsDown(100000);"), Err(VmError::CallDepth));
}

#[test]
fn without_a_module_a_lua_file_says_it_cannot_run() {
    let engine = engine(WebLuaPlugin::default());
    let files = MemFiles::new();
    let mut vm = server(&engine, &files);
    assert_eq!(run(&mut vm, &files, "call strupr abc"), Ok(true));
    let log: Vec<String> = vm.log_texts();
    assert!(
        log.iter()
            .any(|l| l.contains("Lua needs the page's Lua module")),
        "{log:?}"
    );
    assert_eq!(result(&vm), "");
}

#[test]
fn the_host_makes_the_c_abi_s_checks() {
    let engine = engine(WebLuaPlugin::new(Commands));
    let files = MemFiles::new();
    let mut vm = server(&engine, &files);
    // Refused, not run: a name that is not one, and what the call policy refuses (the console says why).
    for (line, why) in [
        ("call bad-name", ""),
        ("call exec main.cs", "from a script language"),
    ] {
        assert_eq!(run(&mut vm, &files, line), Ok(true), "{line}");
        assert_eq!(result(&vm), "refused", "{line}");
        let log = vm.log_texts().join(
            "
",
        );
        assert!(log.contains(why), "{line}: {log}");
    }
    assert_eq!(
        String::from_utf8_lossy(&vm.global("$Pref::Lua::MemoryLimit").unwrap_or_default()),
        "64",
        "the memory cap has its default"
    );
}
