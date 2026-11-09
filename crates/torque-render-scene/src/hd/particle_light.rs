//! Lit particles: which particle draws the enhanced renderer lights, and their sprites shaped for it.
//!
//! TGE draws every particle unlit. A [`ParticleLight`] rule picks draws to light instead ([`material::LIT`]): the sun through
//! its cascades at each sprite's center, the scene's ambient and sky light, and the point and spot lights at each pixel, all
//! with a wrapped response so a camera-facing sprite reads as a puff, not a card. [`lit_when_blended`] is the usual rule:
//! alpha-blended smoke and dust lit, additive sparks, flashes and flares left glowing. A game picks per emitter by its own rule
//! (a draw's label names its source).

use std::fmt;

use glam::Vec3;

use crate::hd::{HdDraw, material, oct_encode};
use crate::{Blend, Draw};

/// Which particle draws are lit ([`crate::hd::HdScene::particle_light`]).
#[derive(Clone, Copy)]
pub struct ParticleLight(pub fn(&Draw) -> bool);

impl ParticleLight {
    /// Alpha-blended draws lit, additive ones unlit.
    pub const BLENDED: ParticleLight = ParticleLight(lit_when_blended);

    pub fn lights(self, d: &Draw) -> bool {
        (self.0)(d)
    }
}

impl PartialEq for ParticleLight {
    fn eq(&self, other: &ParticleLight) -> bool {
        std::ptr::fn_addr_eq(self.0, other.0)
    }
}

impl fmt::Debug for ParticleLight {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ParticleLight(..)")
    }
}

/// The usual rule: a draw blended over what is behind it (smoke, dust) is lit; an additive one (a glow) is not.
pub fn lit_when_blended(d: &Draw) -> bool {
    d.blend == Blend::Alpha
}

/// Marks `hd`'s sprites [`material::LIT`]: each vertex's normal is its direction from its sprite's center and its `extra` the
/// sprite's half diagonal (f32 bits), so the shader finds the center and bends the sprite's normal into a dome. Every six
/// vertices must be one quad (two triangles over four corners); a draw that is not is left unlit (returns false).
pub fn shape_sprites(hd: &mut HdDraw) -> bool {
    if hd.vertices.is_empty() || !hd.vertices.len().is_multiple_of(6) {
        return false;
    }
    let mut shaped = Vec::with_capacity(hd.vertices.len());
    for quad in hd.vertices.as_chunks::<6>().0 {
        let mut corners = [[0.0f32; 3]; 4];
        let mut found = 0;
        for v in quad {
            if corners[..found].contains(&v.position) {
                continue;
            }
            if found == 4 {
                return false;
            }
            corners[found] = v.position;
            found += 1;
        }
        if found != 4 {
            return false;
        }
        let center =
            Vec3::from_array([0, 1, 2].map(|k| corners.iter().map(|c| c[k]).sum::<f32>() * 0.25));
        let offset = |p: [f32; 3]| Vec3::from(p) - center;
        let half = corners.iter().map(|&c| offset(c).length()).sum::<f32>() * 0.25;
        if !(half > 1e-6 && half.is_finite()) {
            return false;
        }
        for v in quad {
            let o = offset(v.position);
            let l = o.length().max(1e-6);
            let mut v = *v;
            v.normal = oct_encode((o / l).to_array());
            v.extra = half.to_bits();
            v.material = (v.material & !material::ZERO_NORMAL) | material::LIT;
            shaped.push(v);
        }
    }
    hd.vertices = shaped;
    true
}

#[cfg(test)]
#[path = "particle_light_tests.rs"]
mod tests;
