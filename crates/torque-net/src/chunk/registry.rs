//! What a plugin registers for a bulk channel: the logic ([`ChunkChannel`]), the entry that names it and sets its limits
//! ([`ChannelEntry`]), and the registry of an engine's entries in id order ([`ChannelRegistry`]).

use std::rc::Rc;

use super::upstream::MAX_UPSTREAM_MESSAGE;
use super::{ChannelLimits, Kind, MAX_CHANNELS};
use crate::handshake::ConnId;

/// The smallest per-packet budget a channel may ask for: room for a chunk's header and a few bytes.
pub const MIN_BUDGET: usize = 16;
/// The largest.
pub const MAX_BUDGET: usize = 4_096;

/// The prefix a manifest puts before the name of a client-to-server channel. No server-to-client channel may start with it, so
/// the two directions' names never collide.
pub const UPSTREAM_NAME_PREFIX: &str = "up:";

/// Which way a channel's messages go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// From the server to each client (a snapshot when enabled, then deltas).
    ToClient,
    /// From each client to the server, whose logic gets them ([`ChunkChannel::receive_from`]).
    ToServer,
}

/// A channel's logic. The server's copy answers for snapshots (and, in a client-to-server channel, gets the messages); the
/// client's copy gets the messages.
pub trait ChunkChannel: 'static {
    /// Server: the whole state `client` should have, asked when the client's channels are enabled and on
    /// a resync. `None` sends nothing.
    fn snapshot(&mut self, _client: ConnId) -> Option<Vec<u8>> {
        None
    }

    /// Client: a message arrived whole, in the order the server queued them.
    fn receive(&mut self, _kind: Kind, _data: &[u8]) {}

    /// Server, in a client-to-server channel: a message from `from` arrived whole, in the order that client queued them. The
    /// bytes are a client's, so treat them as hostile; the server's caps ([`super::upstream`]) bound their size and rate. The
    /// default is [`receive`](Self::receive), for a channel whose logic does not care which client sent it.
    fn receive_from(&mut self, _from: ConnId, kind: Kind, data: &[u8]) {
        self.receive(kind, data);
    }

    /// Server, in a client-to-server channel: the connection `conn` ended, so state kept for it can go.
    fn disconnected(&mut self, _conn: ConnId) {}

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
    /// A server-to-client channel whose name starts with [`UPSTREAM_NAME_PREFIX`], which the manifest keeps for
    /// client-to-server ones.
    Reserved(String),
    /// A client-to-server channel that allows messages above [`MAX_UPSTREAM_MESSAGE`].
    MessageSize { name: String, bytes: usize },
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
            ChannelError::Reserved(n) => write!(
                f,
                "channel {n}: names starting with {UPSTREAM_NAME_PREFIX} are reserved"
            ),
            ChannelError::MessageSize { name, bytes } => write!(
                f,
                "channel {name}: messages of {bytes} bytes are above the {MAX_UPSTREAM_MESSAGE} a client may send"
            ),
        }
    }
}

impl std::error::Error for ChannelError {}

/// A channel a plugin added: its name, limits and how to make a copy of its logic for each side.
#[derive(Clone)]
pub struct ChannelEntry {
    name: String,
    direction: Direction,
    limits: ChannelLimits,
    make: Rc<dyn Fn() -> Box<dyn ChunkChannel>>,
}

impl ChannelEntry {
    /// An entry named `name` with `limits`, whose logic `make` creates, for a channel from the server to each client.
    pub fn new<C: ChunkChannel>(
        name: impl Into<String>,
        limits: ChannelLimits,
        make: impl Fn() -> C + 'static,
    ) -> ChannelEntry {
        ChannelEntry::in_direction(Direction::ToClient, name, limits, make)
    }

    /// An entry for a channel from each client to the server. The server's logic gets the messages
    /// ([`ChunkChannel::receive_from`]); a client sends through its channels.
    pub fn to_server<C: ChunkChannel>(
        name: impl Into<String>,
        limits: ChannelLimits,
        make: impl Fn() -> C + 'static,
    ) -> ChannelEntry {
        ChannelEntry::in_direction(Direction::ToServer, name, limits, make)
    }

