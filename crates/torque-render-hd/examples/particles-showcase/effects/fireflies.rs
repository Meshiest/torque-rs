//! Fireflies over a meadow at dusk: a few dozen of them, unseen as they drift slowly through the grass until one lights:
//! a small crisp point of light that creeps along a short gentle curve (a little arc, rising a touch) and fades out along
//! it, a thin short streak of the same points behind it. Each keeps its own irregular pace, some give two in a row, and a wisp now and then sets off a neighbor,
//! so only a handful are lit at any moment. The sun is down behind the tree line; a thin mist drifts over the meadow.

use std::f32::consts::{PI, TAU};
use std::ops::Range;

use glam::{Vec2, Vec3};
use torque_render_scene::hd::{DynamicObject, HdLight};
use torque_render_scene::particles::{EmitterData, ParticleData, ParticleExtras, Rng};

use super::{Effect, Floor, Lcg, Node, Setting, emitter, one, particle};
use crate::scenery;
use crate::sprites::{MIST, SPARK};

/// The camera, and the air the fireflies wander in.
const EYE: [f32; 3] = [0.0, -7.0, 1.7];
const MEADOW_X: Range<f32> = -8.5..8.5;
const MEADOW_Y: Range<f32> = -3.0..12.5;
const HEIGHT: Range<f32> = 0.3..2.6;

/// A wisp's brightness is laid in steps of 1/LEVELS, each its own look.
const LEVELS: usize = 7;
/// Seconds a wisp flies, how far it goes (units), and the seconds between a firefly's wisps.
const WISP: Range<f32> = 1.0..1.5;
const SPAN: Range<f32> = 0.15..0.35;
const GAP: Range<f32> = 6.0..14.0;

/// A wisp in flight: its curve and when it started.
struct Wisp {
    start: Vec3,
    forward: Vec3,
    side: Vec3,
    rise: f32,
    length: f32,
    t0: f32,
}

impl Wisp {
    /// The head `u` (0..1) of the way along: a gentle arc forward that rises a little and sways once to the side.
    fn at(&self, u: f32) -> Vec3 {
        self.start
            + self.forward * u
            + Vec3::Z * (self.rise * (PI * u).sin())
            + self.side * (0.2 * (TAU * u).sin())
    }

    /// Its brightness `u` of the way along: lights up, flies, fades out.
    fn brightness(u: f32) -> f32 {
        (PI * u.clamp(0.0, 1.0)).sin().powf(1.2)
    }
}

/// A firefly: its slow drift (unseen) and its rhythm.
struct Firefly {
    pos: Vec3,
    heading: f32,
    turn: f32,
    speed: f32,
    target_z: f32,
    retarget: f32,
    /// When its next wisp starts, the wisp in flight, whether it gives two in a row and whether the next is the second.
    next: f32,
    wisp: Option<Wisp>,
    double: bool,
    again: bool,
    tempo: f32,
}

impl Firefly {
    /// One step of drift: the heading wanders, the height makes slowly for its target; near the meadow's edge it turns in.
    fn drift(&mut self, t: f32, dt: f32, rng: &mut Rng) {
        self.turn = (self.turn + (rng.f() - 0.5) * 2.0 * dt) * (1.0 - 0.5 * dt);
        self.turn = self.turn.clamp(-0.8, 0.8);
        self.heading += self.turn * dt;
        let margin = 1.0;
        if self.pos.x < MEADOW_X.start + margin
            || self.pos.x > MEADOW_X.end - margin
            || self.pos.y < MEADOW_Y.start + margin
            || self.pos.y > MEADOW_Y.end - margin
        {
            let center = Vec2::new(0.0, 0.5 * (MEADOW_Y.start + MEADOW_Y.end));
            let to = center - self.pos.truncate();
            let want = to.y.atan2(to.x);
            let diff = (want - self.heading + PI).rem_euclid(TAU) - PI;
            self.heading += diff.clamp(-2.0 * dt, 2.0 * dt);
        }
        self.pos += Vec3::new(self.heading.cos(), self.heading.sin(), 0.0) * (self.speed * dt);
        if t >= self.retarget {
            self.retarget = t + 4.0 + 6.0 * rng.f();
            self.target_z = HEIGHT.start + (HEIGHT.end - HEIGHT.start) * rng.f().powf(1.8);
        }
        self.pos.z += (self.target_z - self.pos.z) * 0.3 * dt;
    }

