//! The checks every value passes before Rapier sees it: finite, held to a magnitude, a rotation normalized.
//!
//! Each returns `None` for a value that is not finite (or a rotation too near zero to normalize); callers refuse those with
//! [`crate::Error::NonFinite`] rather than guessing a value. The plain guards are [`torque_collide::math`]'s (the scene crate's).

/// Whether every component is finite.
pub fn finite<const N: usize>(v: &[f32; N]) -> bool {
    torque_collide::math::all_finite(v)
}

/// `v` with each component held to `-max..=max`; `None` when one is not finite.
pub use torque_collide::math::finite_coords as coords;

/// `v` scaled down to a length of at most `max`; `None` when a component is not finite. The length is taken in `f64`, so a
/// vector of huge finite components does not overflow on the way.
pub fn magnitude(v: [f32; 3], max: f32) -> Option<[f32; 3]> {
    if !finite(&v) {
        return None;
    }
    let len = v
        .iter()
        .map(|&x| f64::from(x) * f64::from(x))
        .sum::<f64>()
        .sqrt();
    let max = f64::from(max);
    if len <= max {
        return Some(v);
    }
    let k = max / len;
    Some(v.map(|x| (f64::from(x) * k) as f32))
}

/// A quaternion `[x, y, z, w]` made unit length; `None` when it is not finite or its length is under `1e-6`. Its sign is kept
/// (`q` and `-q` are the same rotation; a peer that sends one keeps it).
pub fn rotation(q: [f32; 4]) -> Option<[f32; 4]> {
    if !finite(&q) {
        return None;
    }
    let len = q
        .iter()
        .map(|&x| f64::from(x) * f64::from(x))
        .sum::<f64>()
        .sqrt();
    if !(len >= 1e-6) {
        return None;
    }
    Some(q.map(|x| (f64::from(x) / len) as f32))
}

/// A scalar held to `lo..=hi`; `None` when it is not finite.
pub use torque_collide::math::finite_clamp as scalar;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_values_are_refused_and_finite_ones_held() {
        assert_eq!(coords([f32::NAN, 0.0, 0.0], 10.0), None);
        assert_eq!(coords([1e30, -1e30, 3.0], 10.0), Some([10.0, -10.0, 3.0]));
        assert_eq!(magnitude([f32::INFINITY, 0.0, 0.0], 1.0), None);
        let m = magnitude([f32::MAX, f32::MAX, 0.0], 2.0).unwrap();
        assert!((m[0] - 2.0f32.sqrt()).abs() < 1e-6, "{m:?}");
        assert_eq!(rotation([0.0; 4]), None);
        assert_eq!(rotation([0.0, 0.0, 0.0, -2.0]), Some([0.0, 0.0, 0.0, -1.0]));
        assert_eq!(scalar(f32::NAN, 0.0, 1.0), None);
        assert_eq!(scalar(5.0, 0.0, 1.0), Some(1.0));
    }
}
