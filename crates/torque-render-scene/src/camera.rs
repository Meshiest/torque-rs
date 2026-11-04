//! The camera a scene is drawn from (near plane 0.1, the fov in radians as `deg * pi / 180` in
//! floats) and TGE's `GuiTSCtrl::onRender` (the frustum from the horizontal fov, `dglSetFrustum` with the far plane at the scene's
//! visible distance, then the inverse of the camera's transform as the modelview). The Camera's transform is
//! `Camera::setPosition`'s: `EulerF(0, 0, yaw) * EulerF(pitch, 0, 0)` with the position as its last column.
//!
//! Credit: the frustum follows Torque3D's MIT-licensed `gui/3d/guiTSControl.cpp` (Copyright (c) 2012 GarageGames, LLC); the
//! code here is this crate's own.

use crate::source::Eye;

use crate::math::{Mat, gl_mul};

/// The near plane.
pub const NEAR: f32 = 0.1;

/// The camera's numbers before they become matrices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// The Camera object's transform (TGE row-major).
    pub transform: Mat,
    /// Radians, horizontal.
    pub fov: f32,
    pub near: f32,
    pub far: f32,
    /// `glFrustum`'s left, right, bottom, top as `onRender` computes them (floats, then widened).
    pub frustum: [f32; 4],
}

impl Camera {
    /// The camera of `eye` with a field of view of `fov_deg` degrees (a whole one from a connection's heads, or a zooming client's
    /// float), the far plane at `far`, on a `width` x `height` canvas.
    pub fn from_eye(
        eye: impl Into<Eye>,
        fov_deg: impl Into<f32>,
        far: f32,
        width: u32,
        height: u32,
    ) -> Camera {
        let eye: Eye = eye.into();
        let mut transform = Mat::mul(&Mat::euler_z(eye.yaw), &Mat::euler_x(eye.pitch));
        transform.set_column3(eye.position);
        let fov = fov_deg.into() * std::f32::consts::PI / 180.0;
        let near = NEAR;
        // onRender: wwidth = near * tan(fov / 2); wheight = h / w * wwidth; then the update rect (the whole canvas).
        let wwidth = near * (fov / 2.0).tan();
        let wheight = height as f32 / width as f32 * wwidth;
        let hscale = wwidth * 2.0 / width as f32;
        let vscale = wheight * 2.0 / height as f32;
        let left = -wwidth;
        let right = width as f32 * hscale - wwidth;
        let top = wheight;
        let bottom = wheight - vscale * height as f32;
        Camera {
            transform,
            fov,
            near,
            far,
            frustum: [left, right, bottom, top],
        }
    }

    /// The eye's world position.
    pub fn eye(&self) -> [f32; 3] {
        self.transform.column(3)
    }

    /// GL's projection (`glFrustum` times `darkToOGLCoord`) times the modelview (the camera's affine inverse), column-major.
    pub fn view_proj(&self) -> [f64; 16] {
        let [l, r, b, t] = self.frustum.map(f64::from);
        let (n, f) = (f64::from(self.near), f64::from(self.far));
        #[rustfmt::skip]
        let frustum = [
            2.0 * n / (r - l), 0.0, 0.0, 0.0,
            0.0, 2.0 * n / (t - b), 0.0, 0.0,
            (r + l) / (r - l), (t + b) / (t - b), -(f + n) / (f - n), -1.0,
            0.0, 0.0, -2.0 * f * n / (f - n), 0.0,
        ];
        // TGE's y-forward, z-up eye space to GL's: (x, y, z) -> (x, z, -y).
        #[rustfmt::skip]
        let dark_to_ogl = [
            1.0, 0.0, 0.0, 0.0,
            0.0, 0.0, -1.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ];
        // dglMultMatrix hands GL the transpose of a row-major MatrixF: GL's column c is the matrix's row c.
        let inv = self.transform.affine_inverse();
        let mut modelview = [0.0f64; 16];
        for c in 0..4 {
            for r in 0..4 {
                modelview[c * 4 + r] = f64::from(inv.m[r * 4 + c]);
            }
        }
        gl_mul(&gl_mul(&frustum, &dark_to_ogl), &modelview)
    }

    pub fn matrices(&self) -> crate::Camera {
        crate::Camera {
            eye: self.eye(),
            view_proj: self.view_proj().map(|v| v as f32),
            fov: self.fov,
            near: self.near,
            far: self.far,
        }
    }
}

#[cfg(test)]
#[path = "camera_tests.rs"]
mod tests;
