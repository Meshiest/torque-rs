//! A model's convex collision: the hulls of a [`torque_mapcore::collision::ConvexSet`] (a `.dif`'s first detail
//! level's), the same convexes for the movers and for rays.
//!
//! **A hull** is its points and its planes ([`torque_mapcore::collision::HullRange`]: the ranges as the file names them, every index
//! checked here; a flipped plane reference negates its plane). Interior files' planes face outward (every point behind them); a
//! plane with every point in front is turned round, one with points on both sides beyond the tolerance is dropped. Each plane
//! that holds three or more of the points is a face: those points in order round the face, clockwise seen from outside (the
//! order the polygon lists take). The hulls' emit strings and the BSP are not used:
//! the rays clip the segment by each hull's planes, so that a ray meets what a mover collides with.
//!
//! **Placement** ([`InteriorShape`]): a point is scaled, turned and moved (`M * (S * p)`); a world segment comes into the
//! interior's space by the affine inverse and the reciprocal scale (the fraction along it is the same), a plane goes out by
//! `R * (n / s)` normalised. A hull's bins (a grid over the interior's x and y) find the hulls a box or a segment meets.

use std::sync::Arc;

use torque_mapcore::collision::ConvexSet;

use crate::map::terrain::MapHit;
use crate::math::{Box3, F3, Matrix, Plane};
use crate::{Exhausted, work};

/// The most hulls one query looks at (cap).
pub const MAP_QUERY_HULLS: usize = 16_384;
/// The most bins on a side of an interior's grid.
pub const MAX_BINS: usize = 64;
/// The most bin entries an interior's grid holds (a hull that would take more goes in the wide list).
pub const MAX_BIN_ENTRIES: usize = 1 << 20;

/// One face of a hull: its outward plane and its points (indices into the hull's), clockwise seen from outside.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    pub plane: [f64; 4],
    pub verts: Vec<u32>,
}

/// One convex hull in the interior's space.
#[derive(Debug, Clone, PartialEq)]
pub struct Hull {
    pub points: Vec<F3>,
    /// The same corners in `f64` (the planes' own intersections).
    pub corners: Vec<[f64; 3]>,
    /// Outward planes `(n, d)`: inside where `n . p + d <= 0`.
    pub planes: Vec<[f64; 4]>,
    pub faces: Vec<Face>,
    pub bounds: Box3,
}

/// The hulls of one `.dif` (shared by every instance of the file).
#[derive(Debug, Clone, PartialEq)]
pub struct InteriorGeom {
    pub hulls: Vec<Hull>,
    pub bounds: Box3,
    bins: usize,
    cell: [f64; 2],
    grid: Vec<Vec<u32>>,
    wide: Vec<u32>,
}

/// What making a file's hulls costs (work units), for the caller's budget: every hull's [`hull_cost`] and the bins'
/// ([`bins_cost`]).
pub fn build_cost(i: &ConvexSet) -> u64 {
    (0..i.hulls.len())
        .fold(0u64, |c, k| c.saturating_add(hull_cost(i, k)))
        .saturating_add(bins_cost(i.hulls.len()))
}

/// The plane references hull `k` names (as the file names them, not held to the list).
fn plane_range(i: &ConvexSet, k: usize) -> (usize, usize) {
    i.hulls
        .get(k)
        .map_or((0, 0), |h| (h.plane_start as usize, h.plane_end as usize))
}

/// What making hull `k` of `i` ([`hull_of`]) costs at most: a unit for each plane index it names (counted whole, however many
/// there are: a hull naming too many is refused only once they are counted), each point gathered and measured against each plane
/// kept, and for the polytope each plane triple times the planes (the polytope's planes held to [`MAX_HULL_PLANES`]).
pub fn hull_cost(i: &ConvexSet, k: usize) -> u64 {
    let Some(hull) = i.hulls.get(k) else {
        return 0;
    };
    let (start, end) = plane_range(i, k);
    let named = end.saturating_sub(start) as u64;
    let p = (named + 6).min(MAX_HULL_PLANES as u64 + 1);
    let points = u64::from(hull.point_count);
    named
        .saturating_add(points.saturating_mul(p + 1))
        .saturating_add(p * p * p * p / 6 + 16)
}

