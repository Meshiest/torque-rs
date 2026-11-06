//! Torque's own connection half ([`TgeConn`]): torque-net's handshake and packet layer, a ghost table read with TGE's record codecs
//! (torque-net's `GhostClient` over torque-ghost-world's [`TgeCodec`]) into the client's world over TGE's set ([`TgeWorld`], a
//! [`ClientWorld`] of [`TgeClientSet`]), with Torque's answers everywhere else: no file gate, no server half for a local
//! connection, no events of a game's own, the default prediction ([`crate::predict::game::TorquePrediction`]).
//!
//! **The feed** ([`TgeFeed`]): the challenge is sent as the connect starts; TGE's `NetInterface::checkTimeouts` looks at the
//! handshake once more than [`TGE_TIMEOUT_CHECK_MS`] has passed since it last looked, and sends the challenge again once more than
//! [`TGE_HANDSHAKE_WAIT_MS`] has passed since it last went out, [`TGE_HANDSHAKE_TRIES`] times (five sends in all) before the
//! connect times out unanswered; a server packet is read as TGE's client reads it: the head (the move
//! ack and the control section, its packet data by the class of the control object's ghost), the event section (net strings,
//! remote commands, the connection messages, the ghost-always objects, the datablocks) and the ghost section. An event class
//! without a reader here does not decode: the connection ends loudly, as the engine's connection ends on any section it cannot
//! read.
//!
//! **Datablocks.** A `SimDataBlockEvent` is read as TGE's `SimDataBlockEvent::unpack` reads it: `flag(modified)`, then the id
//! (`ranged` over the datablock-id range), the class id, the position in the server's datablock group and the group's size,
//! then the class's `packData` record ([`TgeDatablocks`]). The datablock joins the world under its id (its fields named as
//! the script fields, [`ghosts::datablocks::fields::fields`]) and the scripts hear `onDataBlockObjectReceived(index, total)`.
//! The server's `DataBlocksDone` message is answered with `DataBlocksDownloadDone` and its sequence: the client has every
//! datablock it was sent (the server then runs `onDataBlocksDone`). The client's packet carries its control block and the events it posted (a remote command, the answer to the
//! server's ghost-always message).
//!
//! **The world** ([`TgeWorld`]) poses nothing (every shape stands at its default pose), names a ghost by its ShapeBase name block,
//! and draws to the Sky's visible distance.
//!
//! Credit: the connection follows TGE 1.4's `NetConnection` / `GameConnection` client half (`netConnection.cc`,
//! `netInterface.cc`, `netInterface.h`, `gameConnection.cc`), as Torque3D (MIT) carries it in `sim/netConnection.cpp`,
//! `sim/netInterface.cpp` and `sim/netInterface.h` (each Copyright (c) 2012 GarageGames, LLC), `T3D/gameBase/gameConnection.cpp` (Copyright (c) 2012 GarageGames, LLC;
//! Copyright (C) 2015 Faust Logic, Inc.) and, for `read_datablock`, `T3D/gameBase/gameConnectionEvents.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code is this crate's own.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use ghost_world::tge::{TgeClientSet, TgeCodec, TgeDatablock, shape_part};
use ghost_world::{ClientWorld, Ghost};
use ghosts::datablocks::DatablockCodecSet;
use ghosts::{NetWidths, TgeClass, TgeDatablocks, TgeRecord, Value, tge_reps};
use net::NetError;
use net::bits::BitReader;
use net::classes::{Kind, NetClasses};
use net::client::{Client, ClientEvent as Handshake};
use net::control::{ClientBlock, ControlSection, ServerHead};
use net::events::{
    ConnectionMessage, EventCtx, EventIds, EventQueue, EventReceiver, EventRegistry, EventUnpacker,
    NetEvent, SectionParams,
};
use net::ghost::GhostClient;
use net::handshake::ConnectFields;
use net::huffman::Table;
use net::profile::DEFAULT_DATABLOCK_BITS;
use net::protocol::{RateConfig, Received, Role, reader_at};
use shapes::instance::ShapeInstance;
use shapes::shape::Shape;
use torque_server::net::codecs::{DATABLOCKS_DONE, DATABLOCKS_DOWNLOAD_DONE};
use ts_vm::fields::{FieldRules, FieldType};
use ts_vm::vm::Vm;

