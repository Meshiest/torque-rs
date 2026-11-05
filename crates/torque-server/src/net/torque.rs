//! Torque's net game ([`TorqueNet`]): the network host of a server that adds nothing of its own. No events past Torque's
//! ([`NoGameEvents`]), TGE's object classes ghosted with their record codecs ([`TgeGhostGame`]: the class of each object is
//! looked up by name in `torque_ghosts`' TGE set and its record packed from the object's static fields), Torque's string coder
//! table, and every rule of [`NetGame`] at its default. [`NetPasses`] is the network host the main loop drives
//! ([`NetHost`]) over any net game's shared state.
//!
//! Credit: the update priority follows `NetObject::getUpdatePriority` in Torque3D's MIT-licensed `sim/netObject.cpp`, and the
//! views of a player's arm and look action, a projectile and a shape's images follow `T3D/player.cpp`, `T3D/projectile.cpp` and
//! `T3D/shapeBase.cpp` (each `Copyright (c) 2012 GarageGames, LLC`, `Copyright (C) 2015 Faust Logic, Inc.`); `quat_of` follows
//! `QuatF::set(MatrixF)` in `math/mQuat.cpp` (`Copyright (c) 2012 GarageGames, LLC`); the Sky's script fields behind its record
//! (`sky_field`) are those of TGE 1.4's `terrain/sky.cc` (`Copyright (C) GarageGames.com, Inc.`; Torque3D has no Sky class).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use torque_ghosts::codec::NetWidths;
use torque_ghosts::light::ENGINE_OBJECT_CLASSES;
use torque_ghosts::set::GhostCodecSet;
use torque_ghosts::tge::{TgeClass, TgeGhosts};
use torque_ghosts::{Link, ObjectView, PackCtx, Value};
use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};
use torque_net::events::{ChargeFn, EventCtx, EventIds, WireEvent};
use torque_net::ghost::{CamInfo, ObjectSource, TorqueGhosts};
use torque_net::handshake::ConnId;
use torque_net::huffman::Table;
use torque_ts_vm::fields::Value as Field;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::ghosts::{Conns, GhostGame, GhostHub, Registry, SourceEnv};
use super::hostevent::{GameEvents, ReceiveCx};
use super::{NetGame, NetShared, pump};
use crate::server::{NetHost, Phase};
use torque_game::sim::ProcessHandle;
use torque_game::sim::pass::{Outcome, advance_objects_within};

/// Torque's net game: Torque's string coder table and nothing of a game's own.
#[derive(Debug)]
pub struct TorqueNet {
    table: Rc<Table>,
}

impl Default for TorqueNet {
    fn default() -> TorqueNet {
        TorqueNet {
            table: Rc::new(Table::torque()),
        }
    }
}

impl NetGame for TorqueNet {
    type Events = NoGameEvents;
    type Ghosts = TgeGhostGame;
    type Conn = ();

    fn string_table(&self) -> Rc<Table> {
        Rc::clone(&self.table)
    }
}

/// A game event of a game that has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoEvent {}

impl WireEvent for NoEvent {
    fn class(&self, _ids: &EventIds) -> u32 {
        match *self {}
    }

    fn pack(&self, _w: &mut BitWriter, _ctx: &EventCtx<'_>) {
        match *self {}
    }
}

/// The events of a game that adds none to Torque's.
#[derive(Debug, Default)]
pub struct NoGameEvents;

impl GameEvents for NoGameEvents {
    type Event = NoEvent;
    type Conn = ();

    fn unpack(
        _class: u32,
        _r: &mut BitReader,
        _ctx: &EventCtx<'_>,
        _charge: &mut ChargeFn,
    ) -> Result<Option<NoEvent>, NetError> {
        Ok(None)
    }

    fn on_received<S: GhostGame>(
        &mut self,
        _conn: &mut (),
        event: NoEvent,
        _cx: &mut ReceiveCx<'_, S>,
    ) -> Option<NetError> {
        match event {}
    }
}