    /// A wisp starts here: a short curve ahead, a little to one side, up a touch.
    fn light(&mut self, t: f32, rng: &mut Rng) -> &Wisp {
        let span = SPAN.start + (SPAN.end - SPAN.start) * rng.f();
        let heading = self.heading + (rng.f() - 0.5) * 1.2;
        let forward = Vec3::new(heading.cos(), heading.sin(), 0.0) * span;
        let side = Vec3::new(-heading.sin(), heading.cos(), 0.0) * span;
        self.wisp = Some(Wisp {
            start: self.pos,
            forward,
            side: side * if rng.f() < 0.5 { 1.0 } else { -1.0 },
            rise: (0.1 + 0.4 * rng.f()) * span,
            length: WISP.start + (WISP.end - WISP.start) * rng.f(),
            t0: t,
        });
        self.wisp.as_ref().expect("the wisp just lit")
    }
}

pub struct Fireflies {
    nodes: Vec<Node>,
    flies: Vec<Firefly>,
    /// The glows by brightness level (1..=LEVELS).
    glows: Range<usize>,
    mist: usize,
    mist_due: f32,
}

impl Fireflies {
    pub fn new() -> Fireflies {
        // A point of light: a tight spark laid every step along a wisp; the fresh ones at the head make its light, the
        // older ones a thin short streak behind it.
        let glow = ParticleData {
            lifetime_ms: 260,
            colors: [
                [1.0, 0.97, 0.6, 0.0],
                [1.0, 0.92, 0.45, 0.45],
                [1.0, 0.8, 0.3, 0.12],
                [1.0, 0.7, 0.2, 0.0],
            ],
            times: [0.0, 0.1, 0.5, 1.0],
            ..particle(SPARK)
        };
        let mist = ParticleData {
            drag: 0.5,
            lifetime_ms: 9000,
            lifetime_variance_ms: 3000,
            inv_alpha: true,
            spin_min: -4.0,
            spin_max: 4.0,
            colors: [
                [0.55, 0.6, 0.75, 0.0],
                [0.55, 0.6, 0.75, 0.09],
                [0.55, 0.6, 0.75, 0.08],
                [0.55, 0.6, 0.75, 0.0],
            ],
            sizes: [2.5, 4.0, 5.0, 5.5],
            times: [0.0, 0.2, 0.6, 1.0],
            ..particle(MIST)
        };
        let mut nodes = Vec::new();
        let first = nodes.len();
        for level in 1..=LEVELS {
            let k = level as f32 / LEVELS as f32;
            let size = 0.025 + 0.03 * k;
            one(
                &mut nodes,
                EmitterData {
                    velocity: 0.0,
                    velocity_variance: 0.0,
                    ..emitter()
                },
                vec![(
                    ParticleData {
                        colors: glow.colors.map(|c| [c[0], c[1], c[2], c[3] * k]),
                        sizes: [size; 4],
                        ..glow.clone()
                    },
                    ParticleExtras::default(),
                )],
            );
        }
        let glows = first..nodes.len();
        let mist = one(
            &mut nodes,
            EmitterData {
                velocity: 0.04,
                velocity_variance: 0.02,
                theta_max: 10.0,
                ..emitter()
            },
            vec![(
                mist,
                ParticleExtras {
                    turbulence: 0.25,
                    turbulence_scale: 3.5,
                    ..ParticleExtras::default()
                },
            )],
        );
        let mut g = Lcg(5);
        let flies = (0..90)
            .map(|_| Firefly {
                // More of them near the camera, most of them low.
                pos: Vec3::new(
                    g.range(MEADOW_X.start, MEADOW_X.end),
                    MEADOW_Y.start + (MEADOW_Y.end - MEADOW_Y.start) * g.unit().powf(1.5),
                    HEIGHT.start + 1.2 * g.unit().powf(2.0),
                ),
                heading: g.range(0.0, TAU),
                turn: g.range(-0.3, 0.3),
                speed: g.range(0.04, 0.12),
                target_z: HEIGHT.start + 1.0 * g.unit().powf(1.8),
                retarget: g.range(0.0, 6.0),
                next: g.range(0.0, 10.0),
                wisp: None,
                double: g.unit() < 0.3,
                again: false,
                tempo: g.range(0.8, 1.25),
            })
            .collect();
        Fireflies {
            nodes,
            flies,
            glows,
            mist,
            mist_due: 0.0,
        }
    }

