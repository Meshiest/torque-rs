//! The builder's verification: a built interior's bytes read back as the engine reads them, checked against its brushes by brute
//! force (the `verify` feature; the tests and `just dif-verify` run it).
//!
//! - **The file:** [`torque_maps::dif::parse`] gives back exactly what was built, and [`torque_maps::dif::Interior::check`] passes.
//! - **Surfaces:** points sampled on the brushes' drawn faces (area-weighted) lie on exactly one drawn triangle of the
//!   importer ([`torque_maps::legacy::IMPORTER`]) when no other brush covers them (inside it, or on a face of it looking back,
//!   or on a face of an earlier brush looking the same way), on none when one does, with the material and the texture
//!   coordinates of the face drawn there; and back, points sampled on the drawn triangles lie on such a face. Each material's
//!   drawn area equals its surfaces'.
//! - **Collision:** random segments through `torque_collide`'s `InteriorShape` against each brush's planes clipped by brute
//!   force: the first entry's fraction and normal.
//! - **The parts the engine does not read:** random points the written BSP calls solid exactly when they are inside a brush;
//!   each hull's emit strings decoded rebuild its brush's faces; each polygon list decodes to its hull's surfaces.
//!
//! A point within [`BAND`] of a brush plane that is not its own face's (an edge, a corner, a seam) is left out of the point
//! checks: there "inside" turns on the last bits.

use std::collections::HashMap;
use std::sync::Arc;

use torque_collide::map::{InteriorGeom, InteriorShape};
use torque_mapcore::Importer;
use torque_maps::dif::{self, BspChild, Interior, NULL_SURFACE, PLANE_FLIPPED};

use crate::brush::{Brush, TexGen};
use crate::build::{Built, Options, build, is_null_material};
use crate::geom::{self, Plane, V3};
use crate::hull;

/// How far from a plane a point must be to be checked.
pub const BAND: f64 = 1e-3;
/// How far a point may be from a drawn triangle, and its texture coordinates from the face's.
pub const TOLERANCE: f64 = 1e-4;

/// How many of each check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub surface_points: usize,
    pub segments: usize,
    pub bsp_points: usize,
}

impl Counts {
    /// The full counts: 100,000 surface points, 1,000,000 segments and 1,000,000 BSP points.
    pub const FULL: Counts = Counts {
        surface_points: 100_000,
        segments: 1_000_000,
        bsp_points: 1_000_000,
    };

    /// Each count times `f` (at least one).
    pub fn scaled(f: f64) -> Counts {
        let s = |n: usize| ((n as f64 * f) as usize).max(1);
        Counts {
            surface_points: s(Counts::FULL.surface_points),
            segments: s(Counts::FULL.segments),
            bsp_points: s(Counts::FULL.bsp_points),
        }
    }
}

/// What the verification found; [`Report::clean`] when every check holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub surfaces: usize,
    pub triangles: usize,
    pub hulls: usize,
    pub bsp_nodes: usize,
    /// The file read back is what was built, and the check passed.
    pub read_back: bool,
    pub check_errors: usize,
    pub surface_checked: usize,
    /// The face points another brush covers.
    pub covered: usize,
    /// A face point drawn no time, or two, or drawn where it is covered.
    pub surface_misses: usize,
    pub uv_mismatches: usize,
    pub back_checked: usize,
    /// A drawn point on no uncovered face of its material.
    pub back_misses: usize,
    pub worst_area: f64,
    pub segments: usize,
    /// Segments that graze a brush or start on one, not compared.
    pub grazes: usize,
    pub hits: usize,
    pub ray_mismatches: usize,
    pub worst_fraction: f64,
    pub bsp_checked: usize,
    pub bsp_mismatches: usize,
    pub emit_differences: usize,
    pub poly_list_differences: usize,
    /// The first mismatch, described.
    pub first: Option<String>,
}

impl Report {
    pub fn clean(&self) -> bool {
        self.read_back
            && self.check_errors == 0
            && self.surface_misses == 0
            && self.uv_mismatches == 0
            && self.back_misses == 0
            && self.worst_area <= TOLERANCE
            && self.ray_mismatches == 0
            && self.bsp_mismatches == 0
            && self.emit_differences == 0
            && self.poly_list_differences == 0
    }

