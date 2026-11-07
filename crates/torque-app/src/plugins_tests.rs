//! The default plugins: their order and stages, that a disabled one removes exactly its natives, and that one engine makes
//! any number of servers and clients that share nothing.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;

use torque_client::conn::tge::TgeConn;
use torque_client::state::ClientState;
use torque_engine::{
    ClientStage, Engine, EngineBuilder, Plugin, PluginGroup, PluginId, PluginType, ServerStage,
    Stage,
};
use torque_game::scene::container::{Container, ContainerConfig};
use torque_game::sim_classes::SimClasses;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

use crate::client::{ClientBuilder, ClientNet, ClientSetup};
use crate::plugins::{
    ActionMapPlugin, AudioPlugin, ClientHostPlugin, ConnPlugin, DefaultPlugins, GameClassesPlugin,
    GuiPlugin, LivePerfPlugin, MinimalPlugins, NetHostPlugin, ScreenPlugin, ServerGamePlugin,
    ServerQueryPlugin, SimClassesPlugin, TerrainPlugin, WaterPlugin, WebPageRunnerPlugin,
    WindowRunnerPlugin, ZoomPlugin,
};
use crate::server::{GameServer, Listen, ServerStart};

/// The runner `DefaultPlugins` has on this build: the window natively, the page on the web.
fn default_runner() -> PluginId {
    if cfg!(target_arch = "wasm32") {
        WebPageRunnerPlugin::ID
    } else {
        WindowRunnerPlugin::ID
    }
}

fn engine(group: impl PluginGroup) -> Engine {
    EngineBuilder::new()
        .add_plugins(group)
        .build()
        .expect("the engine builds")
}

fn start_with_files(engine: &Engine) -> Result<GameServer, String> {
    let files = MemFiles::new();
    files.insert("server.cs", b"function startNothing() {}");
    GameServer::start_engine(
        engine,
        files,
        &ServerStart {
            listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
            globals: &[],
            start: "startNothing",
            args: &[],
        },
    )
}

fn server(engine: &Engine) -> GameServer {
    start_with_files(engine).expect("the server starts")
}

fn client(engine: &Engine, net: ClientNet) -> ClientBuilder {
    ClientBuilder::from_engine(engine, None, net).expect("the client is made")
}

fn lines(listing: &str) -> BTreeSet<String> {
    listing.lines().map(str::to_owned).collect()
}

/// The lines `after` has that `before` lacks.
fn added(before: &str, after: &str) -> BTreeSet<String> {
    lines(after).difference(&lines(before)).cloned().collect()
}

#[test]
fn the_group_lists_its_plugins_in_the_order_the_installs_ran() {
    let engine = engine(DefaultPlugins);
    assert_eq!(
        engine.plugins(),
        [
            GameClassesPlugin::ID,
            ServerGamePlugin::ID,
            TerrainPlugin::ID,
            WaterPlugin::ID,
            NetHostPlugin::ID,
            GuiPlugin::ID,
            ActionMapPlugin::ID,
            ScreenPlugin::ID,
            ConnPlugin::ID,
            ServerQueryPlugin::ID,
            AudioPlugin::ID,
            ClientHostPlugin::ID,
            default_runner(),
        ]
    );
    assert!(engine.warnings().is_empty(), "{:?}", engine.warnings());
}

#[test]
fn the_plugin_ids_are_the_stable_names() {
    let ids: Vec<&str> = engine(DefaultPlugins)
        .plugins()
        .iter()
        .map(|id| id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "torque.game-classes",
            "torque.server-game",
            "torque.terrain",
            "torque.water",
            "torque.net-host",
            "torque.gui",
            "torque.action-map",
            "torque.screen",
            "torque.conn",
            "torque.server-query",
            "torque_audio::AudioPlugin",
            "torque.frame-host",
            if cfg!(target_arch = "wasm32") {
                "torque.web-page-runner"
            } else {
                "torque.window-runner"
            },
        ]
    );
}

