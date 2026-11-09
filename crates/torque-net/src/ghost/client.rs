//! A reference ghost client: the reader of the ghost section of a data packet and of `GhostAlwaysObjectEvent`'s body
//! (`NetConnection::ghostReadPacket` and the event's unpack), over a pluggable record codec. It exists to decode
//! this crate's own packets in tests and in the datagram decoder; the host supplies the real codec.
//!
//! Every count is read against the bits left; an error is a value, never a panic.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `sim/netGhost.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

use std::collections::BTreeMap;

use crate::NetError;
use crate::bits::BitReader;
use crate::cap::NET_GHOST_LIMIT;

/// How a NewGhost record names its class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassForm {
    /// The ghost policy's implied class (`flag(1)` and no class id; only where the records carry the implied-class flag, see
    /// [`crate::ghost::GhostPolicy::implied_class_flag`]).
    Implied,
    /// An explicit class id (`NetClassBitSize` bits).
    Id(u32),
}

/// The record bodies: what the object codec reads after the framing.
pub trait GhostCodec {
    type Object;
    /// A NewGhost record or a ghost-always event: builds the object and reads its first `unpackUpdate`.
    fn create(&mut self, class: ClassForm, r: &mut BitReader) -> Result<Self::Object, NetError>;
    /// The object the client registers for the empty form of a `GhostAlwaysObjectEvent` (a
    /// placeholder NetObject is registered at that index, so the slot is taken).
    fn placeholder(&mut self) -> Self::Object;
    /// An update record for an existing ghost.
    fn update(&mut self, object: &mut Self::Object, r: &mut BitReader) -> Result<(), NetError>;
}

/// What one section held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SectionReport {
    /// `flag(mGhosting)` was 1.
    pub ghosting: bool,
    pub created: usize,
    pub updated: usize,
    pub deleted: usize,
    /// Indices in the order the records came.
    pub first_index: Option<u32>,
}

/// One change to the table, in the order it happened (kept only after [`GhostClient::enable_log`]): what a client world replays
/// after each section and event to follow the objects without walking the whole table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhostChange {
    /// A NewGhost record, a ghost-always event (either form: the empty one registers a placeholder) created the object at the index.
    Created(u32),
    /// An update record was applied to it.
    Updated(u32),
    Deleted(u32),
    /// Every ghost was forgotten ([`GhostClient::clear`]).
    Cleared,
}

/// The client's ghost table (`mLocalGhosts`).
#[derive(Debug)]
pub struct GhostClient<C: GhostCodec> {
    codec: C,
    ghosts: BTreeMap<u32, C::Object>,
    class_bits: u8,
    ghost_bits: u8,
    /// NewGhost records carry the implied-class flag ahead of the class id.
    implied_flag: bool,
    /// The changes since the last [`GhostClient::take_log`], when enabled.
    log: Option<Vec<GhostChange>>,
    /// The ghost index of the record read last (still set when its decode failed: the error's ghost id).
    last_index: Option<u32>,
}

impl<C: GhostCodec> GhostClient<C> {
    /// `class_bits` is the object group's class id width (6); `ghost_bits` the ghost index width of events (12 at the default ghost limit, [`crate::profile::DEFAULT_GHOST_LIMIT`]).
    pub fn new(codec: C, class_bits: u8, ghost_bits: u8) -> GhostClient<C> {
        GhostClient {
            codec,
            ghosts: BTreeMap::new(),
            class_bits,
            ghost_bits,
            implied_flag: false,
            log: None,
            last_index: None,
        }
    }

    /// NewGhost records carry the implied-class flag ahead of the class id (the server's ghost policy says so; off in Torque).
    pub fn set_implied_class_flag(&mut self, on: bool) {
        self.implied_flag = on;
    }

    /// The size of the connection's ghost table, `2^ghost_bits` (at most [`NET_GHOST_LIMIT`]): every index it holds is below it.
    pub fn table_size(&self) -> u32 {
        let bits = self.ghost_bits.clamp(1, crate::cap::NET_MAX_GHOST_BITS);
        (1u32 << bits).min(NET_GHOST_LIMIT)
    }

    /// The ghost index of the record read last, or being read when a read failed.
    pub fn last_index(&self) -> Option<u32> {
        self.last_index
    }

    /// Starts keeping the changes ([`GhostChange`]); the caller drains them with [`GhostClient::take_log`].
    pub fn enable_log(&mut self) {
        self.log.get_or_insert_with(Vec::new);
    }

    /// The changes since the last call (empty when the log is off).
    pub fn take_log(&mut self) -> Vec<GhostChange> {
        self.log.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// The changes kept since the last [`GhostClient::take_log`] (None when the log is off).
    pub fn log(&self) -> Option<&[GhostChange]> {
        self.log.as_deref()
    }

    fn note(&mut self, change: GhostChange) {
        if let Some(log) = &mut self.log {
            log.push(change);
        }
    }

    pub fn codec(&self) -> &C {
        &self.codec
    }

    pub fn codec_mut(&mut self) -> &mut C {
        &mut self.codec
    }

    pub fn len(&self) -> usize {
        self.ghosts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ghosts.is_empty()
    }

    pub fn ghost(&self, index: u32) -> Option<&C::Object> {
        self.ghosts.get(&index)
    }

    pub fn indices(&self) -> impl Iterator<Item = u32> + '_ {
        self.ghosts.keys().copied()
    }

