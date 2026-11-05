//! A player's host (the `p2p` feature): a game server that players join straight over WebRTC data channels, found by a
//! room code through a room service (`torque-rooms`; the protocol is [`crate::net::room`]), with no port to forward.
//!
//! [`P2pHost`] is a [`Transport`] with two parts: a client socket to the room service ([`RoomLink`], opened when the
//! transport binds: it asks for a room and waits for its code), and the data channels of [`super::rtc`] in its host mode
//! (full ICE, so the host's own checks open its NAT; the joiner's trickled candidates taken; a server-reflexive candidate
//! learned from the room's STUN server). A joiner the service announces (`peer <n> <ip>`) is given a stand-in address of
//! that IP ([`super::stand_in`]), so the host's per-address limits and the scripts' bans hold for it; its offer and
//! candidates arrive through the room, the answer goes back the same way, and once its channel is open its datagrams go on
//! the channel both ways. Nothing of the game passes through the service, and nothing falls back to it: a joiner has no
//! other link.
//!
//! **Hostile input.** The room service is the host's own choice, but what it relays comes from joiners: every signaling
//! message goes through the data channels' checks (offers that are not data-only, too many offers, too many peers in all,
//! per address and per second are refused; at most [`super::rtc::MAX_REMOTE_CANDIDATES`] candidates a joiner), and every
//! datagram through their per-pass and per-second caps in wire bytes. A pass reads at most [`ROOM_MESSAGES_PER_PASS`]
//! room messages. Joiners announced but not yet connected are capped ([`P2pHost::joiners`]); past it the service is told
//! to drop the new one.
//!
//! **Leaving.** A joiner whose channel closes is reported gone ([`Transport::take_gone`]): the host drops its connection
//! at once. When the host closes ([`Transport::close`], or the transport is dropped), each channel's DTLS session is
//! closed, and every joiner sees its channel close: the game ends for them with a message. There is no host migration.

use std::collections::{HashMap, VecDeque};
use std::io::{self, ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};

use tungstenite::protocol::{Role, WebSocket, WebSocketConfig};
use tungstenite::{Error as WsError, Message};
use web_time::{Duration, Instant};

use super::rtc::{Reply, RtcSetup, RtcSide};
use super::stand_in::StandIns;
use crate::net::room::{FromService, MAX_MESSAGE, ToService, show_code};
use crate::net::transport::{BindError, BindPolicy, Recv, Transport};

/// The most room messages one pass reads.
pub const ROOM_MESSAGES_PER_PASS: usize = 16;
/// How often the host pings the room service (it closes a socket quiet for 30 s).
pub const PING_EVERY: Duration = Duration::from_secs(10);
/// How long connecting to the room service and getting a room may take.
pub const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// A client socket to a room service: plain `ws://` (TLS is not made here yet), binary messages of room text, at most
/// [`MAX_MESSAGE`] bytes each way.
pub struct RoomLink {
    ws: WebSocket<TcpStream>,
    closed: bool,
    last_sent: Instant,
}

/// The parts of a `ws://host[:port][/path]` URL (the path defaults to the web transport's socket path).
pub fn parse_ws_url(url: &str) -> Result<(String, u16, String), String> {
    if url.starts_with("wss://") {
        return Err("a wss:// room service needs TLS, which a desktop host does not make yet: use ws:// on a private network".into());
    }
    let rest = url
        .strip_prefix("ws://")
        .ok_or_else(|| format!("a room service URL starts with ws://, not {url:?}"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, super::WEB_SOCKET_PATH),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h,
            p.parse::<u16>()
                .map_err(|_| format!("the port number in {url:?}"))?,
        ),
        None => (authority, 80),
    };
    let ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-');
    if host.is_empty() || !host.chars().all(ok) || !path.chars().all(|c| c.is_ascii_graphic()) {
        return Err(format!("{url:?} is not a ws:// URL this host takes"));
    }
    Ok((host.to_string(), port, path.to_string()))
}

