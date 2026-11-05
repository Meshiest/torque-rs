//! One TCP connection of the web transport: an HTTP exchange (a request head read under caps, a file or a refusal written
//! back) that may become a game socket (a WebSocket whose binary messages are the datagrams).

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::SocketAddr;
use std::rc::Rc;

use tungstenite::protocol::frame::CloseFrame;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::{Role, WebSocket, WebSocketConfig};
use tungstenite::{Error as WsError, Message};
use web_time::{Duration, Instant};

use super::http::{self, Parsed, Status, WEB_SOCKET_PATH, WebFiles};
use super::meter::{Caps, Metered, Over};
use super::stand_in::StandIns;
use super::tls::{Pipe, WireCaps};
use super::{Access, WebCounters, WebLimits};

/// The most bytes one connection writes in one pass (a large file goes out over several passes, so one download does not
/// hold a pass).
const WRITE_PER_PASS: usize = 4 << 20;
/// The most bytes one write call is given.
const WRITE_CHUNK: usize = 64 << 10;
/// The most requests one connection's pass answers.
const REQUESTS_PER_PASS: usize = 4;
/// The read buffer of a game socket (its messages are at most a datagram).
const SOCKET_READ_BUFFER: usize = 4096;
/// The wire bytes a game socket may send beyond its frames' bytes, per frame (a TLS record's header, tag and content type,
/// with room to spare).
const RECORD_ROOM: usize = 64;

/// What an HTTP connection may read off the wire.
pub(crate) fn http_wire_caps(limits: &WebLimits) -> WireCaps {
    WireCaps {
        bytes_per_pass: limits.http_bytes_per_pass,
        bytes_per_s: limits.http_bytes_per_s,
    }
}

/// What a game socket may read off the wire: its frames' caps, and a record's room for each frame.
fn socket_wire_caps(limits: &WebLimits) -> WireCaps {
    WireCaps {
        bytes_per_pass: limits.bytes_per_pass + limits.messages_per_pass * RECORD_ROOM,
        bytes_per_s: limits.bytes_per_s + limits.messages_per_s as usize * RECORD_ROOM,
    }
}

/// Why a connection was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Closed {
    /// The other end closed it, or its stream failed.
    Peer,
    /// The response was the last one (`Connection: close`, a refusal, the request cap).
    Done,
    /// A head that did not come in time, or a socket gone quiet.
    Timeout,
    /// Writes that made no progress for too long, or a socket's unsent bytes past the cap.
    Stalled,
    /// A message larger than a datagram.
    Oversize,
    /// More bytes or frames a second than the caps.
    Rate,
    /// A text message, a broken frame, or more frames in one message than the cap.
    Protocol,
    /// An idle HTTP connection closed to make room for a new one when every place was taken.
    Evicted,
}

impl Closed {
    fn count(self, counters: &mut WebCounters) {
        let slot = match self {
            Closed::Peer | Closed::Done => &mut counters.closed,
            Closed::Timeout => &mut counters.closed_timeout,
            Closed::Stalled => &mut counters.closed_stalled,
            Closed::Oversize => &mut counters.closed_oversize,
            Closed::Rate => &mut counters.closed_rate,
            Closed::Protocol => &mut counters.closed_protocol,
            Closed::Evicted => &mut counters.evicted,
        };
        *slot += 1;
    }
}

/// A response being written.
struct Outgoing {
    head: Vec<u8>,
    body: Option<Rc<[u8]>>,
    sent: usize,
    after: After,
    /// When the response must be all written (set when its writing starts): the stall timeout plus its bytes at the
    /// slowest rate allowed ([`WebLimits::min_send_rate`]), so a reader that takes a byte now and then is not kept forever.
    deadline: Option<Instant>,
}

/// What follows a response.
#[derive(Clone, Copy, PartialEq, Eq)]
enum After {
    KeepAlive,
    Close,
    Upgrade,
}

enum Phase {
    Http {
        pipe: Pipe,
        buf: Vec<u8>,
        out: Option<Outgoing>,
    },
    Socket {
        ws: Box<WebSocket<Metered>>,
    },
    Gone,
}

