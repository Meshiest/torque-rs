//! The enhanced renderer's per-frame upload of a window's control syncs (the CPU side of `HdWorld::upload`: the moving shapes'
//! vertices and indices encoded and written), on a headless device. The particles go up with the draw (the `frame-cpu` example).

mod common;

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use torque_render_core::Gpu;
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};

/// One control sync's upload of `s`, its queued writes submitted.
fn sync(gpu: &Gpu, world: &mut HdWorld<NoExtension>, s: &mut PlainScene) {
    s.frame.generation += 1;
    s.frame.control_generation += 1;
    world.upload(s);
    gpu.queue().submit([]);
}

fn upload(c: &mut Criterion) {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("upload benches skipped: {e}");
            return;
        }
    };
    for (name, stamp) in [
        ("upload_loose 64 figures rebuilt", None),
        ("upload_loose 64 figures kept", Some(1)),
    ] {
        let mut world: HdWorld<NoExtension> = HdWorld::new(&gpu);
        let mut s = common::scene(stamp, false);
        s.frame.generation += 1;
        world.upload(&s);
        c.bench_function(name, |b| {
            b.iter(|| {
                sync(&gpu, &mut world, &mut s);
                black_box(world.oversize_uploads())
            })
        });
    }
}

criterion_group!(benches, upload);
criterion_main!(benches);
