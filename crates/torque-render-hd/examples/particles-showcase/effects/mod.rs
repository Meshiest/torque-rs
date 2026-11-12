//! The effects: their datablocks (`ParticleData` and `ParticleEmitterData` as the client holds them, with the engine's
//! optional fields) and the script that moves their emitters each step, as a game's script moves a `ParticleEmitterNode`.
//! The sparks and the portal are here; the smoke, the fireflies, the magic circle (with its glowing figure), the railgun, the
//! fireworks, the dust in a sunbeam, the lava and the petals each have a file.

mod circle;
mod explosion;
mod fireflies;
mod fireworks;
mod fog;
mod fountain;
mod galaxy;
mod glyphs;
mod grit;
mod lava;
mod petals;
mod railgun;
mod smoke;
mod storm;

use std::f32::consts::TAU;
use std::ops::Range;

use glam::{Vec2, Vec3};
use torque_render_scene::Draw;
use torque_render_scene::hd::distortion::HdDistorter;
use torque_render_scene::hd::{DynamicObject, HdLight, HdMode, HdSpot, Quality};
use torque_render_scene::particles::{
    Emitter, EmitterData, EmitterExtras, ParticleData, ParticleExtras, Rng,
};
use torque_render_scene::source::Eye;
use torque_render_scene::textures::TextureSet;

pub use self::circle::Circle;
pub use self::explosion::Explosion;
pub use self::fireflies::Fireflies;
pub use self::fireworks::Fireworks;
pub use self::fog::Fog;
pub use self::fountain::Fountain;
pub use self::galaxy::Galaxy;
pub use self::glyphs::{Glyphs, glyph_sprites};
pub use self::grit::{Grit, GritKind};
pub use self::lava::Lava;
pub use self::petals::Petals;
pub use self::railgun::Railgun;
pub use self::smoke::Smoke;
pub use self::storm::Storm;
use crate::sprites::{GLOW, MOTE, SMOKE, SPARK};

/// TGE's `ParticleData` defaults with `textureName` set: white, a size of 1 for a second, no forces.
fn particle(texture: &str) -> ParticleData {
    ParticleData {
        drag: 0.0,
        wind: 0.0,
        gravity: 0.0,
        inherited_vel: 0.0,
        constant_accel: 0.0,
        lifetime_ms: 1000,
        lifetime_variance_ms: 0,
        spin_speed: 0.0,
        spin_min: 0.0,
        spin_max: 0.0,
        inv_alpha: false,
        colors: [[1.0; 4]; 4],
        sizes: [1.0; 4],
        times: [0.0, 0.33, 0.66, 1.0],
        textures: vec![texture.to_string()],
        animate: false,
        fps: 0,
        extras: ParticleExtras::default(),
    }
}

/// TGE's `ParticleEmitterData` defaults, emitting along its axis.
fn emitter() -> EmitterData {
    EmitterData {
        period_ms: 100,
        period_variance_ms: 0,
        velocity: 2.0,
        velocity_variance: 1.0,
        offset: 0.0,
        theta_min: 0.0,
        theta_max: 90.0,
        phi_ref_vel: 0.0,
        phi_variance: 360.0,
        override_advance: false,
        lifetime_ms: 0,
        use_emitter_sizes: false,
        use_emitter_colors: false,
        particles: vec![1],
        extras: EmitterExtras::default(),
    }
}

/// One emitter and its count of particles it holds (the ones made since the last hand-over), and its bursts' spread
/// (its `thetaMax`, degrees).
pub struct Node {
    pub emitter: Emitter,
    pub live: usize,
    spread: f32,
}

impl Node {
    fn new(data: EmitterData, particles: Vec<(ParticleData, ParticleExtras)>) -> Node {
        let spread = data.theta_max;
        let data_blocks = particles
            .into_iter()
            .map(|(p, extras)| ParticleData { extras, ..p })
            .collect();
        let emitter = Emitter::of_data(data, data_blocks).expect("a particle datablock");
        Node {
            emitter,
            live: 0,
            spread,
        }
    }

