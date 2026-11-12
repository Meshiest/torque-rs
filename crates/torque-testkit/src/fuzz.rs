//! Fuzz loops: how many rounds to run and a per-case runner that names the case that failed.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::time::{Duration, Instant};

/// The rounds a fuzz test runs: the number in the environment variable `var` (`TORQUE_FUZZ_*`), or `default` when it is unset
/// or not a number.
#[must_use]
pub fn rounds(var: &str, default: u64) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// The slowest case of a [`run`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slowest {
    /// Its number.
    pub case: u64,
    /// How long it took.
    pub took: Duration,
}

/// Runs `f` for the cases `0..cases`. A case that panics is reported on stderr by number (so a failure repeats from its
/// seed) and the panic goes on. Returns the slowest case, to see a hang coming before it is one.
pub fn run(cases: u64, mut f: impl FnMut(u64)) -> Slowest {
    let mut slowest = Slowest {
        case: 0,
        took: Duration::ZERO,
    };
    for case in 0..cases {
        let start = Instant::now();
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| f(case))) {
            eprintln!("fuzz case {case} of {cases} failed");
            resume_unwind(panic);
        }
        let took = start.elapsed();
        if took > slowest.took {
            slowest = Slowest { case, took };
        }
    }
    slowest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_fall_back_to_the_default() {
        assert_eq!(rounds("TORQUE_TESTKIT_SURELY_UNSET", 42), 42);
    }

    #[test]
    fn run_visits_every_case_and_reports_the_slowest() {
        let mut seen = Vec::new();
        let slowest = run(5, |case| {
            seen.push(case);
            if case == 3 {
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        assert_eq!(seen, [0, 1, 2, 3, 4]);
        assert_eq!(slowest.case, 3);
    }

    #[test]
    #[should_panic(expected = "boom")]
    fn a_failing_case_panics_on() {
        run(3, |case| assert!(case < 1, "boom"));
    }
}
