//! Particles: TGE's particle engine on the client (`particleEngine.cc`), each emitter's particles made, moved, aged and drawn:
//!
//! - **Emitting along a path** (`ParticleEmitter::emitParticles(start, end, axis, velocity, ms)`): the axis's perpendicular
//!   `axisx` (`axis x (0,0,1)` unless the axis is within 0.9 of z, then `axis x (0,1,0)`: a game's own through
//!   [`ParticleDatablocks::near_z_cross`]); a particle every `ejectionPeriodMS`
//!   (plus a random `+-periodVarianceMS`), each at the point between the last call's position and this one's, the time left of
//!   the call run on it at once unless `overrideAdvance`; the time to the next particle kept across calls. A call longer than
//!   [`MAX_CALL_MS`] makes nothing (this crate's cap: a stalled frame does not pour out particles).
//! - **A projectile's trail** (`Projectile::emitParticles(from, to, vel, ms)`): a path from where the projectile was to where
//!   it is, the axis against its velocity ([`trail_axis`]; up when it has none); [`Emitter::emit_trail`].
//! - **A burst** (`emitParticles(center, normal, radius, velocity, count)`): `count` particles in the half-box of `radius` on
//!   the normal's side.
//! - **A particle** (`addParticle` with `ParticleData::initializeParticle`): the speed `ejectionVelocity +- velocityVariance`,
//!   the direction the axis turned by `theta` (`thetaMin..thetaMax`) about `axisx` then by `phi` (`phiReferenceVel` per second
//!   of the emitter's clock plus `phiVariance` at random) about the axis; the position offset by `ejectionOffset` along it; one
//!   of the emitter's particle datablocks at random; the velocity plus the inherited one times `inheritedVelFactor`, the
//!   acceleration the velocity times `constantAcceleration`, the life `lifetimeMS +- lifetimeVarianceMS`, the spin `spinSpeed`
//!   plus a random `spinRandomMin..Max`.
//! - **A step** (`updateSingleParticle`): `v += (a - v * drag - wind * windCoefficient - (0, 0, 9.81 * gravityCoefficient)) *
//!   dt`, `p += v * dt`; the colour and size between the keys the age has reached (the emitter's own colours and sizes with
//!   `useEmitterColors` / `useEmitterSizes`), the last key's past the last time. The particle a call made is not stepped again
//!   by the next frame.
//! - **Drawn** (`ParticleEmitter::renderObject`, `renderBillboardParticle`): textured (`GL_MODULATE`, clamped), blended
//!   `SRC_ALPHA, ONE` unless the particle datablock has `useInvAlpha` (or the emitter's own colours ask for it: [`Emitter::set_colors`]),
//!   depth writes off, no lighting, no fog; a square of `size * 0.5` turned by the spin (`spinSpeed * age`, degrees a second) in
//!   the camera's plane. The emitter's texture is its first particle's datablock's first one (an `animateTexture` datablock steps
//!   its list at `framesPerSec`).
//!
//! **Approximations**: the random numbers are the emitters' own ([`Rng`], TGE's `MRandomLCG` step; TGE's particle engine draws
//! from one file-wide `sgRandom`); the wind is still air; an emitter's particles sort with the frame's translucent draws as one,
//! at its centre; `orientParticles` draws billboards too. **Caps:** at most [`MAX_EMITTERS`] emitters (an [`Emitters`] set) and
//! [`MAX_PARTICLES`] particles in all, a burst of [`MAX_BURST`] (a server's datablocks name the counts).
//!
//! The datablocks come through [`ParticleDatablocks`] (TGE's client set reads its records' fields).
//!
//! Credit: follows TGE 1.4's `game/fx/particleEngine.cc`, `game/projectile.cc` (the trail) and `math/mRandom.cc` (Copyright (C)
//! GarageGames.com, Inc.), as Torque3D carries them under the MIT license (`T3D/fx/particleEmitter.cpp`, `T3D/fx/particle.cpp`,
//! `T3D/projectile.cpp`: Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.; `math/mRandom.cpp`: Copyright
//! (c) 2012 GarageGames, LLC). The code here is this crate's own.

use std::collections::{BTreeMap, BTreeSet};

