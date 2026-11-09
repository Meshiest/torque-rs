//! The codecs of a game's network protocol: what the network host writes and reads that a game sets ([`NetCodecs`], a `Vm`
//! extension). The host reaches the handshake's datagram types and server settings, the ghost-id width, the class tables, the
//! event registry, the server query's answers, the round trip of an infinitely lagging connection, the control section's
//! format, the connection messages' numbers, the address text and a few console switches only through it. Without a game's,
//! they are Torque's profiles from `net` ([`TorqueCodecs`]).
//!
//! Credit: [`TorqueCodecs::sim_mask`] follows the mask enums of TGE 1.4's `game/shapeBase.h`, `game/gameBase.h`,
//! `game/player.h`, `game/item.h`, `game/camera.h` and `game/staticShape.h` (each Copyright (C) GarageGames.com, Inc.);
//! Torque3D's enums differ. A Player control object's packet data ([`player_packet_data`]) follows TGE 1.4's
//! `Player::writePacketData` in `game/player.cc` (Copyright (C) GarageGames.com, Inc.), whose MIT-licensed successor is
//! Torque3D's `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is
//! this crate's own.

use std::any::Any;
use std::fmt::Debug;
use std::net::{IpAddr, SocketAddr};
use std::rc::Rc;
use std::sync::OnceLock;

use torque_game::dbvalues::DbValues;
use torque_ghosts::TgePacketData;
use torque_ghosts::codec::F32;
use torque_ghosts::codec::NetWidths;
use torque_ghosts::datablocks::{
    DatablockCodecSet, ScriptField, TgeDatablockClass, TgeDatablockRecord, TgeDatablocks,
};
use torque_ghosts::light::ENGINE_OBJECT_CLASSES;
use torque_ghosts::plugin::{PluginClass, PluginClasses, Record};
use torque_ghosts::set::GhostCodecSet;
use torque_ghosts::tails::{PlayerTail, PlayerTailMotion, ShapeTail, TailData, WheeledTail};
use torque_ghosts::tge::{TGE_DATABLOCK_CLASSES, TgeClass, TgeGhosts, tge_reps};
use torque_ghosts::{ObjectView, PackCtx, Value};
use torque_net::bits::BitWriter;
use torque_net::classes::{Direction, Kind, NetClasses};
use torque_net::control::{ControlData, ControlObject, ControlState, Damage};
use torque_net::events::{EventIds, EventRegistry};
use torque_net::handshake::{HandshakeTypes, ServerConfig};
use torque_net::huffman::Table;
use torque_net::profile::{
    DEFAULT_DATABLOCK_BITS, DEFAULT_GHOST_LIMIT, HandshakeProfile, QueryProfile,
};
use torque_net::protocol::RateConfig;
use torque_net::serverquery::Request;
use torque_sim::player::PlayerState;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::datablock::{DatablockRecord, DatablockView, DbPackError, DbValue};

use super::transport::LanRules;

/// The host's preferences a server's handshake settings take ([`NetCodecs::configure_server`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerPrefs {
    /// `$Server::LAN`.
    pub lan: bool,
    /// The packets a second the server sends a client.
    pub to_client_rate: u32,
    /// The ghost-id width a client of the engine's own is given when it is wider than the server's (none: the server's).
    pub engine_ghost_bits: Option<u8>,
    /// How many connections may hold a ghost table wider than the server's, and their slots in all.
    pub wide_clients: usize,
    pub wide_slots: u64,
}

/// A ghostable class as the ghost registry keeps it ([`NetCodecs::ghost_class`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GhostClassInfo {
    /// The codecs' own number of the class (what the registry hands back to them).
    pub code: u16,
    /// The class id of the object group.
    pub class_id: u32,
    /// `mNetFlags`: Ghostable, and ScopeAlways for the classes whose constructor sets it.
    pub flags: u32,
    /// The class is the Sky (its visible distance sizes the scope).
    pub sky: bool,
}

/// A game's network codecs. Every method has Torque's answer by default.
pub trait NetCodecs: Debug {
    // ---- the handshake

