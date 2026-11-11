//! `reloadDataBlocks` and the development watcher over the memory transport: an edited script file is run again within a
//! poll interval and its datablocks reach a connected client, an unchanged file is never run again, a syntax error keeps
//! every datablock and is tried again only when the file changes again, nothing is read outside development, the missing
//! datablocks are removed only when asked and only without users, and a file that makes other objects is warned about once.

use crate::datablock_live::{Rig, answer, global, id_of};
use crate::net_client::eval;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use torque_engine::assets::HOT_RELOAD_INTERVAL_MS;
use torque_server::reload::{self, DevGate};
use torque_server::server::TICK_MS;
use torque_ts_vm::host::{FileHost, FileRead, FileStat, MemFiles};
use torque_ts_vm::vm::Level;

/// The server ticks one poll interval spans, and the tick the poll lands on.
const POLL_TICKS: usize = (HOT_RELOAD_INTERVAL_MS as usize).div_ceil(TICK_MS as usize) + 1;

/// `Desc0`.. with the given `referenceDistance`s (a raw float on the wire).
fn descs(values: &[f32]) -> String {
    values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            format!(
                "datablock AudioDescription(Desc{i}) {{ is3D = true; referenceDistance = {v}; }};\n"
            )
        })
        .collect()
}

/// The server's files: every write stamps a later change time.
struct Files {
    mem: MemFiles,
    modified_calls: Rc<Cell<u64>>,
    stamp: u64,
}

impl Files {
    fn new() -> Files {
        Files {
            mem: MemFiles::new(),
            modified_calls: Rc::new(Cell::new(0)),
            stamp: 0,
        }
    }

    fn write(&mut self, path: &str, text: &str) {
        self.stamp += 1;
        self.mem.insert_at(path, text.as_bytes(), self.stamp);
    }

    fn host(&self) -> Counting {
        Counting {
            mem: self.mem.clone(),
            modified_calls: Rc::clone(&self.modified_calls),
        }
    }
}

/// The files, counting the change times read.
#[derive(Clone)]
struct Counting {
    mem: MemFiles,
    modified_calls: Rc<Cell<u64>>,
}

impl FileHost for Counting {
    fn read(&self, path: &[u8]) -> FileRead {
        self.mem.read(path)
    }

    fn stat(&self, path: &[u8]) -> FileStat {
        self.mem.stat(path)
    }

    fn modified(&self, path: &[u8]) -> Option<u64> {
        self.modified_calls.set(self.modified_calls.get() + 1);
        self.mem.modified(path)
    }

    fn files(&self) -> Rc<Vec<Vec<u8>>> {
        self.mem.files()
    }
}