impl RoomLink {
    /// Connects to the room service at `url` (blocking up to [`OPEN_TIMEOUT`]).
    pub fn connect(url: &str) -> io::Result<RoomLink> {
        let invalid = |e: String| io::Error::new(ErrorKind::InvalidInput, e);
        let (host, port, path) = parse_ws_url(url).map_err(invalid)?;
        let addr = (host.as_str(), port)
            .to_socket_addrs()?
            .find(SocketAddr::is_ipv4)
            .ok_or_else(|| invalid(format!("no IPv4 address for {host}")))?;
        let mut tcp = TcpStream::connect_timeout(&addr, OPEN_TIMEOUT)?;
        tcp.set_read_timeout(Some(OPEN_TIMEOUT))?;
        tcp.set_nodelay(true)?;
        let key = super::http::base64(&nonce());
        write!(
            tcp,
            "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n\r\n"
        )?;
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() > 4096 || tcp.read(&mut byte)? != 1 {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "the room service's answer",
                ));
            }
            head.extend_from_slice(&byte);
        }
        let head = String::from_utf8_lossy(&head);
        let accept = super::accept_key(&key);
        let accepted = head.lines().any(|l| {
            l.split_once(':').is_some_and(|(k, v)| {
                k.trim().eq_ignore_ascii_case("sec-websocket-accept") && v.trim() == accept
            })
        });
        if !head.starts_with("HTTP/1.1 101 ") || !accepted {
            let line = head.lines().next().unwrap_or("").to_string();
            return Err(io::Error::new(
                ErrorKind::ConnectionRefused,
                format!("the room service refused: {line}"),
            ));
        }
        tcp.set_read_timeout(None)?;
        tcp.set_nonblocking(true)?;
        let config = WebSocketConfig::default()
            .read_buffer_size(4096)
            .write_buffer_size(0)
            .max_write_buffer_size(256 << 10)
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE));
        Ok(RoomLink {
            ws: WebSocket::from_raw_socket(tcp, Role::Client, Some(config)),
            closed: false,
            last_sent: Instant::now(),
        })
    }

    pub fn is_open(&self) -> bool {
        !self.closed
    }

    /// Sends one room message: false when the link is closed (or closes now).
    pub fn send(&mut self, text: &str) -> bool {
        if self.closed {
            return false;
        }
        self.last_sent = Instant::now();
        match self.ws.send(Message::binary(text.as_bytes().to_vec())) {
            Ok(()) => true,
            Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => true,
            Err(_) => {
                self.closed = true;
                false
            }
        }
    }

    /// The next room message (text), if one has come; anything that is not one closes the link.
    pub fn take(&mut self) -> Option<String> {
        while !self.closed {
            match self.ws.read() {
                Ok(Message::Binary(data)) => match String::from_utf8(data.to_vec()) {
                    Ok(text) => return Some(text),
                    Err(_) => self.closed = true,
                },
                Ok(Message::Close(_)) => self.closed = true,
                Ok(_) => {}
                Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => return None,
                Err(_) => self.closed = true,
            }
        }
        None
    }

    /// Pings the service when nothing was sent for [`PING_EVERY`] (it closes quiet sockets).
    pub fn keep_alive(&mut self) {
        if self.last_sent.elapsed() >= PING_EVERY {
            self.send(&ToService::Ping.write());
        }
    }

    /// The address the link leaves from (the interface that reaches the service).
    pub fn local_ip(&self) -> Option<IpAddr> {
        self.ws.get_ref().local_addr().ok().map(|a| a.ip())
    }
}

/// 16 bytes for the handshake's key (the service checks only its form).
fn nonce() -> [u8; 16] {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut bytes = [7u8; 16];
    let _ = SystemRandom::new().fill(&mut bytes);
    bytes
}

