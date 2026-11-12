//! A railgun at night: each shot is a solid beam (a white-hot core in a blue halo, a camera-facing glowing decal) that fades
//! in a third of a second and leaves a trail of rings along its path, each a thin glowing circle perpendicular to the beam.
//! The rings linger a few seconds and break up, the ones near the muzzle first: within a quarter second a ring is cut into
//! three arcs, the arcs into shorter ones, those into dashes, then the dashes shrink away, every piece still on its circle,
//! their ends soft, dimming as they go and shedding motes. The target, a stack of boxes, bursts with a flash into a big
//! soft cloud of blue dust (the smoke sprites tinted blue, alpha-blended, billowing out and up from the whole stack, then
//! settling and drifting), lit faintly from within, with sparks and fine glowing grains inside it; its boxes tumble away.
//! Three shots at three targets.

use std::f32::consts::TAU;

use glam::Vec3;
use torque_render_scene::hd::{DynamicObject, HdLight};
use torque_render_scene::particles::{EmitterData, ParticleData, ParticleExtras, Rng};
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Draw, Vertex};

use super::{Effect, Node, Setting, emitter, hash, light, one, particle, plain, smooth};
use crate::scenery::{self, beam, cuboid, ribbon};
use crate::sprites::{GLOW, LINE, MOTE, SMOKES, SPARK};

const EYE: Vec3 = Vec3::new(-2.8, -4.5, 2.3);
const MUZZLE: Vec3 = Vec3::new(0.0, 1.4, 1.15);
/// The shots: when, and which target.
const SHOTS: [(f32, usize); 3] = [(1.2, 1), (4.6, 0), (8.0, 2)];
/// How long the beam shows, how long the rings last, their spacing along the beam and their radius.
const BEAM_S: f32 = 0.35;
const RING_S: f32 = 3.6;
const RING_STEP: f32 = 0.55;
const RING_R: f32 = 0.42;
const RING_SEGMENTS: usize = 120;
/// Each ring's clock runs this much behind the one before it along the beam: the breaking runs out from the muzzle.
const RING_WAVE: f32 = 0.03;
/// The cuts that break a ring, on the ring's own clock: when they open (s), how many around the circle, how fast each gap
/// widens (turns of the circle a second) and how wide one may get (turns): the first three leave the pieces between them,
/// the last closes the dashes completely.
const CUTS: [(f32, u32, f32, f32); 4] = [
    (0.25, 3, 0.04, 0.05),
    (0.55, 8, 0.03, 0.02),
    (0.9, 18, 0.02, 0.009),
    (1.3, 48, 0.005, 0.5),
];
/// Half the stack a target is: the dust comes out of all of it.
const TARGET_HALF: Vec3 = Vec3::new(0.75, 0.35, 0.9);

/// A target: a stack of boxes (center, half sides, color) until it is hit, then its boxes as debris (with a velocity).
struct Target {
    aim: Vec3,
    boxes: Vec<(Vec3, Vec3, [u8; 3])>,
    hit: bool,
    debris: Vec<(Vec3, Vec3, Vec3, [u8; 3])>,
}

impl Target {
    /// A stack at (`x`, `y`): two boxes side by side, one across them, a small one on top.
    fn stack(x: f32, y: f32) -> Target {
        let gray = [92, 90, 86];
        let rust = [110, 68, 46];
        Target {
            aim: Vec3::new(x, y, 0.9),
            boxes: vec![
                (Vec3::new(x - 0.37, y, 0.35), Vec3::splat(0.35), gray),
                (Vec3::new(x + 0.37, y, 0.35), Vec3::splat(0.35), rust),
                (Vec3::new(x, y, 1.05), Vec3::new(0.7, 0.3, 0.35), gray),
                (Vec3::new(x, y, 1.62), Vec3::splat(0.22), rust),
            ],
            hit: false,
            debris: Vec::new(),
        }
    }

    /// The beam along `dir` hits: the boxes fly apart.
    fn smash(&mut self, dir: Vec3, rng: &mut Rng) {
        self.hit = true;
        self.debris = self
            .boxes
            .iter()
            .map(|&(center, half, color)| {
                let away = (center - self.aim).normalize_or(Vec3::Z);
                let vel =
                    away * (3.0 + 3.0 * rng.f()) + Vec3::Z * (2.5 + 3.0 * rng.f()) + dir * 2.5;
                (center, vel, half, color)
            })
            .collect();
    }

