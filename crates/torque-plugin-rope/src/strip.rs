//! The ribbon and line styles as triangles: a flat strip that always faces the camera along a curve.
//!
//! A [`StripSet`] holds each strip's centerline, made once for each [`RopeWorld::revision`](crate::RopeWorld::revision): a
//! ribbon's is a smooth Catmull-Rom curve through the simulated rope's points ([`spline`]), a line's is the two ends. What
//! depends on the camera is made each frame by [`StripMesh::build`] from a [`StripView`]:
//!
//! - **Facing the camera.** At each sample the strip's width runs along the cross product of the curve's direction and the
//!   direction to the eye, so the strip is flat to the screen however the rope bends. Looking along the curve that cross product
//!   goes to zero; the width then turns to the camera's own right (or up) direction, blended in smoothly, so the strip never
//!   twists and never turns edge-on.
//! - **A minimum on-screen width.** The width is the rope's `thickness`, but never less than [`MIN_PIXELS`] on the screen, so a
//!   distant cable or beam stays a thin line instead of dropping out.
//! - **Caps.** Each end is pushed out by half a width, so a strip seen end-on is a small square and not nothing.
//! - **Soft edges.** The triangles reach [`EDGE_PIXELS`] past the true edge, and each vertex carries its place across the strip
//!   (-1 to 1) and the half width in pixels; the shader fades the last pixel to the edge, so the sides are smooth.

use glam::Vec3;

use crate::data::{RopeParams, RopeStyle};
use crate::mesh::Draw;
use crate::sim::Appearance;

/// The pieces each segment of a ribbon is cut into when it is drawn.
pub const SUBDIVISIONS: usize = 4;
/// The least width, in pixels, a strip is drawn with.
pub const MIN_PIXELS: f32 = 2.0;
/// Bytes of one [`StripVertex`] as the shader reads it: position, place across, half width in pixels, flags, reach, color, texture
/// place, scroll and glow's brightness.
pub const STRIP_VERTEX_BYTES: usize = 48;
/// [`StripVertex::flags`]: the strip glows.
pub const FLAG_GLOW: f32 = 1.0;
/// [`StripVertex::flags`]: the sun's shadows fall on the strip.
pub const FLAG_RECEIVE: f32 = 2.0;
/// How far outside the strip's true edge the triangles reach, in pixels, so the pixels the edge half covers are drawn and can
/// be faded.
pub const EDGE_PIXELS: f32 = 0.5;
/// The nearest distance in front of the eye a pixel's size is measured at.
const NEAR_DEPTH: f32 = 0.05;

/// Points on a Catmull-Rom curve through `points`, `subdivisions` for each piece between two of them. The curve passes through
/// every point (the points are samples `0`, `subdivisions`, `2 * subdivisions`, ...); the ends are extended by reflection. Fewer
/// than three points, or no subdivision, give the points as they are.
pub fn spline(points: &[Vec3], subdivisions: usize) -> Vec<Vec3> {
    let n = points.len();
    if n < 3 || subdivisions < 2 {
        return points.to_vec();
    }
    let mut out = Vec::with_capacity((n - 1) * subdivisions + 1);
    for i in 0..n - 1 {
        let (p1, p2) = (points[i], points[i + 1]);
        let p0 = if i == 0 { p1 * 2.0 - p2 } else { points[i - 1] };
        let p3 = if i + 2 < n {
            points[i + 2]
        } else {
            p2 * 2.0 - p1
        };
        for k in 0..subdivisions {
            let t = k as f32 / subdivisions as f32;
            let (t2, t3) = (t * t, t * t * t);
            out.push(
                (p1 * 2.0
                    + (p2 - p0) * t
                    + (p0 * 2.0 - p1 * 5.0 + p2 * 4.0 - p3) * t2
                    + (p1 * 3.0 - p0 - p2 * 3.0 + p3) * t3)
                    * 0.5,
            );
        }
    }
    out.push(points[n - 1]);
    out
}

/// One ribbon or line: its curve and how it is painted.
#[derive(Clone, Debug, PartialEq)]
pub struct Strip {
    /// The centerline, from the start to the end.
    pub samples: Vec<Vec3>,
    /// The width in world units.
    pub width: f32,
    /// The paint (the alpha makes it see-through).
    pub color: [u8; 4],
    /// Whether it is drawn unlit.
    pub emissive: bool,
    /// Whether it casts a shadow (a band of its width that faces the sun).
    pub cast_shadow: bool,
    /// Whether the sun's shadows fall on it.
    pub receive_shadow: bool,
    /// The texture's number in the world's table (0: none).
    pub texture: u32,
    /// The world length of one repeat of the texture.
    pub texture_length: f32,
    /// How fast the texture runs along the strip, repeats a second.
    pub scroll: f32,
    /// The width at the far end (`width` tapers to it along the strip).
    pub width_end: f32,
    /// The color at the far end (the paint blends to it along the strip).
    pub color_end: Option<[u8; 4]>,
    /// How bright a glowing strip is (the color is multiplied by it in the HDR frame).
    pub intensity: f32,
    /// Whether it adds its light to what is behind it.
    pub additive: bool,
}

/// The ribbons and lines of a world.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StripSet {
    /// The [`RopeWorld::revision`](crate::RopeWorld::revision) it was made for.
    pub revision: u64,
    /// The strips, in rope order.
    pub strips: Vec<Strip>,
}

