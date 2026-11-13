//! The global illumination's test scenes, drawn offscreen at 1920 x 1080 (Ultra): two rooms split by a wall half a unit thick
//! with a door, room A lit by a ceiling panel (`two-rooms`), the same with the door shut (`two-rooms-shut`: whatever room B
//! takes is a leak), the `area-lights` example's room (`area-room`), and a courtyard under the afternoon sun (`courtyard`).
//! Room B holds a wall of 20 blocks the voxels' update is timed on.
//!
//! `cargo run -p torque-render-hd --release --example gi-room -- [--out <dir>, default `.scratch/gi-room`] [--shots] [--gi] [--voxels] [--reference]
//! [--blocks N] [--quality high|ultra]`:
//! - `--shots` writes each scene's picture (`<scene>.png`), and with `--gi` the same with global illumination
//!   (`<scene>-gi.png`);
//! - `--voxels` writes pictures of each scene's voxels by their albedo and their light (`<scene>-voxels-albedo.png`,
//!   `<scene>-voxels-lit.png`), from the scene's camera and, for the courtyard, from above;
//! - `--reference` writes the path-traced reference's table of each sampled region's ambient (`reference-<scene>.txt`: the
//!   region, its mean luminance and its linear color), for the two rooms open and shut;
//! - `--blocks N` times the voxels' update while one block of room B's wall is taken out or put back each frame for N frames:
//!   the `gi voxels` mark's median and worst milliseconds (under the bench lock);
//! - `--compare` draws the two rooms with global illumination until the probes settle (every probe traced every frame) and
//!   compares each region's ambient with the reference's (the door open), and room B's with room A's (the door shut: the
//!   leak);
//! - `--repeat` draws each scene's picture with global illumination twice, each in a new renderer, and counts the pixels that
//!   differ.

mod reference;
mod scenes;

use std::path::{Path, PathBuf};

use torque_render_core::{Frame, Gpu, Offscreen};
use torque_render_hd::HdWorld;
use torque_render_hd::ext::PlainScene;
use torque_render_hd::gi::GiView;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::{HdCamera, HdMode, Quality};

use crate::reference::{Traced, luminance};
use crate::scenes::{Room, TestScene};

/// Frames drawn for each picture (the anti-aliasing's history and the capture's faces settle over them).
const FRAMES: usize = 48;
/// Frames of `--blocks` that change blocks before the timed ones.
const WARM: usize = 30;
/// Sample points a side of each region.
const POINTS: usize = 5;

struct Args {
    out: PathBuf,
    shots: bool,
    gi: bool,
    voxels: bool,
    reference: bool,
    blocks: Option<usize>,
    compare: bool,
    repeat: bool,
    quality: Quality,
}

fn args() -> Result<Args, String> {
    let mut a = Args {
        out: PathBuf::from(".scratch/gi-room"),
        shots: false,
        gi: false,
        voxels: false,
        reference: false,
        blocks: None,
        compare: false,
        repeat: false,
        quality: Quality::Ultra,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--out" => a.out = PathBuf::from(value()?),
            "--shots" => a.shots = true,
            "--gi" => a.gi = true,
            "--voxels" => a.voxels = true,
            "--reference" => a.reference = true,
            "--compare" => a.compare = true,
            "--repeat" => a.repeat = true,
            "--blocks" => a.blocks = Some(value()?.parse().map_err(|e| format!("{e}"))?),
            "--quality" => {
                let v = value()?;
                a.quality = Quality::named(&v).ok_or_else(|| format!("no quality {v}"))?;
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if !a.shots && !a.reference && !a.voxels && a.blocks.is_none() && !a.compare && !a.repeat {
        a.shots = true;
    }
    Ok(a)
}

/// Draws `frames` frames of `t` into `target`, the last one read back.
fn draw(
    gpu: &Gpu,
    world: &mut HdWorld<Room>,
    t: &mut TestScene,
    target: &Offscreen,
    frames: usize,
) -> Result<Frame, String> {
    for _ in 0..frames {
        t.scene.frame.generation += 1;
        t.scene.time += 1.0 / 30.0;
        world.upload(&t.scene);
        world.draw(
            &t.scene,
            &t.camera,
            Some((&target.color, target.color.texture().format(), [0, 0])),
        )?;
    }
    gpu.read_back(target).map_err(|e| e.to_string())
}

fn write_png(path: &Path, frame: &Frame) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), frame.width, frame.height);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header()
        .and_then(|mut w| w.write_image_data(&frame.rgb()))
        .map_err(|e| e.to_string())
}

