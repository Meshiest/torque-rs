//! A client that speaks only Torque's protocol (no plugin, no handshake extension) against a server whose engine has the rope plugin:
//! it is never sent a rope, its packets read with TGE's own codec and its table of classes, and it sees what any TGE server would
//! send it. The same server's ropes do reach a client that has the plugin's classes. (A server whose engine has a networked plugin
//! refuses a client without it at the manifest, so this server is the host without that gate: how a game that takes both kinds of
//! client would run.)

use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;

use torque_engine::{EngineBuilder, ServerStage};
use torque_ghost_world::ClientWorld;
use torque_ghost_world::tge::{TgeClientSet, TgeCodec};
use torque_ghosts::plugin::PluginClasses;
use torque_ghosts::{NetWidths, TgeClass, tge_reps};
use torque_net::classes::{Kind, NetClasses};
use torque_net::client::{Client, ClientEvent as Handshake, ClientState};
use torque_net::control::{ClientBlock, ServerHead};
use torque_net::events::{
    ConnectionMessage, EventCtx, EventIds, EventQueue, EventReceiver, EventRegistry, NetEvent,
    SectionParams,
};
use torque_net::ghost::{ClientEvent, GhostClient, GhostConfig, GhostEventReader};
use torque_net::handshake::ConnectFields;
use torque_net::huffman::Table;
use torque_net::profile::{DEFAULT_DATABLOCK_BITS, DEFAULT_GHOST_LIMIT};
use torque_net::protocol::{Received, Role, reader_at};
use torque_plugin_rope::RopePlugin;
use torque_plugin_rope::net::{AnchorWire, RopeGhost};
use torque_server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque_server::net::transport::{MemoryEnd, MemoryTransport, SharedTransport};
use torque_server::net::{Host, HostParts, NetClock, NetShared, attach_host};
use torque_server::server::{Server, TICK_MS};
use torque_server::state::ServerState;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

type NetServer = Server<HostParts<TorqueNet>>;

const SERVER_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 28_000);

/// A server with the game classes, the rope plugin's classes and natives, and a network host on a memory transport (no gate).
fn game_server() -> (
    NetServer,
    Rc<RefCell<NetShared<TorqueNet>>>,
    MemoryEnd,
    PluginClasses,
) {
    let engine = EngineBuilder::new()
        .add_plugins(RopePlugin::default())
        .build()
        .expect("the engine builds");
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
    engine
        .replay(ServerStage::Classes, &mut vm)
        .expect("the plugin's classes");
    vm.set_file_host(MemFiles::new());
    let state = Rc::new(RefCell::new(ServerState::<HostParts<TorqueNet>>::default()));
    let game = torque_server::game::install(&mut vm, &state);
    let mut s = Server::with_state(vm, Rc::clone(&state));
    let (transport, end) = MemoryTransport::new();
    let transport: SharedTransport = Rc::new(RefCell::new(transport));
    transport
        .borrow_mut()
        .bind(SERVER_ADDR.ip(), SERVER_ADDR.port())
        .unwrap();
    let shared = attach_host(
        &mut s.vm,
        Host {
            transport,
            game: TorqueNet::default(),
            engine: Rc::clone(&state),
            clock: NetClock::Sim,
            ghosts: TgeGhostGame,
            container: game.container,
            static_writes: None,
        },
    );
    s.attach_net(Rc::new(NetPasses(Rc::clone(&shared))))
        .unwrap();
    engine
        .replay(ServerStage::Net, &mut s.vm)
        .expect("the plugin's script registry");
    state.borrow_mut().allow_connections = true;
    (s, shared, end, engine.net_classes())
}

/// A client of the server: torque-net's handshake and packet layer, a ghost table read with TGE's client codec and the plugin classes
/// it was given (none for a client that is Torque's alone), and the client's world of ghosts.
struct TestClient {
    addr: SocketAddr,
    client: Client,
    table: Table,
    ids: EventIds,
    classes: NetClasses,
    plugins: PluginClasses,
    widths: NetWidths,
    ghosts: Option<GhostClient<TgeCodec>>,
    world: ClientWorld<TgeClientSet>,
    out: EventQueue<NetEvent>,
    receiver: EventReceiver<ClientEvent>,
    packets: usize,
    errors: Vec<String>,
}

