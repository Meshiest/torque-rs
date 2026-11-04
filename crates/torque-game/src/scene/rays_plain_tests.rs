use super::*;

const LO: [f32; 3] = [-1.0, -1.0, 0.0];
const HI: [f32; 3] = [1.0, 1.0, 2.0];

fn hit(t: f32) -> Option<RayHit> {
    Some(RayHit {
        t,
        normal: [0.0, 0.0, 1.0],
    })
}

#[test]
fn torques_player_slab_enters_through_the_box_and_points_back_along_the_segment() {
    let (t, n) = torque_player_slab(LO, HI, [-3.0, 0.0, 1.0], [3.0, 0.0, 1.0]).unwrap();
    assert_eq!(t, 2.0 / 6.0);
    assert_eq!(n, [-1.0, 0.0, 0.0]);
    let (t, n) = torque_player_slab(LO, HI, [0.0, 0.0, 5.0], [0.0, 0.0, -5.0]).unwrap();
    assert_eq!(t, 0.3);
    assert_eq!(n, [0.0, 0.0, 1.0]);
    assert_eq!(
        torque_player_slab(LO, HI, [-3.0, 5.0, 1.0], [3.0, 5.0, 1.0]),
        None
    );
    // A segment that starts inside is hit at 0.
    assert_eq!(
        torque_player_slab(LO, HI, [0.0, 0.0, 1.0], [5.0, 0.0, 1.0]).map(|h| h.0),
        Some(0.0)
    );
}

#[test]
fn torques_rules_keep_the_nearest_los_hit_the_object_box_and_no_item_fallback() {
    let rules = TorqueRays;
    let answer = [hit(0.7), hit(0.2), None, hit(0.5)]
        .into_iter()
        .fold(None, |a, h| rules.los_pick(a, h));
    assert_eq!(answer.map(|h| h.t), Some(0.2));
    assert!(
        [None, hit(1e8)]
            .into_iter()
            .fold(None, |a, h| rules.los_pick(a, h))
            .is_none()
    );
    assert_eq!(rules.player_poly_box(LO, HI), (LO, HI));
    assert_eq!(
        rules.item_without_los(LO, HI, [-3.0, 0.0, 1.0], [3.0, 0.0, 1.0]),
        None
    );
}
