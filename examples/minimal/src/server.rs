//! The dedicated server: the game's scripts ([`crate::game`]), a datagram transport (torque-server's), the handshake and the
//! connection table (torque-net's `handshake::Server`), and per connection the event queue and the ghost manager, whose records
//! are TGE's (`TgeGhosts`).
//!
//! **One tick** ([`DedicatedServer::tick`]): drain the socket (handshake datagrams, client packets: acks, the client's head, its
//! events), the connections' timeouts, the scripts' clock (`schedule`), the objects' changes, then a data packet to every
//! connection whose send gate is open: the server head, the event section and the ghost section.
//!
//! **What the example leaves out** (a game adds them): the mission phases and the datablocks a client is sent before ghosting
//! (here ghosting starts as soon as a client is accepted, and the records carry datablock ids only), a control object (the head
//! sends none; the client's camera is only the scope object), moves, remote commands and net strings, and the server query.

use std::collections::BTreeMap;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use torque::ghosts::{GhostCodecSet, NetWidths, PackCtx, TgeClass, TgeGhosts, tge_reps};
use torque::net::bits::BitWriter;
use torque::net::classes::{Kind, NetClasses};
use torque::net::control::{CameraPacketData, ClientBlock, ControlSection, Damage, ServerHead};
use torque::net::events::{
    EventCtx, EventIds, EventQueue, EventReceiver, EventRegistry, NetEvent, NetEventUnpacker,
    SectionParams, free_charge,
};
use torque::net::ghost::{
    CamInfo, GhostConfig, GhostEvent, GhostManager, MessageOutcome, NoScripts, ObjId, ObjectSource,
    SectionBudget, net_flags,
};
use torque::net::handshake::{
    self, ConnId, Decision, Finished, Handled, ServerConfig, TimeoutEvent,
};
use torque::net::huffman::Table;
use torque::net::profile::DEFAULT_DATABLOCK_BITS;
use torque::net::protocol::{Received, Role, reader_at};
use torque::script::text::atoi;
use torque::script::vm::vm::Vm;
use torque::server::net::transport::{
    BindError, BindPolicy, LanRules, RECV_BUFFER_BYTES, Recv, Transport, UdpTransport,
};
use torque::server::pacing::PassClock;

use crate::game::{self, LoadError};
use crate::view::{ScriptView, ghost_class, moved_mask, placement};

/// The most datagrams one tick reads (the rest wait for the next tick).
const DATAGRAMS_PER_TICK: usize = 1024;

/// How a server is made.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// The game folder (the one that holds the `example` folder).
    pub root: PathBuf,
    /// The address the socket binds: a private or loopback address (the bind policy refuses a public one).
    pub address: Ipv4Addr,
    /// The port number; 0 takes any free one.
    pub port: u16,
    /// `$Pref::Server::MaxPlayers`.
    pub max_players: usize,
}

impl ServerOptions {
    /// The bundled game on the loopback address.
    pub fn loopback(port: u16) -> ServerOptions {
        ServerOptions {
            root: game::bundled_root().to_path_buf(),
            address: Ipv4Addr::LOCALHOST,
            port,
            max_players: 8,
        }
    }
}

/// Why a server did not start.
#[derive(Debug)]
pub enum StartError {
    Load(LoadError),
    Bind(BindError),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::Load(e) => write!(f, "the scripts did not load: {e}"),
            StartError::Bind(e) => write!(f, "the socket did not open: {e}"),
        }
    }
}

impl std::error::Error for StartError {}

/// What the server tells its host (the `min-server` program prints them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerNote {
    /// A line the scripts printed.
    Console(String),
    Connected {
        conn: u32,
        from: SocketAddr,
    },
    /// The client said it has the ghost-always objects: normal ghosting began.
    Ghosting {
        conn: u32,
    },
    Left {
        conn: u32,
        reason: String,
    },
}

/// The tables both sides of a connection derive the same way: the string coder, the event ids, the net classes.
#[derive(Debug)]
pub struct NetTables {
    pub table: Table,
    pub ids: EventIds,
    pub classes: NetClasses,
}

impl NetTables {
    /// TGE's: Torque's string coder table, Torque's event classes, and the net classes of TGE's class list.
    pub fn tge() -> NetTables {
        let events = EventRegistry::torque();
        NetTables {
            table: Table::torque(),
            ids: events.ids().expect("Torque's event classes derive"),
            classes: NetClasses::derive(&tge_reps(&events)).expect("TGE's classes derive"),
        }
    }

