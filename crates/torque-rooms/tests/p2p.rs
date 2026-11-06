//! A player's host (torque-server's `net::web::p2p`) and joiners on the loopback address, through a live room service: a
//! joiner finds the host by its code, signals through the room, opens its data channel straight to the host, and
//! datagrams go both ways under a stand-in address; when the joiner's room socket closes its channel carries on, and when
//! the host closes the joiner's channel closes. Hostile joiners (media offers, offers that are not SDP, too many
//! candidates, many offers) are turned away without touching the open channel, and no host pass takes long.

use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::channel::{ChannelConfig, ChannelId, Reliability};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig};
use torque_rooms::{Ice, RoomLimits, Rooms, Service, service_limits};
use torque_server::net::room::{FromService, ToService};
use torque_server::net::transport::{BindPolicy, LanRules, RECV_BUFFER_BYTES, Recv, Transport};
use torque_server::net::web::p2p::{P2pHost, P2pSetup, RoomLink};
use torque_server::net::web::rtc::{RtcSetup, signal};
use torque_server::net::web::{WebLimits, WebSetup, is_stand_in};

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// A room service on its own thread (its transport is not `Send`), passed until the guard drops; the longest pass is
/// reported when it stops.
struct Running {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    longest: mpsc::Receiver<Duration>,
}

impl Running {
    fn url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    fn stop(self) -> Duration {
        self.stop.store(true, Ordering::Relaxed);
        self.longest.recv().unwrap()
    }
}

