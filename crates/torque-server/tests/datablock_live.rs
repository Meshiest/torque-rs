//! The live datablock chain over the memory transport, read by a client that speaks Torque's protocol and records every
//! `SimDataBlockEvent` it gets: datablocks added and edited mid-game reach the client once each, in key order (a referrer after
//! the datablock it names), an edit made while the client still downloads its mission's datablocks is not lost, a lossy link
//! converges, and the server's callbacks run once per chain.

use crate::net_client;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::rc::Rc;

use net_client::{NetServer, connection_of, eval};
use torque_ghost_world::tge::TgeCodec;
use torque_ghosts::datablocks::DatablockCodecSet;
use torque_ghosts::datablocks::fields::fields;
use torque_ghosts::{NetWidths, TgeDatablocks, Value, tge_reps};
use torque_net::NetError;
use torque_net::bits::BitReader;
use torque_net::classes::{Kind, NetClasses};
use torque_net::client::{Client, ClientEvent as Handshake, ClientState};
use torque_net::control::{ClientBlock, ServerHead};
use torque_net::events::{
    ConnectionMessage, EventCtx, EventIds, EventQueue, EventReceiver, EventRegistry, EventUnpacker,
    NetEvent, SectionParams,
};
use torque_net::ghost::{GhostClient, GhostConfig};
use torque_net::handshake::{ConnId, ConnectFields};
use torque_net::huffman::Table;
use torque_net::profile::{DEFAULT_DATABLOCK_BITS, DEFAULT_GHOST_LIMIT};
use torque_net::protocol::{Received, Role, reader_at};
use torque_server::net::codecs::{DATABLOCKS_DONE, DATABLOCKS_DOWNLOAD_DONE};
use torque_server::net::torque::{NetPasses, TgeGhostGame, TorqueNet};
use torque_server::net::transport::{MemoryEnd, MemoryTransport, SharedTransport};
use torque_server::net::{Host, HostParts, NetClock, NetShared, attach_host};
use torque_server::server::{Server, TICK_MS};
use torque_server::state::ServerState;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

const SERVER_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 28_000);

/// The test game: a connection gets its datablocks at once and the script counts the chain's callbacks.
const GAME: &str = r#"
new SimGroup(ClientGroup);
function GameConnection::onConnect(%client) { ClientGroup.add(%client); $client = %client; %client.transmitDataBlocks(1); }
function GameConnection::onDataBlocksDone(%client, %seq) { $done++; }
function GameConnection::onDataBlocksUpdated(%client, %count) { $updated++; $counts = $counts @ %count @ ","; }
"#;

type Shared = Rc<RefCell<NetShared<TorqueNet>>>;

fn game_server() -> (NetServer, Shared, MemoryEnd) {
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
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
    state.borrow_mut().allow_connections = true;
    eval(&mut s, GAME);
    (s, shared, end)
}

fn global(s: &NetServer, name: &str) -> String {
    String::from_utf8_lossy(&s.vm.global(name).unwrap_or_default()).into_owned()
}

fn answer(s: &mut NetServer, code: &str) -> String {
    String::from_utf8(s.vm.eval(code.as_bytes()).unwrap()).unwrap()
}

fn id_of(s: &NetServer, name: &str) -> u32 {
    s.vm.objects().find(name.as_bytes()).expect(name)
}

/// Declares `n` AudioDescriptions `Desc{from}`..: `referenceDistance` (a raw float on the wire) is a quarter of the index.
fn declare_descs(s: &mut NetServer, from: usize, n: usize) {
    let mut code = String::new();
    for i in from..from + n {
        code.push_str(&format!(
            "datablock AudioDescription(Desc{i}) {{ is3D = true; referenceDistance = {}; }};\n",
            distance(i)
        ));
    }
    eval(s, &code);
}

fn distance(i: usize) -> f32 {
    i as f32 * 0.25
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

    /// True `percent` times in a hundred.
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

// ---- the client

/// One `SimDataBlockEvent` the client read, in the order read.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    id: u32,
    index: u32,
    total: u32,
    body: bool,
}

/// A datablock event with a body: the id, the index, the total and the record's fields.
type DbBody = (u32, u32, u32, Vec<(String, u32, Value)>);

