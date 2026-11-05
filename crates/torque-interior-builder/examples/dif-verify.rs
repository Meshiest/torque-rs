//! The builder's verification at its full counts on the 20 generated brush maps: 100,000 surface points, 1,000,000 segments and
//! 1,000,000 BSP points a map. `cargo run --release -p torque-interior-builder --features verify --example dif-verify [share [seeds]]`;
//! prints one line a map and exits with an error when a check fails.

use torque_interior_builder::Options;
use torque_interior_builder::verify::{Counts, generated, run};

fn main() -> std::process::ExitCode {
    let share: f64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(1.0);
    let counts = Counts::scaled(share);
    let mut seeds: Vec<u64> = std::env::args()
        .skip(2)
        .filter_map(|a| a.parse().ok())
        .collect();
    if seeds.is_empty() {
        seeds = (0..20).collect();
    }
    let mut failed = 0;
    for &seed in &seeds {
        let brushes = generated(seed);
        match run(&brushes, &Options::default(), counts, seed) {
            Ok(r) => {
                println!("map {seed:2} ({} brushes): {}", brushes.len(), r.line());
                failed += usize::from(!r.clean());
                if let Some(f) = &r.first {
                    println!("  first: {f}");
                }
            }
            Err(e) => {
                println!("map {seed:2}: {e}");
                failed += 1;
            }
        }
    }
    println!("{failed} of {} maps failed", seeds.len());
    if failed == 0 {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