/// The first `stun:` URL's host and port in an ICE server list (the room's JSON), if it names one.
pub fn first_stun(ice: &str) -> Option<(String, u16)> {
    let start = ice.find("\"stun:")? + 6;
    let rest = ice.get(start..)?;
    let url = rest.get(..rest.find('"')?)?;
    let url = url.split('?').next()?;
    match url.rsplit_once(':') {
        Some((host, port)) => Some((host.to_string(), port.parse().ok()?)),
        None => Some((url.to_string(), 3478)),
    }
}

/// How a player's host is set up: the room service's URL, and its data channels (the UDP port is the one the transport
/// binds; `host` is set).
#[derive(Debug, Clone, Default)]
pub struct P2pSetup {
    pub service: String,
    pub rtc: RtcSetup,
    /// The most datagrams waiting for the host.
    pub queue: usize,
}

/// A joiner the service announced.
#[derive(Debug, Clone, Copy)]
struct Joiner {
    stand_in: SocketAddr,
    ip: IpAddr,
}

/// A player's host's transport (module docs).
pub struct P2pHost {
    policy: BindPolicy,
    setup: P2pSetup,
    side: Option<RtcSide>,
    link: Option<RoomLink>,
    code: Option<String>,
    ice: String,
    inbox: VecDeque<(SocketAddr, Vec<u8>)>,
    stand_ins: StandIns,
    joiners: HashMap<u32, Joiner>,
    by_stand_in: HashMap<SocketAddr, u32>,
    gone: Vec<SocketAddr>,
    status: Vec<String>,
    last_pump: Option<Instant>,
    reflexive_shown: bool,
}

impl P2pHost {
    pub fn new(policy: BindPolicy, setup: P2pSetup) -> P2pHost {
        P2pHost {
            policy,
            setup,
            side: None,
            link: None,
            code: None,
            ice: String::new(),
            inbox: VecDeque::new(),
            stand_ins: StandIns::default(),
            joiners: HashMap::new(),
            by_stand_in: HashMap::new(),
            gone: Vec::new(),
            status: Vec::new(),
            last_pump: None,
            reflexive_shown: false,
        }
    }

    /// The room's code (`ABCD-EFGH`), once the service gave one.
    pub fn code(&self) -> Option<String> {
        self.code.as_deref().map(show_code)
    }

    /// The ICE servers the room gave (JSON).
    pub fn ice(&self) -> &str {
        &self.ice
    }

    /// The joiners announced and not gone.
    pub fn joiners(&self) -> usize {
        self.joiners.len()
    }

    /// Whether the joiner at `stand_in` has its channel open.
    pub fn is_open(&self, stand_in: SocketAddr) -> bool {
        self.side.as_ref().is_some_and(|s| s.is_open(stand_in))
    }

    /// What the data channels have done.
    pub fn counters(&self) -> super::rtc::RtcCounters {
        self.side.as_ref().map(|s| s.counters()).unwrap_or_default()
    }

    /// Whether the link to the room service is open (joiners can still come).
    pub fn room_open(&self) -> bool {
        self.link.as_ref().is_some_and(RoomLink::is_open)
    }

    fn say(&mut self, line: String) {
        if self.status.len() < 64 {
            self.status.push(line);
        }
    }

    /// One pass: room messages read and answered, the data channels' pass, and joiners whose channel closed reported gone.
    pub fn pump(&mut self) {
        let now = Instant::now();
        self.last_pump = Some(now);
        for _ in 0..ROOM_MESSAGES_PER_PASS {
            let Some(text) = self.link.as_mut().and_then(RoomLink::take) else {
                break;
            };
            self.room_message(&text, now);
        }
        if let Some(link) = self.link.as_mut() {
            link.keep_alive();
            if !link.is_open() {
                self.link = None;
                self.say("p2p: the room service closed the link: no one else can join".into());
            }
        }
        let Some(side) = self.side.as_mut() else {
            return;
        };
        side.pump(now, &mut self.inbox, self.setup.queue.max(1));
        let reflexive = side.reflexive().filter(|_| !self.reflexive_shown);
        let notices = side.take_notices();
        if let Some(mapped) = reflexive {
            self.reflexive_shown = true;
            self.say(format!("p2p: the STUN server sees this host at {mapped}"));
        }
        for (stand_in, _why) in notices {
            self.end(stand_in);
        }
    }

