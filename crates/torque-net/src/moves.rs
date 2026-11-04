//! Moves: the client's input, `Move`, the client-to-server move block, the server's bounded move list and
//! the client's resending queue.
//!
//! **Wire.** One move: per angle (yaw, pitch, roll) `flag(p != 0)` and, if set, `int(16, p)`; then three `int(6)` axes (x, y, z);
//! `flag(free_look)`; one `flag(trigger)` per trigger key of the [`MoveFormat`] (Torque's `MaxTriggerKeys`, 6, unless a game sets
//! fewer): 28 bits at least, 76 at most with six. The unpack clamps each axis to `0..=32` (six bits carry
//! up to 63; TGE did not clamp). A block (after the head's camera flag and 32-bit control checksum): `int(32) start`, `int(5)
//! count` (so up to 31), the moves. The fov flag and byte follow it.
//!
//! **Conversion** (`Move::unclamp`, what the server's `processTick` sees). Angle: `IANG2FANG`, `2 pi / 65536` times the 16 bits
//! as a signed number ([`crate::library::MathLibrary::move_angle`]). Axis: `(raw - 16) / 16` (exact).
//!
//! **Reader** ([`MoveList::read_block`]). `skip = lastMoveAck - start` as a signed 32-bit number. Negative: `lastMoveAck =
//! start` (a client may jump), nothing skipped. Otherwise the first `min(skip, count)` moves are read and dropped and `start`
//! advances by that. The rest are appended with `id = start + i` and `lastMoveAck += count`. The ack the server sends back is
//! `lastMoveAck - list size` ([`MoveList::ack`]).
//!
//! **Bounded list.** An unbounded list would let a client grow it without limit; this one holds [`NET_MOVE_LIST`] and drops the newest when full (see the constant). A block
//! is parsed completely before it changes anything, so an overrun leaves the list as it was.
//!
//! **Hostile input.** Counts are 5 bits; nothing indexes with a wire value; the reader charges one unit per move.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/gameBase/moveManager.cpp`,
//! `T3D/gameBase/moveList.cpp` (Copyright (c) 2012 GarageGames, LLC each) and `T3D/gameBase/gameConnection.cpp` (Copyright (c)
//! 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use std::collections::VecDeque;

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::cap::{NET_MOVE_LIST, NET_MOVE_SENDS, NET_MOVES_PER_PACKET, NET_MOVES_WRITE_MAX};
use crate::events::ChargeFn;

/// Torque's trigger keys (`MaxTriggerKeys`): the most a move holds.
pub const MAX_TRIGGER_KEYS: usize = 6;

/// What a game changes about a move on the wire: the trigger flags it carries (the first `trigger_keys` of
/// [`Move::triggers`]; at most [`MAX_TRIGGER_KEYS`]). The default is Torque's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveFormat {
    pub trigger_keys: usize,
}

impl Default for MoveFormat {
    fn default() -> MoveFormat {
        MoveFormat {
            trigger_keys: MAX_TRIGGER_KEYS,
        }
    }
}

impl MoveFormat {
    /// The trigger flags on the wire.
    pub fn triggers(&self) -> usize {
        self.trigger_keys.min(MAX_TRIGGER_KEYS)
    }

    /// The fewest and most bits one move takes.
    pub fn min_bits(&self) -> usize {
        3 + 18 + 1 + self.triggers()
    }

    pub fn max_bits(&self) -> usize {
        3 * 17 + 18 + 1 + self.triggers()
    }
}

/// The largest value of a move axis after the clamp.
pub const MOVE_AXIS_MAX: u32 = 32;

/// The fewest and most bits one move of Torque's format takes on the wire.
pub const MOVE_MIN_BITS: usize = 3 + 18 + 1 + MAX_TRIGGER_KEYS;
pub const MOVE_MAX_BITS: usize = 3 * 17 + 18 + 1 + MAX_TRIGGER_KEYS;

