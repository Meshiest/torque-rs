//! A charge under a stack of crates on a concrete pad, by day. It detonates: a flash that lights the whole pad for a quarter
//! of a second, a fireball of hot puffs that rolls up on its own heat and darkens into a column of dark smoke as it cools, a
//! shockwave (a sphere of screen-space distortion that swells from the blast to its full size in under half a second and
//! fades), a shock ring that races out along the ground and fades, a ring of pale dust pushed out behind it, the crates thrown as
//! chunks on ballistic arcs that smoke as they fly and bounce where they land, and a shower of smaller debris and sparks
//! that bounce too. The glowing parts (flash, fire, sparks, ring) add light; the smoke, dust and debris are lit by the sun.

use std::f32::consts::TAU;
use std::ops::Range;

use glam::Vec3;
use torque_render_scene::hd::distortion::{DistortionLook, HdDistorter, sphere};
use torque_render_scene::hd::{DynamicObject, HdLight};
use torque_render_scene::particles::{EmitterData, ParticleData, ParticleExtras, Rng};
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Draw, Vertex};

use super::{Effect, Node, Setting, emitter, group, hash, light, one, particle, plain};
use crate::scenery::{self, cuboid, ribbon};
use crate::sprites::{GLOW, LINE, MOTE, SMOKES, SPARK};

const EYE: [f32; 3] = [-6.5, -18.0, 2.6];
/// Where the charge sits.
const CHARGE: Vec3 = Vec3::new(0.0, 0.0, 0.3);
/// When the charge goes off after the stage is set, and how long the show runs before the stack is built again.
const FUSE: f32 = 1.2;
const PERIOD: f32 = 15.0;
/// The shockwave: how long it swells and fades (seconds) and the radius it reaches (units).
const WAVE_S: f32 = 0.45;
const WAVE_RADIUS: f32 = 11.0;
/// How long the shock ring shows and how far it gets (units).
const RING_S: f32 = 0.9;
const RING_REACH: f32 = 34.0;
const RING_SEGMENTS: usize = 96;
/// The scene's gravity (units a second squared; the datablocks' `gravity` of 1 is the same).
const G: f32 = 9.81;

/// A crate or barrel: where it stands, its half sides and color; once thrown, its velocity and how long it has flown.
#[derive(Clone, Copy)]
struct Chunk {
    pos: Vec3,
    half: Vec3,
    color: [u8; 3],
    vel: Vec3,
    flying: Option<f32>,
}

impl Chunk {
    fn new(pos: [f32; 3], half: [f32; 3], color: [u8; 3]) -> Chunk {
        Chunk {
            pos: Vec3::from(pos),
            half: Vec3::from(half),
            color,
            vel: Vec3::ZERO,
            flying: None,
        }
    }

    /// Thrown by the blast: away from the charge and up, the near ones hardest.
    fn throw(&mut self, rng: &mut Rng) {
        let from = self.pos - CHARGE;
        let away = Vec3::new(from.x, from.y, 0.0).normalize_or(Vec3::X);
        let near = (1.0 - from.length() / 4.0).clamp(0.3, 1.0);
        self.vel = away * (4.0 + 5.0 * rng.f()) * near
            + Vec3::Z * (4.5 + 4.5 * rng.f()) * near
            + Vec3::new(rng.f() - 0.5, rng.f() - 0.5, 0.0) * 3.0;
        self.flying = Some(0.0);
    }

    fn fly(&mut self, dt: f32) {
        let Some(age) = &mut self.flying else {
            return;
        };
        *age += dt;
        self.vel.z -= G * dt;
        self.vel *= 1.0 - 0.15 * dt;
        self.pos += self.vel * dt;
        if self.pos.z < self.half.z {
            self.pos.z = self.half.z;
            self.vel.z = -self.vel.z * 0.3;
            self.vel.x *= 0.55;
            self.vel.y *= 0.55;
        }
    }

    /// Whether it still smokes: for its first seconds in the air, while it moves.
    fn smoking(&self) -> bool {
        self.flying
            .is_some_and(|age| age < 2.6 && self.vel.length() > 1.5)
    }
}

