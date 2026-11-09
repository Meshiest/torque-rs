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

/// What a host knows of the link to a peer before it has a session on it: what the link guarantees and the largest datagram
/// it takes. A transport reports it per peer, and both ends of one link must report the same facts (a client that thinks
/// its link validated sends no `Hello`, which a plain link's server never answers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkKind {
    pub facts: LinkFacts,
    pub max_datagram: usize,
}

impl LinkKind {
    /// Plain UDP: LAN and development only.
    pub const PLAIN_UDP: LinkKind = LinkKind {
        facts: LinkFacts::PLAIN,
        max_datagram: 1_200,
    };

    /// The in-process link.
    pub const MEMORY: LinkKind = LinkKind {
        facts: LinkFacts::MEMORY,
        max_datagram: 65_535,
    };

    /// A WebRTC data channel: ICE proved the address, DTLS encrypts, SCTP runs congestion control.
    pub const WEB_RTC: LinkKind = LinkKind {
        facts: LinkFacts {
            validated: true,
            secure: true,
            congestion_controlled: true,
            server_identity: None,
        },
        max_datagram: 1_200,
    };

    /// A WebSocket (`secure`: `wss`): TCP proved the address and runs congestion control.
    #[must_use]
    pub const fn web_socket(secure: bool) -> LinkKind {
        LinkKind {
            facts: LinkFacts {
                validated: true,
                secure,
                congestion_controlled: true,
                server_identity: None,
            },
            max_datagram: 16_384,
        }
    }
}
