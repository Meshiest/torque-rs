//! Datablock removal on the wire, in the engine's protocol over the memory link with a fifth of the datagrams lost and the rest
//! reordered: a client of the engine's framing (a `Session` driven by hand) reads the server's ghost lane as a TGE payload, keeps
//! the datablocks it was sent in a `ClientWorld`, retires the ones the server removes and purges them once no ghost names them.
//! Over seeded orders of repoints, deletes over users and plain removals the client's set ends equal to the server's, every ghost
//! always names a datablock the client holds, each removed id is told once and after its body, and a retired datablock is purged
//! within two ticks of the last ghost leaving it. A server of Torque's protocol posts nothing.

use crate::net_client::{self, NetServer, connection_of, eval};

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::SocketAddr;
use std::rc::Rc;

use torque_engine::ProtocolKind;
use torque_ghost_world::ClientWorld;
use torque_ghost_world::tge::{TgeClientSet, TgeCodec, TgeDatablock};
use torque_ghost_world::world::DatablockRetire;
use torque_ghosts::datablocks::DatablockCodecSet;
use torque_ghosts::datablocks::fields::fields;
use torque_ghosts::{NetWidths, TgeDatablocks, tge_reps};
use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};
use torque_net::classes::{Kind, NetClasses};
use torque_net::control::{ClientBlock, ServerHead};
use torque_net::engine::bridge::connect_of;
use torque_net::engine::{
    Event, Link, LinkFacts, LinkKind, NoRoom, PacketOrder, Session, SessionConfig, StreamLane,
    StreamSource,
};
use torque_net::events::{
    ConnectionMessage, EventCtx, EventIds, EventQueue, EventReceiver, EventRegistry, EventUnpacker,
    NetEvent, SectionParams,
};
use torque_net::ghost::GhostClient;
use torque_net::huffman::Table;
use torque_net::profile::{DEFAULT_DATABLOCK_BITS, HandshakeProfile};
use torque_net::protocol::{Role, reader_at};
use torque_server::net::NetShared;
use torque_server::net::codecs::{DATABLOCKS_DONE, DATABLOCKS_DOWNLOAD_DONE};
use torque_server::net::events::RemovePostError;
use torque_server::net::peer::ProtocolMode;
use torque_server::net::torque::TorqueNet;
use torque_server::net::transport::MemoryEnd;
use torque_server::server::TICK_MS;
use torque_ts_vm::sim::ObjId;

type Shared = Rc<RefCell<NetShared<TorqueNet>>>;

const SERVER: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 28_000);

/// A camera to scope from, two shapes, and a connection that gets its datablocks at once and ghosts from the camera.
const SCENE: &str = r#"
new SimGroup(ClientGroup);
datablock CameraData(EyeData) { mass = 1; };
new Camera(Eye) { dataBlock = EyeData; position = "0 0 0"; };
function GameConnection::onConnect(%client) { ClientGroup.add(%client); %client.transmitDataBlocks(1); }
function GameConnection::onDataBlocksDone(%client, %seq) { $done++; %client.setControlObject(Eye); %client.activateGhosting(); }
function GameConnection::onDataBlocksUpdated(%client, %count) { $updated++; }
"#;

/// A server of `protocol` with the game classes, the shapes the datablocks name and the scene.
fn game_server(protocol: ProtocolKind) -> (NetServer, Shared, MemoryEnd) {
    use torque_server::net::torque::{NetPasses, TgeGhostGame};
    use torque_server::net::transport::{MemoryTransport, SharedTransport};
    use torque_server::net::{Host, HostParts, NetClock, attach_host};
    use torque_server::state::ServerState;
    use torque_ts_vm::host::MemFiles;

    let files = MemFiles::new();
    for (name, half) in [("a.dts", 1.0), ("b.dts", 2.0)] {
        let b = [-half, -half, 0.0, half, half, half];
        let shape = torque_shapes::synth::hull_shape(b, &[("detail2", b)], &[], &["ambient"]);
        files.insert(name, &torque_shapes::write::write_dts(&shape));
    }
    let mut vm = torque_game::testing::vm();
    vm.set_file_host(files);
    let state = Rc::new(RefCell::new(ServerState::<HostParts<TorqueNet>>::default()));
    let game = torque_server::game::install(&mut vm, &state);
    let mut s = NetServer::with_state(vm, Rc::clone(&state));
    s.vm.set_extension(Rc::new(ProtocolMode(protocol)));
    let (transport, end) = MemoryTransport::new();
    let transport: SharedTransport = Rc::new(RefCell::new(transport));
    transport
        .borrow_mut()
        .bind(SERVER.ip(), SERVER.port())
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
    eval(&mut s, SCENE);
    (s, shared, end)
}

