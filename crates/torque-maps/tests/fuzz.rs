//! The readers' fuzz: 10,000 cut, bit-flipped and overwritten files from the synthetic seeds and, when the samples are there,
//! from small sample files (`TORQUE_FUZZ_MAPS` and `TORQUE_FUZZ_SEED` override the count and the seed). 0 panics and every case's
//! requests under the caps.

mod common;

use torque_maps::fuzz::{Kind, Seed, run, synthetic_seeds};

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Up to `n` of the smallest sample files of an extension.
fn sample_seeds(kind: Kind, extension: &str, max_bytes: u64, n: usize) -> Vec<Seed> {
    let Some(root) = common::samples() else {
        return Vec::new();
    };
    let mut files = Vec::new();
    common::walk(&root, &mut files);
    let mut hits: Vec<_> = files
        .into_iter()
        .filter(|p| common::ext(p) == extension)
        .filter_map(|p| std::fs::metadata(&p).ok().map(|m| (m.len(), p)))
        .filter(|(len, _)| *len <= max_bytes)
        .collect();
    hits.sort();
    hits.dedup_by_key(|h| h.0);
    let step = (hits.len() / n.max(1)).max(1);
    hits.into_iter()
        .step_by(step)
        .take(n)
        .filter_map(|(_, p)| std::fs::read(p).ok())
        .map(|bytes| Seed { kind, bytes })
        .collect()
}

#[test]
fn hostile_map_files_never_panic_and_the_allocation_caps_hold() {
    let mut seeds = synthetic_seeds();
    let synthetic = seeds.len();
    seeds.extend(sample_seeds(Kind::Dif, "dif", 120_000, 12));
    seeds.extend(sample_seeds(Kind::Ter, "ter", 1 << 20, 2));
    seeds.extend(sample_seeds(Kind::Dml, "dml", 4_096, 4));
    let cases = env("TORQUE_FUZZ_MAPS", 10_000) as usize;
    let summary = run(&seeds, cases, env("TORQUE_FUZZ_SEED", 0x6d61_7073));
    println!(
        "{} seeds ({synthetic} synthetic); {}",
        seeds.len(),
        summary.line()
    );
    if let Some(f) = &summary.first_failure {
        println!("first failure: {f}");
    }
    assert!(summary.clean(), "{}", summary.line());
    // A reader whose work is tied to its input takes milliseconds on a file this small; a case near this is a quadratic path.
    let bound = std::time::Duration::from_secs(if cfg!(debug_assertions) { 10 } else { 2 });
    assert!(summary.longest < bound, "{}", summary.line());
    assert_eq!(summary.cases, cases.max(seeds.len()));
    // Not vacuous: most mutations are refused, some still read, and some of those fail the check.
    assert!(summary.refused > cases / 4, "{}", summary.line());
    assert!(summary.read > cases / 50, "{}", summary.line());
    assert!(summary.checked_ok > 0, "{}", summary.line());
}
