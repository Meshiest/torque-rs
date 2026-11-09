//! The server's console functions (`propSpawn` and kin) and the placement checks a game calls before it spawns a prop for a
//! player ([`ServerProps::check_placement`], `propCheckPlacement`). Props have no client entry point: a game's own server
//! commands decide who may spawn what.

use torque_collide::types::{INTERIOR, ITEM, PLAYER, STATIC_SHAPE, STATIC_TS, TERRAIN, VEHICLE};
use torque_engine::args::vec3;
use torque_engine::{Res, ScriptClass, ScriptError, VmMut, function};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use crate::data::{PropData, PropParams};
use crate::quant;
use crate::server::{
    ServerProps, axis_angle, is_class, owner_key, owner_of, parse_transform, player_boxes, prop_of,
    spawn, write_back,
};
use crate::world::{Prop, PropWorld, world_half};

/// `"x y z"` as a vector of finite numbers.
fn finite_vec(text: &str, what: &str) -> Result<[f32; 3], ScriptError> {
    let v = vec3(text, what)?;
    if v.iter().all(|c| c.is_finite()) {
        Ok(v)
    } else {
        Err(ScriptError::new(format!("{what} must be finite")))
    }
}

/// spawns a prop of the datablock at a transform and returns its object id
///
/// `transform` is `"x y z"` or `"x y z ax ay az angle"`. `owner` is the connection (a client) the prop counts against: its
/// per-player cap (`$Pref::Props::MaxPerPlayer`, and the datablock's `maxPerPlayer`), its address's (`$Pref::Props::
/// MaxPerAddress`), and its props outlive its leaving by `$Pref::Props::OrphanSeconds`. Those caps apply only when `owner` is
/// given: a prop with no owner counts against the server's cap alone, so a game spawning for a player passes the player's
/// connection. `predictId` (1 to 255) ties the prop to
/// one the owner's client showed at once. Past a cap, or for a pose out of the world, the call prints an error and returns 0.
/// The prop is put in `MissionCleanup` when there is one; the datablock's `onPropSpawn(%this, %prop)` is called.
#[function(name = "propSpawn")]
fn prop_spawn(
    mut vm: VmMut,
    props: Res<ServerProps>,
    datablock: &str,
    transform: &str,
    owner: Option<i32>,
    predict_id: Option<i32>,
) -> Result<i32, ScriptError> {
    let data = vm
        .objects()
        .find(datablock.as_bytes())
        .ok_or_else(|| ScriptError::new(format!("there is no object {datablock:?}")))?;
    let pose = parse_transform(transform)?;
    let owner = owner
        .and_then(|o| ObjId::try_from(o).ok())
        .filter(|&o| o != 0 && vm.objects().contains(o))
        .map(|o| owner_of(&mut vm, o));
    let predict = predict_id.filter(|p| (1..=255).contains(p)).unwrap_or(0) as u8;
    match spawn(&mut vm, &props, data, pose, owner, predict) {
        Ok(id) => Ok(id as i32),
        Err(e) => Err(ScriptError::new(e)),
    }
}

/// takes a prop away from the world and every client; true if there was one
#[function(name = "propDelete")]
fn prop_delete(mut vm: VmMut, props: Res<ServerProps>, prop: i32) -> bool {
    let Ok(id) = prop_of(&vm, prop) else {
        return false;
    };
    if let Some(world) = props.0.borrow_mut().world.as_mut() {
        world.remove(id);
    }
    vm.delete_object(id).is_ok()
}

/// Runs `f` on the world and writes back the mask it returns.
fn change(
    vm: &mut Vm,
    props: &ServerProps,
    id: u32,
    f: impl FnOnce(&mut PropWorld) -> Option<u32>,
) -> bool {
    let mut srv = props.0.borrow_mut();
    let world = srv.world_mut(vm);
    let Some(mask) = f(world) else {
        return false;
    };
    write_back(vm, world, id, mask);
    true
}

/// pins a prop where it is: it no longer moves, and others rest on it; true if it was frozen now
///
/// False for a prop already frozen, or whose datablock has `canFreeze = false`.
#[function(name = "propFreeze")]
fn prop_freeze(mut vm: VmMut, props: Res<ServerProps>, prop: i32) -> Result<bool, ScriptError> {
    let id = prop_of(&vm, prop)?;
    Ok(change(&mut vm, &props, id, |w| w.freeze(id)))
}

