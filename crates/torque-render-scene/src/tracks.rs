//! Tire tracks: strips laid on the ground behind the wheels of the vehicles the client runs, as a game asks for them
//! ([`TrackRules`]; TGE has none, and no datablock carries them, so the wire stays TGE's).
//!
//! - **Laid** ([`TireTracks::update`]): a wheel in contact with the ground lays a segment each time its contact point has gone
//!   [`TrackRules::spacing`] from the end of its strip, as wide as [`TrackRules::width`] (the game's: its tires' width). The
//!   segment is the ground's triangles under it (the host's `surfaces` near the point, [`crate::decals::Triangle`]) clipped
//!   to it: between the plane across the strip at its last end (the plane the last segment ended on, so a curve leaves no
//!   gap and no overlap) and the plane across its new end, and between its two sides, each triangle facing up within
//!   [`crate::decals::CLIP_ANGLE_COS`]. So it lies on the terrain over the hills, cut where the triangles meet, never in the
//!   air. A wheel that leaves the ground, or jumps further than [`MAX_GAP`] (a respawn), ends its strip.
//! - **Textured**: [`TrackRules::texture`] across the strip (u from side to side) and repeating along it every strip's width
//!   (v by the distance laid).
//! - **Kept**: each segment for [`TrackRules::lifetime`] seconds, fading over its last [`TrackRules::fade`]; past
//!   [`TrackRules::max_segments`] the oldest goes first.
//! - **Drawn** ([`TireTracks::draws`]), in a draw for each of [`FADE_STEPS`] steps of their fade: multiplied over the ground ([`Blend::Modulate2x`]: 0.5 gray leaves it as it is, so the
//!   texture darkens the grass and takes its light and shadow), fading toward 0.5 gray, depth-tested without depth writes and
//!   drawn [`TRACK_DEPTH_BIAS`] of their distance nearer the eye (no fighting with the ground), unlit and fogged.

use std::collections::{BTreeMap, VecDeque};

use crate::decals::{CLIP_ANGLE_COS, DecalPoint, Triangle, clip};
use crate::textures::TextureSet;
use crate::{Blend, Cull, Draw, Filter, Sampler, TexEnv, Vertex};

/// The tracks' depth bias: each vertex drawn this share of its distance nearer the eye along the eye's ray (more than a
/// decal's: a strip lies along the ground at a glancing angle far off).
pub const TRACK_DEPTH_BIAS: f32 = 0.008;

/// The steps of opacity the segments are drawn in (one draw each).
pub const FADE_STEPS: u32 = 16;

/// The furthest a contact point may move between frames and still continue its strip (units).
pub const MAX_GAP: f32 = 4.0;

/// A game's tire tracks.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackRules {
    /// The strip's width (units).
    pub width: f32,
    /// How far a wheel goes between two segments (units).
    pub spacing: f32,
    /// The texture (a path the reader finds), multiplied over the ground.
    pub texture: String,
    /// Seconds each segment is kept, and how long of that it fades out.
    pub lifetime: f32,
    pub fade: f32,
    /// The most segments kept at once.
    pub max_segments: usize,
}

impl Default for TrackRules {
    fn default() -> TrackRules {
        TrackRules {
            width: 0.4,
            spacing: 0.5,
            texture: String::new(),
            lifetime: 25.0,
            fade: 8.0,
            max_segments: 2048,
        }
    }
}

/// A wheel as the tracks see it this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackWheel {
    /// The vehicle's ghost and the wheel's number.
    pub key: (u32, usize),
    pub contact: bool,
    /// The contact point.
    pub point: [f32; 3],
}

/// The host's ground near a point: its triangles within a reach.
pub type Surfaces<'a> = dyn Fn([f32; 3], f32) -> Vec<Triangle> + 'a;

/// One segment laid.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackSegment {
    pub triangles: Vec<[DecalPoint; 3]>,
    /// Its middle (where it sorts among the translucent draws).
    pub center: [f32; 3],
    /// Seconds since it was laid.
    pub age: f32,
}