    /// The handshake's datagram types.
    fn handshake_types(&self) -> HandshakeTypes {
        HandshakeTypes::TORQUE
    }

    /// A server's handshake settings before the host's preferences.
    fn server_config(&self, max_players: usize) -> ServerConfig {
        ServerConfig::new(max_players)
    }

    /// The host's preferences into a server's settings: the rates `checkMaxRate` gives (the to-server preference 32), and
    /// whatever the profile's extension takes.
    fn configure_server(&self, config: &mut ServerConfig, prefs: &ServerPrefs) {
        config.rates =
            RateConfig::new(32, prefs.to_client_rate, RateConfig::standard().packet_size);
    }

    /// The wide ghost tables' budget when the host's preferences say nothing: (connections, slots in all).
    fn wide_defaults(&self) -> (usize, u64) {
        (0, 0)
    }

    /// Which senders count as on the LAN, and what the transport's bind policy and broadcast listener take.
    fn lan_rules(&self) -> &'static LanRules {
        &LanRules::PLAIN
    }

    /// The text of a client's address (`getAddress`, `onConnectRequest`'s address argument): `Net::addressToString`'s
    /// `IP:a.b.c.d:port` (`IP6:[address]:port` for IPv6) by default.
    fn address_text(&self, addr: SocketAddr) -> String {
        match addr {
            SocketAddr::V4(_) => format!("IP:{addr}"),
            SocketAddr::V6(_) => format!("IP6:{addr}"),
        }
    }

    /// The address text of the server half of a local connection once its client half is gone, and the address argument
    /// its `onConnectRequest` gets: the text of the local connection's pseudo-address by default.
    fn local_address_text(&self) -> String {
        self.address_text(super::local::LOCAL_ADDR)
    }

    // ---- ghost ids and the class tables

    /// The ghost-id width of a ghost table of `limit` slots.
    fn ghost_id_bits(&self, limit: u32) -> u8 {
        HandshakeProfile::default().ghost_bits(limit)
    }

    /// The ghost limit a server starts with.
    fn start_ghost_limit(&self) -> u32 {
        DEFAULT_GHOST_LIMIT
    }

    /// The networked object classes with their ids: by default TGE 1.4's object and datablock classes and Torque's events
    /// (`torque-ghosts`' lists).
    fn object_classes(&self) -> &NetClasses {
        static TGE: OnceLock<NetClasses> = OnceLock::new();
        TGE.get_or_init(|| {
            NetClasses::derive(&tge_reps(&EventRegistry::torque())).expect("TGE's table derives")
        })
    }

    /// The ghostable class of an object of class `name` (none: the object is not ghosted). By default TGE 1.4's classes that
    /// `torque_ghosts`' TGE set has a record codec for (and the engine's own, `torque_ghosts::light`'s, past the table's ids), each numbered by its place in [`TgeClass::ALL`] and with the net flags its
    /// constructor chain sets ([`TgeClass::net_flags`]: a `Camera` is registered but not Ghostable).
    ///
    /// A plugin's class ([`plugin_classes`](Self::plugin_classes)) takes its id past those, from
    /// [`plugin_ghost_base`](Self::plugin_ghost_base) in its set's order, numbered [`PluginClasses::ghost_code`]; one whose id
    /// does not fit the class id width is not ghosted.
    fn ghost_class(&self, name: &str) -> Option<GhostClassInfo> {
        let Some(class) = TgeGhosts::class_by_name(name) else {
            return self.plugin_ghost_class(name);
        };
        let classes = self.object_classes();
        // The engine's own classes: the ids past the table's, in order (a connection's source shows them to engine peers only).
        let class_id = match ENGINE_OBJECT_CLASSES
            .iter()
            .position(|n| *n == class.name())
        {
            Some(k) => {
                let id = u32::try_from(classes.count(Kind::Object) + k).ok()?;
                (id < 1 << self.ghost_class_bits()).then_some(id)?
            }
            None => classes.id_of(Kind::Object, class.name())?,
        };
        Some(GhostClassInfo {
            code: class.code(),
            class_id,
            flags: class.net_flags(),
            sky: class == TgeClass::Sky,
        })
    }

    /// The ghostable class of an object of the plugin class `name` ([`ghost_class`](Self::ghost_class)'s plugin half).
    fn plugin_ghost_class(&self, name: &str) -> Option<GhostClassInfo> {
        let plugins = self.plugin_classes()?;
        let class = plugins.ghost(name)?;
        let class_id = plugins.id_of(Kind::Object, name, self.plugin_ghost_base())?;
        (u64::from(class_id) < 1u64 << self.ghost_class_bits()).then_some(())?;
        Some(GhostClassInfo {
            code: plugins.ghost_code(name)?,
            class_id,
            flags: class.net_flags(),
            sky: false,
        })
    }

    /// The plugins' ghost and datablock classes the codecs send (none by default; the engine's own codecs have the engine's,
    /// [`EngineCodecs`]). A game's own codecs that carry them answer the set its script engine was given
    /// ([`plugin_classes`](fn@plugin_classes)).
    fn plugin_classes(&self) -> Option<&PluginClasses> {
        None
    }

    /// The first object class id of the plugins' ghost classes: past the class table's objects and the engine's own classes.
    fn plugin_ghost_base(&self) -> u32 {
        PluginClasses::ghost_base(self.object_classes().count(Kind::Object))
    }

    /// The first datablock class id of the plugins' datablock classes: past the class table's datablocks.
    fn plugin_datablock_base(&self) -> u32 {
        PluginClasses::datablock_base(self.datablock_classes().len())
    }

    /// The dirty bits a class's own `setTransform` marks (0: the base `SceneObject::setTransform`, which marks nothing).
    fn transform_mask(&self, code: u16) -> u32 {
        let _ = code;
        0
    }

    /// The record mask of the bits a game class's simulation marks on an object of ghost class `code` (torque-sim's numbering
    /// of a class's own bits, which the game classes hand the ghost layer): the same bits by default.
    fn sim_mask(&self, code: u16, bits: u32) -> u32 {
        let _ = code;
        bits
    }

    /// The width of a class id in a ghost record.
    fn ghost_class_bits(&self) -> u8 {
        self.object_classes()
            .id_bits(torque_net::classes::Kind::Object)
    }

    /// The networked datablock classes' names: TGE 1.4's by default.
    fn datablock_classes(&self) -> Vec<&'static str> {
        TGE_DATABLOCK_CLASSES.to_vec()
    }

    // ---- events

    /// The event classes a connection knows.
    fn event_registry(&self) -> EventRegistry {
        EventRegistry::torque()
    }

    /// The event classes with their directions (the registry's, in registration order).
    fn event_classes(&self) -> Vec<(String, Direction)> {
        self.event_registry().classes().to_vec()
    }

    /// The event ids of a derived class table; `None` when a class the codecs need is missing.
    fn event_ids_of(&self, classes: &NetClasses) -> Option<EventIds> {
        EventIds::from_classes(classes).ok()
    }

    /// The event ids of [`event_registry`](Self::event_registry).
    fn event_ids(&self) -> Option<EventIds> {
        self.event_registry().ids().ok()
    }

    /// The width of a datablock id in the audio events.
    fn audio_id_bits(&self) -> u8 {
        DEFAULT_DATABLOCK_BITS
    }

    /// The width of a datablock id in a `SimDataBlockEvent`.
    fn datablock_id_bits(&self) -> u8 {
        self.audio_id_bits()
    }

    /// The networked datablock class of an object of class `name` as the class table names it (none: the object is not sent).
    /// By default TGE 1.4's classes with a record codec in `torque_ghosts`' [`TgeDatablocks`], then the plugins' datablock
    /// classes ([`plugin_classes`](Self::plugin_classes)).
    fn datablock_class(&self, name: &str) -> Option<&'static str> {
        TgeDatablocks::class_by_name(name)
            .map(TgeDatablockClass::name)
            .or_else(|| Some(self.plugin_classes()?.datablock(name)?.name()))
    }

    /// The record of datablock `id` of `class` (its `packData`) written from the host's view of it; `values` are the
    /// datablocks' computed values. None: the class has no writer, and the event goes out as the one 0 bit. By default the
    /// class's record of [`TgeDatablocks`], in the widths of a server's start ([`TgeDbRecord`]); a plugin's class's record
    /// ([`PluginDbRecord`]) with its class id past the table's.
    fn datablock_record(
        &self,
        class: &str,
        id: ObjId,
        view: &dyn DatablockView,
        values: &DbValues,
    ) -> Option<Rc<dyn DatablockRecord>> {
        let _ = (id, values);
        let widths = NetWidths::new(
            self.ghost_id_bits(self.start_ghost_limit()),
            self.datablock_id_bits(),
        );
        let Some(class) = TgeDatablocks::class_by_name(class) else {
            let plugins = self.plugin_classes()?;
            let plugin = plugins.datablock(class)?;
            let class_id = plugins.id_of(Kind::DataBlock, class, self.plugin_datablock_base())?;
            let (record, _) = plugin.pack(&PluginDbView(view), !0, &PackCtx::new(widths));
            return Some(Rc::new(PluginDbRecord {
                class: plugin,
                class_id,
                record,
                widths,
            }));
        };
        let record = TgeDatablocks::pack(class, &TgeView { class, view }, widths);
        Some(Rc::new(TgeDbRecord { record, widths }))
    }

    /// What the pack of datablock `id` does to the host once its record went into an event (the record was rendered).
    fn datablock_packed(
        &self,
        vm: &mut Vm,
        values: &DbValues,
        id: ObjId,
        record: &dyn DatablockRecord,
    ) {
        let _ = (vm, values, id, record);
    }

    /// The command class variant a remote command is sent as (`secure`: a game's own command class, where it has one).
    fn command_variant(&self, secure: bool) -> u8 {
        let _ = secure;
        0
    }

    /// The prefix of the function a received command of `variant` calls (`on_server`: a client's command on a server
    /// connection). On a server `variant` is whatever byte the client sent (in the engine's protocol it is a field of the command's
    /// body, not a class id the event reader checked), so it is untrusted: an implementation answers for any value and never
    /// indexes with it.
    fn command_prefix(&self, variant: u8, on_server: bool) -> &'static [u8] {
        let _ = variant;
        torque_net::remote::function_prefix(on_server)
    }

    // ---- the server query

    /// The ping answer's version fields and the info types.
    fn query_profile(&self) -> QueryProfile {
        QueryProfile::default()
    }

    /// Whether the info handler gets a datagram whose first byte is `ty`.
    fn is_info_type(&self, ty: u8) -> bool {
        self.query_profile().is_info_type(ty)
    }

    /// The ping answer.
    fn ping_response(&self, table: &Table, request: &Request, name: &[u8]) -> Vec<u8> {
        torque_net::serverquery::ping_response(table, request, name, &self.query_profile())
    }

    /// The CPU field of the info answer when the processor was not measured.
    fn unknown_cpu_mhz(&self) -> u16 {
        0
    }

    /// Whether a request type outside Torque's query is the game's own (none by default).
    fn extra_query(&self, ty: u8) -> bool {
        let _ = ty;
        false
    }

    /// Whether the server answers a request of the game's own type `ty` with `flags`.
    fn answers_extra_query(&self, ty: u8, flags: u8, allow: bool, single_player: bool) -> bool {
        let _ = (ty, flags, allow, single_player);
        false
    }

    /// The answer to a request of the game's own type `ty`.
    fn extra_query_response(&self, ty: u8, flags: u8, key: u32) -> Vec<u8> {
        let _ = (ty, flags, key);
        Vec::new()
    }

    // ---- the connection messages

    /// The `ConnectionMessage` the server sends when a datablock chain ends: Torque's `DataBlocksDone` (6) by default.
    fn datablocks_done_message(&self) -> u8 {
        DATABLOCKS_DONE
    }

    /// The `ConnectionMessage` whose sequence, from a client, runs `onDataBlocksDone`: Torque's `DataBlocksDownloadDone` (7)
    /// by default.
    fn datablocks_download_done_message(&self) -> u8 {
        DATABLOCKS_DOWNLOAD_DONE
    }

    /// Whether a `ConnectionMessage` from a client is a protocol error (`Invalid packet.`: a server connection is not ghosting
    /// to the client). By default the messages only a ghosting side sends: `GhostAlwaysDone` 0, `EndGhosting` 2,
    /// `GhostAlwaysStarting` 3, `SendNextDownloadRequest` 4 and `FileDownloadSizeMessage` 5.
    fn rejects_client_message(&self, message: u8) -> bool {
        matches!(message, 0 | 2..=5)
    }

    // ---- console lines

    /// Whether a NetString a client sent prints its `Mapping string: ... to index: ...` line: always by default.
    fn shows_string_mappings(&self, vm: &Vm) -> bool {
        let _ = vm;
        true
    }

    // ---- the packets and the control section

    /// The datablock events a connection's chain keeps queued or unacked (`peer`: the client is one of the engine's own, the
    /// handshake's extension): Torque's `DataBlockQueueCount` by default.
    fn datablock_window(&self, peer: bool) -> usize {
        let _ = peer;
        super::datablocks::DATABLOCK_QUEUE_COUNT
    }

    /// How a datablock record's matrix fields become the transforms it writes ([`tge_affine`](super::datablocks::tge_affine)
    /// by default).
    fn affine_fn(&self) -> super::datablocks::AffineFn {
        super::datablocks::tge_affine
    }

    /// The round trip (ms) above which the host reports the connection as lagging (none: never).
    fn infinite_lag_ms(&self) -> Option<f32> {
        None
    }

    /// A new connection's control state (the moves' format, the control section's).
    fn control_state(&self) -> ControlState {
        ControlState::new()
    }

    /// The bits of a server-to-client head with nothing set.
    fn head_min_bits(&self) -> usize {
        torque_net::control::HEAD_MIN_BITS
    }

    /// The connection's control object `obj` as the head writer sees it (its ghost index, damage flash and packet data), with
    /// `ghost_index` the connection's ghost index of an object. None by default: the head carries no control object.
    fn control_object<'a>(
        &self,
        vm: &'a Vm,
        obj: ObjId,
        ghost_index: &'a dyn Fn(ObjId) -> Option<u32>,
    ) -> Option<Box<dyn ControlObject + 'a>> {
        let _ = (vm, obj, ghost_index);
        None
    }

    /// How many objects the control object's chain (an object a control object controls in turn) may hold.
    fn control_chain(&self) -> usize {
        1
    }

    /// The dirty bits an object is marked with when its controlling client changes: GameBase's `ControlMask` by default.
    fn controlling_client_mask(&self) -> u32 {
        torque_ghosts::parts::mask::CONTROL
    }
}

