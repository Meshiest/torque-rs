use super::*;

#[test]
fn the_plain_table_is_in_use_until_another_is_installed() {
    // Tests of this crate never install a table: `get` answers the plain one.
    assert!(std::ptr::eq(get(), &PLAIN));
    assert!(install(&PLAIN));
    assert!(install(&PLAIN));
}

/// A rotation of a quarter turn about z with a translation.
fn quarter_turn() -> Matrix {
    [
        0.0, -1.0, 0.0, 5.0, //
        1.0, 0.0, 0.0, -2.0, //
        0.0, 0.0, 1.0, 3.0, //
        0.0, 0.0, 0.0, 1.0,
    ]
}

#[test]
fn the_plain_transform_maths_move_boxes_points_and_directions() {
    let m = quarter_turn();
    let (lo, hi) = (PLAIN.mul_box)(&m, [0.0, 0.0, 0.0], [2.0, 1.0, 1.0]);
    assert_eq!((lo, hi), ([4.0, -2.0, 3.0], [5.0, 0.0, 4.0]));
    assert_eq!((PLAIN.rotate)(&m, [1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]);
    let inverse = (PLAIN.affine_inverse)(&m);
    // The inverse takes the moved origin back to the origin, and a moved point back to where it was.
    assert_eq!(
        (PLAIN.to_object_space)(&inverse, [1.0; 3], [5.0, -2.0, 3.0]),
        [0.0; 3]
    );
    assert_eq!(
        (PLAIN.object_point)(&inverse, [1.0; 3], [4.0, -1.0, 3.0]),
        [1.0, 1.0, 0.0]
    );
    // The object's scale divides the point in its space.
    assert_eq!(
        (PLAIN.object_point)(&inverse, [2.0, 4.0, 1.0], [4.0, -1.0, 3.0]),
        [0.5, 0.25, 0.0]
    );
    assert_eq!(
        (PLAIN.box_center)([0.0, -2.0, 4.0], [2.0, 2.0, 8.0]),
        [1.0, 0.0, 6.0]
    );
    assert_eq!(
        (PLAIN.world_sphere)([-1.0, -2.0, -2.0], [1.0, 2.0, 2.0]),
        ([0.0; 3], 3.0)
    );
    assert_eq!((PLAIN.clamp_scale)([0.0, 1e9, -3.0]), [0.0, 1e9, -3.0]);
}

#[test]
fn the_plain_ray_tests_follow_the_segment() {
    let (min, max) = ([0.0; 3], [1.0; 3]);
    assert!((PLAIN.ray_box)(min, max, [-1.0, 0.5, 0.5], [2.0, 0.5, 0.5]));
    assert!((PLAIN.ray_box)(min, max, [2.0, 0.5, 0.5], [-1.0, 0.5, 0.5]));
    assert!(!(PLAIN.ray_box)(
        min,
        max,
        [-1.0, 0.5, 0.5],
        [-0.1, 0.5, 0.5]
    ));
    assert!(!(PLAIN.ray_box)(
        min,
        max,
        [-1.0, 2.0, 0.5],
        [2.0, 2.0, 0.5]
    ));
    assert!((PLAIN.ray_box)(min, max, [0.5, 0.5, 0.5], [0.6, 0.5, 0.5]));
    // Diagonal past the corner: inside each slab, but not at the same time.
    assert!(!(PLAIN.ray_box)(
        min,
        max,
        [-1.0, 3.5, 0.5],
        [3.5, -1.0, 0.5]
    ));
    assert_eq!(
        (PLAIN.ray_point)([0.0; 3], [8.0, 4.0, 0.0], 0.25),
        [2.0, 1.0, 0.0]
    );
    assert_eq!(
        (PLAIN.ray_point)([8.0, 4.0, 0.0], [0.0; 3], 0.25),
        [6.0, 3.0, 0.0]
    );
}

#[test]
fn the_plain_search_distances_are_torques() {
    let (min, max) = ([2.0, -1.0, -2.0], [4.0, 1.0, 2.0]);
    assert_eq!((PLAIN.center_distance)(min, max, [0.0, 4.0, 0.0]), 5.0);
    // Less the smallest side (2), not below zero.
    assert_eq!((PLAIN.radius_distance)(min, max, [0.0, 4.0, 0.0]), 3.0);
    assert_eq!((PLAIN.radius_distance)(min, max, [3.0, 0.0, 0.0]), 0.0);
    // Inside every span: 0; below x by 2 and above y by 3: 4 + 9.
    assert_eq!((PLAIN.box_distance_squared)(min, max, [3.0, 0.0, 0.0]), 0.0);
    assert_eq!(
        (PLAIN.box_distance_squared)(min, max, [0.0, 4.0, 0.0]),
        13.0
    );
    assert!((PLAIN.within_radius)(8.99, 3.0));
    assert!(!(PLAIN.within_radius)(9.0, 3.0));
    assert!(!(PLAIN.within_radius)(f32::NAN, 3.0));
}

#[test]
fn the_plain_sort_is_stable_puts_nans_last_and_stops_paying_when_refused() {
    let mut items = vec![
        (3.0, 0),
        (1.0, 1),
        (f32::NAN, 2),
        (1.0, 3),
        (-0.0, 4),
        (0.0, 5),
    ];
    let mut compares = 0;
    (PLAIN.sort_by_distance)(&mut items, &mut || {
        compares += 1;
        true
    })
    .expect("paid");
    let order: Vec<u32> = items.iter().map(|i| i.1).collect();
    assert_eq!(order, [4, 5, 1, 3, 0, 2]);
    assert!(compares > 0);
    let mut paid = 0;
    let mut items: Vec<(f32, u32)> = (0..100).map(|i| ((100 - i) as f32, i)).collect();
    let answer = (PLAIN.sort_by_distance)(&mut items, &mut || {
        paid += 1;
        paid <= 10
    });
    assert_eq!(answer, Err(Exhausted));
    assert_eq!(paid, 11);
    // The sort still finishes: the pairs are all there, in order.
    assert!(items.windows(2).all(|w| w[0].0 <= w[1].0));
}
