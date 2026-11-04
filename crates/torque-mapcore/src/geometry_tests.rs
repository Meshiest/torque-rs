use super::*;

#[test]
fn the_identity_changes_nothing_and_a_swap_moves_axes() {
    let p = [1.0, -2.0, 3.5];
    assert_eq!(Transform::IDENTITY.point(p), p);
    let t = Transform {
        scale: 2.0,
        axes: [(1, 1.0), (0, -1.0), (2, 1.0)],
    };
    assert_eq!(t.point(p), [-4.0, -2.0, 7.0]);
    assert_eq!(t.direction(p), [-2.0, -1.0, 3.5]);
}
