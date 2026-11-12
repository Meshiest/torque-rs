//! `GjkCollisionState`: the distance between two convexes by GJK.
//!
//! The state keeps the simplex: the support points of each convex in its own space (`p`, `q`), their differences in the world
//! (`y`), the set (`bits`), the sub-determinants (`det`, 16 x 4) and the dot products (`dp`, 4 x 4).
//! [`GjkState::distance`] stops when the estimate is past `dont_care`, when it no longer improves by more
//! than the relative error 1e-5, when a support point repeats or the simplex cannot shrink (an "irregularity"), when the simplex
//! is full, when the distance is below 0.001, or after **16 iterations**: the counter is checked after its increment
//! against 15 ([`MAX_GJK_ITERATIONS`]).

use crate::convex::{Convex, World};
use crate::library;
use crate::math::vec3::sub;
use crate::math::{F3, Matrix, dot, inverse, mul_p, mul_v};
use crate::work;

/// The GJK iteration limit: the counter stops the loop once past 15.
pub const MAX_GJK_ITERATIONS: u32 = 16;
/// The relative error (`rel_error`).
pub const GJK_REL_ERROR: f32 = 1e-5;
/// The distance below which the loop stops.
pub const GJK_TOLERANCE: f32 = 0.001;

/// One pair's GJK state.
#[derive(Debug, Clone, PartialEq)]
pub struct GjkState {
    pub a: Convex,
    pub b: Convex,
    /// The current estimate of the distance.
    pub dist: f32,
    /// The vector between the closest points.
    pub v: F3,
    pub p: [F3; 4],
    pub q: [F3; 4],
    pub y: [F3; 4],
    pub bits: u32,
    pub all_bits: u32,
    pub det: [[f32; 4]; 16],
    pub dp: [[f32; 4]; 4],
    pub last: usize,
    pub last_bit: u32,
    /// Iterations of the last `distance` (kept per state).
    pub iterations: u32,
}

/// `VectorF::len` as `distance` and `reset` take it: `mSqrt` of the vector's dot with itself.
fn length(v: F3) -> f32 {
    dot(v, v).sqrt()
}

/// `compute_det` as `gjk.cpp` writes it: the new point's dot products with the simplex, then the sub-determinants.
pub fn plain_compute_det(s: &mut GjkState) {
    use crate::math::plain::dot;
    let last = s.last;
    for i in 0..4 {
        if s.bits & (1 << i) != 0 {
            let v = dot(s.y[i], s.y[last]);
            s.dp[i][last] = v;
            s.dp[last][i] = v;
        }
    }
    s.dp[last][last] = dot(s.y[last], s.y[last]);
    let lb = s.last_bit as usize;
    s.det[lb][last] = 1.0;
    for j in 0..4 {
        let sj = 1usize << j;
        if s.bits as usize & sj == 0 {
            continue;
        }
        let s2 = sj | lb;
        let dp = s.dp;
        s.det[s2][j] = dp[last][last] - dp[last][j];
        s.det[s2][last] = dp[j][j] - dp[j][last];
        for k in 0..j {
            let sk = 1usize << k;
            if s.bits as usize & sk == 0 {
                continue;
            }
            let s3 = sk | s2;
            let det = s.det;
            s.det[s3][k] =
                det[s2][j] * (dp[j][j] - dp[j][k]) + det[s2][last] * (dp[last][j] - dp[last][k]);
            s.det[s3][j] = det[sk | lb][k] * (dp[k][k] - dp[k][j])
                + det[sk | lb][last] * (dp[last][k] - dp[last][j]);
            s.det[s3][last] = det[sk | sj][k] * (dp[k][k] - dp[k][last])
                + det[sk | sj][j] * (dp[j][k] - dp[j][last]);
        }
    }
    if s.all_bits == 15 {
        let dp = s.dp;
        let det = s.det;
        s.det[15][0] = det[14][1] * (dp[1][1] - dp[1][0])
            + det[14][2] * (dp[2][1] - dp[2][0])
            + det[14][3] * (dp[3][1] - dp[3][0]);
        s.det[15][1] = det[13][0] * (dp[0][0] - dp[0][1])
            + det[13][2] * (dp[2][0] - dp[2][1])
            + det[13][3] * (dp[3][0] - dp[3][1]);
        s.det[15][2] = det[11][0] * (dp[0][0] - dp[0][2])
            + det[11][1] * (dp[1][0] - dp[1][2])
            + det[11][3] * (dp[3][0] - dp[3][2]);
        s.det[15][3] = det[7][0] * (dp[0][0] - dp[0][3])
            + det[7][1] * (dp[1][0] - dp[1][3])
            + det[7][2] * (dp[2][0] - dp[2][3]);
    }
}

