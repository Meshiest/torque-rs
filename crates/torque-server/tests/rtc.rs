//! The web transport's data channels on the loopback address: a client signals over its game socket, opens an
//! unreliable, unordered channel and sends datagrams on it both ways under its socket's stand-in, falls back to the socket
//! when the channel goes, and under 10% loss the channel keeps carrying datagrams in order with gaps. Forged datagrams
//! (garbage, checks and records, even from a peer's own address) are dropped and close nothing; a second peer checking
//! from another's address cannot take its traffic; a peer that breaks its channel's rules (oversized or text messages, a
//! second channel, too many messages) loses the channel and keeps its socket; media offers and too many offers or peers
//! are turned away; no pass takes long.

use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::Arc;
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::channel::{ChannelConfig, ChannelId, Reliability};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig};
use torque_server::net::transport::{BindPolicy, LanRules, RECV_BUFFER_BYTES, Recv, Transport};
use torque_server::net::web::http::base64;
use torque_server::net::web::rtc::{MAX_REMOTES, RtcLimits, RtcSetup, signal};
use torque_server::net::web::{
    WEB_SOCKET_PATH, WebLimits, WebSetup, WebTransport, accept_key, is_stand_in,
};
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::{Error as WsError, Message};

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// The longest one server pass may take while a flood comes in. A pass that does the flood's work in it takes far longer;
/// a shared CI machine can stall any pass for tens of milliseconds, so the bound leaves room for that.
const PASS_LIMIT: Duration = Duration::from_millis(100);

fn site(rtc: RtcSetup) -> (WebTransport, SocketAddr) {
    let policy = BindPolicy::new(LOOPBACK, false, &LanRules::PLAIN).unwrap();
    let mut t = WebTransport::new(
        policy,
        WebSetup {
            limits: WebLimits::default(),
            rtc: Some(rtc),
            ..WebSetup::default()
        },
    );
    t.bind(LOOPBACK, 0).unwrap();
    let addr = t.local_addr().unwrap();
    (t, addr)
}

fn recv(t: &mut impl Transport) -> Option<(SocketAddr, Vec<u8>)> {
    let mut buf = vec![0u8; RECV_BUFFER_BYTES];
    match t.recv(&mut buf) {
        Recv::Datagram { len, from } => Some((from, buf[..len].to_vec())),
        _ => None,
    }
}

/// A game socket opened by hand while the transport is pumped (the handshake blocks), then made non-blocking.
fn open_socket(t: &mut WebTransport, addr: SocketAddr) -> WebSocket<TcpStream> {
    let h = thread::spawn(move || {
        use std::io::{Read, Write};
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let key = base64(&[5; 16]);
        write!(
            s,
            "GET {WEB_SOCKET_PATH} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n\r\n"
        )
        .unwrap();
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            assert_eq!(s.read(&mut b).unwrap(), 1);
            head.push(b[0]);
        }
        let head = String::from_utf8(head).unwrap();
        assert!(head.starts_with("HTTP/1.1 101 "), "{head}");
        assert!(head.contains(&accept_key(&key)));
        s
    });
    while !h.is_finished() {
        t.pump();
        sleep(Duration::from_millis(1));
    }
    let s = h.join().unwrap();
    s.set_nonblocking(true).unwrap();
    WebSocket::from_raw_socket(s, Role::Client, None)
}

/// The game channel's config.
fn game_channel() -> ChannelConfig {
    ChannelConfig {
        label: signal::CHANNEL_LABEL.into(),
        ordered: false,
        reliability: Reliability::MaxRetransmits { retransmits: 0 },
        negotiated: None,
        protocol: String::new(),
    }
}

/// A browser's side, made with the same stack: a game socket for signaling and fallback, and a peer over UDP.
struct Client {
    ws: WebSocket<TcpStream>,
    rtc: Option<Rtc>,
    udp: UdpSocket,
    local: SocketAddr,
    pending: Option<SdpPendingOffer>,
    channel: Option<ChannelId>,
    /// Datagrams from the channel, with when they came.
    on_channel: Vec<(Instant, Vec<u8>)>,
    on_socket: Vec<Vec<u8>>,
    texts: Vec<String>,
    socket_closed: bool,
    channel_closed: bool,
    /// Whether its ICE checks were answered (the server's stack authenticated them).
    ice_connected: bool,
    /// More sockets, each a candidate of its own.
    extra: Vec<(SocketAddr, UdpSocket)>,
    /// Drops its own DTLS records (its checks still go): a handshake that never completes.
    no_dtls: bool,
}