    /// An entry whose messages go `direction`.
    pub fn in_direction<C: ChunkChannel>(
        direction: Direction,
        name: impl Into<String>,
        limits: ChannelLimits,
        make: impl Fn() -> C + 'static,
    ) -> ChannelEntry {
        ChannelEntry {
            name: name.into(),
            direction,
            limits,
            make: Rc::new(move || Box::new(make())),
        }
    }

    pub fn direction(&self) -> Direction {
        self.direction
    }

    /// The name the manifest lists: the channel's own for a server-to-client one, and with [`UPSTREAM_NAME_PREFIX`] before it
    /// for a client-to-server one.
    pub fn manifest_name(&self) -> String {
        match self.direction {
            Direction::ToClient => self.name.clone(),
            Direction::ToServer => format!("{UPSTREAM_NAME_PREFIX}{}", self.name),
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
            .field("direction", &self.direction)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

/// The channels of an engine, in id order: sorted by name, so both ends (and the handshake's manifest, which carries
/// [`names`](ChannelRegistry::names)) agree on the ids without sending them. The two directions have ids of their own: the
/// methods without a direction in their name (`names`, `id`, `limits`, `len`, `make_all`) are about the server-to-client
/// channels, as they were before client-to-server ones existed, and the `up_` ones about the others.
#[derive(Clone, Debug, Default)]
pub struct ChannelRegistry {
    entries: Vec<ChannelEntry>,
    up: Vec<ChannelEntry>,
}

impl ChannelRegistry {
    /// The registry of `entries`.
    pub fn new(entries: &[ChannelEntry]) -> Result<ChannelRegistry, ChannelError> {
        let (mut up, mut entries): (Vec<_>, Vec<_>) = entries
            .iter()
            .cloned()
            .partition(|e| e.direction == Direction::ToServer);
        for list in [&mut entries, &mut up] {
            list.sort_by(|a, b| a.name.cmp(&b.name));
            if list.len() > MAX_CHANNELS {
                return Err(ChannelError::TooMany);
            }
            for pair in list.windows(2) {
                if pair[0].name == pair[1].name {
                    return Err(ChannelError::Duplicate(pair[0].name.clone()));
                }
            }
            for e in list.iter() {
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
        }
        if let Some(e) = entries
            .iter()
            .find(|e| e.name.starts_with(UPSTREAM_NAME_PREFIX))
        {
            return Err(ChannelError::Reserved(e.name.clone()));
        }
        if let Some(e) = up
            .iter()
            .find(|e| e.limits.max_message > MAX_UPSTREAM_MESSAGE)
        {
            return Err(ChannelError::MessageSize {
                name: e.name.clone(),
                bytes: e.limits.max_message,
            });
        }
        Ok(ChannelRegistry { entries, up })
    }

    /// Whether no channel is registered in either direction (then nothing in the packets changes).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.up.is_empty()
    }

    /// Whether any server-to-client channel is registered (the connection then has that direction's section).
    pub fn has_downstream(&self) -> bool {
        !self.entries.is_empty()
    }

    /// Whether any client-to-server channel is registered (the connection then has that direction's section).
    pub fn has_upstream(&self) -> bool {
        !self.up.is_empty()
    }

    /// The client-to-server channels' names in id order.
    pub fn up_names(&self) -> Vec<&str> {
        self.up.iter().map(|e| e.name.as_str()).collect()
    }

    /// How many client-to-server channels there are.
    pub fn up_len(&self) -> usize {
        self.up.len()
    }

    /// The id of the client-to-server channel `name`.
    pub fn up_id(&self, name: &str) -> Option<usize> {
        self.up.binary_search_by(|e| e.name.as_str().cmp(name)).ok()
    }

    /// The client-to-server channels' limits in id order.
    pub fn up_limits(&self) -> Vec<ChannelLimits> {
        self.up.iter().map(|e| e.limits).collect()
    }

    /// A fresh copy of each client-to-server channel's logic, in id order.
    pub fn make_up(&self) -> Vec<Option<Box<dyn ChunkChannel>>> {
        self.up.iter().map(|e| Some((e.make)())).collect()
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
