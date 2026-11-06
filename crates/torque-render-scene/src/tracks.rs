//! Tire tracks: strips laid on the ground behind the wheels of the vehicles the client runs, as a game asks for them
//! ([`TrackRules`]; TGE has none, and no datablock carries them, so the wire stays TGE's).
//!
//! - **Laid** ([`TireTracks::update`]): a wheel in contact with the ground lays a segment each time its contact point has gone
//!   [`TrackRules::spacing`] from the end of its strip, as wide as [`TrackRules::width`] (the game's: its tires' width); a
//!   longer way since the last frame is laid in pieces of at most the spacing. The segment is the ground's triangles under it
//!   (the host's `surfaces` near the point, [`crate::decals::Triangle`]) clipped to it: between the plane across the strip at
//!   its last end (the plane the last segment ended on, so a curve leaves no gap and no overlap) and the plane across its new
//!   end, and between its two sides, each triangle facing up within [`crate::decals::CLIP_ANGLE_COS`]. So it lies on the
//!   terrain over the hills, cut where the triangles meet, never in the air. A wheel that turns back on its strip (backing up,
//!   or a turn sharper than [`MAX_TURN_COS`] in one segment) starts the next segment square across its own way: the end's
//!   plane would face the other way and the segment would run on ahead of the wheel. A wheel that leaves the ground, or
//!   jumps further than [`MAX_GAP`] past what [`MAX_SPEED`] covers in the frame (a respawn), ends its strip.
//! - **In a leading wheel's strip**: a wheel rolling in the strip of a wheel of its vehicle ahead of it on its way (a rear
//!   tire in the front tire's track; the front in the rear's backing up) lays none there (where its line lies within half the
//!   width of that strip's segments laid in the last [`FOLLOW_AGE`] seconds), so the strip is multiplied over the ground once,
//!   not darkened again by each tire that follows it; over an older strip of its vehicle it lays its own. A segment or head
//!   that enters or leaves the leader's strip is cut square across where its line crosses that strip's edge (found in
//!   [`EDGE_STEPS`] halvings), so the two meet with no gap. A leading wheel lays its own strip whatever lies under it.
//! - **The head** ([`TireTracks::heads`]): each strip runs on from its last segment to the wheel's contact point this frame,
//!   in a head laid again every frame (replaced by the next segment), so the strip ends under the tire.
//! - **Textured**: [`TrackRules::texture`] across the strip (u from side to side) and repeating along it every strip's width
//!   (v by the distance laid).
//! - **Kept**: each segment for [`TrackRules::lifetime`] seconds, fading over its last [`TrackRules::fade`]; past
//!   [`TrackRules::max_segments`] the oldest goes first.
//! - **Drawn** ([`TireTracks::draws`]), in a draw for each of [`FADE_STEPS`] steps of their fade: multiplied over the ground ([`Blend::Modulate2x`]: 0.5 gray leaves it as it is, so the
//!   texture darkens the grass and takes its light and shadow), fading toward 0.5 gray, depth-tested without depth writes and
//!   drawn [`TRACK_DEPTH_BIAS`] of their distance nearer the eye (no fighting with the ground), unlit and fogged.

use std::collections::{BTreeMap, VecDeque};

use glam::Vec3;

use crate::decals::{CLIP_ANGLE_COS, DecalPoint, Triangle, clip};
use crate::textures::TextureSet;
use crate::{Blend, Cull, Draw, Filter, Sampler, TexEnv, Vertex};

/// The tracks' depth bias: each vertex drawn this share of its distance nearer the eye along the eye's ray (more than a
/// decal's: a strip lies along the ground at a glancing angle far off).
pub const TRACK_DEPTH_BIAS: f32 = 0.008;

/// The steps of opacity the segments are drawn in (one draw each).
pub const FADE_STEPS: u32 = 16;

/// The furthest a contact point may move between frames and still continue its strip (units), past what [`MAX_SPEED`] covers in
/// the frame's time.
pub const MAX_GAP: f32 = 4.0;

/// The fastest a wheel is taken to roll (units a second): a long frame (a hitch) bridges what it covers at that speed, over at
/// most [`MAX_BRIDGE_DT`] (a strip's longest bridge, so a stall lays no more than a few hundred segments in a frame).
pub const MAX_SPEED: f32 = 60.0;
/// The longest frame (seconds) a strip bridges at [`MAX_SPEED`].
pub const MAX_BRIDGE_DT: f32 = 0.25;

/// The sharpest turn between a strip's last segment and its next that still joins them (the cosine of the angle between their
/// ways): past it the next starts square across its own way.
pub const MAX_TURN_COS: f32 = 0.5;

