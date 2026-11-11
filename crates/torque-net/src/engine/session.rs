//! One peer's engine-mode session, either side: the connect flow, then packets with lanes. Sans-IO: the host hands each
//! datagram it receives to [`Session::receive`] and calls [`Session::poll`] once a tick, which sends at most one datagram
//! over the [`Link`].
//!
//! **Connect flow.** On a validated link the client's first packet already carries `Connect` on the control lane. On a plain
//! link it first sends `Hello` (padded to [`HELLO_BYTES`]) until a `Cookie` answers, then every packet until the accept is an
//! `Opening` that carries the cookie and is padded to [`HELLO_BYTES`] too. A server session on a plain link is made only by the
//! [`super::Gate`], once a cookie verified. Until such a server has seen a packet that the client could only send after
//! reading the accept, it sends at most one datagram per datagram received, no larger than it.
//!
//! **Plain links carry a token.** There every packet after the client's openings is a `TokenData` with the client's nonce (or,
//! once that side is established, the session id), and a packet without the right token is ignored, so a sender off the path
//! cannot inject packets into a session without guessing 64 bits. On a validated link the link already binds the peer.
//!
//! **The challenge.** A server whose connects carry auth tickets answers a connect with a challenge, a nonce it drew
//! ([`Session::challenge`]); the client makes its ticket for that nonce and answers with it ([`Event::Challenge`],
//! [`Session::answer`]), and only then does the server decide. A ticket is so bound to a value the server chose for this
//! session: one captured on another connection, or made before the challenge, is for another nonce.
//!
//! **What is read when.** Until a client has the accept, and until a server has accepted and seen the client's first data
//! packet, only the control lane is read: a packet with any other section is dropped unread and not acknowledged, so its
//! sender counts it lost and resends.
//!
//! **Lanes and room.** A packet is filled in [`Lane`] order. Each lane first gets at most its budget (a share of the packet's
//! payload room); then the message lanes (control, RPC, unreliable) may take what is left, in the same order. So a lower lane
//! never takes room a higher lane wanted, and a higher lane at its budget leaves the rest of the packet to the lanes below it.
//! The unreliable lane saves its unused share from packet to packet, up to one whole message, so a message larger than one
//! packet's share still leaves, at the lane's budget on average. Before the accept a packet is at most
//! [`DEFAULT_MAX_PACKET`] bytes, after it at most the accept's limit (the server's sends; a client's are also at most
//! [`NET_MAX_DATAGRAM`](crate::cap::NET_MAX_DATAGRAM), whatever the accept says), and always at most the link's
//! [`Link::max_datagram`] of that tick; a received datagram past the limit is dropped unread.
//!
//! **Fates of stream packets are the peer's.** The layers above a stream lane (ghosts, events, channels) send again what a lost
//! packet carried, so a packet with stream data is counted lost only when the peer can no longer read it: its ack mask lacks
//! the packet and acknowledges one [`REORDER_THRESHOLD`](super::REORDER_THRESHOLD) or more after it (the receiver reads no
//! packet that far behind its newest), or the peer's newest packet acknowledges none of it though it was sent a loss timeout
//! earlier (an outage). Never by the clock alone: a peer that stops reading for a long frame reads every packet that waited.
//! The sender keeps those packets within the ack mask ([`ACK_BITS`](super::ACK_BITS)) of what it sends, and at its edge sends
//! nothing until the peer's acks come.
//!
//! **Memory.** What a peer can make a session hold is bounded: reassembly by [`LaneLimits`], events between two drains by
//! [`MAX_EVENTS`] and [`LaneLimits::event_bytes`]. A peer that passes a cap closes the session with
//! [`CloseReason::Protocol`].

use std::collections::VecDeque;

use super::framing::{
    self, Accept, BODY_HEADER, Body, Connect, Control, CookieValue, Datagram, Disconnect, Fragment,
    Hello, Lane, Prefix, Reject, RejectCode, SECTION_HEADER, StreamLane, UNRELIABLE_HEADER,
    Unreliable, Versions,
};
use super::lanes::{
    Flight, FragRef, Received, Record, ReliableRecv, ReliableSend, SendError, UnreliableRecv,
    UnreliableSend,
};
use super::link::{Link, LinkFacts, NoRoom};
use super::{DEFAULT_MAX_PACKET, ENGINE_FRAMING, HELLO_BYTES, MAX_PACKET};
use crate::NetError;

/// How long a connect may take, from the session's start, before it times out.
pub const CONNECT_TIMEOUT_MS: u32 = 10_000;
/// How long an established session may hear nothing before it times out.
pub const IDLE_TIMEOUT_MS: u32 = 15_000;
/// How often a client resends its `Hello` until a cookie comes.
pub const HELLO_RESEND_MS: u32 = 250;
/// How often a client on a plain link sends an `Opening` with nothing in it while it waits for the accept, so the server has
/// a datagram to answer.
pub const OPENING_PING_MS: u32 = 100;
/// How long an established session stays quiet before it sends a bare ack.
pub const KEEPALIVE_MS: u32 = 1_000;
/// Events a session holds for its host; one more is a protocol error (the host drains them every tick).
pub const MAX_EVENTS: usize = 4_096;

const INITIAL_RTT_MS: u32 = 200;

/// How a message is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Delivery {
    /// Resent until acknowledged, in order among the session's reliable messages.
    #[default]
    Reliable,
    /// Sent once in the next packet with room, dropped on loss, never handed on after a newer one.
    Unreliable,
}

