//! Prints one `name hash` line per scenario (and `all` for their fold), the same lines on every target. `--detail` adds what each
//! run came to. The names to run can be given as arguments.

use std::process::ExitCode;

use torque_trace_hash::hash::TraceHasher;
use torque_trace_hash::scenarios;

fn main() -> ExitCode {
    let mut detail = false;
    let mut names = Vec::new();
    for arg in std::env::args().skip(1) {
        if arg == "--detail" {
            detail = true;
        } else {
            names.push(arg);
        }
    }
    let mut all = TraceHasher::new();
    let mut failed = false;
    for scenario in scenarios::all() {
        if !names.is_empty() && !names.iter().any(|n| n == scenario.name) {
            continue;
        }
        match (scenario.run)() {
            Ok(outcome) => {
                all.u64(outcome.hash);
                if detail {
                    println!(
                        "{} {:016x} ({} values, {:.2} units to {:.1?}, {} corrections, {} ticks driving)",
                        scenario.name,
                        outcome.hash,
                        outcome.values,
                        outcome.distance,
                        outcome.end,
                        outcome.corrections,
                        outcome.driven_ticks
                    );
                } else {
                    println!("{} {:016x}", scenario.name, outcome.hash);
                }
            }
            Err(e) => {
                eprintln!("{}: {e}", scenario.name);
                failed = true;
            }
        }
    }
    println!("all {:016x}", all.finish());
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
