//! The enhanced renderer's anti-aliasing and ambient occlusion as plain data:
//! the quality ladder ([`PostQuality`]), the TAA's
//! sub-pixel jitter (Halton(2, 3), 8 phases), the internal (TAAU) size, the jittered camera and a shape's motion between two frames.
//!
//! **The ladder**: Low draws at 75 % with our FXAA-style edge filter and no AO; Medium at 85 % with TAAU and SSAO (8 taps,
//! half resolution); High at 100 % with TAA and GTAO at half resolution; Ultra at 100 % with TAA, the contrast-adaptive sharpen and
//! GTAO at full resolution.
//!
//! **The jitter**: frame `k` of a history samples at the pixel's centre plus (`halton(k mod 8 + 1, 2)` less 1/2, `halton(k mod 8 + 1, 3)`
//! less 1/2) pixels (x right, y down); the count starts at 0 on every cut, so a frame after the same number of warm-up frames is the same
//! frame.

use crate::hd::{DynamicObject, HdCamera, Quality};
use crate::math::Mat;

/// The TAA's jitter phases.
pub const JITTER_PHASES: u64 = 8;
/// The camera moving farther than this between two frames (units) is a cut: the histories start again.
pub const CUT_DISTANCE: f32 = 32.0;
/// The ambient occlusion's reach (world units).
pub const AO_RADIUS: f32 = 0.5;
/// GTAO's slices and steps a side (Jimenez et al. 2016), and SSAO's taps.
pub const GTAO_SLICES: u32 = 3;
pub const GTAO_STEPS: u32 = 6;
pub const SSAO_TAPS: u32 = 8;

/// How the frame is anti-aliased.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AntiAliasing {
    /// Our own FXAA-style edge filter on the frame (no history, no jitter).
    Fxaa,
    /// Temporal anti-aliasing (with the render scale below 1: TAAU, the history at the output's size).
    Taa,
}

/// Which ambient occlusion the frame has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AoMethod {
    Off,
    /// Normal-oriented hemisphere SSAO, [`SSAO_TAPS`] taps.
    Ssao,
    /// Ground-truth AO (horizon based), [`GTAO_SLICES`] slices of [`GTAO_STEPS`] steps a side.
    Gtao,
}

/// One quality's anti-aliasing and AO.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PostQuality {
    /// The internal frame's size as a share of the output's.
    pub render_scale: f32,
    pub aa: AntiAliasing,
    /// The contrast-adaptive sharpen after the TAA.
    pub sharpen: bool,
    pub ao: AoMethod,
    /// The AO at the internal frame's full resolution (else at half, upsampled by depth).
    pub ao_full_res: bool,
}

impl Quality {
    /// Low 75 % FXAA-style, no AO; Medium 85 % TAAU, SSAO; High 100 % TAA, GTAO half; Ultra 100 % TAA + sharpen, GTAO full.
    pub fn post(self) -> PostQuality {
        match self {
            Quality::Low => PostQuality {
                render_scale: 0.75,
                aa: AntiAliasing::Fxaa,
                sharpen: false,
                ao: AoMethod::Off,
                ao_full_res: false,
            },
            Quality::Medium => PostQuality {
                render_scale: 0.85,
                aa: AntiAliasing::Taa,
                sharpen: false,
                ao: AoMethod::Ssao,
                ao_full_res: false,
            },
            Quality::High => PostQuality {
                render_scale: 1.0,
                aa: AntiAliasing::Taa,
                sharpen: false,
                ao: AoMethod::Gtao,
                ao_full_res: false,
            },
            Quality::Ultra => PostQuality {
                render_scale: 1.0,
                aa: AntiAliasing::Taa,
                sharpen: true,
                ao: AoMethod::Gtao,
                ao_full_res: true,
            },
        }
    }
}

/// The radical inverse of `index` in `base` (Halton's sequence; index 0 is 0).
pub fn halton(mut index: u64, base: u64) -> f32 {
    let mut f = 1.0f64;
    let mut r = 0.0f64;
    let b = base.max(2) as f64;
    while index > 0 {
        f /= b;
        r += f * (index % base.max(2)) as f64;
        index /= base.max(2);
    }
    r as f32
}

/// Frame `frame`'s sub-pixel offset (pixels, x right and y down, each in (-1/2, 1/2)).
pub fn jitter(frame: u64) -> [f32; 2] {
    let i = frame % JITTER_PHASES + 1;
    [halton(i, 2) - 0.5, halton(i, 3) - 0.5]
}

