//! The mesh input's verification: every piece built from a [`MeshInput`] read back as the engine reads it and checked against
//! the input by brute force.
//!
//! - **The files:** each piece's bytes parse to what was built, and `Interior::check` passes.
//! - **Surfaces:** points sampled on the input's drawn triangles (area-weighted; a point within [`BAND`] of its triangle's
//!   edges left out) lie on exactly one drawn triangle of the importer facing the same way, with the triangle's material and
//!   texture coordinates; points sampled on the drawn triangles lie on an input triangle of their material; each material's
//!   drawn area equals the input's.
//! - **Collision against the hulls:** random segments through every piece's `InteriorShape` against each hull's planes
//!   clipped by brute force (as the brush checks do).
//! - **Collision against the source:** the same segments against the input itself: the first entry into a closed part's or
//!   an authored hull's triangles, or into a slab (an open part's hull is its slab by definition). A segment that starts
//!   inside one, or crosses a triangle within [`GRAZE`] of its edges, is not compared.
//! - **Points:** random points the pieces' BSPs call solid exactly when inside a hull, and inside a hull exactly when inside
//!   a closed part or an authored hull (by the parity of a ray's crossings) or a slab. A point within [`BAND`] of a nearby
//!   hull's plane or of an input triangle is left out.
//! - **Hull strings:** each hull's emit strings and polygon list, as for brushes.

use std::sync::Arc;

use super::{
    BAND, Counts, Report, Rng, TOLERANCE, Tri, bsp_solid, enter, enter_moved, hull_strings, sample,
    wide,
};
use crate::geom::{self, Plane, V3};
use crate::mesh::{HullKind, MeshBuilt, MeshInput, MeshOptions, build_mesh};
use torque_collide::map::{InteriorGeom, InteriorShape};
use torque_mapcore::Importer;
use torque_maps::dif;

/// How near a triangle's edge a crossing may be and still count.
pub const GRAZE: f64 = 2.0 * TOLERANCE;

/// A box tree over items' boxes, for the brute-force lookups.
pub struct Bvh {
    nodes: Vec<Node>,
    order: Vec<usize>,
}

struct Node {
    lo: V3,
    hi: V3,
    /// Children (`left`, `left + 1`) or none for a leaf of `order[start..end]`.
    left: Option<usize>,
    start: usize,
    end: usize,
}

impl Bvh {
    pub fn new(boxes: &[(V3, V3)]) -> Bvh {
        let mut order: Vec<usize> = (0..boxes.len()).collect();
        let mut nodes = vec![Node {
            lo: [0.0; 3],
            hi: [0.0; 3],
            left: None,
            start: 0,
            end: boxes.len(),
        }];
        let mut stack = vec![0usize];
        while let Some(k) = stack.pop() {
            let (start, end) = (nodes[k].start, nodes[k].end);
            let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
            for &i in &order[start..end] {
                for a in 0..3 {
                    lo[a] = lo[a].min(boxes[i].0[a]);
                    hi[a] = hi[a].max(boxes[i].1[a]);
                }
            }
            nodes[k].lo = lo;
            nodes[k].hi = hi;
            if end - start <= 4 {
                continue;
            }
            let axis = (0..3)
                .max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b])))
                .unwrap_or(0);
            let mid = (start + end) / 2;
            let c = |i: &usize| boxes[*i].0[axis] + boxes[*i].1[axis];
            order[start..end].select_nth_unstable_by(mid - start, |a, b| c(a).total_cmp(&c(b)));
            let left = nodes.len();
            nodes[k].left = Some(left);
            for (s, e) in [(start, mid), (mid, end)] {
                nodes.push(Node {
                    lo: [0.0; 3],
                    hi: [0.0; 3],
                    left: None,
                    start: s,
                    end: e,
                });
                stack.push(nodes.len() - 1);
            }
        }
        Bvh { nodes, order }
    }

    /// Calls `f` with every item whose box `meets` accepts.
    fn visit(&self, meets: &dyn Fn(V3, V3) -> bool, f: &mut dyn FnMut(usize)) {
        if self.order.is_empty() {
            return;
        }
        let mut stack = vec![0usize];
        while let Some(k) = stack.pop() {
            let n = &self.nodes[k];
            if !meets(n.lo, n.hi) {
                continue;
            }
            match n.left {
                Some(l) => stack.extend([l, l + 1]),
                None => self.order[n.start..n.end].iter().for_each(|&i| f(i)),
            }
        }
    }

    /// The items whose boxes (grown by `pad`) hold `p`.
    pub fn at(&self, p: V3, pad: f64) -> Vec<usize> {
        let mut out = Vec::new();
        self.visit(
            &|lo, hi| (0..3).all(|k| p[k] >= lo[k] - pad && p[k] <= hi[k] + pad),
            &mut |i| out.push(i),
        );
        out
    }

    /// The items whose boxes (grown by `pad`) the segment `s`-`e` meets.
    pub fn along(&self, s: V3, e: V3, pad: f64) -> Vec<usize> {
        let mut out = Vec::new();
        self.visit(&|lo, hi| segment_meets_box(s, e, lo, hi, pad), &mut |i| {
            out.push(i)
        });
        out
    }
}