#[test]
fn each_stage_replays_its_plugins_in_add_order() {
    let engine = engine(DefaultPlugins);
    let at = |stage: Stage| -> Vec<&'static str> {
        engine.recipes(stage).map(|id| id.as_str()).collect()
    };
    let server = |stage| at(Stage::Server(stage));
    assert_eq!(server(ServerStage::Classes), ["torque.game-classes"]);
    assert!(server(ServerStage::Prepare).is_empty());
    assert_eq!(server(ServerStage::Game), ["torque.server-game"]);
    assert_eq!(
        server(ServerStage::World),
        ["torque.terrain", "torque.water"]
    );
    assert_eq!(server(ServerStage::Net), ["torque.net-host"]);

    let client = |stage| at(Stage::Client(stage));
    assert_eq!(client(ClientStage::Classes), ["torque.game-classes"]);
    assert!(client(ClientStage::Game).is_empty());
    assert_eq!(client(ClientStage::Gui), ["torque.gui"]);
    assert_eq!(client(ClientStage::Input), ["torque.action-map"]);
    assert_eq!(client(ClientStage::Screen), ["torque.screen"]);
    assert_eq!(client(ClientStage::Conn), ["torque.conn"]);
    assert_eq!(client(ClientStage::Query), ["torque.server-query"]);
    assert_eq!(client(ClientStage::Natives), ["torque_audio::AudioPlugin"]);
    assert!(client(ClientStage::Host).is_empty());
    // The page bridge is filed for web builds only, by the web page runner.
    assert_eq!(
        client(ClientStage::Window),
        if cfg!(target_arch = "wasm32") {
            vec!["torque.web-page-runner"]
        } else {
            vec![]
        }
    );
}

/// A plugin that notes each stage it is replayed at, in a global of the script engine.
#[derive(torque_engine::PluginId)]
#[plugin(id = "test.trace")]
struct Trace;

fn note(vm: &mut Vm, global: &str, text: &str) {
    let mut seen = vm.global(global).unwrap_or_default();
    seen.extend_from_slice(text.as_bytes());
    vm.set_global_text(global.as_bytes(), &seen);
}

impl Plugin for Trace {
    fn build(&self, engine: &mut EngineBuilder) {
        for &stage in ServerStage::ALL {
            engine.server(stage, move |vm, _| {
                note(vm, "$Trace::Server", &format!("{stage:?} "));
                Ok(())
            });
        }
        for &stage in ClientStage::ALL {
            engine.client(stage, move |vm, _| {
                note(vm, "$Trace::Client", &format!("{stage:?} "));
                Ok(())
            });
        }
    }
}

fn traced() -> Engine {
    engine(PluginGroup::build(DefaultPlugins).add(Trace))
}

#[test]
fn a_server_replays_every_server_stage_in_order_and_no_client_stage() {
    let server = server(&traced());
    let trace = |name: &str| {
        server
            .vm()
            .global(name)
            .map(|v| String::from_utf8_lossy(&v).into_owned())
    };
    assert_eq!(
        trace("$Trace::Server").as_deref(),
        Some("Classes Prepare Game World Net ")
    );
    assert_eq!(trace("$Trace::Client"), None);
}

#[test]
fn a_client_replays_every_client_stage_in_order_and_no_server_stage() {
    let builder = client(&traced(), ClientNet::None);
    // The window's stage is the window's own; the host's runs as the host is made.
    let trace = |vm: &Vm, name: &str| {
        vm.global(name)
            .map(|v| String::from_utf8_lossy(&v).into_owned())
    };
    assert_eq!(
        trace(&builder.vm, "$Trace::Client").as_deref(),
        Some("Classes Game Gui Input Screen Conn Query Natives ")
    );
    assert_eq!(trace(&builder.vm, "$Trace::Server"), None);
    let client = builder.build::<GameServer>();
    assert_eq!(
        client
            .vm()
            .global("$Trace::Client")
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .as_deref(),
        Some("Classes Game Gui Input Screen Conn Query Natives Host ")
    );
}