/// How much a session may queue and hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneLimits {
    /// The largest control message (a connect with its manifest, ticket and arguments).
    pub control_message: usize,
    /// The largest reliable message.
    pub rpc_message: usize,
    /// Reliable bytes queued and not yet acknowledged; a send past it is [`SendError::Busy`].
    pub rpc_send_queue: usize,
    /// Reliable bytes the receiver holds while it assembles them.
    pub rpc_receive_held: usize,
    /// The largest unreliable message, sent or received (at most a `u16` length).
    pub unreliable_message: usize,
    /// Unreliable bytes queued and not yet sent.
    pub unreliable_send_queue: usize,
    /// Bytes of messages, stream sections and connects the session holds as events between two drains.
    pub event_bytes: usize,
}

impl Default for LaneLimits {
    fn default() -> LaneLimits {
        LaneLimits {
            control_message: 64 << 10,
            rpc_message: 64 << 10,
            rpc_send_queue: 64 << 10,
            rpc_receive_held: 256 << 10,
            unreliable_message: 1_024,
            unreliable_send_queue: 16 << 10,
            event_bytes: 4 << 20,
        }
    }
}

/// Each lane's share of a packet's payload room in its first pass, in percent, indexed in [`Lane`] order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneBudgets(pub [u8; 6]);

impl LaneBudgets {
    #[must_use]
    pub fn of(&self, lane: Lane) -> usize {
        usize::from(self.0[lane.index()].min(100))
    }
}

impl Default for LaneBudgets {
    /// Control 100, RPC 50, unreliable 25, ghosts, moves and channels 100.
    fn default() -> LaneBudgets {
        LaneBudgets([100, 50, 25, 100, 100, 100])
    }
}

/// What both sides of a session are set up with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    /// The game id; a server refuses a connect for another.
    pub game: String,
    pub limits: LaneLimits,
    pub budgets: LaneBudgets,
}

impl SessionConfig {
    #[must_use]
    pub fn new(game: &str) -> SessionConfig {
        SessionConfig {
            game: game.to_owned(),
            limits: LaneLimits::default(),
            budgets: LaneBudgets::default(),
        }
    }
}

/// Where a session is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Client on a plain link: waiting for a cookie.
    Hello,
    /// Client: connect sent, waiting for the accept. Server: waiting for the connect.
    Opening,
    /// Server: the connect arrived; the host decides ([`Session::accept`] or [`Session::reject`]), or challenges first
    /// ([`Session::challenge`]).
    Deciding,
    /// Server: the challenge went out; waiting for the client's ticket.
    Challenging,
    /// Server: accepted, waiting for the client's first packet after the accept.
    Accepted,
    Established,
    Closed,
}

/// Why a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    /// The server refused the connect.
    Rejected {
        code: RejectCode,
        text: String,
    },
    /// The peer said goodbye.
    Disconnected(String),
    /// This side said goodbye or refused.
    Local,
    TimedOut,
    /// The peer sent something malformed, or past a cap.
    Protocol(NetError),
}

/// What a session tells its host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Server: a client asks to connect (its game and versions already matched).
    ConnectRequest(Box<Connect>),
    /// Client: the server accepted.
    Accepted(Accept),
    /// Client: the server challenged with this nonce; the host answers with a ticket made for it ([`Session::answer`]).
    Challenge(u64),
    /// Server: the client's answer to the challenge, its ticket (empty: it has none); the host decides.
    Ticket(Vec<u8>),
    /// A whole message of the RPC lane or the unreliable lane.
    Message {
        delivery: Delivery,
        data: Vec<u8>,
    },
    /// A stream lane's section of packet `seq`.
    Stream {
        lane: StreamLane,
        seq: u16,
        data: Vec<u8>,
    },
    /// Packet `seq`, into which a stream lane wrote, arrived or was lost.
    Notify {
        seq: u16,
        delivered: bool,
    },
    Closed(CloseReason),
}

impl Event {
    /// The bytes it holds that a peer chose.
    fn held(&self) -> usize {
        match self {
            Event::ConnectRequest(c) => {
                c.game.len()
                    + c.manifest.len()
                    + c.ticket.len()
                    + c.mods.len()
                    + c.password.len()
                    + c.args.iter().map(String::len).sum::<usize>()
            }
            Event::Message { data, .. } | Event::Stream { data, .. } | Event::Ticket(data) => {
                data.len()
            }
            Event::Accepted(_) | Event::Challenge(_) | Event::Notify { .. } | Event::Closed(_) => 0,
        }
    }
}

/// What one [`Session::poll`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polled {
    /// Nothing to send.
    Idle,
    /// One datagram of this many bytes.
    Sent(usize),
    /// The link had no room; nothing changed.
    NoRoom,
    Closed,
}

/// What the host writes into the stream lanes of each packet, and hears back about.
pub trait StreamSource {
    /// Writes at most `room` bytes of `lane`'s section of packet `seq` into `out` (nothing: no section). A section longer
    /// than `room` is dropped, not sent.
    fn write(&mut self, lane: StreamLane, seq: u16, room: usize, out: &mut Vec<u8>);

    /// Packet `seq` was not sent after all (the link had no room): forget what was written into it.
    fn unsent(&mut self, seq: u16);
}

/// A host with nothing in the stream lanes.
pub struct NoStreams;

impl StreamSource for NoStreams {
    fn write(&mut self, _: StreamLane, _: u16, _: usize, _: &mut Vec<u8>) {}