/// TGE's objects as the hub ghosts them: Torque's ghosting policy, every ghostable object in scope, each record packed by
/// `torque_ghosts`' TGE codec of the object's class.
#[derive(Debug, Default)]
pub struct TgeGhostGame;

impl GhostGame for TgeGhostGame {
    type Policy = TorqueGhosts;
    type Source<'a> = TgeSource<'a>;
    type Cursor = ();
    type Conn = ();

    fn new_policy(&self) -> TorqueGhosts {
        TorqueGhosts
    }

    fn with_source<R>(
        hub: &GhostHub<Self>,
        env: SourceEnv<'_>,
        conn: Option<ConnId>,
        f: impl FnOnce(&mut TgeSource<'_>, &mut HashMap<ConnId, Conns<Self>>) -> R,
    ) -> R {
        let mut guard = hub.0.borrow_mut();
        let hub = &mut *guard;
        let engine_peer = conn
            .and_then(|c| hub.conns.get(&c))
            .is_some_and(|c| c.engine_peer);
        let always = ghost_always_of(env.vm, &hub.reg);
        let controlled = hub.conns.values().filter_map(|c| c.control).collect();
        let mut src = TgeSource {
            vm: env.vm,
            reg: &hub.reg,
            table: env.table,
            ghost_bits: env.ghost_bits,
            always,
            controlled,
            engine_peer,
        };
        f(&mut src, &mut hub.conns)
    }
}

/// `GhostAlwaysSet` in set order when the `Vm` has it, else the registry's ScopeAlways objects in id order.
fn ghost_always_of(vm: &Vm, reg: &Registry) -> Vec<ObjId> {
    let objects = vm.objects();
    if let Some(members) = objects
        .find(b"GhostAlwaysSet")
        .and_then(|set| objects.get(set))
        .and_then(|o| o.members())
    {
        return members.as_slice().to_vec();
    }
    let mut ids: Vec<ObjId> = reg
        .entries
        .iter()
        .filter(|(_, e)| e.flags & super::ghosts::flags::SCOPE_ALWAYS != 0)
        .map(|(id, _)| *id)
        .collect();
    ids.sort_unstable();
    ids
}

/// The global that makes a server ghost the engine's own classes (`torque_ghosts::light`'s) to every client, not only to the
/// engine's own (the local connection, a client granted the handshake's extension): set it to 1 when every client is the
/// engine's (a TGE client cannot read their records).
pub const ENGINE_CLASSES_GLOBAL: &str = "$Server::EngineClasses";

/// The world as one connection's ghost manager sees it, for the length of one call.
pub struct TgeSource<'a> {
    pub vm: &'a Vm,
    pub reg: &'a Registry,
    /// The string coder's table; a source made for scoping alone has none and packs nothing.
    pub table: Option<&'a Table>,
    pub ghost_bits: u8,
    always: Vec<ObjId>,
    /// The objects some connection controls (the ShapeBase part's `controlled` flag).
    controlled: Vec<ObjId>,
    /// The connection's peer is the engine's own client: the engine's own classes are ghosted to it (to no other).
    engine_peer: bool,
}

impl TgeSource<'_> {
    fn class(&self, obj: ObjId) -> Option<TgeClass> {
        let objects = self.vm.objects();
        let name = objects.classes.name(objects.get(obj)?.class());
        TgeGhosts::class_by_name(std::str::from_utf8(name).ok()?)
    }

    /// Whether the connection may know the object's class: TGE's always, the engine's own on an engine peer, or on any
    /// connection of a server that says all its clients are the engine's ([`ENGINE_CLASSES_GLOBAL`] true).
    fn known(&self, obj: ObjId) -> bool {
        let engine = self
            .reg
            .entries
            .get(&obj)
            .and_then(|e| TgeClass::of_code(e.code))
            .is_some_and(|c| ENGINE_OBJECT_CLASSES.contains(&c.name()));
        !engine
            || self.engine_peer
            || self
                .vm
                .global(ENGINE_CLASSES_GLOBAL)
                .is_some_and(|v| torque_ts_vm::text::atof(&v) != 0.0)
    }
}

