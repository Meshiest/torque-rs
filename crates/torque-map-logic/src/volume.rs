//! Convex volumes: a brush of a brush model (a trigger's, a door's) as its corner points, outward planes and edges, in the
//! model's own space; placed by the entity's origin and a turn, and tested against boxes through torque-collide's separating
//! axis overlap ([`torque_collide::sat`]).

use torque_collide::sat::{Penetration, Polytope, penetration};

/// A convex volume.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Volume {
    pub points: Vec<[f32; 3]>,
    /// `[nx, ny, nz, d]`, unit normals facing out: inside where `n . p + d <= 0`.
    pub planes: Vec<[f32; 4]>,
    /// Index pairs into `points`.
    pub edges: Vec<[u32; 2]>,
}

/// The rotation of Source angles (pitch, yaw, roll in degrees), row-major.
pub fn rotation(angles: [f32; 3]) -> [[f32; 3]; 3] {
    let [p, y, r] = angles.map(f32::to_radians);
    let (sp, cp) = p.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sr, cr) = r.sin_cos();
    [
        [cp * cy, sr * sp * cy - cr * sy, cr * sp * cy + sr * sy],
        [cp * sy, sr * sp * sy + cr * cy, cr * sp * sy - sr * cy],
        [-sp, sr * cp, cr * cp],
    ]
}

fn mul(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

/// Where a volume is: an offset and a turn (Source angles) about the model's origin.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pose {
    pub origin: [f32; 3],
    pub angles: [f32; 3],
}

impl Pose {
    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let q = if self.angles == [0.0; 3] {
            p
        } else {
            mul(&rotation(self.angles), p)
        };
        [0, 1, 2].map(|k| q[k] + self.origin[k])
    }
}

impl Volume {
    /// The box with corners `min` and `max`.
    pub fn of_box(min: [f32; 3], max: [f32; 3]) -> Volume {
        let points = (0..8)
            .map(|i| {
                [
                    if i & 1 == 0 { min[0] } else { max[0] },
                    if i & 2 == 0 { min[1] } else { max[1] },
                    if i & 4 == 0 { min[2] } else { max[2] },
                ]
            })
            .collect();
        let planes = vec![
            [1.0, 0.0, 0.0, -max[0]],
            [-1.0, 0.0, 0.0, min[0]],
            [0.0, 1.0, 0.0, -max[1]],
            [0.0, -1.0, 0.0, min[1]],
            [0.0, 0.0, 1.0, -max[2]],
            [0.0, 0.0, -1.0, min[2]],
        ];
        let mut edges = Vec::new();
        for i in 0u32..8 {
            for bit in [1u32, 2, 4] {
                if i & bit == 0 {
                    edges.push([i, i | bit]);
                }
            }
        }
        Volume {
            points,
            planes,
            edges,
        }
    }

    /// The smallest and largest corner (zeros when it has no points).
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        if self.points.is_empty() {
            return ([0.0; 3], [0.0; 3]);
        }
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in &self.points {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    }

    /// The volume placed by `pose`.
    pub fn placed(&self, pose: &Pose) -> Volume {
        let rot = (pose.angles != [0.0; 3]).then(|| rotation(pose.angles));
        let points = self.points.iter().map(|&p| pose.point(p)).collect();
        let planes = self
            .planes
            .iter()
            .map(|pl| {
                let n = match &rot {
                    Some(r) => mul(r, [pl[0], pl[1], pl[2]]),
                    None => [pl[0], pl[1], pl[2]],
                };
                // A point q on the old plane goes to R q + o; d' = -(n' . (R q + o)) = d - n' . o.
                let d =
                    pl[3] - (n[0] * pose.origin[0] + n[1] * pose.origin[1] + n[2] * pose.origin[2]);
                [n[0], n[1], n[2], d]
            })
            .collect();
        Volume {
            points,
            planes,
            edges: self.edges.clone(),
        }
    }

    /// Whether `p` is inside every plane (or outside one by at most `tol`).
    pub fn contains(&self, p: [f32; 3], tol: f32) -> bool {
        !self.planes.is_empty()
            && self
                .planes
                .iter()
                .all(|pl| pl[0] * p[0] + pl[1] * p[1] + pl[2] * p[2] + pl[3] <= tol)
    }

    /// As torque-collide's polytope.
    pub fn polytope(&self) -> Polytope {
        let mut p = Polytope::default();
        for &pt in &self.points {
            p.add_point(pt);
        }
        for pl in &self.planes {
            p.add_normal([pl[0], pl[1], pl[2]]);
        }
        for e in &self.edges {
            p.add_edge(e[0], e[1]);
        }
        p
    }

    /// Whether the box `min`..`max` and this volume overlap (more than touching), by the separating axis test.
    pub fn overlaps_box(&self, min: [f32; 3], max: [f32; 3]) -> bool {
        let (lo, hi) = self.bounds();
        if (0..3).any(|k| !(lo[k] < max[k] && min[k] < hi[k])) {
            return false;
        }
        let a = Volume::of_box(min, max).polytope();
        let mut out = Penetration::default();
        penetration(&a, &self.polytope(), &mut out)
    }

    /// The box grown by `by` on every side.
    pub fn grown_bounds(&self, by: f32) -> ([f32; 3], [f32; 3]) {
        let (lo, hi) = self.bounds();
        (lo.map(|v| v - by), hi.map(|v| v + by))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_overlaps_what_it_crosses_and_not_what_it_touches() {
        let v = Volume::of_box([0.0; 3], [10.0; 3]);
        assert!(v.overlaps_box([5.0; 3], [15.0; 3]));
        assert!(!v.overlaps_box([10.0, 0.0, 0.0], [20.0, 10.0, 10.0]));
        assert!(!v.overlaps_box([11.0; 3], [12.0; 3]));
        assert!(v.contains([1.0, 1.0, 1.0], 0.0));
        assert!(!v.contains([-1.0, 1.0, 1.0], 0.0));
    }

    #[test]
    fn placing_moves_points_and_planes_together() {
        let v = Volume::of_box([0.0; 3], [10.0, 2.0, 2.0]);
        let p = v.placed(&Pose {
            origin: [100.0, 0.0, 0.0],
            angles: [0.0, 90.0, 0.0],
        });
        // Turned a quarter about z: the long side now runs along y.
        let (lo, hi) = p.bounds();
        assert!(
            (lo[0] - 98.0).abs() < 1e-3 && (hi[1] - 10.0).abs() < 1e-3,
            "{lo:?} {hi:?}"
        );
        assert!(p.contains([99.0, 5.0, 1.0], 1e-3));
        assert!(!p.contains([105.0, 1.0, 1.0], 1e-3));
        assert!(p.overlaps_box([98.5, 4.0, 0.5], [99.5, 6.0, 1.5]));
    }
}
