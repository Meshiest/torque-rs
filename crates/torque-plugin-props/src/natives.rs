//! The server's console functions (`propSpawn` and kin) and the command a client spawns a prop with (`serverCmdPropSpawn`),
//! checked on the server.

use std::rc::Rc;

use torque_collide::types::{INTERIOR, ITEM, PLAYER, STATIC_SHAPE, STATIC_TS, TERRAIN, VEHICLE};
use torque_engine::args::vec3;
use torque_engine::command::{ArgType, CommandSpec};
use torque_engine::{Res, ScriptClass, ScriptError, VmMut, function};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use crate::data::{PropData, PropParams};
use crate::quant;
use crate::server::{
    SPAWN_RATE, ServerProps, address_of, axis_angle, is_class, owner_key, owner_of,
    parse_transform, player_boxes, prop_of, spawn, write_back,
};
use crate::world::{Prop, PropWorld, world_half};

/// How far in front of a player's eye `serverCmdPropSpawn` puts a prop (units).
pub const SPAWN_REACH: f32 = 2.5;

/// What a client's spawn point may not be blocked by, or overlap: the map, static shapes, vehicles, players and items (props
/// are checked against the physics world's own record).
pub const SPAWN_BLOCKERS: u32 =
    TERRAIN | INTERIOR | STATIC_SHAPE | STATIC_TS | VEHICLE | PLAYER | ITEM;

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
/// MaxPerAddress`), and its props outlive its leaving by `$Pref::Props::OrphanSeconds`. `predictId` (1 to 255) ties the prop to
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
        prop_awake_count
    ]
}

/// The object a client controls (its `getControlObject()`), if it is one.
pub(crate) fn control_of(vm: &mut Vm, client: ObjId) -> Option<ObjId> {
    let text = vm.call_method(client, b"getControlObject", &[]).ok()?;
    let id = torque_ts_vm::text::atof(&text) as i64;
    u32::try_from(id)
        .ok()
        .filter(|&o| o != 0 && vm.objects().contains(o))
}

/// A player's eye (at 85% of its box's height over its position) and where `serverCmdPropSpawn` puts a prop for it: in front of
/// the eye along the way the player faces, at the eye's height. None when it is not a player.
fn eye_and_point(vm: &Vm, player: ObjId) -> Option<([f32; 3], [f32; 3])> {
    let state = torque_game::sim::player::state(vm, player)?;
    let m = state.obj_to_world;
    let b = state.world_box;
    let eye = [m[3], m[7], b.min[2] + (b.max[2] - b.min[2]) * 0.85];
    let forward = [m[1], m[5], 0.0];
    let len = (forward[0] * forward[0] + forward[1] * forward[1]).sqrt();
    if !(len > 1.0e-4) {
        return None;
    }
    let point = [0, 1, 2].map(|k| eye[k] + forward[k] / len * SPAWN_REACH);
    Some((eye, point))
}

/// The text of a point for a native's argument (finite numbers, as Rust prints them: no exponent).
fn point_text(p: [f32; 3]) -> Vec<u8> {
    format!("{} {} {}", p[0], p[1], p[2]).into_bytes()
}

/// Whether the server's ray from `eye` to `point` meets the map, a shape, a vehicle, a player or an item (the player `own`
/// left out).
fn ray_blocked(vm: &mut Vm, eye: [f32; 3], point: [f32; 3], own: ObjId) -> bool {
    let args = [
        point_text(eye),
        point_text(point),
        SPAWN_BLOCKERS.to_string().into_bytes(),
        own.to_string().into_bytes(),
    ];
    match vm.invoke(None, b"containerRayCast", &args) {
        Ok(hit) => torque_ts_vm::text::atoi(&hit) != 0,
        // A ray the server could not cast is a refusal.
        Err(_) => true,
    }
}

