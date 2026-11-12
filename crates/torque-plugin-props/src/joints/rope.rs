//! Ropes that show joints (feature `rope`): `jointRope(%joint, %ropeData)` makes a networked rope ([`torque_plugin_rope`]) bound
//! to the rope driver [`DRIVER`], and each client's driver ([`JointRopeDriver`]) puts its ends at the joint's ends as the client
//! draws them, gives it the joint's length, and draws it straight when the joint is at its limit. The rope is a visual: it pulls on
//! nothing, the joint does. A rope goes with its joint.

use std::rc::Rc;

use torque_engine::args::{obj_id, vec3_text};
use torque_engine::{EngineBuilder, Res, ScriptError, VmMut, function};
use torque_plugin_rope::{Drive, DriveEnd, RopeDriver, RopeDrivers};
use torque_ts_vm::vm::Vm;

use super::client::ClientJoints;
use super::records::EndWire;
use super::server::joint_of;
use crate::server::ServerProps;

/// The name the joints' rope driver is registered under (looked up whatever its case: through the string table it may arrive as
/// the class's `Joint`).
pub const DRIVER: &str = "joint";

/// A client's driver of the ropes that show joints: it is asked with the client's ghost of the joint.
#[derive(Clone, Debug)]
pub struct JointRopeDriver(pub ClientJoints);

fn drive_end(end: &EndWire) -> DriveEnd {
    match *end {
        EndWire::Point(p) => DriveEnd::Point(p),
        EndWire::Ghost { ghost, offset } => DriveEnd::Ghost {
            ghost,
            node: String::new(),
            offset,
        },
        EndWire::Free | EndWire::Waiting => DriveEnd::Free,
    }
}

impl RopeDriver for JointRopeDriver {
    /// The joint's ends, its length and its limit; a broken joint's rope hangs from its first end. None for a joint the client
    /// does not hold (the rope is let go).
    fn drive(&self, _vm: &Vm, key: u32) -> Option<Drive> {
        let j = self.0.with(|b| b.get(key).copied()).flatten()?;
        let end = if j.broken {
            DriveEnd::Free
        } else {
            drive_end(&j.b)
        };
        let mut drive = Drive::new(drive_end(&j.a), end);
        if j.kind.has_reach() {
            drive.length = Some(j.length);
            drive.settle = torque_physics_rapier::TICK;
        }
        drive.at_limit = j.at_limit && !j.broken;
        Some(drive)
    }
}

/// makes a networked rope of `ropeData` that shows a joint (its ends where the joint's are as each client draws them, its length
/// the joint's, straight while the joint is at its limit) and returns the rope's object id
///
/// The rope pulls on nothing. It goes with the joint; a second call replaces the first rope.
#[function(name = "jointRope")]
fn joint_rope(
    mut vm: VmMut,
    props: Res<ServerProps>,
    joint: i32,
    rope_data: &str,
) -> Result<i32, ScriptError> {
    let id = joint_of(&vm, joint)?;
    let (a, b, old) = props
        .with(|s| {
            let w = s.world.as_ref()?;
            let j = w.joint(id)?;
            Some((w.end_point(&j.a), w.end_point(&j.b), j.rope))
        })
        .flatten()
        .ok_or_else(|| ScriptError::new(format!("{joint} is not a joint")))?;
    let text = |p: Option<[f32; 3]>| vec3_text(p.unwrap_or_default()).into_bytes();
    let made = vm
        .invoke(
            None,
            b"ropeNetCreate",
            &[text(a), text(b), rope_data.as_bytes().to_vec()],
        )
        .map_err(|e| ScriptError::new(e.to_string()))?;
    let rope = torque_ts_vm::text::atoi(&made);
    let rope_id = obj_id(rope);
    if rope_id == 0 {
        return Err(ScriptError::new("the rope was not made"));
    }
    torque_plugin_rope::net::server::set_driver(&mut vm, rope_id, DRIVER, id)?;
    if old != 0 && old != rope_id && vm.objects().contains(old) {
        crate::server::delete_logged(&mut vm, old);
    }
    if let Some(world) = props.0.borrow_mut().world.as_mut() {
        world.set_rope(id, rope_id);
    }
    torque_game::scene::access::set_field(
        &mut vm,
        id,
        b"rope",
        torque_ts_vm::fields::Value::Int(rope),
    );
    Ok(rope)
}

/// The joints' rope driver on each client (when the engine has the rope plugin) and `jointRope` on the server.
pub(crate) fn register(engine: &mut EngineBuilder, joints: &ClientJoints) {
    let driver: Rc<dyn RopeDriver> = Rc::new(JointRopeDriver(joints.clone()));
    engine.client(torque_engine::ClientStage::Natives, move |vm, engine| {
        match engine.resource::<RopeDrivers>() {
            // A client made again from the engine finds this driver there already; another plugin's under the name is said.
            Some(d) => {
                if !d.register_shared(DRIVER, Rc::clone(&driver)) && !d.is_registered_as(DRIVER, &driver) {
                    vm.warn(format!(
                        "props: another plugin's rope driver is registered as {DRIVER:?}: jointRope's ropes follow it, not their joints"
                    ));
                }
            }
            None => vm.warn(
                "props: jointRope's ropes need the rope plugin on the client; they are drawn as they were made".to_string(),
            ),
        }
        Ok(())
    });
    engine
        .server_script()
        .natives(torque_engine::natives![joint_rope]);
}
