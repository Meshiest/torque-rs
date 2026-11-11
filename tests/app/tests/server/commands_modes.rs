//! Script commands (`commandToServer`, `commandToClient`) on a real server and client over the memory transport, stepped by
//! hand, once with Torque's protocol and once with the engine's: the scripts' callbacks see the same arguments, tags and
//! order in both (the logs the test game writes are equal), and in the engine's protocol the commands that carry no limit of
//! their own are held to the default ones. No clock is read.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use torque_app::GameFiles;
use torque_app::client::{ClientBuilder, ClientNet, GameClient};
use torque_app::config::{EngineConfig, GameConfig};
use torque_app::plugins::DefaultPlugins;
use torque_app::server::{GameServer, Listen, ServerStart};
use torque_client::net::{DatagramQueue, MemoryDatagrams, NetPolicy};
use torque_engine::command::{ArgType, CommandSpec};
use torque_engine::{EngineBuilder, Plugin, PluginId};

const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_300);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Tge,
    Engine,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Mode::Tge => "tge",
            Mode::Engine => "engine",
        }
    }
}

#[derive(PluginId)]
struct Declared;

impl Plugin for Declared {
    fn build(&self, engine: &mut EngineBuilder) {
        // Four a second after a burst of eight; the function is the script's.
        engine.server_command_limits(
            "Flood",
            CommandSpec::new().arg("n", ArgType::Int).rate(4, 8),
        );
        // A permission nobody holds.
        engine.server_command_limits("Gated", CommandSpec::new().requires("test.gated"));
    }
}

struct Rig {
    server: GameServer,
    client: GameClient,
    addr: SocketAddrV4,
    sent: DatagramQueue,
    incoming: DatagramQueue,
    ticks: usize,
}

impl Rig {
    fn new(mode: Mode) -> Rig {
        let mut files = GameFiles::default();
        files.insert("server.cs", include_bytes!("commands_modes/server.cs"));
        files.insert("main.cs", include_bytes!("commands_modes/main.cs"));
        let toml = format!(
            "[game]\nname = \"Command mode test\"\nserver = \"server.cs\"\nclient = \"main.cs\"\n\n[net]\nprotocol = \"{}\"\n\n[plugins]\ndisable = [\"torque.audio\", \"torque.water\", \"torque.terrain\", \"torque.server-query\"]\n",
            mode.label()
        );
        files.insert("game.toml", toml.as_bytes());
        let config = GameConfig::load(&files).expect("game.toml");
        let mut builder = EngineBuilder::new();
        builder
            .add_plugins(DefaultPlugins)
            .config(&config)
            .add_plugins(Declared);
        let engine = builder.build().expect("the engine builds");
        let server = GameServer::start_engine(
            &engine,
            files.mem_files(),
            &ServerStart {
                listen: Listen::Memory(SERVER),
                globals: &[],
                start: "startIt",
                args: &[],
            },
        )
        .expect("the server starts");
        // The client has the same plugins: the limits are the server's, the manifests must agree.
        let mut client_builder = EngineBuilder::new();
        client_builder
            .add_plugins(DefaultPlugins)
            .config(&config)
            .add_plugins(Declared);
        let client_engine = client_builder.build().expect("the client's engine builds");
        let port = 50_301;
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        let mem = MemoryDatagrams {
            local_port: port,
            ..MemoryDatagrams::default()
        };
        let (sent, incoming) = (mem.sent.clone(), mem.incoming.clone());
        let mut builder = ClientBuilder::from_engine(
            &client_engine,
            Some(files.mem_files()),
            ClientNet::External,
        )
        .expect("the client builds");
        builder.set_socket(Box::new(mem), NetPolicy::Loopback);
        builder.exec_entry().expect("the client's script");
        let client: GameClient = builder.build();
        Rig {
            server,
            client,
            addr,
            sent,
            incoming,
            ticks: 0,
        }
    }

