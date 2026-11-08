//! A plain server over the engine's defaults: a `Vm` of the Torque3D dialect with its files in memory and TGE's class specs, a
//! mission that is a script (`ExecMission`), no hooks, TGE's network class tables, and the main loop's clock, budgets and end;
//! with a network host of Torque's net game, a loopback client's join and a TGE object's ghost.

use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::Arc;

use torque_game::scene::container::{Container, ContainerConfig};
use torque_ghosts::tge::TgeClass;
use torque_net::bits::BitWriter;
use torque_net::classes::Kind;
use torque_net::client::{Client, ClientEvent, ClientState};
use torque_net::connect::{AuthProvider, BlockSource, ConnectBlock, ManifestGate, SharedSecret};
use torque_net::ghost::net_flags;
use torque_net::handshake::ConnectFields;
use torque_net::huffman::Table;
use torque_net::manifest::Manifest;
use torque_scene::BinGeometry;
use torque_server::net::codecs::net_codecs;
use torque_server::net::datablock::{DatablockView, DbValue};
use torque_server::net::ghosts::SourceEnv;
use torque_server::net::hostevent::HostEvent;
use torque_server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque_server::net::transport::{MemoryEnd, MemoryTransport, SharedTransport};
use torque_server::net::{Host, HostParts, NetClock, NetShared, attach_host};
use torque_server::server::{FIRST_PASS_MS, RunEnd, Server, TICK_MS};
use torque_server::state::{START_GHOST_BITS, ServerState};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

/// A mission that builds its group and starts a timer: each firing prints the simulation time and schedules the next one, three
/// in all.
const MISSION: &[u8] = b"new SimGroup(MissionGroup) {\n\
    new ScriptObject(FirstSpawn) { kind = \"spawn\"; };\n\
};\n\
$fired = 0;\n\
function tick() {\n\
    $fired++;\n\
    echo(\"tick \" @ $fired @ \" at \" @ getSimTime());\n\
    if ($fired < 3)\n\
        schedule(32, 0, tick);\n\
}\n\
schedule(0, 0, tick);\n";

fn server(files: &MemFiles) -> Server {
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
    vm.set_file_host(files.clone());
    Server::new(vm)
}

fn lines<N: torque_server::state::NetParts>(server: &Server<N>) -> Vec<String> {
    server
        .vm
        .log()
        .iter()
        .map(|l| String::from_utf8_lossy(&l.text).into_owned())
        .collect()
}

#[test]
fn a_script_mission_loads_and_its_timer_fires_on_the_ticks() {
    let files = MemFiles::new();
    files.insert("missions/tiny.mis", MISSION);
    let mut s = server(&files);
    assert_eq!(s.state.borrow().ghost_bits, START_GHOST_BITS);
    assert!(s.state.borrow().allow_connections);
    let codecs = net_codecs(&s.vm);
    let classes = codecs.object_classes();
    for (kind, name) in [
        (Kind::Object, "Player"),
        (Kind::Object, "Sky"),
        (Kind::DataBlock, "PlayerData"),
    ] {
        assert!(classes.id_of(kind, name).is_some(), "{name}");
    }
    assert!(codecs.datablock_classes().contains(&"ShapeBaseImageData"));
    s.load_mission(b"missions/tiny.mis")
        .expect("the mission loads");
    let group =
        s.vm.objects()
            .find(b"MissionGroup")
            .expect("the mission's group");
    let marker =
        s.vm.objects()
            .find(b"FirstSpawn")
            .unwrap_or_else(|| panic!("its member: {:?}", lines(&s)));
    assert_eq!(
        s.vm.objects().get(marker).and_then(|o| o.group()),
        Some(group)
    );
    assert_eq!(s.vm.sim_time(), 0, "the clock waits for the main loop");
    assert!(!s.is_idle(), "the timer is pending");

    let limit = FIRST_PASS_MS + 4 * TICK_MS;
    assert_eq!(s.run(limit, |_| false), RunEnd::TimeLimit);
    assert_eq!(s.vm.sim_time(), limit);
    let ticks: Vec<String> = lines(&s)
        .into_iter()
        .filter(|l| l.starts_with("tick"))
        .collect();
    assert_eq!(
        ticks,
        ["tick 1 at 1024", "tick 2 at 1056", "tick 3 at 1088"],
        "the first pass is the start-up's, then a tick every 32 ms"
    );
    assert!(s.is_idle());
    assert!(s.event_errors.is_empty());
    assert!(s.run_steps() > 0);
}

