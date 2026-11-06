//! A shot fired in the arena, seen from the side: pictures of the world alone (no HUD, crosshair or chat) at the renderer's
//! highest quality, for the project's front page ([`shot_frames`], [`shot_picture`]).
//!
//! Three clients over the loopback address: Ann hosts and holds the camera (her figure stands a step behind it, looking where
//! it looks, so the shot's flare, which faces her eye, faces the camera too), Bob fires at Carl, who stands facing him a few steps away. The pictures follow Bob's shot from its flash, a frame apart: the
//! muzzle's flare, the bolt in flight with its ribbon, Carl hit.

use std::f32::consts::PI;
use std::io;

use torque::app::shots::{FRAME_MS, run};
use torque_render_core::{Frame, Gpu};
use torque_render_scene::hd::Quality;
use torque_render_scene::hd::kit::look_at;

use crate::client::{ArenaGame, ClientOptions, NetMode};
use crate::files::GameFiles;
use crate::shots::WAIT_LIMIT_MS;

type Seat = torque::app::shots::Seat<ArenaGame>;

/// Where Bob and Carl stand (their feet) and face.
pub const BOB: ([f32; 3], f32) = ([4.0, -16.0, 0.1], 0.0);
pub const CARL: ([f32; 3], f32) = ([4.0, -11.5, 0.1], PI);
/// The camera: its eye, what it looks at, its field of view (degrees).
pub const CAMERA: ([f32; 3], [f32; 3], u8) = ([-1.7, -15.1, 1.7], [4.15, -14.0, 1.15], 60);
/// Bob's aim below level (radians): at Carl's chest.
pub const AIM_PITCH: f32 = 0.12;
/// The frames taken after the flash.
pub const FRAMES: usize = 16;

fn run_until(
    gpu: &Gpu,
    seats: &mut [&mut Seat],
    what: &str,
    done: impl Fn(&[&mut Seat]) -> bool,
) -> Result<(), String> {
    let mut waited = 0;
    while !done(seats) {
        if waited > WAIT_LIMIT_MS {
            return Err(format!("timed out waiting for {what}"));
        }
        run(gpu, seats, FRAME_MS)?;
        waited += FRAME_MS;
    }
    Ok(())
}

fn place(host: &mut ArenaGame, name: &str, (at, yaw): ([f32; 3], f32)) -> Result<(), String> {
    let server = host.server.as_mut().ok_or("no hosted server")?;
    let [x, y, z] = at.map(|v| v.to_string());
    server.call("placeFigure", &[name, &x, &y, &z, &yaw.to_string()])?;
    Ok(())
}

/// Bob's shot at Carl as Ann's camera sees it, `FRAMES` pictures at `size`, a frame apart from the shot's flash.
pub fn shot_frames(
    gpu: &Gpu,
    size: (u32, u32),
    make_files: impl Fn() -> io::Result<GameFiles>,
) -> Result<Vec<Frame>, String> {
    let seat = |name: &str| -> Result<Seat, String> {
        let files = make_files().map_err(|e| e.to_string())?;
        let game = ArenaGame::new(
            files,
            ClientOptions {
                net: NetMode::Loopback,
                name: name.to_string(),
                port: 0,
                ..ClientOptions::default()
            },
        )?;
        Seat::new(game, gpu, size)
    };
    let (mut a, mut b, mut c) = (seat("Ann")?, seat("Bob")?, seat("Carl")?);
    a.view.set_quality(Quality::Ultra);
    a.game.call("hostGame", &[])?;
    run_until(gpu, &mut [&mut a], "the hosted game", |s| {
        s[0].game.in_game()
    })?;
    let port = a.game.hosted_address().ok_or("no hosted address")?.port();
    for joiner in [&mut b, &mut c] {
        joiner.game.set_global("$Arena::Port", &port.to_string());
        joiner.game.call("showJoin", &[])?;
    }
    run_until(
        gpu,
        &mut [&mut a, &mut b, &mut c],
        "the list of games",
        |s| {
            s[1..].iter().all(|j| {
                j.game
                    .global("$Arena::ServerRows")
                    .parse::<u32>()
                    .is_ok_and(|rows| rows > 0)
            })
        },
    )?;
    b.game.call("joinSelected", &[])?;
    c.game.call("joinSelected", &[])?;
    run_until(
        gpu,
        &mut [&mut a, &mut b, &mut c],
        "the other players",
        |s| s[1].game.in_game() && s[2].game.in_game() && s[0].game.scene().others().len() >= 2,
    )?;
    let (from, to, fov) = CAMERA;
    let look = [to[0] - from[0], to[1] - from[1]];
    let n = look[0].hypot(look[1]);
    let ann = [from[0] - look[0] / n, from[1] - look[1] / n, 0.1];
    place(&mut a.game, "Ann", (ann, look[0].atan2(look[1])))?;
    place(&mut a.game, "Bob", BOB)?;
    place(&mut a.game, "Carl", CARL)?;
    run(gpu, &mut [&mut a, &mut b, &mut c], 1500)?;
    let mut eye = look_at(from, to);
    eye.fov = Some(fov);
    a.eye = Some(eye);
    b.game.set_view_pitch(AIM_PITCH);
    run(gpu, &mut [&mut a, &mut b, &mut c], 300)?;
    let flashes = a.game.scene().flashes;
    b.game.call("fire", &["1"])?;
    run_until(
        gpu,
        &mut [&mut a, &mut b, &mut c],
        "the shot's flash",
        |s| s[0].game.scene().flashes > flashes,
    )?;
    let mut out = Vec::with_capacity(FRAMES);
    for _ in 0..FRAMES {
        out.push(a.world_picture(gpu)?);
        run(gpu, &mut [&mut a, &mut b, &mut c], FRAME_MS)?;
    }
    b.game.call("fire", &["0"])?;
    Ok(out)
}

/// The frame of [`shot_frames`] the front page shows.
pub const PICTURE_FRAME: usize = 2;

/// The front page's picture of the arena: Bob's shot in flight toward Carl.
pub fn shot_picture(
    gpu: &Gpu,
    size: (u32, u32),
    make_files: impl Fn() -> io::Result<GameFiles>,
) -> Result<Frame, String> {
    let mut frames = shot_frames(gpu, size, make_files)?;
    Ok(frames.swap_remove(PICTURE_FRAME.min(frames.len() - 1)))
}
