//! `Engine::run` over the runners built in: each runner is a plugin on one slot, a dedicated server beside `DefaultPlugins`'
//! other plugins, that two runners fail the build, that errors come out of `run`, and that no window or device is touched when
//! there is nothing to show.

use std::cell::{Cell, RefCell};
use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;

use torque_client::conn::tge::{TgeConn, TgeFeed, start_feed};
use torque_client::conn::{ConnGame, Connect, Started};
use torque_client::state::ClientState;
use torque_engine::{
    Engine, EngineBuilder, Plugin, PluginGroup, PluginType, RunError, Runner, ServerStage,
};
use torque_gui::GuiDraws;
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Native;

use crate::client::{ClientBuilder, ClientNet, GameClient};
use crate::orbit::Orbit;
use crate::plugins::{DefaultPlugins, WindowRunnerPlugin};
use crate::run::{
    Dedicated, DedicatedRunnerPlugin, EngineDedicated, EngineRun, OffscreenRunnerPlugin, Stop,
    WebPageRunnerPlugin,
};
use crate::server::{GameServer, Listen};
use crate::shots::FRAME_MS;
use crate::view::WorldView;
use crate::{App, GameApp, Host};

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
            DefaultPlugins.disable::<WindowRunnerPlugin>(),
            DedicatedRunnerPlugin,
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
        DefaultPlugins.disable::<WindowRunnerPlugin>(),
        DedicatedRunnerPlugin,
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
fn each_runner_is_a_plugin_that_sets_the_slot() {
    fn sets_the_slot(plugin: impl PluginType) {
        let engine = EngineBuilder::new()
            .add_plugins(plugin)
            .build()
            .expect("the engine builds");
        assert!(engine.slot::<dyn Runner>().is_some());
    }
    sets_the_slot(WindowRunnerPlugin);
    sets_the_slot(WebPageRunnerPlugin);
    sets_the_slot(OffscreenRunnerPlugin::new((64, 64), 100));
    sets_the_slot(DedicatedRunnerPlugin);
}

#[test]
fn two_runners_fail_the_build_naming_both() {
    let err = EngineBuilder::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(DedicatedRunnerPlugin)
        .dedicated(Dedicated::new(files(), memory(), "startIt"))
        .run()
        .unwrap_err();
    assert!(matches!(err, RunError::Build(_)), "{err:?}");
    let message = err.to_string();
    assert!(
        message.starts_with("the engine did not build:"),
        "{message}"
    );
    assert!(message.contains("torque.window-runner"), "{message}");
    assert!(message.contains("torque.dedicated-runner"), "{message}");
    assert!(message.contains("only one plugin may set it"), "{message}");
}

#[test]
fn the_default_runner_is_the_platforms_and_switching_is_a_toggle() {
    let default = EngineBuilder::new()
        .add_plugins(DefaultPlugins)
        .build()
        .unwrap();
    assert!(default.is_plugin_added::<WindowRunnerPlugin>());
    assert!(!default.is_plugin_added::<WebPageRunnerPlugin>());
    let swapped = EngineBuilder::new()
        .add_plugins((
            DefaultPlugins.disable::<WindowRunnerPlugin>(),
            OffscreenRunnerPlugin::new((64, 64), 100),
        ))
        .build()
        .unwrap();
    assert!(!swapped.is_plugin_added::<WindowRunnerPlugin>());
    assert!(swapped.is_plugin_added::<OffscreenRunnerPlugin>());
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
        .add_plugins((
            DefaultPlugins.disable::<WindowRunnerPlugin>(),
            DedicatedRunnerPlugin,
            Broken,
        ))
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
        .add_plugins(DefaultPlugins.disable::<WindowRunnerPlugin>())
        .run()
        .unwrap_err();
    assert_eq!(err, RunError::NoRunner);
}

#[test]
fn the_window_and_offscreen_runners_need_a_game_and_open_nothing_without_one() {
    let without_window = DefaultPlugins.disable::<WindowRunnerPlugin>();
    let err = EngineBuilder::new()
        .add_plugins(DefaultPlugins)
        .run()
        .unwrap_err();
    assert!(err.to_string().contains("no game to show"), "{err}");
    let err = EngineBuilder::new()
        .add_plugins((without_window, OffscreenRunnerPlugin::new((64, 64), 100)))
        .run()
        .unwrap_err();
    assert!(err.to_string().contains("no game to show"), "{err}");
}

#[test]
fn the_web_page_runner_is_for_browser_builds() {
    let err = EngineBuilder::new()
        .add_plugins(WebPageRunnerPlugin)
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
        .run()
        .unwrap_err();
    assert!(matches!(err, RunError::Build(_)), "{err:?}");
}

#[test]
fn a_game_that_cannot_be_made_comes_out_of_run() {
    let err = EngineBuilder::new()
        .add_plugins((
            DefaultPlugins.disable::<WindowRunnerPlugin>(),
            OffscreenRunnerPlugin::new((64, 64), 100),
        ))
        .app(|_: &Engine| -> Result<Nothing, String> { Err("the files are not there".into()) })
        .run()
        .unwrap_err();
    assert_eq!(err, RunError::Failed("the files are not there".into()));
}

#[test]
fn the_config_knows_the_runner_ids_the_engine_ships() {
    use crate::config::RUNNER_IDS;
    let mut ids = [
        WindowRunnerPlugin::ID,
        WebPageRunnerPlugin::ID,
        OffscreenRunnerPlugin::ID,
        DedicatedRunnerPlugin::ID,
    ]
    .map(|id| id.as_str());
    let mut known = RUNNER_IDS;
    ids.sort_unstable();
    known.sort_unstable();
    assert_eq!(known, ids);
}

