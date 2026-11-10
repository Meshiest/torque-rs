//! What goes over the `props-frozen` channel: the static state of a frozen prop, and the end of one.
//!
//! A message is a run of operations, each a tag byte and little-endian fields:
//!
//! | op | tag | fields | bytes |
//! |---|---|---|---|
//! | set | 1 | slot `u16`, datablock `u32`, flags `u8` (bit 0: the receiving client owns it), position 3 x `i32` (1/1024 units), the rotation's largest-component index `u8`, the other three components 3 x `u16` | 27 |
//! | remove | 2 | slot `u16` | 3 |
//!
//! A prop is known by its slot, the key both sides give its collision shape (the same number a ghost's record carries), so a
//! client that holds a prop both ways can tell it is one prop. [`read`] takes bytes a server sent: it checks every field
//! against its range, allocates only for the operations the bytes can hold, and gives either all of a message's operations or
//! none.

use crate::quant::Quantized;
use crate::world::MAX_PROPS;

/// The tag of an op that sets a prop's state.
pub const TAG_SET: u8 = 1;
/// The tag of an op that removes a prop.
pub const TAG_REMOVE: u8 = 2;
/// The bytes of a set op.
pub const SET_BYTES: usize = 27;
/// The bytes of a remove op.
pub const REMOVE_BYTES: usize = 3;
/// The most ops a message holds: a server writes up to [`crate::frozen::MAX_OPS_PER_SEND`] and a client refuses more than this.
pub const MAX_MESSAGE_OPS: usize = 8192;
/// The longest message a client accepts.
pub const MAX_MESSAGE_BYTES: usize = MAX_MESSAGE_OPS * SET_BYTES;

const FLAG_MINE: u8 = 1;

/// What a client is told of a frozen prop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frozen {
    /// The `PropData` datablock's id.
    pub data: u32,
    /// Whether the client it is sent to owns it.
    pub mine: bool,
    /// Where it stands, at the wire's precision (stopped: a frozen prop does not move).
    pub q: Quantized,
}

/// One operation of a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// The prop in `slot` is this (a new one, or the same one again with another state).
    Set { slot: u16, prop: Frozen },
    /// The prop in `slot` is gone from the channel.
    Remove { slot: u16 },
}

impl Op {
    /// The slot the op is about.
    pub fn slot(&self) -> u16 {
        match *self {
            Op::Set { slot, .. } | Op::Remove { slot } => slot,
        }
    }
}

/// Why a message was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageError {
    /// An op ran past the end of the message.
    Truncated,
    /// A tag that is no op.
    Tag(u8),
    /// A slot past the most props a server holds.
    Slot(u16),
    /// Flag bits that mean nothing.
    Flags(u8),
    /// A state outside the wire's ranges.
    Range,
    /// More ops than a message may hold.
    TooMany,
}

impl std::fmt::Display for MessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageError::Truncated => f.write_str("an op runs past the end of the message"),
            MessageError::Tag(t) => write!(f, "{t} is no op"),
            MessageError::Slot(s) => write!(f, "slot {s} is past the most props a server holds"),
            MessageError::Flags(b) => write!(f, "flags {b:#04x} mean nothing"),
            MessageError::Range => f.write_str("a state is outside the wire's ranges"),
            MessageError::TooMany => write!(f, "more than {MAX_MESSAGE_OPS} ops"),
        }
    }
}

impl std::error::Error for MessageError {}

/// The bytes of `ops`.
#[must_use]
pub fn write(ops: &[Op]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ops.len() * SET_BYTES);
    for op in ops {
        match *op {
            Op::Set { slot, prop } => {
                out.push(TAG_SET);
                out.extend(slot.to_le_bytes());
                out.extend(prop.data.to_le_bytes());
                out.push(if prop.mine { FLAG_MINE } else { 0 });
                for v in prop.q.pos {
                    out.extend(v.to_le_bytes());
                }
                out.push(prop.q.rot_index);
                for v in prop.q.rot {
                    out.extend(v.to_le_bytes());
                }
            }
            Op::Remove { slot } => {
                out.push(TAG_REMOVE);
                out.extend(slot.to_le_bytes());
            }
        }
    }
    out
}

/// A cursor over a message's bytes.
struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], MessageError> {
        let (head, rest) = self
            .0
            .split_first_chunk::<N>()
            .ok_or(MessageError::Truncated)?;
        self.0 = rest;
        Ok(*head)
    }

    fn u8(&mut self) -> Result<u8, MessageError> {
        self.take::<1>().map(|[b]| b)
    }

    fn u16(&mut self) -> Result<u16, MessageError> {
        self.take().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32, MessageError> {
        self.take().map(u32::from_le_bytes)
    }

    fn i32(&mut self) -> Result<i32, MessageError> {
        self.take().map(i32::from_le_bytes)
    }
}

fn slot(c: &mut Cursor<'_>) -> Result<u16, MessageError> {
    let slot = c.u16()?;
    if u32::from(slot) >= MAX_PROPS {
        return Err(MessageError::Slot(slot));
    }
    Ok(slot)
}

/// The ops of a message from a server.
///
/// # Errors
///
/// A [`MessageError`] for the first thing wrong with it; no op is given then.
pub fn read(bytes: &[u8]) -> Result<Vec<Op>, MessageError> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(MessageError::TooMany);
    }
    let mut c = Cursor(bytes);
    let mut ops = Vec::with_capacity(bytes.len() / SET_BYTES + 1);
    while !c.0.is_empty() {
        if ops.len() >= MAX_MESSAGE_OPS {
            return Err(MessageError::TooMany);
        }
        ops.push(match c.u8()? {
            TAG_SET => {
                let slot = slot(&mut c)?;
                let data = c.u32()?;
                let flags = c.u8()?;
                if flags & !FLAG_MINE != 0 {
                    return Err(MessageError::Flags(flags));
                }
                let q = Quantized {
                    pos: [c.i32()?, c.i32()?, c.i32()?],
                    rot_index: c.u8()?,
                    rot: [c.u16()?, c.u16()?, c.u16()?],
                    ..Quantized::default()
                };
                if q != q.held() {
                    return Err(MessageError::Range);
                }
                Op::Set {
                    slot,
                    prop: Frozen {
                        data,
                        mine: flags & FLAG_MINE != 0,
                        q,
                    },
                }
            }
            TAG_REMOVE => Op::Remove {
                slot: slot(&mut c)?,
            },
            tag => return Err(MessageError::Tag(tag)),
        });
    }
    Ok(ops)
}