use super::{ConnGame, Connect, Feed, FeedEvent, Link, Started, WorldOf};
use crate::datablocks::{Unpacked, UnpackedField};
use crate::hud::{HudWorld, ShapeName};
use crate::predict::game::{ControlState, PredictWorld};
use crate::render::{PoseInputs, PoseView};
use crate::state::{ClientState, GameSlot};

/// The work a WheeledVehicleData's shape may cost to read for its wheel count.
pub const WHEEL_SHAPE_BUDGET: u64 = 50_000_000;

/// How long an unanswered challenge waits before it is sent again (TGE's `ChallengeRetryTime`).
pub const TGE_HANDSHAKE_WAIT_MS: u32 = 2500;
/// How many times the challenge is sent again before the connect gives up (TGE's `ChallengeRetryCount`: the time-out check
/// gives up once the sends outnumber it).
pub const TGE_HANDSHAKE_TRIES: u32 = 4;
/// How often the handshake's time-outs are looked at (TGE's `TimeoutCheckInterval`).
pub const TGE_TIMEOUT_CHECK_MS: u32 = 1500;
/// The most datagrams one pump reads.
pub const DATAGRAMS_PER_PUMP: usize = 1024;
/// The most events waiting for the connection before the feed stops reading datagrams.
pub const MAX_WAITING_EVENTS: usize = 65_536;
/// The view distance without a Sky.
pub const DEFAULT_VIEW_DISTANCE: f32 = 500.0;

// ---- the world

/// The client's world over TGE's set: the record world and the prediction's control state.
#[derive(Debug, Default)]
pub struct TgeWorld {
    pub records: ClientWorld<TgeClientSet>,
    /// The control Player's state as the client predicts it.
    pub control_state: Option<ControlState>,
}

impl Deref for TgeWorld {
    type Target = ClientWorld<TgeClientSet>;

    fn deref(&self) -> &ClientWorld<TgeClientSet> {
        &self.records
    }
}

impl DerefMut for TgeWorld {
    fn deref_mut(&mut self) -> &mut ClientWorld<TgeClientSet> {
        &mut self.records
    }
}

/// Every shape stands at its default pose: no Player posing, no threads, no mount nodes.
impl PoseView for TgeWorld {
    type Set = TgeClientSet;

    fn pose_inputs(&self, record: &TgeRecord) -> Option<PoseInputs> {
        let _ = record;
        None
    }

    fn record_crouched(&self, record: &TgeRecord) -> bool {
        let _ = record;
        false
    }

    fn crouched(&self, index: u32) -> bool {
        let _ = index;
        false
    }

    fn import_sequences(&self, shape: &mut Shape, dir: &str, sequences: &[Vec<u8>]) {
        let _ = (shape, dir, sequences);
    }

    fn pose(
        &self,
        shape: &Shape,
        instance: &mut ShapeInstance,
        record: &TgeRecord,
        pitch: f32,
        crouched: bool,
    ) {
        let _ = (shape, instance, record, pitch, crouched);
    }

    fn pose_live(
        &self,
        index: u32,
        db: u32,
        shape: &Shape,
        record: &TgeRecord,
        pitch: f32,
    ) -> Option<ShapeInstance> {
        let _ = (index, db, shape, record, pitch);
        None
    }

    fn mount_render(&self, index: u32) -> Option<[f32; 16]> {
        let _ = index;
        None
    }
}

impl HudWorld for TgeWorld {
    /// The Sky's visible and fog distances (its `visibility` block), capped by the frame's view distance.
    fn view_distances(&self) -> (f32, f32) {
        let sky = self.ghosts.values().find_map(|g| match &g.record {
            Some(TgeRecord::Sky(s)) => s.visibility.map(|v| [v[0].get(), v[1].get()]),
            _ => None,
        });
        let [visible, fog] = sky.unwrap_or([DEFAULT_VIEW_DISTANCE; 2]);
        let cap = self.max_view_distance.unwrap_or(f32::MAX);
        (visible.min(cap), fog.min(cap))
    }

    /// The ghosts with a ShapeBase part.
    fn named_candidates(&self) -> Vec<(u32, &Ghost<TgeClientSet>)> {
        self.ghosts
            .iter()
            .filter(|(_, g)| g.record.as_ref().and_then(shape_part).is_some())
            .map(|(i, g)| (*i, g))
            .collect()
    }

