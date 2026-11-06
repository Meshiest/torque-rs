//! The buggy taken hard through a turn of the race track, seen from outside the turn: a picture of the world alone (no HUD or
//! hints) at the renderer's highest quality, for the project's front page ([`turn_picture`]).
//!
//! One player alone: into the buggy, the buggy set down on the track [`RUN_UP`] m before its tightest turn, the day held in the
//! late afternoon with its roof lamps on, then driven at about [`SPEED`] m/s along the track (steered toward the centerline [`LOOK_AHEAD`] m on), the dust
//! thrown up and the tire tracks laid behind it, until it is [`IN_TURN`] m into the turn.

use std::io;

use torque::app::shots::{Seat, run};
use torque::client::input::{Key, MouseButton};
use torque_example_assets::track::track;
use torque_render_core::{Frame, Gpu};
use torque_render_scene::hd::Quality;
use torque_render_scene::hd::kit::look_at;

use crate::client::{ClientOptions, VehicleDemo};
use crate::files::GameFiles;

/// The longest a step may wait (ms of the clock).
pub const WAIT_LIMIT_MS: u32 = 20_000;
/// The day's time held (0 sunrise, 0.25 noon, 0.5 sunset).
pub const TIME: f32 = 0.43;
/// How far before the turn the buggy is set down, the speed it is driven at (m/s), how far ahead it steers toward and how far
/// into the turn it is pictured (m).
pub const RUN_UP: f32 = 140.0;
pub const SPEED: f32 = 17.0;
pub const LOOK_AHEAD: f32 = 20.0;
pub const IN_TURN: f32 = -6.0;
/// The camera: how far ahead of the buggy along the track, out from the centerline on the turn's outside, and up (m).
pub const CAMERA: [f32; 3] = [11.0, 9.0, 2.4];

fn eval(seat: &mut Seat<VehicleDemo>, code: &str) -> Result<(), String> {
    seat.game
        .server
        .as_mut()
        .ok_or("no server")?
        .call("eval", &[code])
        .map(|_| ())
}

/// The unit heading of the track `s` m round.
fn heading(s: f32) -> [f32; 2] {
    let t = track();
    let (a, _) = t.at(s - 2.0);
    let (b, _) = t.at(s + 2.0);
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let n = dx.hypot(dy).max(1e-6);
    [dx / n, dy / n]
}

/// The turn of the track over 40 m centered `s` m round (radians, left positive).
fn turn_at(s: f32) -> f32 {
    let (a, b) = (heading(s - 20.0), heading(s + 20.0));
    (a[0] * b[1] - a[1] * b[0]).atan2(a[0] * b[0] + a[1] * b[1])
}

/// The tightest turn of the loop: how far round it is.
pub fn tightest_turn() -> f32 {
    let t = track();
    let n = (t.length() / 5.0) as usize;
    (0..n)
        .map(|k| k as f32 * 5.0)
        .max_by(|a, b| turn_at(*a).abs().total_cmp(&turn_at(*b).abs()))
        .unwrap_or(0.0)
}

/// The keys at the wheel for the next step: the throttle to [`SPEED`], steered toward the track [`LOOK_AHEAD`] m on.
fn drive(seat: &mut Seat<VehicleDemo>) {
    let (Some(d), Some(m)) = (seat.game.driven(), seat.game.buggy_pose()) else {
        return;
    };
    let p = m.column(3);
    let t = track();
    let Some(near) = t.nearest(p[0], p[1]) else {
        return;
    };
    let (q, _) = t.at(near.along + LOOK_AHEAD);
    let to = [q[0] - p[0], q[1] - p[1]];
    let speed = d.velocity[0].hypot(d.velocity[1]);
    let f = if speed > 2.0 {
        [d.velocity[0] / speed, d.velocity[1] / speed]
    } else {
        [d.forward[0], d.forward[1]]
    };
    let cross = (f[0] * to[1] - f[1] * to[0]) / to[0].hypot(to[1]).max(1e-3);
    seat.game.key(Key::W, speed < SPEED);
    seat.game.key(Key::A, cross > 0.06);
    seat.game.key(Key::D, cross < -0.06);
}

/// Where along the loop the buggy is.
fn along(seat: &Seat<VehicleDemo>) -> Option<f32> {
    let p = seat.game.buggy_pose()?.column(3);
    track().nearest(p[0], p[1]).map(|n| n.along)
}