    /// The numbers on one line.
    pub fn line(&self) -> String {
        format!(
            "{} surfaces, {} triangles, {} hulls, {} nodes; read back {}, check errors {}; surface points {} ({} covered) misses {} uv {} back {} misses {} area {:.1e}; segments {} ({} grazing) hits {} mismatches {} worst {:.1e}; bsp points {} mismatches {}; emit differences {}, poly list differences {}",
            self.surfaces,
            self.triangles,
            self.hulls,
            self.bsp_nodes,
            self.read_back,
            self.check_errors,
            self.surface_checked,
            self.covered,
            self.surface_misses,
            self.uv_mismatches,
            self.back_checked,
            self.back_misses,
            self.worst_area,
            self.segments,
            self.grazes,
            self.hits,
            self.ray_mismatches,
            self.worst_fraction,
            self.bsp_checked,
            self.bsp_mismatches,
            self.emit_differences,
            self.poly_list_differences
        )
    }
}

/// A small deterministic generator.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform in `[lo, hi)`.
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * ((self.next_u64() >> 11) as f64 / (1u64 << 53) as f64)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }

    pub fn unit(&mut self) -> V3 {
        loop {
            let v = [
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
            ];
            if let Some(n) = geom::normalize(v)
                && geom::length(v) <= 1.0
            {
                return n;
            }
        }
    }
}

const MATERIALS: [&str; 4] = ["stone", "wood", "metal", "NULL"];

/// A generated brush map `seed`: a ground slab (sometimes 150 units wide, past a lightmap page), boxes on a half-unit grid
/// (touching and sharing faces) and off it, two copies of one box, boxes cut by slanted planes and round convex rocks, with
/// world-aligned and arbitrary texture planes.
pub fn generated(seed: u64) -> Vec<Brush> {
    let mut r = Rng::new(seed + 1);
    let mut out = Vec::new();
    let material = |r: &mut Rng| -> String {
        let k = r.below(10);
        MATERIALS[if k == 0 { 3 } else { k % 3 }].to_string()
    };
    let texgen = |r: &mut Rng, n: V3| -> TexGen {
        if r.below(2) == 0 {
            TexGen::world(n, r.range(0.5, 4.0))
        } else {
            let (a, b) = (r.unit(), r.unit());
            let s = r.range(0.2, 2.0);
            TexGen {
                u: [a[0] * s, a[1] * s, a[2] * s, r.range(-1.0, 1.0)],
                v: [b[0] * s, b[1] * s, b[2] * s, r.range(-1.0, 1.0)],
            }
        }
    };
    let paint = |r: &mut Rng, b: &mut Brush| {
        let m = material(r);
        for f in &mut b.faces {
            f.material = if r.below(6) == 0 {
                material(r)
            } else {
                m.clone()
            };
            f.texgen = texgen(r, f.plane.n);
        }
    };
    let half = if seed % 4 == 3 { 75.0 } else { 20.0 };
    let mut ground = Brush::cuboid([-half, -half, -1.0], [half, half, 0.0], "", |n| {
        TexGen::world(n, 1.0)
    });
    paint(&mut r, &mut ground);
    out.push(ground);
    let boxes = 4 + r.below(10);
    for k in 0..boxes {
        let snap = k % 2 == 0;
        let q = |r: &mut Rng, lo: f64, hi: f64| {
            let v = r.range(lo, hi);
            if snap { (v * 2.0).round() / 2.0 } else { v }
        };
        let min = [
            q(&mut r, -15.0, 15.0),
            q(&mut r, -15.0, 15.0),
            q(&mut r, -0.5, 5.0),
        ];
        let size = [
            q(&mut r, 0.5, 8.0),
            q(&mut r, 0.5, 8.0),
            q(&mut r, 0.5, 6.0),
        ];
        let max = [0, 1, 2].map(|i| min[i] + size[i].max(0.5));
        let mut b = Brush::cuboid(min, max, "", |n| TexGen::world(n, 1.0));
        paint(&mut r, &mut b);
        if k == 1 {
            out.push(b.clone());
        }
        out.push(b);
    }
    for _ in 0..2 + r.below(3) {
        let min = [
            r.range(-15.0, 12.0),
            r.range(-15.0, 12.0),
            r.range(0.0, 4.0),
        ];
        let max = [0, 1, 2].map(|i| min[i] + r.range(1.0, 5.0));
        let mut b = Brush::cuboid(min, max, "", |n| TexGen::world(n, 1.0));
        for _ in 0..1 + r.below(2) {
            let c = [0, 1, 2].map(|i| r.range(min[i], max[i]));
            if let Some(plane) = Plane::new(r.unit(), c) {
                b.faces.push(crate::brush::Face {
                    plane,
                    material: String::new(),
                    texgen: TexGen::world(plane.n, 1.0),
                });
            }
        }
        if b.polygons().is_ok() {
            paint(&mut r, &mut b);
            out.push(b);
        }
    }
    for _ in 0..1 + r.below(2) {
        let c = [
            r.range(-12.0, 12.0),
            r.range(-12.0, 12.0),
            r.range(0.0, 4.0),
        ];
        let radius = r.range(0.7, 2.5);
        let planes: Vec<Plane> = (0..8 + r.below(8))
            .filter_map(|_| {
                let n = r.unit();
                Plane::new(n, geom::add(c, geom::scale(n, radius)))
            })
            .collect();
        let mut b = Brush::from_planes(&planes, "", |n| TexGen::world(n, 1.0));
        if b.polygons().is_ok() {
            paint(&mut r, &mut b);
            out.push(b);
        }
    }
    out
}