/// An object's static fields as a TGE packer reads them: a field by its registered name, each component of a vector field one
/// element; anything else is null.
pub struct FieldView<'a> {
    pub vm: &'a Vm,
    pub obj: ObjId,
}

impl ObjectView for FieldView<'_> {
    fn get(&self, field: &str, index: usize) -> Value {
        let objects = self.vm.objects();
        let Some(object) = objects.get(self.obj) else {
            return Value::Null;
        };
        let Some((_, slots)) = objects
            .classes
            .static_field(object.class(), field.as_bytes())
        else {
            return Value::Null;
        };
        let (slot, component) = if slots.len() > 1 {
            (slots.get(index).copied(), 0)
        } else {
            (slots.first().copied(), index)
        };
        let Some(value) = slot.and_then(|s| object.slot(s)) else {
            return Value::Null;
        };
        let float = |v: Option<&f32>| v.map_or(Value::Null, |&f| Value::F32(f));
        match value {
            Field::Text(t) => Value::Str(t.to_vec()),
            Field::Int(i) => Value::Int(i64::from(*i)),
            Field::Float(f) => Value::F32(*f),
            Field::Bool(b) => Value::Bool(*b),
            Field::ColorI(c) => c
                .get(component)
                .map_or(Value::Null, |&b| Value::Int(i64::from(b))),
            Field::ColorF(c) => float(c.get(component)),
            Field::Floats2(p) => float(p.get(component)),
            Field::Floats3(p) => float(p.get(component)),
            Field::Ints2(p) => p
                .get(component)
                .map_or(Value::Null, |&i| Value::Int(i64::from(i))),
            Field::Object(id) => Value::Id(*id),
            Field::Matrix(_) | Field::IntList(_) | Field::FloatList(_) => Value::Null,
        }
    }
}

/// A TGE object as its record's packer reads it: the game classes' state where torque-game keeps it, the static fields for the
/// rest ([`FieldView`]).
///
/// - every object: `datablock` (the `dataBlock` field), `transform` (the object's matrix as a position and a unit quaternion,
///   `QuatF::set(MatrixF)`), `position` (its translation), `scale`; a ShapeBase is fully visible (`fade_value` 1) and runs no
///   script thread (`thread_sequence` -1), its damage over the datablock's `maxDamage`;
/// - a simulated Player ([`torque_game::sim::player::state`]): `velocity`, `rotation` (its yaw at 2), `head`, `falling`,
///   `state` (the action state), `action` (the action animation), `arm_action` (`setArmThread`'s), `energy`, and from its
///   datablock `look_action`, `max_look_angle`,
///   `max_energy` and `max_damage`; the move fields are the last move's.
/// - a Sky: its record's fields from the persist fields of their TGE names (`visibleDistance`, `fogDistance`, `fogColor`,
///   `materialList`, `useSkyTextures`, the cloud layers' `cloudText` and `cloudHeightPer`, ...);
/// - a ShapeBase's mounted images (the game classes' image slots): per slot `image_datablock`, `image_wet`, `image_ammo`,
///   `image_loaded`, `image_target`, `image_trigger_down` and `image_fire_count`;
/// - a simulated Projectile ([`torque_game::sim::projectile::state`]): `position`, `velocity`, `current_tick` (its age),
///   `source_slot`, `hidden` (exploded) and the explosion's `explosion_position`, `explosion_normal` and `collide_hit_type`; the
///   link `source_object` (its source object's ghost on the connection, [`GameView::ghost_of`]).
///
/// The record is never left out for the controlling client (`controlled_here` false): the client draws the control object from
/// it like any ghost.
pub struct GameView<'a> {
    pub fields: FieldView<'a>,
    /// Whether some connection controls the object.
    pub controlled: bool,
    /// The connection's ghost index of an object (`getGhostIndex`; none: no links are resolved).
    pub ghost_of: Option<&'a dyn Fn(ObjId) -> Option<u32>>,
}