fn answer(s: &mut NetServer, code: &str) -> String {
    String::from_utf8(s.vm.eval(code.as_bytes()).unwrap()).unwrap()
}

fn id_of(s: &NetServer, name: &str) -> ObjId {
    s.vm.objects().find(name.as_bytes()).expect(name)
}

/// The server's datablock ids.
fn server_ids(s: &NetServer) -> BTreeSet<u32> {
    let objects = s.vm.objects();
    objects
        .members(objects.datablock_group())
        .map(|m| (0..m.len()).filter_map(|i| m.get(i)).collect())
        .unwrap_or_default()
}

/// `xorshift64*`: the seeded choices of a run.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

// ---- the client

/// An event of the server's ghost lane as the client reads it.
#[derive(Debug, Clone)]
enum Ev {
    Message(ConnectionMessage),
    Always,
    /// A datablock body: the id and the record's fields.
    DataBlock(Option<(u32, TgeDatablock)>),
    Remove(u32),
}

struct Unpacker<'a> {
    ghosts: &'a mut GhostClient<TgeCodec>,
    classes: &'a NetClasses,
}

impl EventUnpacker for Unpacker<'_> {
    type Event = Ev;

    fn unpack(
        &mut self,
        class: u32,
        r: &mut BitReader,
        ctx: &EventCtx<'_>,
    ) -> Result<Ev, NetError> {
        let widths = NetWidths::new(ctx.ghost_bits, ctx.datablock_bits)
            .with_extensions(ctx.ids.datablock_remove().is_some());
        let (lo, hi) = widths.datablock_range();
        if class == ctx.ids.ghost_always_object {
            return self.ghosts.read_always_event(r).map(|_| Ev::Always);
        }
        if ctx.ids.datablock_remove() == Some(class) {
            return r.read_ranged_u32(lo, hi).map(Ev::Remove);
        }
        if class != ctx.ids.sim_data_block {
            return NetEvent::unpack(class, r, ctx).map(|e| match e {
                NetEvent::ConnectionMessage(m) => Ev::Message(m),
                _ => Ev::DataBlock(None),
            });
        }
        if !r.read_flag()? {
            return Ok(Ev::DataBlock(None));
        }
        let id = r.read_ranged_u32(lo, hi)?;
        let class_id = r.read_bits(self.classes.id_bits(Kind::DataBlock))?;
        let _index = r.read_bits(ctx.datablock_bits)?;
        let _total = r.read_bits(ctx.datablock_bits + 1)?;
        let name = self
            .classes
            .classes(Kind::DataBlock)
            .get(class_id as usize)
            .map(|c| c.name.as_str())
            .ok_or(NetError::Invalid("datablock class id"))?;
        let class =
            TgeDatablocks::class_by_name(name).ok_or(NetError::Invalid("datablock class"))?;
        let mut charge = torque_ghosts::free_charge();
        let record = TgeDatablocks::decode(class, r, ctx.table, &mut charge, widths)?;
        Ok(Ev::DataBlock(Some((
            id,
            TgeDatablock {
                class: name.to_string(),
                fields: fields(&record, widths),
            },
        ))))
    }
}

/// The client's side of the memory link: what the session sends waits here for the rig, which loses and reorders it.
#[derive(Default)]
struct HeldLink {
    out: Vec<Vec<u8>>,
}

impl Link for HeldLink {
    fn max_datagram(&self) -> usize {
        LinkKind::MEMORY.max_datagram
    }

    fn send(&mut self, datagram: &[u8]) -> Result<(), NoRoom> {
        self.out.push(datagram.to_vec());
        Ok(())
    }

    fn facts(&self) -> LinkFacts {
        LinkFacts::MEMORY
    }
}

/// The move lane's writer: the control block and the client's events.
struct MoveLane<'a> {
    order: &'a mut PacketOrder,
    out: &'a mut EventQueue<NetEvent>,
    ctx: EventCtx<'a>,
}

