//! Bulk channels through a real server: two clients join a Torque net game on the memory transport, the channels are enabled for
//! them, and a blob of megabytes goes to each in chunks while the server's ghost traffic goes on. The test client reads the channel
//! section at the front of each data packet with [`ClientChannels`] (the same reader the engine's client uses) and acknowledges
//! the packets it hears.

use std::cell::RefCell;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::rc::Rc;

use torque_game::scene::container::{Container, ContainerConfig};
use torque_net::bits::BitReader;
use torque_net::chunk::patch;
use torque_net::chunk::upstream::UpstreamCaps;
use torque_net::chunk::{ChannelLimits, Kind, SendError};
use torque_net::client::{Client, ClientEvent, ClientState};
use torque_net::control::ClientBlock;
use torque_net::events::{
    ConnectionMessage, EventCtx, EventIds, EventQueue, EventRegistry, NetEvent, SectionParams,
};
use torque_net::handshake::{ConnId, ConnectFields};
use torque_net::huffman::Table;
use torque_net::profile::DEFAULT_DATABLOCK_BITS;
use torque_net::protocol::{RateConfig, reader_at};
use torque_scene::BinGeometry;
use torque_server::net::channels::{
    ChannelEntry, ChannelHub, ChannelRegistry, ChunkChannel, ClientChannels, install,
};
use torque_server::net::ghosts::SourceEnv;
use torque_server::net::hostevent::HostEvent;
use torque_server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque_server::net::transport::{MemoryEnd, MemoryTransport, SharedTransport};
use torque_server::net::{Host, HostParts, NetClock, NetShared, attach_host};
use torque_server::server::{Server, TICK_MS};
use torque_server::state::ServerState;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

type NetServer = Server<HostParts<TorqueNet>>;

const SERVER_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 28_000);

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        ((self.next() >> 32) as usize) % n.max(1)
    }
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

/// What the server's channel logic holds: each client's own snapshot (the scoping), made on demand.
#[derive(Clone, Default)]
struct World {
    blobs: Rc<RefCell<HashMap<ConnId, Vec<u8>>>>,
}

struct ServerLogic(World);

impl ChunkChannel for ServerLogic {
    fn snapshot(&mut self, client: ConnId) -> Option<Vec<u8>> {
        self.0.blobs.borrow().get(&client).cloned()
    }
}

/// What a client's channel logic did: the blob it holds, and each message's kind and size.
#[derive(Clone, Default)]
struct View {
    blob: Rc<RefCell<Vec<u8>>>,
    log: Rc<RefCell<Vec<(Kind, usize)>>>,
    errors: Rc<RefCell<usize>>,
}

struct ClientLogic(View);

impl ChunkChannel for ClientLogic {
    fn receive(&mut self, kind: Kind, data: &[u8]) {
        self.0.log.borrow_mut().push((kind, data.len()));
        let mut blob = self.0.blob.borrow_mut();
        match kind {
            Kind::Snapshot => *blob = data.to_vec(),
            Kind::Delta => {
                if patch::apply(&mut blob, data, 8 << 20).is_err() {
                    *self.0.errors.borrow_mut() += 1;
                }
            }
        }
    }
}

type Upload = (ConnId, Kind, Vec<u8>);

/// What the server's logic of the client-to-server channel got: each message with the connection it came from, and the
/// connections it was told ended.
#[derive(Clone, Default)]
struct UpLog {
    got: Rc<RefCell<Vec<Upload>>>,
    gone: Rc<RefCell<Vec<ConnId>>>,
}

struct UpLogic(UpLog);

impl ChunkChannel for UpLogic {
    fn receive_from(&mut self, from: ConnId, kind: Kind, data: &[u8]) {
        self.0.got.borrow_mut().push((from, kind, data.to_vec()));
    }

    fn disconnected(&mut self, conn: ConnId) {
        self.0.gone.borrow_mut().push(conn);
    }
}

const BUDGET: usize = 480;

fn limits() -> ChannelLimits {
    ChannelLimits::new(BUDGET).with_max_message(4 << 20)
}

/// A channel that has nothing to say.
struct Quiet;

impl ChunkChannel for Quiet {}

fn registry_with<C: ChunkChannel>(make: impl Fn() -> C + 'static) -> ChannelRegistry {
    registry_up(make, None)
}