/// Whether a box of half-extents `half` at `at` overlaps the map, a shape, a vehicle, a player or an item
/// (`containerBoxEmpty`), or another prop.
fn box_blocked(vm: &mut Vm, props: &ServerProps, at: [f32; 3], half: [f32; 3]) -> bool {
    let args = [
        SPAWN_BLOCKERS.to_string().into_bytes(),
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

/// Tells a client its spawn was refused (`clientCmdPropRefused(%reason)`) and prints a line, at most one a second.
fn refuse(vm: &mut Vm, props: &ServerProps, client: ObjId, why: &str) {
    let (line, tag) = {
        let mut srv = props.0.borrow_mut();
        let due = srv.tick >= srv.refused_at;
        if due {
            srv.refused_at = srv.tick + 1000 / 32;
        }
        let tag = match srv.refused_tag {
            Some(tag) => Some(tag),
            None => {
                let tag = vm.tags_mut().add_string(b"PropRefused");
                srv.refused_tag = tag;
                tag
            }
        };
        (due, tag)
    };
    if line {
        vm.error(format!("serverCmdPropSpawn: client {client}: {why}"));
    }
    if let Some(tag) = tag {
        let args = [
            client.to_string().into_bytes(),
            format!("\u{1}{tag}").into_bytes(),
            why.as_bytes().to_vec(),
        ];
        let _ = vm.invoke(None, b"commandToClient", &args);
    }
}

/// The command a client sends to spawn a prop: `commandToServer('PropSpawn', %datablock [, %predictId])`.
///
/// The server checks everything: its address's spawn rate (the command's own, kept per address so a player cannot spend it
/// again by connecting again), the datablock is a `PropData` with `clientSpawn = true`, the client controls a player, the point
/// (in front of the player's eye, never one the client sends) is inside the world, the server's ray from the eye reaches it,
/// the prop's box there overlaps nothing (the map, shapes, vehicles, players, items, props), and the caps. A refusal prints an
/// error (at most one a second) and sends the client `clientCmdPropRefused(%reason)`.
pub(crate) fn cmd_prop_spawn(
    mut vm: VmMut,
    props: Res<ServerProps>,
    client: i32,
    datablock: String,
    predict_id: Option<i32>,
) {
    let Ok(client) = ObjId::try_from(client) else {
        return;
    };
    if !vm.objects().contains(client) {
        return;
    }
    let address = address_of(&mut vm, client);
    if !props.0.borrow_mut().take_spawn(&address) {
        return;
    }
    if let Err(why) = check_and_spawn(&mut vm, &props, client, &address, &datablock, predict_id) {
        refuse(&mut vm, &props, client, &why);
    }
}

/// The command's checks, then the spawn; the reason when it is refused.
fn check_and_spawn(
    vm: &mut Vm,
    props: &ServerProps,
    client: ObjId,
    address: &Rc<str>,
    datablock: &str,
    predict_id: Option<i32>,
) -> Result<(), String> {
    let data = vm
        .objects()
        .find(datablock.as_bytes())
        .filter(|&d| is_class(vm, d, <PropData as ScriptClass>::NAME))
        .ok_or("that is not a prop datablock")?;
    let kind = PropData::read(vm, data).map_err(|_| "that is not a prop datablock")?;
    if !kind.client_spawn {
        return Err("players may not spawn that prop".into());
    }
    let player = control_of(vm, client).ok_or("you control no player")?;
    let (eye, at) = eye_and_point(vm, player).ok_or("you control no player")?;
    if at.iter().any(|v| !(v.abs() < quant::MAX_COORD)) {
        return Err("the point is out of the world".into());
    }
    if ray_blocked(vm, eye, at, player) {
        return Err("something is in the way".into());
    }
    let half = PropParams::from(&kind).shape.half_extents();
    if box_blocked(vm, props, at, half) {
        return Err("there is no room there".into());
    }
    let mut owner = owner_of(vm, client);
    owner.address = Some(Rc::clone(address));
    let predict = predict_id.filter(|p| (1..=255).contains(p)).unwrap_or(0) as u8;
    spawn(
        vm,
        props,
        data,
        (at, [0.0, 0.0, 0.0, 1.0]),
        Some(owner),
        predict,
    )
    .map(|_| ())
}

/// The command's limits: a datablock name and an optional predict id, 4 a second after a burst of 8 per connection (and
/// per address, [`SPAWN_RATE`]).
pub fn spawn_spec() -> CommandSpec {
    CommandSpec::new()
        .arg("datablock", ArgType::Text)
        .optional("predictId", ArgType::Int)
        .rate(SPAWN_RATE.0, SPAWN_RATE.1)
        .max_arg_bytes(64)
        .describe("spawns a prop in front of the client's player")
}