fn segment_meets_box(s: V3, e: V3, lo: V3, hi: V3, pad: f64) -> bool {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for k in 0..3 {
        let d = e[k] - s[k];
        let (a, b) = (lo[k] - pad, hi[k] + pad);
        if d.abs() < 1e-300 {
            if s[k] < a || s[k] > b {
                return false;
            }
            continue;
        }
        let (mut ta, mut tb) = ((a - s[k]) / d, (b - s[k]) / d);
        if ta > tb {
            std::mem::swap(&mut ta, &mut tb);
        }
        t0 = t0.max(ta);
        t1 = t1.min(tb);
        if t0 > t1 {
            return false;
        }
    }
    true
}

/// A triangle's distance inside its edges at `s` (negative outside): the least of the three, in the input's units.
fn edge_dist(t: &Tri, s: V3) -> f64 {
    let (w, _) = t.weights(s);
    let twice = 2.0 * t.area;
    (0..3)
        .map(|i| {
            let (a, b) = (t.p[(i + 1) % 3], t.p[(i + 2) % 3]);
            w[i] * twice / geom::length(geom::sub(b, a)).max(1e-300)
        })
        .fold(f64::INFINITY, f64::min)
}

fn tri_box(t: &Tri) -> (V3, V3) {
    geom::bounds(t.p.iter().copied())
}

/// The plane of a triangle facing its front (`n`).
fn plane_of(t: &Tri) -> Plane {
    Plane {
        n: t.n,
        d: -geom::dot(t.n, t.p[0]),
    }
}

/// The input's triangles as the checks take them, each with its material's number among `names`; None for one of no area.
fn as_tri(t: &crate::mesh::Tri, material: usize) -> Option<Tri> {
    let plane = t.plane()?;
    let area = geom::area(&t.p);
    (area > 1e-12).then_some(Tri {
        p: t.p,
        uv: t.uv,
        n: plane.n,
        material,
        area,
    })
}

/// A solid of the source: its triangles' numbers among the solid triangles.
struct Solids {
    tris: Vec<Tri>,
    owner: Vec<usize>,
    count: usize,
    bvh: Bvh,
}

impl Solids {
    /// Whether `p` is inside a solid: an odd number of crossings of a ray from it.
    fn inside(&self, p: V3, reach: f64) -> bool {
        let dir = geom::normalize([0.5773, 0.4123, 0.7049]).unwrap_or([0.0, 0.0, 1.0]);
        let e = geom::add(p, geom::scale(dir, reach));
        let mut odd = vec![false; self.count];
        for k in self.bvh.along(p, e, 0.0) {
            let t = &self.tris[k];
            let pl = plane_of(t);
            let (a, b) = (pl.dist(p), pl.dist(e));
            if (a > 0.0) == (b > 0.0) {
                continue;
            }
            let q = geom::add(p, geom::scale(geom::sub(e, p), a / (a - b)));
            if edge_dist(t, q) >= 0.0 {
                odd[self.owner[k]] ^= true;
            }
        }
        odd.iter().any(|&o| o)
    }