    /// The ShapeBase part's name (`setShapeName`), white, seen as far as the scene draws.
    fn shape_name(&self, ghost: &Ghost<TgeClientSet>) -> Option<ShapeName> {
        let rare = shape_part(ghost.record.as_ref()?)?
            .body
            .as_ref()?
            .rare
            .as_ref()?;
        let name = self.net_string(rare.name.as_ref()?)?;
        (!name.is_empty()).then(|| ShapeName {
            name: name.to_vec(),
            color: [1.0; 3],
            distance: self.view_distances().0,
        })
    }

    fn label_at_eye(&self, ghost: &Ghost<TgeClientSet>) -> bool {
        matches!(ghost.class, Some(TgeClass::Player | TgeClass::AiPlayer))
    }
}

impl PredictWorld for TgeWorld {
    /// Every ghost: TGE's static collision world has no index of its own.
    fn container_ghosts(&self) -> Vec<(u32, &Ghost<TgeClientSet>)> {
        self.ghosts.iter().map(|(i, g)| (*i, g)).collect()
    }

    fn set_control_state(&mut self, state: Option<ControlState>) {
        self.control_state = state;
    }
}

/// `checkMaxRate`'s preferences as the client's scripts set them: `$pref::Net::PacketRateToServer` (32 when unset),
/// `$pref::Net::PacketRateToClient` (the rate the client asks the server for, 10 when unset) and `$pref::Net::PacketSize` (200),
/// each clamped as [`RateConfig::new`] clamps it.
pub fn client_rates(vm: &Vm) -> RateConfig {
    let standard = RateConfig::standard();
    let pref = |name: &str, unset: u32| {
        vm.global(name)
            .filter(|t| !t.trim_ascii().is_empty())
            .map_or(unset, |t| u32::try_from(ts_vm::text::atoi(&t)).unwrap_or(0))
    };
    RateConfig::new(
        pref("$pref::Net::PacketRateToServer", standard.to_server_rate),
        pref("$pref::Net::PacketRateToClient", standard.to_client_rate),
        pref("$pref::Net::PacketSize", standard.packet_size),
    )
}

/// The server ended its ghosting (connection message 2: a mission's end): the ghosts and everything the world kept of them
/// go, the control object with them. What belongs to the connection stays, as TGE's client keeps it: the datablocks (the next
/// mission's download may send only the ones that changed), the net strings both ways, the commands' state, the clock and the
/// counters that number births, effects and moves (a ghost born at a reused index is still a new birth).
fn end_ghosting(world: &mut ClientWorld<TgeClientSet>) {
    let old = std::mem::take(world);
    world.datablocks = old.datablocks;
    world.strings = old.strings;
    world.client_strings = old.client_strings;
    world.commands = old.commands;
    world.resets = old.resets;
    world.births = old.births;
    world.clock_ms = old.clock_ms;
    world.animate = old.animate;
    world.anim_ms = old.anim_ms;
    world.hide_own_items = old.hide_own_items;
    world.max_view_distance = old.max_view_distance;
    world.effect_seq = old.effect_seq;
    world.effects_dropped = old.effects_dropped;
    world.player_move_seq = old.player_move_seq;
    world.camera_move_seq = old.camera_move_seq;
    world.vehicle_update_seq = old.vehicle_update_seq;
}

// ---- the datablocks

/// A datablock event as TGE's feed hands it over: the server's id and the fields its codec read.
#[derive(Debug, Clone, PartialEq)]
pub struct TgeDatablockEvent {
    pub id: u32,
    pub datablock: TgeDatablock,
}

/// A value as the console prints it (`None`: no text: a null, a transform).
fn value_text(rules: FieldRules, value: &Value) -> Option<Vec<u8>> {
    use ts_vm::fields::Value as V;
    let (ty, v) = match value {
        Value::Null | Value::Xform(_) => return None,
        Value::Bool(b) => (FieldType::Bool, V::Bool(*b)),
        Value::Int(i) => (FieldType::S32, V::Int(*i as i32)),
        Value::F32(f) => (FieldType::F32, V::Float(*f)),
        Value::Str(s) => return Some(s.clone()),
        // The client's datablocks are unnamed: a pointer prints "" (its id travels beside the text).
        Value::Id(_) => return Some(Vec::new()),
    };
    Some(ts_vm::fields::get_with(rules, ty, &v, &|_| None))
}

