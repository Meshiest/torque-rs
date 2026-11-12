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
//! then the class's `packData` record ([`TgeDatablocks`]). A class id past the table's is a plugin's datablock class, read with
//! its own record; a plugin's ghost classes are read past the engine's own ([`TgeFeed::set_plugins`]: the client's engine's
//! set). The datablock joins the world under its id (its fields named as the script fields,
//! [`ghosts::datablocks::fields::fields`]) and the scripts hear `onDataBlockObjectReceived(index, total)`. The server's
//! `DataBlocksDone` message is answered with `DataBlocksDownloadDone` and its sequence: the client has every datablock it was
//! sent (the server then runs `onDataBlocksDone`). The client's packet carries its control block and the events it posted (a
//! remote command, the answer to the server's ghost-always message).
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
use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use ghost_world::tge::{TgeClientSet, TgeCodec, TgeDatablock, shape_part};
use ghost_world::world::{DatablockRetire, TouchedGhosts};
use ghost_world::{ClientWorld, Ghost};
use ghosts::block::BlockSections;
use ghosts::datablocks::DatablockCodecSet;
use ghosts::plugin::PluginClasses;
use ghosts::{NetWidths, TgeClass, TgeDatablocks, TgeRecord, Value, tge_reps};
use net::NetError;
use net::bits::{BitReader, BitWriter};
use net::classes::{Kind, NetClasses};
use net::client::{Client, ClientEvent as Handshake};
use net::connect::BlockSource;
use net::control::{ClientBlock, ControlSection, ServerHead};
use net::events::{
    ConnectionMessage, EventCtx, EventIds, EventQueue, EventReceiver, EventRegistry, EventUnpacker,
    NetEvent, SectionParams,
};
use net::fair::TokenBucket;
use net::ghost::GhostClient;
use net::handshake::ConnectFields;
use net::huffman::Table;
use net::profile::DEFAULT_DATABLOCK_BITS;
use net::protocol::{RateConfig, Received, Role, reader_at};
use shapes::instance::ShapeInstance;
use shapes::shape::Shape;
use torque_engine::net_classes::{
    ClientGhostLookup, ClientGhosts, DrawnShapes, GhostSinks, PluginStills, RawGhostEvent,
};
use torque_engine::{Engine, ProtocolKind};
use torque_server::net::channels::ClientChannels;
use torque_server::net::codecs::{DATABLOCKS_DONE, DATABLOCKS_DOWNLOAD_DONE};
use torque_server::net::rpc::RpcLanes;
use ts_vm::fields::{FieldRules, FieldType};
use ts_vm::vm::Vm;

use super::engine_wire::{EngineNonceKey, EngineStart, EngineWire, WireEvent, engine_nonce};
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
/// The pref that caps the datablocks a connection keeps (the world's table and the script group), held under the wire's id
/// range ([`max_datablocks`]).
pub const MAX_DATABLOCKS_PREF: &str = "$Pref::Client::MaxDataBlocks";
/// The cap when the pref is unset, over Torque's protocol.
pub const DEFAULT_MAX_DATABLOCKS_TGE: usize = 1_024;
/// The cap when the pref is unset, over the engine's protocol.
pub const DEFAULT_MAX_DATABLOCKS_ENGINE: usize = 4_096;
/// Datablock bodies a connection takes per second once its first datablocks-done message is in (a live round's; a server's
/// send rate stays well under: a window of events per packet).
pub const LIVE_DATABLOCKS_PER_S: u32 = 256;
/// The burst the rate allows: one wire id range, so a round that sends every datablock once is never cut.
pub const LIVE_DATABLOCK_BURST: u32 = 1 << DEFAULT_DATABLOCK_BITS;
/// Bodies refused for this long without the bucket filling again end the connection.
pub const LIVE_DATABLOCK_FLOOD_MS: u32 = 2_000;
/// The reason the connection ends with.
pub const LIVE_DATABLOCK_FLOOD_REASON: &str = "datablock flood";
/// The reason a connection ends with when the server removed more datablocks than the world keeps retired at once
/// ([`ghost_world::world::MAX_RETIRED_DATABLOCKS`]).
pub const RETIRED_DATABLOCKS_REASON: &str = "too many retired datablocks";

/// The datablocks a connection keeps at most: [`MAX_DATABLOCKS_PREF`] (its mode's default when unset or not a number), at
/// least 1 and never more than the wire's id range (the ids a `SimDataBlockEvent` can carry).
pub fn max_datablocks(vm: &Vm, engine: bool, widths: NetWidths) -> usize {
    let (lo, hi) = widths.datablock_range();
    let range = hi.wrapping_sub(lo).saturating_add(1).max(1) as usize;
    let unset = if engine {
        DEFAULT_MAX_DATABLOCKS_ENGINE
    } else {
        DEFAULT_MAX_DATABLOCKS_TGE
    };
    let pref = vm
        .global(MAX_DATABLOCKS_PREF)
        .filter(|t| !t.trim_ascii().is_empty())
        .map_or(unset, |t| {
            usize::try_from(ts_vm::text::atoi(&t)).unwrap_or(0)
        });
    pref.clamp(1, range)
}

/// The rate limit on datablock bodies after the first datablocks-done message: [`LIVE_DATABLOCKS_PER_S`] a second with a burst
/// of [`LIVE_DATABLOCK_BURST`]. A body with no token is refused; refusals that go on for [`LIVE_DATABLOCK_FLOOD_MS`] without
/// the bucket filling back to a second's worth are a flood.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveBucket {
    tokens: TokenBucket<u32>,
    /// When the refusals started, until the bucket fills back.
    excess_since: Option<u32>,
}

