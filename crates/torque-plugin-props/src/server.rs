//! The server's half: the `Prop` console class a prop is (its fields are what [`PropGhost`](crate::records::PropGhost) is packed
//! from), the server's state ([`ServerProps`]), spawning, the collision source players meet props through, and the order of the
//! props' updates in each packet. The console functions and the placement checks are [`crate::natives`]; the tick hook is
//! [`crate::tick`].

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::rc::Rc;

use torque_collide::source::{CollisionShape, CollisionSource, ShapeKind, SourceHit};
use torque_collide::types::STATIC_SHAPE;
use torque_engine::{EngineClass, Prefs, ScriptClass, ScriptError, VmPrefs};
use torque_ts_vm::fields::Value as Field;
use torque_ts_vm::geometry;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{FieldTarget, Vm};

use crate::data::{PropData, PropParams};
use crate::hits::{Blast, Hit, MAX_PENDING_HITS, ShotPush};
use crate::quant::{self, Quantized, conjugate};
use crate::records::{MASK_FLAGS, MASK_INIT, MASK_STATE, STATE_FIELDS};
use crate::world::{Owner, OwnerKey, PropLimits, PropWorld};

/// The class name as scripts see it.
pub const CLASS: &str = <NetProp as ScriptClass>::NAME;

/// `$Pref::Props::NetShare`'s default.
pub const DEFAULT_NET_SHARE: f32 = 0.4;

/// A server's prefs for props. Each is read every tick (the sub-steps when the world is made) and held to its range.
#[derive(Prefs, Clone, Debug, PartialEq)]
#[prefs(prefix = "$Pref::Props")]
pub struct PropPrefs {
    /// The physics sub-steps of a 32 ms tick, read when the server makes its world.
    #[pref(default = 4, range = 1..=8)]
    pub substeps: i32,
    /// The most props one player may own.
    #[pref(default = 64, range = 1..=1024, name = "MaxPerPlayer")]
    pub max_per_player: i32,
    /// The most props the players of one remote address may own between them, orphans counted (room for players behind one
    /// router; a player who comes back under a new key starts no new quota).
    #[pref(default = 256, range = 1..=1024, name = "MaxPerAddress")]
    pub max_per_address: i32,
    /// The most props on the server.
    #[pref(default = 1024, range = 1..=1024, name = "MaxProps")]
    pub max_props: i32,
    /// The most props awake at once; past it the farthest from every player sleep.
    #[pref(default = 256, range = 1..=1024, name = "MaxAwake")]
    pub max_awake: i32,
    /// The physics work a tick may take (work units); a step past it does not run and the awake cap is lowered for a while.
    #[pref(default = 4000000, range = 1000..=1000000000, name = "StepBudget")]
    pub step_budget: i32,
    /// Seconds a prop outlives its owner's connection.
    #[pref(default = 120.0, range = 0.0..=3600.0, name = "OrphanSeconds")]
    pub orphan_seconds: f32,
    /// The share of each packet the props' updates take at most (0.05 to 1).
    #[pref(default = 0.4, range = 0.05..=1.0, name = "NetShare")]
    pub net_share: f32,
    /// The most joints one player may own.
    #[pref(default = 64, range = 1..=1024, name = "MaxJointsPerPlayer")]
    pub max_joints_per_player: i32,
    /// The most joints the players of one remote address may own between them, orphans counted.
    #[pref(default = 256, range = 1..=1024, name = "MaxJointsPerAddress")]
    pub max_joints_per_address: i32,
    /// The most joints on the server.
    #[pref(default = 1024, range = 1..=1024, name = "MaxJoints")]
    pub max_joints: i32,
    /// The most joints one prop may be an end of.
    #[pref(default = 16, range = 1..=64, name = "MaxJointsPerProp")]
    pub max_joints_per_prop: i32,
}