/// TGE's unpack of a datablock event ([`crate::game::ClientGame::unpack_datablock`]'s default): each field the codec read
/// as the console prints it (`rules`), an array element by its index (a field with no element past 0 is a scalar), a pointer
/// with its id.
pub fn unpack_datablock(rules: FieldRules, event: &TgeDatablockEvent) -> Unpacked {
    let d = &event.datablock;
    let array = |name: &str| d.fields.iter().any(|(n, i, _)| n == name && *i > 0);
    let fields = d
        .fields
        .iter()
        .filter_map(|(name, index, value)| {
            let text = value_text(rules, value)?;
            Some(UnpackedField {
                name: name.as_bytes().to_vec(),
                index: if array(name) {
                    index.to_string().into_bytes()
                } else {
                    Vec::new()
                },
                text,
                pointer: match value {
                    Value::Id(id) => *id,
                    _ => None,
                },
            })
        })
        .collect();
    Unpacked {
        id: event.id,
        class: d.class.clone(),
        fields,
        error: None,
        emitter_ids: None,
    }
}

// ---- the tables

/// TGE's net tables: Torque's string coder, Torque's event classes, the net classes of TGE's class list.
pub struct TgeTables {
    pub table: Table,
    pub ids: EventIds,
    pub classes: NetClasses,
}

/// The stream's compression point as a packet's head left it: raw floats the server sent, so a packet whose point is not finite
/// (it would put every position read against it there) is refused.
pub fn stream_point(r: &net::bits::BitReader) -> Result<[f32; 3], NetError> {
    let cp = r.compression_point();
    if cp.iter().all(|c| c.is_finite()) {
        Ok(cp)
    } else {
        Err(NetError::Invalid("compression point not finite"))
    }
}

impl TgeTables {
    /// TGE's tables (Torque's derive: neither fails).
    pub fn tge() -> Result<TgeTables, String> {
        let events = EventRegistry::torque();
        Ok(TgeTables {
            table: Table::torque(),
            ids: events.ids().map_err(|e| e.to_string())?,
            classes: NetClasses::derive(&tge_reps(&events)).map_err(|e| format!("{e:?}"))?,
        })
    }

    fn ctx(&self, widths: NetWidths) -> EventCtx<'_> {
        EventCtx {
            table: &self.table,
            ids: &self.ids,
            ghost_bits: widths.ghost_bits,
            datablock_bits: widths.datablock_bits,
        }
    }
}

// ---- the feed

/// An event of TGE's server as the feed reads it.
#[derive(Debug, Clone)]
enum Incoming {
    Net(NetEvent),
    /// A ghost-always object (its index; `None` for the empty form).
    Always(Option<u32>),
    /// A datablock (`None`: the one-bit event of a datablock the client already has), its position and the group's size.
    DataBlock(Option<(TgeDatablockEvent, u32, u32)>),
}

struct Unpacker<'a> {
    ghosts: &'a mut GhostClient<TgeCodec>,
    classes: &'a NetClasses,
}

/// `SimDataBlockEvent::unpack`: the flag, the id, the class id, the position, the group's size and the record.
fn read_datablock(
    r: &mut BitReader,
    ctx: &EventCtx<'_>,
    classes: &NetClasses,
) -> Result<Option<(TgeDatablockEvent, u32, u32)>, NetError> {
    if !r.read_flag()? {
        return Ok(None);
    }
    let widths = NetWidths::new(ctx.ghost_bits, ctx.datablock_bits);
    let (lo, hi) = widths.datablock_range();
    let id = r.read_ranged_u32(lo, hi)?;
    let class_id = r.read_bits(classes.id_bits(Kind::DataBlock))?;
    let index = r.read_bits(ctx.datablock_bits)?;
    let total = r.read_bits(ctx.datablock_bits + 1)?;
    let name = classes
        .classes(Kind::DataBlock)
        .get(class_id as usize)
        .map(|c| c.name.as_str())
        .ok_or(NetError::Invalid("datablock class id"))?;
    let class = TgeDatablocks::class_by_name(name).ok_or(NetError::Invalid("datablock class"))?;
    let record =
        TgeDatablocks::decode(class, r, ctx.table, &mut net::events::free_charge(), widths)?;
    let datablock = TgeDatablock {
        class: name.to_string(),
        fields: ghosts::datablocks::fields::fields(&record, widths),
    };
    Ok(Some((TgeDatablockEvent { id, datablock }, index, total)))
}