#[derive(Debug, Clone)]
enum Ev {
    Message(ConnectionMessage),
    Always,
    DataBlock(Option<DbBody>),
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
        if class == ctx.ids.ghost_always_object {
            return self.ghosts.read_always_event(r).map(|_| Ev::Always);
        }
        if class != ctx.ids.sim_data_block {
            return NetEvent::unpack(class, r, ctx).map(|e| match e {
                NetEvent::ConnectionMessage(m) => Ev::Message(m),
                _ => Ev::DataBlock(None),
            });
        }
        // `SimDataBlockEvent::unpack`: the flag, the id, the class id, the position, the total, the record.
        if !r.read_flag()? {
            return Ok(Ev::DataBlock(None));
        }
        let widths = NetWidths::new(ctx.ghost_bits, ctx.datablock_bits);
        let (lo, hi) = widths.datablock_range();
        let id = r.read_ranged_u32(lo, hi)?;
        let class_id = r.read_bits(self.classes.id_bits(Kind::DataBlock))?;
        let index = r.read_bits(ctx.datablock_bits)?;
        let total = r.read_bits(ctx.datablock_bits + 1)?;
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
            index,
            total,
            fields(&record, widths),
        ))))
    }
}

/// A client of the server over Torque's protocol that keeps the datablocks it was sent and every datablock event it read.
struct LiveClient {
    addr: SocketAddr,
    client: Client,
    table: Table,
    ids: EventIds,
    classes: NetClasses,
    widths: NetWidths,
    ghosts: Option<GhostClient<TgeCodec>>,
    out: EventQueue<NetEvent>,
    receiver: EventReceiver<Ev>,
    datablocks: BTreeMap<u32, Vec<(String, u32, Value)>>,
    seen: Vec<Seen>,
    dones: u32,
    errors: Vec<String>,
}

impl LiveClient {
    fn new(port: u16) -> LiveClient {
        let events = EventRegistry::torque();
        LiveClient {
            addr: SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), port),
            client: Client::new(u32::from(port), ConnectFields::standard(b"", Vec::new())),
            table: Table::torque(),
            ids: events.ids().unwrap(),
            classes: NetClasses::derive(&tge_reps(&events)).unwrap(),
            widths: NetWidths::engine_default(),
            ghosts: None,
            out: EventQueue::new(),
            receiver: EventReceiver::new(),
            datablocks: BTreeMap::new(),
            seen: Vec::new(),
            dones: 0,
            errors: Vec::new(),
        }
    }

    fn start(&mut self, end: &MemoryEnd) {
        end.inject(self.addr, &self.client.challenge_request());
    }

    fn connected(&self) -> bool {
        *self.client.state() == ClientState::Connected
    }

    fn ids(&self) -> BTreeSet<u32> {
        self.datablocks.keys().copied().collect()
    }

    /// A datablock's `referenceDistance`, as the client holds it (the record's member name, less its prefix).
    fn distance(&self, id: u32) -> Option<f32> {
        self.datablocks
            .get(&id)?
            .iter()
            .find_map(|(n, _, v)| match v {
                Value::F32(f)
                    if n.trim_start_matches('m')
                        .eq_ignore_ascii_case("referenceDistance") =>
                {
                    Some(*f)
                }
                _ => None,
            })
    }

    /// A profile's `description` pointer.
    fn description(&self, id: u32) -> Option<u32> {
        self.datablocks
            .get(&id)?
            .iter()
            .find_map(|(n, _, v)| match v {
                Value::Id(target) if n == "description" => *target,
                _ => None,
            })
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
                let codec = TgeCodec::new(names, self.widths, self.table.clone());
                self.ghosts = Some(GhostClient::new(
                    codec,
                    self.classes.id_bits(Kind::Object),
                    ghost_bits,
                ));
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
    ) -> Result<(), NetError> {
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
            &mut Unpacker {
                ghosts,
                classes: &self.classes,
            },
            &mut incoming,
            None,
        )?;
        ghosts.read_section(&mut r)?;
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
                Ev::DataBlock(Some((id, index, total, record))) => {
                    self.seen.push(Seen {
                        id,
                        index,
                        total,
                        body: true,
                    });
                    self.datablocks.insert(id, record);
                }
                Ev::DataBlock(None) | Ev::Always => {}
            }
        }
        Ok(())
    }

    /// The client's packet; `lost` builds it (the packet layer counts the send) and drops it on the way.
    fn send(&mut self, end: &MemoryEnd, now: u32, lost: bool) {
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
        if !lost {
            end.inject(self.addr, &w.into_bytes());
        }
    }
}

/// A server and its clients, stepped by hand, with a share of the datagrams lost.
struct Rig {
    s: NetServer,
    shared: Shared,
    end: MemoryEnd,
    clients: Vec<LiveClient>,
    /// Datagrams lost, in a hundred.
    loss: u64,
    rng: Rng,
    next_port: u16,
}

