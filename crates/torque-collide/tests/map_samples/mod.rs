//! The sample maps (the directory named by `TORQUE_MAPS_DIR`; the tests skip when it is not set); a seeded generator; the
//! brute-force triangle test the map collision is checked against.

use std::path::{Path, PathBuf};

pub fn samples() -> Option<PathBuf> {
    let d = std::env::var("TORQUE_MAPS_DIR").ok()?;
    Some(PathBuf::from(d)).filter(|p| p.is_dir())
}

/// Every file under `dir` with the extension `ext` (any case), sorted.
pub fn files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut all = Vec::new();
    walk(dir, &mut all);
    all.retain(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)));
    all
}

/// xorshift64*.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in `[lo, hi)`.
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * ((self.next() >> 11) as f64 / (1u64 << 53) as f64)
    }
}

/// Möller and Trumbore's segment test (two-sided): the fraction along `s + t * dir` (0..1) where it meets the triangle.
pub fn moller_trumbore(s: [f64; 3], dir: [f64; 3], tri: [[f64; 3]; 3]) -> Option<f64> {
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let e1 = sub(tri[1], tri[0]);
    let e2 = sub(tri[2], tri[0]);
    let p = cross(dir, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-300 {
        return None;
    }
    let inv = 1.0 / det;
    let tv = sub(s, tri[0]);
    let u = dot(tv, p) * inv;
    if !(-1e-12..=1.0 + 1e-12).contains(&u) {
        return None;
    }
    let q = cross(tv, e1);
    let v = dot(dir, q) * inv;
    if v < -1e-12 || u + v > 1.0 + 1e-12 {
        return None;
    }
    let t = dot(e2, q) * inv;
    (0.0..=1.0).contains(&t).then_some(t)
}

/// Runs `f` on a thread with a large stack (the debug build's frames).
pub fn big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

/// The rays a check casts: `full` in a release build, `debug` in a debug one (`TORQUE_MAP_RAYS` overrides).
pub fn ray_count(full: usize, debug: usize) -> usize {
    if let Some(n) = std::env::var("TORQUE_MAP_RAYS")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        return n;
    }
    if cfg!(debug_assertions) { debug } else { full }
}

/// How far two hits along a segment `len` metres long may be apart: 1e-4 m, plus two of the float fraction's steps near 1 (a
/// ray's answer is a float fraction, like `RayInfo::t`).
pub fn tolerance(len: f64) -> f64 {
    1e-4 + len * 2.0 * f64::from(f32::EPSILON)
}