impl Client {
    fn new(t: &mut WebTransport, addr: SocketAddr) -> Client {
        let ws = open_socket(t, addr);
        let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        udp.set_nonblocking(true).unwrap();
        let local = udp.local_addr().unwrap();
        Client {
            ws,
            rtc: None,
            udp,
            local,
            pending: None,
            channel: None,
            on_channel: Vec::new(),
            on_socket: Vec::new(),
            texts: Vec::new(),
            socket_closed: false,
            channel_closed: false,
            ice_connected: false,
            extra: Vec::new(),
            no_dtls: false,
        }
    }

    /// Makes the peer with `channels` and sends its offer (with `sdp_extra` appended).
    fn offer_with(&mut self, channels: &[ChannelConfig], sdp_extra: &str) {
        let mut rtc = RtcConfig::new()
            .set_crypto_provider(Arc::new(str0m::crypto::from_feature_flags()))
            .build(Instant::now());
        rtc.add_local_candidate(Candidate::host(self.local, "udp").unwrap());
        for (local, _) in &self.extra {
            rtc.add_local_candidate(Candidate::host(*local, "udp").unwrap());
        }
        let mut api = rtc.sdp_api();
        for c in channels {
            api.add_channel_with_config(c.clone());
        }
        let (offer, pending) = api.apply().unwrap();
        self.text(&format!("offer\n{}{sdp_extra}", offer.to_sdp_string()));
        self.rtc = Some(rtc);
        self.pending = Some(pending);
    }

    fn offer(&mut self) {
        self.offer_with(&[game_channel()], "");
    }

    fn text(&mut self, text: &str) {
        let _ = self.ws.send(Message::text(text));
        let _ = self.ws.flush();
    }

    fn send_socket(&mut self, data: &[u8]) {
        let _ = self.ws.send(Message::binary(data.to_vec()));
        let _ = self.ws.flush();
    }

    /// Sends on the channel (`binary` false: text).
    fn send_channel(&mut self, data: &[u8], binary: bool) -> bool {
        let (Some(rtc), Some(id)) = (self.rtc.as_mut(), self.channel) else {
            return false;
        };
        let sent = rtc
            .channel(id)
            .is_some_and(|mut c| c.write(binary, data).unwrap_or(false));
        self.outputs();
        sent
    }

    /// Reads the socket and the peer's UDP, runs the peer's timers and sends its outputs.
    fn step(&mut self) {
        loop {
            match self.ws.read() {
                Ok(Message::Text(text)) => {
                    let text = text.as_str().to_string();
                    if let Some(sdp) = text.strip_prefix("answer\n")
                        && let (Some(rtc), Some(pending)) = (self.rtc.as_mut(), self.pending.take())
                    {
                        // An answer to an earlier offer (a test that offers again) does not fit: dropped.
                        if let Ok(answer) = SdpAnswer::from_sdp_string(sdp) {
                            let _ = rtc.sdp_api().accept_answer(pending, answer);
                        }
                    }
                    self.texts.push(text);
                }
                Ok(Message::Binary(data)) => self.on_socket.push(data.to_vec()),
                Ok(_) => {}
                Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.socket_closed = true;
                    break;
                }
            }
        }
        let now = Instant::now();
        if let Some(rtc) = self.rtc.as_mut() {
            let mut buf = vec![0u8; 2048];
            let sockets = std::iter::once((self.local, &self.udp))
                .chain(self.extra.iter().map(|(a, u)| (*a, u)));
            for (local, udp) in sockets {
                loop {
                    match udp.recv_from(&mut buf) {
                        Ok((n, from)) => {
                            if let Ok(r) = Receive::new(Protocol::Udp, from, local, &buf[..n]) {
                                let _ = rtc.handle_input(Input::Receive(now, r));
                            }
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(_) => {}
                    }
                }
            }
            let _ = rtc.handle_input(Input::Timeout(now));
        }
        self.outputs();
    }

    fn outputs(&mut self) {
        let Some(rtc) = self.rtc.as_mut() else {
            return;
        };
        loop {
            match rtc.poll_output() {
                Ok(Output::Timeout(_)) | Err(_) => break,
                Ok(Output::Transmit(t)) => {
                    if self.no_dtls && (20..64).contains(&t.contents.first().copied().unwrap_or(0))
                    {
                        continue;
                    }
                    let udp = self
                        .extra
                        .iter()
                        .find(|(a, _)| *a == t.source)
                        .map_or(&self.udp, |(_, u)| u);
                    let _ = udp.send_to(&t.contents, t.destination);
                }
                Ok(Output::Event(Event::ChannelOpen(id, label))) => {
                    if label == signal::CHANNEL_LABEL {
                        self.channel = Some(id);
                    }
                }
                Ok(Output::Event(Event::ChannelData(d))) => {
                    self.on_channel.push((Instant::now(), d.data));
                }
                Ok(Output::Event(Event::IceConnectionStateChange(
                    IceConnectionState::Connected | IceConnectionState::Completed,
                ))) => {
                    self.ice_connected = true;
                }
                Ok(Output::Event(Event::ChannelClose(_) | Event::Closed)) => {
                    self.channel_closed = true;
                }
                Ok(Output::Event(_)) => {}
            }
        }
    }