/// A prop on the server: the object a server script makes with `propSpawn` and every client of the engine sees as a shape. The
/// fields are the server's record of it, which a ghost is packed from; the physics world holds the truth and the tick writes it
/// here, so a script reads a prop through the `prop*` functions (or `getTransform`), not by writing these.
#[expect(
    dead_code,
    reason = "the fields are the class's script fields; Rust reads and writes them through the script engine"
)]
#[derive(EngineClass)]
#[class(name = "Prop", parent = "SceneObject")]
pub struct NetProp {
    /// The `PropData` datablock's object id.
    data_block: i32,
    /// The key of its collision shape on both sides (0 to 1,023).
    slot: i32,
    /// The owner's connection object (0: none, or gone).
    owner_conn: i32,
    /// The predict id the owner's client gave it (0: none).
    predict: i32,
    /// 1 while it sleeps, 1 while it is frozen.
    asleep: i32,
    frozen: i32,
    /// The state at the wire's precision: the position in 1/1024 units, the rotation's largest component and the other three in
    /// 15 bits, the speed and the spin in 1/256 units a second.
    #[field(name = "qPosX")]
    q_pos_x: i32,
    #[field(name = "qPosY")]
    q_pos_y: i32,
    #[field(name = "qPosZ")]
    q_pos_z: i32,
    #[field(name = "qRotI", default = 3)]
    q_rot_i: i32,
    #[field(name = "qRotA", default = 16383)]
    q_rot_a: i32,
    #[field(name = "qRotB", default = 16383)]
    q_rot_b: i32,
    #[field(name = "qRotC", default = 16383)]
    q_rot_c: i32,
    #[field(name = "qLinX")]
    q_lin_x: i32,
    #[field(name = "qLinY")]
    q_lin_y: i32,
    #[field(name = "qLinZ")]
    q_lin_z: i32,
    #[field(name = "qAngX")]
    q_ang_x: i32,
    #[field(name = "qAngY")]
    q_ang_y: i32,
    #[field(name = "qAngZ")]
    q_ang_z: i32,
}

/// The server's state for props: the world (made at the first tick or spawn, with the sub-steps the prefs say then) and what the
/// tick hook keeps between ticks.
#[derive(Debug)]
pub struct PropServer {
    pub world: Option<PropWorld>,
    /// The ticks run.
    pub tick: u64,
    /// The share of each packet the props take (the pref, read each tick).
    pub net_share: f32,
    /// The object each owner's connection controls, by connection (made each tick, for the priority, which runs inside the
    /// ghosting and so may not ask the ghosting layer itself).
    pub(crate) controls: BTreeMap<u32, u32>,
    /// The projectiles' hits on props since the last step, in the order they hit (at most [`MAX_PENDING_HITS`]).
    pub(crate) hits: Vec<Hit>,
    /// The explosions since the last step, in the order they went off (at most [`MAX_PENDING_HITS`]).
    pub(crate) blasts: Vec<Blast>,
}

impl Default for PropServer {
    /// No world yet, and the prefs' default share until the first tick reads the pref.
    fn default() -> PropServer {
        PropServer {
            world: None,
            tick: 0,
            net_share: DEFAULT_NET_SHARE,
            controls: BTreeMap::new(),
            hits: Vec::new(),
            blasts: Vec::new(),
        }
    }
}

impl PropServer {
    /// The world, made now if it is not yet.
    pub fn world_mut(&mut self, vm: &Vm) -> &mut PropWorld {
        self.world.get_or_insert_with(|| {
            let prefs = vm.prefs::<PropPrefs>();
            let mut world = PropWorld::new(prefs.substeps as u8, limits_of(&prefs));
            world.joint_limits = joint_limits_of(&prefs);
            world
        })
    }
}