/// A drawn triangle of the importer.
struct Tri {
    p: [V3; 3],
    uv: [[f64; 2]; 3],
    n: V3,
    material: usize,
    area: f64,
}

impl Tri {
    /// The barycentric weights of `s` projected on the triangle, and its distance from the triangle's plane.
    fn weights(&self, s: V3) -> ([f64; 3], f64) {
        let e1 = geom::sub(self.p[1], self.p[0]);
        let e2 = geom::sub(self.p[2], self.p[0]);
        let n = geom::cross(e1, e2);
        let nn = geom::dot(n, n);
        let d = geom::sub(s, self.p[0]);
        let off = geom::dot(d, n) / nn.sqrt();
        let b1 = geom::dot(geom::cross(d, e2), n) / nn;
        let b2 = geom::dot(geom::cross(e1, d), n) / nn;
        ([1.0 - b1 - b2, b1, b2], off)
    }

    /// Whether `s` is on the triangle (within [`TOLERANCE`] of its plane, inside it by `margin` of its size).
    fn holds(&self, s: V3, margin: f64) -> Option<[f64; 3]> {
        let (w, off) = self.weights(s);
        (off.abs() < TOLERANCE && w.iter().all(|&x| x >= -margin)).then_some(w)
    }
}

/// The faces of each plane (both ways), for the point checks.
struct Faces<'a> {
    brushes: &'a [Brush],
    polys: Vec<Vec<Vec<V3>>>,
}

/// The face drawn at a face point, as the checks reckon it.
enum Verdict {
    /// Too near a seam.
    Skip,
    Covered,
    /// The face drawn there: brush, face.
    Drawn(usize, usize),
}

impl Faces<'_> {
    /// What is drawn at `s`, a point of face (`bi`, `fi`) (module docs).
    fn verdict(&self, s: V3, bi: usize, fi: usize) -> Verdict {
        let plane = self.brushes[bi].faces[fi].plane;
        let mut winner = (bi, fi);
        for (k, b) in self.brushes.iter().enumerate() {
            // Only a brush the point is in or near.
            if b.faces.iter().any(|g| g.plane.dist(s) > BAND) {
                continue;
            }
            let mut on_same = None;
            let mut on_opposite = false;
            for (gi, g) in b.faces.iter().enumerate() {
                let d = g.plane.dist(s);
                if g.plane.same(&plane) && d.abs() < BAND {
                    on_same = Some(gi);
                    continue;
                }
                if g.plane.same(&plane.flipped()) && d.abs() < BAND {
                    on_opposite = true;
                    continue;
                }
                if d.abs() < BAND {
                    return Verdict::Skip;
                }
            }
            if k == bi {
                continue;
            }
            match (on_same, on_opposite) {
                (None, false) | (_, true) => return Verdict::Covered,
                (Some(gi), false) => {
                    if self.polys[k][gi].is_empty() {
                        continue;
                    }
                    if k < winner.0 {
                        winner = (k, gi);
                    }
                }
            }
        }
        Verdict::Drawn(winner.0, winner.1)
    }
}

