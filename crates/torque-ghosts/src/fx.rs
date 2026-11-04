//! TGE's `ParticleEmitterNode` and `fxLight` (both GameBase-derived).
//!
//! ```text
//! ParticleEmitterNode: GameBase part; F32 x16 (matrix), F32 x3 (scale); flag(emitter): ranged(emitter datablock id)
//!                      (every update: the node keeps no mask of its own)
//! fxLight:             GameBase part; flag(AttachChange, bit 1, && attach valid): flag(attached object):
//!                      ranged(index, 0, MaxGhostCount) (GB + 1 bits);
//!                      flag(ConfigChange, bit 0): affine transform (193), raw byte (enable), F32 (icon size)
//! ```
//! fxLight's two bits are bits 0 and 1, the same bits as SceneObject's `ScaleMask` and GameBase's `InitialUpdateMask`: a
//! config change also sends the GameBase part's scale. An attached object with no ghost index yet goes out as `U32(-1)` cut
//! to the field (all ones). The host clears its attach-valid flag after a pack that sent the attach block.
//!
//! **View fields:** the GameBase part's; ParticleEmitterNode: `matrix` 0..16, `scale` 0..3, `emitter` (the emitter datablock's
//! id); fxLight: `attach_valid`, the link `attached`, `transform`, `enable`, `icon_size`.
//!
//! Credit: ParticleEmitterNode follows TGE 1.4's `game/fx/particleEmitter.cc` (Copyright (C) GarageGames.com, Inc.);
//! fxLight follows TGE 1.4's `game/fx/fxLight.cc` ("Written by Melvyn May, Started on 4th May 2002", part of the Torque
//! Game Engine). ParticleEmitterNode's MIT-licensed successor is Torque3D's `T3D/fx/particleEmitterNode.cpp` (Copyright
//! (c) 2012 GarageGames, LLC), which added mask bits, an active flag and a velocity; the layout here is TGE's. Torque3D has
//! no fxLight. The code is this crate's own.

use crate::codec::{Codec, F32, Layout, R, Xf, datablock_id, ghost_ranged, opt, wrap_ranged};
use crate::parts::{GameBase, pack_gamebase};
use crate::props::matrix;
use crate::{Link, ObjectView, PackCtx, boolean, id, raw, raws, xform};

/// `ParticleEmitterNode`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParticleEmitterNode {
    pub game: GameBase,
    pub matrix: [F32; 16],
    pub scale: [F32; 3],
    pub emitter: Option<u32>,
}

impl Layout for ParticleEmitterNode {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.game.layout(c)?;
        c.floats("node.matrix", &mut self.matrix)?;
        c.floats("node.scale", &mut self.scale)?;
        opt(c, "node.emitter?", &mut self.emitter, |c, d| {
            datablock_id(c, "node.emitter", d)
        })
    }
}

pub fn pack_particle_emitter_node(
    view: &dyn ObjectView,
    mask: u32,
    ctx: &PackCtx,
) -> ParticleEmitterNode {
    ParticleEmitterNode {
        game: pack_gamebase(view, mask, ctx),
        matrix: matrix(view),
        scale: raws::<3>(view, "scale", 0),
        emitter: id(view, "emitter", 0).map(|d| ctx.wire_db(Some(d))),
    }
}

/// fxLight's mask bits.
pub const FX_LIGHT_CONFIG_MASK: u32 = 1 << 0;
pub const FX_LIGHT_ATTACH_MASK: u32 = 1 << 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FxLightConfig {
    pub transform: Xf,
    pub enable: u8,
    pub icon_size: F32,
}

/// `fxLight`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FxLight {
    pub game: GameBase,
    /// `None` no attach block, `Some(None)` detached, `Some(Some(index))` attached to the ghost.
    pub attach: Option<Option<u32>>,
    pub config: Option<FxLightConfig>,
}

impl Layout for FxLight {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.game.layout(c)?;
        opt(c, "fxlight.attach?", &mut self.attach, |c, a| {
            opt(c, "fxlight.attached", a, |c, g| {
                ghost_ranged(c, "fxlight.object", g)
            })
        })?;
        opt(c, "fxlight.config?", &mut self.config, |c, k| {
            c.xform("fxlight.transform", &mut k.transform)?;
            c.raw8("fxlight.enable", &mut k.enable)?;
            c.raw32("fxlight.iconSize", &mut k.icon_size.0)
        })
    }
}

pub fn pack_fx_light(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> FxLight {
    let hi = 1u32
        .checked_shl(u32::from(ctx.ghost_bits()))
        .unwrap_or(u32::MAX);
    let attach =
        (mask & FX_LIGHT_ATTACH_MASK != 0 && boolean(view, "attach_valid", 0)).then(|| match view
            .link("attached")
        {
            Link::Null => None,
            Link::Ghost(g) => Some(wrap_ranged(g, 0, hi)),
            Link::NotGhosted => Some(wrap_ranged(u32::MAX, 0, hi)),
        });
    let config = (mask & FX_LIGHT_CONFIG_MASK != 0).then(|| FxLightConfig {
        transform: xform(view, "transform"),
        enable: u8::from(boolean(view, "enable", 0)),
        icon_size: raw(view, "icon_size", 0),
    });
    FxLight {
        game: pack_gamebase(view, mask, ctx),
        attach,
        config,
    }
}