/// `QuatF::set(MatrixF)` of a row-major matrix's rotation part.
fn quat_of(m: &[f32; 16]) -> [f32; 4] {
    let at = |r: usize, c: usize| m[r * 4 + c];
    let trace = at(0, 0) + at(1, 1) + at(2, 2);
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt();
        let w = s * 0.5;
        let s = 0.5 / s;
        return [
            (at(1, 2) - at(2, 1)) * s,
            (at(2, 0) - at(0, 2)) * s,
            (at(0, 1) - at(1, 0)) * s,
            w,
        ];
    }
    let mut i = 0;
    if at(1, 1) > at(0, 0) {
        i = 1;
    }
    if at(2, 2) > at(i, i) {
        i = 2;
    }
    let j = (i + 1) % 3;
    let k = (j + 1) % 3;
    let s = (at(i, i) - (at(j, j) + at(k, k)) + 1.0).sqrt();
    let mut q = [0.0; 4];
    q[i] = s * 0.5;
    let s = 0.5 / s;
    q[j] = (at(i, j) + at(j, i)) * s;
    q[k] = (at(i, k) + at(k, i)) * s;
    q[3] = (at(j, k) - at(k, j)) * s;
    q
}

impl GameView<'_> {
    fn matrix(&self) -> Option<[f32; 16]> {
        torque_game::scene::access::matrix_of(self.fields.vm, self.fields.obj)
    }

    fn datablock(&self) -> Option<ObjId> {
        torque_game::scene::access::object_field(self.fields.vm, self.fields.obj, b"dataBlock")
    }

    fn db_float(&self, name: &[u8], default: f32) -> f32 {
        self.datablock().map_or(default, |db| {
            torque_game::sim::player::float(self.fields.vm, db, name, default)
        })
    }

    /// A Player's field from its simulated state, if the object is one.
    fn player(&self, field: &str, index: usize) -> Option<Value> {
        use torque_game::sim::player;
        let (vm, obj) = (self.fields.vm, self.fields.obj);
        let s = player::state(vm, obj)?;
        let info = player::info(vm, obj);
        let data = |pick: usize| {
            info.as_ref().map_or(Value::Null, |i| {
                let d = &i.data;
                Value::F32([d.max_look_angle, d.max_energy, d.max_damage][pick])
            })
        };
        let component = |v: [f32; 3]| v.get(index).map_or(Value::Null, |f| Value::F32(*f));
        Some(match field {
            "velocity" => component(s.velocity),
            "rotation" => component(s.rot),
            "head" => component(s.head),
            "falling" => Value::Bool(s.falling),
            "state" => Value::Int(i64::from(s.action_state)),
            "action" => Value::Int(i64::from(s.action_anim)),
            // The action thread as `setActionThread` left it: held at its end, at its end, first-person, and its position.
            "action_hold" | "action_at_end" | "action_first_person" | "action_position" => {
                let st = torque_game::scene::shapebase::state(vm, obj)?;
                let p = st.player.as_ref()?;
                match field {
                    "action_hold" => Value::Bool(p.hold),
                    "action_at_end" => Value::Bool(p.at_end),
                    "action_first_person" => Value::Bool(p.fsp),
                    _ => {
                        let pos = p
                            .action_thread
                            .zip(st.body.as_ref())
                            .and_then(|(t, b)| b.instance.thread(t))
                            .map_or(0.0, |t| t.pos);
                        Value::F32(pos)
                    }
                }
            }
            // The arm action `setArmThread` chose (no action until one is), and the datablock's `look` action.
            "arm_action" => Value::Int(i64::from(
                torque_game::scene::shapebase::state(vm, obj)
                    .and_then(|st| st.player.as_ref())
                    .map_or(::torque_ghosts::player::NULL_ANIMATION, |p| p.arm_action),
            )),
            "look_action" => Value::Int(i64::from(info.as_ref().map_or(0, |i| i.look_action))),
            "energy" => Value::F32(s.energy),
            "damage" => Value::F32(s.damage),
            "damage_state" => Value::Int(i64::from(s.damage_state)),
            "max_look_angle" => data(0),
            "max_energy" => data(1),
            "max_damage" => data(2),
            // A move's axis as its 6-bit word: -1 to 1 over 0 to 32, 16 the middle.
            "move_axes" => {
                let m = &s.last_move;
                let v = [m.x, m.y, m.z].get(index).copied().unwrap_or(0.0);
                Value::Int(((v.clamp(-1.0, 1.0) * 16.0) as i64) + 16)
            }
            "move_angles" => Value::Int(0),
            "move_free_look" => Value::Bool(s.last_move.free_look),
            "move_triggers" => {
                Value::Bool(s.last_move.triggers.get(index).copied().unwrap_or(false))
            }
            _ => return None,
        })
    }
}

