//! The enhanced renderer's reflections and water as plain data: the quality ladder, the water's numbers, and the cameras of the
//! secondary views (the water's mirrored camera and the light capture's six cube faces).
//!
//! - **Ladder** ([`ReflectionQuality`]): screen-space reflections (SSR) off, off, at half and at full resolution; the water's
//!   reflection from the sky only (Low), SSR with the sky behind it (Medium), a planar reflection at half (High) and at full
//!   resolution (Ultra); the camera-local light capture (a cube, one face a frame) at High and Ultra, the sky cube below that.
//! - **Mirrored camera** ([`mirror_camera`]): the main camera's view times the reflection about the water plane `z = level`; it sees
//!   what the main camera would see in a mirror, with the same projection (the same pixels), its eye below the water. A normal camera
//!   at the reflected eye with the pitch negated sees the same picture upside down ([`reflected_eye`]: the planar test's reference).
//! - **Capture faces** ([`cube_face_camera`]): 90-degree square views along the cube's six axes in WebGPU's cube layout (the face's
//!   texel (x right, y down) holds the direction the cube map samples there), so a direction samples what was drawn along it.

use crate::source::Eye;

use crate::hd::{HdCamera, HdScene, Quality};
use crate::math::gl_mul;

/// Where the screen-space reflections are traced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsrResolution {
    Off,
    Half,
    Full,
}

/// What the water reflects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaterReflection {
    /// The sky cube only.
    Sky,
    /// Screen-space reflections, the sky cube where they miss.
    Ssr,
    /// The planar reflection: the opaque scene drawn by the mirrored camera (at half the internal frame's size, or at its size).
    Planar { half: bool },
}

/// A quality's reflections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReflectionQuality {
    pub ssr: SsrResolution,
    pub water: WaterReflection,
    /// The camera-local light capture (else the sky cube is what SSR misses fall back to).
    pub capture: bool,
    /// The most Hi-Z steps a mirror ray takes (fewer for rougher surfaces).
    pub ssr_steps: u32,
}

/// The reflections of a quality.
pub trait QualityReflections {
    fn reflections(self) -> ReflectionQuality;
}

impl QualityReflections for Quality {
    fn reflections(self) -> ReflectionQuality {
        match self {
            Quality::Low => ReflectionQuality {
                ssr: SsrResolution::Off,
                water: WaterReflection::Sky,
                capture: false,
                ssr_steps: 0,
            },
            Quality::Medium => ReflectionQuality {
                ssr: SsrResolution::Off,
                water: WaterReflection::Ssr,
                capture: false,
                ssr_steps: 48,
            },
            Quality::High => ReflectionQuality {
                ssr: SsrResolution::Half,
                water: WaterReflection::Planar { half: true },
                capture: true,
                ssr_steps: 64,
            },
            Quality::Ultra => ReflectionQuality {
                ssr: SsrResolution::Full,
                water: WaterReflection::Planar { half: false },
                capture: true,
                ssr_steps: 96,
            },
        }
    }
}

/// The light capture's face side (texels) and its mip levels (256 down to 1).
pub const CAPTURE_SIZE: u32 = 256;
pub const CAPTURE_MIPS: u32 = 9;
/// The light capture's near plane (units).
pub const CAPTURE_NEAR: f32 = 0.1;

/// The water's roughness (its sun glint).
pub const WATER_ROUGHNESS: f32 = 0.06;
/// The water's reflectance at normal incidence (Schlick; water's index 1.33).
pub const WATER_F0: f32 = 0.02;

/// The water plane's height (the first blended plane's), if the scene has water.
pub fn water_level<G>(scene: &HdScene<G>) -> Option<f32> {
    scene
        .water
        .iter()
        .find_map(|d| d.vertices.first().map(|v| v.position[2]))
}

/// The water's colour (its first vertex's, gamma bytes), if the scene has water.
pub fn water_color<G>(scene: &HdScene<G>) -> Option<[u8; 4]> {
    scene
        .water
        .iter()
        .find_map(|d| d.vertices.first().map(|v| v.color))
}

/// The reflection about the plane `z = level` (column-major): `(x, y, 2 level - z)`.
pub fn mirror_matrix(level: f32) -> [f64; 16] {
    #[rustfmt::skip]
    let m = [
        1.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, -1.0, 0.0,
        0.0, 0.0, 2.0 * f64::from(level), 1.0,
    ];
    m
}

/// The main camera seen in the mirror `z = level`: the view times the reflection (its handedness flipped: front faces turn the
/// other way), the eye reflected, the projection and frustum kept.
pub fn mirror_camera(cam: &HdCamera, level: f32) -> HdCamera {
    let view = gl_mul(&cam.view.map(f64::from), &mirror_matrix(level));
    let view_proj = gl_mul(&cam.proj.map(f64::from), &view);
    HdCamera {
        eye: [cam.eye[0], cam.eye[1], 2.0 * level - cam.eye[2]],
        view: view.map(|v| v as f32),
        view_proj: view_proj.map(|v| v as f32),
        ..*cam
    }
}

/// The eye reflected in the plane `z = level` with its pitch negated: a normal camera there sees the mirrored camera's picture upside
/// down (the planar test's reference).
pub fn reflected_eye(eye: impl Into<Eye>, level: f32) -> Eye {
    let eye: Eye = eye.into();
    Eye {
        position: [
            eye.position[0],
            eye.position[1],
            2.0 * level - eye.position[2],
        ],
        pitch: -eye.pitch,
        ..eye
    }
}

/// A cube face's axes in WebGPU's cube layout (faces +X, -X, +Y, -Y, +Z, -Z): the direction it looks along, the one its texels' x
/// runs along and the one its rows run against (up).
pub fn cube_face_axes(face: usize) -> ([f64; 3], [f64; 3], [f64; 3]) {
    match face {
        0 => ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        1 => ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        2 => ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        3 => ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        4 => ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        _ => ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    }
}

/// The camera that draws cube face `face` from `eye`: 90 degrees, square, `size` texels, reversed depth from `near` to `far`. Its
/// axes are left-handed (front faces turn the other way, as in a mirror).
pub fn cube_face_camera(eye: [f32; 3], face: usize, near: f32, far: f32, size: u32) -> HdCamera {
    let (fwd, right, up) = cube_face_axes(face);
    let e = eye.map(f64::from);
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    // World to eye space: rows right, up, -forward.
    #[rustfmt::skip]
    let view = [
        right[0], up[0], -fwd[0], 0.0,
        right[1], up[1], -fwd[1], 0.0,
        right[2], up[2], -fwd[2], 0.0,
        -dot(right, e), -dot(up, e), dot(fwd, e), 1.0,
    ];
    let (n, f) = (f64::from(near), f64::from(far.max(near * 2.0)));
    #[rustfmt::skip]
    let proj = [
        1.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, n / (f - n), -1.0,
        0.0, 0.0, f * n / (f - n), 0.0,
    ];
    HdCamera {
        eye,
        view: view.map(|v| v as f32),
        proj: proj.map(|v| v as f32),
        view_proj: gl_mul(&proj, &view).map(|v| v as f32),
        frustum: [-near, near, -near, near],
        near,
        far: f as f32,
        width: size,
        height: size,
    }
}

#[cfg(test)]
#[path = "reflect_tests.rs"]
mod tests;
