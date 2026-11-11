//! The link contract: what engine mode needs from whatever carries its datagrams.
//!
//! A link moves unreliable, unordered datagrams of at most [`Link::max_datagram`] bytes, may refuse one for want of room, and
//! reports what it already guarantees ([`LinkFacts`]). Everything else (sequence numbers, acks, lanes, fragmentation) is the
//! engine's, so the same engine bytes run on every link. The contract matches a QUIC connection's unreliable datagrams
//! (`max_size` moves with the path MTU; a send with the buffer full is refused instead of dropping an older datagram) and a
//! WebTransport session's (`maxDatagramSize`, the writer's backpressure), as well as plain UDP, WebRTC data channels,
//! WebSockets and the in-process memory link. Receiving is the host's: it hands each datagram to the session.

use std::fmt;
use std::str::FromStr;

/// The SHA-256 of the server key's SubjectPublicKeyInfo, as the client's link verified it. Written and read as
/// `sha256:<64 hex digits>` ([`fmt::Display`], [`FromStr`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServerIdentity(pub [u8; 32]);

impl fmt::Display for ServerIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("sha256:")?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// A text that is not `sha256:` followed by 64 hex digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadIdentityText;

impl fmt::Display for BadIdentityText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a server identity is `sha256:` followed by 64 hex digits")
    }
}

impl std::error::Error for BadIdentityText {}

impl FromStr for ServerIdentity {
    type Err = BadIdentityText;

    fn from_str(text: &str) -> Result<ServerIdentity, BadIdentityText> {
        let hex = text.strip_prefix("sha256:").ok_or(BadIdentityText)?;
        if hex.len() != 64 || !hex.is_ascii() {
            return Err(BadIdentityText);
        }
        let mut out = [0; 32];
        for (byte, pair) in out.iter_mut().zip(hex.as_bytes().chunks(2)) {
            let digit = |c: u8| (c as char).to_digit(16).ok_or(BadIdentityText);
            *byte = (digit(pair[0])? << 4 | digit(pair[1])?) as u8;
        }
        Ok(ServerIdentity(out))
    }
}

/// A value both ends of one secure link derive from its handshake and no third party can: the TLS exporter of RFC 8446
/// section 7.5 with the label [`ChannelBinding::LABEL`], 32 bytes, an empty context. A ticket made for it fails on any
/// other connection, however it was relayed (the channel-binding practice of RFC 9266). Links that cannot derive one
/// (plain UDP, memory, WebRTC, WebSocket) have none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChannelBinding(pub [u8; 32]);

impl ChannelBinding {
    /// The exporter label.
    pub const LABEL: &'static [u8] = b"EXPORTER-torque-auth";
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_identity_round_trips_through_its_text_and_bad_texts_are_refused() {
        let id = ServerIdentity(std::array::from_fn(|i| i as u8 * 7));
        let text = id.to_string();
        assert!(
            text.starts_with("sha256:") && text.len() == 7 + 64,
            "{text}"
        );
        assert_eq!(text.parse::<ServerIdentity>(), Ok(id));
        for bad in ["", "sha256:", "sha1:00", &text[..70], &format!("{text}0")] {
            assert_eq!(
                bad.parse::<ServerIdentity>(),
                Err(BadIdentityText),
                "{bad:?}"
            );
        }
        let odd = format!("sha256:{}zz", &text[7..69]);
        assert_eq!(odd.parse::<ServerIdentity>(), Err(BadIdentityText));
    }
}
