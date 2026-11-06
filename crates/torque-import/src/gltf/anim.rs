//! An animation channel's curve, evaluated at any time as glTF defines it: step, linear (spherical for rotations) and cubic
//! Hermite splines.

use glam::Vec4;
use torque_modelcore::math;

/// How a curve moves between its keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Interp {
    Step,
    Linear,
    Cubic,
}

/// A channel's keys: times (non-decreasing) and values (`dims` numbers each; a cubic curve keeps in-tangent, value and
/// out-tangent for each key).
pub(crate) struct Curve {
    pub times: Vec<f32>,
    pub values: Vec<[f32; 4]>,
    pub interp: Interp,
    pub rotation: bool,
}

impl Curve {
    /// The value of key `k`.
    pub fn key(&self, k: usize) -> [f32; 4] {
        let at = if self.interp == Interp::Cubic {
            3 * k + 1
        } else {
            k
        };
        self.values.get(at).copied().unwrap_or_default()
    }

    /// The value at time `t` (the first key's before it, the last key's after it).
    pub fn sample(&self, t: f32) -> [f32; 4] {
        let n = self.times.len();
        if n == 0 {
            return [0.0; 4];
        }
        let after = self.times.partition_point(|&x| x <= t);
        if after == 0 {
            return self.key(0);
        }
        if after >= n {
            return self.key(n - 1);
        }
        let k = after - 1;
        let (t0, t1) = (self.times[k], self.times[k + 1]);
        let span = t1 - t0;
        if span <= 0.0 || t == t0 {
            return self.key(k);
        }
        let s = (t - t0) / span;
        let (a, b) = (self.key(k), self.key(k + 1));
        let out = match self.interp {
            Interp::Step => a,
            Interp::Linear if self.rotation => math::slerp(a, b, s),
            Interp::Linear => Vec4::from(a).lerp_monotonic(b.into(), s).to_array(),
            Interp::Cubic => {
                let tangent =
                    |i: usize| Vec4::from(self.values.get(i).copied().unwrap_or_default());
                let (out_tangent, in_tangent) = (tangent(3 * k + 2), tangent(3 * (k + 1)));
                let (s2, s3) = (s * s, s * s * s);
                let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
                let h10 = s3 - 2.0 * s2 + s;
                let h01 = -2.0 * s3 + 3.0 * s2;
                let h11 = s3 - s2;
                (Vec4::from(a) * h00
                    + out_tangent * (h10 * span)
                    + Vec4::from(b) * h01
                    + in_tangent * (h11 * span))
                    .to_array()
            }
        };
        if self.rotation {
            math::normalize_quat(out)
        } else {
            out
        }
    }
}
