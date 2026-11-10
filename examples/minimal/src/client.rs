//! The client: torque-net's client half of the handshake and the packet layer, a ghost table read with TGE's record codecs
//! (`TgeCodec`), and the client's world of ghosts over TGE's client set (`TgeClientSet`).
//!
//! **One tick** ([`ExampleClient::tick`]): drain the socket (handshake replies; server packets: the head, the event section with
//! the ghosting messages, the ghost section, which the world then takes), then a data packet when the send gate is open (the
//! client's head with no moves, and the event section: the client's answer to the ghosting messages). Until it is accepted, the
//! client sends its challenge request again every second.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use torque::ghost_world::ClientWorld;
use torque::ghost_world::tge::{TgeClientSet, TgeCodec};
use torque::ghost_world::world::Pose;
use torque::ghosts::NetWidths;
use torque::net::client::{Client, ClientEvent as Handshake, ClientState};
use torque::net::control::{ClientBlock, ServerHead};
use torque::net::events::{ConnectionMessage, EventQueue, EventReceiver, NetEvent, SectionParams};
use torque::net::ghost::{ClientEvent, GhostClient, GhostEventReader};
use torque::net::handshake::ConnectFields;
use torque::net::profile::DEFAULT_DATABLOCK_BITS;
use torque::net::protocol::{Received, Role, reader_at};
use torque::server::net::transport::{
    BindError, BindPolicy, LanRules, RECV_BUFFER_BYTES, Recv, Transport, UdpTransport,
};

use crate::server::NetTables;

/// The most datagrams one tick reads.
const DATAGRAMS_PER_TICK: usize = 1024;
/// How often a challenge request is sent again until the server answers.
const RETRY_MS: u32 = 1000;

/// What the client tells its host.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientNote {
    Accepted {
        ghost_bits: u8,
    },
    Rejected(String),
    Disconnected(String),
    /// The server's ghosting began (its `ConnectionMessage` 0 arrived and was answered).
    GhostingStarted,
    /// A packet that did not read: the connection is unusable.
    BadPacket(String),
}

/// One ghost as the client sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct SeenGhost {
    pub index: u32,
    pub class: &'static str,
    pub datablock: Option<u32>,
    pub position: Option<[f32; 3]>,
    pub updates: u32,
}

impl std::fmt::Display for SeenGhost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ghost {} {}", self.index, self.class)?;
        if let Some(db) = self.datablock {
            write!(f, " datablock {db}")?;
        }
        if let Some([x, y, z]) = self.position {
            write!(f, " at ({x:.2}, {y:.2}, {z:.2})")?;
        }
        write!(f, ", {} updates", self.updates)
    }
}

/// The client.
pub struct ExampleClient {
    transport: Box<dyn Transport>,
    server: SocketAddr,
    client: Client,
    nets: NetTables,
    ghosts: Option<GhostClient<TgeCodec>>,
    world: ClientWorld<TgeClientSet>,
    events: EventQueue<NetEvent>,
    receiver: EventReceiver<ClientEvent>,
    widths: NetWidths,
    notes: Vec<ClientNote>,
    packets: usize,
    last_try: Option<u32>,
}

impl ExampleClient {
    /// A client on a UDP socket of the loopback address that will connect to `server`.
    pub fn connect(server: SocketAddr) -> Result<ExampleClient, BindError> {
        let address = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let policy =
            BindPolicy::new(address, false, &LanRules::PLAIN).map_err(BindError::Policy)?;
        let mut transport = UdpTransport::new(policy);
        transport.bind(address, 0)?;
        Ok(ExampleClient::over(Box::new(transport), server))
    }

    /// A client over a bound transport.
    pub fn over(transport: Box<dyn Transport>, server: SocketAddr) -> ExampleClient {
        // The connect sequence tells this connection's packets from an earlier one's; any value will do.
        let sequence = std::process::id().wrapping_mul(2_654_435_761) | 1;
        ExampleClient {
            transport,
            server,
            client: Client::new(sequence, ConnectFields::standard(b"", Vec::new())),
            nets: NetTables::tge(),
            ghosts: None,
            world: ClientWorld::default(),
            events: EventQueue::new(),
            receiver: EventReceiver::new(),
            widths: NetWidths::engine_default(),
            notes: Vec::new(),
            packets: 0,
            last_try: None,
        }
    }

    /// Whether the server accepted the client and the connection is up.
    pub fn connected(&self) -> bool {
        *self.client.state() == ClientState::Connected
    }

    /// The client's world of ghosts.
    pub fn world(&self) -> &ClientWorld<TgeClientSet> {
        &self.world
    }

    /// The server packets read so far.
    pub fn packets(&self) -> usize {
        self.packets
    }

    /// What happened since the last call.
    pub fn take_notes(&mut self) -> Vec<ClientNote> {
        std::mem::take(&mut self.notes)
    }

    /// The ghosts the client has, by ghost index.
    pub fn seen(&self) -> Vec<SeenGhost> {
        self.world
            .ghosts
            .iter()
            .map(|(&index, ghost)| SeenGhost {
                index,
                class: ghost.class.map_or("placeholder", |c| c.name()),
                datablock: ghost.datablock(),
                position: ghost.pose().map(|pose| match pose {
                    Pose::Affine { xf, .. } => xf.position.map(|f| f.get()),
                    Pose::Matrix { m, .. } => [m[3], m[7], m[11]],
                    Pose::Euler { position, .. }
                    | Pose::Quat { position, .. }
                    | Pose::Item { position, .. } => position,
                }),
                updates: ghost.updates,
            })
            .collect()
    }

