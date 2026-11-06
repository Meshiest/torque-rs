//! Brushes: convex solids, each the intersection of the spaces behind its faces' planes.

use crate::geom::{self, EPS, Plane, V3};

/// A face's texture planes: `u = u . p + u[3]`, `v = v . p + v[3]` in texture repeats (a `.dif`'s texgen, exactly).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexGen {
    pub u: [f64; 4],
    pub v: [f64; 4],
}

impl TexGen {
    /// The texture coordinates at `p`.
    pub fn at(&self, p: V3) -> [f64; 2] {
        [
            self.u[0] * p[0] + self.u[1] * p[1] + self.u[2] * p[2] + self.u[3],
            self.v[0] * p[0] + self.v[1] * p[1] + self.v[2] * p[2] + self.v[3],
        ]
    }

    /// One repeat every `size` units along the two world axes across `normal`'s main axis (the axis-aligned projection), `u`
    /// along the first and `v` down the second.
    pub fn world(normal: V3, size: f64) -> TexGen {
        let k = main_axis(normal);
        let (a, b) = match k {
            2 => (0, 1),
            0 => (1, 2),
            _ => (0, 2),
        };
        let mut u = [0.0; 4];
        let mut v = [0.0; 4];
        u[a] = 1.0 / size;
        v[b] = -1.0 / size;
        TexGen { u, v }
    }
}

/// The axis `n` points most along (the first of equals).
pub fn main_axis(n: V3) -> usize {
    let a = n.map(f64::abs);
    if a[2] >= a[0] && a[2] >= a[1] {
        2
    } else if a[0] >= a[1] {
        0
    } else {
        1
    }
}

/// One face of a brush: its plane (facing out), its material and texture planes.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    pub plane: Plane,
    pub material: String,
    pub texgen: TexGen,
}

/// A convex solid.
#[derive(Debug, Clone, PartialEq)]
pub struct Brush {
    pub faces: Vec<Face>,
}

/// Why a brush is not a solid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrushError {
    /// Fewer than four faces hold a polygon: the planes bound no volume.
    NoVolume,
    /// A face's polygon is still the size it started at: the planes do not close the brush.
    Open,
    /// A value is not finite.
    NotFinite,
}

/// How far a face's starting polygon reaches (in the input's units); a brush larger than this is open.
pub const MAX_EXTENT: f64 = 1.0e6;

impl Brush {
    /// The box `min`..`max` with `material` on every face, textured by `texgen` of each face's outward normal.
    pub fn cuboid(min: V3, max: V3, material: &str, texgen: impl Fn(V3) -> TexGen) -> Brush {
        let mut faces = Vec::with_capacity(6);
        for k in 0..3 {
            for (sign, at) in [(-1.0, min), (1.0, max)] {
                let mut n = V3::ZERO;
                n[k] = sign;
                if let Some(plane) = Plane::new(n, at) {
                    faces.push(Face {
                        plane,
                        material: material.to_string(),
                        texgen: texgen(n),
                    });
                }
            }
        }
        Brush { faces }
    }

    /// A brush of the given planes (each facing out), all with `material` and `texgen` of each face's normal.
    pub fn from_planes(planes: &[Plane], material: &str, texgen: impl Fn(V3) -> TexGen) -> Brush {
        Brush {
            faces: planes
                .iter()
                .map(|&plane| Face {
                    plane,
                    material: material.to_string(),
                    texgen: texgen(plane.n),
                })
                .collect(),
        }
    }

    /// Each face's polygon (clockwise seen from outside), empty for a face whose plane holds none (a plane that does not touch
    /// the solid, or a second copy of a plane).
    pub fn polygons(&self) -> Result<Vec<Vec<V3>>, BrushError> {
        polygons_of(self.faces.iter().map(|f| f.plane))
    }

    /// The brush's corners (each once).
    pub fn corners(&self) -> Result<Vec<V3>, BrushError> {
        let mut out: Vec<V3> = Vec::new();
        for p in self.polygons()?.into_iter().flatten() {
            if !out.iter().any(|&q| (p - q).length() < EPS) {
                out.push(p);
            }
        }
        Ok(out)
    }

    /// Whether `p` is inside (more than `margin` behind every face).
    pub fn contains(&self, p: V3, margin: f64) -> bool {
        self.faces.iter().all(|f| f.plane.dist(p) < -margin)
    }

    /// The box of the brush's corners.
    pub fn bounds(&self) -> Result<(V3, V3), BrushError> {
        Ok(geom::bounds(self.corners()?))
    }
}

