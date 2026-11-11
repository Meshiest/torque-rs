//! A client over its server ([`ClientBuilder::over_server`]): the two share one script engine, so the server's datablocks are
//! the client's objects and a client's script can host the game in that engine; the client host ticks the server. A client
//! of its own is made as before.

use std::cell::Cell;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;

use torque_client::conn::tge::TgeConn;
use torque_engine::{
    ClientStage, Engine, EngineBuilder, Plugin, PluginGroup, PluginId, PluginType, ProtocolKind,
};
use torque_gui::GuiDraws;
use torque_render_core::Gpu;
use torque_render_hd::ext::NoExtension;
use torque_render_scene::source::Eye;
use torque_server::server::TICK_MS;
use torque_ts_text::atoi;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Native;

use crate::client::{ClientBuilder, ClientNet, GameClient, carry_between};
use crate::orbit::Orbit;
use crate::plugins::{
    ClientParts, DefaultPlugins, GuiPlugin, OffscreenRunnerPlugin, WindowRunnerPlugin, set_protocol,
};
use crate::run::EngineRun;
use crate::server::{GameServer, Listen, ServerStart};
use crate::shots::FRAME_MS;
use crate::slots::HostSlots;
use crate::view::WorldView;
use crate::{GameApp, Host};

const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);
const REMOTE: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_001);

/// A plugin that runs `build` on the engine builder.
struct Setup(Box<dyn Fn(&mut EngineBuilder)>);

impl Plugin for Setup {
    fn build(&self, engine: &mut EngineBuilder) {
        (self.0)(engine);
    }
}

impl PluginType for Setup {
    const ID: PluginId = PluginId::new("test.over_server");
}

/// The default plugins in Torque's protocol, the window's runner off, and `build` beside them.
fn builder(build: impl Fn(&mut EngineBuilder) + 'static) -> EngineBuilder {
    let mut builder = EngineBuilder::new();
    builder.add_plugins((
        DefaultPlugins.disable::<WindowRunnerPlugin>(),
        Setup(Box::new(build)),
    ));
    set_protocol(&mut builder, ProtocolKind::Tge);
    builder
}

fn engine(build: impl Fn(&mut EngineBuilder) + 'static) -> Engine {
    builder(build).build().expect("the engine builds")
}

fn files(entries: &[(&str, &str)]) -> MemFiles {
    let files = MemFiles::new();
    for (path, text) in entries {
        files.insert(path, text.as_bytes());
    }
    files
}

/// A server of `engine` over `files` on the memory transport, its start function `startNothing`.
fn start(engine: &Engine, files: MemFiles) -> GameServer {
    GameServer::start_engine(
        engine,
        files,
        &ServerStart {
            listen: Listen::Memory(SERVER),
            globals: &[],
            start: "startNothing",
            args: &[],
        },
    )
    .expect("the server starts")
}

fn eval(builder: &mut ClientBuilder<TgeConn, GameServer>, code: &str) -> String {
    String::from_utf8_lossy(&builder.vm.eval(code.as_bytes()).expect("the code runs")).into_owned()
}

const SERVER_CS: &str = "datablock CameraData(EyeData) {};\n\
    $Made = new ScriptObject(FromServer).getId();\n\
    function startNothing() {}\n\
    function serverSide() { return \"server\"; }\n";

