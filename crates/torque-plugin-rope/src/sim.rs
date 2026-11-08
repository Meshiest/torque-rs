//! The rope simulation: position-based dynamics on a chain of points, stepped by a fixed substep.
//!
//! A rope is a chain of `segments + 1` points. Each substep moves every free point by its own last move (Verlet) and by gravity,
//! then pulls neighboring points back to the segment's resting length a few times over (the distance constraints), pushes points
//! two apart away from each other when the rope has some stiffness (the bend constraints), and keeps the points above a floor
//! if the world has one. A pinned end is not moved by any of it: it is put where its anchor is, a share of the way each substep,
//! so an anchor that jumps drags the rope instead of breaking it.
//!
//! **Fixed substeps.** [`RopeWorld::advance`] takes the time that passed and runs whole substeps of [`SUBSTEP_SECONDS`], at most
//! [`MAX_SUBSTEPS`] a call; time past that is dropped (the ropes slow down on a stalled frame, they do not explode), and a remainder
//! under a substep waits for the next call. The result depends on the sequence of substeps only, never on how the time was cut
//! into calls.
//!
//! **Caps.** At most [`MAX_ROPES`] ropes, [`MAX_TOTAL_SEGMENTS`] segments over all of them, [`crate::MAX_SEGMENTS_PER_ROPE`] in one.
//!
//! **Hostile numbers.** A point, a speed or a floor that is not a finite number is made one on the way in; a coordinate past
//! [`MAX_COORD`] is held to it. Should a rope's points leave the finite range anyway, the rope is laid out again between its ends.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use glam::Vec3;
use torque_ts_vm::sim::ObjId;

use crate::data::{MAX_LENGTH, RopeLook, RopeParams, RopeStyle};
use crate::driver::{Drive, DriveEnd};
use crate::image::ImageData;
use crate::mesh::{TubeMesh, TubeSource};
use crate::strip::StripSet;

/// The most ropes a world holds.
pub const MAX_ROPES: usize = 256;
/// The most texture files a world warns about (a script that names endless files is not given endless warnings, or memory).
pub const MAX_WARNED: usize = 256;
/// The most textures a world holds (a texture is one file and one filter).
pub const MAX_TEXTURES: usize = 64;
/// The most segments over all the ropes of a world.
pub const MAX_TOTAL_SEGMENTS: usize = 4096;
/// One substep (seconds): the rope runs at 240 Hz inside, whatever the frame rate is.
pub const SUBSTEP_SECONDS: f32 = 1.0 / 240.0;
/// The most substeps one [`RopeWorld::advance`] (one drawn frame) runs: a longer hitch is dropped, not caught up.
pub const MAX_SUBSTEPS: usize = 8;
/// How many times a substep goes over the constraints.
pub const ITERATIONS: usize = 6;
/// The fastest an end that follows what is drawn moves toward it (units a second): faster than a car, so an anchor moves with
/// the drawn object frame by frame and only a jump across the map is followed over a few frames.
pub const MAX_DRAWN_PIN_SPEED: f32 = 400.0;
/// The ticks without a drawn frame after which a client's tick runs the ropes itself (a client that does not draw).
pub const IDLE_TICKS: u32 = 8;
/// The farthest from the origin a point may be (world units).
pub const MAX_COORD: f32 = 100_000.0;
/// The fastest a point may move (units a second).
pub const MAX_SPEED: f32 = 200.0;
/// The fastest a pinned end moves toward its anchor (units a second): an anchor that jumps is followed over a few ticks.
pub const MAX_PIN_SPEED: f32 = 60.0;
/// How slowly a rope follows an object it hangs from: the time it takes to close most of the gap between two ticks' readings.
pub const ANCHOR_SMOOTHING: f32 = 0.03;
/// A rope with no length of its own is this much longer than the distance between its ends.
pub const DEFAULT_SLACK: f32 = 1.1;
/// The shortest rope.
pub const MIN_LENGTH: f32 = 0.02;

/// The longest a rope's lifetime or reel may be (seconds).
pub const MAX_SECONDS: f32 = 3600.0;

/// A rope's number in a world (never 0).
pub type RopeId = u32;

/// One of a rope's two ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RopeEnd {
    /// The first point.
    Start,
    /// The last point.
    End,
}

impl RopeEnd {
    /// The end a script names: `0` or `start`, `1` or `end`.
    pub fn parse(text: &str) -> Option<RopeEnd> {
        match text.trim().to_ascii_lowercase().as_str() {
            "0" | "start" => Some(RopeEnd::Start),
            "1" | "end" => Some(RopeEnd::End),
            _ => None,
        }
    }

    fn index(self) -> usize {
        match self {
            RopeEnd::Start => 0,
            RopeEnd::End => 1,
        }
    }
}

/// What holds one end of a rope.
#[derive(Clone, Debug, PartialEq)]
pub enum Anchor {
    /// Nothing: the end falls with the rest.
    Free,
    /// A point in the world.
    Point([f32; 3]),
    /// An object, found through the world's [`AnchorResolver`](crate::AnchorResolver) (`node` names a place on it).
    Object {
        /// The object.
        id: ObjId,
        /// The place on the object; empty for its origin.
        node: String,
    },
    /// A ghost as the frame draws it ([`RopeWorld::pin_ghost`]).
    Ghost {
        /// The ghost's number.
        ghost: u32,
        /// The place on its shape; empty for its origin.
        node: String,
        /// The place held, as an offset from the node in the ghost's own frame (zero: the node itself).
        offset: [f32; 3],
    },
}

/// Why a call on the world was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RopeError {
    /// The world holds [`MAX_ROPES`] ropes.
    TooManyRopes,
    /// The rope would take the world past [`MAX_TOTAL_SEGMENTS`].
    TooManySegments,
    /// There is no rope with the number.
    NoSuchRope(RopeId),
    /// The world holds [`MAX_TEXTURES`] textures.
    TooManyTextures,
}

impl fmt::Display for RopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RopeError::TooManyRopes => write!(f, "there are already {MAX_ROPES} ropes"),
            RopeError::TooManySegments => {
                write!(
                    f,
                    "the ropes would have more than {MAX_TOTAL_SEGMENTS} segments"
                )
            }
            RopeError::NoSuchRope(id) => write!(f, "there is no rope {id}"),
            RopeError::TooManyTextures => {
                write!(f, "the ropes already use {MAX_TEXTURES} textures")
            }
        }
    }
}

impl std::error::Error for RopeError {}

/// `c` as a coordinate the simulation can hold: a number that is not finite becomes 0, the rest is kept within [`MAX_COORD`].
pub(crate) fn coord(c: f32) -> f32 {
    if c.is_finite() {
        c.clamp(-MAX_COORD, MAX_COORD)
    } else {
        0.0
    }
}

/// `p` as a point the simulation can hold ([`coord`] for each coordinate).
pub(crate) fn point(p: [f32; 3]) -> Vec3 {
    Vec3::from(p.map(coord))
}

/// Where a ghost's mount is drawn this frame: its place, and the way it points (the barrel of a muzzle, the aim).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mount {
    /// The place.
    pub at: [f32; 3],
    /// The direction the mount points, if it has one.
    pub dir: Option<[f32; 3]>,
}

/// Which way a driven curve leaves one of its ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tangent {
    /// Toward the other end (a straight leaving: the curve at that end is as the other end's tangent makes it).
    Auto,
    /// Along a fixed direction (not necessarily a unit vector).
    Fixed([f32; 3]),
    /// Along the direction the end's mount points as the frame draws it (a muzzle's barrel, an aim).
    Mount,
}

