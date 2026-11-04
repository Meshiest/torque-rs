//! A map interior's rays and convexes against brute force: on every sample `.dif`, placed
//! turned, scaled and moved, rays through the hull grid and each hull's planes against every front face of every hull cut into
//! triangles (Möller and Trumbore's test). 100,000 rays a file in a release build (`cargo test -p torque-collide --release --test
//! interior_rays`), fewer in a debug one; every hit within 1e-4 m. The hulls' faces also hold every hull point behind every
//! plane and wind clockwise seen from outside (the polygon lists' order). Skips without the samples.

mod map_samples;

use std::sync::Arc;

use map_samples::{Rng, big_stack, files, moller_trumbore, ray_count, samples};
use torque_collide::map::{InteriorGeom, InteriorShape, xf_matrix};
use torque_collide::math::Box3;

/// Whether the segment's box meets `b`.
fn meets(s: [f64; 3], e: [f64; 3], b: &Box3) -> bool {
    (0..3).all(|k| s[k].min(e[k]) <= f64::from(b.max[k]) && s[k].max(e[k]) >= f64::from(b.min[k]))
}

/// The nearest front-face hit over every hull's faces.
fn brute(shape: &InteriorShape, s: [f64; 3], e: [f64; 3]) -> Option<f64> {
    let dir = [e[0] - s[0], e[1] - s[1], e[2] - s[2]];
    let mut best: Option<f64> = None;
    for k in 0..shape.geom().hulls.len() as u32 {
        let Some(b) = shape.hull_box(k) else { continue };
        if !meets(s, e, &b) {
            continue;
        }
        let pts = shape.hull_points_wide(k);
        for (plane, verts) in shape.hull_faces(k) {
            let n = [plane.x, plane.y, plane.z].map(f64::from);
            if n[0] * dir[0] + n[1] * dir[1] + n[2] * dir[2] >= 0.0 {
                continue;
            }
            for j in 1..verts.len().saturating_sub(1) {
                let tri = [verts[0], verts[j], verts[j + 1]].map(|v| pts[v as usize]);
                if let Some(t) = moller_trumbore(s, dir, tri)
                    && best.is_none_or(|b| t < b)
                {
                    best = Some(t);
                }
            }
        }
    }
    best
}

