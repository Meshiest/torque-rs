//! A fountain on a sunny plaza: a center jet straight up from a pedestal and a ring of eight jets arcing out from its foot,
//! every drop a crisp lit bead thrown with gravity, falling into the basin and skipping once off its water. Each drop's
//! fall is known from its throw, so where the streams land a splash of droplets rises and a soft mist drifts up, lit by the
//! sun from behind the camera. Space makes the center jet surge for a moment.

use std::collections::VecDeque;
use std::f32::consts::TAU;

use glam::Vec3;
use torque_render_scene::Vertex;
use torque_render_scene::hd::{DynamicObject, HdLight};
use torque_render_scene::particles::{EmitterData, ParticleData, ParticleExtras, Rng};

use super::{Effect, Node, Setting, emitter, one, particle, plain};
use crate::scenery::{self, cuboid, vertex};
use crate::sprites::{DROP, MIST, SPARK};

const EYE: Vec3 = Vec3::new(-1.8, -11.5, 2.3);
const CENTER: Vec3 = Vec3::new(0.0, 0.0, 0.0);
/// Toward the sun: straight behind the camera, 40 degrees up, so the spray and the mist are lit toward the eye.
const SUN: [f32; 3] = [-0.119, -0.757, 0.643];
/// The pedestal's cap: its half width and its top.
const CAP: (f32, f32) = (0.75, WATER + 0.3);
/// The basin's rim (inner and outer radius, height) and its water's level.
const RIM: (f32, f32, f32) = (2.7, 3.1, 0.55);
const WATER: f32 = 0.42;
/// The pedestal's half width and the nozzles' height.
const PEDESTAL: f32 = 0.28;
const NOZZLE: f32 = 1.55;
/// The ring jets: how many, how far out from the pedestal they start, their climb from the horizontal (radians) and speed.
const JETS: usize = 8;
const JET_OUT: f32 = 0.42;
const JET_CLIMB: f32 = 0.96;
const JET_SPEED: f32 = 3.9;
const CENTER_SPEED: f32 = 7.6;
const G: f32 = 9.8;

struct Landing {
    t: f32,
    at: Vec3,
    /// Whether this splash also lets mist go.
    mist: bool,
}

pub struct Fountain {
    nodes: Vec<Node>,
    drops: usize,
    glints: usize,
    splash: usize,
    mist: usize,
    landings: VecDeque<Landing>,
    /// Until when the center jet surges.
    surge_until: f32,
    /// Drops owed by the rates and not yet thrown.
    debt: f32,
}

