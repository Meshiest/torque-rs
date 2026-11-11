//! The client's end: one connection to one server over a `quinn_proto::Endpoint`, the engine's datagrams on QUIC DATAGRAM
//! frames both ways, as the client's socket ([`Datagrams`]). The server's key is checked against the identity given
//! ([`Trust::Identity`]) or the one remembered for its address ([`Trust::Known`], trust on first use); the client's view of
//! the link (connected, the identity, a refusal's text) is read through a [`QuicWatch`].

use std::cell::RefCell;
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use bytes::{Bytes, BytesMut};
use quinn_proto::{
    Connection, ConnectionHandle, DatagramEvent, Endpoint, EndpointEvent, Event, SendDatagramError,
    VarInt,
};
use torque_client::net::Datagrams;
use torque_net::engine::{ChannelBinding, LinkFacts, LinkKind, ServerIdentity};
use torque_server::net::transport::BindError;

use crate::clock::{Clock, SystemClock};
use crate::io::{Io, UdpIo};
use crate::known::{KnownError, KnownServers};
use crate::server::READS_PER_PASS;
use crate::tls::{Expected, PinVerifier, QUIC_MAX_DATAGRAM, QuicLimits};

const RECV_BYTES: usize = 1 << 16;

/// The most application datagrams a client holds for its host before it reads them; a datagram past it is dropped.
pub const CLIENT_INBOX: usize = 1_024;

/// The most bytes of application datagrams a client holds for its host. A datagram over [`QUIC_MAX_DATAGRAM`] is dropped on
/// receipt whatever the room.
pub const CLIENT_INBOX_BYTES: usize = CLIENT_INBOX * QUIC_MAX_DATAGRAM;

/// How a client decides whether the server's key is the right one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    /// The server must be this identity (from a listing, an invite, a typed hash).
    Identity(ServerIdentity),
    /// The identity remembered for the address in this known-servers file, if any; else the one presented is trusted and
    /// remembered there.
    Known(PathBuf),
}

/// What the link is up to, as the client's host sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkState {
    /// The handshake is done and datagrams flow.
    pub connected: bool,
    /// The server's identity: the one expected from the start, or the one seen and trusted on first use.
    pub identity: Option<ServerIdentity>,
    /// Why the connection ended, if it did (a refused identity names both hashes).
    pub error: Option<String>,
    /// Something to tell the player that did not end the connection (the known-servers file could not be written).
    pub note: Option<String>,
}

/// A view of a [`QuicClient`]'s link state, handed to whoever connected.
#[derive(Debug, Clone, Default)]
pub struct QuicWatch(Rc<RefCell<LinkState>>);

impl QuicWatch {
    /// The state now.
    #[must_use]
    pub fn state(&self) -> LinkState {
        self.0.borrow().clone()
    }
}

/// A client endpoint over the socket `I` with the clock `C` (module docs). [`QuicDatagrams`] is the real one.
pub struct QuicClient<I: Io, C: Clock> {
    io: I,
    clock: C,
    endpoint: Endpoint,
    server: SocketAddrV4,
    conn: Option<(ConnectionHandle, Connection)>,
    verifier: Arc<PinVerifier>,
    known: Option<KnownServers>,
    binding: Option<ChannelBinding>,
    max_datagram: Option<usize>,
    timeout: Option<Instant>,
    inbox: VecDeque<Bytes>,
    inbox_bytes: usize,
    watch: QuicWatch,
    stray: u64,
    /// Datagrams dropped at the full inbox, or for their size.
    inbox_dropped: u64,
    buf: Vec<u8>,
    recv_buf: Vec<u8>,
}

/// The client endpoint over a real UDP socket and the system clock.
pub type QuicDatagrams = QuicClient<UdpIo, SystemClock>;

