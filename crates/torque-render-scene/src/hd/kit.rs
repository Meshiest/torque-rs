//! Small helpers for building check scenes: an eye looking at a point, a deterministic random generator and a box object.

use crate::hd::{DynamicObject, HdDraw, HdVertex, material, oct_encode};
use crate::source::Eye;
use crate::{Blend, Cull};

/// The eye at `position` looking at `target` (TGE's Camera angles: yaw about z from +y, pitch down positive).
pub fn look_at(position: [f32; 3], target: [f32; 3]) -> Eye {
    let d = [
        target[0] - position[0],
        target[1] - position[1],
        target[2] - position[2],
    ];
    let h = (d[0] * d[0] + d[1] * d[1]).sqrt();
    Eye {
        position,
        pitch: (-d[2]).atan2(h),
        yaw: -(-d[0]).atan2(d[1]),
        fov: Some(90),
    }
}

/// A small deterministic generator (random lights and samples for checks).
pub struct Lcg(u64);

impl Lcg {
    pub fn new(seed: u64) -> Lcg {
        Lcg(seed ^ 0x9e37_79b9_7f4a_7c15)
    }
    /// A float in [0, 1).
    pub fn unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
}

/// A box shape (a stand-in for a person) of half sides `h` at `centre`, opaque.
pub fn box_object(ghost: u32, centre: [f32; 3], h: [f32; 3]) -> DynamicObject {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let mut n = [0.0f32; 3];
            n[axis] = sign;
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let base = vertices.len() as u32;
            for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let mut p = centre;
                p[axis] += sign * h[axis];
                p[u] += a * h[u];
                p[v] += b * h[v];
                vertices.push(HdVertex {
                    position: p,
                    normal: oct_encode(n),
                    uv: [0.0; 2],
                    color: [200, 160, 60, 255],
                    material: material::KIND_SHAPE << material::KIND_SHIFT,
                    extra: 0,
                });
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    let r = (h[0] * h[0] + h[1] * h[1] + h[2] * h[2]).sqrt();
    DynamicObject {
        ghost,
        previous: None,
        transform: crate::math::Mat::IDENTITY,
        prev_transform: crate::math::Mat::IDENTITY,
        translucent: false,
        centre,
        radius: r,
        draws: vec![HdDraw {
            label: "box shape".into(),
            vertices,
            indices,
            texture: None,
            blend: Blend::Opaque,
            cull: Cull::None,
            depth_test: true,
            depth_write: true,
            pbr: None,
        }],
        stamp: None,
        nodes: Vec::new(),
    }
}