impl StreamSource for MoveLane<'_> {
    fn write(&mut self, lane: StreamLane, seq: u16, room: usize, out: &mut Vec<u8>) {
        if lane != StreamLane::Moves || room == 0 {
            return;
        }
        let wide = self.order.sent(seq);
        let packet_bits = room * 8;
        let mut w = BitWriter::with_limit(packet_bits);
        w.enable_string_buffer();
        ClientBlock::default().write(&mut w);
        let params = SectionParams {
            ghosting: false,
            hold_ordered: false,
            packet_bits,
            min_start_bits: w.bits(),
        };
        let written = self
            .out
            .write_section_stalling(&mut w, &self.ctx, wide, params);
        if written.is_ok() && !w.is_overflowed() {
            out.extend_from_slice(&w.into_bytes());
        } else {
            self.order.unsent(seq);
        }
    }

    fn unsent(&mut self, seq: u16) {
        self.order.unsent(seq);
    }
}

/// What the client saw of one removed datablock: the client tick its remove came at, the first tick no ghost named it any more,
/// and the tick the purge dropped it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Removed {
    told: u32,
    unnamed: Option<u32>,
    purged: Option<u32>,
}

/// A client of the engine's framing: a `Session` on the memory link, the server's ghost lane read as a TGE payload into a
/// `ClientWorld` (the ghosts and the datablocks), the removes retired and purged as a client of the engine's does.
struct EngineClient {
    addr: SocketAddr,
    session: Session,
    link: HeldLink,
    order: PacketOrder,
    table: Table,
    ids: EventIds,
    classes: NetClasses,
    widths: NetWidths,
    ghosts: Option<GhostClient<TgeCodec>>,
    world: ClientWorld<TgeClientSet>,
    out: EventQueue<NetEvent>,
    receiver: EventReceiver<Ev>,
    tick: u32,
    packets: usize,
    dones: u32,
    /// The ids whose bodies came, in order.
    bodies: Vec<u32>,
    /// The removes that came, in order.
    removes: Vec<u32>,
    removed: BTreeMap<u32, Removed>,
    ignored: u32,
    /// Ghosts seen naming a datablock the world did not hold, by tick.
    missing: Vec<(u32, u32)>,
    errors: Vec<String>,
}

impl EngineClient {
    fn new(port: u16, nonce: u64) -> EngineClient {
        let events = EventRegistry::engine();
        let profile = HandshakeProfile::default();
        let game = String::from_utf8_lossy(&profile.game).into_owned();
        let connect = connect_of(&profile, None, b"", &[]).expect("a connect");
        let session = Session::client(
            SessionConfig::new(&game),
            LinkFacts::MEMORY,
            connect,
            nonce,
            0,
        )
        .expect("a session");
        EngineClient {
            addr: SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), port),
            session,
            link: HeldLink::default(),
            order: PacketOrder::default(),
            table: Table::torque(),
            ids: events.ids().unwrap(),
            classes: NetClasses::derive(&tge_reps(&events)).unwrap(),
            widths: NetWidths::engine_default(),
            ghosts: None,
            world: ClientWorld::default(),
            out: EventQueue::new(),
            receiver: EventReceiver::new(),
            tick: 0,
            packets: 0,
            dones: 0,
            bodies: Vec::new(),
            removes: Vec::new(),
            removed: BTreeMap::new(),
            ignored: 0,
            missing: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn connected(&self) -> bool {
        self.ghosts.is_some()
    }

    fn ids(&self) -> BTreeSet<u32> {
        self.world.datablocks.keys().copied().collect()
    }

    fn accepted(&mut self, ghost_bits: u8) {
        self.widths = NetWidths::new(ghost_bits, DEFAULT_DATABLOCK_BITS);
        let names = self
            .classes
            .classes(Kind::Object)
            .iter()
            .map(|c| c.name.as_str());
        let codec = TgeCodec::new(names, self.widths, self.table.clone());
        let mut ghosts = GhostClient::new(codec, self.classes.id_bits(Kind::Object), ghost_bits);
        ghosts.enable_log();
        self.ghosts = Some(ghosts);
    }

