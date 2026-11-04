//! `Lightning` and `Precipitation` (GameBase-derived).
//!
//! ```text
//! Lightning:     GameBase part; flag(mask & 0x0a != 0) [InitialUpdateMask | ExtendedInfoMask, bits 1 and 3]:
//!                F32 x3 (position), F32 x3 (scale), F32 x7 (strike width, chance to hit, strike radius, bolt start radius,
//!                colour r g b), F32 x3 (fade colour r g b), raw byte (use fog), raw 32 bits (strikes per minute)   (552 bits)
//! Precipitation: GameBase part; flag(Data, bit 5): raw 32 drops, F32 x8 (min/max speed, box width/height, min/max mass, max
//!                turbulence, turbulence speed), flag useTurbulence, flag rotateWithCamVel, flag doCollision;
//!                flag(Percentage, bit 6): F32;
//!                flag(Storm, bit 7, && no other bit in the mask): F32 end percentage, raw 32 total time (ms)
//! ```
//! A Lightning initial update with a scale and a datablock is 651 bits plus the datablock id's width.
//!
//! **View fields:** the GameBase part's; Lightning: `position` 0..3, `scale` 0..3, `strike_width`, `chance_to_hit_target`,
//! `strike_radius`, `bolt_start_radius`, `color` 0..3, `fade_color` 0..3, `use_fog`, `strikes_per_minute`; Precipitation:
//! `drops`, `min_speed`, `max_speed`, `box_width`, `box_height`, `min_mass`, `max_mass`, `max_turbulence`, `turbulence_speed`,
//! `use_turbulence`, `rotate_with_cam_vel`, `do_collision`, `percentage`, `storm_end_percentage`, `storm_total_time`.
//!
//! Credit: these follow TGE 1.4's `game/fx/lightning.cc` and `game/fx/precipitation.cc` (Copyright (C) GarageGames.com,
//! Inc.), carried into the MIT-licensed Torque3D as `T3D/fx/lightning.cpp` and `T3D/fx/precipitation.cpp` (each Copyright (c)
//! 2012 GarageGames, LLC). Torque3D's Precipitation adds fields; the layout here is TGE's. The code is this crate's own.

use crate::codec::{Codec, F32, Layout, R, opt};
use crate::parts::{GameBase, pack_gamebase};
use crate::{ObjectView, PackCtx, boolean, int, raw, raws, word};

/// The mask bits that carry the body: `InitialUpdateMask | ExtendedInfoMask`.
pub const LIGHTNING_BODY_MASK: u32 = 0x0a;

/// The body of Lightning.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LightningBody {
    pub position: [F32; 3],
    pub scale: [F32; 3],
    pub strike_width: F32,
    pub chance_to_hit_target: F32,
    pub strike_radius: F32,
    pub bolt_start_radius: F32,
    pub color: [F32; 3],
    pub fade_color: [F32; 3],
    pub use_fog: u8,
    pub strikes_per_minute: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Lightning {
    pub game: GameBase,
    pub body: Option<LightningBody>,
}

impl Layout for Lightning {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.game.layout(c)?;
        opt(c, "lightning.body?", &mut self.body, |c, b| {
            c.floats("lightning.position", &mut b.position)?;
            c.floats("lightning.scale", &mut b.scale)?;
            c.raw32("lightning.strikeWidth", &mut b.strike_width.0)?;
            c.raw32("lightning.chanceToHitTarget", &mut b.chance_to_hit_target.0)?;
            c.raw32("lightning.strikeRadius", &mut b.strike_radius.0)?;
            c.raw32("lightning.boltStartRadius", &mut b.bolt_start_radius.0)?;
            c.floats("lightning.color", &mut b.color)?;
            c.floats("lightning.fadeColor", &mut b.fade_color)?;
            c.raw8("lightning.useFog", &mut b.use_fog)?;
            c.raw32("lightning.strikesPerMinute", &mut b.strikes_per_minute)
        })
    }
}

