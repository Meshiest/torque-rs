//! The fuzz (`torque_shapes::fuzz`): 2,000 `.dts` and 2,000 `.dsq` cases in a debug build. `TORQUE_FUZZ_DTS`, `TORQUE_FUZZ_DSQ`,
//! `TORQUE_FUZZ_QUERIES` and `TORQUE_FUZZ_SEED` override the counts and the seed (a release run of 50,000 of each and 10,000
//! collision queries on hostile meshes is the long form); a failure prints its seed. The named cases run first in every run.

use torque_shapes::fuzz::{Config, run};

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[test]
fn hostile_dts_and_dsq_files_never_panic_and_the_allocation_cap_holds() {
    let config = Config {
        dts: env("TORQUE_FUZZ_DTS", 2_000) as usize,
        dsq: env("TORQUE_FUZZ_DSQ", 2_000) as usize,
        queries: env("TORQUE_FUZZ_QUERIES", 1_000) as usize,
        seed: env("TORQUE_FUZZ_SEED", 0x5eed_0003),
    };
    let summary = run(config);
    println!("{}", summary.line());
    if let Some(first) = &summary.first_failure {
        println!("first failure: {first}");
    }
    assert!(summary.clean(), "{}", summary.line());
    // The run is not vacuous: most cases are refused, some are read, and the unmutated ones round-trip.
    assert!(summary.dts_parsed > config.dts / 20, "{}", summary.line());
    assert!(summary.dts_refused > config.dts / 20, "{}", summary.line());
    assert!(summary.dts_exact > config.dts / 20, "{}", summary.line());
    assert!(summary.dsq_parsed > config.dsq / 20, "{}", summary.line());
    assert!(summary.dsq_refused > config.dsq / 20, "{}", summary.line());
    assert!(
        summary.imports_ok > 0 && summary.imports_refused > 0,
        "{}",
        summary.line()
    );
    // The collision queries ran on the shapes that parsed, with hits and with budgets that ran out, not vacuously.
    assert!(
        summary.collision_built > config.dts / 20 && summary.collision_refused > 0,
        "{}",
        summary.line()
    );
    assert!(
        summary.collision_rays > config.dts && summary.collision_boxes > config.dts,
        "{}",
        summary.line()
    );
    assert!(
        summary.collision_hits > 0 && summary.collision_ended > 0,
        "{}",
        summary.line()
    );
    // The hostile meshes: the wanted number of queries was asked, some hit, some ran out of budget, some builds were refused.
    assert!(
        summary.hostile_queries >= config.queries,
        "{}",
        summary.line()
    );
    assert!(
        summary.hostile_hits > 0 && summary.hostile_ended > 0 && summary.hostile_refused > 0,
        "{}",
        summary.line()
    );
}