impl Fountain {
    pub fn new() -> Fountain {
        // A drop: a bright lit bead, a little stretched along its flight, that skips once off the basin's water.
        let drop = ParticleData {
            gravity: 1.0,
            lifetime_ms: 1500,
            lifetime_variance_ms: 500,
            inv_alpha: true,
            colors: [
                [0.86, 0.93, 1.0, 0.9],
                [0.86, 0.93, 1.0, 0.9],
                [0.86, 0.93, 1.0, 0.85],
                [0.86, 0.93, 1.0, 0.0],
            ],
            sizes: [0.042; 4],
            times: [0.0, 0.5, 0.9, 1.0],
            ..particle(DROP)
        };
        // A glint: the sun caught in a drop, a tiny point of light along for part of the flight.
        let glint = ParticleData {
            gravity: 1.0,
            lifetime_ms: 500,
            lifetime_variance_ms: 300,
            colors: [
                [1.0, 1.0, 0.95, 0.0],
                [1.0, 1.0, 0.95, 0.7],
                [1.0, 1.0, 0.95, 0.5],
                [1.0, 1.0, 0.95, 0.0],
            ],
            sizes: [0.03; 4],
            times: [0.0, 0.2, 0.7, 1.0],
            ..particle(SPARK)
        };
        // The splash's droplets: small, quick, up and out of the water where a stream lands.
        let splash = ParticleData {
            gravity: 1.0,
            lifetime_ms: 380,
            lifetime_variance_ms: 160,
            inv_alpha: true,
            colors: [
                [0.9, 0.95, 1.0, 0.85],
                [0.9, 0.95, 1.0, 0.8],
                [0.9, 0.95, 1.0, 0.6],
                [0.9, 0.95, 1.0, 0.0],
            ],
            sizes: [0.028, 0.026, 0.022, 0.015],
            ..particle(DROP)
        };
        // The mist: soft sheets that rise slowly off the impact, thin, lit by the sun, blown a little on the breeze.
        let mist = ParticleData {
            drag: 1.3,
            gravity: -0.035,
            wind: 0.5,
            lifetime_ms: 2300,
            lifetime_variance_ms: 900,
            inv_alpha: true,
            spin_min: -15.0,
            spin_max: 15.0,
            colors: [
                [0.95, 0.97, 1.0, 0.0],
                [0.95, 0.97, 1.0, 0.022],
                [0.95, 0.97, 1.0, 0.014],
                [0.95, 0.97, 1.0, 0.0],
            ],
            sizes: [0.12, 0.35, 0.6, 0.9],
            times: [0.0, 0.2, 0.6, 1.0],
            ..particle(MIST)
        };
        let mut nodes = Vec::new();
        // Each drop's throw is the script's own (the emitter adds no spread), so its landing is known.
        let thrown = |speed: f32| EmitterData {
            velocity: speed,
            velocity_variance: 0.0,
            theta_max: 0.0,
            ..emitter()
        };
        let skip = ParticleExtras {
            size_by_velocity: 0.012,
            collide: true,
            bounce_elasticity: 0.25,
            bounce_friction: 0.5,
            ..ParticleExtras::default()
        };
        let drops = one(&mut nodes, thrown(1.0), vec![(drop, skip)]);
        let glints = one(&mut nodes, thrown(1.0), vec![plain(glint)]);
        let splash = one(
            &mut nodes,
            EmitterData {
                velocity: 1.6,
                velocity_variance: 0.8,
                theta_max: 40.0,
                ..emitter()
            },
            vec![(
                splash,
                ParticleExtras {
                    size_by_velocity: 0.01,
                    collide: true,
                    bounce_elasticity: 0.2,
                    bounce_friction: 0.5,
                    ..ParticleExtras::default()
                },
            )],
        );
        let mist = one(
            &mut nodes,
            EmitterData {
                velocity: 0.5,
                velocity_variance: 0.25,
                theta_max: 35.0,
                ..emitter()
            },
            vec![(
                mist,
                ParticleExtras {
                    turbulence: 0.5,
                    turbulence_scale: 0.8,
                    ..ParticleExtras::default()
                },
            )],
        );
        Fountain {
            nodes,
            drops,
            glints,
            splash,
            mist,
            landings: VecDeque::new(),
            surge_until: -1.0,
            debt: 0.0,
        }
    }