impl LiveBucket {
    pub fn new(now: u32) -> LiveBucket {
        LiveBucket {
            tokens: TokenBucket::starting_at(
                LIVE_DATABLOCKS_PER_S.into(),
                LIVE_DATABLOCK_BURST.into(),
                now,
            ),
            excess_since: None,
        }
    }

    /// Takes a token at `now`: false when there is none.
    pub fn take(&mut self, now: u32) -> bool {
        self.tokens.refill(now);
        if self.tokens.has(LIVE_DATABLOCKS_PER_S.into()) {
            self.excess_since = None;
        }
        if !self.tokens.has(1) {
            self.excess_since.get_or_insert(now);
            return false;
        }
        self.tokens.spend(1);
        true
    }

    /// Whether the refusals have gone on for [`LIVE_DATABLOCK_FLOOD_MS`].
    pub fn flooded(&self, now: u32) -> bool {
        self.excess_since
            .is_some_and(|since| now.wrapping_sub(since) >= LIVE_DATABLOCK_FLOOD_MS)
    }
}
/// The view distance without a Sky.
pub const DEFAULT_VIEW_DISTANCE: f32 = 500.0;

// ---- the world

/// The client's world over TGE's set: the record world and the prediction's control state.
#[derive(Debug, Default)]
pub struct TgeWorld {
    pub records: ClientWorld<TgeClientSet>,
    /// The control Player's state as the client predicts it.
    pub control_state: Option<ControlState>,
    /// The plugin ghosts the plugins' handlers have heard of, for each handler: the ghost's index, its birth and its updates.
    pub plugin_seen: Vec<HashMap<u32, (u64, u32)>>,
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
    let mut old = std::mem::take(world);
    // No ghost is left to name a retired datablock, and the next mission's chain may send a datablock at any id.
    old.purge_all_retired_datablocks();
    world.datablocks = old.datablocks;
    world.datablock_gen = old.datablock_gen;
    world.datablock_epoch = old.datablock_epoch;
    world.datablocks_purged = old.datablocks_purged;
    world.strings = old.strings;
    world.client_strings = old.client_strings;
    world.commands = old.commands;
    world.resets = old.resets;
    world.births = old.births;
    world.clock_ms = old.clock_ms;
    world.animate = old.animate;
    world.anim_ms = old.anim_ms;
    world.hide_own_items = old.hide_own_items;
    world.first_person_nodes = old.first_person_nodes;
    world.first_person_clear = old.first_person_clear;
    world.max_view_distance = old.max_view_distance;
    world.plugin_shapes = old.plugin_shapes;
    world.stills = old.stills;
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
        TgeTables::with_events(&EventRegistry::torque())
    }

    /// The engine protocol's tables: Torque's with the engine's own event classes ([`EventRegistry::engine`]), as a server of
    /// that protocol derives them.
    pub fn engine() -> Result<TgeTables, String> {
        TgeTables::with_events(&EventRegistry::engine())
    }

    fn with_events(events: &EventRegistry) -> Result<TgeTables, String> {
        Ok(TgeTables {
            table: Table::torque(),
            ids: events.ids().map_err(|e| e.to_string())?,
            classes: NetClasses::derive(&tge_reps(events)).map_err(|e| format!("{e:?}"))?,
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
    /// The server removed this datablock (the engine's protocol).
    DataBlockRemove(u32),
}

struct Unpacker<'a> {
    ghosts: &'a mut GhostClient<TgeCodec>,
    classes: &'a NetClasses,
    plugins: &'a PluginClasses,
}

/// `SimDataBlockEvent::unpack`: the flag, the id, the class id, the position, the group's size and the record. A class id past
/// the table's is a plugin's datablock class (`plugins`, numbered from the table's end); any other is refused.
fn read_datablock(
    r: &mut BitReader,
    ctx: &EventCtx<'_>,
    classes: &NetClasses,
    plugins: &PluginClasses,
    charge: &mut ghosts::ChargeFn<'_>,
) -> Result<Option<(TgeDatablockEvent, u32, u32)>, NetError> {
    if !r.read_flag()? {
        return Ok(None);
    }
    // The engine protocol's event table is the one with the datablock removal event: its records carry the extension blocks.
    let widths = NetWidths::new(ctx.ghost_bits, ctx.datablock_bits)
        .with_extensions(ctx.ids.datablock_remove().is_some());
    let (lo, hi) = widths.datablock_range();
    let id = r.read_ranged_u32(lo, hi)?;
    let class_id = r.read_bits(classes.id_bits(Kind::DataBlock))?;
    let index = r.read_bits(ctx.datablock_bits)?;
    let total = r.read_bits(ctx.datablock_bits + 1)?;
    let table_len = classes.count(Kind::DataBlock);
    if class_id as usize >= table_len {
        let class = plugins
            .by_id(
                Kind::DataBlock,
                class_id,
                PluginClasses::datablock_base(table_len),
            )
            .ok_or(NetError::Invalid("datablock class id"))?;
        let record = class.decode(r, ctx.table, charge, &ghosts::ReadCtx::new(widths, true))?;
        let datablock = TgeDatablock {
            class: class.name().to_string(),
            fields: record.record_fields(),
        };
        return Ok(Some((TgeDatablockEvent { id, datablock }, index, total)));
    }
    let name = classes
        .classes(Kind::DataBlock)
        .get(class_id as usize)
        .map(|c| c.name.as_str())
        .ok_or(NetError::Invalid("datablock class id"))?;
    let class = TgeDatablocks::class_by_name(name).ok_or(NetError::Invalid("datablock class"))?;
    let record = TgeDatablocks::decode(class, r, ctx.table, charge, widths)?;
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
            // A datablock event draws on its datagram's budget, as the ghost records do.
            let mut charge = self.ghosts.codec().charge();
            read_datablock(r, ctx, self.classes, self.plugins, &mut charge).map(Incoming::DataBlock)
        } else if ctx.ids.datablock_remove() == Some(class) {
            let (lo, hi) = NetWidths::new(ctx.ghost_bits, ctx.datablock_bits).datablock_range();
            r.read_ranged_u32(lo, hi).map(Incoming::DataBlockRemove)
        } else {
            NetEvent::unpack(class, r, ctx).map(Incoming::Net)
        }
    }
}

