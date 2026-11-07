//! `Engine::run` over the runners built in: a dedicated server from `DefaultPlugins` and a test plugin, that errors come out
//! of `run`, and that no window or device is touched when there is nothing to show.

use std::cell::{Cell, RefCell};
use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;

use torque_engine::{Engine, EngineBuilder, Plugin, PluginGroup, RunError, ServerStage};
use torque_gui::GuiDraws;
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Native;

use crate::plugins::{DefaultPlugins, RunnerPlugin};
use crate::run::{Dedicated, EngineRun, Offscreen, RunMode, Stop};
use crate::server::Listen;
use crate::view::WorldView;
use crate::{App, Host};

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
fn run_starts_a_dedicated_server_ticks_and_stops_on_request() {
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
            DefaultPlugins,
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
        DefaultPlugins,
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
fn a_build_error_comes_out_of_run() {
    let err = EngineBuilder::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(RunnerPlugin)
        .dedicated(Dedicated::new(files(), memory(), "startIt"))
        .run()
        .unwrap_err();
    assert!(matches!(err, RunError::Build(_)), "{err:?}");
    assert!(
        err.to_string().starts_with("the engine did not build:"),
        "{err}"
    );
    assert!(err.to_string().contains("torque.runner"), "{err}");
}

/// A plugin whose server recipe fails.
#[derive(torque_engine::PluginId)]
#[plugin(id = "test.broken")]
struct Broken;

impl Plugin for Broken {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.server(
            ServerStage::Prepare,
            |_, _| Err("the map is missing".into()),
        );
    }
}

#[test]
fn a_recipe_error_comes_out_of_run_naming_the_plugin() {
    let err = EngineBuilder::new()
        .add_plugins((DefaultPlugins, Broken))
        .dedicated(Dedicated::new(files(), memory(), "startIt"))
        .run()
        .unwrap_err();
    let RunError::Failed(message) = &err else {
        panic!("{err:?}");
    };
    assert!(message.contains("test.broken"), "{message}");
    assert!(message.contains("the map is missing"), "{message}");
}

#[test]
fn an_engine_without_the_runner_plugin_has_no_runner() {
    let err = EngineBuilder::new()
        .add_plugins(DefaultPlugins.disable::<RunnerPlugin>())
        .run()
        .unwrap_err();
    assert_eq!(err, RunError::NoRunner);
}

#[test]
fn the_window_and_offscreen_modes_need_a_game_and_open_nothing_without_one() {
    for mode in [
        RunMode::Auto,
        RunMode::Window,
        RunMode::Offscreen(Offscreen {
            size: (64, 64),
            ms: 100,
            picture: None,
        }),
    ] {
        let err = EngineBuilder::new()
            .add_plugins(DefaultPlugins)
            .run_mode(mode)
            .run()
            .unwrap_err();
        assert!(err.to_string().contains("no game to show"), "{err}");
    }
}

#[test]
fn the_web_mode_is_for_browser_builds() {
    let err = EngineBuilder::new()
        .add_plugins(DefaultPlugins)
        .run_mode(RunMode::Web)
        .run()
        .unwrap_err();
    assert!(err.to_string().contains("only in a browser build"), "{err}");
}

/// A game that is never started; only its type matters.
struct Nothing;

impl App for Nothing {
    fn host(&mut self) -> &mut Host {
        unreachable!("the test never runs the game")
    }

    fn host_ref(&self) -> &Host {
        unreachable!("the test never runs the game")
    }

    fn title(&self) -> String {
        String::new()
    }

    fn log_name(&self) -> &str {
        ""
    }

    fn view(&self, _: &Gpu) -> WorldView {
        unreachable!("the test never runs the game")
    }

    fn advance(&mut self, _: u32) {}

    fn frame(&mut self, _: &mut WorldView) -> (GuiDraws, Eye) {
        unreachable!("the test never runs the game")
    }
}

#[test]
fn naming_two_games_fails_the_build() {
    let err = EngineBuilder::new()
        .add_plugins(DefaultPlugins)
        .app(|_: &Engine| Ok(Nothing))
        .app(|_: &Engine| Ok(Nothing))
        .dedicated(Dedicated::new(files(), memory(), "startIt"))
        .run()
        .unwrap_err();
    assert!(matches!(err, RunError::Build(_)), "{err:?}");
}

#[test]
fn a_game_that_cannot_be_made_comes_out_of_run() {
    let err = EngineBuilder::new()
        .add_plugins(DefaultPlugins)
        .app(|_: &Engine| -> Result<Nothing, String> { Err("the files are not there".into()) })
        .run_mode(RunMode::Offscreen(Offscreen {
            size: (64, 64),
            ms: 100,
            picture: None,
        }))
        .run()
        .unwrap_err();
    assert_eq!(err, RunError::Failed("the files are not there".into()));
}
