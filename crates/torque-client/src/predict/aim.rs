//! Smooth aim (an engine option a game turns on, [`super::game::PredictGame::smooth_aim`]): the control object's view drawn ahead of
//! its last tick by what the next move will do to it, so the view follows the mouse within the frame instead of a tick or two
//! behind it.
//!
//! The frame's angles are `next(full) - back * (next(rest) - latest)`: `latest` the last tick's yaw and head, `full` the move the
//! next tick will make of the move globals as they stand (the mouse's waiting `$mvYaw`/`$mvPitch` plus the keys' turn speeds,
//! made and quantised as `getNextMove` and `Move::clamp` make them, [`crate::net::moves`]), `rest` the same move without the
//! mouse's waiting turn, and `back` the frame's place between the last tick (0) and the one before (1). Right after a tick (`back`
//! 1, nothing waiting) the view is the latest tick's; just before the next one (`back` 0) it is what that tick's move makes, so the
//! view never steps when the move lands, in any mode. What changes with no mouse turn (the keys' turn, the head going back to the
//! body after free look) is drawn a tick ahead between the ticks, as the mouse's turn is.
//!
//! `next` is the Player's part of a move as the step runs it, the game's ([`super::game::PredictGame::next_angles`]); TGE's
//! ([`next`]) is `Player::processTick`'s driver split (with free look the Player keeps the look angles, else the control object
//! takes the move) and `updateMove`'s head and rotation. The moves and the prediction are not touched: these are copies of the
//! state's numbers. [`mount_frame`] does the same for the Player the rider controls (a mount, a turret), whose angles the view
//! rides on.
//!
//! Credit: follows TGE's Player and vehicle moves as Torque3D carries them (MIT: `T3D/player.cpp`, Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.; `T3D/vehicles/vehicle.cpp`, Copyright (c) 2012 GarageGames, LLC).

use sim::player::{PlayerData, PlayerState};

use super::game::{GamePrefs, PredictGame, PredictWorld};
use crate::net::moves::{MoveRules, MoveVars};

/// The next move as the frame's move globals would make it, with and without the mouse's waiting turn.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Waiting {
    pub full: sim::Move,
    pub rest: sim::Move,
}

impl Waiting {
    /// The move globals of `vm` as they stand, made into moves by its game's rules ([`crate::net::moves::move_rules`]).
    pub fn of(vm: &ts_vm::vm::Vm) -> Waiting {
        Waiting::new(&MoveVars::of(vm), &*crate::net::moves::move_rules(vm))
    }

    /// The moves `vars` make by `rules`: the axes `x` and `y`, the yaw and the pitch, and free look.
    pub fn new(vars: &MoveVars, rules: &dyn MoveRules) -> Waiting {
        let make = |mouse: bool| {
            let [yaw, pitch, _] = vars.angles(rules, mouse);
            let [x, y, _] = vars.axes();
            let mv = net::moves::Move::from_raw(
                [rules.clamp_axis(x), rules.clamp_axis(y), 16],
                [rules.clamp_angle(yaw), rules.clamp_angle(pitch), 0],
                vars.free_look,
                [false; 6],
            );
            sim::Move {
                x: mv.x,
                y: mv.y,
                z: mv.z,
                yaw: mv.yaw,
                pitch: mv.pitch,
                roll: mv.roll,
                free_look: mv.free_look,
                triggers: mv.triggers,
            }
        };
        Waiting {
            full: make(true),
            rest: make(false),
        }
    }

    /// The next move as `processTick` will read it (`mouse` false: without the mouse's waiting turn).
    pub fn next_move(&self, mouse: bool) -> sim::Move {
        if mouse { self.full } else { self.rest }
    }
}

/// The driven vehicle's part of the next move: its steering and the datablock fields the move reads.
#[derive(Debug, Clone, Copy)]
pub struct Steer {
    /// Whether the vehicle is the Player's control object (`controlled_info(..).vehicle`) and its mount.
    pub controls: bool,
    pub mounted_on: bool,
    pub steering: [f32; 2],
    pub max_angle: f32,
    /// The game's steering rules ([`super::game::SteerRules`]): strafe steering and its rate.
    pub strafe: bool,
    pub strafe_rate: f32,
    /// The auto-return, its rate and top speed, and the controlling client's auto-return flag
    /// ([`super::game::PredictGame::client_auto_return`]).
    pub auto_return: bool,
    pub return_rate: f32,
    pub return_speed: f32,
    pub client_return: bool,
}

/// What the next move reads besides the state and the move.
#[derive(Debug, Clone, Copy)]
pub struct Ctx {
    /// The camera distance (`freelook_value`: 0 in the first person).
    pub camera_pos: f32,
    /// The game's preferences as the frame read them.
    pub prefs: GamePrefs,
    pub drive: Option<Steer>,
    /// The control object is a Player or AIPlayer (a turret the Player rides): the move's turn goes to it, not to the Player.
    pub controls_player: bool,
}

