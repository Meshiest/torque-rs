//! Script languages of extensions, through a toy language written with the SDK: `exec` hands its files to it, it calls
//! the script engine back (nested calls, budgets, depth), defines functions, reads scripts through `exec`'s checks, and
//! hears when a script engine goes.

use std::sync::atomic::{AtomicU64, Ordering};

use torque_engine::{ClientStage, Engine, EngineBuilder, ServerStage};
use torque_plugin_loader::ExtensionPlugin;
use torque_plugin_sdk::abi::TORQUE_ABI_VERSION;
use torque_plugin_sdk::{
    Call, CallError, Definition, Extension, Limit, Registrar, ScriptLanguage, Side,
};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::policy::ExecPolicy;
use torque_ts_vm::vm::{Vm, VmError};

/// How many script engines the toy language heard go.
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// A language of one command per line: `call <function> <args...>` (the result into `$Toy::Result`), `define <name>
/// <id>`, `read <path>` (the text or `refused`), `info`, `spin` (charges until the budget is spent), `defineIn
/// <package> <name> <id>`.
struct Toy;

impl Toy {
    fn line(&self, call: Call<'_>, line: &str) -> Result<(), Limit> {
        let mut words = line.split_whitespace();
        let set = |value: &[u8]| {
            call.set_global("$Toy::Result", value);
        };
        match words.next() {
            Some("call") => {
                let name = words.next().unwrap_or_default();
                let args: Vec<&[u8]> = words.map(str::as_bytes).collect();
                let (namespace, name) = name.rsplit_once("::").unwrap_or(("", name));
                match call.call_function(namespace, name, &args) {
                    Ok(result) => set(&result),
                    Err(CallError::Limit) => return Err(Limit),
                    Err(CallError::Refused(status)) => set(format!("refused {status}").as_bytes()),
                }
            }
            Some(command @ ("define" | "defineIn")) => {
                let package = if command == "defineIn" {
                    words.next().unwrap_or_default().to_string()
                } else {
                    call.script_package()
                };
                let name = words.next().unwrap_or_default();
                let id = words.next().and_then(|w| w.parse().ok()).unwrap_or(0);
                let (namespace, name) = name.rsplit_once("::").unwrap_or(("", name));
                let defined = call.define_function(&Definition {
                    namespace,
                    name,
                    package: &package,
                    usage: "",
                    id,
                });
                set(if defined.is_ok() {
                    b"defined"
                } else {
                    b"refused"
                });
            }
            Some("read") => match call.read_script(words.next().unwrap_or_default()) {
                Ok(text) => set(&text),
                Err(CallError::Limit) => return Err(Limit),
                Err(CallError::Refused(_)) => set(b"refused"),
            },
            Some("info") => {
                let info = call.vm_info().expect("a live call");
                set(format!("{:?} {}", info.side, info.vm != 0).as_bytes());
            }
            Some("spin") => loop {
                call.charge(1000)?;
            },
            _ => {}
        }
        Ok(())
    }
}

impl ScriptLanguage for Toy {
    fn run_file(&self, call: Call<'_>, _path: &str, source: &[u8]) -> Result<(), Limit> {
        for line in String::from_utf8_lossy(source).lines() {
            self.line(call, line)?;
        }
        Ok(())
    }

    fn call_defined(&self, call: Call<'_>, id: u64) -> Result<Vec<u8>, Limit> {
        let args: Vec<String> = (0..call.arg_count())
            .map(|i| String::from_utf8_lossy(call.arg_bytes(i).unwrap_or_default()).into_owned())
            .collect();
        match id {
            // Calls back down: `down <n>` calls the script function `tsDown(n)`.
            7 => {
                let n = args.first().cloned().unwrap_or_default();
                call.call_function("", "tsDown", &[n.as_bytes()])
                    .map_err(|_| Limit)
            }
            // Spins until the budget is spent.
            8 => loop {
                call.charge(1000)?;
            },
            _ => Ok(format!("toy {id}: {}", args.join(",")).into_bytes()),
        }
    }