/// What a pass hands every connection.
pub(crate) struct Pass<'a> {
    pub now: Instant,
    pub limits: &'a WebLimits,
    pub files: Option<&'a WebFiles>,
    pub inbox: &'a mut VecDeque<(SocketAddr, Vec<u8>)>,
    pub counters: &'a mut WebCounters,
    /// The stand-in addresses of the game sockets (and the sockets each peer address holds).
    pub stand_ins: &'a mut StandIns,
    /// Who may reach the transport (the Host and Origin checks).
    pub access: &'a Access,
    /// Whether the connections are TLS (the page's origin is `https`).
    pub tls: bool,
    /// The TLS handshakes this pass may still step ([`WebLimits::handshakes_per_pass`]).
    pub handshakes: usize,
    /// The stand-ins of the game sockets closed since the host last asked ([`super::WebTransport`]'s `take_gone`).
    pub gone: &'a mut Vec<SocketAddr>,
    /// The data channels, when the transport takes them.
    #[cfg(feature = "webrtc")]
    pub rtc: Option<&'a mut super::rtc::RtcSide>,
}

/// What a game socket's signaling asked for.
#[cfg_attr(not(feature = "webrtc"), allow(dead_code))]
enum Signaled {
    Nothing,
    Reply(String),
    Refused,
}

impl Pass<'_> {
    /// The largest message a game socket may send: a datagram, or a signaling message when data channels are on.
    fn max_socket_message(&self) -> usize {
        #[cfg(feature = "webrtc")]
        if let Some(rtc) = self.rtc.as_deref() {
            return self.limits.max_message.max(rtc.limits().max_signal);
        }
        self.limits.max_message
    }

    /// The most text (signaling) messages a game socket may send (none without data channels).
    fn signals_per_socket(&self) -> u32 {
        #[cfg(feature = "webrtc")]
        if let Some(rtc) = self.rtc.as_deref() {
            return rtc.limits().signals_per_socket;
        }
        0
    }

    /// A signaling message from the game socket of `stand_in` (TCP address `peer`).
    #[cfg_attr(not(feature = "webrtc"), allow(unused_variables))]
    fn signal(&mut self, stand_in: SocketAddr, peer: SocketAddr, text: &str) -> Signaled {
        #[cfg(feature = "webrtc")]
        if let Some(rtc) = self.rtc.as_deref_mut() {
            return match rtc.signal(stand_in, peer.ip(), text, self.now) {
                Ok(super::rtc::Reply::None) => Signaled::Nothing,
                Ok(super::rtc::Reply::Text(t)) => Signaled::Reply(t),
                Err(()) => Signaled::Refused,
            };
        }
        Signaled::Refused
    }

    /// When the data channel of `stand_in` last carried a game message.
    #[cfg_attr(not(feature = "webrtc"), allow(unused_variables))]
    fn rtc_heard(&self, stand_in: SocketAddr) -> Option<Instant> {
        #[cfg(feature = "webrtc")]
        return self.rtc.as_deref().and_then(|r| r.last_heard(stand_in));
        #[cfg(not(feature = "webrtc"))]
        None
    }

    /// The game socket of `stand_in` closed: its data channel goes with it.
    #[cfg_attr(not(feature = "webrtc"), allow(unused_variables))]
    fn rtc_remove(&mut self, stand_in: SocketAddr) {
        #[cfg(feature = "webrtc")]
        if let Some(rtc) = self.rtc.as_deref_mut() {
            rtc.remove(stand_in);
        }
    }
}

/// The most closed sockets kept for the host to drop their connections (the rest time out as before).
const GONE_MAX: usize = 4096;

pub(crate) struct Conn {
    /// The TCP peer's address.
    pub peer: SocketAddr,
    /// The socket's stand-in address ([`super::stand_in`]), taken when the upgrade's answer is queued (it reserves the
    /// socket's place under the per-address cap) and given back when the connection ends.
    pub stand_in: Option<SocketAddr>,
    phase: Phase,
    /// When the connection was taken.
    opened: Instant,
    /// When the present wait began: the head's (the connection's start, or the last response's end), or the socket's last
    /// game message (pings and other control frames do not keep a socket open).
    since: Instant,
    /// Since when writes have made no progress (None while they go through).
    stalled: Option<Instant>,
    requests: u32,
    /// The signaling (text) messages the game socket sent.
    signals: u32,
}

