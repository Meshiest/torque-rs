// ANCHOR: sim
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::data::RopeData;

/// The most ropes at once: a script that makes ropes in a loop stops here.
pub const MAX_ROPES: usize = 64;

/// A chain of points held at both ends, moved by gravity and kept to its length (position-based dynamics).
pub struct Rope {
    pub points: Vec<[f32; 3]>,
    before: Vec<[f32; 3]>,
    piece: f32,
    /// Where the first and the last point are held.
    ends: [[f32; 3]; 2],
    pub color: [u8; 4],
}

impl Rope {
    pub fn new(data: &RopeData, start: [f32; 3], end: [f32; 3]) -> Rope {
        // The data is clamped by its field ranges, but a rope still checks what it divides by.
        let segments = data.segments.clamp(2, 64) as usize;
        let points: Vec<[f32; 3]> = (0..=segments)
            .map(|i| {
                let t = i as f32 / segments as f32;
                [0, 1, 2].map(|k| start[k] + (end[k] - start[k]) * t)
            })
            .collect();
        Rope {
            before: points.clone(),
            points,
            piece: data.length.max(0.01) / segments as f32,
            ends: [start, end],
            color: data.color,
        }
    }

    /// Runs on by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        let last = self.points.len() - 1;
        for i in 1..last {
            let p = self.points[i];
            let moved = [0, 1, 2].map(|k| p[k] + (p[k] - self.before[i][k]) * 0.995);
            self.before[i] = p;
            self.points[i] = [moved[0], moved[1], moved[2] - 9.81 * dt * dt];
        }
        self.points[0] = self.ends[0];
        self.points[last] = self.ends[1];
        for _ in 0..16 {
            for i in 0..last {
                let (a, b) = (self.points[i], self.points[i + 1]);
                let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                let pull = (len - self.piece) / len * 0.5;
                // The held points do not move; the other end of their piece takes all of the correction.
                let wa = if i == 0 { 0.0 } else { 2.0 };
                let wb = if i + 1 == last { 0.0 } else { 2.0 };
                let (push_a, push_b) = (d.map(|c| c * pull * wa), d.map(|c| c * pull * wb));
                for (k, (a, b)) in push_a.iter().zip(&push_b).enumerate() {
                    self.points[i][k] += a;
                    self.points[i + 1][k] -= b;
                }
            }
        }
    }
}

/// The ropes of the engine, shared by the natives, the tick and the render pass. A render pass must be `Send` on native targets, so
/// the ropes sit behind an `Arc<Mutex<..>>`.
#[derive(Clone, Default)]
pub struct Ropes(Arc<Mutex<Vec<Rope>>>);

impl Ropes {
    pub fn lock(&self) -> MutexGuard<'_, Vec<Rope>> {
        // A panic in one holder must not take every rope down with it.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
// ANCHOR_END: sim