    /// The first entry of the segment into a solid's triangles; `Err` when it crosses one within [`GRAZE`] of its edges, or
    /// starts or ends within it of one's plane where the crossing would be.
    fn first_entry(&self, s: V3, e: V3) -> Result<Option<(f64, V3)>, ()> {
        let mut best: Option<(f64, V3)> = None;
        let len = geom::length(geom::sub(e, s));
        for k in self.bvh.along(s, e, GRAZE) {
            let t = &self.tris[k];
            let pl = plane_of(t);
            let (a, b) = (pl.dist(s), pl.dist(e));
            if a.abs() < GRAZE || b.abs() < GRAZE {
                // An end on the plane: a graze when it is over the triangle.
                let near = if a.abs() < GRAZE { s } else { e };
                if edge_dist(t, near) > -GRAZE {
                    return Err(());
                }
                if (a > 0.0) == (b > 0.0) {
                    continue;
                }
            }
            if (a > 0.0) == (b > 0.0) {
                continue;
            }
            if (a - b).abs() < 1e-3 * len {
                // Nearly along the plane.
                return Err(());
            }
            let f = a / (a - b);
            let q = geom::add(s, geom::scale(geom::sub(e, s), f));
            let d = edge_dist(t, q);
            if d.abs() < GRAZE {
                return Err(());
            }
            if d > 0.0 && a > 0.0 && best.is_none_or(|(bt, _)| f < bt) {
                best = Some((f, t.n));
            }
        }
        Ok(best)
    }
}

/// Builds `input` and runs every check (module docs) with `counts`, its random draws from `seed`.
pub fn run_mesh(
    input: &MeshInput,
    options: &MeshOptions,
    counts: Counts,
    seed: u64,
) -> Result<Report, String> {
    let built = build_mesh(input, options).map_err(|e| e.to_string())?;
    check_mesh(input, &built, counts, seed)
}

