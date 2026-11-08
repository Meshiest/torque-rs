//! The link contract: what engine mode needs from whatever carries its datagrams.
//!
//! A link moves unreliable, unordered datagrams of at most [`Link::max_datagram`] bytes, may refuse one for want of room, and
//! reports what it already guarantees ([`LinkFacts`]). Everything else (sequence numbers, acks, lanes, fragmentation) is the
//! engine's, so the same engine bytes run on every link. The contract matches a QUIC connection's unreliable datagrams
//! (`max_size` moves with the path MTU; a send with the buffer full is refused instead of dropping an older datagram) and a
//! WebTransport session's (`maxDatagramSize`, the writer's backpressure), as well as plain UDP, WebRTC data channels,
//! WebSockets and the in-process memory link. Receiving is the host's: it hands each datagram to the session.

/// The SHA-256 of the server key's SubjectPublicKeyInfo, as the client's link verified it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServerIdentity(pub [u8; 32]);

/// What a link guarantees on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LinkFacts {
    /// The link proved the peer's address (QUIC address validation, ICE, TCP, a process boundary). A link without it is
    /// plain: the engine runs its own cookie round trip first.
    pub validated: bool,
    /// Encrypted and integrity-checked.
    pub secure: bool,
    /// The link runs its own congestion control (a "no room" from it is that controller speaking).
    pub congestion_controlled: bool,
    /// The server key the client's link verified, if it verified one.
    pub server_identity: Option<ServerIdentity>,
}

impl LinkFacts {
    /// Plain UDP: nothing proven, nothing encrypted.
    pub const PLAIN: LinkFacts = LinkFacts {
        validated: false,
        secure: false,
        congestion_controlled: false,
        server_identity: None,
    };

    /// The in-process link: the peer is this process.
    pub const MEMORY: LinkFacts = LinkFacts {
        validated: true,
        secure: false,
        congestion_controlled: false,
        server_identity: None,
    };
}

/// The link had no room for a datagram: it was not sent, and the engine counts it as never sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoRoom;

/// One peer's datagram link.
pub trait Link {
    /// The largest datagram the link takes now. A session reads it again for every packet it builds.
    fn max_datagram(&self) -> usize;

    /// Sends one datagram of at most [`Link::max_datagram`] bytes.
    ///
    /// # Errors
    /// [`NoRoom`]: the link cannot take it now; nothing was sent.
    fn send(&mut self, datagram: &[u8]) -> Result<(), NoRoom>;

    /// What the link guarantees. It does not change during a session.
    fn facts(&self) -> LinkFacts;
}