/// The stack: four crates and a barrel either side.
fn stack() -> Vec<Chunk> {
    let pine = [122, 96, 58];
    let dark = [96, 74, 44];
    let drum = [60, 72, 64];
    vec![
        Chunk::new([-0.5, -0.2, 0.45], [0.45; 3], pine),
        Chunk::new([0.5, 0.1, 0.45], [0.45; 3], dark),
        Chunk::new([0.0, -0.05, 1.35], [0.45; 3], pine),
        Chunk::new([-0.1, 0.9, 0.4], [0.4; 3], dark),
        Chunk::new([1.7, -0.6, 0.55], [0.3, 0.3, 0.55], drum),
        Chunk::new([-1.6, 0.7, 0.55], [0.3, 0.3, 0.55], drum),
    ]
}

pub struct Explosion {
    /// When the show's clock started (it starts over every `PERIOD`).
    t0: f32,
    /// When the charge went off on that clock.
    boom: Option<f32>,
    nodes: Vec<Node>,
    chunks: Vec<Chunk>,
    trails: Range<usize>,
    flash: usize,
    fire: usize,
    soot: usize,
    core: usize,
    column: usize,
    dust: usize,
    debris: usize,
    sparks: usize,
}

impl Explosion {
    pub fn new() -> Explosion {
        let flash = ParticleData {
            lifetime_ms: 180,
            colors: [
                [1.0, 0.95, 0.85, 1.0],
                [1.0, 0.8, 0.5, 0.8],
                [1.0, 0.5, 0.2, 0.3],
                [1.0, 0.3, 0.1, 0.0],
            ],
            sizes: [3.0, 7.0, 9.0, 10.0],
            times: [0.0, 0.25, 0.6, 1.0],
            ..particle(GLOW)
        };
        // The fireball's cloth: puffs of burning fuel, orange in the sun and sooting over as they cool, thrown with the
        // smoke they become, all lifted on the heat; a few hot puffs inside them add the glow of the core.
        let puff = ParticleData {
            drag: 2.2,
            gravity: -0.35,
            lifetime_ms: 1100,
            lifetime_variance_ms: 400,
            inv_alpha: true,
            spin_min: -60.0,
            spin_max: 60.0,
            colors: [
                [1.0, 0.92, 0.6, 0.95],
                [1.0, 0.52, 0.14, 0.85],
                [0.45, 0.2, 0.08, 0.5],
                [0.2, 0.17, 0.15, 0.0],
            ],
            sizes: [0.5, 1.6, 2.3, 2.7],
            times: [0.0, 0.15, 0.5, 1.0],
            ..particle(SMOKES[1])
        };
        let hot = ParticleData {
            drag: 2.4,
            lifetime_ms: 550,
            lifetime_variance_ms: 250,
            inv_alpha: false,
            colors: [
                [1.0, 0.95, 0.75, 0.7],
                [1.0, 0.6, 0.2, 0.45],
                [0.7, 0.2, 0.03, 0.2],
                [0.2, 0.03, 0.0, 0.0],
            ],
            sizes: [0.6, 1.4, 1.8, 2.0],
            times: [0.0, 0.15, 0.5, 1.0],
            ..puff.clone()
        };
        let smoke = ParticleData {
            drag: 1.8,
            gravity: -0.22,
            lifetime_ms: 5500,
            lifetime_variance_ms: 1800,
            inv_alpha: true,
            spin_min: -25.0,
            spin_max: 25.0,
            colors: [
                [0.12, 0.1, 0.08, 0.0],
                [0.14, 0.12, 0.1, 0.65],
                [0.26, 0.25, 0.24, 0.35],
                [0.4, 0.4, 0.4, 0.0],
            ],
            sizes: [0.7, 2.6, 4.4, 5.8],
            times: [0.0, 0.08, 0.5, 1.0],
            ..particle(SMOKES[2])
        };
        let dust = ParticleData {
            drag: 2.0,
            gravity: 0.05,
            lifetime_ms: 2800,
            lifetime_variance_ms: 900,
            inv_alpha: true,
            spin_min: -30.0,
            spin_max: 30.0,
            colors: [
                [0.6, 0.55, 0.47, 0.0],
                [0.6, 0.55, 0.47, 0.5],
                [0.62, 0.58, 0.52, 0.25],
                [0.65, 0.62, 0.58, 0.0],
            ],
            sizes: [0.3, 1.5, 2.6, 3.2],
            times: [0.0, 0.12, 0.55, 1.0],
            ..particle(SMOKES[3])
        };
        let debris = ParticleData {
            drag: 0.15,
            gravity: 1.0,
            lifetime_ms: 3500,
            lifetime_variance_ms: 1200,
            inv_alpha: true,
            colors: [
                [0.14, 0.11, 0.08, 1.0],
                [0.14, 0.11, 0.08, 1.0],
                [0.14, 0.11, 0.08, 1.0],
                [0.14, 0.11, 0.08, 0.0],
            ],
            sizes: [0.14, 0.14, 0.12, 0.1],
            times: [0.0, 0.4, 0.9, 1.0],
            ..particle(MOTE)
        };
        let spark = ParticleData {
            drag: 0.5,
            gravity: 1.0,
            lifetime_ms: 1300,
            lifetime_variance_ms: 600,
            colors: [
                [1.0, 1.0, 0.92, 1.0],
                [1.0, 0.8, 0.45, 0.9],
                [1.0, 0.45, 0.12, 0.5],
                [0.5, 0.1, 0.0, 0.0],
            ],
            sizes: [0.07, 0.065, 0.05, 0.03],
            times: [0.0, 0.15, 0.6, 1.0],
            ..particle(SPARK)
        };
        let trail = ParticleData {
            drag: 1.2,
            gravity: -0.05,
            lifetime_ms: 1800,
            lifetime_variance_ms: 600,
            inv_alpha: true,
            spin_min: -20.0,
            spin_max: 20.0,
            colors: [
                [0.3, 0.28, 0.26, 0.0],
                [0.3, 0.28, 0.26, 0.4],
                [0.35, 0.34, 0.33, 0.2],
                [0.4, 0.4, 0.4, 0.0],
            ],
            sizes: [0.15, 0.5, 0.85, 1.1],
            times: [0.0, 0.15, 0.6, 1.0],
            ..particle(SMOKES[0])
        };
        let swirl = |turbulence: f32, turbulence_scale: f32| ParticleExtras {
            turbulence,
            turbulence_scale,
            ..ParticleExtras::default()
        };
        let mut nodes = Vec::new();
        let trails = group(
            &mut nodes,
            6,
            &EmitterData {
                period_ms: 12,
                velocity: 0.6,
                velocity_variance: 0.4,
                theta_max: 30.0,
                ..emitter()
            },
            &[(trail, swirl(0.6, 1.0))],
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
        let ball = EmitterData {
            velocity: 7.0,
            velocity_variance: 3.0,
            theta_max: 75.0,
            ..emitter()
        };
        let fire = one(
            &mut nodes,
            ball.clone(),
            vec![(puff.clone(), swirl(1.2, 1.2))],
        );
        let soot = one(
            &mut nodes,
            ball.clone(),
            vec![(smoke.clone(), swirl(0.9, 2.0))],
        );
        let core = one(
            &mut nodes,
            EmitterData {
                velocity: 5.0,
                velocity_variance: 2.0,
                ..ball
            },
            vec![(hot, swirl(1.2, 1.2))],
        );
        let column = one(
            &mut nodes,
            EmitterData {
                velocity: 15.0,
                velocity_variance: 4.0,
                theta_max: 12.0,
                ..emitter()
            },
            vec![
                (
                    ParticleData {
                        drag: 1.6,
                        sizes: [0.4, 1.2, 1.8, 2.0],
                        ..puff
                    },
                    swirl(1.0, 1.5),
                ),
                (ParticleData { drag: 1.4, ..smoke }, swirl(0.9, 2.0)),
            ],
        );
        let dust = one(
            &mut nodes,
            EmitterData {
                velocity: 16.0,
                velocity_variance: 4.0,
                theta_max: 8.0,
                ..emitter()
            },
            vec![(dust, swirl(0.6, 1.5))],
        );
        let bounce = |size_by_velocity: f32, bounce_elasticity: f32| ParticleExtras {
            size_by_velocity,
            collide: true,
            bounce_elasticity,
            bounce_friction: 0.4,
            ..ParticleExtras::default()
        };
        let debris = one(
            &mut nodes,
            EmitterData {
                velocity: 14.0,
                velocity_variance: 7.0,
                theta_max: 60.0,
                ..emitter()
            },
            vec![(debris, bounce(0.0, 0.35))],
        );
        let sparks = one(
            &mut nodes,
            EmitterData {
                velocity: 22.0,
                velocity_variance: 8.0,
                theta_max: 85.0,
                ..emitter()
            },
            vec![(spark, bounce(0.03, 0.4))],
        );
        Explosion {
            t0: 0.0,
            boom: None,
            nodes,
            chunks: stack(),
            trails,
            flash,
            fire,
            soot,
            core,
            column,
            dust,
            debris,
            sparks,
        }
    }

    /// The charge goes off at `t` (the show's clock).
    fn detonate(&mut self, t: f32, rng: &mut Rng) {
        self.boom = Some(t);
        let up = CHARGE + Vec3::Z * 0.4;
        self.nodes[self.flash].burst(CHARGE + Vec3::Z * 1.5, Vec3::Z, 1, rng);
        self.nodes[self.fire].burst(up, Vec3::Z, 200, rng);
        self.nodes[self.soot].burst(up, Vec3::Z, 300, rng);
        self.nodes[self.core].burst(up, Vec3::Z, 70, rng);
        self.nodes[self.column].burst(up, Vec3::Z, 90, rng);
        self.nodes[self.debris].burst(up, Vec3::Z, 260, rng);
        self.nodes[self.sparks].burst(up, Vec3::Z, 380, rng);
        // The dust: a ring of throws, each a little above the ground, so it runs out along the pad.
        let n = 440;
        for k in 0..n {
            let a = TAU * (k as f32 + 0.6 * rng.f()) / n as f32;
            let dir = Vec3::new(a.cos(), a.sin(), 0.1 + 0.08 * rng.f()).normalize();
            self.nodes[self.dust].burst(CHARGE + dir * 0.9, dir, 1, rng);
        }
        for chunk in &mut self.chunks {
            chunk.throw(rng);
        }
        for i in self.trails.clone() {
            self.nodes[i].emitter.break_path();
        }
    }

    /// The shock ring's radius and how bright it is at age `a`: fast off the mark, slowing as it spreads, gone in `RING_S`.
    fn ring(a: f32) -> (f32, f32) {
        let r = 0.8 + RING_REACH * (1.0 - (-a / 0.45).exp());
        let f = (1.0 - a / RING_S).clamp(0.0, 1.0).powf(1.6);
        (r, f)
    }
}

impl Effect for Explosion {
    fn setting(&self) -> Setting {
        Setting {
            // Midday, clear: a hard sun high to the south-west, the sky's blue in the shade.
            sun: ([0.4, -0.45, 0.8], [1.45, 1.32, 1.1], [0.28, 0.32, 0.4]),
            atmosphere: true,
            // The pad's bunker wall behind, concrete blocks about, a water tower's footing to one side.
            props: vec![
                ([0.0, 16.0, 1.2], [10.0, 0.7, 1.2], [118, 116, 110]),
                ([-9.0, 5.0, 0.6], [0.6, 0.6, 0.6], [128, 126, 120]),
                ([-7.6, 5.4, 0.6], [0.6, 0.6, 0.6], [128, 126, 120]),
                ([9.5, 7.0, 0.5], [1.2, 1.2, 0.5], [122, 120, 114]),
                ([12.0, 12.0, 2.2], [0.35, 0.35, 2.2], [96, 98, 104]),
            ],
            ..Setting::new(EYE, [0.0, 0.0, 3.0], 56)
        }
    }

