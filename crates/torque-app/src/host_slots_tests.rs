//! The host's slots on the client side: the script engine a client is made on (the same factory as its servers'), the files
//! of a game's own host, and the client's entry.

use std::cell::{Cell, RefCell};
use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;

use torque_engine::{BuildError, ClientStage, Engine, EngineBuilder, Plugin, PluginId, PluginType};
use torque_ts_vm::host::{FileHost, FileRead, MemFiles};
use torque_ts_vm::vm::Vm;

use crate::client::{ClientBuilder, ClientNet};
use crate::plugins::DefaultPlugins;
use crate::server::{GameServer, Listen, ServerStart};
use crate::slots::HostSlots;

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
    const ID: PluginId = PluginId::new("test.host_slots");
}

fn plugin(id: &'static str, build: impl Fn(&mut EngineBuilder) + 'static) -> Setup {
    Setup {
        id,
        build: Box::new(build),
    }
}

fn builder(plugins: Vec<Setup>) -> EngineBuilder {
    let mut builder = EngineBuilder::new();
    builder.add_plugins(DefaultPlugins);
    for plugin in plugins {
        builder.add_plugins(plugin);
    }
    builder
}

fn engine(plugins: Vec<Setup>) -> Engine {
    builder(plugins).build().expect("the engine builds")
}

fn client_files() -> MemFiles {
    let files = MemFiles::new();
    files.insert("main.cs", b"$Main = 1;");
    files
}

fn client(engine: &Engine) -> ClientBuilder {
    ClientBuilder::from_engine(engine, Some(client_files()), ClientNet::None)
        .expect("the client is made")
}

fn global(builder: &ClientBuilder, name: &str) -> Option<Vec<u8>> {
    builder.vm.global(name)
}

#[test]
fn script_engine_slot_makes_the_clients_engine_and_its_servers() {
    let made = Rc::new(Cell::new(0));
    let engine = engine(vec![plugin("test.engine", {
        let made = Rc::clone(&made);
        move |engine| {
            let made = Rc::clone(&made);
            engine.script_engine(move || {
                made.set(made.get() + 1);
                let mut vm = Vm::new();
                vm.set_global("$Made", "1");
                vm
            });
        }
    })]);
    let builder = client(&engine);
    assert_eq!(global(&builder, "$Made").as_deref(), Some(b"1".as_slice()));
    let files = MemFiles::new();
    files.insert("server.cs", b"function startNothing() {}");
    let server = GameServer::start_engine(
        &engine,
        files,
        &ServerStart {
            listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
            globals: &[],
            start: "startNothing",
            args: &[],
        },
    )
    .expect("the server starts");
    assert_eq!(
        server.vm().global("$Made").as_deref(),
        Some(b"1".as_slice())
    );
    assert_eq!(
        made.get(),
        2,
        "one script engine for the client, one for the server"
    );
}

/// A file host of a game's own over `files`, counting its reads.
struct Counting {
    files: MemFiles,
    reads: Rc<Cell<u32>>,
}

impl FileHost for Counting {
    fn read(&self, path: &[u8]) -> FileRead {
        self.reads.set(self.reads.get() + 1);
        self.files.read(path)
    }
}

#[test]
fn script_files_from_engine_with_files_reads_through_the_games_host() {
    let reads = Rc::new(Cell::new(0));
    let host = Counting {
        files: client_files(),
        reads: Rc::clone(&reads),
    };
    let mut builder = ClientBuilder::from_engine_with_files(&engine(vec![]), host, ClientNet::None)
        .expect("the client is made");
    builder.exec_entry().expect("the entry runs");
    assert!(reads.get() > 0, "main.cs was read through the host");
    assert_eq!(global(&builder, "$Main").as_deref(), Some(b"1".as_slice()));
}

#[test]
fn client_entry_replaces_the_entry_script_and_the_entry_stage_replays_after_it() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let engine = engine(vec![plugin("test.entry", {
        let log = Rc::clone(&log);
        move |engine| {
            let for_entry = Rc::clone(&log);
            engine.client_entry(move |vm| {
                vm.set_global("$Custom", "1");
                for_entry.borrow_mut().push("entry");
                Ok(())
            });
            let for_stage = Rc::clone(&log);
            engine.client(ClientStage::Entry, move |_, _| {
                for_stage.borrow_mut().push("stage");
                Ok(())
            });
        }
    })]);
    let mut builder = client(&engine);
    builder.exec_entry().expect("the entry runs");
    assert_eq!(global(&builder, "$Main"), None, "main.cs did not run");
    assert_eq!(
        global(&builder, "$Custom").as_deref(),
        Some(b"1".as_slice())
    );
    assert_eq!(*log.borrow(), ["entry", "stage"]);
}

#[test]
fn client_entry_an_engine_without_one_runs_the_entry_script() {
    let mut builder = client(&engine(vec![]));
    builder.exec_entry().expect("the entry runs");
    assert_eq!(global(&builder, "$Main").as_deref(), Some(b"1".as_slice()));
}

#[test]
fn client_entry_an_error_is_exec_entrys() {
    let engine = engine(vec![plugin("test.entry", |engine| {
        engine.client_entry(|_| Err("the entry script is not in the program".to_string()));
    })]);
    let Err(error) = client(&engine).exec_entry() else {
        panic!("the entry fails");
    };
    assert!(
        error.contains("the entry script is not in the program"),
        "{error}"
    );
}

#[test]
fn client_entry_a_second_setter_fails_the_build_naming_both() {
    let mut builder = builder(vec![
        plugin("test.first", |engine| {
            engine.client_entry(|_| Ok(()));
        }),
        plugin("test.second", |engine| {
            engine.client_entry(|_| Ok(()));
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