impl EventUnpacker for Unpacker<'_> {
    type Event = Incoming;

    fn unpack(
        &mut self,
        class: u32,
        r: &mut BitReader,
        ctx: &EventCtx<'_>,
    ) -> Result<Incoming, NetError> {
        if class == ctx.ids.ghost_always_object {
            self.ghosts.read_always_event(r).map(Incoming::Always)
        } else if class == ctx.ids.sim_data_block {
            read_datablock(r, ctx, self.classes).map(Incoming::DataBlock)
        } else {
            NetEvent::unpack(class, r, ctx).map(Incoming::Net)
        }
    }
}

/// TGE's feed: torque-net's client half of the handshake and the packet layer, the ghost table read with TGE's record codecs.
pub struct TgeFeed {
    link: Box<dyn Link>,
    /// The game's files ([`crate::game::ClientGame::file_reader`]): a WheeledVehicleData's shape, for its wheel count.
    reader: Option<crate::predict::FileReader>,
    /// The wheel count of each WheeledVehicleData received (`WheeledVehicleData::preload`'s `wheelCount`: the hubs its shape
    /// has), which a WheeledVehicle's record is read by.
    wheel_counts: std::collections::BTreeMap<u32, usize>,
    shapes: crate::predict::drive::ShapeCache,
    client: Client,
    tables: TgeTables,
    ghosts: Option<GhostClient<TgeCodec>>,
    out: EventQueue<NetEvent>,
    receiver: EventReceiver<Incoming>,
    widths: NetWidths,
    events: VecDeque<FeedEvent<()>>,
    packets: usize,
    now: u32,
    /// When the challenge went out last, and how many times.
    tried: Option<u32>,
    tries: u32,
    /// When the time-outs were last looked at.
    checked: Option<u32>,
    gone: bool,
    control_set: bool,
}

impl TgeFeed {
    /// A feed over `link` whose connect request carries `args` and `password`; a local connection's (`local`: the protocol and
    /// the ghost index width its server half accepted) is connected at once.
    pub fn new(
        link: Box<dyn Link>,
        password: &[u8],
        args: Vec<Vec<u8>>,
        seq: u32,
        local: Option<(u32, u8)>,
    ) -> Result<TgeFeed, String> {
        TgeFeed::with_rates(link, password, args, seq, local, RateConfig::standard())
    }

    /// [`TgeFeed::new`] whose packet layer asks for `rates` (`checkMaxRate`'s preferences, [`client_rates`]).
    pub fn with_rates(
        link: Box<dyn Link>,
        password: &[u8],
        args: Vec<Vec<u8>>,
        seq: u32,
        local: Option<(u32, u8)>,
        rates: RateConfig,
    ) -> Result<TgeFeed, String> {
        let tables = TgeTables::tge()?;
        let fields = ConnectFields::standard(password, args);
        let client = match local {
            Some((protocol, bits)) => Client::local(fields, rates, protocol, u32::from(bits), 0),
            None => Client::new(seq, fields).with_rates(rates),
        };
        let mut feed = TgeFeed {
            link,
            reader: None,
            wheel_counts: std::collections::BTreeMap::new(),
            shapes: crate::predict::drive::ShapeCache::default(),
            client,
            tables,
            ghosts: None,
            out: EventQueue::new(),
            receiver: EventReceiver::new(),
            widths: NetWidths::engine_default(),
            events: VecDeque::new(),
            packets: 0,
            now: 0,
            tried: None,
            tries: 0,
            checked: None,
            gone: false,
            control_set: false,
        };
        match local {
            Some((_, bits)) => feed.accepted(bits),
            None => feed.challenge(),
        }
        Ok(feed)
    }

    /// The game's files the feed reads a WheeledVehicleData's shape from.
    pub fn set_reader(&mut self, reader: Option<crate::predict::FileReader>) {
        self.reader = reader;
    }

    fn connected(&self) -> bool {
        *self.client.state() == net::client::ClientState::Connected
    }