/// Torque's `DataBlocksDone` (`NumConnectionMessages`, the first id past the ghost and file-download messages).
pub const DATABLOCKS_DONE: u8 = 6;
/// Torque's `DataBlocksDownloadDone`.
pub const DATABLOCKS_DOWNLOAD_DONE: u8 = 7;

/// Torque's codecs.
#[derive(Debug, Default)]
pub struct TorqueCodecs;

/// Torque's codecs with the plugins' ghost and datablock classes ([`NetCodecs::plugin_classes`]): what [`net_codecs`] gives a
/// script engine whose engine has plugin net classes (the set its extension) and no game's own codecs.
#[derive(Debug, Default)]
pub struct EngineCodecs {
    pub plugins: Rc<PluginClasses>,
}

impl NetCodecs for EngineCodecs {
    fn plugin_classes(&self) -> Option<&PluginClasses> {
        Some(&self.plugins)
    }

    fn sim_mask(&self, code: u16, bits: u32) -> u32 {
        TorqueCodecs.sim_mask(code, bits)
    }

    fn control_object<'a>(
        &self,
        vm: &'a Vm,
        obj: ObjId,
        ghost_index: &'a dyn Fn(ObjId) -> Option<u32>,
    ) -> Option<Box<dyn ControlObject + 'a>> {
        TorqueCodecs.control_object(vm, obj, ghost_index)
    }
}