    fn has_text(&self, text: &str) -> bool {
        self.texts.iter().any(|t| t == text)
    }

    fn count_text_prefix(&self, prefix: &str) -> usize {
        self.texts.iter().filter(|t| t.starts_with(prefix)).count()
    }
}

/// Pumps the transport and steps `clients` until `done` or `ms` pass; returns the longest pass.
fn run(
    t: &mut WebTransport,
    clients: &mut [&mut Client],
    ms: u64,
    mut done: impl FnMut(&mut WebTransport, &mut [&mut Client]) -> bool,
) -> Duration {
    let start = Instant::now();
    let mut longest = Duration::ZERO;
    while start.elapsed() < Duration::from_millis(ms) {
        let pass = Instant::now();
        t.pump();
        longest = longest.max(pass.elapsed());
        for c in clients.iter_mut() {
            c.step();
        }
        if done(t, clients) {
            break;
        }
        sleep(Duration::from_millis(1));
    }
    longest
}

/// A client with its channel open; returns it and its stand-in.
fn opened(t: &mut WebTransport, addr: SocketAddr) -> (Client, SocketAddr) {
    let mut c = Client::new(t, addr);
    run(t, &mut [&mut c], 2000, |_, c| c[0].has_text(signal::HELLO));
    assert!(
        c.has_text(signal::HELLO),
        "the server says it takes data channels"
    );
    c.offer();
    run(t, &mut [&mut c], 5000, |_, c| c[0].channel.is_some());
    assert!(
        c.channel.is_some(),
        "the channel opens: {:?}",
        t.rtc_counters()
    );
    assert!(c.send_channel(b"hello", true));
    let mut from = None;
    run(t, &mut [&mut c], 2000, |t, _| {
        from = recv(t).map(|(a, _)| a);
        from.is_some()
    });
    let from = from.expect("the channel's datagram reached the host");
    assert!(is_stand_in(from));
    (c, from)
}

#[test]
fn a_data_channel_carries_datagrams_both_ways_and_falls_back_to_the_socket() {
    let (mut t, addr) = site(RtcSetup::default());
    assert_eq!(
        t.rtc_candidates(),
        vec![t.rtc_addr().unwrap()],
        "the bind address is the one candidate"
    );
    let (mut c, stand_in) = opened(&mut t, addr);
    assert!(t.rtc_open(stand_in));

    // The socket's datagrams come from the same stand-in.
    c.send_socket(b"over the socket");
    let mut got = None;
    run(&mut t, &mut [&mut c], 2000, |t, _| {
        got = recv(t);
        got.is_some()
    });
    assert_eq!(got, Some((stand_in, b"over the socket".to_vec())));

    // The host's datagrams go on the channel.
    assert!(t.send_to(stand_in, b"down"));
    run(&mut t, &mut [&mut c], 2000, |_, c| {
        !c[0].on_channel.is_empty()
    });
    assert_eq!(
        c.on_channel.first().map(|(_, d)| d.as_slice()),
        Some(&b"down"[..])
    );
    assert!(c.on_socket.is_empty());
    assert_eq!(t.rtc_peers().len(), 1);

    // The client gives up its channel: the host's datagrams go on the socket again.
    c.rtc = None;
    c.channel = None;
    c.text(signal::CLOSED);
    run(&mut t, &mut [&mut c], 2000, |t, _| !t.rtc_open(stand_in));
    assert!(!t.rtc_open(stand_in));
    assert!(t.send_to(stand_in, b"down again"));
    run(&mut t, &mut [&mut c], 2000, |_, c| {
        !c[0].on_socket.is_empty()
    });
    assert_eq!(c.on_socket, vec![b"down again".to_vec()]);
    assert_eq!(t.sockets(), 1, "the socket stays");

    // The socket closes: its peer goes with it.
    let (mut d, d_in) = opened(&mut t, addr);
    assert!(t.rtc_open(d_in));
    let _ = d.ws.close(None);
    let _ = d.ws.flush();
    run(&mut t, &mut [&mut d], 2000, |t, _| t.rtc_peers().is_empty());
    assert!(t.rtc_peers().is_empty());
    assert!(t.take_gone().contains(&d_in));
}

/// The longest run of consecutive sequence numbers in `0..total` that are missing from `got` (ascending).
fn longest_missing_run(got: &[u32], total: u32) -> u32 {
    let mut longest = 0;
    let mut next = 0;
    for &n in got.iter().chain(&[total]) {
        longest = longest.max(n.saturating_sub(next));
        next = n + 1;
    }
    longest
}