    /// The session's events: the accept, the ghost lane's packets, the fates of this side's packets.
    fn drain(&mut self) {
        while let Some(event) = self.session.next_event() {
            match event {
                Event::Accepted(accept) => self.accepted(accept.widths.ghost_id),
                Event::Stream {
                    lane: StreamLane::Ghosts,
                    data,
                    ..
                } => {
                    if let Err(e) = self.packet(&data) {
                        self.errors.push(e.to_string());
                    }
                }
                Event::Notify { seq, delivered } => {
                    self.order.resolve(seq, delivered);
                    while let Some((send_seq, delivered)) = self.order.pop() {
                        let mut settled = Vec::new();
                        self.out.on_notify(send_seq, delivered, &mut settled);
                    }
                }
                Event::Closed(why) => self.errors.push(format!("{why:?}")),
                Event::Challenge(_) => {
                    if let Err(e) = self.session.answer(Vec::new()) {
                        self.errors.push(e.to_string());
                    }
                }
                Event::Stream { .. }
                | Event::Message { .. }
                | Event::ConnectRequest(_)
                | Event::Ticket(_) => self.errors.push("an unexpected event".to_string()),
            }
        }
    }

    /// One packet of the server's ghost lane: the head, the events and the ghost section.
    fn packet(&mut self, data: &[u8]) -> Result<(), NetError> {
        let Some(ghosts) = self.ghosts.as_mut() else {
            return Ok(());
        };
        let mut r = reader_at(data, 0);
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
        self.receiver.read_section(
            &mut r,
            &ctx,
            Role::Client,
            &mut Unpacker {
                ghosts,
                classes: &self.classes,
            },
            &mut incoming,
            None,
        )?;
        ghosts.read_section(&mut r)?;
        self.packets += 1;
        for event in incoming {
            match event.event {
                Ev::Message(m) => match m.message {
                    0 => {
                        let _ = self
                            .out
                            .post(NetEvent::ConnectionMessage(ConnectionMessage {
                                seq: m.seq,
                                message: 1,
                                count: 0,
                            }));
                    }
                    DATABLOCKS_DONE => {
                        self.dones += 1;
                        let _ = self
                            .out
                            .post(NetEvent::ConnectionMessage(ConnectionMessage {
                                seq: m.seq,
                                message: DATABLOCKS_DOWNLOAD_DONE,
                                count: 0,
                            }));
                    }
                    _ => {}
                },
                Ev::DataBlock(Some((id, datablock))) => {
                    // A client of the engine's refuses a body for a retired id; this one records it as an error.
                    if self.world.is_datablock_retired(id) {
                        self.errors.push(format!("a body for retired {id}"));
                        continue;
                    }
                    self.bodies.push(id);
                    self.world.apply_datablock(id, datablock);
                }
                Ev::DataBlock(None) | Ev::Always => {}
                Ev::Remove(id) => {
                    self.removes.push(id);
                    match self.world.retire_datablock(id) {
                        DatablockRetire::Retired => {
                            self.removed.insert(
                                id,
                                Removed {
                                    told: self.tick,
                                    ..Removed::default()
                                },
                            );
                        }
                        DatablockRetire::Unknown | DatablockRetire::Already => self.ignored += 1,
                        DatablockRetire::OverCap => self.errors.push("over the cap".to_string()),
                    }
                }
            }
        }
        let changes = ghosts.take_log();
        self.world
            .apply_ghosts(self.packets, &changes, &*ghosts, [0.0; 3], &mut ());
        Ok(())
    }

    /// The tick's end: every ghost checked against the datablocks held, the purge of what nothing names, the packet out.
    fn tick(&mut self, now: u32) {
        self.tick += 1;
        let named: BTreeSet<u32> = self
            .world
            .ghosts
            .values()
            .filter_map(|g| g.datablock())
            .collect();
        for &db in &named {
            // A ghost may come before the body of a datablock added in the same tick (the add's own race, Torque's too);
            // one that names a datablock the client had and let go is the removal's fault.
            if !self.world.datablocks.contains_key(&db) && self.bodies.contains(&db) {
                self.missing.push((self.tick, db));
            }
        }
        for (&id, r) in &mut self.removed {
            if r.unnamed.is_none() && !named.contains(&id) {
                r.unnamed = Some(self.tick);
            }
        }
        self.world.purge_retired_datablocks(|_| false);
        for id in self.world.take_purged_datablocks() {
            if let Some(r) = self.removed.get_mut(&id) {
                r.purged = Some(self.tick);
            }
        }
        let EngineClient {
            session,
            link,
            order,
            out,
            table,
            ids,
            widths,
            ..
        } = self;
        let ctx = EventCtx {
            table,
            ids,
            ghost_bits: widths.ghost_bits,
            datablock_bits: widths.datablock_bits,
        };
        let mut lane = MoveLane { order, out, ctx };
        session.poll(link, &mut lane, now);
        self.drain();
    }

