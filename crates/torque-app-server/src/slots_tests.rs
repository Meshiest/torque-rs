//! The main loop's seams on the engine builder: hooks (a list) and the mission loader (exclusive), made once per server before
//! its entry script.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;
use std::time::Duration;

use torque_engine::{BuildError, Engine, EngineBuilder, Plugin, PluginId, PluginType, ServerStage};
use torque_server::server::{
    HookList, MissionError, MissionLoader, NoHooks, PassSlice, PassView, ServerHooks,
};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

use crate::plugins::MinimalPlugins;
use crate::server::{GameServer, Listen, ServerStart};
use crate::slots::{ServerSlots, make_mission_loader, make_server_hooks};

type Log = Rc<RefCell<Vec<String>>>;

/// A plugin that runs `build` on the engine builder under its own id.
struct Setup {
    id: &'static str,
    build: Box<dyn Fn(&mut EngineBuilder)>,
}

impl Plugin for Setup {
    fn build(&self, engine: &mut EngineBuilder) {
        (self.build)(engine);
    }

    fn id(&self) -> Option<&str> {
        Some(self.id)
    }
}

impl PluginType for Setup {
    const ID: PluginId = PluginId::new("test.slots");
}

fn plugin(id: &'static str, build: impl Fn(&mut EngineBuilder) + 'static) -> Setup {
    Setup {
        id,
        build: Box::new(build),
    }
}

fn builder(plugins: Vec<Setup>) -> EngineBuilder {
    let mut builder = EngineBuilder::new();
    builder.add_plugins(MinimalPlugins);
    for plugin in plugins {
        builder.add_plugins(plugin);
    }
    builder
}

fn engine(plugins: Vec<Setup>) -> Engine {
    builder(plugins).build().expect("the engine builds")
}

fn start(engine: &Engine, files: MemFiles) -> Result<GameServer, String> {
    GameServer::start_engine(
        engine,
        files,
        &ServerStart {
            listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
            globals: &[],
            start: "startIt",
            args: &[],
        },
    )
}

fn files() -> MemFiles {
    let files = MemFiles::new();
    files.insert(
        "server.cs",
        b"$Entry = 1;\nfunction startIt() { $Started = 1; }",
    );
    files
}

/// Hooks that log what they see, a name first.
struct Logging {
    name: &'static str,
    log: Log,
    slice: Option<PassSlice>,
    sliced: Cell<u32>,
}

impl Logging {
    fn new(name: &'static str, log: &Log, slice: Option<PassSlice>) -> Rc<Logging> {
        Rc::new(Logging {
            name,
            log: Rc::clone(log),
            slice,
            sliced: Cell::new(0),
        })
    }
}

impl ServerHooks for Logging {
    fn before_events(&self, _: &mut Vm, delta_ms: u32) {
        self.log
            .borrow_mut()
            .push(format!("{} {delta_ms}", self.name));
    }

    fn pass_slice(&self, _: &PassView<'_>) -> Option<PassSlice> {
        self.slice
    }

    fn pass_sliced(&self, sliced: bool) {
        self.sliced.set(self.sliced.get() + u32::from(sliced));
    }
}

fn adding(name: &'static str, log: &Log) -> Setup {
    let log = Rc::clone(log);
    plugin(name, move |engine| {
        let log = Rc::clone(&log);
        engine.server_hooks(move |_| Ok(Logging::new(name, &log, None) as Rc<dyn ServerHooks>));
    })
}

/// A loader whose report is its number.
struct Fixed(u32);

impl MissionLoader for Fixed {
    fn load(&self, vm: &mut Vm, mission: &[u8]) -> Result<Box<dyn Any>, MissionError> {
        vm.set_global("$Loaded", &String::from_utf8_lossy(mission));
        Ok(Box::new(self.0))
    }
}

fn fixed(number: u32) -> Result<Rc<dyn MissionLoader>, String> {
    Ok(Rc::new(Fixed(number)))
}

#[test]
fn server_hooks_one_entry_runs_before_events_once_a_pass_with_the_delta() {
    let log = Log::default();
    let engine = engine(vec![adding("a", &log)]);
    let mut server = start(&engine, files()).expect("the server starts");
    for _ in 0..5 {
        server.tick();
    }
    let log = log.borrow();
    assert_eq!(log.len(), 5, "{log:?}");
    // The first pass covers the start-up, the clock going to 1,024 ms at once; each pass after is one 32 ms tick.
    assert_eq!(log[0], "a 1024");
    assert!(log[1..].iter().all(|line| line == "a 32"), "{log:?}");
}