/// What a client's engine adds to every connect request it makes: its manifest and a ticket from its auth provider
/// ([`Engine::connect_block`]). Attached to the client's script engine by [`ConnectHello::install`]; a network connection takes
/// its [`BlockSource`] when it starts and the block is made once the server's challenge response is in, for that server and that
/// connection (a local connection needs none). An engine that says nothing on the wire attaches nothing, so its requests are
/// Torque's.
pub struct ConnectHello {
    engine: Engine,
}

impl ConnectHello {
    /// Attaches the engine's hello to `vm` when the engine [says anything on the wire](Engine::is_networked) or speaks the
    /// engine's protocol (whose connect always carries the manifest).
    pub fn install(vm: &mut Vm, engine: &Engine) {
        if engine.is_networked() || engine.protocol() == ProtocolKind::Engine {
            vm.set_extension(Rc::new(ConnectHello {
                engine: engine.clone(),
            }));
        }
    }

    /// The sections of the per-ghost block the client's engine reads (its plugins put them on the engine, and are in its
    /// manifest, so a server that accepts the client writes the same).
    #[must_use]
    pub fn blocks(&self) -> BlockSections {
        self.engine
            .resource::<BlockSections>()
            .map(|b| *b)
            .unwrap_or_default()
    }

    /// Whether the client's engine speaks the engine's protocol.
    #[must_use]
    pub fn engine_mode(&self) -> bool {
        self.engine.protocol() == ProtocolKind::Engine
    }

    /// What makes the block of one connect: an `Err` it returns is the auth provider's reason it has no ticket.
    fn source(&self, vm: &Vm) -> BlockSource {
        let engine = self.engine.clone();
        // What the client holds of mods is read once, as the connection starts.
        let mods = vm
            .extension::<torque_engine::net::ConnectExtras>()
            .and_then(|extras| (extras.0)(vm));
        let always = self.engine_mode();
        BlockSource::new(move |cx| {
            let block = if always {
                Some(engine.engine_connect_block(&net::profile::HandshakeProfile::default(), cx)?)
            } else {
                engine.connect_block(cx)?
            };
            Ok(block.map(|mut block| {
                block.mods.clone_from(&mods);
                block
            }))
        })
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
    /// The bulk channels of the engine, read from the front of each server packet once they are active.
    channels: Option<Rc<ClientChannels>>,
    /// The plugins' ghost and datablock classes the feed reads past the class table's ids.
    plugins: PluginClasses,
    /// The sections of the per-ghost block after each record (none: no block, as from a TGE server).
    blocks: BlockSections,
    /// The engine-mode session that carries the connection in place of TGE's handshake and packet header
    /// ([`TgeFeed::over_engine`]).
    engine: Option<EngineWire>,
    /// Engine mode's RPC carrier, which the session's RPC lanes fill and empty.
    rpc_lanes: Option<Rc<RpcLanes>>,
    /// Datablocks-done messages seen: the second and later end live rounds (`onDataBlocksChanged`).
    dones: u32,
    /// The datablocks the connection keeps at most ([`max_datablocks`], read at each tick's start).
    max_datablocks: usize,
    /// The rate limit on datablock bodies after the first datablocks-done message.
    live: LiveBucket,
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
        let mut feed = TgeFeed::made(link, password, args, seq, local, rates)?;
        match local {
            Some((_, bits)) => feed.accepted(bits),
            None => feed.challenge(),
        }
        Ok(feed)
    }

    /// A feed whose connection rides an engine-mode session ([`super::engine_wire`]): the connect is made at the first
    /// pump, from `start`; `lanes` carries the RPCs.
    pub fn over_engine(
        link: Box<dyn Link>,
        start: EngineStart,
        lanes: Option<Rc<RpcLanes>>,
        rates: RateConfig,
    ) -> Result<TgeFeed, String> {
        let mut feed = TgeFeed::made(link, &start.password, start.args.clone(), 0, None, rates)?;
        feed.tables = TgeTables::engine()?;
        feed.engine = Some(EngineWire::new(start));
        feed.rpc_lanes = lanes;
        Ok(feed)
    }

    /// Reads and writes the engine protocol's event table ([`TgeTables::engine`]): for a local connection whose server half
    /// speaks the engine's protocol (its event layer derives that table), before the first packet.
    pub fn use_engine_tables(&mut self) -> Result<(), String> {
        self.tables = TgeTables::engine()?;
        Ok(())
    }

    fn made(
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
        Ok(TgeFeed {
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
            channels: None,
            plugins: PluginClasses::default(),
            blocks: BlockSections::default(),
            engine: None,
            rpc_lanes: None,
            dones: 0,
            max_datablocks: DEFAULT_MAX_DATABLOCKS_TGE,
            live: LiveBucket::new(0),
        })
    }

