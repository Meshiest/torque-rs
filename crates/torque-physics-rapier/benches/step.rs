//! The world's cost: 64 boxes dropped onto a terrain and an interior and stepped 100 ticks (at the default 4 sub-steps), the
//! map cells they need built on the way; and a scratch world of one body built, stepped one tick and dropped, as a per-tick
//! solver would use it.

#[path = "../tests/common/mod.rs"]
mod common;

use std::hint::black_box;
use std::rc::Rc;

use common::{Rng, box_hull, free, interior, terrain};
use criterion::{Criterion, criterion_group};
use torque_collide::ObjectId;
use torque_collide::map::MapShape;
use torque_physics_rapier::{BodyDesc, BodyId, Config, Shape, World};

const TICKS: usize = 100;

fn maps() -> (Rc<MapShape>, Rc<MapShape>) {
    let hills = terrain(
        |i, j| ((i as f32 * 0.3).sin() + (j as f32 * 0.21).cos()) * 3.0,
        8,
        [-1024.0, -1024.0],
        10.0,
        false,
        Vec::new(),
    );
    let mut hulls = vec![box_hull([-16.0, -16.0, -1.0], [16.0, 16.0, 0.0])];
    for k in 0..12 {
        let x = -14.0 + k as f32 * 2.5;
        hulls.push(box_hull([x, -2.0, 0.0], [x + 1.0, 2.0, 2.5]));
    }
    (hills, interior(hulls, [0.0, 0.0, 14.0]))
}

fn world(maps: &(Rc<MapShape>, Rc<MapShape>), bodies: u32) -> World {
    let mut w = World::new(Config::default());
    w.add_map(ObjectId(1), Rc::clone(&maps.0)).unwrap();
    w.add_map(ObjectId(2), Rc::clone(&maps.1)).unwrap();
    let mut rng = Rng(42);
    for k in 0..bodies {
        let d = BodyDesc {
            shape: Shape::Box {
                half: [
                    rng.range(0.3, 0.8),
                    rng.range(0.3, 0.8),
                    rng.range(0.3, 0.8),
                ],
            },
            position: [
                rng.range(-30.0, 30.0),
                rng.range(-30.0, 30.0),
                rng.range(20.0, 30.0),
            ],
            rotation: [
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0),
                1.0,
            ],
            ..BodyDesc::default()
        };
        w.add_body(BodyId(k), &d, &mut free).unwrap();
    }
    w
}

fn drop(c: &mut Criterion) {
    let maps = maps();
    c.bench_function("world/64 boxes x 100 ticks", |b| {
        b.iter(|| {
            let mut w = world(&maps, 64);
            for _ in 0..TICKS {
                w.step(&mut free).unwrap();
            }
            black_box(w.state_words().len())
        })
    });
}

fn scratch(c: &mut Criterion) {
    let maps = maps();
    c.bench_function("world/scratch: 1 body, build + 1 tick", |b| {
        b.iter(|| {
            let mut w = world(&maps, 1);
            w.step(&mut free).unwrap();
            black_box(w.state_words().len())
        })
    });
}

criterion_group!(benches, drop, scratch);

fn main() {
    benches();
    Criterion::default().configure_from_args().final_summary();
}