/// `distance`'s test `mFabs(mDist - mu) <= mDist * rel_error`.
pub fn plain_converged(dist: f32, mu: f32) -> bool {
    (dist - mu).abs() <= dist * GJK_REL_ERROR
}

impl GjkState {
    /// The constructor and `set`: the pair, an empty simplex, then `reset`.
    pub fn new(world: &dyn World, a: Convex, b: Convex, a2w: &Matrix, b2w: &Matrix) -> GjkState {
        let mut s = GjkState {
            a,
            b,
            dist: 0.0,
            v: [0.0; 3],
            p: [[0.0; 3]; 4],
            q: [[0.0; 3]; 4],
            y: [[0.0; 3]; 4],
            bits: 0,
            all_bits: 0,
            det: [[0.0; 4]; 16],
            dp: [[0.0; 4]; 4],
            last: 0,
            last_bit: 1,
            iterations: 0,
        };
        s.reset(world, a2w, b2w);
        s
    }

    /// `swap`: the two convexes exchanged and `v` negated (its sign bits flipped).
    pub fn swap(&mut self) {
        std::mem::swap(&mut self.a, &mut self.b);
        self.v = [-self.v[0], -self.v[1], -self.v[2]];
    }

    /// `reset`: `v` the difference of the two convexes' support points for the zero vector, moved to the world
    /// (`mulP` each, then the difference of the floats), and `dist` its length.
    pub fn reset(&mut self, world: &dyn World, a2w: &Matrix, b2w: &Matrix) {
        let pa = self.a.support(world, [0.0; 3]);
        let sa = mul_p(a2w, pa);
        let pb = self.b.support(world, [0.0; 3]);
        let sb = mul_p(b2w, pb);
        self.v = sub(sa, sb);
        self.dist = length(self.v);
    }

    /// `compute_det` (through the table): the new point's dot products with the simplex, then the sub-determinants.
    fn compute_det(&mut self) {
        (library::get().gjk_compute_det)(self);
    }

    /// `valid`: for every point of the full set, a member of `s` needs a determinant above zero, a non-member one not
    /// above zero; every comparison is false for a NaN, so a NaN passes both tests.
    fn valid(&self, s: u32) -> bool {
        for i in 0..4 {
            let bit = 1u32 << i;
            if self.all_bits & bit == 0 {
                continue;
            }
            if s & bit != 0 {
                if self.det[s as usize][i] <= 0.0 {
                    return false;
                }
            } else if self.det[(s | bit) as usize][i] > 0.0 {
                return false;
            }
        }
        true
    }

    /// `compute_vector`: `v` the determinant-weighted sum of the simplex's points, times the reciprocal of the
    /// determinants' sum.
    fn compute_vector(&mut self, bits: u32) {
        let mut sum = 0.0f32;
        let mut v = [0.0f32; 3];
        for i in 0..4 {
            if bits & (1 << i) == 0 {
                continue;
            }
            let det = self.det[bits as usize][i];
            sum += det;
            for (vc, yc) in v.iter_mut().zip(self.y[i]) {
                *vc += yc * det;
            }
        }
        let inv = 1.0 / sum;
        self.v = [v[0] * inv, v[1] * inv, v[2] * inv];
    }