fn add(a: f32, b: f32) -> f32 {
    (f64::from(a) + f64::from(b)) as f32
}

/// TGE's yaw and head of the control Player after `mv` (the Player's part of its next tick): `Player::processTick`'s driver split
/// (a Player with a control object keeps only the look angles of a free-look move) and `updateMove`'s head and rotation (the
/// pitch held to the look angles; with free look, mounted on node 0 or in the third person, the head turns within
/// `maxFreelookAngle`; else the body turns and the head eases halfway back, its pitch too while it controls an object). The drawn
/// yaw is not wrapped into one turn (a whole turn draws the same).
pub fn next(s: &PlayerState, db: &PlayerData, mv: &sim::Move, ctx: &Ctx) -> (f32, [f32; 3]) {
    let mut head = s.head;
    let mut yaw = s.rot[2];
    if s.damage_state != 0 {
        return (yaw, head);
    }
    let n = match s.control_object {
        Some(_) if mv.free_look => sim::Move {
            yaw: mv.yaw,
            pitch: mv.pitch,
            roll: mv.roll,
            free_look: true,
            ..sim::Move::default()
        },
        Some(_) => sim::Move::default(),
        None => *mv,
    };
    head[0] = held(head[0] + n.pitch, db.min_look_angle, db.max_look_angle);
    let third_person = ctx.camera_pos != 0.0;
    if n.free_look && ((s.mount.is_some() && s.mount_node == 0) || third_person) {
        let max = db.max_freelook_angle;
        head[2] = held(head[2] + n.yaw, -max, max);
    } else {
        yaw += n.yaw;
        head[2] *= 0.5;
        if s.control_object.is_some() {
            head[0] *= 0.5;
        }
    }
    (yaw, head)
}

/// `mClampF`: below `low` is `low`, above `high` is `high`.
fn held(v: f32, low: f32, high: f32) -> f32 {
    if v < low {
        low
    } else if v > high {
        high
    } else {
        v
    }
}

/// The frame's yaw and head: `next(full) - back * (next(rest) - latest)` (see the module's notes).
pub fn frame<W: PredictWorld>(
    game: &dyn PredictGame<W>,
    s: &PlayerState,
    db: &PlayerData,
    waiting: &Waiting,
    back: f32,
    ctx: &Ctx,
) -> (f32, [f32; 3]) {
    let b = back.clamp(0.0, 1.0);
    let (fy, fh) = game.next_angles(s, db, &waiting.next_move(true), ctx);
    let (ry, rh) = game.next_angles(s, db, &waiting.next_move(false), ctx);
    (
        fy - b * (ry - s.rot[2]),
        [0, 1, 2].map(|k| fh[k] - b * (rh[k] - s.head[k])),
    )
}

/// TGE's yaw and head pitch a move gives the Player its rider controls (`Player::processTick`'s driver split hands it the control
/// move): the whole move's, none of it with free look held (TGE's split does not read the camera distance `camera_pos`).
pub fn mount_move(mv: &sim::Move, camera_pos: f32) -> [f32; 2] {
    let _ = camera_pos;
    if mv.free_look {
        [0.0, 0.0]
    } else {
        [mv.yaw, mv.pitch]
    }
}

/// `updateMove`'s head pitch for that Player: `head + pitch` held to its datablock's look angles.
pub fn mount_pitch(pitch: f32, by: f32, db: &PlayerData) -> f32 {
    let v = add(by, pitch);
    let t = if v > db.max_look_angle {
        db.max_look_angle
    } else {
        v
    };
    if t > db.min_look_angle {
        t
    } else {
        db.min_look_angle
    }
}

/// The yaw and pitch of the Player the rider controls (a mount, a turret) as the frame draws them, from the `base` the moves sent
/// so far left it: the next move's turn, less `back` of the turn without the mouse's (see [`frame`]).
pub fn mount_frame<W: PredictWorld>(
    game: &dyn PredictGame<W>,
    base: [f32; 2],
    db: &PlayerData,
    damaged: bool,
    waiting: &Waiting,
    back: f32,
    camera_pos: f32,
) -> [f32; 2] {
    if damaged {
        return base;
    }
    let b = back.clamp(0.0, 1.0);
    let full = game.control_turn(&waiting.next_move(true), camera_pos);
    let rest = game.control_turn(&waiting.next_move(false), camera_pos);
    let pf = mount_pitch(base[1], full[1], db);
    let pr = mount_pitch(base[1], rest[1], db);
    [base[0] + full[0] - b * rest[0], pf - b * (pr - base[1])]
}

#[cfg(test)]
#[path = "aim_tests.rs"]
mod tests;