/// A record of [`TgeDatablocks`] as the host carries it, with the widths it was packed in. Its render is charged as one entry.
#[derive(Debug)]
pub struct TgeDbRecord {
    pub record: TgeDatablockRecord,
    pub widths: NetWidths,
}

impl DatablockRecord for TgeDbRecord {
    fn encode(&self, w: &mut BitWriter, table: &Table) -> Result<(), DbPackError> {
        TgeDatablocks::encode(&self.record, w, table, self.widths)
            .map_err(|_| DbPackError::Malformed("a TGE datablock record"))
    }

    fn entries(&self) -> usize {
        1
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A record of a plugin's datablock class as the host carries it: its class id past the table's, and the widths it was
/// packed in. Its render is charged as one entry per script field.
#[derive(Debug)]
pub struct PluginDbRecord {
    pub class: PluginClass,
    pub class_id: u32,
    pub record: Box<dyn Record>,
    pub widths: NetWidths,
}

impl DatablockRecord for PluginDbRecord {
    fn encode(&self, w: &mut BitWriter, table: &Table) -> Result<(), DbPackError> {
        self.record
            .encode(w, table, self.widths)
            .map_err(|_| DbPackError::Malformed("a plugin's datablock record"))
    }

    fn entries(&self) -> usize {
        self.record.record_fields().len().max(1)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn class_id(&self) -> Option<u32> {
        Some(self.class_id)
    }
}

/// The host's view of a plugin's datablock: each field by its own name.
struct PluginDbView<'a>(&'a dyn DatablockView);

impl ObjectView for PluginDbView<'_> {
    fn get(&self, member: &str, index: usize) -> Value {
        db_value(self.0.get(member, index))
    }
}

/// A host's datablock value as a packer reads it.
fn db_value(value: DbValue) -> Value {
    match value {
        DbValue::Null => Value::Null,
        DbValue::Bool(b) => Value::Bool(b),
        DbValue::Int(i) => Value::Int(i),
        DbValue::F32(f) => Value::F32(f),
        DbValue::Str(s) => Value::Str(s),
        DbValue::Id(id) => Value::Id(id),
        DbValue::Xform(a) => Value::xform(a.as_ref()),
    }
}

/// The host's view of a datablock (by script field) as `torque_ghosts`' packers read it (by member,
/// [`TgeDatablockClass::script_field`]).
struct TgeView<'a> {
    class: TgeDatablockClass,
    view: &'a dyn DatablockView,
}

