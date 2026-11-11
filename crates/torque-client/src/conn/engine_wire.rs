//! A client connection's engine-mode session ([`net::engine`]): its framing in place of TGE's handshake and packet header,
//! under the same feed ([`super::tge::TgeFeed`]), which reads the server's ghost lane and writes its move lane exactly as it
//! reads and writes a TGE packet's payload.
//!
//! The connect is made when the feed first pumps (the block source, with the engine's manifest, is set after the feed is
//! made). It carries no ticket: a server that checks tickets challenges the connect, and the ticket is made then, for the
//! address dialed, the server's challenge, the identity the link verified and the link's channel binding
//! ([`ticket_context`]); the link's kind and binding are read again every pass, since a secure link learns them only as its
//! handshake ends (before any answer of the server can arrive). Each packet the
//! feed writes into gets a wide number in send order, and the fates come back in that order ([`PacketOrder`]), as the
//! event queue and the upstream channels expect. The script callbacks are TGE mode's: the connect's start is the challenge
//! (`ChallengeSent`), its answer (a cookie on a plain link, the first packet out on a validated one) the challenge's answer.

use std::hash::{BuildHasher, Hasher, RandomState};
use std::net::SocketAddr;

use net::cap::NET_MAX_DATAGRAM;
use net::connect::{BlockSource, ConnectBlock, reason};
use net::engine::bridge::{connect_of, ticket_context};
use net::engine::{
    ChannelBinding, CloseReason, Event, Link as EngineLink, LinkFacts, LinkKind, NoRoom, NoStreams,
    PacketOrder, Session, SessionConfig, State, StreamLane, StreamSource,
};
use net::handshake::siphash24;
use net::profile::HandshakeProfile;
use torque_server::net::rpc::RpcLanes;

use super::{FeedEvent, Link};

/// A key the client's nonces are drawn from in place of the system's randomness, as an extension of the client's script
/// engine: a run that must repeat (the trace hashes) sets one. With one set, the TGE connect sequence is the seeded one too
/// (the Sim clock's and the object's), so the trace's packets repeat; without, it mixes in the system's randomness.
pub struct EngineNonceKey(pub [u8; 16]);

/// The nonce of the connect with sequence `seq`: from `key` when there is one, else from the system's randomness.
#[must_use]
pub fn engine_nonce(key: Option<&EngineNonceKey>, seq: u32) -> u64 {
    match key {
        Some(key) => siphash24(&key.0, &seq.to_le_bytes()),
        None => system_random(),
    }
}

/// 64 bits from the system's random source: the operating system's natively. In a browser (no OS source here) the standard
/// library's hasher seed mixed with the clock.
#[must_use]
pub fn system_random() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut bytes = [0u8; 8];
        if getrandom::fill(&mut bytes).is_ok() {
            return u64::from_le_bytes(bytes);
        }
    }
    let mut hasher = RandomState::new().build_hasher();
    if let Ok(now) = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH) {
        hasher.write_u128(now.as_nanos());
    }
    hasher.finish()
}

/// What a connection in engine mode starts from.
pub struct EngineStart {
    /// The address dialed (what a ticket is made for).
    pub server: SocketAddr,
    pub nonce: u64,
    /// What makes the connect's block: the engine's manifest and the ticket.
    pub source: Option<BlockSource>,
    pub password: Vec<u8>,
    pub args: Vec<Vec<u8>>,
    pub kind: LinkKind,
}

/// What the session told the feed.
pub enum WireEvent {
    /// The connect is out (TGE's challenge request).
    Sent,
    /// The server answered (TGE's challenge response).
    Answered,
    /// The server accepted with this ghost-id width.
    Accepted { ghost_bits: u8 },
    /// The server's ghost lane of one packet: what a TGE packet carries after its header.
    Packet { seq: u16, data: Vec<u8> },
    /// An RPC batch from the server.
    Rpc(Vec<u8>),
    /// One of this side's packets arrived or was lost, in send order.
    Fate { send_seq: u32, delivered: bool },
    /// The connection ended this way.
    Ended(FeedEvent<()>),
    /// The server sent what no server of this engine sends.
    Failed(String),
}

/// The client's link as the session's.
struct WireLink<'a> {
    link: &'a mut dyn Link,
    kind: LinkKind,
}

impl EngineLink for WireLink<'_> {
    /// The one place the client's send limit is made: the session sends at most the accept's `max_packet` and at most this, so
    /// `min(max_packet, NET_MAX_DATAGRAM)`. A client never sends over [`NET_MAX_DATAGRAM`] whatever the accept says (a rule of the
    /// engine's protocol, `book/src/network-protocols.md`): a server drops a longer datagram as it is read, and a reliable lane
    /// would send it again for ever.
    fn max_datagram(&self) -> usize {
        self.kind.max_datagram.min(NET_MAX_DATAGRAM)
    }

    fn send(&mut self, datagram: &[u8]) -> Result<(), NoRoom> {
        self.link.send(datagram).map_err(|_| NoRoom)
    }

    fn facts(&self) -> LinkFacts {
        self.kind.facts
    }
}