/// [`registry_with`] and, when `up` is given, a client-to-server channel "uploads" whose server logic records into it.
fn registry_up<C: ChunkChannel>(
    make: impl Fn() -> C + 'static,
    up: Option<UpLog>,
) -> ChannelRegistry {
    let mut entries = vec![
        ChannelEntry::new("terrain", limits(), make),
        // A second channel that stays quiet: ids come from the sorted names, so "terrain" is channel 1.
        ChannelEntry::new("aaa-unused", ChannelLimits::new(64), || Quiet),
    ];
    if let Some(log) = up {
        entries.push(ChannelEntry::to_server("uploads", limits(), move || {
            UpLogic(log.clone())
        }));
    }
    ChannelRegistry::new(&entries).unwrap()
}

fn net_server() -> (NetServer, Rc<RefCell<NetShared<TorqueNet>>>, MemoryEnd) {
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
    vm.set_file_host(MemFiles::new());
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

/// One test client: its address, its protocol half, its channel reader and what its logic saw.
struct Peer {
    addr: SocketAddr,
    client: Client,
    channels: ClientChannels,
    view: View,
    conn: ConnId,
    /// Bytes of the datagrams the server sent it, and the most bits one packet's channel section took.
    heard_bytes: usize,
    widest_section: usize,
    packets: usize,
    queue: EventQueue<NetEvent>,
}

struct Link {
    loss: usize,
    reorder: bool,
    rng: Rng,
    /// The percent of the clients packets the link loses on the way to the server.
    up_loss: usize,
}

impl Link {
    fn clean() -> Link {
        Link {
            loss: 0,
            reorder: false,
            rng: Rng(1),
            up_loss: 0,
        }
    }
}

struct Rig {
    s: NetServer,
    shared: Rc<RefCell<NetShared<TorqueNet>>>,
    end: MemoryEnd,
    peers: Vec<Peer>,
    hub: ChannelHub,
    world: World,
    up: UpLog,
    table: Table,
    sun: torque_ts_vm::sim::ObjId,
    link: Link,
    ids: EventIds,
}

fn lines<N: torque_server::state::NetParts>(server: &Server<N>) -> Vec<String> {
    server
        .vm
        .log()
        .iter()
        .map(|l| String::from_utf8_lossy(&l.text).into_owned())
        .collect()
}

/// A server with channels registered (not enabled) and `clients` joined, ghosting the Sun to each.
fn rig(clients: usize) -> Rig {
    rig_with(clients, Some(1_000))
}

fn rig_with(clients: usize, packet_bytes: Option<u32>) -> Rig {
    rig_up(clients, packet_bytes, false)
}

/// [`rig_with`], and with `upstream` the engine also has the client-to-server channel "uploads".
fn rig_up(clients: usize, packet_bytes: Option<u32>, upstream: bool) -> Rig {
    let (mut s, shared, end) = net_server();
    s.vm.eval(b"new SimGroup(ClientGroup); $pref::Net::PacketRateToClient = 32;")
        .unwrap();
    let world = World::default();
    let up = UpLog::default();
    let hub = install(
        &shared,
        registry_up(
            {
                let world = world.clone();
                move || ServerLogic(world.clone())
            },
            upstream.then(|| up.clone()),
        ),
    )
    .expect("channels are registered");
    let table = Table::torque();
    let mut peers = Vec::new();
    for i in 0..clients {
        let addr = SocketAddr::from(([127, 0, 0, 1], 50_000 + i as u16));
        let mut client = Client::new(7 + i as u32, ConnectFields::standard(b"", Vec::new()))
            .with_rates(RateConfig {
                to_server_rate: 32,
                to_client_rate: 32,
                packet_size: 1_000,
            });
        end.inject(addr, &client.challenge_request());
        for _ in 0..12 {
            s.run(TICK_MS, |_| false);
            let now = s.vm.sim_time();
            for (to, datagram) in end.take_sent() {
                if to == addr
                    && let ClientEvent::Send(bytes) = client.on_datagram(&table, &datagram, now)
                {
                    end.inject(addr, &bytes);
                }
            }
            if client.state() == &ClientState::Connected {
                break;
            }
        }
        assert_eq!(client.state(), &ClientState::Connected, "{:?}", lines(&s));
        let conn = {
            let sh = shared.borrow();
            let obj = sh.conns.values().find(|r| r.addr == addr).unwrap().obj;
            sh.conns[&obj].conn.expect("accepted")
        };
        // The engine's own clients get fuller packets than TGE's 200 bytes (the handshake extension's packet size).
        shared
            .borrow_mut()
            .hs
            .connection_mut(conn)
            .expect("the connection")
            .protocol
            .set_packet_bytes(packet_bytes);
        if upstream {
            // The same fuller packets in the other direction: a client packet of the default 200 bytes holds few chunks.
            client
                .protocol_mut()
                .expect("connected")
                .set_packet_bytes(packet_bytes);
        }
        let view = View::default();
        let channels = ClientChannels::new(&registry_up(
            {
                let view = view.clone();
                move || ClientLogic(view.clone())
            },
            upstream.then(|| up.clone()),
        ));
        // What the engine client's connection does when its accept says the server writes channels.
        channels.set_active(true);
        peers.push(Peer {
            addr,
            client,
            channels,
            view,
            conn,
            heard_bytes: 0,
            widest_section: 0,
            packets: 0,
            queue: EventQueue::new(),
        });
    }
    // The Sun is ScopeAlways: ghosting each client sends it at once, and a mask bit set each tick is a ghost update each tick.
    s.vm.eval(b"new Sun(TheSun) { azimuth = 30; elevation = 45; };")
        .unwrap();
    let sun = s.vm.objects().find(b"TheSun").unwrap();
    let ghosts = shared.borrow().ghosts.clone().unwrap();
    ghosts.added(&mut s.vm, sun);
    let layer = shared.borrow().events.clone();
    for p in &peers {
        let env = SourceEnv {
            vm: &s.vm,
            table: Some(&table),
            ghost_bits: ghosts.conn_ghost_bits(p.conn),
        };
        let events = ghosts.activate(env, p.conn).expect("ghosting starts");
        layer
            .post_events(p.conn, events.into_iter().map(HostEvent::Ghost).collect())
            .expect("queued");
        // The client's "ready for the rest" (its first ghosting message), which starts the ghost records.
        ghosts
            .on_message(
                p.conn,
                &ConnectionMessage {
                    seq: 1,
                    message: 1,
                    count: 0,
                },
            )
            .expect("ready");
    }
    Rig {
        s,
        shared,
        end,
        peers,
        hub,
        world,
        up,
        table,
        sun,
        link: Link::clean(),
        ids: EventRegistry::torque().ids().expect("ids"),
    }
}

impl Rig {
    fn records(&self) -> u64 {
        let hub = self.shared.borrow().ghosts.clone().unwrap();
        self.peers
            .iter()
            .map(|p| hub.totals(p.conn).map_or(0, |t| t.records))
            .sum()
    }

    /// One tick: the sun changes, the server runs, each client hears what the link lets through and acknowledges it.
    fn tick(&mut self) {
        let hub = self.shared.borrow().ghosts.clone().unwrap();
        hub.set_mask(self.sun, 1);
        self.s.run(TICK_MS, |_| false);
        let now = self.s.vm.sim_time();
        let mut by_peer: HashMap<SocketAddr, Vec<Vec<u8>>> = HashMap::new();
        for (to, datagram) in self.end.take_sent() {
            by_peer.entry(to).or_default().push(datagram);
        }
        for p in &mut self.peers {
            let mut datagrams = by_peer.remove(&p.addr).unwrap_or_default();
            datagrams.retain(|_| !self.link.rng.chance(self.link.loss));
            if self.link.reorder && datagrams.len() > 1 {
                datagrams.reverse();
            }
            let mut heard = false;
            for datagram in datagrams {
                p.heard_bytes += datagram.len();
                let ClientEvent::Game {
                    received,
                    payload_bit,
                } = p.client.on_datagram(&self.table, &datagram, now)
                else {
                    continue;
                };
                for n in &received.notifies {
                    p.channels.on_notify(n.send_seq, n.delivered);
                }
                if !received.payload {
                    continue;
                }
                heard = true;
                p.packets += 1;
                let mut r: BitReader<'_> = reader_at(&datagram, payload_bit);
                let before = r.position();
                p.channels.read(&mut r).expect("the server's section reads");
                p.widest_section = p.widest_section.max(r.position() - before);
            }
            // The client's own packet each tick (32 a second, like a real client): its rates, and the acknowledgements.
            if let Some(mut w) = p.client.begin_data_packet(now, false) {
                w.enable_string_buffer();
                let protocol = p.client.protocol().expect("connected");
                // What the engine client does: the upstream chunks first, in the room the moves leave.
                if p.channels.writes_upstream() {
                    let mut moves = torque_net::bits::BitWriter::new();
                    ClientBlock::default().write(&mut moves);
                    p.channels.write_upstream(
                        &mut w,
                        protocol.last_send_seq(),
                        protocol.packet_limit_bits(),
                        moves.bits(),
                    );
                }
                ClientBlock::default().write(&mut w);
                let params = SectionParams {
                    ghosting: false,
                    hold_ordered: false,
                    packet_bits: protocol.packet_limit_bits(),
                    min_start_bits: w.bits(),
                };
                let send_seq = protocol.last_send_seq();
                let ctx = EventCtx {
                    table: &self.table,
                    ids: &self.ids,
                    ghost_bits: 14,
                    datablock_bits: DEFAULT_DATABLOCK_BITS,
                };
                let _ = p
                    .queue
                    .write_section_stalling(&mut w, &ctx, send_seq, params);
                if !self.link.rng.chance(self.link.up_loss) {
                    self.end.inject(p.addr, &w.into_bytes());
                }
            } else if heard && let Some(proto) = p.client.protocol_mut() {
                self.end.inject(p.addr, &proto.ack_packet(now));
            }
        }
    }

    fn enable_all(&mut self) {
        for p in &self.peers {
            self.hub.enable(p.conn);
        }
    }

    fn run_until(&mut self, max: usize, mut done: impl FnMut(&Rig) -> bool) -> usize {
        for n in 0..max {
            if done(self) {
                return n;
            }
            self.tick();
        }
        panic!("not done in {max} ticks");
    }
}

fn blob(seed: u64, n: usize) -> Vec<u8> {
    let mut rng = Rng(seed);
    (0..n).map(|_| rng.next() as u8).collect()
}

#[test]
fn a_two_megabyte_blob_reaches_two_clients_within_the_budget_while_ghosts_keep_updating() {
    let mut rig = rig(2);
    // Each client gets its own snapshot (the scoping): the first the whole blob, the second another one, smaller.
    let (a, b) = (blob(1, 2 << 20), blob(2, 1 << 20));
    rig.world
        .blobs
        .borrow_mut()
        .insert(rig.peers[0].conn, a.clone());
    rig.world
        .blobs
        .borrow_mut()
        .insert(rig.peers[1].conn, b.clone());

    // The ghost rate with the channels quiet: records written over 150 ticks.
    for _ in 0..30 {
        rig.tick();
    }
    let r0 = rig.records();
    for _ in 0..150 {
        rig.tick();
    }
    let quiet = rig.records() - r0;
    eprintln!(
        "quiet: {quiet} records, packets {:?}, bytes {:?}",
        rig.peers.iter().map(|p| p.packets).collect::<Vec<_>>(),
        rig.peers.iter().map(|p| p.heard_bytes).collect::<Vec<_>>()
    );
    assert!(quiet >= 250, "two clients, a record a tick each: {quiet}");

    rig.enable_all();
    let r1 = rig.records();
    let ticks = rig.run_until(60_000, |r| {
        r.peers
            .iter()
            .all(|p| !p.view.log.borrow().is_empty() && r.hub.is_idle(p.conn))
    });
    let busy = rig.records() - r1;
    let ghost_rate_quiet = quiet as f64 / 150.0;
    let ghost_rate_busy = busy as f64 / ticks as f64;
    eprintln!(
        "2 MiB + 1 MiB in {ticks} ticks ({} s of game time); ghost records per tick: {ghost_rate_quiet:.2} quiet, {ghost_rate_busy:.2} busy; \
         widest channel section {:?} bits; {:?} packets",
        ticks * 32 / 1000,
        rig.peers
            .iter()
            .map(|p| p.widest_section)
            .collect::<Vec<_>>(),
        rig.peers.iter().map(|p| p.packets).collect::<Vec<_>>(),
    );
    assert_eq!(
        *rig.peers[0].view.blob.borrow(),
        a,
        "the first client's snapshot"
    );
    assert_eq!(
        *rig.peers[1].view.blob.borrow(),
        b,
        "the second client's snapshot"
    );
    assert_eq!(
        rig.peers[0].view.log.borrow().as_slice(),
        &[(Kind::Snapshot, 2 << 20)]
    );
    for p in &rig.peers {
        assert!(
            p.widest_section <= BUDGET * 8 + 1,
            "a packet's channel bytes stay under the budget: {} bits",
            p.widest_section
        );
        assert_eq!(p.channels.held_bytes(), 0);
    }
    // The ghosts were not starved: as many updates a tick as with the channels quiet.
    assert!(
        ghost_rate_busy >= ghost_rate_quiet * 0.95,
        "ghost updates per tick fell from {ghost_rate_quiet:.2} to {ghost_rate_busy:.2}"
    );
}

#[test]
fn a_delta_after_the_snapshot_sends_only_the_change_and_each_client_gets_what_it_is_scoped_to() {
    let mut rig = rig(2);
    let base = blob(3, 300_000);
    for p in &rig.peers {
        rig.world.blobs.borrow_mut().insert(p.conn, base.clone());
    }
    rig.enable_all();
    rig.run_until(5_000, |r| {
        r.peers.iter().all(|p| *p.view.blob.borrow() == base)
    });

    // The blob changes in 1 KiB; only the second client is in scope of the change.
    let mut changed = base.clone();
    changed[100_000..101_024].fill(0xee);
    let delta = patch::diff(&base, &changed).unwrap();
    assert!(delta.len() < 1_100, "a small delta: {} bytes", delta.len());
    let second = rig.peers[1].conn;
    let refused = rig.hub.send_each("terrain", Kind::Delta, |c| {
        (c == second).then(|| delta.clone())
    });
    assert!(refused.is_empty());
    rig.run_until(2_000, |r| *r.peers[1].view.blob.borrow() == changed);
    for _ in 0..50 {
        rig.tick();
    }
    assert_eq!(
        *rig.peers[0].view.blob.borrow(),
        base,
        "out of scope: unchanged"
    );
    assert_eq!(
        rig.peers[0].view.log.borrow().as_slice(),
        &[(Kind::Snapshot, 300_000)]
    );
    assert_eq!(
        rig.peers[1].view.log.borrow().as_slice(),
        &[(Kind::Snapshot, 300_000), (Kind::Delta, delta.len())]
    );
    assert_eq!(*rig.peers[1].view.errors.borrow(), 0);

    // A broadcast reaches both, in order after the earlier messages.
    let mut later = changed.clone();
    later[0] = 1;
    let d2 = patch::diff(&changed, &later).unwrap();
    rig.peers[0].view.blob.borrow_mut().clone_from(&changed);
    assert!(rig.hub.broadcast("terrain", Kind::Delta, &d2).is_empty());
    rig.run_until(2_000, |r| {
        r.peers.iter().all(|p| p.view.blob.borrow()[0] == 1)
    });
}

#[test]
fn a_lossy_reordering_link_still_delivers_the_blob_and_the_delta_in_order() {
    let mut rig = rig(2);
    rig.link = Link {
        loss: 20,
        reorder: true,
        rng: Rng(77),
        up_loss: 0,
    };
    let base = blob(5, 400_000);
    for p in &rig.peers {
        rig.world.blobs.borrow_mut().insert(p.conn, base.clone());
    }
    rig.enable_all();
    let mut changed = base.clone();
    changed[7..70].fill(9);
    let delta = patch::diff(&base, &changed).unwrap();
    // The delta is queued right behind the snapshot: it must apply to the finished snapshot, not before.
    assert!(rig.hub.broadcast("terrain", Kind::Delta, &delta).is_empty());
    let ticks = rig.run_until(60_000, |r| {
        r.peers.iter().all(|p| *p.view.blob.borrow() == changed)
    });
    eprintln!("400 KB and a delta over a link losing 20% and reordering: {ticks} ticks");
    for p in &rig.peers {
        assert_eq!(
            p.view.log.borrow().as_slice(),
            &[(Kind::Snapshot, 400_000), (Kind::Delta, delta.len())]
        );
        assert_eq!(*p.view.errors.borrow(), 0);
    }
}

#[test]
fn a_message_over_the_cap_and_a_backlog_over_its_cap_are_refused_cleanly() {
    let mut rig = rig(1);
    rig.enable_all();
    let conn = rig.peers[0].conn;
    assert_eq!(
        rig.hub
            .send_to(conn, "terrain", Kind::Snapshot, vec![0; (4 << 20) + 1]),
        Err(SendError::TooLarge {
            len: (4 << 20) + 1,
            cap: 4 << 20
        })
    );
    assert_eq!(
        rig.hub.send_to(conn, "terrain", Kind::Delta, vec![]),
        Err(SendError::Empty)
    );
    assert_eq!(
        rig.hub.send_to(conn, "nope", Kind::Delta, vec![1]),
        Err(SendError::UnknownChannel)
    );
    // The backlog cap is 8 MiB: two full messages fit, a third does not.
    for _ in 0..2 {
        rig.hub
            .send_to(conn, "terrain", Kind::Delta, vec![1; 4 << 20])
            .unwrap();
    }
    assert_eq!(
        rig.hub.send_to(conn, "terrain", Kind::Delta, vec![1; 100]),
        Err(SendError::Backlog)
    );
    assert_eq!(rig.hub.backlog(conn, "terrain"), 8 << 20);
    // Refused sends queued nothing, and the connection is untouched: it still ghosts and acknowledges.
    let r = rig.records();
    for _ in 0..30 {
        rig.tick();
    }
    assert!(rig.records() > r);
    assert_eq!(rig.peers[0].client.state(), &ClientState::Connected);
    // A resync drops the messages that have not started and queues a snapshot behind the one that has.
    rig.world.blobs.borrow_mut().insert(conn, vec![5; 1_000]);
    rig.hub.resync(conn, "terrain").unwrap();
    assert!(rig.hub.backlog(conn, "terrain") < 8 << 20);
}

#[test]
fn channels_are_off_until_enabled_and_a_layer_with_none_enabled_writes_the_same_bits() {
    // The same scenario with and without the layer installed: every datagram the server sends is the same bytes.
    let run = |with_layer: bool| -> Vec<Vec<u8>> {
        let (mut s, shared, end) = net_server();
        s.vm.eval(b"new SimGroup(ClientGroup);").unwrap();
        if with_layer {
            install(&shared, registry_with(|| ServerLogic(World::default()))).unwrap();
        }
        let table = Table::torque();
        let addr = SocketAddr::from(([127, 0, 0, 1], 50_000));
        let mut client = Client::new(7, ConnectFields::standard(b"", Vec::new()));
        end.inject(addr, &client.challenge_request());
        let mut sent = Vec::new();
        for _ in 0..40 {
            s.run(TICK_MS, |_| false);
            let now = s.vm.sim_time();
            let mut heard = false;
            for (_, datagram) in end.take_sent() {
                sent.push(datagram.clone());
                match client.on_datagram(&table, &datagram, now) {
                    ClientEvent::Send(bytes) => {
                        end.inject(addr, &bytes);
                    }
                    ClientEvent::Game { received, .. } => heard |= received.payload,
                    _ => {}
                }
            }
            if heard && let Some(p) = client.protocol_mut() {
                end.inject(addr, &p.ack_packet(now));
            }
        }
        assert_eq!(client.state(), &ClientState::Connected);
        sent
    };
    let plain = run(false);
    let layered = run(true);
    assert!(plain.len() > 10);
    assert_eq!(plain.len(), layered.len());
    // The challenge nonce and digest are drawn per run; compare the game packets (the ones with the game bit).
    let game =
        |v: &[Vec<u8>]| -> Vec<Vec<u8>> { v.iter().filter(|d| d[0] & 1 == 1).cloned().collect() };
    assert!(game(&plain).len() > 5);
    assert_eq!(game(&plain), game(&layered));
}

#[test]
fn a_registry_names_ids_and_budgets_are_checked() {
    let e = |name: &str, budget: usize| {
        ChannelEntry::new(name, ChannelLimits::new(budget), || {
            ServerLogic(World::default())
        })
    };
    let reg = ChannelRegistry::new(&[e("zeta", 100), e("alpha", 100), e("mid", 100)]).unwrap();
    assert_eq!(reg.names(), ["alpha", "mid", "zeta"]);
    assert_eq!(
        (reg.id("alpha"), reg.id("zeta"), reg.id("none")),
        (Some(0), Some(2), None)
    );
    assert!(ChannelRegistry::new(&[e("a", 100), e("a", 100)]).is_err());
    assert!(ChannelRegistry::new(&[e("a", 4)]).is_err());
    assert!(ChannelRegistry::new(&[e("a", 100_000)]).is_err());
    assert!(ChannelRegistry::new(&[e("", 100)]).is_err());
    assert!(ChannelRegistry::new(&[]).unwrap().is_empty());
}

#[test]
fn tges_small_packets_carry_the_channel_too() {
    let mut rig = rig_with(1, None);
    let base = blob(8, 30_000);
    rig.world
        .blobs
        .borrow_mut()
        .insert(rig.peers[0].conn, base.clone());
    rig.enable_all();
    let ticks = rig.run_until(20_000, |r| *r.peers[0].view.blob.borrow() == base);
    eprintln!(
        "30 KB in 200-byte packets: {ticks} ticks, widest section {} bits",
        rig.peers[0].widest_section
    );
}

// ---- client to server

/// Queues `data` on peer `n`s "uploads" channel.
fn upload(rig: &Rig, n: usize, data: Vec<u8>) {
    rig.peers[n]
        .channels
        .send("uploads", Kind::Snapshot, data)
        .expect("queued");
}

#[test]
fn a_four_megabyte_upload_arrives_whole_over_a_link_losing_five_percent_while_ghosts_keep_updating()
{
    let mut rig = rig_up(1, Some(1_000), true);
    rig.hub.set_upstream_caps(UpstreamCaps {
        hold_per_connection: 8 << 20,
        ..UpstreamCaps::default()
    });
    rig.peers[0].channels.set_upstream_backlog(8 << 20);
    // The caps are read when the connection is enabled, and its client starts over with them.
    rig.enable_all();
    rig.peers[0].channels.reset();
    rig.peers[0].channels.set_active(true);
    rig.link.up_loss = 5;
    rig.link.loss = 5;
    // The ghost rate with nothing uploading.
    for _ in 0..30 {
        rig.tick();
    }
    let r0 = rig.records();
    for _ in 0..150 {
        rig.tick();
    }
    let quiet = rig.records() - r0;

    let data = blob(9, 4 << 20);
    upload(&rig, 0, data.clone());
    let conn = rig.peers[0].conn;
    let r1 = rig.records();
    let mut peak = 0;
    let ticks = rig.run_until(100_000, |r| {
        peak = peak.max(r.hub.upstream_held(conn));
        !r.up.got.borrow().is_empty()
    });
    let busy = rig.records() - r1;
    eprintln!(
        "4 MiB upstream in {ticks} ticks, held at most {peak} bytes; ghost records {quiet} in 150 quiet ticks, {busy} in {ticks} busy"
    );
    let got = rig.up.got.borrow();
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].0, got[0].1), (conn, Kind::Snapshot));
    assert!(got[0].2 == data, "the upload arrived intact");
    assert!(peak <= 8 << 20);
    drop(got);
    assert_eq!(rig.hub.upstream_held_total(), 0);
    // The connection stayed up, the moves and acknowledgements went on, and the ghosts were not starved.
    assert_eq!(rig.peers[0].client.state(), &ClientState::Connected);
    assert_eq!(rig.hub.connected(), vec![conn]);
    let (rate_quiet, rate_busy) = (quiet as f64 / 150.0, busy as f64 / ticks as f64);
    assert!(
        rate_busy >= rate_quiet * 0.9,
        "ghost updates per tick fell from {rate_quiet:.2} to {rate_busy:.2}"
    );
}

