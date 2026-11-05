//! The enhanced renderer's shadows as plain data:
//! the quality ladder ([`ShadowQuality`]), the sun's cascades ([`cascades`]) and the point lights' cube faces ([`cube_face`]; a spot
//! light's only those its cone meets, [`cone_meets_face`]), and the CPU references the shadow checks measure the GPU against (rays against boxes, shadow polygons).
//!
//! **Cascades** (cascaded shadow maps, Engel 2006; Zhang et al. 2006's practical split): the view from the near plane to the shadow
//! distance cut at `λ log + (1 - λ) linear` with λ = [`SPLIT_LAMBDA`]; each slice's light view fitted to its bounding sphere (the
//! sphere's size does not change when the camera turns, so the map's texel size stays fixed), its centre snapped to whole texels in
//! the light's plane (a still or a moving camera sees the same texel grid: the shadow edges do not crawl), and the light's near plane
//! pulled back toward the sun (casters in front of it are flattened onto it by depth clamping: pancaking).
//!
//! Matrices are column-major, world to clip with wgpu's depth range; the light looks along `-towards`, depth grows away from the sun.

use crate::hd::{HdCamera, Quality};

/// The practical split's weight of the logarithmic split.
pub const SPLIT_LAMBDA: f64 = 0.7;
/// The farthest the sun's shadows reach (units; capped at the visible distance) and where they start to fade (a fraction of it).
pub const SHADOW_DISTANCE: f32 = 160.0;
pub const SHADOW_FADE: f32 = 0.85;
/// The most cascades and point-light slots any quality has (six: a map's range, [`ShadowQuality::for_distance`]).
pub const MAX_CASCADES: usize = 6;
/// The cascades a range past [`SHADOW_DISTANCE`] (a map's) has over its quality's, at most [`MAX_CASCADES`].
pub const MAP_EXTRA_CASCADES: usize = 2;
/// Off: a map's range takes its quality's cascades and the practical split as a near range does (on by default).
pub static MAP_CASCADES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

fn map_cascades() -> bool {
    MAP_CASCADES.load(std::sync::atomic::Ordering::Relaxed)
}
pub const MAX_POINT_SHADOWS: usize = 16;
/// The near plane of a point light's cube faces (units).
pub const POINT_NEAR: f32 = 0.05;
/// The sun's angular radius for the PCSS (degrees: a little larger than the real sun's 0.27 for softer penumbrae).
pub const SUN_RADIUS_DEG: f32 = 0.5;

/// One quality's shadows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowQuality {
    /// Sun cascades and each one's side in texels.
    pub cascades: usize,
    pub cascade_size: u32,
    /// The PCF filter's taps (a rotated Vogel disk), and PCSS's blocker-search taps (0: plain PCF).
    pub pcf_taps: u32,
    pub blocker_taps: u32,
    /// Point lights with cube shadows, each face's side, the taps a lookup takes.
    pub point_lights: usize,
    pub point_size: u32,
    pub point_taps: u32,
    /// The contact shadows' march steps (0: off).
    pub contact_steps: u32,
}

impl ShadowQuality {
    pub fn pcss(&self) -> bool {
        self.blocker_taps > 0
    }

    /// This quality's shadows for a scene whose sun shadows reach `distance` (`HdScene::shadow_distance`): past
    /// [`SHADOW_DISTANCE`] (a map's room-sized range) [`MAP_EXTRA_CASCADES`] more cascades, which [`cascades`] spreads
    /// logarithmically past the first (stretched over a map's range, four were about 3 times coarser at mid distance
    /// and 17 times the pixel at 4 units).
    pub fn for_distance(self, distance: f32) -> ShadowQuality {
        if distance > SHADOW_DISTANCE && map_cascades() {
            ShadowQuality {
                cascades: (self.cascades + MAP_EXTRA_CASCADES).min(MAX_CASCADES),
                ..self
            }
        } else {
            self
        }
    }
}

