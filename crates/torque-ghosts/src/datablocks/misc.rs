//! TGE's remaining datablocks: [`TriggerData`], [`PathedInteriorData`], [`TsShapeConstructor`], and [`NoData`], the empty
//! record of `SimDataBlock` and `GameBaseData`.
//!
//! ```text
//! SimDataBlock, GameBaseData: nothing
//! TriggerData:                S32 tickPeriodMS
//! PathedInteriorData:         db x3 sound (start, sustain, stop)
//! TSShapeConstructor:         string mShape; int(7) sequences; string per sequence
//! ```
//! A TSShapeConstructor sends its sequence slots that are set, in slot order; the decoder refuses more than
//! [`MAX_SHAPE_SEQUENCES`].
//!
//! Credit: these follow TGE 1.4's `game/trigger.cc` and `ts/tsShapeConstruct.cc` (Copyright (C) GarageGames.com, Inc.) and
//! `interior/pathedInterior.cc` (which carries no copyright notice), and `SimDataBlock` and `GameBaseData` of
//! `console/simBase.cc` and `game/gameBase.cc` (Copyright (C) GarageGames.com, Inc.). Their MIT-licensed successors are
//! Torque3D's `T3D/trigger.cpp` and `ts/tsShapeConstruct.cpp` (each Copyright (c) 2012 GarageGames, LLC; Torque3D's shape
//! constructor is a different design); Torque3D has no PathedInterior. The lists are TGE's; the code is this crate's own.

use super::{MAX_SHAPE_SEQUENCES, db_refs, dbs, present};
use crate::codec::{Codec, Layout, NetWidths, R};
use crate::{ObjectView, text, word};

/// The sounds of a PathedInteriorData (`MaxSounds`).
pub const PATHED_INTERIOR_SOUNDS: usize = 3;

/// The record of a class that sends nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NoData;

impl Layout for NoData {
    fn layout<C: Codec>(&mut self, _c: &mut C) -> R {
        Ok(())
    }
}

/// `TriggerData::packData`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TriggerData {
    /// An `S32`'s 32 bits.
    pub tick_period_ms: u32,
}

impl Layout for TriggerData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        c.raw32("tickPeriodMS", &mut self.tick_period_ms)
    }
}

pub fn pack_trigger_data(v: &dyn ObjectView) -> TriggerData {
    TriggerData {
        tick_period_ms: word(v, "tickPeriodMS", 0),
    }
}

/// `PathedInteriorData::packData`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PathedInteriorData {
    pub sound: [Option<u32>; PATHED_INTERIOR_SOUNDS],
}

impl Layout for PathedInteriorData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        db_refs(c, "sound", &mut self.sound)
    }
}

pub fn pack_pathed_interior_data(v: &dyn ObjectView, w: NetWidths) -> PathedInteriorData {
    PathedInteriorData {
        sound: dbs(v, "sound", w),
    }
}

/// `TSShapeConstructor::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TsShapeConstructor {
    pub shape: Vec<u8>,
    pub sequences: Vec<Vec<u8>>,
}

impl Layout for TsShapeConstructor {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        c.string("mShape", &mut self.shape)?;
        c.seq(
            "mSequence",
            &mut self.sequences,
            7,
            MAX_SHAPE_SEQUENCES,
            1,
            |c, s| c.string("mSequence", s),
        )
    }
}

/// Packs a TSShapeConstructor: `mSequence` 0..127, the slots the view has a value for.
pub fn pack_ts_shape_constructor(v: &dyn ObjectView) -> TsShapeConstructor {
    TsShapeConstructor {
        shape: text(v, "mShape", 0),
        sequences: (0..MAX_SHAPE_SEQUENCES)
            .filter(|&i| present(v, "mSequence", i))
            .map(|i| text(v, "mSequence", i))
            .collect(),
    }
}
