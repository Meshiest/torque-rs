use super::*;

#[test]
fn mutants_differ_from_the_sample_and_repeat_with_the_seed() {
    let sample: Vec<u8> = (0..64).collect();
    let mut a = Rng::new(7);
    let mut b = Rng::new(7);
    let mut changed = 0;
    for _ in 0..200 {
        let m = mutate(&sample, &mut a);
        assert_eq!(m, mutate(&sample, &mut b));
        changed += usize::from(m != sample);
    }
    assert!(changed > 150);
}

#[test]
fn mutating_nothing_does_not_panic() {
    let mut rng = Rng::new(1);
    for _ in 0..100 {
        let _ = mutate(&[], &mut rng);
    }
}

#[test]
fn panics_are_counted() {
    let report = fuzz_sink("panics", b"abcdef", 100, |bytes| {
        assert!(bytes.len() > 100_000, "always");
    });
    assert_eq!(
        report,
        FuzzReport {
            cases: 100,
            panics: 100
        }
    );
}

#[test]
fn the_cases_come_from_the_environment_or_the_default() {
    assert_eq!(cases_from_env("TORQUE_FUZZ_UPLOAD_NO_SUCH_VARIABLE", 9), 9);
}