    /// The basin: its rim as a ring wall, its water as a disc, the pedestal in the middle.
    fn scenery() -> Vec<DynamicObject> {
        const SEGS: usize = 40;
        let (inner, outer, height) = RIM;
        let stone = [150, 146, 138, 255];
        let mut out = Vec::new();
        let ring = |r: f32, z: f32, k: usize| {
            let a = TAU * k as f32 / SEGS as f32;
            CENTER + Vec3::new(a.cos() * r, a.sin() * r, z)
        };
        let quad =
            |a: Vec3, b: Vec3, c: Vec3, d: Vec3, n: Vec3, color: [u8; 4], out: &mut Vec<Vertex>| {
                out.extend([
                    vertex(a, n, color, [0.0; 2]),
                    vertex(b, n, color, [0.0; 2]),
                    vertex(c, n, color, [0.0; 2]),
                    vertex(a, n, color, [0.0; 2]),
                    vertex(c, n, color, [0.0; 2]),
                    vertex(d, n, color, [0.0; 2]),
                ]);
            };
        for k in 0..SEGS {
            let a = TAU * (k as f32 + 0.5) / SEGS as f32;
            let n_out = Vec3::new(a.cos(), a.sin(), 0.0);
            quad(
                ring(outer, 0.0, k),
                ring(outer, 0.0, k + 1),
                ring(outer, height, k + 1),
                ring(outer, height, k),
                n_out,
                stone,
                &mut out,
            );
            quad(
                ring(inner, height, k),
                ring(inner, height, k + 1),
                ring(inner, WATER - 0.05, k + 1),
                ring(inner, WATER - 0.05, k),
                -n_out,
                stone,
                &mut out,
            );
            quad(
                ring(inner, height, k),
                ring(outer, height, k),
                ring(outer, height, k + 1),
                ring(inner, height, k + 1),
                Vec3::Z,
                stone,
                &mut out,
            );
        }
        let rim = scenery::mesh(310, "basin rim", out, false, None);
        // The water: a dark blue-green disc, lit.
        let mut water = Vec::new();
        let center = vertex(
            CENTER + Vec3::Z * WATER,
            Vec3::Z,
            [26, 58, 72, 255],
            [0.0; 2],
        );
        for k in 0..SEGS {
            water.extend([
                center,
                vertex(ring(inner, WATER, k), Vec3::Z, [30, 66, 80, 255], [0.0; 2]),
                vertex(
                    ring(inner, WATER, k + 1),
                    Vec3::Z,
                    [30, 66, 80, 255],
                    [0.0; 2],
                ),
            ]);
        }
        let water = scenery::mesh(311, "basin water", water, false, None);
        let mut pedestal = Vec::new();
        cuboid(
            CENTER + Vec3::Z * (WATER + 0.15),
            Vec3::new(0.75, 0.75, 0.15),
            [140, 136, 128],
            &mut pedestal,
        );
        cuboid(
            CENTER + Vec3::Z * (NOZZLE * 0.5 + 0.2),
            Vec3::new(PEDESTAL, PEDESTAL, NOZZLE * 0.5 - 0.2),
            [150, 146, 138],
            &mut pedestal,
        );
        cuboid(
            CENTER + Vec3::Z * (NOZZLE - 0.06),
            Vec3::new(PEDESTAL + 0.12, PEDESTAL + 0.12, 0.06),
            [140, 136, 128],
            &mut pedestal,
        );
        vec![
            rim,
            water,
            scenery::mesh(312, "pedestal", pedestal, false, None),
        ]
    }

    /// A drop thrown from `at` along `dir` (unit) at `speed`, and its landing on the water put down: `every` is (how many
    /// drops a splash, how many splashes a puff of mist).
    fn throw(&mut self, t: f32, at: Vec3, dir: Vec3, speed: f32, every: (u32, u32), rng: &mut Rng) {
        self.nodes[self.drops].emitter.data.velocity = speed;
        self.nodes[self.drops].burst(at, dir, 1, rng);
        if rng.f() < 0.08 {
            self.nodes[self.glints].emitter.data.velocity = speed;
            self.nodes[self.glints].burst(at, dir, 1, rng);
        }
        let (splash_every, mist_every) = every;
        if rng.f() * splash_every as f32 >= 1.0 {
            return;
        }
        let v = dir * speed;
        // Where the drop comes down: on the pedestal's cap when it falls within it, else on the water.
        let fall_to = |level: f32| {
            let fall = (v.z + (v.z * v.z + 2.0 * G * (at.z - level)).max(0.0).sqrt()) / G;
            (fall, at + v * fall - Vec3::Z * (0.5 * G * fall * fall))
        };
        let (fall, landing) = fall_to(CAP.1);
        let on_cap = landing.x.abs() < CAP.0 && landing.y.abs() < CAP.0;
        let (fall, landing) = if on_cap {
            (fall, landing)
        } else {
            fall_to(WATER)
        };
        if landing.truncate().length() > RIM.0 - 0.1 {
            return;
        }
        self.landings.push_back(Landing {
            t: t + fall,
            at: Vec3::new(landing.x, landing.y, if on_cap { CAP.1 } else { WATER }),
            mist: rng.f() * (mist_every as f32) < 1.0,
        });
    }