/// Points spread over polygons by area: each polygon fanned, a triangle picked by its area, a point in it.
fn sample(r: &mut Rng, polys: &[(usize, Vec<V3>)], n: usize) -> Vec<(usize, V3)> {
    let mut tris: Vec<(usize, [V3; 3], f64)> = Vec::new();
    for (id, p) in polys {
        for k in 1..p.len().saturating_sub(1) {
            let t = [p[0], p[k], p[k + 1]];
            let a = geom::area(&t);
            if a > 1e-12 {
                tris.push((*id, t, a));
            }
        }
    }
    let total: f64 = tris.iter().map(|t| t.2).sum();
    let mut cum = Vec::with_capacity(tris.len());
    let mut acc = 0.0;
    for t in &tris {
        acc += t.2;
        cum.push(acc);
    }
    if tris.is_empty() {
        return Vec::new();
    }
    (0..n)
        .map(|_| {
            let x = r.range(0.0, total);
            let i = cum.partition_point(|&c| c < x).min(tris.len() - 1);
            let (id, t, _) = &tris[i];
            let (mut a, mut b) = (r.range(0.0, 1.0), r.range(0.0, 1.0));
            if a + b > 1.0 {
                a = 1.0 - a;
                b = 1.0 - b;
            }
            let p = geom::add(
                t[0],
                geom::add(
                    geom::scale(geom::sub(t[1], t[0]), a),
                    geom::scale(geom::sub(t[2], t[0]), b),
                ),
            );
            (*id, p)
        })
        .collect()
}

fn wide(p: [f32; 3]) -> V3 {
    p.map(f64::from)
}

/// The plane of a file's plane index, its flip applied.
fn file_plane(i: &Interior, index: u16) -> Plane {
    let p = i.planes[(index & !PLANE_FLIPPED) as usize];
    let pl = Plane {
        n: wide(p.normal),
        d: f64::from(p.d),
    };
    if index & PLANE_FLIPPED != 0 {
        pl.flipped()
    } else {
        pl
    }
}

/// Builds `brushes` and runs every check (module docs) with `counts`, its random draws from `seed`.
pub fn run(
    brushes: &[Brush],
    options: &Options,
    counts: Counts,
    seed: u64,
) -> Result<Report, String> {
    let built: Built = build(brushes, options).map_err(|e| e.to_string())?;
    check(brushes, &built, counts, seed)
}