#[test]
fn the_run_ends_on_quit_and_on_the_callers_condition() {
    let files = MemFiles::new();
    files.insert("missions/quit.mis", b"schedule(1100, 0, quit);\n");
    let mut s = server(&files);
    s.load_mission(b"missions/quit.mis").expect("loads");
    assert_eq!(s.run(10_000, |_| false), RunEnd::Quit);
    assert!(s.vm.sim_time() >= 1100 && s.vm.sim_time() < 1100 + TICK_MS);

    let mut s = server(&MemFiles::new());
    let mut passes = 0;
    let end = s.run(10_000, |_| {
        passes += 1;
        passes > 3
    });
    assert_eq!(end, RunEnd::Until);
    assert_eq!(s.vm.sim_time(), FIRST_PASS_MS + 2 * TICK_MS);
}

#[test]
fn a_script_that_schedules_itself_forever_ends_the_run_on_its_budget() {
    let files = MemFiles::new();
    files.insert(
        "missions/storm.mis",
        b"function storm() { for (%i = 0; %i < 100; %i++) $n++; schedule(0, 0, storm); }\nschedule(0, 0, storm);\n",
    );
    let mut s = server(&files);
    s.load_mission(b"missions/storm.mis").expect("loads");
    s.set_run_step_limit(200_000);
    assert_eq!(s.run(60_000, |_| false), RunEnd::StepLimit);
    let spent = s.run_steps();
    assert!(spent >= 200_000, "{spent}");
    // The next run has a whole budget again, and a restarting run goes on through every budget it spends.
    let mut restarts = 0;
    let end = s.run_restarting(FIRST_PASS_MS + 40 * TICK_MS, |_| false, |_, n| restarts = n);
    assert_eq!(end, RunEnd::TimeLimit);
    assert!(restarts > 0);
}

#[test]
fn a_missing_mission_is_refused_and_the_end_runs_on_exit() {
    let files = MemFiles::new();
    files.insert(
        "main.cs",
        b"function onExit() { echo(\"bye\"); }\nnoSuchFunction();\n",
    );
    let mut s = server(&files);
    assert!(s.load_mission(b"missions/none.mis").is_err());
    s.load_mission(b"main.cs")
        .expect("a script runs as a mission");
    let finished = s.finish_report();
    assert!(finished.exit.is_ok());
    assert!(finished.log.iter().any(|l| l.text == b"bye"));
    assert_eq!(
        finished.report.missing.first().map(|m| m.0.as_str()),
        Some("noSuchFunction")
    );
}

/// A datablock as a host's view of it answers (by script field): its `volume` (an `AudioDescription`'s) and nothing else.
struct LoudView;

impl DatablockView for LoudView {
    fn get(&self, field: &str, _index: usize) -> DbValue {
        match field {
            "volume" => DbValue::F32(0.5),
            _ => DbValue::Null,
        }
    }
}

/// A datablock with every field unset.
struct NullView;

impl DatablockView for NullView {
    fn get(&self, _field: &str, _index: usize) -> DbValue {
        DbValue::Null
    }
}

#[test]
fn torques_codecs_ghost_tge_classes_by_their_constructors_flags_and_write_tge_datablock_records() {
    let vm = Vm::new();
    let codecs = net_codecs(&vm);
    let classes = codecs.object_classes();
    for (name, flags) in [
        ("Sun", net_flags::GHOSTABLE | net_flags::SCOPE_ALWAYS),
        ("TSStatic", net_flags::GHOSTABLE | net_flags::SCOPE_ALWAYS),
        (
            "AudioEmitter",
            net_flags::GHOSTABLE | net_flags::SCOPE_ALWAYS,
        ),
        ("Player", net_flags::GHOSTABLE),
        ("StaticShape", net_flags::GHOSTABLE),
        ("Camera", 0),
    ] {
        let info = codecs.ghost_class(name).expect(name);
        assert_eq!(info.flags, flags, "{name}");
        assert_eq!(
            Some(info.class_id),
            classes.id_of(Kind::Object, name),
            "{name}"
        );
        assert_eq!(TgeClass::of_code(info.code).map(TgeClass::name), Some(name));
        assert_eq!(info.sky, name == "Sky");
    }
    assert!(codecs.ghost_class("Sky").is_some_and(|i| i.sky));
    // A class of TGE's table with no record codec, and a class that is not networked, are not ghosted.
    assert_eq!(codecs.ghost_class("TerrainBlock"), None);
    assert_eq!(codecs.ghost_class("ScriptObject"), None);

    assert_eq!(
        codecs.datablock_class("AudioDescription"),
        Some("AudioDescription")
    );
    assert_eq!(codecs.datablock_class("ScriptObject"), None);
    let values = torque_game::dbvalues::DbValues::default();
    let record = codecs
        .datablock_record("AudioDescription", 40, &LoudView, &values)
        .expect("TGE's AudioDescription has a record");
    let bits = |record: &dyn torque_server::net::datablock::DatablockRecord| {
        let mut w = BitWriter::new();
        record
            .encode(&mut w, &Table::torque())
            .expect("the record encodes");
        w.into_bytes()
    };
    let quiet = codecs
        .datablock_record("AudioDescription", 40, &NullView, &values)
        .expect("a record of the defaults");
    assert_ne!(bits(&*record), bits(&*quiet), "the volume is in the record");
    assert!(
        codecs
            .datablock_record("ScriptObject", 41, &LoudView, &values)
            .is_none()
    );
}