impl ObjectView for TgeView<'_> {
    fn get(&self, member: &str, index: usize) -> Value {
        db_value(match self.class.script_field(member, index) {
            ScriptField::Own => self.view.get(member, index),
            ScriptField::Field(field, index) => self.view.get(field, index),
            ScriptField::Absent => DbValue::Null,
        })
    }
}

impl NetCodecs for TorqueCodecs {
    /// The game classes number a ShapeBase's bits from bit 4 (`setShapeName` marks 0x10, the damage 0x20, a mount 0x80,
    /// the fade 0x100, the sounds, threads and images from 0x1000, 0x10000 and 0x10_0000; a Player's, an Item's and a
    /// Camera's own bits from 24). TGE 1.4's records put each one bit higher, past GameBase's `ControlMask` (bit 4):
    /// `NameMask` 5, `DamageMask` 6, `MountedMask` 8, `CloakMask` 9, the sounds from 13, the threads from 17, the images
    /// from 21, a Player's `ActionMask` 25 and `MoveMask` 26, an Item's `HiddenMask` 25 to `RotationMask` 28, a Camera's
    /// `MoveMask` 25. The scale bit (0) is the same, and so is every bit of a class without a ShapeBase part.
    ///
    /// A vehicle's step numbers its own bits over the low ShapeBase ones (`torque_vehicle::state::net_mask`): its pose (0x10)
    /// and a `setTransform` (0x40) are TGE's `PositionMask` (bit 25), its energy (0x20) `EnergyMask` (26) with the damage, its
    /// wheels' datablocks (0x80) `WheelMask` (27); the ShapeBase bits from 0x100 move one higher as a Player's.
    fn sim_mask(&self, code: u16, bits: u32) -> u32 {
        match TgeClass::of_code(code) {
            Some(
                TgeClass::Player
                | TgeClass::AiPlayer
                | TgeClass::Item
                | TgeClass::StaticShape
                | TgeClass::ScopeAlwaysShape
                | TgeClass::Camera,
            ) => (bits & 0x0000_000f) | ((bits & 0x7fff_fff0) << 1),
            Some(
                TgeClass::WheeledVehicle
                | TgeClass::AiWheeledVehicle
                | TgeClass::FlyingVehicle
                | TgeClass::HoverVehicle,
            ) => {
                use torque_ghosts::vehicle::mask::{ENERGY, POSITION, WHEEL};
                use torque_vehicle::state::net_mask;
                let mut out = (bits & 0x0000_000f) | ((bits & 0x00ff_ff00) << 1);
                if bits & (net_mask::POSE | net_mask::WARP) != 0 {
                    out |= POSITION;
                }
                if bits & net_mask::ENERGY != 0 {
                    out |= ENERGY | torque_ghosts::shapebase::mask::DAMAGE;
                }
                if bits & net_mask::WHEELS != 0 {
                    out |= WHEEL;
                }
                out
            }
            _ => bits,
        }
    }