    /// The joiner at `stand_in` is over: its stand-in given back, the host told, the service told to drop its signaling.
    fn end(&mut self, stand_in: SocketAddr) {
        let Some(peer) = self.by_stand_in.remove(&stand_in) else {
            return;
        };
        if let Some(j) = self.joiners.remove(&peer) {
            self.stand_ins.give_back(j.ip, j.stand_in);
        }
        if self.gone.len() < 4096 {
            self.gone.push(stand_in);
        }
        if let Some(link) = self.link.as_mut() {
            link.send(&ToService::Drop(peer).write());
        }
    }

    fn room_message(&mut self, text: &str, now: Instant) {
        let Some(message) = FromService::parse(text) else {
            return;
        };
        let max = self.setup.rtc.limits.max_peers.saturating_mul(2);
        match message {
            FromService::Peer { peer, ip } => {
                if self.joiners.len() >= max || self.joiners.contains_key(&peer) {
                    if let Some(link) = self.link.as_mut() {
                        link.send(&ToService::Drop(peer).write());
                    }
                    return;
                }
                let ip = IpAddr::V4(ip);
                let stand_in = self.stand_ins.take(ip);
                self.joiners.insert(peer, Joiner { stand_in, ip });
                self.by_stand_in.insert(stand_in, peer);
            }
            FromService::From { peer, body } => {
                let Some(j) = self.joiners.get(&peer).copied() else {
                    return;
                };
                let Some(side) = self.side.as_mut() else {
                    return;
                };
                match side.signal(j.stand_in, j.ip, body, now) {
                    Ok(Reply::Text(answer)) => {
                        if let Some(link) = self.link.as_mut() {
                            link.send(&ToService::To(peer, &answer).write());
                        }
                    }
                    Ok(Reply::None) => {}
                    Err(()) => {
                        side.remove(j.stand_in);
                        self.end(j.stand_in);
                    }
                }
            }
            FromService::Left(peer) => {
                // Its signaling is over: a joiner whose channel is not open by now will not open it.
                let Some(j) = self.joiners.get(&peer).copied() else {
                    return;
                };
                if !self.is_open(j.stand_in) {
                    if let Some(side) = self.side.as_mut() {
                        side.remove(j.stand_in);
                    }
                    self.end(j.stand_in);
                }
            }
            FromService::Expired => {
                self.say("p2p: the room's time is up: no one else can join".into());
            }
            FromService::Refused(why) => self.say(format!("p2p: the room service refused: {why}")),
            FromService::Room { .. }
            | FromService::Joined { .. }
            | FromService::Signal(_)
            | FromService::Closed(_)
            | FromService::Pong => {}
        }
    }

    /// Opens the room: connects, asks for one and waits for its code.
    fn open_room(&mut self) -> Result<(), String> {
        let mut link = RoomLink::connect(&self.setup.service).map_err(|e| e.to_string())?;
        if !link.send(&ToService::Host.write()) {
            return Err("the room service closed the link".into());
        }
        let deadline = Instant::now() + OPEN_TIMEOUT;
        while Instant::now() < deadline && link.is_open() {
            let Some(text) = link.take() else {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            };
            match FromService::parse(&text) {
                Some(FromService::Room { code, ice, .. }) => {
                    self.code = Some(code.to_string());
                    self.ice = ice.to_string();
                    self.link = Some(link);
                    return Ok(());
                }
                Some(FromService::Refused(why)) => {
                    return Err(format!("the room service refused a room: {why}"));
                }
                _ => {}
            }
        }
        Err("the room service gave no room in time".into())
    }
}