/// The largest bend a curve has either way.
pub const MAX_BEND: f32 = 4.0;

/// The most a texture runs along a rope, repeats a second either way.
pub const MAX_SCROLL: f32 = 1000.0;

/// A texture's speed as a rope keeps it: held to [`MAX_SCROLL`] either way, 0 for a number that is not one. Every way a speed gets
/// to a rope (a script's call, a server's ghost) goes through it.
pub fn clean_scroll(speed: f32) -> f32 {
    if speed.is_finite() {
        speed.clamp(-MAX_SCROLL, MAX_SCROLL)
    } else {
        0.0
    }
}

impl Tangent {
    /// The tangent as a rope keeps it: a fixed direction must be three finite numbers, not all 0 (held to [`MAX_COORD`]), else the
    /// end leaves toward the other end.
    pub fn sanitized(self) -> Tangent {
        match self {
            Tangent::Fixed(d) if d.iter().all(|c| c.is_finite()) && d.iter().any(|c| *c != 0.0) => {
                Tangent::Fixed(d.map(|c| c.clamp(-MAX_COORD, MAX_COORD)))
            }
            Tangent::Fixed(_) => Tangent::Auto,
            other => other,
        }
    }
}

/// The shape a driven rope has: a cubic Bezier from the start end to the far end. The start leaves along `start`, the far end
/// leaves (back toward the start) along `end`; each handle is `bend` times the distance between the ends long, so a `bend` of 0 is a
/// straight line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Curve {
    /// How the start leaves.
    pub start: Tangent,
    /// How the far end leaves.
    pub end: Tangent,
    /// The handles' length over the distance between the ends.
    pub bend: f32,
}

impl Curve {
    /// The curve as a rope keeps it: the bend held to [`MAX_BEND`] either way (0 for a number that is not one) and the tangents
    /// [sanitized](Tangent::sanitized). Every way a curve gets to a rope (a script's call, a server's ghost) goes through it.
    pub fn sanitized(self) -> Curve {
        Curve {
            start: self.start.sanitized(),
            end: self.end.sanitized(),
            bend: if self.bend.is_finite() {
                self.bend.clamp(-MAX_BEND, MAX_BEND)
            } else {
                0.0
            },
        }
    }

    /// The point of the curve at `t` (0 to 1) from `a` to `b`, given the unit directions the ends leave along (`da` from `a`, `db`
    /// from `b`).
    pub fn at(a: Vec3, b: Vec3, da: Vec3, db: Vec3, bend: f32, t: f32) -> Vec3 {
        let h = a.distance(b) * bend;
        let (p1, p2) = (a + da * h, b + db * h);
        let u = 1.0 - t;
        a * (u * u * u) + p1 * (3.0 * u * u * t) + p2 * (3.0 * u * t * t) + b * (t * t * t)
    }

    /// `n` points (at least 2) along the curve, the first at `a` and the last at `b`, evenly spaced by the curve's length (the
    /// curve's own parameter bunches points where the handles are long, and a texture would stretch there).
    pub fn sample(a: Vec3, b: Vec3, da: Vec3, db: Vec3, bend: f32, n: usize) -> Vec<Vec3> {
        const FINE: usize = 64;
        let fine: Vec<Vec3> = (0..=FINE)
            .map(|i| Curve::at(a, b, da, db, bend, i as f32 / FINE as f32))
            .collect();
        let mut length = vec![0.0f32; FINE + 1];
        for i in 1..=FINE {
            length[i] = length[i - 1] + fine[i].distance(fine[i - 1]);
        }
        let total = length[FINE];
        let n = n.max(2);
        let mut at = 0;
        (0..n)
            .map(|k| {
                if k == 0 {
                    return a;
                }
                if k == n - 1 || total <= 0.0 {
                    return b;
                }
                let want = total * k as f32 / (n - 1) as f32;
                while at + 1 < FINE && length[at + 1] < want {
                    at += 1;
                }
                let span = (length[at + 1] - length[at]).max(1.0e-9);
                fine[at].lerp(fine[at + 1], ((want - length[at]) / span).clamp(0.0, 1.0))
            })
            .collect()
    }
}

/// One texture a world holds: a file as loaded, and how it is filtered.
#[derive(Clone, Debug)]
pub struct TextureEntry {
    /// The file.
    pub path: String,
    /// Whether it is filtered `"nearest"` (else linear with mip levels).
    pub nearest: bool,
    /// The decoded image.
    pub image: Arc<ImageData>,
}

/// The textures of a world, numbered from 1 in the order they were added (0 is no texture).
#[derive(Clone, Debug, Default)]
pub struct TextureTable {
    /// A count that changes whenever an entry is added or its image replaced.
    pub revision: u64,
    /// The entries: texture `n` is `entries[n - 1]`.
    pub entries: Vec<TextureEntry>,
}

impl TextureTable {
    /// The number of the texture of file `path` filtered `nearest` or not (0: the world holds none).
    pub fn find(&self, path: &str, nearest: bool) -> u32 {
        self.entries
            .iter()
            .position(|e| e.nearest == nearest && e.path == path)
            .map_or(0, |i| i as u32 + 1)
    }

    /// Texture `id`.
    pub fn get(&self, id: u32) -> Option<&TextureEntry> {
        self.entries.get((id as usize).checked_sub(1)?)
    }
}

/// What a rope is drawn with: its datablock's values, each replaced by what a script set on the rope ([`RopeLook`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Appearance {
    /// The paint.
    pub color: [u8; 4],
    /// Whether a strip glows.
    pub emissive: bool,
    /// The texture's number in the world's table (0: none).
    pub texture: u32,
    /// The texture's speed, repeats a second.
    pub scroll: f32,
}

/// An end held by something.
#[derive(Clone, Debug)]
struct Pin {
    /// The object it follows, if it follows one (else a fixed point).
    object: Option<(ObjId, String)>,
    /// Where the end is held now.
    at: Vec3,
    /// Where it is going: the point, or the object's last reading, or where the frame draws the ghost it follows.
    target: Vec3,
    /// The ghost (and the place on its shape) whose drawn transform it follows each frame, if it follows one.
    ghost: Option<(u32, String)>,
    /// Where the ghost is drawn this frame (None: not drawn, or no ghost: `target` is the script's reading).
    drawn: Option<Vec3>,
    /// The way the ghost's mount points this frame, if it says.
    aim: Option<Vec3>,
    /// Whether the client has seen the ghost an end pinned to it (`pin_ghost`) follows.
    ghost_seen: bool,
    /// The place held, as an offset from the node in the ghost's own frame (zero: the node itself).
    offset: Vec3,
}

/// A change of length in progress ([`RopeWorld::set_length`]).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reel {
    from: f32,
    to: f32,
    left: f32,
    seconds: f32,
}

/// What is left of a temporary rope's life ([`RopeWorld::set_lifetime`]).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Life {
    left: f32,
    fade: f32,
}