    /// A simulated Player as TGE's head writes it: its ghost index, its damage flash and white-out, and `Player::writePacketData`
    /// ([`player_packet_data`]); any other object writes no control section.
    fn control_object<'a>(
        &self,
        vm: &'a Vm,
        obj: ObjId,
        ghost_index: &'a dyn Fn(ObjId) -> Option<u32>,
    ) -> Option<Box<dyn ControlObject + 'a>> {
        torque_game::sim::player::state(vm, obj)?;
        Some(Box::new(PlayerControl {
            vm,
            obj,
            ghost_index,
        }))
    }
}

/// A simulated Player as the connection's control object ([`TorqueCodecs::control_object`]).
struct PlayerControl<'a> {
    vm: &'a Vm,
    obj: ObjId,
    ghost_index: &'a dyn Fn(ObjId) -> Option<u32>,
}

impl ControlObject for PlayerControl<'_> {
    fn ghost_index(&self) -> Option<u32> {
        (self.ghost_index)(self.obj)
    }

    fn damage(&self) -> Damage {
        torque_game::sim::player::state(self.vm, self.obj).map_or_else(Damage::default, |s| {
            Damage::from_levels(s.damage_flash, s.white_out)
        })
    }

    /// `Player::writePacketData`; a Player that controls a simulated WheeledVehicle (its driver) names the vehicle's ghost
    /// and the vehicle's own packet data follows (`WheeledVehicle::writePacketData`, [`WheeledTail::pack`]).
    fn packet_data(&self) -> Box<dyn ControlData> {
        let s = torque_game::sim::player::state(self.vm, self.obj);
        let mut tail = s.map_or_else(PlayerTail::default, player_packet_data);
        let driven = s
            .and_then(|s| s.control_object)
            .map(|o| o.0)
            .filter(|&o| torque_game::sim::vehicle_methods::wheeled_mount(self.vm, o).is_some())
            .and_then(|o| Some((o, (self.ghost_index)(o)?)));
        let nested = driven.map(|(o, index)| {
            tail.control = Some(index);
            let view = super::torque::GameView {
                fields: super::torque::FieldView {
                    vm: self.vm,
                    obj: o,
                },
                controlled: true,
                ghost_of: None,
                connection: None,
            };
            Box::new(TgePacketData::new(TailData::Wheeled(WheeledTail::pack(
                &view,
            )))) as Box<dyn ControlData>
        });
        let mut data = TgePacketData::new(TailData::Player(tail));
        data.nested = nested;
        Box::new(data)
    }
}

