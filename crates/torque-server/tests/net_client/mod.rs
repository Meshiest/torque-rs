//! A client of a server on the memory transport: torque-net's handshake and packet layer, a ghost table read with TGE's client
//! codec, and the client's world of ghosts. Shared by the tests that read what the server ghosts.

#![allow(dead_code)]

use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;

use torque_ghost_world::ClientWorld;
use torque_ghost_world::tge::{TgeClientSet, TgeCodec};
use torque_ghosts::NetWidths;
use torque_ghosts::plugin::PluginClasses;
use torque_ghosts::tge_reps;
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
use torque_server::net::torque::TorqueNet;
use torque_server::net::transport::MemoryEnd;
use torque_server::net::{HostParts, NetShared};
use torque_server::server::{Server, TICK_MS};
use torque_ts_vm::sim::ObjId;

pub type NetServer = Server<HostParts<TorqueNet>>;

const GAME_SERVER_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 28_000);

/// A server with the game classes over `files` (its shapes), a memory transport bound at port 28,000 and connections
/// allowed, after its first tick: the shared state of its network host and the transport's other end.
pub fn game_server(
    files: torque_ts_vm::host::MemFiles,
) -> (NetServer, Rc<RefCell<NetShared<TorqueNet>>>, MemoryEnd) {
    use torque_server::net::torque::{NetPasses, TgeGhostGame};
    use torque_server::net::transport::{MemoryTransport, SharedTransport};
    use torque_server::net::{Host, NetClock, attach_host};
    use torque_server::state::ServerState;

    let mut vm = torque_game::testing::vm();
    vm.set_file_host(files);
    let state = Rc::new(RefCell::new(ServerState::<HostParts<TorqueNet>>::default()));
    let game = torque_server::game::install(&mut vm, &state);
    let mut s = Server::with_state(vm, Rc::clone(&state));
    let (transport, end) = MemoryTransport::new();
    let transport: SharedTransport = Rc::new(RefCell::new(transport));
    transport
        .borrow_mut()
        .bind(GAME_SERVER_ADDR.ip(), GAME_SERVER_ADDR.port())
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
    state.borrow_mut().allow_connections = true;
    s.run(TICK_MS, |_| false);
    (s, shared, end)
}

/// A client of the server: torque-net's handshake and packet layer, a ghost table read with TGE's client codec and the
/// plugins' classes, and the client's world of ghosts.
pub struct TestClient {
    pub addr: SocketAddr,
    pub client: Client,
    pub table: Table,
    pub ids: EventIds,
    pub classes: NetClasses,
    pub plugins: PluginClasses,
    pub widths: NetWidths,
    pub ghosts: Option<GhostClient<TgeCodec>>,
    pub world: ClientWorld<TgeClientSet>,
    pub out: EventQueue<NetEvent>,
    pub receiver: EventReceiver<ClientEvent>,
    pub packets: usize,
    pub errors: Vec<String>,
}

impl TestClient {
    pub fn new(port: u16, seq: u32, plugins: PluginClasses) -> TestClient {
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

    pub fn start(&mut self, end: &MemoryEnd) {
        end.inject(self.addr, &self.client.challenge_request());
    }

    pub fn datagram(&mut self, end: &MemoryEnd, data: &[u8], now: u32) {
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

    pub fn connected(&self) -> bool {
        *self.client.state() == ClientState::Connected
    }
}

/// Runs `ticks` server ticks, carrying the datagrams between the server and the clients.
pub fn run(s: &mut NetServer, end: &MemoryEnd, clients: &mut [&mut TestClient], ticks: usize) {
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

pub fn connection_of(shared: &Rc<RefCell<NetShared<TorqueNet>>>, addr: SocketAddr) -> ObjId {
    shared
        .borrow()
        .conns
        .values()
        .find(|r| r.addr == addr)
        .map(|r| r.obj)
        .expect("the connection's object")
}

pub fn eval(s: &mut NetServer, code: &str) {
    s.vm.eval(code.as_bytes()).unwrap();
}
