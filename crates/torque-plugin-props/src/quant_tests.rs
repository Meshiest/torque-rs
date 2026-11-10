use super::*;

/// A small xorshift generator.
fn rng(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

fn float(seed: &mut u64, lo: f32, hi: f32) -> f32 {
    lo + (hi - lo) * ((rng(seed) >> 40) as f32 / (1u64 << 24) as f32)
}

fn state(seed: &mut u64) -> BodyState {
    let q = [0; 4].map(|_| float(seed, -1.0, 1.0));
    let len = q.iter().map(|c| c * c).sum::<f32>().sqrt().max(1.0e-3);
    let reach = [10.0, 1_000.0, 90_000.0][(rng(seed) % 3) as usize];
    BodyState {
        position: [0; 3].map(|_| float(seed, -reach, reach)),
        rotation: q.map(|c| c / len),
        linvel: [0; 3].map(|_| float(seed, -400.0, 400.0)),
        angvel: [0; 3].map(|_| float(seed, -150.0, 150.0)),
        sleeping: false,
    }
}

#[test]
fn a_quantized_state_quantizes_back_to_the_same_state() {
    let mut seed = 0x1234_5678_9abc_def1;
    for _ in 0..50_000 {
        let q = Quantized::of(&state(&mut seed));
        let again = Quantized::of(&q.state());
        assert_eq!(again.pos, q.pos);
        assert_eq!((again.lin, again.ang), (q.lin, q.ang));
        // Two components of nearly the same size may trade places as the largest: the same turn, and stable from then on.
        let (a, b) = (q.rotation(), again.rotation());
        let dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
        assert!(dot.abs() > 0.999_99, "{q:?} -> {again:?}");
        assert_eq!(Quantized::of(&again.state()), again, "{q:?} -> {again:?}");
    }
}

#[test]
fn the_precision_is_the_wires() {
    let mut seed = 99;
    for _ in 0..20_000 {
        let s = state(&mut seed);
        let back = Quantized::of(&s).state();
        for k in 0..3 {
            // Half a step, and the f32 rounding far from the origin.
            let tol = 0.5 / 1024.0 + s.position[k].abs() * 1.0e-7;
            assert!((back.position[k] - s.position[k]).abs() <= tol);
            assert!((back.linvel[k] - s.linvel[k]).abs() <= 0.5 / 256.0 + 1.0e-4);
        }
        // The same rotation (or its negative), within the 15 bits.
        let dot: f32 = s
            .rotation
            .iter()
            .zip(&back.rotation)
            .map(|(a, b)| a * b)
            .sum();
        assert!(dot.abs() > 0.999_99, "{s:?} {back:?}");
        let len: f32 = back.rotation.iter().map(|c| c * c).sum::<f32>().sqrt();
        assert!((len - 1.0).abs() < 1.0e-5);
    }
}

#[test]
fn hostile_and_degenerate_values_are_held() {
    let bad = BodyState {
        position: [f32::NAN, f32::INFINITY, -1.0e30],
        rotation: [0.0; 4],
        linvel: [f32::NAN, 1.0e9, -1.0e9],
        angvel: [f32::NEG_INFINITY, 0.0, 1.0e9],
        sleeping: false,
    };
    let q = Quantized::of(&bad);
    let s = q.state();
    assert!(s.position.iter().all(|v| v.abs() <= MAX_COORD));
    assert_eq!(s.rotation, [0.0, 0.0, 0.0, 1.0]);
    assert!(s.linvel.iter().all(|v| v.abs() <= MAX_SPEED));
    assert!(s.angvel.iter().all(|v| v.abs() <= MAX_SPIN));
    let wild = Quantized::from_words([
        i32::MAX,
        i32::MIN,
        7,
        99,
        70_000,
        -5,
        1,
        i32::MAX,
        0,
        0,
        i32::MIN,
        0,
        0,
    ]);
    assert_eq!(wild, wild.held());
    assert!(wild.state().rotation.iter().all(|c| c.is_finite()));
}

#[test]
fn the_identity_is_exact_and_the_matrix_turns_points() {
    let q = Quantized::default();
    assert_eq!(q.rotation(), [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(Quantized::of(&q.state()), q);
    // A quarter turn about z takes +x to +y.
    let h = std::f32::consts::FRAC_1_SQRT_2;
    let m = matrix([1.0, 2.0, 3.0], [0.0, 0.0, h, h]);
    let p = [1.0, 0.0, 0.0];
    let out: [f32; 3] = std::array::from_fn(|r| {
        m[r * 4] * p[0] + m[r * 4 + 1] * p[1] + m[r * 4 + 2] * p[2] + m[r * 4 + 3]
    });
    assert!((out[0] - 1.0).abs() < 1e-6);
    assert!((out[1] - 3.0).abs() < 1e-6);
    assert!((out[2] - 3.0).abs() < 1e-6);
}