/// The caps the prefs say.
pub fn limits_of(prefs: &PropPrefs) -> PropLimits {
    PropLimits {
        per_player: prefs.max_per_player as u32,
        per_address: prefs.max_per_address as u32,
        per_server: prefs.max_props as u32,
        awake: prefs.max_awake as u32,
        step_budget: prefs.step_budget as u64,
        orphan_ticks: (f64::from(prefs.orphan_seconds) * 1000.0 / 32.0) as u64,
    }
    .held()
}

/// The joint caps the prefs say.
pub fn joint_limits_of(prefs: &PropPrefs) -> crate::joints::JointLimits {
    crate::joints::JointLimits {
        per_player: prefs.max_joints_per_player as u32,
        per_address: prefs.max_joints_per_address as u32,
        per_server: prefs.max_joints as u32,
        per_prop: prefs.max_joints_per_prop as u32,
    }
    .held()
}

/// The engine resource that holds a server's props. A server made from the engine starts it over.
#[derive(Clone, Debug, Default)]
pub struct ServerProps(pub Rc<RefCell<PropServer>>);

impl ServerProps {
    /// A fresh state (a new server).
    pub fn reset(&self) {
        *self.0.borrow_mut() = PropServer::default();
    }

    /// Reads the state, if no one is changing it.
    pub fn with<R>(&self, f: impl FnOnce(&PropServer) -> R) -> Option<R> {
        self.0.try_borrow().ok().map(|s| f(&s))
    }
}

/// A connection's owner key: the game's identity for the player (the connection's `propOwnerKey` field, its whole text up to
/// [`crate::world::MAX_OWNER_KEY`] bytes) when the game sets one, else the connection itself, so a player who comes back under
/// the same identity gets their props back.
pub fn owner_key(vm: &Vm, conn: ObjId) -> OwnerKey {
    let text = vm.read_field_of(FieldTarget::Live(conn), b"propOwnerKey", b"");
    OwnerKey::new(&text, conn)
}

/// The quota key of a connection's address text (`getAddress()`'s: `"IP:1.2.3.4:28000"`, `"IP6:[fe80::1%3]:28000"`): the IP
/// with its `IP:`/`IP6:` tag, brackets, port and zone taken off and made canonical (an IPv4 address mapped into IPv6 is the IPv4
/// one); an IPv4 address is its own key, an IPv6 one its /64 (one network's addresses share a quota). None for text that is no
/// address (`"local"`, junk).
pub fn address_key(text: &str) -> Option<Rc<str>> {
    let text = text.trim();
    let text = text
        .strip_prefix("IP6:")
        .or_else(|| text.strip_prefix("IP:"))
        .unwrap_or(text);
    let host = match text.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(rest),
        // A bare IPv6 address has colons of its own: only a single colon is a port.
        None if text.matches(':').count() == 1 => text.split(':').next().unwrap_or(text),
        None => text,
    };
    let host = host.split('%').next().unwrap_or(host);
    let ip: IpAddr = host.parse().ok()?;
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    };
    Some(Rc::from(match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    }))
}

/// The quota key of a connection's remote address ([`address_key`]); a connection whose address cannot be read is a key of its
/// own (`"conn:<id>"`), so no owner escapes the per-address cap.
pub fn address_of(vm: &mut Vm, conn: ObjId) -> Rc<str> {
    vm.call_method(conn, b"getAddress", &[])
        .ok()
        .and_then(|text| address_key(&String::from_utf8_lossy(&text)))
        .unwrap_or_else(|| Rc::from(format!("conn:{conn}")))
}

/// Who owns a prop a connection spawns: its key and address, the connection holding it.
pub fn owner_of(vm: &mut Vm, conn: ObjId) -> Owner {
    Owner {
        key: owner_key(vm, conn),
        address: Some(address_of(vm, conn)),
        conn: Some(conn),
        left: None,
    }
}