    /// The ghost settings of a connection: the ghost limit and the width of an object class id.
    pub fn ghost_config(&self, max_ghosts: u32) -> GhostConfig {
        GhostConfig {
            max_ghosts,
            local: false,
            class_bits: self.classes.id_bits(Kind::Object),
        }
    }

    /// The event layer's context for a connection of `widths`.
    pub fn event_ctx(&self, widths: NetWidths) -> EventCtx<'_> {
        EventCtx {
            table: &self.table,
            ids: &self.ids,
            ghost_bits: widths.ghost_bits,
            datablock_bits: widths.datablock_bits,
        }
    }
}

/// One client the server ghosts to.
#[derive(Debug)]
struct Client {
    /// The camera `onClientEnterGame` made: the scope object.
    camera: Option<ObjId>,
    ghosts: GhostManager,
    events: EventQueue<GhostEvent>,
    receiver: EventReceiver<NetEvent>,
    widths: NetWidths,
}

/// The server's objects as the ghost manager reads them.
struct Scene<'a> {
    vm: &'a Vm,
    ghosted: &'a BTreeMap<ObjId, (TgeClass, Vec<Vec<u8>>)>,
    nets: &'a NetTables,
    widths: NetWidths,
    now: u32,
}

impl ObjectSource for Scene<'_> {
    fn exists(&self, obj: ObjId) -> bool {
        self.ghosted.contains_key(&obj)
    }

    fn class_id(&self, obj: ObjId) -> Option<u32> {
        let (class, _) = self.ghosted.get(&obj)?;
        self.nets.classes.id_of(Kind::Object, class.name())
    }

    fn net_flags(&self, obj: ObjId) -> u32 {
        if self.exists(obj) {
            net_flags::GHOSTABLE
        } else {
            0
        }
    }

    fn pack_update(
        &mut self,
        obj: ObjId,
        mask: u32,
        w: &mut BitWriter,
        _ghost_of: &dyn Fn(ObjId) -> Option<u32>,
    ) -> u32 {
        let Some(&(class, _)) = self.ghosted.get(&obj) else {
            return 0;
        };
        let view = ScriptView {
            vm: self.vm,
            id: obj,
            class,
        };
        let ctx = PackCtx::for_writer(w, self.widths, self.now);
        let (record, ret) = TgeGhosts::pack(class, &view, mask, &ctx);
        // A record that does not fit overflows the stream; the manager sees the overflow and rolls the record back.
        let _ = TgeGhosts::encode(&record, w, &self.nets.table, self.widths);
        ret
    }

    fn update_priority(&self, _obj: ObjId, _cam: &CamInfo, _mask: u32, _skip: u8) -> f32 {
        1.0
    }

    fn camera_info(&self, scope: ObjId) -> Option<CamInfo> {
        let view = ScriptView {
            vm: self.vm,
            id: scope,
            class: TgeClass::Camera,
        };
        let eye = std::array::from_fn(|i| torque::ghosts::fld(&view, "position", i));
        Some(CamInfo {
            object: scope,
            eye,
            forward: [0.0, 1.0, 0.0],
            ..CamInfo::default()
        })
    }

    fn sky_visible_distance(&self) -> Option<f32> {
        None
    }

    fn mount_of(&self, _scope: ObjId) -> Option<ObjId> {
        None
    }

    /// The example has no scene graph: every ghosted object is in scope.
    fn scope_query(
        &self,
        _scope: ObjId,
        _cam: &CamInfo,
        _radius: f32,
        visit: &mut dyn FnMut(ObjId) -> bool,
    ) {
        for &id in self.ghosted.keys() {
            if !visit(id) {
                return;
            }
        }
    }

    fn scope_distance(&self, obj: ObjId, _scope: ObjId) -> Option<f32> {
        self.exists(obj).then_some(0.0)
    }

    fn ghost_always(&self) -> &[ObjId] {
        &[]
    }
}

/// A key for the challenge digests from the standard library's per-process random seed (a game draws one from the OS).
fn challenge_key() -> [u8; 16] {
    let state = RandomState::new();
    let mut key = [0u8; 16];
    key[..8].copy_from_slice(&state.hash_one(1u8).to_le_bytes());
    key[8..].copy_from_slice(&state.hash_one(2u8).to_le_bytes());
    key
}

