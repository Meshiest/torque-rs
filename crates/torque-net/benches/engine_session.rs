//! Engine mode's sessions: 64 connected pairs over loopback links, each tick every side sending a reliable message, an
//! unreliable one and a ghost-lane section, then delivering and polling. Reported with the allocations a tick makes (a counting
//! global allocator), per pair and per tick.

#[path = "../tests/engine_common/mod.rs"]
mod engine_common;

use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use engine_common::Pair;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use torque_net::engine::{Delivery, LinkFacts, StreamLane};

#[global_allocator]
static GLOBAL: &StatsAlloc<std::alloc::System> = &INSTRUMENTED_SYSTEM;

const PAIRS: usize = 64;

fn pairs() -> Vec<Pair> {
    (0..PAIRS)
        .map(|_| {
            let mut p = Pair::connected(LinkFacts::MEMORY);
            p.client_streams.lanes = vec![StreamLane::Moves];
            p.server_streams.lanes = vec![StreamLane::Ghosts];
            p
        })
        .collect()
}

fn tick(pairs: &mut [Pair]) {
    for p in pairs {
        p.client
            .send(vec![1; 60], Delivery::Reliable)
            .expect("queues");
        p.server()
            .send(vec![2; 90], Delivery::Reliable)
            .expect("queues");
        p.server()
            .send(vec![3; 120], Delivery::Unreliable)
            .expect("queues");
        p.tick();
        p.client_events.clear();
        p.server_events.clear();
        p.client_streams.written.clear();
        p.server_streams.written.clear();
    }
}

fn sessions(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine_session");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(300));
    group.measurement_time(Duration::from_secs(2));
    group.bench_function("tick_64_pairs", |b| {
        let mut pairs = pairs();
        for _ in 0..20 {
            tick(&mut pairs);
        }
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                tick(&mut pairs);
            }
            start.elapsed()
        });
    });
    group.finish();

    // Allocations a tick makes: counted once over a fixed run, printed beside the timing.
    let mut pairs = pairs();
    for _ in 0..20 {
        tick(&mut pairs);
    }
    const TICKS: u64 = 50;
    let region = Region::new(GLOBAL);
    for _ in 0..TICKS {
        tick(&mut pairs);
    }
    let stats = region.change();
    let per = TICKS * PAIRS as u64;
    println!(
        "engine_session/allocations: {} allocations and {} bytes per pair-tick (both sides polled and delivered)",
        stats.allocations as u64 / per,
        stats.bytes_allocated as u64 / per
    );
}

criterion_group!(benches, sessions);
criterion_main!(benches);
