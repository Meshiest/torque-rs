//! A map terrain's rays against brute force: on every sample `.ter`, rays walked over the grid
//! against every triangle of every square under the segment (Möller and Trumbore's test), and vertical drops against the height
//! of the square's two triangles as drawing cuts them (`torque_maps::ter::split45`, interpolated directly). 100,000 rays and 10,000
//! drops a file in a release build (`cargo test -p torque-collide --release --test terrain_heightfield`), fewer in a debug one; every
//! hit within 1e-4 m. Skips without the samples.

mod map_samples;

use std::sync::Arc;

use map_samples::{Rng, big_stack, files, moller_trumbore, ray_count, samples};
use torque_collide::map::{HeightField, TerrainShape};
use torque_mapcore::budget::HEIGHTFIELD_SIDE as SIZE;
use torque_mapcore::collision::HeightfieldPlacement;

fn split45(x: usize, y: usize) -> bool {
    torque_mapcore::collision::Split::Alternating.main_diagonal(x, y)
}

/// The height of `ter` at grid point (gx, gy) of square (x, y): the triangle of the cut it is in, interpolated.
fn drawn_height(t: &TerrainShape, x: i64, y: i64, fx: f64, fy: f64) -> f64 {
    let h = |dx: i64, dy: i64| f64::from(t.corner(x + dx, y + dy)[2]);
    let w = |v: i64| v.rem_euclid(SIZE as i64) as usize;
    if split45(w(x), w(y)) {
        if fx > fy {
            h(0, 0) + fx * (h(1, 0) - h(0, 0)) + fy * (h(1, 1) - h(1, 0))
        } else {
            h(0, 0) + fy * (h(0, 1) - h(0, 0)) + fx * (h(1, 1) - h(0, 1))
        }
    } else if fx + fy < 1.0 {
        h(0, 0) + fx * (h(1, 0) - h(0, 0)) + fy * (h(0, 1) - h(0, 0))
    } else {
        h(1, 1) + (1.0 - fx) * (h(0, 1) - h(1, 1)) + (1.0 - fy) * (h(1, 0) - h(1, 1))
    }
}

/// The nearest front hit of the segment over every triangle of the squares its box covers.
fn brute(t: &TerrainShape, s: [f64; 3], e: [f64; 3]) -> Option<f64> {
    let dir = [e[0] - s[0], e[1] - s[1], e[2] - s[2]];
    let sq = f64::from(t.square());
    let o = t.corner(0, 0).map(f64::from);
    let cell = |v: f64, k: usize| ((v - o[k]) / sq).floor() as i64;
    let (x0, x1) = (cell(s[0].min(e[0]), 0) - 1, cell(s[0].max(e[0]), 0) + 1);
    let (y0, y1) = (cell(s[1].min(e[1]), 1) - 1, cell(s[1].max(e[1]), 1) + 1);
    let mut best: Option<f64> = None;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if !t.has_square(x, y) {
                continue;
            }
            for half in 0..2u8 {
                let tri = t.triangle(x, y, half).map(|v| v.map(f64::from));
                let a = [
                    tri[1][0] - tri[0][0],
                    tri[1][1] - tri[0][1],
                    tri[1][2] - tri[0][2],
                ];
                let b = [
                    tri[2][0] - tri[0][0],
                    tri[2][1] - tri[0][1],
                    tri[2][2] - tri[0][2],
                ];
                let nz = a[0] * b[1] - a[1] * b[0];
                let n = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], nz];
                if n[0] * dir[0] + n[1] * dir[1] + n[2] * dir[2] >= 0.0 {
                    continue;
                }
                if let Some(h) = moller_trumbore(s, dir, tri)
                    && best.is_none_or(|b| h < b)
                {
                    best = Some(h);
                }
            }
        }
    }
    best
}