impl StripSet {
    /// The strips of ropes given as their points, parameters and what they are drawn with; a rope drawn as a tube, with fewer than
    /// two points or with a point that is not finite, is left out.
    pub fn build<'a>(
        revision: u64,
        ropes: impl Iterator<Item = (&'a [Vec3], &'a RopeParams, Appearance)>,
    ) -> StripSet {
        let mut strips = Vec::new();
        for (points, params, look) in ropes {
            if points.len() < 2 || points.iter().any(|p| !p.is_finite()) {
                continue;
            }
            let samples = match params.style {
                RopeStyle::Tube => continue,
                RopeStyle::Line => vec![points[0], points[points.len() - 1]],
                RopeStyle::Ribbon => spline(points, SUBDIVISIONS),
            };
            strips.push(Strip {
                samples,
                width: params.thickness,
                color: look.color,
                emissive: look.emissive,
                cast_shadow: params.cast_shadow,
                receive_shadow: params.receive_shadow,
                texture: look.texture,
                texture_length: if params.texture_length > 0.0 {
                    params.texture_length
                } else {
                    params.thickness.max(1.0e-3)
                },
                scroll: look.scroll,
                width_end: params.end_thickness(),
                color_end: params.color_end,
                intensity: if look.emissive {
                    params.emissive_intensity
                } else {
                    1.0
                },
                additive: params.additive,
            });
        }
        StripSet { revision, strips }
    }
}

/// What a strip needs to know of the camera: where it is, which way it faces, and how big a pixel is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StripView {
    /// The eye.
    pub eye: Vec3,
    /// The camera's right, up and forward (unit vectors).
    pub right: Vec3,
    /// See `right`.
    pub up: Vec3,
    /// See `right`.
    pub forward: Vec3,
    /// The world size of a pixel one unit in front of the eye.
    pub pixel: f32,
}

impl StripView {
    /// The view of a camera: its eye, its view and projection matrices (column major, as the renderer's camera has them, the
    /// camera looking down its -z) and the height of the picture in pixels.
    pub fn from_matrices(
        eye: [f32; 3],
        view: &[f32; 16],
        proj: &[f32; 16],
        height: u32,
    ) -> StripView {
        StripView {
            eye: Vec3::from(eye),
            right: Vec3::new(view[0], view[4], view[8]),
            up: Vec3::new(view[1], view[5], view[9]),
            forward: -Vec3::new(view[2], view[6], view[10]),
            pixel: 2.0 / (proj[5].abs().max(1.0e-6) * height.max(1) as f32),
        }
    }

    /// The world size of a pixel at `p`.
    pub fn pixel_at(&self, p: Vec3) -> f32 {
        (p - self.eye).dot(self.forward).max(NEAR_DEPTH) * self.pixel
    }
}

/// One corner of a strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StripVertex {
    /// Where it is in the world.
    pub position: [f32; 3],
    /// Which side of the strip: -1 on one edge, 1 on the other.
    pub across: f32,
    /// Half the strip's width on the screen, in pixels, with the [`EDGE_PIXELS`] on each side (at least `MIN_PIXELS / 2` and
    /// that).
    pub half_pixels: f32,
    /// [`FLAG_GLOW`] for a strip that glows plus [`FLAG_RECEIVE`] for one the sun's shadows fall on.
    pub flags: f32,
    /// Half the strip's width in the world: how far toward the sun a shadow lookup is moved off the strip (a strip faces the
    /// camera and its shadow caster the sun, so the two are not one surface).
    pub reach: f32,
    /// The paint.
    pub color: [u8; 4],
    /// Where the corner is on the texture: `u` in repeats along the strip (by arc length of its curve, so it runs on across
    /// the joints), `v` across it from 0 to 1 (a little outside at the soft edge).
    pub uv: [f32; 2],
    /// How fast the texture runs along the strip, repeats a second.
    pub scroll: f32,
    /// How bright a glowing strip is.
    pub intensity: f32,
}

/// The strips of a [`StripSet`] as triangles for one camera.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StripMesh {
    /// The corners, two for each sample of a strip, the caps among them.
    pub vertices: Vec<StripVertex>,
    /// Three indices a triangle.
    pub indices: Vec<u32>,
    /// The index runs by texture (a run is a stretch of strips in order with one texture).
    pub draws: Vec<Draw>,
}

/// How far along each sample of a curve is, in repeats of a texture `repeat` long.
fn arc_repeats(samples: &[Vec3], repeat: f32) -> Vec<f32> {
    let mut along = vec![0.0f32; samples.len()];
    for i in 1..samples.len() {
        along[i] = along[i - 1] + samples[i].distance(samples[i - 1]) / repeat;
    }
    along
}

impl StripMesh {
    fn push_draw(&mut self, texture: u32, casting: bool, additive: bool, before: u32) {
        let count = self.indices.len() as u32 - before;
        if count == 0 {
            return;
        }
        match self.draws.last_mut() {
            Some(d) if d.texture == texture && d.casting == casting && d.additive == additive => {
                d.count += count
            }
            _ => self.draws.push(Draw {
                texture,
                casting,
                additive,
                first: before,
                count,
            }),
        }
    }
}

/// 0 below `a`, 1 above `b`, smooth between.
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The longest a joint's offset is made, as a multiple of the half width (a miter's limit: a sharper turn, seen from the camera,
/// is cut short instead of spiking).
pub const MITER_LIMIT: f32 = 2.0;