type NetServer = Server<HostParts<TorqueNet>>;

/// A plain server with a network host of Torque's net game on a memory transport bound to [`SERVER_ADDR`], taking
/// connections, with Torque's codecs (TGE's ghost classes).
fn net_server() -> (NetServer, Rc<RefCell<NetShared<TorqueNet>>>, MemoryEnd) {
    let files = MemFiles::new();
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
    vm.set_file_host(files);
    let state = Rc::new(RefCell::new(ServerState::<HostParts<TorqueNet>>::default()));
    let mut s = Server::with_state(vm, Rc::clone(&state));
    let (transport, end) = MemoryTransport::new();
    let transport: SharedTransport = Rc::new(RefCell::new(transport));
    transport
        .borrow_mut()
        .bind(SERVER_ADDR.ip(), SERVER_ADDR.port())
        .unwrap();
    let container = Rc::new(Container::new(ContainerConfig {
        bins: BinGeometry::default(),
        indexes: Vec::new(),
        candidate_order: None,
    }));
    let shared = attach_host(
        &mut s.vm,
        Host {
            transport,
            game: TorqueNet::default(),
            engine: Rc::clone(&state),
            step_limit: None,
            clock: NetClock::Sim,
            ghosts: TgeGhostGame,
            container,
            static_writes: None,
        },
    );
    s.attach_net(Rc::new(NetPasses(Rc::clone(&shared))))
        .unwrap();
    state.borrow_mut().allow_connections = true;
    (s, shared, end)
}

const SERVER_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 28_000);

#[test]
fn game_flagged_datagrams_from_no_connection_share_their_ips_work_whatever_the_port() {
    let (mut s, shared, end) = net_server();
    // Datagrams that look like game packets (the first byte's low bit) from one IP over many ports, with no connection. Each
    // reserves its worst case against its source's share: counted by its port, every one would find a fresh share.
    let datagram = vec![0x01u8; 1_400];
    for port in 40_000..40_100u16 {
        end.inject(SocketAddr::from(([127, 0, 0, 2], port)), &datagram);
    }
    s.run(TICK_MS, |_| false);
    let sh = shared.borrow();
    // The IP's share pays for 19 (each charged its cost, the next reserving 1,644 units of the 2,048); the rest are skipped.
    assert_eq!(
        (sh.counters.received, sh.counters.over_budget),
        (19, 81),
        "one share for the IP"
    );
    assert!(sh.stash.is_none(), "none waits for the tick's budget");
}

