//! The example without a window: two clients over the loopback address, the first hosting, every frame drawn offscreen as the
//! window would draw it, and pictures taken (see [`write_all`]).

use std::io;
use std::path::{Path, PathBuf};

use torque::app::shots::{Seat, Shots, run};
use torque::app::window::offscreen_gpu;
use torque::client::input::{Key, MouseButton};
use torque_render_core::{Frame, Gpu};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::math::Mat;
use torque_render_scene::source::Eye;

use crate::client::{ClientOptions, NetMode, VehicleDemo};
use crate::files::GameFiles;

/// The longest a wait may take (ms of the clock).
pub const WAIT_LIMIT_MS: u32 = 20_000;
/// How far north the buggy is when the passenger's picture is taken, and when the hill's: on its steep near slope.
pub const RIDE_SHOT_Y: f32 = 40.0;
pub const HILL_SHOT_Y: f32 = 80.0;
/// How long after the tracks were laid their fading picture is taken (ms of the clock): well into their fade.
pub const TRACKS_FADED_MS: u32 = 24_000;
/// The age (seconds) under which a track was laid by the S over the hill (the picture from above frames those).
pub const S_TRACK_AGE: f32 = 4.0;

/// An eye at `at` looking at `target`, the view's own field of view.
fn eye(at: [f32; 3], target: [f32; 3]) -> Eye {
    let mut e = look_at(at, target);
    e.fov = None;
    e
}

/// The point `local` of the buggy (in its own space) where the client draws it.
fn on_buggy(m: &Mat, local: [f32; 3]) -> [f32; 3] {
    m.mul_p(local)
}

/// Runs both seats until `done`, at most [`WAIT_LIMIT_MS`].
fn run_until(
    gpu: &Gpu,
    a: &mut Seat<VehicleDemo>,
    b: &mut Seat<VehicleDemo>,
    what: &str,
    done: impl Fn(&VehicleDemo, &VehicleDemo) -> bool,
) -> Result<(), String> {
    let mut waited = 0;
    while !done(&a.game, &b.game) {
        if waited >= WAIT_LIMIT_MS {
            return Err(format!("{what} did not come"));
        }
        run(gpu, &mut [&mut *a, &mut *b], 100)?;
        waited += 100;
    }
    Ok(())
}

/// A picture of `seat` from `eye` (or its own), after the renderer settled.
fn picture(gpu: &Gpu, seat: &mut Seat<VehicleDemo>, at: Option<Eye>) -> Result<Frame, String> {
    seat.eye = at;
    let frame = seat.picture(gpu);
    seat.eye = None;
    frame
}

/// A click of the left button in the middle of the view.
fn click(game: &mut VehicleDemo) {
    game.host.mouse_button(MouseButton::Left, true, (640, 360));
    game.host.mouse_button(MouseButton::Left, false, (640, 360));
}