    fn unsent(&mut self, _: u16) {}
}

/// What a gate-opened server may still send before the client proves it reads at its address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reply {
    /// One datagram of at most this many bytes, answering the last datagram received.
    Owed(usize),
    /// The last datagram was answered.
    Spent,
}

/// How a server session knows the client is at its address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Proof {
    /// The link proved it.
    Link,
    /// The gate verified this cookie; every `Opening` must carry it, and until the session is established the server only
    /// answers.
    Cookie { cookie: CookieValue, reply: Reply },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Client {
        versions: Versions,
        cookie: Option<CookieValue>,
    },
    Server {
        proof: Proof,
    },
}

/// One peer's session.
pub struct Session {
    role: Role,
    state: State,
    config: SessionConfig,
    facts: LinkFacts,
    /// The client's nonce (a server on a validated link learns it from the first `Opening`).
    nonce: Option<u64>,
    accept: Option<Accept>,
    close: Option<CloseReason>,
    created_ms: u32,
    last_recv_ms: u32,
    last_send_ms: Option<u32>,
    control_send: ReliableSend,
    control_recv: ReliableRecv,
    rpc_send: ReliableSend,
    rpc_recv: ReliableRecv,
    unreliable_send: UnreliableSend,
    unreliable_recv: UnreliableRecv,
    /// The unreliable lane's saved share, at most one whole message.
    unreliable_credit: usize,
    received: Received,
    flight: Flight,
    /// While the stream window is full: the packet that blocks it and when this side first saw it block (see `poll_packet`).
    window_stall: Option<(u16, u32)>,
    /// The packets one ack or expiry resolved, kept between calls so resolving allocates nothing once warm.
    resolved: Vec<(Record, bool)>,
    ack_owed: bool,
    srtt_ms: u32,
    events: VecDeque<Event>,
    event_bytes: usize,
    /// A reject or a goodbye to send at the next poll, which then closes the session.
    outbox: Option<Vec<u8>>,
    /// Client: a challenge came (a second one is a protocol error).
    challenged: bool,
    /// Packets of this side known to have arrived, and known lost, since the session began.
    fates: PacketFates,
}

/// How many of a session's packets are known to have arrived and to have been lost (every packet, whatever its lanes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PacketFates {
    pub delivered: u64,
    pub lost: u64,
}

impl Session {
    fn new(
        role: Role,
        state: State,
        config: SessionConfig,
        facts: LinkFacts,
        nonce: Option<u64>,
        now: u32,
    ) -> Session {
        let l = config.limits;
        Session {
            role,
            state,
            facts,
            nonce,
            accept: None,
            close: None,
            created_ms: now,
            last_recv_ms: now,
            last_send_ms: None,
            control_send: ReliableSend::new(l.control_message, l.control_message),
            control_recv: ReliableRecv::new(l.control_message, l.control_message),
            rpc_send: ReliableSend::new(l.rpc_send_queue, l.rpc_message),
            rpc_recv: ReliableRecv::new(l.rpc_receive_held, l.rpc_message),
            unreliable_send: UnreliableSend::new(l.unreliable_send_queue, l.unreliable_message),
            unreliable_recv: UnreliableRecv::default(),
            unreliable_credit: 0,
            received: Received::default(),
            flight: Flight::default(),
            window_stall: None,
            resolved: Vec::new(),
            ack_owed: false,
            srtt_ms: INITIAL_RTT_MS,
            events: VecDeque::new(),
            event_bytes: 0,
            outbox: None,
            challenged: false,
            fates: PacketFates::default(),
            config,
        }
    }

    /// A client that connects with `connect` over a link with `facts` (the connect's game and versions are also the
    /// `Hello`'s). `nonce` comes from the host's random source.
    ///
    /// # Errors
    /// The connect does not encode, or is larger than the control lane takes.
    pub fn client(
        mut config: SessionConfig,
        facts: LinkFacts,
        connect: Connect,
        nonce: u64,
        now: u32,
    ) -> Result<Session, NetError> {
        config.game.clone_from(&connect.game);
        let versions = connect.versions;
        let bytes = Control::Connect(Box::new(connect)).encode()?;
        let len = bytes.len();
        let state = if facts.validated {
            State::Opening
        } else {
            State::Hello
        };
        let role = Role::Client {
            versions,
            cookie: None,
        };
        let mut s = Session::new(role, state, config, facts, Some(nonce), now);
        if s.control_send.push(bytes).is_err() {
            return Err(NetError::Cap {
                what: "connect bytes",
                count: len as u64,
                cap: s.config.limits.control_message as u64,
            });
        }
        Ok(s)
    }

    /// A server session on a link that proved the client's address.
    ///
    /// # Errors
    /// The link is not validated: a plain link's sessions come from the gate.
    pub fn server(config: SessionConfig, facts: LinkFacts, now: u32) -> Result<Session, NetError> {
        if !facts.validated {
            return Err(NetError::Invalid(
                "a plain link's server sessions come from the gate",
            ));
        }
        let role = Role::Server { proof: Proof::Link };
        Ok(Session::new(role, State::Opening, config, facts, None, now))
    }

    /// A server session on a plain link, for the client whose cookie the gate verified.
    pub(crate) fn admitted(
        config: SessionConfig,
        nonce: u64,
        cookie: CookieValue,
        now: u32,
    ) -> Session {
        let proof = Proof::Cookie {
            cookie,
            reply: Reply::Spent,
        };
        let role = Role::Server { proof };
        let facts = LinkFacts::PLAIN;
        Session::new(role, State::Opening, config, facts, Some(nonce), now)
    }