#[test]
fn terrain_heightfield() {
    let Some(root) = samples() else {
        println!("skipped: TORQUE_MAPS_DIR is not set");
        return;
    };
    big_stack(move || {
        let ters = files(&root, "ter");
        let rays = ray_count(100_000, 3_000);
        let drops = ray_count(10_000, 1_000).min(10_000);
        let (mut files_done, mut cast, mut hits, mut mismatches, mut dropped, mut drop_bad) =
            (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
        let mut worst = 0.0f64;
        for (n, path) in ters.iter().enumerate() {
            use torque_mapcore::Importer;
            let Ok(ter) = torque_maps::legacy::IMPORTER.heightfield(&std::fs::read(path).unwrap())
            else {
                panic!("{} does not read", path.display());
            };
            let field = Arc::new(HeightField::new(&ter));
            // Every other file repeats (its rays cross the block's edges and wrap), with a hole run.
            let repeat = n % 2 == 1;
            let t = TerrainShape::new(
                Arc::clone(&field),
                &HeightfieldPlacement::new(
                    torque_maps::legacy::terrain_origin(8),
                    3.5,
                    8,
                    repeat,
                    torque_maps::ter::empty_squares(&[5 | 7 << 8 | 4 << 16]),
                ),
            );
            let span = 8.0 * SIZE as f64;
            let o = t.corner(0, 0).map(f64::from);
            let (zl, zh) = (f64::from(field.min()) + 3.5, f64::from(field.max()) + 3.5);
            let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ n as u64);
            let mut charge = |_: u64| true;
            for _ in 0..rays {
                let margin = if repeat { span * 0.25 } else { 40.0 };
                let s = [
                    rng.range(o[0] - margin, o[0] + span + margin),
                    rng.range(o[1] - margin, o[1] + span + margin),
                    rng.range(zl - 5.0, zh + 30.0),
                ];
                let reach = rng.range(0.5, 24.0 * 8.0);
                let a = rng.range(0.0, std::f64::consts::TAU);
                let down = rng.range(-1.0, 0.4);
                let e = [
                    s[0] + reach * a.cos(),
                    s[1] + reach * a.sin(),
                    s[2] + down * rng.range(1.0, 120.0),
                ];
                let sf = s.map(|v| v as f32);
                let ef = e.map(|v| v as f32);
                let (s, e) = (sf.map(f64::from), ef.map(f64::from));
                let len =
                    ((e[0] - s[0]).powi(2) + (e[1] - s[1]).powi(2) + (e[2] - s[2]).powi(2)).sqrt();
                let got = t.cast_ray(sf, ef, &mut charge).unwrap();
                let want = brute(&t, s, e);
                cast += 1;
                match (got, want) {
                    (Some((tg, nrm)), Some(tw)) => {
                        hits += 1;
                        let err = (f64::from(tg) - tw).abs() * len;
                        worst = worst.max(err);
                        if err > map_samples::tolerance(len) || nrm[2] <= 0.0 {
                            mismatches += 1;
                        }
                    }
                    (None, None) => {}
                    _ => mismatches += 1,
                }
            }
            for _ in 0..drops {
                let x = rng.range(o[0], o[0] + span) as f32;
                let y = rng.range(o[1], o[1] + span) as f32;
                let gx = (f64::from(x) - o[0]) / 8.0;
                let gy = (f64::from(y) - o[1]) / 8.0;
                let (ix, iy) = (gx.floor() as i64, gy.floor() as i64);
                if !t.has_square(ix, iy) {
                    continue;
                }
                dropped += 1;
                let want = drawn_height(&t, ix, iy, gx - ix as f64, gy - iy as f64);
                let top = (zh + 10.0) as f32;
                let bottom = (zl - 10.0) as f32;
                let hit = t
                    .cast_ray([x, y, top], [x, y, bottom], &mut charge)
                    .unwrap();
                let got = hit.map(|(h, _)| {
                    f64::from(top) + f64::from(h) * (f64::from(bottom) - f64::from(top))
                });
                if got.is_none_or(|g| (g - want).abs() > 1e-4) {
                    drop_bad += 1;
                }
            }
            files_done += 1;
        }
        println!(
            "terrain_heightfield: {files_done} files, {cast} rays ({hits} hits), {mismatches} mismatches (worst {worst:.2e} m), \
             {dropped} drops, {drop_bad} off the drawn height"
        );
        assert!(
            files_done >= 1,
            "the sample terrains are there ({files_done})"
        );
        assert_eq!(mismatches, 0);
        assert_eq!(drop_bad, 0);
    });
}