/// A transform `"x y z [ax ay az angle]"` (an axis and an angle in radians, as Torque writes one) as a position and a unit
/// quaternion of the physics world, turning the way the same transform turns a scene object (Torque's own maths).
pub fn parse_transform(text: &str) -> Result<([f32; 3], [f32; 4]), ScriptError> {
    let bad =
        || ScriptError::new("the transform must be \"x y z [ax ay az angle]\" of finite numbers");
    let w: Vec<f32> = text
        .split_ascii_whitespace()
        .take(7)
        .map(|w| torque_ts_vm::text::atof(w.as_bytes()) as f32)
        .collect();
    if w.len() < 3 || w.iter().any(|v| !v.is_finite()) {
        return Err(bad());
    }
    let position = [w[0], w[1], w[2]];
    let mut rotation = [0.0, 0.0, 0.0, 1.0];
    if w.len() >= 7 {
        let axis = [w[3], w[4], w[5]];
        let len = crate::world::length(axis);
        if len > 1.0e-6 {
            let unit = axis.map(|a| a / len);
            rotation = conjugate(geometry::quat_from_axis_angle(unit, w[6]));
        }
    }
    Ok((position, rotation))
}

/// A unit quaternion of the physics world as Torque's `"ax ay az angle"` (the inverse of [`parse_transform`]'s turn).
pub fn axis_angle(q: [f32; 4]) -> [f32; 4] {
    let (axis, angle) = geometry::axis_angle_from_quat(conjugate(q));
    [axis[0], axis[1], axis[2], angle]
}

/// Deletes an object, printing why when its deletion failed (an `onRemove` that stopped on an error).
pub(crate) fn delete_logged(vm: &mut Vm, id: u32) {
    if let Err(e) = vm.delete_object(id) {
        vm.warn(format!("props: deleting {id}: {e}"));
    }
}

pub(crate) fn is_class(vm: &Vm, id: ObjId, class: &str) -> bool {
    let objects = vm.objects();
    objects
        .get(id)
        .is_some_and(|o| objects.classes.name(o.class()) == class.as_bytes())
}

/// The prop `id`, if it is one.
pub(crate) fn prop_of(vm: &Vm, id: i32) -> Result<ObjId, ScriptError> {
    let obj = ObjId::try_from(id).unwrap_or(0);
    if is_class(vm, obj, CLASS) {
        Ok(obj)
    } else {
        Err(ScriptError::new(format!("{id} is not a prop")))
    }
}

/// Writes a prop's record fields from the world and marks `mask` of its ghost dirty; the scene transform follows its pose.
pub(crate) fn write_back(vm: &mut Vm, world: &PropWorld, id: u32, mask: u32) {
    use torque_game::scene::access::set_field;
    let Some(p) = world.get(id) else {
        return;
    };
    if mask & (MASK_STATE | MASK_FLAGS) != 0 {
        for (name, word) in STATE_FIELDS.iter().zip(p.q.words()) {
            set_field(vm, id, name.as_bytes(), Field::Int(word));
        }
        set_field(vm, id, b"asleep", Field::Int(i32::from(p.rest.is_asleep())));
        set_field(vm, id, b"frozen", Field::Int(i32::from(p.rest.is_frozen())));
        let m = quant::matrix(p.q.position(), p.q.rotation());
        torque_game::scene::object::set_plain_transform(vm, id, &m);
    }
    if mask & MASK_INIT != 0 {
        let conn = p
            .owner
            .as_ref()
            .and_then(|o| o.conn)
            .map_or(0, |c| c as i32);
        set_field(vm, id, b"ownerConn", Field::Int(conn));
        set_field(vm, id, b"slot", Field::Int(i32::from(p.slot)));
        set_field(vm, id, b"predict", Field::Int(i32::from(p.predict)));
    }
    torque_game::scene::ghost::mark(vm, id, mask);
}