    #[must_use]
    pub fn state(&self) -> State {
        self.state
    }

    #[must_use]
    pub fn facts(&self) -> LinkFacts {
        self.facts
    }

    /// The accept, once there is one.
    #[must_use]
    pub fn accepted(&self) -> Option<&Accept> {
        self.accept.as_ref()
    }

    #[must_use]
    pub fn close_reason(&self) -> Option<&CloseReason> {
        self.close.as_ref()
    }

    /// The client's nonce: drawn by the client, learnt by a server from the first `Opening`. What a ticket is bound to on
    /// both sides, with the server's address.
    #[must_use]
    pub fn nonce(&self) -> Option<u64> {
        self.nonce
    }

    /// How many of this side's packets are known to have arrived and to have been lost so far (a loss count over every packet,
    /// not only the ones with stream data).
    #[must_use]
    pub fn fates(&self) -> PacketFates {
        self.fates
    }

    /// The smoothed round trip, in milliseconds (200 before the first ack).
    #[must_use]
    pub fn round_trip_ms(&self) -> u32 {
        self.srtt_ms
    }

    /// Reliable message bytes queued that the peer has not acknowledged (an RPC carrier's backpressure).
    #[must_use]
    pub fn unacked_reliable(&self) -> usize {
        self.rpc_send.unacked()
    }

    /// Unreliable messages dropped because no packet could hold them any more.
    #[must_use]
    pub fn dropped_unreliable(&self) -> u64 {
        self.unreliable_send.dropped()
    }

    /// The bytes the events not yet taken hold (at most [`LaneLimits::event_bytes`]).
    #[must_use]
    pub fn held_event_bytes(&self) -> usize {
        self.event_bytes
    }

    /// The next event for the host.
    pub fn next_event(&mut self) -> Option<Event> {
        let event = self.events.pop_front()?;
        self.event_bytes -= event.held();
        Some(event)
    }

    /// Queues one message on the RPC lane or the unreliable lane. It is sent once the session is established. With
    /// [`Session::unacked_reliable`] this is what an RPC carrier needs: batches in, backpressure out.
    ///
    /// # Errors
    /// [`SendError::TooLarge`] past the lane's message limit, [`SendError::Busy`] with the lane's send queue full,
    /// [`SendError::Closed`].
    pub fn send(&mut self, data: Vec<u8>, delivery: Delivery) -> Result<(), SendError> {
        if self.state == State::Closed {
            return Err(SendError::Closed);
        }
        match delivery {
            Delivery::Reliable => self.rpc_send.push(data),
            Delivery::Unreliable => self.unreliable_send.push(data),
        }
    }

    /// Server: accepts the connect the host was asked about.
    ///
    /// # Errors
    /// The session is not waiting for a decision.
    pub fn accept(&mut self, accept: Accept) -> Result<(), NetError> {
        if self.state != State::Deciding || !matches!(self.role, Role::Server { .. }) {
            return Err(NetError::Invalid("accept outside a decision"));
        }
        let bytes = Control::Accept(accept).encode()?;
        self.control_send
            .push(bytes)
            .map_err(|_| NetError::Invalid("accept does not fit the control lane"))?;
        self.accept = Some(accept);
        self.state = State::Accepted;
        Ok(())
    }

    /// Server: challenges the connect the host was asked about with `nonce` (from the host's random source): the client's
    /// ticket comes back as [`Event::Ticket`], and the host decides then.
    ///
    /// # Errors
    /// The session is not waiting for a decision.
    pub fn challenge(&mut self, nonce: u64) -> Result<(), NetError> {
        if self.state != State::Deciding || !matches!(self.role, Role::Server { .. }) {
            return Err(NetError::Invalid("challenge outside a decision"));
        }
        let bytes = Control::Challenge(nonce).encode()?;
        self.control_send
            .push(bytes)
            .map_err(|_| NetError::Invalid("challenge does not fit the control lane"))?;
        self.state = State::Challenging;
        Ok(())
    }

    /// Client: answers the server's challenge with `ticket` (made for the challenge's nonce; empty when the client has none).
    ///
    /// # Errors
    /// No challenge came, or the ticket is longer than a ticket may be.
    pub fn answer(&mut self, ticket: Vec<u8>) -> Result<(), NetError> {
        if !self.challenged || self.accept.is_some() || !matches!(self.role, Role::Client { .. }) {
            return Err(NetError::Invalid("an answer without a challenge"));
        }
        let bytes = Control::Ticket(ticket).encode()?;
        self.control_send
            .push(bytes)
            .map_err(|_| NetError::Invalid("ticket does not fit the control lane"))
    }

    /// Server: refuses the connect. The reject goes out at the next poll, which closes the session.
    pub fn reject(&mut self, code: RejectCode, text: &str) {
        if matches!(self.role, Role::Server { .. })
            && matches!(
                self.state,
                State::Opening | State::Deciding | State::Challenging
            )
            && let Some(nonce) = self.nonce
            && let Ok(bytes) = Reject::new(nonce, code, text).encode()
        {
            self.outbox = Some(bytes);
            return;
        }
        self.finish(CloseReason::Local);
    }

