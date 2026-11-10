//! Other languages `exec` runs (`Vm::set_script_language`): the file is read through the file host after the exec policy,
//! handed to the language inside a run, and the run's budgets apply to it.

use std::cell::RefCell;
use std::rc::Rc;

use torque_ts_vm::host::MemFiles;
use torque_ts_vm::policy::{ExecPolicy, ScriptLanguage};
use torque_ts_vm::vm::{Vm, VmError};

/// Records what it ran; charges one unit of work per byte when asked to, and calls `echo` back.
#[derive(Default)]
struct Recorder {
    ran: RefCell<Vec<(String, String)>>,
}

impl ScriptLanguage for Recorder {
    fn run(&self, vm: &mut Vm, path: &[u8], source: &[u8]) -> Result<(), VmError> {
        let text = String::from_utf8_lossy(source).into_owned();
        self.ran
            .borrow_mut()
            .push((String::from_utf8_lossy(path).into_owned(), text.clone()));
        if text == "spin" {
            while vm.charge_work(64) {}
            return Err(VmError::StepLimit);
        }
        if let Some(arg) = text.strip_prefix("echo ") {
            let back = vm.invoke(None, b"strupr", &[arg.as_bytes().to_vec()])?;
            vm.set_global_text(b"$Echo", &back);
        }
        Ok(())
    }
}

fn vm_with(files: &MemFiles) -> (Vm, Rc<Recorder>) {
    let mut vm = Vm::new();
    vm.set_file_host(files.clone());
    let recorder = Rc::new(Recorder::default());
    assert!(vm.set_script_language("lua", recorder.clone()));
    (vm, recorder)
}

#[test]
fn exec_hands_a_file_of_another_language_to_it() {
    let files = MemFiles::new();
    files.insert("mods/a/server.lua", b"echo hello");
    let (mut vm, recorder) = vm_with(&files);
    assert_eq!(vm.exec_file(b"mods/a/server.LUA", false), Ok(true));
    assert_eq!(recorder.ran.borrow().len(), 1);
    assert_eq!(vm.global("$Echo").as_deref(), Some(&b"HELLO"[..]));
    let lines: Vec<String> = vm.log_texts();
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("Executing mods/a/server")),
        "{lines:?}"
    );
}

#[test]
fn a_missing_file_warns_and_runs_nothing() {
    let files = MemFiles::new();
    let (mut vm, recorder) = vm_with(&files);
    assert_eq!(vm.exec_file(b"nope.lua", false), Ok(false));
    assert!(recorder.ran.borrow().is_empty());
    assert!(
        vm.log_texts()
            .iter()
            .any(|l| l == "Missing file: nope.lua!")
    );
}

#[test]
fn the_exec_policy_checks_the_file_first() {
    struct Refuse;
    impl ExecPolicy for Refuse {
        fn check(&self, _vm: &mut Vm, _path: &[u8]) -> Result<bool, VmError> {
            Ok(false)
        }
    }
    let files = MemFiles::new();
    files.insert("x.lua", b"echo a");
    let (mut vm, recorder) = vm_with(&files);
    vm.set_exec_policy(Rc::new(Refuse));
    assert_eq!(vm.exec_file(b"x.lua", false), Ok(false));
    assert!(recorder.ran.borrow().is_empty());
}

#[test]
fn the_run_s_step_limit_stops_the_language() {
    let files = MemFiles::new();
    files.insert("spin.lua", b"spin");
    let (mut vm, _) = vm_with(&files);
    vm.set_step_limit(Some(10_000));
    assert_eq!(vm.exec_file(b"spin.lua", false), Err(VmError::StepLimit));
    // The Vm is still usable: the next run starts with a fresh budget.
    files.insert("ok.lua", b"echo again");
    assert_eq!(vm.exec_file(b"ok.lua", false), Ok(true));
    assert_eq!(vm.global("$Echo").as_deref(), Some(&b"AGAIN"[..]));
}

#[test]
fn torque_script_s_extensions_are_never_taken_and_others_only_once() {
    let mut vm = Vm::new();
    for ext in ["cs", ".CS", "dso", "gui", "mis", "ed", ""] {
        assert!(
            !vm.set_script_language(ext, Rc::new(Recorder::default())),
            "{ext}"
        );
    }
    assert!(vm.script_language("lua").is_none());
    let first: Rc<dyn ScriptLanguage> = Rc::new(Recorder::default());
    assert!(vm.set_script_language(".Lua", Rc::clone(&first)));
    assert!(vm.script_language("LUA").is_some());
    // A second language for the same files is refused: the first keeps them.
    assert!(!vm.set_script_language("lua", Rc::new(Recorder::default())));
    assert!(Rc::ptr_eq(&vm.script_language("lua").unwrap(), &first));
    assert!(vm.script_language_of(b"a/b.c/x.lua").is_some());
    assert!(vm.script_language_of(b"a/b.lua/x").is_none());
}

#[test]
fn a_field_spec_splits_and_joins_as_a_field_write_names_it() {
    use torque_ts_vm::sim::{dynamic_name, split_field_spec};
    let joined = |spec: &[u8]| {
        let (name, index) = split_field_spec(spec);
        dynamic_name(name, index)
    };
    assert_eq!(split_field_spec(b"comm[and]"), (&b"comm"[..], &b"and"[..]));
    assert_eq!(joined(b"comm[and]"), b"command");
    assert_eq!(joined(b"command[0]"), b"command0");
    assert_eq!(
        joined(b"comm[and"),
        b"command",
        "a missing ] runs to the end"
    );
    assert_eq!(
        joined(b"c[omm[and]]"),
        b"comm[and",
        "the index ends at the first ]"
    );
    assert_eq!(joined(b"plain"), b"plain");
    assert_eq!(joined(b"comm\0[x]"), b"comm", "cut at a NUL");
    let long = [b"c[ommand".as_slice(), &[b'x'; 300], b"]"].concat();
    assert_eq!(joined(&long).len(), torque_ts_vm::sim::MAX_FIELD_NAME);
    assert!(joined(&long).starts_with(b"command"));
}
