//! The race track, its landforms and the forests: review pictures, and the frame time at the start and along the lap.
//!
//! `cargo run -p torque-example-vehicle-demo --release --example track-shots -- <dir>` plays alone and writes into `<dir>`:
//! `overview.png` (the whole loop from high above), driver's-eye views along the track past the two stretches with the most
//! forest beside them (`drive-a.png`, `drive-b.png`), over a crest (`crest.png`) and along the ridge (`ridge.png`), one
//! looking up at a forest's edge (`look-up.png`), the trees at a forest's edge close up (`trees-close.png`), each kind of
//! tree side by side (`lineup.png`), and the low sun: the buggy driven up the straight at golden hour, from behind
//! (`golden-drive.png`), a driver's-eye view where a forest's edge stands on the sun's side of the track and its shadows
//! reach across it (`golden-shadows.png`), the same with the sun lower from above the track's far side (`golden-high.png`),
//! and the sun setting behind the tall trees of the first stretch, from over the track (`sunset.png`).
//! `--bench` instead times the frames: the start view held, then an eye flown round the lap a few meters above the track; it
//! prints the mean, the median and the 95th percentile (ms). `--no-trees` leaves the forests out of the map and `--no-layers`
//! draws the ground with its first texture alone (the costs apart).

use std::path::PathBuf;
use std::time::Instant;

use torque::app::App as _;
use torque::app::shots::{Seat, Shots, run};
use torque::app::window::offscreen_gpu;
use torque::client::input::{Key, MouseButton};
use torque_example_assets::track::track;
use torque_example_assets::trees::forest;
use torque_example_vehicle_demo::client::{ClientOptions, VehicleDemo};
use torque_example_vehicle_demo::files::{GameFiles, MISSION};
use torque_render_core::Gpu;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::source::Eye;

const SIZE: (u32, u32) = (1280, 720);
/// A driver's eyes over the ground (m).
const EYES: f32 = 1.5;

fn eye(at: [f32; 3], target: [f32; 3]) -> Eye {
    let mut e = look_at(at, target);
    e.fov = None;
    e
}

fn ground(game: &VehicleDemo, x: f32, y: f32) -> f32 {
    game.terrain.height(x, y).unwrap_or(20.0)
}

/// The eye `k` of `n` round the lap: a few meters behind and above the centerline, looking ahead along it.
fn lap_eye(game: &VehicleDemo, k: usize, n: usize) -> Eye {
    let t = track();
    let s = t.length() * k as f32 / n as f32;
    let (p, _) = t.at(s - 9.0);
    let (q, _) = t.at(s + 14.0);
    eye(
        [p[0], p[1], ground(game, p[0], p[1]) + 3.2],
        [q[0], q[1], ground(game, q[0], q[1]) + 1.0],
    )
}

/// A driver's eyes on the centerline `s` m round the loop, looking `ahead` m on along it.
fn driver_eye(game: &VehicleDemo, s: f32, ahead: f32) -> Eye {
    let t = track();
    let (p, _) = t.at(s);
    let (q, _) = t.at(s + ahead);
    eye(
        [p[0], p[1], ground(game, p[0], p[1]) + EYES],
        [q[0], q[1], ground(game, q[0], q[1]) + EYES],
    )
}

/// The trees within `r` m of the centerline's point `s` m round the loop (`west`: only those west of it).
fn trees_near(s: f32, r: f32, west: bool) -> Vec<[f32; 3]> {
    let (p, _) = track().at(s);
    forest()
        .iter()
        .map(|t| t.foot)
        .filter(|f| (f[0] - p[0]).hypot(f[1] - p[1]) < r && (!west || f[0] < p[0] - 10.0))
        .collect()
}

/// The point round the loop (every 10 m) with the most trees within 45 m (`west`: west of it), at least 400 m from each of
/// `apart`.
fn most_forest(west: bool, apart: &[f32]) -> f32 {
    let t = track();
    let far = |s: f32| {
        apart.iter().all(|&a| {
            let d = (s - a).rem_euclid(t.length());
            d.min(t.length() - d) > 400.0
        })
    };
    (0..(t.length() / 10.0) as usize)
        .map(|k| k as f32 * 10.0)
        .filter(|&s| far(s))
        .max_by_key(|&s| trees_near(s, 45.0, west).len())
        .unwrap_or(0.0)
}