/// One rope: its points and what holds its ends.
#[derive(Clone, Debug)]
pub struct Rope {
    params: RopeParams,
    /// The resting length of one segment.
    rest: f32,
    pos: Vec<Vec3>,
    prev: Vec<Vec3>,
    /// The points before the last substep, and the points as the frame shows them (between that state and the last one).
    before: Vec<Vec3>,
    shown: Vec<Vec3>,
    pins: [Option<Pin>; 2],
    /// What a script set on this rope in place of the datablock's values.
    look: RopeLook,
    /// The shape a driven rope has (None: straight, or simulated).
    curve: Option<Curve>,
    /// Where the ends were last time the points were all finite, to lay the rope out again from.
    good: [Vec3; 2],
    /// A reel in progress: the length goes from one value to another over a time.
    reel: Option<Reel>,
    /// A temporary rope's time left.
    life: Option<Life>,
    /// Whether a driver says the rope is at its limit: it is drawn straight between its ends whatever their distance.
    at_limit: bool,
    /// The length a driver last gave ([`RopeWorld::drive`]); None when none does.
    driven_length: Option<f32>,
}

impl Rope {
    fn new(params: RopeParams, a: Vec3, b: Vec3) -> Rope {
        let chord = a.distance(b);
        let length = if params.length > 0.0 {
            params.length
        } else {
            chord * DEFAULT_SLACK
        }
        .clamp(MIN_LENGTH, MAX_LENGTH);
        let segments = params.pieces();
        let mut rope = Rope {
            params,
            rest: if params.style == RopeStyle::Line {
                chord.max(MIN_LENGTH)
            } else {
                length / segments as f32
            },
            pos: Vec::new(),
            prev: Vec::new(),
            before: Vec::new(),
            shown: Vec::new(),
            pins: [
                Some(Pin {
                    object: None,
                    at: a,
                    target: a,
                    ghost: None,
                    drawn: None,
                    aim: None,
                    ghost_seen: false,
                    offset: Vec3::ZERO,
                }),
                Some(Pin {
                    object: None,
                    at: b,
                    target: b,
                    ghost: None,
                    drawn: None,
                    aim: None,
                    ghost_seen: false,
                    offset: Vec3::ZERO,
                }),
            ],
            look: RopeLook::default(),
            curve: None,
            good: [a, b],
            reel: None,
            life: None,
            at_limit: false,
            driven_length: None,
        };
        rope.layout(a, b);
        rope
    }

    /// Puts the points on a sagging arc from `a` to `b` with no speed.
    fn layout(&mut self, a: Vec3, b: Vec3) {
        let segments = self.params.pieces();
        let length = self.rest * segments as f32;
        let chord = a.distance(b);
        let at = |t: f32| -> Vec3 {
            if chord < 1.0e-4 {
                return a + Vec3::NEG_Z * (length * t);
            }
            let dir = (b - a) / chord;
            let mut down = Vec3::NEG_Z - dir * dir.dot(Vec3::NEG_Z);
            down = if down.length() < 0.05 {
                dir.any_orthonormal_vector()
            } else {
                down.normalize()
            };
            // A parabola of the sag that makes its arc `length` long.
            let slack = (length - chord).max(0.0);
            let sag = (3.0 * chord * slack / 8.0).sqrt().min(length);
            a + (b - a) * t + down * (sag * 4.0 * t * (1.0 - t))
        };
        self.pos = if self.params.style == RopeStyle::Line {
            vec![a, b]
        } else {
            (0..=segments)
                .map(|i| at(i as f32 / segments as f32))
                .collect()
        };
        self.prev = self.pos.clone();
        self.before = self.pos.clone();
        self.shown = self.pos.clone();
        self.good = [a, b];
        for (k, end) in [a, b].into_iter().enumerate() {
            if let Some(pin) = &mut self.pins[k] {
                pin.at = end;
                pin.target = if pin.object.is_some() {
                    end
                } else {
                    pin.target
                };
            }
        }
    }

    /// Whether the rope's shape is driven (a line, `sim = "none"` or a curve set) and not simulated.
    pub fn driven(&self) -> bool {
        self.params.style == RopeStyle::Line || self.params.driven || self.curve.is_some()
    }

    /// The curve a driven rope has been given (None: straight, or simulated).
    pub fn curve(&self) -> Option<Curve> {
        self.curve
    }

    /// Lays a driven rope out on its curve between where its ends are drawn (a straight line when it has no curve). True if that
    /// moved a point.
    fn drive(&mut self) -> bool {
        let n = self.pos.len();
        let end =
            |pin: &Option<Pin>, held: Vec3| pin.as_ref().map_or(held, |p| p.drawn.unwrap_or(p.at));
        let a = end(&self.pins[0], self.pos[0]);
        let b = end(&self.pins[1], self.pos[n - 1]);
        let curve = self.curve.unwrap_or(Curve {
            start: Tangent::Auto,
            end: Tangent::Auto,
            bend: 0.0,
        });
        let toward = (b - a).try_normalize().unwrap_or(Vec3::X);
        let leaving = |t: Tangent, pin: &Option<Pin>, default: Vec3| match t {
            Tangent::Auto => default,
            Tangent::Fixed(d) => Vec3::from(d).try_normalize().unwrap_or(default),
            Tangent::Mount => pin
                .as_ref()
                .and_then(|p| p.aim)
                .and_then(Vec3::try_normalize)
                .unwrap_or(default),
        };
        let (da, db) = (
            leaving(curve.start, &self.pins[0], toward),
            leaving(curve.end, &self.pins[1], -toward),
        );
        let bend = if curve.bend.is_finite() {
            curve.bend
        } else {
            0.0
        };
        let laid = Curve::sample(a, b, da, db, bend, n);
        let mut moved = false;
        for (i, at) in laid.into_iter().map(|p| point(p.to_array())).enumerate() {
            moved |= self.pos[i] != at;
            self.pos[i] = at;
        }
        self.prev.clone_from(&self.pos);
        self.before.clone_from(&self.pos);
        self.shown.clone_from(&self.pos);
        moved
    }

    /// Puts what holds the end `end` as a driver says; true if that changed anything.
    fn drive_end(&mut self, end: RopeEnd, want: &DriveEnd) -> bool {
        let k = end.index();
        let here = self.pos[if k == 0 { 0 } else { self.pos.len() - 1 }];
        match want {
            DriveEnd::Free => self.pins[k].take().is_some(),
            DriveEnd::Point(at) => {
                let at = point(*at);
                match &mut self.pins[k] {
                    Some(pin) if pin.object.is_none() && pin.ghost.is_none() => {
                        let same = pin.drawn == Some(at);
                        pin.drawn = Some(at);
                        pin.target = at;
                        !same
                    }
                    pin => {
                        *pin = Some(Pin {
                            object: None,
                            at: here,
                            target: at,
                            ghost: None,
                            drawn: Some(at),
                            aim: None,
                            ghost_seen: false,
                            offset: Vec3::ZERO,
                        });
                        true
                    }
                }
            }
            DriveEnd::Ghost {
                ghost,
                node,
                offset,
            } => {
                let offset = point(*offset);
                let same = matches!(
                    &self.pins[k],
                    Some(Pin { object: None, ghost: Some((g, n)), offset: o, .. })
                        if g == ghost && n == node && *o == offset
                );
                if !same {
                    self.pins[k] = Some(Pin {
                        object: None,
                        at: here,
                        target: here,
                        ghost: Some((*ghost, node.clone())),
                        drawn: None,
                        aim: None,
                        ghost_seen: false,
                        offset,
                    });
                }
                !same
            }
        }
    }

    /// How much of itself a temporary rope still shows: 1 until the last `fade` seconds of its life, then falling to 0 (a rope with no
    /// lifetime, or no fade, is 1). A tube draws thinner by it, a ribbon or a line fainter.
    pub fn fade(&self) -> f32 {
        match self.life {
            Some(Life { left, fade }) if fade > 0.0 => (left / fade).clamp(0.0, 1.0),
            _ => 1.0,
        }
    }

