//! A moving shape dissolving ([`HdScene::dissolves`](crate::hd::HdScene::dissolves)): what the enhanced renderer's passes clip
//! and light by. A surface point is drawn where its noise field ([`field`]) is at or over the dissolve's threshold, its edge
//! glows where the field is under `edge_to`; the field is a function of the point in its object's own space, so the pattern
//! stays on the object as it moves.
//!
//! The field is `shaders/dissolve_common.wgsl`'s `dissolve_field` in torque-render-hd: three octaves of value noise of the
//! point over the cell size, mixed with the point's height share by the sweep. A change here changes the shader in the same
//! way (`the_cpu_field_is_the_shaders` there compares them).

use crate::math::Mat;

/// The threshold of a surface drawn whole, and of one gone (finite: the shaders compare them with finite fields).
pub const WHOLE: f32 = -1.0e30;
pub const GONE: f32 = 1.0e30;

/// One dissolving object as the passes take it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdDissolve {
    /// The object's ghost ([`crate::hd::DynamicObject::ghost`]).
    pub ghost: u32,
    /// The field under which the surface is gone ([`WHOLE`]: none of it, [`GONE`]: all of it).
    pub threshold: f32,
    /// The field under which a drawn point is in the glowing edge (at least `threshold`).
    pub edge_to: f32,
    /// The edge's light (linear, HDR: past 1 the bloom picks it up).
    pub edge_color: [f32; 3],
    /// The noise's cell (object units; at least [`MIN_CELL`]).
    pub cell: f32,
    /// The height share's weight (-1..1: 1 eats from the top down, -1 from the bottom up, 0 noise alone).
    pub sweep: f32,
    /// The object's lowest and highest z in its own space (the height share's span).
    pub z_range: [f32; 2],
    /// The share of the surface eaten (0 whole, 1 gone): what the passes that do not clip go by (a secondary view draws the
    /// object whole under one half and leaves it out over it).
    pub eaten: f32,
}

impl HdDissolve {
    /// The world-to-object matrix the passes take a point into the field's space with: the inverse of `transform` (the
    /// object's, [`crate::hd::DynamicObject::transform`]), column-major.
    #[must_use]
    pub fn to_object(transform: &Mat) -> [f32; 16] {
        let inv = super::post::affine_inverse_scaled(transform);
        let mut out = [0.0f32; 16];
        for r in 0..4 {
            for c in 0..4 {
                out[c * 4 + r] = inv.m[r * 4 + c];
            }
        }
        out
    }

    /// Whether the surface is wholly gone.
    #[must_use]
    pub fn gone(&self) -> bool {
        self.threshold >= GONE
    }
}

/// Whether a renderer that draws no clip (another renderer on the slot, a plain view) leaves ghost `ghost`'s object out: over
/// half of it is eaten, so a dissolve vanishes halfway through and a materialize appears halfway (the engine's own secondary
/// views draw it so too).
#[must_use]
pub fn hidden_unclipped(dissolves: &[HdDissolve], ghost: u32) -> bool {
    dissolves.iter().any(|d| d.ghost == ghost && d.eaten > 0.5)
}

/// The smallest noise cell (object units): a smaller one is aliased noise at any distance.
pub const MIN_CELL: f32 = 0.01;

/// A lattice point's value in `0..1` (an integer hash of its coordinates).
fn lattice(x: i32, y: i32, z: i32) -> f32 {
    // The coordinates' bits as WGSL's bitcast<u32> takes them.
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    (h >> 8) as f32 / 16_777_216.0
}

/// Value noise at `p`: the lattice values about it blended by the smoothstep of its place in its cell.
fn value_noise(p: [f32; 3]) -> f32 {
    let i = p.map(f32::floor);
    let f = [p[0] - i[0], p[1] - i[1], p[2] - i[2]];
    let u = f.map(|t| t * t * (3.0 - 2.0 * t));
    let (x, y, z) = (i[0] as i32, i[1] as i32, i[2] as i32);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let row = |dy: i32, dz: i32| {
        lerp(
            lattice(x, y + dy, z + dz),
            lattice(x + 1, y + dy, z + dz),
            u[0],
        )
    };
    let face = |dz: i32| lerp(row(0, dz), row(1, dz), u[1]);
    lerp(face(0), face(1), u[2])
}

/// The three octaves' sum at `q` (the point over the cell size), in `0..1`.
pub fn noise(q: [f32; 3]) -> f32 {
    let at = |s: f32, o: f32| value_noise(q.map(|c| c * s + o));
    at(1.0, 0.0) * 0.5 + at(2.03, 17.1) * 0.3 + at(4.01, 31.7) * 0.2
}

/// The field at the object-space point `p`: the noise of `p` over `cell`, mixed with the height share by `sweep`.
pub fn field(p: [f32; 3], cell: f32, sweep: f32, z_range: [f32; 2]) -> f32 {
    let n = noise(p.map(|c| c / cell.max(MIN_CELL)));
    let span = z_range[1] - z_range[0];
    let h = if span > 1e-6 {
        ((p[2] - z_range[0]) / span).clamp(0.0, 1.0)
    } else {
        0.5
    };
    let s = sweep.clamp(-1.0, 1.0);
    let ramp = if s > 0.0 { 1.0 - h } else { h };
    n * (1.0 - s.abs()) + ramp * s.abs()
}

#[cfg(test)]
#[path = "dissolve_tests.rs"]
mod tests;