/// The checks of [`run`] on an interior built from `brushes` (the tests hand it a damaged one).
pub fn check(
    brushes: &[Brush],
    built: &Built,
    counts: Counts,
    seed: u64,
) -> Result<Report, String> {
    let bytes = built.bytes().map_err(|e| e.to_string())?;
    let mut report = Report::default();
    let read = dif::parse(&bytes).map_err(|e| e.to_string())?;
    let i = &read.details[0];
    report.read_back = read == built.dif && read.trailing == 0 && i.plain_surfaces;
    report.check_errors = usize::from(i.check().is_err());
    report.surfaces = i.surfaces.len();
    report.hulls = i.hulls.hulls.len();
    report.bsp_nodes = i.bsp_nodes.len();
    let mut r = Rng::new(seed);

    // Surfaces.
    let imported = torque_maps::legacy::IMPORTER
        .surfaces(&bytes)
        .map_err(|e| e.0)?;
    let materials: Vec<String> = imported.by_material.keys().cloned().collect();
    let mut tris: Vec<Tri> = Vec::new();
    for (mi, corners) in imported.by_material.values().enumerate() {
        for c in corners.chunks_exact(3) {
            let p = [wide(c[0].0), wide(c[1].0), wide(c[2].0)];
            let area = geom::area(&p);
            if area < 1e-10 {
                continue;
            }
            tris.push(Tri {
                p,
                uv: [0, 1, 2].map(|k| c[k].2.map(f64::from)),
                n: wide(c[0].1),
                material: mi,
                area,
            });
        }
    }
    report.triangles = tris.len();
    // Triangles by the cells of a grid their boxes reach, for the point lookups.
    let (glo, ghi) = (wide(i.bounds.min), wide(i.bounds.max));
    let cell = (0..3).map(|k| ghi[k] - glo[k]).fold(1.0f64, f64::max) / 32.0;
    let cell_of = |p: V3| p.map(|v| (v / cell).floor() as i64);
    let mut grid: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    for (k, t) in tris.iter().enumerate() {
        let (a, b) = geom::bounds(t.p.iter().copied());
        let (ca, cb) = (
            cell_of(a.map(|v| v - 2.0 * TOLERANCE)),
            cell_of(b.map(|v| v + 2.0 * TOLERANCE)),
        );
        for x in ca[0]..=cb[0] {
            for y in ca[1]..=cb[1] {
                for z in ca[2]..=cb[2] {
                    grid.entry([x, y, z]).or_default().push(k);
                }
            }
        }
    }
    let polys: Vec<Vec<Vec<V3>>> = brushes
        .iter()
        .map(|b| b.polygons().unwrap_or_default())
        .collect();
    let faces = Faces { brushes, polys };
    let mut drawn_faces: Vec<(usize, Vec<V3>)> = Vec::new();
    let mut ids = Vec::new();
    for (bi, b) in brushes.iter().enumerate() {
        for (fi, f) in b.faces.iter().enumerate() {
            if !is_null_material(&f.material) && !faces.polys[bi][fi].is_empty() {
                drawn_faces.push((ids.len(), faces.polys[bi][fi].clone()));
                ids.push((bi, fi));
            }
        }
    }
    let material_of = |name: &str| materials.iter().position(|m| m == name);
    for (id, s) in sample(&mut r, &drawn_faces, counts.surface_points) {
        let (bi, fi) = ids[id];
        let n = brushes[bi].faces[fi].plane.n;
        let verdict = faces.verdict(s, bi, fi);
        if matches!(verdict, Verdict::Skip) {
            continue;
        }
        report.surface_checked += 1;
        let candidates = grid.get(&cell_of(s)).map_or(&[][..], Vec::as_slice);
        let on: Vec<(usize, [f64; 3])> = candidates
            .iter()
            .filter(|&&k| geom::dot(tris[k].n, n) > 1.0 - 1e-3)
            .filter_map(|&k| tris[k].holds(s, 1e-7).map(|w| (k, w)))
            .collect();
        let strictly = candidates
            .iter()
            .filter(|&&k| geom::dot(tris[k].n, n) > 1.0 - 1e-3)
            .filter(|&&k| tris[k].holds(s, -1e-6).is_some())
            .count();
        match verdict {
            Verdict::Covered => {
                report.covered += 1;
                if !on.is_empty() {
                    report.surface_misses += 1;
                }
            }
            Verdict::Drawn(wb, wf) => {
                let face = &brushes[wb].faces[wf];
                let Some(&(k, w)) = on.first() else {
                    report.surface_misses += 1;
                    continue;
                };
                if strictly > 1 || Some(tris[k].material) != material_of(&face.material) {
                    report.surface_misses += 1;
                }
                let t = &tris[k];
                let uv = [0, 1].map(|c| w[0] * t.uv[0][c] + w[1] * t.uv[1][c] + w[2] * t.uv[2][c]);
                let want = face.texgen.at(s);
                if (0..2).any(|c| (uv[c] - want[c]).abs() > TOLERANCE * want[c].abs().max(1.0)) {
                    report.uv_mismatches += 1;
                }
            }
            Verdict::Skip => {}
        }
    }
    // And back: points on the drawn triangles.
    let tri_polys: Vec<(usize, Vec<V3>)> = tris
        .iter()
        .enumerate()
        .map(|(k, t)| (k, t.p.to_vec()))
        .collect();
    for (k, s) in sample(&mut r, &tri_polys, counts.surface_points) {
        let t = &tris[k];
        // The source faces on the triangle's plane, facing its way, that hold the point.
        let mut holder = None;
        let mut near_edge = false;
        for &(bi, fi) in &ids {
            let f = &brushes[bi].faces[fi];
            if geom::dot(f.plane.n, t.n) < 1.0 - 1e-3 || f.plane.dist(s).abs() > TOLERANCE {
                continue;
            }
            let poly = &faces.polys[bi][fi];
            let inside = (0..poly.len()).all(|e| {
                let (a, b) = (poly[e], poly[(e + 1) % poly.len()]);
                let edge_n = geom::cross(f.plane.n, geom::sub(b, a));
                let d = geom::dot(geom::sub(s, a), edge_n) / geom::length(edge_n);
                if d.abs() < BAND {
                    near_edge = true;
                }
                d <= 0.0
            });
            if inside && holder.is_none() {
                holder = Some((bi, fi));
            }
        }
        if near_edge {
            continue;
        }
        let Some((bi, fi)) = holder else {
            report.back_checked += 1;
            report.back_misses += 1;
            continue;
        };
        match faces.verdict(s, bi, fi) {
            Verdict::Skip => {}
            Verdict::Covered => {
                report.back_checked += 1;
                report.back_misses += 1;
            }
            Verdict::Drawn(wb, wf) => {
                report.back_checked += 1;
                if material_of(&brushes[wb].faces[wf].material) != Some(t.material) {
                    report.back_misses += 1;
                }
            }
        }
    }
    // Area per material.
    let mut drawn_area: HashMap<usize, f64> = HashMap::new();
    for t in &tris {
        *drawn_area.entry(t.material).or_default() += t.area;
    }
    let mut built_area: HashMap<usize, f64> = HashMap::new();
    for (bi, fi, poly) in &built.surfaces {
        if let Some(m) = material_of(&brushes[*bi].faces[*fi].material) {
            *built_area.entry(m).or_default() += geom::area(poly);
        }
    }
    for m in 0..materials.len() {
        let (a, b) = (
            drawn_area.get(&m).copied().unwrap_or(0.0),
            built_area.get(&m).copied().unwrap_or(0.0),
        );
        report.worst_area = report.worst_area.max((a - b).abs() / b.max(1e-9));
    }

    // Collision.
    let set = torque_maps::legacy::IMPORTER
        .convexes(&bytes)
        .map_err(|e| e.0)?;
    let geom_ = Arc::new(InteriorGeom::new(&set));
    let shape = InteriorShape::new(geom_, torque_collide::math::IDENTITY, [1.0; 3])
        .ok_or("the interior does not place")?;
    let (lo, hi) = (wide(i.bounds.min), wide(i.bounds.max));
    let pad = [0, 1, 2].map(|k| (hi[k] - lo[k]) * 0.1 + 0.5);
    let point = |r: &mut Rng| [0, 1, 2].map(|k| r.range(lo[k] - pad[k], hi[k] + pad[k]));
    for _ in 0..counts.segments {
        let (s, e) = (point(&mut r), point(&mut r));
        let dir = geom::sub(e, s);
        let mut best: Option<f64> = None;
        let mut hits: Vec<(f64, V3)> = Vec::new();
        for b in brushes {
            if let Some(h) = enter(b, s, dir) {
                hits.push(h);
                best = Some(best.map_or(h.0, |x: f64| x.min(h.0)));
            }
        }
        report.segments += 1;
        let got = shape
            .cast_ray(s.map(|v| v as f32), e.map(|v| v as f32), &mut |_| true)
            .map_err(|_| "a ray ran out of work")?;
        // A segment that grazes a brush (it meets the brush grown by a hair and misses it shrunk by one, or the other way
        // round) or starts on one: either answer holds.
        let grazes = brushes.iter().any(|b| {
            let moved = |by: f64| Brush {
                faces: b
                    .faces
                    .iter()
                    .map(|f| crate::brush::Face {
                        plane: Plane {
                            n: f.plane.n,
                            d: f.plane.d + by,
                        },
                        ..f.clone()
                    })
                    .collect(),
            };
            let (grown, shrunk) = (
                enter(&moved(-TOLERANCE), s, dir),
                enter(&moved(TOLERANCE), s, dir),
            );
            grown.is_some() != shrunk.is_some()
                || (b.faces.iter().all(|f| f.plane.dist(s) < BAND)
                    && b.faces.iter().any(|f| f.plane.dist(s).abs() < BAND))
        });
        if grazes {
            report.grazes += 1;
            continue;
        }
        match (best, got) {
            (None, None) => {}
            (Some(t), Some((gt, gn))) => {
                report.hits += 1;
                let gt = f64::from(gt);
                report.worst_fraction = report.worst_fraction.max((gt - t).abs());
                let normal_ok = hits.iter().any(|&(ht, hn)| {
                    (ht - gt).abs() <= TOLERANCE && geom::length(geom::sub(hn, wide(gn))) < 1e-3
                });
                if (gt - t).abs() > TOLERANCE || !normal_ok {
                    report.ray_mismatches += 1;
                    report.first.get_or_insert_with(|| {
                        format!("segment {s:?} to {e:?}: brushes {hits:?}, file ({gt}, {gn:?})")
                    });
                }
            }
            (b, g) => {
                report.ray_mismatches += 1;
                report.first.get_or_insert_with(|| {
                    format!("segment {s:?} to {e:?}: brushes {b:?} {hits:?}, file {g:?}")
                });
            }
        }
    }

    // The BSP.
    for _ in 0..counts.bsp_points {
        let p = point(&mut r);
        if brushes
            .iter()
            .flat_map(|b| &b.faces)
            .any(|f| f.plane.dist(p).abs() < BAND)
        {
            continue;
        }
        report.bsp_checked += 1;
        let inside = brushes.iter().any(|b| b.contains(p, 0.0));
        if bsp_solid(i, p) != inside {
            report.bsp_mismatches += 1;
        }
    }

    // Emit strings and polygon lists.
    let h = &i.hulls;
    for (k, hull) in h.hulls.iter().enumerate() {
        let points: Vec<V3> = (0..hull.hull_count as usize)
            .map(|v| wide(i.points[h.hull_indices[hull.hull_start as usize + v] as usize]))
            .collect();
        let plane_end = h
            .hulls
            .get(k + 1)
            .map_or(h.hull_plane_indices.len(), |n| n.plane_start as usize);
        let planes: Vec<u16> = h.hull_plane_indices[hull.plane_start as usize..plane_end].to_vec();
        // The brush's faces as (plane index, sorted hull point numbers).
        let mut want: Vec<(u16, Vec<u8>)> = Vec::new();
        for poly in faces.polys[k].iter().filter(|p| !p.is_empty()) {
            let n = geom::area_vector(poly);
            let Some(pl) = planes.iter().find(|&&pi| {
                geom::dot(file_plane(i, pi).n, n) < 0.0
                    && poly.iter().all(|&v| file_plane(i, pi).dist(v).abs() < 1e-3)
            }) else {
                report.emit_differences += 1;
                continue;
            };
            let mut pts: Vec<u8> = poly
                .iter()
                .filter_map(|&v| {
                    points
                        .iter()
                        .position(|&q| geom::length(geom::sub(q, v)) < 1e-3)
                        .map(|x| x as u8)
                })
                .collect();
            pts.sort_unstable();
            pts.dedup();
            want.push((*pl, pts));
        }
        let mut got: Vec<(u16, Vec<u8>)> = Vec::new();
        for v in 0..hull.hull_count as usize {
            let at = h.hull_emit_string_indices[hull.hull_start as usize + v] as usize;
            let Some(decoded) = h.emit_strings.get(at..).and_then(hull::decode_emit_string) else {
                report.emit_differences += 1;
                continue;
            };
            for f in decoded {
                let mut pts = f.points.clone();
                pts.sort_unstable();
                got.push((
                    planes.get(f.plane as usize).copied().unwrap_or(u16::MAX),
                    pts,
                ));
            }
        }
        want.sort();
        want.dedup();
        got.sort();
        got.dedup();
        report.emit_differences += want.iter().filter(|w| !got.contains(w)).count()
            + got.iter().filter(|g| !want.contains(g)).count();
        // The polygon list against the hull's surfaces.
        let plane_end = h.hulls.get(k + 1).map_or(h.poly_list_planes.len(), |n| {
            n.poly_list_plane_start as usize
        });
        let point_end = h.hulls.get(k + 1).map_or(h.poly_list_points.len(), |n| {
            n.poly_list_point_start as usize
        });
        let string_end = h.hulls.get(k + 1).map_or(h.poly_list_strings.len(), |n| {
            n.poly_list_string_start as usize
        });
        let list = hull::PolyList {
            planes: h.poly_list_planes[hull.poly_list_plane_start as usize..plane_end].to_vec(),
            points: h.poly_list_points[hull.poly_list_point_start as usize..point_end].to_vec(),
            string: h.poly_list_strings[hull.poly_list_string_start as usize..string_end].to_vec(),
        };
        let expected: Vec<hull::HullSurface> = h.hull_surface_indices[hull.surface_start as usize
            ..hull.surface_start as usize + hull.surface_count as usize]
            .iter()
            .map(|&sr| collision_fan(i, sr))
            .collect();
        match hull::decode_poly_list(&list) {
            Some(decoded) if decoded == expected => {}
            _ => report.poly_list_differences += 1,
        }
    }
    Ok(report)
}