    /// A rig whose client has connected and whose server has run `onConnect`.
    fn connected(mode: Mode) -> Rig {
        let mut rig = Rig::new(mode);
        rig.client
            .call("connectTo", &[&SERVER.to_string()])
            .expect("connect");
        assert!(rig.run_until(400, |r| r.global("$Connected") == "1"));
        rig.run(10);
        rig
    }

    fn carry(&mut self) {
        let Some(end) = self.server.memory_end() else {
            return;
        };
        for (to, data) in self.sent.borrow_mut().drain(..) {
            if SocketAddr::V4(to) == SERVER {
                end.inject(SocketAddr::V4(self.addr), &data);
            }
        }
        for (to, data) in end.take_sent() {
            if SocketAddr::V4(self.addr) == to {
                let SocketAddr::V4(from) = SERVER else {
                    continue;
                };
                self.incoming.borrow_mut().push_back((from, data));
            }
        }
    }

    /// One tick of the server, then of the client.
    fn step(&mut self) {
        self.carry();
        self.server.tick();
        self.carry();
        self.client.step(32);
        self.carry();
        self.ticks += 1;
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.step();
        }
    }

    fn run_until(&mut self, limit: usize, mut done: impl FnMut(&Rig) -> bool) -> bool {
        for _ in 0..limit {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }

    fn global(&self, name: &str) -> String {
        self.server
            .vm()
            .global(name)
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_default()
    }

    fn eval(&mut self, code: &str) -> String {
        self.server
            .vm_mut()
            .eval(code.as_bytes())
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_else(|e| panic!("{code}: {e}"))
    }

    /// Runs `code` on the client: the commands it sends all leave in the client's next tick.
    fn client_eval(&mut self, code: &str) {
        self.client.call("eval", &[code]).expect("client eval");
    }

    fn drops(&mut self, command: &str) -> usize {
        self.eval(&format!("return commandDrops($Client, \"{command}\");"))
            .parse()
            .expect("a count")
    }

    fn rpc_drops(&mut self, rpc: &str) -> usize {
        self.eval(&format!("return rpcDrops($Client, \"{rpc}\");"))
            .parse()
            .expect("a count")
    }

    fn still_connected(&self) -> bool {
        self.eval_global_client_exists()
    }

    fn eval_global_client_exists(&self) -> bool {
        self.server.vm().global("$Client").is_some()
            && self
                .server
                .vm()
                .objects()
                .contains(self.global("$Client").parse().unwrap_or(0))
    }

    fn client_log(&self) -> String {
        self.client
            .vm()
            .global("$ClientLog")
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_default()
    }
}

/// What the scripts of both sides saw, and what the server counted.
#[derive(Debug, PartialEq)]
struct Logs {
    server: String,
    client: String,
    flood_ran: String,
    flood_drops: usize,
    gated_ran: String,
    gated_drops: usize,
}

/// The shared suite: round trips of numbers and text, tags both ways, an argument over its cap, the most arguments, empty
/// arguments, order, a flood of a command with a limit, and a command whose permission nobody holds.
fn suite(mode: Mode) -> Logs {
    let mut rig = Rig::connected(mode);
    rig.client_eval(
        "commandToServer('Echo', \"hello\", 42, -7, \"a b\");\
         commandToServer('Echo', \"\", 0, 2147483647, \"x\");\
         commandToServer('Tagged', 'Hi %1', \"ann\");\
         commandToServer('Empty', \"one\", \"\", \"three\");",
    );
    rig.run(20);
    let long: String = (0..300)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect();
    rig.client_eval(&format!("commandToServer('Long', \"{long}\");"));
    // Nineteen arguments are what the test function takes; a twentieth after the command's tag is the most a call has, a
    // twenty-first is not sent.
    rig.client_eval(
        "commandToServer('Many', 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19);\
         commandToServer('Many', 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21);",
    );
    rig.client_eval("for (%i = 0; %i < 50; %i++) commandToServer('Order', %i);");
    rig.run(30);
    rig.client_eval("for (%i = 0; %i < 100; %i++) commandToServer('Flood', %i);");
    rig.client_eval("for (%i = 0; %i < 5; %i++) commandToServer('Gated');");
    rig.run(10);
    assert!(
        rig.still_connected(),
        "{mode:?}: the client stays: {:?}",
        rig.server.drain_log()
    );
    Logs {
        server: rig.global("$Log"),
        client: rig.client_log(),
        flood_ran: format!("{}:{}", rig.global("$Flood"), rig.global("$FloodSeq")),
        flood_drops: rig.drops("Flood"),
        gated_ran: rig.global("$Gated"),
        gated_drops: rig.drops("Gated"),
    }
}