/// `Player::writePacketData` of a simulated Player ([`torque_sim::player::Player::packet_words`]'s words): the energy, the
/// position, velocity and last contact when it is not mounted, the head's x and z and the yaw. The record's other fields (the
/// recharge rate, the state, the recover ticks, the jump delay) are none of those words: they are written as the engine's
/// client writes the packet data it takes its checksum of (0, none), so a client that predicted the Player right matches.
/// The index of an object the Player controls is the host's to add ([`TorqueCodecs::control_object`]: a driven WheeledVehicle).
pub fn player_packet_data(s: &PlayerState) -> PlayerTail {
    PlayerTail {
        shape: ShapeTail {
            energy: F32::new(s.energy),
            recharge_rate: F32::new(0.0),
        },
        motion: s.mount.is_none().then(|| PlayerTailMotion {
            position: s.position().map(F32::new),
            velocity: s.velocity.map(F32::new),
            last_contact: s.jump_surface_last.min(15),
        }),
        head: [F32::new(s.head[0]), F32::new(s.head[2])],
        rotation_z: F32::new(s.rot[2]),
        ..PlayerTail::default()
    }
}

struct Installed(Rc<dyn NetCodecs>);

/// The game's codecs for `vm` from now on.
pub fn set_net_codecs(vm: &mut Vm, codecs: Rc<dyn NetCodecs>) {
    vm.set_extension(Rc::new(Installed(codecs)));
}