impl Quality {
    /// The shadows of this quality: Low 2 x 2048 PCF 9 (1024 went blotchy), no point shadows, no contact; Medium 3 x 2048 PCF
    /// 16, 4 x 256, contact;
    /// High 4 x 2048 PCSS, 8 x 512; Ultra 4 x 4096 PCSS, 16 x 512. The enhanced renderer sizes its point cubes from these in
    /// three tiers (`torque_render_hd::shadows::point_tiers`: High 4 x 512, 8 x 256 and 16 x 128).
    pub fn shadows(self) -> ShadowQuality {
        match self {
            Quality::Low => ShadowQuality {
                cascades: 2,
                cascade_size: 2048,
                pcf_taps: 9,
                blocker_taps: 0,
                point_lights: 0,
                point_size: 256,
                point_taps: 0,
                contact_steps: 0,
            },
            Quality::Medium => ShadowQuality {
                cascades: 3,
                cascade_size: 2048,
                pcf_taps: 16,
                blocker_taps: 0,
                point_lights: 4,
                point_size: 256,
                point_taps: 6,
                contact_steps: 12,
            },
            Quality::High => ShadowQuality {
                cascades: 4,
                cascade_size: 2048,
                pcf_taps: 16,
                blocker_taps: 12,
                point_lights: 8,
                point_size: 512,
                point_taps: 8,
                contact_steps: 16,
            },
            Quality::Ultra => ShadowQuality {
                cascades: 4,
                cascade_size: 4096,
                pcf_taps: 24,
                blocker_taps: 16,
                point_lights: MAX_POINT_SHADOWS,
                point_size: 512,
                point_taps: 12,
                contact_steps: 24,
            },
        }
    }
}

/// One sun cascade: its world-to-clip matrix, bounding sphere and the numbers the lookup needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cascade {
    pub view_proj: [f32; 16],
    pub centre: [f32; 3],
    pub radius: f32,
    /// The slice's view depths.
    pub near_depth: f32,
    pub far_depth: f32,
    /// One texel's side in world units.
    pub texel: f32,
    /// The light-space depth range (world units) that clip z 0..1 spans.
    pub depth_range: f32,
}

/// The light's basis: right, up, forward (`-towards`).
pub fn light_basis(towards: [f32; 3]) -> [[f64; 3]; 3] {
    let t = towards.map(f64::from);
    let l = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt().max(1e-12);
    let f = [-t[0] / l, -t[1] / l, -t[2] / l];
    let helper = if f[2].abs() < 0.99 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let right = normalize(cross(f, helper));
    let up = cross(right, f);
    [right, up, f]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(a: [f64; 3]) -> [f64; 3] {
    let l = dot(a, a).sqrt().max(1e-300);
    a.map(|v| v / l)
}

/// The practical split distances: `n + 1` view depths from `near` to `far` (Zhang et al. 2006, λ = [`SPLIT_LAMBDA`]).
pub fn practical_splits(near: f32, far: f32, n: usize) -> Vec<f32> {
    let (near, far) = (f64::from(near.max(1e-4)), f64::from(far.max(near * 1.01)));
    (0..=n)
        .map(|i| {
            let f = i as f64 / n.max(1) as f64;
            let log = near * (far / near).powf(f);
            let lin = near + (far - near) * f;
            (SPLIT_LAMBDA * log + (1.0 - SPLIT_LAMBDA) * lin) as f32
        })
        .collect()
}

/// The view depths the cascades span, fitted to the static geometry in view (as sample-distribution shadow maps fit them to the depth
/// buffer's range, Lauritzen et al. 2011, here from the cells' boxes on the CPU): from the nearest box seen (the camera's near plane
/// when one is within [`MIN_FITTED_START`] units, else a power of two below 0.8 of its distance) to the farthest (a power of two
/// above), at most [`SHADOW_DISTANCE`] or four times the start, and the far plane; where it was cut short of the farthest box the
/// shadows fade out over its last [`SHADOW_FADE`]. Powers of two keep the range, so the cascades and their cached maps, the same
/// while the camera moves about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowRange {
    pub start: f32,
    pub end: f32,
    /// Where the fade starts (`end` when nothing seen is beyond the range).
    pub fade: f32,
}

