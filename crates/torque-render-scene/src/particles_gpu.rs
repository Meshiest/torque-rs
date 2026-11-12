//! An emitter's particles handed to the enhanced renderer's GPU pool ([`Emitter::hand_over`]): the emitter keeps TGE's rules of
//! when and where a particle is made (its period and variance, the path, `initializeParticle`, the random numbers) and runs a
//! new particle on the CPU until the frame is drawn; then the particle goes to the pool, which steps, ages and draws it with
//! TGE's `updateSingleParticle` and `renderBillboardParticle`. The CPU's cost is one [`GpuSpawn`] a particle made, not a
//! quad a particle alive a frame.
//!
//! A frame's hand-over is a [`SpriteFrame`]: the particles made since the last frame, each naming one of the frame's
//! [`SpriteLook`]s (a particle datablock's keys and textures, with the emitter's own colors where it uses them), and the
//! simulation time the pool steps its particles by before it takes the new ones.

use super::{AGED_SPIN_TO_RADIANS, Emitter, MAX_MISSING_NAMES, MISSING, Orient};
use crate::textures::TextureSet;
use crate::{Blend, Filter, Sampler};

/// The pool's sampler for every particle texture: linear, clamped, mipmapped (the CPU path's).
pub const SPRITE_SAMPLER: Sampler = Sampler {
    repeat: false,
    mag: Filter::Linear,
    min: Filter::Linear,
    mip: Filter::Linear,
    mipmaps: true,
};

/// A particle as the pool takes it: its state when handed over, its look an index into its frame's [`SpriteFrame::looks`]
/// (the scene's record table once the scene took the frame).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuSpawn {
    pub pos: [f32; 3],
    pub age_ms: i32,
    pub vel: [f32; 3],
    pub life_ms: i32,
    /// The constant acceleration (the velocity at birth times `constantAcceleration`).
    pub acc: [f32; 3],
    /// Degrees a second (`spinSpeed` plus its random share).
    pub spin: f32,
    pub look: u32,
    /// Made by the emitter's last call: the pool's next step ages it without moving it, as the CPU's `advance` does.
    pub fresh: bool,
    pub color: [f32; 4],
    pub size: f32,
    /// The unit direction it was ejected along (what an emitter that orients its particles without their velocity turns them to).
    pub orient_dir: [f32; 3],
}

impl GpuSpawn {
    /// The bytes of one pool particle (64): `pos, age`, `vel, life`, `acc, spin`, `look, flags, color (RGBA8), size`.
    pub const BYTES: usize = 64;
    /// Its flags word: alive (always, for a spawn) and fresh, and from [`GpuSpawn::ORIENT_SHIFT`] up the direction it was
    /// ejected along ([`orient_bits`]).
    pub const ALIVE: u32 = 1;
    pub const FRESH: u32 = 2;
    pub const ORIENT_SHIFT: u32 = 2;

    /// The particle's bytes as the pool's `Particle` holds them, the color rounded to bytes as the CPU path's vertices are.
    pub fn write(&self, out: &mut Vec<u8>) {
        let f = |out: &mut Vec<u8>, v: f32| out.extend_from_slice(&v.to_le_bytes());
        let i = |out: &mut Vec<u8>, v: i32| out.extend_from_slice(&v.to_le_bytes());
        let u = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
        self.pos.iter().for_each(|&v| f(out, v));
        i(out, self.age_ms);
        self.vel.iter().for_each(|&v| f(out, v));
        i(out, self.life_ms);
        self.acc.iter().for_each(|&v| f(out, v));
        f(out, self.spin);
        u(out, self.look);
        u(
            out,
            Self::ALIVE
                | if self.fresh { Self::FRESH } else { 0 }
                | (orient_bits(self.orient_dir) << Self::ORIENT_SHIFT),
        );
        u(out, u32::from_le_bytes(self.color.map(unit_byte)));
        f(out, self.size);
    }

    /// The spin angle at its age (radians), as the CPU path turns its quad.
    pub fn angle(&self) -> f32 {
        self.spin * self.age_ms as f32 * AGED_SPIN_TO_RADIANS
    }
}

/// The 30 bits a direction travels in: the unit vector of `dir` (up when it has none) folded onto an octahedron, its two
/// coordinates 15-bit unorm (`gpu_particle_types.wgsl`'s `pool_orient_dir` unfolds them).
pub fn orient_bits(dir: [f32; 3]) -> u32 {
    let len: f32 = dir.iter().map(|c| c.abs()).sum();
    if !(len.is_finite() && len > 1e-12) {
        return orient_bits([0.0, 0.0, 1.0]);
    }
    let n = dir.map(|c| c / len);
    let fold = |a: f32, b: f32| (1.0 - b.abs()) * if a >= 0.0 { 1.0 } else { -1.0 };
    let (x, y) = if n[2] < 0.0 {
        (fold(n[0], n[1]), fold(n[1], n[0]))
    } else {
        (n[0], n[1])
    };
    let q = |v: f32| ((v * 0.5 + 0.5).clamp(0.0, 1.0) * 32767.0).round() as u32;
    q(x) | (q(y) << 15)
}

