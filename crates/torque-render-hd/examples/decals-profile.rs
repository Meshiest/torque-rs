//! The projected decals' cost a frame: the bench scene (a floor, 64 figures) with `--count` decals over the floor in view
//! (bullet holes and scorches multiplied over it, paint splats blended by their alpha, 0.3 to 0.8 units across), drawn through
//! the decal pass. Printed: the decal pass's GPU median (its `decals` mark) against its 0.5 ms mark, the frame's, the boxes
//! drawn, left out by the fill budget and the draws a frame, and the client side's CPU a frame (the boxes made and the scene's
//! take of them). `--huge` lays the largest decals a server can send (64 units across) around the eye instead.
//!
//! `cargo run -p torque-render-hd --release --example decals-profile -- [--count N] [--quality low|medium|high|ultra]
//! [--size WxH] [--frames N] [--huge]`. A timing: run it under the GPU bench lock.

#[path = "../benches/common/mod.rs"]
mod common;
#[path = "decal_kit/mod.rs"]
mod decal_kit;

use decal_kit::{DecalTextures, decal};
use torque_render_core::Gpu;
use torque_render_hd::ext::NoExtension;
use torque_render_hd::profile::{Profile, median};
use torque_render_hd::{DecalProjection, HdWorld};
use torque_render_scene::Blend;
use torque_render_scene::decals::DecalManager;
use torque_render_scene::hd::kit::Lcg;
use torque_render_scene::hd::{HdCamera, HdMode, Quality};
use torque_render_scene::textures::TextureSet;

struct Args {
    count: usize,
    quality: Quality,
    size: (u32, u32),
    frames: usize,
    /// The largest decals a server can send (64 units across) around the eye in place of the floor's small ones.
    huge: bool,
}

fn args() -> Result<Args, String> {
    let mut a = Args {
        count: 4096,
        quality: Quality::High,
        size: (1920, 1080),
        frames: 200,
        huge: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--count" => a.count = value()?.parse().map_err(|e| format!("--count: {e}"))?,
            "--frames" => a.frames = value()?.parse().map_err(|e| format!("--frames: {e}"))?,
            "--quality" => {
                let q = value()?;
                a.quality = Quality::ALL
                    .into_iter()
                    .find(|x| x.name() == q)
                    .ok_or(format!("no quality {q}"))?;
            }
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').ok_or(format!("--size {v}: WxH"))?;
                a.size = (
                    w.parse().map_err(|e| format!("--size: {e}"))?,
                    h.parse().map_err(|e| format!("--size: {e}"))?,
                );
            }
            "--huge" => a.huge = true,
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    Ok(a)
}

/// The decal pass's GPU time a frame it is measured against (4096 decals in view at 1080p High).
const MARK_MS: f64 = 0.5;

fn mark(rows: &[(&'static str, f64)], name: &str) -> f64 {
    rows.iter().find(|(n, _)| *n == name).map_or(0.0, |r| r.1)
}

fn main() -> Result<(), String> {
    let a = args()?;
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let mut s = common::scene(Some(1), false);
    s.mode = Some(HdMode::new(a.quality));
    s.frame.size = a.size;
    let eye = s.frame.eye.expect("the scene's eye");
    let camera = HdCamera::new(
        eye,
        f32::from(s.frame.fov),
        s.frame.visible_distance,
        a.size.0,
        a.size.1,
    );
    let mut world: HdWorld<NoExtension> = HdWorld::new(&gpu);
    world.features.projected_decals = DecalProjection::On;
    if !world.projected_decals_at(a.quality) {
        return Err("this device does not project decals".into());
    }
    // The decals on the floor in front of the eye, a square grid's cells each with one at a random place, size and turn.
    let mut m = DecalManager::new(256, 1.0e9, 1.0);
    m.set_projected(Some(a.count));
    let looks = [
        decal("hole", [1.0, 1.0], Blend::Modulate2x),
        decal("scorch", [1.0, 1.0], Blend::Modulate2x),
        decal("splat", [1.0, 1.0], Blend::Alpha),
    ];
    let mut rng = Lcg::new(7);
    let side = (a.count as f32).sqrt().ceil() as usize;
    for i in 0..a.count {
        let (cx, cy) = ((i % side) as f32, (i / side) as f32);
        let at = [
            -11.0 + 22.0 * (cx + rng.unit()) / side as f32,
            -3.0 + 25.0 * (cy + rng.unit()) / side as f32,
            0.0,
        ];
        let (at, size) = if a.huge {
            let e = eye.position;
            (
                [
                    e[0] + 4.0 * rng.unit() - 2.0,
                    e[1] + 4.0 * rng.unit() - 2.0,
                    0.0,
                ],
                64.0,
            )
        } else {
            (at, 0.3 + 0.5 * rng.unit())
        };
        let mut d = looks[i % looks.len()].clone();
        d.size = [size, size];
        m.add_box(&d, at, [0.0, 0.0, 1.0], rng.unit() * std::f32::consts::TAU);
    }
    let mut cpu = Vec::with_capacity(a.frames);
    let mut frame = |world: &mut HdWorld<NoExtension>, cpu: Option<&mut Vec<f64>>| {
        let t = web_time::Instant::now();
        let mut set = TextureSet::new(&DecalTextures);
        let boxes = m.boxes(&mut set);
        s.frame.set_decals(&boxes, set);
        if let Some(c) = cpu {
            c.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        s.frame.generation += 1;
        s.frame.control_generation += 1;
        world.upload(&s);
        world.draw(&s, &camera, None)
    };
    for _ in 0..30 {
        frame(&mut world, None)?;
    }
    world.finish()?;
    let mut p = Profile::begin(&mut world);
    for _ in 0..a.frames {
        frame(&mut world, Some(&mut cpu))?;
        p.frame(&mut world);
    }
    let c = world.decal_counts();
    let (w, h) = a.size;
    println!(
        "decals-profile: {} decals ({} boxes drawn, {} left out by the fill budget, {} draws) at {} {w}x{h}, {} frames",
        m.len(),
        c.boxes,
        c.dropped,
        c.draws,
        a.quality.name(),
        a.frames
    );
    println!("{}", p.report(0.01));
    let (gpu_rows, frame_ms) = p.gpu_medians();
    let pass = mark(&gpu_rows, "decals");
    println!(
        "decals gpu: {pass:.3} ms median (the frame {frame_ms:.3} ms): {} the {MARK_MS} ms mark; cpu: boxes and the scene's take {:.3} ms median",
        if pass < MARK_MS { "under" } else { "OVER" },
        median(&cpu)
    );
    Ok(())
}