#[test]
fn a_config_chooses_the_runner_by_id() {
    use crate::config::{EngineConfig, GameConfig};
    // Two runners fail the build; the config's choice turns the group's off.
    let mut clash = EngineBuilder::new();
    clash.add_plugins((DefaultPlugins, DedicatedRunnerPlugin));
    assert!(clash.build().is_err());

    let config = GameConfig::parse("[runner]\nid = \"torque.dedicated-runner\"\n").unwrap();
    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, DedicatedRunnerPlugin))
        .config(&config)
        .build()
        .expect("one runner is left");
    assert!(engine.is_plugin_added::<DedicatedRunnerPlugin>());
    assert!(!engine.is_plugin_added::<WindowRunnerPlugin>());
    assert!(engine.warnings().is_empty(), "{:?}", engine.warnings());

    // The other way round: the window's, with the dedicated one added but not wanted.
    let config = GameConfig::parse("[runner]\nid = \"torque.window-runner\"\n").unwrap();
    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, DedicatedRunnerPlugin))
        .config(&config)
        .build()
        .expect("one runner is left");
    assert!(engine.is_plugin_added::<WindowRunnerPlugin>());
    assert!(!engine.is_plugin_added::<DedicatedRunnerPlugin>());
}

/// A game that implements [`GameApp`] itself (it is no [`App`]) over a client of conn game `C` of a plain engine, counting
/// the frames drawn.
struct Counted<C: ConnGame> {
    client: GameClient<GameServer, C>,
    frames: Rc<Cell<u32>>,
}

impl<C: ConnGame> GameApp for Counted<C> {
    type Conn = C;

    fn host(&mut self) -> &mut Host<C> {
        &mut self.client.host
    }

    fn host_ref(&self) -> &Host<C> {
        &self.client.host
    }

    fn title(&self) -> String {
        "counted".to_string()
    }

    fn log_name(&self) -> &str {
        "counted"
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        WorldView::new(gpu)
    }

    fn advance(&mut self, ms: u32) {
        self.client.step(ms);
    }

    fn frame(&mut self, _: &mut WorldView) -> (GuiDraws, Eye) {
        self.frames.set(self.frames.get() + 1);
        (
            self.client.host.draw_frame(),
            Orbit::default().eye([0.0; 3]),
        )
    }
}

#[test]
fn app_for_names_a_game_app_and_the_offscreen_runner_draws_64_frames_of_it() {
    if let Err(e) = crate::window::offscreen_gpu() {
        eprintln!("skipped: {e}");
        return;
    }
    let frames = Rc::new(Cell::new(0));
    let counted = Rc::clone(&frames);
    EngineBuilder::new()
        .add_plugins((
            DefaultPlugins.disable::<WindowRunnerPlugin>(),
            OffscreenRunnerPlugin::new((96, 54), 64 * FRAME_MS),
        ))
        .app_for(move |engine: &Engine| {
            let client = ClientBuilder::from_engine(engine, None, ClientNet::None)?.build();
            Ok(Counted {
                client,
                frames: Rc::clone(&counted),
            })
        })
        .run()
        .expect("the game runs");
    assert_eq!(frames.get(), 64);
}

/// A conn game of the test's own: TGE's feed with parts of its own.
#[derive(Debug, Default)]
struct OwnConn;

impl ConnGame for OwnConn {
    type Feed = TgeFeed;
    type Parts = u32;

    const HANDSHAKE_WAIT_MS: u32 = TgeConn::HANDSHAKE_WAIT_MS;
    const HANDSHAKE_TRIES: u32 = TgeConn::HANDSHAKE_TRIES;

    fn start(
        vm: &mut torque_ts_vm::vm::Vm,
        client: &Rc<RefCell<ClientState<OwnConn>>>,
        connect: Connect,
    ) -> Result<Started<OwnConn>, String> {
        let _ = client;
        start_feed(vm, connect)
    }
}

#[test]
fn conn_game_app_for_draws_64_offscreen_frames_of_a_client_of_its_own_conn_game() {
    if let Err(e) = crate::window::offscreen_gpu() {
        eprintln!("skipped: {e}");
        return;
    }
    let frames = Rc::new(Cell::new(0));
    let counted = Rc::clone(&frames);
    EngineBuilder::new()
        .add_plugins((
            DefaultPlugins.disable::<WindowRunnerPlugin>(),
            OffscreenRunnerPlugin::new((96, 54), 64 * FRAME_MS),
        ))
        .app_for(move |engine: &Engine| {
            let client =
                ClientBuilder::<OwnConn>::from_engine_with_conn(engine, None, ClientNet::None)?
                    .build();
            Ok(Counted {
                client,
                frames: Rc::clone(&counted),
            })
        })
        .run()
        .expect("the game runs");
    assert_eq!(frames.get(), 64);
}

#[test]
fn app_for_is_the_same_exclusive_slot_as_app() {
    let err = EngineBuilder::new()
        .add_plugins(DefaultPlugins)
        .app(|_: &Engine| Ok(Nothing))
        .app_for(|_: &Engine| Ok(Nothing))
        .run()
        .unwrap_err();
    assert!(matches!(err, RunError::Build(_)), "{err:?}");
}

#[test]
fn every_app_is_a_game_app_over_the_tge_connection() {
    fn conn<A: GameApp>() -> std::any::TypeId {
        std::any::TypeId::of::<A::Conn>()
    }
    assert_eq!(conn::<Nothing>(), std::any::TypeId::of::<TgeConn>());
}