    /// A `DataBlockRemoveEvent` for `id`: the world retires the datablock ([`ClientWorld::retire_datablock`]) and the host's
    /// purge drops it once nothing names it. An id the world does not hold, or holds retired already, is ignored and counted
    /// ([`FeedEvent::DataBlockRemoveIgnored`]); one past the retired cap ends the connection with [`RETIRED_DATABLOCKS_REASON`].
    fn remove_datablock(&mut self, world: &mut TgeWorld, id: u32) {
        match world.records.retire_datablock(id) {
            DatablockRetire::Retired => {}
            DatablockRetire::Unknown | DatablockRetire::Already => {
                self.events.push_back(FeedEvent::DataBlockRemoveIgnored);
            }
            DatablockRetire::OverCap => {
                self.disconnect(RETIRED_DATABLOCKS_REASON);
                self.end(FeedEvent::Dropped(
                    RETIRED_DATABLOCKS_REASON.as_bytes().to_vec(),
                ));
            }
        }
    }

    /// Whether a datablock event with a body for `id` is taken: a retired id and a new id when the table is at its cap are
    /// refused, and after the first datablocks-done message every body spends a token of the live rate; a refusal is counted
    /// and refusals that last [`LIVE_DATABLOCK_FLOOD_MS`] end the connection with [`LIVE_DATABLOCK_FLOOD_REASON`].
    fn takes_datablock(&mut self, world: &TgeWorld, id: u32) -> bool {
        // A removed datablock's id takes nothing until the purge let it go: the server never sends one.
        if world.records.is_datablock_retired(id) {
            self.events.push_back(FeedEvent::DataBlockRefused);
            return false;
        }
        let known = world.records.datablocks.contains_key(&id);
        if !known && world.records.datablocks.len() >= self.max_datablocks {
            self.events.push_back(FeedEvent::DataBlockRefused);
            return false;
        }
        if self.dones > 0 && !self.live.take(self.now) {
            self.events.push_back(FeedEvent::DataBlockRefused);
            if self.live.flooded(self.now) {
                self.disconnect(LIVE_DATABLOCK_FLOOD_REASON);
                self.end(FeedEvent::Dropped(
                    LIVE_DATABLOCK_FLOOD_REASON.as_bytes().to_vec(),
                ));
            }
            return false;
        }
        true
    }

    /// The engine's bulk channels this connection reads. They start over for this connection ([`ClientChannels::reset`]) and
    /// are turned on when the server's accept says its gate matched the channels this client listed in its manifest.
    pub fn set_channels(&mut self, channels: Option<Rc<ClientChannels>>) {
        if let Some(channels) = &channels {
            channels.reset();
        }
        self.channels = channels;
    }

    /// The plugins' ghost and datablock classes the feed reads (the server's set: the client's engine has the same plugins).
    pub fn set_plugins(&mut self, plugins: PluginClasses) {
        if let Some(ghosts) = self.ghosts.as_mut() {
            ghosts.codec_mut().plugins = plugins.clone();
        }
        self.plugins = plugins;
    }

    /// The sections of the per-ghost block the server writes after each record ([`ghosts::block`]; the client's engine
    /// has the plugins that fill them, and so has the server, whose manifest matched: [`ConnectHello::blocks`]).
    pub fn set_blocks(&mut self, blocks: BlockSections) {
        if let Some(ghosts) = self.ghosts.as_mut() {
            ghosts.codec_mut().blocks = blocks;
        }
        self.blocks = blocks;
    }

    /// Adds the block `source` makes (the engine's manifest and a ticket for `server`, the address dialed, and the challenge) to the
    /// connect request, after its arguments.
    pub fn set_connect_source(&mut self, server: std::net::SocketAddr, source: BlockSource) {
        self.client.set_block_source(server, source);
    }

    /// The game's files the feed reads a WheeledVehicleData's shape from.
    pub fn set_reader(&mut self, reader: Option<crate::predict::FileReader>) {
        self.reader = reader;
    }

    fn connected(&self) -> bool {
        match &self.engine {
            Some(wire) => wire.is_accepted(),
            None => *self.client.state() == net::client::ClientState::Connected,
        }
    }

