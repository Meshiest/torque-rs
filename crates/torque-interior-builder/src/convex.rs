//! The convex hull of a set of points (incremental, `f64`): the planes of its faces, each facing out.
//!
//! Points within [`EPS`] of each other are one; a point closer than [`ON_FACE`] to a face already made counts as on it, and
//! faces whose corners lie within it of one plane are one face; a set that is flat or a line within [`EPS`] has no hull.

use std::collections::HashMap;

use crate::geom::{self, Budget, EPS, Plane, V3};

/// How near a face's plane a point counts as on it: well under [`EPS`], so a hull is within it of its points, well over
/// the rounding of `f32` coordinates a few hundred units out.
pub const ON_FACE: f64 = 1e-5;

/// The most points a hull is made of (after equal ones are merged): the work grows with their square.
pub const MAX_POINTS: usize = 4_096;

/// The most faces of a hull (a brush has at most 255): merging triangles into faces grows with triangles times faces.
pub const MAX_FACES: usize = 1_024;

/// The most live triangles a point (and 64 more): a closed hull has fewer than two a point, a few more come of points within
/// [`ON_FACE`] of its faces.
const MAX_TRIANGLES_PER_POINT: usize = 4;

/// Why a set of points has no hull.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HullFail {
    /// Fewer than four points off one plane.
    Flat,
    /// More than [`MAX_POINTS`] points.
    TooMany(usize),
    /// More than [`MAX_FACES`] faces.
    Faces(usize),
    /// Far more triangles than a closed hull of the points has (`2n - 4`): points so near each other's faces that the
    /// triangles no longer close and multiply.
    Tangled,
    /// A point that is not finite.
    NotFinite,
    /// More work than the budget allows.
    Work,
}

/// A triangle of the hull, counter-clockwise seen from outside, and its plane.
struct Tri {
    v: [usize; 3],
    plane: Plane,
}