#[test]
fn server_hooks_run_in_add_order() {
    let log = Log::default();
    let engine = engine(vec![
        adding("a", &log),
        adding("b", &log),
        adding("c", &log),
    ]);
    let mut server = start(&engine, files()).expect("the server starts");
    server.tick();
    server.tick();
    let names: Vec<String> = log
        .borrow()
        .iter()
        .map(|line| line.split(' ').next().unwrap().to_string())
        .collect();
    assert_eq!(names, ["a", "b", "c", "a", "b", "c"]);
}

#[test]
fn server_hooks_pass_slice_is_the_first_answer_and_every_hook_hears_the_pass() {
    let log = Log::default();
    let slice = |steps| {
        Some(PassSlice {
            wall: Duration::from_millis(5),
            steps,
        })
    };
    let list = [
        Logging::new("none", &log, None),
        Logging::new("first", &log, slice(11)),
        Logging::new("second", &log, slice(22)),
    ];
    let hooks = HookList::new(
        list.iter()
            .map(|h| Rc::clone(h) as Rc<dyn ServerHooks>)
            .collect(),
    );
    let vm = Vm::new();
    let view = PassView {
        vm: &vm,
        host_pass_steps: None,
        remote_client: false,
        local_client: false,
    };
    assert_eq!(hooks.pass_slice(&view), slice(11));
    hooks.pass_sliced(true);
    assert!(list.iter().all(|h| h.sliced.get() == 1));
    assert_eq!(HookList::default().pass_slice(&view), None);
}

#[test]
fn server_hooks_a_slice_from_a_plugin_reaches_the_main_loop() {
    let log = Log::default();
    let engine = engine(vec![plugin("test.slicer", {
        let log = Rc::clone(&log);
        move |engine| {
            let log = Rc::clone(&log);
            engine.server_hooks(move |_| {
                let slice = PassSlice {
                    wall: Duration::from_millis(5),
                    steps: 1_000,
                };
                Ok(Logging::new("s", &log, Some(slice)) as Rc<dyn ServerHooks>)
            });
        }
    })]);
    let mut server = start(&engine, files()).expect("the server starts");
    for _ in 0..4 {
        server.tick();
    }
    assert_eq!(server.server.hook_sliced_passes(), 4);
}

#[test]
fn server_hooks_factory_sees_what_a_game_recipe_set() {
    let seen = Rc::new(RefCell::new(None));
    let engine = engine(vec![
        // The factory's plugin is added before the recipe's: the stage, not the add order, decides what it sees.
        plugin("test.reader", {
            let seen = Rc::clone(&seen);
            move |engine| {
                let seen = Rc::clone(&seen);
                engine.server_hooks(move |vm| {
                    *seen.borrow_mut() = vm.global("$FromGame");
                    Ok(Rc::new(NoHooks) as Rc<dyn ServerHooks>)
                });
            }
        }),
        plugin("test.writer", |engine| {
            engine.server(ServerStage::Game, |vm, _| {
                vm.set_global("$FromGame", "set by a recipe");
                Ok(())
            });
        }),
    ]);
    start(&engine, files()).expect("the server starts");
    assert_eq!(
        seen.borrow().as_deref(),
        Some(b"set by a recipe".as_slice())
    );
}

