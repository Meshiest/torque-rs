//! Times the figure scenario's mission load: the server starts the trace world (its script and datablocks), the client connects,
//! and the rig steps until the client predicts its control object. Prints one line: the median of the timed loads, in milliseconds.
//!
//! `cargo run --release -p torque-trace-hash --example time_load -- [loads]` (default 5, after one untimed load).

use std::time::Instant;

use torque_trace_hash::rig::{Control, Rig};
use torque_trace_hash::scenarios::Baseline;

const SETTLE_LIMIT: u32 = 400;

fn load() -> Result<f64, String> {
    let start = Instant::now();
    let mut rig = Rig::new(Baseline, Control::Figure)?;
    if !rig.settle(SETTLE_LIMIT) {
        return Err("the client never predicted its control object".into());
    }
    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    Ok(elapsed)
}

fn main() -> Result<(), String> {
    let loads: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(5);
    load()?;
    let mut times = (0..loads).map(|_| load()).collect::<Result<Vec<_>, _>>()?;
    times.sort_by(f64::total_cmp);
    println!("{:.3}", times[times.len() / 2]);
    Ok(())
}
