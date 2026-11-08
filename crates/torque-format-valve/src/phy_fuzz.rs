//! The hostile-input fuzz for physics models: seeded mutations of small synthetic files (none from a game), each given to the
//! reader and to what is made from its result, under `catch_unwind`; 0 panics. The mutations are the crate's (byte flips, cuts,
//! insertions, repeats, 32-bit fields set to edge values, overlapping `(count, offset)` pairs) and three of this format's: a
//! 28-byte tree node, a 16-byte ledge header or a 16-byte triangle copied over another place, so nodes, ledges and triangles read
//! the same bytes many times. `TORQUE_FUZZ_VALVE` sets the count (20,000 by default).

use std::panic::{AssertUnwindSafe, catch_unwind};

use super::synth;
use crate::collision::{ModelCollision, model_space_hulls};
use crate::fuzz::{Rng, mutate};
use crate::hull::{Hull, convex_hull};
use crate::mdl::Mdl;
use crate::phy::{self, Phy};
use crate::synth as models;

const CASES: usize = 20_000;

fn cases() -> usize {
    std::env::var("TORQUE_FUZZ_VALVE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(CASES)
}

fn seeds() -> Vec<Vec<u8>> {
    let pieces = vec![
        synth::box_points([-8.0, -8.0, 0.0], [8.0, 8.0, 16.0]),
        synth::wedge(),
        synth::box_points([10.0; 3], [14.0; 3]),
        synth::box_points([-30.0; 3], [-20.0; 3]),
    ];
    vec![
        synth::two_pieces(),
        synth::ragdoll(),
        synth::file(&[synth::legacy(&pieces)], synth::TEXT),
        synth::file(
            &[
                synth::modern(0, &pieces),
                synth::modern(1, &pieces),
                synth::modern(0, &pieces[..1]),
            ],
            synth::TEXT,
        ),
    ]
}

/// One mutation of `seed`: the crate's, then sometimes a block copied over another place.
fn mutate_phy(seed: &[u8], r: &mut Rng) -> Vec<u8> {
    let mut b = mutate(seed, r);
    for _ in 0..r.below(3) {
        let len = [28, 16, 16][r.below(3)];
        if b.len() < len + 4 {
            break;
        }
        let from = r.below(b.len() - len);
        let to = r.below(b.len() - len);
        let block = b[from..from + len].to_vec();
        b[to..to + len].copy_from_slice(&block);
    }
    b
}

/// Everything made from a file: its pieces in a model's space, its fallbacks.
fn use_all(b: &[u8], mdl: &Mdl) {
    if let Ok(p) = Phy::parse(b) {
        assert!(p.solids.len() <= phy::MAX_SOLIDS);
        let ledges: usize = p.solids.iter().map(|s| s.ledges.len()).sum();
        assert!(ledges <= phy::MAX_LEDGES);
        for s in &p.solids {
            for l in &s.ledges {
                assert!(l.points.iter().flatten().all(|v| v.is_finite()));
                assert!(!l.hull.planes.is_empty());
            }
        }
        let _ = model_space_hulls(&p, mdl);
        let _ = ModelCollision::from_parts(Some(&p), None, mdl, &mut || None);
        let _ = (p.props(0), p.hulls().count());
    }
}

#[test]
fn fuzz_physics_files_without_a_panic() {
    let seeds = seeds();
    let mdl = Mdl::parse(&models::mdl(8)).unwrap();
    for s in &seeds {
        assert!(Phy::parse(s).is_ok());
        use_all(s, &mdl);
    }
    let mut r = Rng::new(0x9a17_c011_1de5);
    let mut panics = Vec::new();
    let started = std::time::Instant::now();
    let (mut read, mut refused) = (0usize, 0usize);
    for case in 0..cases() {
        let b = mutate_phy(&seeds[case % seeds.len()], &mut r);
        if catch_unwind(AssertUnwindSafe(|| use_all(&b, &mdl))).is_err() {
            panics.push(case);
        }
        if Phy::parse(&b).is_ok() {
            read += 1;
        } else {
            refused += 1;
        }
    }
    println!(
        "{} cases in {:.1} s: {read} read, {refused} refused, {} panics",
        cases(),
        started.elapsed().as_secs_f32(),
        panics.len()
    );
    assert!(
        panics.is_empty(),
        "{} panics: {:?}",
        panics.len(),
        &panics[..panics.len().min(10)]
    );
    // The mutations must still reach the reader's work, not only its first check.
    assert!(
        read > cases() / 10,
        "only {read} of {} mutants read",
        cases()
    );
}

#[test]
fn fuzz_the_hull_builder_without_a_panic() {
    let mut r = Rng::new(0x4811_0770_ab1e);
    let specials = [
        0.0f32,
        1.0,
        -1.0,
        1e-30,
        1e30,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::MIN_POSITIVE,
    ];
    let mut hulls = 0;
    for case in 0..cases() {
        let n = r.below(40);
        let kind = r.below(4);
        let pts: Vec<[f32; 3]> = (0..n)
            .map(|_| {
                let mut p = [0f32; 3];
                for v in &mut p {
                    *v = match kind {
                        0 => (r.below(2001) as f32 - 1000.0) * 0.01,
                        1 => specials[r.below(specials.len())],
                        2 => (r.below(5) as f32) * 0.5,
                        _ => {
                            if r.below(16) == 0 {
                                specials[r.below(specials.len())]
                            } else {
                                r.below(100) as f32 - 50.0
                            }
                        }
                    };
                }
                p
            })
            .collect();
        let mut work = 1 << 16;
        let out = catch_unwind(AssertUnwindSafe(|| convex_hull(&pts, &mut work)));
        let Ok(h) = out else {
            panic!("the hull builder panicked on case {case}: {pts:?}");
        };
        if let Some(h) = h {
            hulls += 1;
            check(&h, &pts);
        }
    }
    println!("{hulls} of {} point sets had a hull", cases());
    assert!(hulls > cases() / 20);
}

/// A hull holds every finite point it was made from (within a tolerance of its size).
fn check(h: &Hull, pts: &[[f32; 3]]) {
    let finite: Vec<[f32; 3]> = pts
        .iter()
        .copied()
        .filter(|p| p.iter().all(|v| v.is_finite()))
        .collect();
    let (lo, hi) = crate::hull::bounds(&finite);
    let extent = (0..3).map(|k| hi[k] - lo[k]).fold(0.0f32, f32::max);
    let tol = 1e-4 * extent.max(1.0);
    for p in &finite {
        assert!(h.contains(*p, tol), "{p:?} outside {h:?}");
    }
    assert!(h.triangles.len() >= 4);
    assert!(h.triangle_planes.len() == h.triangles.len());
}
