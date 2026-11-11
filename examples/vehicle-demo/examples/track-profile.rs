//! Where a frame's time goes in the vehicle demo, offscreen: the start view and a few places round the race track (a driver's
//! eyes toward its forests, the crest), each held still and drawn at High and at Ultra for `--frames` frames (60 by default)
//! after the renderer settled, then the GPU time between the frame's pass marks as medians over the frames, the CPU steps'
//! means, and the passes and draw calls by kind (`torque_render_hd::profile`).
//!
//! `cargo run -p torque-example-vehicle-demo --release --example track-profile -- [--frames N] [--quality high|ultra]
//! [--place start|drive|crest] [--size WxH]`. The machine's load moves the means; the medians stand.

use torque::app::shots::{Seat, run};
use torque::app::window::offscreen_gpu;
use torque_example_assets::track::track;
use torque_example_vehicle_demo::client::{ClientOptions, VehicleDemo};
use torque_example_vehicle_demo::files::GameFiles;
use torque_render_core::Gpu;
use torque_render_hd::profile::Profile;
use torque_render_scene::hd::Quality;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::source::Eye;

/// The pictures' size unless `--size` says otherwise.
const SIZE: (u32, u32) = (1280, 720);
/// A driver's eyes over the ground (m).
const EYES: f32 = 1.5;
/// The frames drawn before the measured ones (the histories and the pipelines settled).
const SETTLE: usize = 30;
/// The places round the loop (m along the centerline) a driver's eyes are profiled from: beside the forests and before the crest.
const PLACES: [(&str, f32); 2] = [("drive", 760.0), ("crest", 2460.0)];

fn eye(at: [f32; 3], target: [f32; 3]) -> Eye {
    let mut e = look_at(at, target);
    e.fov = None;
    e
}

fn ground(game: &VehicleDemo, x: f32, y: f32) -> f32 {
    game.terrain.height(x, y).unwrap_or(20.0)
}

/// A driver's eyes on the centerline `s` m round the loop, looking 40 m on along it.
fn driver_eye(game: &VehicleDemo, s: f32) -> Eye {
    let t = track();
    let s = s.min(t.length() - 50.0);
    let (p, _) = t.at(s);
    let (q, _) = t.at(s + 40.0);
    eye(
        [p[0], p[1], ground(game, p[0], p[1]) + EYES],
        [q[0], q[1], ground(game, q[0], q[1]) + EYES],
    )
}

/// A game played alone, in the world, its day held mid-morning and settled.
fn started(gpu: &Gpu, files: GameFiles, size: (u32, u32)) -> Result<Seat<VehicleDemo>, String> {
    let game = VehicleDemo::new(files, ClientOptions::default())?;
    let mut a = Seat::new(game, gpu, size)?;
    a.game.play_alone()?;
    let mut waited = 0;
    while !(a.game.in_world() && a.game.figure().is_some()) {
        if waited > 20_000 {
            return Err("the game did not start".into());
        }
        run(gpu, &mut [&mut a], 100)?;
        waited += 100;
    }
    a.game
        .server
        .as_mut()
        .ok_or("no server")?
        .set_day(0.2, false)?;
    run(gpu, &mut [&mut a], 3000)?;
    Ok(a)
}

/// The view from `eye` (the game's own when None) held for `frames` frames at `quality`, profiled.
fn profile(
    gpu: &Gpu,
    seat: &mut Seat<VehicleDemo>,
    eye: Option<Eye>,
    quality: Quality,
    frames: usize,
) -> Result<Profile, String> {
    seat.view.set_quality(quality);
    seat.eye = eye;
    seat.view.reset_history();
    for _ in 0..SETTLE {
        seat.frame(gpu)?;
    }
    let mut p = Profile::begin(seat.view.renderer());
    for _ in 0..frames {
        seat.frame(gpu)?;
        p.frame(seat.view.renderer());
    }
    seat.view.renderer().profile_passes = false;
    seat.eye = None;
    Ok(p)
}

fn main() -> Result<(), String> {
    let mut frames = 60;
    let mut quality = None;
    let mut place = None;
    let mut size = SIZE;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--frames" => frames = value()?.parse().map_err(|e| format!("{e}"))?,
            "--place" => place = Some(value()?),
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').ok_or("--size needs WxH")?;
                size = (
                    w.parse().map_err(|e| format!("{e}"))?,
                    h.parse().map_err(|e| format!("{e}"))?,
                );
            }
            "--quality" => {
                let v = value()?;
                quality = Some(Quality::named(&v).ok_or_else(|| format!("no quality {v}"))?);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let qualities: Vec<Quality> = match quality {
        Some(q) => vec![q],
        None => vec![Quality::High, Quality::Ultra],
    };
    let wants = |name: &str| place.as_deref().is_none_or(|p| p == name);
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let files = GameFiles::bundled().map_err(|e| e.to_string())?;
    let mut seat = started(&gpu, files, size)?;
    let mut places: Vec<(&str, Option<Eye>)> = Vec::new();
    if wants("start") {
        places.push(("start", None));
    }
    for (name, s) in PLACES {
        if wants(name) {
            places.push((name, Some(driver_eye(&seat.game, s))));
        }
    }
    for (name, eye) in places {
        for &q in &qualities {
            let p = profile(&gpu, &mut seat, eye, q, frames)?;
            println!("track-{name} {} {}x{}", q.name(), size.0, size.1);
            print!("{}", p.report(0.05));
        }
    }
    Ok(())
}