/// The end of a wheel's strip.
#[derive(Debug, Clone, Copy, PartialEq)]
struct End {
    point: [f32; 3],
    /// The horizontal direction the strip ended along (the plane its last segment ended on).
    along: [f32; 2],
    /// The texture's v at the end.
    v: f32,
}

/// The tire tracks of the vehicles the client runs (module docs).
#[derive(Debug, Clone)]
pub struct TireTracks {
    pub rules: TrackRules,
    segments: VecDeque<TrackSegment>,
    ends: BTreeMap<(u32, usize), End>,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn flat(v: [f32; 3]) -> Option<[f32; 2]> {
    let l = v[0].hypot(v[1]);
    (l > 1e-4 && l.is_finite()).then(|| [v[0] / l, v[1] / l])
}

impl TireTracks {
    pub fn new(rules: TrackRules) -> TireTracks {
        TireTracks {
            rules,
            segments: VecDeque::new(),
            ends: BTreeMap::new(),
        }
    }

    /// The segments kept, oldest first.
    pub fn segments(&self) -> impl Iterator<Item = &TrackSegment> {
        self.segments.iter()
    }

    pub fn len(&self) -> usize {
        self.segments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Forgets every segment and strip.
    pub fn clear(&mut self) {
        self.segments.clear();
        self.ends.clear();
    }

    /// The frame, `dt` seconds after the last: the segments aged (the ones past their lifetime gone), then each wheel's strip
    /// laid on to where it touches the ground (module docs). Wheels not given end their strips.
    pub fn update(&mut self, wheels: &[TrackWheel], surfaces: &Surfaces<'_>, dt: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        for s in &mut self.segments {
            s.age += dt;
        }
        let lifetime = self.rules.lifetime;
        self.segments.retain(|s| s.age < lifetime);
        let mut kept = BTreeMap::new();
        for w in wheels {
            if !w.contact || w.point.iter().any(|c| !c.is_finite()) {
                continue;
            }
            let Some(end) = self.ends.get(&w.key).copied() else {
                kept.insert(
                    w.key,
                    End {
                        point: w.point,
                        along: [0.0, 0.0],
                        v: 0.0,
                    },
                );
                continue;
            };
            let d = [0, 1, 2].map(|k| w.point[k] - end.point[k]);
            let gone = d[0].hypot(d[1]);
            if gone > MAX_GAP {
                kept.insert(
                    w.key,
                    End {
                        point: w.point,
                        along: [0.0, 0.0],
                        v: 0.0,
                    },
                );
                continue;
            }
            if gone < self.rules.spacing.max(0.01) {
                kept.insert(w.key, end);
                continue;
            }
            let Some(along) = flat(d) else {
                kept.insert(w.key, end);
                continue;
            };
            // The strip's first segment starts square across its own direction.
            let start_along = if end.along == [0.0, 0.0] {
                along
            } else {
                end.along
            };
            let v = end.v + gone / self.rules.width.max(0.01);
            self.lay(end, w.point, start_along, along, surfaces);
            kept.insert(
                w.key,
                End {
                    point: w.point,
                    along,
                    v,
                },
            );
        }
        self.ends = kept;
    }

