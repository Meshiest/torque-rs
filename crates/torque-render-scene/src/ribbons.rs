//! Ribbons: a continuous strip laid along the recent path of something that moves (a bolt's trail), as Torque3D's `Ribbon`
//! draws one from its `RibbonData`: the first step of the modern particle system beside TGE's.
//!
//! - **The data** ([`RibbonData`]): `RibbonData`'s four keys of `size`, `color` and `position` (each key's place along the
//!   strip, 0 at its head to 1 at its tail), its texture, and how long the strip is. Torque3D measures the length in segments
//!   (`ribbonLength`, one a tick); here it is a time ([`RibbonData::lifetime_ms`]: a point of the path is dropped once it is
//!   that old), so the strip does not depend on the frame rate, with at most [`MAX_RIBBON_POINTS`] points. A projectile's
//!   trail becomes one through its `particleEmitter`'s first ParticleData ([`RibbonData::of_particle`],
//!   [`projectile_ribbon`]): its colors, sizes and times over a particle's life become the strip's over its length, its life
//!   the strip's.
//! - **Laid** ([`Ribbon::add_point`]): each new point of the path goes at the head (`addSegmentPoint`); one closer than
//!   [`MIN_STEP`] to the last moves it instead.
//! - **Drawn** ([`Ribbon::draw`]): each point a pair of vertices across the path, turned to face the eye (`prepRenderImage`:
//!   the cross of the path's direction there and the direction to the eye), its half width and color the keys' at its place
//!   along the strip (its age over the lifetime: the head bright, the tail fading to nothing); textured across its width (the
//!   texture's middle row), blended added unless the data asks for alpha, depth-tested without depth writes, unlit. A strip
//!   whose path has ended ([`Ribbon::dead`]) keeps aging until its last point is gone.
//!
//! Credit: follows Torque3D's MIT-licensed `T3D/fx/ribbon.cpp` and `T3D/fx/ribbonNode.cpp` (each Copyright (c) 2014
//! GarageGames, LLC). The code here is this crate's own.

use std::collections::VecDeque;

use ghost_world::tge::TgeClientSet;
use ghost_world::world::ClientWorld;
use ghosts::Value;

use crate::particles::{ParticleData, ParticleDatablocks};
use crate::textures::TextureSet;
use crate::{Blend, Cull, Draw, Filter, Sampler, TexEnv, Vertex};

/// `RibbonData::NumFields`: the keys.
pub const RIBBON_KEYS: usize = 4;
/// The most points a strip keeps.
pub const MAX_RIBBON_POINTS: usize = 256;
/// A new point closer than this to the head moves the head instead (units).
pub const MIN_STEP: f32 = 0.01;

/// A ribbon's look and length.
#[derive(Debug, Clone, PartialEq)]
pub struct RibbonData {
    /// Each key's half width (units).
    pub sizes: [f32; RIBBON_KEYS],
    pub colors: [[f32; 4]; RIBBON_KEYS],
    /// Each key's place along the strip, 0 (head) to 1 (tail), rising; a negative one ends the list.
    pub positions: [f32; RIBBON_KEYS],
    /// How long a point of the path stays (ms).
    pub lifetime_ms: f32,
    pub texture: Option<String>,
    /// Blended by alpha instead of added.
    pub alpha: bool,
}

impl RibbonData {
    /// The strip a trail of `p`'s particles makes: its keys over a particle's life over the strip's length (a particle's size
    /// is its full width), the life the strip's (the mean of its range), its first texture and its blend.
    pub fn of_particle(p: &ParticleData) -> RibbonData {
        RibbonData {
            sizes: p.sizes.map(|s| s * 0.5),
            colors: p.colors,
            positions: p.times,
            lifetime_ms: (p.lifetime_ms.max(1) as f32).max(1.0),
            texture: p.textures.first().cloned(),
            alpha: p.inv_alpha,
        }
    }

    /// The half width and color at `t` (0 head to 1 tail) along the strip (`Ribbon::prepRenderImage`'s walk of the keys).
    pub fn at(&self, t: f32) -> (f32, [f32; 4]) {
        let (mut size, mut color) = (self.sizes[0], self.colors[0]);
        for j in 0..RIBBON_KEYS - 1 {
            let (here, next) = (self.positions[j], self.positions[j + 1]);
            if here < 0.0 || here > t {
                break;
            }
            if next <= here {
                size = self.sizes[j];
                color = self.colors[j];
                break;
            }
            let k = ((t - here) / (next - here)).clamp(0.0, 1.0);
            size = self.sizes[j] + (self.sizes[j + 1] - self.sizes[j]) * k;
            color = [0, 1, 2, 3]
                .map(|c| self.colors[j][c] + (self.colors[j + 1][c] - self.colors[j][c]) * k);
        }
        (size, color)
    }
}

/// The strip a projectile of ProjectileData `db` trails: its `particleEmitter`'s first ParticleData's
/// ([`RibbonData::of_particle`]), when the client holds them.
pub fn projectile_ribbon(world: &ClientWorld<TgeClientSet>, db: u32) -> Option<RibbonData> {
    let emitter = match world.datablocks.get(&db)?.get("particleEmitter", 0)? {
        Value::Id(id) => (*id)?,
        Value::Int(v) => u32::try_from(*v).ok()?,
        _ => return None,
    };
    let data = world.emitter_data(emitter)?;
    let particle = world.particle_data(*data.particles.first()?)?;
    Some(RibbonData::of_particle(&particle))
}