use ghost_world::tge::TgeClientSet;
use ghost_world::world::ClientWorld;
use ghosts::Value;

use glam::Vec3;

use crate::math::{Mat, unit_over};
use crate::textures::TextureSet;
use crate::{Blend, Cull, Draw, Filter, Sampler, TexEnv, Vertex};

/// The most texture names [`take_missing_textures`] keeps between takes.
const MAX_MISSING_NAMES: usize = 64;

thread_local! {
    /// Particle texture names that did not load since the last [`take_missing_textures`] (each once).
    static MISSING: std::cell::RefCell<BTreeSet<String>> = const { std::cell::RefCell::new(BTreeSet::new()) };
}

/// The particle texture names that did not load (their particles not drawn) since the last call, for the client to say once.
pub fn take_missing_textures() -> Vec<String> {
    MISSING.with(|m| std::mem::take(&mut *m.borrow_mut()).into_iter().collect())
}

/// Engine caps.
pub const MAX_EMITTERS: usize = 512;
pub const MAX_PARTICLES: usize = 20_000;
pub const MAX_BURST: u32 = 2_000;
/// The longest call of an emitter that makes particles (a longer one makes none).
pub const MAX_CALL_MS: u32 = 2_000;
/// Gravity (the constant 9.81 the step multiplies by `gravityCoefficient`).
const GRAVITY: f32 = 9.81;
/// `AgedSpinToRadians`: degrees a second at an age in ms.
const AGED_SPIN_TO_RADIANS: f32 = (1.0 / 1000.0) * (std::f32::consts::PI / 180.0);

/// A `ParticleData` as the client holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticleData {
    pub drag: f32,
    pub wind: f32,
    pub gravity: f32,
    pub inherited_vel: f32,
    pub constant_accel: f32,
    pub lifetime_ms: i32,
    pub lifetime_variance_ms: i32,
    pub spin_speed: f32,
    pub spin_min: f32,
    pub spin_max: f32,
    pub inv_alpha: bool,
    pub colors: [[f32; 4]; 4],
    pub sizes: [f32; 4],
    pub times: [f32; 4],
    pub textures: Vec<String>,
    pub animate: bool,
    pub fps: i32,
}

/// A `ParticleEmitterData` as the client holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct EmitterData {
    pub period_ms: i32,
    pub period_variance_ms: i32,
    pub velocity: f32,
    pub velocity_variance: f32,
    pub offset: f32,
    pub theta_min: f32,
    pub theta_max: f32,
    pub phi_ref_vel: f32,
    pub phi_variance: f32,
    pub override_advance: bool,
    pub lifetime_ms: i32,
    pub use_emitter_sizes: bool,
    pub use_emitter_colors: bool,
    pub particles: Vec<u32>,
}

/// What the emitters read of a client's datablocks: its `ParticleData` and `ParticleEmitterData` as the client holds them
/// (`None`: no such datablock of that class). TGE's client set reads its records' fields ([`ClientWorld`]).
pub trait ParticleDatablocks {
    fn particle_data(&self, id: u32) -> Option<ParticleData>;
    fn emitter_data(&self, id: u32) -> Option<EmitterData>;

    /// The vector an emitter's axis within 0.9 of z is crossed with for its perpendicular `axisx`: TGE's is y
    /// (`ParticleEmitter::emitParticles`).
    fn near_z_cross(&self) -> [f32; 3] {
        [0.0, 1.0, 0.0]
    }
}

/// TGE's `MaxParticleSize`: a size key travels as a fraction of it.
const MAX_PARTICLE_SIZE: f32 = 50.0;

