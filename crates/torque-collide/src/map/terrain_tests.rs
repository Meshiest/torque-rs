use super::*;

fn field(f: impl Fn(usize, usize) -> f32) -> Arc<HeightField> {
    let heights: Vec<f32> = (0..SIZE * SIZE).map(|i| f(i % SIZE, i / SIZE)).collect();
    let min = heights.iter().copied().fold(f32::INFINITY, f32::min);
    let max = heights.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    Arc::new(HeightField {
        heights,
        min,
        max,
        split: Split::Alternating,
    })
}

/// A block placed as the legacy importer places one, raised to `z`.
fn place(square: i32, z: f32, repeat: bool, empty: &[u32]) -> HeightfieldPlacement {
    HeightfieldPlacement::new(
        torque_maps::legacy::terrain_origin(square),
        z,
        square,
        repeat,
        torque_maps::ter::empty_squares(empty),
    )
}

fn free(_: u64) -> bool {
    true
}

#[test]
fn a_flat_block_is_hit_at_its_height_and_holes_let_rays_through() {
    let t = TerrainShape::new(field(|_, _| 10.0), &place(8, 2.0, false, &[0]));
    // Sample (0, 0) at -1024; square (0, 0) is a hole.
    let hit = t
        .cast_ray([-100.0, -100.0, 30.0], [-100.0, -100.0, 0.0], &mut free)
        .unwrap()
        .unwrap();
    assert!((hit.0 - 18.0 / 30.0).abs() < 1e-6, "{hit:?}");
    assert!((hit.1[2] - 1.0).abs() < 1e-6);
    assert!(
        t.cast_ray([-1020.0, -1020.0, 30.0], [-1020.0, -1020.0, 0.0], &mut free)
            .unwrap()
            .is_none()
    );
    // Off the block it does not repeat.
    assert!(
        t.cast_ray([2000.0, 0.0, 30.0], [2000.0, 0.0, 0.0], &mut free)
            .unwrap()
            .is_none()
    );
    assert_eq!(t.height(5.0, 5.0), Some(12.0));
    let r = TerrainShape::new(field(|_, _| 10.0), &place(8, 2.0, true, &[]));
    assert!(
        r.cast_ray([2000.0, 0.0, 30.0], [2000.0, 0.0, 0.0], &mut free)
            .unwrap()
            .is_some()
    );
    // From below: no hit (front faces only).
    assert!(
        t.cast_ray([-100.0, -100.0, 0.0], [-100.0, -100.0, 30.0], &mut free)
            .unwrap()
            .is_none()
    );
}

#[test]
fn the_cut_alternates_as_drawing_has_it() {
    // A bump at sample (1, 1): square (0, 0) is cut from (0, 0) to (1, 1), so its middle is half way up the bump.
    let t = TerrainShape::new(
        field(|x, y| if (x, y) == (1, 1) { 8.0 } else { 0.0 }),
        &place(1, 0.0, false, &[]),
    );
    let at = |x: f32, y: f32| t.height(-128.0 + x, -128.0 + y).unwrap();
    assert!((at(0.5, 0.5) - 4.0).abs() < 1e-5);
    // Square (1, 0) is cut from (2, 0) to (1, 1): its middle is half way too, but a point near (1, 0) is low.
    assert!((at(1.5, 0.5) - 4.0).abs() < 1e-5);
    assert!(at(1.1, 0.1) < at(1.9, 0.9) + 1e-5);
    let mut out = Vec::new();
    let mut capped = false;
    t.pieces(
        &Box3::new([-128.0, -128.0, -1.0], [-127.5, -127.5, 1.0]),
        &mut free,
        &mut out,
        &mut capped,
    )
    .unwrap();
    assert_eq!(out, [(0, 0, 0), (0, 0, 1)]);
    assert!(!capped);
}