#[test]
fn over_server_shares_the_servers_objects_and_a_datablock_is_the_clients() {
    let engine = engine(|engine| {
        engine.script().global("$Test::Both", "default");
    });
    let mut server = start(
        &engine,
        files(&[(
            "server.cs",
            &format!("{SERVER_CS}$Test::Both = \"server\";"),
        )]),
    );
    let datablock = server.call("nameToID", &["EyeData"]).expect("nameToID");
    assert!(atoi(datablock.as_bytes()) > 0, "{datablock}");
    let made = server
        .vm()
        .global("$Made")
        .expect("the server made an object");
    let mut builder = ClientBuilder::<TgeConn>::over_server(&engine, server, ClientNet::None)
        .expect("the client is made over the server");
    // The server's datablock and functions are the client's.
    assert_eq!(
        eval(
            &mut builder,
            "return nameToID(EyeData) SPC EyeData.getClassName();"
        ),
        format!("{datablock} CameraData")
    );
    assert_eq!(eval(&mut builder, "return serverSide();"), "server");
    // The client's stages left the server's globals of both sides as its scripts set them.
    assert_eq!(eval(&mut builder, "return $Test::Both;"), "server");
    // One counter: the client's objects come after the server's.
    let after = eval(&mut builder, "return new ScriptObject(FromClient).getId();");
    assert!(
        atoi(after.as_bytes()) > atoi(&made),
        "{after} after {made:?}"
    );
    // The client's GUI classes are in the same engine.
    assert_eq!(
        eval(&mut builder, "return isObject(new GuiControl(ClientGui));"),
        "1"
    );
    let mut client: GameClient<GameServer, TgeConn, GameServer> = builder.build();
    // The client host's server host is the server: its script engine is the client's, and the client's steps tick it.
    assert_eq!(
        client
            .host
            .server
            .call("eval", &["return nameToID(FromClient);"]),
        client.call("nameToID", &["FromClient"])
    );
    let before = client.host.server.vm().sim_time();
    client.step(10 * TICK_MS);
    assert!(client.host.server.vm().sim_time() >= before + 9 * TICK_MS);
}

#[test]
fn a_client_over_a_server_lists_the_server_natives_it_replaced_and_one_of_its_own_none() {
    let engine = engine(|engine| {
        engine.server(torque_engine::ServerStage::Game, |vm, _| {
            vm.register("whichHalf", 1, 1, "whichHalf();", |_, _| {
                Native::Str(b"server".to_vec())
            });
            Ok(())
        });
        engine.client(torque_engine::ClientStage::Natives, |vm, _| {
            vm.register("whichHalf", 1, 1, "whichHalf();", |_, _| {
                Native::Str(b"client".to_vec())
            });
            Ok(())
        });
    });
    let own =
        ClientBuilder::from_engine(&engine, None, ClientNet::None).expect("a client of its own");
    assert!(own.replaced_natives().is_empty());
    let server = start(&engine, files(&[("server.cs", SERVER_CS)]));
    let mut builder = ClientBuilder::<TgeConn>::over_server(&engine, server, ClientNet::None)
        .expect("the client is made over the server");
    assert_eq!(eval(&mut builder, "return whichHalf();"), "client");
    assert!(
        builder.replaced_natives().iter().any(|n| n == "whichHalf"),
        "{:?}",
        builder.replaced_natives()
    );
}

/// The server half's entry: nothing hosted, connections off, and `allowConnections(bool)` for the client's script.
fn hosting_engine() -> Engine {
    engine(|engine| {
        engine.server_entry(|entry| {
            let connections = entry.connections.clone();
            entry.vm.register(
                "allowConnections",
                2,
                2,
                "allowConnections(bool);",
                move |_, call| {
                    connections.allow(call.argv.get(1).is_some_and(|a| atoi(a) != 0));
                    Native::Void
                },
            );
            Ok(())
        });
    })
}

/// The host's files: its client's entry (`main.cs`) and the scripts `createServer` runs in the same engine.
fn host_files() -> MemFiles {
    files(&[
        (
            "main.cs",
            "function createServer() { exec(\"server/game.cs\"); allowConnections(1); }",
        ),
        (
            "server/game.cs",
            "datablock CameraData(HostEye) {};\n\
             function GameConnection::onConnect(%client) { $Joined++; }",
        ),
    ])
}

/// A player's client of its own on the memory link to the host's server.
fn remote(engine: &Engine) -> GameClient {
    let mut builder = ClientBuilder::from_engine(
        engine,
        Some(files(&[(
            "remote.cs",
            "function join() { if (isObject(ServerConnection)) ServerConnection.delete();\n\
             new GameConnection(ServerConnection); ServerConnection.connect(\"127.0.0.1:28000\"); }\n\
             function GameConnection::onConnectionAccepted(%this) { $Accepted = 1; }",
        )])),
        ClientNet::Memory {
            client: REMOTE,
            server: SERVER,
        },
    )
    .expect("the remote client is made");
    builder.exec("remote.cs").expect("the remote's script");
    builder.build()
}