/// The scenes the pictures are of.
fn all_scenes(quality: Quality) -> Vec<TestScene> {
    let mut out = vec![
        scenes::two_rooms(true),
        scenes::two_rooms(false),
        scenes::area_room(),
        scenes::courtyard(),
    ];
    for t in &mut out {
        t.scene.mode = Some(HdMode::new(quality));
    }
    out
}

/// Each scene's picture, and with `gi` the same with global illumination.
fn shots(gpu: &Gpu, out: &Path, quality: Quality, gi: bool) -> Result<(), String> {
    let target = gpu
        .offscreen(scenes::SIZE.0, scenes::SIZE.1)
        .map_err(|e| e.to_string())?;
    let mut world: HdWorld<Room> = HdWorld::new(gpu);
    for mut t in all_scenes(quality) {
        let runs: &[bool] = if gi { &[false, true] } else { &[false] };
        for &on in runs {
            world.gi_mut().enabled = on;
            world.gi_mut().reset();
            world.reset_history();
            let frame = draw(gpu, &mut world, &mut t, &target, FRAMES)
                .map_err(|e| format!("{}: {e}", t.name))?;
            let suffix = if on { "-gi" } else { "" };
            let path = out.join(format!("{}{suffix}.png", t.name));
            write_png(&path, &frame)?;
            println!("{}", path.display());
        }
    }
    Ok(())
}

/// A linear picture as a PNG (clamped, sRGB-encoded).
fn write_linear(path: &Path, (w, h, px): &(u32, u32, Vec<[f32; 4]>)) -> Result<(), String> {
    let rgb: Vec<u8> = px
        .iter()
        .flat_map(|p| {
            [p[0], p[1], p[2]].map(|c| {
                let c = c.clamp(0.0, 1.0);
                let s = if c <= 0.003_130_8 {
                    12.92 * c
                } else {
                    1.055 * c.powf(1.0 / 2.4) - 0.055
                };
                (s * 255.0).round() as u8
            })
        })
        .collect();
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), *w, *h);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(&rgb))
        .map_err(|e| e.to_string())
}

/// Pictures of each scene's voxels from its camera (and the courtyard's from above), after the frames that make them all.
fn voxels(gpu: &Gpu, out: &Path, quality: Quality) -> Result<(), String> {
    let target = gpu
        .offscreen(scenes::SIZE.0, scenes::SIZE.1)
        .map_err(|e| e.to_string())?;
    let mut world: HdWorld<Room> = HdWorld::new(gpu);
    world.gi_mut().enabled = true;
    world.gi_mut().converge = true;
    for mut t in all_scenes(quality) {
        world.gi_mut().reset();
        world.reset_history();
        draw(gpu, &mut world, &mut t, &target, 32).map_err(|e| format!("{}: {e}", t.name))?;
        let mut views = vec![(t.name.to_string(), t.camera)];
        if t.name == "courtyard" {
            let (w, h) = scenes::SIZE;
            let above = HdCamera::new(
                look_at([16.0, -18.0, 14.0], [0.0, 0.0, 0.5]),
                90.0_f32,
                600.0,
                w,
                h,
            );
            views.push(("courtyard-above".to_string(), above));
        }
        for (name, camera) in views {
            for (view, what) in [(GiView::Albedo, "albedo"), (GiView::Radiance, "lit")] {
                let image = world.gi_debug_image(&camera, view)?.ok_or("no voxels")?;
                let path = out.join(format!("{name}-voxels-{what}.png"));
                write_linear(&path, &image)?;
                println!("{}", path.display());
            }
        }
    }
    Ok(())
}