    /// Says goodbye. It goes out at the next poll, which closes the session.
    pub fn disconnect(&mut self, reason: &str) {
        if self.state == State::Closed {
            return;
        }
        let token = self.accept.map(|a| a.session).or(self.nonce);
        match token.map(|t| Disconnect::new(t, reason).encode()) {
            Some(Ok(bytes)) => self.outbox = Some(bytes),
            _ => self.finish(CloseReason::Local),
        }
    }

    /// Takes one datagram from the peer.
    ///
    /// Ignored, with no effect at all: a datagram past this side's packet limit, one that is not engine framing of this
    /// version, a kind this side does not take in its state, a packet kind that does not fit the link (`TokenData` on a
    /// validated link, `Data` on a plain one), and one with the wrong nonce, cookie or token. A packet that passes those
    /// checks and is then malformed or past a cap closes the session ([`CloseReason::Protocol`]).
    pub fn receive(&mut self, data: &[u8], now: u32) {
        if self.state == State::Closed || data.len() > self.recv_limit() {
            return;
        }
        let Ok(datagram) = framing::parse(data) else {
            return;
        };
        match (self.role, datagram) {
            (Role::Client { .. }, Datagram::Cookie(c)) => self.on_cookie(&c, now),
            (Role::Client { .. }, Datagram::Reject(r))
                if Some(r.token) == self.nonce
                    && matches!(self.state, State::Hello | State::Opening) =>
            {
                self.close_with(CloseReason::Rejected {
                    code: r.code,
                    text: r.text,
                });
            }
            (Role::Client { .. }, Datagram::Data { token, body })
                if matches!(self.state, State::Opening | State::Established)
                    && self.token_fits(token) =>
            {
                self.on_body(body, now);
            }
            (
                Role::Server { proof },
                Datagram::Opening {
                    nonce,
                    cookie,
                    body,
                },
            ) => {
                let proven = match proof {
                    Proof::Link => true,
                    Proof::Cookie { cookie: c, .. } => cookie == Some(c),
                };
                if !proven || *self.nonce.get_or_insert(nonce) != nonce {
                    return;
                }
                self.owe_reply(data.len());
                self.on_body(body, now);
            }
            (Role::Server { .. }, Datagram::Data { token, body })
                if self.token_fits(token)
                    && (self.state == State::Established
                        || self.state == State::Accepted && Body::read(body).is_ok()) =>
            {
                self.state = State::Established;
                self.on_body(body, now);
            }
            (_, Datagram::Disconnect(d)) if self.token_is(d.token) => {
                self.close_with(CloseReason::Disconnected(d.reason));
            }
            _ => {}
        }
    }

    /// The largest datagram this side reads: the accept's limit once there is an accept, [`DEFAULT_MAX_PACKET`] before.
    fn recv_limit(&self) -> usize {
        usize::from(self.accept.map_or(DEFAULT_MAX_PACKET, |a| a.max_packet)).min(MAX_PACKET)
    }

    /// The largest packet this side sends: the accept's limit once this side is established (so the packet that carries the
    /// accept still fits the client's limit before it), [`DEFAULT_MAX_PACKET`] before.
    fn send_limit(&self) -> usize {
        let limit = match self.accept {
            Some(a) if self.state == State::Established => a.max_packet,
            _ => DEFAULT_MAX_PACKET,
        };
        usize::from(limit).min(MAX_PACKET)
    }

    /// `token` is the nonce or the session id.
    fn token_is(&self, token: u64) -> bool {
        Some(token) == self.nonce || self.accept.is_some_and(|a| a.session == token)
    }

    /// A packet's token fits the link: none on a validated link, the nonce or the session id on a plain one.
    fn token_fits(&self, token: Option<u64>) -> bool {
        match token {
            None => self.facts.validated,
            Some(t) => !self.facts.validated && self.token_is(t),
        }
    }

    fn owe_reply(&mut self, len: usize) {
        if let Role::Server {
            proof: Proof::Cookie { reply, .. },
        } = &mut self.role
        {
            *reply = Reply::Owed(len);
        }
    }

    fn on_cookie(&mut self, c: &framing::Cookie, now: u32) {
        let Role::Client { versions, cookie } = &mut self.role else {
            return;
        };
        if self.state != State::Hello
            || Some(c.nonce) != self.nonce
            || c.version != ENGINE_FRAMING
            || !versions.contains(c.version)
        {
            return;
        }
        *cookie = Some(c.cookie);
        self.state = State::Opening;
        self.last_recv_ms = now;
        self.last_send_ms = None;
    }

    fn on_body(&mut self, bytes: &[u8], now: u32) {
        if let Err(e) = self.read_body(bytes, now) {
            self.close_with(CloseReason::Protocol(e));
        }
    }