/// `ticks` ticks of the host and of the remote, the link carried around each.
fn run(
    host: &mut GameClient<GameServer, TgeConn, GameServer>,
    remote: &mut GameClient,
    ticks: u32,
) {
    for _ in 0..ticks {
        host.step(TICK_MS);
        carry_between(&host.host.server, &[&*remote]);
        remote.step(TICK_MS);
        carry_between(&host.host.server, &[&*remote]);
    }
}

#[test]
fn a_clients_create_server_hosts_the_game_in_its_own_script_engine() {
    let engine = hosting_engine();
    let server = start(&engine, host_files());
    let mut builder = ClientBuilder::<TgeConn>::over_server(&engine, server, ClientNet::None)
        .expect("the host's client");
    builder.exec_entry().expect("the host's main.cs");
    let mut host = builder.build();
    let mut remote = remote(&engine);
    // Nothing is hosted yet: the server half takes no connection.
    remote.call("join", &[]).expect("join");
    run(&mut host, &mut remote, 40);
    assert!(!remote.flag("$Accepted"), "{:?}", host.server_log);
    // The client's script hosts the game in the engine it shares with the server half.
    host.call("createServer", &[]).expect("createServer");
    assert_eq!(host.call("isObject", &["HostEye"]).as_deref(), Ok("1"));
    remote.call("join", &[]).expect("join");
    run(&mut host, &mut remote, 80);
    assert!(remote.flag("$Accepted"), "{:?}", remote.client_log);
    // The server half's callback ran in the host's engine: the host's client reads what it wrote.
    assert_eq!(host.global("$Joined"), "1");
}

/// A game whose client runs over its server: the runners drive the server's ticks through the client host.
struct Shared {
    client: GameClient<GameServer, TgeConn, GameServer>,
    frames: Rc<Cell<u32>>,
    server_ms: Rc<Cell<u32>>,
}

impl GameApp for Shared {
    type Conn = TgeConn;
    type Server = GameServer;
    type Ext = NoExtension;

    fn host(&mut self) -> &mut Host<TgeConn, GameServer> {
        &mut self.client.host
    }

    fn host_ref(&self) -> &Host<TgeConn, GameServer> {
        &self.client.host
    }

    fn title(&self) -> String {
        "shared".to_string()
    }

    fn log_name(&self) -> &str {
        "shared"
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        WorldView::new(gpu)
    }

    fn advance(&mut self, ms: u32) {
        self.client.step(ms);
    }

    fn frame(&mut self, _: &mut WorldView) -> (GuiDraws, Eye) {
        self.frames.set(self.frames.get() + 1);
        self.server_ms.set(self.client.host.server.vm().sim_time());
        (
            self.client.host.draw_frame(),
            Orbit::default().eye([0.0; 3]),
        )
    }
}

#[test]
fn app_for_draws_64_offscreen_frames_of_a_game_whose_server_host_is_its_server() {
    if let Err(e) = crate::window::offscreen_gpu() {
        eprintln!("skipped: {e}");
        return;
    }
    let frames = Rc::new(Cell::new(0));
    let server_ms = Rc::new(Cell::new(0));
    let (counted, timed) = (Rc::clone(&frames), Rc::clone(&server_ms));
    let mut engine = builder(|_| {});
    engine.add_plugins(OffscreenRunnerPlugin::new((96, 54), 64 * FRAME_MS));
    engine
        .app_for(move |engine: &Engine| {
            let server = start(engine, files(&[("server.cs", SERVER_CS)]));
            let client =
                ClientBuilder::<TgeConn>::over_server(engine, server, ClientNet::None)?.build();
            Ok(Shared {
                client,
                frames: Rc::clone(&counted),
                server_ms: Rc::clone(&timed),
            })
        })
        .run()
        .expect("the game runs");
    assert_eq!(frames.get(), 64);
    // The server's clock is the client's: 64 frames of 16 ms on top of the start-up's first pass.
    assert!(server_ms.get() >= 63 * FRAME_MS, "{}", server_ms.get());
}

/// A server of `engine` over `files` on the memory transport at `at`, its start function `startNothing`.
fn start_at(engine: &Engine, files: MemFiles, at: SocketAddr) -> GameServer {
    GameServer::start_engine(
        engine,
        files,
        &ServerStart {
            listen: Listen::Memory(at),
            globals: &[],
            start: "startNothing",
            args: &[],
        },
    )
    .expect("the server starts")
}