/// Terrain and water add behavior to classes the server game registered, not natives of their own: the native lists agree, and
/// both match a bare script engine's before and after the install.
#[test]
fn disabling_a_server_plugin_removes_exactly_its_natives() {
    let full = server(&engine(DefaultPlugins)).vm().native_listing();
    for (name, expected) in [
        ("water", {
            let container = Rc::new(Container::new(ContainerConfig::default()));
            let mut vm = Vm::new();
            let before = vm.native_listing();
            crate::water::install(&mut vm, &container);
            added(&before, &vm.native_listing())
        }),
        ("terrain", {
            let container = Rc::new(Container::new(ContainerConfig::default()));
            let mut vm = Vm::new();
            let before = vm.native_listing();
            crate::terrain::install(&mut vm, &container);
            added(&before, &vm.native_listing())
        }),
    ] {
        let without = match name {
            "water" => engine(DefaultPlugins.disable::<WaterPlugin>()),
            _ => engine(DefaultPlugins.disable::<TerrainPlugin>()),
        };
        let without = server(&without).vm().native_listing();
        assert_eq!(added(&without, &full), expected, "{name}");
        assert!(added(&full, &without).is_empty(), "{name} adds nothing");
    }
}

#[test]
fn disabling_a_client_plugin_removes_exactly_its_natives() {
    let full = client(&engine(DefaultPlugins), ClientNet::None)
        .vm
        .native_listing();

    let query = engine(DefaultPlugins.disable::<ServerQueryPlugin>());
    let without = client(&query, ClientNet::None).vm.native_listing();
    let mut vm = Vm::new();
    let state = Rc::new(RefCell::new(ClientState::<TgeConn>::default()));
    let before = vm.native_listing();
    torque_client::query::register(&mut vm, &state);
    let expected = added(&before, &vm.native_listing());
    assert!(!expected.is_empty());
    assert_eq!(added(&without, &full), expected);
    assert!(added(&full, &without).is_empty());

    let classes = engine(DefaultPlugins.disable::<GameClassesPlugin>());
    let without = client(&classes, ClientNet::None).vm.native_listing();
    let mut vm = Vm::new();
    let before = vm.native_listing();
    torque_game::specs::install(&mut vm);
    let expected = added(&before, &vm.native_listing());
    assert_eq!(added(&without, &full), expected);
    assert!(added(&full, &without).is_empty());

    let maps = engine(DefaultPlugins.disable::<ActionMapPlugin>());
    let without = client(&maps, ClientNet::None).vm.native_listing();
    let mut vm = Vm::new();
    let before = vm.native_listing();
    vm.register_class("ActionMap", "SimObject", &[]);
    torque_client::actionmap::register(&mut vm, &state);
    let expected = added(&before, &vm.native_listing());
    assert!(!expected.is_empty());
    assert_eq!(added(&without, &full), expected);
    assert!(added(&full, &without).is_empty());
}

/// Audio is the one client plugin whose natives were not wired before: turned off, the client has exactly the natives it had
/// (`tests/natives/client-no-audio.txt` in the natives test), and turned on it has the audio layer's and nothing else.
#[test]
fn disabling_the_audio_plugin_removes_exactly_the_audio_natives() {
    let full = client(&engine(DefaultPlugins), ClientNet::None)
        .vm
        .native_listing();
    let off = engine(DefaultPlugins.disable::<AudioPlugin>());
    assert!(!off.is_plugin_added::<AudioPlugin>());
    let without = client(&off, ClientNet::None).vm.native_listing();

    let mut vm = Vm::new();
    let before = vm.native_listing();
    torque_audio::natives::install_classes(&mut vm);
    torque_audio::natives::register(
        &mut vm,
        &Rc::new(RefCell::new(torque_audio::natives::AudioHost::default())),
        |host| host,
        torque_audio::natives::AlxProfile::tge(),
    );
    let expected = added(&before, &vm.native_listing());
    assert!(expected.iter().any(|l| l.contains("alxPlay")));
    assert_eq!(added(&without, &full), expected);
    assert!(added(&full, &without).is_empty());
    assert!(!without.contains("alxPlay"));
}