/// A color channel as the CPU path's vertex byte.
pub(crate) fn unit_byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// How a particle datablock's particles look and move after birth: its color and size keys (the emitter's own colors with
/// `useEmitterColors`), drag, gravity and wind coefficients, its blend, and its textures (the first, or the `animateTexture`
/// list stepped at `framesPerSec`), each an index into the frame's texture set.
#[derive(Debug, Clone, PartialEq)]
pub struct SpriteLook {
    /// The source's label (what a [`crate::hd::particle_light::ParticleLight`] rule reads).
    pub label: String,
    pub colors: [[f32; 4]; 4],
    pub sizes: [f32; 4],
    pub times: [f32; 4],
    pub drag: f32,
    pub gravity: f32,
    /// `windCoefficient` (the pool's air is still: kept for the wind's term).
    pub wind: f32,
    /// Additive, or alpha-blended (`useInvAlpha`).
    pub blend: Blend,
    /// The textures that loaded, in the datablock's order (empty: untextured).
    pub frames: Vec<usize>,
    /// Frames a second, when it steps through `frames`.
    pub fps: i32,
    /// Its textures named none that loads: alive in the pool, never drawn (the CPU path draws none of them either).
    pub hidden: bool,
    /// How its particles are turned (`orientParticles`, `orientOnVelocity`).
    pub orient: Orient,
}

/// A frame's hand-over to the pool: the particles made since the last one and their looks, the simulation steps (ms, in
/// order: each emitter `advance` since the last frame) the pool steps its particles by before it takes them, and whether it
/// drops every particle it holds first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpriteFrame {
    pub looks: Vec<SpriteLook>,
    pub spawns: Vec<GpuSpawn>,
    pub steps: Vec<u32>,
    pub clear: bool,
    /// The air's velocity the pool carries its particles by (units a second; the Sky's).
    pub wind: [f32; 3],
}

impl SpriteFrame {
    /// `look`'s index in the frame (added when it is new).
    pub fn look(&mut self, look: SpriteLook) -> u32 {
        let at = match self.looks.iter().position(|l| *l == look) {
            Some(at) => at,
            None => {
                self.looks.push(look);
                self.looks.len() - 1
            }
        };
        at as u32
    }

    /// Whether it hands over nothing (no spawn, no step, no clear).
    pub fn is_empty(&self) -> bool {
        self.spawns.is_empty() && self.steps.iter().all(|&s| s == 0) && !self.clear
    }
}

impl Emitter {
    /// The particles made since the last hand-over, out of the emitter into `out` (the emitter keeps no particle: it is
    /// empty afterwards, as a dead emitter is once its last particle died), `live` less them. Each particle's look is its
    /// datablock's ([`Emitter::look`]); `label` names the source for the particle light rule.
    pub fn hand_over(
        &mut self,
        out: &mut SpriteFrame,
        textures: &mut TextureSet<'_>,
        label: &str,
        live: &mut usize,
    ) {
        if self.particles.is_empty() {
            return;
        }
        let mut looks: Vec<Option<u32>> = vec![None; self.particle_data.len()];
        let n = self.particles.len();
        for q in &self.particles {
            let data = q.data.min(self.particle_data.len() - 1);
            let look = match looks[data] {
                Some(l) => l,
                None => {
                    let l = out.look(self.look(data, textures, label));
                    looks[data] = Some(l);
                    l
                }
            };
            out.spawns.push(GpuSpawn {
                pos: q.pos,
                age_ms: q.age_ms,
                vel: q.vel,
                life_ms: q.life_ms,
                acc: q.acc,
                spin: q.spin,
                look,
                fresh: q.fresh,
                color: q.color,
                size: q.size,
                orient_dir: q.orient_dir,
            });
        }
        self.particles.clear();
        *live = live.saturating_sub(n);
    }

    /// Particle datablock `data`'s look as this emitter draws it: the emitter's own colors (and their blend) with
    /// `useEmitterColors`, else the datablock's; its textures loaded through `textures` (a list that names textures none of which
    /// loads is noted as [`super::take_missing_textures`] says, and hidden).
    pub fn look(&self, data: usize, textures: &mut TextureSet<'_>, label: &str) -> SpriteLook {
        let p = &self.particle_data[data];
        let own = self
            .data
            .use_emitter_colors
            .then_some(self.colors)
            .flatten();
        let inv_alpha = if own.is_some() {
            self.colors_inv_alpha
        } else {
            p.inv_alpha
        };
        let named: &[String] = if p.animate {
            &p.textures
        } else {
            &p.textures[..p.textures.len().min(1)]
        };
        let frames: Vec<usize> = named
            .iter()
            .filter_map(|name| textures.get_modulated(name))
            .collect();
        let hidden = !named.is_empty() && frames.is_empty();
        if hidden {
            MISSING.with(|m| {
                let mut m = m.borrow_mut();
                if m.len() < MAX_MISSING_NAMES {
                    m.insert(named[0].clone());
                }
            });
        }
        SpriteLook {
            label: label.to_string(),
            colors: own.unwrap_or(p.colors),
            sizes: p.sizes,
            times: p.times,
            drag: p.drag,
            gravity: p.gravity,
            wind: p.wind,
            blend: if inv_alpha {
                Blend::Alpha
            } else {
                Blend::Additive
            },
            frames,
            fps: if p.animate { p.fps } else { 0 },
            hidden,
            orient: self.data.extras.orient,
        }
    }
}

#[cfg(test)]
#[path = "particles_gpu_tests.rs"]
mod tests;