#[test]
fn an_upload_in_small_messages_arrives_in_order_and_a_disconnect_is_told_to_the_logic() {
    let mut rig = rig_up(2, Some(1_000), true);
    rig.enable_all();
    rig.link.up_loss = 15;
    let messages: Vec<Vec<u8>> = (0..12)
        .map(|i| blob(20 + i, 1 + 3_000 * i as usize))
        .collect();
    for m in &messages {
        upload(&rig, 0, m.clone());
        upload(&rig, 1, m.iter().map(|b| !b).collect());
    }
    rig.run_until(40_000, |r| r.up.got.borrow().len() == 24);
    let (a, b) = (rig.peers[0].conn, rig.peers[1].conn);
    for (who, flip) in [(a, false), (b, true)] {
        let mine: Vec<Vec<u8>> = rig
            .up
            .got
            .borrow()
            .iter()
            .filter(|(c, _, _)| *c == who)
            .map(|(_, _, d)| d.clone())
            .collect();
        let expect: Vec<Vec<u8>> = messages
            .iter()
            .map(|m| m.iter().map(|x| if flip { !x } else { *x }).collect())
            .collect();
        assert_eq!(mine, expect, "each client's messages, whole and in order");
    }
    // A client that disconnects is told to the logic.
    let proto = rig.peers[1].client.disconnect_datagram(&rig.table, b"bye");
    rig.end.inject(rig.peers[1].addr, &proto);
    for _ in 0..40 {
        rig.tick();
    }
    assert_eq!(rig.up.gone.borrow().as_slice(), &[b]);
    assert_eq!(rig.hub.upstream_held(b), 0);
}