#[test]
fn both_protocols_give_the_scripts_the_same_commands() {
    let tge = suite(Mode::Tge);
    let engine = suite(Mode::Engine);
    assert_eq!(
        tge, engine,
        "the callback logs differ between the protocols"
    );
    // And they are what the scripts should see, not merely equal.
    let server = &tge.server;
    assert!(server.contains("Echo(C|hello|42|-7|a b)\n"), "{server}");
    assert!(server.contains("Echo(C||0|2147483647|x)\n"), "{server}");
    assert!(server.contains("Tagged(C|Hi ann|ann)\n"), "{server}");
    assert!(server.contains("Empty(one||three)\n"), "{server}");
    assert!(
        server.contains("Long(255|"),
        "an argument is cut at 255 bytes: {server}"
    );
    assert!(server.contains("Many(1|10|19)\n"), "{server}");
    assert_eq!(
        server.matches("Many(").count(),
        1,
        "twenty-one arguments are not sent: {server}"
    );
    let client = &tge.client;
    assert!(client.starts_with("Welcome(Hello there)\n"), "{client}");
    assert!(client.contains("Reply(hello|a b)\n"), "{client}");
    assert!(client.contains("Notice(Seen Hi ann of ann)\n"), "{client}");
    assert_eq!(tge.gated_ran, "");
    assert_eq!(tge.gated_drops, 5);
    // The flood: the burst of eight ran, in order, and the other 92 were counted.
    assert_eq!(tge.flood_ran, "8:0,1,2,3,4,5,6,7,");
    assert_eq!(tge.flood_drops, 92);
}

#[test]
fn nothing_is_sent_before_the_connection_is_accepted_in_either_protocol() {
    for mode in [Mode::Tge, Mode::Engine] {
        let mut rig = Rig::new(mode);
        // No connection at all.
        rig.client_eval("commandToServer('Early');");
        rig.client
            .call("connectTo", &[&SERVER.to_string()])
            .expect("connect");
        // The connection exists and is not accepted: every tick until it is, the script sends one.
        let mut sent = 0;
        while rig.global("$Connected") != "1" && sent < 400 {
            rig.client_eval("if (!$Accepted) commandToServer('Early');");
            rig.step();
            sent += 1;
        }
        assert_eq!(rig.global("$Connected"), "1", "{mode:?}");
        rig.run(20);
        assert_eq!(
            rig.global("$Early"),
            "",
            "{mode:?}: no command before the accept"
        );
    }
}

