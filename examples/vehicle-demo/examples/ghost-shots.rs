//! The ghosting check on the demo itself: the camera held beside the race track at golden hour while the buggy drives past
//! it, every frame drawn as the window draws it (with every history) and against the reference of that instant (a second
//! renderer, the scene frozen with nothing moving, every history started again and drawn until it settles; see
//! `torque_render_hd::ghosting`). After the drive the clock stops: the frames that follow show how long a trail stays.
//!
//! `cargo run -p torque-example-vehicle-demo --release --example ghost-shots -- [--audit] [--out <dir>] [--max-settle <frames>]
//! [--max-trail <percent>]` prints the scene's row (the ghosting check's table), the mean GPU time of the drive's frames and its
//! passes; `--out` writes `sheet-<run>.png`: frames of the pass (rows: the frame, the reference, their difference four times
//! as bright, red where the frame is lighter); `--audit` runs again with each pass's history off on its own.

use std::path::PathBuf;

use torque::app::App as _;
use torque::app::shots::{Seat, run, write_png};
use torque::app::view::DEFAULT_FOV;
use torque::app::window::offscreen_gpu;
use torque::client::input::{Key, MouseButton};
use torque_example_assets::track::track;
use torque_example_vehicle_demo::client::{ClientOptions, VehicleDemo};
use torque_example_vehicle_demo::files::GameFiles;
use torque_render_core::{Frame, Gpu};
use torque_render_hd::HdWorld;
use torque_render_hd::ext::NoExtension;
use torque_render_hd::ghosting::{self, AREA_OK, Image, Measure, TOLERANCE};
use torque_render_scene::hd::HdCamera;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::source::Eye;

const SIZE: (u32, u32) = (960, 540);
/// The frames held before the drive, of the drive, and held still after it.
const SETTLE: usize = 24;
const DRIVE: usize = 240;
const HOLD: usize = 36;
/// The drive's frames the sheet shows.
const SHOWN: [usize; 4] = [
    SETTLE + 150,
    SETTLE + 190,
    SETTLE + DRIVE - 1,
    SETTLE + DRIVE + 6,
];
/// The golden hour's time of day (the day runs on from it, as the map's does: its sun moves).
const GOLDEN: f32 = 0.44;
/// How far along the track the camera is from the buggy's start (m), how far off the centerline and how high.
const AHEAD: f32 = 40.0;
const OFF: f32 = 9.0;
const HIGH: f32 = 3.0;

fn ground(game: &VehicleDemo, x: f32, y: f32) -> f32 {
    game.terrain.height(x, y).unwrap_or(20.0)
}

fn eval(seat: &mut Seat<VehicleDemo>, code: &str) -> Result<(), String> {
    seat.game
        .server
        .as_mut()
        .ok_or("no server")?
        .call("eval", &[code])
        .map(|_| ())
}

fn set_day(seat: &mut Seat<VehicleDemo>, time: f32, play: bool) -> Result<(), String> {
    seat.game
        .server
        .as_mut()
        .ok_or("no server")?
        .set_day(time, play)
}