#[test]
fn longest_missing_run_counts_gaps_and_the_tail() {
    assert_eq!(longest_missing_run(&[0, 1, 2], 3), 0);
    assert_eq!(longest_missing_run(&[0, 3, 4], 5), 2);
    assert_eq!(longest_missing_run(&[1, 2], 5), 2);
    assert_eq!(longest_missing_run(&[1], 5), 3);
    assert_eq!(longest_missing_run(&[], 4), 4);
}

/// The most consecutive datagrams that may be missing. Each is lost with probability 0.1, so a run this long happens by
/// chance about once in 10^11 positions; a channel that stopped delivering misses every datagram after it stopped.
const MAX_MISSING_RUN: u32 = 10;

fn datagram_number(data: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(data.try_into().ok()?))
}

#[test]
fn ten_percent_loss_never_stalls_the_channel() {
    const SENT: u32 = 400;
    let (mut t, addr) = site(RtcSetup {
        loss_percent: 10,
        ..RtcSetup::default()
    });
    let (mut c, stand_in) = opened(&mut t, addr);
    let mut up: Vec<u32> = Vec::new();
    let mut take_up = |t: &mut WebTransport| {
        while let Some((from, data)) = recv(t) {
            if from == stand_in
                && let Some(n) = datagram_number(&data)
            {
                up.push(n);
            }
        }
    };
    // Numbered datagrams each way, paced 5 ms apart. The pacing only gives the transport time between sends; how long a
    // pass takes is not measured, so a slow machine cannot fail the test, a channel that stops delivering does.
    let start = Instant::now();
    for n in 0..SENT {
        c.send_channel(&n.to_be_bytes(), true);
        t.send_to(stand_in, &n.to_be_bytes());
        let until = start + Duration::from_millis(5 * u64::from(n + 1));
        loop {
            t.pump();
            c.step();
            take_up(&mut t);
            if Instant::now() >= until {
                break;
            }
            sleep(Duration::from_millis(1));
        }
    }
    run(&mut t, &mut [&mut c], 300, |_, _| false);
    take_up(&mut t);
    let down: Vec<u32> = c
        .on_channel
        .iter()
        .filter_map(|(_, d)| datagram_number(d))
        .collect();
    for (name, numbers) in [("up", &up), ("down", &down)] {
        assert!(
            numbers.windows(2).all(|w| w[0] < w[1]),
            "{name}: in the order sent: {numbers:?}"
        );
        assert!(
            (300..400).contains(&numbers.len()),
            "{name}: some lost, most through: {}",
            numbers.len()
        );
        let run = longest_missing_run(numbers, SENT);
        assert!(
            run <= MAX_MISSING_RUN,
            "{name}: no stall, {run} datagrams in a row missing: {numbers:?}"
        );
    }
    assert!(t.rtc_counters().loss_dropped > 40, "{:?}", t.rtc_counters());
}

/// Sends `count` datagrams of `make(i)` from `udp` to `to` while the transport is pumped; returns the longest pass.
fn flood(
    t: &mut WebTransport,
    clients: &mut [&mut Client],
    udp: &UdpSocket,
    to: SocketAddr,
    ms: u64,
    mut make: impl FnMut(u32) -> Vec<u8>,
) -> Duration {
    let start = Instant::now();
    let mut longest = Duration::ZERO;
    let mut i = 0u32;
    while start.elapsed() < Duration::from_millis(ms) {
        for _ in 0..200 {
            let _ = udp.send_to(&make(i), to);
            i = i.wrapping_add(1);
        }
        let pass = Instant::now();
        t.pump();
        longest = longest.max(pass.elapsed());
        for c in clients.iter_mut() {
            c.step();
        }
    }
    longest
}