#[test]
fn a_command_no_plugin_declared_is_held_to_the_default_limits_in_the_engines_protocol_only() {
    let mut tge = Rig::connected(Mode::Tge);
    tge.client_eval("for (%i = 0; %i < 1000; %i++) commandToServer('Unlisted', %i);");
    tge.run(20);
    let ran: usize = tge.global("$Unlisted").parse().expect("a count");
    assert!(
        ran > 500,
        "{ran} ran: Torque's protocol has no limit of this kind"
    );
    assert_eq!(tge.drops("*"), 0);

    let mut engine = Rig::connected(Mode::Engine);
    let start = engine.ticks;
    engine.client_eval("for (%i = 0; %i < 1000; %i++) commandToServer('Unlisted', %i);");
    engine.run(40);
    let ran: usize = engine.global("$Unlisted").parse().expect("a count");
    // The burst of 64, and what 32 a second refills in the ticks the calls took to arrive (1,000 calls cross in several packets).
    let most = 64 + ((engine.ticks - start) * 32 * 32).div_ceil(1000);
    assert!((64..=most).contains(&ran), "{ran} ran, at most {most}");
    assert!(ran < 100, "{ran} ran of 1,000 in a tick");
    // The refusals are counted for script, for every name together and for the name.
    assert_eq!(engine.drops("*"), 1000 - ran);
    assert_eq!(engine.drops("Unlisted"), 1000 - ran);
    assert!(engine.still_connected(), "a flood does not drop the client");
    assert!(
        engine
            .server
            .drain_log()
            .iter()
            .any(|l| l.contains("serverCmdUnlisted") && l.contains("refused"))
    );
    // Changing the name buys no more calls: the default bucket is the connection's.
    engine.run(40);
    let before = engine.global("$Unlisted").parse::<usize>().unwrap();
    engine.client_eval("for (%i = 0; %i < 500; %i++) commandToServer('Echo', %i, 1, 2, 3);");
    engine.run(5);
    let echoes = engine.global("$Log").matches("Echo(").count();
    assert!(echoes <= 64, "{echoes} echoes in a burst");
    assert_eq!(engine.global("$Unlisted").parse::<usize>().unwrap(), before);
}

#[test]
fn a_command_body_the_server_cannot_read_ends_the_connection_in_the_engines_protocol() {
    let mut rig = Rig::connected(Mode::Engine);
    // A call of the command RPC with no body: a client of this engine never sends one.
    rig.client_eval("rpcToServer(\"torque.command\");");
    rig.run(10);
    assert!(!rig.still_connected(), "the client is dropped");
    assert!(
        rig.server
            .drain_log()
            .iter()
            .any(|l| l.contains("packet error"))
    );
}

#[test]
fn an_honest_flood_is_judged_by_the_hosts_limits_and_the_rpcs_own_bucket_drops_nothing() {
    let mut rig = Rig::connected(Mode::Engine);
    // The RPC's bucket is a net far above any command's limit: a flood that fits it reaches the host, which counts its refusals.
    rig.client_eval("for (%i = 0; %i < 1000; %i++) commandToServer('Unlisted', %i);");
    rig.run(20);
    assert_eq!(rig.rpc_drops("torque.command"), 0);
    assert!(rig.drops("*") > 0);
}

#[test]
fn a_command_burst_over_a_link_that_takes_big_datagrams_never_stalls() {
    // The memory link takes 64 KiB datagrams; 1,500 commands are some 30 KiB of RPC bytes, which the engine's client must send in
    // datagrams the server reads (the accept limits them to 1,500 bytes).
    let mut rig = Rig::connected(Mode::Engine);
    rig.client_eval("for (%i = 0; %i < 1500; %i++) commandToServer('Unlisted', %i);");
    let mut seen = 0;
    for _ in 0..600 {
        rig.step();
        let ran: usize = rig.global("$Unlisted").parse().unwrap_or(0);
        seen = ran + rig.drops("*") + rig.rpc_drops("torque.command");
        if seen == 1500 {
            break;
        }
    }
    assert_eq!(
        seen, 1500,
        "every command ran or was counted (by the host or by the RPC's own bucket)"
    );
    assert!(rig.still_connected());
    assert_eq!(rig.server.shared.borrow().counters.oversize, 0);
}

#[test]
fn a_datagram_over_the_limit_is_dropped_as_it_is_read_and_counted() {
    let mut rig = Rig::connected(Mode::Engine);
    let end = rig.server.memory_end().expect("the memory end");
    end.inject(SocketAddr::V4(rig.addr), &vec![0xAB; 3_000]);
    rig.run(3);
    assert_eq!(rig.server.shared.borrow().counters.oversize, 1);
    assert!(rig.still_connected(), "the connection is untouched");
    // Torque's protocol leaves it to the handshake, which counts it as ignored.
    let mut tge = Rig::connected(Mode::Tge);
    let end = tge.server.memory_end().expect("the memory end");
    end.inject(SocketAddr::V4(tge.addr), &vec![0xAB; 3_000]);
    tge.run(3);
    assert_eq!(tge.server.shared.borrow().counters.oversize, 0);
}