#[test]
fn a_client_that_floods_its_upload_past_the_rate_is_dropped_and_the_others_are_not() {
    let mut rig = rig_up(2, Some(1_000), true);
    rig.hub.set_upstream_caps(UpstreamCaps {
        bytes_per_second: 2_000,
        byte_burst: 8_000,
        ..UpstreamCaps::default()
    });
    rig.enable_all();
    rig.peers[0].channels.reset();
    rig.peers[0].channels.set_active(true);
    rig.peers[1].channels.reset();
    rig.peers[1].channels.set_active(true);
    let (flood, quiet) = (rig.peers[0].conn, rig.peers[1].conn);
    upload(&rig, 0, blob(30, 1 << 20));
    upload(&rig, 1, blob(31, 3_000));
    rig.run_until(2_000, |r| !r.hub.connected().contains(&flood));
    assert_eq!(rig.hub.connected(), vec![quiet], "the flooder was dropped");
    assert_eq!(rig.hub.upstream_held(flood), 0);
    assert_eq!(rig.up.gone.borrow().as_slice(), &[flood]);
    // The other one is untouched and gets its message through.
    rig.run_until(2_000, |r| {
        r.up.got
            .borrow()
            .iter()
            .any(|(c, _, d)| *c == quiet && d.len() == 3_000)
    });
}