/// A ribbon: its data and its path's points, newest first, each with its age (ms).
#[derive(Debug, Clone, PartialEq)]
pub struct Ribbon {
    pub data: RibbonData,
    points: VecDeque<([f32; 3], f32)>,
    /// The path has ended: no point is added and the strip ages away.
    pub dead: bool,
}

impl Ribbon {
    pub fn new(data: RibbonData) -> Ribbon {
        Ribbon {
            data,
            points: VecDeque::new(),
            dead: false,
        }
    }

    /// The path's points, newest first.
    pub fn points(&self) -> impl Iterator<Item = [f32; 3]> + '_ {
        self.points.iter().map(|p| p.0)
    }

    /// Drops the newest points past `end` along `dir`: a path that ran on past where its projectile hit ends there instead of
    /// folding back to it.
    pub fn cut_past(&mut self, end: [f32; 3], dir: [f32; 3]) {
        let past = |p: &[f32; 3]| (0..3).map(|k| (p[k] - end[k]) * dir[k]).sum::<f32>() > 0.0;
        while self.points.front().is_some_and(|(p, _)| past(p)) {
            self.points.pop_front();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// `p` at the head of the path (see the module docs); nothing for a point off the map's numbers or a dead ribbon.
    pub fn add_point(&mut self, p: [f32; 3]) {
        if self.dead || p.iter().any(|c| !c.is_finite()) {
            return;
        }
        if let Some(head) = self.points.front_mut() {
            let d = (0..3)
                .map(|k| (head.0[k] - p[k]).powi(2))
                .sum::<f32>()
                .sqrt();
            if d < MIN_STEP {
                head.0 = p;
                return;
            }
        }
        self.points.push_front((p, 0.0));
        self.points.truncate(MAX_RIBBON_POINTS);
    }

    /// `ms` on: every point ages, and the ones past the lifetime go (the head stays while the path goes on).
    pub fn advance(&mut self, ms: f32) {
        let ms = if ms.is_finite() { ms.max(0.0) } else { 0.0 };
        for p in &mut self.points {
            p.1 += ms;
        }
        let life = self.data.lifetime_ms;
        let keep_head = !self.dead;
        let mut i = 0;
        self.points.retain(|p| {
            i += 1;
            p.1 < life || (keep_head && i == 1)
        });
    }

    /// The strip seen from `eye` as one draw (see the module docs); none with fewer than two points or a texture that does
    /// not load.
    pub fn draw(&self, eye: [f32; 3], textures: &mut TextureSet<'_>, label: &str) -> Option<Draw> {
        let n = self.points.len();
        if n < 2 {
            return None;
        }
        let texture = match self.data.texture.as_deref() {
            Some(t) => Some(textures.get_modulated(t)?),
            None => None,
        };
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let life = self.data.lifetime_ms.max(1.0);
        let mut pairs = Vec::with_capacity(n);
        for i in 0..n {
            let p = self.points[i].0;
            let (a, b) = match i {
                0 => (p, self.points[1].0),
                _ if i == n - 1 => (self.points[i - 1].0, p),
                _ => (self.points[i - 1].0, self.points[i + 1].0),
            };
            let along = [0, 1, 2].map(|k| a[k] - b[k]);
            let to_eye = [0, 1, 2].map(|k| eye[k] - p[k]);
            let across = [
                along[1] * to_eye[2] - along[2] * to_eye[1],
                along[2] * to_eye[0] - along[0] * to_eye[2],
                along[0] * to_eye[1] - along[1] * to_eye[0],
            ];
            let len = across.iter().map(|c| c * c).sum::<f32>().sqrt();
            if len < 1e-6 || !len.is_finite() {
                continue;
            }
            let t = (self.points[i].1 / life).clamp(0.0, 1.0);
            let (half, color) = self.data.at(t);
            let side = across.map(|c| c / len * half);
            let color = color.map(byte);
            let v = |sign: f32, u: f32| Vertex {
                position: [0, 1, 2].map(|k| p[k] + side[k] * sign),
                normal: [0.0, 0.0, 1.0],
                color,
                uv: [u, 0.5],
            };
            pairs.push((v(-1.0, 0.0), v(1.0, 1.0)));
        }
        let mut vertices = Vec::with_capacity(pairs.len() * 6);
        for w in pairs.windows(2) {
            let ((a0, a1), (b0, b1)) = (w[0], w[1]);
            vertices.extend([a0, b0, a1, a1, b0, b1]);
        }
        if vertices.is_empty() {
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
            blend: if self.data.alpha {
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

    /// The middle of the strip's points (for sorting it with the frame's other translucent draws).
    pub fn center(&self) -> Option<[f32; 3]> {
        let n = self.points.len();
        (n > 0).then(|| {
            let s = self.points.iter().fold([0.0; 3], |a, p| {
                [a[0] + p.0[0], a[1] + p.0[1], a[2] + p.0[2]]
            });
            s.map(|c| c / n as f32)
        })
    }
}

#[cfg(test)]
#[path = "ribbons_tests.rs"]
mod tests;
