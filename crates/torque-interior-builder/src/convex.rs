//! The convex hull of a set of points (incremental, `f64`): the planes of its faces, each facing out.
//!
//! Points within [`EPS`] of each other are one; a point closer than [`ON_FACE`] to a face already made counts as on it, and
//! faces whose corners lie within it of one plane are one face; a set that is flat or a line within [`EPS`] has no hull.

use std::collections::HashMap;

use crate::geom::{self, EPS, Plane, V3};

/// How near a face's plane a point counts as on it: well under [`EPS`], so a hull is within it of its points, well over
/// the rounding of `f32` coordinates a few hundred units out.
pub const ON_FACE: f64 = 1e-5;

/// The most points a hull is made of (after equal ones are merged): the work grows with their square.
pub const MAX_POINTS: usize = 4_096;

/// Why a set of points has no hull.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HullFail {
    /// Fewer than four points off one plane.
    Flat,
    /// More than [`MAX_POINTS`] points.
    TooMany(usize),
    /// A point that is not finite.
    NotFinite,
}

/// A triangle of the hull, counter-clockwise seen from outside, and its plane.
struct Tri {
    v: [usize; 3],
    plane: Plane,
    alive: bool,
}

fn tri(points: &[V3], v: [usize; 3]) -> Option<Tri> {
    let [a, b, c] = v.map(|i| points[i]);
    Some(Tri {
        v,
        plane: Plane::new(geom::cross(geom::sub(b, a), geom::sub(c, a)), a)?,
        alive: true,
    })
}

/// The points with ones within [`EPS`] of an earlier one left out.
fn distinct(points: &[V3]) -> Vec<V3> {
    let cell = |p: V3| p.map(|v| (v / (EPS * 4.0)).floor() as i64);
    let mut cells: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    let mut out: Vec<V3> = Vec::new();
    'next: for &p in points {
        let c = cell(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &i in cells
                        .get(&[c[0] + dx, c[1] + dy, c[2] + dz])
                        .map_or(&[][..], Vec::as_slice)
                    {
                        if geom::length(geom::sub(out[i], p)) < EPS {
                            continue 'next;
                        }
                    }
                }
            }
        }
        cells.entry(c).or_default().push(out.len());
        out.push(p);
    }
    out
}