/// Spawns a prop for a script or a client's command: the caps checked, the object made (natively, with no script text: the
/// datablock is an id), the world's prop, the record written, the object put in `MissionCleanup` when there is one and the
/// datablock's `onPropSpawn` called (an error of the callback is printed: the prop is spawned).
pub(crate) fn spawn(
    vm: &mut Vm,
    props: &ServerProps,
    data: ObjId,
    pose: ([f32; 3], [f32; 4]),
    owner: Option<Owner>,
    predict: u8,
) -> Result<u32, String> {
    if !is_class(vm, data, <PropData as ScriptClass>::NAME) {
        return Err(format!("{data} is not a PropData"));
    }
    let params = PropData::read(vm, data)
        .map(|d| PropParams::from(&d))
        .map_err(|e| e.to_string())?;
    // The caps are checked before the object is made, and again by the world.
    {
        let mut srv = props.0.borrow_mut();
        let world = srv.world_mut(vm);
        if world.len() as u32 >= world.limits.per_server {
            return Err(crate::world::SpawnError::PerServer.to_string());
        }
        if let Some(o) = &owner {
            world.may_own(o, data, &params).map_err(|e| e.to_string())?;
        }
    }
    let datablock = data.to_string();
    let made = vm
        .new_object(CLASS.as_bytes(), &[(b"dataBlock", datablock.as_bytes())])
        .map_err(|e| e.to_string())?;
    let Some(id) = made.filter(|&id| is_class(vm, id, CLASS)) else {
        return Err("the prop object was not made".to_string());
    };
    let half = params.shape.half_extents();
    torque_game::scene::object::set_object_box(vm, id, half.map(|h| -h), half);
    let added = props
        .0
        .borrow_mut()
        .world_mut(vm)
        .spawn(id, data, params, pose, owner, predict);
    if let Err(e) = added {
        delete_logged(vm, id);
        return Err(e.to_string());
    }
    if let Ok(srv) = props.0.try_borrow()
        && let Some(world) = &srv.world
    {
        write_back(vm, world, id, MASK_INIT | MASK_STATE | MASK_FLAGS);
    }
    if let Some(cleanup) = vm.objects().find(b"MissionCleanup")
        && let Err(e) = vm.call_method(cleanup, b"add", &[id.to_string().into_bytes()])
    {
        vm.error(format!("propSpawn: MissionCleanup.add: {e}"));
    }
    if let Err(e) = vm.call_method(data, b"onPropSpawn", &[id.to_string().into_bytes()]) {
        vm.error(format!("propSpawn: onPropSpawn: {e}"));
    }
    Ok(id)
}

/// The boxes of the players now, by id.
pub(crate) fn player_boxes(vm: &Vm) -> Vec<(u32, [f32; 3], [f32; 3])> {
    vm.objects()
        .iter()
        .filter_map(|(id, _)| {
            let s = torque_game::sim::player::state(vm, id)?;
            Some((id, s.world_box.min, s.world_box.max))
        })
        .collect()
}

/// The most a prop's drift (its speed times the packets it waited, nearer ones more) adds to its priority.
pub const DRIFT_CAP: f32 = 20.0;

/// The priority of a prop's update to the connection whose scope object is `scope` (higher goes first): one its player touches
/// first, then one it owns, then by how far it may have moved since it was last sent (its speed times the packets it waited,
/// nearer ones before farther ones, up to [`DRIFT_CAP`]) and how long it has waited; a prop's last update before it sleeps or
/// freezes is sent soon. (The engine holds a share group's priorities below a ghost's kill.)
pub fn priority(props: &ServerProps, vm: &Vm, obj: ObjId, scope: Option<ObjId>, skip: u8) -> f32 {
    let waited = f32::from(skip);
    props
        .with(|s| {
            let p = s.world.as_ref()?.get(obj)?;
            let mut tier = 0.0;
            if let Some(scope) = scope {
                if p.touched.contains(&scope) {
                    tier += 100.0;
                }
                if p.owner
                    .as_ref()
                    .and_then(|o| o.conn)
                    .is_some_and(|c| s.controls.get(&c) == Some(&scope))
                {
                    tier += 10.0;
                }
            }
            if !p.rest.is_awake() {
                tier += 5.0;
            }
            let dist = scope
                .and_then(|s| torque_game::scene::access::matrix_of(vm, s))
                .map_or(0.0, |m| {
                    let at = p.q.position();
                    crate::world::length([at[0] - m[3], at[1] - m[7], at[2] - m[11]])
                });
            // The drift is capped, so a prop that has waited long enough (a still one's first record among many moving ones)
            // goes before one just sent, however fast that one moves.
            let drift = (p.q.speed() * (1.0 + waited) / (1.0 + dist / 32.0)).min(DRIFT_CAP);
            Some(tier + drift + waited * 0.5)
        })
        .flatten()
        .unwrap_or(waited * 0.5)
}