impl Conn {
    pub(crate) fn new(peer: SocketAddr, pipe: Pipe, now: Instant) -> Conn {
        Conn {
            peer,
            stand_in: None,
            phase: Phase::Http {
                pipe,
                buf: Vec::new(),
                out: None,
            },
            opened: now,
            since: now,
            stalled: None,
            requests: 0,
            signals: 0,
        }
    }

    /// Whether this is a game socket.
    pub(crate) fn is_socket(&self) -> bool {
        matches!(self.phase, Phase::Socket { .. })
    }

    /// Since when an HTTP connection has waited for a request with nothing of one read and nothing to write (None for any
    /// other: one mid-request, mid-response, still in its TLS handshake, or a socket).
    pub(crate) fn idle_since(&self) -> Option<Instant> {
        match &self.phase {
            Phase::Http { pipe, buf, out }
                if out.is_none() && buf.is_empty() && !pipe.is_handshaking() =>
            {
                Some(self.since)
            }
            _ => None,
        }
    }

    /// One pass: what can be read and written without blocking. False when the connection is to be dropped (it has been
    /// counted and shut).
    pub(crate) fn advance(&mut self, pass: &mut Pass<'_>) -> bool {
        let closed = match self.phase {
            Phase::Http { .. } => self.advance_http(pass),
            Phase::Socket { .. } => self.advance_socket(pass),
            Phase::Gone => return false,
        };
        if let Some(why) = closed {
            self.shut(why, pass);
            return false;
        }
        true
    }

    /// Ends the connection (a socket says why in its close frame).
    pub(crate) fn shut(&mut self, why: Closed, pass: &mut Pass<'_>) {
        why.count(pass.counters);
        let closed_stand_in = self.stand_in;
        if let Some(addr) = self.stand_in.take() {
            pass.stand_ins.give_back(self.peer.ip(), addr);
        }
        match std::mem::replace(&mut self.phase, Phase::Gone) {
            Phase::Http { mut pipe, .. } => pipe.shut(),
            Phase::Socket { mut ws, .. } => {
                if let Some(addr) = closed_stand_in {
                    pass.rtc_remove(addr);
                    if pass.gone.len() < GONE_MAX {
                        pass.gone.push(addr);
                    }
                }
                let code = match why {
                    Closed::Oversize => Some(CloseCode::Size),
                    Closed::Rate | Closed::Stalled => Some(CloseCode::Policy),
                    Closed::Protocol => Some(CloseCode::Unsupported),
                    Closed::Timeout => Some(CloseCode::Away),
                    Closed::Peer | Closed::Done | Closed::Evicted => None,
                };
                if let Some(code) = code {
                    let _ = ws.close(Some(CloseFrame {
                        code,
                        reason: "".into(),
                    }));
                    let _ = ws.flush();
                }
                ws.get_mut().shut();
            }
            Phase::Gone => {}
        }
    }

