//! The CPU a frame of the enhanced renderer costs on a headless device: the bench scene (a floor, 64 figures, 10,000 particles)
//! uploaded and drawn as a window's control syncs, 1,000 frames, then the mean frame time and the draws' CPU marks.
//!
//! `cargo run -p torque-render-hd --release --example frame-cpu -- [--frames N] [--marks]`. `--marks` also prints the draw's CPU
//! marks and the upload's steps as means a frame (taking them costs a little: the mean frame time is then not the bare one). A
//! timing: run it under the GPU bench lock.

#[path = "../benches/common/mod.rs"]
mod common;

use torque_render_core::Gpu;
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_scene::hd::HdCamera;

fn main() {
    let frames: usize = std::env::args()
        .skip_while(|a| a != "--frames")
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(1_000);
    let marks = std::env::args().any(|a| a == "--marks");
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("no device: {e}");
            return;
        }
    };
    let mut s = common::scene(Some(1), true);
    let eye = s.frame.eye.expect("the scene's eye");
    let (w, h) = common::SIZE;
    let camera = HdCamera::new(eye, f32::from(s.frame.fov), s.frame.visible_distance, w, h);
    let target = gpu.offscreen(w, h).expect("a target");
    let format = target.color.texture().format();
    let mut world: HdWorld<NoExtension> = HdWorld::new(&gpu);
    world.cpu_marks = marks;
    let frame = |world: &mut HdWorld<NoExtension>, s: &mut PlainScene, control: bool| {
        s.frame.generation += 1;
        if control {
            s.frame.control_generation += 1;
        }
        s.frame.particles_generation += 1;
        world.upload(s);
        world
            .draw(s, &camera, Some((&target.color, format, [0, 0])))
            .expect("a frame");
    };
    // A whole sync, then frames that warm the pipelines and the histories.
    frame(&mut world, &mut s, false);
    for _ in 0..60 {
        frame(&mut world, &mut s, true);
    }
    gpu.read_back(&target).expect("read back");
    world.take_cpu_passes();
    world.take_upload_steps();
    let start = web_time::Instant::now();
    for k in 0..frames {
        frame(&mut world, &mut s, true);
        if k % 50 == 49 {
            gpu.read_back(&target).expect("read back");
        }
    }
    gpu.read_back(&target).expect("read back");
    let ms = start.elapsed().as_secs_f64() * 1000.0 / frames as f64;
    println!("frame cpu: {ms:.3} ms a frame over {frames} frames ({w}x{h})");
    if marks {
        for (name, ms) in world.take_cpu_passes() {
            println!("  draw {name:<28} {ms:>8.3}");
        }
        for (name, ms) in world.take_upload_steps() {
            println!("  upload {name:<26} {:>8.3} ms", ms / frames as f64);
        }
    }
}