    fn step(&mut self, t: f32, ms: u32, rng: &mut Rng) {
        if t - self.t0 > PERIOD {
            *self = Explosion::new();
            self.t0 = t;
        }
        let t = t - self.t0;
        let dt = ms as f32 / 1000.0;
        if self.boom.is_none() && t >= FUSE {
            self.detonate(t, rng);
        }
        let trails = self.trails.clone();
        for (k, chunk) in self.chunks.iter_mut().enumerate() {
            chunk.fly(dt);
            let Some(trail) = trails.clone().nth(k) else {
                continue;
            };
            if chunk.smoking() {
                let back = -chunk.vel.normalize_or(Vec3::Z);
                self.nodes[trail].path(chunk.pos, back, ms, rng);
            } else {
                self.nodes[trail].emitter.break_path();
            }
        }
    }

    fn nodes(&mut self) -> &mut [Node] {
        &mut self.nodes
    }

    /// Space: the charge goes off; once it has, the stack is built again and goes off at once.
    fn trigger(&mut self, t: f32, rng: &mut Rng) {
        if self.boom.is_some() {
            *self = Explosion::new();
            self.t0 = t;
        }
        self.detonate(t - self.t0, rng);
    }

    fn lights(&self, t: f32, frame: u32) -> Vec<HdLight> {
        let Some(boom) = self.boom else {
            return Vec::new();
        };
        let a = t - self.t0 - boom;
        let mut out = Vec::new();
        if a < 0.25 {
            out.push(light(
                CHARGE + Vec3::Z * 1.5,
                [1.0, 0.92, 0.78],
                90.0 * (1.0 - a / 0.25),
                45.0,
            ));
        }
        if a < 2.2 {
            // The fireball's glow, rising with it and guttering out.
            let k = (1.0 - a / 2.2).powi(2) * (0.7 + 0.6 * hash(frame));
            out.push(light(
                CHARGE + Vec3::Z * (1.2 + 2.5 * a),
                [1.0, 0.5, 0.15],
                14.0 * k,
                28.0,
            ));
        }
        out
    }