    /// The seconds a temporary rope has left (None: it lives until it is taken away).
    pub fn life_left(&self) -> Option<f32> {
        self.life.map(|l| l.left)
    }

    /// Sets the length (see [`RopeWorld::set_length`]).
    fn reel_to(&mut self, length: f32, seconds: f32) {
        if !length.is_finite() || self.params.style == RopeStyle::Line {
            return;
        }
        let to = length.clamp(MIN_LENGTH, MAX_LENGTH);
        if seconds.is_finite() && seconds > 0.0 {
            let seconds = seconds.min(MAX_SECONDS);
            self.reel = Some(Reel {
                from: self.length(),
                to,
                left: seconds,
                seconds,
            });
        } else {
            self.reel = None;
            self.rest = to / self.params.pieces() as f32;
        }
    }

    /// Whether a driver says the rope is at its limit ([`Drive::at_limit`]): it is then drawn straight and evenly spaced between
    /// its ends even when they are nearer than its length.
    pub fn at_limit(&self) -> bool {
        self.at_limit
    }

    /// Whether the rope's length is changing ([`RopeWorld::set_length`]).
    pub fn reeling(&self) -> bool {
        self.reel.is_some()
    }

    /// Whether both ends are held and `a` and `b` (where they are) are as far apart as the rope is long, or a driver says the rope is
    /// at its limit: it is then drawn straight between them instead of stretching.
    fn taut_between(&self, a: Vec3, b: Vec3) -> bool {
        self.pins[0].is_some()
            && self.pins[1].is_some()
            && !self.driven()
            && (self.at_limit || a.distance(b) >= self.length())
    }

    /// Whether the rope is drawn taut now: both ends held, and as far apart as it is long (or at its limit, for a driven rope).
    pub fn taut(&self) -> bool {
        let shown = self.drawn_points();
        match (shown.first(), shown.last()) {
            (Some(&a), Some(&b)) => self.taut_between(a, b),
            _ => false,
        }
    }

    /// Spaces `points` evenly on the line from the first to the last.
    fn straighten(points: &mut [Vec3]) {
        let n = points.len();
        if n < 2 {
            return;
        }
        let (a, b) = (points[0], points[n - 1]);
        for (i, p) in points.iter_mut().enumerate() {
            *p = a.lerp(b, i as f32 / (n - 1) as f32);
        }
    }

    /// Runs a reel on by `dt` seconds.
    fn reel_by(&mut self, dt: f32) {
        let Some(reel) = &mut self.reel else {
            return;
        };
        reel.left -= dt;
        let done = reel.left <= 0.0;
        let share = if done {
            1.0
        } else {
            1.0 - (reel.left / reel.seconds).clamp(0.0, 1.0)
        };
        let length = (reel.from + (reel.to - reel.from) * share).clamp(MIN_LENGTH, MAX_LENGTH);
        let pieces = self.params.pieces() as f32;
        if self.params.style != RopeStyle::Line {
            self.rest = length / pieces;
        }
        if done {
            self.reel = None;
        }
    }

    /// The rope's parameters.
    pub fn params(&self) -> &RopeParams {
        &self.params
    }

    /// What a script set on this rope in place of the datablock's values.
    pub fn look(&self) -> &RopeLook {
        &self.look
    }

    /// What the rope is drawn with: the datablock's values with the rope's own over them (`textures` says what the texture file
    /// of an override is).
    pub fn appearance(&self, textures: &TextureTable) -> Appearance {
        let texture = match &self.look.texture {
            Some(path) => textures.find(path, self.params.texture_nearest),
            None => self.params.texture,
        };
        Appearance {
            color: self.look.color.unwrap_or(self.params.color),
            emissive: self.look.emissive.unwrap_or(self.params.emissive),
            texture,
            scroll: self.look.scroll.unwrap_or(self.params.texture_scroll),
        }
    }

    /// The resting length of the whole rope (a line is as long as the distance between its ends).
    pub fn length(&self) -> f32 {
        match self.params.style {
            RopeStyle::Line => self.pos[0].distance(self.pos[self.pos.len() - 1]),
            _ => self.rest * self.segments() as f32,
        }
    }

    /// The pieces it is cut into.
    pub fn segments(&self) -> usize {
        self.pos.len() - 1
    }

