//! Prints one `name mode hash` line per scenario of the engine's own plugins (and `all mode hash` for each mode's fold), then the
//! predictors' scenarios (and `predictors mode hash`), then the physics scenarios (and `physics none hash`), then the plugins' (and `plugins mode hash`), the same lines on
//! every target. The mode is the network protocol the run spoke (`none` for a world stepped alone, which speaks none).
//! `--detail` adds what each run came to. The names to run can be given as arguments.

use std::process::ExitCode;

use torque_trace_hash::hash::TraceHasher;
use torque_trace_hash::rig::Mode;
use torque_trace_hash::scenarios::{self, Outcome, Scenario};

/// One scenario's line: its name, label and hash, and with `detail` what the run came to.
fn print_outcome(scenario: &Scenario, outcome: &Outcome, detail: bool) {
    let (name, label) = (scenario.name, scenario.label());
    if detail {
        println!(
            "{name} {label} {:016x} ({} values, {:.2} units to {:.1?}, {} corrections, {} ticks driving)",
            outcome.hash,
            outcome.values,
            outcome.distance,
            outcome.end,
            outcome.corrections,
            outcome.driven_ticks
        );
    } else {
        println!("{name} {label} {:016x}", outcome.hash);
    }
}

/// Runs `list`, printing each line, and gives the fold of the hashes that ran (None when none did). `failed` is set by a failure.
fn run_list(
    list: &[Scenario],
    names: &[String],
    detail: bool,
    failed: &mut bool,
) -> Option<TraceHasher> {
    let mut fold = TraceHasher::new();
    let mut any = false;
    for scenario in list {
        if !names.is_empty() && !names.iter().any(|n| n == scenario.name) {
            continue;
        }
        match (scenario.run)() {
            Ok(outcome) => {
                fold.u64(outcome.hash);
                any = true;
                print_outcome(scenario, &outcome, detail);
            }
            Err(e) => {
                eprintln!("{} {}: {e}", scenario.name, scenario.label());
                *failed = true;
            }
        }
    }
    any.then_some(fold)
}

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
    let mut failed = false;
    let mut folds = Vec::new();
    for mode in Mode::ALL {
        let list: Vec<Scenario> = scenarios::all()
            .into_iter()
            .filter(|s| s.mode == Some(mode))
            .collect();
        let fold = run_list(&list, &names, detail, &mut failed).unwrap_or_default();
        folds.push((mode.label(), fold));
    }
    for (label, fold) in folds {
        println!("all {label} {:016x}", fold.finish());
    }
    for (group, list) in [
        ("predictors", scenarios::predictors()),
        ("physics", scenarios::physics()),
        ("plugins", scenarios::plugins()),
    ] {
        let label = list.first().map_or("none", Scenario::label);
        if let Some(fold) = run_list(&list, &names, detail, &mut failed) {
            println!("{group} {label} {:016x}", fold.finish());
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
