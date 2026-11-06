use super::*;

#[test]
fn the_rate_is_averaged_and_published_as_tge_does() {
    let mut f = Fps::default();
    let mut published = Vec::new();
    // 100 frames a second for 3 s, the game's clock with them.
    for i in 0..300 {
        let t = i as f32 * 0.01;
        if let Some(p) = f.update(t, t) {
            published.push((t, p));
        }
    }
    // First when the real time is past 0.25 + 0.5 s, then every 0.75 s.
    assert!((published[0].0 - 0.76).abs() < 0.011, "{published:?}");
    assert!(
        (published[1].0 - published[0].0 - 0.75).abs() < 0.021,
        "{published:?}"
    );
    let last = &published.last().unwrap().1;
    assert_eq!(last.0, "100.0");
    assert_eq!(f.text().as_deref(), Some("100 fps  10.0 ms"));
}

#[test]
fn the_window_gives_the_frames_of_each_quarter_second() {
    let mut w = FrameWindow::default();
    let mut rates = Vec::new();
    // 60 frames a second for a second, then 20.
    for i in 0..60 {
        rates.extend(w.frame(f64::from(i) * 1000.0 / 60.0));
    }
    for i in 1..=20 {
        rates.extend(w.frame(1000.0 + f64::from(i) * 50.0));
    }
    assert_eq!(rates.len(), 7, "{rates:?}");
    assert!(
        rates[..3].iter().all(|r| (r - 60.0).abs() < 0.5),
        "{rates:?}"
    );
    assert!((rates.last().unwrap() - 20.0).abs() < 0.5, "{rates:?}");
}
