//! The server's half of joints: the `Joint` console class a joint is (its fields are what [`JointGhost`](super::records::JointGhost)
//! is packed from), making one, writing its record from the world, and the tethers grapples put on players.

use std::rc::Rc;

use torque_engine::{EngineClass, ScriptClass};
use torque_game::player_tether::{Tether, TetherSource};
use torque_ts_vm::fields::Value as Field;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::records::{MASK_FLAGS, MASK_INIT, MASK_LENGTH, MASK_SPRING};
use super::{End, Joint};
use crate::server::{ServerProps, delete_logged, is_class};
use crate::world::PropWorld;

/// The class name as scripts see it.
pub const CLASS: &str = <NetJoint as ScriptClass>::NAME;

/// A joint on the server: the object a server script makes with `jointTether` and kin, which every client of the engine is sent.
/// The fields are the server's record of it, which a ghost is packed from; the physics world holds the truth and the tick writes
/// it here, so a script reads a joint through the `joint*` functions.
#[expect(
    dead_code,
    reason = "the fields are the class's script fields; Rust reads and writes them through the script engine"
)]
#[derive(EngineClass)]
#[class(name = "Joint", parent = "SceneObject")]
pub struct NetJoint {
    /// The kind: 0 tether, 1 winch, 2 weld, 3 hinge, 4 grapple.
    kind: i32,
    /// What holds each end: 1 a point, 2 an object (a prop or a player); the point, the object's id and the point held in the
    /// object's frame (a player's: an offset from its position).
    a_mode: i32,
    a_point: [f32; 3],
    a_object: i32,
    a_offset: [f32; 3],
    b_mode: i32,
    b_point: [f32; 3],
    b_object: i32,
    b_offset: [f32; 3],
    /// The length now and the shortest (a tether's, a winch's, a grapple's).
    length: f32,
    min_length: f32,
    /// The spring past the length (stiffness 0: hard).
    stiffness: f32,
    damping: f32,
    /// 1 while a tether's ends are at its length, 1 once it broke.
    at_limit: i32,
    broken: i32,
    /// The networked rope that shows it (0: none).
    rope: i32,
}

/// The joint `id`, if it is one.
pub(crate) fn joint_of(vm: &Vm, id: i32) -> Result<ObjId, torque_engine::ScriptError> {
    let obj = ObjId::try_from(id).unwrap_or(0);
    if is_class(vm, obj, CLASS) {
        Ok(obj)
    } else {
        Err(torque_engine::ScriptError::new(format!(
            "{id} is not a joint"
        )))
    }
}

fn set(vm: &mut Vm, id: u32, name: &'static [u8], value: Field) {
    torque_game::scene::access::set_field(vm, id, name, value);
}

/// Each end's console fields: its mode, point, object and offset.
const END_FIELDS: [[&[u8]; 4]; 2] = [
    [b"aMode", b"aPoint", b"aObject", b"aOffset"],
    [b"bMode", b"bPoint", b"bObject", b"bOffset"],
];

/// Writes a joint's record fields from the world and marks `mask` of its ghost dirty.
pub(crate) fn write_back(vm: &mut Vm, world: &PropWorld, id: u32, mask: u32) {
    let Some(j) = world.joint(id) else {
        return;
    };
    if mask & MASK_INIT != 0 {
        set(vm, id, b"kind", Field::Int(j.kind.id().number() as i32));
        for ([mode_f, point_f, object_f, offset_f], end) in END_FIELDS.into_iter().zip([j.a, j.b]) {
            let (mode, point, object, offset) = match end {
                End::Point(p) => (1, p, 0, [0.0; 3]),
                End::World => (1, [0.0; 3], 0, [0.0; 3]),
                End::Prop { id: object, local } => (2, [0.0; 3], object, local),
                End::Player { id: object, offset } => (2, [0.0; 3], object, offset),
            };
            set(vm, id, mode_f, Field::Int(mode));
            set(vm, id, point_f, Field::Floats3(point));
            set(vm, id, object_f, Field::Int(object as i32));
            set(vm, id, offset_f, Field::Floats3(offset));
        }
    }
    if mask & (MASK_LENGTH | MASK_SPRING) != 0 {
        let reach = j.kind.reach();
        let (length, min) = reach.map_or((0.0, 0.0), |r| (r.max, r.min));
        let (k, c) = reach
            .and_then(|r| r.spring)
            .map_or((0.0, 0.0), |s| (s.stiffness, s.damping));
        set(vm, id, b"length", Field::Float(length));
        set(vm, id, b"minLength", Field::Float(min));
        set(vm, id, b"stiffness", Field::Float(k));
        set(vm, id, b"damping", Field::Float(c));
    }
    if mask & MASK_FLAGS != 0 {
        set(vm, id, b"atLimit", Field::Int(i32::from(j.status.at_limit)));
        set(vm, id, b"broken", Field::Int(i32::from(j.status.broken)));
    }
    torque_game::scene::ghost::mark(vm, id, mask);
}