    /// Sends one datagram on a game socket: false when it was not (not a socket, or its unsent bytes are past the cap, which
    /// closes it).
    pub(crate) fn send(&mut self, data: &[u8], pass: &mut Pass<'_>) -> bool {
        let Phase::Socket { ws, .. } = &mut self.phase else {
            return false;
        };
        match ws.send(Message::binary(data.to_vec())) {
            Ok(()) => true,
            // Queued: the next flush sends it.
            Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                self.stalled.get_or_insert(pass.now);
                true
            }
            Err(WsError::WriteBufferFull(_)) => {
                self.shut(Closed::Stalled, pass);
                false
            }
            Err(_) => {
                self.shut(Closed::Peer, pass);
                false
            }
        }
    }

    /// Sends a text (signaling) message on a game socket: false when it was not (as [`Conn::send`]).
    #[cfg_attr(not(feature = "webrtc"), allow(dead_code))]
    pub(crate) fn send_text(&mut self, text: &str, pass: &mut Pass<'_>) -> bool {
        let Phase::Socket { ws, .. } = &mut self.phase else {
            return false;
        };
        match ws.send(Message::text(text)) {
            Ok(()) => true,
            Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                self.stalled.get_or_insert(pass.now);
                true
            }
            Err(WsError::WriteBufferFull(_)) => {
                self.shut(Closed::Stalled, pass);
                false
            }
            Err(_) => {
                self.shut(Closed::Peer, pass);
                false
            }
        }
    }

    pub(crate) fn is_gone(&self) -> bool {
        matches!(self.phase, Phase::Gone)
    }

    /// Closes an idle HTTP connection to make room ([`Conn::idle_since`]; it holds no stand-in).
    pub(crate) fn evict(&mut self, counters: &mut WebCounters) {
        Closed::Evicted.count(counters);
        if let Phase::Http { mut pipe, .. } = std::mem::replace(&mut self.phase, Phase::Gone) {
            pipe.shut();
        }
    }

    fn advance_http(&mut self, pass: &mut Pass<'_>) -> Option<Closed> {
        if let Phase::Http { pipe, .. } = &mut self.phase {
            pipe.refill(pass.now);
        }
        for _ in 0..REQUESTS_PER_PASS {
            let Phase::Http { pipe, buf, out } = &mut self.phase else {
                return None;
            };
            // A TLS handshake: bounded in time, and in how many step in one pass (each is work on the server's thread).
            if pipe.is_handshaking() {
                if pass.now.duration_since(self.opened) > pass.limits.handshake_timeout {
                    return Some(Closed::Timeout);
                }
                if pass.handshakes == 0 {
                    return None;
                }
                pass.handshakes -= 1;
            }
            if let Some(o) = out {
                let deadline = *o.deadline.get_or_insert_with(|| {
                    let bytes = o.head.len() + o.body.as_ref().map_or(0, |b| b.len());
                    let rate = pass.limits.min_send_rate.max(1);
                    pass.now
                        + pass.limits.stall_timeout
                        + Duration::from_millis((bytes as u64).saturating_mul(1000) / rate as u64)
                });
                let (done, progressed) = match write_out(pipe, o) {
                    Ok(r) => r,
                    Err(_) => return Some(Closed::Peer),
                };
                if !done {
                    let late = pass.now > deadline;
                    return (stall_check(&mut self.stalled, progressed, pass.now, pass.limits)
                        || late)
                        .then_some(Closed::Stalled);
                }
                self.stalled = None;
                let after = o.after;
                *out = None;
                match after {
                    After::Close => return Some(Closed::Done),
                    After::Upgrade => return self.upgrade(pass),
                    After::KeepAlive => {
                        self.since = pass.now;
                        if self.requests >= pass.limits.requests_per_connection {
                            return Some(Closed::Done);
                        }
                    }
                }
                continue;
            }
            // Read what has come, up to the head's cap, while the head has time left (each read is one bounded step, held to
            // the wire's budget: the deadline is checked before any is taken).
            let late = pass.now.duration_since(self.since) > pass.limits.head_timeout;
            let mut chunk = [0u8; 4096];
            while buf.len() < http::MAX_HEAD_BYTES && !late {
                let room = (http::MAX_HEAD_BYTES - buf.len()).min(chunk.len());
                match pipe.read(&mut chunk[..room]) {
                    Ok(0) => return Some(Closed::Peer),
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    Err(_) => return Some(Closed::Peer),
                }
            }
            if pipe.over() {
                return Some(Closed::Rate);
            }
            let parsed = http::parse_head(buf);
            let request = match parsed {
                Parsed::Partial => return late.then_some(Closed::Timeout),
                Parsed::Refuse(status) => {
                    pass.counters.refusals += 1;
                    *out = Some(refuse(status));
                    continue;
                }
                Parsed::Request(r) => r,
            };
            buf.drain(..request.head_len);
            self.requests += 1;
            pass.counters.requests += 1;
            if let Err(status) = http::check_access(&request, pass.access, pass.tls) {
                pass.counters.refusals += 1;
                *out = Some(refuse(status));
                continue;
            }
            let keep = request.keep_alive
                && self.requests < pass.limits.requests_per_connection
                && buf.is_empty();
            *out = Some(if request.wants_upgrade {
                match request.upgrade {
                    // Bytes after an upgrade's head would reach the socket unmetered (a browser sends no frame before
                    // the 101): refused.
                    Some(_) if !buf.is_empty() => {
                        pass.counters.refusals += 1;
                        refuse(Status::BadRequest)
                    }
                    Some(key) => {
                        let open = pass.stand_ins.count(self.peer.ip());
                        if open >= pass.limits.sockets_per_ip || self.stand_in.is_some() {
                            pass.counters.refusals += 1;
                            refuse(Status::TooManyRequests)
                        } else {
                            // The place is reserved now: a second upgrade from the address while this answer is still
                            // being written counts it.
                            self.stand_in = Some(pass.stand_ins.take(self.peer.ip()));
                            Outgoing {
                                head: http::upgrade_head(&key),
                                body: None,
                                sent: 0,
                                after: After::Upgrade,
                                deadline: None,
                            }
                        }
                    }
                    None => {
                        pass.counters.refusals += 1;
                        refuse(Status::BadRequest)
                    }
                }
            } else {
                let file = (request.path != WEB_SOCKET_PATH)
                    .then(|| pass.files.and_then(|f| f.get(&request.path)))
                    .flatten();
                match file {
                    Some(file) => {
                        pass.counters.files += 1;
                        Outgoing {
                            head: http::response_head(
                                Status::Ok,
                                file.content_type,
                                file.bytes.len(),
                                keep,
                            ),
                            body: Some(Rc::clone(&file.bytes)),
                            sent: 0,
                            after: if keep { After::KeepAlive } else { After::Close },
                            deadline: None,
                        }
                    }
                    None => {
                        pass.counters.refusals += 1;
                        refuse(Status::NotFound)
                    }
                }
            });
        }
        None
    }

    /// The 101 is written: the connection is a game socket from now on.
    fn upgrade(&mut self, pass: &mut Pass<'_>) -> Option<Closed> {
        let Phase::Http { mut pipe, buf, .. } = std::mem::replace(&mut self.phase, Phase::Gone)
        else {
            return Some(Closed::Peer);
        };
        pipe.set_caps(socket_wire_caps(pass.limits));
        // With data channels a text (signaling) message may be larger than a datagram; a binary one never is.
        let max_message = pass.max_socket_message();
        let config = WebSocketConfig::default()
            .read_buffer_size(SOCKET_READ_BUFFER)
            .write_buffer_size(0)
            .max_write_buffer_size(pass.limits.write_buffer)
            .max_message_size(Some(max_message))
            .max_frame_size(Some(max_message));
        let limits = pass.limits;
        let metered = Metered::new(
            pipe,
            Caps {
                bytes_per_pass: limits.bytes_per_pass,
                bytes_per_s: limits.bytes_per_s,
                frames_per_s: limits.messages_per_s,
                frames_per_message: limits.frames_per_message,
            },
            pass.now,
        );
        // Nothing is read while the 101 is written, and an upgrade with bytes after its head was refused: every frame goes
        // through the meter.
        if !buf.is_empty() {
            return Some(Closed::Protocol);
        }
        let ws = WebSocket::from_raw_socket(metered, Role::Server, Some(config));
        self.phase = Phase::Socket { ws: Box::new(ws) };
        pass.counters.upgrades += 1;
        self.since = pass.now;
        self.stalled = None;
        // A transport that takes data channels says so first.
        #[cfg(feature = "webrtc")]
        if pass.rtc.is_some() && !self.send_text(super::rtc::signal::HELLO, pass) {
            return None;
        }
        self.advance_socket(pass)
    }

    fn advance_socket(&mut self, pass: &mut Pass<'_>) -> Option<Closed> {
        let Some(peer) = self.stand_in else {
            return Some(Closed::Peer);
        };
        let Phase::Socket { ws } = &mut self.phase else {
            return None;
        };
        ws.get_mut().refill(pass.now);
        // Game messages on the socket's data channel keep it open as its own do.
        if let Some(heard) = pass.rtc_heard(peer)
            && heard > self.since
        {
            self.since = heard;
        }
        // A socket gone quiet is closed before anything more is read from it.
        if pass.now.duration_since(self.since) > pass.limits.idle_timeout {
            return Some(Closed::Timeout);
        }
        let flushed = match ws.flush() {
            Ok(()) => true,
            Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => false,
            Err(_) => return Some(Closed::Peer),
        };
        for _ in 0..pass.limits.messages_per_pass {
            let message = match ws.read() {
                Ok(m) => m,
                Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => break,
                Err(WsError::Io(e)) if e.kind() == ErrorKind::Interrupted => continue,
                Err(WsError::Capacity(_)) => return Some(Closed::Oversize),
                Err(WsError::WriteBufferFull(_)) => return Some(Closed::Stalled),
                Err(WsError::Protocol(_) | WsError::Utf8(_) | WsError::AttackAttempt) => {
                    return Some(Closed::Protocol);
                }
                Err(_) => return Some(Closed::Peer),
            };
            match message {
                Message::Binary(data) if data.len() > pass.limits.max_message => {
                    return Some(Closed::Oversize);
                }
                Message::Binary(data) => {
                    self.since = pass.now;
                    if pass.inbox.len() < pass.limits.queue {
                        pass.counters.messages += 1;
                        pass.inbox.push_back((peer, data.to_vec()));
                    } else {
                        pass.counters.queue_full += 1;
                    }
                }
                // Signaling for a data channel, when the transport takes them; else a browser never sends text.
                Message::Text(text) => {
                    self.signals += 1;
                    if self.signals > pass.signals_per_socket() {
                        return Some(Closed::Protocol);
                    }
                    match pass.signal(peer, self.peer, text.as_str()) {
                        Signaled::Nothing => {}
                        Signaled::Refused => return Some(Closed::Protocol),
                        Signaled::Reply(reply) => match ws.send(Message::text(reply)) {
                            Ok(()) => {}
                            Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                                self.stalled.get_or_insert(pass.now);
                            }
                            Err(WsError::WriteBufferFull(_)) => return Some(Closed::Stalled),
                            Err(_) => return Some(Closed::Peer),
                        },
                    }
                }
                // Pings are answered by the socket itself; a close is answered on the next read, which then ends it.
                Message::Ping(_) | Message::Pong(_) | Message::Close(_) | Message::Frame(_) => {}
            }
        }
        match ws.get_ref().over() {
            Some(Over::Rate) => return Some(Closed::Rate),
            Some(Over::Frames) => return Some(Closed::Protocol),
            None => {}
        }
        if stall_check(&mut self.stalled, flushed, pass.now, pass.limits) {
            return Some(Closed::Stalled);
        }
        let quiet = pass.now.duration_since(self.since);
        (quiet > pass.limits.idle_timeout).then_some(Closed::Timeout)
    }
}