/// The point round the loop (every 10 m) where the track runs across the sun's ground direction `sun` (unit), with a forest's
/// edge just on the sun's side of it (trees within 40 m), open ground beyond that edge (few trees 40 to 160 m on) and none
/// on the other side, outside the start straight: where the low sun comes in under the trees and their shadows fall across
/// the track in stripes.
fn shadow_stretch(sun: [f32; 2]) -> f32 {
    let t = track();
    let count = |s: f32, r: f32, toward: bool| {
        let (p, _) = t.at(s);
        forest()
            .iter()
            .filter(|f| {
                let d = [f.foot[0] - p[0], f.foot[1] - p[1]];
                let along = d[0] * sun[0] + d[1] * sun[1];
                d[0].hypot(d[1]) < r && if toward { along > 10.0 } else { along < -10.0 }
            })
            .count() as i64
    };
    (0..(t.length() / 10.0) as usize)
        .map(|k| k as f32 * 10.0)
        .filter(|&s| {
            let h = t.at(s).1;
            s > 200.0 && s < t.length() - 200.0 && (h[0] * sun[0] + h[1] * sun[1]).abs() < 0.45
        })
        .max_by_key(|&s| {
            let edge = count(s, 40.0, true);
            edge.min(12) * 4 - (count(s, 160.0, true) - edge) - 4 * count(s, 40.0, false)
        })
        .unwrap_or(0.0)
}

/// The sun's direction over the ground (unit, toward it) as the client has it, and its height (the sine of its elevation).
fn sun_ground(game: &VehicleDemo) -> ([f32; 2], f32) {
    let s = game.sun().unwrap_or([0.0, 0.0, 1.0]);
    let n = s[0].hypot(s[1]).max(1e-3);
    ([s[0] / n, s[1] / n], s[2])
}

fn stats(name: &str, mut ms: Vec<f64>) {
    ms.sort_by(f64::total_cmp);
    let mean = ms.iter().sum::<f64>() / ms.len() as f64;
    let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    println!(
        "{name}: {} frames, mean {mean:.2} ms, median {:.2} ms, p95 {:.2} ms",
        ms.len(),
        at(0.5),
        at(0.95)
    );
}

/// One frame drawn and finished, timed (ms).
fn timed(gpu: &Gpu, seat: &mut Seat<VehicleDemo>) -> Result<f64, String> {
    let start = Instant::now();
    seat.game.advance(16);
    seat.frame(gpu)?;
    gpu.finish().map_err(|e| e.to_string())?;
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}

fn set_day(seat: &mut Seat<VehicleDemo>, time: f32) -> Result<(), String> {
    seat.game
        .server
        .as_mut()
        .ok_or("no server")?
        .set_day(time, false)
}

fn eval(seat: &mut Seat<VehicleDemo>, code: &str) -> Result<(), String> {
    seat.game
        .server
        .as_mut()
        .ok_or("no server")?
        .call("eval", &[code])
        .map(|_| ())
}

/// The keys at the wheel for the next step: the throttle up to about 50 km/h, steered toward the track 26 m ahead.
fn drive(seat: &mut Seat<VehicleDemo>) {
    let Some(d) = seat.game.driven() else {
        return;
    };
    let Some(m) = seat.game.buggy_pose() else {
        return;
    };
    let p = m.column(3);
    let t = track();
    let Some(near) = t.nearest(p[0], p[1]) else {
        return;
    };
    let (q, _) = t.at(near.along + 26.0);
    let to = [q[0] - p[0], q[1] - p[1]];
    let speed = d.velocity[0].hypot(d.velocity[1]);
    let f = if speed > 2.0 {
        [d.velocity[0] / speed, d.velocity[1] / speed]
    } else {
        [d.forward[0], d.forward[1]]
    };
    let cross = f[0] * to[1] - f[1] * to[0];
    let len = to[0].hypot(to[1]).max(1e-3);
    seat.game.key(Key::W, speed < 14.0);
    seat.game.key(Key::A, cross / len > 0.12);
    seat.game.key(Key::D, cross / len < -0.12);
}