/// One move. The raw fields are what the wire carries; the floats are `processTick`'s inputs, derived by [`Move::finish`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Move {
    /// Axis values 0..=32 (16 is zero).
    pub px: u32,
    pub py: u32,
    pub pz: u32,
    /// Angles as 16-bit values (signed when converted).
    pub pyaw: u32,
    pub ppitch: u32,
    pub proll: u32,
    pub free_look: bool,
    pub triggers: [bool; MAX_TRIGGER_KEYS],
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    /// `start + i` once the server has read it; not on the wire.
    pub id: u32,
    pub send_count: u8,
}

fn angle(p: u32) -> f32 {
    (crate::library::get().move_angle)(p as u16)
}

fn axis(raw: u32) -> f32 {
    (raw as i32 - 16) as f32 / 16.0
}

impl Move {
    /// A move from raw values; axes are clamped to `0..=32` and angles to 16 bits, then the floats are derived.
    pub fn from_raw(
        axes: [u32; 3],
        angles: [u32; 3],
        free_look: bool,
        triggers: [bool; MAX_TRIGGER_KEYS],
    ) -> Move {
        let mut m = Move {
            px: axes[0],
            py: axes[1],
            pz: axes[2],
            pyaw: angles[0],
            ppitch: angles[1],
            proll: angles[2],
            free_look,
            triggers,
            ..Move::default()
        };
        m.finish();
        m
    }

    /// Clamps the raw axes to `0..=32`, masks the angles to 16 bits and derives the floats.
    pub fn finish(&mut self) {
        self.px = self.px.min(MOVE_AXIS_MAX);
        self.py = self.py.min(MOVE_AXIS_MAX);
        self.pz = self.pz.min(MOVE_AXIS_MAX);
        self.pyaw &= 0xffff;
        self.ppitch &= 0xffff;
        self.proll &= 0xffff;
        self.x = axis(self.px);
        self.y = axis(self.py);
        self.z = axis(self.pz);
        self.yaw = angle(self.pyaw);
        self.pitch = angle(self.ppitch);
        self.roll = angle(self.proll);
    }

    /// `Move::pack` in Torque's format.
    pub fn pack(&self, w: &mut BitWriter) {
        self.pack_with(w, MoveFormat::default());
    }

    /// `Move::pack` in `format`.
    pub fn pack_with(&self, w: &mut BitWriter, format: MoveFormat) {
        for p in [self.pyaw, self.ppitch, self.proll] {
            w.write_flag(p != 0);
            if p != 0 {
                w.write_bits(p, 16);
            }
        }
        for p in [self.px, self.py, self.pz] {
            w.write_bits(p, 6);
        }
        w.write_flag(self.free_look);
        for &t in self.triggers.iter().take(format.triggers()) {
            w.write_flag(t);
        }
    }

    /// `Move::unpack` in Torque's format: the clamp and the conversion included. `id` and `send_count` are left 0.
    pub fn unpack(r: &mut BitReader) -> Result<Move, NetError> {
        Move::unpack_with(r, MoveFormat::default())
    }

    /// `Move::unpack` in `format` (the triggers it does not carry are clear).
    pub fn unpack_with(r: &mut BitReader, format: MoveFormat) -> Result<Move, NetError> {
        let mut angles = [0u32; 3];
        for a in &mut angles {
            if r.read_flag()? {
                *a = r.read_bits(16)?;
            }
        }
        let mut axes = [0u32; 3];
        for a in &mut axes {
            *a = r.read_bits(6)?;
        }
        let free_look = r.read_flag()?;
        let mut triggers = [false; MAX_TRIGGER_KEYS];
        for t in triggers.iter_mut().take(format.triggers()) {
            *t = r.read_flag()?;
        }
        Ok(Move::from_raw(axes, angles, free_look, triggers))
    }

    /// Reads one move and drops it.
    pub fn skip(r: &mut BitReader) -> Result<(), NetError> {
        Move::unpack(r).map(|_| ())
    }
}