    fn tumble(&mut self, dt: f32) {
        for (pos, vel, half, _) in &mut self.debris {
            vel.z -= 9.8 * dt;
            *vel *= 1.0 - 0.3 * dt;
            *pos += *vel * dt;
            if pos.z < half.z {
                pos.z = half.z;
                vel.z = -vel.z * 0.25;
                *vel *= 0.6;
            }
        }
    }

    fn shape(&self, out: &mut Vec<Vertex>) {
        if self.hit {
            for &(pos, _, half, color) in &self.debris {
                cuboid(pos, half, color, out);
            }
        } else {
            for &(center, half, color) in &self.boxes {
                cuboid(center, half, color, out);
            }
        }
    }
}

struct Shot {
    t0: f32,
    from: Vec3,
    to: Vec3,
    seed: u32,
}

impl Shot {
    fn dir(&self) -> Vec3 {
        (self.to - self.from).normalize_or(Vec3::Y)
    }

    /// The rings' centers along the beam.
    fn rings(&self) -> impl Iterator<Item = Vec3> + '_ {
        let dir = self.dir();
        let reach = self.from.distance(self.to) - 0.4;
        (0..)
            .map(move |k| 0.8 + RING_STEP * k as f32)
            .take_while(move |&s| s < reach)
            .map(move |s| self.from + dir * s)
    }

    /// How bright what is left of the rings is at age `a` (0..1).
    fn bright(a: f32) -> f32 {
        (1.0 - a / RING_S).clamp(0.0, 1.0).powf(1.3)
    }

    /// Ring `k`'s own age at the shot's age `a`.
    fn ring_age(a: f32, k: usize) -> f32 {
        a - RING_WAVE * k as f32
    }

    /// How much of ring `k` is left at `turn` of its circle (0..1) at its age `age`: 1 whole, 0 gone. Each of `CUTS` opens
    /// gaps at seeded places around the circle that widen with age, soft over about half a segment at their ends; what is
    /// between them is the pieces.
    fn left(&self, k: usize, age: f32, turn: f32) -> f32 {
        let mut left = 1.0f32;
        for (level, &(start, n, rate, cap)) in CUTS.iter().enumerate() {
            if age < start {
                break;
            }
            let half = (rate * (age - start)).min(cap);
            let feather = (0.6 / RING_SEGMENTS as f32).min(0.3 / n as f32);
            let salt = self.seed ^ (k as u32 * 131 + level as u32 * 7919);
            let phase = hash(salt);
            for i in 0..n {
                let jitter = 0.6 * (hash(salt ^ (i * 17 + 3)) - 0.5);
                let cut = (i as f32 + 0.5 + jitter) / n as f32 + phase;
                let d = ((turn - cut + 0.5).rem_euclid(1.0) - 0.5).abs();
                left *= smooth((d - half) / feather);
                if left <= 0.0 {
                    return 0.0;
                }
            }
        }
        left
    }

    /// The beam and its rings at `t`, into `out` (the line texture across).
    fn shape(&self, t: f32, out: &mut Vec<Vertex>) {
        let a = t - self.t0;
        let dir = self.dir();
        if a < BEAM_S {
            let f = (1.0 - a / BEAM_S).powf(1.5);
            let core = [255, 255, 255, (255.0 * f) as u8];
            let halo = [
                (80.0 * f) as u8,
                (150.0 * f) as u8,
                (255.0 * f) as u8,
                (200.0 * f) as u8,
            ];
            beam(self.from, self.to, 0.1, core, EYE, out);
            beam(self.from, self.to, 0.6, halo, EYE, out);
        }
        let bright = Shot::bright(a);
        let radius = RING_R * (1.0 + 0.4 * a / RING_S);
        let u = dir.cross(Vec3::Z).normalize_or(Vec3::X);
        let v = u.cross(dir).normalize_or(Vec3::Z);
        let mut points: Vec<Vec3> = Vec::with_capacity(RING_SEGMENTS + 1);
        let mut colors: Vec<[u8; 4]> = Vec::with_capacity(RING_SEGMENTS + 1);
        for (k, center) in self.rings().enumerate() {
            let age = Shot::ring_age(a, k);
            // White-hot when just made, the ring's blue soon after.
            let hot = (-age.max(0.0) / 0.5).exp();
            let c = [0.25, 0.55, 1.0].map(|s| (s + (1.0 - s) * hot) * bright);
            points.clear();
            colors.clear();
            let mut any = false;
            // One closed ribbon around the circle: where a piece is gone its color is black, which adds nothing.
            for seg in 0..=RING_SEGMENTS {
                let turn = (seg % RING_SEGMENTS) as f32 / RING_SEGMENTS as f32;
                let angle = TAU * turn;
                points.push(center + (u * angle.cos() + v * angle.sin()) * radius);
                let left = self.left(k, age, turn);
                any |= left > 0.0;
                let f = |s: f32| (s * left * 255.0).min(255.0) as u8;
                colors.push([f(c[0]), f(c[1]), f(c[2]), 255]);
            }
            if any {
                ribbon(&points, &colors, 0.16, 0.0, dir, out);
            }
        }
    }
}