/// The dedicated server.
pub struct DedicatedServer {
    vm: Vm,
    transport: Box<dyn Transport>,
    handshake: handshake::Server,
    nets: NetTables,
    clients: BTreeMap<ConnId, Client>,
    /// The ghosted objects: their class and the placement fields last sent.
    ghosted: BTreeMap<ObjId, (TgeClass, Vec<Vec<u8>>)>,
    notes: Vec<ServerNote>,
    now: u32,
    ticks: u64,
}

impl DedicatedServer {
    /// Loads the game and binds a UDP socket on `options.address:options.port`.
    pub fn start(options: &ServerOptions) -> Result<DedicatedServer, StartError> {
        let address = IpAddr::V4(options.address);
        let policy = BindPolicy::new(address, false, &LanRules::PLAIN)
            .map_err(|why| StartError::Bind(BindError::Policy(why)))?;
        let mut transport = UdpTransport::new(policy);
        transport
            .bind(address, options.port)
            .map_err(StartError::Bind)?;
        let vm = game::load(&options.root).map_err(StartError::Load)?;
        Ok(DedicatedServer::over(
            vm,
            Box::new(transport),
            options.max_players,
        ))
    }

    /// A server over a loaded game and a bound transport (an in-memory one in tests).
    pub fn over(vm: Vm, transport: Box<dyn Transport>, max_players: usize) -> DedicatedServer {
        let mut server = DedicatedServer {
            vm,
            transport,
            handshake: handshake::Server::new(ServerConfig::new(max_players), challenge_key()),
            nets: NetTables::tge(),
            clients: BTreeMap::new(),
            ghosted: BTreeMap::new(),
            notes: Vec::new(),
            now: 0,
            ticks: 0,
        };
        server.take_console();
        server.sync_objects();
        server
    }