/// The most internal pixels a TAA frame draws (1920 x 1080's): a larger output is drawn at the scale that holds this many
/// and the TAA's history brings it up to the output's size (TAAU), as Medium does at every size. 1080p and below are unchanged.
pub const MAX_TAA_INTERNAL_PIXELS: f64 = 1920.0 * 1080.0;

impl PostQuality {
    /// The render scale for an output of `width` x `height`: the quality's, at most what holds
    /// [`MAX_TAA_INTERNAL_PIXELS`] with the TAA.
    pub fn render_scale_at(&self, width: u32, height: u32) -> f32 {
        let px = f64::from(width) * f64::from(height);
        if self.aa != AntiAliasing::Taa || px <= MAX_TAA_INTERNAL_PIXELS {
            return self.render_scale;
        }
        self.render_scale
            .min((MAX_TAA_INTERNAL_PIXELS / px).sqrt() as f32)
    }
}

/// The window's render scale (the caller's setting): how many of the output's pixels the enhanced renderer draws before the
/// anti-aliasing scales them up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RenderScale {
    /// The quality's own scale and no more upscaling (High and Ultra draw every pixel of the window, Medium 85 %, Low 75 %): the
    /// window's default, for a GPU with room to spare.
    Native,
    /// The quality's scale, and with the TAA at most [`MAX_TAA_INTERNAL_PIXELS`] drawn (TAAU above 1080p's pixels: for a weaker GPU).
    Auto,
    /// This share of the output's pixels each way at every quality ([`RenderScale::MIN`] to 1).
    Fixed(f32),
}

impl RenderScale {
    /// The least fixed scale.
    pub const MIN: f32 = 0.5;

    /// `native`, `auto` or a share from 0.5 to 1 (None for anything else).
    pub fn parse(text: &str) -> Option<RenderScale> {
        match text.trim() {
            "native" => Some(RenderScale::Native),
            "auto" => Some(RenderScale::Auto),
            t => t
                .parse::<f32>()
                .ok()
                .filter(|s| (RenderScale::MIN..=1.0).contains(s))
                .map(|s| RenderScale::Fixed((s * 100.0).round() / 100.0)),
        }
    }

    /// The words [`RenderScale::parse`] reads back.
    pub fn name(self) -> String {
        match self {
            RenderScale::Native => "native".to_string(),
            RenderScale::Auto => "auto".to_string(),
            RenderScale::Fixed(s) => format!("{s}"),
        }
    }

    /// The internal frame's share of an output of `width` x `height` at quality `pq`.
    pub fn scale(self, pq: &PostQuality, width: u32, height: u32) -> f32 {
        match self {
            RenderScale::Native => pq.render_scale,
            RenderScale::Auto => pq.render_scale_at(width, height),
            RenderScale::Fixed(s) => s.clamp(RenderScale::MIN, 1.0),
        }
    }
}

/// The internal frame's size for an output of `width` x `height` at `scale` (at least 1 x 1, rounded).
pub fn internal_size(width: u32, height: u32, scale: f32) -> (u32, u32) {
    let s = if scale.is_finite() {
        scale.clamp(0.25, 1.0)
    } else {
        1.0
    };
    let f = |v: u32| ((v as f32 * s).round() as u32).clamp(1, v.max(1));
    (f(width), f(height))
}

impl HdCamera {
    /// The same view and projection drawn at `width` x `height` pixels (the TAAU's internal frame: the image is the same, its
    /// pixels fewer).
    pub fn sized(&self, width: u32, height: u32) -> HdCamera {
        HdCamera {
            width,
            height,
            ..*self
        }
    }

    /// The camera with its image moved by `j` pixels (x right, y down): every point lands `j` pixels from where it did.
    pub fn jittered(&self, j: [f32; 2]) -> HdCamera {
        let dx = -2.0 * j[0] / self.width.max(1) as f32;
        let dy = 2.0 * j[1] / self.height.max(1) as f32;
        let mut c = *self;
        // The projection's third column (what multiplies eye z) gains (dx, dy) in its x and y rows; with clip w = -z that moves the
        // NDC by (-dx, -dy).
        c.proj[8] += dx;
        c.proj[9] += dy;
        let v = &self.view;
        for col in 0..4 {
            c.view_proj[col * 4] += dx * v[col * 4 + 2];
            c.view_proj[col * 4 + 1] += dy * v[col * 4 + 2];
        }
        c
    }
}

