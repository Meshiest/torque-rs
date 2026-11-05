use super::*;

const FIREFOX: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:145.0) Gecko/20100101 Firefox/145.0";
const CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";
const SAFARI: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";

#[test]
fn firefox_starts_lower_and_the_others_keep_the_games_quality() {
    assert_eq!(web_quality(FIREFOX, ""), Some(FIREFOX_QUALITY));
    assert_eq!(web_quality(FIREFOX, "?map=office"), Some(FIREFOX_QUALITY));
    assert_eq!(web_quality(CHROME, "?map=office"), None);
    assert_eq!(web_quality(SAFARI, ""), None);
}

#[test]
fn the_address_names_the_quality_in_any_browser() {
    assert_eq!(web_quality(FIREFOX, "?quality=high"), Some(Quality::High));
    assert_eq!(
        web_quality(CHROME, "?map=office&quality=Medium"),
        Some(Quality::Medium)
    );
    assert_eq!(web_quality(CHROME, "?quality=low&fps"), Some(Quality::Low));
    // a name that is no quality: as if none were named
    assert_eq!(web_quality(CHROME, "?quality=max"), None);
    assert_eq!(web_quality(FIREFOX, "?quality="), Some(FIREFOX_QUALITY));
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
