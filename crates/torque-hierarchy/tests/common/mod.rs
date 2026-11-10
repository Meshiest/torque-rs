//! A seeded generator, a host with rigid node matrices, and pose comparisons for the tests.

#![allow(dead_code)]

use std::collections::BTreeMap;

use torque_collide::math::{Matrix, quat_matrix};
use torque_hierarchy::{Host, Local, ObjId, Pose};

/// SplitMix64: small, seeded and the same everywhere.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// 0 to `n - 1`.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    /// In `[0, 1)`.
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// In `[lo, hi)`.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    pub fn quat(&mut self) -> [f32; 4] {
        loop {
            let q = [
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
            ];
            let l = q.iter().map(|v| v * v).sum::<f32>().sqrt();
            if (0.1..=1.0).contains(&l) {
                return q.map(|v| v / l);
            }
        }
    }
}

pub fn random_pose(r: &mut Rng) -> Pose {
    let mut world = quat_matrix(r.quat());
    world[3] = r.range(-5.0, 5.0);
    world[7] = r.range(-5.0, 5.0);
    world[11] = r.range(-5.0, 5.0);
    Pose {
        world,
        scale: [r.range(0.5, 1.5), r.range(0.5, 1.5), r.range(0.5, 1.5)],
    }
}

pub fn random_local(r: &mut Rng) -> Local {
    Local {
        translation: [r.range(-0.5, 0.5), r.range(-0.5, 0.5), r.range(-0.5, 0.5)],
        rotation: r.quat(),
        scale: [r.range(0.5, 1.5), r.range(0.5, 1.5), r.range(0.5, 1.5)],
    }
}

/// Objects with a pose each; nodes 0 to 3 exist on every object, at a place that depends on the object.
#[derive(Debug, Default)]
pub struct World {
    pub poses: BTreeMap<ObjId, Pose>,
}

impl Host for World {
    fn pose(&self, object: ObjId) -> Option<Pose> {
        self.poses.get(&object).copied()
    }

    fn node_matrix(&self, object: ObjId, node: u16) -> Option<Matrix> {
        if node >= 4 {
            return None;
        }
        let turn = f32::from(node) * 0.3;
        let mut m = quat_matrix([0.0, 0.0, (turn / 2.0).sin(), (turn / 2.0).cos()]);
        m[3] = (f32::from(node) + 1.0) * 0.1;
        m[7] = (object.0 % 3) as f32 * 0.1;
        m[11] = 0.05;
        Some(m)
    }
}

/// How far apart two transforms are: the distance between their origins and the angle between their rotations.
pub fn pose_error(a: &Matrix, b: &Matrix) -> (f32, f32) {
    let t = |m: &Matrix| [f64::from(m[3]), f64::from(m[7]), f64::from(m[11])];
    let (ta, tb) = (t(a), t(b));
    let dist = ((ta[0] - tb[0]).powi(2) + (ta[1] - tb[1]).powi(2) + (ta[2] - tb[2]).powi(2)).sqrt();
    // M = A^T B over the 3x3 parts, then the angle from its antisymmetric part and trace.
    let m = |i: usize, j: usize| -> f64 {
        (0..3)
            .map(|k| f64::from(a[k * 4 + i]) * f64::from(b[k * 4 + j]))
            .sum()
    };
    let v = [
        (m(2, 1) - m(1, 2)) / 2.0,
        (m(0, 2) - m(2, 0)) / 2.0,
        (m(1, 0) - m(0, 1)) / 2.0,
    ];
    let sin = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let cos = (m(0, 0) + m(1, 1) + m(2, 2) - 1.0) / 2.0;
    (dist as f32, sin.atan2(cos) as f32)
}

/// A 64-bit FNV-1a accumulator.
#[derive(Debug, Clone, Copy)]
pub struct Hash(pub u64);

impl Hash {
    pub fn new() -> Hash {
        Hash(0xcbf2_9ce4_8422_2325)
    }

    pub fn add(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    pub fn matrix(&mut self, m: &Matrix) {
        for v in m {
            self.add(u64::from(v.to_bits()));
        }
    }
}
