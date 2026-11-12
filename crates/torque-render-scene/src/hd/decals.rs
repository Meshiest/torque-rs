//! Projected decals: each decal of a frame as an oriented box ([`HdDecal`], [`super::HdScene::decals`]) that the enhanced
//! renderer projects onto the opaque frame's depth (`torque_render_hd`'s decal pass), instead of the CPU path's triangles
//! clipped to the surfaces ([`crate::decals::DecalManager::add_clipped`]).
//!
//! **The box.** Centered on the hit, its face `half_x` by `half_y` in the surface's plane, `half_z` (the face's normal times the
//! box's half depth) through it. A pixel of the opaque frame whose point lies in the box, on a surface that takes decals
//! ([`super::DynamicObject::takes_decals`]; the static geometry always does), facing within [`CLIP_ANGLE_COS`] of the
//! decal's normal, takes the texture at the point's place across the face (the CPU path's texture coordinates). It fades out
//! over [`FACING_FADE`] of facing past that cosine, so a face turned away ends the decal as the CPU path's clip does.
//!
//! **Limits.** [`HdDecal::sanitized`] keeps a box only when every value is finite, its axes are of some length and none is
//! longer than half of [`MAX_DECAL_SIZE`] (the CPU path's cap on a datablock's size): a server's datablocks and hits set them.

use glam::Vec3;

use crate::decals::{CLIP_ANGLE_COS, MAX_DECAL_SIZE};
use crate::hd::origin::Shift;
use crate::{Blend, Sampler};

/// Past [`CLIP_ANGLE_COS`] the share of facing over which a projected decal fades in (the CPU path cuts at the cosine).
pub const FACING_FADE: f32 = 0.1;

/// A decal the enhanced renderer projects (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdDecal {
    pub center: [f32; 3],
    /// Half the face along each of its sides.
    pub half_x: [f32; 3],
    pub half_y: [f32; 3],
    /// The face's unit normal times the box's half depth.
    pub half_z: [f32; 3],
    /// Its texture (an index into the scene's table once set, [`super::HdScene::set_decals`]) and sampler.
    pub texture: (usize, Sampler),
    /// [`Blend::Modulate2x`] (multiplied over the surface) or [`Blend::Alpha`] (anything else is drawn as alpha).
    pub blend: Blend,
    /// Its fade (0 to 1): the CPU path's vertex alpha, in 255ths.
    pub opacity: f32,
}

impl HdDecal {
    /// The radius of a sphere around the center holding the box.
    pub fn radius(&self) -> f32 {
        let [x, y, z] = [self.half_x, self.half_y, self.half_z].map(Vec3::from);
        (x.length_squared() + y.length_squared() + z.length_squared()).sqrt()
    }

    /// The decal when it can be drawn (see the module docs): its opacity held to 0..1, None when that is 0.
    #[must_use]
    pub fn sanitized(mut self) -> Option<HdDecal> {
        let finite = |v: &[f32; 3]| v.iter().all(|c| c.is_finite());
        if !finite(&self.center) || !self.opacity.is_finite() {
            return None;
        }
        let most = 0.5 * MAX_DECAL_SIZE;
        for axis in [self.half_x, self.half_y, self.half_z] {
            let l = Vec3::from(axis).length();
            if !finite(&axis) || !(l > 1e-6 && l <= most * (1.0 + 1e-5)) {
                return None;
            }
        }
        self.opacity = self.opacity.clamp(0.0, 1.0);
        (self.opacity > 0.0).then_some(self)
    }

    /// The share of a pixel's decal kept on a surface whose normal's cosine to the decal's is `facing` (see the module docs).
    pub fn facing_share(facing: f32) -> f32 {
        let t = ((facing - CLIP_ANGLE_COS) / FACING_FADE).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
}

impl Shift for HdDecal {
    fn shift(&mut self, by: [f32; 3]) {
        self.center.shift(by);
    }
}

#[cfg(test)]
#[path = "decals_tests.rs"]
mod tests;
