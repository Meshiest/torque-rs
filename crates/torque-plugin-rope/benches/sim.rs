//! The ropes' cost a frame on a client: 64 ropes of 32 segments between pairs of fixed points, run on by a 60 Hz frame and
//! their tube mesh asked for, as the pass does. "At rest" is the ropes after they have hung still for twenty seconds; "swinging"
//! pushes every rope every frame. Run under the bench lock: `cargo bench -p torque-plugin-rope --bench sim`.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use torque_plugin_rope::{RopeId, RopeParams, RopeWorld};

const ROPES: usize = 64;
const FRAME: f32 = 1.0 / 60.0;

/// 64 ropes, each 12 units long between two points 10 apart, run on for twenty seconds so they hang still.
fn world() -> (RopeWorld, Vec<RopeId>) {
    let mut w = RopeWorld::new();
    let params = RopeParams {
        segments: 32,
        length: 12.0,
        ..RopeParams::default()
    };
    let ids = (0..ROPES)
        .map(|k| {
            let y = k as f32 * 2.0;
            w.create(params, [0.0, y, 10.0], [10.0, y, 10.0])
                .expect("a rope")
        })
        .collect();
    for _ in 0..1200 {
        w.advance(FRAME);
    }
    (w, ids)
}

fn bench(c: &mut Criterion) {
    let (mut w, _) = world();
    c.bench_function("64 ropes x 32 segments, at rest", |b| {
        b.iter(|| {
            w.advance(FRAME);
            black_box(w.mesh())
        })
    });
    let (mut w, ids) = world();
    let mut frame = 0u32;
    c.bench_function("64 ropes x 32 segments, swinging", |b| {
        b.iter(|| {
            frame += 1;
            let push = if frame.is_multiple_of(2) { 0.5 } else { -0.5 };
            for &id in &ids {
                w.impulse(id, [push, 0.0, 0.0]).expect("the rope");
            }
            w.advance(FRAME);
            black_box(w.mesh())
        })
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