/// The inverse of an affine `Mat` (a 3 x 3 part with any scale, and a translation); the identity when singular.
pub fn affine_inverse_scaled(m: &Mat) -> Mat {
    let a = |r: usize, c: usize| f64::from(m.m[r * 4 + c]);
    let det = a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1))
        - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0))
        + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0));
    if det.abs() < 1e-30 || !det.is_finite() {
        return Mat::IDENTITY;
    }
    // Row r, column c: the cofactor of (c, r) over the determinant (the adjugate).
    let inv = [0, 1, 2].map(|r| {
        [0, 1, 2].map(|c| {
            let (r1, r2) = ((c + 1) % 3, (c + 2) % 3);
            let (c1, c2) = ((r + 1) % 3, (r + 2) % 3);
            (a(r1, c1) * a(r2, c2) - a(r1, c2) * a(r2, c1)) / det
        })
    });
    let t = [a(0, 3), a(1, 3), a(2, 3)];
    let mut out = Mat::IDENTITY;
    for (r, row) in inv.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            out.m[r * 4 + c] = *v as f32;
        }
        out.m[r * 4 + 3] = -(row[0] * t[0] + row[1] * t[1] + row[2] * t[2]) as f32;
    }
    out
}

/// The least distance (world units) a shape's points must move in a frame to count as moving ([`DynamicObject::moving_bound`]).
pub const MOVING_EPSILON: f32 = 1e-4;

impl DynamicObject {
    /// Where the shape's shadow may have changed since the last frame: its bounding sphere (centre, radius) grown by the most any
    /// of its points moved (its own previous positions, or its rigid motion at its bound's corners), covering both poses; None
    /// when it stood still.
    pub fn moving_bound(&self) -> Option<[f32; 4]> {
        let n: usize = self.draws.iter().map(|d| d.vertices.len()).sum();
        let mut d2 = 0.0f32;
        // The meshes in their own space: each one's box corners now and at its last frame's place.
        for p in &self.nodes {
            let s = p.sphere;
            for corner in 0..8u32 {
                let q = [0usize, 1, 2]
                    .map(|k| s[k] + if (corner >> k) & 1 == 1 { s[3] } else { -s[3] });
                let (a, b) = (p.now.mul_p(q), p.before.mul_p(q));
                d2 = d2.max((0..3).map(|k| (a[k] - b[k]).powi(2)).sum());
            }
        }
        match self.previous.as_deref().filter(|p| p.len() == n) {
            Some(prev) => {
                for (v, p) in self.draws.iter().flat_map(|d| d.vertices.iter()).zip(prev) {
                    d2 = d2.max((0..3).map(|k| (v.position[k] - p[k]).powi(2)).sum());
                }
            }
            None => {
                let m = self.motion();
                if m == COLUMN_IDENTITY && self.nodes.is_empty() {
                    return None;
                }
                for corner in (0..8u32).filter(|_| m != COLUMN_IDENTITY) {
                    let q = [0usize, 1, 2].map(|k| {
                        self.centre[k]
                            + if (corner >> k) & 1 == 1 {
                                self.radius
                            } else {
                                -self.radius
                            }
                    });
                    let moved = [0usize, 1, 2]
                        .map(|r| m[r] * q[0] + m[4 + r] * q[1] + m[8 + r] * q[2] + m[12 + r]);
                    d2 = d2.max((0..3).map(|k| (moved[k] - q[k]).powi(2)).sum());
                }
            }
        }
        let d = d2.sqrt();
        (d.is_finite() && d > MOVING_EPSILON).then(|| {
            [
                self.centre[0],
                self.centre[1],
                self.centre[2],
                self.radius + d,
            ]
        })
    }

    /// Where the shape's points were at its previous transform: the column-major matrix taking a point of its draws (world space at
    /// the current transform) to the world point it was (`prev * current^-1`; the identity when it has not moved).
    pub fn motion(&self) -> [f32; 16] {
        if self.prev_transform == self.transform {
            return COLUMN_IDENTITY;
        }
        let m = Mat::mul(
            &self.prev_transform,
            &affine_inverse_scaled(&self.transform),
        );
        let mut out = [0.0f32; 16];
        for r in 0..4 {
            for c in 0..4 {
                out[c * 4 + r] = m.m[r * 4 + c];
            }
        }
        out
    }
}

/// The column-major identity.
pub const COLUMN_IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

#[cfg(test)]
#[path = "post_tests.rs"]
mod tests;