    /// The address the socket is bound to.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.transport.local_addr()
    }

    /// The script engine (the mission's objects, the console).
    pub fn vm(&self) -> &Vm {
        &self.vm
    }

    /// The connected clients.
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    /// The objects the server ghosts.
    pub fn ghosted_count(&self) -> usize {
        self.ghosted.len()
    }

    /// The ticks run so far.
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// What happened since the last call.
    pub fn take_notes(&mut self) -> Vec<ServerNote> {
        std::mem::take(&mut self.notes)
    }

    /// Runs `ticks` ticks paced by the wall clock as TGE's dedicated main loop is (one every 15.625 ms), with `each` called after
    /// every tick.
    pub fn run(&mut self, ticks: u64, mut each: impl FnMut(&mut DedicatedServer)) {
        let mut clock = PassClock::new();
        let mut now = self.now;
        for _ in 0..ticks {
            now = now.wrapping_add(clock.wait_pass());
            self.tick(now);
            each(self);
        }
    }

    /// One tick at `now` (milliseconds of the server's clock).
    pub fn tick(&mut self, now: u32) {
        let elapsed = now.wrapping_sub(self.now);
        self.now = now;
        self.receive(now);
        for event in self.handshake.poll_timeouts(now) {
            match event {
                TimeoutEvent::Ping { to, datagram, .. } => {
                    self.transport.send_to(to, &datagram);
                }
                TimeoutEvent::TimedOut { conn } => self.leave(conn, "timed out"),
                TimeoutEvent::PendingExpired { conn } => self.leave(conn, "never heard from"),
            }
        }
        if elapsed > 0 {
            for error in self.vm.advance_time(elapsed) {
                self.notes
                    .push(ServerNote::Console(format!("script error: {error:?}")));
            }
        }
        self.take_console();
        self.sync_objects();
        self.send(now);
        self.ticks += 1;
    }

    /// Disconnects every client (`Server shutting down.`).
    pub fn shutdown(&mut self) {
        for conn in self.handshake.ids() {
            if let Some((to, datagram)) =
                self.handshake
                    .disconnect_datagram(&self.nets.table, conn, b"Server shutting down.")
            {
                self.transport.send_to(to, &datagram);
            }
            self.handshake.remove(conn);
            self.leave(conn, "server shut down");
        }
        self.transport.close();
    }

    fn take_console(&mut self) {
        for line in self.vm.drain_log() {
            self.notes.push(ServerNote::Console(
                String::from_utf8_lossy(&line.text).into_owned(),
            ));
        }
    }

    // ---- receiving

    fn receive(&mut self, now: u32) {
        let mut buf = vec![0u8; RECV_BUFFER_BYTES];
        for _ in 0..DATAGRAMS_PER_TICK {
            match self.transport.recv(&mut buf) {
                Recv::Empty => break,
                Recv::Skipped => {}
                Recv::Datagram { len, from } => self.datagram(from, &buf[..len], now),
            }
        }
    }

    fn datagram(&mut self, from: SocketAddr, data: &[u8], now: u32) {
        match self
            .handshake
            .handle_datagram(&self.nets.table, from, data, now)
        {
            Handled::Ignored(_) => {}
            Handled::Reply(reply) => {
                self.transport.send_to(from, &reply);
            }
            Handled::Request(request) => {
                match self.handshake.finish_request(
                    &self.nets.table,
                    *request,
                    Decision::Accept,
                    now,
                ) {
                    Finished::Accepted {
                        conn,
                        reply,
                        replaced,
                    } => {
                        if let Some(old) = replaced {
                            self.leave(old, "replaced by a new connection");
                        }
                        self.transport.send_to(from, &reply);
                        self.enter(conn, from);
                    }
                    Finished::Rejected { reply } => {
                        self.transport.send_to(from, &reply);
                    }
                    Finished::Refused => {}
                }
            }
            Handled::Game {
                conn,
                received,
                payload_bit,
            } => self.game_packet(conn, &received, data, payload_bit, now),
            Handled::GameError { conn, .. } => self.drop_client(conn, b"Packet Error."),
            Handled::Disconnected { conn, reason } => {
                self.leave(conn, &String::from_utf8_lossy(&reason));
            }
        }
    }

    /// A client was accepted: the scripts make its camera, and ghosting is activated at once.
    fn enter(&mut self, conn: ConnId, from: SocketAddr) {
        let camera = self
            .vm
            .invoke(
                None,
                b"onClientEnterGame",
                &[conn.0.to_string().into_bytes()],
            )
            .ok()
            .and_then(|id| u32::try_from(atoi(&id)).ok())
            .filter(|&id| self.vm.objects().contains(id));
        self.take_console();
        self.sync_objects();
        let cfg = self.nets.ghost_config(self.handshake.config().ghost_limit);
        let widths = NetWidths::new(cfg.ghost_bits(), DEFAULT_DATABLOCK_BITS);
        let Ok(mut ghosts) = GhostManager::new(cfg) else {
            return;
        };
        ghosts.set_scope_object(camera);
        let scene = Scene {
            vm: &self.vm,
            ghosted: &self.ghosted,
            nets: &self.nets,
            widths,
            now: self.now,
        };
        let mut events = EventQueue::new();
        for event in ghosts.activate(&scene).unwrap_or_default() {
            // A new queue takes the activation's few events.
            let _ = events.post(event);
        }
        self.clients.insert(
            conn,
            Client {
                camera,
                ghosts,
                events,
                receiver: EventReceiver::new(),
                widths,
            },
        );
        self.notes
            .push(ServerNote::Connected { conn: conn.0, from });
    }

    fn game_packet(
        &mut self,
        conn: ConnId,
        received: &Received,
        data: &[u8],
        payload_bit: usize,
        now: u32,
    ) {
        let Some(client) = self.clients.get_mut(&conn) else {
            return;
        };
        let mut settled = Vec::new();
        for notify in &received.notifies {
            client
                .events
                .on_notify(notify.send_seq, notify.delivered, &mut settled);
            client.ghosts.on_notify(notify.send_seq, notify.delivered);
        }
        if received.answer_ping
            && let Some(connection) = self.handshake.connection_mut(conn)
        {
            let ack = connection.protocol.ack_packet(now);
            self.transport.send_to(connection.addr, &ack);
        }
        if !received.payload {
            return;
        }
        let mut r = reader_at(data, payload_bit);
        r.enable_string_buffer();
        let mut out = Vec::new();
        let read = ClientBlock::read(&mut r, &mut free_charge()).and_then(|_| {
            client.receiver.read_section(
                &mut r,
                &self.nets.event_ctx(client.widths),
                Role::Server,
                &mut NetEventUnpacker,
                &mut out,
                None,
            )
        });
        let mut valid = read.is_ok();
        for incoming in out {
            let NetEvent::ConnectionMessage(message) = incoming.event else {
                continue;
            };
            match client
                .ghosts
                .on_connection_message(&message, &mut NoScripts)
            {
                Ok(MessageOutcome::Ready) => self.notes.push(ServerNote::Ghosting { conn: conn.0 }),
                Ok(_) => {}
                Err(_) => valid = false,
            }
        }
        if !valid {
            self.drop_client(conn, b"Invalid packet.");
        }
    }

    /// Disconnects a client with `reason`.
    fn drop_client(&mut self, conn: ConnId, reason: &[u8]) {
        if let Some((to, datagram)) =
            self.handshake
                .disconnect_datagram(&self.nets.table, conn, reason)
        {
            self.transport.send_to(to, &datagram);
        }
        self.handshake.remove(conn);
        self.leave(conn, &String::from_utf8_lossy(reason));
    }

    /// A client is gone: the scripts delete its camera.
    fn leave(&mut self, conn: ConnId, reason: &str) {
        let Some(client) = self.clients.remove(&conn) else {
            return;
        };
        let camera = client.camera.unwrap_or(0).to_string();
        let _ = self.vm.invoke(
            None,
            b"onClientLeaveGame",
            &[conn.0.to_string().into_bytes(), camera.into_bytes()],
        );
        self.take_console();
        self.notes.push(ServerNote::Left {
            conn: conn.0,
            reason: reason.to_string(),
        });
    }

    // ---- the objects

    /// The ghosted objects as the scripts left them: an object gone is detached from every connection, an object whose
    /// placement changed is marked dirty for every connection. A new object needs nothing: the next scope query finds it.
    fn sync_objects(&mut self) {
        let mut now = BTreeMap::new();
        for (id, _) in self.vm.objects().iter() {
            if let Some(class) = ghost_class(&self.vm, id) {
                now.insert(id, (class, placement(&self.vm, id)));
            }
        }
        for (id, (class, old)) in &self.ghosted {
            match now.get(id) {
                None => {
                    for client in self.clients.values_mut() {
                        client.ghosts.detach_object(*id);
                    }
                }
                Some((_, placed)) if placed != old => {
                    for client in self.clients.values_mut() {
                        client.ghosts.set_mask(*id, moved_mask(*class));
                    }
                }
                Some(_) => {}
            }
        }
        self.ghosted = now;
    }

    // ---- sending

    fn send(&mut self, now: u32) {
        let DedicatedServer {
            vm,
            transport,
            handshake,
            nets,
            clients,
            ghosted,
            ..
        } = self;
        for (conn, client) in clients.iter_mut() {
            let Some(connection) = handshake.connection_mut(*conn) else {
                continue;
            };
            let protocol = &mut connection.protocol;
            if !protocol.may_send(now, false) {
                continue;
            }
            let mut w = protocol.begin_data_packet(now);
            w.enable_string_buffer();
            let send_seq = protocol.last_send_seq();
            let packet_bits = protocol.packet_limit_bits();
            ServerHead::<CameraPacketData> {
                move_ack: 0,
                damage: Damage::default(),
                control: ControlSection::None,
                camera_object: None,
                camera_data: None,
                first_person: None,
                fov: None,
            }
            .write(&mut w, client.widths.ghost_bits);
            let mut scene = Scene {
                vm,
                ghosted,
                nets,
                widths: client.widths,
                now,
            };
            let params = SectionParams {
                ghosting: client.ghosts.is_ghosting(),
                hold_ordered: false,
                packet_bits,
                min_start_bits: w.bits(),
            };
            let ctx = nets.event_ctx(client.widths);
            {
                let mut pack = client.ghosts.pack_ctx(&mut scene);
                // An event no packet could carry stalls the queue; the example's events are a few bits each.
                let _ = client
                    .events
                    .write_section_with_stalling(&mut w, &ctx, &mut pack, send_seq, params);
                for (obj, bits) in pack.into_left() {
                    client.ghosts.set_mask(obj, bits);
                }
            }
            client.ghosts.write_section(
                &mut w,
                packet_bits,
                send_seq,
                now,
                &mut scene,
                SectionBudget {
                    scan: &mut |_| true,
                    records: &mut |_| true,
                },
            );
            transport.send_to(connection.addr, &w.into_bytes());
        }
    }
}