    /// A segment from `end` to `to`: the ground's triangles between the plane across `start_along` at the end and the plane
    /// across `along` at `to`, and within half the width of the line between them.
    fn lay(
        &mut self,
        end: End,
        to: [f32; 3],
        start_along: [f32; 2],
        along: [f32; 2],
        surfaces: &Surfaces<'_>,
    ) {
        if self.rules.max_segments == 0 {
            return;
        }
        let half = 0.5 * self.rules.width;
        let a = end.point;
        let mid = [0, 1, 2].map(|k| 0.5 * (a[k] + to[k]));
        let side = [-along[1], along[0], 0.0];
        let start = [start_along[0], start_along[1], 0.0];
        let ahead = [along[0], along[1], 0.0];
        let reach = 0.5 * (to[0] - a[0]).hypot(to[1] - a[1]) + half + 1.0;
        let mut triangles = Vec::new();
        for tri in surfaces(mid, reach) {
            let e1 = [0, 1, 2].map(|k| tri[1][k] - tri[0][k]);
            let e2 = [0, 1, 2].map(|k| tri[2][k] - tri[0][k]);
            let n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let len = dot(n, n).sqrt();
            if len < 1e-9 || n[2].abs() / len < CLIP_ANGLE_COS {
                continue;
            }
            let mut poly = tri.to_vec();
            // Past the end the strip ended on, before its new end, within its sides.
            poly = clip(&poly, a, start.map(|c| -c), 0.0);
            poly = clip(&poly, to, ahead, 0.0);
            poly = clip(&poly, mid, side, half);
            poly = clip(&poly, mid, side.map(|c| -c), half);
            if poly.len() < 3 {
                continue;
            }
            // u across from side to side; v on from the end's by the distance along the strip.
            let point = |p: [f32; 3]| -> DecalPoint {
                let r = [p[0] - a[0], p[1] - a[1], 0.0];
                let u = 0.5 + dot(r, side) / (2.0 * half);
                (p, [u, end.v + dot(r, ahead) / self.rules.width.max(0.01)])
            };
            for i in 1..poly.len() - 1 {
                triangles.push([point(poly[0]), point(poly[i]), point(poly[i + 1])]);
            }
        }
        if triangles.is_empty() {
            return;
        }
        while self.segments.len() >= self.rules.max_segments {
            self.segments.pop_front();
        }
        self.segments.push_back(TrackSegment {
            triangles,
            center: mid,
            age: 0.0,
        });
    }

    /// A segment's opacity now: 1 until its fade starts, then down to 0 at the end of its lifetime.
    pub fn opacity(&self, s: &TrackSegment) -> f32 {
        let left = self.rules.lifetime - s.age;
        if self.rules.fade <= 0.0 {
            return if left > 0.0 { 1.0 } else { 0.0 };
        }
        (left / self.rules.fade).clamp(0.0, 1.0)
    }

    /// The tracks as draws (module docs): the segments in [`FADE_STEPS`] draws, one a step of their opacity (multiplied, their
    /// order does not matter), each at its nearest segment's distance from `eye`; none when the texture does not load.
    pub fn draws(&self, eye: [f32; 3], textures: &mut TextureSet<'_>) -> Vec<(f32, Draw)> {
        let Some(texture) = textures.get_modulated(&self.rules.texture) else {
            return Vec::new();
        };
        let mut steps: BTreeMap<u8, (f32, Vec<Vertex>)> = BTreeMap::new();
        for s in &self.segments {
            let step = (self.opacity(s) * FADE_STEPS as f32).ceil() as u8;
            if step == 0 {
                continue;
            }
            let alpha = (f32::from(step) / FADE_STEPS as f32 * 255.0).round() as u8;
            let vertex = |(p, uv): DecalPoint| Vertex {
                position: [0, 1, 2].map(|k| p[k] + (eye[k] - p[k]) * TRACK_DEPTH_BIAS),
                normal: [0.0, 0.0, 1.0],
                color: [255, 255, 255, alpha],
                uv,
            };
            let dist = (0..3)
                .map(|k| (s.center[k] - eye[k]).powi(2))
                .sum::<f32>()
                .sqrt();
            let entry = steps.entry(step).or_insert((f32::MAX, Vec::new()));
            entry.0 = entry.0.min(dist);
            entry
                .1
                .extend(s.triangles.iter().flat_map(|t| t.map(vertex)));
        }
        steps
            .into_values()
            .map(|(dist, vertices)| {
                (
                    dist,
                    Draw {
                        label: "tire tracks".to_string(),
                        vertices,
                        lighting: false,
                        tex_env: TexEnv::Modulate,
                        texture: Some((
                            texture,
                            Sampler {
                                repeat: true,
                                mag: Filter::Linear,
                                min: Filter::Linear,
                                mip: Filter::Linear,
                                mipmaps: true,
                            },
                        )),
                        blend: Blend::Modulate2x,
                        cull: Cull::None,
                        depth_test: true,
                        depth_write: false,
                        fog: true,
                        lights: Vec::new(),
                        vertex_key: None,
                    },
                )
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "tracks_tests.rs"]
mod tests;