#[test]
fn a_hostile_upstream_section_drops_the_connection_cleanly() {
    use torque_net::chunk::{CHUNK_LEN_BITS, id_bits};
    let mut rig = rig_up(1, Some(1_000), true);
    rig.enable_all();
    let conn = rig.peers[0].conn;
    for _ in 0..5 {
        rig.tick();
    }
    assert_eq!(rig.hub.connected(), vec![conn]);
    // A chunk of a message far above the channels size cap (the width of its length field is the cap's).
    let size_bits = torque_net::bits::bit_length(4 << 20);
    let now = rig.s.vm.sim_time();
    let p = &mut rig.peers[0];
    let mut w = p.client.begin_data_packet(now, false).expect("a packet");
    w.write_flag(true);
    w.write_bits(0, id_bits(1));
    w.write_flag(false);
    w.write_bits(0, 16);
    w.write_bits((1 << size_bits) - 1, size_bits);
    w.write_bits(0, size_bits);
    w.write_bits(1, CHUNK_LEN_BITS);
    w.write_u8(1);
    w.write_flag(false);
    rig.end.inject(p.addr, &w.into_bytes());
    rig.s.run(TICK_MS, |_| false);
    assert!(rig.hub.connected().is_empty(), "dropped for the bad packet");
    assert_eq!(rig.hub.upstream_held_total(), 0);
    assert_eq!(rig.up.gone.borrow().as_slice(), &[conn]);
    assert!(rig.up.got.borrow().is_empty());
}

