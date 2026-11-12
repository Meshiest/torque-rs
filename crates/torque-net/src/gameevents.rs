//! The small Torque events of a game connection: `Sim2DAudioEvent`, `Sim3DAudioEvent`, `PathManagerEvent` and
//! `LightningStrikeEvent`. Each has a `pack`, an `unpack` and a [`WireEvent`] impl, so the event queue sends them and `unpack` is
//! the datagram decoder's.
//!
//! | event | direction | guarantee | body |
//! |---|---|---|---|
//! | `Sim2DAudioEvent` | to client | ordered | `int(DB, profile id - 3)` |
//! | `Sim3DAudioEvent` | to client | ordered | `int(DB, profile id - 3)`, `flag(cone)`, \[3 bytes + `flag(w < 0)`\], `writeCompressedPoint(pos, 0.5)` |
//! | `PathManagerEvent` | any | ordered | `u32`, `flag`, `u32`, `u32 count`, per entry 3 + 4 `F32` and two words |
//! | `LightningStrikeEvent` | to client | ordered | `flag(lightning)` \[`ranged(index, 0, MaxGhostCount)`, `int(10)`, `int(10)`, `flag(target)` \[`ranged(index)`\]\] |
//!
//! `DB` is the datablock-id width ([`EventCtx::datablock_bits`], `DataBlockObjectIdBitSize`). The 3D audio quaternion bytes decode
//! UNSIGNED: a negative component is written as its low byte (`writeFloat(q, 8)`) and arrives positive.
//!
//! **Hostile input.** The path entries go through [`crate::cap`] ([`NET_PATH_ENTRIES`] and the bits left: a count past them is an
//! error, never an allocation; a game may clamp the count instead, [`PathCount`]). A decoder charges one unit per element.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/gameBase/gameConnectionEvents.cpp`
//! (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.), `scene/pathManager.cpp` and
//! `T3D/fx/lightning.cpp` (Copyright (c) 2012 GarageGames, LLC each); the code here is this crate's own.

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::cap::{self, NET_PATH_ENTRIES};
use crate::events::{ChargeFn, EventCtx, EventIds, WireEvent};
use crate::library;
use torque_bytes::pay;

/// The scale of a `Sim3DAudioEvent`'s compressed position (`SoundPosAccuracy`).
pub const AUDIO_POINT_SCALE: f32 = 0.5;

/// `DataBlockObjectIdFirst`: the wire carries a profile id less this.
pub const DATABLOCK_ID_FIRST: u32 = 3;

/// The mask of a `bits`-wide field (at most 32 bits).
fn mask(bits: u8) -> u32 {
    match bits.min(32) {
        32 => u32::MAX,
        b => (1u32 << b) - 1,
    }
}

fn write_profile(w: &mut BitWriter, profile: u32, bits: u8) {
    w.write_bits(profile.wrapping_sub(DATABLOCK_ID_FIRST), bits.min(32));
}

fn read_profile(r: &mut BitReader, bits: u8) -> Result<u32, NetError> {
    Ok((r.read_bits(bits.min(32))? & mask(bits)).wrapping_add(DATABLOCK_ID_FIRST))
}

/// `Sim2DAudioEvent` (server to client): the profile's object id, 3 less on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sim2DAudioEvent {
    pub profile: u32,
}

impl Sim2DAudioEvent {
    /// Writes the profile id in `datablock_bits` bits.
    pub fn pack(&self, w: &mut BitWriter, datablock_bits: u8) {
        write_profile(w, self.profile, datablock_bits);
    }

    pub fn unpack(r: &mut BitReader, datablock_bits: u8) -> Result<Sim2DAudioEvent, NetError> {
        Ok(Sim2DAudioEvent {
            profile: read_profile(r, datablock_bits)?,
        })
    }
}

impl WireEvent for Sim2DAudioEvent {
    fn class(&self, ids: &EventIds) -> u32 {
        ids.sim_2d_audio
    }