/// A move block as it is on the wire: the start index and the moves.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MoveBlock {
    pub start: u32,
    pub moves: Vec<Move>,
}

impl MoveBlock {
    /// `int(32) start`, `int(5) count`, the moves in Torque's format. At most [`NET_MOVES_PER_PACKET`] go out.
    pub fn write(&self, w: &mut BitWriter) {
        self.write_with(w, MoveFormat::default());
    }

    /// [`MoveBlock::write`] in `format`.
    pub fn write_with(&self, w: &mut BitWriter, format: MoveFormat) {
        let count = self.moves.len().min(NET_MOVES_PER_PACKET);
        w.write_u32(self.start);
        w.write_bits(count as u32, 5);
        for m in self.moves.iter().take(count) {
            m.pack_with(w, format);
        }
    }

    /// Reads a block in Torque's format completely or not at all. Charges one unit per move.
    pub fn read(r: &mut BitReader, charge: &mut ChargeFn) -> Result<MoveBlock, NetError> {
        MoveBlock::read_with(r, MoveFormat::default(), charge)
    }

    /// [`MoveBlock::read`] in `format`.
    pub fn read_with(
        r: &mut BitReader,
        format: MoveFormat,
        charge: &mut ChargeFn,
    ) -> Result<MoveBlock, NetError> {
        let start = r.read_u32()?;
        let count = r.read_bits(5)? as usize;
        let mut moves = Vec::with_capacity(count.min(NET_MOVES_PER_PACKET));
        for _ in 0..count {
            if !charge(1) {
                return Err(NetError::Invalid("work budget"));
            }
            moves.push(Move::unpack_with(r, format)?);
        }
        Ok(MoveBlock { start, moves })
    }
}

/// What reading a block did to the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Absorbed {
    /// Moves appended to the list.
    pub appended: usize,
    /// Moves read and dropped as already acknowledged.
    pub skipped: usize,
    /// Moves dropped because the list was full.
    pub dropped: usize,
    /// The block's start was ahead of `lastMoveAck` (a negative signed difference): `lastMoveAck` jumped to it.
    pub jumped: bool,
}

/// The server's list of a connection's moves (a GameConnection's move list).
#[derive(Debug, Clone, Default)]
pub struct MoveList {
    last_move_ack: u32,
    moves: VecDeque<Move>,
    format: MoveFormat,
}

impl MoveList {
    /// A list whose blocks are read in Torque's format.
    pub fn new() -> MoveList {
        MoveList::default()
    }

    /// A list whose blocks are read in `format`.
    pub fn with_format(format: MoveFormat) -> MoveList {
        MoveList {
            format,
            ..MoveList::default()
        }
    }

    pub fn format(&self) -> MoveFormat {
        self.format
    }

    pub fn last_move_ack(&self) -> u32 {
        self.last_move_ack
    }

    pub fn len(&self) -> usize {
        self.moves.len()
    }

    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    pub fn moves(&self) -> impl Iterator<Item = &Move> {
        self.moves.iter()
    }

    /// The ack the server's head carries: `lastMoveAck - size` (wrapping).
    pub fn ack(&self) -> u32 {
        self.last_move_ack.wrapping_sub(self.moves.len() as u32)
    }

    /// Applies a parsed block.
    pub fn absorb(&mut self, block: &MoveBlock) -> Absorbed {
        let mut out = Absorbed::default();
        let mut start = block.start;
        let mut rest: &[Move] = &block.moves;
        let skip = self.last_move_ack.wrapping_sub(start) as i32;
        if skip < 0 {
            self.last_move_ack = start;
            out.jumped = true;
        } else {
            let n = (skip as usize).min(rest.len());
            out.skipped = n;
            rest = rest.get(n..).unwrap_or(&[]);
            start = start.wrapping_add(n as u32);
        }
        for (i, m) in rest.iter().enumerate() {
            if self.moves.len() >= NET_MOVE_LIST {
                out.dropped += 1;
                continue;
            }
            let mut m = *m;
            m.id = start.wrapping_add(i as u32);
            m.send_count = 0;
            self.moves.push_back(m);
            out.appended += 1;
        }
        self.last_move_ack = self.last_move_ack.wrapping_add(rest.len() as u32);
        out
    }