/// The width at a joint of a strip as the camera sees it: the unit direction `side` (flat to the screen, across the strip) and
/// the factor `scale` that carries the joint's two edges to where the pieces on either side of it meet.
///
/// `prev` is the way from the piece before to the joint and `next` the way from the joint to the piece after (None at an end);
/// `eye` is the unit direction to the camera. Each piece has a normal across it in the screen's plane; the joint's `side` is the
/// bisector of the two, so the edges of the two pieces line up at the joint with no gap or overlap, and `scale` is 1 over the
/// cosine of half the turn (a miter), at most [`MITER_LIMIT`]. A piece that runs along the line of sight has no usable normal and
/// counts for less, and for nothing if it is along it exactly; with neither, the width turns to the camera's right (or up). The
/// width is turned to face the way `reference` does (the joint before), so the strip does not twist from one joint to the next.
fn joint(
    axes: [Vec3; 2],
    eye: Vec3,
    prev: Option<Vec3>,
    next: Option<Vec3>,
    reference: Option<Vec3>,
) -> (Vec3, f32) {
    let in_plane = |v: Vec3| (v - eye * v.dot(eye)).try_normalize();
    let fallback = in_plane(axes[0])
        .or_else(|| in_plane(axes[1]))
        .unwrap_or_else(|| eye.any_orthonormal_vector());
    let turned = |v: Vec3, to: Vec3| if v.dot(to) < 0.0 { -v } else { v };
    let reference = reference.unwrap_or(fallback);
    let fallback = turned(fallback, reference);
    let mut sum = Vec3::ZERO;
    let mut weight = 0.0;
    let mut solid = [None, None];
    for (k, piece) in [prev, next].into_iter().enumerate() {
        let Some(v) = piece else { continue };
        let cross = eye.cross(v);
        let (Some(n), Some(len)) = (cross.try_normalize(), v.try_normalize().map(|_| v.length()))
        else {
            continue;
        };
        let n = turned(n, reference);
        let w = smoothstep(0.02, 0.25, cross.length() / len);
        sum += n * w;
        weight += w;
        if w >= 0.5 {
            solid[k] = Some(n);
        }
    }
    let raw = sum + fallback * (1.0 - f32::min(weight, 1.0));
    let side = in_plane(raw).unwrap_or(fallback);
    let cosine = solid
        .into_iter()
        .flatten()
        .map(|n| side.dot(n))
        .fold(1.0, f32::min);
    (side, 1.0 / cosine.max(1.0 / MITER_LIMIT))
}

impl StripMesh {
    /// The triangles of `set` for a camera.
    pub fn build(set: &StripSet, view: &StripView) -> StripMesh {
        let mut mesh = StripMesh::default();
        for strip in &set.strips {
            mesh.add(strip, view);
        }
        mesh
    }