/// lets a frozen prop move again, at rest; true if it was frozen
#[function(name = "propUnfreeze")]
fn prop_unfreeze(mut vm: VmMut, props: Res<ServerProps>, prop: i32) -> Result<bool, ScriptError> {
    let id = prop_of(&vm, prop)?;
    Ok(change(&mut vm, &props, id, |w| w.unfreeze(id)))
}

/// pushes a prop: `impulse` is `"x y z"` (mass times units a second), held to the world's limits; wakes it
///
/// False for a frozen prop.
#[function(name = "propApplyImpulse")]
fn prop_apply_impulse(
    vm: VmMut,
    props: Res<ServerProps>,
    prop: i32,
    impulse: &str,
) -> Result<bool, ScriptError> {
    let id = prop_of(&vm, prop)?;
    let j = finite_vec(impulse, "the impulse")?;
    Ok(props.0.borrow_mut().world_mut(&vm).impulse(id, j))
}

/// sets a prop's velocity (`"x y z"`, units a second, held to its datablock's `maxSpeed`) and, if given, its spin (radians a
/// second about each axis); wakes it
#[function(name = "propSetVelocity")]
fn prop_set_velocity(
    vm: VmMut,
    props: Res<ServerProps>,
    prop: i32,
    velocity: &str,
    spin: Option<&str>,
) -> Result<bool, ScriptError> {
    let id = prop_of(&vm, prop)?;
    let v = finite_vec(velocity, "the velocity")?;
    let mut srv = props.0.borrow_mut();
    let world = srv.world_mut(&vm);
    let w = match spin {
        Some(s) => finite_vec(s, "the spin")?,
        None => world.get(id).map_or([0.0; 3], |p| p.q.state().angvel),
    };
    Ok(world.set_velocity(id, v, w))
}

fn with_prop<R>(props: &ServerProps, id: u32, f: impl FnOnce(&Prop) -> R) -> Option<R> {
    props
        .with(|s| s.world.as_ref().and_then(|w| w.get(id)).map(f))
        .flatten()
}

fn text3(v: [f32; 3]) -> String {
    format!("{} {} {}", v[0], v[1], v[2])
}

/// a prop's velocity, `"x y z"` (units a second), as the clients are sent it
#[function(name = "propGetVelocity")]
fn prop_get_velocity(vm: VmMut, props: Res<ServerProps>, prop: i32) -> Result<String, ScriptError> {
    let id = prop_of(&vm, prop)?;
    Ok(with_prop(&props, id, |p| text3(p.q.state().linvel)).unwrap_or_default())
}

/// a prop's spin, `"x y z"` (radians a second about each axis)
#[function(name = "propGetSpin")]
fn prop_get_spin(vm: VmMut, props: Res<ServerProps>, prop: i32) -> Result<String, ScriptError> {
    let id = prop_of(&vm, prop)?;
    Ok(with_prop(&props, id, |p| text3(p.q.state().angvel)).unwrap_or_default())
}

/// a prop's transform, `"x y z ax ay az angle"`, as the clients are sent it
#[function(name = "propGetTransform")]
fn prop_get_transform(
    vm: VmMut,
    props: Res<ServerProps>,
    prop: i32,
) -> Result<String, ScriptError> {
    let id = prop_of(&vm, prop)?;
    Ok(with_prop(&props, id, |p| {
        let [x, y, z] = p.q.position();
        let [ax, ay, az, angle] = axis_angle(p.q.rotation());
        format!("{x} {y} {z} {ax} {ay} {az} {angle}")
    })
    .unwrap_or_default())
}

/// whether a prop is asleep (still, and costing nothing until something wakes it)
#[function(name = "propIsAsleep")]
fn prop_is_asleep(vm: VmMut, props: Res<ServerProps>, prop: i32) -> Result<bool, ScriptError> {
    let id = prop_of(&vm, prop)?;
    Ok(with_prop(&props, id, |p| p.rest.is_asleep()).unwrap_or(false))
}