/// TGE's datablocks as the client's records hold them: the values `ParticleData::packData` and `ParticleEmitterData::packData`
/// wrote, by their script fields ([`ghosts::datablocks::fields`]), read back as `unpackData` reads them into the members (a
/// `readFloat(n)` is the integer over `2^n - 1`, a lifetime travels in 32 ms steps, a speed in hundredths); a block the server
/// did not send keeps the constructor's default.
impl ParticleDatablocks for ClientWorld<TgeClientSet> {
    fn particle_data(&self, id: u32) -> Option<ParticleData> {
        let db = self
            .datablocks
            .get(&id)
            .filter(|d| d.class == "ParticleData")?;
        let int = |name: &str, i: u32| match db.get(name, i) {
            Some(Value::Int(v)) => Some(*v),
            _ => None,
        };
        // `readFloat(bits)`.
        let unit = |name: &str, i: u32, bits: u32| {
            let top = (1i64 << bits) - 1;
            int(name, i).map(|v| v.clamp(0, top) as f32 / top as f32)
        };
        let float = |name: &str, default: f32| match db.get(name, 0) {
            Some(Value::F32(x)) if x.is_finite() => *x,
            _ => default,
        };
        // `readInt(10) << 5`.
        let ms = |name: &str, default: i32| {
            int(name, 0).map_or(default, |v| (v.clamp(0, 1023) << 5) as i32)
        };
        let flag = |name: &str| matches!(db.get(name, 0), Some(Value::Bool(true)));
        let text = |name: &str, i: u32| match db.get(name, i) {
            Some(Value::Str(s)) if !s.is_empty() => {
                Some(String::from_utf8_lossy(s).replace('\\', "/"))
            }
            _ => None,
        };
        // The keys the server sent (`readInt(2) + 1` of them); the others keep the constructor's white, 1 and 0, 1, 2, 2.
        let mut colors = [[1.0f32; 4]; 4];
        let mut sizes = [1.0f32; 4];
        let mut times = [0.0, 1.0, 2.0, 2.0];
        for k in 0..4u32 {
            for c in 0..4u32 {
                if let Some(v) = unit("colors", 4 * k + c, 7) {
                    colors[k as usize][c as usize] = v;
                }
            }
            if let Some(v) = unit("sizes", k, 14) {
                sizes[k as usize] = v * MAX_PARTICLE_SIZE;
            }
            if let Some(v) = unit("times", k, 8) {
                times[k as usize] = v;
            }
        }
        // `readInt(11) - 1000` each, when sent.
        let spin = |i: u32| int("spinRandom", i).map_or(0.0, |v| (v.clamp(0, 2047) - 1000) as f32);
        let animate = flag("animateTexture");
        // The texture list as the codec names it (`textureName` is its first): the first drawn, or the list stepped with
        // `animateTexture`.
        let textures: Vec<String> = (0..ghosts::datablocks::MAX_PARTICLE_TEXTURES as u32)
            .filter_map(|i| text("animTexName", i))
            .collect();
        Some(ParticleData {
            drag: unit("dragCoefficient", 0, 10).map_or(0.0, |v| v * 5.0),
            wind: float("windCoefficient", 1.0),
            // `readSignedFloat(12) * 10`.
            gravity: unit("gravityCoefficient", 0, 12).map_or(0.0, |v| (v * 2.0 - 1.0) * 10.0),
            inherited_vel: unit("inheritedVelFactor", 0, 9).unwrap_or(0.0),
            constant_accel: float("constantAcceleration", 0.0),
            lifetime_ms: ms("lifetimeMS", 1000),
            lifetime_variance_ms: ms("lifetimeVarianceMS", 0),
            spin_speed: float("spinSpeed", 0.0),
            spin_min: spin(0),
            spin_max: spin(1),
            inv_alpha: flag("useInvAlpha"),
            colors,
            sizes,
            times,
            textures,
            animate,
            // `numFrames` (1) by default; `onAdd` makes one past 200 (or negative, as a `U32`) 20.
            fps: match int("framesPerSec", 0).unwrap_or(1) {
                f @ 0..=200 => f as i32,
                _ => 20,
            },
        })
    }