/// The keys at the wheel for the next step: the throttle up to about 50 km/h, steered toward the track 26 m ahead.
fn drive(seat: &mut Seat<VehicleDemo>) {
    let (Some(d), Some(m)) = (seat.game.driven(), seat.game.buggy_pose()) else {
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

/// A game played alone, in the buggy on the start straight at golden hour, settled; and the still eye beside the track.
fn started(gpu: &Gpu) -> Result<(Seat<VehicleDemo>, Eye), String> {
    let files = GameFiles::bundled().map_err(|e| e.to_string())?;
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
    set_day(&mut a, 0.2, false)?;
    run(gpu, &mut [&mut a], 3000)?;
    a.game.host.mouse_motion(0.0, 110.0);
    run(gpu, &mut [&mut a], 300)?;
    a.game
        .host
        .mouse_button(MouseButton::Left, true, (480, 270));
    a.game
        .host
        .mouse_button(MouseButton::Left, false, (480, 270));
    let mut waited = 0;
    while !a.game.driving() {
        if waited > 10_000 {
            return Err("no seat".into());
        }
        run(gpu, &mut [&mut a], 100)?;
        waited += 100;
    }
    let t = track();
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
    set_day(&mut a, GOLDEN, true)?;
    run(gpu, &mut [&mut a], 2000)?;
    // Beside the track ahead, on the side away from the sun (the buggy's shadow falls toward the eye).
    let (c, hc) = t.at(s0 + AHEAD);
    let sun = a.game.sun().unwrap_or([0.0, 0.0, 1.0]);
    let mut side = [-hc[1], hc[0]];
    if side[0] * sun[0] + side[1] * sun[1] > 0.0 {
        side = [hc[1], -hc[0]];
    }
    let at = [c[0] + side[0] * OFF, c[1] + side[1] * OFF];
    let target = [c[0] - hc[0] * 2.0, c[1] - hc[1] * 2.0];
    let mut eye = look_at(
        [at[0], at[1], ground(&a.game, at[0], at[1]) + HIGH],
        [
            target[0],
            target[1],
            ground(&a.game, target[0], target[1]) + 0.5,
        ],
    );
    eye.fov = None;
    Ok((a, eye))
}

struct Run {
    name: &'static str,
    measure: Measure,
    gpu_ms: Option<f64>,
    passes: Vec<(&'static str, f64)>,
    sheet: Vec<Image>,
}

fn one(
    gpu: &Gpu,
    name: &'static str,
    set: fn(&mut HdWorld<NoExtension>),
    sheet: bool,
) -> Result<Run, String> {
    let (mut a, eye) = started(gpu)?;
    set(a.view.renderer());
    let mut reference: HdWorld<NoExtension> = HdWorld::new(gpu);
    let ref_target = gpu.offscreen(SIZE.0, SIZE.1).map_err(|e| e.to_string())?;
    let mut areas = Vec::new();
    let mut rows = [Vec::new(), Vec::new(), Vec::new()];
    let (mut ms, mut n) = (0.0, 0usize);
    let mut sums = [0.0f64; 6];
    for f in 0..SETTLE + DRIVE + HOLD {
        if (SETTLE..SETTLE + DRIVE).contains(&f) {
            drive(&mut a);
        }
        // The clock runs through the settling and the drive, then stops: the buggy and its dust stand still.
        if f < SETTLE + DRIVE {
            a.game.advance(16);
        }
        let _ = a.game.frame(&mut a.view);
        a.view
            .draw_world(&a.target.color, [0, 0, SIZE.0, SIZE.1], eye)?;
        if (SETTLE..SETTLE + DRIVE).contains(&f)
            && let Some(t) = a.view.renderer().gpu_ms()
        {
            ms += t;
            n += 1;
            if let Some(g) = a.view.renderer().gpu_times() {
                for (s, v) in sums.iter_mut().zip([
                    g.shadow_maps,
                    g.contact,
                    g.ao,
                    g.aa,
                    g.reflections,
                    g.clouds,
                ]) {
                    *s += v;
                }
            }
        }
        let img = ghosting::image(&gpu.read_back(&a.target).map_err(|e| e.to_string())?);
        // The reference: the same scene with nothing moving.
        let mut still = a.view.scene().clone();
        for o in &mut still.frame.objects {
            o.prev_transform = o.transform;
            o.previous = None;
            for p in &mut o.nodes {
                p.before = p.now;
            }
        }
        still.frame.drawn_objects = None;
        reference.upload(&still);
        let fov = eye.fov.unwrap_or(DEFAULT_FOV);
        let camera = HdCamera::new(eye, fov, still.frame.visible_distance, SIZE.0, SIZE.1);
        let r = ghosting::settled(gpu, &mut reference, &still, &camera, &ref_target)?;
        areas.push(ghosting::share_off(&img, &r));
        if sheet && SHOWN.contains(&f) {
            rows[2].push(ghosting::difference(&img, &r));
            rows[0].push(img);
            rows[1].push(r);
        }
    }
    for key in [Key::W, Key::A, Key::D] {
        a.game.key(key, false);
    }
    let measure = Measure::of(&areas, SETTLE, &[SETTLE + DRIVE]);
    let passes = if n > 0 {
        [
            "shadow maps",
            "contact",
            "ao",
            "aa",
            "reflections",
            "clouds",
        ]
        .into_iter()
        .zip(sums.map(|s| s / n as f64))
        .collect()
    } else {
        Vec::new()
    };
    let sheet = rows.concat();
    Ok(Run {
        name,
        measure,
        gpu_ms: (n > 0).then(|| ms / n as f64),
        passes,
        sheet,
    })
}

/// The sheet: `images` in rows of [`SHOWN`]'s length, at half size.
fn sheet(images: &[Image]) -> Frame {
    let cols = SHOWN.len();
    let (w, h) = (SIZE.0 as usize / 2, SIZE.1 as usize / 2);
    let rows = images.len().div_ceil(cols);
    let mut rgba = vec![0u8; cols * w * rows * h * 4];
    for (k, img) in images.iter().enumerate() {
        let (cx, cy) = (k % cols, k / cols);
        for y in 0..h {
            for x in 0..w {
                let mut c = [0.0f32; 3];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = img[(2 * y + dy) * SIZE.0 as usize + 2 * x + dx];
                    for i in 0..3 {
                        c[i] += p[i] / 4.0;
                    }
                }
                let o = ((cy * h + y) * cols * w + cx * w + x) * 4;
                for i in 0..3 {
                    rgba[o + i] = c[i].round().clamp(0.0, 255.0) as u8;
                }
                rgba[o + 3] = 255;
            }
        }
    }
    Frame {
        width: (cols * w) as u32,
        height: (rows * h) as u32,
        rgba,
    }
}

fn main() -> Result<(), String> {
    let (mut audit, mut out, mut max_settle, mut max_trail) = (false, None, None, None);
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("{arg} wants a value"));
        match arg.as_str() {
            "--audit" => audit = true,
            "--out" => out = Some(PathBuf::from(value()?)),
            "--max-settle" => {
                max_settle = Some(value()?.parse::<usize>().map_err(|e| e.to_string())?)
            }
            "--max-trail" => max_trail = Some(value()?.parse::<f64>().map_err(|e| e.to_string())?),
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    if let Some(dir) = &out {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    println!(
        "a pixel is off past {TOLERANCE}/255 a channel; a frame is within at {:.1} % off or less; settle in frames",
        AREA_OK * 100.0
    );
    println!("{}", Measure::HEADER);
    let mut failed = Vec::new();
    for (name, set) in ghosting::settings::<NoExtension>(audit) {
        let r = one(&gpu, name, set, out.is_some())?;
        println!("{}", r.measure.row("buggy-pass", r.name));
        if let Some(ms) = r.gpu_ms {
            let passes: Vec<String> = r
                .passes
                .iter()
                .map(|(p, v)| format!("{p} {v:.2}"))
                .collect();
            println!("  gpu {ms:.2} ms a frame ({})", passes.join(", "));
        }
        if let Some(dir) = &out {
            let path = dir.join(format!("sheet-{}.png", name.replace(' ', "-")));
            write_png(&sheet(&r.sheet), &path).map_err(|e| e.to_string())?;
            println!("  {}", path.display());
        }
        if name == "default" {
            if max_settle.is_some_and(|m| r.measure.settle > m) {
                failed.push(format!("settles in {} frames", r.measure.settle));
            }
            if max_trail.is_some_and(|m| r.measure.trail * 100.0 > m) {
                failed.push(format!("trail {:.2} %", r.measure.trail * 100.0));
            }
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}