pub fn shadow_range(
    camera: &HdCamera,
    boxes: impl IntoIterator<Item = [[f32; 3]; 2]>,
) -> ShadowRange {
    shadow_range_of(camera, seen_reach(camera, boxes))
}

/// The nearest point and the farthest corner (units from the eye) of the boxes the camera sees, or None when it sees none.
pub fn seen_reach(
    camera: &HdCamera,
    boxes: impl IntoIterator<Item = [[f32; 3]; 2]>,
) -> Option<(f32, f32)> {
    let e = camera.eye;
    let (mut lo, mut hi) = (f32::INFINITY, 0.0f32);
    for [a, b] in boxes {
        if !camera.sees_box(a, b) {
            continue;
        }
        let mut near2 = 0.0;
        let mut far2 = 0.0;
        for k in 0..3 {
            let d = (e[k] - e[k].clamp(a[k], b[k])).abs();
            near2 += d * d;
            let f = (e[k] - a[k]).abs().max((e[k] - b[k]).abs());
            far2 += f * f;
        }
        lo = lo.min(near2.sqrt());
        hi = hi.max(far2.sqrt());
    }
    lo.is_finite().then_some((lo, hi))
}

/// [`shadow_range`] from the seen boxes' nearest point and farthest corner ([`seen_reach`]).
pub fn shadow_range_of(camera: &HdCamera, reach: Option<(f32, f32)>) -> ShadowRange {
    shadow_range_within(camera, reach, SHADOW_DISTANCE)
}

/// [`shadow_range_of`] reaching at most `distance` (the scene's `shadow_distance`) instead of [`SHADOW_DISTANCE`].
pub fn shadow_range_within(
    camera: &HdCamera,
    reach: Option<(f32, f32)>,
    distance: f32,
) -> ShadowRange {
    let distance = if distance.is_finite() {
        distance.max(1.0)
    } else {
        SHADOW_DISTANCE
    };
    let (lo, hi) = reach.unwrap_or((f32::INFINITY, 0.0));
    let cap = distance.min(camera.far);
    if !lo.is_finite() {
        return ShadowRange {
            start: camera.near,
            end: cap,
            fade: cap * SHADOW_FADE,
        };
    }
    let start = if lo < MIN_FITTED_START {
        camera.near
    } else {
        2f32.powi((0.8 * lo).log2().floor() as i32)
    };
    let end = 2f32
        .powi(hi.max(1.0).log2().ceil() as i32)
        .max(MIN_FITTED_START)
        .min(distance.max(4.0 * start))
        .max(start * 4.0)
        .min(camera.far);
    ShadowRange {
        start,
        end,
        fade: if hi > end { end * SHADOW_FADE } else { end },
    }
}

/// The nearest a fitted range starts away from the camera's near plane (units).
pub const MIN_FITTED_START: f32 = 16.0;

/// The farthest the first cascade reaches (units, or twice the range's start when that is farther): over a big world's range
/// the practical split put it at about 12 units, a sphere some 50 units across, so a small detail seen from a step away had a texel
/// several pixels wide and its shadow stepped.
pub const FIRST_SPLIT_MAX: f32 = 4.0;

/// The camera's eye-space to world rotation's columns (the view matrix's rotation transposed) and the eye.
fn eye_to_world(camera: &HdCamera) -> ([[f64; 3]; 3], [f64; 3]) {
    let v = camera.view.map(f64::from);
    // view = [R | -R e]: row r of R is (v[r], v[4 + r], v[8 + r]); eye-space axis k in world is row k of R.
    let axis = |k: usize| [v[k], v[4 + k], v[8 + k]];
    ([axis(0), axis(1), axis(2)], camera.eye.map(f64::from))
}