/// The plugin builds with no sound card: no device is opened, and the client's own mixer is the null sink.
#[test]
fn the_audio_plugin_needs_no_sound_card() {
    let mut builder = client(&engine(DefaultPlugins), ClientNet::None);
    let host = AudioPlugin::host(&builder.vm).expect("every client has its audio state");
    assert!(host.borrow().device.is_none());
    assert_eq!(
        String::from_utf8_lossy(&builder.vm.eval(b"return OpenALInitDriver();").unwrap()),
        "1"
    );
    let off = engine(DefaultPlugins.disable::<AudioPlugin>());
    assert!(AudioPlugin::host(&client(&off, ClientNet::None).vm).is_none());
}

#[test]
fn zoom_and_live_perf_are_opt_in() {
    let default = client(&engine(DefaultPlugins), ClientNet::None)
        .vm
        .native_listing();
    for name in ["setFov", "setDefaultFov", "setZoomSpeed", "showPerf"] {
        assert!(!default.contains(name), "{name}");
    }
    let with = engine(
        PluginGroup::build(DefaultPlugins)
            .add(ZoomPlugin)
            .add(LivePerfPlugin::new("showPerf", "$pref::perf")),
    );
    let listing = client(&with, ClientNet::None).vm.native_listing();
    for name in ["setFov", "setDefaultFov", "setZoomSpeed", "showPerf"] {
        assert!(listing.contains(name), "{name}");
    }
    assert!(added(&listing, &default).is_empty());
}

#[test]
fn the_connection_plugin_registers_only_for_a_client_with_a_socket() {
    let engine = engine(DefaultPlugins);
    let none = client(&engine, ClientNet::None).vm.native_listing();
    let external = client(&engine, ClientNet::External).vm.native_listing();
    assert!(!added(&none, &external).is_empty());
    assert!(added(&external, &none).is_empty());
    let off = self::engine(DefaultPlugins.disable::<ConnPlugin>());
    assert_eq!(client(&off, ClientNet::External).vm.native_listing(), none);
}

#[test]
fn the_setup_flags_pick_the_plugins() {
    // Everything on: the same script engine as the whole group.
    let setup = ClientSetup {
        game_classes: true,
        server_query: true,
        net: ClientNet::External,
        ..ClientSetup::default()
    };
    let by_flags = ClientBuilder::new(setup).expect("the client");
    let by_engine = client(&engine(DefaultPlugins), ClientNet::External);
    assert_eq!(by_flags.vm.native_listing(), by_engine.vm.native_listing());

    // Everything off: no game classes, no query, no connection.
    let bare = ClientBuilder::new(ClientSetup::default()).expect("the client");
    let off = self::engine(
        DefaultPlugins
            .disable::<GameClassesPlugin>()
            .disable::<ServerQueryPlugin>(),
    );
    let by_engine = client(&off, ClientNet::None);
    assert_eq!(bare.vm.native_listing(), by_engine.vm.native_listing());
}

#[test]
fn two_clients_from_one_engine_share_nothing() {
    let engine = engine(DefaultPlugins);
    let mut a = client(&engine, ClientNet::External);
    let b = client(&engine, ClientNet::External);
    assert_eq!(a.vm.native_listing(), b.vm.native_listing());
    assert!(!Rc::ptr_eq(&a.gui.0, &b.gui.0));
    assert!(!Rc::ptr_eq(&a.state, &b.state));
    a.vm.set_global_text(b"$Only::A", b"1");
    assert_eq!(b.vm.global("$Only::A"), None);
}

#[test]
fn one_engine_starts_stops_and_restarts_a_server_with_the_same_natives() {
    let engine = engine(DefaultPlugins);
    let mut first = server(&engine);
    let listing = first.vm().native_listing();
    first.tick();
    first.close();
    drop(first);
    let again = server(&engine);
    assert_eq!(again.vm().native_listing(), listing);
    // Two at once from the one engine, too (on different links).
    let other = server(&engine);
    assert!(!Rc::ptr_eq(&again.game.container, &other.game.container));
}