/// The age (seconds) under which a leading wheel's segment keeps a following wheel from laying its own over it: longer than a
/// rear tire takes to reach the front's track at a crawl, short enough that a vehicle crossing its own older strip (a turn, a
/// lap, a figure eight) lays a new one there instead of leaving a hole in it when the old one fades.
pub const FOLLOW_AGE: f32 = 3.0;

/// The halvings that find where a strip crosses the edge of another's (to 1/4096 of a piece).
pub const EDGE_STEPS: u32 = 12;

/// The shortest head laid (units): a wheel nearer its strip's end than this draws none.
pub const MIN_HEAD: f32 = 0.05;

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
    /// The wheel that laid it, and the line along its middle (from the strip's end before it to its own).
    pub wheel: (u32, usize),
    pub line: [[f32; 3]; 2],
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
    heads: BTreeMap<(u32, usize), TrackSegment>,
}

fn flat(v: [f32; 3]) -> Option<[f32; 2]> {
    let l = v[0].hypot(v[1]);
    (l > 1e-4 && l.is_finite()).then(|| [v[0] / l, v[1] / l])
}

/// The way a segment from `end` along `along` starts across: the end's own (the plane the last segment ended on), or square
/// across `along` for a strip's first segment and one that turns back on it (module docs).
fn start_of(end: End, along: [f32; 2]) -> [f32; 2] {
    let turn = end.along[0] * along[0] + end.along[1] * along[1];
    if end.along == [0.0, 0.0] || turn < MAX_TURN_COS {
        along
    } else {
        end.along
    }
}

impl TireTracks {
    pub fn new(rules: TrackRules) -> TireTracks {
        TireTracks {
            rules,
            segments: VecDeque::new(),
            ends: BTreeMap::new(),
            heads: BTreeMap::new(),
        }
    }

    /// Forgets the strips' ends of ghost `index` (another object took its index): its wheels start new strips. The segments
    /// laid stay and fade.
    pub fn forget(&mut self, index: u32) {
        self.ends.retain(|k, _| k.0 != index);
        self.heads.retain(|k, _| k.0 != index);
    }

    /// The strips' heads this frame (module docs), by wheel.
    pub fn heads(&self) -> impl Iterator<Item = (&(u32, usize), &TrackSegment)> {
        self.heads.iter()
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
        self.heads.clear();
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
        self.heads.clear();
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
            if gone > MAX_GAP + MAX_SPEED * dt.min(MAX_BRIDGE_DT) {
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
            let spacing = self.rules.spacing.max(0.01);
            let Some(along) = flat(d) else {
                kept.insert(w.key, end);
                continue;
            };
            if gone < spacing {
                // The head: from the strip's end to the wheel, as the next segment will start.
                if gone >= MIN_HEAD
                    && let Some(head) =
                        self.piece(w.key, end, w.point, start_of(end, along), along, surfaces)
                {
                    self.heads.insert(w.key, head);
                }
                kept.insert(w.key, end);
                continue;
            }
            // In pieces of at most the spacing, each from the last one's end.
            let pieces = (gone / spacing).ceil().max(1.0) as usize;
            let mut at = end;
            for k in 1..=pieces {
                let f = k as f32 / pieces as f32;
                let to = [0, 1, 2].map(|c| end.point[c] + d[c] * f);
                let step = (to[0] - at.point[0]).hypot(to[1] - at.point[1]);
                let start_along = start_of(at, along);
                if let Some(seg) = self.piece(w.key, at, to, start_along, along, surfaces) {
                    self.push(seg);
                }
                at = End {
                    point: to,
                    along,
                    v: at.v + step / self.rules.width.max(0.01),
                };
            }
            kept.insert(w.key, at);
        }
        self.ends = kept;
    }

    /// Whether `p` lies in the strip of one of the `leaders` (module docs): beside one of their segments younger than
    /// [`FOLLOW_AGE`] (between its ends; not a head, which a wheel that leaves the ground never lays), within half the width of
    /// its line along the ground and within a width up or down.
    fn in_strip(&self, leaders: &[(u32, usize)], p: [f32; 3]) -> bool {
        let half = 0.5 * self.rules.width;
        let p = Vec3::from(p);
        self.segments
            .iter()
            .filter(|s| s.age < FOLLOW_AGE && leaders.contains(&s.wheel))
            .any(|s| {
                let [a, b] = s.line.map(Vec3::from);
                let way = (b - a).truncate();
                let t = (p - a).truncate().dot(way) / way.length_squared().max(1e-8);
                let on = a.lerp(b, t);
                (0.0..=1.0).contains(&t)
                    && (p - on).truncate().length() < half
                    && (p.z - on.z).abs() < 2.0 * half
            })
    }