#[test]
fn server_hooks_factories_ran_before_the_entry_script() {
    type Seen = Rc<RefCell<Vec<(&'static str, Option<Vec<u8>>)>>>;
    let seen = Seen::default();
    let engine = engine(vec![plugin("test.order", {
        let seen = Rc::clone(&seen);
        move |engine| {
            let for_hooks = Rc::clone(&seen);
            engine.server_hooks(move |vm| {
                for_hooks.borrow_mut().push(("hooks", vm.global("$Entry")));
                Ok(Rc::new(NoHooks) as Rc<dyn ServerHooks>)
            });
            let for_loader = Rc::clone(&seen);
            engine.mission_loader(move |vm| {
                for_loader
                    .borrow_mut()
                    .push(("loader", vm.global("$Entry")));
                fixed(1)
            });
        }
    })]);
    let server = start(&engine, files()).expect("the server starts");
    assert_eq!(*seen.borrow(), [("hooks", None), ("loader", None)]);
    assert_eq!(
        server.vm().global("$Entry").as_deref(),
        Some(b"1".as_slice())
    );
}

#[test]
fn server_hooks_factory_error_stops_the_start_with_its_message() {
    let engine = engine(vec![plugin("test.fails", |engine| {
        engine.server_hooks(|_| Err("the map table is missing".to_string()));
    })]);
    let Err(error) = start(&engine, files()) else {
        panic!("the start fails");
    };
    assert!(error.contains("the map table is missing"), "{error}");
    assert!(error.contains("test.fails"), "the plugin is named: {error}");
}

#[test]
fn mission_loader_slot_replaces_the_exec_of_the_mis() {
    let engine = engine(vec![plugin("test.loader", |engine| {
        engine.mission_loader(|_| fixed(7));
    })]);
    let files = files();
    files.insert("maps/a.mis", b"$Execd = 1;");
    let mut server = start(&engine, files).expect("the server starts");
    let report = server.load_mission("maps/a.mis").expect("the loader runs");
    assert_eq!(report.downcast_ref::<u32>(), Some(&7));
    assert_eq!(
        server.vm().global("$Loaded").as_deref(),
        Some(b"maps/a.mis".as_slice())
    );
    assert_eq!(server.vm().global("$Execd"), None, "the mis was not run");
}

#[test]
fn mission_loader_an_engine_without_one_execs_the_mis_as_before() {
    let engine = engine(vec![]);
    let files = files();
    files.insert("maps/a.mis", b"$Execd = 1;");
    let mut server = start(&engine, files).expect("the server starts");
    let report = server.load_mission("maps/a.mis").expect("the mis runs");
    assert!(report.downcast_ref::<()>().is_some());
    assert_eq!(
        server.vm().global("$Execd").as_deref(),
        Some(b"1".as_slice())
    );
    assert!(matches!(
        server.load_mission("maps/none.mis"),
        Err(MissionError::Refused(_))
    ));
}

#[test]
fn mission_loader_a_second_setter_fails_the_build_naming_both() {
    let mut builder = builder(vec![
        plugin("test.first", |engine| {
            engine.mission_loader(|_| fixed(1));
        }),
        plugin("test.second", |engine| {
            engine.mission_loader(|_| fixed(2));
        }),
    ]);
    let Err(error) = builder.build() else {
        panic!("the build fails");
    };
    let text = error.to_string();
    assert!(matches!(error, BuildError::DuplicateSlot { .. }), "{text}");
    assert!(
        text.contains("test.first") && text.contains("test.second"),
        "{text}"
    );
}

#[test]
fn server_hooks_make_server_hooks_serves_a_host_of_its_own() {
    let log = Log::default();
    let mut vm = Vm::new();
    // No entry: none, so a host keeps its plain hooks.
    let plain = engine(vec![]);
    assert!(make_server_hooks(&plain, &mut vm).unwrap().is_none());
    // Several entries: one set of hooks that runs them in add order.
    let engine = engine(vec![adding("a", &log), adding("b", &log)]);
    let hooks = make_server_hooks(&engine, &mut vm)
        .unwrap()
        .expect("the engine has hooks");
    hooks.before_events(&mut vm, 32);
    assert_eq!(*log.borrow(), ["a 32", "b 32"]);
    // A factory's error comes back with its message.
    let failing = self::engine(vec![plugin("test.fails", |engine| {
        engine.server_hooks(|_| Err("no map table".to_string()));
    })]);
    let Err(error) = make_server_hooks(&failing, &mut vm) else {
        panic!("fails");
    };
    assert!(error.contains("no map table"), "{error}");
}

#[test]
fn mission_loader_make_mission_loader_serves_a_host_of_its_own() {
    let mut vm = Vm::new();
    assert!(
        make_mission_loader(&engine(vec![]), &mut vm)
            .unwrap()
            .is_none()
    );
    let engine = engine(vec![plugin("test.loader", |engine| {
        engine.mission_loader(|_| fixed(9));
    })]);
    let loader = make_mission_loader(&engine, &mut vm)
        .unwrap()
        .expect("the engine has a loader");
    let report = loader.load(&mut vm, b"maps/b.mis").expect("loads");
    assert_eq!(report.downcast_ref::<u32>(), Some(&9));
    let failing = self::engine(vec![plugin("test.fails", |engine| {
        engine.mission_loader(|_| Err("no maps".to_string()));
    })]);
    let Err(error) = make_mission_loader(&failing, &mut vm) else {
        panic!("fails");
    };
    assert!(error.contains("no maps"), "{error}");
}

#[test]
fn mission_loader_factory_error_stops_the_start_with_its_message() {
    let engine = engine(vec![plugin("test.fails", |engine| {
        engine.mission_loader(|_| Err("the map folder is missing".to_string()));
    })]);
    let Err(error) = start(&engine, files()) else {
        panic!("the start fails");
    };
    assert!(error.contains("the map folder is missing"), "{error}");
}