    /// The pump in engine mode: the session's own timers, then the datagrams that came, each as the session reads it.
    fn engine_pump(&mut self, world: &mut TgeWorld) -> Result<(), String> {
        let mut out = Vec::new();
        if let Some(wire) = self.engine.as_mut() {
            wire.pump(&mut *self.link, self.now, &mut out);
        }
        self.apply_wire(world, std::mem::take(&mut out))?;
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
            if let Some(wire) = self.engine.as_mut() {
                wire.receive(&data, self.now, &mut out);
            }
            self.apply_wire(world, std::mem::take(&mut out))?;
        }
        Ok(())
    }

    /// What the session said, as the feed's events and the payload's reads and fates.
    fn apply_wire(&mut self, world: &mut TgeWorld, events: Vec<WireEvent>) -> Result<(), String> {
        for event in events {
            if self.gone {
                break;
            }
            match event {
                WireEvent::Sent => self.events.push_back(FeedEvent::ChallengeSent),
                WireEvent::Answered => self.events.push_back(FeedEvent::ChallengeAnswered),
                WireEvent::Accepted { ghost_bits } => {
                    // Every peer of the engine's protocol has the channels on, both ways.
                    let bits = if ghost_bits == 0 {
                        self.default_ghost_bits()
                    } else {
                        ghost_bits
                    };
                    if let Some(channels) = &self.channels {
                        channels.set_active(true);
                        channels.set_ghost_bits(bits);
                    }
                    self.accepted(bits);
                    self.events.push_back(FeedEvent::Accepted);
                }
                WireEvent::Packet { seq, data } => {
                    self.game_packet(world, &Received::payload(u32::from(seq)), &data, 0)
                        .map_err(|e| e.to_string())?;
                }
                WireEvent::Rpc(batch) => {
                    let kept = self
                        .rpc_lanes
                        .as_ref()
                        .is_some_and(|lanes| lanes.received(None, batch));
                    if !kept {
                        return Err(
                            "the server sent RPC messages this client does not take".to_string()
                        );
                    }
                }
                WireEvent::Fate {
                    send_seq,
                    delivered,
                } => {
                    let mut settled = Vec::new();
                    self.out.on_notify(send_seq, delivered, &mut settled);
                    if let Some(channels) = &self.channels {
                        channels.on_notify(send_seq, delivered);
                    }
                }
                WireEvent::Ended(event) => self.end(event),
                WireEvent::Failed(why) => return Err(why),
            }
        }
        Ok(())
    }

    /// The ghost-id width of a server's accept that names none.
    fn default_ghost_bits(&self) -> u8 {
        net::ghost::GhostConfig {
            max_ghosts: net::profile::DEFAULT_GHOST_LIMIT,
            local: false,
            class_bits: self.tables.classes.id_bits(Kind::Object),
        }
        .ghost_bits()
    }

    /// The tick's packet in engine mode: the control block, then the events that wait, in the move lane.
    fn engine_send(&mut self, world: &mut TgeWorld, block: &ClientBlock) {
        let Some(wire) = self.engine.as_mut() else {
            return;
        };
        if let Some(lanes) = &self.rpc_lanes {
            wire.feed_rpcs(lanes);
        }
        let (queue, channels, tables, widths) =
            (&mut self.out, &self.channels, &self.tables, self.widths);
        let mut write = |send_seq: u32, room: usize| {
            let packet_bits = room * 8;
            let mut w = BitWriter::with_limit(packet_bits);
            w.enable_string_buffer();
            // The upstream channels' chunks come first, in the room the moves leave.
            if let Some(channels) = channels
                && channels.writes_upstream()
            {
                let mut moves = BitWriter::new();
                block.write(&mut moves);
                channels.write_upstream(&mut w, send_seq, packet_bits, moves.bits());
            }
            block.write(&mut w);
            let params = SectionParams {
                ghosting: false,
                hold_ordered: false,
                packet_bits,
                min_start_bits: w.bits(),
            };
            // The client's events are a few bits each: a section that does not write sends nothing this tick, and its events
            // wait for the next.
            let written =
                queue.write_section_stalling(&mut w, &tables.ctx(widths), send_seq, params);
            (written.is_ok() && !w.is_overflowed()).then(|| w.into_bytes())
        };
        let mut out = Vec::new();
        wire.send(&mut *self.link, self.now, &mut write, &mut out);
        if let Err(why) = self.apply_wire(world, out) {
            self.end(FeedEvent::Dropped(why.into_bytes()));
        }
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
        // Each datablock's read has the loads' whole budget.
        self.shapes.start_tick();
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
        let codec = TgeCodec::new(names, self.widths, self.tables.table.clone())
            .with_plugins(self.plugins.clone())
            .with_blocks(self.blocks);
        let class_bits = self.tables.classes.id_bits(Kind::Object);
        let mut ghosts = GhostClient::new(codec, class_bits, ghost_bits);
        ghosts.codec_mut().facts.wheels = self.wheel_counts.clone();
        ghosts.enable_log();
        self.ghosts = Some(ghosts);
    }

    fn end(&mut self, event: FeedEvent<()>) {
        self.gone = true;
        if let Some(channels) = &self.channels {
            channels.closed();
        }
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
                if accept.channels
                    && let Some(channels) = &self.channels
                {
                    channels.set_active(true);
                }
                let bits = accept
                    .ghost_bits
                    .map_or_else(|| self.default_ghost_bits(), |b| b as u8);
                if let Some(channels) = &self.channels {
                    channels.set_ghost_bits(bits);
                }
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
            if let Some(channels) = &self.channels {
                channels.on_notify(notify.send_seq, notify.delivered);
            }
        }
        if received.answer_ping
            && let Some(protocol) = self.client.protocol_mut()
        {
            let ack = protocol.ack_packet(self.now);
            let _ = self.link.send(&ack);
        }
        if !received.payload {
            return Ok(());
        }
        let mut r = reader_at(data, payload_bit);
        r.enable_string_buffer();
        // The bulk channels' chunks come first in the payload when the server writes them.
        if let Some(channels) = &self.channels {
            channels.read(&mut r)?;
        }
        let Some(ghosts) = self.ghosts.as_mut() else {
            return Ok(());
        };
        // What one datagram's decoders may spend, so a server cannot make the client expand more per packet than its bytes
        // allow ([`ghosts::datagram_budget`]): the head, the datablock events and the ghost records all draw on the codec's.
        ghosts
            .codec_mut()
            .set_budget(Some(ghosts::datagram_budget(data.len())));
        let head = {
            let mut charge = ghosts.codec().charge();
            let class_of = |index: u32| {
                let g = ghosts.ghost(index)?;
                let wheels = g
                    .datablock
                    .and_then(|db| ghosts.codec().facts.wheels.get(&db).copied());
                g.class?.tail_class(wheels)
            };
            ServerHead::read_with(
                &mut r,
                self.widths.ghost_bits,
                &mut charge,
                // The packet data ends with its frame where the per-ghost block carries the scene hierarchy.
                &mut ghosts::TgeTails::new(&class_of).with_frames(self.blocks.links),
            )?
        };
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
                plugins: &self.plugins,
            },
            &mut incoming,
            None,
        )?;
        ghosts.read_section(&mut r)?;
        self.packets += 1;
        let record = self.packets;
        world.set_clock(self.now);
        for ev in incoming {
            if self.gone {
                break;
            }
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
                    if !self.takes_datablock(world, event.id) {
                        continue;
                    }
                    world.apply_datablock(event.id, event.datablock.clone());
                    self.count_wheels(world, event.id);
                    self.events.push_back(FeedEvent::DataBlock {
                        index,
                        total,
                        body: Box::new(event),
                    });
                }
                Incoming::DataBlock(None) => {}
                Incoming::DataBlockRemove(id) => self.remove_datablock(world, id),
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
    /// (`DataBlocksDone`) is answered with 7 (`DataBlocksDownloadDone`) and its sequence; the second 6 and every later one end a
    /// live round of the server's (the scripts' `onDataBlocksChanged`, when defined).
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
                self.dones = self.dones.saturating_add(1);
                if self.dones > 1 {
                    self.events.push_back(FeedEvent::DataBlocksChanged);
                }
            }
            _ => {}
        }
    }
}

