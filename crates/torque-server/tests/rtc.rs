//! The web transport's data channels on the loopback address: a client signals over its game socket, opens an
//! unreliable, unordered channel and sends datagrams on it both ways under its socket's stand-in, falls back to the socket
//! when the channel goes, and under 10% loss the channel keeps carrying datagrams in order with gaps. Hostile peers
//! (garbage, STUN, DTLS and SCTP floods, oversized and text messages, a second channel, media offers, too many offers or
//! peers) are closed or turned away, and no pass takes long.

use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::Arc;
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::channel::{ChannelConfig, ChannelId, Reliability};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc, RtcConfig};
use torque_server::net::transport::{BindPolicy, LanRules, RECV_BUFFER_BYTES, Recv, Transport};
use torque_server::net::web::http::base64;
use torque_server::net::web::rtc::{RtcLimits, RtcSetup, signal};
use torque_server::net::web::{
    WEB_SOCKET_PATH, WebLimits, WebSetup, WebTransport, accept_key, is_stand_in,
};
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::{Error as WsError, Message};

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

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
        }
    }

    /// Makes the peer with `channels` and sends its offer (with `sdp_extra` appended).
    fn offer_with(&mut self, channels: &[ChannelConfig], sdp_extra: &str) {
        let mut rtc = RtcConfig::new()
            .set_crypto_provider(Arc::new(str0m::crypto::from_feature_flags()))
            .build(Instant::now());
        rtc.add_local_candidate(Candidate::host(self.local, "udp").unwrap());
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
            loop {
                match self.udp.recv_from(&mut buf) {
                    Ok((n, from)) => {
                        if let Ok(r) = Receive::new(Protocol::Udp, from, self.local, &buf[..n]) {
                            let _ = rtc.handle_input(Input::Receive(now, r));
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => {}
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
                    let _ = self.udp.send_to(&t.contents, t.destination);
                }
                Ok(Output::Event(Event::ChannelOpen(id, label))) => {
                    if label == signal::CHANNEL_LABEL {
                        self.channel = Some(id);
                    }
                }
                Ok(Output::Event(Event::ChannelData(d))) => {
                    self.on_channel.push((Instant::now(), d.data));
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

#[test]
fn ten_percent_loss_never_stalls_the_channel() {
    let (mut t, addr) = site(RtcSetup {
        loss_percent: 10,
        ..RtcSetup::default()
    });
    let (mut c, stand_in) = opened(&mut t, addr);
    // 400 numbered datagrams each way, 5 ms apart.
    let mut arrived: Vec<(Instant, u32)> = Vec::new();
    let start = Instant::now();
    for n in 0u32..400 {
        c.send_channel(&n.to_be_bytes(), true);
        t.send_to(stand_in, &n.to_be_bytes());
        let until = start + Duration::from_millis(5 * u64::from(n + 1));
        while Instant::now() < until {
            t.pump();
            c.step();
            while let Some((from, data)) = recv(&mut t) {
                if from == stand_in && data.len() == 4 {
                    arrived.push((
                        Instant::now(),
                        u32::from_be_bytes(data[..4].try_into().unwrap()),
                    ));
                }
            }
            sleep(Duration::from_millis(1));
        }
    }
    run(&mut t, &mut [&mut c], 300, |_, _| false);
    while let Some((from, data)) = recv(&mut t) {
        if from == stand_in && data.len() == 4 {
            arrived.push((
                Instant::now(),
                u32::from_be_bytes(data[..4].try_into().unwrap()),
            ));
        }
    }
    let up: Vec<u32> = arrived.iter().map(|&(_, n)| n).collect();
    let down: Vec<(Instant, u32)> = c
        .on_channel
        .iter()
        .filter(|(_, d)| d.len() == 4)
        .map(|(at, d)| (*at, u32::from_be_bytes(d[..4].try_into().unwrap())))
        .collect();
    for (name, got) in [("up", arrived.clone()), ("down", down)] {
        let numbers: Vec<u32> = got.iter().map(|&(_, n)| n).collect();
        assert!(
            numbers.windows(2).all(|w| w[0] < w[1]),
            "{name}: in the order sent: {numbers:?}"
        );
        assert!(
            (300..400).contains(&numbers.len()),
            "{name}: some lost, most through: {}",
            numbers.len()
        );
        let gap = got
            .windows(2)
            .map(|w| w[1].0.duration_since(w[0].0))
            .max()
            .unwrap();
        assert!(
            gap < Duration::from_millis(150),
            "{name}: no stall, the longest gap {gap:?}"
        );
    }
    assert!(t.rtc_counters().loss_dropped > 40, "{:?}", t.rtc_counters());
    assert!(up.len() < 400);
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
    assert!(
        longest < Duration::from_millis(20),
        "a pass took {longest:?}"
    );
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
    assert!(
        longest < Duration::from_millis(20),
        "a pass took {longest:?}"
    );
    assert!(
        t.rtc_counters().stun_limited > 1000,
        "{:?}",
        t.rtc_counters()
    );
    // The peer still connects.
    run(&mut t, &mut [&mut c], 5000, |_, c| c[0].channel.is_some());
    assert!(c.channel.is_some(), "{:?}", t.rtc_counters());
}

#[test]
fn a_peer_past_its_caps_is_closed_and_loses_its_socket() {
    // Its own address flooding records (they reach its peer): the datagrams a second close it.
    let (mut t, addr) = site(RtcSetup::default());
    let (mut c, _) = opened(&mut t, addr);
    let to = t.rtc_addr().unwrap();
    let udp = c.udp.try_clone().unwrap();
    let longest = flood(&mut t, &mut [&mut c], &udp, to, 1500, |i| {
        let mut d = noise(i, 300);
        d[0] = 23;
        d[1] = 0xfe;
        d[2] = 0xfd;
        d
    });
    assert!(
        longest < Duration::from_millis(20),
        "a pass took {longest:?}"
    );
    let counters = t.rtc_counters();
    assert_eq!(
        counters.closed_rate + counters.closed_protocol,
        1,
        "{counters:?}"
    );
    run(&mut t, &mut [&mut c], 1000, |_, c| c[0].socket_closed);
    assert!(c.socket_closed, "its game socket is closed too");
    assert_eq!(t.sockets(), 0);

    // A message larger than a datagram.
    let (mut c, _) = opened(&mut t, addr);
    c.send_channel(&vec![1u8; 4000], true);
    run(&mut t, &mut [&mut c], 2000, |_, c| c[0].socket_closed);
    assert!(c.socket_closed);
    assert_eq!(t.rtc_counters().closed_oversize, 1);

    // Text on the channel.
    let (mut c, _) = opened(&mut t, addr);
    c.send_channel(b"words", false);
    run(&mut t, &mut [&mut c], 2000, |_, c| c[0].socket_closed);
    assert!(c.socket_closed);

    // A second channel.
    let (mut c, _) = opened(&mut t, addr);
    let rtc = c.rtc.as_mut().unwrap();
    let mut api = rtc.direct_api();
    api.create_data_channel(ChannelConfig {
        label: "extra".into(),
        ..game_channel()
    });
    c.outputs();
    run(&mut t, &mut [&mut c], 3000, |_, c| c[0].socket_closed);
    assert!(c.socket_closed, "a second channel closes the peer");

    // More messages a second than the cap.
    let (mut c, _) = opened(&mut t, addr);
    let start = Instant::now();
    let mut longest = Duration::ZERO;
    while !c.socket_closed && start.elapsed() < Duration::from_secs(3) {
        for _ in 0..50 {
            c.send_channel(b"m", true);
        }
        let pass = Instant::now();
        t.pump();
        longest = longest.max(pass.elapsed());
        c.step();
        while recv(&mut t).is_some() {}
    }
    assert!(c.socket_closed, "{:?}", t.rtc_counters());
    assert!(
        longest < Duration::from_millis(20),
        "a pass took {longest:?}"
    );
    let counters = t.rtc_counters();
    assert!(counters.closed_protocol >= 2, "{counters:?}");
    assert!(counters.closed_rate >= 1, "{counters:?}");
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