/// The checks of [`run_mesh`] on pieces built from `input` (the tests hand it damaged ones).
pub fn check_mesh(
    input: &MeshInput,
    built: &MeshBuilt,
    counts: Counts,
    seed: u64,
) -> Result<Report, String> {
    let plan = &built.plan;
    let mut report = Report {
        pieces: built.pieces.len(),
        read_back: true,
        ..Report::default()
    };
    let mut r = Rng::new(seed);
    let mut names: Vec<String> = input.materials.clone();
    let name_of = |n: &str, names: &mut Vec<String>| {
        names.iter().position(|m| m == n).unwrap_or_else(|| {
            names.push(n.to_string());
            names.len() - 1
        })
    };

    // The pieces read back.
    let mut drawn: Vec<Tri> = Vec::new();
    let mut shapes = Vec::new();
    let mut interiors: Vec<dif::Interior> = Vec::new();
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for piece in &built.pieces {
        let bytes = piece.built.bytes().map_err(|e| e.to_string())?;
        let read = dif::parse(&bytes).map_err(|e| e.to_string())?;
        let i = &read.details[0];
        report.read_back &= read == piece.built.dif && read.trailing == 0 && i.plain_surfaces;
        report.check_errors += usize::from(i.check().is_err());
        report.surfaces += i.surfaces.len();
        report.hulls += i.hulls.hulls.len();
        report.bsp_nodes += i.bsp_nodes.len();
        for k in 0..3 {
            lo[k] = lo[k].min(f64::from(i.bounds.min[k]));
            hi[k] = hi[k].max(f64::from(i.bounds.max[k]));
        }
        let imported = torque_maps::legacy::IMPORTER
            .surfaces(&bytes)
            .map_err(|e| e.0)?;
        for (name, corners) in &imported.by_material {
            let mi = name_of(name, &mut names);
            for c in corners.chunks_exact(3) {
                let p = [wide(c[0].0), wide(c[1].0), wide(c[2].0)];
                let area = geom::area(&p);
                if area < 1e-10 {
                    continue;
                }
                drawn.push(Tri {
                    p,
                    uv: [0, 1, 2].map(|k| c[k].2.map(f64::from)),
                    n: wide(c[0].1),
                    material: mi,
                    area,
                });
            }
        }
        let set = torque_maps::legacy::IMPORTER
            .convexes(&bytes)
            .map_err(|e| e.0)?;
        let shape = InteriorShape::new(
            Arc::new(InteriorGeom::new(&set)),
            torque_collide::math::IDENTITY,
            [1.0; 3],
        )
        .ok_or("a piece does not place")?;
        shapes.push(shape);
        let polys: Vec<Vec<Vec<V3>>> = piece
            .hulls
            .iter()
            .map(|&k| plan.hulls[k].brush.polygons().unwrap_or_default())
            .collect();
        hull_strings(i, &polys, &mut report);
        interiors.push(read.details.into_iter().next().ok_or("no detail")?);
    }
    report.triangles = drawn.len();
    let drawn_bvh = Bvh::new(&drawn.iter().map(tri_box).collect::<Vec<_>>());

    // The input's drawn triangles.
    let mut source: Vec<Tri> = Vec::new();
    for m in &input.drawn {
        for t in &m.tris {
            if crate::build::is_null_material(&input.materials[t.material]) {
                continue;
            }
            source.extend(as_tri(t, t.material));
        }
    }
    let source_bvh = Bvh::new(&source.iter().map(tri_box).collect::<Vec<_>>());

    // Surfaces.
    let polys: Vec<(usize, Vec<V3>)> = source
        .iter()
        .enumerate()
        .map(|(k, t)| (k, t.p.to_vec()))
        .collect();
    for (k, s) in sample(&mut r, &polys, counts.surface_points) {
        let t = &source[k];
        if edge_dist(t, s) < BAND {
            continue;
        }
        report.surface_checked += 1;
        let candidates = drawn_bvh.at(s, 2.0 * TOLERANCE);
        let facing = |j: &&usize| geom::dot(drawn[**j].n, t.n) > 1.0 - 1e-3;
        let on: Vec<(usize, [f64; 3])> = candidates
            .iter()
            .filter(facing)
            .filter_map(|&j| drawn[j].holds(s, 1e-7).map(|w| (j, w)))
            .collect();
        let strictly = candidates
            .iter()
            .filter(facing)
            .filter(|&&j| drawn[j].holds(s, -1e-6).is_some())
            .count();
        let Some(&(j, w)) = on.first() else {
            report.surface_misses += 1;
            report
                .first
                .get_or_insert_with(|| format!("input point {s:?} is drawn nowhere"));
            continue;
        };
        if strictly > 1 || drawn[j].material != t.material {
            report.surface_misses += 1;
            report.first.get_or_insert_with(|| {
                format!(
                    "input point {s:?}: drawn {strictly} times, material {}",
                    drawn[j].material
                )
            });
        }
        let d = &drawn[j];
        let uv = [0, 1].map(|c| w[0] * d.uv[0][c] + w[1] * d.uv[1][c] + w[2] * d.uv[2][c]);
        let (ws, _) = t.weights(s);
        let want = [0, 1].map(|c| ws[0] * t.uv[0][c] + ws[1] * t.uv[1][c] + ws[2] * t.uv[2][c]);
        if (0..2).any(|c| (uv[c] - want[c]).abs() > TOLERANCE * want[c].abs().max(1.0)) {
            report.uv_mismatches += 1;
            report
                .first
                .get_or_insert_with(|| format!("input point {s:?}: uv {uv:?}, want {want:?}"));
        }
    }
    let tri_polys: Vec<(usize, Vec<V3>)> = drawn
        .iter()
        .enumerate()
        .map(|(k, t)| (k, t.p.to_vec()))
        .collect();
    for (k, s) in sample(&mut r, &tri_polys, counts.surface_points) {
        let t = &drawn[k];
        let mut holder = None;
        let mut near_edge = false;
        for j in source_bvh.at(s, 2.0 * TOLERANCE) {
            let f = &source[j];
            if geom::dot(f.n, t.n) < 1.0 - 1e-3 || plane_of(f).dist(s).abs() > TOLERANCE {
                continue;
            }
            let d = edge_dist(f, s);
            if d.abs() < BAND {
                near_edge = true;
            }
            if d >= 0.0 && holder.is_none_or(|h: usize| source[h].material != t.material) {
                holder = Some(j);
            }
        }
        if near_edge {
            continue;
        }
        report.back_checked += 1;
        if holder.is_none_or(|h| source[h].material != t.material) {
            report.back_misses += 1;
            report.first.get_or_insert_with(|| {
                format!("drawn point {s:?} is on no input triangle of its material")
            });
        }
    }
    let mut drawn_area = vec![0.0; names.len()];
    let mut source_area = vec![0.0; names.len()];
    for t in &drawn {
        drawn_area[t.material] += t.area;
    }
    for t in &source {
        source_area[t.material] += t.area;
    }
    for m in 0..names.len() {
        let (a, b) = (drawn_area[m], source_area[m]);
        if a > 0.0 || b > 0.0 {
            report.worst_area = report.worst_area.max((a - b).abs() / b.max(1e-9));
        }
    }

    // The source's solids: closed parts, then authored hulls.
    let mut solid_tris = Vec::new();
    let mut owner = Vec::new();
    let solid_meshes = plan
        .solids
        .iter()
        .map(|s| &s.tris)
        .chain(plan.hulls.iter().filter_map(|h| match h.kind {
            HullKind::Authored(k) => Some(&input.authored[k].tris),
            _ => None,
        }));
    let mut count = 0;
    for tris in solid_meshes {
        for t in tris {
            if let Some(x) = as_tri(t, 0) {
                solid_tris.push(x);
                owner.push(count);
            }
        }
        count += 1;
    }
    let solids = Solids {
        bvh: Bvh::new(&solid_tris.iter().map(tri_box).collect::<Vec<_>>()),
        tris: solid_tris,
        owner,
        count,
    };
    let hull_boxes: Vec<(V3, V3)> = plan
        .hulls
        .iter()
        .map(|h| h.brush.bounds().unwrap_or(([0.0; 3], [0.0; 3])))
        .collect();
    let hull_bvh = Bvh::new(&hull_boxes);
    let is_slab = |k: usize| plan.hulls[k].kind == HullKind::Slab;

    // Collision.
    let pad = [0, 1, 2].map(|k| (hi[k] - lo[k]) * 0.1 + 0.5);
    let reach = (0..3).map(|k| hi[k] - lo[k] + 2.0 * pad[k]).sum::<f64>() * 2.0;
    let point = |r: &mut Rng| [0, 1, 2].map(|k| r.range(lo[k] - pad[k], hi[k] + pad[k]));
    for _ in 0..counts.segments {
        let (s, e) = (point(&mut r), point(&mut r));
        let dir = geom::sub(e, s);
        report.segments += 1;
        let mut got: Option<(f64, V3)> = None;
        for shape in &shapes {
            let hit = shape
                .cast_ray(s.map(|v| v as f32), e.map(|v| v as f32), &mut |_| true)
                .map_err(|_| "a ray ran out of work")?;
            if let Some((t, n)) = hit
                && got.is_none_or(|(bt, _)| f64::from(t) < bt)
            {
                got = Some((f64::from(t), wide(n)));
            }
        }
        let candidates = hull_bvh.along(s, e, 2.0 * BAND);
        let mut hits: Vec<(f64, V3, usize)> = Vec::new();
        let mut grazes = false;
        for &k in &candidates {
            let b = &plan.hulls[k].brush;
            if let Some(h) = enter(b, s, dir) {
                hits.push((h.0, h.1, k));
                // At an edge or a corner every face entered at that moment is the hit's.
                for f in &b.faces {
                    let (a, e) = (f.plane.dist(s), f.plane.dist(geom::add(s, dir)));
                    if a > 0.0 && e < a && (a / (a - e) - h.0).abs() <= TOLERANCE {
                        hits.push((h.0, f.plane.n, k));
                    }
                }
            }
            grazes |= enter_moved(b, s, dir, -TOLERANCE).is_some()
                != enter_moved(b, s, dir, TOLERANCE).is_some()
                || (b.faces.iter().all(|f| f.plane.dist(s) < BAND)
                    && b.faces.iter().any(|f| f.plane.dist(s).abs() < BAND));
        }
        let compare = |want: Option<(f64, V3)>, all: &[(f64, V3)]| match (want, got) {
            (None, None) => true,
            (Some((t, _)), Some((gt, gn))) => {
                (gt - t).abs() <= TOLERANCE
                    && all.iter().any(|&(ht, hn)| {
                        (ht - gt).abs() <= TOLERANCE && geom::length(geom::sub(hn, gn)) < 1e-3
                    })
            }
            _ => false,
        };
        if grazes {
            report.grazes += 1;
        } else {
            let best = hits
                .iter()
                .map(|h| (h.0, h.1))
                .min_by(|a, b| a.0.total_cmp(&b.0));
            let all: Vec<(f64, V3)> = hits.iter().map(|h| (h.0, h.1)).collect();
            if best.is_some() {
                report.hits += 1;
            }
            if let (Some((t, _)), Some((gt, _))) = (best, got) {
                report.worst_fraction = report.worst_fraction.max((gt - t).abs());
            }
            if !compare(best, &all) {
                report.ray_mismatches += 1;
                report.first.get_or_insert_with(|| {
                    format!("segment {s:?} to {e:?}: hulls {best:?}, file {got:?}")
                });
            }
        }
        // Against the source.
        if solids.count == 0 && !plan.hulls.iter().any(|h| h.kind == HullKind::Slab) {
            continue;
        }
        report.source_segments += 1;
        let in_slab = candidates.iter().any(|&k| {
            is_slab(k)
                && plan.hulls[k]
                    .brush
                    .faces
                    .iter()
                    .all(|f| f.plane.dist(s) < BAND)
        });
        let entry = if grazes || in_slab || (solids.count > 0 && solids.inside(s, reach)) {
            Err(())
        } else {
            solids.first_entry(s, e)
        };
        let Ok(entry) = entry else {
            report.source_skipped += 1;
            continue;
        };
        let mut all: Vec<(f64, V3)> = hits
            .iter()
            .filter(|h| is_slab(h.2))
            .map(|h| (h.0, h.1))
            .collect();
        all.extend(entry);
        let best = all.iter().copied().min_by(|a, b| a.0.total_cmp(&b.0));
        if !compare(best, &all) {
            report.source_mismatches += 1;
            report.first.get_or_insert_with(|| {
                format!("segment {s:?} to {e:?}: source {best:?}, file {got:?}")
            });
        }
    }

    // Points.
    for _ in 0..counts.bsp_points {
        let p = point(&mut r);
        let near = hull_bvh.at(p, BAND);
        if near.iter().any(|&k| {
            plan.hulls[k]
                .brush
                .faces
                .iter()
                .any(|f| f.plane.dist(p).abs() < BAND)
        }) {
            continue;
        }
        let near_solid = solids.bvh.at(p, BAND).into_iter().any(|k| {
            let t = &solids.tris[k];
            plane_of(t).dist(p).abs() < BAND && edge_dist(t, p) > -BAND
        });
        if near_solid {
            continue;
        }
        report.bsp_checked += 1;
        let in_hull = near.iter().any(|&k| plan.hulls[k].brush.contains(p, 0.0));
        let in_bsp = interiors.iter().any(|i| {
            let (a, b) = (wide(i.bounds.min), wide(i.bounds.max));
            (0..3).all(|k| p[k] >= a[k] - BAND && p[k] <= b[k] + BAND) && bsp_solid(i, p)
        });
        if in_bsp != in_hull {
            report.bsp_mismatches += 1;
            report
                .first
                .get_or_insert_with(|| format!("point {p:?}: hulls {in_hull}, BSP {in_bsp}"));
        }
        if solids.count > 0 || near.iter().any(|&k| is_slab(k)) {
            report.solid_checked += 1;
            let in_source = near
                .iter()
                .any(|&k| is_slab(k) && plan.hulls[k].brush.contains(p, 0.0))
                || (solids.count > 0 && solids.inside(p, reach));
            if in_source != in_hull {
                report.solid_mismatches += 1;
                report.first.get_or_insert_with(|| {
                    format!("point {p:?}: hulls {in_hull}, source {in_source}")
                });
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify::fixtures::{BIG, fixture, read};

    /// The share of the full counts a debug test run takes.
    const SHARE: f64 = 0.003;

    #[test]
    fn the_generated_mesh_fixtures_pass_verification() {
        for seed in 0..20 {
            if seed == BIG {
                continue;
            }
            let f = fixture(seed);
            let input = read(&f).unwrap();
            let r = run_mesh(&input, &MeshOptions::default(), Counts::scaled(SHARE), seed).unwrap();
            assert!(
                r.clean(),
                "fixture {seed} ({}): {}\n{:?}",
                f.name,
                r.line(),
                r.first
            );
            assert!(
                r.surface_checked > 0 && r.hits > 0 && r.bsp_checked > 0,
                "fixture {seed}: {}",
                r.line()
            );
            assert_eq!(input.objects.len(), f.volumes, "fixture {seed}");
        }
    }

    #[test]
    fn a_set_over_the_piece_limits_is_cut_into_pieces_that_pass() {
        let f = fixture(1);
        let input = read(&f).unwrap();
        let options = MeshOptions {
            piece_hulls: 40,
            piece_polygons: 60,
            ..MeshOptions::default()
        };
        let built = build_mesh(&input, &options).unwrap();
        assert!(built.pieces.len() > 4, "{}", built.pieces.len());
        let r = check_mesh(&input, &built, Counts::scaled(SHARE), 5).unwrap();
        assert!(r.clean(), "{}\n{:?}", r.line(), r.first);
    }

    #[test]
    fn damage_to_a_piece_is_caught() {
        let f = fixture(0);
        let input = read(&f).unwrap();
        let good = build_mesh(&input, &MeshOptions::default()).unwrap();
        let counts = Counts::scaled(SHARE);
        let r = check_mesh(&input, &good, counts, 1).unwrap();
        assert!(
            r.clean() && r.source_segments > 0 && r.solid_checked > 0,
            "{}",
            r.line()
        );
        let damaged = |f: &dyn Fn(&mut dif::Interior)| {
            let mut b = good.clone();
            f(&mut b.pieces[0].built.dif.details[0]);
            check_mesh(&input, &b, counts, 1).unwrap()
        };
        let r = damaged(&|i| i.texgens.iter_mut().for_each(|t| t.y[3] += 0.01));
        assert!(r.uv_mismatches > 0, "{}", r.line());
        let r = damaged(&|i| {
            i.surfaces.pop();
            i.normal_lightmaps.pop();
            i.alarm_lightmaps.pop();
            i.zones[0].surface_count -= 1;
            i.zone_surfaces.pop();
        });
        assert!(r.surface_misses > 0, "{}", r.line());
        // A hull's plane moved: both the hull and the source checks see it.
        let r = damaged(&|i| {
            let p = i.hulls.hull_plane_indices[0] & !dif::PLANE_FLIPPED;
            i.planes[p as usize].d += 0.25;
        });
        assert!(
            r.ray_mismatches > 0 && r.source_mismatches > 0,
            "{}",
            r.line()
        );
        // The plan's leaves made smaller than the solid: the source checks see it.
        let mut shrunk = good.clone();
        for h in &mut shrunk.plan.hulls {
            for f in &mut h.brush.faces {
                f.plane.d += 0.05;
            }
        }
        let r = check_mesh(&input, &shrunk, counts, 1).unwrap();
        assert!(r.solid_mismatches > 0, "{}", r.line());
    }

    #[test]
    fn a_blockbench_model_builds_with_a_hull_a_cube_and_passes_verification() {
        let face = r#"{"uv":[0,0,16,16]}"#;
        let faces = format!(
            r#"{{"north":{face},"east":{face},"south":{face},"west":{face},"up":{face},"down":{face}}}"#
        );
        let cube = |name: &str, from: [i32; 3], to: [i32; 3], extra: &str| {
            format!(
                r#"{{"name":"{name}","type":"cube","from":{from:?},"to":{to:?},"origin":[8,0,8]{extra},"faces":{faces}}}"#
            )
        };
        let text = format!(
            r#"{{"meta":{{"format_version":"4.10"}},"resolution":{{"width":16,"height":16}},"elements":[{},{},{},{}],
               "outliner":[]}}"#,
            cube("floor", [-64, -8, -64], [64, 0, 64], ""),
            cube("crate", [0, 0, 0], [16, 16, 16], r#","rotation":[0,30,0]"#),
            cube("plinth", [32, 0, 32], [48, 8, 40], r#","inflate":1"#),
            r#"{"name":"wedge","type":"mesh","origin":[-32,0,-32],"vertices":{"a":[0,0,0],"b":[16,0,0],"c":[0,0,16],
               "d":[0,16,0]},"faces":{"f1":{"vertices":["a","b","c"]},"f2":{"vertices":["a","d","b"]},
               "f3":{"vertices":["a","c","d"]},"f4":{"vertices":["b","d","c"]}}}"#,
        );
        let options = torque_import::Options {
            box_hull: false,
            ..torque_import::Options::default()
        };
        let model = torque_import::bbmodel::read(text.as_bytes(), &options, &mut |_| true).unwrap();
        let input = MeshInput::from_model(&model).unwrap();
        let built = build_mesh(&input, &MeshOptions::default()).unwrap();
        // Every cube and the closed wedge (its faces counter-clockwise seen from outside, as Blockbench keeps them) is one closed
        // part, each one hull.
        assert_eq!(built.plan.solids.len(), 4);
        assert_eq!(built.plan.hulls.len(), 4);
        let r = check_mesh(&input, &built, Counts::scaled(SHARE), 3).unwrap();
        assert!(
            r.clean() && r.source_segments > 0,
            "{}
{:?}",
            r.line(),
            r.first
        );
    }
}