    fn emitter_data(&self, id: u32) -> Option<EmitterData> {
        let db = self
            .datablocks
            .get(&id)
            .filter(|d| d.class == "ParticleEmitterData")?;
        let int = |name: &str| match db.get(name, 0) {
            Some(Value::Int(v)) => Some(*v),
            _ => None,
        };
        // A speed in hundredths (`readInt(n) / 100`).
        let hundredths = |name: &str, default: f32| int(name).map_or(default, |v| v as f32 / 100.0);
        let angle = |name: &str, default: f32| int(name).map_or(default, |v| v as f32);
        let flag = |name: &str| matches!(db.get(name, 0), Some(Value::Bool(true)));
        // The particle datablocks (`dataBlockIds`), at most 16 named, each a `ParticleData` the client holds.
        let particles: Vec<u32> = (0..16)
            .filter_map(|i| match db.get("dataBlockIds", i) {
                Some(Value::Int(v)) => u32::try_from(*v).ok().filter(|v| *v > 0),
                Some(Value::Id(v)) => *v,
                _ => None,
            })
            .filter(|id| {
                self.datablocks
                    .get(id)
                    .is_some_and(|d| d.class == "ParticleData")
            })
            .collect();
        if particles.is_empty() {
            return None;
        }
        Some(EmitterData {
            period_ms: int("ejectionPeriodMS").unwrap_or(100).clamp(1, 1 << 20) as i32,
            period_variance_ms: int("periodVarianceMS").unwrap_or(0).clamp(0, 1 << 20) as i32,
            velocity: hundredths("ejectionVelocity", 2.0),
            velocity_variance: hundredths("velocityVariance", 1.0),
            offset: hundredths("ejectionOffset", 0.0),
            theta_min: angle("thetaMin", 0.0),
            theta_max: angle("thetaMax", 90.0),
            phi_ref_vel: angle("phiReferenceVel", 0.0),
            phi_variance: angle("phiVariance", 360.0),
            override_advance: flag("overrideAdvance"),
            // `readInt(10) << 5`.
            lifetime_ms: int("lifetimeMS").map_or(0, |v| (v.clamp(0, 1023) << 5) as i32),
            use_emitter_sizes: flag("useEmitterSizes"),
            use_emitter_colors: flag("useEmitterColors"),
            particles,
        })
    }
}

/// An emitter set's random numbers (Park-Miller, as TGE's `MRandomLCG` steps; seeded once per run).
#[derive(Debug, Clone)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Rng {
        Rng(seed.clamp(1, 0x7fff_fffe))
    }

    fn next(&mut self) -> u32 {
        self.0 = ((u64::from(self.0) * 16807) % 0x7fff_ffff) as u32;
        self.0
    }

    /// 0..1.
    pub fn f(&mut self) -> f32 {
        self.next() as f32 / 2_147_483_647.0
    }

    /// `lo..=hi`.
    pub fn i(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next() % (hi - lo + 1) as u32) as i32
    }
}

/// One particle.
#[derive(Debug, Clone, PartialEq)]
pub struct Particle {
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    acc: [f32; 3],
    pub life_ms: i32,
    pub age_ms: i32,
    /// The particle datablock (an index into the emitter's list).
    data: usize,
    pub color: [f32; 4],
    pub size: f32,
    spin: f32,
    /// Made by the last call: the next frame does not step it again.
    fresh: bool,
}

/// `v` at unit length (as it is when that is 1e-6 or less, or not finite).
fn unit_dir(v: Vec3) -> Vec3 {
    unit_over(v, 1e-6).unwrap_or(v)
}

/// `v` turned by `angle` (radians) about the unit `axis` (an `AngAxisF`'s matrix applied).
fn rotate(v: Vec3, axis: Vec3, angle: f32) -> Vec3 {
    let m = Mat::from_axis_angle(axis.to_array(), angle);
    m.mul_v(v.to_array()).into()
}

/// The axis a projectile's trail is emitted along (`Projectile::emitParticles`): against its velocity, up when it has none.
pub fn trail_axis(vel: [f32; 3]) -> [f32; 3] {
    unit_over(-Vec3::from(vel), 1e-6).map_or([0.0, 0.0, 1.0], |a| a.to_array())
}

/// One emitter: its datablocks, its particles and its clock.
#[derive(Debug, Clone)]
pub struct Emitter {
    pub data: EmitterData,
    pub particle_data: Vec<ParticleData>,
    /// The live particles, oldest first (TGE links each new one at the head of its list: it draws them newest first).
    pub particles: Vec<Particle>,
    /// The time to the next particle (`mNextParticleTime`), the emitter's clock (`mInternalClock`, ms).
    next_ms: i32,
    clock_ms: i32,
    /// The last call's end position, axis and velocity (interpolated from by the next).
    last: Option<([f32; 3], [f32; 3], [f32; 3])>,
    /// What an axis within 0.9 of z is crossed with ([`ParticleDatablocks::near_z_cross`]).
    near_z_cross: [f32; 3],
    /// The emitter's own colours ([`Emitter::set_colors`]) and whether they draw blended by alpha.
    colors: Option<[[f32; 4]; 4]>,
    colors_inv_alpha: bool,
    /// No more particles: removed once the last one died (`deleteWhenEmpty`).
    pub dead: bool,
}