/// whether a prop is frozen
#[function(name = "propIsFrozen")]
fn prop_is_frozen(vm: VmMut, props: Res<ServerProps>, prop: i32) -> Result<bool, ScriptError> {
    let id = prop_of(&vm, prop)?;
    Ok(with_prop(&props, id, |p| p.rest.is_frozen()).unwrap_or(false))
}

/// the connection that owns a prop, 0 for none (or an owner who left)
#[function(name = "propGetOwner")]
fn prop_get_owner(vm: VmMut, props: Res<ServerProps>, prop: i32) -> Result<i32, ScriptError> {
    let id = prop_of(&vm, prop)?;
    Ok(with_prop(&props, id, |p| {
        p.owner
            .as_ref()
            .and_then(|o| o.conn)
            .map_or(0, |c| c as i32)
    })
    .unwrap_or(0))
}

/// how many props there are, or, given a client, how many its owner key owns (orphans of an earlier connection under the same
/// `propOwnerKey` counted)
#[function(name = "propCount")]
fn prop_count(vm: VmMut, props: Res<ServerProps>, owner: Option<i32>) -> i32 {
    let key = owner
        .and_then(|o| ObjId::try_from(o).ok())
        .filter(|&o| vm.objects().contains(o))
        .map(|o| owner_key(&vm, o));
    props
        .with(|s| {
            s.world.as_ref().map_or(0, |w| match &key {
                Some(k) => w.owned_by(k) as i32,
                None => w.len() as i32,
            })
        })
        .unwrap_or(0)
}

/// how many props are awake now
#[function(name = "propAwakeCount")]
fn prop_awake_count(props: Res<ServerProps>) -> i32 {
    props
        .with(|s| s.world.as_ref().map_or(0, |w| w.awake() as i32))
        .unwrap_or(0)
}

/// The object a client controls (its `getControlObject()`), if it is one.
pub(crate) fn control_of(vm: &mut Vm, client: ObjId) -> Option<ObjId> {
    let text = vm.call_method(client, b"getControlObject", &[]).ok()?;
    let id = torque_ts_vm::text::atof(&text) as i64;
    u32::try_from(id)
        .ok()
        .filter(|&o| o != 0 && vm.objects().contains(o))
}

/// checks where a prop of the datablock could be spawned for a player: "" when it may, else the reason
///
/// `transform` is the prop's `"x y z [ax ay az angle]"` and `eye` the point it is placed from (a player's eye), at most
/// `reach` units away (32 when left out). The checks: the datablock is a `PropData`; the transform is finite, its rotation a
/// turn, and inside the world; the eye is finite and inside the world; the server's ray from `eye` to the prop's position meets
/// nothing (the map, shapes, vehicles, players, items); the prop's box there overlaps nothing (the same, and other props).
/// `ignore` is left out of the ray alone: it must be the object the server itself gives that client to control (its player,
/// `%client.getControlObject()`), never an id the client sent, and an object that is neither a player nor controlled by a
/// connection is refused. Props have no client entry point: a game's own server command (with its declared limits and
/// permission) calls this, then `propSpawn`.
#[function(name = "propCheckPlacement")]
fn prop_check_placement(
    mut vm: VmMut,
    props: Res<ServerProps>,
    datablock: &str,
    transform: &str,
    eye: &str,
    ignore: Option<i32>,
    reach: Option<f32>,
) -> String {
    let data = vm.objects().find(datablock.as_bytes()).unwrap_or(0);
    let check = parse_transform(transform)
        .map_err(|_| PlacementError::Transform)
        .and_then(|pose| {
            let eye = finite_vec(eye, "the eye").map_err(|_| PlacementError::Eye)?;
            let mut placement = Placement::new(data, pose, eye);
            if let Some(o) = ignore
                .and_then(|o| ObjId::try_from(o).ok())
                .filter(|&o| o != 0)
            {
                placement = placement.ignoring(o);
            }
            if let Some(r) = reach {
                placement = placement.within(r);
            }
            props.check_placement(&mut vm, &placement)
        });
    check.err().map_or_else(String::new, |e| e.to_string())
}

