//! What a plugin registers for a bulk channel: the logic ([`ChunkChannel`]), the entry that names it and sets its limits
//! ([`ChannelEntry`]), and the registry of an engine's entries in id order ([`ChannelRegistry`]).

use std::rc::Rc;

use super::{ChannelLimits, Kind, MAX_CHANNELS};
use crate::handshake::ConnId;

/// The smallest per-packet budget a channel may ask for: room for a chunk's header and a few bytes.
pub const MIN_BUDGET: usize = 16;
/// The largest.
pub const MAX_BUDGET: usize = 4_096;

/// A channel's logic. The server's copy answers for snapshots; the client's copy gets the messages.
pub trait ChunkChannel: 'static {
    /// Server: the whole state `client` should have, asked when the client's channels are enabled and on
    /// a resync. `None` sends nothing.
    fn snapshot(&mut self, _client: ConnId) -> Option<Vec<u8>> {
        None
    }

    /// Client: a message arrived whole, in the order the server queued them.
    fn receive(&mut self, _kind: Kind, _data: &[u8]) {}

    /// Client: a new connection is starting (its message numbers and half-received messages are new), so state kept for the last
    /// one is stale.
    fn reset(&mut self) {}

    /// Client: the connection ended.
    fn closed(&mut self) {}

    /// Lets the code that owns the client's script engine reach the logic by its own type
    /// (`ClientChannels::with_channel`): `Some(self)` in a channel that wants it.
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }
}

/// Why a channel could not be registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelError {
    /// Two channels have this name.
    Duplicate(String),
    /// More than [`MAX_CHANNELS`].
    TooMany,
    /// A budget outside [`MIN_BUDGET`]..=[`MAX_BUDGET`].
    Budget { name: String, budget: usize },
    /// An empty name.
    Name,
}

impl std::fmt::Display for ChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelError::Duplicate(n) => write!(f, "two channels are named {n}"),
            ChannelError::TooMany => write!(f, "more than {MAX_CHANNELS} channels"),
            ChannelError::Budget { name, budget } => write!(
                f,
                "channel {name}: a budget of {budget} bytes is outside {MIN_BUDGET}..={MAX_BUDGET}"
            ),
            ChannelError::Name => f.write_str("a channel needs a name"),
        }
    }
}

impl std::error::Error for ChannelError {}

/// A channel a plugin added: its name, limits and how to make a copy of its logic for each side.
#[derive(Clone)]
pub struct ChannelEntry {
    name: String,
    limits: ChannelLimits,
    make: Rc<dyn Fn() -> Box<dyn ChunkChannel>>,
}

impl ChannelEntry {
    /// An entry named `name` with `limits`, whose logic `make` creates.
    pub fn new<C: ChunkChannel>(
        name: impl Into<String>,
        limits: ChannelLimits,
        make: impl Fn() -> C + 'static,
    ) -> ChannelEntry {
        ChannelEntry {
            name: name.into(),
            limits,
            make: Rc::new(move || Box::new(make())),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn limits(&self) -> ChannelLimits {
        self.limits
    }

    /// A new copy of the channel's logic.
    pub fn make(&self) -> Box<dyn ChunkChannel> {
        (self.make)()
    }
}

impl std::fmt::Debug for ChannelEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelEntry")
            .field("name", &self.name)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

/// The channels of an engine, in id order: sorted by name, so both ends (and the handshake's manifest, which carries
/// [`names`](ChannelRegistry::names)) agree on the ids without sending them.
#[derive(Clone, Debug, Default)]
pub struct ChannelRegistry {
    entries: Vec<ChannelEntry>,
}

impl ChannelRegistry {
    /// The registry of `entries`.
    pub fn new(entries: &[ChannelEntry]) -> Result<ChannelRegistry, ChannelError> {
        let mut entries = entries.to_vec();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        if entries.len() > MAX_CHANNELS {
            return Err(ChannelError::TooMany);
        }
        for pair in entries.windows(2) {
            if pair[0].name == pair[1].name {
                return Err(ChannelError::Duplicate(pair[0].name.clone()));
            }
        }
        for e in &entries {
            if e.name.is_empty() {
                return Err(ChannelError::Name);
            }
            if !(MIN_BUDGET..=MAX_BUDGET).contains(&e.limits.budget) {
                return Err(ChannelError::Budget {
                    name: e.name.clone(),
                    budget: e.limits.budget,
                });
            }
        }
        Ok(ChannelRegistry { entries })
    }

    /// Whether no channel is registered (then nothing in the packets changes).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The names in id order.
    pub fn names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.name.as_str()).collect()
    }

    /// The id of `name`.
    pub fn id(&self, name: &str) -> Option<usize> {
        self.entries
            .binary_search_by(|e| e.name.as_str().cmp(name))
            .ok()
    }

    /// The limits in id order.
    pub fn limits(&self) -> Vec<ChannelLimits> {
        self.entries.iter().map(|e| e.limits).collect()
    }

    /// A fresh copy of each channel's logic, in id order.
    pub fn make_all(&self) -> Vec<Option<Box<dyn ChunkChannel>>> {
        self.entries.iter().map(|e| Some((e.make)())).collect()
    }
}