/// The bounding sphere (eye space) of the view slice between depths `d0` and `d1` (the frustum's rectangles' centres on a line).
fn slice_sphere(camera: &HdCamera, d0: f64, d1: f64) -> ([f64; 3], f64) {
    let [l, r, b, t] = camera.frustum.map(f64::from);
    let n = f64::from(camera.near);
    let corners = |d: f64| -> [[f64; 3]; 4] {
        let s = d / n;
        [
            [l * s, b * s, -d],
            [r * s, b * s, -d],
            [l * s, t * s, -d],
            [r * s, t * s, -d],
        ]
    };
    let (c0, c1) = (corners(d0), corners(d1));
    let mid = |c: &[[f64; 3]; 4]| [0, 1, 2].map(|k| 0.25 * (c[0][k] + c[1][k] + c[2][k] + c[3][k]));
    let (mid0, mid1) = (mid(&c0), mid(&c1));
    // On the line mid0 + (mid1 - mid0) s, equally far from the slice's farthest near and far corners.
    let far_corner = |c: &[[f64; 3]; 4], m: [f64; 3]| {
        c.iter()
            .map(|p| [p[0] - m[0], p[1] - m[1], p[2] - m[2]])
            .map(|d| dot(d, d))
            .fold(0.0, f64::max)
    };
    let (h0, h1) = (far_corner(&c0, mid0), far_corner(&c1, mid1));
    let axis = [mid1[0] - mid0[0], mid1[1] - mid0[1], mid1[2] - mid0[2]];
    let len2 = dot(axis, axis).max(1e-300);
    // |s a|^2 + h0 = |(1 - s) a|^2 + h1  =>  s = (len2 + h1 - h0) / (2 len2).
    let s = ((len2 + h1 - h0) / (2.0 * len2)).clamp(0.0, 1.0);
    let centre = [0, 1, 2].map(|k| mid0[k] + axis[k] * s);
    let radius = c0
        .iter()
        .chain(&c1)
        .map(|p| {
            let d = [p[0] - centre[0], p[1] - centre[1], p[2] - centre[2]];
            dot(d, d).sqrt()
        })
        .fold(0.0, f64::max);
    (centre, radius)
}

/// A cascade's centre moves in steps of its map's side over this (in texels), its square widened by a step.
pub const CASCADE_SNAP_SHARE: f64 = 16.0;
/// A cascade's depth range moves in steps of this many of its square's steps: the square moves a step at a time (the cached map
/// scrolled and only the strip it uncovers drawn), the depth range only every so many (widened by that much, so the slice still
/// fits; a scroll copies the depths as they are, which a moved range would change).
pub const CASCADE_DEPTH_STEPS: f64 = 8.0;