    /// `WheeledVehicleData::preload` on the client for datablock `id` (just received): its wheel count from the hubs of the
    /// shape it names, read from the game's files, told to the ghost codec. A datablock of another class, or whose shape does
    /// not read, has none (a WheeledVehicle of it does not decode).
    fn count_wheels(&mut self, world: &TgeWorld, id: u32) {
        if world
            .datablocks
            .get(&id)
            .map(crate::records::EngineDatablock::class_name)
            != Some("WheeledVehicleData")
        {
            return;
        }
        let Some(path) = crate::predict::drive::shape_path(world, id) else {
            return;
        };
        let mut work = 0;
        let Some(shape) =
            self.shapes
                .get(path, self.reader.as_ref(), &mut work, WHEEL_SHAPE_BUDGET)
        else {
            return;
        };
        let Some(data) = crate::predict::drive::wheeled_data(world, id, &shape) else {
            return;
        };
        let count = vehicle::wheeled::wheel_count(&data);
        self.wheel_counts.insert(id, count);
        if let Some(ghosts) = self.ghosts.as_mut() {
            ghosts.codec_mut().facts.wheels.insert(id, count);
        }
    }

    fn challenge(&mut self) {
        self.tried = Some(self.now);
        self.tries += 1;
        let request = self.client.challenge_request();
        let _ = self.link.send(&request);
        self.events.push_back(FeedEvent::ChallengeSent);
    }

    fn accepted(&mut self, ghost_bits: u8) {
        self.widths = NetWidths::new(ghost_bits, DEFAULT_DATABLOCK_BITS);
        let names = self
            .tables
            .classes
            .classes(Kind::Object)
            .iter()
            .map(|c| c.name.as_str());
        let codec = TgeCodec::new(names, self.widths, self.tables.table.clone());
        let class_bits = self.tables.classes.id_bits(Kind::Object);
        let mut ghosts = GhostClient::new(codec, class_bits, ghost_bits);
        ghosts.codec_mut().facts.wheels = self.wheel_counts.clone();
        ghosts.enable_log();
        self.ghosts = Some(ghosts);
    }

    fn end(&mut self, event: FeedEvent<()>) {
        self.gone = true;
        self.events.push_back(event);
    }

    fn datagram(&mut self, world: &mut TgeWorld, data: &[u8]) -> Result<(), String> {
        match self.client.on_datagram(&self.tables.table, data, self.now) {
            Handshake::Nothing => {}
            Handshake::Send(reply) => {
                let _ = self.link.send(&reply);
                self.events.push_back(FeedEvent::ChallengeAnswered);
            }
            Handshake::Accepted(accept) => {
                let bits = accept.ghost_bits.map_or_else(
                    || {
                        net::ghost::GhostConfig {
                            max_ghosts: net::profile::DEFAULT_GHOST_LIMIT,
                            local: false,
                            class_bits: self.tables.classes.id_bits(Kind::Object),
                        }
                        .ghost_bits()
                    },
                    |b| b as u8,
                );
                self.accepted(bits);
                self.events.push_back(FeedEvent::Accepted);
            }
            Handshake::Rejected(reason) => self.end(FeedEvent::Rejected(reason)),
            Handshake::Disconnected(reason) => self.end(FeedEvent::Dropped(reason)),
            Handshake::Game {
                received,
                payload_bit,
            } => self
                .game_packet(world, &received, data, payload_bit)
                .map_err(|e| e.to_string())?,
            Handshake::GameError(e) => return Err(e.to_string()),
        }
        Ok(())
    }