impl Emitter {
    /// An emitter of `ParticleEmitterData` `datablock` (`None`: no such datablock, or none of its particle datablocks held).
    pub fn new<D: ParticleDatablocks + ?Sized>(world: &D, datablock: u32) -> Option<Emitter> {
        let data = world.emitter_data(datablock)?;
        let particle_data: Vec<ParticleData> = data
            .particles
            .iter()
            .filter_map(|id| world.particle_data(*id))
            .collect();
        let mut e = Emitter::of_data(data, particle_data)?;
        e.near_z_cross = world.near_z_cross();
        Some(e)
    }

    /// An emitter of `data` making particles of `particle_data` (`None`: no particle datablock), TGE's in every rule.
    pub fn of_data(data: EmitterData, particle_data: Vec<ParticleData>) -> Option<Emitter> {
        if particle_data.is_empty() {
            return None;
        }
        Some(Emitter {
            data,
            particle_data,
            particles: Vec::new(),
            next_ms: 0,
            clock_ms: 0,
            last: None,
            near_z_cross: [0.0, 1.0, 0.0],
            colors: None,
            colors_inv_alpha: false,
            dead: false,
        })
    }

    /// Forgets the last call's end: the next path starts at its own start (an emitter moved far, or emitting again after a
    /// pause).
    pub fn break_path(&mut self) {
        self.last = None;
    }

    /// The emitter's own colour keys (`ParticleEmitter::setColors`), and whether they draw blended by alpha
    /// (`ONE_MINUS_SRC_ALPHA`) instead of added.
    pub fn set_colors(&mut self, keys: [[f32; 4]; 4], inv_alpha: bool) {
        self.colors = Some(keys);
        self.colors_inv_alpha = inv_alpha;
    }

    fn add_particle(
        &mut self,
        pos: Vec3,
        axis: Vec3,
        vel: Vec3,
        axisx: Vec3,
        rng: &mut Rng,
        live: &mut usize,
    ) -> Option<usize> {
        if *live >= MAX_PARTICLES {
            return None;
        }
        let d = &self.data;
        let initial = d.velocity + (d.velocity_variance * 2.0 * rng.f()) - d.velocity_variance;
        let theta = (d.theta_max - d.theta_min) * rng.f() + d.theta_min;
        let reference = (self.clock_ms as f32 / 1000.0) * d.phi_ref_vel;
        let phi = reference + rng.f() * d.phi_variance;
        let mut dir = rotate(axis, unit_dir(axisx), theta.to_radians());
        dir = rotate(dir, unit_dir(axis), phi.to_radians());
        let which = ((rng.f() * self.particle_data.len() as f32).ceil() as usize)
            .saturating_sub(1)
            .min(self.particle_data.len() - 1);
        let p = &self.particle_data[which];
        let v = dir * initial + vel * p.inherited_vel;
        let acc = v * p.constant_accel;
        let mut life = p.lifetime_ms;
        if p.lifetime_variance_ms > 0 {
            life += rng.i(-p.lifetime_variance_ms, p.lifetime_variance_ms);
        }
        let spin = p.spin_speed + p.spin_min + rng.f() * (p.spin_max - p.spin_min);
        self.particles.push(Particle {
            pos: (pos + dir * d.offset).to_array(),
            vel: v.to_array(),
            acc: acc.to_array(),
            life_ms: life.max(0),
            age_ms: 0,
            data: which,
            color: p.colors[0],
            size: p.sizes[0],
            spin,
            fresh: false,
        });
        *live += 1;
        Some(self.particles.len() - 1)
    }

