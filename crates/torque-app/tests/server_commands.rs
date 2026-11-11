//! Server commands a plugin declares, on a real server and an engine client over the memory transport, stepped by hand: a
//! flood of calls is refused past the bucket, oversize and malformed arguments are refused before the handler, the refusals
//! are counted for script, and commands no plugin declared run as TGE's do. No clock is read.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use torque_app::GameFiles;
use torque_app::client::{ClientBuilder, ClientNet, GameClient};
use torque_app::config::{EngineConfig, GameConfig};
use torque_app::plugins::{DefaultPlugins, set_protocol};
use torque_app::server::{GameServer, Listen, ServerStart};
use torque_client::net::{DatagramQueue, MemoryDatagrams, NetPolicy};
use torque_engine::command::{ArgType, CommandSpec};
use torque_engine::{Engine, EngineBuilder, Plugin, PluginId, ProtocolKind, ResMut};

const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);
const BURST: usize = 5;

/// What the plugin's handlers were called with: `(command, client, argument)`.
#[derive(Default)]
struct Calls(Vec<(&'static str, i32, String)>);

fn ping(client: i32, n: i32, mut calls: ResMut<Calls>) {
    calls.0.push(("Ping", client, n.to_string()));
}

fn say(client: i32, text: String, mut calls: ResMut<Calls>) {
    calls.0.push(("Say", client, text));
}

#[derive(PluginId)]
struct Commands;

impl Plugin for Commands {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.init_resource::<Calls>();
        // Ten a second after a burst of five.
        engine.server_command(
            "Ping",
            CommandSpec::new()
                .arg("n", ArgType::Int)
                .rate(10, BURST as u32),
            ping,
        );
        // Plenty of calls, small text.
        engine.server_command(
            "Say",
            CommandSpec::new()
                .arg("text", ArgType::Text)
                .max_arg_bytes(8)
                .rate(1000, 1000),
            say,
        );
        // The function is the script's.
        engine.server_command_limits(
            "Scripted",
            CommandSpec::new().arg("n", ArgType::Int).rate(10, 4),
        );
    }
}

struct Rig {
    server: GameServer,
    engine: Engine,
    client: GameClient,
    addr: SocketAddrV4,
    sent: DatagramQueue,
    incoming: DatagramQueue,
    ticks: usize,
}

impl Rig {
    fn new() -> Rig {
        let mut files = GameFiles::default();
        files.insert("server.cs", include_bytes!("server_commands/server.cs"));
        files.insert("main.cs", include_bytes!("server_commands/main.cs"));
        files.insert("game.toml", include_bytes!("server_commands/game.toml"));
        let config = GameConfig::load(&files).expect("game.toml");
        let mut builder = EngineBuilder::new();
        builder
            .add_plugins(DefaultPlugins)
            .config(&config)
            .add_plugins(Commands);
        // `commandToServer` is Torque's protocol; the engine's protocol sends calls as RPCs.
        set_protocol(&mut builder, ProtocolKind::Tge);
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
        // The client has the default plugins only: the commands are the server's.
        let mut client_builder = EngineBuilder::new();
        client_builder.add_plugins(DefaultPlugins).config(&config);
        set_protocol(&mut client_builder, ProtocolKind::Tge);
        let client_engine = client_builder.build().expect("the client's engine builds");
        let port = 50_001;
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
        let mut client: GameClient = builder.build();
        client
            .call("connectTo", &[&SERVER.to_string()])
            .expect("connect");
        let mut rig = Rig {
            server,
            engine,
            client,
            addr,
            sent,
            incoming,
            ticks: 0,
        };
        assert!(rig.run_until(400, |r| r.global("$Connected") == "1"));
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

    fn calls(&self, command: &str) -> Vec<(i32, String)> {
        self.engine
            .resource::<Calls>()
            .expect("calls")
            .0
            .iter()
            .filter(|(name, ..)| *name == command)
            .map(|(_, client, arg)| (*client, arg.clone()))
            .collect()
    }

    fn drops(&mut self, command: &str) -> usize {
        self.eval(&format!("return commandDrops($Client, \"{command}\");"))
            .parse()
            .expect("a count")
    }

    fn connected(&self) -> bool {
        self.global("$Connected") == "1"
    }
}

#[test]
fn a_flood_of_calls_runs_the_burst_and_the_refill_and_no_more_and_the_client_stays() {
    let mut rig = Rig::new();
    rig.client_eval("for (%i = 0; %i < 300; %i++) commandToServer('Ping', %i);");
    let before = rig.ticks;
    rig.run(200);
    let elapsed = rig.ticks - before;
    let ran = rig.calls("Ping").len();
    // The burst, and what 10 a second refills in the ticks the flood took to arrive (a tick is 32 ms).
    let most = BURST + (elapsed * 10 * 32).div_ceil(1000);
    assert!(
        (BURST..=most).contains(&ran),
        "{ran} calls, between {BURST} and {most}"
    );
    // Everything that did not run is counted for the connection, and script reads the count.
    assert_eq!(ran + rig.drops("Ping"), 300);
    assert_eq!(rig.drops("serverCmdPing"), 300 - ran, "with the prefix too");
    assert_eq!(rig.drops("Say"), 0, "another command's count");
    // The first calls are the ones that ran, with the client's id and the argument as sent.
    let first = rig.calls("Ping");
    let first_five: Vec<String> = first.iter().take(BURST).map(|(_, n)| n.clone()).collect();
    assert_eq!(first_five, ["0", "1", "2", "3", "4"]);
    let order: Vec<usize> = first.iter().map(|(_, n)| n.parse().unwrap()).collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "in the order sent");
    assert!(
        first
            .iter()
            .all(|(client, _)| client.to_string() == rig.global("$Client"))
    );
    assert!(rig.connected(), "the flood did not drop the client");
    assert!(
        rig.server
            .drain_log()
            .iter()
            .any(|l| l.contains("serverCmdPing") && l.contains("refused"))
    );
    // Given time to refill, the next flood gets a burst again.
    rig.run(32 * 3);
    let ran_before = rig.calls("Ping").len();
    rig.client_eval("for (%i = 0; %i < 100; %i++) commandToServer('Ping', 1000 + %i);");
    let start = rig.ticks;
    rig.run(60);
    let more = rig.calls("Ping").len() - ran_before;
    let most = BURST + ((rig.ticks - start) * 10 * 32).div_ceil(1000);
    assert!(
        (BURST..=most).contains(&more),
        "{more} calls, at most {most}"
    );
}

#[test]
fn oversize_extra_missing_and_malformed_arguments_are_refused_without_calling_the_handler() {
    let mut rig = Rig::new();
    rig.client_eval(
        "commandToServer('Say', \"hi\");\
         commandToServer('Say', \"a text over eight bytes\");\
         commandToServer('Say', \"one\", \"two\");\
         commandToServer('Say');\
         commandToServer('Ping', \"abc\");\
         commandToServer('Ping');\
         commandToServer('Ping', 1, 2);\
         commandToServer('Ping', 7);",
    );
    rig.run(40);
    assert_eq!(
        rig.calls("Say")
            .iter()
            .map(|(_, a)| a.as_str())
            .collect::<Vec<_>>(),
        ["hi"]
    );
    assert_eq!(
        rig.calls("Ping")
            .iter()
            .map(|(_, a)| a.as_str())
            .collect::<Vec<_>>(),
        ["7"]
    );
    assert_eq!((rig.drops("Say"), rig.drops("Ping")), (3, 3));
    assert!(rig.connected());
}

#[test]
fn a_command_whose_function_is_the_scripts_is_limited_the_same_way() {
    let mut rig = Rig::new();
    rig.client_eval("for (%i = 0; %i < 100; %i++) commandToServer('Scripted', %i);");
    let start = rig.ticks;
    rig.run(100);
    let ran: usize = rig.global("$Scripted").parse().expect("a count");
    let most = 4 + ((rig.ticks - start) * 10 * 32).div_ceil(1000);
    assert!((4..=most).contains(&ran), "{ran} calls, at most {most}");
    assert_eq!(ran + rig.drops("Scripted"), 100);
}

#[test]
fn commands_no_plugin_declared_and_the_scripts_own_calls_are_not_limited() {
    let mut rig = Rig::new();
    rig.client_eval("for (%i = 0; %i < 100; %i++) commandToServer('Legacy', %i);");
    rig.run(100);
    assert_eq!(rig.global("$Legacy"), "100");
    assert_eq!(rig.drops("Legacy"), 0);
    // A call from a script is the server's own: no bucket, no argument check.
    rig.eval("for (%i = 0; %i < 50; %i++) serverCmdPing(1, %i);");
    assert_eq!(rig.calls("Ping").len(), 50);
}

#[test]
fn unknown_commands_cost_the_connections_share_and_their_warnings_are_limited() {
    let mut rig = Rig::new();
    let _ = rig.server.drain_log();
    // A command that spends the connection's share of the window first: the unknown ones after it in that window are dropped
    // and counted, and the later ones are charged to the next windows' shares.
    rig.client_eval(
        "commandToServer('Spend'); for (%i = 0; %i < 4000; %i++) commandToServer('NoSuchCommand', %i);",
    );
    let start = rig.ticks;
    let mut most_charged = 0;
    for _ in 0..250 {
        rig.run(1);
        let sh = rig.server.shared().expect("the network host").borrow();
        let charged = sh.conns.values().map(|r| r.share_used).max().unwrap_or(0);
        if sh.counters.over_share > 0 {
            most_charged = most_charged.max(charged);
        }
    }
    let seconds = ((rig.ticks - start) * 32).div_ceil(1000);
    let (unknown, over_share) = {
        let sh = rig.server.shared().expect("the network host").borrow();
        (sh.counters.unknown_commands, sh.counters.over_share)
    };
    assert!(
        unknown > 0 && over_share > 0,
        "{unknown} unknown, {over_share} over the share"
    );
    // Every command is counted but those after `Spend` in its packet: a run that stops at its limit ends its packet's work.
    let counted = unknown + over_share;
    assert!((3_900..=4_000).contains(&counted), "{counted} counted");
    assert!(
        most_charged >= 300 * torque_server::net::NET_UNKNOWN_COMMAND_STEPS,
        "the unknown commands were charged: {most_charged} steps"
    );
    let warnings = rig
        .server
        .drain_log()
        .into_iter()
        .filter(|l| l.contains("Unknown command"))
        .count();
    // The script engine's per-source limit: at most its lines a second, and a count line for the rest.
    let most = torque_ts_vm::vm::MAX_UNKNOWN_COMMAND_LINES as usize * (seconds + 1);
    assert!(
        (1..=most).contains(&warnings),
        "{warnings} warnings in {seconds} s"
    );
    assert!(rig.connected(), "the client stays");
}

impl Rig {
    /// [`Rig::step`] with what the client sends mutated by `mutate` on its way (it may drop a datagram by emptying it).
    fn step_through(&mut self, mutate: &mut dyn FnMut(&mut Vec<u8>)) {
        if let Some(end) = self.server.memory_end() {
            for (to, mut data) in self.sent.borrow_mut().drain(..) {
                mutate(&mut data);
                if SocketAddr::V4(to) == SERVER && !data.is_empty() {
                    end.inject(SocketAddr::V4(self.addr), &data);
                }
            }
        }
        self.server.tick();
        self.carry();
        self.client.step(32);
        self.ticks += 1;
    }
}

/// The whole receive path of an established connection (the pump, the packet layer, the events with their commands and
/// net strings, the moves and the followups) under hostile packets, by structure: an engine client sends commands of random
/// names (declared, the scripts', unknown), random argument counts and random contents (numbers, tags, long and binary
/// text), so the events are well formed and hostile; then most datagrams keep their header (the first 8 bytes) and have
/// their payload's bits flipped, cut short or replaced, and a few are mangled whole. The server never panics, and a
/// connection it drops for a bad packet is joined again. `TORQUE_FUZZ_SERVER` sets the ticks (default 600).
#[test]
fn hostile_packets_of_an_established_connection_never_panic_the_server() {
    use torque_net::testrng::TestRng;
    let ticks: usize = std::env::var("TORQUE_FUZZ_SERVER")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600);
    let mut rng = TestRng::new(0x5eed_0f5e_7e47);
    let names = [
        "Legacy",
        "Say",
        "Ping",
        "Scripted",
        "NoSuchCommand",
        "x",
        "Ping2",
    ];
    let mut rig = Rig::new();
    let mut done = 0;
    while done < ticks {
        let mut code = String::new();
        for _ in 0..1 + rng.index(6) {
            let name = names[rng.index(names.len())];
            code.push_str(&format!("commandToServer('{name}'"));
            for _ in 0..rng.index(19) {
                let arg = match rng.below(5) {
                    0 => rng.below(1 << 20).to_string(),
                    1 => format!("'tag{}'", rng.below(40)),
                    2 => format!("\"{}\"", "w".repeat(rng.index(256))),
                    3 => "\"\x01\x02\t\n\"".to_string(),
                    _ => "\"\"".to_string(),
                };
                code.push_str(", ");
                code.push_str(&arg);
            }
            code.push_str(");");
        }
        rig.client_eval(&code);
        for _ in 0..8 {
            let mut mutate = |data: &mut Vec<u8>| {
                let head = 8.min(data.len());
                match rng.below(8) {
                    0..=3 => {}
                    4 => {
                        for _ in 0..1 + rng.below(3) {
                            let at = head + rng.index(data.len() - head);
                            if let Some(b) = data.get_mut(at) {
                                *b ^= 1 << rng.below(8);
                            }
                        }
                    }
                    5 => data.truncate(head + rng.index(data.len() - head + 1)),
                    6 => {
                        for b in data.iter_mut().skip(head) {
                            if rng.below(8) == 0 {
                                *b = rng.byte();
                            }
                        }
                        let extra = rng.index(64);
                        data.extend((0..extra).map(|_| rng.byte()));
                    }
                    _ => {
                        // Mangled whole, the header too.
                        for b in data.iter_mut() {
                            if rng.below(4) == 0 {
                                *b = rng.byte();
                            }
                        }
                    }
                }
            };
            rig.step_through(&mut mutate);
            done += 1;
        }
        if rig
            .server
            .shared()
            .expect("the network host")
            .borrow()
            .hs
            .is_empty()
        {
            rig = Rig::new();
        }
    }
    // The server still runs its ticks.
    rig.server.tick();
}