    fn game_packet(
        &mut self,
        world: &mut TgeWorld,
        received: &Received,
        data: &[u8],
        payload_bit: usize,
    ) -> Result<(), NetError> {
        let mut settled = Vec::new();
        for notify in &received.notifies {
            self.out
                .on_notify(notify.send_seq, notify.delivered, &mut settled);
        }
        if received.answer_ping
            && let Some(protocol) = self.client.protocol_mut()
        {
            let ack = protocol.ack_packet(self.now);
            let _ = self.link.send(&ack);
        }
        let Some(ghosts) = self.ghosts.as_mut() else {
            return Ok(());
        };
        if !received.payload {
            return Ok(());
        }
        let mut r = reader_at(data, payload_bit);
        r.enable_string_buffer();
        let mut charge = net::events::free_charge();
        let class_of = |index: u32| {
            let g = ghosts.ghost(index)?;
            let wheels = g
                .datablock
                .and_then(|db| ghosts.codec().facts.wheels.get(&db).copied());
            g.class?.tail_class(wheels)
        };
        let head = ServerHead::read_with(
            &mut r,
            self.widths.ghost_bits,
            &mut charge,
            &mut ghosts::TgeTails::new(&class_of),
        )?;
        // The point the head left the stream at, which the ghosts are read against: a resent Player at a vehicle's wheel sends no
        // position of its own, and the vehicle's packet data nested in it sets the point.
        let cp = stream_point(&r)?;
        let mut incoming = Vec::new();
        self.receiver.read_section(
            &mut r,
            &self.tables.ctx(self.widths),
            Role::Client,
            &mut Unpacker {
                ghosts,
                classes: &self.tables.classes,
            },
            &mut incoming,
            None,
        )?;
        ghosts.read_section(&mut r)?;
        self.packets += 1;
        let record = self.packets;
        world.set_clock(self.now);
        for ev in incoming {
            match ev.event {
                Incoming::Net(NetEvent::ConnectionMessage(m)) => self.message(world, m),
                Incoming::Net(NetEvent::NetString(s)) => {
                    world.apply_net_string(s.slot, &s.text);
                    self.events.push_back(FeedEvent::NetString {
                        slot: s.slot,
                        text: s.text,
                    });
                }
                Incoming::Net(NetEvent::Remote(cmd)) => {
                    world.apply_remote(&cmd.args);
                    self.events.push_back(FeedEvent::Remote(cmd));
                }
                Incoming::DataBlock(Some((event, index, total))) => {
                    world.apply_datablock(event.id, event.datablock.clone());
                    self.count_wheels(world, event.id);
                    self.events.push_back(FeedEvent::DataBlock {
                        index,
                        total,
                        body: Box::new(event),
                    });
                }
                Incoming::DataBlock(None) => {}
                Incoming::Always(index) => {
                    if let Some(i) = index {
                        world.apply_ghost_always(i);
                    }
                    self.events.push_back(FeedEvent::GhostAlwaysObject);
                }
            }
        }
        if let Some(ghosts) = self.ghosts.as_mut() {
            let changes = ghosts.take_log();
            world.apply_ghosts(record, &changes, &*ghosts, cp, &mut ());
        }
        world.apply_server_head(record, &head);
        if !self.control_set && !matches!(head.control, ControlSection::None) {
            self.control_set = true;
            self.events.push_back(FeedEvent::ControlSet);
        }
        self.events.push_back(FeedEvent::Head {
            move_ack: head.move_ack,
            control: head.control,
        });
        Ok(())
    }

    /// A connection message: 0 (the ghost-always objects are done) is answered with 1 (ready for the rest), 2 (the server ended
    /// its ghosting) clears the table, 3 (the ghost-always objects start) is the scripts' `onGhostAlwaysStarted(count)`, 6
    /// (`DataBlocksDone`) is answered with 7 (`DataBlocksDownloadDone`) and its sequence.
    fn message(&mut self, world: &mut TgeWorld, m: ConnectionMessage) {
        match m.message {
            0 => {
                let ready = NetEvent::ConnectionMessage(ConnectionMessage {
                    seq: m.seq,
                    message: 1,
                    count: 0,
                });
                let _ = self.out.post(ready);
            }
            2 => {
                if let Some(ghosts) = self.ghosts.as_mut() {
                    ghosts.clear();
                }
                end_ghosting(&mut world.records);
            }
            3 => self
                .events
                .push_back(FeedEvent::GhostAlwaysStarted(m.count)),
            DATABLOCKS_DONE => {
                let done = NetEvent::ConnectionMessage(ConnectionMessage {
                    seq: m.seq,
                    message: DATABLOCKS_DOWNLOAD_DONE,
                    count: 0,
                });
                let _ = self.out.post(done);
            }
            _ => {}
        }
    }
}

impl Feed for TgeFeed {
    type World = TgeWorld;
    type Event = ();

    fn tick_start(&mut self, vm: &Vm, now: u32) {
        let _ = vm;
        self.now = now;
    }

    /// The challenge again when the time-out check finds it due (the connect gives up once the sends outnumber the tries),
    /// then the datagrams that came.
    fn pump(&mut self, world: &mut TgeWorld) -> Result<(), String> {
        if self.gone {
            return Ok(());
        }
        let handshaking = matches!(
            self.client.state(),
            net::client::ClientState::Idle | net::client::ClientState::Challenging
        );
        if handshaking
            && self
                .checked
                .is_none_or(|c| self.now.wrapping_sub(c) > TGE_TIMEOUT_CHECK_MS)
        {
            self.checked = Some(self.now);
            if self
                .tried
                .is_none_or(|t| self.now.wrapping_sub(t) > TGE_HANDSHAKE_WAIT_MS)
            {
                if self.tries > TGE_HANDSHAKE_TRIES {
                    self.end(FeedEvent::HandshakeTimedOut);
                    return Ok(());
                }
                self.challenge();
            }
        }
        for _ in 0..DATAGRAMS_PER_PUMP {
            if self.events.len() >= MAX_WAITING_EVENTS || self.gone {
                break;
            }
            let Some(data) = self
                .link
                .recv(std::time::Duration::ZERO)
                .map_err(|e| e.to_string())?
            else {
                break;
            };
            self.datagram(world, &data)?;
        }
        Ok(())
    }