    fn read_body(&mut self, bytes: &[u8], now: u32) -> Result<(), NetError> {
        let body = Body::read(bytes)?;
        let seq = body.header.seq;
        if !self.received.is_new(seq) {
            return Ok(());
        }
        // Until established only the control lane is read (module docs).
        let only_control = Lane::ALL[1..].iter().all(|&l| body.section(l).is_empty());
        if self.state != State::Established && !only_control {
            return Ok(());
        }
        let control = Fragment::read_all(body.section(Lane::Control))?;
        let rpc = Fragment::read_all(body.section(Lane::Rpc))?;
        let unreliable = Unreliable::read_all(body.section(Lane::Unreliable))?;
        let cap = self.config.limits.unreliable_message;
        if let Some(m) = unreliable.iter().find(|m| m.bytes.len() > cap) {
            return Err(NetError::Cap {
                what: "unreliable message bytes",
                count: m.bytes.len() as u64,
                cap: cap as u64,
            });
        }
        let newest = self.received.is_newest(seq);
        self.received.note(seq);
        self.last_recv_ms = now;
        self.ack_owed |= body.has_sections();
        let (ack, mask) = (body.header.ack, body.header.mask);
        let rtt = self.flight.on_ack(ack, mask, now, &mut self.resolved);
        if let Some(rtt) = rtt {
            self.srtt_ms = (self.srtt_ms * 7 + rtt.min(10_000)) / 8;
        }
        // Only the newest packet the peer sent says what it had read when it sent it: an older one may have waited on the way.
        if newest && mask != 0 {
            self.flight
                .expire_unread(ack, now, self.loss_timeout(), &mut self.resolved);
        }
        self.apply_resolved()?;
        for f in &control {
            self.control_recv.fragment(f)?;
        }
        while let Some(message) = self.control_recv.pop() {
            self.on_control(&message)?;
            if self.state == State::Closed || self.outbox.is_some() {
                return Ok(());
            }
        }
        for f in &rpc {
            self.rpc_recv.fragment(f)?;
        }
        while let Some(data) = self.rpc_recv.pop() {
            self.push_event(Event::Message {
                delivery: Delivery::Reliable,
                data,
            })?;
        }
        for m in unreliable {
            if self.unreliable_recv.accept(m.seq) {
                self.push_event(Event::Message {
                    delivery: Delivery::Unreliable,
                    data: m.bytes.to_vec(),
                })?;
            }
        }
        for lane in StreamLane::ALL {
            let section = body.section(lane.lane());
            if !section.is_empty() {
                self.push_event(Event::Stream {
                    lane,
                    seq,
                    data: section.to_vec(),
                })?;
            }
        }
        Ok(())
    }

    fn on_control(&mut self, bytes: &[u8]) -> Result<(), NetError> {
        match (self.role, self.state, Control::decode(bytes)?) {
            (Role::Client { .. }, State::Opening, Control::Accept(a)) => {
                self.accept = Some(a);
                self.state = State::Established;
                self.push_event(Event::Accepted(a))
            }
            (Role::Client { .. }, State::Opening, Control::Challenge(nonce))
                if !self.challenged =>
            {
                self.challenged = true;
                self.push_event(Event::Challenge(nonce))
            }
            (Role::Server { .. }, State::Challenging, Control::Ticket(ticket)) => {
                self.state = State::Deciding;
                self.push_event(Event::Ticket(ticket))
            }
            (Role::Server { .. }, State::Opening, Control::Connect(c)) => {
                if c.game != self.config.game {
                    let text = format!("the server runs {}", self.config.game);
                    self.reject(RejectCode::Game, &text);
                } else if !c.versions.contains(ENGINE_FRAMING) {
                    let text = format!(
                        "server speaks network version {ENGINE_FRAMING}, you have {}",
                        c.versions.max
                    );
                    self.reject(RejectCode::Version, &text);
                } else {
                    self.state = State::Deciding;
                    self.push_event(Event::ConnectRequest(c))?;
                }
                Ok(())
            }
            _ => Err(NetError::Invalid("control message out of turn")),
        }
    }

    /// Holds one event for the host.
    ///
    /// # Errors
    /// The events held would pass [`MAX_EVENTS`] or [`LaneLimits::event_bytes`].
    fn push_event(&mut self, event: Event) -> Result<(), NetError> {
        if self.events.len() >= MAX_EVENTS {
            return Err(NetError::Cap {
                what: "session events",
                count: self.events.len() as u64 + 1,
                cap: MAX_EVENTS as u64,
            });
        }
        let bytes = self.event_bytes + event.held();
        let cap = self.config.limits.event_bytes;
        if bytes > cap {
            return Err(NetError::Cap {
                what: "session event bytes",
                count: bytes as u64,
                cap: cap as u64,
            });
        }
        self.event_bytes = bytes;
        self.events.push_back(event);
        Ok(())
    }

    /// Hands what the resolved packets carried back to their lanes, and tells the host about the ones with stream data.
    ///
    /// # Errors
    /// The host let the events pile up past their cap, so a notify cannot be held.
    fn apply_resolved(&mut self) -> Result<(), NetError> {
        let mut resolved = std::mem::take(&mut self.resolved);
        let mut result = Ok(());
        for (record, arrived) in resolved.drain(..) {
            if arrived {
                self.fates.delivered += 1;
            } else {
                self.fates.lost += 1;
            }
            for (lane, frag) in &record.frags {
                let sender = self.reliable(*lane);
                if arrived {
                    sender.acked(frag);
                } else {
                    sender.lost(frag);
                }
            }
            if record.streams && result.is_ok() {
                result = self.push_event(Event::Notify {
                    seq: record.seq,
                    delivered: arrived,
                });
            }
        }
        self.resolved = resolved;
        result
    }

    fn reliable(&mut self, lane: Lane) -> &mut ReliableSend {
        if lane == Lane::Control {
            &mut self.control_send
        } else {
            &mut self.rpc_send
        }
    }

    /// Closes the session and tells the host why (this one event may pass the caps).
    fn close_with(&mut self, reason: CloseReason) {
        if self.state == State::Closed {
            return;
        }
        self.events.push_back(Event::Closed(reason.clone()));
        self.finish(reason);
    }

