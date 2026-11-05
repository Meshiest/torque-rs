//! The room service: [`Rooms`] over torque-server's web transport, which takes every connection (HTTP, the page's files
//! when it serves them, and the WebSocket upgrade at `/ws`) with its TLS, its Host and Origin checks, its caps per address
//! and its meters of each socket's bytes and frames ([`service_limits`]). Each binary message on a socket is one room
//! message; the transport's stand-in address names the socket.
//!
//! A pass ([`Service::pass`]) is bounded: the transport's own pass is, at most [`MESSAGES_PER_PASS`] messages are handed
//! to the rooms, and the lifetimes are looked at every [`TICK`].

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use torque_net::cap::NET_MAX_DATAGRAM;
use torque_server::net::room::MAX_MESSAGE;
use torque_server::net::transport::{BindError, BindPolicy, Recv, Transport};
use torque_server::net::web::{WebLimits, WebSetup, WebTransport};

use crate::rooms::{Ms, Out, Rooms};

/// The most messages one pass hands to the rooms.
pub const MESSAGES_PER_PASS: usize = 1024;
/// How often the lifetimes are looked at.
pub const TICK: Duration = Duration::from_millis(250);

/// The web transport's caps for a room service: room messages instead of datagrams (at most [`MAX_MESSAGE`] bytes, a
/// signaling rate rather than a game's), and more sockets in all.
pub fn service_limits() -> WebLimits {
    WebLimits {
        max_connections: 2048,
        connections_per_ip: 8,
        sockets_per_ip: 8,
        connects_per_ip_per_s: 8,
        idle_timeout: Duration::from_secs(30),
        messages_per_s: 128,
        bytes_per_s: 256 << 10,
        bytes_per_pass: 16 * (MAX_MESSAGE + 14),
        messages_per_pass: 16,
        max_message: MAX_MESSAGE,
        frames_per_message: 8,
        write_buffer: 128 << 10,
        queue: 8192,
        ..WebLimits::default()
    }
}

/// A room service on one address and port (module docs).
pub struct Service {
    web: WebTransport,
    rooms: Rooms,
    start: Instant,
    last_tick: Option<Instant>,
    buf: Vec<u8>,
    out: Vec<Out>,
}

impl Service {
    /// A service under `policy` (the address it may bind) and `setup` (its files, TLS, access and limits: start from
    /// [`service_limits`]).
    pub fn new(policy: BindPolicy, setup: WebSetup, rooms: Rooms) -> Service {
        let len = setup.limits.max_message.max(NET_MAX_DATAGRAM) + 1;
        Service {
            web: WebTransport::new(policy, setup),
            rooms,
            start: Instant::now(),
            last_tick: None,
            buf: vec![0; len],
            out: Vec::new(),
        }
    }

    pub fn bind(&mut self, ip: IpAddr, port: u16) -> Result<(), BindError> {
        self.web.bind(ip, port)
    }

    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.web.local_addr()
    }

    pub fn web(&self) -> &WebTransport {
        &self.web
    }

    pub fn rooms(&self) -> &Rooms {
        &self.rooms
    }

    fn now_ms(&self) -> Ms {
        self.start.elapsed().as_millis() as Ms
    }

    /// One pass: the transport's, the messages waiting handed to the rooms, closed sockets undone, the lifetimes looked
    /// at, and what the rooms said sent.
    pub fn pass(&mut self) {
        self.web.pump();
        let unix_now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        for _ in 0..MESSAGES_PER_PASS {
            let (len, from) = match self.web.recv(&mut self.buf) {
                Recv::Datagram { len, from } => (len, from),
                Recv::Skipped => continue,
                Recv::Empty => break,
            };
            // A message from a socket that has closed since is dropped.
            let Some(peer) = self.web.peer_of(from) else {
                continue;
            };
            let now = self.now_ms();
            let Some(bytes) = self.buf.get(..len) else {
                continue;
            };
            self.rooms
                .message(from, peer.ip(), bytes, now, unix_now, &mut self.out);
            self.deliver();
        }
        for gone in self.web.take_gone() {
            self.rooms.gone(gone, &mut self.out);
        }
        let now = Instant::now();
        if self.last_tick.is_none_or(|t| now.duration_since(t) >= TICK) {
            self.last_tick = Some(now);
            let ms = self.now_ms();
            self.rooms.tick(ms, &mut self.out);
        }
        self.deliver();
    }

    fn deliver(&mut self) {
        // Closing a host's socket closes its joiners' too: a few rounds at most.
        for _ in 0..4 {
            if self.out.is_empty() {
                return;
            }
            for out in std::mem::take(&mut self.out) {
                match out {
                    Out::Send(to, text) => {
                        self.web.send_to(to, text.as_bytes());
                    }
                    Out::Close(socket) => {
                        self.web.close_socket(socket);
                    }
                }
            }
            // The rooms forgot the sockets they closed already; a socket that closed on its own is undone here.
            for gone in self.web.take_gone() {
                self.rooms.gone(gone, &mut self.out);
            }
        }
    }

    /// Runs passes every `every` until `stop` says so.
    pub fn run(&mut self, every: Duration, mut stop: impl FnMut(&Service) -> bool) {
        while !stop(self) {
            self.pass();
            std::thread::sleep(every);
        }
    }
}