/// The feed's writer of the move lane, as a stream source: each packet's number, then the bytes the feed wrote for it.
struct MoveLane<'a> {
    order: &'a mut PacketOrder,
    write: &'a mut dyn FnMut(u32, usize) -> Option<Vec<u8>>,
}

impl StreamSource for MoveLane<'_> {
    fn write(&mut self, lane: StreamLane, seq: u16, room: usize, out: &mut Vec<u8>) {
        if lane != StreamLane::Moves || room == 0 {
            return;
        }
        let wide = self.order.sent(seq);
        match (self.write)(wide, room) {
            Some(bytes) if !bytes.is_empty() && bytes.len() <= room => {
                out.extend_from_slice(&bytes)
            }
            _ => self.order.unsent(seq),
        }
    }

    fn unsent(&mut self, seq: u16) {
        self.order.unsent(seq);
    }
}

/// Where a connection is, as the feed's callbacks tell it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The connect is out, nothing came back.
    Connecting,
    /// The server answered (TGE's challenge response).
    Answered,
    Accepted,
}

/// One connection's session (module docs).
pub struct EngineWire {
    start: Option<EngineStart>,
    server: SocketAddr,
    source: Option<BlockSource>,
    /// The server's challenge, to answer once the events are drained.
    challenge: Option<u64>,
    session: Option<Session>,
    /// The link's kind and channel binding as of the last pass: a secure link learns the server's identity and derives its
    /// binding when its handshake ends, before any answer of the server can arrive.
    kind: LinkKind,
    binding: Option<ChannelBinding>,
    order: PacketOrder,
    phase: Phase,
}

impl EngineWire {
    #[must_use]
    pub fn new(start: EngineStart) -> EngineWire {
        EngineWire {
            kind: start.kind,
            binding: None,
            server: start.server,
            source: None,
            challenge: None,
            start: Some(start),
            session: None,
            order: PacketOrder::default(),
            phase: Phase::Connecting,
        }
    }

    #[must_use]
    pub fn is_accepted(&self) -> bool {
        self.phase == Phase::Accepted
    }

    /// The session's round trip once accepted; 0 before.
    #[must_use]
    pub fn round_trip(&self) -> f32 {
        match &self.session {
            Some(s) if self.is_accepted() => s.round_trip_ms() as f32,
            _ => 0.0,
        }
    }

    /// Makes the connect and the session, the first time; an `Err` is the block source's refusal (the connect ends with it).
    fn begin(&mut self, now: u32, out: &mut Vec<WireEvent>) -> Result<(), Vec<u8>> {
        let Some(mut start) = self.start.take() else {
            return Ok(());
        };
        let profile = HandshakeProfile::default();
        let cx = ticket_context(
            start.server,
            0,
            self.kind.facts.server_identity,
            self.binding,
        );
        let block = match start.source.as_mut().map(|s| s.make(&cx)) {
            None | Some(Ok(None)) => None,
            // The ticket waits for the server's challenge.
            Some(Ok(Some(block))) => Some(ConnectBlock {
                ticket: None,
                ..block
            }),
            Some(Err(why)) => return Err(reason(&why)),
        };
        self.source = start.source.take();
        let connect = connect_of(&profile, block.as_ref(), &start.password, &start.args)
            .map_err(|why| reason(&why))?;
        let game = String::from_utf8_lossy(&profile.game).into_owned();
        let session = Session::client(
            SessionConfig::new(&game),
            start.kind.facts,
            connect,
            start.nonce,
            now,
        )
        .map_err(|e| reason(&e.to_string()))?;
        self.session = Some(session);
        out.push(WireEvent::Sent);
        Ok(())
    }

    /// The pump before the accept: the connect is made, then the session's `Hello` or openings go out as it times them.
    pub fn pump(&mut self, link: &mut dyn Link, now: u32, out: &mut Vec<WireEvent>) {
        self.refresh(link);
        if let Err(why) = self.begin(now, out) {
            out.push(WireEvent::Ended(FeedEvent::Rejected(why)));
            return;
        }
        if self.is_accepted() {
            return;
        }
        let kind = self.kind;
        if let Some(session) = self.session.as_mut() {
            session.poll(&mut WireLink { link, kind }, &mut NoStreams, now);
            if self.phase == Phase::Connecting
                && session.facts().validated
                && session.state() != State::Closed
            {
                self.phase = Phase::Answered;
                out.push(WireEvent::Answered);
            }
        }
        self.drain(out);
    }