impl Rig {
    fn new() -> Rig {
        let (s, shared, end) = game_server();
        Rig {
            s,
            shared,
            end,
            clients: Vec::new(),
            loss: 0,
            rng: Rng::new(1),
            next_port: 50_000,
        }
    }

    /// Starts a client's connect; its index.
    fn join(&mut self) -> usize {
        let mut client = LiveClient::new(self.next_port);
        self.next_port += 1;
        client.start(&self.end);
        self.clients.push(client);
        self.clients.len() - 1
    }

    fn conn(&self, client: usize) -> ConnId {
        let obj = connection_of(&self.shared, self.clients[client].addr);
        self.shared.borrow().conns[&obj].conn.expect("accepted")
    }

    /// One server tick with the datagrams carried both ways.
    fn tick(&mut self) {
        self.s.run(TICK_MS, |_| false);
        let now = self.s.vm.sim_time();
        for (to, datagram) in self.end.take_sent() {
            if self.loss > 0 && self.rng.chance(self.loss) {
                continue;
            }
            if let Some(c) = self.clients.iter_mut().find(|c| c.addr == to) {
                c.datagram(&self.end, &datagram, now);
            }
        }
        for c in &mut self.clients {
            if c.connected() {
                let lost = self.loss > 0 && self.rng.chance(self.loss);
                c.send(&self.end, now, lost);
            }
        }
    }

    /// Ticks until `done`, at most `limit` ticks; the ticks it took, or None.
    fn run_until(&mut self, limit: usize, mut done: impl FnMut(&Rig) -> bool) -> Option<usize> {
        for tick in 0..limit {
            if done(self) {
                return Some(tick);
            }
            self.tick();
        }
        done(self).then_some(limit)
    }

    /// Connects a client and runs its mission chain to the end.
    fn connected(&mut self) -> usize {
        let c = self.join();
        let took = self.run_until(600, |r| r.clients[c].dones == 1);
        assert!(
            took.is_some(),
            "the mission chain ends: {:?}",
            self.clients[c].errors
        );
        assert!(
            self.clients[c].errors.is_empty(),
            "{:?}",
            self.clients[c].errors
        );
        c
    }

    fn update(&mut self) -> usize {
        answer(&mut self.s, "return updateDataBlocks();")
            .parse()
            .expect("a count")
    }

    /// Runs until client `c` has its next datablocks-done message and the server has heard its answer; the ticks it took.
    fn round(&mut self, c: usize) -> usize {
        let want = self.clients[c].dones + 1;
        let updated = global(&self.s, "$updated").parse::<u32>().unwrap_or(0) + 1;
        let took = self.run_until(400, |r| {
            r.clients[c].dones >= want
                && global(&r.s, "$updated").parse::<u32>().unwrap_or(0) >= updated
        });
        took.unwrap_or_else(|| panic!("the round ends: {:?}", self.clients[c].errors))
    }
}

#[test]
fn five_added_mid_game_reach_the_client_in_fifty_seeded_sequences() {
    let mut rig = Rig::new();
    declare_descs(&mut rig.s, 0, 20);
    let c = rig.connected();
    assert_eq!(rig.clients[c].ids(), server_ids(&rig.s));
    let mut declared = 20;
    for seed in 0..50u64 {
        let mut rng = Rng::new(seed);
        // Some edits of what the client has, then five new ones, in a random order of the two.
        let edits = rng.below(4);
        let mut edited = Vec::new();
        for _ in 0..edits {
            let i = rng.below(declared);
            let v = 100.0 + rng.below(50) as f32 * 0.5;
            eval(&mut rig.s, &format!("Desc{i}.referenceDistance = {v};"));
            edited.push((id_of(&rig.s, &format!("Desc{i}")), v));
        }
        declare_descs(&mut rig.s, declared, 5);
        let new: Vec<u32> = (declared..declared + 5)
            .map(|i| id_of(&rig.s, &format!("Desc{i}")))
            .collect();
        declared += 5;
        let before = rig.clients[c].seen.len();
        assert_eq!(rig.update(), 1, "seed {seed}: one client reached");
        rig.round(c);
        let client = &rig.clients[c];
        assert_eq!(client.ids(), server_ids(&rig.s), "seed {seed}");
        for (i, id) in new.iter().enumerate() {
            assert_eq!(
                client.distance(*id),
                Some(distance(declared - 5 + i)),
                "seed {seed}: new {id}"
            );
        }
        for (id, v) in &edited {
            assert_eq!(client.distance(*id), Some(*v), "seed {seed}: edited {id}");
        }
        // Each of the round's events once, with ordinals within the round.
        let round = &client.seen[before..];
        let total = round.len() as u32;
        assert_eq!(
            round.iter().map(|s| s.index).collect::<Vec<_>>(),
            (0..total).collect::<Vec<_>>(),
            "seed {seed}"
        );
        assert!(round.iter().all(|s| s.total == total), "seed {seed}");
        let mut ids: Vec<u32> = round.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), round.len(), "seed {seed}: each once");
    }
    assert_eq!(global(&rig.s, "$done"), "1", "onDataBlocksDone once");
    assert_eq!(
        global(&rig.s, "$updated"),
        "50",
        "onDataBlocksUpdated once per round"
    );
}