    fn receive(&mut self, data: &[u8], now: u32) {
        self.session.receive(data, now);
        self.drain();
    }
}

// ---- the rig

/// A server and one engine client, stepped by hand: a fifth of the datagrams lost, the rest reordered within a tick and some
/// held back a tick.
struct Rig {
    s: NetServer,
    shared: Shared,
    end: MemoryEnd,
    client: EngineClient,
    rng: Rng,
    loss: u64,
    to_client: VecDeque<Vec<u8>>,
    to_server: VecDeque<Vec<u8>>,
}

impl Rig {
    fn new(seed: u64, loss: u64) -> Rig {
        let (s, shared, end) = game_server(ProtocolKind::Engine);
        let mut rig = Rig {
            s,
            shared,
            end,
            client: EngineClient::new(50_100, seed | 1),
            rng: Rng::new(seed),
            loss,
            to_client: VecDeque::new(),
            to_server: VecDeque::new(),
        };
        for _ in 0..200 {
            rig.step();
            if rig.client.dones > 0 && rig.s.vm.global("$done").is_some() {
                break;
            }
        }
        assert!(rig.client.connected(), "{:?}", rig.client.errors);
        assert_eq!(rig.client.dones, 1, "{:?}", rig.client.errors);
        assert_eq!(
            rig.client.ids(),
            server_ids(&rig.s),
            "the mission chain's datablocks"
        );
        rig
    }

    /// Loses, reorders and holds back `fresh`, then delivers what is due.
    fn shuffle(&mut self, queue: &mut VecDeque<Vec<u8>>, fresh: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
        for datagram in fresh {
            if !self.rng.chance(self.loss) {
                queue.push_back(datagram);
            }
        }
        let n = queue.len();
        for i in 0..n {
            let j = self.rng.below(n);
            queue.swap(i, j);
        }
        let mut due = Vec::new();
        for _ in 0..n {
            let Some(d) = queue.pop_front() else { break };
            if self.loss > 0 && self.rng.chance(20) {
                queue.push_back(d);
            } else {
                due.push(d);
            }
        }
        due
    }

    fn step(&mut self) {
        self.s.run(TICK_MS, |_| false);
        let now = self.s.vm.sim_time();
        let fresh: Vec<Vec<u8>> = self
            .end
            .take_sent()
            .into_iter()
            .filter(|(to, _)| *to == self.client.addr)
            .map(|(_, d)| d)
            .collect();
        let mut queue = std::mem::take(&mut self.to_client);
        for d in self.shuffle(&mut queue, fresh) {
            self.client.receive(&d, now);
        }
        self.to_client = queue;
        self.client.tick(now);
        let fresh = std::mem::take(&mut self.client.link.out);
        let mut queue = std::mem::take(&mut self.to_server);
        for d in self.shuffle(&mut queue, fresh) {
            self.end.inject(self.client.addr, &d);
        }
        self.to_server = queue;
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

    /// A live round, run to the server's `onDataBlocksUpdated`.
    fn round(&mut self) {
        let want = self
            .s
            .vm
            .global("$updated")
            .map_or(1, |v| torque_ts_vm::text::atoi(&v) as usize + 1);
        assert_eq!(answer(&mut self.s, "return updateDataBlocks();"), "1");
        assert!(
            self.run_until(600, |r| {
                r.s.vm
                    .global("$updated")
                    .is_some_and(|v| torque_ts_vm::text::atoi(&v) as usize == want)
            }),
            "the round ends"
        );
    }

    fn ghosted_shapes(&self) -> usize {
        self.client
            .world
            .ghosts
            .values()
            .filter(|g| g.class == Some(torque_ghosts::TgeClass::StaticShape))
            .count()
    }
}

/// What a round does with the datablock under its shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
    /// `removeDataBlock(%db, %fallback)`: the shapes move to the fallback, the datablock goes at once.
    Repoint,
    /// `%db.delete()` over the shapes (a retire), then the shapes deleted: freed by the drain.
    DeleteThenUsers,
    /// The shapes deleted first, then `removeDataBlock(%db)`.
    UsersThenRemove,
    /// `removeDataBlock(%db, "deleteUsers")`.
    DeleteUsers,
}