/// A surface's collision polygon as `collisionFanFromSurface` makes it (a null surface: its windings as they are).
fn collision_fan(i: &Interior, surface: u32) -> hull::HullSurface {
    if surface & NULL_SURFACE != 0 {
        let s = &i.null_surfaces[(surface & !NULL_SURFACE) as usize];
        let w = s.winding_start as usize;
        return hull::HullSurface {
            plane: s.plane,
            points: i.windings[w..w + s.winding_count as usize].to_vec(),
        };
    }
    let s = &i.surfaces[surface as usize];
    let n = s.winding_count as usize;
    let mut order = vec![0usize];
    order.extend((1..n).step_by(2));
    let mut k = (n - 1) & !1;
    while k > 0 {
        order.push(k);
        k -= 2;
    }
    let points = order
        .iter()
        .enumerate()
        .filter(|&(pos, _)| s.fan_mask & (1 << pos) != 0)
        .map(|(_, &o)| i.windings[s.winding_start as usize + o])
        .collect();
    hull::HullSurface {
        plane: s.plane,
        points,
    }
}

/// Whether the written BSP puts `p` in a solid leaf (`getZoneForPoint`'s walk).
fn bsp_solid(i: &Interior, p: V3) -> bool {
    let mut n = 0usize;
    for _ in 0..=i.bsp_nodes.len() {
        let node = &i.bsp_nodes[n];
        let next = if file_plane(i, node.plane).dist(p) >= 0.0 {
            node.front
        } else {
            node.back
        };
        match next {
            BspChild::Node(k) => n = k as usize,
            BspChild::Solid(_) => return true,
            BspChild::Empty(_) => return false,
        }
    }
    false
}

/// The segment `s + t dir` (t in 0..1) entering `b`: the fraction and the plane's normal; None for a miss or a start inside.
pub fn enter(b: &Brush, s: V3, dir: V3) -> Option<(f64, V3)> {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let mut normal = None;
    for f in &b.faces {
        let a = f.plane.dist(s);
        let e = a + geom::dot(f.plane.n, dir);
        if a > 0.0 && e > 0.0 {
            return None;
        }
        if a <= 0.0 && e <= 0.0 {
            continue;
        }
        let t = a / (a - e);
        if a > 0.0 {
            if normal.is_none() || t > t0 {
                t0 = t;
                normal = Some(f.plane.n);
            }
        } else {
            t1 = t1.min(t);
        }
        if t0 > t1 {
            return None;
        }
    }
    normal.map(|n| (t0, n))
}

#[cfg(test)]
#[path = "verify_tests.rs"]
mod tests;