    /// One particle stepped by `ms` (`updateSingleParticle`).
    fn step(&self, q: &mut Particle, ms: i32) {
        let p = &self.particle_data[q.data.min(self.particle_data.len() - 1)];
        let dt = ms as f32 / 1000.0;
        // Still air: the wind term is 0.
        let g = Vec3::new(0.0, 0.0, GRAVITY * p.gravity);
        let vel = Vec3::from(q.vel) + (Vec3::from(q.acc) - Vec3::from(q.vel) * p.drag - g) * dt;
        q.vel = vel.to_array();
        q.pos = (Vec3::from(q.pos) + vel * dt).to_array();
        let t = if q.life_ms > 0 {
            q.age_ms as f32 / q.life_ms as f32
        } else {
            1.0
        };
        let mut i = 1;
        while i < 4 && p.times[i] < t {
            i += 1;
        }
        let (colors, sizes) = (
            if self.data.use_emitter_colors {
                self.colors.unwrap_or(p.colors)
            } else {
                p.colors
            },
            p.sizes,
        );
        if i >= 4 {
            q.color = colors[3];
            q.size = sizes[3];
            return;
        }
        let span = p.times[i] - p.times[i - 1];
        let f = if span > 0.0 {
            ((t - p.times[i - 1]) / span).clamp(0.0, 1.0)
        } else {
            1.0
        };
        q.color = [0, 1, 2, 3].map(|c| colors[i][c] * f + colors[i - 1][c] * (1.0 - f));
        q.size = sizes[i] * f + sizes[i - 1] * (1.0 - f);
    }

    /// `emitParticles(start, end, axis, velocity, ms)`.
    pub fn emit_path(
        &mut self,
        end: [f32; 3],
        axis: [f32; 3],
        vel: [f32; 3],
        ms: u32,
        rng: &mut Rng,
        live: &mut usize,
    ) {
        if ms > MAX_CALL_MS || self.dead {
            return;
        }
        if self.data.lifetime_ms > 0 && self.clock_ms > self.data.lifetime_ms {
            return;
        }
        let axis = unit_dir(axis.into()).to_array();
        let axisx = unit_dir(Vec3::from(axis).cross(if axis[2].abs() >= 0.9 {
            self.near_z_cross.into()
        } else {
            Vec3::Z
        }));
        let (start, start_axis, start_vel) = self.last.unwrap_or((end, axis, vel));
        let along = |t: f32| {
            (
                Vec3::from(start).lerp_monotonic(end.into(), t),
                unit_dir(Vec3::from(start_axis).lerp_monotonic(axis.into(), t)),
                Vec3::from(start_vel).lerp_monotonic(vel.into(), t),
            )
        };
        let ms = ms as i32;
        let mut done = 0i32;
        if self.next_ms > 0 {
            if ms < self.next_ms {
                self.next_ms -= ms;
                self.clock_ms += ms;
                self.last = Some((end, axis, vel));
                return;
            }
            done = self.next_ms;
            self.clock_ms += self.next_ms;
            let (at, at_axis, at_vel) = along(done as f32 / ms as f32);
            if let Some(i) = self.add_particle(at, at_axis, at_vel, axisx, rng, live) {
                self.run_new(i, ms - done);
            }
            self.next_ms = 0;
        }
        // A particle each period until the call's time runs out (bounded by the call's 2,000 ms over a period of at least 1).
        for _ in 0..=MAX_CALL_MS {
            let mut step = self.data.period_ms;
            if self.data.period_variance_ms > 0 {
                step += rng.i(-self.data.period_variance_ms, self.data.period_variance_ms);
            }
            let step = step.max(1);
            if done + step > ms {
                self.next_ms = done + step - ms;
                self.clock_ms += ms - done;
                break;
            }
            done += step;
            self.clock_ms += step;
            let (at, at_axis, at_vel) = along(done as f32 / ms as f32);
            if let Some(i) = self.add_particle(at, at_axis, at_vel, axisx, rng, live)
                && !self.data.override_advance
            {
                self.run_new(i, ms - done);
            }
        }
        self.last = Some((end, axis, vel));
    }