#[test]
fn a_dedicated_server_has_no_client_natives() {
    let server = server(&engine(DefaultPlugins)).vm().native_listing();
    let client = client(&engine(DefaultPlugins), ClientNet::None)
        .vm
        .native_listing();
    let client_only = added(&server, &client);
    assert!(!client_only.is_empty());
    assert!(
        client_only.iter().any(|l| l.contains("ActionMap")),
        "the client has the action maps"
    );
    for line in &client_only {
        assert!(!lines(&server).contains(line));
    }
}

#[test]
fn a_server_needs_its_game_and_its_host() {
    for (off, name) in [
        (
            engine(DefaultPlugins.disable::<NetHostPlugin>()),
            "NetHostPlugin",
        ),
        (
            engine(DefaultPlugins.disable::<ServerGamePlugin>()),
            "ServerGamePlugin",
        ),
    ] {
        let Err(error) = start_with_files(&off) else {
            panic!("a server starts without its {name}");
        };
        assert!(error.contains(name), "{error}");
    }
}

#[test]
fn a_client_needs_its_gui_and_its_host() {
    let off = engine(DefaultPlugins.disable::<GuiPlugin>());
    let Err(error) = ClientBuilder::from_engine(&off, None, ClientNet::None) else {
        panic!("a client is made without a GUI");
    };
    assert!(error.contains("GuiPlugin"), "{error}");
    let off = engine(DefaultPlugins.disable::<ClientHostPlugin>());
    let Err(error) = ClientBuilder::from_engine(&off, None, ClientNet::None) else {
        panic!("a client is made without a host");
    };
    assert!(error.contains("ClientHostPlugin"), "{error}");
}

#[test]
fn the_builds_warnings_reach_the_console_once_per_start() {
    let mut builder = EngineBuilder::new();
    builder
        .add_plugins(DefaultPlugins)
        .disable_id("torque.nothing");
    let warned = builder.build().expect("the engine builds");
    assert_eq!(warned.warnings().len(), 1);

    let mut server = server(&warned);
    let log = server.drain_log();
    let hits = log.iter().filter(|l| l.contains("torque.nothing")).count();
    assert_eq!(hits, 1, "{log:?}");

    let builder = client(&warned, ClientNet::None);
    let hits = builder
        .vm
        .log_texts()
        .iter()
        .filter(|l| l.contains("torque.nothing"))
        .count();
    assert_eq!(hits, 1);

    let mut quiet = self::server(&engine(DefaultPlugins));
    assert!(quiet.drain_log().iter().all(|l| !l.contains("no plugin")));
}

struct NoClasses;
impl SimClasses for NoClasses {}

#[test]
fn the_sim_classes_slot_is_installed_before_the_server_game() {
    let classes: Rc<dyn SimClasses> = Rc::new(NoClasses);
    let mut builder = EngineBuilder::new();
    builder
        .add_plugins(DefaultPlugins)
        .add_plugins(SimClassesPlugin::new(Rc::clone(&classes)));
    let engine = builder.build().expect("the engine builds");
    let server = server(&engine);
    let installed = torque_game::sim_classes::sim_classes(server.vm());
    assert!(Rc::ptr_eq(
        &installed,
        &engine.slot::<dyn SimClasses>().expect("the slot")
    ));
}

#[test]
fn minimal_plugins_make_the_server_default_plugins_make() {
    let minimal = server(&engine(MinimalPlugins)).vm().native_listing();
    let default = server(&engine(DefaultPlugins)).vm().native_listing();
    assert_eq!(minimal, default);
}

#[test]
fn minimal_plugins_and_the_window_runner_do_not_go_together() {
    let err = EngineBuilder::new()
        .add_plugins((MinimalPlugins, WindowRunnerPlugin))
        .build()
        .expect_err("two runners fail the build");
    let message = err.to_string();
    assert!(message.contains("torque.window-runner"), "{message}");
    assert!(message.contains("torque.dedicated-runner"), "{message}");
}