#[test]
fn editing_one_of_a_thousand_sends_exactly_one_event_with_a_body() {
    let mut rig = Rig::new();
    declare_descs(&mut rig.s, 0, 1_000);
    let c = rig.connected();
    assert_eq!(rig.clients[c].ids().len(), 1_000);
    rig.clients[c].seen.clear();
    eval(&mut rig.s, "Desc500.referenceDistance = 77.5;");
    let id = id_of(&rig.s, "Desc500");
    assert_eq!(rig.update(), 1);
    rig.round(c);
    assert_eq!(
        rig.clients[c].seen,
        [Seen {
            id,
            index: 0,
            total: 1,
            body: true
        }]
    );
    assert_eq!(rig.clients[c].distance(id), Some(77.5));
    assert_eq!(global(&rig.s, "$counts"), "1,");
    // A round with nothing to send sends nothing and runs no callback; one asked twice in a tick is one round.
    rig.clients[c].seen.clear();
    assert_eq!(rig.update(), 1, "reached, nothing to send");
    for _ in 0..20 {
        rig.tick();
    }
    assert!(rig.clients[c].seen.is_empty());
    assert_eq!(rig.clients[c].dones, 2);
    eval(&mut rig.s, "Desc1.referenceDistance = 11.5;");
    assert_eq!(rig.update(), 1);
    assert_eq!(rig.update(), 1, "coalesced behind the round that runs");
    rig.round(c);
    for _ in 0..20 {
        rig.tick();
    }
    assert_eq!(rig.clients[c].dones, 3);
    assert_eq!(rig.clients[c].seen.len(), 1);
    assert_eq!(global(&rig.s, "$updated"), "2");
}

#[test]
fn a_referrer_edited_to_name_a_datablock_added_in_the_same_round_arrives_after_it() {
    let mut rig = Rig::new();
    declare_descs(&mut rig.s, 0, 10);
    let mut code = String::new();
    for i in 0..10 {
        code.push_str(&format!(
            "datablock AudioProfile(Prof{i}) {{ description = Desc{i}; }};\n"
        ));
    }
    eval(&mut rig.s, &code);
    let c = rig.connected();
    let mut declared = 10;
    let mut unresolved = 0;
    let mut out_of_order = 0;
    for seed in 0..200u64 {
        let mut rng = Rng::new(seed);
        let added = 1 + rng.below(3);
        declare_descs(&mut rig.s, declared, added);
        let new: Vec<u32> = (declared..declared + added)
            .map(|i| id_of(&rig.s, &format!("Desc{i}")))
            .collect();
        declared += added;
        let mut referrers = Vec::new();
        for _ in 0..1 + rng.below(4) {
            let p = rng.below(10);
            let target = declared - added + rng.below(added);
            eval(&mut rig.s, &format!("Prof{p}.description = Desc{target};"));
            referrers.push(id_of(&rig.s, &format!("Prof{p}")));
            // Half the time the new target is written again after the referrer: its key is then the larger one.
            if rng.chance(50) {
                eval(
                    &mut rig.s,
                    &format!(
                        "Desc{target}.referenceDistance = {};",
                        500.0 + target as f32 * 0.5
                    ),
                );
            }
        }
        let before = rig.clients[c].seen.len();
        rig.update();
        rig.round(c);
        let client = &rig.clients[c];
        let round = &client.seen[before..];
        let at = |id: u32| round.iter().position(|s| s.id == id);
        for p in 0..10 {
            let prof = id_of(&rig.s, &format!("Prof{p}"));
            let Some(target) = client.description(prof) else {
                unresolved += 1;
                continue;
            };
            if !client.datablocks.contains_key(&target) {
                unresolved += 1;
            }
            if new.contains(&target)
                && let (Some(t), Some(r)) = (at(target), at(prof))
                && t > r
            {
                out_of_order += 1;
            }
        }
        for prof in &referrers {
            assert!(
                at(*prof).is_some(),
                "seed {seed}: the edited referrer was sent"
            );
        }
        assert_eq!(client.ids(), server_ids(&rig.s), "seed {seed}");
    }
    assert_eq!(unresolved, 0, "unresolved pointers over 200 graphs");
    assert_eq!(
        out_of_order, 0,
        "referrers before their target over 200 graphs"
    );
}