/// The planes of the faces of the convex hull of `points` (coplanar faces once), each facing out.
pub fn hull_planes(points: &[V3]) -> Result<Vec<Plane>, HullFail> {
    if points.iter().flatten().any(|v| !v.is_finite()) {
        return Err(HullFail::NotFinite);
    }
    let pts = distinct(points);
    if pts.len() > MAX_POINTS {
        return Err(HullFail::TooMany(pts.len()));
    }
    if pts.len() < 4 {
        return Err(HullFail::Flat);
    }
    // A first tetrahedron of points far apart.
    let far = |from: &dyn Fn(V3) -> f64| {
        (0..pts.len())
            .max_by(|&a, &b| from(pts[a]).total_cmp(&from(pts[b])))
            .unwrap_or(0)
    };
    let i0 = far(&|p| -p[0]);
    let i1 = far(&|p| geom::length(geom::sub(p, pts[i0])));
    let d01 = geom::sub(pts[i1], pts[i0]);
    let line = |p: V3| {
        let l = geom::length(d01);
        if l < EPS {
            0.0
        } else {
            geom::length(geom::cross(geom::sub(p, pts[i0]), d01)) / l
        }
    };
    let i2 = far(&line);
    if line(pts[i2]) < EPS {
        return Err(HullFail::Flat);
    }
    let base =
        Plane::new(geom::cross(d01, geom::sub(pts[i2], pts[i0])), pts[i0]).ok_or(HullFail::Flat)?;
    let i3 = far(&|p| base.dist(p).abs());
    if base.dist(pts[i3]).abs() < EPS {
        return Err(HullFail::Flat);
    }
    // Faces turned so the fourth point is behind each.
    let (a, b, c, d) = if base.dist(pts[i3]) > 0.0 {
        (i0, i2, i1, i3)
    } else {
        (i0, i1, i2, i3)
    };
    let mut tris: Vec<Tri> = [[a, b, c], [a, d, b], [b, d, c], [c, d, a]]
        .into_iter()
        .filter_map(|v| tri(&pts, v))
        .collect();
    if tris.len() < 4 {
        return Err(HullFail::Flat);
    }
    // Each directed edge's triangle.
    let mut edges: HashMap<(usize, usize), usize> = HashMap::new();
    for (k, t) in tris.iter().enumerate() {
        for e in 0..3 {
            edges.insert((t.v[e], t.v[(e + 1) % 3]), k);
        }
    }
    for (pi, &p) in pts.iter().enumerate() {
        if [a, b, c, d].contains(&pi) {
            continue;
        }
        let visible: Vec<usize> = (0..tris.len())
            .filter(|&k| tris[k].alive && tris[k].plane.dist(p) > ON_FACE)
            .collect();
        if visible.is_empty() {
            continue;
        }
        // The horizon: edges of a visible triangle whose neighbor across them is not visible.
        let mut horizon = Vec::new();
        for &k in &visible {
            for e in 0..3 {
                let (u, w) = (tris[k].v[e], tris[k].v[(e + 1) % 3]);
                let across = edges.get(&(w, u)).copied();
                if across.is_none_or(|j| !visible.contains(&j)) {
                    horizon.push((u, w));
                }
            }
        }
        for &k in &visible {
            tris[k].alive = false;
            for e in 0..3 {
                let key = (tris[k].v[e], tris[k].v[(e + 1) % 3]);
                if edges.get(&key) == Some(&k) {
                    edges.remove(&key);
                }
            }
        }
        for (u, w) in horizon {
            let Some(t) = tri(&pts, [u, w, pi]) else {
                continue;
            };
            let k = tris.len();
            for e in 0..3 {
                edges.insert((t.v[e], t.v[(e + 1) % 3]), k);
            }
            tris.push(t);
        }
    }
    // Triangles whose corners all lie within ON_FACE of a face already found (facing its way) are on that face. Each face's
    // normal is then its triangles' area-weighted mean, moved out to the farthest point, so every point is behind it.
    let mut faces: Vec<(Plane, V3)> = Vec::new();
    for t in tris.iter().filter(|t| t.alive) {
        let [a, b, c] = t.v.map(|i| pts[i]);
        let area = geom::cross(geom::sub(b, a), geom::sub(c, a));
        let on = |q: &(Plane, V3)| {
            geom::dot(q.0.n, t.plane.n) > 0.9
                && t.v.iter().all(|&i| q.0.dist(pts[i]).abs() < ON_FACE)
        };
        match faces.iter_mut().find(|q| on(q)) {
            Some(f) => f.1 = geom::add(f.1, area),
            None => faces.push((t.plane, area)),
        }
    }
    let mut planes: Vec<Plane> = Vec::new();
    for (q, sum) in faces {
        let n = geom::normalize(sum).unwrap_or(q.n);
        let far = pts
            .iter()
            .map(|&p| geom::dot(n, p))
            .fold(f64::NEG_INFINITY, f64::max);
        let fitted = Plane { n, d: -far };
        // A face the rounding of nearly coplanar points made, whose fit lies off its own triangles, is not one of the hull's.
        if !planes.iter().any(|p| p.same(&fitted))
            && q.n.iter().zip(&n).all(|(x, y)| (x - y).abs() < 1e-3)
        {
            planes.push(fitted);
        }
    }
    Ok(planes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::{Brush, TexGen};

    #[test]
    fn a_cube_of_points_gives_six_planes() {
        let mut pts = Vec::new();
        for i in 0..27 {
            pts.push([(i % 3) as f64, ((i / 3) % 3) as f64, (i / 9) as f64]);
        }
        let planes = hull_planes(&pts).unwrap();
        assert_eq!(planes.len(), 6);
        let b = Brush::from_planes(&planes, "m", |n| TexGen::world(n, 1.0));
        assert_eq!(b.corners().unwrap().len(), 8);
        assert!(b.contains([1.0, 1.0, 1.0], 0.5));
    }

    #[test]
    fn every_point_is_on_or_behind_every_plane() {
        let mut seed = 11u64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        };
        let pts: Vec<V3> = (0..400).map(|_| [next(), next(), next() * 0.5]).collect();
        let planes = hull_planes(&pts).unwrap();
        assert!(planes.len() > 10);
        for p in &pts {
            assert!(planes.iter().all(|q| q.dist(*p) < 1e-6));
        }
        // Each plane holds at least three of the points.
        for q in &planes {
            assert!(pts.iter().filter(|p| q.dist(**p).abs() < 1e-6).count() >= 3);
        }
    }

    #[test]
    fn flat_and_hostile_sets_are_refused() {
        let flat: Vec<V3> = (0..10).map(|i| [i as f64, (i * i) as f64, 0.0]).collect();
        assert_eq!(hull_planes(&flat), Err(HullFail::Flat));
        assert_eq!(hull_planes(&[[0.0; 3]; 3]), Err(HullFail::Flat));
        assert_eq!(
            hull_planes(&[[f64::NAN, 0.0, 0.0], [1.0; 3], [2.0; 3], [3.0, 1.0, 0.0]]),
            Err(HullFail::NotFinite)
        );
        let many: Vec<V3> = (0..MAX_POINTS + 1)
            .map(|i| [i as f64, (i % 7) as f64, (i % 13) as f64])
            .collect();
        assert!(matches!(hull_planes(&many), Err(HullFail::TooMany(_))));
    }
}