    /// A projectile's trail (`Projectile::emitParticles(from, to, vel, ms)`): the path it flew over `ms`, emitted against its
    /// flight (the axis `-vel`, up while it is still). A first call starts at `from`, a later one where the last ended.
    pub fn emit_trail(
        &mut self,
        from: [f32; 3],
        to: [f32; 3],
        vel: [f32; 3],
        ms: u32,
        rng: &mut Rng,
        live: &mut usize,
    ) {
        let axis = trail_axis(vel);
        if self.last.is_none() {
            self.last = Some((from, axis, vel));
        }
        self.emit_path(to, axis, vel, ms, rng, live);
    }

    /// A new particle run for the time left of its call (dropped when that outlives it), not stepped again by the next frame.
    fn run_new(&mut self, i: usize, left: i32) {
        let Some(mut q) = self.particles.get(i).cloned() else {
            return;
        };
        if left > q.life_ms {
            self.particles.remove(i);
            return;
        }
        q.age_ms = left;
        if left > 0 {
            self.step(&mut q, left);
        }
        q.fresh = true;
        self.particles[i] = q;
    }

    /// The burst of an explosion's `particleEmitter`: `count` particles about `center`.
    pub fn emit_burst(
        &mut self,
        center: [f32; 3],
        normal: [f32; 3],
        radius: f32,
        count: u32,
        rng: &mut Rng,
        live: &mut usize,
    ) {
        let mut z = unit_dir(normal.into());
        if z == Vec3::ZERO || !z.is_finite() {
            z = Vec3::Z;
        }
        let y = unit_dir(z.cross(if z.z.abs() < 0.98 { Vec3::Z } else { Vec3::Y }));
        let x = unit_dir(z.cross(y));
        let radius = if radius.is_finite() { radius } else { 0.0 };
        for _ in 0..count.min(MAX_BURST) {
            let mut pos = x * (radius * (1.0 - 2.0 * rng.f()));
            pos += y * (radius * (1.0 - 2.0 * rng.f()));
            pos += z * (radius * rng.f());
            let axis = unit_dir(pos);
            let axis = if axis == Vec3::ZERO { z } else { axis };
            self.add_particle(pos + Vec3::from(center), axis, Vec3::ZERO, z, rng, live);
        }
        self.last = Some((center, z.to_array(), [0.0; 3]));
    }

    /// A frame of `ms` (`ParticleEmitter::advanceTime`): the particles aged, the dead ones removed, the others stepped.
    pub fn advance(&mut self, ms: u32, live: &mut usize) {
        let ms = ms.min(MAX_CALL_MS) as i32;
        if ms == 0 {
            return;
        }
        let before = self.particles.len();
        let mut parts = std::mem::take(&mut self.particles);
        parts.retain_mut(|q| {
            q.age_ms = q.age_ms.saturating_add(ms);
            q.age_ms <= q.life_ms
        });
        for q in parts.iter_mut() {
            if q.fresh {
                q.fresh = false;
                continue;
            }
            self.step(q, ms);
        }
        self.particles = parts;
        *live = live.saturating_sub(before - self.particles.len());
    }

    /// The emitter's centre (where it sorts among the translucent draws).
    pub fn centre(&self) -> Option<[f32; 3]> {
        let n = self.particles.len();
        if n == 0 {
            return None;
        }
        let c: Vec3 = self.particles.iter().map(|q| Vec3::from(q.pos)).sum();
        Some((c * (1.0 / n as f32)).to_array())
    }