#[test]
fn an_edit_made_while_a_connection_still_downloads_reaches_it() {
    let mut rig = Rig::new();
    declare_descs(&mut rig.s, 0, 300);
    let mut lost = 0;
    for seed in 0..100u64 {
        let mut rng = Rng::new(seed);
        // One client at a time: the last one's connection goes (an address holds a few at once).
        if !rig.clients.is_empty() {
            eval(&mut rig.s, "$client.delete();");
            rig.clients.clear();
            for _ in 0..3 {
                rig.tick();
            }
        }
        let c = rig.join();
        assert!(
            rig.run_until(200, |r| r.clients[c].connected()).is_some(),
            "seed {seed}: connects"
        );
        let conn = rig.conn(c);
        // A datablock the chain sent early, edited while the chain still runs.
        let i = rng.below(40);
        let id = id_of(&rig.s, &format!("Desc{i}"));
        let wait = rng.below(6);
        let sent = rig.run_until(400, |r| {
            r.shared
                .borrow()
                .events
                .datablock_sent_key(conn, id)
                .is_some()
        });
        assert!(sent.is_some(), "seed {seed}: the datablock was rendered");
        for _ in 0..wait {
            rig.tick();
        }
        let in_flight = rig
            .shared
            .borrow()
            .events
            .datablock_state(conn)
            .map_or(0, |s| s.3);
        let v = 100.0 + (seed % 50) as f32 * 0.5;
        eval(&mut rig.s, &format!("Desc{i}.referenceDistance = {v};"));
        rig.update();
        // The mission chain, then the round queued behind it (or the round alone, when the chain had just ended).
        assert!(
            rig.run_until(600, |r| r.clients[c].dones >= 2).is_some(),
            "seed {seed}: both chains end ({in_flight} in flight at the edit)"
        );
        if rig.clients[c].distance(id) != Some(v) {
            lost += 1;
        }
        assert_eq!(rig.clients[c].ids(), server_ids(&rig.s), "seed {seed}");
    }
    assert_eq!(lost, 0, "edits lost over 100 races");
}

#[test]
fn twenty_percent_loss_converges_within_120_ticks() {
    let mut rig = Rig::new();
    declare_descs(&mut rig.s, 0, 50);
    let c = rig.connected();
    rig.loss = 20;
    rig.rng = Rng::new(7);
    for i in 0..20 {
        eval(&mut rig.s, &format!("Desc{i}.referenceDistance = 9.5;"));
    }
    declare_descs(&mut rig.s, 50, 10);
    rig.update();
    let took = rig.run_until(120, |r| {
        let client = &r.clients[c];
        client.dones == 2
            && client.ids() == server_ids(&r.s)
            && (0..20).all(|i| client.distance(id_of(&r.s, &format!("Desc{i}"))) == Some(9.5))
    });
    assert!(took.is_some(), "converged: {:?}", rig.clients[c].errors);
    assert!(
        rig.clients[c].errors.is_empty(),
        "{:?}",
        rig.clients[c].errors
    );
}

#[test]
fn a_connection_before_its_mission_chain_is_not_reached_and_gets_everything_from_it() {
    let mut rig = Rig::new();
    eval(
        &mut rig.s,
        "function GameConnection::onConnect(%client) { ClientGroup.add(%client); }",
    );
    declare_descs(&mut rig.s, 0, 5);
    let c = rig.join();
    assert!(rig.run_until(200, |r| r.clients[c].connected()).is_some());
    for _ in 0..5 {
        rig.tick();
    }
    eval(&mut rig.s, "Desc2.referenceDistance = 42.5;");
    assert_eq!(rig.update(), 0, "no connection past its mission chain");
    let obj = connection_of(&rig.shared, rig.clients[c].addr);
    eval(&mut rig.s, &format!("{obj}.transmitDataBlocks(3);"));
    assert!(rig.run_until(200, |r| r.clients[c].dones == 1).is_some());
    assert_eq!(rig.clients[c].ids(), server_ids(&rig.s));
    assert_eq!(rig.clients[c].distance(id_of(&rig.s, "Desc2")), Some(42.5));
    assert!(
        rig.run_until(50, |r| global(&r.s, "$done") == "1")
            .is_some()
    );
    assert_eq!(global(&rig.s, "$updated"), "");
}