    /// Forgets every ghost (the client's side of a reset).
    pub fn clear(&mut self) {
        self.ghosts.clear();
        self.note(GhostChange::Cleared);
    }

    /// The body of a `GhostAlwaysObjectEvent` (after the class id): `int(ghost_bits) index`, `flag(exists)`, and when it does
    /// `int(class_bits)` and the full record. **The empty form** (the object was gone when the event was written, or the event is stale
    /// after a reset) returns `None`, and registers a placeholder object at the index
    /// (`GhostCodec::placeholder`), so later records for that index parse against it.
    pub fn read_always_event(&mut self, r: &mut BitReader) -> Result<Option<u32>, NetError> {
        let index = r.read_bits(self.ghost_bits)?;
        self.last_index = Some(index);
        if !r.read_flag()? {
            self.check_room(index)?;
            let placeholder = self.codec.placeholder();
            self.ghosts.insert(index, placeholder);
            self.note(GhostChange::Created(index));
            return Ok(None);
        }
        let class = r.read_bits(self.class_bits)?;
        self.insert_new(index, ClassForm::Id(class), r)?;
        Ok(Some(index))
    }

    fn insert_new(
        &mut self,
        index: u32,
        class: ClassForm,
        r: &mut BitReader,
    ) -> Result<(), NetError> {
        self.check_room(index)?;
        let object = self.codec.create(class, r)?;
        self.ghosts.insert(index, object);
        self.note(GhostChange::Created(index));
        Ok(())
    }

    /// Whether a ghost may be entered at `index`: the connection's ghost table holds `2^ghost_bits` (24 bits at most), so an
    /// index past it is a hostile server's, and so is a ghost past its count (a real one or a placeholder alike).
    fn check_room(&self, index: u32) -> Result<(), NetError> {
        let limit = self.table_size();
        if index >= limit {
            return Err(NetError::Cap {
                what: "client ghost index",
                count: u64::from(index) + 1,
                cap: u64::from(limit),
            });
        }
        if self.ghosts.len() >= limit as usize {
            return Err(NetError::Cap {
                what: "client ghosts",
                count: self.ghosts.len() as u64 + 1,
                cap: u64::from(limit),
            });
        }
        Ok(())
    }

    /// The ghost section: `flag(mGhosting)`, then (when set) `int(5) + 3` for the id width, and the records up to the closing
    /// `flag(0)`. A delete of an absent ghost is an error (`Attempting to delete ghost %d, NULL ghost encountered`).
    pub fn read_section(&mut self, r: &mut BitReader) -> Result<SectionReport, NetError> {
        let mut report = SectionReport::default();
        if !r.read_flag()? {
            return Ok(report);
        }
        report.ghosting = true;
        let id_bits = (r.read_bits(5)? + 3) as u8;
        while r.read_flag()? {
            let index = r.read_bits(id_bits)?;
            self.last_index = Some(index);
            report.first_index.get_or_insert(index);
            if r.read_flag()? {
                if self.ghosts.remove(&index).is_none() {
                    return Err(NetError::Invalid("delete of an absent ghost"));
                }
                self.note(GhostChange::Deleted(index));
                report.deleted += 1;
            } else if let Some(object) = self.ghosts.get_mut(&index) {
                self.codec.update(object, r)?;
                self.note(GhostChange::Updated(index));
                report.updated += 1;
            } else {
                let class = if self.implied_flag && r.read_flag()? {
                    ClassForm::Implied
                } else {
                    ClassForm::Id(r.read_bits(self.class_bits)?)
                };
                self.insert_new(index, class, r)?;
                report.created += 1;
            }
        }
        Ok(report)
    }
}

/// An event the reference client read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    Message(crate::events::ConnectionMessage),
    /// `GhostAlwaysObjectEvent`: the index it created (`None` for the empty form).
    AlwaysObject(Option<u32>),
}

/// An [`crate::events::EventUnpacker`] that reads `ConnectionMessage` and `GhostAlwaysObjectEvent` and feeds the second to a
/// [`GhostClient`] at once, as a client does when the event runs. (An ordered event is read when the wire is read; the order of
/// running is the receiver's.)
pub struct GhostEventReader<'a, C: GhostCodec> {
    pub client: &'a mut GhostClient<C>,
}

impl<C: GhostCodec> crate::events::EventUnpacker for GhostEventReader<'_, C> {
    type Event = ClientEvent;

    fn unpack(
        &mut self,
        class: u32,
        r: &mut BitReader,
        ctx: &crate::events::EventCtx<'_>,
    ) -> Result<ClientEvent, NetError> {
        if class == ctx.ids.connection_message {
            crate::events::ConnectionMessage::unpack(r, ctx).map(ClientEvent::Message)
        } else if class == ctx.ids.ghost_always_object {
            self.client
                .read_always_event(r)
                .map(ClientEvent::AlwaysObject)
        } else {
            Err(NetError::Invalid("event class has no decoder"))
        }
    }
}