/// A rig whose server ran `a.cs` from `files`.
fn rig(files: &Files) -> Rig {
    let mut rig = Rig::with_files(files.host());
    eval(&mut rig.s, r#"exec("a.cs");"#);
    rig.s.vm.drain_log();
    rig
}

fn lines(rig: &mut Rig, level: Level) -> Vec<String> {
    rig.s
        .vm
        .drain_log()
        .into_iter()
        .filter(|l| l.level == level)
        .map(|l| String::from_utf8_lossy(&l.text).into_owned())
        .collect()
}

fn value(rig: &mut Rig, name: &str) -> f32 {
    answer(&mut rig.s, &format!("return {name}.referenceDistance;"))
        .parse()
        .expect("a number")
}

fn exists(rig: &Rig, name: &str) -> bool {
    rig.s.vm.objects().find(name.as_bytes()).is_some()
}

#[test]
fn an_edited_file_reloads_within_a_poll_interval_and_the_client_gets_the_value() {
    let mut files = Files::new();
    files.write("a.cs", &descs(&[1.0, 2.0, 3.0]));
    let mut rig = rig(&files);
    let c = rig.connected();
    let desc1 = id_of(&rig.s, "Desc1");
    assert_eq!(rig.clients[c].distance(desc1), Some(2.0));
    // A plugin's hook sees the reload before the round reaches anyone: (file, changed, added, report).
    type Seen = Vec<(String, usize, usize, String)>;
    let seen: Rc<RefCell<Seen>> = Rc::default();
    let hook_seen = Rc::clone(&seen);
    assert!(reload::before_round(
        &rig.s.vm,
        Rc::new(move |vm, done| {
            let updated =
                String::from_utf8_lossy(&vm.global("$updated").unwrap_or_default()).into_owned();
            hook_seen.borrow_mut().push((
                String::from_utf8_lossy(&done.file).into_owned(),
                done.changed,
                done.added,
                updated,
            ));
            Ok(())
        })
    ));

    files.write("a.cs", &descs(&[1.0, 20.0, 3.0]));
    let took = rig
        .run_until(POLL_TICKS + 1, |r| reload::stats(&r.s.vm).reloads == 1)
        .expect("the file is reloaded");
    assert!(took <= POLL_TICKS, "{took} ticks");
    assert_eq!(value(&mut rig, "Desc1"), 20.0);
    assert!(
        rig.run_until(200, |r| {
            r.clients[c].distance(desc1) == Some(20.0) && global(&r.s, "$updated") == "1"
        })
        .is_some(),
        "the client has the value and its round settled: {:?}",
        rig.clients[c].errors
    );
    assert_eq!(
        global(&rig.s, "$done"),
        "1",
        "the mission chain is not run again"
    );
    assert_eq!(
        *seen.borrow(),
        [("a.cs".to_string(), 3, 0, String::new())],
        "the hook ran once, before the round"
    );
    let stats = reload::stats(&rig.s.vm);
    assert_eq!((stats.reloads, stats.rounds, stats.failed), (1, 1, 0));
}

#[test]
fn an_unchanged_file_is_never_run_again() {
    let mut files = Files::new();
    files.write("a.cs", &descs(&[1.0, 2.0]));
    let mut rig = rig(&files);
    for _ in 0..5 * POLL_TICKS {
        rig.tick();
    }
    let stats = reload::stats(&rig.s.vm);
    assert!(stats.polls >= 4, "{stats:?}");
    assert!(
        stats.checks >= stats.polls,
        "every poll looked at the file: {stats:?}"
    );
    assert_eq!(stats.reloads, 0);
    let executing: Vec<String> = lines(&mut rig, Level::Normal)
        .into_iter()
        .filter(|l| l.starts_with("Executing"))
        .collect();
    assert!(executing.is_empty(), "{executing:?}");
}

#[test]
fn a_syntax_error_keeps_every_datablock_prints_one_error_and_is_retried_only_after_the_next_change()
{
    let mut files = Files::new();
    files.write("a.cs", &descs(&[1.0, 2.0, 3.0]));
    let mut rig = rig(&files);
    // A missing `;` after the field: the file does not compile.
    files.write(
        "a.cs",
        "datablock AudioDescription(Desc0) { is3D = true; referenceDistance = 50 };\n",
    );
    for _ in 0..POLL_TICKS + 1 {
        rig.tick();
    }
    let stats = reload::stats(&rig.s.vm);
    assert_eq!((stats.failed, stats.reloads), (1, 0), "{stats:?}");
    assert_eq!(
        (
            value(&mut rig, "Desc0"),
            value(&mut rig, "Desc1"),
            value(&mut rig, "Desc2")
        ),
        (1.0, 2.0, 3.0),
        "every datablock is as it was"
    );
    // One syntax report: the error line and the compiler's line of context.
    let errors = lines(&mut rig, Level::Error);
    let reports = errors.iter().filter(|l| l.contains("Syntax error")).count();
    assert_eq!((reports, errors.len()), (1, 2), "{errors:?}");
    assert!(errors[0].starts_with("a.cs"), "{errors:?}");
    for _ in 0..4 * POLL_TICKS {
        rig.tick();
    }
    assert_eq!(
        reload::stats(&rig.s.vm).failed,
        1,
        "not tried again while unchanged"
    );
    assert!(lines(&mut rig, Level::Error).is_empty());
    files.write("a.cs", &descs(&[1.0, 2.0, 30.0]));
    assert!(
        rig.run_until(POLL_TICKS + 1, |r| reload::stats(&r.s.vm).reloads == 1)
            .is_some()
    );
    assert_eq!(value(&mut rig, "Desc2"), 30.0);
}

#[test]
fn outside_development_no_change_time_is_read_and_the_native_is_refused() {
    let mut files = Files::new();
    files.write("a.cs", &descs(&[1.0]));
    let mut rig = rig(&files);
    reload::set_gate(&rig.s.vm, DevGate::PrefOnly);
    files.modified_calls.set(0);
    files.write("a.cs", &descs(&[10.0]));
    for _ in 0..3 * POLL_TICKS {
        rig.tick();
    }
    assert_eq!(files.modified_calls.get(), 0, "no change time read");
    assert_eq!(reload::stats(&rig.s.vm), reload::Stats::default());
    assert_eq!(value(&mut rig, "Desc0"), 1.0);
    assert_eq!(
        answer(&mut rig.s, r#"return reloadDataBlocks("a.cs");"#),
        ""
    );
    let errors = lines(&mut rig, Level::Error);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("development command"), "{errors:?}");
    rig.s.vm.set_global("$Pref::Dev", "1");
    assert!(
        rig.run_until(POLL_TICKS + 1, |r| reload::stats(&r.s.vm).reloads == 1)
            .is_some()
    );
    assert!(files.modified_calls.get() > 0);
    assert_eq!(value(&mut rig, "Desc0"), 10.0);
}

#[test]
fn remove_missing_removes_the_dropped_datablocks_without_users_and_reports_the_rest() {
    let mut files = Files::new();
    let b = [-1.0, -1.0, 0.0, 1.0, 1.0, 1.0];
    let shape = torque_shapes::synth::hull_shape(b, &[("detail2", b)], &[], &["ambient"]);
    files
        .mem
        .insert("a.dts", &torque_shapes::write::write_dts(&shape));
    files.write(
        "a.cs",
        &format!(
            "{}datablock StaticShapeData(Shape1) {{ shapeFile = \"a.dts\"; }};\n",
            descs(&[1.0, 2.0, 3.0])
        ),
    );
    let mut rig = rig(&files);
    eval(&mut rig.s, "new StaticShape(S) { dataBlock = Shape1; };");
    rig.s.vm.drain_log();

    // Reported, not removed, without the flag.
    files.write("a.cs", &descs(&[1.0]));
    assert_eq!(
        answer(&mut rig.s, r#"return reloadDataBlocks("a.cs");"#),
        "changed:1 added:0 removed:0"
    );
    let reported: Vec<String> = lines(&mut rig, Level::Normal)
        .into_iter()
        .filter(|l| l.starts_with("no longer declared by a.cs: "))
        .collect();
    assert_eq!(
        reported,
        [
            "no longer declared by a.cs: Desc1 (AudioDescription)",
            "no longer declared by a.cs: Desc2 (AudioDescription)",
            "no longer declared by a.cs: Shape1 (StaticShapeData)",
        ]
    );
    assert!(exists(&rig, "Desc1") && exists(&rig, "Desc2") && exists(&rig, "Shape1"));

    // With the flag: the two without users go, the used one is refused and named.
    assert_eq!(
        answer(&mut rig.s, r#"return reloadDataBlocks("a.cs", true);"#),
        "changed:1 added:0 removed:2"
    );
    let errors = lines(&mut rig, Level::Error);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("Shape1 (StaticShapeData) is used by 1 object(s)"),
        "{errors:?}"
    );
    assert!(!exists(&rig, "Desc1") && !exists(&rig, "Desc2"));
    assert!(exists(&rig, "Shape1") && exists(&rig, "S"));
    assert_eq!(value(&mut rig, "Desc0"), 1.0);

    // The pref does the same for every reload; the last user gone, the datablock goes.
    eval(&mut rig.s, "S.delete();");
    rig.s.vm.set_global(reload::REMOVE_MISSING_PREF, "1");
    assert_eq!(
        answer(&mut rig.s, r#"return reloadDataBlocks("a.cs");"#),
        "changed:1 added:0 removed:1"
    );
    assert!(!exists(&rig, "Shape1"));
    assert!(lines(&mut rig, Level::Error).is_empty());
    // The watcher has nothing left to do: the file's time is the one the reload saw.
    for _ in 0..2 * POLL_TICKS {
        rig.tick();
    }
    assert_eq!(reload::stats(&rig.s.vm).reloads, 3);
}

#[test]
fn a_reload_asked_for_inside_a_reload_is_refused() {
    let mut files = Files::new();
    let b = [-1.0, -1.0, 0.0, 1.0, 1.0, 1.0];
    let shape = torque_shapes::synth::hull_shape(b, &[("detail2", b)], &[], &["ambient"]);
    files
        .mem
        .insert("a.dts", &torque_shapes::write::write_dts(&shape));
    files.write(
        "a.cs",
        r#"datablock StaticShapeData(Shape1) { shapeFile = "a.dts"; };"#,
    );
    let mut rig = rig(&files);
    // A hook and a used datablock's `onReloaded` both run inside the reload; neither may start another.
    let nested: Rc<RefCell<Vec<String>>> = Rc::default();
    let hook_nested = Rc::clone(&nested);
    assert!(reload::before_round(
        &rig.s.vm,
        Rc::new(move |vm, _| {
            let answer = vm.eval(br#"return reloadDataBlocks("a.cs");"#).unwrap();
            hook_nested
                .borrow_mut()
                .push(String::from_utf8_lossy(&answer).into_owned());
            Ok(())
        })
    ));
    eval(
        &mut rig.s,
        r#"new StaticShape(S) { dataBlock = Shape1; };
           function StaticShapeData::onReloaded(%db) { $reloaded++; $nested = reloadDataBlocks("a.cs"); }"#,
    );
    rig.s.vm.drain_log();
    assert_eq!(
        answer(&mut rig.s, r#"return reloadDataBlocks("a.cs");"#),
        "changed:1 added:0 removed:0"
    );
    assert_eq!(
        *nested.borrow(),
        [String::new()],
        "the hook's reload returned nothing"
    );
    assert_eq!(
        global(&rig.s, "$reloaded"),
        "1",
        "onReloaded ran inside the reload"
    );
    assert_eq!(
        global(&rig.s, "$nested"),
        "",
        "onReloaded's reload returned nothing"
    );
    let errors = lines(&mut rig, Level::Error);
    assert_eq!(
        errors
            .iter()
            .filter(|l| l.contains("while a reload runs"))
            .count(),
        2,
        "{errors:?}"
    );
    let stats = reload::stats(&rig.s.vm);
    assert_eq!((stats.reloads, stats.rounds), (1, 1), "{stats:?}");
}

#[test]
fn a_file_that_creates_other_objects_is_warned_about_once() {
    let mut files = Files::new();
    files.write("a.cs", &format!("{}new SimGroup(Extra);\n", descs(&[1.0])));
    let mut rig = rig(&files);
    assert!(
        lines(&mut rig, Level::Warning).is_empty(),
        "the first run is not a reload"
    );
    for _ in 0..2 {
        assert_eq!(
            answer(&mut rig.s, r#"return reloadDataBlocks("a.cs");"#),
            "changed:1 added:0 removed:0"
        );
    }
    let warnings = lines(&mut rig, Level::Warning);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(
        warnings[0],
        "a.cs creates 1 object(s) besides its datablocks, made again at every reload: Extra (SimGroup)"
    );
}