/// The natives, as the plugin registers them.
pub(crate) fn declared() -> Vec<torque_engine::script::NativeDecl> {
    torque_engine::natives![
        prop_spawn,
        prop_delete,
        prop_freeze,
        prop_unfreeze,
        prop_apply_impulse,
        prop_set_velocity,
        prop_get_velocity,
        prop_get_spin,
        prop_get_transform,
        prop_is_asleep,
        prop_is_frozen,
        prop_get_owner,
        prop_count,
        prop_awake_count,
        prop_check_placement
    ]
}

/// Why a placement was refused ([`ServerProps::check_placement`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementError {
    /// The datablock is not a `PropData`.
    NotPropData,
    /// The transform is not finite, its rotation is not a turn, or the prop would be out of the world.
    Transform,
    /// The eye is not finite or is out of the world.
    Eye,
    /// The prop's position is farther from the eye than the placement's reach.
    TooFar,
    /// The object to leave out of the ray is neither a player nor controlled by a connection.
    Ignore,
    /// The server's ray from the eye meets something before the prop's position.
    Blocked,
    /// The prop's box there overlaps the map, a shape, a vehicle, a player, an item or another prop.
    NoRoom,
}

impl std::fmt::Display for PlacementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PlacementError::NotPropData => "that is not a prop datablock",
            PlacementError::Transform => "the transform is out of the world",
            PlacementError::Eye => "the eye is out of the world",
            PlacementError::TooFar => "that is out of reach",
            PlacementError::Ignore => {
                "only a player or a controlled object may be left out of the ray"
            }
            PlacementError::Blocked => "something is in the way",
            PlacementError::NoRoom => "there is no room there",
        })
    }
}

impl std::error::Error for PlacementError {}

/// What a placement may not be blocked by, or overlap: the map, static shapes, vehicles, players and items (props are checked
/// against the physics world's own record).
pub const PLACEMENT_BLOCKERS: u32 =
    TERRAIN | INTERIOR | STATIC_SHAPE | STATIC_TS | VEHICLE | PLAYER | ITEM;

/// The farthest a prop may be placed from the eye when a placement names no reach (units).
pub const DEFAULT_REACH: f32 = 32.0;

/// How far from 1 a placement's rotation may be (a quaternion that is not a turn is refused).
const TURN_TOLERANCE: f32 = 1.0e-3;

/// Where a game wants to spawn a prop for a player ([`ServerProps::check_placement`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The `PropData` datablock.
    pub data: ObjId,
    /// The prop's position and its rotation, a unit quaternion of the physics world ([`crate::server::parse_transform`]).
    pub pose: ([f32; 3], [f32; 4]),
    /// The point it is placed from: the player's eye.
    pub eye: [f32; 3],
    /// The object left out of the ray: the one the server gives the player to control, never an id the client sent.
    pub ignore: Option<ObjId>,
    /// The farthest the prop may be from the eye ([`DEFAULT_REACH`]).
    pub reach: f32,
}

impl Placement {
    /// A placement of datablock `data` at `pose` from `eye`, nothing left out of the ray, within [`DEFAULT_REACH`].
    #[must_use]
    pub fn new(data: ObjId, pose: ([f32; 3], [f32; 4]), eye: [f32; 3]) -> Placement {
        Placement {
            data,
            pose,
            eye,
            ignore: None,
            reach: DEFAULT_REACH,
        }
    }

    /// The placement with `object` (the player's own controlled object) left out of the ray.
    #[must_use]
    pub fn ignoring(mut self, object: ObjId) -> Placement {
        self.ignore = Some(object);
        self
    }

    /// The placement within `reach` units of the eye.
    #[must_use]
    pub fn within(mut self, reach: f32) -> Placement {
        self.reach = reach;
        self
    }
}