#[test]
fn a_plain_server_accepts_a_loopback_client_and_ghosts_a_tge_object() {
    let (mut s, shared, end) = net_server();
    s.vm.eval(b"new SimGroup(ClientGroup);\nfunction GameConnection::onConnect(%c) { echo(\"connected \" @ %c); }")
        .unwrap();

    // The handshake: the challenge, the connect request, the accept.
    let table = Table::torque();
    let client_addr = SocketAddr::from(([127, 0, 0, 1], 50_000));
    let mut client = Client::new(7, ConnectFields::standard(b"", Vec::new()));
    end.inject(client_addr, &client.challenge_request());
    let deliver = |s: &mut NetServer, client: &mut Client| {
        let now = s.vm.sim_time();
        let mut heard = false;
        for (to, datagram) in end.take_sent() {
            assert_eq!(to, client_addr);
            match client.on_datagram(&table, &datagram, now) {
                ClientEvent::Send(bytes) => {
                    end.inject(client_addr, &bytes);
                }
                ClientEvent::Game { received, .. } => heard |= received.payload,
                _ => {}
            }
        }
        if heard && let Some(p) = client.protocol_mut() {
            end.inject(client_addr, &p.ack_packet(now));
        }
    };
    for _ in 0..10 {
        s.run(TICK_MS, |_| false);
        deliver(&mut s, &mut client);
        if client.state() == &ClientState::Connected {
            break;
        }
    }
    assert_eq!(
        client.state(),
        &ClientState::Connected,
        "{:?} {:?}",
        shared.borrow().counters,
        lines(&s)
    );
    let obj = shared
        .borrow()
        .conns
        .values()
        .find(|r| r.addr == client_addr)
        .map(|r| r.obj)
        .expect("the connection's object");
    assert!(lines(&s).contains(&format!("connected {obj}")));
    let conn = shared.borrow().conns[&obj].conn.expect("accepted");

    // TGE objects through Torque's ghost-class table: the `Sun` is ScopeAlways, so activating the connection's ghosting posts
    // it at once; a `Camera` is registered but its constructor left it not Ghostable, so it gets no ghost.
    s.vm.eval(
        b"new Sun(TheSun) { azimuth = 30; elevation = 45; };
new Camera(TheCamera);",
    )
    .unwrap();
    let sun = s.vm.objects().find(b"TheSun").expect("the sun");
    let camera = s.vm.objects().find(b"TheCamera").expect("the camera");
    let hub = shared.borrow().ghosts.clone().expect("a hub");
    hub.added(&mut s.vm, sun);
    hub.added(&mut s.vm, camera);
    assert!(hub.knows(sun) && hub.knows(camera));
    let env = SourceEnv {
        vm: &s.vm,
        table: Some(&table),
        ghost_bits: hub.conn_ghost_bits(conn),
    };
    let events = hub.activate(env, conn).expect("ghosting starts");
    assert!(!events.is_empty());
    let layer = shared.borrow().events.clone();
    layer
        .post_events(conn, events.into_iter().map(HostEvent::Ghost).collect())
        .expect("the ghost-always events are queued");
    assert!(hub.ghost_index(conn, sun).is_some(), "the sun has a ghost");
    assert!(
        hub.ghost_index(conn, camera).is_none(),
        "a camera is not ghostable"
    );

    // The send passes carry the events; the client acknowledges every packet it hears, and the queue empties.
    for _ in 0..20 {
        s.run(TICK_MS, |_| false);
        deliver(&mut s, &mut client);
    }
    assert_eq!(
        layer.queued(conn).map(|(ordered, ..)| ordered),
        Some(0),
        "the client acknowledged the packets that carried the sun's ghost"
    );
}

/// Sends a client's connect request and runs the server until the client hears an accept or a reject: the client's final event.
fn handshake(
    s: &mut NetServer,
    end: &MemoryEnd,
    addr: SocketAddr,
    fields: ConnectFields,
) -> ClientEvent {
    handshake_as(s, end, addr, Client::new(9, fields))
}

/// [`handshake`] for a client that is already made.
fn handshake_as(
    s: &mut NetServer,
    end: &MemoryEnd,
    addr: SocketAddr,
    mut client: Client,
) -> ClientEvent {
    let table = Table::torque();
    end.inject(addr, &client.challenge_request());
    let mut last = ClientEvent::Nothing;
    for _ in 0..6 {
        s.run(TICK_MS, |_| false);
        for (_, datagram) in end.take_sent() {
            match client.on_datagram(&table, &datagram, s.vm.sim_time()) {
                ClientEvent::Send(bytes) => {
                    end.inject(addr, &bytes);
                }
                event @ (ClientEvent::Rejected(_) | ClientEvent::Accepted(_)) => last = event,
                _ => {}
            }
        }
        if !matches!(last, ClientEvent::Nothing) {
            break;
        }
    }
    last
}

fn rope_manifest(version: u32) -> Manifest {
    Manifest::new(12, 12)
        .with_plugin("torque.rope", version)
        .with_ghosts(["Rope"])
}

fn with_manifest(manifest: Manifest, ticket: Option<Vec<u8>>) -> ConnectFields {
    ConnectFields::standard(b"", Vec::new()).with_block(ConnectBlock {
        manifest: Some(manifest),
        ticket,
    })
}

