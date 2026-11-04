use super::*;

/// A server's hostile count of reactive bounds (100,000 particle draws and as many of last frame's) is cut to the nearest few
/// in linear time: well under a frame's budget, the nearest kept in order, last frame's that are this frame's not twice.
#[test]
fn a_hostile_count_of_reactive_bounds_is_cut_to_the_nearest_in_linear_time() {
    let n = 100_000usize;
    let bound = |i: usize| [i as f32 * 0.01, (i % 97) as f32, 1.0, -0.5];
    let now: Vec<[f32; 4]> = (0..n).map(bound).collect();
    let before: Vec<[f32; 4]> = (0..n).map(|i| bound(i + n / 2)).collect();
    let eye = [0.0f32; 3];
    let start = std::time::Instant::now();
    let kept = nearest_bounds(now.clone(), eye, REACTIVE_BOUNDS);
    let movers = frame_movers(now, &before, eye, 16);
    let took = start.elapsed();
    assert!(took.as_millis() < 500, "{took:?}");
    assert_eq!(kept.len(), REACTIVE_BOUNDS);
    assert_eq!(movers.len(), 16);
    let near = |b: &[f32; 4]| near_side(b, eye);
    assert!(kept.windows(2).all(|w| near(&w[0]) <= near(&w[1])));
    // The nearest of all are kept.
    let mut all: Vec<f32> = (0..n).map(|i| near(&bound(i))).collect();
    all.sort_by(f32::total_cmp);
    assert_eq!(near(&kept[REACTIVE_BOUNDS - 1]), all[REACTIVE_BOUNDS - 1]);
    // A bound in both lists is there once.
    let mut bits: Vec<[u32; 4]> = movers.iter().map(|b| b.map(f32::to_bits)).collect();
    bits.sort_unstable();
    bits.dedup();
    assert_eq!(bits.len(), movers.len());
    assert!(nearest_bounds(Vec::new(), eye, 4).is_empty());
    assert!(nearest_bounds(vec![[0.0; 4]], eye, 0).is_empty());
}

/// A light switched off, dimmed or on counts as changed (its bound, the radius negative); one that stayed, moved a little or
/// flickered a little does not. A hostile count stays linear.
#[test]
fn a_light_switched_off_dimmed_or_on_counts_as_changed() {
    let eye = [0.0f32; 3];
    let lamp = |x: f32, b: f32| ([x, 0.0, 2.0], [b, b, b], 6.0);
    let before = [
        lamp(0.0, 1.0),
        lamp(10.0, 1.0),
        lamp(20.0, 1.0),
        lamp(30.0, 1.0),
    ];
    // The first gone, the second dimmed to a half, the third moved 0.3 and 10 % dimmer, the fourth as it was, a fifth new.
    let now = [
        lamp(10.0, 0.5),
        lamp(20.3, 0.9),
        lamp(30.0, 1.0),
        lamp(40.0, 1.0),
    ];
    let mut changed = changed_lights(&before, &now, eye, 16);
    changed.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let xs: Vec<f32> = changed.iter().map(|b| b[0]).collect();
    assert_eq!(xs, vec![0.0, 10.0, 10.0, 40.0]);
    assert!(changed.iter().all(|b| b[3] == -6.0));
    assert!(changed_lights(&before, &before, eye, 16).is_empty());
    let many: Vec<([f32; 3], [f32; 3], f32)> =
        (0..20_000).map(|i| lamp(i as f32 * 0.37, 1.0)).collect();
    let start = std::time::Instant::now();
    let gone = changed_lights(&many, &[], eye, REACTIVE_BOUNDS);
    assert!(start.elapsed().as_millis() < 500);
    assert_eq!(gone.len(), REACTIVE_BOUNDS);
}