    /// The points, from the start to the end (`segments() + 1` of them).
    pub fn points(&self) -> impl ExactSizeIterator<Item = [f32; 3]> + '_ {
        self.pos.iter().map(|p| p.to_array())
    }

    /// What holds the end `end` now.
    pub fn anchor(&self, end: RopeEnd) -> Anchor {
        match &self.pins[end.index()] {
            None => Anchor::Free,
            Some(Pin {
                object: Some((id, node)),
                ..
            }) => Anchor::Object {
                id: *id,
                node: node.clone(),
            },
            Some(Pin {
                ghost: Some((ghost, node)),
                offset,
                ..
            }) => Anchor::Ghost {
                ghost: *ghost,
                node: node.clone(),
                offset: offset.to_array(),
            },
            Some(pin) => Anchor::Point(pin.target.to_array()),
        }
    }

    /// The points as the frame draws them: the state between the last two substeps by how far into the next one the frame is,
    /// an end that follows a drawn ghost exactly where the ghost is drawn.
    pub fn drawn_points(&self) -> &[Vec3] {
        if self.shown.len() == self.pos.len() {
            &self.shown
        } else {
            &self.pos
        }
    }

    fn refresh_shown(&mut self, share: f32) {
        let n = self.pos.len();
        if self.before.len() != n {
            self.before.clone_from(&self.pos);
        }
        let share = if share.is_finite() {
            share.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.shown.clear();
        self.shown.extend(
            self.before
                .iter()
                .zip(&self.pos)
                .map(|(b, p)| b.lerp(*p, share)),
        );
        for (end, index) in [0, n - 1].into_iter().enumerate() {
            if let Some(at) = self.pins[end].as_ref().and_then(|p| p.drawn) {
                self.shown[index] = at;
            }
        }
        if self.taut_between(self.shown[0], self.shown[n - 1]) {
            Rope::straighten(&mut self.shown);
        }
    }

    /// The most a segment is longer or shorter than its resting length, as a share of it (0 for a rope exactly at rest).
    pub fn strain(&self) -> f32 {
        if self.params.style == RopeStyle::Line {
            return 0.0;
        }
        self.pos
            .windows(2)
            .map(|w| ((w[0].distance(w[1]) - self.rest) / self.rest).abs())
            .fold(0.0, f32::max)
    }

    /// The fastest point (units a second), by the last substep's move.
    pub fn speed(&self) -> f32 {
        self.pos
            .iter()
            .zip(&self.prev)
            .map(|(p, q)| p.distance(*q) / SUBSTEP_SECONDS)
            .fold(0.0, f32::max)
    }

    fn fixed(&self, i: usize) -> bool {
        (i == 0 && self.pins[0].is_some()) || (i + 1 == self.pos.len() && self.pins[1].is_some())
    }

    /// Runs `steps` substeps; the anchors that follow an object close `alpha` of their gap over the call.
    fn run(&mut self, steps: usize, floor: Option<f32>, alpha: f32) {
        let last = self.pos.len() - 1;
        let from = self.pins.each_ref().map(|p| p.as_ref().map(|p| p.at));
        let reach = MAX_PIN_SPEED * steps as f32 * SUBSTEP_SECONDS;
        let drawn_reach = MAX_DRAWN_PIN_SPEED * steps as f32 * SUBSTEP_SECONDS;
        for pin in self.pins.iter_mut().flatten() {
            if pin.drawn.is_some() {
                pin.at += (pin.target - pin.at).clamp_length_max(drawn_reach);
                continue;
            }
            let wanted = if pin.object.is_some() {
                (pin.target - pin.at) * alpha
            } else {
                pin.target - pin.at
            };
            pin.at += wanted.clamp_length_max(reach);
        }
        if self.driven() {
            self.reel_by(steps as f32 * SUBSTEP_SECONDS);
            // A line, and a rope whose shape is driven, are not simulated: its ends are where the pins are, and a free end stays where it was.
            for (end, index) in [0, last].into_iter().enumerate() {
                if let Some(pin) = &self.pins[end] {
                    self.pos[index] = pin.at;
                    self.prev[index] = pin.at;
                }
            }
            self.before.clone_from(&self.pos);
            self.check();
            return;
        }
        for k in 1..=steps {
            self.reel_by(SUBSTEP_SECONDS);
            if k == steps {
                self.before.clone_from(&self.pos);
            }
            let share = k as f32 / steps as f32;
            for (end, index) in [0, last].into_iter().enumerate() {
                if let (Some(pin), Some(start)) = (&self.pins[end], from[end]) {
                    let p = start.lerp(pin.at, share);
                    self.pos[index] = p;
                    self.prev[index] = p;
                }
            }
            self.substep(SUBSTEP_SECONDS, floor);
        }
        // Ends held farther apart than the rope is long: it lies straight between them, not stretched.
        if self.taut_between(self.pos[0], self.pos[last]) {
            Rope::straighten(&mut self.pos);
            self.prev.clone_from(&self.pos);
        }
        self.check();
    }

    fn substep(&mut self, dt: f32, floor: Option<f32>) {
        let n = self.pos.len();
        let gravity = Vec3::new(0.0, 0.0, -self.params.gravity);
        let keep = (-self.params.damping * dt).exp();
        let reach = MAX_SPEED * dt;
        for i in 0..n {
            if self.fixed(i) {
                continue;
            }
            let moved = ((self.pos[i] - self.prev[i]) * keep).clamp_length_max(reach);
            self.prev[i] = self.pos[i];
            self.pos[i] += moved + gravity * (dt * dt);
        }
        let bend = self.params.stiffness * 0.5;
        let radius = self.params.thickness * 0.5;
        for it in 0..ITERATIONS {
            if it % 2 == 0 {
                for i in 0..n - 1 {
                    self.link(i, i + 1, self.rest, 1.0, false);
                }
            } else {
                for i in (0..n - 1).rev() {
                    self.link(i, i + 1, self.rest, 1.0, false);
                }
            }
            if bend > 0.0 {
                for i in 0..n - 2 {
                    self.link(i, i + 2, 2.0 * self.rest, bend, true);
                }
            }
            if let Some(floor) = floor {
                self.rest_on(floor + radius);
            }
        }
    }

    /// Moves points `i` and `j` toward being `target` apart (only when nearer than that, if `push_only`), by `share` of the way.
    fn link(&mut self, i: usize, j: usize, target: f32, share: f32, push_only: bool) {
        let wi = if self.fixed(i) { 0.0 } else { 1.0 };
        let wj = if self.fixed(j) { 0.0 } else { 1.0 };
        let w = wi + wj;
        if w == 0.0 {
            return;
        }
        let d = self.pos[j] - self.pos[i];
        let len = d.length();
        if len < 1.0e-9 || (push_only && len >= target) {
            return;
        }
        let c = (len - target) / len * share / w;
        self.pos[i] += d * (c * wi);
        self.pos[j] -= d * (c * wj);
    }

    /// Holds the free points above `height`; one that touches it loses most of its sideways speed.
    fn rest_on(&mut self, height: f32) {
        for i in 0..self.pos.len() {
            if self.fixed(i) || self.pos[i].z >= height {
                continue;
            }
            self.pos[i].z = height;
            let slide = self.pos[i] - self.prev[i];
            self.prev[i] = Vec3::new(
                self.pos[i].x - slide.x * 0.7,
                self.pos[i].y - slide.y * 0.7,
                height,
            );
        }
    }

    /// Lays the rope out again if any point left the finite range.
    fn check(&mut self) {
        let last = self.pos.len() - 1;
        let sound = self
            .pos
            .iter()
            .chain(&self.prev)
            .all(|p| p.is_finite() && p.abs().max_element() <= MAX_COORD);
        if sound {
            self.good = [self.pos[0], self.pos[last]];
        } else {
            self.layout(self.good[0], self.good[1]);
        }
    }
}

/// The ropes of one engine and the clock that runs them.
#[derive(Debug, Default)]
pub struct RopeWorld {
    ropes: BTreeMap<RopeId, Rope>,
    next: RopeId,
    floor: Option<f32>,
    /// Time not yet run in substeps.
    left: f32,
    revision: u64,
    substeps: u64,
    mesh: Option<Arc<TubeMesh>>,
    strips: Option<Arc<StripSet>>,
    /// The time of the last drawn frame, and the ticks since one.
    clock: Option<f32>,
    idle_ticks: u32,
    textures: Arc<TextureTable>,
    /// The texture files that were warned about.
    warned: std::collections::HashSet<String>,
}

impl RopeWorld {
    /// A world with no rope.
    pub fn new() -> RopeWorld {
        RopeWorld::default()
    }

    /// Makes a rope of `params` between two points, both ends pinned there. The rope lies on a sag between them first, and
    /// is its own `length` long (see [`RopeParams::length`]).
    pub fn create(
        &mut self,
        params: RopeParams,
        start: [f32; 3],
        end: [f32; 3],
    ) -> Result<RopeId, RopeError> {
        let segments = params.pieces();
        if self.ropes.len() >= MAX_ROPES {
            return Err(RopeError::TooManyRopes);
        }
        if self.total_segments() + segments > MAX_TOTAL_SEGMENTS {
            return Err(RopeError::TooManySegments);
        }
        let id = self.next.wrapping_add(1).max(1);
        self.next = id;
        self.ropes
            .insert(id, Rope::new(params, point(start), point(end)));
        self.revision += 1;
        Ok(id)
    }

    /// Takes a rope away. False: there was none.
    pub fn remove(&mut self, id: RopeId) -> bool {
        let gone = self.ropes.remove(&id).is_some();
        if gone {
            self.revision += 1;
        }
        gone
    }

    /// Takes every rope away.
    pub fn clear(&mut self) {
        if !self.ropes.is_empty() {
            self.revision += 1;
        }
        self.ropes.clear();
    }

    fn rope_mut(&mut self, id: RopeId) -> Result<&mut Rope, RopeError> {
        self.ropes.get_mut(&id).ok_or(RopeError::NoSuchRope(id))
    }

    /// Pins an end to a point (the end glides there over the next substeps).
    pub fn pin_point(&mut self, id: RopeId, end: RopeEnd, at: [f32; 3]) -> Result<(), RopeError> {
        let rope = self.rope_mut(id)?;
        let here = rope.pos[if end == RopeEnd::Start {
            0
        } else {
            rope.pos.len() - 1
        }];
        rope.pins[end.index()] = Some(Pin {
            object: None,
            at: here,
            target: point(at),
            ghost: None,
            drawn: None,
            aim: None,
            ghost_seen: false,
            offset: Vec3::ZERO,
        });
        self.revision += 1;
        Ok(())
    }

