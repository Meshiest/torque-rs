//! The AIPlayer's console methods: TGE 1.4's nine (`stop`, `clearAim`, `setMoveSpeed`, `setMoveDestination`,
//! `getMoveDestination`, `setAimLocation`, `getAimLocation`, `setAimObject` and `getAimObject`) and two that come from
//! Torque3D (`setMoveTolerance` and `getMoveSpeed`). Each parses its arguments and applies the call to the AIPlayer's
//! [`AiState`] (`sim`'s `ai::console`), which the Player's tick reads to build the AIPlayer's move (`getAIMove`) and to call
//! `onReachDestination`, `onMoveStuck`, `onTargetEnterLOS` and `onTargetExitLOS` on its datablock. An AIPlayer gets its state
//! at its first call; until then it stands, as a stopped one does.
//!
//! Credit: the methods' names, arguments and defaults follow Torque3D's MIT-licensed `T3D/aiPlayer.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this crate's own.

use ::sim::ai::{AiOp, AiState};
use collide::ObjectId;
use ts_vm::fields::atob;
use ts_vm::vm::{Native, NativeCall, Vm};

use super::methods::{arg, f, text3, vec3};
use super::state_mut;
use crate::methods::register_method;
use crate::scene::camera::is_class;

/// The AIPlayer's state, made at its first call. None: the object is not an AIPlayer (or has no Player state).
fn ai_mut<'a>(vm: &'a mut Vm, c: &NativeCall) -> Option<&'a mut AiState> {
    let id = c.this_object?;
    if !is_class(vm, id, "AIPlayer") {
        return None;
    }
    Some(state_mut(vm, id)?.ai.get_or_insert_with(Box::default))
}

/// Applies `op` to the AIPlayer the call is on.
fn apply(vm: &mut Vm, c: &NativeCall, op: AiOp) -> Native {
    if let Some(ai) = ai_mut(vm, c) {
        op.apply(ai);
    }
    Native::Void
}

/// What `read` gives of the AIPlayer's state (its defaults before its first call).
fn read<T>(vm: &mut Vm, c: &NativeCall, field: impl FnOnce(&AiState) -> T) -> T {
    match ai_mut(vm, c) {
        Some(ai) => field(ai),
        None => field(&AiState::new()),
    }
}

pub fn register(vm: &mut Vm) {
    register_method(vm, "AIPlayer", "stop", |vm, c| apply(vm, c, AiOp::Stop));
    register_method(vm, "AIPlayer", "clearAim", |vm, c| {
        apply(vm, c, AiOp::ClearAim)
    });
    register_method(vm, "AIPlayer", "setMoveSpeed", |vm, c| {
        apply(vm, c, AiOp::SetMoveSpeed(f(c, 2)))
    });
    register_method(vm, "AIPlayer", "getMoveSpeed", |vm, c| {
        Native::Float(f64::from(read(vm, c, |ai| ai.move_speed)))
    });
    register_method(vm, "AIPlayer", "setMoveTolerance", |vm, c| {
        apply(vm, c, AiOp::SetMoveTolerance(f(c, 2)))
    });
    // `setMoveDestination(goal, slowdown = true)`.
    register_method(vm, "AIPlayer", "setMoveDestination", |vm, c| {
        let slowdown = c.argv.len() <= 3 || atob(arg(c, 3));
        apply(vm, c, AiOp::SetMoveDestination(vec3(c, 2), slowdown))
    });
    register_method(vm, "AIPlayer", "getMoveDestination", |vm, c| {
        Native::Str(text3(read(vm, c, AiState::move_destination), false))
    });
    register_method(vm, "AIPlayer", "setAimLocation", |vm, c| {
        apply(vm, c, AiOp::SetAimLocation(vec3(c, 2)))
    });
    register_method(vm, "AIPlayer", "getAimLocation", |vm, c| {
        Native::Str(text3(read(vm, c, AiState::aim_location), false))
    });
    // `setAimObject(obj, offset = "0 0 0")`: a GameBase by name or id; anything else aims at nothing.
    register_method(vm, "AIPlayer", "setAimObject", |vm, c| {
        let target = vm
            .objects()
            .find(arg(c, 2))
            .filter(|t| is_class(vm, *t, "GameBase"))
            .map(ObjectId);
        apply(vm, c, AiOp::SetAimObject(target, vec3(c, 3)))
    });
    // `getAimObject()`: the id, -1 for none (or one that is gone).
    register_method(vm, "AIPlayer", "getAimObject", |vm, c| {
        let aimed = read(vm, c, AiState::aim_object_id).filter(|o| vm.objects().contains(o.0));
        Native::Int(aimed.and_then(|o| i32::try_from(o.0).ok()).unwrap_or(-1))
    });
}