    /// The meadow: its grass, the trees behind it and the dark line of the woods on the horizon.
    fn scenery() -> Vec<DynamicObject> {
        let mut grass = Vec::new();
        scenery::blades(17, 4500, -9.0..9.0, -4.5..5.0, &mut grass);
        scenery::blades(18, 2500, -12.0..12.0, 5.0..16.0, &mut grass);
        let mut trees = Vec::new();
        let mut g = Lcg(19);
        for (k, (x, y)) in [
            (-13.0, 19.0),
            (-7.5, 21.5),
            (-2.0, 24.0),
            (4.0, 20.5),
            (9.5, 23.0),
            (14.5, 18.5),
            (-18.0, 24.0),
            (19.0, 23.0),
        ]
        .into_iter()
        .enumerate()
        {
            let height = g.range(3.0, 5.0);
            let foot = Vec3::new(x, y, 0.0);
            scenery::cuboid(
                foot + Vec3::Z * height * 0.5,
                Vec3::new(0.18, 0.18, height * 0.5),
                [14, 12, 10],
                &mut trees,
            );
            for j in 0..3 {
                let off = Vec3::new(g.range(-1.2, 1.2), g.range(-0.8, 0.8), g.range(-0.6, 1.0));
                let r = g.range(1.6, 2.6);
                scenery::canopy(
                    foot + Vec3::Z * (height + 0.6) + off,
                    Vec3::new(r, r * g.range(0.8, 1.1), r * 0.75),
                    100 + (k * 3 + j) as u64,
                    [9, 15, 8],
                    &mut trees,
                );
            }
        }
        let mut woods = Vec::new();
        scenery::cuboid(
            Vec3::new(0.0, 31.0, 1.1),
            Vec3::new(48.0, 3.0, 1.1),
            [4, 5, 4],
            &mut woods,
        );
        vec![
            scenery::mesh(100, "grass", grass, false, None),
            scenery::mesh(101, "trees", trees, false, None),
            scenery::mesh(102, "woods", woods, false, None),
        ]
    }
}

impl Effect for Fireflies {
    fn setting(&self) -> Setting {
        Setting {
            // The sun just down behind the trees: its last light rims the canopies; the sky's blue is the ambient.
            sun: ([0.25, 0.96, 0.07], [0.32, 0.17, 0.09], [0.075, 0.085, 0.14]),
            clear: [0.03, 0.035, 0.065],
            scenery: Fireflies::scenery(),
            floor: Floor::Grass,
            ..Setting::new(EYE, [0.4, 8.0, 0.6], 58)
        }
    }

    fn step(&mut self, t: f32, ms: u32, rng: &mut Rng) {
        let dt = ms as f32 / 1000.0;
        // The wisps that start this step: a neighbor may answer one.
        let mut started = Vec::new();
        for fly in &mut self.flies {
            if fly.wisp.is_none() {
                fly.drift(t, dt, rng);
                if t >= fly.next {
                    started.push(fly.light(t, rng).start);
                }
            }
            let Some(wisp) = &fly.wisp else { continue };
            let u = (t - wisp.t0) / wisp.length;
            if u < 1.0 {
                let level = (Wisp::brightness(u) * LEVELS as f32).round() as usize;
                if level >= 1 {
                    self.nodes[self.glows.start + level - 1].burst(wisp.at(u), Vec3::Z, 1, rng);
                }
                continue;
            }
            // The wisp is out: the firefly flies on from where it ended.
            fly.pos = wisp.at(1.0);
            fly.wisp = None;
            if fly.double && !fly.again {
                fly.again = true;
                fly.next = t + 0.5 + 0.4 * rng.f();
            } else {
                fly.again = false;
                fly.next = t + fly.tempo * (GAP.start + (GAP.end - GAP.start) * rng.f());
            }
        }
        // Loose waves: a firefly near one that just lit, with a while still to wait, sometimes answers soon.
        for fly in &mut self.flies {
            if fly.wisp.is_some() || fly.next < t + 1.5 {
                continue;
            }
            if started.iter().any(|p| p.distance_squared(fly.pos) < 4.0) && rng.f() < 0.3 {
                fly.next = t + 0.3 + 1.0 * rng.f();
            }
        }
        // The mist: a sheet born low over the meadow, one puff at a time.
        self.mist_due -= dt;
        while self.mist_due <= 0.0 {
            self.mist_due += 0.12;
            let at = Vec3::new(
                MEADOW_X.start - 1.0 + rng.f() * (MEADOW_X.end - MEADOW_X.start + 2.0),
                MEADOW_Y.start + rng.f() * (MEADOW_Y.end - MEADOW_Y.start),
                0.3,
            );
            self.nodes[self.mist].burst(at, Vec3::Z, 1, rng);
        }
    }

    fn nodes(&mut self) -> &mut [Node] {
        &mut self.nodes
    }

    /// The fireflies are their own light only: a point light each made patches on the grass that read as lamps.
    fn lights(&self, _t: f32, _frame: u32) -> Vec<HdLight> {
        Vec::new()
    }
}
