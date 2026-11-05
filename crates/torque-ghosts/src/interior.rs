//! `InteriorInstance` (SceneObject-derived: no GameBase part; its own mask bits: `InitMask` 0, `TransformMask` 1, `AlarmMask` 2,
//! the light groups from 3, `SkinBaseMask` 11, `AudioMask` 12).
//!
//! ```text
//! flag(Init):  raw32 (CRC), string255 (interior file), flag (show terrain inside), F32 x16 (matrix), F32 x3 (scale),
//!              flag (alarm), string255 (skin base), flag + ranged datablock (audio profile), flag + ranged datablock (audio
//!              environment), flag (GL lighting)
//! else:        flag(Transform): F32 x16, F32 x3; flag (alarm); then a flag a light group of the interior's animated lights
//! ```
//!
//! The light groups depend on the interior file (its animated lights), so this record carries none: an interior with animated
//! lights is not sent or read here.
//!
//! **View fields:** `matrix` 0..16, `scale` 0..3, `crc`, `interiorFile`, `showTerrainInside`, `alarm`, `skinBase`,
//! `audioProfile`, `audioEnvironment`, `useGLLighting`.
//!
//! Credit: this follows TGE 1.4's `interior/interiorInstance.cc` (`InteriorInstance::packUpdate`, Copyright (C)
//! GarageGames.com, Inc.), whose MIT-licensed successor is Torque3D 2.0's `interior/interiorInstance.cpp` (Copyright (c) 2012
//! GarageGames, LLC). The layout is TGE's; the code is this crate's own.

use crate::codec::{Codec, F32, Layout, R, datablock_id, opt};
use crate::props::matrix;
use crate::{ObjectView, boolean, id, raws, text, word};

pub const INTERIOR_INIT_MASK: u32 = 1 << 0;
pub const INTERIOR_TRANSFORM_MASK: u32 = 1 << 1;
pub const INTERIOR_ALARM_MASK: u32 = 1 << 2;

/// The first update's block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InteriorInit {
    pub crc: u32,
    pub file: Vec<u8>,
    pub show_terrain_inside: bool,
    pub matrix: [F32; 16],
    pub scale: [F32; 3],
    pub alarm: bool,
    pub skin_base: Vec<u8>,
    pub audio_profile: Option<u32>,
    pub audio_environment: Option<u32>,
    pub gl_lighting: bool,
}

/// `InteriorInstance`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InteriorInstance {
    pub init: Option<InteriorInit>,
    /// A later update's transform and scale.
    pub transform: Option<([F32; 16], [F32; 3])>,
    /// A later update's alarm state.
    pub alarm: bool,
}

fn datablock<C: Codec>(c: &mut C, name: &'static str, v: &mut Option<u32>) -> R {
    opt(c, name, v, |c, id| datablock_id(c, name, id))
}

impl Layout for InteriorInstance {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        opt(c, "interior.init?", &mut self.init, |c, i| {
            c.raw32("interior.crc", &mut i.crc)?;
            c.string("interior.file", &mut i.file)?;
            c.flag("interior.showTerrainInside", &mut i.show_terrain_inside)?;
            c.floats("interior.matrix", &mut i.matrix)?;
            c.floats("interior.scale", &mut i.scale)?;
            c.flag("interior.alarm", &mut i.alarm)?;
            c.string("interior.skinBase", &mut i.skin_base)?;
            datablock(c, "interior.audioProfile", &mut i.audio_profile)?;
            datablock(c, "interior.audioEnvironment", &mut i.audio_environment)?;
            c.flag("interior.glLighting", &mut i.gl_lighting)
        })?;
        if self.init.is_some() {
            self.transform = None;
            self.alarm = false;
            return Ok(());
        }
        opt(
            c,
            "interior.transform?",
            &mut self.transform,
            |c, (m, s)| {
                c.floats("interior.matrix", m)?;
                c.floats("interior.scale", s)
            },
        )?;
        c.flag("interior.alarm", &mut self.alarm)
    }
}

pub fn pack_interior_instance(view: &dyn ObjectView, mask: u32) -> InteriorInstance {
    let placed = || (matrix(view), raws::<3>(view, "scale", 0));
    if mask & INTERIOR_INIT_MASK != 0 {
        let (matrix, scale) = placed();
        return InteriorInstance {
            init: Some(InteriorInit {
                crc: word(view, "crc", 0),
                file: text(view, "interiorFile", 0),
                show_terrain_inside: boolean(view, "showTerrainInside", 0),
                matrix,
                scale,
                alarm: boolean(view, "alarm", 0),
                skin_base: text(view, "skinBase", 0),
                audio_profile: id(view, "audioProfile", 0),
                audio_environment: id(view, "audioEnvironment", 0),
                gl_lighting: boolean(view, "useGLLighting", 0),
            }),
            transform: None,
            alarm: false,
        };
    }
    InteriorInstance {
        init: None,
        transform: (mask & INTERIOR_TRANSFORM_MASK != 0).then(placed),
        alarm: boolean(view, "alarm", 0),
    }
}