fn service() -> Running {
    let (tx, rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    thread::spawn(move || {
        let policy = BindPolicy::new(LOOPBACK, false, &LanRules::PLAIN).unwrap();
        let setup = WebSetup {
            limits: WebLimits {
                connects_per_ip_per_s: 64,
                ..service_limits()
            },
            ..WebSetup::default()
        };
        let mut s = Service::new(
            policy,
            setup,
            Rooms::new(RoomLimits::default(), Ice::default()),
        );
        s.bind(LOOPBACK, 0).unwrap();
        tx.send(s.local_addr().unwrap()).unwrap();
        let mut longest = Duration::ZERO;
        while !flag.load(Ordering::Relaxed) {
            let t = Instant::now();
            s.pass();
            longest = longest.max(t.elapsed());
            sleep(Duration::from_millis(1));
        }
        let _ = done_tx.send(longest);
    });
    Running {
        addr: rx.recv().unwrap(),
        stop,
        longest: done_rx,
    }
}

fn host(url: &str) -> P2pHost {
    let policy = BindPolicy::new(LOOPBACK, false, &LanRules::PLAIN).unwrap();
    let mut h = P2pHost::new(
        policy,
        P2pSetup {
            service: url.to_string(),
            rtc: RtcSetup::default(),
            queue: 4096,
        },
    );
    h.bind(LOOPBACK, 0).unwrap();
    h
}

fn recv(t: &mut impl Transport) -> Option<(SocketAddr, Vec<u8>)> {
    let mut buf = vec![0u8; RECV_BUFFER_BYTES];
    match t.recv(&mut buf) {
        Recv::Datagram { len, from } => Some((from, buf[..len].to_vec())),
        _ => None,
    }
}

fn game_channel() -> ChannelConfig {
    ChannelConfig {
        label: signal::CHANNEL_LABEL.into(),
        ordered: false,
        reliability: Reliability::MaxRetransmits { retransmits: 0 },
        negotiated: None,
        protocol: String::new(),
    }
}

/// A joiner made with the same stack as the host: a room socket for signaling and a peer over UDP.
struct Joiner {
    room: Option<RoomLink>,
    peer: Option<u32>,
    refused: Option<String>,
    closed_by_room: Option<String>,
    rtc: Option<Rtc>,
    udp: UdpSocket,
    local: SocketAddr,
    pending: Option<SdpPendingOffer>,
    channel: Option<ChannelId>,
    got: Vec<Vec<u8>>,
    channel_closed: bool,
}

impl Joiner {
    fn new(url: &str) -> Joiner {
        let url = url.to_string();
        let room = thread::spawn(move || RoomLink::connect(&url))
            .join()
            .unwrap()
            .unwrap();
        let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        udp.set_nonblocking(true).unwrap();
        let local = udp.local_addr().unwrap();
        Joiner {
            room: Some(room),
            peer: None,
            refused: None,
            closed_by_room: None,
            rtc: None,
            udp,
            local,
            pending: None,
            channel: None,
            got: Vec::new(),
            channel_closed: false,
        }
    }

    fn say(&mut self, message: &ToService<'_>) {
        if let Some(room) = self.room.as_mut() {
            room.send(&message.write());
        }
    }

    /// Makes the peer and sends its offer (`edit` may change it) and its host candidate.
    fn offer_with(&mut self, edit: impl Fn(String) -> String) {
        let mut rtc = RtcConfig::new()
            .set_crypto_provider(Arc::new(str0m::crypto::from_feature_flags()))
            .build(Instant::now());
        let candidate = Candidate::host(self.local, "udp").unwrap();
        let line = candidate.to_sdp_string();
        rtc.add_local_candidate(candidate);
        let mut api = rtc.sdp_api();
        api.add_channel_with_config(game_channel());
        let (offer, pending) = api.apply().unwrap();
        let sdp = edit(offer.to_sdp_string());
        self.say(&ToService::Signal(&format!("offer\n{sdp}")));
        self.say(&ToService::Signal(&format!("candidate\n{line}")));
        self.rtc = Some(rtc);
        self.pending = Some(pending);
    }

    fn step(&mut self) {
        if let Some(room) = self.room.as_mut() {
            room.begin_pass();
        }
        while let Some(text) = self.room.as_mut().and_then(RoomLink::take) {
            match FromService::parse(&text) {
                Some(FromService::Joined { peer, .. }) => self.peer = Some(peer),
                Some(FromService::Refused(why)) => self.refused = Some(why.to_string()),
                Some(FromService::Closed(why)) => self.closed_by_room = Some(why.to_string()),
                Some(FromService::Signal(body)) => {
                    if let Some(sdp) = body.strip_prefix("answer\n")
                        && let (Some(rtc), Some(pending)) = (self.rtc.as_mut(), self.pending.take())
                        && let Ok(answer) = SdpAnswer::from_sdp_string(sdp)
                    {
                        let _ = rtc.sdp_api().accept_answer(pending, answer);
                    }
                }
                _ => {}
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
                Ok(Output::Event(Event::ChannelOpen(id, _))) => self.channel = Some(id),
                Ok(Output::Event(Event::ChannelData(d))) => self.got.push(d.data),
                Ok(Output::Event(
                    Event::ChannelClose(_)
                    | Event::Closed
                    | Event::IceConnectionStateChange(IceConnectionState::Disconnected),
                )) => self.channel_closed = true,
                Ok(Output::Event(_)) => {}
            }
        }
    }

    fn send(&mut self, data: &[u8]) -> bool {
        let (Some(rtc), Some(id)) = (self.rtc.as_mut(), self.channel) else {
            return false;
        };
        let sent = rtc
            .channel(id)
            .is_some_and(|mut c| c.write(true, data).unwrap_or(false));
        self.outputs();
        sent
    }
}

/// Steps the host and `joiners` until `done` or `ms` pass; returns the host's longest pass.
fn run(
    h: &mut P2pHost,
    joiners: &mut [&mut Joiner],
    ms: u64,
    mut done: impl FnMut(&mut P2pHost, &mut [&mut Joiner]) -> bool,
) -> Duration {
    let start = Instant::now();
    let mut longest = Duration::ZERO;
    while start.elapsed() < Duration::from_millis(ms) {
        let t = Instant::now();
        h.service();
        longest = longest.max(t.elapsed());
        for j in joiners.iter_mut() {
            j.step();
        }
        if done(h, joiners) {
            break;
        }
        sleep(Duration::from_millis(1));
    }
    longest
}

/// A joiner in the host's room with its channel open, and its stand-in.
fn connected(h: &mut P2pHost, url: &str, code: &str) -> (Joiner, SocketAddr) {
    let mut j = Joiner::new(url);
    j.say(&ToService::Join(code));
    run(h, &mut [&mut j], 3000, |_, j| j[0].peer.is_some());
    assert!(j.peer.is_some(), "joined: {:?}", j.refused);
    j.offer_with(|s| s);
    run(h, &mut [&mut j], 8000, |_, j| j[0].channel.is_some());
    assert!(j.channel.is_some(), "the channel opens: {:?}", h.counters());
    assert!(j.send(b"hello"));
    let mut from = None;
    run(h, &mut [&mut j], 3000, |h, _| {
        from = recv(h);
        from.is_some()
    });
    let (from, data) = from.expect("the joiner's datagram reached the host");
    assert_eq!(data, b"hello");
    assert!(is_stand_in(from), "{from}");
    (j, from)
}

#[test]
fn a_joiner_reaches_a_players_host_through_a_room() {
    let rooms = service();
    let mut h = host(&rooms.url());
    let code = h.code().expect("the room's code");
    assert_eq!(code.len(), 9, "{code}");
    let (mut j, at) = connected(&mut h, &rooms.url(), &code.to_lowercase());
    // The host's datagram goes on the channel.
    assert!(h.send_to(at, b"welcome"));
    run(&mut h, &mut [&mut j], 3000, |_, j| !j[0].got.is_empty());
    assert_eq!(j.got, [b"welcome".to_vec()]);
    // The joiner's room socket closes once it is in: its channel carries on.
    j.room = None;
    run(&mut h, &mut [&mut j], 500, |_, _| false);
    assert!(h.is_open(at), "the channel outlives the room socket");
    assert!(j.send(b"still here"));
    let mut got = None;
    run(&mut h, &mut [&mut j], 3000, |h, _| {
        got = recv(h);
        got.is_some()
    });
    assert_eq!(got, Some((at, b"still here".to_vec())));
    // The host leaves: the joiner's channel closes.
    h.close();
    let started = Instant::now();
    while !j.channel_closed && started.elapsed() < Duration::from_secs(15) {
        j.step();
        sleep(Duration::from_millis(5));
    }
    assert!(j.channel_closed, "the joiner sees the host leave");
    assert!(rooms.stop() < Duration::from_millis(50));
}

#[test]
fn hostile_joiners_are_turned_away_and_the_host_stays_quick() {
    let rooms = service();
    let mut h = host(&rooms.url());
    let code = h.code().unwrap();
    let (mut good, at) = connected(&mut h, &rooms.url(), &code);
    // A media offer is dropped from the room by the host; an offer that is not SDP never leaves the service, which
    // closes the socket that sent it.
    type Edit = fn(String) -> String;
    let edits: [(Edit, bool); 2] = [
        (|s| s + "m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n", true),
        (|_| "not sdp at all".to_string(), false),
    ];
    for (edit, by_host) in edits {
        let mut bad = Joiner::new(&rooms.url());
        bad.say(&ToService::Join(&code));
        run(&mut h, &mut [&mut bad, &mut good], 3000, |_, j| {
            j[0].peer.is_some()
        });
        bad.offer_with(edit);
        let longest = run(&mut h, &mut [&mut bad, &mut good], 3000, |_, j| {
            j[0].closed_by_room.is_some() || j[0].room.as_ref().is_some_and(|r| !r.is_open())
        });
        if by_host {
            assert_eq!(bad.closed_by_room.as_deref(), Some("dropped"));
        } else {
            assert!(bad.room.as_ref().is_some_and(|r| !r.is_open()));
        }
        assert!(
            longest < Duration::from_millis(20),
            "a pass took {longest:?}"
        );
    }
    // A joiner that names another machine as its candidate (in its offer and trickled) cannot aim the host's checks
    // at it: the loopback address, and every other address the host may not check, is refused.
    let victim = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    victim.set_nonblocking(true).unwrap();
    let port = victim.local_addr().unwrap().port();
    let mut aiming = Joiner::new(&rooms.url());
    aiming.say(&ToService::Join(&code));
    run(&mut h, &mut [&mut aiming, &mut good], 3000, |_, j| {
        j[0].peer.is_some()
    });
    aiming.offer_with(move |s| {
        s + &format!(
            "a=candidate:9 1 udp 2122260223 127.0.0.1 {port} typ host
"
        )
    });
    for target in [
        "127.0.0.1",
        "10.0.0.1",
        "192.168.1.1",
        "224.0.0.1",
        "169.254.169.254",
        "0.0.0.0",
    ] {
        aiming.say(&ToService::Signal(&format!(
            "candidate
candidate:7 1 udp 2122260223 {target} {port} typ host"
        )));
    }
    let refused_before = h.counters().candidates_refused;
    run(&mut h, &mut [&mut aiming, &mut good], 3000, |_, _| false);
    let mut buf = [0u8; 2048];
    assert!(
        victim.recv_from(&mut buf).is_err(),
        "the host sent nothing to the named address"
    );
    assert!(
        h.counters().candidates_refused >= refused_before + 6,
        "{:?}",
        h.counters()
    );
    // Floods of candidates and offers: the room caps the joiner's messages, the host caps what it takes.
    let mut chatty = Joiner::new(&rooms.url());
    chatty.say(&ToService::Join(&code));
    run(&mut h, &mut [&mut chatty, &mut good], 3000, |_, j| {
        j[0].peer.is_some()
    });
    chatty.offer_with(|s| s);
    // A new peer's first pass makes its stack (bounded by the new peers a second); the floods below are timed.
    run(&mut h, &mut [&mut chatty, &mut good], 8000, |_, j| {
        j[0].channel.is_some()
    });
    for n in 0..100u16 {
        let line = format!(
            "candidate:{n} 1 udp 2122260223 127.0.0.1 {} typ host",
            10_000 + n
        );
        chatty.say(&ToService::Signal(&format!("candidate\n{line}")));
        chatty.say(&ToService::Signal(
            "candidate\ncandidate:1 1 udp 1 abcd.local 9 typ host",
        ));
    }
    let longest = run(&mut h, &mut [&mut chatty, &mut good], 2000, |_, j| {
        j[0].closed_by_room.is_some() || j[0].room.as_ref().is_some_and(|r| !r.is_open())
    });
    assert!(
        longest < Duration::from_millis(20),
        "a pass took {longest:?}"
    );
    // The good joiner's channel never noticed.
    assert!(
        h.is_open(at),
        "{:?} {:?} {}",
        h.counters(),
        chatty.closed_by_room,
        good.channel_closed
    );
    assert!(good.send(b"after"));
    let mut got = None;
    run(&mut h, &mut [&mut good], 3000, |h, _| {
        while let Some(d) = recv(h) {
            if d.1 == b"after" {
                got = Some(d);
            }
        }
        got.is_some()
    });
    assert_eq!(got, Some((at, b"after".to_vec())));
    assert!(!good.channel_closed);
    assert!(rooms.stop() < Duration::from_millis(50));
}