impl Feed for TgeFeed {
    type World = TgeWorld;
    type Event = ();

    fn tick_start(&mut self, vm: &Vm, now: u32) {
        self.now = now;
        self.max_datablocks = max_datablocks(vm, self.engine.is_some(), self.widths);
    }

    /// The challenge again when the time-out check finds it due (the connect gives up once the sends outnumber the tries),
    /// then the datagrams that came.
    fn pump(&mut self, world: &mut TgeWorld) -> Result<(), String> {
        if self.gone {
            return Ok(());
        }
        if self.engine.is_some() {
            return self.engine_pump(world);
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
        if let Some(wire) = self.engine.as_mut() {
            wire.disconnect(&mut *self.link, reason, self.now);
        } else if self.connected() {
            let bye = self
                .client
                .disconnect_datagram(&self.tables.table, reason.as_bytes());
            let _ = self.link.send(&bye);
        }
        self.gone = true;
        if let Some(channels) = &self.channels {
            channels.closed();
        }
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
        match &self.engine {
            Some(wire) if wire.is_accepted() => {
                net::profile::HandshakeProfile::default().protocol_current
            }
            Some(_) => 0,
            None => self.client.accept().map_or(0, |a| a.protocol),
        }
    }

    fn round_trip(&self) -> f32 {
        match &self.engine {
            Some(wire) => wire.round_trip(),
            None => self.client.protocol().map_or(0.0, |p| p.round_trip()),
        }
    }

    fn room_ordered(&self) -> usize {
        self.out.room_ordered()
    }

    fn post_net(&mut self, events: Vec<NetEvent>) {
        for e in events {
            let _ = self.out.post(e);
        }
    }

    fn engine_session(&self) -> bool {
        self.engine.is_some()
    }

    fn send_tick(&mut self, world: &mut TgeWorld, block: &ClientBlock) {
        if self.gone || !self.connected() {
            return;
        }
        if self.engine.is_some() {
            self.engine_send(world, block);
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
        // The upstream channels' chunks come first in the payload, in the room the moves leave.
        if let Some(channels) = &self.channels
            && channels.writes_upstream()
        {
            let mut moves = BitWriter::new();
            block.write(&mut moves);
            channels.write_upstream(&mut w, send_seq, packet_bits, moves.bits());
        }
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

/// A new connection's [`TgeFeed`] and [`TgeWorld`], as [`TgeConn`] starts one, for any conn game `G` whose feed is TGE's:
/// the engine's protocol when the engine speaks it, the engine's channels, plugin classes and per-ghost blocks, and the
/// plugin shapes the world draws.
///
/// # Errors
/// The feed could not start on the connection's link.
pub fn start_feed<G: ConnGame<Feed = TgeFeed>>(
    vm: &mut Vm,
    connect: Connect,
) -> Result<Started<G>, String> {
    let local = connect.local.map(|a| (a.protocol, a.ghost_bits));
    let reader = crate::game::installed_game(vm).and_then(|g| g.file_reader());
    let hello = vm.extension::<ConnectHello>();
    let mut feed = match hello.filter(|h| local.is_none() && h.engine_mode()) {
        Some(hello) => {
            vm.set_extension(Rc::new(torque_engine::net::ConnectTarget(
                connect.server.into(),
            )));
            let start = EngineStart {
                server: connect.server.into(),
                nonce: engine_nonce(vm.extension::<EngineNonceKey>().as_deref(), connect.seq),
                source: Some(hello.source(vm)),
                password: connect.password,
                args: connect.args,
                kind: connect.link.kind(),
            };
            TgeFeed::over_engine(
                connect.link,
                start,
                vm.extension::<RpcLanes>(),
                client_rates(vm),
            )?
        }
        None => TgeFeed::with_rates(
            connect.link,
            &connect.password,
            connect.args,
            connect.seq,
            local,
            client_rates(vm),
        )?,
    };
    // A local connection's server half speaks the engine's protocol when the engine does: its event layer derives the
    // engine's event table, so the feed reads and writes the same one.
    if local.is_some()
        && vm
            .extension::<ConnectHello>()
            .is_some_and(|h| h.engine_mode())
    {
        feed.use_engine_tables()?;
    }
    if local.is_none()
        && feed.engine.is_none()
        && let Some(hello) = vm.extension::<ConnectHello>()
    {
        vm.set_extension(Rc::new(torque_engine::net::ConnectTarget(
            connect.server.into(),
        )));
        feed.set_connect_source(connect.server.into(), hello.source(vm));
    }
    feed.set_reader(reader.clone());
    feed.set_channels(vm.extension::<ClientChannels>());
    if let Some(plugins) = torque_server::net::codecs::plugin_classes(vm) {
        feed.set_plugins((*plugins).clone());
    }
    if let Some(hello) = vm.extension::<ConnectHello>() {
        feed.set_blocks(hello.blocks());
    }
    // The plugin ghost classes the renderer draws as shapes.
    let mut world = TgeWorld::default();
    if let Some(drawn) = vm.extension::<DrawnShapes>() {
        world.records.plugin_shapes = (*drawn).clone();
    }
    if let Some(stills) = vm.extension::<PluginStills>() {
        world.records.stills = (*stills).clone();
    }
    Ok(Started {
        feed,
        world,
        part: Box::new(super::NoPart),
        reader,
    })
}

/// Torque's connection half: [`TgeFeed`] into [`TgeWorld`], Torque's answers to the rest.
#[derive(Debug, Default)]
pub struct TgeConn;

/// What runs at the end of each tick of TGE's connection ([`ConnGame::ticked`]), when a host sets it as the script
/// engine's extension: the world's sounds on the host's mixer, which this crate has none of.
pub struct TgeTicked(pub fn(&mut Vm, &Rc<RefCell<ClientState<TgeConn>>>));

impl ConnGame for TgeConn {
    type Feed = TgeFeed;
    type Parts = TgeParts;

    fn ticked(vm: &mut Vm, client: &Rc<RefCell<ClientState<TgeConn>>>) {
        if let Some(hook) = vm.extension::<TgeTicked>() {
            (hook.0)(vm, client);
        }
    }

    const HANDSHAKE_WAIT_MS: u32 = TGE_HANDSHAKE_WAIT_MS;
    const HANDSHAKE_TRIES: u32 = TGE_HANDSHAKE_TRIES;

    fn start(
        vm: &mut Vm,
        client: &Rc<RefCell<ClientState<TgeConn>>>,
        connect: Connect,
    ) -> Result<Started<TgeConn>, String> {
        let _ = client;
        start_feed(vm, connect)
    }

    fn frame(game: &GameSlot, world: &mut WorldOf<TgeConn>) {
        let _ = (game, world);
    }

    fn register_natives(vm: &mut Vm, client: &Rc<RefCell<ClientState<TgeConn>>>, gui: &gui::Gui) {
        let _ = gui;
        vm.set_extension(Rc::new(ClientGhostLookup(Rc::new(GhostPlaces(Rc::clone(
            client,
        ))))));
    }

    fn world_left(st: &mut ClientState<TgeConn>, world: &mut TgeWorld) {
        // The plugin ghosts the handlers were told of go with the world: they hear it at the next tick.
        let seen = std::mem::take(&mut world.plugin_seen);
        let gone = &mut st.parts.gone;
        if gone.len() < seen.len() {
            gone.resize_with(seen.len(), Vec::new);
        }
        for (k, seen) in seen.into_iter().enumerate() {
            gone[k].extend(seen.into_keys());
        }
    }

    fn tick_detached(vm: &mut Vm, client: &Rc<RefCell<ClientState<TgeConn>>>) {
        flush_gone_plugin_ghosts(vm, client);
    }

    fn tick(vm: &mut Vm, client: &Rc<RefCell<ClientState<TgeConn>>>) {
        flush_gone_plugin_ghosts(vm, client);
        deliver_plugin_ghosts(vm, client);
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

/// Where the connection's ghosts are, for the plugins ([`ClientGhosts`]): a scene-hierarchy child where its parents' poses put it
/// ([`ClientWorld::world_pose`]), any other ghost where the client draws it or its record puts it. A parent's node counts as the parent's
/// origin: the connection holds no shapes to pose.
struct GhostPlaces(Rc<RefCell<ClientState<TgeConn>>>);

impl ClientGhosts for GhostPlaces {
    fn position(&self, index: u32) -> Option<[f32; 3]> {
        if let Some(m) = self.pose(index) {
            return Some(collide::math::translation(&m));
        }
        let st = self.0.try_borrow().ok()?;
        st.conn
            .conn
            .as_ref()?
            .world
            .records
            .ghosts
            .get(&index)?
            .position
    }

    fn pose(&self, index: u32) -> Option<collide::math::Matrix> {
        let st = self.0.try_borrow().ok()?;
        let records = &st.conn.conn.as_ref()?.world.records;
        records.world_pose(index, &|_, _| None).map(|(m, _)| m)
    }

    fn tick_pose(&self, index: u32) -> Option<collide::math::Matrix> {
        let st = self.0.try_borrow().ok()?;
        let records = &st.conn.conn.as_ref()?.world.records;
        records
            .tick_pose(index, &|_, _| None, &|_| None)
            .map(|(m, _)| m)
    }

    fn parent(&self, index: u32) -> Option<u32> {
        let st = self.0.try_borrow().ok()?;
        let records = &st.conn.conn.as_ref()?.world.records;
        Some(records.links.parent(index)?.object.0)
    }

    fn has_children(&self, index: u32) -> bool {
        let Ok(st) = self.0.try_borrow() else {
            return false;
        };
        st.conn
            .conn
            .as_ref()
            .is_some_and(|c| c.world.records.links.children(index).next().is_some())
    }
}

/// What the client keeps of the plugin ghosts of a connection that is over, until its handlers have heard that they are gone.
#[derive(Debug, Default)]
pub struct TgeParts {
    /// For each handler, the indexes of the ghosts it was told of.
    pub gone: Vec<Vec<u32>>,
}

/// Tells each handler that the ghosts of a connection that ended are gone.
fn flush_gone_plugin_ghosts(vm: &mut Vm, client: &Rc<RefCell<ClientState<TgeConn>>>) {
    if client.borrow().parts.gone.iter().all(Vec::is_empty) {
        return;
    }
    let gone = std::mem::take(&mut client.borrow_mut().parts.gone);
    let Some(sinks) = vm.extension::<GhostSinks>() else {
        return;
    };
    for (sink, indexes) in sinks.0.iter().zip(gone) {
        for index in indexes {
            sink.run(vm, RawGhostEvent::Removed { index });
        }
    }
}

/// One thing that happened to a plugin ghost, with its record when it has one.
enum Heard {
    Added(u32, Box<dyn ghosts::plugin::Record>),
    Changed(u32, Box<dyn ghosts::plugin::Record>),
    Removed(u32),
}

/// Whether a ghost is one of a plugin class's with a record of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Presence {
    Present,
    Absent,
}

/// What the handler `k` of a plugin class has heard of its ghosts (`seen`), against the world's.
struct PluginHearing<'a> {
    seen: &'a mut HashMap<u32, (u64, u32)>,
    records: &'a ClientWorld<TgeClientSet>,
    class: TgeClass,
    k: usize,
}

impl PluginHearing<'_> {
    /// What the handler is to hear of the ghost `index`, if it is a ghost of the class: it is added, replaced (made again on its
    /// index) or changed (an update reached it), or there is nothing new.
    fn hear(&mut self, index: u32, heard: &mut Vec<(usize, Heard)>) -> Presence {
        let k = self.k;
        let Some(ghost) = self
            .records
            .ghosts
            .get(&index)
            .filter(|g| g.class == Some(self.class))
        else {
            return Presence::Absent;
        };
        let Some(TgeRecord::Plugin(_, record)) = &ghost.record else {
            return Presence::Absent;
        };
        let birth = self.records.birth_order.get(&index).copied().unwrap_or(0);
        match self.seen.get(&index) {
            None => heard.push((k, Heard::Added(index, record.clone()))),
            Some(&(was, _)) if was != birth => {
                heard.push((k, Heard::Removed(index)));
                heard.push((k, Heard::Added(index, record.clone())));
            }
            Some(&(_, updates)) if updates != ghost.updates => {
                heard.push((k, Heard::Changed(index, record.clone())));
            }
            Some(_) => return Presence::Present,
        }
        self.seen.insert(index, (birth, ghost.updates));
        Presence::Present
    }
}

/// Tells each plugin handler of a ghost class ([`torque_engine::net_classes::GhostSinks`]) which ghosts of its class appeared,
/// changed (an update reached them) or went since the last tick. The handlers run with no borrow of the client held.
fn deliver_plugin_ghosts(vm: &mut Vm, client: &Rc<RefCell<ClientState<TgeConn>>>) {
    let Some(sinks) = vm.extension::<GhostSinks>() else {
        return;
    };
    let Some(plugins) = torque_server::net::codecs::plugin_classes(vm) else {
        return;
    };
    let mut heard: Vec<(usize, Heard)> = Vec::new();
    {
        let mut st = client.borrow_mut();
        let Some(conn) = st.conn.conn.as_mut() else {
            return;
        };
        let world = &mut conn.world;
        world.plugin_seen.resize_with(sinks.0.len(), HashMap::new);
        // Only the ghosts the applied sections changed can have news, unless the changes were too many to list.
        let touched = match world.records.take_touched() {
            TouchedGhosts::Listed(mut changed) => {
                changed.sort_unstable();
                changed.dedup();
                Some(changed)
            }
            TouchedGhosts::All => None,
        };
        for (k, sink) in sinks.0.iter().enumerate() {
            let Some(class) = plugins.ghost(sink.class()) else {
                continue;
            };
            let class = TgeClass::Plugin(class);
            let mut hearing = PluginHearing {
                seen: &mut world.plugin_seen[k],
                records: &world.records,
                class,
                k,
            };
            match &touched {
                None => {
                    let mut present = HashSet::new();
                    for (index, _) in world.records.of_class(class) {
                        if hearing.hear(index, &mut heard) == Presence::Present {
                            present.insert(index);
                        }
                    }
                    let gone: Vec<u32> = hearing
                        .seen
                        .keys()
                        .copied()
                        .filter(|index| !present.contains(index))
                        .collect();
                    for index in gone {
                        hearing.seen.remove(&index);
                        heard.push((k, Heard::Removed(index)));
                    }
                }
                Some(changed) => {
                    for &index in changed {
                        if hearing.hear(index, &mut heard) == Presence::Absent
                            && hearing.seen.remove(&index).is_some()
                        {
                            heard.push((k, Heard::Removed(index)));
                        }
                    }
                }
            }
        }
    }
    for (k, event) in heard {
        let sink = &sinks.0[k];
        match &event {
            Heard::Added(index, record) => sink.run(
                vm,
                RawGhostEvent::Added {
                    index: *index,
                    record: &**record,
                },
            ),
            Heard::Changed(index, record) => sink.run(
                vm,
                RawGhostEvent::Changed {
                    index: *index,
                    record: &**record,
                },
            ),
            Heard::Removed(index) => sink.run(vm, RawGhostEvent::Removed { index: *index }),
        }
    }
}

#[cfg(test)]
#[path = "tge_tests.rs"]
mod tests;