    /// A unit direction within `spread` radians of `dir`.
    fn jitter(dir: Vec3, spread: f32, rng: &mut Rng) -> Vec3 {
        let side = dir
            .cross(if dir.z.abs() < 0.9 { Vec3::Z } else { Vec3::X })
            .normalize_or(Vec3::X);
        let up = side.cross(dir);
        let a = rng.f() * TAU;
        let r = spread * rng.f().sqrt();
        (dir + (side * a.cos() + up * a.sin()) * r.tan()).normalize_or(dir)
    }
}

impl Effect for Fountain {
    fn setting(&self) -> Setting {
        Setting {
            sun: (SUN, [1.7, 1.5, 1.25], [0.3, 0.36, 0.46]),
            clear: [0.5, 0.64, 0.85],
            props: vec![
                // Benches about the plaza and a low wall at the back.
                ([-6.5, 2.0, 0.42], [0.4, 1.2, 0.05], [96, 70, 48]),
                ([-6.5, 2.0, 0.2], [0.3, 1.0, 0.2], [70, 68, 66]),
                ([6.5, 1.0, 0.42], [0.4, 1.2, 0.05], [96, 70, 48]),
                ([6.5, 1.0, 0.2], [0.3, 1.0, 0.2], [70, 68, 66]),
                ([0.0, 14.0, 0.5], [12.0, 0.3, 0.5], [128, 122, 112]),
            ],
            scenery: Fountain::scenery(),
            atmosphere: true,
            wind: [-0.25, -0.05, 0.0],
            ..Setting::new(EYE.to_array(), [0.0, 0.0, 1.7], 50)
        }
    }

    fn step(&mut self, t: f32, ms: u32, rng: &mut Rng) {
        let dt = ms as f32 / 1000.0;
        let surge = t < self.surge_until;
        // Drops a second from the center jet and from each ring jet.
        let center_rate = if surge { 2600.0 } else { 1400.0 };
        let jet_rate = 520.0;
        self.debt += (center_rate + jet_rate * JETS as f32) * dt;
        let share = center_rate / (center_rate + jet_rate * JETS as f32);
        let nozzle = CENTER + Vec3::Z * NOZZLE;
        while self.debt >= 1.0 {
            self.debt -= 1.0;
            if rng.f() < share {
                let speed = if surge { 9.4 } else { CENTER_SPEED } * (0.97 + 0.06 * rng.f());
                let dir = Fountain::jitter(Vec3::Z, 0.03, rng);
                self.throw(t, nozzle, dir, speed, (3, 5), rng);
            } else {
                let k = (rng.f() * JETS as f32) as usize % JETS;
                let a = TAU * k as f32 / JETS as f32;
                let out = Vec3::new(a.cos(), a.sin(), 0.0);
                let at = nozzle + out * JET_OUT - Vec3::Z * 0.1;
                let dir = out * JET_CLIMB.cos() + Vec3::Z * JET_CLIMB.sin();
                let dir = Fountain::jitter(dir, 0.015, rng);
                let speed = JET_SPEED * (0.98 + 0.04 * rng.f());
                self.throw(t, at, dir, speed, (5, 4), rng);
            }
        }
        while let Some(front) = self.landings.front()
            && front.t <= t
        {
            let (at, mist) = (front.at, front.mist);
            self.landings.pop_front();
            self.nodes[self.splash].burst(at + Vec3::Z * 0.02, Vec3::Z, 3, rng);
            if mist {
                self.nodes[self.mist].burst(at + Vec3::Z * 0.1, Vec3::Z, 1, rng);
            }
        }
    }

    fn nodes(&mut self) -> &mut [Node] {
        &mut self.nodes
    }

    fn trigger(&mut self, t: f32, _rng: &mut Rng) {
        self.surge_until = t + 2.0;
    }

    fn lights(&self, _t: f32, _frame: u32) -> Vec<HdLight> {
        Vec::new()
    }
}