pub struct Railgun {
    /// When the show's clock started (it starts over after the last shot).
    t0: f32,
    nodes: Vec<Node>,
    targets: Vec<Target>,
    shots: Vec<Shot>,
    fired: usize,
    dust: usize,
    glow: usize,
    grains: usize,
    sparks: usize,
    flash: usize,
    motes: usize,
}

impl Railgun {
    pub fn new() -> Railgun {
        // A puff of dust: a smoke sprite (self-shading wisps) alpha-blended, lit by the moonlight like the ground, that
        // swells fast, then hangs and slowly settles. Pale, so it stays brighter than the concrete it covers (the sparks
        // and flash apart); the glows and lights inside give it its blue.
        let puff = ParticleData {
            drag: 1.3,
            gravity: 0.03,
            lifetime_ms: 3800,
            lifetime_variance_ms: 1400,
            inv_alpha: true,
            spin_min: -25.0,
            spin_max: 25.0,
            colors: [
                [0.9, 0.95, 1.0, 0.0],
                [0.82, 0.88, 1.0, 0.5],
                [0.78, 0.84, 1.0, 0.34],
                [0.75, 0.8, 1.0, 0.0],
            ],
            sizes: [0.5, 1.6, 2.8, 3.4],
            times: [0.0, 0.12, 0.55, 1.0],
            ..particle(SMOKES[0])
        };
        // Half the sprites curl on fine turbulence, half on coarse: fibers inside a slow billow.
        let puffs: Vec<(ParticleData, ParticleExtras)> = SMOKES
            .iter()
            .enumerate()
            .map(|(k, name)| {
                let (turbulence, turbulence_scale) =
                    if k % 2 == 0 { (0.9, 0.9) } else { (0.5, 2.2) };
                (
                    ParticleData {
                        textures: vec![name.to_string()],
                        ..puff.clone()
                    },
                    ParticleExtras {
                        turbulence,
                        turbulence_scale,
                        ..ParticleExtras::default()
                    },
                )
            })
            .collect();
        // The cloud's light from within: wide faint glows that add only.
        let glow = ParticleData {
            drag: 1.4,
            lifetime_ms: 2200,
            lifetime_variance_ms: 800,
            colors: [
                [0.35, 0.55, 1.0, 0.0],
                [0.3, 0.5, 1.0, 0.16],
                [0.25, 0.4, 1.0, 0.07],
                [0.2, 0.3, 0.9, 0.0],
            ],
            sizes: [0.6, 1.6, 2.4, 2.6],
            times: [0.0, 0.15, 0.6, 1.0],
            ..particle(GLOW)
        };
        // Fine glowing grains that sparkle inside the cloud and sink slowly.
        let grain = ParticleData {
            drag: 1.1,
            gravity: 0.12,
            lifetime_ms: 2400,
            lifetime_variance_ms: 1000,
            colors: [
                [0.9, 0.95, 1.0, 0.0],
                [0.7, 0.85, 1.0, 1.0],
                [0.4, 0.6, 1.0, 0.6],
                [0.25, 0.35, 1.0, 0.0],
            ],
            sizes: [0.02, 0.04, 0.03, 0.0],
            times: [0.0, 0.1, 0.6, 1.0],
            ..particle(MOTE)
        };
        let spark = ParticleData {
            drag: 0.3,
            gravity: 1.0,
            lifetime_ms: 900,
            lifetime_variance_ms: 400,
            colors: [
                [1.0, 1.0, 1.0, 1.0],
                [0.7, 0.9, 1.0, 1.0],
                [0.3, 0.6, 1.0, 0.7],
                [0.2, 0.3, 1.0, 0.0],
            ],
            sizes: [0.04, 0.035, 0.03, 0.015],
            times: [0.0, 0.15, 0.6, 1.0],
            ..particle(SPARK)
        };
        let flash = ParticleData {
            lifetime_ms: 160,
            colors: [
                [0.8, 0.9, 1.0, 0.9],
                [0.6, 0.8, 1.0, 0.6],
                [0.4, 0.6, 1.0, 0.2],
                [0.3, 0.5, 1.0, 0.0],
            ],
            sizes: [1.5, 2.5, 3.0, 3.2],
            ..particle(GLOW)
        };
        let mote = ParticleData {
            drag: 1.0,
            lifetime_ms: 1300,
            lifetime_variance_ms: 400,
            colors: [
                [0.6, 0.85, 1.0, 0.0],
                [0.5, 0.75, 1.0, 0.8],
                [0.35, 0.55, 1.0, 0.4],
                [0.25, 0.35, 1.0, 0.0],
            ],
            sizes: [0.03, 0.035, 0.025, 0.0],
            ..particle(MOTE)
        };
        let mut nodes = Vec::new();
        let dust = one(
            &mut nodes,
            EmitterData {
                velocity: 2.2,
                velocity_variance: 1.2,
                theta_max: 70.0,
                ..emitter()
            },
            puffs,
        );
        let glow = one(
            &mut nodes,
            EmitterData {
                velocity: 1.2,
                velocity_variance: 0.6,
                theta_max: 90.0,
                ..emitter()
            },
            vec![plain(glow)],
        );
        let grains = one(
            &mut nodes,
            EmitterData {
                velocity: 2.5,
                velocity_variance: 1.5,
                theta_max: 180.0,
                ..emitter()
            },
            vec![(
                grain,
                ParticleExtras {
                    turbulence: 0.8,
                    turbulence_scale: 0.7,
                    ..ParticleExtras::default()
                },
            )],
        );
        let sparks = one(
            &mut nodes,
            EmitterData {
                velocity: 7.0,
                velocity_variance: 3.0,
                theta_max: 180.0,
                ..emitter()
            },
            vec![(
                spark,
                ParticleExtras {
                    size_by_velocity: 0.03,
                    collide: true,
                    bounce_elasticity: 0.35,
                    bounce_friction: 0.3,
                    ..ParticleExtras::default()
                },
            )],
        );
        let flash = one(
            &mut nodes,
            EmitterData {
                velocity: 0.0,
                velocity_variance: 0.0,
                ..emitter()
            },
            vec![plain(flash)],
        );
        let motes = one(
            &mut nodes,
            EmitterData {
                velocity: 0.3,
                velocity_variance: 0.2,
                theta_max: 30.0,
                ..emitter()
            },
            vec![(
                mote,
                ParticleExtras {
                    turbulence: 0.9,
                    turbulence_scale: 0.6,
                    ..ParticleExtras::default()
                },
            )],
        );
        Railgun {
            t0: 0.0,
            nodes,
            targets: vec![
                Target::stack(-3.5, 12.5),
                Target::stack(1.5, 16.0),
                Target::stack(5.5, 13.5),
            ],
            shots: Vec::new(),
            fired: 0,
            dust,
            glow,
            grains,
            sparks,
            flash,
            motes,
        }
    }