impl ServerProps {
    /// Whether a prop may be spawned as `placement` says for a player: the checks of `propCheckPlacement`. A game calls it from
    /// its own server command before `propSpawn`; the props' caps are `propSpawn`'s own.
    ///
    /// # Errors
    ///
    /// The first check that fails: [`PlacementError::NotPropData`], [`PlacementError::Transform`] (not finite, a rotation that
    /// is not a turn, out of the world), [`PlacementError::Eye`], [`PlacementError::TooFar`] (past the reach; a reach that is not
    /// a finite positive number refuses every placement), [`PlacementError::Ignore`] (an object left out of the ray that is
    /// neither a player nor controlled), [`PlacementError::Blocked`] (the ray), [`PlacementError::NoRoom`] (the box).
    pub fn check_placement(
        &self,
        vm: &mut Vm,
        placement: &Placement,
    ) -> Result<(), PlacementError> {
        let &Placement {
            data,
            pose: (at, rotation),
            eye,
            ignore,
            reach,
        } = placement;
        if !is_class(vm, data, <PropData as ScriptClass>::NAME) {
            return Err(PlacementError::NotPropData);
        }
        let kind = PropData::read(vm, data).map_err(|_| PlacementError::NotPropData)?;
        let inside = |p: &[f32; 3]| p.iter().all(|v| v.abs() < quant::MAX_COORD);
        let turn = rotation.iter().map(|c| c * c).sum::<f32>().sqrt();
        if !(inside(&at) && (turn - 1.0).abs() < TURN_TOLERANCE) {
            return Err(PlacementError::Transform);
        }
        if !inside(&eye) {
            return Err(PlacementError::Eye);
        }
        let d: [f32; 3] = std::array::from_fn(|k| at[k] - eye[k]);
        let distance = crate::world::length(d);
        if !(reach > 0.0 && distance <= reach) {
            return Err(PlacementError::TooFar);
        }
        if let Some(o) = ignore
            && torque_game::sim::player::state(vm, o).is_none()
            && !torque_game::scene::ghost::is_control_object(vm, o)
        {
            return Err(PlacementError::Ignore);
        }
        if ray_blocked(vm, eye, at, ignore) {
            return Err(PlacementError::Blocked);
        }
        let half = world_half(PropParams::from(&kind).shape.half_extents(), rotation);
        if box_blocked(vm, self, at, half) {
            return Err(PlacementError::NoRoom);
        }
        Ok(())
    }
}

/// The text of a point for a native's argument (finite numbers, as Rust prints them: no exponent).
fn point_text(p: [f32; 3]) -> Vec<u8> {
    format!("{} {} {}", p[0], p[1], p[2]).into_bytes()
}

/// Whether the server's ray from `eye` to `point` meets the map, a shape, a vehicle, a player or an item (`ignore` left out;
/// none is the id 0, which names no object).
fn ray_blocked(vm: &mut Vm, eye: [f32; 3], point: [f32; 3], ignore: Option<ObjId>) -> bool {
    let args = [
        point_text(eye),
        point_text(point),
        PLACEMENT_BLOCKERS.to_string().into_bytes(),
        ignore.unwrap_or(0).to_string().into_bytes(),
    ];
    match vm.invoke(None, b"containerRayCast", &args) {
        Ok(hit) => torque_ts_vm::text::atoi(&hit) != 0,
        // A ray the server could not cast is a refusal.
        Err(_) => true,
    }
}

/// Whether a box of half-extents `half` (along the world's axes) at `at` overlaps the map, a shape, a vehicle, a player or an
/// item (`containerBoxEmpty`), or another prop.
fn box_blocked(vm: &mut Vm, props: &ServerProps, at: [f32; 3], half: [f32; 3]) -> bool {
    let args = [
        PLACEMENT_BLOCKERS.to_string().into_bytes(),
        point_text(at),
        half[0].to_string().into_bytes(),
        half[1].to_string().into_bytes(),
        half[2].to_string().into_bytes(),
    ];
    let empty = match vm.invoke(None, b"containerBoxEmpty", &args) {
        Ok(text) => torque_ts_vm::text::atoi(&text) != 0,
        Err(_) => false,
    };
    if !empty {
        return true;
    }
    let players = player_boxes(vm)
        .iter()
        .any(|(_, min, max)| (0..3).all(|k| at[k] + half[k] > min[k] && at[k] - half[k] < max[k]));
    players
        || props
            .with(|s| {
                s.world.as_ref().is_some_and(|w| {
                    w.props().values().any(|p| {
                        let c = p.q.position();
                        let h = world_half(p.params.shape.half_extents(), p.q.rotation());
                        (0..3).all(|k| (at[k] - c[k]).abs() < half[k] + h[k])
                    })
                })
            })
            .unwrap_or(true)
}