    /// One tick at `now` (milliseconds of the client's clock).
    pub fn tick(&mut self, now: u32) {
        if matches!(
            self.client.state(),
            ClientState::Idle | ClientState::Challenging | ClientState::Connecting
        ) && self
            .last_try
            .is_none_or(|t| now.wrapping_sub(t) >= RETRY_MS)
        {
            self.last_try = Some(now);
            let request = self.client.challenge_request();
            self.transport.send_to(self.server, &request);
        }
        let mut buf = vec![0u8; RECV_BUFFER_BYTES];
        for _ in 0..DATAGRAMS_PER_TICK {
            match self.transport.recv(&mut buf) {
                Recv::Empty => break,
                Recv::Skipped => {}
                Recv::Datagram { len, from } if from == self.server => {
                    self.datagram(&buf[..len], now);
                }
                Recv::Datagram { .. } => {}
            }
        }
        if self.connected() {
            self.send(now);
        }
    }

    /// Says goodbye to the server.
    pub fn disconnect(&mut self) {
        if self.connected() {
            let datagram = self
                .client
                .disconnect_datagram(&self.nets.table, b"Client quit.");
            self.transport.send_to(self.server, &datagram);
        }
        self.transport.close();
    }

    fn datagram(&mut self, data: &[u8], now: u32) {
        match self.client.on_datagram(&self.nets.table, data, now) {
            Handshake::Nothing => {}
            Handshake::Send(reply) => {
                self.transport.send_to(self.server, &reply);
            }
            Handshake::Accepted(accept) => {
                let ghost_bits = accept.ghost_bits.map_or_else(
                    || {
                        self.nets
                            .ghost_config(torque::net::profile::DEFAULT_GHOST_LIMIT)
                            .ghost_bits()
                    },
                    |bits| bits as u8,
                );
                self.widths = NetWidths::new(ghost_bits, DEFAULT_DATABLOCK_BITS);
                let names = self
                    .nets
                    .classes
                    .classes(torque::net::classes::Kind::Object)
                    .iter()
                    .map(|c| c.name.as_str());
                let codec = TgeCodec::new(names, self.widths, self.nets.table.clone());
                let class_bits = self
                    .nets
                    .classes
                    .id_bits(torque::net::classes::Kind::Object);
                let mut ghosts = GhostClient::new(codec, class_bits, ghost_bits);
                ghosts.enable_log();
                self.ghosts = Some(ghosts);
                self.notes.push(ClientNote::Accepted { ghost_bits });
            }
            Handshake::Rejected(reason) => self.notes.push(ClientNote::Rejected(
                String::from_utf8_lossy(&reason).into_owned(),
            )),
            Handshake::Disconnected(reason) => self.notes.push(ClientNote::Disconnected(
                String::from_utf8_lossy(&reason).into_owned(),
            )),
            Handshake::Game {
                received,
                payload_bit,
            } => {
                if let Err(e) = self.game_packet(&received, data, payload_bit, now) {
                    self.notes.push(ClientNote::BadPacket(e.to_string()));
                }
            }
            Handshake::GameError(e) => self.notes.push(ClientNote::BadPacket(e.to_string())),
        }
    }

    fn game_packet(
        &mut self,
        received: &Received,
        data: &[u8],
        payload_bit: usize,
        now: u32,
    ) -> Result<(), torque::net::NetError> {
        let mut settled = Vec::new();
        for notify in &received.notifies {
            self.events
                .on_notify(notify.send_seq, notify.delivered, &mut settled);
        }
        if received.answer_ping
            && let Some(protocol) = self.client.protocol_mut()
        {
            let ack = protocol.ack_packet(now);
            self.transport.send_to(self.server, &ack);
        }
        let Some(ghosts) = self.ghosts.as_mut() else {
            return Ok(());
        };
        if !received.payload {
            return Ok(());
        }
        let mut r = reader_at(data, payload_bit);
        r.enable_string_buffer();
        let mut charge = torque::net::events::free_charge();
        ServerHead::read(&mut r, self.widths.ghost_bits, &mut charge)?;
        let mut out = Vec::new();
        self.receiver.read_section(
            &mut r,
            &self.nets.event_ctx(self.widths),
            Role::Client,
            &mut GhostEventReader { client: ghosts },
            &mut out,
            None,
        )?;
        ghosts.read_section(&mut r)?;
        self.packets += 1;
        for incoming in out {
            let ClientEvent::Message(message) = incoming.event else {
                continue;
            };
            match message.message {
                // GhostAlwaysDone: the client has the ghost-always objects and asks for the rest.
                0 => {
                    let ready = NetEvent::ConnectionMessage(ConnectionMessage {
                        seq: message.seq,
                        message: 1,
                        count: 0,
                    });
                    // A new connection's queue takes the one message.
                    let _ = self.events.post(ready);
                    self.notes.push(ClientNote::GhostingStarted);
                }
                // EndGhosting: the server reset its ghosts.
                2 => {
                    ghosts.clear();
                    self.world = ClientWorld::default();
                }
                _ => {}
            }
        }
        let changes = ghosts.take_log();
        self.world.set_clock(now);
        // The head sends no control object, so the stream's compression point stays at the origin.
        self.world
            .apply_ghosts(self.packets, &changes, &*ghosts, [0.0; 3], &mut ());
        Ok(())
    }

    fn send(&mut self, now: u32) {
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
        // The client's events are a few bits each: an error is not possible here.
        let _ = self.events.write_section_stalling(
            &mut w,
            &self.nets.event_ctx(self.widths),
            send_seq,
            params,
        );
        self.transport.send_to(self.server, &w.into_bytes());
    }
}
