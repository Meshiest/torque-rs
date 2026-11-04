//! The client's move each tick, as TGE's `GameConnection::getNextMove` and `Move::clamp` make it from the move variables
//! (`$mv*`) that the move map's binds set.
//!
//! **getNextMove.** No move while the connection holds more than 45 unacknowledged moves. Then (in floats): `pitch = $mvPitch +
//! ($mvPitchUpSpeed - $mvPitchDownSpeed)`, `yaw = $mvYaw + ($mvYawLeftSpeed - $mvYawRightSpeed)`, `roll = $mvRoll +
//! ($mvRollRightSpeed - $mvRollLeftSpeed)`, and `$mvPitch`, `$mvYaw`, `$mvRoll` go back to 0; `x = $mvRightAction - $mvLeftAction`,
//! `y = $mvForwardAction - $mvBackwardAction`, `z = $mvUpAction - $mvDownAction`; `freeLook = $mvFreeLook`; trigger `i` is set when
//! `$mvTriggerCount<i>` is odd, or when the count changed and the last one was even (a press and release between two moves still
//! fires once); the last counts are kept.
//!
//! **clamp.** Each angle `FANG2IANG`: `(S16)(65536 / 2pi * a)`, the product truncated toward zero and kept to its low 16 bits (a
//! whole turn wraps; a NaN or an infinity is 0); each axis `clampRangeClamp`: `0` below -1, `32` above 1, else `(S32)((v + 1) * 16)`;
//! then `Move::finish` (torque-net's) makes the floats `processTick` reads from the raw values on both sides.
//!
//! **The game's rules** ([`MoveRules`], a `Vm` extension: [`set_move_rules`]) may make the angles' sums and the two clamps their
//! own way; TGE's ([`TgeMoves`]) without them.
//!
//! Credit: follows TGE's move manager as Torque3D carries it (MIT: `T3D/gameBase/moveManager.cpp`, Copyright (c) 2012
//! GarageGames, LLC; `T3D/gameBase/gameConnection.cpp`, Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic,
//! Inc.).

use std::fmt::Debug;
use std::rc::Rc;

use net::moves::{MAX_TRIGGER_KEYS, Move};
use ts_vm::vm::Vm;

/// More unacknowledged moves than this and `getNextMove` makes none.
pub const MAX_PENDING_MOVES: usize = 45;
/// `FANG2IANG`'s scale: `65536 / 2pi` in double precision.
pub const ANGLE_SCALE: f64 = 65_536.0 / std::f64::consts::TAU;

/// The trigger counts the last move saw (TGE's `mPrevTriggerCount`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TriggerMemory(pub [u32; MAX_TRIGGER_KEYS]);

/// How a game makes a move of the move variables (a `Vm` extension: [`set_move_rules`]). Every method has TGE's answer by default.
pub trait MoveRules: Debug {
    /// An angle: the mouse's turn plus the keys' speeds, `turn + (plus - minus)` (TGE's: in floats).
    fn angle(&self, turn: f32, plus: f32, minus: f32) -> f32 {
        turn + (plus - minus)
    }

    /// `Move::clamp`'s angle, its raw 16 bits (TGE's [`clamp_angle`]).
    fn clamp_angle(&self, a: f32) -> u32 {
        clamp_angle(a)
    }

    /// `Move::clamp`'s axis, its raw 6 bits (TGE's [`clamp_axis`]).
    fn clamp_axis(&self, v: f32) -> u32 {
        clamp_axis(v)
    }
}

/// TGE's rules.
#[derive(Debug, Default)]
pub struct TgeMoves;

impl MoveRules for TgeMoves {}

struct Installed(Rc<dyn MoveRules>);

/// The game's move rules for `vm` from now on.
pub fn set_move_rules(vm: &mut Vm, rules: Rc<dyn MoveRules>) {
    vm.set_extension(Rc::new(Installed(rules)));
}

/// The move rules `vm`'s game set, else TGE's.
pub fn move_rules(vm: &Vm) -> Rc<dyn MoveRules> {
    vm.extension::<Installed>().map_or_else(
        || Rc::new(TgeMoves) as Rc<dyn MoveRules>,
        |i| Rc::clone(&i.0),
    )
}

/// `FANG2IANG`: `(S16)(65536 / 2pi * a)`, the product truncated toward zero and kept to its low 16 bits (a NaN or an infinity is 0).
pub fn clamp_angle(a: f32) -> u32 {
    let v = (f64::from(a) * ANGLE_SCALE).trunc();
    if !v.is_finite() {
        return 0;
    }
    ((v % 65_536.0) as i32 as u32) & 0xffff
}