    fn pack(&self, w: &mut BitWriter, ctx: &EventCtx<'_>) {
        Sim2DAudioEvent::pack(self, w, ctx.datablock_bits);
    }
}

/// The orientation of a cone sound: the quaternion's x, y, z as bytes and the sign of w.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cone {
    pub xyz: [u8; 3],
    pub w_negative: bool,
}

impl Cone {
    /// What the packer writes for a normalised quaternion `[x, y, z, w]`: `writeFloat(q, 8)` per component, the LOW BYTE (a negative
    /// component wraps), and `flag(w < 0)`.
    pub fn from_quaternion(q: [f32; 4]) -> Cone {
        let byte = |v: f32| ((library::get().write_float)(v, 8) & 0xff) as u8;
        Cone {
            xyz: [byte(q[0]), byte(q[1]), byte(q[2])],
            w_negative: q[3] < 0.0,
        }
    }

    /// What the client rebuilds: `x = byte / 255` unsigned (`readFloat(8)`), `w = sqrt(1 - min(x² + y² + z², 1))`
    /// ([`crate::library::MathLibrary::cone_w`]), negated when the flag was set.
    pub fn quaternion(&self) -> [f32; 4] {
        let c = |b: u8| f32::from(b) / 255.0;
        let (x, y, z) = (c(self.xyz[0]), c(self.xyz[1]), c(self.xyz[2]));
        let w = (library::get().cone_w)(x, y, z);
        [x, y, z, if self.w_negative { -w } else { w }]
    }
}

/// `Sim3DAudioEvent` (server to client). The position is compressed against the stream's
/// compression point (the control object's position once the head has set it), at scale 0.5.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sim3DAudioEvent {
    pub profile: u32,
    /// Present when the profile's description has a cone (`coneInside` or `coneOutside` non-zero).
    pub cone: Option<Cone>,
    pub position: [f32; 3],
}

impl Sim3DAudioEvent {
    /// Writes the event, the profile id in `datablock_bits` bits.
    pub fn pack(&self, w: &mut BitWriter, datablock_bits: u8) {
        write_profile(w, self.profile, datablock_bits);
        w.write_flag(self.cone.is_some());
        if let Some(cone) = &self.cone {
            for b in cone.xyz {
                w.write_u8(b);
            }
            w.write_flag(cone.w_negative);
        }
        w.write_compressed_point(self.position, AUDIO_POINT_SCALE);
    }

    pub fn unpack(r: &mut BitReader, datablock_bits: u8) -> Result<Sim3DAudioEvent, NetError> {
        let profile = read_profile(r, datablock_bits)?;
        let cone = if r.read_flag()? {
            let xyz = [r.read_u8()?, r.read_u8()?, r.read_u8()?];
            Some(Cone {
                xyz,
                w_negative: r.read_flag()?,
            })
        } else {
            None
        };
        let position = r.read_compressed_point(AUDIO_POINT_SCALE)?;
        Ok(Sim3DAudioEvent {
            profile,
            cone,
            position,
        })
    }
}

impl WireEvent for Sim3DAudioEvent {
    fn class(&self, ids: &EventIds) -> u32 {
        ids.sim_3d_audio
    }

    fn pack(&self, w: &mut BitWriter, ctx: &EventCtx<'_>) {
        Sim3DAudioEvent::pack(self, w, ctx.datablock_bits);
    }
}

/// One path point of a `PathManagerEvent`: position (3 floats), rotation (4 floats), and two words (TGE's `msToNext` and
/// `smoothingType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PathEntry {
    pub position: [u32; 3],
    pub rotation: [u32; 4],
    pub time: u32,
    pub smoothing: u32,
}

/// The bits of one path entry.
pub const PATH_ENTRY_BITS: usize = 9 * 32;

