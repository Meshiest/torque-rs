//! A dedicated server run by `Engine::run` on `MinimalPlugins`: it ticks, stops on request or at its cap, and says what is
//! missing when the engine was given no server.

use std::cell::{Cell, RefCell};
use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;

use torque_engine::{EngineBuilder, Plugin, RunError, ServerStage};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Native;

use crate::plugins::MinimalPlugins;
use crate::run::{Dedicated, EngineDedicated, Stop};
use crate::server::Listen;

/// Counts the ticks a script reports and asks the server to stop at the third.
#[derive(torque_engine::PluginId)]
#[plugin(id = "test.counter")]
struct Counter {
    stop: Stop,
    ticks: Rc<Cell<u32>>,
}

impl Plugin for Counter {
    fn build(&self, engine: &mut EngineBuilder) {
        let stop = self.stop.clone();
        let ticks = Rc::clone(&self.ticks);
        engine.server(ServerStage::Game, move |vm, _| {
            let stop = stop.clone();
            let ticks = Rc::clone(&ticks);
            vm.register("noteTick", 1, 1, "() counts a tick", move |_vm, _| {
                ticks.set(ticks.get() + 1);
                if ticks.get() >= 3 {
                    stop.request();
                }
                Native::Void
            });
            Ok(())
        });
    }
}

fn files() -> MemFiles {
    let files = MemFiles::new();
    files.insert(
        "server.cs",
        b"function startIt() { echo(\"started\"); onTick(); }\n\
          function onTick() { noteTick(); schedule(32, 0, \"onTick\"); }",
    );
    files
}

fn memory() -> Listen {
    Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000)))
}

#[test]
fn run_starts_a_dedicated_server_on_minimal_plugins_ticks_and_stops_on_request() {
    let stop = Stop::new();
    let ticks = Rc::new(Cell::new(0));
    let lines = Rc::new(RefCell::new(Vec::<String>::new()));
    let mut server = Dedicated::new(files(), memory(), "startIt");
    server.stop = stop.clone();
    // The cap only guards the test: the stop request is what ends the run.
    server.ticks = Some(500);
    server.print = Some({
        let lines = Rc::clone(&lines);
        Rc::new(move |line: &str| lines.borrow_mut().push(line.to_string()))
    });
    EngineBuilder::new()
        .add_plugins((
            MinimalPlugins,
            Counter {
                stop: stop.clone(),
                ticks: Rc::clone(&ticks),
            },
        ))
        .dedicated(server)
        .run()
        .expect("the server runs");
    assert_eq!(ticks.get(), 3);
    assert!(stop.is_requested());
    assert!(
        lines.borrow().iter().any(|l| l.contains("started")),
        "{:?}",
        lines.borrow()
    );
}

#[test]
fn the_tick_cap_ends_a_dedicated_run_without_a_request() {
    let mut server = Dedicated::new(files(), memory(), "startIt");
    server.ticks = Some(2);
    server.print = Some(Rc::new(|_| {}));
    let ticks = Rc::new(Cell::new(0));
    let mut builder = EngineBuilder::new();
    builder.add_plugins((
        MinimalPlugins,
        Counter {
            // Not the server's stop: only the cap ends this run.
            stop: Stop::new(),
            ticks: Rc::clone(&ticks),
        },
    ));
    builder.dedicated(server).run().expect("the server runs");
    assert!(ticks.get() >= 1);
}

#[test]
fn an_engine_given_no_server_says_so() {
    let err = EngineBuilder::new()
        .add_plugins(MinimalPlugins)
        .run()
        .unwrap_err();
    let RunError::Failed(message) = &err else {
        panic!("{err:?}");
    };
    assert!(message.contains("no dedicated server"), "{message}");
}