/// `clampRangeClamp`: `0` below -1, `32` above 1, else `(S32)((v + 1) * 16)` in floats (a NaN is 0).
pub fn clamp_axis(v: f32) -> u32 {
    if v < -1.0 {
        0
    } else if v > 1.0 {
        32
    } else {
        ((v + 1.0) * 16.0) as i32 as u32
    }
}

/// The move variables a move is made of, as they stand (TGE's `MoveManager` statics).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MoveVars {
    /// `$mvYaw`, `$mvPitch`, `$mvRoll`: the mouse's turn since the last move.
    pub turn: [f32; 3],
    /// The keys' turn speeds of each angle, plus and minus: `$mvYawLeftSpeed` and `$mvYawRightSpeed`, `$mvPitchUpSpeed` and
    /// `$mvPitchDownSpeed`, `$mvRollRightSpeed` and `$mvRollLeftSpeed`.
    pub speeds: [[f32; 2]; 3],
    /// The axes' actions, plus and minus: `$mvRightAction` and `$mvLeftAction`, `$mvForwardAction` and `$mvBackwardAction`,
    /// `$mvUpAction` and `$mvDownAction`.
    pub actions: [[f32; 2]; 3],
    /// `$mvFreeLook`.
    pub free_look: bool,
}

impl MoveVars {
    pub fn of(vm: &Vm) -> MoveVars {
        let f = |name: &str| vm.global_f32(name).unwrap_or(0.0);
        let pair = |a: &str, b: &str| [f(a), f(b)];
        MoveVars {
            turn: [f("$mvYaw"), f("$mvPitch"), f("$mvRoll")],
            speeds: [
                pair("$mvYawLeftSpeed", "$mvYawRightSpeed"),
                pair("$mvPitchUpSpeed", "$mvPitchDownSpeed"),
                pair("$mvRollRightSpeed", "$mvRollLeftSpeed"),
            ],
            actions: [
                pair("$mvRightAction", "$mvLeftAction"),
                pair("$mvForwardAction", "$mvBackwardAction"),
                pair("$mvUpAction", "$mvDownAction"),
            ],
            free_look: vm.global_int("$mvFreeLook").unwrap_or(0) & 0xff != 0,
        }
    }

    /// The move's yaw, pitch and roll by `rules`; without the mouse's turn when `mouse` is false.
    pub fn angles(&self, rules: &dyn MoveRules, mouse: bool) -> [f32; 3] {
        [0, 1, 2].map(|k| {
            let turn = if mouse { self.turn[k] } else { 0.0 };
            rules.angle(turn, self.speeds[k][0], self.speeds[k][1])
        })
    }

    /// The move's x, y and z: each axis's `plus - minus`.
    pub fn axes(&self) -> [f32; 3] {
        self.actions.map(|[plus, minus]| plus - minus)
    }
}

/// The move of this tick, or `None` when `pending` moves are already waiting for the server (more than [`MAX_PENDING_MOVES`]).
/// The first `keys` triggers are read (the move format's, at most [`MAX_TRIGGER_KEYS`]); the others stay clear.
pub fn next_move(
    vm: &mut Vm,
    last: &mut TriggerMemory,
    pending: usize,
    keys: usize,
) -> Option<Move> {
    if pending > MAX_PENDING_MOVES {
        return None;
    }
    let vars = MoveVars::of(vm);
    for name in ["$mvPitch", "$mvYaw", "$mvRoll"] {
        vm.set_global(name, "0");
    }
    let rules = move_rules(vm);
    let [yaw, pitch, roll] = vars.angles(&*rules, true);
    let [x, y, z] = vars.axes();
    let mut triggers = [false; MAX_TRIGGER_KEYS];
    for (i, t) in triggers.iter_mut().enumerate().take(keys) {
        let count = vm.global_int(&format!("$mvTriggerCount{i}")).unwrap_or(0);
        let prev = last.0[i];
        *t = count & 1 != 0 || (prev & 1 == 0 && prev != count);
        last.0[i] = count;
    }
    Some(Move::from_raw(
        [
            rules.clamp_axis(x),
            rules.clamp_axis(y),
            rules.clamp_axis(z),
        ],
        [
            rules.clamp_angle(yaw),
            rules.clamp_angle(pitch),
            rules.clamp_angle(roll),
        ],
        vars.free_look,
        triggers,
    ))
}

#[cfg(test)]
#[path = "moves_tests.rs"]
mod tests;