/// The polygon of each plane of a convex solid bounded by `planes` (see [`Brush::polygons`]).
pub fn polygons_of(planes: impl Iterator<Item = Plane>) -> Result<Vec<Vec<V3>>, BrushError> {
    let all: Vec<Plane> = planes.collect();
    if all.iter().any(|p| !(p.n.is_finite() && p.d.is_finite())) {
        return Err(BrushError::NotFinite);
    }
    let first = clip_all(&all, MAX_EXTENT)?;
    if first
        .iter()
        .flatten()
        .any(|v| v.abs().cmpgt(V3::splat(MAX_EXTENT * 0.5)).any())
    {
        return Err(BrushError::Open);
    }
    // Again from polygons a little larger than the solid: the error of a cut grows with the polygon it cuts.
    let reach = first
        .iter()
        .flatten()
        .flat_map(|v| v.abs().to_array())
        .fold(0.0f64, f64::max);
    clip_all(&all, reach * 4.0 + 1.0)
}

/// Each plane's polygon from a square of half side `half`, cut by the others.
fn clip_all(all: &[Plane], half: f64) -> Result<Vec<Vec<V3>>, BrushError> {
    let mut out = Vec::with_capacity(all.len());
    let mut held = 0;
    for (i, p) in all.iter().enumerate() {
        let mut w = geom::base_winding(p, half);
        for (j, q) in all.iter().enumerate() {
            if i == j || w.is_empty() {
                continue;
            }
            // A second copy of a plane keeps the first's polygon only.
            if p.same(q) {
                if j < i {
                    w.clear();
                }
                continue;
            }
            w = geom::clip_back(&w, q);
        }
        geom::dedup(&mut w);
        if w.len() >= 3 && geom::area(&w) > EPS * EPS {
            held += 1;
            out.push(w);
        } else {
            out.push(Vec::new());
        }
    }
    if held < 4 {
        return Err(BrushError::NoVolume);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(n: V3) -> TexGen {
        TexGen::world(n, 1.0)
    }

    #[test]
    fn a_cuboid_has_six_square_faces_and_eight_corners() {
        let b = Brush::cuboid(V3::new(0.0, 0.0, 0.0), V3::new(1.0, 2.0, 3.0), "m", grid);
        let polys = b.polygons().unwrap();
        assert_eq!(polys.len(), 6);
        assert!(polys.iter().all(|p| p.len() == 4));
        assert_eq!(b.corners().unwrap().len(), 8);
        let total: f64 = polys.iter().map(|p| geom::area(p)).sum();
        assert!((total - 2.0 * (2.0 + 3.0 + 6.0)).abs() < 1e-9);
        // Every face clockwise seen from outside.
        for (f, p) in b.faces.iter().zip(&polys) {
            assert!(geom::area_vector(p).dot(f.plane.n) < 0.0);
        }
        assert!(b.contains(V3::new(0.5, 1.0, 1.5), 0.0));
        assert!(!b.contains(V3::new(1.5, 1.0, 1.5), 0.0));
    }

    #[test]
    fn open_or_flat_brushes_are_refused() {
        let up = Plane::new(V3::new(0.0, 0.0, 1.0), V3::splat(0.0)).unwrap();
        let down = Plane::new(V3::new(0.0, 0.0, -1.0), V3::new(0.0, 0.0, -1.0)).unwrap();
        assert_eq!(
            Brush::from_planes(&[up, down], "m", grid).polygons(),
            Err(BrushError::NoVolume)
        );
        let side = Plane::new(V3::new(1.0, 0.0, 0.0), V3::splat(0.0)).unwrap();
        let other = Plane::new(V3::new(0.0, 1.0, 0.0), V3::splat(0.0)).unwrap();
        assert_eq!(
            Brush::from_planes(&[up, down, side, other], "m", grid).polygons(),
            Err(BrushError::Open)
        );
    }

    #[test]
    fn a_redundant_plane_holds_no_polygon() {
        let mut b = Brush::cuboid(V3::splat(0.0), V3::splat(1.0), "m", grid);
        b.faces.push(Face {
            plane: Plane::new(V3::new(1.0, 0.0, 0.0), V3::new(5.0, 0.0, 0.0)).unwrap(),
            material: "m".into(),
            texgen: grid(V3::new(1.0, 0.0, 0.0)),
        });
        b.faces.push(b.faces[0].clone());
        let polys = b.polygons().unwrap();
        assert!(polys[6].is_empty() && polys[7].is_empty() && polys[0].len() == 4);
    }
}
