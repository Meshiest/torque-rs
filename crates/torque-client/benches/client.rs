//! The presentation's vector math: 64 other Players eased between their updates.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use torque_client::predict::ghost_players::{GhostMotion, TICK, Update};

/// Player `i`'s update at tick `t`: running round a circle, an update every third tick.
fn update(i: usize, t: usize) -> Update {
    let a = i as f32 * 0.4 + t as f32 * 0.01;
    let r = 10.0 + i as f32 * 0.1;
    Update {
        position: [r * a.cos(), r * a.sin(), 0.5 * (a * 3.0).sin()],
        velocity: [-r * a.sin(), r * a.cos(), 0.0],
        yaw: a,
        head: (a * 0.5).sin() * 0.5,
    }
}

fn ghosts(c: &mut Criterion) {
    c.bench_function("ghost easing 64 players x 1k ticks", |b| {
        b.iter(|| {
            let mut players: Vec<GhostMotion> =
                (0..64).map(|i| GhostMotion::new(update(i, 0))).collect();
            for t in 1..=1_000 {
                for (i, p) in players.iter_mut().enumerate() {
                    if t % 3 == 0 {
                        p.receive(update(i, t));
                    }
                    p.advance(TICK);
                }
            }
            black_box(players.iter().map(|p| p.position()[0]).sum::<f32>())
        })
    });
}

criterion_group!(benches, ghosts);
criterion_main!(benches);