impl GameView<'_> {
    /// A simulated Projectile's field from its state, if the object is one.
    fn projectile(&self, field: &str, index: usize) -> Option<Value> {
        let s = torque_game::sim::projectile::state(self.fields.vm, self.fields.obj)?;
        let component = |v: [f32; 3]| v.get(index).map_or(Value::Null, |f| Value::F32(*f));
        Some(match field {
            "position" => component(s.position),
            "velocity" => component(s.velocity),
            "current_tick" => Value::Int(i64::from(s.age)),
            "source_slot" => Value::Int(i64::from(s.source_slot)),
            "hidden" => Value::Bool(s.exploded),
            "explosion_position" => component(s.explosion_pos),
            "explosion_normal" => component(s.explosion_normal),
            "collide_hit_type" => Value::Int(i64::from(s.explosion_type)),
            _ => return None,
        })
    }
}

impl GameView<'_> {
    /// A ShapeBase's mounted image `index` from the game classes' image slots, for the image fields.
    fn image(&self, field: &str, index: usize) -> Option<Value> {
        if !field.starts_with("image_") {
            return None;
        }
        let set = torque_game::scene::ext::get(self.fields.vm.objects(), self.fields.obj)?
            .images
            .as_deref()?;
        let slot = set.slots.get(index)?;
        Some(match field {
            "image_datablock" => Value::Id(slot.db),
            "image_wet" => Value::Bool(slot.wet),
            "image_ammo" => Value::Bool(slot.ammo),
            "image_loaded" => Value::Bool(slot.loaded),
            "image_target" => Value::Bool(slot.target),
            "image_trigger_down" => Value::Bool(slot.trigger_down),
            "image_fire_count" => Value::Int(i64::from(slot.fire_count)),
            _ => return None,
        })
    }
}