/// The pictures at `size`, written into `dir`: `menu.png` (host or join), `start.png` (the two figures beside the buggy, the
/// hills beyond), `buggy.png` (the buggy from its front three quarters), `cockpit.png` (the driver's hands on the wheel, close
/// up), `two-seated.png` (the driver and the passenger from outside), `passenger.png` (the passenger's view while the other
/// drives), `driver-eyes.png` and `passenger-eyes.png` (the same moment from each seated figure's eyes) `hill.png` (climbing the round hill's steep side, from the side), `dust.png` (the tires' dust pulling away),
/// `tracks-behind.png` and `tracks-above.png` (the tracks curving over the hill from behind and from above) and
/// `tracks-fading.png` (the same tracks most of their lifetime later). Returns the paths.
pub fn write_all(
    dir: &Path,
    size: (u32, u32),
    make_files: impl Fn() -> io::Result<GameFiles>,
) -> Result<Vec<PathBuf>, String> {
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let seat = |name: &str| -> Result<Seat<VehicleDemo>, String> {
        let files = make_files().map_err(|e| e.to_string())?;
        let game = VehicleDemo::new(
            files,
            ClientOptions {
                net: NetMode::Loopback,
                name: name.to_string(),
                port: 0,
                ..ClientOptions::default()
            },
        )?;
        Seat::new(game, &gpu, size)
    };
    let (mut a, mut b) = (seat("Ann")?, seat("Bob")?);
    let mut shots = Shots::new(dir)?;
    let mut save = |frame: &Frame, name: &str| shots.save(frame, name);

    run(&gpu, &mut [&mut a], 200)?;
    save(&picture(&gpu, &mut a, None)?, "menu")?;
    a.game.host_game()?;
    run_until(&gpu, &mut a, &mut b, "the hosted game", |a, _| {
        a.in_world() && a.figure().is_some()
    })?;
    let address = a.game.hosted_address().ok_or("no hosted address")?;
    b.game.join(&address.to_string())?;
    run_until(&gpu, &mut a, &mut b, "the second player", |a, b| {
        b.in_world() && b.figure().is_some() && a.ghost_count() > 3
    })?;
    // The buggy settles on its springs, the map's name leaves the middle of the view.
    run(&gpu, &mut [&mut a, &mut b], 3000)?;
    let ground = a.game.terrain.height(0.0, 0.0).unwrap_or(0.0);
    let g = |x: f32, y: f32, z: f32| [x, y, ground + z];
    save(
        &picture(
            &gpu,
            &mut a,
            Some(eye(g(-10.0, -8.5, 3.4), g(0.0, 2.0, 1.0))),
        )?,
        "start",
    )?;
    save(
        &picture(&gpu, &mut a, Some(eye(g(4.6, 5.8, 2.1), g(0.0, 0.2, 0.75))))?,
        "buggy",
    )?;

    // In: each looks down at the buggy and clicks; the first takes the driver's seat (on its side), the second the passenger's.
    for game in [&mut a.game, &mut b.game] {
        game.host.mouse_motion(0.0, 110.0);
    }
    run(&gpu, &mut [&mut a, &mut b], 300)?;
    click(&mut a.game);
    run_until(&gpu, &mut a, &mut b, "the driver's seat", |a, _| {
        a.driving()
    })?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    let m = a.game.buggy_pose().ok_or("no buggy")?;
    let (wheel, _) = torque_example_assets::buggy::steering_wheel();
    save(
        &picture(
            &gpu,
            &mut a,
            Some(eye(
                on_buggy(&m, [-0.7, 1.6, 2.15]),
                on_buggy(&m, [wheel[0] + 0.12, wheel[1] - 0.2, wheel[2] - 0.12]),
            )),
        )?,
        "cockpit",
    )?;
    click(&mut b.game);
    run_until(&gpu, &mut a, &mut b, "the passenger's seat", |a, b| {
        b.seated() && a.riders().len() == 2
    })?;
    run(&gpu, &mut [&mut a, &mut b], 800)?;
    let m = a.game.buggy_pose().ok_or("no buggy")?;
    save(
        &picture(
            &gpu,
            &mut a,
            Some(eye(
                on_buggy(&m, [3.3, 4.6, 2.4]),
                on_buggy(&m, [0.0, -0.2, 0.9]),
            )),
        )?,
        "two-seated",
    )?;

    // The drive north: the passenger's view on the way, then the climb up the round hill's steep side.
    a.game.key(Key::W, true);
    // Pulling away: the tires spin up and throw their dust, seen low from behind and to the side.
    run(&gpu, &mut [&mut a, &mut b], 900)?;
    let m = a.game.buggy_pose().ok_or("no buggy")?;
    save(
        &picture(
            &gpu,
            &mut a,
            Some(eye(
                on_buggy(&m, [-4.0, -6.5, 1.3]),
                on_buggy(&m, [0.0, -0.8, 0.5]),
            )),
        )?,
        "dust",
    )?;
    run_until(&gpu, &mut a, &mut b, "the drive", |a, _| {
        a.buggy_pose().is_some_and(|m| m.column(3)[1] > RIDE_SHOT_Y)
    })?;
    save(&picture(&gpu, &mut b, None)?, "passenger")?;
    // The same moment from the driver's eyes and from the passenger's (the passenger looking a little toward the driver).
    a.game.set_first_person(true);
    save(&picture(&gpu, &mut a, None)?, "driver-eyes")?;
    a.game.set_first_person(false);
    b.game.set_first_person(true);
    b.game.set_global("$Drive::OrbitYaw", "-0.5");
    save(&picture(&gpu, &mut b, None)?, "passenger-eyes")?;
    b.game.set_first_person(false);
    b.game.set_global("$Drive::OrbitYaw", "0");
    run_until(&gpu, &mut a, &mut b, "the hill", |a, _| {
        a.buggy_pose().is_some_and(|m| m.column(3)[1] > HILL_SHOT_Y)
    })?;
    let m = a.game.buggy_pose().ok_or("no buggy")?;
    let p = m.column(3);
    let side = eye(
        [p[0] - 8.5, p[1] - 3.0, p[2] + 1.6],
        [p[0], p[1] + 1.5, p[2] + 0.8],
    );
    save(&picture(&gpu, &mut a, Some(side))?, "hill")?;
    // Over the top in an S at an easy pace (off the throttle and braked down first): left, right, left; the tracks curve over
    // the hill behind, seen from the chase camera raised, then from above once the buggy stopped.
    a.game.key(Key::W, false);
    a.game.key(Key::Space, true);
    run(&gpu, &mut [&mut a, &mut b], 2200)?;
    a.game.key(Key::Space, false);
    let mut path = vec![a.game.buggy_pose().ok_or("no buggy")?.column(3)];
    for (key, ms) in [(Key::A, 1500), (Key::D, 2300), (Key::A, 1500)] {
        a.game.key(key, true);
        a.game.set_global("$Drive::Throttle", "0.3");
        run(&gpu, &mut [&mut a, &mut b], ms)?;
        a.game.key(key, false);
        path.push(a.game.buggy_pose().ok_or("no buggy")?.column(3));
        if path.len() == 3 {
            a.game.set_global("$Drive::OrbitPitch", "0.4");
            save(&picture(&gpu, &mut a, None)?, "tracks-behind")?;
            a.game.set_global("$Drive::OrbitPitch", "0");
        }
    }
    a.game.set_global("$Drive::Throttle", "0");
    a.game.key(Key::Space, true);
    run(&gpu, &mut [&mut a, &mut b], 2000)?;
    a.game.key(Key::Space, false);
    // The whole S in view: over the middle of the box the tracks it laid span, as high as the box is long.
    let laid: Vec<[f32; 3]> = a
        .game
        .scene()
        .tracks()
        .map(|t| {
            t.segments()
                .filter(|s| s.age < S_TRACK_AGE)
                .map(|s| s.center)
                .collect()
        })
        .unwrap_or_default();
    path.clear();
    path.extend(laid);
    let lo = [0, 1, 2].map(|k| path.iter().map(|p| p[k]).fold(f32::MAX, f32::min));
    let hi = [0, 1, 2].map(|k| path.iter().map(|p| p[k]).fold(f32::MIN, f32::max));
    let mid = [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
    let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(10.0);
    let above = eye([mid[0], mid[1] - 0.3 * extent, mid[2] + 1.05 * extent], mid);
    save(&picture(&gpu, &mut a, Some(above))?, "tracks-above")?;
    // Most of their lifetime later the tracks fade into the grass.
    run(&gpu, &mut [&mut a, &mut b], TRACKS_FADED_MS)?;
    save(&picture(&gpu, &mut a, Some(above))?, "tracks-fading")?;
    Ok(shots.paths)
}
