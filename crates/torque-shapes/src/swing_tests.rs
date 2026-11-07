use std::f32::consts::{FRAC_PI_2, PI};

use super::*;

fn dot4(a: [f32; 4], b: [f32; 4]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

fn unit4(q: [f32; 4]) -> [f32; 4] {
    let l = dot4(q, q).sqrt();
    q.map(|c| c / l)
}

#[test]
fn composed_rotations_turn_as_their_matrices_multiply() {
    let a = unit4([0.3, -0.5, 0.2, 0.8]);
    let b = unit4([-0.6, 0.1, 0.4, 0.5]);
    let m = math::mat_mul(&math::quat_matrix(a), &math::quat_matrix(b));
    let q = math::quat_matrix(compose(a, b));
    let undone = math::quat_matrix(compose(conjugate(a), compose(a, b)));
    let mb = math::quat_matrix(b);
    for k in [0, 1, 2, 4, 5, 6, 8, 9, 10] {
        assert!(
            (m[k] - q[k]).abs() < 1e-5,
            "element {k}: {} and {}",
            m[k],
            q[k]
        );
        assert!((undone[k] - mb[k]).abs() < 1e-5, "undone, element {k}");
    }
}

#[test]
fn a_turn_about_an_axis_is_right_handed() {
    let v = apply(about([0.0, 0.0, 1.0], FRAC_PI_2), [1.0, 0.0, 0.0]);
    assert!(v[0].abs() < 1e-5 && (v[1] - 1.0).abs() < 1e-5, "{v:?}");
}

/// A limb along -z, from hanging to raised straight ahead along +y and turned all but half round about itself.
fn raised() -> [f32; 4] {
    compose(
        about([1.0, 0.0, 0.0], FRAC_PI_2),
        about([0.0, 0.0, 1.0], PI - 0.04),
    )
}

#[test]
fn a_limb_swings_along_the_great_circle_and_turns_about_itself() {
    let axis = Some([0.0, 0.0, -1.0]);
    let to = raised();
    for k in 0..=10 {
        let d = apply(
            limb(IDENTITY, to, axis, None, k as f32 / 10.0),
            [0.0, 0.0, -1.0],
        );
        // Down and forward, never out to a side or back.
        assert!(
            d[0].abs() < 1e-4 && d[1] > -1e-4 && d[2] < 1e-4,
            "{k}: {d:?}"
        );
    }
    let end = limb(IDENTITY, to, axis, None, 1.0);
    assert!(dot4(end, to).abs() > 0.9999, "ends at the target: {end:?}");
}

#[test]
fn a_limb_with_a_side_leans_out_to_it_on_the_way() {
    let axis = Some([0.0, 0.0, -1.0]);
    let to = raised();
    let mid = apply(
        limb(IDENTITY, to, axis, Some([1.0, 0.0, 0.0]), 0.5),
        [0.0, 0.0, -1.0],
    );
    // Half the way between down and ahead, leaned out to the right.
    assert!(mid[0] > 0.3 && mid[1] > 0.3 && mid[2] < -0.3, "{mid:?}");
    let end = limb(IDENTITY, to, axis, Some([1.0, 0.0, 0.0]), 1.0);
    assert!(dot4(end, to).abs() > 0.9999, "ends at the target: {end:?}");
    let start = limb(IDENTITY, to, axis, Some([1.0, 0.0, 0.0]), 0.0);
    assert!(
        dot4(start, IDENTITY).abs() > 0.9999,
        "starts where it was: {start:?}"
    );
}

/// The blend of a limb along -z from `from` to `to` with side `out`, checked at 101 shares: no NaN, the start and the end
/// where they belong, and no step between shares a pop.
fn reaches(name: &str, from: [f32; 4], to: [f32; 4], out: Option<[f32; 3]>) {
    let axis = Some([0.0, 0.0, -1.0]);
    let mut last = from;
    for k in 0..=100 {
        let q = limb(from, to, axis, out, k as f32 / 100.0);
        assert!(q.iter().all(|c| c.is_finite()), "{name} at {k}: {q:?}");
        let step = 1.0 - dot4(unit4(q), unit4(last)).abs();
        assert!(step < 0.01, "{name}: a pop of {step} at {k}");
        last = q;
    }
    let start = limb(from, to, axis, out, 0.0);
    let end = limb(from, to, axis, out, 1.0);
    assert!(
        dot4(unit4(start), from).abs() > 0.99999,
        "{name} starts at {start:?}"
    );
    assert!(
        dot4(unit4(end), to).abs() > 0.99999,
        "{name} ends at {end:?}"
    );
}

#[test]
fn a_limb_reaches_its_end_whichever_way_it_points() {
    let right = Some([1.0, 0.0, 0.0]);
    // The same way at both ends, turned about itself.
    reaches("same way", IDENTITY, about([0.0, 0.0, 1.0], 2.0), right);
    reaches(
        "same way, no side",
        IDENTITY,
        about([0.0, 0.0, 1.0], 2.0),
        None,
    );
    // Exactly opposite (down to up): with a side square to it, along it either way, and none.
    let up = about([1.0, 0.0, 0.0], PI);
    reaches("opposite, side out", IDENTITY, up, right);
    reaches(
        "opposite, side along the start",
        IDENTITY,
        up,
        Some([0.0, 0.0, -1.0]),
    );
    reaches(
        "opposite, side along the end",
        IDENTITY,
        up,
        Some([0.0, 0.0, 1.0]),
    );
    reaches("opposite, no side", IDENTITY, up, None);
    // All but opposite.
    for eps in [1e-3f32, 1e-5] {
        let l = (1.0 + eps * eps).sqrt();
        let to = about([1.0 / l, eps / l, 0.0], PI - eps);
        reaches(
            &format!("{eps} from opposite, side out"),
            IDENTITY,
            to,
            right,
        );
        reaches(
            &format!("{eps} from opposite, side along"),
            IDENTITY,
            to,
            Some([0.0, 0.0, -1.0]),
        );
        reaches(&format!("{eps} from opposite, no side"), IDENTITY, to, None);
    }
}