/// The sun's cascades for `camera` and a sun `towards`, `quality.cascades` of `quality.cascade_size` texels, over `range`.
pub fn cascades(
    camera: &HdCamera,
    towards: [f32; 3],
    quality: &ShadowQuality,
    range: &ShadowRange,
) -> Vec<Cascade> {
    let n = quality.cascades.clamp(1, MAX_CASCADES);
    let near = range.start.max(camera.near);
    let far = range.end.min(camera.far).max(near * 4.0);
    let mut splits = practical_splits(near, far, n);
    if n > 1 {
        splits[1] = splits[1].min(FIRST_SPLIT_MAX.max(2.0 * near));
    }
    // A range past the shadow distance (a map's): the cascades after the first spread logarithmically to its end, so each holds the same
    // share of the pixels it covers (the practical split's linear part gave the second cascade 4 to 82 units over 512).
    if far > SHADOW_DISTANCE && n > 2 && map_cascades() {
        let first = f64::from(splits[1]);
        let ratio = f64::from(far) / first;
        for (i, s) in splits.iter_mut().enumerate().take(n).skip(2) {
            *s = (first * ratio.powf((i - 1) as f64 / (n - 1) as f64)) as f32;
        }
    }
    let (rot, eye) = eye_to_world(camera);
    let [right, up, fwd] = light_basis(towards);
    let res = f64::from(quality.cascade_size.max(16));
    let spheres: Vec<([f64; 3], f64)> = (0..n)
        .map(|i| slice_sphere(camera, f64::from(splits[i]), f64::from(splits[i + 1])))
        .collect();
    (0..n)
        .map(|i| {
            let (c_eye, r) = spheres[i];
            // How far past the slice's centre a receiver of the sun's shadows may lie: within its own sphere or a farther
            // slice's. A receiver in this cascade's square past its far side would take its shadow from a coarser cascade,
            // which leaves out the moving shapes this one holds (`holds_shadow`): a held item's shadow on the ground under it
            // stopped along a line.
            let deep = spheres[i..]
                .iter()
                .map(|(c, s)| {
                    let d = [c[0] - c_eye[0], c[1] - c_eye[1], c[2] - c_eye[2]];
                    dot(d, d).sqrt() + s
                })
                .fold(r, f64::max);
            // Both on a 1/64-unit grid (they depend only on the frustum: the same every frame).
            let r = (r * 64.0).ceil() / 64.0;
            let deep = (deep * 64.0).ceil() / 64.0;
            let c = [0, 1, 2].map(|k| {
                eye[k] + rot[0][k] * c_eye[0] + rot[1][k] * c_eye[1] + rot[2][k] * c_eye[2]
            });
            // The centre snapped to a coarse grid of SNAP_TEXELS texels (a sixteenth of the map), the square widened by a
            // step so it still holds the slice's sphere, so the cached static cascade is drawn again only when the camera crosses a
            // step (it was snapped to one texel and its depth not at all: walking drew all four cascades whole every frame).
            let k = (res / CASCADE_SNAP_SHARE).round().max(1.0);
            let texel = 2.0 * r / (res - 2.0 * k);
            let step = k * texel;
            let r = 0.5 * texel * res;
            let cx = (dot(c, right) / step).round() * step;
            let cy = (dot(c, up) / step).round() * step;
            let zstep = step * CASCADE_DEPTH_STEPS;
            let cz = (dot(c, fwd) / zstep).round() * zstep;
            let pull = r.max(20.0);
            let dn = cz - r - 0.5 * zstep - pull;
            // The far side reaches every receiver past the slice (`deep`), at least the square's half side.
            let range = r + deep.max(r) + zstep + pull;
            // Rows: x = (p.right - cx) / r, y = (p.up - cy) / r, z = (p.fwd - dn) / range, w = 1.
            let rows = [
                [right[0] / r, right[1] / r, right[2] / r, -cx / r],
                [up[0] / r, up[1] / r, up[2] / r, -cy / r],
                [fwd[0] / range, fwd[1] / range, fwd[2] / range, -dn / range],
                [0.0, 0.0, 0.0, 1.0],
            ];
            let mut m = [0.0f32; 16];
            for (row, vals) in rows.iter().enumerate() {
                for (col, v) in vals.iter().enumerate() {
                    m[col * 4 + row] = *v as f32;
                }
            }
            let centre = [0, 1, 2].map(|k| (right[k] * cx + up[k] * cy + fwd[k] * cz) as f32);
            Cascade {
                view_proj: m,
                centre,
                radius: r as f32,
                near_depth: splits[i],
                far_depth: splits[i + 1],
                texel: texel as f32,
                depth_range: range as f32,
            }
        })
        .collect()
}

/// A column-major matrix times a point (x, y, z, w).
pub fn transform(m: &[f32; 16], p: [f32; 3]) -> [f32; 4] {
    [0, 1, 2, 3].map(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r])
}

/// The light-space box (clip x, y, z) of a world box under a cascade's matrix.
pub fn light_box(m: &[f32; 16], lo: [f32; 3], hi: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let mut a = [f32::INFINITY; 3];
    let mut b = [f32::NEG_INFINITY; 3];
    for k in 0..8 {
        let p = [
            if k & 1 == 0 { lo[0] } else { hi[0] },
            if k & 2 == 0 { lo[1] } else { hi[1] },
            if k & 4 == 0 { lo[2] } else { hi[2] },
        ];
        let q = transform(m, p);
        for j in 0..3 {
            a[j] = a[j].min(q[j]);
            b[j] = b[j].max(q[j]);
        }
    }
    (a, b)
}

