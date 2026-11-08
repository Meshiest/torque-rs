//! The scenarios repeat themselves in one process, and ran: a hash of a run that never moved or never predicted would prove nothing.

use torque_trace_hash::scenarios::{self, Baseline, TICKS};

#[test]
fn every_scenario_repeats_itself_and_ran_the_whole_script() {
    for scenario in scenarios::all().into_iter().chain(scenarios::predictors()) {
        let first = (scenario.run)().expect(scenario.name);
        let second = (scenario.run)().expect(scenario.name);
        assert_eq!(
            first, second,
            "{} differs from one run to the next",
            scenario.name
        );
        assert!(
            first.values > u64::from(TICKS) * 10,
            "{}: {first:?}",
            scenario.name
        );
        assert!(first.distance > 10.0, "{}: {first:?}", scenario.name);
    }
}

#[test]
fn the_figure_corrects_its_prediction_and_the_buggy_is_driven_by_the_client() {
    let figure = scenarios::figure(Baseline).unwrap();
    assert!(
        figure.corrections > 0,
        "a replay of corrections runs: {figure:?}"
    );
    let buggy = scenarios::buggy(Baseline).unwrap();
    assert_eq!(buggy.driven_ticks, TICKS, "{buggy:?}");
    assert!(
        buggy.end[2].abs() < 1.0,
        "the buggy stays on the floor: {buggy:?}"
    );
}

#[test]
fn the_two_scenarios_differ_and_the_per_plugin_entry_point_folds_both() {
    let figure = scenarios::figure(Baseline).unwrap().hash;
    let buggy = scenarios::buggy(Baseline).unwrap().hash;
    assert_ne!(figure, buggy);
    let folded = scenarios::trace_hash::<Baseline>().unwrap();
    assert_eq!(folded, scenarios::trace_hash::<Baseline>().unwrap());
    assert_ne!(folded, figure);
}