impl<I: Io, C: Clock> QuicClient<I, C> {
    /// Opens a connection to `server` over the bound socket `io`, the server's key checked as `trust` says. The handshake
    /// runs as the socket is pumped; [`QuicClient::watch`] says when it is done or why it failed.
    ///
    /// # Errors
    /// The socket is not bound, the known-servers file cannot be read or is full with this server not in it, or the
    /// endpoint refused the connect.
    pub fn connect(
        io: I,
        clock: C,
        server: SocketAddrV4,
        trust: Trust,
        limits: &QuicLimits,
        seed: Option<[u8; 32]>,
    ) -> Result<QuicClient<I, C>, String> {
        if io.local_addr().is_none() {
            return Err("the client's socket is not bound".to_string());
        }
        let (expected, known) = match trust {
            Trust::Identity(id) => (Some((id, Expected::Given)), None),
            Trust::Known(path) => {
                let known = KnownServers::load(&path).map_err(|e| e.to_string())?;
                let expected = known
                    .get(SocketAddr::V4(server))
                    .map(|id| (id, Expected::Remembered));
                if expected.is_none() && !known.has_room() {
                    // No pin is ever forgotten on its own: the user makes room first.
                    return Err(KnownError::Full(path).to_string());
                }
                (expected, Some(known))
            }
        };
        let verifier = Arc::new(PinVerifier::new(SocketAddr::V4(server), expected));
        let config =
            crate::tls::client_config(Arc::clone(&verifier), limits).map_err(|e| e.to_string())?;
        let mut endpoint = Endpoint::new(
            Arc::new(crate::tls::endpoint_config(seed)),
            None,
            true,
            None,
        );
        let now = clock.now();
        let conn = endpoint
            .connect(
                now,
                config,
                SocketAddr::V4(server),
                &server.ip().to_string(),
            )
            .map_err(|e| format!("the connection to {server}: {e}"))?;
        let watch = QuicWatch::default();
        watch.0.borrow_mut().identity = expected.map(|(id, _)| id);
        let mut client = QuicClient {
            io,
            clock,
            endpoint,
            server,
            conn: Some(conn),
            verifier,
            known,
            binding: None,
            max_datagram: None,
            timeout: None,
            inbox: VecDeque::new(),
            inbox_bytes: 0,
            watch,
            stray: 0,
            inbox_dropped: 0,
            buf: Vec::with_capacity(RECV_BYTES),
            recv_buf: vec![0; RECV_BYTES],
        };
        client.drive(now);
        Ok(client)
    }

    /// The link's state, to read as the handshake goes.
    #[must_use]
    pub fn watch(&self) -> QuicWatch {
        self.watch.clone()
    }

    /// Whether the handshake is done and the connection open.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.watch.0.borrow().connected && self.conn.is_some()
    }

    /// Why the connection ended, if it did.
    #[must_use]
    pub fn error(&self) -> Option<String> {
        self.watch.0.borrow().error.clone()
    }

    /// The server's identity: the one expected, or the one seen and trusted.
    #[must_use]
    pub fn identity(&self) -> Option<ServerIdentity> {
        self.watch.0.borrow().identity
    }

    /// The socket.
    pub fn io_mut(&mut self) -> &mut I {
        &mut self.io
    }

    /// Moves to another socket (a new address: the connection migrates to it).
    pub fn rebind(&mut self, io: I) {
        self.io = io;
        if let Some((_, conn)) = self.conn.as_mut() {
            conn.local_address_changed();
        }
        let now = self.clock.now();
        self.drive(now);
    }

    /// Hangs up now.
    pub fn close(&mut self) {
        let now = self.clock.now();
        if let Some((_, conn)) = self.conn.as_mut() {
            conn.close(now, VarInt::from_u32(0), Bytes::from_static(b"closed"));
        }
        self.drive(now);
        // The close went out; nothing waits for the draining period.
        if let Some((ch, _)) = self.conn.take() {
            self.endpoint.handle_event(ch, EndpointEvent::drained());
        }
        self.watch.0.borrow_mut().connected = false;
    }

    /// Reads what the socket has and moves the connection along.
    pub fn pump(&mut self) {
        let now = self.clock.now();
        for _ in 0..READS_PER_PASS {
            let Some((len, from)) = self.io.recv(&mut self.recv_buf) else {
                break;
            };
            let data = BytesMut::from(&self.recv_buf[..len]);
            self.buf.clear();
            match self
                .endpoint
                .handle(now, from, None, None, data, &mut self.buf)
            {
                Some(DatagramEvent::ConnectionEvent(ch, event)) => {
                    if let Some((_, conn)) = self.conn.as_mut().filter(|(mine, _)| *mine == ch) {
                        conn.handle_event(event);
                    }
                }
                Some(DatagramEvent::Response(transmit)) => {
                    self.io
                        .send(transmit.destination, &self.buf[..transmit.size]);
                }
                Some(DatagramEvent::NewConnection(incoming)) => self.endpoint.ignore(incoming),
                None => self.stray += 1,
            }
        }
        if let Some(t) = self.timeout
            && t <= now
            && let Some((_, conn)) = self.conn.as_mut()
        {
            conn.handle_timeout(now);
        }
        self.drive(now);
    }

    fn drive(&mut self, now: Instant) {
        let Some((ch, conn)) = self.conn.as_mut() else {
            return;
        };
        let ch = *ch;
        let mut drained = false;
        while let Some(event) = conn.poll_endpoint_events() {
            drained |= event.is_drained();
            if let Some(back) = self.endpoint.handle_event(ch, event) {
                conn.handle_event(back);
            }
        }
        let mut lost = None;
        while let Some(event) = conn.poll() {
            match event {
                Event::Connected => {
                    let mut binding = [0; 32];
                    self.binding = conn
                        .crypto_session()
                        .export_keying_material(&mut binding, ChannelBinding::LABEL, &[])
                        .ok()
                        .map(|()| ChannelBinding(binding));
                    let seen = self.verifier.seen();
                    let mut state = self.watch.0.borrow_mut();
                    state.connected = true;
                    state.identity = seen.or(state.identity);
                    if let (Some(known), Some(id)) = (self.known.as_mut(), seen)
                        && self.verifier.expected().is_none()
                        && let Err(e) = known.remember(SocketAddr::V4(self.server), id)
                    {
                        state.note = Some(format!(
                            "the server's identity could not be remembered: {e}"
                        ));
                    }
                }
                Event::ConnectionLost { reason } => lost = Some(reason.to_string()),
                Event::DatagramReceived => {
                    while let Some(data) = conn.datagrams().recv() {
                        if data.len() > QUIC_MAX_DATAGRAM
                            || self.inbox.len() >= CLIENT_INBOX
                            || self.inbox_bytes + data.len() > CLIENT_INBOX_BYTES
                        {
                            self.inbox_dropped += 1;
                            continue;
                        }
                        self.inbox_bytes += data.len();
                        self.inbox.push_back(data);
                    }
                }
                Event::HandshakeDataReady | Event::Stream(_) | Event::DatagramsUnblocked => {}
            }
        }
        loop {
            self.buf.clear();
            let Some(transmit) = conn.poll_transmit(now, 1, &mut self.buf) else {
                break;
            };
            self.io
                .send(transmit.destination, &self.buf[..transmit.size]);
        }
        self.max_datagram = conn.datagrams().max_size();
        self.timeout = conn.poll_timeout();
        if let Some(reason) = lost {
            let mut state = self.watch.0.borrow_mut();
            state.connected = false;
            state.error = Some(self.verifier.refusal().unwrap_or(reason));
        }
        if drained || conn.is_drained() {
            self.endpoint.handle_event(ch, EndpointEvent::drained());
            self.conn = None;
        }
    }
}