/// The methods a client's connection natives take over, called on objects of the server's.
const METHODS_CS: &str = "function startNothing() {}\n\
    datablock PlayerData(Body) { shapeFile = \"body.dts\"; boundingBox = \"1 1 2\"; maxDamage = 100; };\n\
    new Player(Pl) { dataBlock = Body; };\n\
    function methods() {\n\
      %c = new GameConnection(SrvConn);\n\
      %out = isObject(Pl) SPC %c.getAddress() SPC %c.getPing() SPC %c.getPacketLoss() SPC %c.getControlObject()\n\
        SPC %c.getGhostID(Pl) SPC Pl.getState();\n\
      %c.delete();\n\
      return %out SPC isObject(SrvConn);\n\
    }\n";

/// The methods' server files: the script and the Player's shape.
fn methods_files() -> MemFiles {
    let files = files(&[("server.cs", METHODS_CS)]);
    let bounds = [-0.5, -0.5, 0.0, 0.5, 0.5, 2.0];
    let mut body = torque_shapes::synth::hull_shape(
        bounds,
        &[("detail2", bounds)],
        &["eye", "cam"],
        &["root", "look", "hold"],
    );
    // The synthetic shape names its root node `root` too; a shape's names are unique.
    body.shape.names[1] = b"base".to_vec();
    files.insert("body.dts", &torque_shapes::write::write_dts(&body));
    files
}

#[test]
fn the_replaced_methods_answer_for_the_servers_connections_and_players_as_the_server_does() {
    let engine = engine(|_| {});
    let mut alone = start(&engine, methods_files());
    let expected = alone.call("methods", &[]).expect("methods");
    assert!(expected.starts_with("1 "), "the Player is made: {expected}");
    let server = start(&engine, methods_files());
    let mut builder = ClientBuilder::<TgeConn>::over_server(&engine, server, ClientNet::External)
        .expect("the client is made over the server");
    for method in [
        "GameConnection::delete",
        "GameConnection::getControlObject",
        "NetConnection::getAddress",
        "NetConnection::getGhostID",
        "NetConnection::getPacketLoss",
        "NetConnection::getPing",
        "Player::getState",
    ] {
        assert!(
            builder.replaced_natives().iter().any(|n| n == method),
            "{method}: {:?}",
            builder.replaced_natives()
        );
    }
    assert_eq!(eval(&mut builder, "return methods();"), expected);
}

/// A plugin that makes the client's GUI at the `Gui` stage itself, as a game's own GUI plugin does.
fn own_gui(engine: &mut EngineBuilder) {
    engine.client(ClientStage::Gui, |vm, _| {
        let parts = vm
            .extension::<ClientParts>()
            .ok_or("no client is being made")?;
        parts.set_gui(torque_gui::register(vm))
    });
}

#[test]
fn a_games_own_gui_plugin_gives_a_client_over_its_server_its_gui_and_none_of_the_engines_prefs() {
    let mut builder = EngineBuilder::new();
    builder.add_plugins((
        DefaultPlugins
            .disable::<WindowRunnerPlugin>()
            .disable::<GuiPlugin>(),
        Setup(Box::new(own_gui)),
    ));
    set_protocol(&mut builder, ProtocolKind::Tge);
    let engine = builder.build().expect("the engine builds");
    let server = start(&engine, files(&[("server.cs", SERVER_CS)]));
    let mut client = ClientBuilder::<TgeConn>::over_server(&engine, server, ClientNet::None)
        .expect("the client is made over the server with the game's GUI");
    assert!(engine.warnings().is_empty(), "{:?}", engine.warnings());
    assert_eq!(eval(&mut client, "return serverSide();"), "server");
    // The screen's natives are made over the game's GUI.
    assert_eq!(eval(&mut client, "return isFunction(setScreenMode);"), "1");
    // The engine's GUI plugin declares its prefs; the game's declares none.
    assert_eq!(eval(&mut client, "return $Pref::Gui::SmoothScroll;"), "");
}

#[test]
fn the_engines_gui_plugin_declares_its_prefs_on_a_client_over_its_server() {
    let engine = engine(|_| {});
    let server = start(&engine, files(&[("server.cs", SERVER_CS)]));
    let mut client = ClientBuilder::<TgeConn>::over_server(&engine, server, ClientNet::None)
        .expect("the client is made over the server with the engine's GUI");
    assert_ne!(eval(&mut client, "return $Pref::Gui::SmoothScroll;"), "");
}