/// Whether writes that made no progress since `stalled` have stalled for longer than the cap (progress clears the mark).
fn stall_check(
    stalled: &mut Option<Instant>,
    progressed: bool,
    now: Instant,
    limits: &WebLimits,
) -> bool {
    if progressed {
        *stalled = None;
        return false;
    }
    let since = *stalled.get_or_insert(now);
    now.duration_since(since) > limits.stall_timeout
}

fn refuse(status: Status) -> Outgoing {
    Outgoing {
        head: http::refusal(status),
        body: None,
        sent: 0,
        after: After::Close,
        deadline: None,
    }
}

/// Writes what the stream takes now. Returns whether the response is all written (and, under TLS, handed to the stream),
/// and whether any byte went.
fn write_out(pipe: &mut Pipe, out: &mut Outgoing) -> std::io::Result<(bool, bool)> {
    let body_len = out.body.as_ref().map_or(0, |b| b.len());
    let total = out.head.len() + body_len;
    let mut progressed = false;
    let mut budget = WRITE_PER_PASS;
    while out.sent < total && budget > 0 {
        let rest: &[u8] = if out.sent < out.head.len() {
            &out.head[out.sent..]
        } else {
            let body = out.body.as_deref().unwrap_or_default();
            &body[out.sent - out.head.len()..]
        };
        let n = rest.len().min(WRITE_CHUNK).min(budget);
        match pipe.write(&rest[..n]) {
            Ok(0) => break,
            Ok(n) => {
                out.sent += n;
                budget -= n;
                progressed = true;
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let flushed = match pipe.flush() {
        Ok(()) => true,
        Err(e) if e.kind() == ErrorKind::WouldBlock => false,
        Err(e) => return Err(e),
    };
    Ok((out.sent >= total && flushed, progressed))
}
