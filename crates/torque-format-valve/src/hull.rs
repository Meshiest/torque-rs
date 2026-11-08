//! Convex hulls of point sets: an incremental hull in `f64` (the collision pieces of physics models, and the fallback hulls of
//! a model's render mesh or bounding box).
//!
//! A [`Hull`] keeps its corner points, its triangles (outward, counter-clockwise seen from outside) and its distinct planes
//! (unit normals facing out: a point is inside where `n . p + d <= 0`). The builder is bounded: the caller hands it a work
//! budget (one unit per face examined per point added) and it gives up, rather than run on, when the budget is spent. A point set
//! with no volume (all in a line or a plane) has no hull: [`convex_hull`] returns `None`.

use std::collections::{HashMap, HashSet};

/// A convex hull.
#[derive(Debug, Clone, PartialEq)]
pub struct Hull {
    pub points: Vec<[f32; 3]>,
    /// Indices into `points`.
    pub triangles: Vec<[u32; 3]>,
    /// Each triangle's index into `planes`.
    pub triangle_planes: Vec<u32>,
    /// `[nx, ny, nz, d]`: outward unit normals; inside where `n . p + d <= 0`.
    pub planes: Vec<[f32; 4]>,
}

impl Hull {
    /// The smallest and largest corner.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        bounds(&self.points)
    }

    /// Whether `p` is inside every plane, or outside one by no more than `tol`.
    pub fn contains(&self, p: [f32; 3], tol: f32) -> bool {
        self.planes
            .iter()
            .all(|pl| pl[0] * p[0] + pl[1] * p[1] + pl[2] * p[2] + pl[3] <= tol)
    }

    /// The surface area each plane's triangles cover together (one entry per plane).
    pub fn plane_areas(&self) -> Vec<f32> {
        let mut areas = vec![0f64; self.planes.len()];
        for (t, tri) in self.triangles.iter().enumerate() {
            let at = |i: u32| self.points.get(i as usize).copied().map(widen);
            let (Some(a), Some(b), Some(c)) = (at(tri[0]), at(tri[1]), at(tri[2])) else {
                continue;
            };
            let n = cross(sub(b, a), sub(c, a));
            let area = 0.5 * dot(n, n).sqrt();
            if let Some(slot) = self
                .triangle_planes
                .get(t)
                .and_then(|&p| areas.get_mut(p as usize))
            {
                *slot += area;
            }
        }
        areas.into_iter().map(|a| a as f32).collect()
    }

    /// The indices of the `max` planes covering the most area, in plane order (all of them when there are no more than `max`).
    pub fn largest_planes(&self, max: usize) -> Vec<u32> {
        let mut order: Vec<u32> = (0..self.planes.len() as u32).collect();
        if order.len() > max {
            let areas = self.plane_areas();
            order.sort_by(|&a, &b| areas[b as usize].total_cmp(&areas[a as usize]));
            order.truncate(max);
            order.sort_unstable();
        }
        order
    }

    /// The edges that are creases, as index pairs into `points` (the diagonals of a face made of several triangles are left out).
    pub fn edges(&self) -> Vec<[u32; 2]> {
        let mut owner: HashMap<(u32, u32), u32> = HashMap::new();
        for (t, tri) in self.triangles.iter().enumerate() {
            let plane = self.triangle_planes.get(t).copied().unwrap_or(0);
            for k in 0..3 {
                owner.insert((tri[k], tri[(k + 1) % 3]), plane);
            }
        }
        let mut out = Vec::new();
        for tri in &self.triangles {
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                if owner.get(&(a, b)) != owner.get(&(b, a)) {
                    out.push([a.min(b), a.max(b)]);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The box with corners `min` and `max`, as a hull.
    pub fn of_box(min: [f32; 3], max: [f32; 3]) -> Hull {
        let points: Vec<[f32; 3]> = (0..8)
            .map(|i| {
                [
                    if i & 1 == 0 { min[0] } else { max[0] },
                    if i & 2 == 0 { min[1] } else { max[1] },
                    if i & 4 == 0 { min[2] } else { max[2] },
                ]
            })
            .collect();
        // Each face: its four corners counter-clockwise from outside, its outward normal.
        const FACES: [([u32; 4], [f32; 3]); 6] = [
            ([0, 4, 6, 2], [-1.0, 0.0, 0.0]),
            ([1, 3, 7, 5], [1.0, 0.0, 0.0]),
            ([0, 1, 5, 4], [0.0, -1.0, 0.0]),
            ([2, 6, 7, 3], [0.0, 1.0, 0.0]),
            ([0, 2, 3, 1], [0.0, 0.0, -1.0]),
            ([4, 5, 7, 6], [0.0, 0.0, 1.0]),
        ];
        let mut triangles = Vec::new();
        let mut triangle_planes = Vec::new();
        let mut planes = Vec::new();
        for (k, (q, n)) in FACES.iter().enumerate() {
            let corner = points[q[0] as usize];
            let d = -(n[0] * corner[0] + n[1] * corner[1] + n[2] * corner[2]);
            planes.push([n[0], n[1], n[2], d]);
            triangles.push([q[0], q[1], q[2]]);
            triangles.push([q[0], q[2], q[3]]);
            triangle_planes.push(k as u32);
            triangle_planes.push(k as u32);
        }
        Hull {
            points,
            triangles,
            triangle_planes,
            planes,
        }
    }

    /// This hull with each point (and so each plane) passed through `f`, a rigid or uniformly scaled map.
    pub fn mapped(&self, f: &dyn Fn([f32; 3]) -> [f32; 3]) -> Hull {
        let points: Vec<[f32; 3]> = self.points.iter().map(|&p| f(p)).collect();
        // Planes are rebuilt from a triangle each: a map that turns or flips the hull keeps its triangles' winding right only
        // when it is a rotation, so the normals are re-oriented against the hull's centroid.
        let n = points.len().max(1) as f64;
        let mut c = [0f64; 3];
        for p in &points {
            for k in 0..3 {
                c[k] += f64::from(p[k]) / n;
            }
        }
        let mut triangles = self.triangles.clone();
        let mut planes: Vec<[f32; 4]> = Vec::with_capacity(self.planes.len());
        let mut done = vec![false; self.planes.len()];
        planes.resize(self.planes.len(), [0.0; 4]);
        for (t, tri) in triangles.iter_mut().enumerate() {
            let a = widen(points[tri[0] as usize]);
            let b = widen(points[tri[1] as usize]);
            let cc = widen(points[tri[2] as usize]);
            let mut nrm = cross(sub(b, a), sub(cc, a));
            let len = dot(nrm, nrm).sqrt();
            if len > 0.0 {
                nrm = [nrm[0] / len, nrm[1] / len, nrm[2] / len];
            }
            let mut d = -dot(nrm, a);
            if dot(nrm, c) + d > 0.0 {
                tri.swap(1, 2);
                nrm = [-nrm[0], -nrm[1], -nrm[2]];
                d = -d;
            }
            let pi = self.triangle_planes.get(t).copied().unwrap_or(0) as usize;
            if let (Some(slot), Some(flag)) = (planes.get_mut(pi), done.get_mut(pi))
                && !*flag
            {
                *slot = [nrm[0] as f32, nrm[1] as f32, nrm[2] as f32, d as f32];
                *flag = true;
            }
        }
        Hull {
            points,
            triangles,
            triangle_planes: self.triangle_planes.clone(),
            planes,
        }
    }
}

/// The smallest and largest of `points` on each axis (zeros for none).
pub fn bounds(points: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let Some(first) = points.first() else {
        return ([0.0; 3], [0.0; 3]);
    };
    let (mut lo, mut hi) = (*first, *first);
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

fn widen(p: [f32; 3]) -> [f64; 3] {
    p.map(f64::from)
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

struct Face {
    v: [usize; 3],
    n: [f64; 3],
    d: f64,
    alive: bool,
}

impl Face {
    /// A face on `a`, `b`, `c` facing away from `inside`; None when the three are (nearly) in a line.
    fn new(p: &[[f64; 3]], v: [usize; 3], inside: [f64; 3]) -> Option<Face> {
        let (a, b, c) = (p[v[0]], p[v[1]], p[v[2]]);
        let mut n = cross(sub(b, a), sub(c, a));
        let len = dot(n, n).sqrt();
        if len <= 1e-300 || !len.is_finite() {
            return None;
        }
        n = [n[0] / len, n[1] / len, n[2] / len];
        let mut d = -dot(n, a);
        let mut v = v;
        if dot(n, inside) + d > 0.0 {
            v.swap(1, 2);
            n = [-n[0], -n[1], -n[2]];
            d = -d;
        }
        Some(Face {
            v,
            n,
            d,
            alive: true,
        })
    }

    fn dist(&self, q: [f64; 3]) -> f64 {
        dot(self.n, q) + self.d
    }
}

/// The convex hull of `points`, or None when they have no volume (fewer than four distinct points, all in a plane or a line), a
/// point is not finite, or `work` (a budget of face examinations) runs out. `work` is charged what was spent.
pub fn convex_hull(points: &[[f32; 3]], work: &mut u64) -> Option<Hull> {
    let mut p: Vec<[f64; 3]> = points
        .iter()
        .filter(|q| q.iter().all(|v| v.is_finite()))
        .map(|&q| widen(q))
        .collect();
    p.sort_by(|a, b| {
        a[0].total_cmp(&b[0])
            .then(a[1].total_cmp(&b[1]))
            .then(a[2].total_cmp(&b[2]))
    });
    p.dedup();
    if p.len() < 4 {
        return None;
    }
    let (lo, hi) = {
        let f: Vec<[f32; 3]> = p.iter().map(|q| q.map(|v| v as f32)).collect();
        bounds(&f)
    };
    let extent = (0..3)
        .map(|k| f64::from(hi[k]) - f64::from(lo[k]))
        .fold(0.0, f64::max);
    if extent <= 0.0 || extent.is_nan() {
        return None;
    }
    let eps = 1e-10 * extent;

    // The first tetrahedron: the two farthest apart along the longest axis, the farthest from their line, the farthest from
    // their plane.
    let axis = (0..3)
        .max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b])))
        .unwrap_or(0);
    let i0 = (0..p.len()).min_by(|&a, &b| p[a][axis].total_cmp(&p[b][axis]))?;
    let i1 = (0..p.len()).max_by(|&a, &b| p[a][axis].total_cmp(&p[b][axis]))?;
    let line = sub(p[i1], p[i0]);
    let line_len = dot(line, line).sqrt();
    if line_len <= eps {
        return None;
    }
    let off_line = |q: [f64; 3]| {
        let c = cross(line, sub(q, p[i0]));
        dot(c, c).sqrt() / line_len
    };
    let i2 = (0..p.len()).max_by(|&a, &b| off_line(p[a]).total_cmp(&off_line(p[b])))?;
    if off_line(p[i2]) <= eps {
        return None;
    }
    let base = cross(sub(p[i1], p[i0]), sub(p[i2], p[i0]));
    let base_len = dot(base, base).sqrt();
    let off_plane = |q: [f64; 3]| dot(base, sub(q, p[i0])).abs() / base_len;
    let i3 = (0..p.len()).max_by(|&a, &b| off_plane(p[a]).total_cmp(&off_plane(p[b])))?;
    if off_plane(p[i3]) <= eps {
        return None;
    }
    let centroid = {
        let mut c = [0f64; 3];
        for &i in &[i0, i1, i2, i3] {
            for k in 0..3 {
                c[k] += p[i][k] / 4.0;
            }
        }
        c
    };
    let mut faces: Vec<Face> = Vec::new();
    for tri in [[i0, i1, i2], [i0, i1, i3], [i0, i2, i3], [i1, i2, i3]] {
        faces.push(Face::new(&p, tri, centroid)?);
    }

    // The rest in a fixed pseudo-random order (sorted input is the incremental builder's worst case: every point would see most
    // of the hull).
    let mut order: Vec<usize> = (0..p.len())
        .filter(|q| ![i0, i1, i2, i3].contains(q))
        .collect();
    let mut state = 0x9e37_79b9_7f4a_7c15u64 ^ p.len() as u64;
    for i in (1..order.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        order.swap(i, (state % (i as u64 + 1)) as usize);
    }
    let mut live = faces.len();
    for q in order {
        let spent = faces.len() as u64;
        if *work < spent {
            *work = 0;
            return None;
        }
        *work -= spent;
        let point = p[q];
        // The faces the point sees, from the one it sees most through the faces that share an edge with it and see it too (the
        // visible faces of a convex solid are one connected patch; rounding must not make them two).
        let mut seed: Option<(usize, f64)> = None;
        let mut sees: HashSet<usize> = HashSet::new();
        for (f, face) in faces.iter().enumerate() {
            if !face.alive {
                continue;
            }
            let d = face.dist(point);
            if d > eps {
                sees.insert(f);
                if seed.is_none_or(|(_, best)| d > best) {
                    seed = Some((f, d));
                }
            }
        }
        let Some((seed, _)) = seed else { continue };
        let mut owner: HashMap<(usize, usize), usize> = HashMap::new();
        for &f in &sees {
            let v = faces[f].v;
            for k in 0..3 {
                owner.insert((v[k], v[(k + 1) % 3]), f);
            }
        }
        let mut visible = vec![seed];
        let mut taken: HashSet<usize> = HashSet::from([seed]);
        let mut next = 0;
        while next < visible.len() {
            let v = faces[visible[next]].v;
            next += 1;
            for k in 0..3 {
                if let Some(&n) = owner.get(&(v[(k + 1) % 3], v[k]))
                    && taken.insert(n)
                {
                    visible.push(n);
                }
            }
        }
        let mut edges: HashSet<(usize, usize)> = HashSet::new();
        for &f in &visible {
            let v = faces[f].v;
            for k in 0..3 {
                edges.insert((v[k], v[(k + 1) % 3]));
            }
        }
        let mut horizon: Vec<(usize, usize)> = edges
            .iter()
            .copied()
            .filter(|&(a, b)| !edges.contains(&(b, a)))
            .collect();
        horizon.sort_unstable();
        for &f in &visible {
            faces[f].alive = false;
        }
        live -= visible.len();
        for (a, b) in horizon {
            // The simplex's centroid is inside every hull grown from it.
            faces.push(Face::new(&p, [a, b, q], centroid)?);
            live += 1;
        }
        // Dead faces are dropped now and then so that the scan stays near the live count.
        if faces.len() > 64 && faces.len() > 4 * live {
            faces.retain(|f| f.alive);
        }
    }

    // Compact the corners and merge the planes.
    let live: Vec<&Face> = faces.iter().filter(|f| f.alive).collect();
    if live.len() < 4 {
        return None;
    }
    let mut remap: HashMap<usize, u32> = HashMap::new();
    let mut out_points: Vec<[f32; 3]> = Vec::new();
    let mut triangles = Vec::with_capacity(live.len());
    let mut triangle_planes = Vec::with_capacity(live.len());
    // Faces on one plane (their normals and offsets agree to a part in a million) share it; its normal is the sum of their
    // area-weighted normals (a thin triangle's own normal is the noisiest) and it passes through the group's outermost corner.
    let mut groups: HashMap<[i64; 4], usize> = HashMap::new();
    let mut sums: Vec<[f64; 3]> = Vec::new();
    let mut members: Vec<Vec<usize>> = Vec::new();
    let step = 1e-6 * extent;
    for f in live {
        let mut t = [0u32; 3];
        for k in 0..3 {
            let idx = *remap.entry(f.v[k]).or_insert_with(|| {
                out_points.push(p[f.v[k]].map(|v| v as f32));
                (out_points.len() - 1) as u32
            });
            t[k] = idx;
        }
        triangles.push(t);
        let key = [
            (f.n[0] * 1e6).round() as i64,
            (f.n[1] * 1e6).round() as i64,
            (f.n[2] * 1e6).round() as i64,
            (f.d / step).round() as i64,
        ];
        let g = *groups.entry(key).or_insert_with(|| {
            sums.push([0.0; 3]);
            members.push(Vec::new());
            sums.len() - 1
        });
        let (a, b, c) = (p[f.v[0]], p[f.v[1]], p[f.v[2]]);
        let area = cross(sub(b, a), sub(c, a));
        for k in 0..3 {
            sums[g][k] += area[k];
        }
        members[g].extend_from_slice(&f.v);
        triangle_planes.push(g as u32);
    }
    let mut planes = Vec::with_capacity(sums.len());
    for (g, sum) in sums.iter().enumerate() {
        let len = dot(*sum, *sum).sqrt();
        if len <= 0.0 || len.is_nan() {
            return None;
        }
        let n = [sum[0] / len, sum[1] / len, sum[2] / len];
        let far = members[g]
            .iter()
            .map(|&v| dot(n, p[v]))
            .fold(f64::NEG_INFINITY, f64::max);
        planes.push([n[0] as f32, n[1] as f32, n[2] as f32, (-far) as f32]);
    }
    Some(Hull {
        points: out_points,
        triangles,
        triangle_planes,
        planes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Vec<[f32; 3]> {
        Hull::of_box([-1.0; 3], [1.0; 3]).points
    }

    #[test]
    fn a_cube_is_six_planes() {
        let mut work = 1 << 20;
        let mut pts = cube();
        pts.push([0.0, 0.0, 0.0]);
        pts.push([0.5, -0.5, 0.25]);
        let h = convex_hull(&pts, &mut work).unwrap();
        assert_eq!(h.points.len(), 8);
        assert_eq!(h.planes.len(), 6);
        assert_eq!(h.triangles.len(), 12);
        assert_eq!(h.edges().len(), 12);
        for p in &pts {
            assert!(h.contains(*p, 1e-5));
        }
        assert!(!h.contains([1.5, 0.0, 0.0], 1e-5));
    }

    #[test]
    fn a_box_hull_matches_the_builder() {
        let b = Hull::of_box([0.0, 1.0, 2.0], [3.0, 5.0, 4.0]);
        for p in &b.points {
            assert!(b.contains(*p, 1e-5));
        }
        let mut work = 1 << 20;
        let h = convex_hull(&b.points, &mut work).unwrap();
        assert_eq!(h.planes.len(), 6);
        assert_eq!(h.bounds(), b.bounds());
        assert!(b.contains([1.0, 2.0, 3.0], 0.0));
        assert!(!b.contains([1.0, 0.5, 3.0], 0.0));
    }

    #[test]
    fn flat_and_thin_sets_have_no_hull() {
        let mut work = 1 << 20;
        assert!(convex_hull(&[[0.0; 3]; 10], &mut work).is_none());
        let flat = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.5, 0.5, 0.0],
        ];
        assert!(convex_hull(&flat, &mut work).is_none());
        let line = [[0.0; 3], [1.0, 1.0, 1.0], [2.0, 2.0, 2.0], [3.0, 3.0, 3.0]];
        assert!(convex_hull(&line, &mut work).is_none());
        assert!(convex_hull(&[[f32::NAN, 0.0, 0.0]; 5], &mut work).is_none());
    }

    #[test]
    fn a_sphere_of_points_holds_every_point() {
        let mut pts = Vec::new();
        for i in 0..12 {
            for j in 1..8 {
                let a = i as f32 * std::f32::consts::TAU / 12.0;
                let b = j as f32 * std::f32::consts::PI / 8.0;
                pts.push([
                    b.sin() * a.cos() * 10.0,
                    b.sin() * a.sin() * 10.0,
                    b.cos() * 10.0,
                ]);
            }
        }
        pts.push([0.0, 0.0, 10.0]);
        pts.push([0.0, 0.0, -10.0]);
        let mut work = 1 << 24;
        let h = convex_hull(&pts, &mut work).unwrap();
        for p in &pts {
            assert!(h.contains(*p, 1e-4), "{p:?}");
        }
        // Every corner is a point of the set, and a convex solid has more faces than corners / 2.
        assert!(h.points.iter().all(|q| pts.contains(q)));
        assert!(h.triangles.len() >= h.points.len() * 2 - 4);
    }

    #[test]
    fn the_work_budget_stops_the_builder() {
        let pts: Vec<[f32; 3]> = (0..200)
            .map(|i| {
                let t = i as f32;
                [t.sin() * 5.0, t.cos() * 5.0, (t * 0.37).sin() * 5.0]
            })
            .collect();
        let mut tiny = 10;
        assert!(convex_hull(&pts, &mut tiny).is_none());
        let mut plenty = 1 << 24;
        assert!(convex_hull(&pts, &mut plenty).is_some());
    }

    #[test]
    fn mapping_keeps_the_planes_true() {
        let h = Hull::of_box([0.0; 3], [1.0, 2.0, 3.0]);
        let m = h.mapped(&|p| [p[0], p[2], -p[1]]);
        for p in &m.points {
            assert!(m.contains(*p, 1e-5));
        }
        assert!(m.contains([0.5, 1.5, -1.0], 0.0));
        assert!(!m.contains([0.5, 3.5, -1.0], 0.0));
    }
}