fn tri(points: &[V3], v: [usize; 3]) -> Option<Tri> {
    let [a, b, c] = v.map(|i| points[i]);
    Some(Tri {
        v,
        plane: Plane::new(geom::cross(geom::sub(b, a), geom::sub(c, a)), a)?,
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

/// The planes of the faces of the convex hull of `points` (coplanar faces once), each facing out. Each point charges `work`
/// the live triangles it is tested against and the visible ones it removes; merging the faces charges the faces tested.
pub fn hull_planes(points: &[V3], work: &mut Budget) -> Result<Vec<Plane>, HullFail> {
    if points.iter().flatten().any(|v| !v.is_finite()) {
        return Err(HullFail::NotFinite);
    }
    if !work.charge(points.len() as u64) {
        return Err(HullFail::Work);
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
    // Each directed edge's live triangle; the live triangles in the order they were made; slots of dead ones to reuse.
    let mut edges: HashMap<(usize, usize), usize> = HashMap::new();
    for (k, t) in tris.iter().enumerate() {
        for e in 0..3 {
            edges.insert((t.v[e], t.v[(e + 1) % 3]), k);
        }
    }
    let mut live: Vec<usize> = (0..tris.len()).collect();
    let mut free: Vec<usize> = Vec::new();
    // `seen[k] == pi + 1`: triangle k is visible from point pi.
    let mut seen: Vec<usize> = vec![0; tris.len()];
    for (pi, &p) in pts.iter().enumerate() {
        if [a, b, c, d].contains(&pi) {
            continue;
        }
        if !work.charge(live.len() as u64) {
            return Err(HullFail::Work);
        }
        let mark = pi + 1;
        let mut visible = Vec::new();
        live.retain(|&k| {
            let v = tris[k].plane.dist(p) > ON_FACE;
            if v {
                seen[k] = mark;
                visible.push(k);
            }
            !v
        });
        if visible.is_empty() {
            continue;
        }
        if !work.charge(visible.len() as u64 * 6) {
            return Err(HullFail::Work);
        }
        // The horizon: edges of a visible triangle whose neighbor across them is not visible.
        let mut horizon = Vec::new();
        for &k in &visible {
            for e in 0..3 {
                let (u, w) = (tris[k].v[e], tris[k].v[(e + 1) % 3]);
                let across = edges.get(&(w, u)).copied();
                if across.is_none_or(|j| seen[j] != mark) {
                    horizon.push((u, w));
                }
            }
        }
        for &k in &visible {
            for e in 0..3 {
                let key = (tris[k].v[e], tris[k].v[(e + 1) % 3]);
                if edges.get(&key) == Some(&k) {
                    edges.remove(&key);
                }
            }
            free.push(k);
        }
        for (u, w) in horizon {
            let Some(t) = tri(&pts, [u, w, pi]) else {
                continue;
            };
            let k = match free.pop() {
                Some(k) => {
                    tris[k] = t;
                    k
                }
                None => {
                    tris.push(t);
                    seen.push(0);
                    tris.len() - 1
                }
            };
            for e in 0..3 {
                edges.insert((tris[k].v[e], tris[k].v[(e + 1) % 3]), k);
            }
            live.push(k);
        }
        if live.len() > MAX_TRIANGLES_PER_POINT * pts.len() + 64 {
            return Err(HullFail::Tangled);
        }
    }
    // Triangles whose corners all lie within ON_FACE of a face already found (facing its way) are on that face. Each face's
    // normal is then its triangles' area-weighted mean, moved out to the farthest point, so every point is behind it.
    let mut faces: Vec<(Plane, V3)> = Vec::new();
    for t in live.iter().map(|&k| &tris[k]) {
        if !work.charge(faces.len() as u64 + 1) {
            return Err(HullFail::Work);
        }
        let [a, b, c] = t.v.map(|i| pts[i]);
        let area = geom::cross(geom::sub(b, a), geom::sub(c, a));
        let on = |q: &(Plane, V3)| {
            geom::dot(q.0.n, t.plane.n) > 0.9
                && t.v.iter().all(|&i| q.0.dist(pts[i]).abs() < ON_FACE)
        };
        match faces.iter_mut().find(|q| on(q)) {
            Some(f) => f.1 = geom::add(f.1, area),
            None => {
                if faces.len() == MAX_FACES {
                    return Err(HullFail::Faces(MAX_FACES + 1));
                }
                faces.push((t.plane, area));
            }
        }
    }
    if !work.charge(faces.len() as u64 * (pts.len() + faces.len()) as u64) {
        return Err(HullFail::Work);
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
        let planes = hull_planes(&pts, &mut Budget::new(u64::MAX)).unwrap();
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
        let planes = hull_planes(&pts, &mut Budget::new(u64::MAX)).unwrap();
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
        assert_eq!(
            hull_planes(&flat, &mut Budget::new(u64::MAX)),
            Err(HullFail::Flat)
        );
        assert_eq!(
            hull_planes(&[[0.0; 3]; 3], &mut Budget::new(u64::MAX)),
            Err(HullFail::Flat)
        );
        assert_eq!(
            hull_planes(
                &[[f64::NAN, 0.0, 0.0], [1.0; 3], [2.0; 3], [3.0, 1.0, 0.0]],
                &mut Budget::new(u64::MAX)
            ),
            Err(HullFail::NotFinite)
        );
        let many: Vec<V3> = (0..MAX_POINTS + 1)
            .map(|i| [i as f64, (i % 7) as f64, (i % 13) as f64])
            .collect();
        assert!(matches!(
            hull_planes(&many, &mut Budget::new(u64::MAX)),
            Err(HullFail::TooMany(_))
        ));
        // A dense moment curve: its points lie so near the faces of the hull so far that the triangles stop closing.
        let curve: Vec<V3> = (0..4_095)
            .map(|i| {
                let t = i as f64 / 2_047.0 - 1.0;
                [100.0 * t, 100.0 * t * t, 100.0 * t * t * t]
            })
            .collect();
        let start = web_time::Instant::now();
        assert_eq!(
            hull_planes(&curve, &mut Budget::new(u64::MAX)),
            Err(HullFail::Tangled)
        );
        assert!(start.elapsed().as_secs() < 5);
    }

    #[test]
    fn hull_work_is_charged() {
        // Points on a bowl in a spiral outward: every one a corner, about 8,000 triangles.
        let n = 4_095;
        let bowl: Vec<V3> = (0..n)
            .map(|i| {
                let r = 10.0 + 90.0 * i as f64 / n as f64;
                let a = i as f64 * 2.399_963_229_728_653;
                [r * a.cos(), r * a.sin(), r * r / 100.0]
            })
            .collect();
        let mut work = Budget::new(100_000);
        assert_eq!(hull_planes(&bowl, &mut work), Err(HullFail::Work));
        assert!(work.spent > work.cap);
        assert_eq!(
            hull_planes(&bowl, &mut Budget::new(u64::MAX)),
            Err(HullFail::Faces(MAX_FACES + 1))
        );
        let mut work = Budget::new(u64::MAX);
        hull_planes(&bowl[..400], &mut work).unwrap();
        assert!(work.spent > 400 * 400, "{}", work.spent);
    }
}