    fn disconnect(&mut self, reason: &str) {
        if self.gone {
            return;
        }
        if self.connected() {
            let bye = self
                .client
                .disconnect_datagram(&self.tables.table, reason.as_bytes());
            let _ = self.link.send(&bye);
        }
        self.gone = true;
    }

    fn take_events(&mut self, max: usize) -> Vec<FeedEvent<()>> {
        let n = max.min(self.events.len());
        self.events.drain(..n).collect()
    }

    fn is_gone(&self) -> bool {
        self.gone
    }

    fn ghost_bits(&self) -> u8 {
        self.widths.ghost_bits
    }

    fn protocol_version(&self) -> u32 {
        self.client.accept().map_or(0, |a| a.protocol)
    }

    fn round_trip(&self) -> f32 {
        self.client.protocol().map_or(0.0, |p| p.round_trip())
    }

    fn room_ordered(&self) -> usize {
        self.out.room_ordered()
    }

    fn post_net(&mut self, events: Vec<NetEvent>) {
        for e in events {
            let _ = self.out.post(e);
        }
    }

    fn send_tick(&mut self, world: &mut TgeWorld, block: &ClientBlock) {
        let _ = world;
        if self.gone || !self.connected() {
            return;
        }
        let Some(mut w) = self.client.begin_data_packet(self.now, false) else {
            return;
        };
        let Some(protocol) = self.client.protocol() else {
            return;
        };
        let send_seq = protocol.last_send_seq();
        let packet_bits = protocol.packet_limit_bits();
        w.enable_string_buffer();
        block.write(&mut w);
        let params = SectionParams {
            ghosting: false,
            hold_ordered: false,
            packet_bits,
            min_start_bits: w.bits(),
        };
        // The client's events are a few bits each: an error is not possible here.
        let _ = self.out.write_section_stalling(
            &mut w,
            &self.tables.ctx(self.widths),
            send_seq,
            params,
        );
        let _ = self.link.send(&w.into_bytes());
    }
}

/// Torque's connection half: [`TgeFeed`] into [`TgeWorld`], Torque's answers to the rest.
#[derive(Debug, Default)]
pub struct TgeConn;

impl ConnGame for TgeConn {
    type Feed = TgeFeed;
    type Parts = ();

    const HANDSHAKE_WAIT_MS: u32 = TGE_HANDSHAKE_WAIT_MS;
    const HANDSHAKE_TRIES: u32 = TGE_HANDSHAKE_TRIES;

    fn start(
        vm: &mut Vm,
        client: &Rc<RefCell<ClientState<TgeConn>>>,
        connect: Connect,
    ) -> Result<Started<TgeConn>, String> {
        let _ = client;
        let local = connect.local.map(|a| (a.protocol, a.ghost_bits));
        let reader = crate::game::installed_game(vm).and_then(|g| g.file_reader());
        let mut feed = TgeFeed::with_rates(
            connect.link,
            &connect.password,
            connect.args,
            connect.seq,
            local,
            client_rates(vm),
        )?;
        feed.set_reader(reader.clone());
        Ok(Started {
            feed,
            world: TgeWorld::default(),
            part: Box::new(super::NoPart),
            reader,
        })
    }

    fn frame(game: &GameSlot, world: &mut WorldOf<TgeConn>) {
        let _ = (game, world);
    }

    /// The first Sun's colour (its `update` block's colour).
    fn sun_color(world: &TgeWorld) -> Option<[f32; 3]> {
        world.ghosts.values().find_map(|g| match &g.record {
            Some(TgeRecord::Sun(s)) => s.update.as_ref().map(|u| {
                let c = u.color;
                [c[0].get(), c[1].get(), c[2].get()]
            }),
            _ => None,
        })
    }
}

#[cfg(test)]
#[path = "tge_tests.rs"]
mod tests;
