//! TGE's `Projectile` (GameBase-derived).
//!
//! ```text
//! GameBase part
//! flag(initial): compressed point (position); velocity block (flag(len > 0.02): normal vector (10), int(13) speed);
//!                ranged(tick, 0, 4095) (12 bits); flag(source object ghosted): ranged(index, 0, MaxGhostCount) (GB + 1 bits),
//!                ranged(slot, 0, 3) (2 bits)
//! flag(Explosion, bit 6, && hidden): F32 x3 position, F32 x3 normal, raw 32 (collide hit type)
//! flag(Bounce, bit 5): F32 x3 position, F32 x3 velocity
//! ```
//! A source object that has no ghost index yet writes flag 0 and the packer keeps the initial bit in its return mask.
//!
//! **View fields:** the GameBase part's; `position` 0..3, `velocity` 0..3, `current_tick`, the link `source_object`,
//! `source_slot`, `hidden`, `explosion_position` 0..3, `explosion_normal` 0..3, `collide_hit_type`.
//!
//! Credit: this follows TGE 1.4's `game/projectile.cc` (Copyright (C) GarageGames.com, Inc.), whose MIT-licensed successor is
//! Torque3D's `T3D/projectile.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The layout is
//! TGE's (Torque3D reworked the initial block); the code is this crate's own.

use crate::codec::{Codec, F32, Layout, R, ghost_ranged, mask_bits, opt, wrap_ranged};
use crate::motion::{CPoint, Velocity};
use crate::parts::mask::INITIAL;
use crate::parts::{GameBase, pack_gamebase};
use crate::{Link, ObjectView, PackCtx, boolean, int, raws, word};

pub const BOUNCE_MASK: u32 = 1 << 5;
pub const EXPLOSION_MASK: u32 = 1 << 6;
/// The tick count's range, `0..=MaxLivingTicks`.
pub const MAX_LIVING_TICKS: u32 = 4095;
/// The velocity block's normal vector width.
pub const VELOCITY_NORMAL_BITS: u8 = 10;

/// The source object and the image slot it fired from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Source {
    pub object: u32,
    pub slot: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProjectileInitial {
    pub position: CPoint,
    pub velocity: Option<Velocity>,
    pub tick: u32,
    pub source: Option<Source>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Explosion {
    pub position: [F32; 3],
    pub normal: [F32; 3],
    pub hit_type: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Projectile {
    pub game: GameBase,
    pub initial: Option<ProjectileInitial>,
    pub explosion: Option<Explosion>,
    /// Position and velocity.
    pub bounce: Option<([F32; 3], [F32; 3])>,
}

impl Layout for Projectile {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.game.layout(c)?;
        opt(c, "proj.initial?", &mut self.initial, |c, i| {
            i.position.layout(c)?;
            opt(c, "proj.velocity?", &mut i.velocity, |c, v| {
                v.layout(c, VELOCITY_NORMAL_BITS)
            })?;
            c.ranged("proj.tick", &mut i.tick, 0, MAX_LIVING_TICKS)?;
            opt(c, "proj.source?", &mut i.source, |c, s| {
                ghost_ranged(c, "proj.source", &mut s.object)?;
                c.ranged("proj.slot", &mut s.slot, 0, 3)
            })
        })?;
        opt(c, "proj.explosion?", &mut self.explosion, |c, e| {
            c.floats("proj.explosion.position", &mut e.position)?;
            c.floats("proj.explosion.normal", &mut e.normal)?;
            c.raw32("proj.explosion.hitType", &mut e.hit_type)
        })?;
        opt(c, "proj.bounce?", &mut self.bounce, |c, b| {
            c.floats("proj.bounce.position", &mut b.0)?;
            c.floats("proj.bounce.velocity", &mut b.1)
        })
    }
}

/// Packs a Projectile: the record and the return mask.
pub fn pack_projectile(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> (Projectile, u32) {
    let game = pack_gamebase(view, mask, ctx);
    let mut ret = 0;
    let initial = (mask & INITIAL != 0).then(|| {
        let source = match view.link("source_object") {
            Link::Ghost(g) => {
                let hi = 1u32
                    .checked_shl(u32::from(ctx.ghost_bits()))
                    .unwrap_or(u32::MAX);
                Some(Source {
                    object: wrap_ranged(g, 0, hi),
                    slot: mask_bits(word(view, "source_slot", 0), 2),
                })
            }
            Link::NotGhosted => {
                ret |= INITIAL;
                None
            }
            Link::Null => None,
        };
        ProjectileInitial {
            position: CPoint::pack(
                raws::<3>(view, "position", 0).map(F32::get),
                ctx.compression_point,
            ),
            velocity: Velocity::pack(
                raws::<3>(view, "velocity", 0).map(F32::get),
                VELOCITY_NORMAL_BITS,
            ),
            tick: wrap_ranged(int(view, "current_tick", 0) as u32, 0, MAX_LIVING_TICKS),
            source,
        }
    });
    let explosion = (mask & EXPLOSION_MASK != 0 && boolean(view, "hidden", 0)).then(|| Explosion {
        position: raws::<3>(view, "explosion_position", 0),
        normal: raws::<3>(view, "explosion_normal", 0),
        hit_type: word(view, "collide_hit_type", 0),
    });
    let bounce = (mask & BOUNCE_MASK != 0).then(|| {
        (
            raws::<3>(view, "position", 0),
            raws::<3>(view, "velocity", 0),
        )
    });
    (
        Projectile {
            game,
            initial,
            explosion,
            bounce,
        },
        ret,
    )
}