    fn vm_dropped(&self, _vm: u64) {
        DROPPED.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct ToyExtension;

impl Extension for ToyExtension {
    const ID: &'static str = "test.toy";
    const VERSION: &'static str = "1.0.0";

    fn build(&self, ext: &mut Registrar) {
        ext.script_language(Side::Both, "toy", Toy);
    }
}

torque_plugin_sdk::init_fn!(toy_init, ToyExtension);

fn engine() -> Engine {
    EngineBuilder::new()
        .add_plugins(ExtensionPlugin::from_init(TORQUE_ABI_VERSION, toy_init).expect("it starts"))
        .build()
        .expect("the engine builds")
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
    String::from_utf8_lossy(&vm.global("$Toy::Result").unwrap_or_default()).into_owned()
}

fn run(vm: &mut Vm, files: &MemFiles, source: &str) -> Result<bool, VmError> {
    files.insert("test.toy", source.as_bytes());
    vm.exec_file(b"test.toy", false)
}

#[test]
fn exec_runs_a_file_of_the_language_and_it_calls_scripts_back() {
    let engine = engine();
    let files = MemFiles::new();
    let mut vm = server(&engine, &files);
    assert_eq!(run(&mut vm, &files, "call strupr shout"), Ok(true));
    assert_eq!(result(&vm), "SHOUT");
    assert_eq!(run(&mut vm, &files, "info"), Ok(true));
    assert_eq!(result(&vm), "Server true");
    let mut client = Vm::new();
    client.set_file_host(files.clone());
    for &stage in ClientStage::ALL {
        engine.replay(stage, &mut client).unwrap();
    }
    assert_eq!(client.exec_file(b"test.toy", false), Ok(true));
    assert_eq!(result(&client), "Client true");
}

#[test]
fn defined_functions_call_the_language() {
    let engine = engine();
    let files = MemFiles::new();
    let mut vm = server(&engine, &files);
    assert_eq!(
        run(&mut vm, &files, "define toyEcho 3\ndefine Thing::poke 4"),
        Ok(true)
    );
    assert_eq!(vm.eval(b"return toyEcho(1, 2);").unwrap(), b"toy 3: 1,2");
    vm.eval(b"new ScriptObject(T) { class = Thing; };").unwrap();
    // A method's object comes first, as the script wrote it.
    assert_eq!(vm.eval(b"return T.poke(9);").unwrap(), b"toy 4: T,9");
    // A package no file of the language was given is refused.
    assert_eq!(
        run(&mut vm, &files, "defineIn SomeoneElse toyEvil 5"),
        Ok(true)
    );
    assert_eq!(result(&vm), "refused");
}

#[test]
fn read_script_keeps_to_the_language_s_files_and_exec_s_checks() {
    struct Refuse;
    impl ExecPolicy for Refuse {
        fn check(&self, _vm: &mut Vm, path: &[u8]) -> Result<bool, VmError> {
            Ok(!path.starts_with(b"sent/"))
        }
    }
    let engine = engine();
    let files = MemFiles::new();
    files.insert("lib/a.toy", b"module");
    files.insert("lib/a.cs", b"script");
    files.insert("sent/b.toy", b"from a server");
    let mut vm = server(&engine, &files);
    vm.set_exec_policy(std::rc::Rc::new(Refuse));
    for (path, expected) in [
        ("lib/a.toy", "module"),
        ("lib/a.cs", "refused"),
        ("sent/b.toy", "refused"),
        ("../lib/a.toy", "refused"),
        ("lib/../lib/a.toy", "refused"),
        ("/lib/a.toy", "refused"),
        ("C:/lib/a.toy", "refused"),
        ("lib\\a.toy", "refused"),
        ("lib/missing.toy", "refused"),
    ] {
        assert_eq!(run(&mut vm, &files, &format!("read {path}")), Ok(true));
        assert_eq!(result(&vm), expected, "{path}");
    }
}

#[test]
fn the_step_limit_stops_the_language_and_holds_across_calls() {
    let engine = engine();
    let files = MemFiles::new();
    let mut vm = server(&engine, &files);
    vm.set_step_limit(Some(100_000));
    assert_eq!(run(&mut vm, &files, "spin"), Err(VmError::StepLimit));
    assert_eq!(run(&mut vm, &files, "define toySpin 8"), Ok(true));
    assert_eq!(vm.eval(b"toySpin(); $after = 1;"), Err(VmError::StepLimit));
    assert_eq!(vm.global("$after"), None);
    vm.set_step_limit(None);
    // With no limit of its own, the default budget stops it.
    assert_eq!(run(&mut vm, &files, "spin"), Err(VmError::StepLimit));
    assert_eq!(vm.eval(b"return 1 + 1;").unwrap(), b"2");
}

#[test]
fn nested_calls_back_stop_at_the_call_depth_on_a_small_stack() {
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(|| {
            let engine = engine();
            let files = MemFiles::new();
            let mut vm = server(&engine, &files);
            vm.eval(
                b"function tsDown(%n) { if (%n <= 0) return \"bottom\"; return toyDown(%n - 1); }",
            )
            .unwrap();
            assert_eq!(run(&mut vm, &files, "define toyDown 7"), Ok(true));
            assert_eq!(vm.eval(b"return tsDown(10);").unwrap(), b"bottom");
            assert_eq!(vm.eval(b"return tsDown(100000);"), Err(VmError::CallDepth));
            assert_eq!(vm.eval(b"return tsDown(2);").unwrap(), b"bottom");
        })
        .unwrap()
        .join()
        .expect("no overflow");
}

#[test]
fn the_language_hears_when_a_script_engine_goes() {
    let engine = engine();
    let files = MemFiles::new();
    let before = DROPPED.load(Ordering::SeqCst);
    {
        let mut vm = server(&engine, &files);
        assert_eq!(run(&mut vm, &files, "info"), Ok(true));
    }
    assert!(DROPPED.load(Ordering::SeqCst) > before);
}

#[test]
fn the_call_policy_is_the_host_s_whatever_the_namespace() {
    let engine = engine();
    let files = MemFiles::new();
    let mut vm = server(&engine, &files);
    for line in [
        "call exec main.cs",
        "call SimObject::save x",
        "call SimObject::call x save",
        "call FileObject::openForWrite x out.cs",
        "call schedule 0 0 echo exec",
        "call SimObject::schedule x 0 save",
    ] {
        assert_eq!(run(&mut vm, &files, line), Ok(true));
        assert_eq!(result(&vm), "refused 5", "{line}");
    }
    assert!(
        vm.log_texts()
            .iter()
            .any(|l| l.contains("from a script language"))
    );
    // A realm with other rights puts its own policy in: here an allowlist.
    struct Only(&'static str);
    impl torque_plugin_loader::call_policy::CallPolicy for Only {
        fn check(&self, _: &str, name: &str, _: bool, _: &[Vec<u8>]) -> Result<(), String> {
            if name.eq_ignore_ascii_case(self.0) {
                Ok(())
            } else {
                Err(format!("`{name}` is not allowed here"))
            }
        }

        fn check_field(&self, _: &[u8], _: &[u8], _: &[u8]) -> Result<(), String> {
            Err("no field writes here".to_string())
        }
    }
    torque_plugin_loader::call_policy::set_call_policy(&mut vm, std::rc::Rc::new(Only("strupr")));
    assert_eq!(run(&mut vm, &files, "call strupr a"), Ok(true));
    assert_eq!(result(&vm), "A");
    assert_eq!(run(&mut vm, &files, "call strlwr A"), Ok(true));
    assert_eq!(result(&vm), "refused 5");
}

#[test]
fn a_queued_call_is_checked_again_as_it_fires() {
    use std::cell::Cell;
    use std::rc::Rc;
    use torque_plugin_loader::call_policy::{CallPolicy, set_call_policy};
    // Refuses `tsMark` itself, and lets `schedule` through: only the check as the call fires sees `tsMark`.
    struct NoMark(Cell<u32>);
    impl CallPolicy for NoMark {
        fn check(&self, _: &str, name: &str, _: bool, _: &[Vec<u8>]) -> Result<(), String> {
            if name.eq_ignore_ascii_case("tsMark") {
                self.0.set(self.0.get() + 1);
                return Err("not tsMark".to_string());
            }
            Ok(())
        }
        fn check_field(&self, _: &[u8], _: &[u8], _: &[u8]) -> Result<(), String> {
            Ok(())
        }
    }
    let engine = engine();
    let files = MemFiles::new();
    let mut vm = server(&engine, &files);
    vm.eval(b"function tsMark() { $marked = $marked + 1; }")
        .unwrap();
    // The default policy lets an ordinary queued call run.
    assert_eq!(run(&mut vm, &files, "call schedule 0 0 tsMark"), Ok(true));
    vm.advance_time(1);
    assert_eq!(vm.global("$marked").as_deref(), Some(&b"1"[..]));
    // A queued call refused when it fires is skipped, with a warning.
    let policy = Rc::new(NoMark(Cell::new(0)));
    set_call_policy(&mut vm, policy.clone());
    assert_eq!(run(&mut vm, &files, "call schedule 0 0 tsMark"), Ok(true));
    assert_ne!(result(&vm), "refused 5", "queued");
    vm.advance_time(1);
    assert_eq!(
        vm.global("$marked").as_deref(),
        Some(&b"1"[..]),
        "it did not run"
    );
    assert!(
        vm.log_texts()
            .iter()
            .any(|l| l.contains("did not run: not tsMark"))
    );
    // A call TorqueScript queues itself is not the language's: it runs.
    vm.eval(b"schedule(0, 0, tsMark);").unwrap();
    vm.advance_time(1);
    assert_eq!(vm.global("$marked").as_deref(), Some(&b"2"[..]));
}