/// The front page's picture of the buggy in the turn, at `size`.
pub fn turn_picture(
    gpu: &Gpu,
    size: (u32, u32),
    make_files: impl Fn() -> io::Result<GameFiles>,
) -> Result<Frame, String> {
    let files = make_files().map_err(|e| e.to_string())?;
    let game = VehicleDemo::new(files, ClientOptions::default())?;
    let mut a = Seat::new(game, gpu, size)?;
    a.view.set_quality(Quality::Ultra);
    a.game.play_alone()?;
    let mut waited = 0;
    while !(a.game.in_world() && a.game.figure().is_some()) {
        if waited > WAIT_LIMIT_MS {
            return Err("the game did not start".into());
        }
        run(gpu, &mut [&mut a], 100)?;
        waited += 100;
    }
    a.game
        .server
        .as_mut()
        .ok_or("no server")?
        .set_day(TIME, false)?;
    run(gpu, &mut [&mut a], 2000)?;
    // Into the buggy: look down at it and click.
    let middle = ((size.0 / 2) as i32, (size.1 / 2) as i32);
    a.game.host.mouse_motion(0.0, 110.0);
    run(gpu, &mut [&mut a], 300)?;
    a.game.host.mouse_button(MouseButton::Left, true, middle);
    a.game.host.mouse_button(MouseButton::Left, false, middle);
    let mut waited = 0;
    while !a.game.driving() {
        if waited > WAIT_LIMIT_MS {
            return Err("no seat".into());
        }
        run(gpu, &mut [&mut a], 100)?;
        waited += 100;
    }
    let t = track();
    let turn = tightest_turn();
    let start = turn - RUN_UP;
    let (p, hd) = t.at(start);
    let angle = (-hd[0]).atan2(hd[1]);
    let z = a.game.terrain.height(p[0], p[1]).unwrap_or(20.0) + 1.2;
    eval(
        &mut a,
        &format!(
            "Buggy.setTransform(\"{} {} {z} 0 0 1 {angle}\");",
            p[0], p[1]
        ),
    )?;
    run(gpu, &mut [&mut a], 1500)?;
    // The roof lamps on (F at the wheel): their beam on the track ahead and their glow.
    a.game.key(Key::F, true);
    run(gpu, &mut [&mut a], 100)?;
    a.game.key(Key::F, false);
    let mut waited = 0;
    while a.game.headlight_on() != Some(true) {
        if waited > WAIT_LIMIT_MS {
            return Err("the headlight did not come on".into());
        }
        run(gpu, &mut [&mut a], 100)?;
        waited += 100;
    }
    // Driven up to the turn, then pictured in it.
    let goal = (turn + IN_TURN).rem_euclid(t.length());
    let mut waited = 0;
    loop {
        drive(&mut a);
        run(gpu, &mut [&mut a], 16)?;
        waited += 16;
        let Some(s) = along(&a) else { continue };
        let ahead = (goal - s).rem_euclid(t.length());
        if ahead < 2.0 || ahead > t.length() - 2.0 {
            break;
        }
        if waited > 4 * WAIT_LIMIT_MS {
            return Err("the buggy did not reach the turn".into());
        }
    }
    let m = a.game.buggy_pose().ok_or("no buggy")?;
    let body = m.column(3);
    let s = along(&a).unwrap_or(goal);
    let (q, _) = t.at(s + CAMERA[0]);
    let hq = heading(s + CAMERA[0]);
    // The turn's outside: the right of the way for a left turn.
    let out = if turn_at(turn) > 0.0 { 1.0 } else { -1.0 };
    let right = [hq[1], -hq[0]];
    let cx = q[0] + right[0] * out * CAMERA[1];
    let cy = q[1] + right[1] * out * CAMERA[1];
    let cz = a.game.terrain.height(cx, cy).unwrap_or(body[2]) + CAMERA[2];
    let mut eye = look_at([cx, cy, cz], [body[0], body[1], body[2] + 0.3]);
    eye.fov = Some(60);
    a.eye = Some(eye);
    let frame = a.world_picture(gpu);
    a.eye = None;
    for key in [Key::W, Key::A, Key::D] {
        a.game.key(key, false);
    }
    frame
}