    /// The emitter moved to `at` over `ms`, emitting along `axis` on the way (`emitParticles(start, end, axis, ...)`).
    fn path(&mut self, at: Vec3, axis: Vec3, ms: u32, rng: &mut Rng) {
        self.emitter.emit_path(
            at.to_array(),
            axis.to_array(),
            [0.0; 3],
            ms,
            rng,
            &mut self.live,
        );
    }

    /// `count` particles at `at`, each thrown within the emitter's `thetaMax` of `dir` (evenly over that cone). An
    /// explosion's burst of no radius throws every particle along its normal, so the spread is made here, a particle at a
    /// time.
    fn burst(&mut self, at: Vec3, dir: Vec3, count: u32, rng: &mut Rng) {
        for _ in 0..count {
            let thrown = within_cone(dir, self.spread, rng);
            self.emitter.emit_burst(
                at.to_array(),
                thrown.to_array(),
                0.0,
                1,
                rng,
                &mut self.live,
            );
        }
    }
}

/// A unit direction within `spread` degrees of `dir`, evenly over that cone.
pub fn within_cone(dir: Vec3, spread: f32, rng: &mut Rng) -> Vec3 {
    let dir = dir.normalize_or(Vec3::Z);
    let side = dir
        .cross(if dir.z.abs() < 0.9 { Vec3::Z } else { Vec3::X })
        .normalize_or(Vec3::X);
    let up = side.cross(dir);
    let floor = spread.to_radians().cos();
    let c = floor + (1.0 - floor) * rng.f();
    let s = (1.0 - c * c).max(0.0).sqrt();
    let phi = rng.f() * TAU;
    dir * c + (side * phi.cos() + up * phi.sin()) * s
}

/// `n` emitters of the same datablocks, appended to `nodes`; their places in it.
fn group(
    nodes: &mut Vec<Node>,
    n: usize,
    data: &EmitterData,
    particles: &[(ParticleData, ParticleExtras)],
) -> Range<usize> {
    let start = nodes.len();
    nodes.extend((0..n).map(|_| Node::new(data.clone(), particles.to_vec())));
    start..nodes.len()
}

fn one(
    nodes: &mut Vec<Node>,
    data: EmitterData,
    particles: Vec<(ParticleData, ParticleExtras)>,
) -> usize {
    nodes.push(Node::new(data, particles));
    nodes.len() - 1
}

fn plain(p: ParticleData) -> (ParticleData, ParticleExtras) {
    (p, ParticleExtras::default())
}

/// A light's color times `k`.
fn light(position: Vec3, color: [f32; 3], k: f32, radius: f32) -> HdLight {
    HdLight {
        position: position.to_array(),
        color: color.map(|c| c * k),
        radius,
        carrier: None,
    }
}

/// A hash of `n` in 0..1 (a flicker that is the same every run).
fn hash(n: u32) -> f32 {
    let mut x = n.wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2c1b_3c6d);
    x ^= x >> 12;
    (x & 0xffff) as f32 / 65535.0
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The ground an effect stands on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Floor {
    Concrete,
    Grass,
    Planks,
    Basalt,
}

/// What an effect is drawn in: the camera, the light and the props, the air.
pub struct Setting {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    pub fov: u8,
    /// Towards the sun, its light and the ambient light.
    pub sun: ([f32; 3], [f32; 3], [f32; 3]),
    /// The sky's color behind the stage (gamma 0..1).
    pub clear: [f32; 3],
    /// Boxes on the stage: center, half sides, color.
    pub props: Vec<([f32; 3], [f32; 3], [u8; 3])>,
    /// The stage's own shapes (grass, trees), still through the clip.
    pub scenery: Vec<DynamicObject>,
    pub floor: Floor,
    /// The physical sky over the stage (else the clear color, with the stars at night).
    pub atmosphere: bool,
    pub wind: [f32; 3],
    /// The distance fog's color, start and end when the stage is not the usual clear air (a hazy room: the volumetric
    /// light's medium is the fog's own, and the unlit haze is as dark as the room).
    pub fog: Option<([f32; 3], f32, f32)>,
}