const ORDERS: [Order; 4] = [
    Order::Repoint,
    Order::DeleteThenUsers,
    Order::UsersThenRemove,
    Order::DeleteUsers,
];

#[test]
fn over_a_hundred_seeded_orders_the_clients_set_equals_the_servers_and_no_ghost_names_a_missing_datablock()
 {
    let mut rig = Rig::new(7, 20);
    let mut rng = Rng::new(11);
    let mut removed_ids: Vec<u32> = Vec::new();
    for round in 0..100usize {
        let order = ORDERS[rng.below(ORDERS.len())];
        let shapes = 1 + rng.below(3);
        let mut code = format!(
            "datablock StaticShapeData(Gone{round}) {{ shapeFile = \"a.dts\"; }}; datablock StaticShapeData(Kept{round}) {{ shapeFile = \"b.dts\"; }};"
        );
        for i in 0..shapes {
            code.push_str(&format!(
                "new StaticShape(S{round}_{i}) {{ dataBlock = Gone{round}; position = \"{i} {round} 0\"; }};"
            ));
        }
        eval(&mut rig.s, &code);
        let gone = id_of(&rig.s, &format!("Gone{round}"));
        rig.round();
        assert!(
            rig.run_until(200, |r| r.ghosted_shapes() == shapes
                && r.client.ids() == server_ids(&r.s)),
            "round {round}: the shapes and datablocks reach the client ({} shapes, {:?})",
            rig.ghosted_shapes(),
            rig.client.errors
        );
        let delete_shapes = |rig: &mut Rig| {
            let code: String = (0..shapes)
                .map(|i| format!("S{round}_{i}.delete();"))
                .collect();
            eval(&mut rig.s, &code);
        };
        match order {
            Order::Repoint => {
                assert_eq!(
                    answer(
                        &mut rig.s,
                        &format!("return removeDataBlock(Gone{round}, Kept{round});")
                    ),
                    "1"
                );
            }
            Order::DeleteThenUsers => {
                eval(&mut rig.s, &format!("Gone{round}.delete();"));
                assert!(rig.s.vm.objects().is_retiring(gone));
                rig.run(1 + rng.below(4));
                delete_shapes(&mut rig);
            }
            Order::UsersThenRemove => {
                delete_shapes(&mut rig);
                rig.run(1 + rng.below(4));
                assert_eq!(
                    answer(&mut rig.s, &format!("return removeDataBlock(Gone{round});")),
                    "1"
                );
            }
            Order::DeleteUsers => {
                assert_eq!(
                    answer(
                        &mut rig.s,
                        &format!("return removeDataBlock(Gone{round}, \"deleteUsers\");")
                    ),
                    "1"
                );
            }
        }
        removed_ids.push(gone);
        assert!(
            rig.run_until(300, |r| !r.s.vm.objects().contains(gone)
                && r.client.ids() == server_ids(&r.s)),
            "round {round} ({order:?}): the client's set follows the server's: {:?} vs {:?}; {:?}",
            rig.client.ids(),
            server_ids(&rig.s),
            rig.client.errors
        );
        if order != Order::Repoint {
            // The shapes are gone from the client too before the next round's shapes come.
            assert!(
                rig.run_until(100, |r| r.ghosted_shapes() == 0),
                "round {round}: the shapes leave"
            );
        }
        // Delete the kept shapes too, so the next round starts clean.
        if order == Order::Repoint {
            delete_shapes(&mut rig);
            assert!(rig.run_until(100, |r| r.ghosted_shapes() == 0));
        }
    }
    let client = &rig.client;
    assert!(client.errors.is_empty(), "{:?}", client.errors);
    assert_eq!(
        client.missing,
        Vec::<(u32, u32)>::new(),
        "a ghost drawn with a missing datablock"
    );
    assert_eq!(
        client.ignored, 0,
        "every remove named a datablock the client held"
    );
    for id in &removed_ids {
        assert_eq!(
            client.removes.iter().filter(|r| *r == id).count(),
            1,
            "{id}: told once"
        );
        let body = client
            .bodies
            .iter()
            .position(|b| b == id)
            .expect("a body first");
        let last_body = client.bodies.iter().rposition(|b| b == id).unwrap();
        assert!(body <= last_body);
        let r = client.removed[id];
        let purged = r.purged.expect("purged");
        let freed = r.told.max(r.unnamed.unwrap_or(r.told));
        assert!(
            purged <= freed + 2,
            "{id}: purged at {purged}, told at {} and unnamed at {:?}",
            r.told,
            r.unnamed
        );
    }
    assert_eq!(client.ids(), server_ids(&rig.s));
}