    /// The shadow casters of `set` for a sun in the direction `towards_sun` (a unit vector): each strip that casts a shadow as a
    /// band of its width that faces the sun, so its shadow does not change as the camera moves. The ends are not capped.
    pub fn build_shadow(set: &StripSet, towards_sun: Vec3) -> StripMesh {
        let mut mesh = StripMesh::default();
        let sun = towards_sun.try_normalize().unwrap_or(Vec3::Z);
        let right = sun.any_orthonormal_vector();
        let axes = [right, sun.cross(right)];
        for strip in set.strips.iter().filter(|s| s.cast_shadow) {
            let s = &strip.samples;
            if s.len() < 2 {
                continue;
            }
            let first = mesh.vertices.len() as u32;
            let before = mesh.indices.len() as u32;
            let along = arc_repeats(s, strip.texture_length);
            let mut side = None;
            for (i, &p) in s.iter().enumerate() {
                let prev = i.checked_sub(1).map(|j| p - s[j]);
                let next = s.get(i + 1).map(|&q| q - p);
                let (w, miter) = joint(axes, sun, prev, next, side);
                side = Some(w);
                let share = if along[s.len() - 1] > 0.0 {
                    (along[i] / along[s.len() - 1]).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let width = strip.width + (strip.width_end - strip.width) * share;
                for across in [-1.0f32, 1.0] {
                    mesh.vertices.push(StripVertex {
                        position: (p + w * (width * 0.5 * miter * across)).to_array(),
                        across,
                        half_pixels: 0.0,
                        flags: 0.0,
                        reach: 0.0,
                        color: strip.color,
                        uv: [along[i], (across + 1.0) * 0.5],
                        scroll: strip.scroll,
                        intensity: 1.0,
                    });
                }
            }
            for q in 0..s.len() as u32 - 1 {
                let a = first + q * 2;
                mesh.indices.extend([a, a + 1, a + 3, a, a + 3, a + 2]);
            }
            mesh.push_draw(strip.texture, true, false, before);
        }
        mesh
    }

    /// The vertices as the shader reads them (little endian).
    pub fn vertex_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.vertices.len() * STRIP_VERTEX_BYTES);
        for v in &self.vertices {
            for c in v
                .position
                .into_iter()
                .chain([v.across, v.half_pixels, v.flags, v.reach])
            {
                out.extend_from_slice(&c.to_le_bytes());
            }
            out.extend_from_slice(&v.color);
            for c in [v.uv[0], v.uv[1], v.scroll, v.intensity] {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        out
    }

    /// The indices as the shader reads them (little endian).
    pub fn index_bytes(&self) -> Vec<u8> {
        self.indices.iter().flat_map(|i| i.to_le_bytes()).collect()
    }

    fn add(&mut self, strip: &Strip, view: &StripView) {
        let s = &strip.samples;
        let Some(last) = s.len().checked_sub(1) else {
            return;
        };
        if last == 0 {
            return;
        }
        let mut tangents = Vec::with_capacity(s.len());
        let mut kept = (s[last] - s[0]).try_normalize().unwrap_or(Vec3::X);
        for i in 0..=last {
            let t = (s[(i + 1).min(last)] - s[i.saturating_sub(1)])
                .try_normalize()
                .unwrap_or(kept);
            kept = t;
            tangents.push(t);
        }
        // Each sample as (place, direction, half width in the world with the edge's margin, half width in pixels with it).
        let half = |p: Vec3, share: f32| {
            let pixel = view.pixel_at(p);
            let width = strip.width + (strip.width_end - strip.width) * share;
            let world = (width * 0.5).max(MIN_PIXELS * 0.5 * pixel);
            (
                world,
                world + EDGE_PIXELS * pixel,
                world / pixel + EDGE_PIXELS,
            )
        };
        // Each cut as (place, half width with the margin, half width in pixels, u, v's scale): the caps run the texture on past the ends
        // by their length, and the margin's corners are a little outside the texture's v from 0 to 1.
        let along = arc_repeats(s, strip.texture_length);
        let share = |i: usize| {
            if along[last] > 0.0 {
                (along[i] / along[last]).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        let mut cuts = Vec::with_capacity(s.len() + 2);
        let (cap, hw, hp) = half(s[0], 0.0);
        cuts.push((
            s[0] - tangents[0] * cap,
            hw,
            hp,
            along[0] - cap / strip.texture_length,
            hw / cap,
            0.0,
        ));
        for (i, &p) in s.iter().enumerate() {
            let (world, hw, hp) = half(p, share(i));
            cuts.push((p, hw, hp, along[i], hw / world, share(i)));
        }
        let (cap, hw, hp) = half(s[last], 1.0);
        cuts.push((
            s[last] + tangents[last] * cap,
            hw,
            hp,
            along[last] + cap / strip.texture_length,
            hw / cap,
            1.0,
        ));
        let paint = |share: f32| {
            let end = strip.color_end.unwrap_or(strip.color);
            let mix =
                |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * share).round() as u8;
            [
                mix(strip.color[0], end[0]),
                mix(strip.color[1], end[1]),
                mix(strip.color[2], end[2]),
                mix(strip.color[3], end[3]),
            ]
        };

        let first = self.vertices.len() as u32;
        let before = self.indices.len() as u32;
        let flags = if strip.emissive { FLAG_GLOW } else { 0.0 }
            + if strip.receive_shadow {
                FLAG_RECEIVE
            } else {
                0.0
            };
        let mut side = None;
        for (i, &(p, hw, hp, u, vs, along_share)) in cuts.iter().enumerate() {
            let to_eye = (view.eye - p).try_normalize().unwrap_or(-view.forward);
            let prev = i.checked_sub(1).map(|j| p - cuts[j].0);
            let next = cuts.get(i + 1).map(|c| c.0 - p);
            let (w, miter) = joint([view.right, view.up], to_eye, prev, next, side);
            side = Some(w);
            for across in [-1.0f32, 1.0] {
                self.vertices.push(StripVertex {
                    position: (p + w * (hw * miter * across)).to_array(),
                    across,
                    half_pixels: hp,
                    flags,
                    reach: hw,
                    color: paint(along_share),
                    uv: [u, 0.5 + across * 0.5 * vs],
                    scroll: strip.scroll,
                    intensity: strip.intensity,
                });
            }
        }
        for q in 0..cuts.len() as u32 - 1 {
            let a = first + q * 2;
            self.indices.extend([a, a + 1, a + 3, a, a + 3, a + 2]);
        }
        self.push_draw(strip.texture, false, strip.additive, before);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a rope with `params` is drawn with when a script set nothing on it.
    fn look(params: &RopeParams) -> Appearance {
        Appearance {
            color: params.color,
            emissive: params.emissive,
            texture: params.texture,
            scroll: params.texture_scroll,
        }
    }

    /// A camera at `eye` looking along `forward` (z up), 540 pixels high, a 90 degree field.
    fn view(eye: Vec3, forward: Vec3) -> StripView {
        let forward = forward.normalize();
        let right = forward.cross(Vec3::Z).try_normalize().unwrap_or(Vec3::X);
        StripView {
            eye,
            right,
            up: right.cross(forward),
            forward,
            pixel: 2.0 / 540.0,
        }
    }

    fn ribbon(points: &[Vec3], width: f32) -> StripSet {
        let params = RopeParams {
            style: RopeStyle::Ribbon,
            thickness: width,
            ..RopeParams::default()
        };
        StripSet::build(1, std::iter::once((points, &params, look(&params))))
    }

    /// The cross-sections of a mesh: the two vertices of each cut.
    fn cuts(mesh: &StripMesh) -> Vec<([Vec3; 2], [f32; 2])> {
        mesh.vertices
            .chunks(2)
            .map(|c| {
                (
                    [Vec3::from(c[0].position), Vec3::from(c[1].position)],
                    [c[0].half_pixels, c[1].half_pixels],
                )
            })
            .collect()
    }

    fn cable() -> Vec<Vec3> {
        (0..=8)
            .map(|i| {
                let x = i as f32 - 4.0;
                Vec3::new(x, 0.3 * (x * 0.9).sin(), -0.1 * x * x + 1.6)
            })
            .collect()
    }

    #[test]
    fn the_spline_passes_through_every_point() {
        let points = cable();
        let curve = spline(&points, 4);
        assert_eq!(curve.len(), (points.len() - 1) * 4 + 1);
        for (k, p) in points.iter().enumerate() {
            assert!(curve[k * 4].distance(*p) < 1.0e-5, "point {k}");
        }
        // Between the points it is smooth: no sample strays far from the chord of its piece.
        for (k, w) in points.windows(2).enumerate() {
            for j in 1..4 {
                let c = curve[k * 4 + j];
                let near = (0..=8)
                    .map(|i| w[0].lerp(w[1], i as f32 / 8.0).distance(c))
                    .fold(f32::MAX, f32::min);
                assert!(near < 0.25, "piece {k} sample {j}: {near}");
            }
        }
    }

    #[test]
    fn the_spline_of_a_straight_run_stays_straight_and_short_runs_are_kept() {
        let line: Vec<Vec3> = (0..5).map(|i| Vec3::new(i as f32, 0.0, 2.0)).collect();
        for p in spline(&line, 4) {
            assert!(p.y.abs() < 1.0e-6 && (p.z - 2.0).abs() < 1.0e-6);
        }
        let two = [Vec3::ZERO, Vec3::ONE];
        assert_eq!(spline(&two, 4), two);
        assert_eq!(spline(&cable(), 1), cable());
        assert!(spline(&[], 4).is_empty());
    }

    #[test]
    fn a_strip_faces_the_camera_and_is_as_wide_as_its_thickness() {
        let eye = Vec3::new(0.5, -9.0, 2.5);
        let v = view(eye, Vec3::new(0.0, 1.0, -0.1));
        let mesh = StripMesh::build(&ribbon(&cable(), 0.3), &v);
        let sections = cuts(&mesh);
        // Samples, and a cap at each end.
        assert_eq!(sections.len(), 8 * SUBDIVISIONS + 1 + 2);
        for ([a, b], _) in &sections {
            let d = *b - *a;
            let center = (*a + *b) * 0.5;
            let to_eye = (eye - center).normalize();
            assert!(d.normalize().dot(to_eye).abs() < 1.0e-4, "flat to the view");
            let want = 0.3 + v.pixel_at(center);
            assert!((d.length() - want).abs() < 2.0e-3, "{}", d.length());
        }
        for pair in sections.windows(2) {
            let (d0, d1) = (pair[0].0[1] - pair[0].0[0], pair[1].0[1] - pair[1].0[0]);
            assert!(d0.dot(d1) > 0.0, "the strip does not twist");
        }
        assert!(
            mesh.indices
                .iter()
                .all(|&i| (i as usize) < mesh.vertices.len())
        );
        assert_eq!(mesh.indices.len(), (sections.len() - 1) * 6);
        assert!(mesh.vertices.iter().all(|v| v.across.abs() == 1.0));
    }

    #[test]
    fn a_strip_turns_with_the_camera() {
        let points = cable();
        let set = ribbon(&points, 0.2);
        for eye in [
            Vec3::new(0.0, -9.0, 1.5),
            Vec3::new(0.0, 9.0, 4.0),
            Vec3::new(1.0, 0.5, 12.0),
        ] {
            let v = view(eye, Vec3::new(0.0, 0.0, 1.0) - eye);
            for ([a, b], _) in cuts(&StripMesh::build(&set, &v)) {
                let d = b - a;
                let to_eye = (eye - (a + b) * 0.5).normalize();
                assert!(d.normalize().dot(to_eye).abs() < 1.0e-4, "{eye}");
                assert!((d.length() - 0.2 - v.pixel_at((a + b) * 0.5)).abs() < 2.0e-3);
            }
        }
    }

    #[test]
    fn a_far_strip_keeps_a_minimum_width_on_the_screen() {
        let points = [Vec3::new(-2.0, 400.0, 3.0), Vec3::new(2.0, 400.0, 3.0)];
        let set = ribbon(&points, 0.01);
        let v = view(Vec3::new(0.0, 0.0, 3.0), Vec3::Y);
        let mesh = StripMesh::build(&set, &v);
        let pixel = v.pixel_at(Vec3::new(0.0, 400.0, 3.0));
        for ([a, b], half) in cuts(&mesh) {
            let on_screen = (b - a).length() / v.pixel_at(a);
            let want = MIN_PIXELS + 2.0 * EDGE_PIXELS;
            assert!((on_screen - want).abs() < 0.05, "{on_screen} pixels");
            assert!((half[0] - want * 0.5).abs() < 0.05);
        }
        assert!(
            pixel * MIN_PIXELS > 0.01,
            "the true width is less than that"
        );
        // Near, the true width wins.
        let near = StripMesh::build(
            &ribbon(&[Vec3::new(-2.0, 2.0, 3.0), Vec3::new(2.0, 2.0, 3.0)], 0.5),
            &v,
        );
        for ([a, b], half) in cuts(&near) {
            assert!(((b - a).length() - 0.5 - v.pixel_at(a)).abs() < 1.0e-4);
            assert!(half[0] > MIN_PIXELS * 0.5 + EDGE_PIXELS);
        }
    }

    #[test]
    fn looking_along_a_strip_leaves_a_width_and_no_nan() {
        let points: Vec<Vec3> = (0..=6)
            .map(|i| Vec3::new(0.0, i as f32 * 1.5, 2.0))
            .collect();
        let set = ribbon(&points, 0.2);
        // Right down the strip, and a little off it either way.
        for (eye, forward) in [
            (Vec3::new(0.0, -8.0, 2.0), Vec3::Y),
            (Vec3::new(0.0, -8.0, 2.0), Vec3::new(0.004, 1.0, 0.0)),
            (Vec3::new(0.001, -8.0, 2.002), Vec3::Y),
            (Vec3::new(0.0, 20.0, 2.0), -Vec3::Y),
        ] {
            let v = view(eye, forward);
            let mesh = StripMesh::build(&set, &v);
            assert!(
                mesh.vertices
                    .iter()
                    .all(|c| c.position.iter().all(|x| x.is_finite()))
            );
            let sections = cuts(&mesh);
            for ([a, b], _) in &sections {
                let want = 0.2 + v.pixel_at((*a + *b) * 0.5);
                assert!(
                    ((*b - *a).length() - want).abs() < 1.0e-3,
                    "{eye} {forward}"
                );
            }
            for pair in sections.windows(2) {
                let (d0, d1) = (pair[0].0[1] - pair[0].0[0], pair[1].0[1] - pair[1].0[0]);
                assert!(d0.dot(d1) > 0.0, "it does not twist: {eye} {forward}");
            }
            // The caps give the end-on strip some area: the first and last cut are not at one place with their neighbors.
            let first = (sections[0].0[0] + sections[0].0[1]) * 0.5;
            let second = (sections[1].0[0] + sections[1].0[1]) * 0.5;
            assert!(
                first.distance(second) >= 0.099,
                "{}",
                first.distance(second)
            );
        }
    }

    /// The distance from `p` to the infinite line through `a` and `b`.
    fn line_distance(p: Vec3, a: Vec3, b: Vec3) -> f32 {
        let d = (b - a).normalize();
        (p - a - d * (p - a).dot(d)).length()
    }

    #[test]
    fn the_joints_are_mitered_so_the_edges_of_neighboring_pieces_line_up() {
        // A right-angle corner seen from the front: the strip's edges stay as far from each piece's line as from the other
        // end of that piece.
        let corner = [
            Vec3::new(-3.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 3.0),
        ];
        let set = StripSet {
            revision: 1,
            strips: vec![Strip {
                samples: corner.to_vec(),
                width: 0.4,
                color: [255; 4],
                emissive: false,
                cast_shadow: false,
                receive_shadow: true,
                texture: 0,
                texture_length: 0.1,
                scroll: 0.0,
                width_end: 0.4,
                color_end: None,
                intensity: 1.0,
                additive: false,
            }],
        };
        let v = view(Vec3::new(0.0, -10.0, 1.0), Vec3::Y);
        let mesh = StripMesh::build(&set, &v);
        let sections = cuts(&mesh);
        assert_eq!(sections.len(), 5);
        let side_of = |k: usize, line: [Vec3; 2]| {
            let [a, b] = sections[k].0;
            (
                line_distance(a, line[0], line[1]),
                line_distance(b, line[0], line[1]),
            )
        };
        let first = [corner[0], corner[1]];
        let (a0, b0) = side_of(1, first);
        let (a1, b1) = side_of(2, first);
        let want = a0;
        for d in [b0, a1, b1] {
            assert!((d - want).abs() < 1.0e-3, "{d} against {want}");
        }
        // The corner's offset is longer than the half width by the miter (about the square root of two).
        let width = (sections[2].0[1] - sections[2].0[0]).length();
        let straight = (sections[1].0[1] - sections[1].0[0]).length();
        assert!(
            (width / straight - 2.0f32.sqrt()).abs() < 0.05,
            "{width} {straight}"
        );
    }

    #[test]
    fn a_sharp_turn_is_cut_short_by_the_miter_limit() {
        // A hairpin: the way back is the way there.
        let hairpin = [
            Vec3::new(-3.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(-3.0, 0.0, 0.05),
        ];
        let set = StripSet {
            revision: 1,
            strips: vec![Strip {
                samples: hairpin.to_vec(),
                width: 0.4,
                color: [255; 4],
                emissive: false,
                cast_shadow: false,
                receive_shadow: true,
                texture: 0,
                texture_length: 0.1,
                scroll: 0.0,
                width_end: 0.4,
                color_end: None,
                intensity: 1.0,
                additive: false,
            }],
        };
        let v = view(Vec3::new(0.0, -10.0, 1.0), Vec3::Y);
        let sections = cuts(&StripMesh::build(&set, &v));
        let straight = (sections[1].0[1] - sections[1].0[0]).length();
        for ([a, b], _) in &sections {
            let w = (*b - *a).length();
            assert!(w.is_finite() && w <= straight * MITER_LIMIT + 1.0e-3, "{w}");
        }
    }

    #[test]
    fn a_shadow_caster_faces_the_sun_and_is_as_wide_as_the_thickness() {
        let sun = Vec3::new(0.3, -0.4, 0.86).normalize();
        let set = ribbon(&cable(), 0.3);
        let mesh = StripMesh::build_shadow(&set, sun);
        let sections = cuts(&mesh);
        // The samples, with no caps.
        assert_eq!(sections.len(), 8 * SUBDIVISIONS + 1);
        for ([a, b], _) in &sections {
            let d = *b - *a;
            assert!(d.normalize().dot(sun).abs() < 1.0e-4, "flat to the sun");
            assert!(d.length() >= 0.3 - 1.0e-4 && d.length() <= 0.3 * MITER_LIMIT + 1.0e-4);
        }
        for pair in sections.windows(2) {
            let (d0, d1) = (pair[0].0[1] - pair[0].0[0], pair[1].0[1] - pair[1].0[0]);
            assert!(d0.dot(d1) > 0.0, "it does not twist");
        }
        // The camera has no say: the same band whoever looks, unlike the strip drawn for the camera.
        let seen = StripMesh::build(&set, &view(Vec3::new(0.5, -9.0, 2.5), Vec3::Y));
        assert_ne!(seen.vertices.len(), mesh.vertices.len());
        // Straight along the ground the band is exactly `thickness` wide as the sun's light lands (measured across the way it runs).
        let straight = ribbon(&[Vec3::new(-3.0, 0.0, 2.0), Vec3::new(3.0, 0.0, 2.0)], 0.3);
        for ([a, b], _) in cuts(&StripMesh::build_shadow(&straight, sun)) {
            assert!(((b - a).length() - 0.3).abs() < 1.0e-4);
            assert!((b - a).dot(Vec3::X).abs() < 1.0e-4, "across the run");
        }
    }

    #[test]
    fn only_the_strips_that_cast_are_shadow_casters() {
        let points = [Vec3::ZERO, Vec3::X, Vec3::new(2.0, 1.0, 0.0)];
        let params = RopeParams {
            style: RopeStyle::Ribbon,
            cast_shadow: false,
            ..RopeParams::default()
        };
        let none = StripSet::build(1, std::iter::once((&points[..], &params, look(&params))));
        assert!(!none.strips[0].cast_shadow);
        assert!(StripMesh::build_shadow(&none, Vec3::Z).vertices.is_empty());
        let casting = RopeParams {
            cast_shadow: true,
            ..params
        };
        let some = StripSet::build(1, std::iter::once((&points[..], &casting, look(&casting))));
        assert!(!StripMesh::build_shadow(&some, Vec3::Z).vertices.is_empty());
    }

    #[test]
    fn the_texture_runs_on_by_arc_length_along_the_curve_and_across_the_width() {
        let points = cable();
        let mut params = RopeParams {
            style: RopeStyle::Ribbon,
            thickness: 0.2,
            texture_length: 0.5,
            texture_scroll: 2.0,
            ..RopeParams::default()
        };
        params.texture = 3;
        let set = StripSet::build(1, std::iter::once((&points[..], &params, look(&params))));
        let strip = &set.strips[0];
        assert_eq!(
            (strip.texture, strip.texture_length, strip.scroll),
            (3, 0.5, 2.0)
        );
        let mesh = StripMesh::build(&set, &view(Vec3::new(0.5, -9.0, 2.5), Vec3::Y));
        let corners: Vec<&[StripVertex]> = mesh.vertices.chunks(2).collect();
        // Both corners of a cut are at one u; u grows by the distance between the samples over the repeat, with no jump at the
        // joints (the first and the last cut are the caps, which run the texture on a half width past the ends).
        for pair in &corners {
            assert_eq!(pair[0].uv[0], pair[1].uv[0]);
            assert!(pair[0].uv[1] < 0.5 && pair[1].uv[1] > 0.5);
            assert!(pair.iter().all(|v| v.scroll == 2.0));
        }
        let samples = &strip.samples;
        for (i, w) in samples.windows(2).enumerate() {
            let du = corners[i + 2][0].uv[0] - corners[i + 1][0].uv[0];
            let want = w[0].distance(w[1]) / 0.5;
            assert!((du - want).abs() < 1.0e-4, "piece {i}: {du} {want}");
        }
        let total: f32 = samples.windows(2).map(|w| w[0].distance(w[1])).sum::<f32>() / 0.5;
        let (first, last) = (corners[1][0].uv[0], corners[samples.len()][0].uv[0]);
        assert!(
            first.abs() < 1.0e-6 && (last - total).abs() < 1.0e-3,
            "{first} {last} {total}"
        );
        // The soft edge's corners lie a little outside the texture's v from 0 to 1.
        assert!(corners[2][0].uv[1] < 0.0 + 1.0e-6 || corners[2][0].uv[1] <= 0.05);
        assert!(corners[2][1].uv[1] >= 0.95);
        // One run of indices, with the texture.
        assert_eq!(mesh.draws.len(), 1);
        assert_eq!(mesh.draws[0].texture, 3);
        assert_eq!(mesh.draws[0].count as usize, mesh.indices.len());
    }

    #[test]
    fn strips_of_other_textures_make_runs_in_rope_order() {
        let points = [Vec3::ZERO, Vec3::X, Vec3::new(2.0, 1.0, 0.0)];
        let mut set = StripSet::default();
        for texture in [0, 0, 2, 0] {
            let params = RopeParams {
                style: RopeStyle::Ribbon,
                texture,
                ..RopeParams::default()
            };
            set.strips.extend(
                StripSet::build(1, std::iter::once((&points[..], &params, look(&params)))).strips,
            );
        }
        let mesh = StripMesh::build(&set, &view(Vec3::new(0.0, -9.0, 2.0), Vec3::Y));
        let runs: Vec<(u32, u32)> = mesh.draws.iter().map(|d| (d.texture, d.count)).collect();
        let one = mesh.indices.len() as u32 / 4;
        assert_eq!(runs, [(0, 2 * one), (2, one), (0, one)]);
        assert_eq!(mesh.draws[1].first, 2 * one);
    }

    #[test]
    fn a_strip_tapers_and_blends_its_color_along_its_length() {
        let points: Vec<Vec3> = (0..=4)
            .map(|i| Vec3::new(0.0, 2.0 + i as f32, 2.0))
            .collect();
        let params = RopeParams {
            style: RopeStyle::Ribbon,
            thickness: 0.6,
            thickness_end: 0.1,
            color: [255, 0, 0, 255],
            color_end: Some([0, 0, 255, 0]),
            emissive: true,
            emissive_intensity: 4.0,
            additive: true,
            ..RopeParams::default()
        };
        let set = StripSet::build(1, std::iter::once((&points[..], &params, look(&params))));
        let strip = &set.strips[0];
        assert_eq!(
            (strip.width_end, strip.intensity, strip.additive),
            (0.1, 4.0, true)
        );
        let mesh = StripMesh::build(&set, &view(Vec3::new(0.0, -6.0, 2.0), Vec3::Y));
        let cuts = cuts(&mesh);
        // Wide at the start, thin at the end, narrowing all along.
        let widths: Vec<f32> = cuts.iter().map(|([a, b], _)| (*b - *a).length()).collect();
        assert!(
            widths[1] > 0.55 && widths[widths.len() - 2] < 0.2,
            "{widths:?}"
        );
        assert!(
            widths.windows(2).all(|w| w[1] <= w[0] + 1.0e-3),
            "{widths:?}"
        );
        // The paint runs from the color to the color at the end, by arc length, and a strip that glows has its brightness.
        let at = |cut: usize| mesh.vertices[cut * 2];
        assert_eq!(at(1).color, [255, 0, 0, 255]);
        assert_eq!(at(cuts.len() - 2).color, [0, 0, 255, 0]);
        let middle = at(1 + 8).color;
        assert!(
            middle[0] > 100 && middle[2] > 100 && (100..160).contains(&middle[3]),
            "{middle:?}"
        );
        assert!(mesh.vertices.iter().all(|v| v.intensity == 4.0));
        // Additive strips are their own runs.
        assert!(mesh.draws.iter().all(|d| d.additive));
        let plain = RopeParams {
            additive: false,
            ..params
        };
        let both = StripSet {
            revision: 1,
            strips: [params, plain]
                .iter()
                .map(|p| {
                    StripSet::build(1, std::iter::once((&points[..], p, look(p))))
                        .strips
                        .remove(0)
                })
                .collect(),
        };
        let mesh = StripMesh::build(&both, &view(Vec3::new(0.0, -6.0, 2.0), Vec3::Y));
        let runs: Vec<bool> = mesh.draws.iter().map(|d| d.additive).collect();
        assert_eq!(runs, [true, false]);
        // The shadow band tapers too.
        let band = StripMesh::build_shadow(&both, Vec3::Z);
        let [first, last] = [&band.vertices[..2], &band.vertices[32..34]]
            .map(|c| (Vec3::from(c[1].position) - Vec3::from(c[0].position)).length());
        assert!(first > 0.55 && last < 0.2, "{first} {last}");
    }

    #[test]
    fn a_line_is_its_two_ends() {
        let params = RopeParams {
            style: RopeStyle::Line,
            thickness: 0.04,
            color: [255, 0, 0, 200],
            emissive: true,
            receive_shadow: false,
            ..RopeParams::default()
        };
        let ends = [Vec3::new(-3.0, 0.0, 1.0), Vec3::new(5.0, 2.0, 1.5)];
        let set = StripSet::build(7, std::iter::once((&ends[..], &params, look(&params))));
        assert_eq!(set.revision, 7);
        assert_eq!(set.strips.len(), 1);
        assert_eq!(set.strips[0].samples, ends);
        let mesh = StripMesh::build(&set, &view(Vec3::new(0.0, -8.0, 1.0), Vec3::Y));
        assert_eq!(cuts(&mesh).len(), 4);
        assert!(
            mesh.vertices
                .iter()
                .all(|v| v.flags == FLAG_GLOW && v.color == [255, 0, 0, 200])
        );
    }

    #[test]
    fn tubes_and_broken_ropes_are_left_out() {
        let good = [Vec3::ZERO, Vec3::X, Vec3::new(2.0, 1.0, 0.0)];
        let bad = [Vec3::ZERO, Vec3::new(f32::NAN, 0.0, 0.0)];
        let one = [Vec3::ZERO];
        let tube = RopeParams::default();
        let ribbon_params = RopeParams {
            style: RopeStyle::Ribbon,
            ..RopeParams::default()
        };
        let set = StripSet::build(
            1,
            [
                (&good[..], &tube, look(&tube)),
                (&bad[..], &ribbon_params, look(&ribbon_params)),
                (&one[..], &ribbon_params, look(&ribbon_params)),
                (&good[..], &ribbon_params, look(&ribbon_params)),
            ]
            .into_iter(),
        );
        assert_eq!(set.strips.len(), 1);
        assert_eq!(set.strips[0].samples.len(), 2 * SUBDIVISIONS + 1);
    }

    #[test]
    fn the_bytes_are_the_vertices_and_indices() {
        let mesh = StripMesh::build(
            &ribbon(&cable(), 0.1),
            &view(Vec3::new(0.0, -9.0, 2.0), Vec3::Y),
        );
        assert_eq!(
            mesh.vertex_bytes().len(),
            mesh.vertices.len() * STRIP_VERTEX_BYTES
        );
        assert_eq!(mesh.index_bytes().len(), mesh.indices.len() * 4);
    }
}