/// The share of a cascade's square (in its clip space, -1..1) a lookup is sure to be taken from: the forward pass
/// takes a point from the first cascade whose square holds it with its filter's margin, far less than this.
pub const HELD_SHARE: f32 = 0.9;

/// Whether a cascade holds a box's whole shadow: every receiver of its shadow lies under it along the light, so at the
/// same place across the cascade's square, and inside [`HELD_SHARE`] of it a lookup is taken from this cascade (or a finer one),
/// never from the coarser ones after it (each cascade's far side reaches every receiver of the range: [`cascades`]): a moving
/// shape held by a cascade need not be drawn into those.
pub fn holds_shadow(m: &[f32; 16], lo: [f32; 3], hi: [f32; 3]) -> bool {
    let (a, b) = light_box(m, lo, hi);
    a[0] >= -HELD_SHARE && b[0] <= HELD_SHARE && a[1] >= -HELD_SHARE && b[1] <= HELD_SHARE
}

/// Whether a box may cast into a cascade: inside its square across, not wholly beyond its far side (anything toward the sun is
/// pancaked).
pub fn casts_into(m: &[f32; 16], lo: [f32; 3], hi: [f32; 3]) -> bool {
    let (a, b) = light_box(m, lo, hi);
    b[0] >= -1.0 && a[0] <= 1.0 && b[1] >= -1.0 && a[1] <= 1.0 && a[2] <= 1.0
}

/// The texel rectangle `[x0, y0, x1, y1)` a box covers in a cascade of `size` texels (one texel of margin), `None` off the map.
pub fn texel_rect(m: &[f32; 16], lo: [f32; 3], hi: [f32; 3], size: u32) -> Option<[u32; 4]> {
    let (a, b) = light_box(m, lo, hi);
    if a[2] > 1.0 {
        return None;
    }
    let s = size as f32;
    // u = 0.5 x + 0.5; v = 0.5 - 0.5 y.
    let x0 = ((a[0] * 0.5 + 0.5) * s).floor() - 1.0;
    let x1 = ((b[0] * 0.5 + 0.5) * s).ceil() + 1.0;
    let y0 = ((0.5 - b[1] * 0.5) * s).floor() - 1.0;
    let y1 = ((0.5 - a[1] * 0.5) * s).ceil() + 1.0;
    let c = |v: f32| v.clamp(0.0, s) as u32;
    let r = [c(x0), c(y0), c(x1), c(y1)];
    (r[2] > r[0] && r[3] > r[1]).then_some(r)
}

/// The union of two texel rectangles.
pub fn rect_union(a: Option<[u32; 4]>, b: [u32; 4]) -> [u32; 4] {
    match a {
        None => b,
        Some(a) => [
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ],
    }
}