pub fn pack_lightning(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> Lightning {
    Lightning {
        game: pack_gamebase(view, mask, ctx),
        body: (mask & LIGHTNING_BODY_MASK != 0).then(|| LightningBody {
            position: raws::<3>(view, "position", 0),
            scale: raws::<3>(view, "scale", 0),
            strike_width: raw(view, "strike_width", 0),
            chance_to_hit_target: raw(view, "chance_to_hit_target", 0),
            strike_radius: raw(view, "strike_radius", 0),
            bolt_start_radius: raw(view, "bolt_start_radius", 0),
            color: raws::<3>(view, "color", 0),
            fade_color: raws::<3>(view, "fade_color", 0),
            use_fog: u8::from(boolean(view, "use_fog", 0)),
            strikes_per_minute: int(view, "strikes_per_minute", 0) as u32,
        }),
    }
}

/// Precipitation's mask bits.
pub mod precipitation_mask {
    pub const DATA: u32 = 1 << 5;
    pub const PERCENTAGE: u32 = 1 << 6;
    pub const STORM: u32 = 1 << 7;
}

/// Precipitation's data block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PrecipitationData {
    pub drops: u32,
    pub min_speed: F32,
    pub max_speed: F32,
    pub box_width: F32,
    pub box_height: F32,
    pub min_mass: F32,
    pub max_mass: F32,
    pub max_turbulence: F32,
    pub turbulence_speed: F32,
    pub use_turbulence: bool,
    pub rotate_with_cam_vel: bool,
    pub do_collision: bool,
}

/// A storm's end percentage and total time (ms).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PrecipitationStorm {
    pub end_percentage: F32,
    pub total_time: u32,
}

/// `Precipitation` (GameBase-derived).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Precipitation {
    pub game: GameBase,
    pub data: Option<PrecipitationData>,
    pub percentage: Option<F32>,
    pub storm: Option<PrecipitationStorm>,
}

impl Layout for Precipitation {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.game.layout(c)?;
        opt(c, "precip.data?", &mut self.data, |c, d| {
            c.raw32("precip.drops", &mut d.drops)?;
            for (name, f) in [
                ("precip.minSpeed", &mut d.min_speed),
                ("precip.maxSpeed", &mut d.max_speed),
                ("precip.boxWidth", &mut d.box_width),
                ("precip.boxHeight", &mut d.box_height),
                ("precip.minMass", &mut d.min_mass),
                ("precip.maxMass", &mut d.max_mass),
                ("precip.maxTurbulence", &mut d.max_turbulence),
                ("precip.turbulenceSpeed", &mut d.turbulence_speed),
            ] {
                c.raw32(name, &mut f.0)?;
            }
            c.flag("precip.useTurbulence", &mut d.use_turbulence)?;
            c.flag("precip.rotateWithCamVel", &mut d.rotate_with_cam_vel)?;
            c.flag("precip.doCollision", &mut d.do_collision)
        })?;
        opt(c, "precip.percentage?", &mut self.percentage, |c, p| {
            c.raw32("precip.percentage", &mut p.0)
        })?;
        opt(c, "precip.storm?", &mut self.storm, |c, s| {
            c.raw32("precip.storm.endPercentage", &mut s.end_percentage.0)?;
            c.raw32("precip.storm.totalTime", &mut s.total_time)
        })
    }
}

/// Packs a Precipitation (the return mask is 0). The storm block goes out only on an update whose mask has no bit besides
/// the three of the class.
pub fn pack_precipitation(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> Precipitation {
    use precipitation_mask::{DATA, PERCENTAGE, STORM};
    Precipitation {
        game: pack_gamebase(view, mask, ctx),
        data: (mask & DATA != 0).then(|| PrecipitationData {
            drops: word(view, "drops", 0),
            min_speed: raw(view, "min_speed", 0),
            max_speed: raw(view, "max_speed", 0),
            box_width: raw(view, "box_width", 0),
            box_height: raw(view, "box_height", 0),
            min_mass: raw(view, "min_mass", 0),
            max_mass: raw(view, "max_mass", 0),
            max_turbulence: raw(view, "max_turbulence", 0),
            turbulence_speed: raw(view, "turbulence_speed", 0),
            use_turbulence: boolean(view, "use_turbulence", 0),
            rotate_with_cam_vel: boolean(view, "rotate_with_cam_vel", 0),
            do_collision: boolean(view, "do_collision", 0),
        }),
        percentage: (mask & PERCENTAGE != 0).then(|| raw(view, "percentage", 0)),
        storm: (mask & !(DATA | PERCENTAGE | STORM) == 0 && mask & STORM != 0).then(|| {
            PrecipitationStorm {
                end_percentage: raw(view, "storm_end_percentage", 0),
                total_time: word(view, "storm_total_time", 0),
            }
        }),
    }
}