#[test]
fn a_remove_of_an_unknown_id_is_ignored_and_counted_and_one_in_use_stays_retired_until_the_ghost_leaves()
 {
    let mut rig = Rig::new(3, 0);
    eval(
        &mut rig.s,
        "datablock StaticShapeData(Used) { shapeFile = \"a.dts\"; }; new StaticShape(U) { dataBlock = Used; position = \"1 1 0\"; };",
    );
    let used = id_of(&rig.s, "Used");
    rig.round();
    assert!(rig.run_until(100, |r| r.ghosted_shapes() == 1
        && r.client.ids().contains(&used)));
    let conn = {
        let addr = rig.client.addr;
        let shared = Rc::clone(&rig.shared);
        let obj = connection_of(&shared, addr);
        shared.borrow().conns[&obj].conn.expect("accepted")
    };
    // A removal of an id the client never had, and one of the datablock its ghost names.
    let stranger = used + 500;
    rig.shared
        .borrow()
        .events
        .post_datablock_removes(conn, &[stranger, used])
        .unwrap();
    assert!(rig.run_until(50, |r| r.client.removes.len() == 2));
    assert_eq!(rig.client.ignored, 1, "the unknown id");
    assert!(rig.client.world.is_datablock_retired(used));
    rig.run(10);
    assert!(
        rig.client.world.datablocks.contains_key(&used),
        "kept while the ghost names it"
    );
    assert!(rig.client.missing.is_empty());
    // The ghost leaves: purged within two ticks.
    eval(&mut rig.s, "U.delete();");
    assert!(rig.run_until(100, |r| r.ghosted_shapes() == 0));
    let gone_at = rig.client.tick;
    assert!(rig.run_until(3, |r| !r.client.world.datablocks.contains_key(&used)));
    let r = rig.client.removed[&used];
    assert!(
        r.purged.is_some_and(|p| p <= gone_at + 2),
        "{r:?} vs {gone_at}"
    );
    assert!(rig.client.missing.is_empty());
    assert!(rig.client.errors.is_empty(), "{:?}", rig.client.errors);
}

#[test]
fn a_forced_remove_post_under_torques_event_table_posts_nothing() {
    let (mut s, shared, end) = game_server(ProtocolKind::Tge);
    let mut client = net_client::TestClient::new(50_111, 7, Default::default());
    client.start(&end);
    net_client::run(&mut s, &end, &mut [&mut client], 30);
    assert!(client.connected());
    eval(
        &mut s,
        "datablock StaticShapeData(Lone) { shapeFile = \"a.dts\"; };",
    );
    assert_eq!(answer(&mut s, "return updateDataBlocks();"), "1");
    net_client::run(&mut s, &end, &mut [&mut client], 30);
    let obj = connection_of(&shared, client.addr);
    let conn = shared.borrow().conns[&obj].conn.expect("accepted");
    let lone = id_of(&s, "Lone");
    let before = shared.borrow().events.queued(conn);
    // Torque's table has no class for the event: a packed one would corrupt the client's event stream.
    assert_eq!(
        shared
            .borrow()
            .events
            .post_datablock_removes(conn, &[lone, lone + 500]),
        Err(RemovePostError::NoClass)
    );
    assert_eq!(
        shared.borrow().events.queued(conn),
        before,
        "nothing posted"
    );
    net_client::run(&mut s, &end, &mut [&mut client], 30);
    assert!(client.connected(), "the client's stream is intact");
}

