use super::*;

#[test]
fn the_page_asks_for_a_quality_by_name_and_the_window_takes_it_once() {
    assert_eq!(take_quality(), None);
    request_quality("medium").unwrap();
    request_quality(" Ultra ").unwrap();
    // the last asked for wins
    assert_eq!(take_quality(), Some(Quality::Ultra));
    assert_eq!(take_quality(), None);
    assert!(request_quality("max").is_err());
    assert_eq!(take_quality(), None, "a refused name asks for nothing");
}

#[test]
fn a_query_value_by_name() {
    assert_eq!(
        query_value("?map=office&nolock=1", "map").as_deref(),
        Some("office")
    );
    assert_eq!(
        query_value("?map=office&nolock=1", "nolock").as_deref(),
        Some("1")
    );
    assert_eq!(query_value("?nolock", "nolock"), None);
    assert_eq!(query_value("", "map"), None);
}

#[test]
fn fetched_files_are_taken_only_as_the_list_names_them() {
    let pack = b"the pack's bytes".to_vec();
    let list = Manifest::of([(WEB_PACK, pack.as_slice())]).unwrap();
    assert!(
        add_file(WEB_PACK, pack.clone()).is_err(),
        "no list read yet"
    );
    read_manifest(&list.to_json()).unwrap();
    assert!(
        add_file("other.pack", pack.clone()).is_err(),
        "not in the list"
    );
    assert!(
        add_file(WEB_PACK, b"changed".to_vec()).is_err(),
        "not the list's bytes"
    );
    assert_eq!(
        take_fetched().unwrap_err(),
        format!("{WEB_PACK} was not fetched")
    );
    read_manifest(&list.to_json()).unwrap();
    add_file(WEB_PACK, pack.clone()).unwrap();
    assert_eq!(take_fetched().unwrap(), vec![(WEB_PACK.to_string(), pack)]);
    assert!(take_fetched().is_err(), "the list is forgotten once taken");
}

#[test]
fn frame_times_are_summed_up_every_half_second() {
    let start = Instant::now();
    let mut times = FrameTimes::new();
    assert_eq!(times.frame(start), None);
    let mut summary = None;
    // 16 ms frames, one of 40 ms among them
    let mut t = 0u64;
    for k in 1..=40 {
        t += if k == 10 { 40 } else { 16 };
        if let Some(s) = times.frame(start + Duration::from_millis(t)) {
            summary = Some((k, s));
            break;
        }
    }
    let (k, s) = summary.expect("a summary within 40 frames");
    // 9 frames of 16 ms, one of 40, then 16 ms frames to 500 ms: (500 - 184) / 16 = 19.75, so the 30th frame (504 ms)
    assert_eq!(k, 30);
    assert!((s.max_ms - 40.0).abs() < 1e-6, "{s:?}");
    assert!((s.mean_ms - 504.0 / 30.0).abs() < 1e-6, "{s:?}");
    assert!((s.fps - 30.0 / 0.504).abs() < 1e-6, "{s:?}");
    // the next summary starts from that frame
    let next = (1..=40)
        .find_map(|k| times.frame(start + Duration::from_millis(t + 20 * k)))
        .expect("a second summary");
    assert!((next.max_ms - 20.0).abs() < 1e-6, "{next:?}");
}
