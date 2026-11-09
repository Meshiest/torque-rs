use super::*;
use crate::caps::MAX_SCALE;

/// A small seeded generator: xorshift64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn quat(&mut self) -> Quat {
        loop {
            let q = [0; 4].map(|_| self.unit() * 2.0 - 1.0);
            let length = q.iter().map(|v| v * v).sum::<f32>().sqrt();
            if (0.1..=1.0).contains(&length) {
                return q.map(|v| v / length);
            }
        }
    }
}

/// The angle between two unit quaternions, either sign, from the chord (an `acos` of the dot product loses the small angles).
fn angle(a: &Quat, b: &Quat) -> f32 {
    let chord = |sign: f64| {
        a.iter()
            .zip(b)
            .map(|(x, y)| (f64::from(*x) - sign * f64::from(*y)).powi(2))
            .sum::<f64>()
            .sqrt()
    };
    (2.0 * (chord(1.0).min(chord(-1.0)) / 2.0).asin()) as f32
}

#[test]
fn translation_lands_within_half_a_step() {
    let mut r = Rng(1);
    for _ in 0..10_000 {
        let t = [0; 3].map(|_| (r.unit() * 2.0 - 1.0) * MAX_TRANSLATION);
        let local = Local {
            translation: t,
            ..Local::IDENTITY
        };
        let back = quantize(&local).translation;
        for k in 0..3 {
            // Half a step, and the spacing of f32 at the largest values.
            assert!(
                (back[k] - t[k]).abs() <= 0.5 / TRANSLATION_STEPS + 1e-3,
                "{t:?} {back:?}"
            );
        }
    }
}

#[test]
fn rotation_error_stays_under_the_bound() {
    let mut r = Rng(2);
    let mut worst = 0.0f32;
    for _ in 0..100_000 {
        let q = r.quat();
        let back = decode_rotation(&encode_rotation(&q));
        worst = worst.max(angle(&q, &back));
        let length = back.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((length - 1.0).abs() < 1e-5);
    }
    assert!(worst <= 2.5e-4, "{worst}");
}

#[test]
fn scale_error_is_relative_and_ends_are_kept() {
    let mut r = Rng(3);
    for _ in 0..10_000 {
        let s = MIN_SCALE * (MAX_SCALE / MIN_SCALE).powf(r.unit());
        let back = decode_scale(encode_scale(s));
        assert!((back / s - 1.0).abs() <= 2.5e-4, "{s} {back}");
    }
    assert_eq!(
        decode_scale(encode_scale(MAX_SCALE)),
        scale_value(encode_scale(MAX_SCALE)).clamp(MIN_SCALE, MAX_SCALE)
    );
    assert!((decode_scale(encode_scale(MIN_SCALE)) / MIN_SCALE - 1.0).abs() <= 2.5e-4);
    assert_eq!(decode_scale(0), MIN_SCALE);
    assert_eq!(decode_scale(u16::MAX), MAX_SCALE);
    assert_eq!(decode_scale(encode_scale(1.0)), 1.0);
}

#[test]
fn quantizing_twice_changes_nothing() {
    let mut r = Rng(4);
    for _ in 0..10_000 {
        let local = Local {
            translation: [0; 3].map(|_| (r.unit() * 2.0 - 1.0) * 500.0),
            rotation: r.quat(),
            scale: [0; 3].map(|_| 0.1 + r.unit() * 10.0),
        };
        let once = QuantLocal::from_local(&local);
        let twice = QuantLocal::from_local(&once.to_local());
        assert_eq!(once.translation, twice.translation);
        assert_eq!(once.scale, twice.scale);
        assert!(angle(&once.to_local().rotation, &twice.to_local().rotation) < 1e-3);
        assert!(once.check().is_ok());
    }
}

#[test]
fn any_code_decodes_to_a_valid_pose_and_check_refuses_the_wild_ones() {
    let mut r = Rng(5);
    let mut refused = 0;
    for _ in 0..100_000 {
        let word = r.next();
        let code = QuantLocal {
            translation: [word as i32, (word >> 16) as i32, (word >> 32) as i32],
            rotation: QuantRotation {
                dropped: (word >> 8) as u8,
                parts: [(word >> 3) as u16, (word >> 19) as u16, (word >> 40) as u16],
            },
            scale: [(word >> 7) as u16, (word >> 23) as u16, (word >> 45) as u16],
        };
        assert_eq!(code.to_local().validate(), Ok(()), "{code:?}");
        refused += usize::from(code.check().is_err());
    }
    assert!(refused > 90_000);
    let hostile = Local {
        translation: [f32::NAN, f32::INFINITY, -1e30],
        rotation: [f32::NAN; 4],
        scale: [f32::NAN, -1.0, 0.0],
    };
    assert_eq!(quantize(&hostile).validate(), Ok(()));
}