    /// The part of the piece of `wheel`'s strip from `end` to `to` that lies outside the strips of the wheels of its vehicle
    /// ahead of it along `along` (module docs; where their strips ended last frame): all of it, none, or the part before it
    /// enters one or after it leaves one, cut square across where it crosses that strip's edge.
    fn piece(
        &self,
        wheel: (u32, usize),
        end: End,
        to: [f32; 3],
        start_along: [f32; 2],
        along: [f32; 2],
        surfaces: &Surfaces<'_>,
    ) -> Option<TrackSegment> {
        let leaders: Vec<(u32, usize)> = self
            .ends
            .iter()
            .filter(|(k, o)| {
                k.0 == wheel.0
                    && **k != wheel
                    && (o.point[0] - end.point[0]) * along[0]
                        + (o.point[1] - end.point[1]) * along[1]
                        > 0.0
            })
            .map(|(k, _)| *k)
            .collect();
        let from_in = self.in_strip(&leaders, end.point);
        if from_in == self.in_strip(&leaders, to) {
            if from_in {
                return None;
            }
            return self.segment(wheel, end, to, start_along, along, surfaces);
        }
        let at = |f: f32| (Vec3::from(end.point).lerp(Vec3::from(to), f)).to_array();
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..EDGE_STEPS {
            let m = 0.5 * (lo + hi);
            if self.in_strip(&leaders, at(m)) == from_in {
                lo = m;
            } else {
                hi = m;
            }
        }
        let edge = at(0.5 * (lo + hi));
        if from_in {
            let step = (edge[0] - end.point[0]).hypot(edge[1] - end.point[1]);
            let out = End {
                point: edge,
                along,
                v: end.v + step / self.rules.width.max(0.01),
            };
            self.segment(wheel, out, to, along, along, surfaces)
        } else {
            self.segment(wheel, end, edge, start_along, along, surfaces)
        }
    }

    /// Keeps a segment laid (the oldest let go past [`TrackRules::max_segments`]).
    fn push(&mut self, seg: TrackSegment) {
        if self.rules.max_segments == 0 {
            return;
        }
        while self.segments.len() >= self.rules.max_segments {
            self.segments.pop_front();
        }
        self.segments.push_back(seg);
    }

    /// A segment from `end` to `to`: the ground's triangles between the plane across `start_along` at the end and the plane
    /// across `along` at `to`, and within half the width of the line between them; none when no triangle is under it.
    fn segment(
        &self,
        wheel: (u32, usize),
        end: End,
        to: [f32; 3],
        start_along: [f32; 2],
        along: [f32; 2],
        surfaces: &Surfaces<'_>,
    ) -> Option<TrackSegment> {
        let half = 0.5 * self.rules.width;
        let a = end.point;
        let mid = ((Vec3::from(a) + Vec3::from(to)) * 0.5).to_array();
        let side = Vec3::new(-along[1], along[0], 0.0);
        let start = Vec3::new(start_along[0], start_along[1], 0.0);
        let ahead = Vec3::new(along[0], along[1], 0.0);
        let reach = 0.5 * (to[0] - a[0]).hypot(to[1] - a[1]) + half + 1.0;
        let mut triangles = Vec::new();
        for tri in surfaces(mid, reach) {
            let [p0, p1, p2] = tri.map(Vec3::from);
            let n = (p1 - p0).cross(p2 - p0);
            let len = n.length();
            if len < 1e-9 || n.z.abs() / len < CLIP_ANGLE_COS {
                continue;
            }
            let mut poly = tri.to_vec();
            // Past the end the strip ended on, before its new end, within its sides.
            poly = clip(&poly, a, (-start).to_array(), 0.0);
            poly = clip(&poly, to, ahead.to_array(), 0.0);
            poly = clip(&poly, mid, side.to_array(), half);
            poly = clip(&poly, mid, (-side).to_array(), half);
            if poly.len() < 3 {
                continue;
            }
            // u across from side to side; v on from the end's by the distance along the strip.
            let point = |p: [f32; 3]| -> DecalPoint {
                let r = Vec3::new(p[0] - a[0], p[1] - a[1], 0.0);
                let u = 0.5 + r.dot(side) / (2.0 * half);
                (p, [u, end.v + r.dot(ahead) / self.rules.width.max(0.01)])
            };
            for i in 1..poly.len() - 1 {
                triangles.push([point(poly[0]), point(poly[i]), point(poly[i + 1])]);
            }
        }
        (!triangles.is_empty()).then_some(TrackSegment {
            triangles,
            wheel,
            line: [a, to],
            center: mid,
            age: 0.0,
        })
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
        for s in self.segments.iter().chain(self.heads.values()) {
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