/// Makes joint `joint` as a `Joint` object (natively, with no script text), adds it to the world, writes its record and puts it
/// in `MissionCleanup` when there is one; its object id.
pub(crate) fn make(vm: &mut Vm, props: &ServerProps, joint: Joint) -> Result<u32, String> {
    // A prop hung on a parent has no joints, even before the tick makes it a child of the physics world.
    if joint
        .props()
        .any(|p| torque_game::scene::links::parent_of(vm, p).is_some())
    {
        return Err(super::JointError::Child.to_string());
    }
    // The caps and the ends are checked before the object is made, and again by the world.
    {
        let mut srv = props.0.borrow_mut();
        let world = srv.world_mut(vm);
        world
            .may_join(joint.kind.id(), &joint.a, &joint.b, joint.owner.as_ref())
            .map_err(|e| e.to_string())?;
    }
    let made = vm
        .new_object(CLASS.as_bytes(), &[])
        .map_err(|e| e.to_string())?;
    let Some(id) = made.filter(|&id| is_class(vm, id, CLASS)) else {
        return Err("the joint object was not made".to_string());
    };
    let added = props.0.borrow_mut().world_mut(vm).add_joint(id, joint);
    let mask = match added {
        Ok(mask) => mask,
        Err(e) => {
            delete_logged(vm, id);
            return Err(e.to_string());
        }
    };
    if let Ok(srv) = props.0.try_borrow()
        && let Some(world) = &srv.world
    {
        write_back(vm, world, id, mask);
    }
    if let Some(cleanup) = vm.objects().find(b"MissionCleanup")
        && let Err(e) = vm.call_method(cleanup, b"add", &[id.to_string().into_bytes()])
    {
        vm.error(format!("joint: MissionCleanup.add: {e}"));
    }
    Ok(id)
}

/// Deletes the objects of joints the world took out, and the ropes that showed them.
pub(crate) fn delete_objects(vm: &mut Vm, gone: &[(u32, u32)]) {
    for &(joint, rope) in gone {
        if rope != 0 && vm.objects().contains(rope) {
            delete_logged(vm, rope);
        }
        if is_class(vm, joint, CLASS) {
            delete_logged(vm, joint);
        }
    }
}

/// The grapples' tethers on the server's players, from the props' world.
pub struct ServerTethers(pub ServerProps);

impl TetherSource for ServerTethers {
    fn tether(&self, player: u32) -> Option<Tether> {
        self.0
            .with(|s| s.world.as_ref()?.tether_of(player))
            .flatten()
    }

    fn pulled(&self, player: u32, change: [f32; 3], mass: f32) {
        if let Ok(mut s) = self.0.0.try_borrow_mut()
            && let Some(world) = s.world.as_mut()
        {
            world.pulled(player, change, mass);
        }
    }
}

/// The server's tether source, for [`torque_game::player_tether::add`].
pub fn tethers(props: &ServerProps) -> Rc<dyn TetherSource> {
    Rc::new(ServerTethers(props.clone()))
}