    /// `distance` between `a` at `a2w` and `b` at `b2w`: see the module. `inverses` are the world-to-object matrices when
    /// the caller has them; else the inverses of the two transforms. Each iteration is charged; a refused charge ends the loop as
    /// the iteration limit does.
    pub fn distance(
        &mut self,
        world: &mut dyn World,
        a2w: &Matrix,
        b2w: &Matrix,
        dont_care: f32,
        inverses: Option<(&Matrix, &Matrix)>,
    ) -> f32 {
        self.iterations = 0;
        let (w2a, w2b) = match inverses {
            Some((x, y)) => (*x, *y),
            None => (inverse(a2w), inverse(b2w)),
        };
        self.reset(world, a2w, b2w);
        self.bits = 0;
        self.all_bits = 0;
        let mut mu = 0.0f32;
        loop {
            self.last = 0;
            self.last_bit = 1;
            while self.bits & self.last_bit != 0 && self.last < 3 {
                self.last += 1;
                self.last_bit <<= 1;
            }
            let neg = [-self.v[0], -self.v[1], -self.v[2]];
            let va = mul_v(&w2a, neg);
            self.p[self.last] = self.a.support(world, va);
            let vb = mul_v(&w2b, self.v);
            self.q[self.last] = self.b.support(world, vb);
            let sa = mul_p(a2w, self.p[self.last]);
            let sb = mul_p(b2w, self.q[self.last]);
            let w = sub(sa, sb);
            let nm = dot(self.v, w) / self.dist;
            if nm > mu {
                mu = nm;
            }
            if mu > dont_care {
                return mu;
            }
            if (library::get().gjk_converged)(self.dist, mu) {
                return self.dist;
            }
            self.iterations += 1;
            for i in 0..4 {
                if self.all_bits & (1 << i) != 0 && self.y[i] == w {
                    return self.dist;
                }
            }
            if self.iterations > MAX_GJK_ITERATIONS - 1 || !world.charge(work::GJK_ITERATION) {
                return self.dist;
            }
            self.y[self.last] = w;
            self.all_bits = self.last_bit | self.bits;
            self.compute_det();
            let mut found = false;
            let mut s = self.bits;
            while s != 0 {
                if s & self.bits == s && self.valid(s | self.last_bit) {
                    self.bits = s | self.last_bit;
                    if self.bits != 15 {
                        self.compute_vector(self.bits);
                    }
                    found = true;
                    break;
                }
                s -= 1;
            }
            if !found {
                if !self.valid(self.last_bit) {
                    return self.dist;
                }
                self.bits = self.last_bit;
                self.v = self.y[self.last];
            }
            self.dist = length(self.v);
            if !(self.bits < 15 && self.dist > GJK_TOLERANCE) {
                break;
            }
        }
        // The distance stays unless mu is ordered (not NaN) and not above zero.
        if self.bits == 15 && mu <= 0.0 {
            self.dist = 0.0;
        }
        self.dist
    }

    /// `getClosestPoints`: the closest points on `a` and `b` in their own spaces, the simplex's support points weighted by the
    /// determinants of the current set.
    pub fn closest_points(&self) -> (F3, F3) {
        let mut pa = [0.0f32; 3];
        let mut pb = [0.0f32; 3];
        let mut sum = 0.0f32;
        for i in 0..4 {
            if self.bits & (1 << i) == 0 {
                continue;
            }
            let det = self.det[self.bits as usize][i];
            sum += det;
            for (c, (ac, bc)) in pa.iter_mut().zip(pb.iter_mut()).enumerate() {
                *ac += self.p[i][c] * det;
                *bc += self.q[i][c] * det;
            }
        }
        let inv = 1.0 / sum;
        let scale = |v: F3| [v[0] * inv, v[1] * inv, v[2] * inv];
        (scale(pa), scale(pb))
    }
}
