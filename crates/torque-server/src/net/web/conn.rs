//! One TCP connection of the web transport: an HTTP exchange (a request head read under caps, a file or a refusal written
//! back) that may become a game socket (a WebSocket whose binary messages are the datagrams).

use std::collections::{HashMap, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::rc::Rc;

use tungstenite::protocol::frame::CloseFrame;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::{Role, WebSocket, WebSocketConfig};
use tungstenite::{Error as WsError, Message};
use web_time::{Duration, Instant};

use super::http::{self, Parsed, Status, WEB_SOCKET_PATH, WebFiles};
use super::tls::Pipe;
use super::{WebCounters, WebLimits};

/// The most bytes one connection writes in one pass (a large file goes out over several passes, so one download does not
/// hold a pass).
const WRITE_PER_PASS: usize = 4 << 20;
/// The most bytes one write call is given.
const WRITE_CHUNK: usize = 64 << 10;
/// The most requests one connection's pass answers.
const REQUESTS_PER_PASS: usize = 4;
/// The read buffer of a game socket (its messages are at most a datagram).
const SOCKET_READ_BUFFER: usize = 4096;

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
    /// More messages a second than the cap.
    Rate,
    /// A text message or a broken frame.
    Protocol,
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
        ws: Box<WebSocket<Pipe>>,
        rate: Rate,
    },
    Gone,
}

/// Messages counted over one second.
struct Rate {
    start: Instant,
    count: u32,
}

impl Rate {
    fn new(now: Instant) -> Rate {
        Rate {
            start: now,
            count: 0,
        }
    }

    fn allow(&mut self, now: Instant, limit: u32) -> bool {
        if now.duration_since(self.start) >= Duration::from_secs(1) {
            self.start = now;
            self.count = 0;
        }
        self.count += 1;
        self.count <= limit
    }
}

/// What a pass hands every connection.
pub(crate) struct Pass<'a> {
    pub now: Instant,
    pub limits: &'a WebLimits,
    pub files: Option<&'a WebFiles>,
    pub inbox: &'a mut VecDeque<(SocketAddr, Vec<u8>)>,
    pub counters: &'a mut WebCounters,
    /// The game sockets open from each address.
    pub sockets: &'a mut HashMap<IpAddr, usize>,
}

pub(crate) struct Conn {
    pub peer: SocketAddr,
    phase: Phase,
    /// When the present wait began: the head's (the connection's start, or the last response's end), or the socket's last
    /// frame.
    since: Instant,
    /// Since when writes have made no progress (None while they go through).
    stalled: Option<Instant>,
    requests: u32,
}

impl Conn {
    pub(crate) fn new(peer: SocketAddr, pipe: Pipe, now: Instant) -> Conn {
        Conn {
            peer,
            phase: Phase::Http {
                pipe,
                buf: Vec::new(),
                out: None,
            },
            since: now,
            stalled: None,
            requests: 0,
        }
    }

    /// Whether this is a game socket.
    pub(crate) fn is_socket(&self) -> bool {
        matches!(self.phase, Phase::Socket { .. })
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
        match std::mem::replace(&mut self.phase, Phase::Gone) {
            Phase::Http { mut pipe, .. } => pipe.shut(),
            Phase::Socket { mut ws, .. } => {
                if let Some(n) = pass.sockets.get_mut(&self.peer.ip()) {
                    *n = n.saturating_sub(1);
                }
                let code = match why {
                    Closed::Oversize => Some(CloseCode::Size),
                    Closed::Rate | Closed::Stalled => Some(CloseCode::Policy),
                    Closed::Protocol => Some(CloseCode::Unsupported),
                    Closed::Timeout => Some(CloseCode::Away),
                    Closed::Peer | Closed::Done => None,
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

    pub(crate) fn is_gone(&self) -> bool {
        matches!(self.phase, Phase::Gone)
    }

    fn advance_http(&mut self, pass: &mut Pass<'_>) -> Option<Closed> {
        for _ in 0..REQUESTS_PER_PASS {
            let Phase::Http { pipe, buf, out } = &mut self.phase else {
                return None;
            };
            if let Some(o) = out {
                let (done, progressed) = match write_out(pipe, o) {
                    Ok(r) => r,
                    Err(_) => return Some(Closed::Peer),
                };
                if !done {
                    return stall_check(&mut self.stalled, progressed, pass.now, pass.limits)
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
            // Read what has come, up to the head's cap.
            let mut chunk = [0u8; 4096];
            while buf.len() < http::MAX_HEAD_BYTES {
                let room = (http::MAX_HEAD_BYTES - buf.len()).min(chunk.len());
                match pipe.read(&mut chunk[..room]) {
                    Ok(0) => return Some(Closed::Peer),
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    Err(_) => return Some(Closed::Peer),
                }
            }
            let parsed = http::parse_head(buf);
            let request = match parsed {
                Parsed::Partial => {
                    let waited = pass.now.duration_since(self.since);
                    return (waited > pass.limits.head_timeout).then_some(Closed::Timeout);
                }
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
            let keep = request.keep_alive
                && self.requests < pass.limits.requests_per_connection
                && buf.is_empty();
            *out = Some(if request.wants_upgrade {
                match request.upgrade {
                    Some(key) => {
                        let open = pass.sockets.get(&self.peer.ip()).copied().unwrap_or(0);
                        if open >= pass.limits.sockets_per_ip {
                            pass.counters.refusals += 1;
                            refuse(Status::TooManyRequests)
                        } else {
                            Outgoing {
                                head: http::upgrade_head(&key),
                                body: None,
                                sent: 0,
                                after: After::Upgrade,
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
        let Phase::Http { pipe, buf, .. } = std::mem::replace(&mut self.phase, Phase::Gone) else {
            return Some(Closed::Peer);
        };
        let config = WebSocketConfig::default()
            .read_buffer_size(SOCKET_READ_BUFFER)
            .write_buffer_size(0)
            .max_write_buffer_size(pass.limits.write_buffer)
            .max_message_size(Some(pass.limits.max_message))
            .max_frame_size(Some(pass.limits.max_message));
        let ws = WebSocket::from_partially_read(pipe, buf, Role::Server, Some(config));
        self.phase = Phase::Socket {
            ws: Box::new(ws),
            rate: Rate::new(pass.now),
        };
        *pass.sockets.entry(self.peer.ip()).or_insert(0) += 1;
        pass.counters.upgrades += 1;
        self.since = pass.now;
        self.stalled = None;
        self.advance_socket(pass)
    }

    fn advance_socket(&mut self, pass: &mut Pass<'_>) -> Option<Closed> {
        let peer = self.peer;
        let Phase::Socket { ws, rate } = &mut self.phase else {
            return None;
        };
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
            self.since = pass.now;
            if !rate.allow(pass.now, pass.limits.messages_per_s) {
                return Some(Closed::Rate);
            }
            match message {
                Message::Binary(data) => {
                    if pass.inbox.len() < pass.limits.queue {
                        pass.counters.messages += 1;
                        pass.inbox.push_back((peer, data.to_vec()));
                    } else {
                        pass.counters.queue_full += 1;
                    }
                }
                Message::Text(_) => return Some(Closed::Protocol),
                // Pings are answered by the socket itself; a close is answered on the next read, which then ends it.
                Message::Ping(_) | Message::Pong(_) | Message::Close(_) | Message::Frame(_) => {}
            }
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