/// The voxels' update timed while one block of room B's wall is taken out or put back each frame for `frames` frames: the
/// `gi voxels` mark's median and worst.
fn blocks(gpu: &Gpu, quality: Quality, frames: usize) -> Result<(), String> {
    let (w, h) = scenes::SIZE;
    let target = gpu.offscreen(w, h).map_err(|e| e.to_string())?;
    let mut world: HdWorld<Room> = HdWorld::new(gpu);
    world.gi_mut().enabled = true;
    let mut t = scenes::two_rooms(true);
    t.scene.mode = Some(HdMode::new(quality));
    let original = scenes::block_wall();
    let mut wall: Vec<Option<scenes::Block>> = original.iter().copied().map(Some).collect();
    let rooms = scenes::two_rooms_blocks(true);
    // Settled: every tile made.
    for _ in 0..600 {
        draw(gpu, &mut world, &mut t, &target, 1)?;
        if world.gi().pending() == 0 {
            break;
        }
    }
    world.profile_passes = true;
    let mut times = Vec::with_capacity(frames);
    // The first WARM frames change blocks as the rest do but are not timed (the GPU and the passes settle).
    for i in 0..frames + WARM {
        let k = i % wall.len();
        wall[k] = if wall[k].is_some() {
            None
        } else {
            Some(original[k])
        };
        let mut all = rooms.clone();
        all.extend(wall.iter().flatten().copied());
        t.scene.frame.cells = scenes::cells_of(&all, 2 + i as u64);
        draw(gpu, &mut world, &mut t, &target, 1)?;
        if let Some(passes) = world.gpu_passes()
            && let Some(ms) = passes.iter().find(|p| p.0 == "gi voxels")
            && i >= WARM
        {
            times.push(ms.1);
        }
    }
    world.profile_passes = false;
    let median = torque_render_hd::profile::median(&times);
    let worst = times.iter().copied().fold(0.0, f64::max);
    let mut slow: Vec<(usize, f64)> = times.iter().copied().enumerate().collect();
    slow.sort_by(|a, b| b.1.total_cmp(&a.1));
    let slow: Vec<String> = slow
        .iter()
        .take(5)
        .map(|(i, ms)| format!("{i}: {ms:.3}"))
        .collect();
    println!("the slowest frames: {}", slow.join(", "));
    println!(
        "gi voxels with one block changed a frame ({} frames, {} {}x{}): median {median:.4} ms, worst {worst:.4} ms",
        times.len(),
        quality.name(),
        w,
        h
    );
    Ok(())
}

/// Frames drawn for the probes to settle before the comparison (every probe traced every frame).
const SETTLE: usize = 96;

/// The probes' ambient over each region of `t` (its points' mean), after the frames that settle them.
fn probe_regions(gpu: &Gpu, t: &mut TestScene, quality: Quality) -> Result<Vec<[f32; 3]>, String> {
    let (w, h) = scenes::SIZE;
    let target = gpu.offscreen(w / 4, h / 4).map_err(|e| e.to_string())?;
    let mut world: HdWorld<Room> = HdWorld::new(gpu);
    world.gi_mut().enabled = true;
    world.gi_mut().converge = true;
    t.scene.mode = Some(HdMode::new(quality));
    let camera = t.camera;
    // From the door, so both rooms are in the finest cascade and the finest voxels (the probes, not their distance, judged).
    t.camera = HdCamera::new(
        look_at([0.25, 0.0, 1.5], [-3.0, 0.0, 1.0]),
        90.0_f32,
        200.0,
        w / 4,
        h / 4,
    );
    draw(gpu, &mut world, t, &target, SETTLE)?;
    t.camera = camera;
    let mut out = Vec::new();
    for r in &t.regions {
        let points: Vec<([f32; 3], [f32; 3])> = r
            .points(POINTS)
            .into_iter()
            .map(|p| (p, r.normal))
            .collect();
        let got = world.gi_ambient_at(&points)?;
        let sum = got
            .iter()
            .fold([0.0f32; 3], |a, b| std::array::from_fn(|k| a[k] + b[k]));
        out.push(sum.map(|c| c / got.len() as f32));
    }
    Ok(out)
}