/// Whether two texel rectangles overlap.
pub fn rects_meet(a: [u32; 4], b: [u32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// Cube face `face` (0 +X, 1 -X, 2 +Y, 3 -Y, 4 +Z, 5 -Z: the cube map's layer order) of a light at `light` reaching `far`: the
/// world-to-clip matrix whose pixels land where a cube lookup of the direction finds them (the WebGPU / D3D face table: x = sc / ma,
/// y = -tc / ma), with clip depth from `POINT_NEAR` to `far`.
pub fn cube_face(light: [f32; 3], far: f32, face: usize) -> [f32; 16] {
    // (sc, tc, ma) as rows over (dx, dy, dz).
    let (sc, tc, ma): ([f32; 3], [f32; 3], [f32; 3]) = match face {
        0 => ([0.0, 0.0, -1.0], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0]),
        1 => ([0.0, 0.0, 1.0], [0.0, -1.0, 0.0], [-1.0, 0.0, 0.0]),
        2 => ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        3 => ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, -1.0, 0.0]),
        4 => ([1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
        _ => ([-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]),
    };
    let n = POINT_NEAR;
    let f = far.max(n * 2.0);
    let a = f / (f - n);
    let b = -n * f / (f - n);
    let dot3 = |r: [f32; 3]| r[0] * light[0] + r[1] * light[1] + r[2] * light[2];
    let rows = [
        [sc[0], sc[1], sc[2], -dot3(sc)],
        [-tc[0], -tc[1], -tc[2], dot3(tc)],
        [a * ma[0], a * ma[1], a * ma[2], -a * dot3(ma) + b],
        [ma[0], ma[1], ma[2], -dot3(ma)],
    ];
    let mut m = [0.0f32; 16];
    for (row, vals) in rows.iter().enumerate() {
        for (col, v) in vals.iter().enumerate() {
            m[col * 4 + row] = *v;
        }
    }
    m
}

/// The angle (radians) a cone of light reaches past its outer half angle for a face to be drawn: the lookup's filter taps spread a
/// little past the cone's edge.
pub const CONE_FACE_MARGIN: f32 = 0.1;

/// Whether a spot light's cone about `direction` (unit), of outer half angle `outer` (radians), meets cube face `face`'s pyramid
/// (the faces of [`cube_face`]; conservative: the pyramid taken as the cone about its axis through its corners).
pub fn cone_meets_face(direction: [f32; 3], outer: f32, face: usize) -> bool {
    let axis = match face {
        0 => [1.0, 0.0, 0.0],
        1 => [-1.0, 0.0, 0.0],
        2 => [0.0, 1.0, 0.0],
        3 => [0.0, -1.0, 0.0],
        4 => [0.0, 0.0, 1.0],
        _ => [0.0, 0.0, -1.0],
    };
    let len = direction.iter().map(|v| v * v).sum::<f32>().sqrt();
    if len < 1e-6 {
        return true;
    }
    let c = (0..3).map(|k| axis[k] * direction[k]).sum::<f32>() / len;
    // The face's corners lie acos(1 / sqrt 3) off its axis.
    let reach = outer + CONE_FACE_MARGIN + (1.0f32 / 3.0f32.sqrt()).acos();
    reach >= std::f32::consts::PI || c.clamp(-1.0, 1.0).acos() <= reach
}

/// Whether a box may be seen by a light's cube face (inside its pyramid and the light's sphere).
pub fn face_sees(light: [f32; 3], radius: f32, face: usize, lo: [f32; 3], hi: [f32; 3]) -> bool {
    // The sphere first.
    let mut d2 = 0.0;
    for k in 0..3 {
        let v = light[k].clamp(lo[k], hi[k]) - light[k];
        d2 += v * v;
    }
    if d2 > radius * radius {
        return false;
    }
    let m = cube_face(light, radius, face);
    let mut out = [true; 5];
    for k in 0..8 {
        let p = [
            if k & 1 == 0 { lo[0] } else { hi[0] },
            if k & 2 == 0 { lo[1] } else { hi[1] },
            if k & 4 == 0 { lo[2] } else { hi[2] },
        ];
        let c = transform(&m, p);
        let w = c[3];
        let inside = [w > 0.0, c[0] <= w, c[0] >= -w, c[1] <= w, c[1] >= -w];
        for j in 0..5 {
            out[j] &= !inside[j];
        }
    }
    !out.iter().any(|o| *o)
}

/// Whether a sphere may be seen by a light's cube face.
pub fn face_sees_sphere(light: [f32; 3], radius: f32, face: usize, c: [f32; 3], r: f32) -> bool {
    face_sees(
        light,
        radius,
        face,
        [c[0] - r, c[1] - r, c[2] - r],
        [c[0] + r, c[1] + r, c[2] + r],
    )
}

/// An axis-aligned box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub lo: [f32; 3],
    pub hi: [f32; 3],
}