impl ObjectView for GameView<'_> {
    /// A simulated Projectile's `source_object` and a SpotLight's `mount_object`: the object's ghost on the connection
    /// (`NotGhosted` while it has none).
    fn link(&self, field: &str) -> Link {
        let (vm, obj) = (self.fields.vm, self.fields.obj);
        let linked = match field {
            "source_object" => {
                torque_game::sim::projectile::state(vm, obj).and_then(|s| s.source.map(|s| s.0))
            }
            "mount_object" => torque_game::scene::lights::spot_mount(vm, obj).map(|m| m.0),
            _ => None,
        };
        let (Some(linked), Some(ghost_of)) = (linked, self.ghost_of) else {
            return Link::Null;
        };
        match ghost_of(linked) {
            Some(g) => Link::Ghost(g),
            None => Link::NotGhosted,
        }
    }

    fn get(&self, field: &str, index: usize) -> Value {
        if let Some(v) = self.player(field, index) {
            return v;
        }
        if let Some(v) = self.image(field, index) {
            return v;
        }
        if let Some(v) = self.projectile(field, index) {
            return v;
        }
        match field {
            "datablock" => Value::Id(self.datablock()),
            "transform" => match self.matrix() {
                Some(m) => {
                    let q = quat_of(&m);
                    Value::Xform(Some([
                        m[3].to_bits(),
                        m[7].to_bits(),
                        m[11].to_bits(),
                        q[0].to_bits(),
                        q[1].to_bits(),
                        q[2].to_bits(),
                        u32::from(q[3] < 0.0),
                    ]))
                }
                None => Value::Xform(None),
            },
            "position" => self
                .matrix()
                .and_then(|m| [m[3], m[7], m[11]].get(index).copied())
                .map_or(Value::Null, Value::F32),
            "scale" => torque_game::scene::access::scale_of(self.fields.vm, self.fields.obj)
                .get(index)
                .map_or(Value::Null, |f| Value::F32(*f)),
            "fade_value" => Value::F32(1.0),
            // A SpotLight's mount node and a TimeOfDay's animation.
            "mount_node" => torque_game::scene::lights::spot_mount(self.fields.vm, self.fields.obj)
                .map_or(Value::Int(0), |m| Value::Int(i64::from(m.1))),
            "animate_time" | "animate_speed" => {
                torque_game::scene::lights::day_animation(self.fields.vm, self.fields.obj)
                    .map_or(Value::F32(0.0), |(t, s)| {
                        Value::F32(if field == "animate_time" { t } else { s })
                    })
            }
            "thread_sequence" => Value::Int(-1),
            "max_damage" => Value::F32(self.db_float(b"maxDamage", 1.0)),
            "max_energy" => Value::F32(self.db_float(b"maxEnergy", 0.0)),
            "controlled" => Value::Bool(self.controlled),
            "controlled_here" => Value::Bool(false),
            _ => self.fields.get(sky_field(field).unwrap_or(field), index),
        }
    }
}

/// The Sky's script field behind a field its record reads, where the record's name is not the script's: the visible and fog
/// distances, the fog and solid colors, the material list, the cloud textures, the wind and the flags (the cloud heights and
/// speeds, the fog volumes and the storms are left out: the script sets them in other shapes).
fn sky_field(field: &str) -> Option<&'static str> {
    Some(match field {
        "visible_distance" => "visibleDistance",
        "fog_distance" => "fogDistance",
        "fog_color" => "fogColor",
        "solid_fill_color" => "SkySolidColor",
        "sky_textures_on" => "useSkyTextures",
        "render_box_bottom" => "renderBottomTexture",
        "effect_precip" => "windEffectPrecipitation",
        "no_render_bans" => "noRenderBans",
        "material_list" => "materialList",
        "cloud_text" => "cloudText",
        "cloud_height" => "cloudHeightPer",
        "wind_velocity" => "windVelocity",
        _ => return None,
    })
}

