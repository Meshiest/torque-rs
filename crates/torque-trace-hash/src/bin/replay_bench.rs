//! The cost of a client's prop replay: 8 predicted crates (a row a Player pushes, each against the next) put back to a
//! server's states and stepped through 46 moves with the Player's push, as a restore does
//! ([`torque_plugin_props::predict`]). Prints the mean and the slowest of the replays in milliseconds.
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
}