#[test]
fn a_server_of_torques_protocol_posts_nothing_on_a_removal_and_keeps_torques_event_table() {
    let (mut s, shared, end) = game_server(ProtocolKind::Tge);
    let ids = shared.borrow().events.ids().expect("ids");
    assert_eq!(ids.count(), 13);
    assert_eq!(ids.datablock_remove(), None);
    let mut client = net_client::TestClient::new(50_110, 7, Default::default());
    client.start(&end);
    net_client::run(&mut s, &end, &mut [&mut client], 30);
    assert!(client.connected());
    eval(
        &mut s,
        "datablock StaticShapeData(Lone) { shapeFile = \"a.dts\"; };",
    );
    assert_eq!(answer(&mut s, "return updateDataBlocks();"), "1");
    net_client::run(&mut s, &end, &mut [&mut client], 30);
    let obj = connection_of(&shared, client.addr);
    let conn = shared.borrow().conns[&obj].conn.expect("accepted");
    let lone = id_of(&s, "Lone");
    assert!(
        shared
            .borrow()
            .events
            .datablock_sent_key(conn, lone)
            .is_some()
    );
    let before = shared.borrow().events.queued(conn);
    assert_eq!(answer(&mut s, "return removeDataBlock(Lone);"), "1");
    assert_eq!(
        shared.borrow().events.queued(conn),
        before,
        "nothing posted"
    );
    assert!(
        shared
            .borrow()
            .events
            .datablock_sent_key(conn, lone)
            .is_some(),
        "the table is left alone"
    );
    // In the engine's protocol the same removal posts one event to the connection that was sent the datablock.
    let (mut s, shared, _end) = game_server(ProtocolKind::Engine);
    let ids = shared.borrow().events.ids().expect("ids");
    assert_eq!(ids.count(), 14);
    assert_eq!(ids.bits(), 4, "the class id keeps its width");
    assert!(ids.datablock_remove().is_some());
    eval(
        &mut s,
        "datablock StaticShapeData(Lone) { shapeFile = \"a.dts\"; };",
    );
    assert_eq!(
        shared.borrow().events.remove_datablock(id_of(&s, "Lone")),
        0,
        "no connection had it"
    );
}

/// A client of the engine's protocol reads the particle fields TGE does not have (a Torque-protocol client is sent none:
/// `datablock_live`'s `a_client_of_torques_protocol_is_sent_particle_datablocks_without_the_engines_fields`).
#[test]
fn the_particle_fields_reach_a_client_of_the_engines_protocol() {
    let fields_of = |world: &ClientWorld<TgeClientSet>, id: ObjId| -> BTreeMap<String, String> {
        world.datablocks[&id]
            .fields
            .iter()
            .map(|(n, i, v)| (format!("{n}[{i}]"), format!("{v:?}")))
            .collect()
    };
    let mut rig = Rig::new(5, 0);
    eval(&mut rig.s, crate::datablock_live::SPARKS);
    rig.round();
    let (sparks, plain, emitter) = (
        id_of(&rig.s, "Sparks"),
        id_of(&rig.s, "Plain"),
        id_of(&rig.s, "SparkEmitter"),
    );
    assert!(rig.run_until(100, |r| r.client.ids().contains(&emitter)));
    let sent = fields_of(&rig.client.world, sparks);
    for (field, want) in [
        ("turbulence[0]", "F32(4.0)"),
        ("turbulenceScale[0]", "F32(2.5)"),
        ("collide[0]", "Bool(true)"),
        ("bounceElasticity[0]", "F32(0.6)"),
        ("bounceFriction[0]", "F32(0.1)"),
        ("lit[0]", "Int(2)"),
        ("softDistance[0]", "F32(1.5)"),
        ("flipbookBlend[0]", "Bool(true)"),
        ("sizeByVelocity[0]", "F32(0.05)"),
    ] {
        assert_eq!(sent.get(field).map(String::as_str), Some(want), "{field}");
    }
    let sent = fields_of(&rig.client.world, emitter);
    assert_eq!(sent.get("trail[0]").map(String::as_str), Some("Bool(true)"));
    assert_eq!(sent.get("budget[0]").map(String::as_str), Some("Int(300)"));
    // A datablock with every field at its default sends the flag alone: no value of the block comes back.
    let quiet = fields_of(&rig.client.world, plain);
    assert!(
        quiet.keys().all(|k| !k.starts_with("turbulence")),
        "{quiet:?}"
    );
    assert!(rig.client.errors.is_empty(), "{:?}", rig.client.errors);
}