/// What making the bins of `hulls` hulls costs at most ([`InteriorGeom::from_hulls`]): the grid, each hull's box and the
/// entries.
pub fn bins_cost(hulls: usize) -> u64 {
    let bins = ((hulls as f64 / 4.0).sqrt().ceil() as u64).clamp(1, MAX_BINS as u64);
    let hulls = hulls as u64;
    (bins * bins)
        .saturating_add(hulls.saturating_mul(16))
        .saturating_add(
            hulls
                .saturating_mul(bins * bins / 2 + 1)
                .min(MAX_BIN_ENTRIES as u64),
        )
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn wide(p: F3) -> [f64; 3] {
    p.map(f64::from)
}

fn box_of(points: &[F3]) -> Box3 {
    let mut b = Box3::new([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for p in points {
        b.min = [0, 1, 2].map(|k| b.min[k].min(p[k]));
        b.max = [0, 1, 2].map(|k| b.max[k].max(p[k]));
    }
    b
}

/// The most planes a hull's polytope is made from (its own and the six of its box; cap: the work is cubic in them).
pub const MAX_HULL_PLANES: usize = 96;

/// One hull of `i` (module docs); None when an index is out of range or the hull has too many planes. A hull whose plane
/// indices and the six of its box are more than [`MAX_HULL_PLANES`] is refused before any of its work.
pub fn hull_of(i: &ConvexSet, k: usize) -> Option<Hull> {
    let hull = i.hulls.get(k)?;
    let (plane_start, plane_end) = plane_range(i, k);
    let plane_end = plane_end.min(i.plane_refs.len());
    if plane_end.saturating_sub(plane_start) + 6 > MAX_HULL_PLANES {
        return None;
    }
    let start = hull.point_start as usize;
    let points: Vec<F3> = (start..start + hull.point_count as usize)
        .map(|j| {
            i.point_refs
                .get(j)
                .and_then(|&p| i.points.get(p as usize))
                .copied()
        })
        .collect::<Option<_>>()?;
    if points.is_empty() {
        return None;
    }
    let bounds = box_of(&points);
    let extent = (0..3)
        .map(|c| f64::from(bounds.max[c] - bounds.min[c]))
        .fold(0.0, f64::max)
        .max(1e-3);
    // The files' points lie off their planes by up to a few tenths of a millimetre (map2dif's rounding).
    let pick = 2e-3 * extent.max(1.0);
    let wp: Vec<[f64; 3]> = points.iter().map(|&p| wide(p)).collect();
    let mut planes: Vec<[f64; 4]> = Vec::new();
    let push = |planes: &mut Vec<[f64; 4]>, n: [f64; 3], dd: f64| {
        if !planes.iter().any(|q| {
            (dot([q[0], q[1], q[2]], n) - 1.0).abs() < 1e-9 && (q[3] - dd).abs() < 1e-9 * extent
        }) {
            planes.push([n[0], n[1], n[2], dd]);
        }
    };
    for r in i.plane_refs.get(plane_start..plane_end).unwrap_or(&[]) {
        if planes.len() > MAX_HULL_PLANES {
            return None;
        }
        let Some(p) = i.planes.get(r.index as usize) else {
            continue;
        };
        let mut n = wide(p.normal);
        let mut dd = f64::from(p.d);
        if r.flipped {
            n = n.map(|v| -v);
            dd = -dd;
        }
        let len = dot(n, n).sqrt();
        if !(len > 1e-12) || !len.is_finite() || !dd.is_finite() {
            continue;
        }
        n = n.map(|v| v / len);
        dd /= len;
        let (lo, hi) = wp
            .iter()
            .map(|&q| dot(n, q) + dd)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), x| {
                (l.min(x), h.max(x))
            });
        if hi > pick && lo < -pick {
            continue;
        }
        if hi > pick {
            n = n.map(|v| -v);
            dd = -dd;
        }
        push(&mut planes, n, dd);
    }
    // The box closes a hull whose planes leave it open (it adds no face to one they close).
    for c in 0..3 {
        let mut n = [0.0; 3];
        n[c] = 1.0;
        push(&mut planes, n, -(f64::from(bounds.max[c]) + pick));
        n[c] = -1.0;
        push(&mut planes, n, f64::from(bounds.min[c]) - pick);
    }
    if planes.len() > MAX_HULL_PLANES {
        return None;
    }
    let (verts, faces) = polytope(&planes, extent);
    if faces.is_empty() {
        return None;
    }
    let points: Vec<F3> = verts.iter().map(|v| v.map(|x| x as f32)).collect();
    let bounds = box_of(&points);
    Some(Hull {
        points,
        corners: verts,
        planes: faces.iter().map(|f| f.plane).collect(),
        faces,
        bounds,
    })
}

/// The polytope `planes` bound (inside where every `n . p + d <= 0`): its corners (each plane triple's point inside all the
/// others; corners nearer than a hair are one) and its faces (each plane's corners, those of the triples it is in, when three or
/// more not in one line are left), clockwise seen from outside.
fn polytope(planes: &[[f64; 4]], extent: f64) -> (Vec<[f64; 3]>, Vec<Face>) {
    let eps = 1e-7 * extent.max(1.0);
    let mut verts: Vec<[f64; 3]> = Vec::new();
    let mut members: Vec<Vec<usize>> = Vec::new();
    let n = planes.len();
    for a in 0..n {
        for b in a + 1..n {
            for c in b + 1..n {
                let Some(p) = meet(planes[a], planes[b], planes[c]) else {
                    continue;
                };
                if !planes
                    .iter()
                    .all(|q| dot([q[0], q[1], q[2]], p) + q[3] <= eps)
                {
                    continue;
                }
                let at = verts.iter().position(|v| {
                    let d = sub(*v, p);
                    dot(d, d) <= eps * eps
                });
                let m = match at {
                    Some(i) => &mut members[i],
                    None => {
                        verts.push(p);
                        members.push(Vec::new());
                        members.last_mut().unwrap()
                    }
                };
                for k in [a, b, c] {
                    if !m.contains(&k) {
                        m.push(k);
                    }
                }
            }
        }
    }
    let mut faces = Vec::new();
    for (k, &plane) in planes.iter().enumerate() {
        let nrm = [plane[0], plane[1], plane[2]];
        let on: Vec<u32> = (0..verts.len() as u32)
            .filter(|&j| members[j as usize].contains(&k))
            .collect();
        if let Some(v) = ring(&verts, nrm, on, extent) {
            faces.push(Face { plane, verts: v });
        }
    }
    (verts, faces)
}

/// The point the three planes meet at (None when two are nearly parallel).
fn meet(p: [f64; 4], q: [f64; 4], r: [f64; 4]) -> Option<[f64; 3]> {
    let (a, b, c) = ([p[0], p[1], p[2]], [q[0], q[1], q[2]], [r[0], r[1], r[2]]);
    let bc = cross(b, c);
    let det = dot(a, bc);
    if det.abs() < 1e-9 {
        return None;
    }
    let ca = cross(c, a);
    let ab = cross(a, b);
    let x = [0, 1, 2].map(|k| (-p[3] * bc[k] - q[3] * ca[k] - r[3] * ab[k]) / det);
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// The corners `on` of a face with normal `n` in order round it, clockwise seen from its front, the ones in line with their
/// neighbours left out; None when fewer than three are left.
fn ring(verts: &[[f64; 3]], n: [f64; 3], mut on: Vec<u32>, extent: f64) -> Option<Vec<u32>> {
    if on.len() < 3 {
        return None;
    }
    let c = on.iter().fold([0.0; 3], |a, &j| {
        let q = verts[j as usize];
        [a[0] + q[0], a[1] + q[1], a[2] + q[2]]
    });
    let c = c.map(|v| v / on.len() as f64);
    let axis = if n[0].abs() < 0.6 {
        [1.0, 0.0, 0.0]
    } else if n[1].abs() < 0.6 {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let u = cross(axis, n);
    let v = cross(n, u);
    let angle = |j: u32| {
        let d = sub(verts[j as usize], c);
        dot(d, v).atan2(dot(d, u))
    };
    on.sort_by(|&a, &b| angle(a).total_cmp(&angle(b)).then(a.cmp(&b)));
    // Counter-clockwise round n now: the corners in line with their neighbours are dropped.
    let area = 1e-12 * extent.max(1.0) * extent.max(1.0);
    let mut dropped = true;
    while dropped && on.len() >= 3 {
        dropped = false;
        for k in 0..on.len() {
            let a = verts[on[(k + on.len() - 1) % on.len()] as usize];
            let b = verts[on[k] as usize];
            let d = verts[on[(k + 1) % on.len()] as usize];
            if dot(cross(sub(b, a), sub(d, b)), n) <= area {
                on.remove(k);
                dropped = true;
                break;
            }
        }
    }
    if on.len() < 3 {
        return None;
    }
    on.reverse();
    Some(on)
}

impl InteriorGeom {
    /// The hulls of `i` and their bins. Every index is checked (a hull that names a point that is not there is left out).
    pub fn new(i: &ConvexSet) -> InteriorGeom {
        InteriorGeom::from_hulls((0..i.hulls.len()).filter_map(|k| hull_of(i, k)).collect())
    }

    /// Hull `k` of `g` alone (for checks).
    pub fn single(g: &InteriorGeom, k: usize) -> InteriorGeom {
        InteriorGeom::from_hulls(g.hulls.get(k).cloned().into_iter().collect())
    }

    /// The bins of `hulls`.
    pub fn from_hulls(hulls: Vec<Hull>) -> InteriorGeom {
        let mut bounds = Box3::new([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for h in &hulls {
            for k in 0..3 {
                bounds.min[k] = bounds.min[k].min(h.bounds.min[k]);
                bounds.max[k] = bounds.max[k].max(h.bounds.max[k]);
            }
        }
        if hulls.is_empty() {
            bounds = Box3::default();
        }
        let bins = ((hulls.len() as f64 / 4.0).sqrt().ceil() as usize).clamp(1, MAX_BINS);
        let cell = [0, 1]
            .map(|k| (f64::from(bounds.max[k]) - f64::from(bounds.min[k])).max(1e-3) / bins as f64);
        let mut g = InteriorGeom {
            hulls,
            bounds,
            bins,
            cell,
            grid: vec![Vec::new(); bins * bins],
            wide: Vec::new(),
        };
        let mut entries = 0usize;
        for (k, h) in g.hulls.iter().enumerate() {
            let Some(([x0, x1], [y0, y1])) = g.range(&h.bounds) else {
                g.wide.push(k as u32);
                continue;
            };
            let n = (x1 - x0 + 1) * (y1 - y0 + 1);
            if n > (bins * bins) / 2 + 1 || entries + n > MAX_BIN_ENTRIES {
                g.wide.push(k as u32);
                continue;
            }
            entries += n;
            for y in y0..=y1 {
                for x in x0..=x1 {
                    g.grid[x + y * bins].push(k as u32);
                }
            }
        }
        g
    }

    /// The bins a local box covers, held to the grid; None for a box that is not finite.
    fn range(&self, b: &Box3) -> Option<([usize; 2], [usize; 2])> {
        let mut out = [[0usize; 2]; 2];
        for (k, out) in out.iter_mut().enumerate() {
            let lo = (f64::from(b.min[k]) - f64::from(self.bounds.min[k])) / self.cell[k];
            let hi = (f64::from(b.max[k]) - f64::from(self.bounds.min[k])) / self.cell[k];
            if !lo.is_finite() || !hi.is_finite() {
                return None;
            }
            let top = (self.bins - 1) as f64;
            *out = [
                lo.floor().clamp(0.0, top) as usize,
                hi.floor().clamp(0.0, top) as usize,
            ];
        }
        Some(([out[0][0], out[0][1]], [out[1][0], out[1][1]]))
    }

    /// The hulls whose bounds meet the local box `b`, ascending, each once; at most [`MAP_QUERY_HULLS`] bin entries looked at.
    pub fn hulls_in(
        &self,
        b: &Box3,
        charge: &mut dyn FnMut(u64) -> bool,
        out: &mut Vec<u32>,
    ) -> Result<(), Exhausted> {
        out.clear();
        if self.hulls.is_empty() || !b.overlaps(&self.bounds) {
            return Ok(());
        }
        let Some(([x0, x1], [y0, y1])) = self.range(b) else {
            return Ok(());
        };
        let mut looked = 0usize;
        let cells = (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y)));
        for list in cells
            .map(|(x, y)| &self.grid[x + y * self.bins])
            .chain(std::iter::once(&self.wide))
        {
            if !charge(work::CANDIDATE) {
                return Err(Exhausted);
            }
            for &k in list {
                looked += 1;
                if looked > MAP_QUERY_HULLS {
                    break;
                }
                if self.hulls[k as usize].bounds.overlaps(b) {
                    out.push(k);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        Ok(())
    }
}

/// The segment `s + t * dir` (t in 0..1) against `hull` (local): the entry fraction and the entry plane's normal; None for a
/// miss or a segment that starts inside (or on) the hull.
pub fn ray_hull(hull: &Hull, s: [f64; 3], dir: [f64; 3]) -> Option<(f64, [f64; 3])> {
    ray_planes(hull.planes.iter().copied(), s, dir)
}

/// [`ray_hull`] against the convex region `planes` bound.
pub fn ray_planes(
    planes: impl Iterator<Item = [f64; 4]>,
    s: [f64; 3],
    dir: [f64; 3],
) -> Option<(f64, [f64; 3])> {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let mut entry: Option<[f64; 3]> = None;
    for p in planes {
        let n = [p[0], p[1], p[2]];
        let a = dot(n, s) + p[3];
        let b = a + dot(n, dir);
        if a > 0.0 && b > 0.0 {
            return None;
        }
        if a <= 0.0 && b <= 0.0 {
            continue;
        }
        let t = a / (a - b);
        if a > 0.0 {
            if entry.is_none() || t > t0 {
                t0 = t;
                entry = Some(n);
            }
        } else if t < t1 {
            t1 = t;
        }
        if t0 > t1 {
            return None;
        }
    }
    entry.map(|n| (t0, n))
}

/// One placed interior: `p -> L p + t` with `L` the rotation times the scale (in `f64`), and `L`'s inverse transpose for planes.
#[derive(Debug, Clone, PartialEq)]
pub struct InteriorShape {
    geom: Arc<InteriorGeom>,
    linear: [[f64; 3]; 3],
    /// `L`'s inverse, transposed (a plane's normal goes out by it).
    inverse_t: [[f64; 3]; 3],
    offset: [f64; 3],
    mirrored: bool,
    world_box: Box3,
}

fn mat3_vec(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

impl InteriorShape {
    /// `geom` at `to_world` (rotation and translation) and `scale`; None for a scale with a zero, tiny or not finite part, or a
    /// transform that is not finite or not invertible.
    pub fn new(geom: Arc<InteriorGeom>, to_world: Matrix, scale: F3) -> Option<InteriorShape> {
        if !scale.iter().all(|s| s.is_finite() && s.abs() >= 1e-4)
            || !to_world.iter().all(|v| v.is_finite())
        {
            return None;
        }
        let m = |r: usize, c: usize| f64::from(to_world[r * 4 + c]) * f64::from(scale[c]);
        let linear = [0, 1, 2].map(|r| [m(r, 0), m(r, 1), m(r, 2)]);
        let [a, b, c] = linear;
        // The inverse transpose is the cofactor matrix over the determinant.
        let cof = [cross(b, c), cross(c, a), cross(a, b)];
        let det = dot(a, cof[0]);
        if !(det.abs() > 1e-18) || !det.is_finite() {
            return None;
        }
        // With rows a, b, c, L's inverse has the columns (b x c, c x a, a x b) / det: they are the rows of its transpose.
        let inverse_t = cof.map(|row| row.map(|v| v / det));
        let mut s = InteriorShape {
            geom,
            linear,
            inverse_t,
            offset: [to_world[3], to_world[7], to_world[11]].map(f64::from),
            mirrored: det < 0.0,
            world_box: Box3::default(),
        };
        s.world_box = s.box_to_world(&s.geom.bounds);
        Some(s)
    }

    pub fn geom(&self) -> &InteriorGeom {
        &self.geom
    }

    pub fn world_box(&self) -> Box3 {
        self.world_box
    }

    /// A local point in the world.
    pub fn point(&self, p: F3) -> F3 {
        self.point_wide(p).map(|v| v as f32)
    }

    /// A local point in the world, in `f64`.
    pub fn point_wide(&self, p: F3) -> [f64; 3] {
        self.corner(wide(p))
    }

    fn corner(&self, p: [f64; 3]) -> [f64; 3] {
        let q = mat3_vec(&self.linear, p);
        [0, 1, 2].map(|k| q[k] + self.offset[k])
    }

    /// A hull's points in the world, in `f64`.
    pub fn hull_points_wide(&self, k: u32) -> Vec<[f64; 3]> {
        self.geom.hulls.get(k as usize).map_or_else(Vec::new, |h| {
            h.corners.iter().map(|&p| self.corner(p)).collect()
        })
    }

    /// A world point in the interior's space (in `f64`): `L^-1 (p - t)`, `L^-1` being the inverse transpose's transpose.
    fn local(&self, p: [f64; 3]) -> [f64; 3] {
        let d = sub(p, self.offset);
        let m = &self.inverse_t;
        [0, 1, 2].map(|c| m[0][c] * d[0] + m[1][c] * d[1] + m[2][c] * d[2])
    }

    /// A local outward plane in the world (in `f64`, normalised): `n' = L^-T n`, `d' = d - n' . t`.
    pub fn plane_wide(&self, p: [f64; 4]) -> [f64; 4] {
        let n = mat3_vec(&self.inverse_t, [p[0], p[1], p[2]]);
        let dd = p[3] - dot(n, self.offset);
        let len = dot(n, n).sqrt().max(1e-300);
        [n[0] / len, n[1] / len, n[2] / len, dd / len]
    }

    /// A local outward plane in the world as a float plane.
    pub fn plane(&self, p: [f64; 4]) -> Plane {
        let w = self.plane_wide(p);
        Plane::new(w[0] as f32, w[1] as f32, w[2] as f32, w[3] as f32)
    }

    /// A local box's world box (its eight corners moved).
    fn box_to_world(&self, b: &Box3) -> Box3 {
        let corners: Vec<F3> = (0..8)
            .map(|c| {
                self.point([
                    if c & 1 != 0 { b.max[0] } else { b.min[0] },
                    if c & 2 != 0 { b.max[1] } else { b.min[1] },
                    if c & 4 != 0 { b.max[2] } else { b.min[2] },
                ])
            })
            .collect();
        box_of(&corners)
    }

    /// A world box's local box, grown by a hair (the corners' rounding).
    fn box_to_local(&self, b: &Box3) -> Box3 {
        let corners: Vec<F3> = (0..8)
            .map(|c| {
                self.local(
                    [
                        if c & 1 != 0 { b.max[0] } else { b.min[0] },
                        if c & 2 != 0 { b.max[1] } else { b.min[1] },
                        if c & 4 != 0 { b.max[2] } else { b.min[2] },
                    ]
                    .map(f64::from),
                )
                .map(|v| v as f32)
            })
            .collect();
        let mut out = box_of(&corners);
        for k in 0..3 {
            let pad = 1e-4 * (1.0 + out.min[k].abs().max(out.max[k].abs()));
            out.min[k] -= pad;
            out.max[k] += pad;
        }
        out
    }

    /// The hulls whose world boxes meet `query` (ascending).
    pub fn pieces(
        &self,
        query: &Box3,
        charge: &mut dyn FnMut(u64) -> bool,
        out: &mut Vec<u32>,
    ) -> Result<(), Exhausted> {
        out.clear();
        if !query.overlaps(&self.world_box) {
            return Ok(());
        }
        let local = self.box_to_local(query);
        self.geom.hulls_in(&local, charge, out)?;
        out.retain(|&k| self.hull_box(k).is_some_and(|b| b.overlaps(query)));
        Ok(())
    }

    /// A hull's points in the world.
    pub fn hull_points(&self, k: u32) -> Vec<F3> {
        self.geom.hulls.get(k as usize).map_or_else(Vec::new, |h| {
            h.points.iter().map(|&p| self.point(p)).collect()
        })
    }

    /// A hull's world box.
    pub fn hull_box(&self, k: u32) -> Option<Box3> {
        let pts = self.hull_points(k);
        (!pts.is_empty()).then(|| box_of(&pts))
    }

    /// A hull's faces in the world: the plane and the point indices, clockwise seen from outside (a mirrored scale turns them).
    pub fn hull_faces(&self, k: u32) -> Vec<(Plane, Vec<u32>)> {
        let Some(h) = self.geom.hulls.get(k as usize) else {
            return Vec::new();
        };
        h.faces
            .iter()
            .map(|f| {
                let mut v = f.verts.clone();
                if self.mirrored {
                    v.reverse();
                }
                (self.plane(f.plane), v)
            })
            .collect()
    }

    /// The segment against the interior's hulls, in the world (each hull's planes moved out): the nearest entry (module docs).
    /// Charged a candidate a bin, a polygon a hull.
    pub fn cast_ray(
        &self,
        start: F3,
        end: F3,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> Result<Option<MapHit>, Exhausted> {
        if !start.iter().chain(&end).all(|v| v.is_finite()) {
            return Ok(None);
        }
        let lo = [0, 1, 2].map(|k| start[k].min(end[k]));
        let hi = [0, 1, 2].map(|k| start[k].max(end[k]));
        let wb = Box3::new(lo, hi);
        if !wb.overlaps(&self.world_box) {
            return Ok(None);
        }
        let mut list = Vec::new();
        self.geom
            .hulls_in(&self.box_to_local(&wb), charge, &mut list)?;
        let s = wide(start);
        let dir = sub(wide(end), s);
        let mut best: Option<(f64, [f64; 3])> = None;
        for &k in &list {
            if !charge(work::POLY) {
                return Err(Exhausted);
            }
            let h = &self.geom.hulls[k as usize];
            let planes = h.planes.iter().map(|&p| self.plane_wide(p));
            if let Some((t, n)) = ray_planes(planes, s, dir)
                && best.is_none_or(|(bt, _)| t < bt)
            {
                best = Some((t, n));
            }
        }
        Ok(best.map(|(t, n)| (t.clamp(0.0, 1.0) as f32, n.map(|v| v as f32))))
    }
}

#[cfg(test)]
#[path = "interior_tests.rs"]
mod tests;