/// `PathManagerEvent`: `u32 modifiedPath`, `flag clearPaths`, `u32 totalTime`, `u32 count`, then `count` entries. A world with no
/// paths sends [`PathManagerEvent::empty`], 97 bits.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PathManagerEvent {
    pub modified_path: u32,
    pub clear: bool,
    pub total_time: u32,
    pub entries: Vec<PathEntry>,
}

/// How a `PathManagerEvent` reader takes the entry count: past `max` it is an error, or with `clamp_signed` the word is read as
/// a signed count and clamped to `0..=max` first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathCount {
    pub max: usize,
    pub clamp_signed: bool,
}

impl Default for PathCount {
    fn default() -> PathCount {
        PathCount {
            max: NET_PATH_ENTRIES,
            clamp_signed: false,
        }
    }
}

impl PathManagerEvent {
    /// The form sent for a world with no paths: the three header values zero and no entries.
    pub fn empty() -> PathManagerEvent {
        PathManagerEvent::default()
    }

    pub fn pack(&self, w: &mut BitWriter) {
        w.write_u32(self.modified_path);
        w.write_flag(self.clear);
        w.write_u32(self.total_time);
        w.write_u32(self.entries.len() as u32);
        for e in &self.entries {
            for p in e.position {
                w.write_u32(p);
            }
            for q in e.rotation {
                w.write_u32(q);
            }
            w.write_u32(e.time);
            w.write_u32(e.smoothing);
        }
    }

    /// Reads the event; a count past [`NET_PATH_ENTRIES`] or past what the bits left hold is an error.
    pub fn unpack(r: &mut BitReader, charge: &mut ChargeFn) -> Result<PathManagerEvent, NetError> {
        PathManagerEvent::unpack_with(r, charge, PathCount::default())
    }

    /// [`PathManagerEvent::unpack`] with the count taken as `count` says (a count past what the bits left hold is still an error).
    pub fn unpack_with(
        r: &mut BitReader,
        charge: &mut ChargeFn,
        count: PathCount,
    ) -> Result<PathManagerEvent, NetError> {
        let modified_path = r.read_u32()?;
        let clear = r.read_flag()?;
        let total_time = r.read_u32()?;
        let word = r.read_u32()?;
        let wanted = if count.clamp_signed {
            (word as i32).clamp(0, i32::try_from(count.max).unwrap_or(i32::MAX)) as u64
        } else {
            u64::from(word)
        };
        let count = cap::count(
            "path entries",
            wanted,
            count.max,
            r.bits_left(),
            PATH_ENTRY_BITS,
        )?;
        pay(charge, count as u64 * 36)?;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let mut e = PathEntry::default();
            for p in &mut e.position {
                *p = r.read_u32()?;
            }
            for q in &mut e.rotation {
                *q = r.read_u32()?;
            }
            e.time = r.read_u32()?;
            e.smoothing = r.read_u32()?;
            entries.push(e);
        }
        Ok(PathManagerEvent {
            modified_path,
            clear,
            total_time,
            entries,
        })
    }
}

impl WireEvent for PathManagerEvent {
    fn class(&self, ids: &EventIds) -> u32 {
        ids.path_manager
    }

    fn pack(&self, w: &mut BitWriter, _ctx: &EventCtx<'_>) {
        PathManagerEvent::pack(self, w);
    }
}

/// `LightningStrikeEvent` (server to client): `flag`, and when the Lightning has a ghost on the
/// connection its index as `ranged(index, 0, MaxGhostCount)` (GB + 1 bits: the width follows the connection's ghost table), two
/// 10-bit values, then `flag` and the target's index the same way when it has a ghost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LightningStrikeEvent {
    /// The Lightning's ghost index; None packs a lone clear flag.
    pub lightning: Option<u32>,
    pub x: u32,
    pub y: u32,
    /// The struck object's ghost index.
    pub target: Option<u32>,
}