    fn fire(&mut self, t0: f32, target: usize, rng: &mut Rng) {
        let to = self.targets[target].aim;
        let shot = Shot {
            t0,
            from: MUZZLE,
            to,
            seed: 0x9e37 * (self.fired as u32 + 1),
        };
        let dir = shot.dir();
        self.targets[target].smash(dir, rng);
        // The dust comes out of the whole stack, each puff thrown away from its middle and up.
        for _ in 0..16 {
            let spot = Vec3::from_array([0, 1, 2].map(|_| 2.0 * rng.f() - 1.0)) * TARGET_HALF;
            let away = spot.normalize_or(Vec3::Z) + Vec3::Z * 0.8;
            self.nodes[self.dust].burst(to + spot, away, 20, rng);
        }
        for _ in 0..3 {
            let spot = Vec3::new(0.0, 0.0, 2.0 * rng.f() - 1.0) * TARGET_HALF * 0.6;
            self.nodes[self.glow].burst(to + spot, Vec3::Z, 12, rng);
        }
        self.nodes[self.grains].burst(to, Vec3::Z, 240, rng);
        self.nodes[self.sparks].burst(to, Vec3::Z, 450, rng);
        self.nodes[self.flash].burst(to, Vec3::Z, 1, rng);
        self.nodes[self.flash].burst(MUZZLE, Vec3::Z, 1, rng);
        self.shots.push(shot);
        self.fired += 1;
    }
}

