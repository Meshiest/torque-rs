//! The parent parts a ghost record starts with: the GameBase part (every GameBase-derived class: the scale and the datablock),
//! and the [`ShapeBasePart`] seam, the part every ShapeBase-derived class starts with. A game whose ShapeBase part differs from
//! the engine's implements its own and instantiates the class bodies that follow the part (the markers) with it.
//!
//! **GameBase** (`GameBase::packUpdate`: mask bit 0 `ScaleMask`, bit 2 `DataBlockMask`):
//! ```text
//! flag(mask & 1): F32 x3 scale
//! flag(mask & 4 && datablock != 0): ranged(id, first, first + 2^DB - 1)
//! ```
//!
//! Credit: the GameBase part follows TGE 1.4's `game/gameBase.cc`, carried into the MIT-licensed Torque3D as
//! `T3D/gameBase/gameBase.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is
//! this crate's own.

use std::fmt::Debug;

use crate::codec::{Codec, F32, R, datablock_id, opt};
use crate::{ObjectView, PackCtx, id, raws};

/// GameBase's update-mask bits: SceneObject's `ScaleMask`, then GameBase's own.
pub mod mask {
    pub const SCALE: u32 = 1 << 0;
    pub const INITIAL: u32 = 1 << 1;
    pub const DATABLOCK: u32 = 1 << 2;
    pub const EXTENDED_INFO: u32 = 1 << 3;
    pub const CONTROL: u32 = 1 << 4;
    /// The first bit a GameBase-derived class takes (`NextFreeMask`).
    pub const NEXT_FREE: u32 = 1 << 5;
}

/// The GameBase part's record.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GameBase {
    /// `flag(mask & 1)` and the scale.
    pub scale: Option<[F32; 3]>,
    /// `flag(mask & 4 && datablock != 0)` and the datablock id.
    pub datablock: Option<u32>,
}

impl GameBase {
    pub fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        opt(c, "gb.scale?", &mut self.scale, |c, s| {
            c.floats("gb.scale", s)
        })?;
        opt(c, "gb.datablock?", &mut self.datablock, |c, d| {
            datablock_id(c, "gb.datablock", d)
        })
    }
}

/// Packs the GameBase part (`GameBase::packUpdate` returns 0: it keeps no bit dirty).
pub fn pack_gamebase(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> GameBase {
    GameBase {
        scale: (mask & 1 != 0).then(|| raws::<3>(view, "scale", 0)),
        datablock: if mask & 4 != 0 {
            id(view, "datablock", 0).map(|d| ctx.wire_db(Some(d)))
        } else {
            None
        },
    }
}

/// The ShapeBase part of a ShapeBase-derived class's record (it contains the GameBase part): its layout, and its pack, which
/// applies the part's own mask rules and returns the bits the part keeps dirty.
pub trait ShapeBasePart: Debug + Clone + PartialEq + Eq + Default {
    /// The first update-mask bit the part leaves to the class that follows it (`NextFreeMask`).
    const NEXT_FREE_MASK: u32;
    /// The part's layout, in wire order.
    fn layout<C: Codec>(&mut self, c: &mut C) -> R;
    /// Packs the part: the record and the return mask.
    fn pack(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> (Self, u32);
    /// The GameBase part inside it.
    fn game(&self) -> &GameBase;
}
