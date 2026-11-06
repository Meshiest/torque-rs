//! The enhanced renderer's camera: the fixed-function camera's field of view, near and far planes and frustum
//! ([`crate::camera::Camera`]), with the view and projection kept apart and the depth reversed (near 1, far 0: a 32-bit float depth
//! keeps its precision far away). Matrices are column-major, world to GL eye space (x right, y up, looking down -z) and on to clip
//! space with wgpu's depth range.

use crate::source::Eye;

use crate::camera::Camera;
use glam::{Vec3, Vec4};

use crate::math::gl_mul;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdCamera {
    pub eye: [f32; 3],
    /// World to eye space.
    pub view: [f32; 16],
    /// Eye to clip space, reversed depth.
    pub proj: [f32; 16],
    pub view_proj: [f32; 16],
    /// The frustum at the near plane (`glFrustum`'s left, right, bottom, top) and the planes.
    pub frustum: [f32; 4],
    pub near: f32,
    pub far: f32,
    pub width: u32,
    pub height: u32,
}

impl HdCamera {
    /// The camera of `eye` (fov `fov_deg` degrees, horizontal) with the far plane at `far`, for `width` x `height` pixels.
    pub fn new(
        eye: impl Into<Eye>,
        fov_deg: impl Into<f32>,
        far: f32,
        width: u32,
        height: u32,
    ) -> HdCamera {
        HdCamera::of(
            &Camera::from_eye(eye, fov_deg, far, width, height),
            width,
            height,
        )
    }

    pub fn of(c: &Camera, width: u32, height: u32) -> HdCamera {
        let [l, r, b, t] = c.frustum.map(f64::from);
        let (n, f) = (f64::from(c.near), f64::from(c.far));
        // glFrustum with the depth row turned to 0.5 (w - z): n / (f - n) z + f n / (f - n).
        #[rustfmt::skip]
        let proj = [
            2.0 * n / (r - l), 0.0, 0.0, 0.0,
            0.0, 2.0 * n / (t - b), 0.0, 0.0,
            (r + l) / (r - l), (t + b) / (t - b), n / (f - n), -1.0,
            0.0, 0.0, f * n / (f - n), 0.0,
        ];
        #[rustfmt::skip]
        let dark_to_ogl = [
            1.0, 0.0, 0.0, 0.0,
            0.0, 0.0, -1.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ];
        let inv = c.transform.affine_inverse();
        let mut modelview = [0.0f64; 16];
        for col in 0..4 {
            for row in 0..4 {
                modelview[col * 4 + row] = f64::from(inv.m[row * 4 + col]);
            }
        }
        let view = gl_mul(&dark_to_ogl, &modelview);
        HdCamera {
            eye: c.eye(),
            view: view.map(|v| v as f32),
            proj: proj.map(|v| v as f32),
            view_proj: gl_mul(&proj, &view).map(|v| v as f32),
            frustum: c.frustum,
            near: c.near,
            far: c.far,
            width,
            height,
        }
    }

    /// Where world point `p` lands: pixel x, y (top left 0, 0) and the reversed depth; `None` behind the eye.
    pub fn project(&self, p: [f32; 3]) -> Option<(f32, f32, f32)> {
        let m = &self.view_proj;
        let c: Vec<f32> = (0..4)
            .map(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r])
            .collect();
        if c[3] <= 0.0 {
            return None;
        }
        Some((
            (c[0] / c[3] + 1.0) * 0.5 * self.width as f32,
            (1.0 - c[1] / c[3]) * 0.5 * self.height as f32,
            c[2] / c[3],
        ))
    }

    /// The world direction (unit) of the ray through pixel `(x, y)` (top left 0, 0; a pixel's centre at + 0.5).
    pub fn ray(&self, x: f32, y: f32) -> [f32; 3] {
        let [l, r, b, t] = self.frustum;
        let e = [
            l + (r - l) * x / self.width as f32,
            t - (t - b) * y / self.height as f32,
            -self.near,
        ];
        // The view's rotation transposed: eye axis k in world is the view's row k.
        let v = &self.view;
        let e = Vec3::from(e);
        let d = Vec3::from_array(
            [0, 1, 2].map(|k| Vec3::new(v[k * 4], v[k * 4 + 1], v[k * 4 + 2]).dot(e)),
        );
        (d / d.length().max(1e-12)).to_array()
    }

    /// The six frustum planes (`a x + b y + c z + d >= 0` inside), from the view-projection (Gribb and Hartmann 2001).
    pub fn planes(&self) -> [[f32; 4]; 6] {
        let m = &self.view_proj;
        let row = |r: usize| Vec4::new(m[r], m[4 + r], m[8 + r], m[12 + r]);
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
        // Reversed depth in [0, 1]: z >= 0 is the far side, w - z >= 0 the near.
        [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2].map(|p| p.to_array())
    }

    /// Whether a box may be seen (not wholly outside a frustum plane).
    pub fn sees_box(&self, lo: [f32; 3], hi: [f32; 3]) -> bool {
        self.planes().iter().all(|p| {
            let x = if p[0] >= 0.0 { hi[0] } else { lo[0] };
            let y = if p[1] >= 0.0 { hi[1] } else { lo[1] };
            let z = if p[2] >= 0.0 { hi[2] } else { lo[2] };
            p[0] * x + p[1] * y + p[2] * z + p[3] >= 0.0
        })
    }
}

#[cfg(test)]
#[path = "camera_tests.rs"]
mod tests;
