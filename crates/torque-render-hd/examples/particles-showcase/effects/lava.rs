//! Lava and embers at night: a pool of lava crusted over, its crust plates glowing through their cracks, and cracks running
//! out from it across the basalt. The pool and the cracks are glow decals (added over the floor into the bloom's range),
//! breathing slowly as a lake does when fresh lava wells up under the crust. Over the pool the hot air carries embers up:
//! crisp points born on the bright cracks that rise slowly, drift on the air and cool from white through orange to a dull
//! red before they go out. Now and then a gas bubble breaks the crust: a flash, a spray of spatter thrown up and out that
//! lands on the rock, bounces, slides and lies there cooling (the script flies the spatter itself: [`Grit`]), a swell of
//! light. Space pops one. The lava lights the boulders and the ground about it, flickering.

use std::f32::consts::TAU;

use glam::Vec3;
use torque_render_scene::Draw;
use torque_render_scene::hd::HdLight;
use torque_render_scene::particles::{EmitterData, ParticleData, ParticleExtras, Rng};
use torque_render_scene::textures::TextureSet;

use super::{
    Effect, Floor, Grit, GritKind, Lcg, Node, Setting, emitter, hash, light, one, particle, plain,
    smooth, within_cone,
};
use crate::scenery::{self, vertex};
use crate::sprites::{DISC, GLOW, LAVA, LINE, SPARK};

/// The pool's center and the radius its rim wanders about.
const POOL: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const RADIUS: f32 = 2.6;
/// The pool's rim: `RIM` points about it at radius `RADIUS` times their wobble.
const RIM: usize = 28;
/// How the pool's light breathes (seconds a cycle).
const BREATH: f32 = 7.0;
/// Seconds between bubbles.
const BUBBLE_GAP: (f32, f32) = (1.6, 4.5);

/// A bubble that broke: where and when.
struct Pop {
    at: Vec3,
    t0: f32,
}

/// The camera (the spatter's streaks face it).
const EYE: Vec3 = Vec3::new(-3.2, -7.4, 2.3);
/// The spatter's throw: its speed, the spread of its speed and its cone (degrees) about the bubble's lean.
const SPRAY: (f32, f32, f32) = (2.6, 1.4, 32.0);

pub struct Lava {
    nodes: Vec<Node>,
    embers: usize,
    spray: Grit,
    flash: usize,
    rim: Vec<f32>,
    cracks: Vec<Vec<Vec3>>,
    owed: f32,
    next_pop: f32,
    pops: Vec<Pop>,
}