    /// Pins an end to an object: the end follows what [`RopeWorld::resolve`] reads for it (until the first reading it stays
    /// where it is).
    pub fn pin_object(
        &mut self,
        id: RopeId,
        end: RopeEnd,
        object: ObjId,
        node: &str,
    ) -> Result<(), RopeError> {
        self.pin_object_at(id, end, object, node, [0.0; 3])
    }

    /// [`RopeWorld::pin_object`] holding the point `offset` from the node, in the object's own frame: the end follows the object
    /// through the ghost the frame draws for it (see [`RopeWorld::pin_ghost_at`]) and so rotates with it; an object the frame
    /// draws no ghost for is followed by the script's reading, and the offset is then added to it unrotated.
    pub fn pin_object_at(
        &mut self,
        id: RopeId,
        end: RopeEnd,
        object: ObjId,
        node: &str,
        offset: [f32; 3],
    ) -> Result<(), RopeError> {
        let rope = self.rope_mut(id)?;
        let here = rope.pos[if end == RopeEnd::Start {
            0
        } else {
            rope.pos.len() - 1
        }];
        rope.pins[end.index()] = Some(Pin {
            object: Some((object, node.to_owned())),
            at: here,
            target: here,
            ghost: None,
            drawn: None,
            aim: None,
            ghost_seen: false,
            offset: point(offset),
        });
        self.revision += 1;
        Ok(())
    }

    /// Pins an end to a ghost as the frame draws it: each frame the end goes where `drawn(ghost, node)` says (see
    /// [`RopeWorld::step_frame`]), exactly. Until the ghost is drawn the end stays where it is.
    pub fn pin_ghost(
        &mut self,
        id: RopeId,
        end: RopeEnd,
        ghost: u32,
        node: &str,
    ) -> Result<(), RopeError> {
        self.pin_ghost_at(id, end, ghost, node, [0.0; 3])
    }

    /// [`RopeWorld::pin_ghost`] holding the point `offset` from the node, in the ghost's own frame: the end is where the frame draws
    /// that point of the ghost (the node's place moved by the offset turned with the ghost's drawn transform), each frame, so it
    /// rotates with the ghost and moves with its interpolation. A client that draws no frame ([`RopeWorld::follow_ghosts`]) adds
    /// the offset to the ghost's position unrotated.
    pub fn pin_ghost_at(
        &mut self,
        id: RopeId,
        end: RopeEnd,
        ghost: u32,
        node: &str,
        offset: [f32; 3],
    ) -> Result<(), RopeError> {
        let rope = self.rope_mut(id)?;
        let here = rope.pos[if end == RopeEnd::Start {
            0
        } else {
            rope.pos.len() - 1
        }];
        rope.pins[end.index()] = Some(Pin {
            object: None,
            at: here,
            target: here,
            ghost: Some((ghost, node.to_owned())),
            drawn: None,
            aim: None,
            ghost_seen: false,
            offset: point(offset),
        });
        self.revision += 1;
        Ok(())
    }

    /// Finds the ghost each end that hangs from an object follows (`ghost_of(object)`; None: the object has no drawn shape and
    /// the end follows what the script reads).
    pub fn resolve_ghosts(&mut self, mut ghost_of: impl FnMut(ObjId) -> Option<u32>) {
        for rope in self.ropes.values_mut() {
            for held in rope.pins.iter_mut().flatten() {
                let Some((id, node)) = &held.object else {
                    continue;
                };
                held.ghost = ghost_of(*id).map(|g| (g, node.clone()));
                if held.ghost.is_none() {
                    held.drawn = None;
                }
            }
        }
    }

    /// Reads where the ghosts that ends pinned with [`RopeWorld::pin_ghost`] follow are, for a client that draws no frame
    /// (`read(ghost, node)`: None when the client has no such ghost). An end whose ghost was read once and is gone lets go, as an end
    /// hung from a deleted object does; the ends that did are returned. Ends whose ghost the frame draws follow that instead
    /// ([`RopeWorld::step_frame`]).
    pub fn follow_ghosts(
        &mut self,
        mut read: impl FnMut(u32, &str) -> Option<[f32; 3]>,
    ) -> Vec<(RopeId, RopeEnd)> {
        let mut lost = Vec::new();
        for (&id, rope) in &mut self.ropes {
            for (k, end) in [RopeEnd::Start, RopeEnd::End].into_iter().enumerate() {
                let Some(held) = &mut rope.pins[k] else {
                    continue;
                };
                if held.object.is_some() {
                    continue;
                }
                let Some((ghost, node)) = &held.ghost else {
                    continue;
                };
                match read(*ghost, node) {
                    Some(at) => {
                        held.ghost_seen = true;
                        if held.drawn.is_none() {
                            held.target = point((Vec3::from(at) + held.offset).to_array());
                        }
                    }
                    None if held.ghost_seen => {
                        rope.pins[k] = None;
                        lost.push((id, end));
                    }
                    None => {}
                }
            }
        }
        if !lost.is_empty() {
            self.revision += 1;
        }
        lost
    }

    /// Sets rope `id`'s length to `length` (held inside [`MIN_LENGTH`] and [`MAX_LENGTH`]): at once when `seconds` is not a
    /// positive number, else reeled in or out evenly over `seconds` (at most [`MAX_SECONDS`]) from the length it has now. A line's
    /// length is the distance between its ends and is not changed. A `length` that is not a number does nothing.
    pub fn set_length(&mut self, id: RopeId, length: f32, seconds: f32) -> Result<(), RopeError> {
        self.rope_mut(id)?.reel_to(length, seconds);
        self.revision += 1;
        Ok(())
    }

    /// Makes rope `id` a temporary one: it is taken away `seconds` (at most [`MAX_SECONDS`]) from now, and shows less of itself
    /// ([`Rope::fade`]) over its last `fade` seconds (held to `0..=seconds`). `seconds` that is not a positive number makes it last
    /// until it is taken away.
    pub fn set_lifetime(&mut self, id: RopeId, seconds: f32, fade: f32) -> Result<(), RopeError> {
        let rope = self.rope_mut(id)?;
        rope.life = (seconds.is_finite() && seconds > 0.0).then(|| {
            let left = seconds.min(MAX_SECONDS);
            let fade = if fade.is_finite() {
                fade.clamp(0.0, left)
            } else {
                0.0
            };
            Life { left, fade }
        });
        self.revision += 1;
        Ok(())
    }

    /// Lets an end go: it falls with the rest.
    pub fn release(&mut self, id: RopeId, end: RopeEnd) -> Result<(), RopeError> {
        let rope = self.rope_mut(id)?;
        rope.pins[end.index()] = None;
        self.revision += 1;
        Ok(())
    }

    /// Gives every free point of a rope the velocity `v` (units a second), up to [`MAX_SPEED`].
    pub fn impulse(&mut self, id: RopeId, v: [f32; 3]) -> Result<(), RopeError> {
        let v = point(v).clamp_length_max(MAX_SPEED) * SUBSTEP_SECONDS;
        let rope = self.rope_mut(id)?;
        for i in 0..rope.pos.len() {
            if !rope.fixed(i) && rope.params.style != RopeStyle::Line {
                rope.prev[i] = rope.pos[i] - v;
            }
        }
        self.revision += 1;
        Ok(())
    }