/// A client that dials `dialed` and signs its ticket for `name` under `key` for the connection it gets the challenge for.
fn signing_client(key: [u8; 16], name: &'static str, dialed: SocketAddr) -> Client {
    let mut client = Client::new(9, ConnectFields::standard(b"", Vec::new()));
    client.set_block_source(
        dialed,
        BlockSource::new(move |cx| {
            Ok(Some(ConnectBlock {
                manifest: Some(rope_manifest(2)),
                ticket: Some(SharedSecret::new(key, name).ticket(cx)?),
            }))
        }),
    );
    client
}

#[test]
fn a_mismatched_client_is_refused_before_the_scripts_see_it_and_before_any_ghost() {
    let (mut s, shared, end) = net_server();
    s.vm.eval(
        b"new SimGroup(ClientGroup);\nfunction GameConnection::onConnectRequest(%c, %a) { echo(\"asked\"); return \"\"; }\nfunction GameConnection::onConnect(%c) { echo(\"connected\"); }",
    )
    .unwrap();
    let gate =
        ManifestGate::new(rope_manifest(2)).with_auth(Arc::new(SharedSecret::new([4; 16], "")));
    shared.borrow_mut().set_connect_gate(gate);
    shared.borrow_mut().set_public_address(Some(SERVER_ADDR));
    let addr = SocketAddr::from(([127, 0, 0, 1], 50_001));

    // A client with an older rope: refused with the reason, the script never asked, nothing in the table.
    let event = handshake(
        &mut s,
        &end,
        addr,
        with_manifest(rope_manifest(1), Some(b"unchecked".to_vec())),
    );
    let ClientEvent::Rejected(reason) = event else {
        panic!("not refused: {event:?}")
    };
    assert_eq!(reason, b"server needs plugin torque.rope 2, you have 1");
    // A client with no manifest at all.
    let addr2 = SocketAddr::from(([127, 0, 0, 1], 50_002));
    let event = handshake(
        &mut s,
        &end,
        addr2,
        ConnectFields::standard(b"", Vec::new()),
    );
    assert!(
        matches!(&event, ClientEvent::Rejected(r) if r == b"server needs plugin torque.rope 2, you do not have it"),
        "{event:?}"
    );
    // A matching client with a forged ticket: refused by the auth provider, still before the scripts.
    let addr3 = SocketAddr::from(([127, 0, 0, 1], 50_003));
    let event = handshake_as(
        &mut s,
        &end,
        addr3,
        signing_client([5; 16], "ada", SERVER_ADDR),
    );
    assert!(
        matches!(&event, ClientEvent::Rejected(r) if r == b"your sign-in was not accepted"),
        "{event:?}"
    );
    assert!(!lines(&s).iter().any(|l| l == "asked" || l == "connected"));
    assert_eq!(
        shared.borrow().hs.len(),
        0,
        "no connection was made for them"
    );
    assert_eq!(shared.borrow().counters.accepted, 0);

    // A ticket made for another server is refused here, whoever relays it.
    let addr5 = SocketAddr::from(([127, 0, 0, 2], 50_005));
    let elsewhere = SocketAddr::from(([203, 0, 113, 9], 28_000));
    let event = handshake_as(
        &mut s,
        &end,
        addr5,
        signing_client([4; 16], "ada", elsewhere),
    );
    assert!(
        matches!(&event, ClientEvent::Rejected(r) if r == b"your sign-in was not accepted"),
        "{event:?}"
    );
    assert!(!lines(&s).iter().any(|l| l == "asked" || l == "connected"));

    // The matching client with a good ticket is accepted, and the script is asked.
    let addr4 = SocketAddr::from(([127, 0, 0, 1], 50_004));
    let event = handshake_as(
        &mut s,
        &end,
        addr4,
        signing_client([4; 16], "ada", SERVER_ADDR),
    );
    assert!(matches!(event, ClientEvent::Accepted(_)), "{event:?}");
    assert!(lines(&s).iter().any(|l| l == "asked"));
    assert_eq!(shared.borrow().counters.accepted, 1);
    let sh = shared.borrow();
    let conn = sh
        .conns
        .values()
        .find(|r| r.addr == addr4)
        .and_then(|r| r.conn)
        .expect("accepted");
    assert_eq!(
        sh.hs.connection(conn).and_then(|c| c.identity.as_deref()),
        Some(&b"ada"[..]),
        "the verified identity is kept with the connection"
    );
}