    /// A datagram from the server.
    pub fn receive(&mut self, data: &[u8], now: u32, out: &mut Vec<WireEvent>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.receive(data, now);
        if self.phase == Phase::Connecting
            && !matches!(session.state(), State::Hello | State::Closed)
        {
            self.phase = Phase::Answered;
            out.push(WireEvent::Answered);
        }
        self.drain(out);
    }

    /// Hands the session the RPC batches the links queued (an unreliable one too large for its lane goes reliably), as many as
    /// its send queue takes.
    pub fn feed_rpcs(&mut self, lanes: &RpcLanes) {
        if let Some(session) = self
            .session
            .as_mut()
            .filter(|_| self.phase == Phase::Accepted)
        {
            lanes.feed(None, session);
        }
    }

    /// The tick's packet once accepted: `write` is given each packet's number and the room of its move lane, and answers
    /// the lane's bytes.
    pub fn send(
        &mut self,
        link: &mut dyn Link,
        now: u32,
        write: &mut dyn FnMut(u32, usize) -> Option<Vec<u8>>,
        out: &mut Vec<WireEvent>,
    ) {
        self.refresh(link);
        let kind = self.kind;
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut lane = MoveLane {
            order: &mut self.order,
            write,
        };
        session.poll(&mut WireLink { link, kind }, &mut lane, now);
        self.drain(out);
    }

    /// Hangs up with `reason`: the goodbye goes out now.
    pub fn disconnect(&mut self, link: &mut dyn Link, reason: &str, now: u32) {
        let kind = self.kind;
        if let Some(session) = self.session.as_mut() {
            session.disconnect(reason);
            session.poll(&mut WireLink { link, kind }, &mut NoStreams, now);
        }
    }

    /// Reads the link's kind and channel binding again (module docs: a secure link learns them as its handshake ends).
    fn refresh(&mut self, link: &dyn Link) {
        self.kind = link.kind();
        self.binding = link.binding();
    }

    /// Answers the server's challenge with a ticket the block source makes for it (none: an empty answer, which a server
    /// that needs one refuses with its reason).
    fn answer(&mut self, out: &mut Vec<WireEvent>) {
        let Some(challenge) = self.challenge.take() else {
            return;
        };
        let cx = ticket_context(
            self.server,
            challenge,
            self.kind.facts.server_identity,
            self.binding,
        );
        let ticket = match self.source.as_mut().map(|s| s.make(&cx)) {
            None | Some(Ok(None)) => Vec::new(),
            Some(Ok(Some(block))) => block.ticket.unwrap_or_default(),
            Some(Err(why)) => {
                out.push(WireEvent::Ended(FeedEvent::Rejected(reason(&why))));
                return;
            }
        };
        if let Some(session) = self.session.as_mut()
            && let Err(e) = session.answer(ticket)
        {
            out.push(WireEvent::Failed(e.to_string()));
        }
    }

    /// The session's events as the feed's, and the fates known so far in send order; a challenge is answered.
    fn drain(&mut self, out: &mut Vec<WireEvent>) {
        self.drain_events(out);
        self.answer(out);
    }

    fn drain_events(&mut self, out: &mut Vec<WireEvent>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        while let Some(event) = session.next_event() {
            match event {
                Event::Accepted(accept) => {
                    self.phase = Phase::Accepted;
                    out.push(WireEvent::Accepted {
                        ghost_bits: accept.widths.ghost_id,
                    });
                }
                Event::Message { data, .. } => out.push(WireEvent::Rpc(data)),
                Event::Stream {
                    lane: StreamLane::Ghosts,
                    seq,
                    data,
                } => out.push(WireEvent::Packet { seq, data }),
                Event::Stream { .. } | Event::ConnectRequest(_) | Event::Ticket(_) => {
                    out.push(WireEvent::Failed(
                        "the server sent a lane a client does not read".to_string(),
                    ));
                }
                Event::Notify { seq, delivered } => self.order.resolve(seq, delivered),
                Event::Challenge(nonce) => self.challenge = Some(nonce),
                Event::Closed(why) => out.push(WireEvent::Ended(ended(why, self.phase))),
            }
            while let Some((send_seq, delivered)) = self.order.pop() {
                out.push(WireEvent::Fate {
                    send_seq,
                    delivered,
                });
            }
        }
    }
}

/// How a session's end reads to the feed, in `phase`.
fn ended(why: CloseReason, phase: Phase) -> FeedEvent<()> {
    let accepted = phase == Phase::Accepted;
    match why {
        CloseReason::Rejected { text, .. } => FeedEvent::Rejected(text.into_bytes()),
        CloseReason::Disconnected(text) => FeedEvent::Dropped(text.into_bytes()),
        CloseReason::TimedOut if accepted => FeedEvent::TimedOut,
        CloseReason::TimedOut => FeedEvent::HandshakeTimedOut,
        CloseReason::Local => FeedEvent::Dropped(Vec::new()),
        CloseReason::Protocol(e) => FeedEvent::Dropped(e.to_string().into_bytes()),
    }
}