impl TestClient {
    fn new(port: u16, seq: u32, plugins: PluginClasses) -> TestClient {
        let events = EventRegistry::torque();
        TestClient {
            addr: SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), port),
            client: Client::new(seq, ConnectFields::standard(b"", Vec::new())),
            table: Table::torque(),
            ids: events.ids().unwrap(),
            classes: NetClasses::derive(&tge_reps(&events)).unwrap(),
            plugins,
            widths: NetWidths::engine_default(),
            ghosts: None,
            world: ClientWorld::default(),
            out: EventQueue::new(),
            receiver: EventReceiver::new(),
            packets: 0,
            errors: Vec::new(),
        }
    }

    fn start(&mut self, end: &MemoryEnd) {
        end.inject(self.addr, &self.client.challenge_request());
    }

    fn datagram(&mut self, end: &MemoryEnd, data: &[u8], now: u32) {
        match self.client.on_datagram(&self.table, data, now) {
            Handshake::Send(reply) => {
                end.inject(self.addr, &reply);
            }
            Handshake::Accepted(accept) => {
                let ghost_bits = accept.ghost_bits.map_or_else(
                    || {
                        GhostConfig {
                            max_ghosts: DEFAULT_GHOST_LIMIT,
                            local: false,
                            class_bits: self.classes.id_bits(Kind::Object),
                        }
                        .ghost_bits()
                    },
                    |b| b as u8,
                );
                self.widths = NetWidths::new(ghost_bits, DEFAULT_DATABLOCK_BITS);
                let names = self
                    .classes
                    .classes(Kind::Object)
                    .iter()
                    .map(|c| c.name.as_str());
                let codec = TgeCodec::new(names, self.widths, self.table.clone())
                    .with_plugins(self.plugins.clone());
                let mut ghosts =
                    GhostClient::new(codec, self.classes.id_bits(Kind::Object), ghost_bits);
                ghosts.enable_log();
                self.ghosts = Some(ghosts);
            }
            Handshake::Game {
                received,
                payload_bit,
            } => {
                if let Err(e) = self.packet(end, &received, data, payload_bit, now) {
                    self.errors.push(e.to_string());
                }
            }
            Handshake::GameError(e) => self.errors.push(e.to_string()),
            Handshake::Rejected(r) | Handshake::Disconnected(r) => {
                self.errors.push(String::from_utf8_lossy(&r).into_owned());
            }
            Handshake::Nothing => {}
        }
    }

    fn packet(
        &mut self,
        end: &MemoryEnd,
        received: &Received,
        data: &[u8],
        payload_bit: usize,
        now: u32,
    ) -> Result<(), torque_net::NetError> {
        let mut settled = Vec::new();
        for notify in &received.notifies {
            self.out
                .on_notify(notify.send_seq, notify.delivered, &mut settled);
        }
        if received.answer_ping
            && let Some(protocol) = self.client.protocol_mut()
        {
            end.inject(self.addr, &protocol.ack_packet(now));
        }
        if !received.payload || self.ghosts.is_none() {
            return Ok(());
        }
        let mut r = reader_at(data, payload_bit);
        r.enable_string_buffer();
        let mut charge = torque_net::events::free_charge();
        ServerHead::read(&mut r, self.widths.ghost_bits, &mut charge)?;
        let mut incoming = Vec::new();
        let ctx = EventCtx {
            table: &self.table,
            ids: &self.ids,
            ghost_bits: self.widths.ghost_bits,
            datablock_bits: self.widths.datablock_bits,
        };
        let ghosts = self.ghosts.as_mut().expect("accepted");
        self.receiver.read_section(
            &mut r,
            &ctx,
            Role::Client,
            &mut GhostEventReader { client: ghosts },
            &mut incoming,
            None,
        )?;
        ghosts.read_section(&mut r)?;
        self.packets += 1;
        for event in incoming {
            let ClientEvent::Message(message) = event.event else {
                continue;
            };
            match message.message {
                // GhostAlwaysDone: ask for the rest.
                0 => {
                    let _ = self
                        .out
                        .post(NetEvent::ConnectionMessage(ConnectionMessage {
                            seq: message.seq,
                            message: 1,
                            count: 0,
                        }));
                }
                // EndGhosting.
                2 => ghosts.clear(),
                _ => {}
            }
        }
        let changes = ghosts.take_log();
        self.world
            .apply_ghosts(self.packets, &changes, &*ghosts, [0.0; 3], &mut ());
        Ok(())
    }

    fn send(&mut self, end: &MemoryEnd, now: u32) {
        let Some(mut w) = self.client.begin_data_packet(now, false) else {
            return;
        };
        let Some(protocol) = self.client.protocol() else {
            return;
        };
        let send_seq = protocol.last_send_seq();
        let packet_bits = protocol.packet_limit_bits();
        w.enable_string_buffer();
        ClientBlock::default().write(&mut w);
        let params = SectionParams {
            ghosting: false,
            hold_ordered: false,
            packet_bits,
            min_start_bits: w.bits(),
        };
        let ctx = EventCtx {
            table: &self.table,
            ids: &self.ids,
            ghost_bits: self.widths.ghost_bits,
            datablock_bits: self.widths.datablock_bits,
        };
        let _ = self
            .out
            .write_section_stalling(&mut w, &ctx, send_seq, params);
        end.inject(self.addr, &w.into_bytes());
    }

    fn connected(&self) -> bool {
        *self.client.state() == ClientState::Connected
    }

    /// The classes of the ghosts the client holds, by name, and its ropes' records.
    fn census(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self
            .world
            .ghosts
            .values()
            .filter_map(|g| g.class.map(|c| c.name()))
            .collect();
        names.sort_unstable();
        names
    }

    fn ropes(&self) -> Vec<RopeGhost> {
        self.world
            .ghosts
            .values()
            .filter_map(|g| match (&g.class, &g.record) {
                (Some(TgeClass::Plugin(_)), Some(record)) => {
                    Some(record.plugin::<RopeGhost>()?.clone())
                }
                _ => None,
            })
            .collect()
    }
}