impl<I: Io, C: Clock> Datagrams for QuicClient<I, C> {
    /// A datagram to the server rides the connection; one to anyone else is not sent.
    fn send_to(&mut self, to: SocketAddrV4, bytes: &[u8]) -> bool {
        if to != self.server {
            return false;
        }
        let Some((_, conn)) = self.conn.as_mut() else {
            return false;
        };
        if conn.is_closed() {
            return false;
        }
        match conn.datagrams().send(Bytes::copy_from_slice(bytes), false) {
            Ok(()) => {
                let now = self.clock.now();
                self.drive(now);
                true
            }
            Err(
                SendDatagramError::Blocked(_)
                | SendDatagramError::TooLarge
                | SendDatagramError::Disabled
                | SendDatagramError::UnsupportedByPeer,
            ) => false,
        }
    }

    fn recv(&mut self) -> Option<(SocketAddrV4, Vec<u8>)> {
        if self.inbox.is_empty() {
            self.pump();
        }
        let data = self.inbox.pop_front()?;
        self.inbox_bytes = self.inbox_bytes.saturating_sub(data.len());
        Some((self.server, data.to_vec()))
    }

    fn port(&self) -> u16 {
        self.io.local_addr().map_or(0, |a| a.port())
    }

    fn lost(&self) -> u64 {
        self.io.lost() + self.stray + self.inbox_dropped
    }

    /// The QUIC link: the server's address validated by the handshake, TLS 1.3, QUIC's congestion control, and the
    /// server's identity (expected from the start, or seen as the handshake ended). Datagrams are the connection's current
    /// size, at most [`QUIC_MAX_DATAGRAM`].
    fn link_kind(&self) -> LinkKind {
        LinkKind {
            facts: LinkFacts {
                validated: true,
                secure: true,
                congestion_controlled: true,
                server_identity: self.identity(),
            },
            max_datagram: self
                .max_datagram
                .unwrap_or(QUIC_MAX_DATAGRAM)
                .min(QUIC_MAX_DATAGRAM),
        }
    }

    fn channel_binding(&self) -> Option<ChannelBinding> {
        self.binding
    }
}

/// Opens a client socket on `ip` at any port, for [`QuicDatagrams`].
///
/// # Errors
/// The system refused the bind.
pub fn client_socket(ip: IpAddr) -> Result<UdpIo, BindError> {
    UdpIo::open(ip, 0)
}
