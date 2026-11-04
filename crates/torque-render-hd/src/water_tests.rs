use super::*;
use torque_render_scene::hd::kit::look_at;

const ZONE: WaterZone = WaterZone {
    min: [-100.0, -100.0, -50.0],
    max: [100.0, 100.0, 1.5],
    tint: [0.2, 0.4, 0.6, 0.5],
};

/// A camera level with the surface sees the waterline across the middle: under it the near plane is in the water, over it
/// it is not; a camera over the water has none of its view under it, one under it all.
#[test]
fn the_waterline_splits_a_view_across_the_surface() {
    let s = HdScene::new(());
    let line = |z: f32| {
        let cam = HdCamera::new(look_at([0.0, 0.0, z], [10.0, 0.0, z]), 90.0, 100.0, 64, 48);
        post_terms(Some(ZONE), &cam)
    };
    let under = |w: [f32; 4], x: f32, y: f32| {
        w[0] * x + w[1] * y + w[2] < 0.0 && w[0] * x + w[1] * y + w[3] >= 0.0
    };
    let (tint, w) = line(1.5);
    assert_eq!(tint[3], 0.5);
    assert!(under(w, 0.0, -0.5) && under(w, 0.9, -0.9));
    assert!(!under(w, 0.0, 0.5) && !under(w, -0.9, 0.9));
    let (_, w) = line(3.0);
    assert!(!under(w, 0.0, -1.0));
    let (_, w) = line(0.5);
    assert!(under(w, 0.0, 1.0) && under(w, 0.0, -1.0));
    // The eye in the zone: the frame's absorption and tint on; over it: off.
    let t = frame_terms(&s, Some(ZONE), [0.0, 0.0, 0.5], true, true);
    assert_eq!(t.water[2], 1.0);
    assert!(t.water[3] > 0.0 && t.under[3] == 0.5);
    let t = frame_terms(&s, Some(ZONE), [0.0, 0.0, 1.5], true, true);
    assert_eq!((t.water[2], t.water[3], t.under[3]), (0.0, 0.0, 0.0));
    // The water's mirrored camera has its eye under the surface: never under water.
    let t = frame_terms(&s, Some(ZONE), [0.0, 0.0, 0.5], true, false);
    assert_eq!((t.water[2], t.water[3], t.under[3]), (0.0, 0.0, 0.0));
}