impl Effect for Railgun {
    fn setting(&self) -> Setting {
        Setting {
            // A moonlit range: enough light to see the targets stand and fall.
            sun: ([-0.3, 0.3, 0.9], [0.1, 0.11, 0.16], [0.07, 0.08, 0.12]),
            // The gun: a pedestal and a long barrel pointing down the range.
            props: vec![
                ([0.0, 0.0, 0.45], [0.3, 0.3, 0.45], [40, 42, 48]),
                ([0.0, 0.5, 1.15], [0.07, 0.9, 0.07], [60, 64, 80]),
                ([0.0, 0.5, 1.15], [0.16, 0.5, 0.03], [50, 52, 62]),
            ],
            ..Setting::new(EYE.to_array(), [1.0, 12.0, 1.0], 58)
        }
    }

    fn step(&mut self, t: f32, ms: u32, rng: &mut Rng) {
        if t - self.t0 > SHOTS[SHOTS.len() - 1].0 + RING_S + 1.5 {
            *self = Railgun::new();
            self.t0 = t;
        }
        let t = t - self.t0;
        let dt = ms as f32 / 1000.0;
        while let Some(&(when, target)) = SHOTS.get(self.fired)
            && t >= when
        {
            self.fire(when, target, rng);
        }
        for target in &mut self.targets {
            target.tumble(dt);
        }
        self.shots.retain(|s| t - s.t0 < RING_S);
        // The rings shed motes as they break up, from the breaking ends of their pieces.
        for shot in &self.shots {
            let a = t - shot.t0;
            let dir = shot.dir();
            let u = dir.cross(Vec3::Z).normalize_or(Vec3::X);
            let v = u.cross(dir).normalize_or(Vec3::Z);
            let radius = RING_R * (1.0 + 0.4 * a / RING_S);
            for (k, center) in shot.rings().enumerate() {
                let age = Shot::ring_age(a, k);
                if age < CUTS[0].0 || rng.f() > 12.0 * dt {
                    continue;
                }
                let turn = rng.f();
                if !(0.02..0.8).contains(&shot.left(k, age, turn)) {
                    continue;
                }
                let angle = TAU * turn;
                let out = u * angle.cos() + v * angle.sin();
                self.nodes[self.motes].burst(center + out * radius, out, 1, rng);
            }
        }
    }

    fn nodes(&mut self) -> &mut [Node] {
        &mut self.nodes
    }

    fn trigger(&mut self, t: f32, rng: &mut Rng) {
        if self.targets.iter().all(|target| target.hit) {
            for target in &mut self.targets {
                *target = Target::stack(target.aim.x, target.aim.y);
            }
        }
        let standing: Vec<usize> = (0..self.targets.len())
            .filter(|&k| !self.targets[k].hit)
            .collect();
        let target = standing[(rng.f() * standing.len() as f32) as usize % standing.len()];
        // Past the scheduled shots, so the schedule does not fire again at this target.
        self.fired = self.fired.max(SHOTS.len());
        self.fire(t - self.t0, target, rng);
    }

    fn lights(&self, t: f32, _frame: u32) -> Vec<HdLight> {
        let t = t - self.t0;
        let mut out = Vec::new();
        for shot in &self.shots {
            let a = t - shot.t0;
            if a < 0.15 {
                out.push(light(MUZZLE, [1.2, 1.6, 2.4], 1.0 - a / 0.15, 6.0));
            }
            if a < 0.4 {
                out.push(light(
                    shot.to + Vec3::Z * 0.5,
                    [0.5, 0.8, 1.6],
                    3.0 * (1.0 - a / 0.4),
                    10.0,
                ));
            }
            // The dust cloud glows from within for a while.
            if a < 2.5 {
                out.push(light(
                    shot.to + Vec3::Z * 0.9,
                    [0.3, 0.5, 1.0],
                    0.5 * (1.0 - a / 2.5),
                    6.0,
                ));
            }
            // What is left of the rings glows faintly along the beam.
            let bright = Shot::bright(a);
            if bright > 0.05 {
                out.push(light(
                    (shot.from + shot.to) * 0.5,
                    [0.3, 0.5, 1.0],
                    0.25 * bright,
                    8.0,
                ));
            }
        }
        out
    }

    fn meshes(&self, _t: f32) -> Vec<DynamicObject> {
        let mut targets = Vec::new();
        for target in &self.targets {
            target.shape(&mut targets);
        }
        vec![scenery::mesh(200, "targets", targets, false, None)]
    }

    fn decals(&self, t: f32, textures: &mut TextureSet<'_>) -> Vec<(f32, Draw)> {
        let t = t - self.t0;
        let mut glow = Vec::new();
        for shot in &self.shots {
            shot.shape(t, &mut glow);
        }
        let line = textures.get(LINE);
        vec![(0.0, scenery::decal("beam", glow, line))]
    }
}