impl Setting {
    /// A bare stage: the camera at `eye` looking at `target`, a night sky, concrete, no props, still air.
    fn new(eye: [f32; 3], target: [f32; 3], fov: u8) -> Setting {
        Setting {
            eye,
            target,
            fov,
            sun: ([-0.2, 0.5, 0.84], [0.03, 0.035, 0.05], [0.02, 0.022, 0.035]),
            clear: [0.006, 0.008, 0.016],
            props: Vec::new(),
            scenery: Vec::new(),
            floor: Floor::Concrete,
            atmosphere: false,
            wind: [0.0; 3],
            fog: None,
        }
    }
}

pub trait Effect {
    fn setting(&self) -> Setting;
    /// The look the show is drawn at: High with the image's defaults, unless the effect needs Ultra's volumetrics.
    fn mode(&self) -> HdMode {
        HdMode::new(Quality::High)
    }
    /// One simulation step of `ms` that ends at `t` (seconds from the clip's start): the emitters moved and their particles
    /// made. Each emitter has been advanced by `ms` already.
    fn step(&mut self, t: f32, ms: u32, rng: &mut Rng);
    fn nodes(&mut self) -> &mut [Node];
    /// The frame's lights at `t`.
    fn lights(&self, t: f32, frame: u32) -> Vec<HdLight>;
    /// The frame's spot lights at `t` (the street lamps); none by default.
    fn spots(&self, _t: f32) -> Vec<HdSpot> {
        Vec::new()
    }
    /// The frame's glowing shapes at `t` (the magic circle's figure), made anew each frame; none by default.
    fn meshes(&self, _t: f32) -> Vec<DynamicObject> {
        Vec::new()
    }
    /// The frame's distorters at `t` (the explosion's shockwave bending the picture behind it); none by default.
    fn distorters(&self, _t: f32) -> Vec<HdDistorter> {
        Vec::new()
    }
    /// The frame's glowing decals at `t` (the magic circle's figure on the floor), their textures from `textures`; none by
    /// default.
    fn decals(&self, _t: f32, _textures: &mut TextureSet<'_>) -> Vec<(f32, Draw)> {
        Vec::new()
    }
    /// The air's velocity at `t` when it changes through the show (a gust), as [`Setting::wind`] holds it; None keeps the
    /// setting's.
    fn wind(&self, _t: f32) -> Option<[f32; 3]> {
        None
    }
    /// How hard the whole scene is lit at `t` beyond the setting's light (0: not at all; 1: a lightning stroke's flash,
    /// lifting the ambient light, the sky and the fog together); none by default.
    fn flash(&self, _t: f32) -> f32 {
        0.0
    }
    /// The window's key at `t`: the circle traces again, the railgun fires, a shell goes up; nothing by default.
    fn trigger(&mut self, _t: f32, _rng: &mut Rng) {}
    /// The camera at `t` when the effect moves it (the galaxy's orbit); None (the default): the stage's own.
    fn eye(&self, _t: f32) -> Option<Eye> {
        None
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// 1. A grinder's spark shower.

pub struct Sparks {
    nodes: Vec<Node>,
    streaks: Range<usize>,
    embers: Range<usize>,
    flash: usize,
    fume: usize,
}

/// Where the grinder's disc meets the steel.
const CONTACT: Vec3 = Vec3::new(0.0, 0.0, 0.52);

impl Sparks {
    pub fn new() -> Sparks {
        let hot = ParticleData {
            drag: 0.25,
            gravity: 1.0,
            lifetime_ms: 450,
            lifetime_variance_ms: 200,
            colors: [
                [1.0, 1.0, 0.9, 1.0],
                [1.0, 0.85, 0.5, 0.9],
                [1.0, 0.5, 0.15, 0.55],
                [0.6, 0.12, 0.02, 0.0],
            ],
            sizes: [0.028, 0.025, 0.02, 0.012],
            times: [0.0, 0.15, 0.6, 1.0],
            ..particle(SPARK)
        };
        let long = ParticleData {
            lifetime_ms: 900,
            lifetime_variance_ms: 300,
            colors: [
                [1.0, 0.9, 0.6, 0.9],
                [1.0, 0.65, 0.25, 0.7],
                [0.9, 0.3, 0.05, 0.45],
                [0.4, 0.06, 0.0, 0.0],
            ],
            ..hot.clone()
        };
        let streak = ParticleExtras {
            size_by_velocity: 0.05,
            collide: true,
            bounce_elasticity: 0.35,
            bounce_friction: 0.3,
            ..ParticleExtras::default()
        };
        let shower = EmitterData {
            period_ms: 1,
            velocity: 6.5,
            velocity_variance: 2.5,
            theta_max: 14.0,
            ..emitter()
        };
        let ember = ParticleData {
            drag: 0.4,
            gravity: 1.0,
            lifetime_ms: 1600,
            lifetime_variance_ms: 400,
            colors: [
                [1.0, 0.8, 0.4, 1.0],
                [1.0, 0.5, 0.15, 0.9],
                [0.8, 0.2, 0.02, 0.6],
                [0.3, 0.03, 0.0, 0.0],
            ],
            sizes: [0.06, 0.05, 0.04, 0.02],
            ..particle(SPARK)
        };
        let flash = ParticleData {
            lifetime_ms: 120,
            colors: [
                [1.0, 0.85, 0.6, 0.6],
                [1.0, 0.6, 0.3, 0.4],
                [1.0, 0.4, 0.1, 0.15],
                [1.0, 0.3, 0.0, 0.0],
            ],
            sizes: [0.45, 0.6, 0.7, 0.75],
            spin_min: -180.0,
            spin_max: 180.0,
            ..particle(GLOW)
        };
        let fume = ParticleData {
            drag: 0.9,
            gravity: -0.06,
            lifetime_ms: 2600,
            lifetime_variance_ms: 600,
            inv_alpha: true,
            colors: [
                [0.5, 0.5, 0.5, 0.0],
                [0.5, 0.5, 0.5, 0.18],
                [0.45, 0.45, 0.45, 0.08],
                [0.4, 0.4, 0.4, 0.0],
            ],
            sizes: [0.12, 0.35, 0.6, 0.8],
            times: [0.0, 0.2, 0.6, 1.0],
            spin_min: -40.0,
            spin_max: 40.0,
            ..particle(SMOKE)
        };
        let mut nodes = Vec::new();
        let streaks = group(&mut nodes, 20, &shower, &[(hot, streak), (long, streak)]);
        let embers = group(
            &mut nodes,
            2,
            &EmitterData {
                period_ms: 8,
                velocity: 4.0,
                velocity_variance: 2.0,
                theta_max: 30.0,
                ..emitter()
            },
            &[(
                ember,
                ParticleExtras {
                    size_by_velocity: 0.02,
                    collide: true,
                    bounce_elasticity: 0.45,
                    bounce_friction: 0.25,
                    ..ParticleExtras::default()
                },
            )],
        );
        let flash = one(
            &mut nodes,
            EmitterData {
                period_ms: 8,
                velocity: 0.0,
                velocity_variance: 0.0,
                ..emitter()
            },
            vec![plain(flash)],
        );
        let fume = one(
            &mut nodes,
            EmitterData {
                period_ms: 40,
                velocity: 0.4,
                velocity_variance: 0.2,
                theta_max: 40.0,
                ..emitter()
            },
            vec![(
                fume,
                ParticleExtras {
                    turbulence: 0.8,
                    turbulence_scale: 0.5,
                    ..ParticleExtras::default()
                },
            )],
        );
        Sparks {
            nodes,
            streaks,
            embers,
            flash,
            fume,
        }
    }

    /// How hard the disc presses at `t` (0..1): it bites, eases off and bites again.
    fn pressure(t: f32) -> f32 {
        let wave = 0.65 + 0.25 * (t * 2.3).sin() + 0.1 * (t * 7.1).sin();
        let lift = smooth((t - 4.6) / 0.3) * (1.0 - smooth((t - 5.4) / 0.3));
        (wave * (1.0 - 0.85 * lift)).clamp(0.0, 1.0)
    }
}

impl Effect for Sparks {
    fn setting(&self) -> Setting {
        Setting {
            sun: ([-0.3, 0.4, 0.86], [0.05, 0.05, 0.06], [0.04, 0.045, 0.06]),
            clear: [0.01, 0.012, 0.018],
            props: vec![
                // The steel block, the grinder's body above the contact, its guard.
                ([0.0, 0.0, 0.25], [0.45, 0.3, 0.25], [70, 72, 78]),
                ([-0.42, 0.0, 0.78], [0.3, 0.07, 0.07], [40, 60, 110]),
                ([-0.08, -0.07, 0.62], [0.12, 0.015, 0.12], [55, 55, 60]),
            ],
            ..Setting::new([-1.1, -4.9, 1.45], [1.7, 0.4, 0.4], 65)
        }
    }

    fn step(&mut self, t: f32, ms: u32, rng: &mut Rng) {
        let p = Sparks::pressure(t);
        let sweep = 0.12 * (t * 0.9).sin() + 0.04 * (t * 3.7).sin();
        let lift = -0.1 + 0.07 * (t * 1.3).sin();
        let axis = Vec3::new(sweep.cos(), sweep.sin(), lift).normalize();
        let active = (self.streaks.len() as f32 * p).round() as usize;
        for (k, i) in self.streaks.clone().enumerate() {
            if k < active {
                self.nodes[i].path(CONTACT, axis, ms, rng);
            } else {
                self.nodes[i].emitter.break_path();
            }
        }
        for i in self.embers.clone() {
            if p > 0.3 {
                self.nodes[i].path(CONTACT, axis, ms, rng);
            }
        }
        if p > 0.2 {
            self.nodes[self.flash].path(CONTACT + Vec3::new(0.04, 0.0, 0.0), Vec3::Z, ms, rng);
        }
        self.nodes[self.fume].path(CONTACT + Vec3::new(0.15, 0.0, 0.0), Vec3::Z, ms, rng);
    }

    fn nodes(&mut self) -> &mut [Node] {
        &mut self.nodes
    }

    fn lights(&self, t: f32, frame: u32) -> Vec<HdLight> {
        let p = Sparks::pressure(t);
        let flicker = 0.6 + 0.8 * hash(frame);
        vec![light(
            CONTACT + Vec3::new(0.35, -0.1, 0.15),
            [2.6, 1.5, 0.55],
            p * flicker,
            6.0,
        )]
    }
}

/// The showcase's own small generator for the effects' fixed choices (paths, glyphs, the grass).
pub struct Lcg(pub u64);

impl Lcg {
    pub fn unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// 2. A spinning portal.

/// The portal's center and its half width and half height (an upright oval facing -y).
const PORTAL: Vec3 = Vec3::new(0.0, 0.0, 2.15);
const RADII: Vec2 = Vec2::new(1.25, 1.9);
/// How fast the rim turns (radians a second, counterclockwise as the camera sees it).
const SPIN: f32 = 2.4;

pub struct Portal {
    nodes: Vec<Node>,
    rim: usize,
    runners: Range<usize>,
    flung: usize,
}

impl Portal {
    pub fn new() -> Portal {
        let rim = ParticleData {
            drag: 0.3,
            lifetime_ms: 420,
            lifetime_variance_ms: 140,
            colors: [
                [0.75, 0.4, 1.0, 0.0],
                [0.85, 0.45, 1.0, 0.8],
                [0.55, 0.25, 1.0, 0.5],
                [0.25, 0.1, 0.9, 0.0],
            ],
            sizes: [0.06, 0.06, 0.05, 0.03],
            times: [0.0, 0.12, 0.6, 1.0],
            ..particle(SPARK)
        };
        let runner = ParticleData {
            drag: 0.8,
            lifetime_ms: 650,
            lifetime_variance_ms: 150,
            colors: [
                [1.0, 0.85, 1.0, 1.0],
                [1.0, 0.5, 0.95, 0.8],
                [0.6, 0.25, 1.0, 0.4],
                [0.3, 0.1, 0.9, 0.0],
            ],
            sizes: [0.13, 0.1, 0.06, 0.0],
            times: [0.0, 0.15, 0.55, 1.0],
            ..particle(MOTE)
        };
        let flung = ParticleData {
            drag: 0.6,
            gravity: 0.5,
            lifetime_ms: 900,
            lifetime_variance_ms: 300,
            colors: [
                [1.0, 0.95, 1.0, 1.0],
                [1.0, 0.55, 0.95, 0.9],
                [0.6, 0.3, 1.0, 0.5],
                [0.3, 0.1, 0.8, 0.0],
            ],
            sizes: [0.035, 0.03, 0.025, 0.015],
            times: [0.0, 0.15, 0.6, 1.0],
            ..particle(SPARK)
        };
        let burst = |velocity: f32, variance: f32, theta: f32| EmitterData {
            velocity,
            velocity_variance: variance,
            theta_max: theta,
            ..emitter()
        };
        let mut nodes = Vec::new();
        let rim = one(
            &mut nodes,
            burst(2.5, 0.4, 0.0),
            vec![(
                rim,
                ParticleExtras {
                    turbulence: 0.4,
                    turbulence_scale: 0.5,
                    size_by_velocity: 0.05,
                    ..ParticleExtras::default()
                },
            )],
        );
        let runners = group(
            &mut nodes,
            6,
            &EmitterData {
                period_ms: 1,
                velocity: 0.15,
                velocity_variance: 0.1,
                ..emitter()
            },
            &[plain(runner)],
        );
        let flung = one(
            &mut nodes,
            burst(3.5, 1.5, 0.0),
            vec![(
                flung,
                ParticleExtras {
                    size_by_velocity: 0.04,
                    collide: true,
                    bounce_elasticity: 0.35,
                    bounce_friction: 0.3,
                    ..ParticleExtras::default()
                },
            )],
        );
        Portal {
            nodes,
            rim,
            runners,
            flung,
        }
    }

    /// The rim's point at angle `a`, its tangent (the spin's way) and its outward normal.
    fn rim_at(a: f32, scale: f32) -> (Vec3, Vec3, Vec3) {
        let (s, c) = a.sin_cos();
        let at = PORTAL + Vec3::new(RADII.x * c, 0.0, RADII.y * s) * scale;
        let along = Vec3::new(-RADII.x * s, 0.0, RADII.y * c).normalize();
        let out = Vec3::new(RADII.y * c, 0.0, RADII.x * s).normalize();
        (at, along, out)
    }
}

impl Effect for Portal {
    fn setting(&self) -> Setting {
        Setting {
            sun: ([-0.2, 0.5, 0.84], [0.03, 0.03, 0.05], [0.02, 0.02, 0.035]),
            clear: [0.008, 0.006, 0.016],
            props: vec![
                // A stone step under the portal and two posts beside it.
                ([0.0, 0.0, 0.1], [1.9, 0.6, 0.1], [62, 60, 66]),
                ([-1.75, 0.0, 1.0], [0.16, 0.16, 1.0], [56, 54, 60]),
                ([1.75, 0.0, 1.0], [0.16, 0.16, 1.0], [56, 54, 60]),
            ],
            ..Setting::new([-2.7, -7.9, 1.75], [0.0, 0.0, 1.95], 55)
        }
    }

    fn step(&mut self, t: f32, ms: u32, rng: &mut Rng) {
        let per_ms = ms as f32 / 16.0;
        // The rim: a band about the oval, each particle running along it.
        for _ in 0..(800.0 * per_ms) as u32 {
            let a = rng.f() * TAU;
            let (at, along, _) = Portal::rim_at(a, 0.96 + 0.08 * rng.f());
            self.nodes[self.rim].burst(at, along, 1, rng);
        }
        // Bright runners chasing about the rim.
        for (k, i) in self.runners.clone().enumerate() {
            let a = SPIN * 1.35 * t + TAU * k as f32 / self.runners.len() as f32;
            let (at, _, out) = Portal::rim_at(a, 1.0);
            self.nodes[i].path(at, out, ms, rng);
        }
        // Sparks flung off the rim, along it and outward.
        for _ in 0..(30.0 * per_ms) as u32 {
            let a = rng.f() * TAU;
            let (at, along, out) = Portal::rim_at(a, 1.02);
            self.nodes[self.flung].burst(at, (along + out * 0.6).normalize(), 1, rng);
        }
    }

    fn nodes(&mut self) -> &mut [Node] {
        &mut self.nodes
    }

    fn lights(&self, t: f32, _frame: u32) -> Vec<HdLight> {
        let pulse = 0.85 + 0.15 * (t * 3.1).sin();
        vec![light(PORTAL - Vec3::Y * 0.4, [1.4, 0.6, 2.4], pulse, 8.0)]
    }
}
