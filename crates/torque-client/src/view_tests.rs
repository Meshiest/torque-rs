use super::*;

#[test]
fn a_scene_is_built_again_when_it_changed_and_is_due() {
    let start = Instant::now();
    let mut pace = RebuildPace::<u32>::default();
    assert!(pace.due(start) && pace.changed(&1, [0.0; 3], (800, 600)));
    pace.built(start, [0.0; 3], 1, (800, 600));
    assert!(!pace.due(start + Duration::from_millis(499)));
    assert!(pace.due(start + REBUILD_EVERY));
    assert!(!pace.changed(&1, [REBUILD_DISTANCE, 0.0, 0.0], (800, 600)));
    assert!(pace.changed(&1, [REBUILD_DISTANCE + 0.1, 0.0, 0.0], (800, 600)));
    assert!(pace.changed(&2, [0.0; 3], (800, 600)));
    assert!(pace.changed(&1, [0.0; 3], (640, 480)));
}

#[test]
fn a_paced_change_builds_at_most_once_a_period_and_at_once_after_a_quiet_spell() {
    let start = Instant::now();
    let every = Duration::from_millis(100);
    let mut pace = Pace::default();
    assert!(pace.allow(start, every), "the first change builds at once");
    let frames = 2000u64;
    let built = (1..frames)
        .filter(|k| pace.allow(start + Duration::from_millis(*k), every))
        .count() as u64;
    let most = frames / every.as_millis() as u64 + 1;
    assert!(built <= most && built >= most - 2, "{built} builds in 2 s");
    assert!(pace.allow(start + Duration::from_millis(frames + 500), every));
}

#[test]
fn lines_of_sight_are_checked_now_and_then_and_ask_for_few_builds() {
    let start = Instant::now();
    let mut watch = SightWatch::default();
    assert!(!watch.due(start), "nothing built yet");
    watch.built(vec![Some(false)]);
    // Every check sees the flare blocked, then clear, then blocked...: at most MAX_SIGHT_REBUILDS builds a REBUILD_EVERY.
    let (mut asked, mut checks) = (0u32, 0u32);
    for k in 0..1000u64 {
        let now = start + Duration::from_millis(10 * (k + 1));
        let mut asked_key = false;
        if watch.changed_at(now, || {
            asked_key = true;
            vec![Some(k % 2 == 0)]
        }) {
            asked += 1;
            watch.built(vec![Some(k % 2 == 0)]);
        }
        checks += u32::from(asked_key);
    }
    // 10 s: a check at most every SIGHT_EVERY, at most 4 builds a half second.
    assert!(checks <= 200, "{checks} checks");
    assert!(asked <= 21 * MAX_SIGHT_REBUILDS, "{asked} builds");
    assert!(asked >= 20, "{asked} builds");
    // A test the world did not make keeps the last answer.
    let mut w = SightWatch::default();
    w.built(vec![Some(true), Some(false)]);
    assert!(w.due(start));
    assert!(!w.differs(&[None, Some(false)]));
    assert!(w.differs(&[Some(false), Some(false)]));
    assert!(w.differs(&[Some(true)]), "a light more or less");
}

#[test]
fn a_warm_up_hands_over_its_result_or_its_error() {
    let warm = Warm::start("warm-test", || Ok(7u32));
    assert_eq!(warm.wait(), Ok(7));
    let failed = Warm::<u32>::start("warm-test", || Err("no device".into()));
    assert_eq!(failed.wait(), Err("no device".into()));
    let polled = Warm::start("warm-test", || Ok("ready"));
    let got = loop {
        if let Some(r) = polled.poll() {
            break r;
        }
        std::thread::yield_now();
    };
    assert_eq!(got, Ok("ready"));
}