#[test]
fn a_connection_whose_channels_are_off_never_reads_an_upstream_section_and_sends_none() {
    let mut rig = rig_up(1, Some(1_000), true);
    // Not enabled: the client has nothing to write, the server nothing to read, and a send is refused.
    rig.peers[0].channels.set_active(false);
    assert!(!rig.peers[0].channels.writes_upstream());
    assert_eq!(
        rig.peers[0]
            .channels
            .send("uploads", Kind::Delta, vec![1, 2, 3]),
        Err(SendError::UnknownChannel)
    );
    for _ in 0..30 {
        rig.tick();
    }
    assert_eq!(rig.peers[0].client.state(), &ClientState::Connected);
    assert!(rig.up.got.borrow().is_empty());
    assert_eq!(rig.hub.upstream_held_total(), 0);
}

#[test]
fn registry_directions_have_ids_of_their_own_and_a_reserved_prefix() {
    let e = |name: &str| ChannelEntry::new(name, ChannelLimits::new(100), || Quiet);
    let u = |name: &str| ChannelEntry::to_server(name, ChannelLimits::new(100), || Quiet);
    let reg = ChannelRegistry::new(&[e("b"), u("b"), u("a"), e("c")]).unwrap();
    assert_eq!(reg.names(), ["b", "c"]);
    assert_eq!(reg.up_names(), ["a", "b"]);
    assert_eq!(
        (reg.id("b"), reg.up_id("b"), reg.up_id("c")),
        (Some(0), Some(1), None)
    );
    assert!(ChannelRegistry::new(&[u("x"), u("x")]).is_err());
    assert!(ChannelRegistry::new(&[e("up:x")]).is_err());
    assert!(ChannelRegistry::new(&[u("up:x")]).is_ok());
    let big = ChannelEntry::to_server(
        "big",
        ChannelLimits::new(100).with_max_message(64 << 20),
        || Quiet,
    );
    assert!(ChannelRegistry::new(&[big]).is_err());
    assert!(!ChannelRegistry::new(&[u("only")]).unwrap().is_empty());
    assert_eq!(ChannelRegistry::new(&[u("only")]).unwrap().len(), 0);
}