    fn meshes(&self, _t: f32) -> Vec<DynamicObject> {
        let mut shape: Vec<Vertex> = Vec::new();
        for chunk in &self.chunks {
            cuboid(chunk.pos, chunk.half, chunk.color, &mut shape);
        }
        vec![scenery::mesh(200, "stack", shape, false, None)]
    }

    /// The shock ring: a thin bright band racing out along the pad.
    /// The shockwave: a sphere of bent air that swells fast from the charge (most of its reach in the first tenth of a
    /// second) and fades as it goes.
    fn distorters(&self, t: f32) -> Vec<HdDistorter> {
        let Some(boom) = self.boom else {
            return Vec::new();
        };
        let a = (t - self.t0 - boom) / WAVE_S;
        if !(0.0..1.0).contains(&a) {
            return Vec::new();
        }
        let radius = 0.6 + WAVE_RADIUS * a.powf(0.55);
        let fade = ((1.0 - a).powf(1.4) * 255.0) as u8;
        vec![HdDistorter::new(
            sphere(CHARGE.to_array(), radius, 32, 64, fade),
            DistortionLook {
                strength: 1.2,
                edge: 3.0,
                chroma: 0.12,
                ..DistortionLook::default()
            },
        )]
    }

    fn decals(&self, t: f32, textures: &mut TextureSet<'_>) -> Vec<(f32, Draw)> {
        let Some(boom) = self.boom else {
            return Vec::new();
        };
        let a = t - self.t0 - boom;
        if !(0.0..RING_S).contains(&a) {
            return Vec::new();
        }
        let (r, f) = Explosion::ring(a);
        let c = [1.0, 0.88, 0.66].map(|s: f32| (s * f * 255.0) as u8);
        let color = [c[0], c[1], c[2], 255];
        let lift = CHARGE.with_z(0.1);
        let points: Vec<Vec3> = (0..=RING_SEGMENTS)
            .map(|k| {
                let angle = TAU * (k % RING_SEGMENTS) as f32 / RING_SEGMENTS as f32;
                lift + Vec3::new(angle.cos(), angle.sin(), 0.0) * r
            })
            .collect();
        let mut glow = Vec::new();
        ribbon(
            &points,
            &vec![color; points.len()],
            0.8 + 2.2 * a,
            0.0,
            Vec3::Z,
            &mut glow,
        );
        vec![(0.0, scenery::decal("shock", glow, textures.get(LINE)))]
    }
}