    fn finish(&mut self, reason: CloseReason) {
        self.state = State::Closed;
        self.close.get_or_insert(reason);
        self.outbox = None;
    }

    fn loss_timeout(&self) -> u32 {
        (self.srtt_ms * 2 + 64).clamp(100, 2_000)
    }

    /// Sends at most one datagram: a pending reject or goodbye, a `Hello`, or a packet with whatever the lanes have.
    /// Also counts timeouts and lost packets.
    pub fn poll(
        &mut self,
        link: &mut dyn Link,
        streams: &mut dyn StreamSource,
        now: u32,
    ) -> Polled {
        if self.state == State::Closed {
            return Polled::Closed;
        }
        let (limit, since) = if self.state == State::Established {
            (IDLE_TIMEOUT_MS, self.last_recv_ms)
        } else {
            (CONNECT_TIMEOUT_MS, self.created_ms)
        };
        if now.wrapping_sub(since) >= limit {
            self.close_with(CloseReason::TimedOut);
            return Polled::Closed;
        }
        if let Some(datagram) = &self.outbox {
            let room = link
                .max_datagram()
                .min(self.answer_limit().unwrap_or(usize::MAX));
            if room == 0 {
                return Polled::Idle;
            }
            let len = datagram.len();
            let sent = len <= room && link.send(datagram).is_ok();
            self.finish(CloseReason::Local);
            return if sent {
                Polled::Sent(len)
            } else {
                Polled::Closed
            };
        }
        self.flight
            .expire(now, self.loss_timeout(), &mut self.resolved);
        if let Err(e) = self.apply_resolved() {
            self.close_with(CloseReason::Protocol(e));
            return Polled::Closed;
        }
        if self.state == State::Hello {
            return self.poll_hello(link, now);
        }
        self.poll_packet(link, streams, now)
    }

    fn poll_hello(&mut self, link: &mut dyn Link, now: u32) -> Polled {
        if self
            .last_send_ms
            .is_some_and(|t| now.wrapping_sub(t) < HELLO_RESEND_MS)
            || link.max_datagram() < HELLO_BYTES
        {
            return Polled::Idle;
        }
        let (Role::Client { versions, .. }, Some(nonce)) = (self.role, self.nonce) else {
            return Polled::Idle;
        };
        let hello = Hello {
            versions,
            nonce,
            game: self.config.game.clone(),
        };
        let Ok(datagram) = hello.encode() else {
            return Polled::Idle;
        };
        match link.send(&datagram) {
            Ok(()) => {
                self.last_send_ms = Some(now);
                Polled::Sent(datagram.len())
            }
            Err(NoRoom) => Polled::NoRoom,
        }
    }

    /// The size of the one datagram a server that may only answer may send now; `None` when it may send freely.
    fn answer_limit(&self) -> Option<usize> {
        match self.role {
            Role::Server {
                proof: Proof::Cookie { reply, .. },
            } if self.state != State::Established => Some(match reply {
                Reply::Owed(len) => len,
                Reply::Spent => 0,
            }),
            _ => None,
        }
    }

    /// What this side's next packet starts with.
    fn prefix(&self) -> Prefix {
        let nonce = self.nonce.unwrap_or_default();
        match self.role {
            Role::Client { cookie, .. } if self.state != State::Established => {
                Prefix::Opening { nonce, cookie }
            }
            _ if self.facts.validated => Prefix::Data,
            _ => Prefix::TokenData {
                token: match self.accept {
                    Some(a) if self.state == State::Established => a.session,
                    _ => nonce,
                },
            },
        }
    }

    fn poll_packet(
        &mut self,
        link: &mut dyn Link,
        streams: &mut dyn StreamSource,
        now: u32,
    ) -> Polled {
        // A peer that keeps the session alive with packets that never ack the stream would hold the window shut for ever:
        // when one packet has blocked it for the idle timeout, the session closes.
        match self.flight.stream_window_blocker() {
            Some(blocker) => {
                let since = match self.window_stall {
                    Some((seq, since)) if seq == blocker => since,
                    _ => now,
                };
                self.window_stall = Some((blocker, since));
                if now.wrapping_sub(since) >= IDLE_TIMEOUT_MS {
                    self.close_with(CloseReason::TimedOut);
                    return Polled::Closed;
                }
                return Polled::Idle;
            }
            None => self.window_stall = None,
        }
        let mut max = link.max_datagram().min(self.send_limit());
        if let Some(answer) = self.answer_limit() {
            max = max.min(answer);
        }
        let prefix = self.prefix();
        let opening = matches!(prefix, Prefix::Opening { .. });
        if max < prefix.size() + BODY_HEADER {
            return Polled::Idle;
        }
        self.flight.make_room(&mut self.resolved);
        if let Err(e) = self.apply_resolved() {
            self.close_with(CloseReason::Protocol(e));
            return Polled::Closed;
        }

        let seq = self.flight.next_seq;
        let credit = self.unreliable_credit;
        let mut fill = Fill::new(max - prefix.size() - BODY_HEADER);
        self.fill(&mut fill, streams, seq);

        // A bare packet goes out only to keep an established session alive, or from a client on a plain link that waits
        // for the accept (the server may only answer what it gets).
        let wait = if opening && !self.facts.validated {
            Some(OPENING_PING_MS)
        } else if self.state == State::Established {
            Some(KEEPALIVE_MS)
        } else {
            None
        };
        let quiet = wait.is_some_and(|wait| {
            self.last_send_ms
                .is_none_or(|t| now.wrapping_sub(t) >= wait)
        });
        let payload = fill.sections.iter().any(|s| !s.is_empty());
        if !payload && !self.ack_owed && !quiet {
            return Polled::Idle;
        }

        let mut datagram = Vec::with_capacity(max);
        prefix.write(&mut datagram);
        let pad_to = if opening && !self.facts.validated {
            max.min(HELLO_BYTES)
        } else {
            0
        };
        let body = Body {
            header: self.received.header(seq),
            sections: fill.sections.each_ref().map(Vec::as_slice),
        };
        let written = body.write(&mut datagram, pad_to);
        let sent = written.is_ok() && datagram.len() <= max && link.send(&datagram).is_ok();
        if !sent {
            for (lane, frag) in &fill.frags {
                self.reliable(*lane).lost(frag);
            }
            self.unreliable_send.give_back(fill.unreliable);
            self.unreliable_credit = credit;
            if fill.streams {
                streams.unsent(seq);
            }
            return Polled::NoRoom;
        }
        self.flight.push(Record {
            seq,
            sent_ms: now,
            frags: fill.frags,
            streams: fill.streams,
        });
        self.ack_owed = false;
        self.last_send_ms = Some(now);
        if let Role::Server {
            proof: Proof::Cookie { reply, .. },
        } = &mut self.role
        {
            *reply = Reply::Spent;
        }
        Polled::Sent(datagram.len())
    }