#[test]
fn a_second_gui_is_refused() {
    let engine = engine(own_gui);
    let server = start(&engine, files(&[("server.cs", SERVER_CS)]));
    let Err(error) = ClientBuilder::<TgeConn>::over_server(&engine, server, ClientNet::None) else {
        panic!("two GUI plugins made the client's GUI");
    };
    assert!(error.contains("two GUI plugins"), "{error}");
}

#[test]
fn a_client_over_a_server_in_the_engines_protocol_with_an_rpc_is_refused() {
    let mut builder = builder(|engine| {
        engine
            .net()
            .rpc::<(), ()>("test.ping")
            .to_server()
            .server(|_, ()| Ok(()));
    });
    set_protocol(&mut builder, ProtocolKind::Engine);
    let engine = builder.build().expect("the engine builds");
    let server = start(&engine, files(&[("server.cs", SERVER_CS)]));
    let Err(error) = ClientBuilder::<TgeConn>::over_server(&engine, server, ClientNet::External)
    else {
        panic!("the client is made over RPC lanes it would share");
    };
    assert!(error.contains("RPC lanes"), "{error}");
    // A client of its own carries them.
    assert!(ClientBuilder::from_engine(&engine, None, ClientNet::External).is_ok());
}

const OTHER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_002);
const HOST: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_003);

#[test]
fn a_servers_command_handler_on_the_shared_engine_sends_nothing_on_the_clients_connection() {
    let engine = engine(|_| {});
    // The server the host's client joins, counting the `Hello` commands it gets.
    let mut other = start_at(
        &engine,
        files(&[(
            "server.cs",
            "function startNothing() {}\nfunction serverCmdHello(%client) { $Hellos++; }",
        )]),
        OTHER,
    );
    // The host's own server half: a remote player's `Relay` makes its handler call `commandToServer`.
    let hosted = start_at(
        &engine,
        files(&[(
            "server.cs",
            "function startNothing() {}\n\
             function serverCmdRelay(%client) { $Relayed++; commandToServer('Hello'); }\n\
             function joinOther() { new GameConnection(ServerConnection); ServerConnection.connect(\"127.0.0.1:28002\"); }\n\
             function GameConnection::onConnectionAccepted(%this) { $Accepted = 1; }",
        )]),
        SERVER,
    );
    let mut host = ClientBuilder::<TgeConn>::over_server(
        &engine,
        hosted,
        ClientNet::Memory {
            client: HOST,
            server: OTHER,
        },
    )
    .expect("the host's client")
    .build::<GameServer>();
    let mut remote = remote(&engine);
    let tick = |host: &mut GameClient<GameServer, TgeConn, GameServer>,
                remote: &mut GameClient,
                other: &mut GameServer| {
        host.step(TICK_MS);
        carry_between(other, &[&*host]);
        other.tick();
        carry_between(other, &[&*host]);
        carry_between(&host.host.server, &[&*remote]);
        remote.step(TICK_MS);
        carry_between(&host.host.server, &[&*remote]);
    };
    host.call("joinOther", &[]).expect("joinOther");
    remote.call("join", &[]).expect("join");
    for _ in 0..80 {
        tick(&mut host, &mut remote, &mut other);
    }
    assert!(host.flag("$Accepted"), "{:?}", host.client_log);
    assert!(remote.flag("$Accepted"), "{:?}", remote.client_log);
    // The host's own script sends on its connection.
    host.call("eval", &["commandToServer('Hello');"])
        .expect("commandToServer");
    for _ in 0..20 {
        tick(&mut host, &mut remote, &mut other);
    }
    assert_eq!(other.vm().global("$Hellos").as_deref(), Some(&b"1"[..]));
    // A remote player's command runs the server's handler, whose commandToServer is the server's own: nothing is sent.
    remote
        .call("eval", &["commandToServer('Relay');"])
        .expect("commandToServer");
    for _ in 0..20 {
        tick(&mut host, &mut remote, &mut other);
    }
    assert_eq!(host.global("$Relayed"), "1");
    assert_eq!(other.vm().global("$Hellos").as_deref(), Some(&b"1"[..]));
}