/// A game played alone over `files`, in the world, its day held mid-morning and settled.
fn started(gpu: &Gpu, files: GameFiles) -> Result<Seat<VehicleDemo>, String> {
    let game = VehicleDemo::new(files, ClientOptions::default())?;
    let mut a = Seat::new(game, gpu, SIZE)?;
    a.game.play_alone()?;
    let mut waited = 0;
    while !(a.game.in_world() && a.game.figure().is_some()) {
        if waited > 20_000 {
            return Err("the game did not start".into());
        }
        run(gpu, &mut [&mut a], 100)?;
        waited += 100;
    }
    set_day(&mut a, 0.2)?;
    run(gpu, &mut [&mut a], 3000)?;
    Ok(a)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut dir = None;
    let mut bench = false;
    let (mut no_trees, mut no_layers) = (false, false);
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--bench" => bench = true,
            "--no-trees" => no_trees = true,
            "--no-layers" => no_layers = true,
            _ => dir = Some(PathBuf::from(arg)),
        }
    }
    let gpu = offscreen_gpu()?;
    let mut files = GameFiles::bundled()?;
    if no_trees {
        let map: String = String::from_utf8_lossy(files.file(MISSION).ok_or("no map")?)
            .lines()
            .filter(|l| !l.contains("InteriorInstance(Forest"))
            .map(|l| format!("{l}\n"))
            .collect();
        files.insert(MISSION, map.as_bytes());
    }
    let mut a = started(&gpu, files)?;
    if no_layers {
        a.view
            .set_terrain(&a.game.files.kit, &a.game.terrain, "kit/checker.png", 8.0);
    }

    if bench {
        for _ in 0..60 {
            timed(&gpu, &mut a)?;
        }
        let mut start = Vec::new();
        for _ in 0..400 {
            start.push(timed(&gpu, &mut a)?);
        }
        stats("start view", start);
        let n = 900;
        let mut lap = Vec::new();
        for k in 0..n {
            a.eye = Some(lap_eye(&a.game, k, n));
            lap.push(timed(&gpu, &mut a)?);
        }
        stats("lap", lap);
        return Ok(());
    }

    let dir = dir.ok_or("give a folder for the pictures")?;
    let mut shots = Shots::new(&dir)?;
    let mut picture = |a: &mut Seat<VehicleDemo>, at: Option<Eye>, name: &str| {
        a.eye = at;
        let frame = a.picture(&gpu);
        a.eye = None;
        frame.and_then(|f| shots.save(&f, name))
    };
    let t = track();

    // The whole loop from high above the middle of it: a game of its own, its map's haze pushed out.
    let g0 = ground(&a.game, 0.0, 0.0);
    {
        let mut files = GameFiles::bundled()?;
        let map = String::from_utf8_lossy(files.file(MISSION).ok_or("no map")?).replace(
            "visibleDistance = 1400; fogDistance = 450;",
            "visibleDistance = 4000; fogDistance = 3000;",
        );
        files.insert(MISSION, map.as_bytes());
        let mut high = started(&gpu, files)?;
        picture(
            &mut high,
            Some(eye([-15.0, -560.0, 680.0], [-12.0, -105.0, g0])),
            "overview",
        )?;
    }

    // Along the track past the two stretches with the most forest beside them, a little before each.
    let a_at = most_forest(false, &[]);
    let b_at = most_forest(false, &[a_at]);
    let e = driver_eye(&a.game, a_at - 40.0, 40.0);
    picture(&mut a, Some(e), "drive-a")?;
    let e = driver_eye(&a.game, b_at - 40.0, 40.0);
    picture(&mut a, Some(e), "drive-b")?;
    // Over the crest on the way back (its top at control point 20.55), from 30 m before it.
    let crest = t.knot(20.55);
    let e = driver_eye(&a.game, crest - 30.0, 45.0);
    picture(&mut a, Some(e), "crest")?;
    // Along the ridge in the south.
    let e = driver_eye(&a.game, t.knot(14.5), 50.0);
    picture(&mut a, Some(e), "ridge")?;

    // Up at the forest's edge beside the first stretch: from the track's edge toward the trees' middle, steeply up.
    let (p, h) = t.at(a_at);
    let near = trees_near(a_at, 50.0, false);
    let n = near.len().max(1) as f32;
    let c = [
        near.iter().map(|f| f[0]).sum::<f32>() / n,
        near.iter().map(|f| f[1]).sum::<f32>() / n,
    ];
    let to = {
        let d = [c[0] - p[0], c[1] - p[1]];
        let l = d[0].hypot(d[1]).max(1e-3);
        [d[0] / l, d[1] / l]
    };
    let from = [p[0] + to[0] * 4.0, p[1] + to[1] * 4.0];
    let gz = ground(&a.game, from[0], from[1]) + EYES;
    picture(
        &mut a,
        Some(eye(
            [from[0], from[1], gz],
            [
                from[0] + to[0] * 22.0 + h[0] * 6.0,
                from[1] + to[1] * 22.0 + h[1] * 6.0,
                gz + 14.0,
            ],
        )),
        "look-up",
    )?;
    // The edge's trees close: the one nearest the track on that side, from 14 m off it toward the track.
    let edge = near
        .iter()
        .min_by(|x, y| {
            let d = |f: &&[f32; 3]| (f[0] - p[0]).hypot(f[1] - p[1]);
            d(x).total_cmp(&d(y))
        })
        .copied()
        .unwrap_or([c[0], c[1], gz]);
    let back = [edge[0] - to[0] * 14.0, edge[1] - to[1] * 14.0];
    let bz = ground(&a.game, back[0], back[1]) + 2.0;
    picture(
        &mut a,
        Some(eye(
            [back[0], back[1], bz],
            [edge[0] + h[0] * 2.0, edge[1] + h[1] * 2.0, edge[2] + 7.0],
        )),
        "trees-close",
    )?;

    // The kinds of tree side by side on the flat field, north-west of the buggy.
    eval(
        &mut a,
        &format!(
            "new InteriorInstance(Lineup) {{ position = \"0 14 {g0}\"; rotation = \"1 0 0 0\"; scale = \"1 1 1\"; \
             interiorFile = \"kit/tree_lineup.dif\"; }}; MissionGroup.add(Lineup);"
        ),
    )?;
    run(&gpu, &mut [&mut a], 1500)?;
    picture(
        &mut a,
        Some(eye([2.0, -48.0, g0 + 5.0], [2.0, 14.0, g0 + 17.0])),
        "lineup",
    )?;
    eval(&mut a, "Lineup.delete();")?;

    // The low sun over a forest's edge beside a stretch running across its light: from the driver's seat as the trees'
    // shadows reach over the track, from above the track on the far side looking back toward the trees and the sun, and at
    // sunset from above the track looking into the sun.
    set_day(&mut a, 0.452)?;
    run(&gpu, &mut [&mut a], 1500)?;
    let (sun, height) = sun_ground(&a.game);
    let w_at = shadow_stretch(sun);
    println!(
        "golden hour: the sun {:.1} degrees up, the stretch {w_at:.0} m round",
        height.asin().to_degrees()
    );
    let e = driver_eye(&a.game, w_at - 35.0, 40.0);
    picture(&mut a, Some(e), "golden-shadows")?;
    set_day(&mut a, 0.468)?;
    run(&gpu, &mut [&mut a], 1500)?;
    let (sun, height) = sun_ground(&a.game);
    println!(
        "lower: the sun {:.1} degrees up",
        height.asin().to_degrees()
    );
    // Over the centerline 40 m back (the track is clear of trees), looking on along it and a little toward the sun's side.
    let (q, hq) = t.at(w_at);
    let (from, _) = t.at(w_at - 40.0);
    let fz = ground(&a.game, from[0], from[1]) + 12.0;
    let qz = ground(&a.game, q[0], q[1]);
    picture(
        &mut a,
        Some(eye(
            [from[0], from[1], fz],
            [
                q[0] + sun[0] * 10.0 + hq[0] * 10.0,
                q[1] + sun[1] * 10.0 + hq[1] * 10.0,
                qz + 2.0,
            ],
        )),
        "golden-high",
    )?;
    set_day(&mut a, 0.488)?;
    run(&gpu, &mut [&mut a], 1500)?;
    let (sun, height) = sun_ground(&a.game);
    println!(
        "sunset: the sun {:.1} degrees up",
        height.asin().to_degrees()
    );
    // Over the track beside the most forest, looking into the sun through the tall trees.
    let (m, _) = t.at(a_at);
    let mz = ground(&a.game, m[0], m[1]);
    picture(
        &mut a,
        Some(eye(
            [m[0], m[1], mz + 8.0],
            [m[0] + sun[0] * 60.0, m[1] + sun[1] * 60.0, mz + 14.0],
        )),
        "sunset",
    )?;

    // In the buggy: look down at it and click, then onto the track and away up the straight at golden hour.
    set_day(&mut a, 0.2)?;
    a.game.host.mouse_motion(0.0, 110.0);
    run(&gpu, &mut [&mut a], 300)?;
    a.game
        .host
        .mouse_button(MouseButton::Left, true, (640, 360));
    a.game
        .host
        .mouse_button(MouseButton::Left, false, (640, 360));
    let mut waited = 0;
    while !a.game.driving() {
        if waited > 10_000 {
            return Err("no seat".into());
        }
        run(&gpu, &mut [&mut a], 100)?;
        waited += 100;
    }
    let s0 = t.length() - 45.0;
    let (p, h) = t.at(s0);
    let angle = (-h[0]).atan2(h[1]);
    let z = ground(&a.game, p[0], p[1]) + 1.2;
    eval(
        &mut a,
        &format!(
            "Buggy.setTransform(\"{} {} {z} 0 0 1 {angle}\");",
            p[0], p[1]
        ),
    )?;
    set_day(&mut a, 0.46)?;
    run(&gpu, &mut [&mut a], 1500)?;
    // Up the straight, steered along the track.
    for _ in 0..60 {
        drive(&mut a);
        run(&gpu, &mut [&mut a], 50)?;
    }
    picture(&mut a, None, "golden-drive")?;
    for key in [Key::W, Key::A, Key::D] {
        a.game.key(key, false);
    }
    for path in &shots.paths {
        println!("{}", path.display());
    }
    Ok(())
}
