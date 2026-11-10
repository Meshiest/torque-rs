//! The cost of a client's prop replay, two ways:
//!
//! - the physics alone: 8 predicted crates (a row a Player pushes, each against the next) put back to a server's states and
//!   stepped through 46 moves with the Player's push, as a restore does ([`torque_plugin_props::predict`]);
//! - the client's whole tick: a server and a client that predicts props, the client holding a crate with seven tethered to it
//!   (8 predicted) among 900 more in its book, the server's packets 45 ticks late, so every update restores the eight and the
//!   client's tick replays 46 moves through its prediction (the Player's move loop with the props part in it).
//!
//! Prints the mean and the slowest in milliseconds.
//!
//! Natively: `cargo run --release -p torque-trace-hash --bin replay-bench`. On wasm32: build it with
//! `cargo build --release --target wasm32-wasip1 -p torque-trace-hash --bin replay-bench` and run the module with
//! `node --no-warnings web/wasi-run.mjs <target>/wasm32-wasip1/release/replay-bench.wasm`. Under the bench lock.

use std::time::Instant;

use torque_plugin_props::data::PropNumbers;
use torque_plugin_props::quant::Quantized;
use torque_plugin_props::world::{PropLimits, PropWorld, Pusher, Rest, StaticPiece};

/// Props predicted and moves replayed.
const PROPS: u32 = 8;
const MOVES: u32 = 46;
/// Replays timed (after as many to warm up).
const ROUNDS: u32 = 50;

fn main() {
    let params = PropNumbers {
        size: [0.8; 3],
        mass: 15.0,
        friction: 0.6,
        ..PropNumbers::default()
    }
    .held();
    let mut world = PropWorld::new(4, PropLimits::default());
    world.sync_statics(vec![(
        0x8000_0001,
        StaticPiece::Box {
            position: [0.0, 0.0, -1.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            half: [200.0, 200.0, 1.0],
        },
    )]);
    for k in 0..PROPS {
        let at = [0.0, k as f32 * 0.82 + 1.0, 0.4];
        world
            .spawn(k + 1, 9, params, (at, [0.0, 0.0, 0.0, 1.0]), None, 0)
            .expect("a crate");
    }
    let states: Vec<(u32, Quantized)> = world.props().iter().map(|(&id, p)| (id, p.q)).collect();
    let pusher = |m: u32| {
        let y = m as f32 * 0.19;
        Pusher {
            id: 999,
            min: [-0.5, y - 0.5, 0.0],
            max: [0.5, y + 0.5, 2.0],
            velocity: [0.0, 6.0, 0.0],
            mass: 90.0,
        }
    };
    let near = |_: [f32; 3]| 0.0;
    let mut times = Vec::with_capacity(ROUNDS as usize);
    for round in 0..ROUNDS * 2 {
        let start = Instant::now();
        for &(id, q) in &states {
            world.restore(id, q, Rest::Awake);
        }
        for m in 0..MOVES {
            world.tick(&[pusher(m)], &near);
        }
        if round >= ROUNDS {
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    let slowest = times.iter().copied().fold(0.0, f64::max);
    println!("replay of {PROPS} props x {MOVES} moves: mean {mean:.3} ms, slowest {slowest:.3} ms");
    client_ticks();
}

/// The client's whole tick with the props part replaying (module docs).
fn client_ticks() {
    let (mut rig, prediction) = match torque_trace_hash::props_predict::replay_bench_rig(MOVES) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("the client tick bench did not start: {e}");
            return;
        }
    };
    // The packets that left before the delay arrive first: wait for the late ones.
    for _ in 0..MOVES * 2 {
        rig.set_global("$mvForwardAction", 1.0);
        rig.step();
    }
    let start = prediction.stats();
    // The ticks that restored (the eight put back and the moves since the ack replayed with the Player's), timed.
    let mut times = Vec::with_capacity(ROUNDS as usize * 2);
    for tick in 0..ROUNDS * 6 {
        rig.set_global("$mvForwardAction", 1.0);
        rig.set_global("$mvYaw", if (tick / 20) % 2 == 0 { 0.01 } else { -0.01 });
        rig.step_server();
        let before = prediction.stats().restores;
        let t = Instant::now();
        rig.step_client();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if prediction.stats().restores > before {
            times.push(ms);
        }
    }
    let stats = prediction.stats();
    let restores = stats.restores - start.restores;
    let replayed = (stats.replayed - start.replayed) as f64 / restores.max(1) as f64;
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    let slowest = times.iter().copied().fold(0.0, f64::max);
    println!(
        "client tick restoring {} props among {} in the book: mean {mean:.3} ms, slowest {slowest:.3} ms ({restores} restores of {replayed:.0} moves)",
        prediction.members().len(),
        torque_trace_hash::props_predict::BENCH_BOOK + 8,
    );
}