#[test]
fn interior_rays() {
    let Some(root) = samples() else {
        println!("skipped: TORQUE_MAPS_DIR is not set");
        return;
    };
    big_stack(move || {
        let difs = files(&root, "dif");
        let rays = ray_count(100_000, 400);
        let (mut done, mut hulls, mut faces, mut cast, mut hits, mut mismatches) =
            (0usize, 0, 0, 0usize, 0usize, 0usize);
        let (mut behind_bad, mut winding_bad) = (0usize, 0usize);
        let mut worst = 0.0f64;
        let mut misses = 0usize;
        let mut failing: Vec<String> = Vec::new();
        for (n, path) in difs.iter().enumerate() {
            let Ok(dif) = torque_maps::dif::parse_with_border_pad(&std::fs::read(path).unwrap())
            else {
                panic!("{} does not read", path.display());
            };
            let Some(detail) = dif.details.first() else {
                continue;
            };
            let geom = Arc::new(InteriorGeom::new(&torque_maps::legacy::convexes(detail)));
            // Turned about an axis, scaled unevenly, moved (every third file mirrored in x).
            let a = 0.37 + n as f32 * 0.11;
            let axis = [0.3f32, -0.2, 0.93];
            let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
            let q = axis.map(|v| v / len * (a * 0.5).sin());
            let m = xf_matrix([12.5, -40.25, 3.0], q, n % 4 == 1);
            let scale = if n % 3 == 2 {
                [-1.25, 0.8, 1.1]
            } else {
                [1.25, 0.8, 1.1]
            };
            let Some(shape) = InteriorShape::new(Arc::clone(&geom), m, scale) else {
                panic!("{}: not placed", path.display());
            };
            hulls += geom.hulls.len();
            for k in 0..geom.hulls.len() as u32 {
                let pts = shape.hull_points(k);
                for (plane, verts) in shape.hull_faces(k) {
                    faces += 1;
                    let extent = shape.hull_box(k).map_or(1.0, |b| {
                        (0..3).map(|c| b.max[c] - b.min[c]).fold(1.0f32, f32::max)
                    });
                    if pts.iter().any(|p| plane.dist(*p) > 1e-3 * extent) {
                        behind_bad += 1;
                    }
                    // Newell's normal of the ring as the polygon lists take it (clockwise seen from outside: it points in).
                    let wide = shape.hull_points_wide(k);
                    let mut w = [0.0f64; 3];
                    for j in 0..verts.len() {
                        let a = wide[verts[j] as usize];
                        let b = wide[verts[(j + 1) % verts.len()] as usize];
                        w[0] += (a[1] - b[1]) * (a[2] + b[2]);
                        w[1] += (a[2] - b[2]) * (a[0] + b[0]);
                        w[2] += (a[0] - b[0]) * (a[1] + b[1]);
                    }
                    let agree = -(w[0] * f64::from(plane.x)
                        + w[1] * f64::from(plane.y)
                        + w[2] * f64::from(plane.z));
                    if agree < 0.0 {
                        winding_bad += 1;
                    }
                }
            }
            let wb = shape.world_box();
            let lo = wb.min.map(f64::from);
            let hi = wb.max.map(f64::from);
            let size: f64 = (0..3).map(|k| hi[k] - lo[k]).fold(1.0, f64::max);
            let mut rng = Rng(0x2545_f491_4f6c_dd1d ^ (n as u64) << 7);
            let mut charge = |_: u64| true;
            let mut bad_here = 0usize;
            for _ in 0..rays {
                let pick =
                    |rng: &mut Rng, k: usize| rng.range(lo[k] - 0.1 * size, hi[k] + 0.1 * size);
                let s = [pick(&mut rng, 0), pick(&mut rng, 1), pick(&mut rng, 2)];
                let e = [pick(&mut rng, 0), pick(&mut rng, 1), pick(&mut rng, 2)];
                // Half the rays are short (a mover's reach).
                let e = if rng.next() & 1 == 0 {
                    let r = rng.range(0.1, 8.0)
                        / ((e[0] - s[0]).powi(2) + (e[1] - s[1]).powi(2) + (e[2] - s[2]).powi(2))
                            .sqrt()
                            .max(1e-9);
                    [
                        s[0] + (e[0] - s[0]) * r,
                        s[1] + (e[1] - s[1]) * r,
                        s[2] + (e[2] - s[2]) * r,
                    ]
                } else {
                    e
                };
                let sf = s.map(|v| v as f32);
                let ef = e.map(|v| v as f32);
                let (s, e) = (sf.map(f64::from), ef.map(f64::from));
                let len =
                    ((e[0] - s[0]).powi(2) + (e[1] - s[1]).powi(2) + (e[2] - s[2]).powi(2)).sqrt();
                let got = shape.cast_ray(sf, ef, &mut charge).unwrap();
                let want = brute(&shape, s, e);
                cast += 1;
                let bad = match (got, want) {
                    (Some((tg, _)), Some(tw)) => {
                        hits += 1;
                        let err = (f64::from(tg) - tw).abs() * len;
                        worst = worst.max(err);
                        err > map_samples::tolerance(len)
                    }
                    (None, None) => false,
                    _ => {
                        misses += 1;
                        true
                    }
                };
                if bad {
                    if mismatches < 8 {
                        println!(
                            "  {} (file {n}): {sf:?} to {ef:?}: hulls {:?}, brute force {:?}",
                            path.display(),
                            got,
                            want
                        );
                        if std::env::var_os("TORQUE_MAP_DEBUG").is_some() {
                            for k in 0..geom.hulls.len() {
                                let one = InteriorGeom::single(&geom, k);
                                let s1 = InteriorShape::new(Arc::new(one), m, scale).unwrap();
                                let a = s1.cast_ray(sf, ef, &mut charge).unwrap();
                                let b = brute(&s1, s, e);
                                if a.map(|x| x.0).is_some() || b.is_some() {
                                    println!("    hull {k}: {a:?} {b:?} {:?}", geom.hulls[k]);
                                }
                            }
                        }
                    }
                    mismatches += 1;
                    bad_here += 1;
                }
            }
            if bad_here > 0 && failing.len() < 16 {
                failing.push(format!("{} ({bad_here})", path.display()));
            }
            done += 1;
        }
        println!(
            "interior_rays: {done} files, {hulls} hulls, {faces} faces, {cast} rays ({hits} hits), {mismatches} mismatches \
             (worst {worst:.2e} m, {misses} a hit against a miss), {behind_bad} faces with points in front, {winding_bad} faces wound the wrong way"
        );
        for f in &failing {
            println!("  mismatches in {f}");
        }
        assert!(done >= 1, "the sample interiors are there ({done})");
        assert_eq!(mismatches, 0);
        assert_eq!(behind_bad, 0);
        assert_eq!(winding_bad, 0);
    });
}