impl LightningStrikeEvent {
    pub fn pack(&self, w: &mut BitWriter, ghost_bits: u8) {
        let max = 1u32 << ghost_bits.min(31);
        let Some(lightning) = self.lightning else {
            w.write_flag(false);
            return;
        };
        w.write_flag(true);
        w.write_ranged_u32(lightning, 0, max);
        w.write_bits(self.x, 10);
        w.write_bits(self.y, 10);
        w.write_flag(self.target.is_some());
        if let Some(target) = self.target {
            w.write_ranged_u32(target, 0, max);
        }
    }

    pub fn unpack(r: &mut BitReader, ghost_bits: u8) -> Result<LightningStrikeEvent, NetError> {
        let max = 1u32 << ghost_bits.min(31);
        if !r.read_flag()? {
            return Ok(LightningStrikeEvent::default());
        }
        let lightning = Some(r.read_ranged_u32(0, max)?);
        let x = r.read_bits(10)?;
        let y = r.read_bits(10)?;
        let target = if r.read_flag()? {
            Some(r.read_ranged_u32(0, max)?)
        } else {
            None
        };
        Ok(LightningStrikeEvent {
            lightning,
            x,
            y,
            target,
        })
    }
}

impl WireEvent for LightningStrikeEvent {
    fn class(&self, ids: &EventIds) -> u32 {
        ids.lightning_strike
    }

    fn pack(&self, w: &mut BitWriter, ctx: &EventCtx<'_>) {
        LightningStrikeEvent::pack(self, w, ctx.ghost_bits);
    }
}

/// Any of the events of this module.
#[derive(Debug, Clone, PartialEq)]
pub enum GameEvent {
    Audio2D(Sim2DAudioEvent),
    Audio3D(Sim3DAudioEvent),
    PathManager(PathManagerEvent),
    LightningStrike(LightningStrikeEvent),
}

// The float fields make the derive impossible; a NaN position is never compared by identity (the host's event type is `Eq` for its queues).
impl Eq for GameEvent {}

impl GameEvent {
    /// Reads the body of the event of class `class`; `Ok(None)` when `class` is none of these (the caller tries another decoder).
    pub fn unpack(
        class: u32,
        r: &mut BitReader,
        ctx: &EventCtx<'_>,
        charge: &mut ChargeFn,
    ) -> Result<Option<GameEvent>, NetError> {
        let ids = ctx.ids;
        Ok(Some(if class == ids.sim_2d_audio {
            GameEvent::Audio2D(Sim2DAudioEvent::unpack(r, ctx.datablock_bits)?)
        } else if class == ids.sim_3d_audio {
            GameEvent::Audio3D(Sim3DAudioEvent::unpack(r, ctx.datablock_bits)?)
        } else if class == ids.path_manager {
            GameEvent::PathManager(PathManagerEvent::unpack(r, charge)?)
        } else if class == ids.lightning_strike {
            GameEvent::LightningStrike(LightningStrikeEvent::unpack(r, ctx.ghost_bits)?)
        } else {
            return Ok(None);
        }))
    }
}

impl WireEvent for GameEvent {
    fn class(&self, ids: &EventIds) -> u32 {
        match self {
            GameEvent::Audio2D(e) => e.class(ids),
            GameEvent::Audio3D(e) => e.class(ids),
            GameEvent::PathManager(e) => e.class(ids),
            GameEvent::LightningStrike(e) => e.class(ids),
        }
    }

    fn pack(&self, w: &mut BitWriter, ctx: &EventCtx<'_>) {
        match self {
            GameEvent::Audio2D(e) => Sim2DAudioEvent::pack(e, w, ctx.datablock_bits),
            GameEvent::Audio3D(e) => Sim3DAudioEvent::pack(e, w, ctx.datablock_bits),
            GameEvent::PathManager(e) => PathManagerEvent::pack(e, w),
            GameEvent::LightningStrike(e) => LightningStrikeEvent::pack(e, w, ctx.ghost_bits),
        }
    }
}

#[cfg(test)]
mod tests;