    /// Fills the lanes of one packet: every lane up to its budget in order, then the message lanes into what is left.
    fn fill(&mut self, fill: &mut Fill, streams: &mut dyn StreamSource, seq: u16) {
        let full = self.state == State::Established;
        let budgets = self.config.budgets;
        for lane in Lane::ALL {
            if lane != Lane::Control && !full {
                break;
            }
            let share = fill.payload * budgets.of(lane) / 100;
            if lane == Lane::Unreliable {
                // The lane saves its share up to one whole message, so a message larger than one packet's share leaves
                // once enough is saved, and the lane stays at its budget on average.
                let most = share.max(
                    self.config.limits.unreliable_message + UNRELIABLE_HEADER + SECTION_HEADER,
                );
                self.unreliable_credit = (self.unreliable_credit + share).min(most);
                let room = self.unreliable_credit.min(fill.left);
                let used = self.fill_lane(fill, lane, room, streams, seq);
                self.unreliable_credit -= used;
            } else {
                self.fill_lane(fill, lane, share.min(fill.left), streams, seq);
            }
        }
        for lane in [Lane::Control, Lane::Rpc, Lane::Unreliable] {
            if lane != Lane::Control && !full {
                break;
            }
            self.fill_lane(fill, lane, fill.left, streams, seq);
        }
    }

    /// Lets `lane` write into at most `room` more bytes of the packet (its section header included); returns what it took.
    fn fill_lane(
        &mut self,
        fill: &mut Fill,
        lane: Lane,
        room: usize,
        streams: &mut dyn StreamSource,
        seq: u16,
    ) -> usize {
        let mut section = std::mem::take(&mut fill.sections[lane.index()]);
        let header = if section.is_empty() {
            SECTION_HEADER
        } else {
            0
        };
        let before = section.len();
        let content_room = room
            .saturating_sub(header)
            .min(usize::from(u16::MAX) - before);
        let mut used = 0;
        if content_room > 0 {
            match lane {
                Lane::Control => {
                    self.control_send
                        .write(lane, content_room, &mut section, &mut fill.frags);
                }
                Lane::Rpc => {
                    self.rpc_send
                        .write(lane, content_room, &mut section, &mut fill.frags);
                }
                Lane::Unreliable => {
                    let packet_room = fill.payload.saturating_sub(SECTION_HEADER);
                    self.unreliable_send.write(
                        content_room,
                        packet_room,
                        &mut section,
                        &mut fill.unreliable,
                    );
                }
                Lane::Ghosts => {
                    fill.stream(streams, StreamLane::Ghosts, seq, content_room, &mut section)
                }
                Lane::Moves => {
                    fill.stream(streams, StreamLane::Moves, seq, content_room, &mut section)
                }
                Lane::Channels => {
                    fill.stream(
                        streams,
                        StreamLane::Channels,
                        seq,
                        content_room,
                        &mut section,
                    );
                }
            }
            let grew = section.len() - before;
            if grew > 0 {
                used = grew + header;
                fill.left -= used;
            }
        }
        fill.sections[lane.index()] = section;
        used
    }
}

/// One packet being filled.
struct Fill {
    /// The payload room of the whole packet (after the prefix and the body header).
    payload: usize,
    left: usize,
    sections: [Vec<u8>; 6],
    frags: Vec<(Lane, FragRef)>,
    unreliable: Vec<(u16, Vec<u8>)>,
    streams: bool,
}

impl Fill {
    fn new(payload: usize) -> Fill {
        Fill {
            payload,
            left: payload,
            sections: Default::default(),
            frags: Vec::new(),
            unreliable: Vec::new(),
            streams: false,
        }
    }

    fn stream(
        &mut self,
        streams: &mut dyn StreamSource,
        lane: StreamLane,
        seq: u16,
        room: usize,
        section: &mut Vec<u8>,
    ) {
        streams.write(lane, seq, room, section);
        if section.len() > room {
            section.clear();
        }
        self.streams |= !section.is_empty();
    }
}