fn run(s: &mut NetServer, end: &MemoryEnd, clients: &mut [&mut TestClient], ticks: usize) {
    for _ in 0..ticks {
        s.run(TICK_MS, |_| false);
        let now = s.vm.sim_time();
        for (to, datagram) in end.take_sent() {
            if let Some(c) = clients.iter_mut().find(|c| c.addr == to) {
                c.datagram(end, &datagram, now);
            }
        }
        for c in clients.iter_mut() {
            if c.connected() {
                c.send(end, now);
            }
        }
    }
}

fn connection_of(shared: &Rc<RefCell<NetShared<TorqueNet>>>, addr: SocketAddr) -> ObjId {
    shared
        .borrow()
        .conns
        .values()
        .find(|r| r.addr == addr)
        .map(|r| r.obj)
        .expect("the connection's object")
}

fn eval(s: &mut NetServer, code: &str) -> String {
    String::from_utf8_lossy(&s.vm.eval(code.as_bytes()).unwrap()).into_owned()
}

#[test]
fn a_client_that_is_torques_alone_is_never_sent_a_rope_and_one_with_the_plugin_is() {
    let (mut s, shared, end, plugins) = game_server();
    assert_eq!(plugins.names(Kind::Object).collect::<Vec<_>>(), ["Rope"]);
    eval(
        &mut s,
        "new SimGroup(ClientGroup);
datablock CameraData(EyeData) {};
datablock RopeData(Cable) { segments = 8; };
new Camera(EyeA) { dataBlock = EyeData; };
new Camera(EyeB) { dataBlock = EyeData; };",
    );
    let rope = eval(&mut s, "return ropeNetCreate(\"0 0 3\", \"6 0 3\", Cable);");
    assert!(rope.parse::<u32>().unwrap() > 0, "{rope}");
    let mut tge = TestClient::new(50_001, 7, PluginClasses::default());
    tge.start(&end);
    run(&mut s, &end, &mut [&mut tge], 10);
    assert!(tge.connected());
    let conn_tge = connection_of(&shared, tge.addr);
    eval(
        &mut s,
        &format!("{conn_tge}.setControlObject(EyeA); {conn_tge}.activateGhosting();"),
    );
    run(&mut s, &end, &mut [&mut tge], 30);

    // Torque's client has what a Torque server sends it: its cameras, no rope, no error.
    assert!(tge.errors.is_empty(), "{:?}", tge.errors);
    assert_eq!(tge.census(), ["Camera", "Camera"]);
    assert!(tge.ropes().is_empty());
    assert!(tge.packets > 5, "{} packets", tge.packets);

    // A server that says its clients are the engine's own sends them the rope: this client has the plugin's classes.
    eval(&mut s, "$Server::EngineClasses = 1;");
    let mut own = TestClient::new(50_002, 9, plugins);
    own.start(&end);
    run(&mut s, &end, &mut [&mut tge, &mut own], 10);
    assert!(own.connected());
    let conn_own = connection_of(&shared, own.addr);
    eval(
        &mut s,
        &format!("{conn_own}.setControlObject(EyeB); {conn_own}.activateGhosting();"),
    );
    run(&mut s, &end, &mut [&mut tge, &mut own], 30);
    // The engine's own client has the rope.
    assert!(own.errors.is_empty(), "{:?}", own.errors);
    assert_eq!(own.census().iter().filter(|c| **c == "Rope").count(), 1);
    let ropes = own.ropes();
    assert_eq!(ropes[0].start, Some(AnchorWire::Point([0.0, 0.0, 3.0])));
    assert_eq!(ropes[0].end, Some(AnchorWire::Point([6.0, 0.0, 3.0])));
}