impl Aabb {
    /// The ray's entry distance and the face's normal (`None`: missed, or the box is behind).
    pub fn hit(&self, o: [f32; 3], d: [f32; 3]) -> Option<(f32, [f32; 3])> {
        let mut t0 = f32::NEG_INFINITY;
        let mut t1 = f32::INFINITY;
        let mut axis = 0;
        for k in 0..3 {
            if d[k].abs() < 1e-12 {
                if o[k] < self.lo[k] || o[k] > self.hi[k] {
                    return None;
                }
                continue;
            }
            let (mut a, mut b) = ((self.lo[k] - o[k]) / d[k], (self.hi[k] - o[k]) / d[k]);
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            if a > t0 {
                t0 = a;
                axis = k;
            }
            t1 = t1.min(b);
        }
        if t0 > t1 || t1 < 0.0 {
            return None;
        }
        let mut n = [0.0; 3];
        n[axis] = if d[axis] > 0.0 { -1.0 } else { 1.0 };
        Some((t0.max(0.0), n))
    }

    /// The box's eight corners.
    pub fn corners(&self) -> [[f32; 3]; 8] {
        let mut out = [[0.0; 3]; 8];
        for (k, c) in out.iter_mut().enumerate() {
            *c = [
                if k & 1 == 0 { self.lo[0] } else { self.hi[0] },
                if k & 2 == 0 { self.lo[1] } else { self.hi[1] },
                if k & 4 == 0 { self.lo[2] } else { self.hi[2] },
            ];
        }
        out
    }
}

/// What a ray first hits among `boxes`: its distance, the box, the face's normal.
pub fn trace(boxes: &[Aabb], o: [f32; 3], d: [f32; 3]) -> Option<(f32, usize, [f32; 3])> {
    let mut best: Option<(f32, usize, [f32; 3])> = None;
    for (i, b) in boxes.iter().enumerate() {
        if let Some((t, n)) = b.hit(o, d)
            && best.is_none_or(|(bt, _, _)| t < bt)
        {
            best = Some((t, i, n));
        }
    }
    best
}

/// Whether anything of `boxes` lies between `p` and `p + d * max_t` (from `eps` on).
pub fn occluded(boxes: &[Aabb], p: [f32; 3], d: [f32; 3], eps: f32, max_t: f32) -> bool {
    let o = [p[0] + d[0] * eps, p[1] + d[1] * eps, p[2] + d[2] * eps];
    boxes
        .iter()
        .any(|b| b.hit(o, d).is_some_and(|(t, _)| t < max_t - eps))
}

/// The convex hull (counter-clockwise) of points in the plane (Andrew's monotone chain).
pub fn convex_hull(mut pts: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross2 = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut hull: Vec<[f32; 2]> = Vec::new();
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &[f32; 2]>> = if pass == 0 {
            Box::new(pts.iter())
        } else {
            Box::new(pts.iter().rev())
        };
        for p in iter {
            while hull.len() >= start + 2
                && cross2(hull[hull.len() - 2], hull[hull.len() - 1], *p) <= 0.0
            {
                hull.pop();
            }
            hull.push(*p);
        }
        hull.pop();
    }
    hull
}

/// Whether `p` is inside a counter-clockwise convex polygon.
pub fn inside_convex(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    (0..poly.len()).all(|i| {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]) >= 0.0
    })
}

/// The shadow a box casts on the plane `z = ground` under a sun `towards` (its corners slid along the sun's rays).
pub fn sun_shadow_polygon(b: &Aabb, towards: [f32; 3], ground: f32) -> Vec<[f32; 2]> {
    let tz = towards[2].max(1e-4);
    convex_hull(
        b.corners()
            .iter()
            .map(|p| {
                let s = (p[2] - ground) / tz;
                [p[0] - towards[0] * s, p[1] - towards[1] * s]
            })
            .collect(),
    )
}

/// The shadow a box casts on the plane `z = ground` from a point light above it (the box wholly below the light).
pub fn point_shadow_polygon(b: &Aabb, light: [f32; 3], ground: f32) -> Vec<[f32; 2]> {
    convex_hull(
        b.corners()
            .iter()
            .map(|p| {
                let t = (ground - light[2]) / (p[2] - light[2]).min(-1e-4);
                [
                    light[0] + (p[0] - light[0]) * t,
                    light[1] + (p[1] - light[1]) * t,
                ]
            })
            .collect(),
    )
}

#[cfg(test)]
#[path = "shadow_tests.rs"]
mod tests;