/// The probes against the reference on the two rooms' regions (the door open), and the leak into room B (the door shut).
fn compare(gpu: &Gpu, quality: Quality) -> Result<(), String> {
    let mut open = scenes::two_rooms(true);
    let reference = Traced::of(&open.scene).regions(&open.regions, POINTS);
    let probes = probe_regions(gpu, &mut open, quality)?;
    println!(
        "{:<14} {:>10} {:>10} {:>8}",
        "region", "reference", "probes", "off"
    );
    let mut offs = Vec::new();
    for ((r, want), got) in open.regions.iter().zip(&reference).zip(&probes) {
        let (a, b) = (luminance(*want), luminance(*got));
        let off = (b - a) / a.max(1e-9);
        offs.push(off.abs());
        println!("{:<14} {a:>10.5} {b:>10.5} {:>7.1} %", r.name, off * 100.0);
    }
    let mean = offs.iter().sum::<f32>() / offs.len() as f32;
    let worst = offs.iter().copied().fold(0.0f32, f32::max);
    println!(
        "two rooms ({}): the probes' ambient off the reference's by {:.1} % on average, {:.1} % at worst",
        quality.name(),
        mean * 100.0,
        worst * 100.0
    );
    let mut shut = scenes::two_rooms(false);
    let probes = probe_regions(gpu, &mut shut, quality)?;
    let room = |prefix: &str| {
        let v: Vec<f32> = shut
            .regions
            .iter()
            .zip(&probes)
            .filter(|(r, _)| r.name.starts_with(prefix))
            .map(|(_, c)| luminance(*c))
            .collect();
        v.iter().sum::<f32>() / v.len().max(1) as f32
    };
    let (a, b) = (room("A "), room("B "));
    println!(
        "two rooms shut ({}): room B's ambient {b:.6}, room A's {a:.6}: {:.2} % of A's",
        quality.name(),
        100.0 * b / a.max(1e-9)
    );
    Ok(())
}

/// Each scene's picture with global illumination drawn twice, each in a new renderer: the pixels that differ.
fn repeat(gpu: &Gpu, quality: Quality) -> Result<(), String> {
    let target = gpu
        .offscreen(scenes::SIZE.0, scenes::SIZE.1)
        .map_err(|e| e.to_string())?;
    for t in all_scenes(quality) {
        let mut frames = Vec::new();
        for _ in 0..2 {
            let mut t = TestScene {
                name: t.name,
                scene: PlainScene {
                    frame: t.scene.frame.clone(),
                    mode: t.scene.mode,
                    time: 0.0,
                },
                camera: t.camera,
                regions: Vec::new(),
            };
            let mut world: HdWorld<Room> = HdWorld::new(gpu);
            world.gi_mut().enabled = true;
            frames.push(draw(gpu, &mut world, &mut t, &target, FRAMES)?.rgb());
        }
        let differ = frames[0]
            .as_chunks::<3>()
            .0
            .iter()
            .zip(frames[1].as_chunks::<3>().0)
            .filter(|(a, b)| a != b)
            .count();
        println!(
            "{} ({}): {differ} pixels differ between two runs",
            t.name,
            quality.name()
        );
    }
    Ok(())
}

/// The reference's tables of the two rooms, open and shut.
fn reference(out: &Path) -> Result<(), String> {
    for t in [scenes::two_rooms(true), scenes::two_rooms(false)] {
        let started = std::time::Instant::now();
        let traced = Traced::of(&t.scene);
        let means = traced.regions(&t.regions, POINTS);
        let mut text = format!(
            "# {}: the ambient's radiance (linear) of each region, {} x {} points of {} paths of up to {} surfaces\n",
            t.name,
            POINTS,
            POINTS,
            reference::PATHS,
            reference::BOUNCES
        );
        for (r, m) in t.regions.iter().zip(&means) {
            text.push_str(&format!(
                "{:<14} {:>10.6} {:>10.6} {:>10.6} {:>10.6}\n",
                r.name,
                luminance(*m),
                m[0],
                m[1],
                m[2]
            ));
        }
        let path = out.join(format!("reference-{}.txt", t.name));
        std::fs::write(&path, &text).map_err(|e| e.to_string())?;
        print!("{text}");
        println!(
            "{} ({:.1} s)",
            path.display(),
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let args = args()?;
    std::fs::create_dir_all(&args.out).map_err(|e| e.to_string())?;
    if args.reference {
        reference(&args.out)?;
    }
    if !args.shots && !args.voxels && args.blocks.is_none() && !args.compare && !args.repeat {
        return Ok(());
    }
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    if args.shots {
        shots(&gpu, &args.out, args.quality, args.gi)?;
    }
    if args.voxels {
        voxels(&gpu, &args.out, args.quality)?;
    }
    if let Some(n) = args.blocks {
        blocks(&gpu, args.quality, n)?;
    }
    if args.compare {
        compare(&gpu, args.quality)?;
    }
    if args.repeat {
        repeat(&gpu, args.quality)?;
    }
    Ok(())
}