impl Lava {
    pub fn new() -> Lava {
        // An ember: a crisp point lifted by the hot air, cooling as it rises and drifts.
        let ember = ParticleData {
            drag: 0.9,
            gravity: -0.018,
            wind: 0.6,
            lifetime_ms: 4200,
            lifetime_variance_ms: 1800,
            colors: [
                [1.0, 0.85, 0.55, 1.0],
                [1.0, 0.55, 0.15, 0.95],
                [0.85, 0.22, 0.03, 0.6],
                [0.35, 0.04, 0.0, 0.0],
            ],
            sizes: [0.032, 0.03, 0.026, 0.018],
            times: [0.0, 0.25, 0.65, 1.0],
            ..particle(SPARK)
        };
        // Spatter from a bubble: thrown hot, falling back, bouncing on the rock and lying where it stops, cooling from
        // white through orange to a dull red that goes out.
        let spatter = Grit::new(
            GritKind {
                gravity: 0.6 * 9.81,
                drag: 0.35,
                elasticity: 0.3,
                friction: 0.45,
                tail: 0.03,
                size: 0.042,
                life: (2.2, 4.0),
                colors: [
                    [1.0, 0.95, 0.8, 1.0],
                    [1.0, 0.7, 0.3, 0.95],
                    [0.9, 0.3, 0.05, 0.55],
                    [0.3, 0.03, 0.0, 0.0],
                ],
                times: [0.0, 0.15, 0.55, 1.0],
            },
            0.0,
        );
        // The bubble's flash: a soft glow over the crust that swells and goes.
        let flash = ParticleData {
            lifetime_ms: 450,
            colors: [
                [1.0, 0.75, 0.4, 0.0],
                [1.0, 0.65, 0.3, 0.55],
                [1.0, 0.4, 0.1, 0.25],
                [0.8, 0.2, 0.02, 0.0],
            ],
            sizes: [0.3, 0.8, 1.1, 1.2],
            times: [0.0, 0.15, 0.6, 1.0],
            spin_min: -60.0,
            spin_max: 60.0,
            ..particle(GLOW)
        };
        let mut nodes = Vec::new();
        let embers = one(
            &mut nodes,
            EmitterData {
                velocity: 0.25,
                velocity_variance: 0.15,
                theta_max: 12.0,
                ..emitter()
            },
            vec![(
                ember,
                ParticleExtras {
                    turbulence: 0.45,
                    turbulence_scale: 0.8,
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
        let mut g = Lcg(71);
        let rim = (0..RIM).map(|_| g.range(0.8, 1.15)).collect();
        // Cracks: jagged lines out from the rim, each a few bends long, one or two forking.
        let mut cracks = Vec::new();
        for k in 0..5 {
            let a = TAU * (k as f32 + g.range(0.1, 0.9)) / 5.0;
            let mut heading = a;
            let mut p = POOL + Vec3::new(a.cos(), a.sin(), 0.0) * (RADIUS * 0.9);
            let mut line = vec![p];
            for _ in 0..(3 + (g.unit() * 4.0) as usize) {
                heading += g.range(-0.9, 0.9);
                p += Vec3::new(heading.cos(), heading.sin(), 0.0) * g.range(0.5, 1.4);
                line.push(p);
            }
            cracks.push(line);
        }
        Lava {
            nodes,
            embers,
            spray: spatter,
            flash,
            rim,
            cracks,
            owed: 0.0,
            next_pop: 1.0,
            pops: Vec::new(),
        }
    }

    /// The pool's rim at angle `a` (radians), on the floor.
    fn rim_at(&self, a: f32) -> Vec3 {
        let f = a.rem_euclid(TAU) / TAU * RIM as f32;
        let i = f as usize % RIM;
        let t = f - f.floor();
        let r = self.rim[i] * (1.0 - t) + self.rim[(i + 1) % RIM] * t;
        POOL + Vec3::new(a.cos(), a.sin(), 0.0) * (RADIUS * r)
    }

    /// A point on the pool, `r` (0..1) of the way out at angle `a`.
    fn pool_at(&self, a: f32, r: f32) -> Vec3 {
        POOL + (self.rim_at(a) - POOL) * r
    }

    /// How bright the pool is at `t`: a slow breath, a swell for a moment after each bubble.
    fn glow(&self, t: f32) -> f32 {
        let breath = 0.78 + 0.22 * (TAU * t / BREATH).sin();
        let swell: f32 = self
            .pops
            .iter()
            .map(|p| {
                let u = (t - p.t0) / 1.2;
                0.3 * smooth(u * 4.0) * (1.0 - smooth(u))
            })
            .sum();
        breath + swell
    }

    fn pop(&mut self, t: f32, rng: &mut Rng) {
        let a = rng.f() * TAU;
        let at = self.pool_at(a, 0.75 * rng.f().sqrt()) + Vec3::Z * 0.03;
        self.nodes[self.flash].burst(at, Vec3::Z, 1, rng);
        let lean = Vec3::new(a.cos() * 0.25, a.sin() * 0.25, 1.0).normalize();
        let (speed, spread, cone) = SPRAY;
        for _ in 0..22 + (rng.f() * 16.0) as u32 {
            let v = within_cone(lean, cone, rng) * (speed + spread * (rng.f() * 2.0 - 1.0));
            self.spray.throw(at, v, rng);
        }
        self.nodes[self.embers].burst(at, Vec3::Z, 12, rng);
        self.pops.push(Pop { at, t0: t });
    }
}

impl Effect for Lava {
    fn setting(&self) -> Setting {
        // Basalt boulders about the pool, a ridge behind it.
        let mut g = Lcg(73);
        let mut props = Vec::new();
        for k in 0..11 {
            let a = TAU * (k as f32 + 0.5) / 11.0 + g.range(-0.2, 0.2);
            let r = RADIUS * g.range(1.35, 1.9);
            let s = g.range(0.3, 0.7);
            let shade = (g.unit() * 8.0) as u8;
            props.push((
                [POOL.x + a.cos() * r, POOL.y + a.sin() * r, s * 0.35],
                [s, s * g.range(0.6, 1.0), s * 0.45],
                [26 + shade, 24 + shade, 26 + shade],
            ));
        }
        props.push(([-2.0, 9.0, 1.2], [7.0, 1.4, 1.2], [20, 18, 20]));
        props.push(([6.5, 7.0, 0.8], [2.5, 1.6, 0.8], [22, 20, 22]));
        Setting {
            sun: (
                [-0.3, 0.4, 0.86],
                [0.02, 0.024, 0.04],
                [0.012, 0.014, 0.024],
            ),
            clear: [0.005, 0.005, 0.012],
            props,
            floor: Floor::Basalt,
            // A light air across the field: the embers lean with it.
            wind: [-0.12, 0.05, 0.0],
            ..Setting::new(EYE.to_array(), [0.3, 1.2, 0.5], 56)
        }
    }

    fn step(&mut self, t: f32, ms: u32, rng: &mut Rng) {
        let dt = ms as f32 / 1000.0;
        // Embers off the hot cracks: a few a frame, more when the pool breathes in.
        self.owed += dt * 85.0 * self.glow(t);
        while self.owed >= 1.0 {
            self.owed -= 1.0;
            let a = rng.f() * TAU;
            let at = self.pool_at(a, 0.95 * rng.f().sqrt()) + Vec3::Z * 0.04;
            self.nodes[self.embers].burst(at, Vec3::Z, 1, rng);
        }
        if t >= self.next_pop {
            self.next_pop = t + BUBBLE_GAP.0 + (BUBBLE_GAP.1 - BUBBLE_GAP.0) * rng.f();
            self.pop(t, rng);
        }
        self.spray.step(dt);
        self.pops.retain(|p| t - p.t0 < 1.5);
    }

    fn nodes(&mut self) -> &mut [Node] {
        &mut self.nodes
    }

    fn lights(&self, t: f32, frame: u32) -> Vec<HdLight> {
        let g = self.glow(t);
        let flicker = 0.9 + 0.1 * hash(frame / 3);
        let color = [2.4, 0.9, 0.18];
        let mut out = vec![
            light(
                POOL + Vec3::new(0.0, 0.0, 0.9),
                color,
                1.3 * g * flicker,
                11.0,
            ),
            light(POOL + Vec3::new(1.4, -0.9, 0.5), color, 0.5 * g, 6.0),
            light(POOL + Vec3::new(-1.3, 1.1, 0.5), color, 0.5 * g, 6.0),
        ];
        for p in &self.pops {
            let u = (t - p.t0) / 1.0;
            let k = smooth(u * 6.0) * (1.0 - smooth(u));
            out.push(light(p.at + Vec3::Z * 0.6, [2.8, 1.4, 0.5], 1.2 * k, 7.0));
        }
        out
    }

    fn decals(&self, t: f32, textures: &mut TextureSet<'_>) -> Vec<(f32, Draw)> {
        let g = self.glow(t);
        // The pool: a fan of triangles from the center to the rim, the lava texture over it, brighter at the center.
        let (lava, line) = (textures.get(LAVA), textures.get(LINE));
        let shade = |k: f32| -> [u8; 4] {
            [
                (255.0 * k).min(255.0) as u8,
                (255.0 * k * 0.62).min(255.0) as u8,
                (255.0 * k * 0.3).min(255.0) as u8,
                255,
            ]
        };
        let uv = |p: Vec3| {
            let d = (p - POOL).truncate() / (RADIUS * 1.2);
            [0.5 + 0.5 * d.x, 0.5 - 0.5 * d.y]
        };
        let mut pool = Vec::new();
        let center = vertex(POOL + Vec3::Z * 0.01, Vec3::Z, shade(g), uv(POOL));
        for i in 0..RIM {
            let a0 = TAU * i as f32 / RIM as f32;
            let a1 = TAU * (i + 1) as f32 / RIM as f32;
            let (p0, p1) = (
                self.rim_at(a0) + Vec3::Z * 0.01,
                self.rim_at(a1) + Vec3::Z * 0.01,
            );
            // The rim cools: dim there.
            let edge = shade(g * 0.55);
            pool.extend([
                center,
                vertex(p0, Vec3::Z, edge, uv(p0)),
                vertex(p1, Vec3::Z, edge, uv(p1)),
            ]);
        }
        // The cracks: thin glowing lines, fading out along their length.
        let mut cracks = Vec::new();
        for crack in &self.cracks {
            let n = crack.len();
            let colors: Vec<[u8; 4]> = (0..n)
                .map(|i| shade(g * 0.9 * (1.0 - i as f32 / n as f32)))
                .collect();
            let lifted: Vec<Vec3> = crack.iter().map(|p| *p + Vec3::Z * 0.012).collect();
            scenery::ribbon(&lifted, &colors, 0.1, 0.04, Vec3::Z, &mut cracks);
        }
        // Each pop's hot spot on the crust: a bright patch that cools over a second.
        let mut spots = Vec::new();
        for p in &self.pops {
            let u = (t - p.t0) / 1.5;
            let k = (1.0 - smooth(u)) * 0.9;
            let c = shade(k);
            let r = 0.35 + 0.25 * smooth(u);
            let corner = |x: f32, y: f32| {
                vertex(
                    p.at + Vec3::new(x * r, y * r, 0.0),
                    Vec3::Z,
                    c,
                    [0.5 + 0.5 * x, 0.5 - 0.5 * y],
                )
            };
            let (a, b, cc, d) = (
                corner(-1.0, -1.0),
                corner(1.0, -1.0),
                corner(1.0, 1.0),
                corner(-1.0, 1.0),
            );
            spots.extend([a, b, cc, a, cc, d]);
        }
        let disc = textures.get(DISC);
        // The spatter in flight and lying on the rock.
        let mut spatter = Vec::with_capacity(self.spray.len() * 6);
        self.spray.shape(EYE, &mut spatter);
        vec![
            (0.0, scenery::decal("lava", pool, lava)),
            (0.0, scenery::decal("cracks", cracks, line)),
            (0.0, scenery::decal("spots", spots, disc)),
            (0.0, scenery::decal("spatter", spatter, line)),
        ]
    }

    fn trigger(&mut self, t: f32, rng: &mut Rng) {
        self.pop(t, rng);
        self.next_pop = t + BUBBLE_GAP.0;
    }
}
