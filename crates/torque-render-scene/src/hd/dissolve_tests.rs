use super::*;

#[test]
fn the_noise_stays_in_its_range_and_varies() {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for i in 0..4000 {
        let p = [
            i as f32 * 0.37 - 700.0,
            (i % 53) as f32 * 0.21,
            (i % 7) as f32 * -1.3,
        ];
        let n = noise(p);
        assert!((0.0..=1.0).contains(&n), "{n} at {p:?}");
        lo = lo.min(n);
        hi = hi.max(n);
    }
    assert!(hi - lo > 0.5, "the noise barely varies: {lo}..{hi}");
}

#[test]
fn a_full_sweep_is_the_height_share_and_the_field_is_continuous_across_cells() {
    let z = [0.0, 2.0];
    assert_eq!(field([0.3, 0.1, 2.0], 0.5, 1.0, z), 0.0);
    assert_eq!(field([0.3, 0.1, 0.0], 0.5, 1.0, z), 1.0);
    assert_eq!(field([0.3, 0.1, 0.0], 0.5, -1.0, z), 0.0);
    // Across a lattice plane the noise does not jump.
    let a = field([0.999_99, 0.3, 0.2], 1.0, 0.0, z);
    let b = field([1.000_01, 0.3, 0.2], 1.0, 0.0, z);
    assert!((a - b).abs() < 1e-3, "{a} {b}");
}

#[test]
fn a_renderer_without_the_clip_hides_an_object_past_half() {
    let d = |ghost, eaten| HdDissolve {
        ghost,
        threshold: 0.5,
        edge_to: 0.5,
        edge_color: [0.0; 3],
        cell: 0.25,
        sweep: 0.0,
        z_range: [0.0, 1.0],
        eaten,
    };
    let all = [d(1, 0.4), d(2, 0.6)];
    assert!(!hidden_unclipped(&all, 1));
    assert!(hidden_unclipped(&all, 2));
    assert!(!hidden_unclipped(&all, 3));
}

#[test]
fn the_object_matrix_takes_a_placed_point_back_to_the_objects_space() {
    let mut m = Mat::from_axis_angle([0.0, 0.0, 1.0], 0.7);
    m.set_column3([3.0, -2.0, 1.0]);
    let p = [0.5, 0.25, 1.5];
    let w = m.mul_p(p);
    let c = HdDissolve::to_object(&m);
    let back: [f32; 3] =
        std::array::from_fn(|r| c[r] * w[0] + c[4 + r] * w[1] + c[8 + r] * w[2] + c[12 + r]);
    for k in 0..3 {
        assert!((back[k] - p[k]).abs() < 1e-5, "{back:?} {p:?}");
    }
}