/// Bytes from a small deterministic generator.
fn noise(seed: u32, len: usize) -> Vec<u8> {
    let mut x = u64::from(seed).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// A STUN binding request naming `ufrag`, with no valid integrity.
fn stun_request(ufrag: &str, seed: u32) -> Vec<u8> {
    let user = format!("{ufrag}:abcd");
    let mut attr = vec![0x00, 0x06];
    attr.extend_from_slice(&(user.len() as u16).to_be_bytes());
    attr.extend_from_slice(user.as_bytes());
    attr.resize(attr.len().div_ceil(4) * 4, 0);
    let mut d = vec![0x00, 0x01];
    d.extend_from_slice(&(attr.len() as u16).to_be_bytes());
    d.extend_from_slice(&[0x21, 0x12, 0xa4, 0x42]);
    d.extend_from_slice(&noise(seed, 12));
    d.extend_from_slice(&attr);
    d
}

#[test]
fn garbage_from_strangers_is_dropped_unread_with_passes_bounded() {
    let (mut t, addr) = site(RtcSetup::default());
    let (mut c, stand_in) = opened(&mut t, addr);
    let rtc_addr = t.rtc_addr().unwrap();
    let stranger = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    // Every first byte, DTLS-looking records, STUN with a fragment no peer has, oversized datagrams.
    let longest = flood(
        &mut t,
        &mut [&mut c],
        &stranger,
        rtc_addr,
        1500,
        |i| match i % 4 {
            0 => noise(i, 1 + (i as usize * 7) % 1400),
            1 => {
                let mut d = noise(i, 200);
                d[0] = 22;
                d[1] = 0xfe;
                d[2] = 0xfd;
                d
            }
            2 => stun_request("nobody00", i),
            _ => noise(i, 3000),
        },
    );
    let counters = t.rtc_counters();
    assert!(counters.stray > 1000, "{counters:?}");
    assert!(longest < PASS_LIMIT, "a pass took {longest:?}");
    // The peer is untouched.
    assert!(t.rtc_open(stand_in));
    assert!(c.send_channel(b"still here", true));
    let mut got = None;
    run(&mut t, &mut [&mut c], 2000, |t, _| {
        got = recv(t);
        got.is_some()
    });
    assert_eq!(got, Some((stand_in, b"still here".to_vec())));
}

#[test]
fn checks_naming_a_peer_from_strangers_are_rate_limited_and_never_close_it() {
    let (mut t, addr) = site(RtcSetup::default());
    let mut c = Client::new(&mut t, addr);
    run(&mut t, &mut [&mut c], 2000, |_, c| {
        c[0].has_text(signal::HELLO)
    });
    c.offer();
    run(&mut t, &mut [&mut c], 2000, |_, c| {
        c[0].texts.iter().any(|t| t.starts_with("answer\n"))
    });
    let answer = c
        .texts
        .iter()
        .find(|t| t.starts_with("answer\n"))
        .unwrap()
        .clone();
    let ufrag = answer
        .lines()
        .find_map(|l| l.strip_prefix("a=ice-ufrag:"))
        .unwrap()
        .trim()
        .to_string();
    assert_eq!(ufrag.len(), 8);
    let stranger = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let rtc_addr = t.rtc_addr().unwrap();
    let longest = flood(&mut t, &mut [], &stranger, rtc_addr, 1500, |i| {
        stun_request(&ufrag, i)
    });
    assert!(longest < PASS_LIMIT, "a pass took {longest:?}");
    assert!(
        t.rtc_counters().stun_limited > 1000,
        "{:?}",
        t.rtc_counters()
    );
    // The peer still connects.
    run(&mut t, &mut [&mut c], 5000, |_, c| c[0].channel.is_some());
    assert!(c.channel.is_some(), "{:?}", t.rtc_counters());
}

/// Datagrams that look like DTLS records of the session (application data, epoch 1) with random contents.
fn fake_record(i: u32) -> Vec<u8> {
    let mut d = noise(i, 300);
    d[0] = 23;
    d[1] = 0xfe;
    d[2] = 0xfd;
    d[3] = 0;
    d[4] = 1;
    d
}

/// Whether `c`'s channel still carries datagrams both ways under `stand_in`.
fn both_ways(t: &mut WebTransport, c: &mut Client, stand_in: SocketAddr, tag: &[u8]) -> bool {
    c.on_channel.clear();
    let mut up = false;
    let start = Instant::now();
    let done = |up: bool, c: &Client| up && !c.on_channel.is_empty();
    while start.elapsed() < Duration::from_secs(3) && !done(up, c) {
        c.send_channel(tag, true);
        t.send_to(stand_in, tag);
        t.pump();
        c.step();
        while let Some((from, data)) = recv(t) {
            up |= from == stand_in && data == tag;
        }
        sleep(Duration::from_millis(5));
    }
    up && c.on_channel.iter().any(|(_, d)| d == tag)
}

#[test]
fn forged_traffic_from_a_peers_own_address_leaves_it_connected() {
    let (mut t, addr) = site(RtcSetup::default());
    let (mut c, stand_in) = opened(&mut t, addr);
    let ufrag = c
        .texts
        .iter()
        .find_map(|t| t.lines().find_map(|l| l.strip_prefix("a=ice-ufrag:")))
        .unwrap()
        .trim()
        .to_string();
    let to = t.rtc_addr().unwrap();
    // Sent from the peer's own address (as a forger would): records of the session's epoch with random contents, checks
    // naming the peer with no valid integrity, and bytes of every kind, far past the caps a second.
    let forger = c.udp.try_clone().unwrap();
    let longest = flood(&mut t, &mut [], &forger, to, 1500, |i| match i % 3 {
        0 => fake_record(i),
        1 => stun_request(&ufrag, i),
        _ => noise(i, 1 + (i as usize * 13) % 1200),
    });
    assert!(longest < PASS_LIMIT, "a pass took {longest:?}");
    let counters = t.rtc_counters();
    assert!(
        counters.rate_dropped + counters.refused + counters.pass_dropped > 1000,
        "{counters:?}"
    );
    assert_eq!(
        counters.closed + counters.closed_rate + counters.closed_protocol + counters.closed_timeout,
        0,
        "nothing was closed: {counters:?}"
    );
    // The flood past, the peer and its socket carry on.
    sleep(Duration::from_millis(1100));
    assert!(t.rtc_open(stand_in));
    assert!(
        both_ways(&mut t, &mut c, stand_in, b"after the flood"),
        "{:?}",
        t.rtc_counters()
    );
    assert!(!c.socket_closed);
    assert_eq!(t.sockets(), 1);
}

#[test]
fn a_second_peer_checking_from_anothers_address_cannot_take_its_traffic() {
    let (mut t, addr) = site(RtcSetup::default());
    let (mut a, a_in) = opened(&mut t, addr);
    // B signals over its own socket, then sends its (valid) checks and its DTLS handshake from A's address.
    let mut b = Client::new(&mut t, addr);
    run(&mut t, &mut [&mut b], 2000, |_, c| {
        c[0].has_text(signal::HELLO)
    });
    b.udp = a.udp.try_clone().unwrap();
    b.local = a.local;
    b.offer();
    run(&mut t, &mut [&mut b], 2000, |_, _| false);
    assert!(b.ice_connected, "B's checks from A's address passed");
    assert!(b.channel.is_none(), "B never gets a channel at A's address");
    let peers = t.rtc_peers();
    assert_eq!(peers.len(), 2);
    assert!(peers.iter().any(|(s, p)| *s != a_in && !p.open));
    // A's traffic still goes to A's peer, both ways (B's client no longer reads A's socket).
    b.rtc = None;
    assert!(t.rtc_open(a_in));
    assert!(
        both_ways(&mut t, &mut a, a_in, b"still A's"),
        "{:?}",
        t.rtc_counters()
    );
    assert!(!a.socket_closed && !b.socket_closed);
}

#[test]
fn a_peer_that_breaks_its_channels_rules_loses_the_channel_and_keeps_its_socket() {
    // Fewer messages a second than datagrams, so that the message cap (not the datagrams', which only drops) is reached.
    let limits = RtcLimits {
        messages_per_s: 100,
        ..RtcLimits::default()
    };
    let (mut t, addr) = site(RtcSetup {
        limits,
        ..RtcSetup::default()
    });
    // Each case: the server tells the client its channel is closed, and the game socket carries on.
    let expect_closed = |t: &mut WebTransport, c: &mut Client, stand_in: SocketAddr, what: &str| {
        run(t, &mut [&mut *c], 3000, |_, c| {
            c[0].has_text(signal::CLOSED)
        });
        assert!(c.has_text(signal::CLOSED), "{what}: {:?}", t.rtc_counters());
        assert!(!c.socket_closed, "{what}: the socket stays");
        assert!(!t.rtc_open(stand_in));
        // The host's datagrams go on the socket again.
        assert!(t.send_to(stand_in, what.as_bytes()));
        run(t, &mut [&mut *c], 2000, |_, c| !c[0].on_socket.is_empty());
        assert_eq!(
            c.on_socket.last().map(Vec::as_slice),
            Some(what.as_bytes()),
            "{what}"
        );
    };

    // A message larger than a datagram.
    let (mut c, s) = opened(&mut t, addr);
    c.send_channel(&vec![1u8; 4000], true);
    expect_closed(&mut t, &mut c, s, "oversize");
    assert_eq!(t.rtc_counters().closed_oversize, 1);

    // Text on the channel.
    let (mut c, s) = opened(&mut t, addr);
    c.send_channel(b"words", false);
    expect_closed(&mut t, &mut c, s, "text");

    // A second channel.
    let (mut c, s) = opened(&mut t, addr);
    let rtc = c.rtc.as_mut().unwrap();
    let mut api = rtc.direct_api();
    api.create_data_channel(ChannelConfig {
        label: "extra".into(),
        ..game_channel()
    });
    c.outputs();
    expect_closed(&mut t, &mut c, s, "a second channel");

    // More messages a second than the cap.
    let (mut c, s) = opened(&mut t, addr);
    let start = Instant::now();
    let mut longest = Duration::ZERO;
    while !c.has_text(signal::CLOSED) && start.elapsed() < Duration::from_secs(3) {
        for _ in 0..50 {
            c.send_channel(b"m", true);
        }
        let pass = Instant::now();
        t.pump();
        longest = longest.max(pass.elapsed());
        c.step();
        while recv(&mut t).is_some() {}
    }
    assert!(longest < PASS_LIMIT, "a pass took {longest:?}");
    expect_closed(&mut t, &mut c, s, "message rate");
    let counters = t.rtc_counters();
    assert_eq!(counters.closed_protocol, 2, "{counters:?}");
    assert_eq!(counters.closed_rate, 1, "{counters:?}");
    assert_eq!(t.sockets(), 4, "every socket stays");
}

#[test]
fn signaling_is_capped() {
    let limits = RtcLimits {
        peers_per_ip: 1,
        connect_timeout: Duration::from_millis(400),
        ..RtcLimits::default()
    };
    let (mut t, addr) = site(RtcSetup {
        limits,
        ..RtcSetup::default()
    });

    // An offer with audio is refused (no voice yet), and closes the socket.
    let mut c = Client::new(&mut t, addr);
    c.text("offer\nv=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n");
    run(&mut t, &mut [&mut c], 2000, |_, c| c[0].socket_closed);
    assert!(c.socket_closed);

    // Malformed signals close it too.
    for bad in ["offer\nnot sdp", "answer\nv=0", "hello"] {
        let mut c = Client::new(&mut t, addr);
        c.text(bad);
        run(&mut t, &mut [&mut c], 2000, |_, c| c[0].socket_closed);
        assert!(c.socket_closed, "{bad}");
    }

    // One peer per address: a second socket's offer is turned away, and the socket stays.
    let mut a = Client::new(&mut t, addr);
    a.offer();
    // It never answers checks.
    a.rtc = None;
    let mut b = Client::new(&mut t, addr);
    b.offer();
    run(&mut t, &mut [&mut a, &mut b], 2000, |_, c| {
        c[1].has_text(signal::BUSY)
    });
    assert!(b.has_text(signal::BUSY));
    assert!(!b.socket_closed);
    assert_eq!(t.rtc_counters().busy, 1);

    // A peer that never connects is dropped at its deadline; its socket is told and stays.
    run(&mut t, &mut [&mut a, &mut b], 2000, |_, c| {
        c[0].has_text(signal::CLOSED)
    });
    assert!(a.has_text(signal::CLOSED));
    assert!(!a.socket_closed);
    assert_eq!(t.rtc_counters().closed_timeout, 1);

    // Offers past the cap close the socket (the first four are answered).
    for _ in 0..5 {
        a.offer();
        run(&mut t, &mut [&mut a], 300, |_, c| c[0].socket_closed);
    }
    assert!(a.socket_closed);
    // Candidates are taken and dropped.
    b.text("candidate\ncandidate:1 1 udp 1 127.0.0.1 9 typ host");
    run(&mut t, &mut [&mut b], 200, |_, _| false);
    assert!(!b.socket_closed);
}

#[test]
fn an_ice_restart_drops_the_channel_and_keeps_the_socket() {
    let (mut t, addr) = site(RtcSetup::default());
    let (mut c, _) = opened(&mut t, addr);
    // A renegotiation with its own credentials (what a restart offers): the channel goes, the socket stays.
    let mut other = RtcConfig::new()
        .set_crypto_provider(Arc::new(str0m::crypto::from_feature_flags()))
        .build(Instant::now());
    other.add_local_candidate(Candidate::host(c.local, "udp").unwrap());
    let mut api = other.sdp_api();
    api.add_channel_with_config(game_channel());
    let (offer, _) = api.apply().unwrap();
    c.text(&format!("offer\n{}", offer.to_sdp_string()));
    run(&mut t, &mut [&mut c], 2000, |_, c| {
        c[0].has_text(signal::CLOSED)
    });
    assert!(c.has_text(signal::CLOSED), "{:?}", t.rtc_counters());
    assert!(!c.socket_closed);
    assert_eq!(
        c.count_text_prefix("answer\n"),
        1,
        "the first offer's answer only, none to a restart"
    );
    assert_eq!(t.rtc_counters().closed_protocol, 1);
    assert!(t.rtc_peers().is_empty());
}

#[test]
fn forged_datagrams_never_keep_a_silent_peer_heard() {
    let limits = RtcLimits {
        idle_timeout: Duration::from_millis(800),
        ..RtcLimits::default()
    };
    let (mut t, addr) = site(RtcSetup {
        limits,
        ..RtcSetup::default()
    });
    let (mut c, stand_in) = opened(&mut t, addr);
    let ufrag = c
        .texts
        .iter()
        .find_map(|t| t.lines().find_map(|l| l.strip_prefix("a=ice-ufrag:")))
        .unwrap()
        .trim()
        .to_string();
    // The browser goes silent; a forger sends from its address: records that do not decrypt, checks that fail their
    // integrity, garbage.
    c.rtc = None;
    let forger = c.udp.try_clone().unwrap();
    let to = t.rtc_addr().unwrap();
    let longest = flood(&mut t, &mut [&mut c], &forger, to, 2500, |i| match i % 3 {
        0 => fake_record(i),
        1 => stun_request(&ufrag, i),
        _ => noise(i, 200),
    });
    assert!(longest < PASS_LIMIT, "a pass took {longest:?}");
    assert!(
        !t.rtc_open(stand_in),
        "the silent peer was closed at its deadline: {:?}",
        t.rtc_counters()
    );
    let counters = t.rtc_counters();
    assert_eq!(counters.closed_timeout + counters.closed, 1, "{counters:?}");
    assert_eq!(
        counters.rate_dropped, 0,
        "forged datagrams spent no budget a second: {counters:?}"
    );
    run(&mut t, &mut [&mut c], 1000, |_, c| {
        c[0].has_text(signal::CLOSED)
    });
    assert!(c.has_text(signal::CLOSED) && !c.socket_closed);
}

#[test]
fn a_peer_maps_at_most_four_source_addresses() {
    let (mut t, addr) = site(RtcSetup::default());
    let mut c = Client::new(&mut t, addr);
    for _ in 0..6 {
        let u = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        u.set_nonblocking(true).unwrap();
        c.extra.push((u.local_addr().unwrap(), u));
    }
    run(&mut t, &mut [&mut c], 2000, |_, c| {
        c[0].has_text(signal::HELLO)
    });
    c.offer();
    run(&mut t, &mut [&mut c], 5000, |_, c| c[0].channel.is_some());
    assert!(c.channel.is_some(), "{:?}", t.rtc_counters());
    // Let every pair's checks pass, then send a datagram from each address the peer's stack accepts.
    run(&mut t, &mut [&mut c], 1500, |_, _| false);
    let to = t.rtc_addr().unwrap();
    for (i, (_, u)) in c.extra.iter().enumerate() {
        let _ = u.send_to(&fake_record(i as u32), to);
    }
    let _ = c.udp.send_to(&fake_record(99), to);
    run(&mut t, &mut [&mut c], 300, |_, _| false);
    assert_eq!(t.rtc_routes(), MAX_REMOTES, "{:?}", t.rtc_counters());
    // The peer carries on (its own address mapped again when it is needed).
    assert!(c.send_channel(b"hi", true));
    let mut got = None;
    run(&mut t, &mut [&mut c], 2000, |t, _| {
        got = recv(t);
        got.is_some()
    });
    assert!(got.is_some_and(|(a, d)| is_stand_in(a) && d == b"hi"));
    assert!(t.rtc_routes() <= MAX_REMOTES);
}

#[test]
fn a_handshake_that_never_completes_stops_costing_work_after_its_budget() {
    let limits = RtcLimits {
        connect_timeout: Duration::from_millis(2500),
        ..RtcLimits::default()
    };
    let budget = limits.connect_datagrams;
    let (mut t, addr) = site(RtcSetup {
        limits,
        ..RtcSetup::default()
    });
    let mut c = Client::new(&mut t, addr);
    c.no_dtls = true;
    run(&mut t, &mut [&mut c], 2000, |_, c| {
        c[0].has_text(signal::HELLO)
    });
    c.offer();
    run(&mut t, &mut [&mut c], 2000, |_, c| c[0].ice_connected);
    assert!(
        c.ice_connected,
        "its checks passed, so its address reaches its peer"
    );
    // Handshake records from its own (validated) address, far past the budget.
    let udp = c.udp.try_clone().unwrap();
    let to = t.rtc_addr().unwrap();
    let handled = t.rtc_counters().handled;
    let longest = flood(&mut t, &mut [&mut c], &udp, to, 1500, |i| {
        let mut d = fake_record(i);
        d[0] = 22;
        d[4] = 0;
        d
    });
    assert!(longest < PASS_LIMIT, "a pass took {longest:?}");
    let counters = t.rtc_counters();
    assert!(counters.connect_spent > 1000, "{counters:?}");
    assert!(
        // The budget, and the client's own checks meanwhile (outside it).
        counters.handled - handled <= u64::from(budget) + 100,
        "the stack was handed {} datagrams during the flood (budget {budget})",
        counters.handled - handled
    );
    // It is dropped at its deadline; its socket stays.
    run(&mut t, &mut [&mut c], 2000, |_, c| {
        c[0].has_text(signal::CLOSED)
    });
    assert!(c.has_text(signal::CLOSED), "{:?}", t.rtc_counters());
    assert!(!c.socket_closed);
    assert_eq!(t.rtc_counters().closed_timeout, 1);
}