/// The codecs `vm`'s game installed, else Torque's: with the plugins' net classes ([`EngineCodecs`]) when the script engine
/// has a set of them ([`plugin_classes`]).
pub fn net_codecs(vm: &Vm) -> Rc<dyn NetCodecs> {
    if let Some(installed) = vm.extension::<Installed>() {
        return Rc::clone(&installed.0);
    }
    match plugin_classes(vm).filter(|p| !p.is_empty()) {
        Some(plugins) => Rc::new(EngineCodecs { plugins }),
        None => Rc::new(TorqueCodecs),
    }
}

/// The plugins' ghost and datablock classes of `vm` (its extension; an engine's plugins give every script engine theirs).
pub fn plugin_classes(vm: &Vm) -> Option<Rc<PluginClasses>> {
    vm.extension::<PluginClasses>()
}

impl LanRules {
    /// Whether a sender at `ip` is on the LAN (`local`: the in-process connection always is). An IPv4-mapped IPv6 address is
    /// its IPv4 address; any other IPv6 address is not on the LAN.
    pub fn is_lan(&self, ip: IpAddr, local: bool) -> bool {
        if local {
            return true;
        }
        match ip {
            IpAddr::V4(v4) => (self.lan_v4)(v4),
            IpAddr::V6(v6) => v6.to_ipv4_mapped().is_some_and(self.lan_v4),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torque_codecs_answer_with_torques_profiles() {
        let c = TorqueCodecs;
        assert_eq!(c.handshake_types(), HandshakeTypes::TORQUE);
        assert_eq!(c.ghost_id_bits(DEFAULT_GHOST_LIMIT), 12);
        assert_eq!(c.audio_id_bits(), DEFAULT_DATABLOCK_BITS);
        assert_eq!(c.event_ids(), EventIds::torque());
        assert_eq!(
            c.object_classes().total(),
            torque_ghosts::tge::TGE_OBJECT_CLASSES.len()
                + TGE_DATABLOCK_CLASSES.len()
                + EventRegistry::torque().classes().len()
        );
        assert_eq!(c.datablock_classes().len(), TGE_DATABLOCK_CLASSES.len());
        assert!(!c.extra_query(22));
        assert_eq!(c.infinite_lag_ms(), None);
        assert_eq!(
            c.address_text("10.0.0.2:28000".parse().unwrap()),
            "IP:10.0.0.2:28000"
        );
        assert_eq!(c.local_address_text(), "IP:0.0.0.0:0");
        assert_eq!(
            (
                c.datablocks_done_message(),
                c.datablocks_download_done_message()
            ),
            (6, 7)
        );
        assert_eq!(
            (0..8)
                .filter(|&m| c.rejects_client_message(m))
                .collect::<Vec<_>>(),
            [0, 2, 3, 4, 5]
        );
        assert_eq!(c.controlling_client_mask(), 1 << 4);
        assert!(c.lan_rules().is_lan("192.168.1.2".parse().unwrap(), false));
        assert!(!c.lan_rules().is_lan("25.1.2.3".parse().unwrap(), false));
        assert!(c.lan_rules().is_lan("8.8.8.8".parse().unwrap(), true));
    }
}
