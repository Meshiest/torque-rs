//! A predicting client's tick with replays: the trace world's figure walking (or its buggy driving) over a link that holds the
//! server's datagrams for 20 steps, so 20-odd of the client's moves wait unacknowledged, and the server moving the control
//! object every 8 ticks, so most heads that arrive are corrections and the client replays every move it holds. Only the
//! client's half of each step is timed. Run under the bench lock: `cargo bench -p torque-trace-hash --bench client`.

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use torque_trace_hash::rig::{Control, Rig};
use torque_trace_hash::scenarios::{Baseline, kick};

/// Steps a datagram from the server waits.
const LATENCY: u32 = 20;
/// The server moves the control object every this many ticks.
const KICK_EVERY: u32 = 8;

fn rig(control: Control) -> Rig {
    let mut rig = Rig::new(Baseline, control).expect("the rig");
    assert!(
        (0..400).any(|_| {
            rig.step();
            rig.predicting()
        }),
        "the client never predicted"
    );
    rig.set_latency(LATENCY);
    rig.set_global("$mvForwardAction", 1.0);
    for _ in 0..64 {
        rig.step();
    }
    rig
}

fn timed_client_steps(rig: &mut Rig, control: Control, iters: u64) -> Duration {
    let mut total = Duration::ZERO;
    for _ in 0..iters {
        if rig.ticks.is_multiple_of(KICK_EVERY) {
            // Back and forth, so a long run stays near the start.
            let side = if (rig.ticks / KICK_EVERY).is_multiple_of(2) {
                0.5
            } else {
                -0.5
            };
            kick(rig, control, [side, 0.0, 0.0]).expect("the kick");
        }
        rig.step_server();
        let start = Instant::now();
        rig.step_client();
        total += start.elapsed();
    }
    total
}

fn bench(c: &mut Criterion) {
    for (name, control) in [("figure", Control::Figure), ("buggy", Control::Buggy)] {
        let mut r = rig(control);
        c.bench_function(&format!("client tick with replays, {name}"), |b| {
            b.iter_custom(|iters| timed_client_steps(&mut r, control, iters))
        });
        let stats = r
            .client
            .borrow()
            .conn
            .conn
            .as_ref()
            .map(|c| c.predict.stats);
        eprintln!("{name}: {:?}", black_box(stats));
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