impl Transport for P2pHost {
    /// Opens the data channels' UDP socket on `ip:port` (the host's candidate: its LAN address lets players on the network
    /// and, with the STUN server's help, outside it reach it), then the room.
    fn bind(&mut self, ip: IpAddr, port: u16) -> Result<(), BindError> {
        self.policy.check(ip).map_err(BindError::Policy)?;
        self.close();
        let mut rtc = self.setup.rtc.clone();
        rtc.host = true;
        rtc.port = port;
        let mut side = RtcSide::new(self.policy, rtc);
        side.bind(ip)?;
        self.side = Some(side);
        if let Err(e) = self.open_room() {
            self.say(format!("p2p: {e}"));
            self.close();
            return Err(BindError::Os(ErrorKind::ConnectionRefused));
        }
        if let Some((host, port)) = first_stun(&self.ice)
            && let Ok(mut found) = (host.as_str(), port).to_socket_addrs()
            && let Some(server) = found.find(SocketAddr::is_ipv4)
            && let Some(side) = self.side.as_mut()
        {
            side.query_reflexive(server);
        }
        let code = self.code().unwrap_or_default();
        self.say(format!("p2p: room {code}"));
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Recv {
        if self.inbox.is_empty()
            && self
                .last_pump
                .is_none_or(|t| t.elapsed() >= Duration::from_millis(1))
        {
            self.pump();
        }
        let Some((from, data)) = self.inbox.pop_front() else {
            return Recv::Empty;
        };
        let len = data.len().min(buf.len());
        if let (Some(dst), Some(src)) = (buf.get_mut(..len), data.get(..len)) {
            dst.copy_from_slice(src);
        }
        Recv::Datagram { len, from }
    }

    fn send_to(&mut self, to: SocketAddr, data: &[u8]) -> bool {
        let queue = self.setup.queue.max(1);
        let Some(side) = self.side.as_mut() else {
            return false;
        };
        side.send(to, data, Instant::now(), &mut self.inbox, queue)
            .unwrap_or(false)
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.side.as_ref().and_then(RtcSide::local_addr)
    }

    fn is_real(&self) -> bool {
        true
    }

    fn close(&mut self) {
        if let Some(side) = self.side.as_mut() {
            side.close();
        }
        self.side = None;
        self.link = None;
        self.joiners.clear();
        self.by_stand_in.clear();
        self.stand_ins.clear();
        self.inbox.clear();
    }

    fn service(&mut self) {
        if self.side.is_some() {
            self.pump();
        }
    }

    fn wants_service(&self) -> bool {
        self.side.is_some()
    }

    fn take_gone(&mut self) -> Vec<SocketAddr> {
        std::mem::take(&mut self.gone)
    }

    fn status(&mut self) -> Vec<String> {
        std::mem::take(&mut self.status)
    }
}

impl Drop for P2pHost {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_urls_and_stun_servers_read() {
        assert_eq!(
            parse_ws_url("ws://127.0.0.1:28100"),
            Ok(("127.0.0.1".into(), 28_100, "/ws".into()))
        );
        assert_eq!(
            parse_ws_url("ws://rooms.example/r/ws"),
            Ok(("rooms.example".into(), 80, "/r/ws".into()))
        );
        for bad in [
            "http://x",
            "wss://x",
            "ws://",
            "ws://a b",
            "ws://x:port",
            "ws://x:70000",
            "ws://x/\u{e9}",
        ] {
            assert!(parse_ws_url(bad).is_err(), "{bad}");
        }
        assert_eq!(
            first_stun("[{\"urls\":[\"stun:192.0.2.1:3479\"]}]"),
            Some(("192.0.2.1".into(), 3479))
        );
        assert_eq!(
            first_stun("[{\"urls\":[\"stun:stun.example\"]}]"),
            Some(("stun.example".into(), 3478))
        );
        assert_eq!(first_stun("[{\"urls\":[\"turn:x:1\"]}]"), None);
        assert_eq!(first_stun("[]"), None);
    }
}