    /// Reads where the objects that ropes hang from are: `read(object, node)` is the place (None: the object is gone, and the
    /// end it held lets go).
    pub fn resolve(&mut self, mut read: impl FnMut(ObjId, &str) -> Option<[f32; 3]>) {
        let mut released = false;
        for rope in self.ropes.values_mut() {
            for pin in &mut rope.pins {
                let Some(held) = pin else { continue };
                let Some((id, node)) = &held.object else {
                    continue;
                };
                match read(*id, node) {
                    Some(at) => held.target = point((Vec3::from(at) + held.offset).to_array()),
                    None => {
                        *pin = None;
                        released = true;
                    }
                }
            }
        }
        if released {
            self.revision += 1;
        }
    }

    /// Keeps the free points of every rope above the height `z` (None: nothing is in the way).
    pub fn set_floor(&mut self, z: Option<f32>) {
        self.floor = z.map(|z| point([0.0, 0.0, z]).z);
    }

    /// The floor, if there is one.
    pub fn floor(&self) -> Option<f32> {
        self.floor
    }

    /// Runs the ropes for a drawn frame at `time` (seconds on the frame's clock: the first call, a time that did not move forward
    /// and a second view of the same frame run nothing). The ends that follow a ghost go where `drawn(ghost, node)` says the
    /// frame draws it (None: not drawn: the end follows the script's reading), exactly, each frame; the ropes then run by the time
    /// since the last frame in substeps ([`SUBSTEP_SECONDS`], at most [`MAX_SUBSTEPS`]) and are shown between their last two
    /// states ([`Rope::drawn_points`]).
    pub fn step_frame(&mut self, time: f32, mut drawn: impl FnMut(u32, &str) -> Option<Mount>) {
        self.step_frame_at(time, |ghost, node, _| drawn(ghost, node));
    }

    /// [`RopeWorld::step_frame`] for a drawer that places the offsets of the ends too: `drawn(ghost, node, offset)` is where the
    /// frame draws the point `offset` from `node` in the ghost's own frame ([`RopeWorld::pin_ghost_at`]; `[0.0; 3]` for an end
    /// pinned without one), with the way the mount points.
    pub fn step_frame_at(
        &mut self,
        time: f32,
        mut drawn: impl FnMut(u32, &str, [f32; 3]) -> Option<Mount>,
    ) {
        self.idle_ticks = 0;
        let seconds = match self.clock {
            Some(last) if time.is_finite() && time > last => time - last,
            _ => 0.0,
        };
        if time.is_finite() {
            self.clock = Some(time);
        }
        for rope in self.ropes.values_mut() {
            for held in rope.pins.iter_mut().flatten() {
                let Some((ghost, node)) = &held.ghost else {
                    continue;
                };
                let mount = drawn(*ghost, node, held.offset.to_array());
                held.drawn = mount.map(|m| point(m.at));
                held.aim = mount.and_then(|m| m.dir).map(Vec3::from);
                if let Some(at) = held.drawn {
                    held.target = at;
                }
            }
        }
        self.advance(seconds);
        if seconds > 0.0 && !self.ropes.is_empty() {
            self.revision += 1;
        }
    }

    /// The client's tick: runs the ropes by `seconds` when no frame has been drawn for [`IDLE_TICKS`] ticks (a client that does
    /// not draw keeps its ropes running; one that draws runs them by [`RopeWorld::step_frame`]).
    pub fn tick(&mut self, seconds: f32) {
        self.idle_ticks = self.idle_ticks.saturating_add(1);
        if self.idle_ticks > IDLE_TICKS {
            self.advance(seconds);
        }
    }

    /// Lays the driven ropes out where their ends are drawn now.
    fn drive_ropes(&mut self) {
        let mut moved = false;
        for rope in self.ropes.values_mut().filter(|r| r.driven()) {
            moved |= rope.drive();
        }
        if moved {
            self.revision += 1;
        }
    }

    /// Gives rope `id` the shape `curve` (None: straight): it is driven from then on, not simulated.
    pub fn set_curve(&mut self, id: RopeId, curve: Option<Curve>) -> Result<(), RopeError> {
        self.rope_mut(id)?.curve = curve.map(Curve::sanitized);
        self.revision += 1;
        Ok(())
    }

    /// Takes a rope's driver's hold off it: it is no longer at its limit, and a length a driver gives next is set at once. The ends,
    /// the length and the curve stay as they were.
    pub fn release_drive(&mut self, id: RopeId) {
        if let Some(rope) = self.ropes.get_mut(&id) {
            rope.at_limit = false;
            rope.driven_length = None;
            self.revision += 1;
        }
    }

    /// Steers rope `id` as a driver says ([`Drive`]): the two ends, the length when the drive has one, whether the rope is at its
    /// limit and its curve. Nothing is done for what is already so (a driver may call this every frame). A point end is where the
    /// frame draws it exactly; a ghost end follows its ghost every frame with no more calls. The rope stays a visual: this moves
    /// what is drawn and pulls on nothing.
    pub fn drive(&mut self, id: RopeId, drive: &Drive) -> Result<(), RopeError> {
        let rope = self.rope_mut(id)?;
        let mut changed = rope.drive_end(RopeEnd::Start, &drive.start);
        changed |= rope.drive_end(RopeEnd::End, &drive.end);
        match drive.length.filter(|l| l.is_finite()) {
            Some(length) => {
                let to = length.clamp(MIN_LENGTH, MAX_LENGTH);
                // What the rope is going to have: a script or a ghost record that changed it since is put back.
                let target = rope.reel.map_or_else(|| rope.length(), |reel| reel.to);
                if rope.params.style != RopeStyle::Line
                    && (rope.driven_length.is_none()
                        || (target - to).abs() > (to * 1.0e-6).max(1.0e-4))
                {
                    let seconds = if rope.driven_length.is_some() {
                        drive.settle
                    } else {
                        0.0
                    };
                    rope.reel_to(to, seconds);
                    changed = true;
                }
                rope.driven_length = Some(to);
            }
            None => rope.driven_length = None,
        }
        let curve = drive.curve.map(Curve::sanitized);
        if rope.curve != curve {
            rope.curve = curve;
            changed = true;
        }
        if rope.at_limit != drive.at_limit {
            rope.at_limit = drive.at_limit;
            changed = true;
        }
        if changed {
            self.revision += 1;
        }
        Ok(())
    }

    fn refresh_shown(&mut self) {
        self.drive_ropes();
        // `pos` is the state `left` before the frame's time and `before` one substep earlier: the rope is shown as far on from
        // `before` toward `pos` as the leftover time is of a step (the more time left over, the nearer `pos`), so it moves on
        // every frame by the same amount.
        let share = self.left / SUBSTEP_SECONDS;
        for rope in self.ropes.values_mut() {
            rope.refresh_shown(share);
        }
    }

    /// Runs the ropes on by `seconds` ([module docs](self)). A time that is not a positive finite number does nothing.
    pub fn advance(&mut self, seconds: f32) {
        if !(seconds.is_finite() && seconds > 0.0) {
            self.refresh_shown();
            return;
        }
        let most = SUBSTEP_SECONDS * MAX_SUBSTEPS as f32;
        self.left = (self.left + seconds.min(most)).min(most + SUBSTEP_SECONDS);
        let steps = ((self.left / SUBSTEP_SECONDS) as usize).min(MAX_SUBSTEPS);
        if steps == 0 {
            self.refresh_shown();
            return;
        }
        self.left -= steps as f32 * SUBSTEP_SECONDS;
        let ran = steps as f32 * SUBSTEP_SECONDS;
        let alpha = 1.0 - (-ran / ANCHOR_SMOOTHING).exp();
        let floor = self.floor;
        for rope in self.ropes.values_mut() {
            rope.run(steps, floor, alpha);
        }
        self.substeps += steps as u64;
        self.age(ran);
        self.refresh_shown();
        if !self.ropes.is_empty() {
            self.revision += 1;
        }
    }