/// The props as the server's collision source: each a box at its pose, keyed by its slot, with a static shape's bit (players,
/// items and projectiles hit it; TGE's vehicles see it as static). A shape stands for its prop, and a projectile's hit on it is
/// kept for the next step ([`crate::hits`]).
#[derive(Debug, Clone)]
pub struct ServerSource(pub ServerProps);

impl CollisionSource for ServerSource {
    fn shapes(&self, out: &mut Vec<CollisionShape>) {
        let _ = self.0.with(|s| {
            let Some(world) = &s.world else { return };
            for p in world.props().values() {
                out.push(shape_of(p.slot, &p.q, p.params.shape.half_extents()));
            }
        });
    }

    fn object_of(&self, key: u32) -> Option<u32> {
        self.0.with(|s| prop_in_slot(s, key)).flatten()
    }

    fn hit(&self, key: u32, hit: &SourceHit) {
        let Ok(mut s) = self.0.0.try_borrow_mut() else {
            return;
        };
        let Some(prop) = prop_in_slot(&s, key) else {
            return;
        };
        if s.hits.len() < MAX_PENDING_HITS {
            s.hits.push(Hit {
                prop,
                by: hit.by,
                datablock: hit.datablock,
                point: hit.point,
                normal: hit.normal,
                velocity: hit.velocity,
            });
        }
    }
}

/// The prop whose collision shape is keyed `key`.
fn prop_in_slot(s: &PropServer, key: u32) -> Option<u32> {
    s.world.as_ref()?.in_slot(u16::try_from(key).ok()?)
}

/// The explosions the server's projectiles make, kept for the next step (at most [`MAX_PENDING_HITS`]).
#[derive(Debug, Clone)]
pub struct Blasts(pub ServerProps);

impl torque_game::sim::projectile::ExplosionWatch for Blasts {
    fn exploded(&self, _projectile: ObjId, datablock: ObjId, at: [f32; 3]) {
        if let Ok(mut s) = self.0.0.try_borrow_mut()
            && s.blasts.len() < MAX_PENDING_HITS
        {
            s.blasts.push(Blast { datablock, at });
        }
    }
}

/// What a projectile's datablock `db` says it pushes with (its `impulse`, `areaImpulse` and `areaImpulseRadius`; defaults for
/// one that is gone).
pub(crate) fn shot_push(vm: &Vm, db: ObjId) -> ShotPush {
    if !vm.objects().contains(db) {
        return ShotPush::default();
    }
    let number = |field: &[u8]| {
        let text = vm.read_field_of(FieldTarget::Live(db), field, b"");
        (!text.is_empty()).then(|| torque_ts_vm::text::atof(&text) as f32)
    };
    ShotPush::of(
        number(b"impulse"),
        number(b"areaImpulse"),
        number(b"areaImpulseRadius"),
    )
}

/// A prop's collision shape: a box of `half` at the state's pose.
pub fn shape_of(slot: u16, q: &Quantized, half: [f32; 3]) -> CollisionShape {
    CollisionShape {
        key: u32::from(slot),
        pose: quant::matrix(q.position(), q.rotation()),
        kind: ShapeKind::Box { half },
        type_mask: STATIC_SHAPE,
    }
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