impl ObjectSource for TgeSource<'_> {
    fn exists(&self, obj: ObjId) -> bool {
        self.vm.objects().contains(obj) && self.reg.entries.contains_key(&obj) && self.known(obj)
    }

    fn class_id(&self, obj: ObjId) -> Option<u32> {
        self.reg.entries.get(&obj).map(|e| e.class_id)
    }

    fn net_flags(&self, obj: ObjId) -> u32 {
        if !self.known(obj) {
            return 0;
        }
        self.reg.entries.get(&obj).map_or(0, |e| e.flags)
    }

    fn pack_update(
        &mut self,
        obj: ObjId,
        mask: u32,
        w: &mut BitWriter,
        ghost_of: &dyn Fn(ObjId) -> Option<u32>,
    ) -> u32 {
        let (Some(class), Some(table)) = (self.class(obj), self.table) else {
            return 0;
        };
        let widths = NetWidths::new(self.ghost_bits, torque_net::profile::DEFAULT_DATABLOCK_BITS);
        let ctx = PackCtx::for_writer(w, widths, self.vm.sim_time());
        let view = GameView {
            fields: FieldView { vm: self.vm, obj },
            controlled: self.controlled.contains(&obj),
            ghost_of: Some(ghost_of),
        };
        let (record, kept) = TgeGhosts::pack(class, &view, mask, &ctx);
        // The packers cut every count to its width first, so a record cannot fail to encode.
        let _ = record.encode(w, table, widths);
        kept
    }

    fn update_priority(&self, _obj: ObjId, _cam: &CamInfo, _mask: u32, skip: u8) -> f32 {
        // `NetObject::getUpdatePriority`: the skip count over ten.
        f32::from(skip) * 0.1
    }

    fn camera_info(&self, scope: ObjId) -> Option<CamInfo> {
        let _ = scope;
        None
    }

    fn sky_visible_distance(&self) -> Option<f32> {
        None
    }

    fn mount_of(&self, _scope: ObjId) -> Option<ObjId> {
        None
    }

    fn scope_query(
        &self,
        _scope: ObjId,
        _cam: &CamInfo,
        _radius: f32,
        visit: &mut dyn FnMut(ObjId) -> bool,
    ) {
        let mut ids: Vec<ObjId> = self.reg.entries.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            if !visit(id) {
                return;
            }
        }
    }

    fn scope_distance(&self, _obj: ObjId, _scope: ObjId) -> Option<f32> {
        None
    }

    fn ghost_always(&self) -> &[ObjId] {
        &self.always
    }
}

/// The network host the main loop drives over a net game's shared state: the receive, send and timeout passes of [`pump`] and
/// the control branch of the process list's pass.
pub struct NetPasses<G: NetGame>(pub Rc<RefCell<NetShared<G>>>);

impl<G: NetGame> NetHost for NetPasses<G> {
    fn clock_ms(&self, sim: u32) -> u32 {
        self.0.borrow().clock_ms(sim)
    }

    fn phase(&self, phase: Phase) {
        self.0.borrow_mut().push_phase(phase);
    }

    fn receive(&self, vm: &mut Vm, now: u32, new_tick: bool) {
        pump::receive(vm, &self.0, now, new_tick);
    }

    fn advance_objects(
        &self,
        vm: &mut Vm,
        list: &ProcessHandle,
        steps: u32,
        budget: u64,
    ) -> Outcome {
        advance_objects_within(vm, list, &mut pump::control_side(&self.0), steps, budget)
    }

    fn send(&self, vm: &mut Vm, now: u32) {
        pump::send_pass(vm, &self.0, now);
    }

    fn timeouts(&self, vm: &mut Vm, now: u32) {
        pump::timeouts(vm, &self.0, now);
    }

    fn clients(&self) -> (bool, bool) {
        let sh = self.0.borrow();
        (
            sh.conns
                .values()
                .any(|rec| rec.local_peer.is_none() && rec.conn.is_some()),
            sh.conns.values().any(|rec| rec.local_peer.is_some()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::quat_of;

    #[test]
    fn a_matrixs_rotation_is_its_unit_quaternion() {
        let identity = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        assert_eq!(quat_of(&identity), [0.0, 0.0, 0.0, 1.0]);
        // A half turn about z (the trace is -1: the largest-diagonal branch).
        let half = [
            -1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let q = quat_of(&half);
        assert!(
            (q[2].abs() - 1.0).abs() < 1e-6 && q[3].abs() < 1e-6,
            "{q:?}"
        );
        // A quarter turn about z, as `MatrixF` stores it (row-major, its rotation the transpose of the column form).
        let (s, c) = std::f32::consts::FRAC_PI_2.sin_cos();
        let quarter = [
            c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let q = quat_of(&quarter);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (q[2].abs() - h).abs() < 1e-6 && (q[3].abs() - h).abs() < 1e-6,
            "{q:?}"
        );
    }
}