    /// Ages the temporary ropes by `seconds`; the ones whose time is up are taken away.
    fn age(&mut self, seconds: f32) {
        let mut over = Vec::new();
        for (&id, rope) in &mut self.ropes {
            if let Some(life) = &mut rope.life {
                life.left -= seconds;
                if life.left <= 0.0 {
                    over.push(id);
                }
            }
        }
        for id in over {
            self.ropes.remove(&id);
            self.revision += 1;
        }
    }

    /// The world's textures, as the pass uploads them.
    pub fn textures(&self) -> Arc<TextureTable> {
        Arc::clone(&self.textures)
    }

    /// Adds the texture of file `path` (filtered `nearest` or not) and returns its number; the one the world holds already for that
    /// file and filter has its image replaced (and is returned).
    pub fn add_texture(
        &mut self,
        path: &str,
        nearest: bool,
        image: Arc<ImageData>,
    ) -> Result<u32, RopeError> {
        let id = self.textures.find(path, nearest);
        let table = Arc::make_mut(&mut self.textures);
        if id != 0 {
            table.entries[id as usize - 1].image = image;
            table.revision += 1;
            self.revision += 1;
            return Ok(id);
        }
        if table.entries.len() >= MAX_TEXTURES {
            return Err(RopeError::TooManyTextures);
        }
        table.entries.push(TextureEntry {
            path: path.to_owned(),
            nearest,
            image,
        });
        table.revision += 1;
        Ok(table.entries.len() as u32)
    }

    /// The textures' files, once each.
    pub fn texture_files(&self) -> Vec<String> {
        let mut files: Vec<String> = Vec::new();
        for e in &self.textures.entries {
            if !files.contains(&e.path) {
                files.push(e.path.clone());
            }
        }
        files
    }

    /// Replaces the image of every texture of file `path` (the file changed on disk); true if there was one.
    pub fn reload_texture(&mut self, path: &str, image: &Arc<ImageData>) -> bool {
        let mut any = false;
        let table = Arc::make_mut(&mut self.textures);
        for e in table.entries.iter_mut().filter(|e| e.path == path) {
            if !Arc::ptr_eq(&e.image, image) {
                e.image = Arc::clone(image);
                any = true;
            }
        }
        if any {
            table.revision += 1;
            self.revision += 1;
        }
        any
    }

    /// True the first time it is asked about `path`: a texture that did not load is warned about once.
    pub fn first_warning(&mut self, path: &str) -> bool {
        if self.warned.len() >= MAX_WARNED {
            return false;
        }
        self.warned.insert(path.to_owned())
    }

    /// Sets the number the datablock's texture of rope `id` is (0: none).
    pub fn set_texture(&mut self, id: RopeId, texture: u32) -> Result<(), RopeError> {
        self.rope_mut(id)?.params.texture = texture;
        self.revision += 1;
        Ok(())
    }

    /// Sets what a script set on rope `id` ([`RopeLook`]): each `Some` replaces the datablock's value for that rope, a `None`
    /// leaves what is set as it is. `clear` puts the values named in it back to the datablock's.
    pub fn set_look(
        &mut self,
        id: RopeId,
        set: RopeLook,
        clear: [bool; 4],
    ) -> Result<(), RopeError> {
        let rope = self.rope_mut(id)?;
        let look = &mut rope.look;
        let [color, texture, scroll, emissive] = clear;
        if color {
            look.color = None;
        }
        if texture {
            look.texture = None;
        }
        if scroll {
            look.scroll = None;
        }
        if emissive {
            look.emissive = None;
        }
        look.color = set.color.or(look.color);
        if let Some(path) = set.texture {
            look.texture = Some(path);
        }
        look.scroll = set.scroll.map(clean_scroll).or(look.scroll);
        look.emissive = set.emissive.or(look.emissive);
        self.revision += 1;
        Ok(())
    }

    /// How many ropes there are.
    pub fn len(&self) -> usize {
        self.ropes.len()
    }

    /// Whether there is no rope.
    pub fn is_empty(&self) -> bool {
        self.ropes.is_empty()
    }

    /// The segments of all the ropes.
    pub fn total_segments(&self) -> usize {
        self.ropes.values().map(Rope::segments).sum()
    }

    /// The rope `id`.
    pub fn rope(&self, id: RopeId) -> Option<&Rope> {
        self.ropes.get(&id)
    }

    /// Every rope, by number.
    pub fn ropes(&self) -> impl Iterator<Item = (RopeId, &Rope)> + '_ {
        self.ropes.iter().map(|(id, rope)| (*id, rope))
    }

    /// How many substeps have run.
    pub fn substeps(&self) -> u64 {
        self.substeps
    }

    /// A count that changes whenever a rope does.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The tubes of the ropes drawn as tubes, as triangles, made once for each [`RopeWorld::revision`].
    pub fn mesh(&mut self) -> Arc<TubeMesh> {
        if let Some(mesh) = &self.mesh
            && mesh.revision == self.revision
        {
            return Arc::clone(mesh);
        }
        let textures = Arc::clone(&self.textures);
        let mesh = Arc::new(TubeMesh::build(
            self.revision,
            self.ropes
                .values()
                .filter(|rope| rope.params.style == RopeStyle::Tube)
                .map(|rope| {
                    let look = rope.appearance(&textures);
                    // A tube about to go thins out (it is not see-through).
                    let thin = rope.fade().max(0.02);
                    TubeSource {
                        points: rope.drawn_points(),
                        thickness: rope.params.thickness * thin,
                        color: look.color,
                        cast_shadow: rope.params.cast_shadow,
                        receive_shadow: rope.params.receive_shadow,
                        texture: look.texture,
                        texture_length: rope.params.texture_length,
                        scroll: look.scroll,
                        thickness_end: rope.params.end_thickness() * thin,
                        color_end: rope.params.color_end,
                    }
                }),
        ));
        self.mesh = Some(Arc::clone(&mesh));
        mesh
    }

    /// The curves of the ropes drawn as ribbons and lines, made once for each [`RopeWorld::revision`].
    pub fn strips(&mut self) -> Arc<StripSet> {
        if let Some(strips) = &self.strips
            && strips.revision == self.revision
        {
            return Arc::clone(strips);
        }
        let textures = Arc::clone(&self.textures);
        // A strip about to go grows fainter: its colors' alpha by the rope's fade.
        let faded = |alpha: u8, fade: f32| (f32::from(alpha) * fade).round() as u8;
        let items: Vec<(&[Vec3], RopeParams, Appearance)> = self
            .ropes
            .values()
            .filter(|rope| rope.params.style != RopeStyle::Tube)
            .map(|rope| {
                let fade = rope.fade();
                let mut params = rope.params;
                let mut look = rope.appearance(&textures);
                if fade < 1.0 {
                    look.color[3] = faded(look.color[3], fade);
                    if let Some(end) = &mut params.color_end {
                        end[3] = faded(end[3], fade);
                    }
                }
                (rope.drawn_points(), params, look)
            })
            .collect();
        let strips = Arc::new(StripSet::build(
            self.revision,
            items
                .iter()
                .map(|(points, params, look)| (*points, params, *look)),
        ));
        self.strips = Some(Arc::clone(&strips));
        strips
    }
}