    /// Reads a block from the stream and applies it.
    pub fn read_block(
        &mut self,
        r: &mut BitReader,
        charge: &mut ChargeFn,
    ) -> Result<Absorbed, NetError> {
        let block = MoveBlock::read_with(r, self.format, charge)?;
        Ok(self.absorb(&block))
    }

    /// `getMoveList` (server): `n = min(size, credit)`, `credit -= n`, the list is cut to its first `n` (the excess is lost).
    /// Returns the moves to process.
    pub fn take_for_tick(&mut self, credit: &mut u32) -> Vec<Move> {
        let n = self.moves.len().min(*credit as usize);
        *credit -= n as u32;
        self.moves.truncate(n);
        self.moves.iter().copied().collect()
    }

    /// `clearMoves(n)`: drops the first `n`.
    pub fn clear_moves(&mut self, n: usize) {
        let n = n.min(self.moves.len());
        self.moves.drain(..n);
    }

    /// The list is emptied (a connection with no control object, every tick).
    pub fn clear(&mut self) {
        self.moves.clear();
    }
}

/// The client's queue of unacknowledged moves and its writer (TGE `moveWritePacket`, but the block has no leading ack).
#[derive(Debug, Clone, Default)]
pub struct MoveQueue {
    first_index: u32,
    moves: Vec<Move>,
}

impl MoveQueue {
    pub fn new() -> MoveQueue {
        MoveQueue::default()
    }

    /// A queue holding `moves` from move `first_index` on (a client's list rebuilt from its packets).
    pub fn from_parts(first_index: u32, moves: Vec<Move>) -> MoveQueue {
        MoveQueue { first_index, moves }
    }

    /// The index of the first move still held (`mFirstMoveIndex`).
    pub fn first_index(&self) -> u32 {
        self.first_index
    }

    pub fn len(&self) -> usize {
        self.moves.len()
    }

    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    /// The moves held, the first being move [`first_index`](Self::first_index) (the client's prediction runs them).
    pub fn moves(&self) -> &[Move] {
        &self.moves
    }

    /// Queues a move (the unacknowledged moves are bounded by [`NET_MOVE_LIST`]: the newest is refused when full).
    pub fn push(&mut self, mut m: Move) -> bool {
        if self.moves.len() >= NET_MOVE_LIST {
            return false;
        }
        m.send_count = 0;
        self.moves.push(m);
        true
    }

    /// The server's ack: drops the moves before it.
    pub fn on_server_ack(&mut self, ack: u32) {
        let behind = ack.wrapping_sub(self.first_index);
        if (behind as i32) <= 0 {
            return;
        }
        let n = (behind as usize).min(self.moves.len());
        self.moves.drain(..n);
        self.first_index = ack;
    }

    /// The block for the next packet: starts at the first move sent fewer than [`NET_MOVE_SENDS`] times (the last one if all were),
    /// at most [`NET_MOVES_WRITE_MAX`] moves, each one's send count rising.
    pub fn block(&mut self) -> MoveBlock {
        let count = self.moves.len();
        let mut offset = self
            .moves
            .iter()
            .position(|m| m.send_count < NET_MOVE_SENDS)
            .unwrap_or(count);
        if offset == count && count != 0 {
            offset -= 1;
        }
        let n = (count - offset).min(NET_MOVES_WRITE_MAX);
        let mut moves = Vec::with_capacity(n);
        for m in self.moves.iter_mut().skip(offset).take(n) {
            m.send_count = m.send_count.saturating_add(1);
            moves.push(*m);
        }
        MoveBlock {
            start: self.first_index.wrapping_add(offset as u32),
            moves,
        }
    }
}

#[cfg(test)]
mod tests;