    /// The emitter's particles as one draw (`renderObject`): `right` and `up` the camera's axes.
    pub fn draw(
        &self,
        right: [f32; 3],
        up: [f32; 3],
        textures: &mut TextureSet<'_>,
        label: &str,
    ) -> Option<Draw> {
        // Newest first, as TGE's list holds them; the newest's datablock draws them all.
        let first = self.particles.last()?;
        let p = &self.particle_data[first.data.min(self.particle_data.len() - 1)];
        let inv_alpha = if self.data.use_emitter_colors && self.colors.is_some() {
            self.colors_inv_alpha
        } else {
            p.inv_alpha
        };
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let mut vertices = Vec::with_capacity(self.particles.len() * 6);
        let mut texture = None;
        for q in self.particles.iter().rev() {
            let frame = if p.animate && !p.textures.is_empty() {
                let n = p.textures.len();
                ((q.age_ms.max(0) as f64 * f64::from(p.fps) * 0.001) as usize) % n
            } else {
                0
            };
            if texture.is_none()
                && let Some(name) = p.textures.get(frame)
            {
                texture = textures.get_modulated(name);
            }
            let width = q.size * 0.5;
            let angle = q.spin * q.age_ms as f32 * AGED_SPIN_TO_RADIANS;
            let (s, c) = angle.sin_cos();
            let color = q.color.map(byte);
            let corner = |x: f32, z: f32, uv: [f32; 2]| {
                let rx = x * c - z * s;
                let rz = x * s + z * c;
                Vertex {
                    position: [0, 1, 2].map(|k| q.pos[k] + (right[k] * rx + up[k] * rz) * width),
                    normal: [0.0, 0.0, 1.0],
                    color,
                    uv,
                }
            };
            let v = [
                corner(-1.0, -1.0, [0.0, 1.0]),
                corner(1.0, -1.0, [1.0, 1.0]),
                corner(1.0, 1.0, [1.0, 0.0]),
                corner(-1.0, 1.0, [0.0, 0.0]),
            ];
            if v.iter().any(|x| x.position.iter().any(|c| !c.is_finite())) {
                continue;
            }
            vertices.extend([v[0], v[1], v[3], v[1], v[2], v[3]]);
        }
        // A particle whose texture names none that loads (a file the client lacks or cannot read) is not drawn: untextured, its
        // quads were flat, near-opaque squares in the particles' colour over whatever was behind them.
        if !vertices.is_empty() && !p.textures.is_empty() && texture.is_none() {
            MISSING.with(|m| {
                let mut m = m.borrow_mut();
                if m.len() < MAX_MISSING_NAMES {
                    m.insert(p.textures[0].clone());
                }
            });
        }
        if vertices.is_empty() || (!p.textures.is_empty() && texture.is_none()) {
            return None;
        }
        Some(Draw {
            label: label.to_string(),
            vertices,
            lighting: false,
            tex_env: if texture.is_some() {
                TexEnv::Modulate
            } else {
                TexEnv::None
            },
            texture: texture.map(|t| {
                (
                    t,
                    Sampler {
                        repeat: false,
                        mag: Filter::Linear,
                        min: Filter::Linear,
                        mip: Filter::Linear,
                        mipmaps: true,
                    },
                )
            }),
            blend: if inv_alpha {
                Blend::Alpha
            } else {
                Blend::Additive
            },
            cull: Cull::None,
            depth_test: true,
            depth_write: false,
            fog: false,
            lights: Vec::new(),
            vertex_key: None,
        })
    }
}

/// The emitters a client runs, by id (from 1, in the order they came): at most [`MAX_EMITTERS`].
#[derive(Debug, Clone)]
pub struct Emitters {
    map: BTreeMap<usize, Emitter>,
    next_id: usize,
}

impl Default for Emitters {
    fn default() -> Emitters {
        Emitters {
            map: BTreeMap::new(),
            next_id: 1,
        }
    }
}

impl Emitters {
    /// Adds `e`: its id, or `None` (`e` dropped) when [`MAX_EMITTERS`] run.
    pub fn add(&mut self, e: Emitter) -> Option<usize> {
        if self.map.len() >= MAX_EMITTERS {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.map.insert(id, e);
        Some(id)
    }

    pub fn get(&self, id: &usize) -> Option<&Emitter> {
        self.map.get(id)
    }

    pub fn get_mut(&mut self, id: &usize) -> Option<&mut Emitter> {
        self.map.get_mut(id)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The emitters and their ids in id order.
    pub fn iter(&self) -> impl Iterator<Item = (&usize, &Emitter)> {
        self.map.iter()
    }

    /// The emitters in id order.
    pub fn values(&self) -> impl Iterator<Item = &Emitter> {
        self.map.values()
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut Emitter> {
        self.map.values_mut()
    }

    /// Keeps the emitters `keep` holds to.
    pub fn retain(&mut self, keep: impl FnMut(&usize, &mut Emitter) -> bool) {
        self.map.retain(keep);
    }
}

#[cfg(test)]
#[path = "particles_tests.rs"]
mod tests;
