//! Engine-mode sessions over test links: the connect flow on validated and plain links, reliability under loss, lane budgets,
//! the datagram size of each tick, amplification and the cookie's binding.

mod engine_common;

use std::collections::BTreeSet;

use engine_common::{GAME, KEY, Pair, Pipe, Rng, TICK_MS, client_addr, connect, kind};
use torque_net::NetError;
use torque_net::engine::gate::{COOKIE_PERIOD_MS, cookie};
use torque_net::engine::{
    self as framing, Body, Datagram, ENGINE_FRAMING, Fragment, Hello, Kind, Versions,
};
use torque_net::engine::{
    CloseReason, Connect, Delivery, Event, Gate, GateConfig, GateOutcome, HELLO_BYTES, Lane,
    LinkFacts, NoStreams, Polled, RejectCode, Session, SessionConfig, State, StreamLane,
};

const QUIC_LIKE: LinkFacts = LinkFacts {
    validated: true,
    secure: true,
    congestion_controlled: true,
    server_identity: None,
};

fn body_of(datagram: &[u8]) -> Body<'_> {
    match framing::parse(datagram).expect("an engine datagram") {
        Datagram::Opening { body, .. } | Datagram::Data { body, .. } => {
            Body::read(body).expect("a body")
        }
        other => panic!("not a packet: {other:?}"),
    }
}

#[test]
fn a_validated_link_gets_no_hello_or_cookie() {
    for facts in [LinkFacts::MEMORY, QUIC_LIKE] {
        let mut p = Pair::new(facts, 3);
        assert!(p.connect(50), "connects over {facts:?}");
        let kinds: Vec<_> =
            p.up.log
                .iter()
                .chain(&p.down.log)
                .filter_map(|(d, _)| kind(d))
                .collect();
        assert!(
            !kinds.contains(&Kind::Hello) && !kinds.contains(&Kind::Cookie),
            "{kinds:?}"
        );
        let first = &p.up.log[0].0;
        assert_eq!(kind(first), Some(Kind::Opening));
        let Ok(Datagram::Opening { cookie: None, .. }) = framing::parse(first) else {
            panic!("the first packet is an opening without a cookie");
        };
        assert!(
            !body_of(first).section(Lane::Control).is_empty(),
            "the connect rides the first packet"
        );
        assert!(first.len() < HELLO_BYTES, "no padding on a validated link");
    }

    // A server on a validated link does not answer a hello.
    let mut server =
        Session::server(SessionConfig::new(GAME), LinkFacts::MEMORY, 0).expect("a server");
    let hello = Hello {
        versions: Versions::CURRENT,
        nonce: 5,
        game: GAME.to_owned(),
    };
    server.receive(&hello.encode().expect("a hello"), 0);
    let mut out = Pipe::new(LinkFacts::MEMORY, 1);
    assert_eq!(server.poll(&mut out, &mut NoStreams, 0), Polled::Idle);
    assert!(out.log.is_empty());
}

#[test]
fn a_plain_link_always_starts_with_hello_and_cookie() {
    for seed in 0..8 {
        let mut p = Pair::new(LinkFacts::PLAIN, seed);
        assert!(p.connect(100), "seed {seed} connects");
        assert_eq!(kind(&p.up.log[0].0), Some(Kind::Hello));
        assert_eq!(p.up.log[0].0.len(), HELLO_BYTES);
        assert_eq!(kind(&p.down.log[0].0), Some(Kind::Cookie));
        // Every opening carries the cookie and is padded.
        for (d, _) in
            p.up.log
                .iter()
                .filter(|(d, _)| kind(d) == Some(Kind::Opening))
        {
            assert!(matches!(
                framing::parse(d),
                Ok(Datagram::Opening {
                    cookie: Some(_),
                    ..
                })
            ));
            assert_eq!(d.len(), HELLO_BYTES);
        }
    }

    // No server session on a plain link except through the gate, and the gate opens none without a cookie.
    assert!(Session::server(SessionConfig::new(GAME), LinkFacts::PLAIN, 0).is_err());
    let mut client = Session::client(SessionConfig::new(GAME), LinkFacts::MEMORY, connect(), 9, 0)
        .expect("a client");
    let mut out = Pipe::new(LinkFacts::MEMORY, 1);
    client.poll(&mut out, &mut NoStreams, 0);
    let mut gate = Gate::new(GateConfig::new(SessionConfig::new(GAME)), KEY);
    let opening = &out.log[0].0;
    assert!(matches!(
        gate.receive(client_addr(), opening, 0),
        GateOutcome::Drop
    ));
    assert_eq!(gate.pending(), 0);
}

#[test]
fn the_server_refuses_another_game_or_version_with_one_line() {
    let cases = [
        (
            Connect {
                game: "other".into(),
                ..connect()
            },
            RejectCode::Game,
            "the server runs fixture",
        ),
        (
            Connect {
                versions: Versions { min: 3, max: 4 },
                ..connect()
            },
            RejectCode::Version,
            "server speaks network version 2, you have 4",
        ),
    ];
    for facts in [LinkFacts::MEMORY, LinkFacts::PLAIN] {
        for (connect, code, text) in cases.clone() {
            let mut p = Pair::with(facts, 1, SessionConfig::new(GAME), connect);
            for _ in 0..40 {
                p.tick();
            }
            let expect = CloseReason::Rejected {
                code,
                text: text.to_owned(),
            };
            assert_eq!(p.client.close_reason(), Some(&expect), "{facts:?}");
            assert!(p.client_events.contains(&Event::Closed(expect)));
        }
    }
}

/// 4 KiB of reliable data in one message over 1,200-byte packets with 5% loss each way arrives once, on both kinds of link.
#[test]
fn a_4k_reliable_message_crosses_lossy_packets_once() {
    let message: Vec<u8> = (0..4_096u32).map(|i| (i * 31 + 7) as u8).collect();
    for facts in [LinkFacts::MEMORY, LinkFacts::PLAIN] {
        for seed in 0..24 {
            let mut p = Pair::new(facts, seed).lossy(5);
            assert!(p.connect(600), "{facts:?} seed {seed} connects");
            p.client
                .send(message.clone(), Delivery::Reliable)
                .expect("queued");
            p.server()
                .send(message.clone(), Delivery::Reliable)
                .expect("queued");
            for _ in 0..600 {
                p.tick();
            }
            for (side, events) in [("server", &p.server_events), ("client", &p.client_events)] {
                let got = Pair::messages(events);
                assert_eq!(
                    got.len(),
                    1,
                    "{facts:?} seed {seed}: the {side} got it once"
                );
                assert_eq!(got[0], &message);
            }
            assert_eq!(p.client.unacked_reliable(), 0, "all acknowledged");
            assert!(
                p.up.log
                    .iter()
                    .chain(&p.down.log)
                    .all(|(d, max)| d.len() <= *max)
            );
        }
    }
}

/// Lanes below one at its budget, or saturating their own, never take room from it: a call queued behind a full channel
/// lane and a backlog of unreliable messages leaves in the very next packet, and a lane at its budget leaves room below it.
#[test]
fn a_lane_at_its_budget_never_delays_a_higher_lane() {
    let mut p = Pair::connected(LinkFacts::MEMORY);
    p.client_streams.lanes = vec![StreamLane::Ghosts, StreamLane::Moves, StreamLane::Channels];
    for _ in 0..12 {
        p.client
            .send(vec![0x55; 900], Delivery::Unreliable)
            .expect("queued");
    }
    for _ in 0..10 {
        p.tick();
    }
    let before = p.up.log.len();
    p.client
        .send(vec![0xaa; 300], Delivery::Reliable)
        .expect("queued");
    p.tick();
    let packet = &p.up.log[before].0;
    let body = body_of(packet);
    let rpc = Fragment::read_all(body.section(Lane::Rpc)).expect("fragments");
    assert_eq!(rpc.len(), 1, "the call is in the next packet");
    assert_eq!(rpc[0].bytes.len(), 300, "whole");

    // The RPC lane at its budget: a backlog of calls takes at most half of each packet the first pass, and the lanes below it
    // still get room in every packet.
    for _ in 0..40 {
        p.client
            .send(vec![0x77; 1_000], Delivery::Reliable)
            .expect("queued");
    }
    let before = p.up.log.len();
    for _ in 0..8 {
        p.tick();
    }
    for (d, _) in &p.up.log[before..] {
        let body = body_of(d);
        let rpc = body.section(Lane::Rpc).len();
        assert!(rpc > 0 && rpc <= 600, "rpc {rpc}");
        assert!(
            !body.section(Lane::Ghosts).is_empty(),
            "ghosts still get room"
        );
    }

    // A lane at its budget does not delay a higher one: the unreliable lane full and the channels full, an RPC still leaves.
    let mut p = Pair::connected(LinkFacts::MEMORY);
    p.client_streams.lanes = vec![StreamLane::Channels];
    for round in 0..20 {
        for _ in 0..3 {
            let _ = p.client.send(vec![0x33; 800], Delivery::Unreliable);
        }
        let before = p.up.log.len();
        p.client
            .send(vec![round; 200], Delivery::Reliable)
            .expect("queued");
        p.tick();
        let body = body_of(&p.up.log[before].0);
        let rpc = Fragment::read_all(body.section(Lane::Rpc)).expect("fragments");
        assert!(rpc.iter().any(|f| f.bytes == [round; 200]), "round {round}");
    }
}

/// No packet is larger than the link's datagram size of its tick, nor than the accept's limit, while the size moves and
/// shrinks mid-message; everything reliable still arrives once.
#[test]
fn a_packet_never_exceeds_the_datagram_size_of_its_tick() {
    for seed in 0..12 {
        let mut p = Pair::new(LinkFacts::MEMORY, seed).lossy(3);
        p.accept.max_packet = 900;
        assert!(p.connect(400));
        p.client_streams.lanes = vec![StreamLane::Moves];
        p.server_streams.lanes = vec![StreamLane::Ghosts, StreamLane::Channels];
        let messages: Vec<Vec<u8>> = (0..5).map(|i| vec![i as u8; 4_000 + i * 700]).collect();
        for m in &messages {
            p.client
                .send(m.clone(), Delivery::Reliable)
                .expect("queued");
            p.server()
                .send(m.clone(), Delivery::Reliable)
                .expect("queued");
        }
        let mut rng = Rng::new(seed);
        for t in 0..900 {
            let max = match t {
                0..=2 => 1_200,
                3..=40 => 150,
                _ => 120 + rng.below(1_200) as usize,
            };
            p.up.max = max;
            p.down.max = max;
            if t % 7 == 0 {
                let _ = p.client.send(vec![0x99; 700], Delivery::Unreliable);
            }
            p.tick();
        }
        for (d, max) in p.up.log.iter().chain(&p.down.log) {
            assert!(
                d.len() <= (*max).min(900),
                "seed {seed}: {} > {max}",
                d.len()
            );
        }
        for (side, events) in [("server", &p.server_events), ("client", &p.client_events)] {
            let reliable: Vec<_> = events
                .iter()
                .filter_map(|e| match e {
                    Event::Message {
                        delivery: Delivery::Reliable,
                        data,
                    } => Some(data),
                    _ => None,
                })
                .collect();
            assert_eq!(
                reliable,
                messages.iter().collect::<Vec<_>>(),
                "seed {seed}: the {side} got each once, in order"
            );
        }
    }
}

/// A link with no room: the packet is not sent, its bytes go back, and the host's stream data for it is forgotten.
#[test]
fn no_room_on_the_link_sends_nothing_and_loses_nothing() {
    let mut p = Pair::connected(LinkFacts::MEMORY);
    p.client_streams.lanes = vec![StreamLane::Moves];
    p.client
        .send(vec![1; 3_000], Delivery::Reliable)
        .expect("queued");
    p.client
        .send(vec![2; 100], Delivery::Unreliable)
        .expect("queued");
    p.up.full = true;
    let sent = p.up.log.len();
    for _ in 0..5 {
        p.tick();
    }
    assert_eq!(p.up.log.len(), sent);
    assert_eq!(p.client_streams.unsent.len(), 5);
    p.up.full = false;
    for _ in 0..20 {
        p.tick();
    }
    let got = Pair::messages(&p.server_events);
    assert_eq!(got, [&vec![2; 100], &vec![1; 3_000]]);
}

/// Before the server knows the client reads at its address, nothing it sends is larger than what it answers, for every
/// datagram kind an unproven source can send, and it answers each datagram at most once.
#[test]
fn no_answer_is_larger_than_its_request() {
    let mut rng = Rng::new(77);
    let mut gate = Gate::new(GateConfig::new(SessionConfig::new(GAME)), KEY);
    let from = client_addr();
    let now = 5_000;
    let good_cookie = cookie(&KEY, from, 42, u64::from(now / COOKIE_PERIOD_MS));

    let mut client = Session::client(
        SessionConfig::new(GAME),
        LinkFacts::PLAIN,
        connect(),
        42,
        now,
    )
    .expect("a client");
    let mut hello_out = Pipe::new(LinkFacts::PLAIN, 1);
    client.poll(&mut hello_out, &mut NoStreams, now);
    let hello = hello_out.log[0].0.clone();

    let mut requests: Vec<(&str, Vec<u8>)> = vec![
        ("hello", hello.clone()),
        ("short hello", hello[..600].to_vec()),
        (
            "hello for another game",
            Hello {
                versions: Versions::CURRENT,
                nonce: 1,
                game: "x".repeat(255),
            }
            .encode()
            .expect("a hello"),
        ),
        (
            "hello of another version",
            Hello {
                versions: Versions { min: 2, max: 9 },
                nonce: 1,
                game: GAME.into(),
            }
            .encode()
            .expect("a hello"),
        ),
        (
            "cookie",
            framing::Cookie {
                version: ENGINE_FRAMING,
                nonce: 42,
                cookie: good_cookie,
            }
            .encode(),
        ),
        (
            "reject",
            framing::Reject::new(42, RejectCode::Full, "full")
                .encode()
                .expect("a reject"),
        ),
        (
            "disconnect",
            framing::Disconnect::new(42, "bye")
                .encode()
                .expect("a disconnect"),
        ),
        ("data", {
            let mut d = Vec::new();
            framing::Prefix::Data.write(&mut d);
            d.extend_from_slice(&[0; 8]);
            d
        }),
        ("garbage", rng.bytes(1_200)),
        ("tge-looking", vec![26, 0, 0, 0, 1]),
    ];
    // Openings: without a cookie, with a wrong one, and with the right one carrying the connect, at sizes down to tiny.
    client.receive(
        &framing::Cookie {
            version: ENGINE_FRAMING,
            nonce: 42,
            cookie: good_cookie,
        }
        .encode(),
        now,
    );
    let mut opening_out = Pipe::new(LinkFacts::PLAIN, 1);
    client.poll(&mut opening_out, &mut NoStreams, now);
    let opening = opening_out.log[0].0.clone();
    let mut bad = opening.clone();
    bad[12] ^= 1;
    requests.push(("opening with a wrong cookie", bad));
    let mut none = opening.clone();
    none[10] = 0;
    requests.push(("opening without a cookie", none));
    requests.push(("opening", opening.clone()));

    for (name, request) in &requests {
        let replies = match gate.receive(from, request, now) {
            GateOutcome::Drop => vec![],
            GateOutcome::Reply(r) => vec![r],
            GateOutcome::Open { mut session, .. } => {
                gate.settle(from);
                let mut out = Pipe::new(LinkFacts::PLAIN, 3);
                for t in 0..20 {
                    session.poll(&mut out, &mut NoStreams, now + t * TICK_MS);
                }
                out.log.into_iter().map(|(d, _)| d).collect()
            }
        };
        assert!(replies.len() <= 1, "{name}: {} answers", replies.len());
        for r in &replies {
            assert!(
                r.len() <= request.len(),
                "{name}: {} answered with {}",
                request.len(),
                r.len()
            );
        }
    }

    // A whole lossy connect on a plain link: until the client's first data packet, the server sends at most one datagram per
    // datagram it got, and none larger.
    for seed in 0..16 {
        let mut p = Pair::new(LinkFacts::PLAIN, seed).lossy(10);
        let (mut got, mut biggest, mut sent) = (0, 0, 0);
        for _ in 0..400 {
            let proven = p
                .server
                .as_ref()
                .is_some_and(|s| s.state() == State::Established);
            if proven {
                break;
            }
            got += p.up.queue.len();
            biggest = biggest.max(p.up.queue.iter().map(Vec::len).max().unwrap_or(0));
            let before = p.down.log.len();
            p.tick();
            for (d, _) in &p.down.log[before..] {
                sent += 1;
                assert!(
                    d.len() <= biggest,
                    "seed {seed}: sent {} having got at most {biggest}",
                    d.len()
                );
            }
            assert!(
                sent <= got,
                "seed {seed}: {sent} datagrams sent for {got} received"
            );
        }
        assert!(p.connect(400), "seed {seed}");
    }
}

#[test]
fn a_cookie_from_another_address_or_epoch_is_refused() {
    let config = || GateConfig::new(SessionConfig::new(GAME));
    let from = client_addr();
    let elsewhere = std::net::SocketAddr::from(([10, 0, 0, 8], 4000));
    let other_port = std::net::SocketAddr::from(([10, 0, 0, 7], 4001));
    let t0 = 1_000_000;

    // An opening that carries the cookie the gate gave `from` at `t0`.
    let mut client = Session::client(
        SessionConfig::new(GAME),
        LinkFacts::PLAIN,
        connect(),
        77,
        t0,
    )
    .expect("a client");
    let mut gate = Gate::new(config(), KEY);
    let mut out = Pipe::new(LinkFacts::PLAIN, 1);
    client.poll(&mut out, &mut NoStreams, t0);
    let GateOutcome::Reply(cookie) = gate.receive(from, &out.log[0].0, t0) else {
        panic!("a cookie");
    };
    client.receive(&cookie, t0);
    client.poll(&mut out, &mut NoStreams, t0);
    let opening = out.log[1].0.clone();

    let opened = |gate: &mut Gate, at, now| {
        matches!(gate.receive(at, &opening, now), GateOutcome::Open { .. })
    };
    assert!(
        !opened(&mut Gate::new(config(), KEY), elsewhere, t0),
        "another address"
    );
    assert!(
        !opened(&mut Gate::new(config(), KEY), other_port, t0),
        "another port"
    );
    assert!(
        !opened(&mut Gate::new(config(), [8; 16]), from, t0),
        "another key"
    );
    let mut later = Gate::new(config(), KEY);
    assert!(
        !opened(&mut later, from, t0 + 2 * COOKIE_PERIOD_MS),
        "two epochs on"
    );
    let mut next = Gate::new(config(), KEY);
    assert!(
        opened(
            &mut next,
            from,
            t0 + COOKIE_PERIOD_MS - (t0 % COOKIE_PERIOD_MS)
        ),
        "the next epoch"
    );
    assert!(opened(&mut gate, from, t0), "the same address and epoch");

    // A cookie for another nonce in an otherwise good opening.
    let mut other_nonce = opening.clone();
    other_nonce[2] ^= 1;
    assert!(!matches!(
        Gate::new(config(), KEY).receive(from, &other_nonce, t0),
        GateOutcome::Open { .. }
    ));
}

#[test]
fn the_gate_bounds_pending_sessions_per_address() {
    let mut gate = Gate::new(GateConfig::new(SessionConfig::new(GAME)), KEY);
    let now = 1_000;
    let mut opened = 0;
    for port in 0..10u16 {
        let from = std::net::SocketAddr::from(([10, 0, 0, 9], 5000 + port));
        let mut client = Session::client(
            SessionConfig::new(GAME),
            LinkFacts::PLAIN,
            connect(),
            u64::from(port),
            now,
        )
        .expect("a client");
        let mut out = Pipe::new(LinkFacts::PLAIN, 1);
        client.poll(&mut out, &mut NoStreams, now);
        let GateOutcome::Reply(c) = gate.receive(from, &out.log[0].0, now) else {
            continue;
        };
        client.receive(&c, now);
        client.poll(&mut out, &mut NoStreams, now);
        if matches!(
            gate.receive(from, &out.log[1].0, now),
            GateOutcome::Open { .. }
        ) {
            opened += 1;
        }
    }
    assert_eq!(opened, 2, "two pending sessions per source address");
    assert_eq!(gate.pending(), 2);
}

#[test]
fn sessions_time_out_and_say_goodbye() {
    let mut p = Pair::connected(LinkFacts::MEMORY);
    p.client.disconnect("leaving");
    p.tick();
    p.tick();
    assert_eq!(p.client.state(), State::Closed);
    assert!(
        p.server_events
            .contains(&Event::Closed(CloseReason::Disconnected("leaving".into())))
    );

    let mut p = Pair::connected(LinkFacts::MEMORY);
    p.up.loss_percent = 100;
    for _ in 0..(16_000 / TICK_MS) {
        p.tick();
    }
    assert!(
        p.server_events
            .contains(&Event::Closed(CloseReason::TimedOut))
    );
}

/// A connected memory pair whose accept allows 64 KiB packets.
fn wide_pair() -> Pair {
    let mut p = Pair::new(LinkFacts::MEMORY, 0);
    p.accept.max_packet = u16::MAX;
    assert!(p.connect(200));
    p.tick();
    p.tick();
    p.up.max = usize::from(u16::MAX);
    p.down.max = usize::from(u16::MAX);
    p
}

/// A packet for a session: `Data` on a validated link, or `TokenData` with `token` on a plain one.
fn packet(token: Option<u64>, seq: u16, sections: [&[u8]; 6]) -> Vec<u8> {
    let mut d = Vec::new();
    match token {
        Some(token) => framing::Prefix::TokenData { token }.write(&mut d),
        None => framing::Prefix::Data.write(&mut d),
    }
    let header = framing::Header {
        seq,
        ack: 0,
        mask: 0,
    };
    Body { header, sections }.write(&mut d, 0).expect("a body");
    d
}

fn unreliable_section(seq: u16, bytes: &[u8]) -> Vec<u8> {
    let mut s = Vec::new();
    framing::Unreliable { seq, bytes }
        .write(&mut s)
        .expect("fits");
    s
}

/// 4,096 packets of nearly 64 KiB of stream data each, never drained: the session holds at most its event bytes and then
/// closes with the cap as its reason.
#[test]
fn undrained_events_stay_under_the_byte_cap() {
    let mut p = wide_pair();
    let cap = framing::LaneLimits::default().event_bytes;
    let ghosts = vec![0x47; 65_000];
    for i in 0..4_096u16 {
        let d = packet(None, 1_000 + i, [&[], &[], &[], &ghosts, &[], &[]]);
        assert!(u16::try_from(d.len()).is_ok());
        p.client.receive(&d, p.now);
        assert!(
            p.client.held_event_bytes() <= cap,
            "{} held",
            p.client.held_event_bytes()
        );
    }
    assert!(matches!(
        p.client.close_reason(),
        Some(CloseReason::Protocol(NetError::Cap {
            what: "session event bytes",
            ..
        }))
    ));
}

/// Datagrams past the packet limit are dropped unread, and an unreliable message past its cap closes the session.
#[test]
fn oversized_datagrams_and_messages_are_refused() {
    // Before an accept the limit is 1,200 bytes, whatever the link takes.
    let mut client = Session::client(SessionConfig::new(GAME), LinkFacts::MEMORY, connect(), 1, 0)
        .expect("a client");
    let mut out = Pipe::new(LinkFacts::MEMORY, 1);
    client.poll(&mut out, &mut NoStreams, 0);
    let mut big = out.log[0].0.clone();
    big.resize(1_201, 0);
    let mut server =
        Session::server(SessionConfig::new(GAME), LinkFacts::MEMORY, 0).expect("a server");
    server.receive(&big, 0);
    assert_eq!(server.next_event(), None, "1,201 bytes are not read");
    server.receive(&out.log[0].0, 0);
    assert!(matches!(
        server.next_event(),
        Some(Event::ConnectRequest(_))
    ));

    // After the accept: past the accept's limit is dropped, within it is read.
    let mut p = Pair::connected(LinkFacts::MEMORY);
    let fine = unreliable_section(500, &[1; 100]);
    let mut d = packet(None, 5_000, [&[], &[], &fine, &[], &[], &[]]);
    d.resize(1_201, 0);
    p.client.receive(&d, p.now);
    assert_eq!(p.client.next_event(), None);
    d.truncate(1_200);
    p.client.receive(&d, p.now);
    assert!(matches!(p.client.next_event(), Some(Event::Message { .. })));

    let huge = unreliable_section(501, &[2; 1_025]);
    p.client.receive(
        &packet(None, 5_001, [&[], &[], &huge, &[], &[], &[]]),
        p.now,
    );
    assert!(matches!(
        p.client.close_reason(),
        Some(CloseReason::Protocol(NetError::Cap {
            what: "unreliable message bytes",
            ..
        }))
    ));
}

/// On a plain link a packet must carry the session's token: one with none, or with another, is ignored.
#[test]
fn plain_link_packets_without_the_token_are_ignored() {
    let mut p = Pair::connected(LinkFacts::PLAIN);
    let session = p.accept.session;
    let last = p.up.log.last().expect("sent").0.clone();
    assert!(
        matches!(framing::parse(&last), Ok(Datagram::Data { token: Some(t), .. }) if t == session),
        "established packets carry the session id"
    );
    let message = unreliable_section(900, &[7; 10]);
    let sections: [&[u8]; 6] = [&[], &[], &message, &[], &[], &[]];
    for (token, seq) in [
        (None, 3_000),
        (Some(0xbad), 3_001),
        (Some(session ^ 1), 3_002),
    ] {
        p.client.receive(&packet(token, seq, sections), p.now);
        let now = p.now;
        p.server().receive(&packet(token, seq, sections), now);
        assert_eq!(p.client.next_event(), None, "{token:?}");
        assert_eq!(p.server().next_event(), None, "{token:?}");
        assert_eq!(p.client.state(), State::Established);
    }
    p.client
        .receive(&packet(Some(session), 3_003, sections), p.now);
    assert!(matches!(p.client.next_event(), Some(Event::Message { .. })));

    // A validated link takes no token: a `TokenData` there is ignored.
    let mut p = Pair::connected(LinkFacts::MEMORY);
    let session = p.accept.session;
    p.client
        .receive(&packet(Some(session), 3_000, sections), p.now);
    assert_eq!(p.client.next_event(), None);
}

/// The opening a client sends after the gate's cookie, from `from` at `now`.
fn opening_through(gate: &mut Gate, from: std::net::SocketAddr, nonce: u64, now: u32) -> Vec<u8> {
    let mut client = Session::client(
        SessionConfig::new(GAME),
        LinkFacts::PLAIN,
        connect(),
        nonce,
        now,
    )
    .expect("a client");
    let mut out = Pipe::new(LinkFacts::PLAIN, 1);
    client.poll(&mut out, &mut NoStreams, now);
    let GateOutcome::Reply(c) = gate.receive(from, &out.log[0].0, now) else {
        panic!("a cookie");
    };
    client.receive(&c, now);
    client.poll(&mut out, &mut NoStreams, now);
    out.log[1].0.clone()
}

/// An opening replayed after its session settled opens nothing.
#[test]
fn a_replayed_opening_opens_no_second_session() {
    let mut gate = Gate::new(GateConfig::new(SessionConfig::new(GAME)), KEY);
    let from = client_addr();
    let t0 = 50_000;
    let opening = opening_through(&mut gate, from, 5, t0);
    assert!(matches!(
        gate.receive(from, &opening, t0),
        GateOutcome::Open { .. }
    ));
    gate.settle(from);
    for later in [0, 1_000, 11_000, COOKIE_PERIOD_MS] {
        assert!(
            matches!(gate.receive(from, &opening, t0 + later), GateOutcome::Drop),
            "a replay {later} ms on"
        );
    }
}

/// The gate's clock never runs back: after the host's clock jumps back, cookies still expire two epochs on.
#[test]
fn a_clock_jump_back_does_not_keep_cookies_alive() {
    let from = client_addr();
    let t0 = 5_000_000;
    let mut gate = Gate::new(GateConfig::new(SessionConfig::new(GAME)), KEY);
    let opening = opening_through(&mut gate, from, 6, t0);
    let back = t0 - 4_000_000;
    assert!(matches!(gate.receive(from, b"x", back), GateOutcome::Drop));
    assert!(
        matches!(
            gate.receive(from, &opening, back + 2 * COOKIE_PERIOD_MS),
            GateOutcome::Drop
        ),
        "a cookie two epochs old is refused"
    );
    // The same cookie one epoch on, with no jump, still opens.
    let mut steady = Gate::new(GateConfig::new(SessionConfig::new(GAME)), KEY);
    let opening = opening_through(&mut steady, from, 6, t0);
    assert!(matches!(
        steady.receive(from, &opening, t0 + COOKIE_PERIOD_MS),
        GateOutcome::Open { .. }
    ));
}

/// A steady stream of 1 KiB unreliable messages takes its budget on average, not a whole message in every packet.
#[test]
fn a_steady_unreliable_stream_leaves_the_stream_lanes_their_budget() {
    let mut p = Pair::connected(LinkFacts::MEMORY);
    p.client_streams.lanes = vec![StreamLane::Ghosts, StreamLane::Moves];
    let before = p.up.log.len();
    for _ in 0..80 {
        let _ = p.client.send(vec![0x31; 1_024], Delivery::Unreliable);
        p.tick();
    }
    let (mut unreliable, mut streams, mut payload) = (0, 0, 0);
    for (d, _) in &p.up.log[before..] {
        let body = body_of(d);
        unreliable += body.section(Lane::Unreliable).len();
        streams += body.section(Lane::Ghosts).len() + body.section(Lane::Moves).len();
        payload += d.len() - 2 - 8;
    }
    assert!(unreliable > 0, "the stream is sent");
    assert!(
        unreliable * 100 <= payload * 25 + 1_100 * 100,
        "unreliable {unreliable} of {payload}"
    );
    assert!(
        streams * 100 >= payload * 70,
        "ghosts and moves {streams} of {payload}"
    );
}

/// A host that never drains: when a notify can no longer be held, the session closes with the cap as its reason.
#[test]
fn a_notify_past_the_event_cap_closes_the_session() {
    let mut p = Pair::connected(LinkFacts::MEMORY);
    p.client_streams.lanes = vec![StreamLane::Moves];
    let mut server = p.server.take().expect("a server");
    for _ in 0..(framing::session::MAX_EVENTS as u32 + 200) {
        while let Some(d) = p.up.queue.pop_front() {
            server.receive(&d, p.now);
        }
        while let Some(d) = p.down.queue.pop_front() {
            p.client.receive(&d, p.now);
        }
        while server.next_event().is_some() {}
        p.client.poll(&mut p.up, &mut p.client_streams, p.now);
        server.poll(&mut p.down, &mut p.server_streams, p.now);
        p.now += TICK_MS;
        if p.client.state() == State::Closed {
            break;
        }
    }
    assert!(matches!(
        p.client.close_reason(),
        Some(CloseReason::Protocol(NetError::Cap {
            what: "session events",
            ..
        }))
    ));
    let mut last = None;
    while let Some(e) = p.client.next_event() {
        last = Some(e);
    }
    assert!(
        matches!(last, Some(Event::Closed(CloseReason::Protocol(_)))),
        "the host hears why"
    );
}

/// Packet sequences and reliable message ids wrap past 65,535 without a loss or a duplicate, under loss.
#[test]
fn sequences_and_message_ids_wrap() {
    let mut p = Pair::new(LinkFacts::PLAIN, 11).lossy(2);
    assert!(p.connect(400));
    let total = 70_000u32;
    let mut got = 0u32;
    for i in 0..total + 600 {
        if i < total {
            p.client
                .send(i.to_le_bytes().to_vec(), Delivery::Reliable)
                .expect("queued");
        }
        p.tick();
        for e in p.server_events.drain(..) {
            if let Event::Message { data, .. } = e {
                assert_eq!(data, got.to_le_bytes(), "in order, once");
                got += 1;
            }
        }
        p.client_events.clear();
        p.up.log.clear();
        p.down.log.clear();
    }
    assert_eq!(got, total);
    assert!(p.established());
}

fn small_gate(per_network: usize, total: usize) -> Gate {
    let mut config = GateConfig::new(SessionConfig::new(GAME));
    config.opened_per_network = per_network;
    // Many hosts of one network ask in one millisecond here: the answers are not what these tests count.
    config.answers.per_network = u32::MAX;
    config.max_opened = total;
    Gate::new(config, KEY)
}

/// Opens one session from `from` through `gate` and settles it; the opening, if it opened.
fn open_one(gate: &mut Gate, from: std::net::SocketAddr, nonce: u64, now: u32) -> Option<Vec<u8>> {
    let opening = opening_through(gate, from, nonce, now);
    let opened = matches!(gate.receive(from, &opening, now), GateOutcome::Open { .. });
    gate.settle(from);
    opened.then_some(opening)
}

fn v4(a: u8, b: u8, c: u8, d: u8) -> std::net::SocketAddr {
    std::net::SocketAddr::from(([a, b, c, d], 4000))
}

/// One /24 opening session after session holds no more than its share of the replay table, and never stops opening.
#[test]
fn one_network_cannot_fill_the_replay_table() {
    let mut gate = small_gate(4, 16);
    let now = 100_000;
    for host in 1..=60u8 {
        assert!(
            open_one(&mut gate, v4(10, 0, 0, host), u64::from(host), now).is_some(),
            "host {host}"
        );
        assert!(gate.remembered() <= 4, "{} remembered", gate.remembered());
    }
    for net in 1..=12u8 {
        assert!(
            open_one(&mut gate, v4(10, net, 0, 1), 7, now).is_some(),
            "network {net}"
        );
    }
    assert_eq!(gate.remembered(), 16);

    // An IPv6 /64 is one network too.
    let mut gate = small_gate(4, 16);
    for host in 1..=20u16 {
        let from = std::net::SocketAddr::from(([0xfd00, 0, 0, 1, 0, 0, 0, host], 4000));
        assert!(open_one(&mut gate, from, u64::from(host), now).is_some());
        assert!(gate.remembered() <= 4);
    }
}

/// Networks enough to fill the table: the oldest entries go, and a fresh client still gets in.
#[test]
fn many_networks_filling_the_replay_table_still_let_a_fresh_client_in() {
    let mut gate = small_gate(4, 16);
    let now = 100_000;
    let mut openings = Vec::new();
    for net in 0..64u8 {
        let from = v4(10, net, 0, 1);
        openings.push((from, open_one(&mut gate, from, 1, now).expect("opens")));
        assert!(gate.remembered() <= 16);
    }
    assert!(
        open_one(&mut gate, v4(192, 168, 1, 2), 999, now).is_some(),
        "a fresh client opens"
    );
    assert_eq!(gate.remembered(), 16);
    // The newest are still remembered, so their replays stay refused.
    for (from, opening) in &openings[openings.len() - 15..] {
        assert!(matches!(
            gate.receive(*from, opening, now),
            GateOutcome::Drop
        ));
    }
}

/// A replay is refused while its entry is remembered; once its own network evicted the entry early, it opens, as the
/// docs say, and is then remembered again.
#[test]
fn a_replay_is_refused_until_its_entry_is_evicted() {
    let mut gate = small_gate(4, 4_096);
    let now = 100_000;
    let first = open_one(&mut gate, v4(10, 0, 0, 1), 1, now).expect("opens");
    for host in 2..=4u8 {
        open_one(&mut gate, v4(10, 0, 0, host), 1, now).expect("opens");
    }
    for later in [0, 5_000, 20_000] {
        assert!(
            matches!(
                gate.receive(v4(10, 0, 0, 1), &first, now + later),
                GateOutcome::Drop
            ),
            "a replay {later} ms on"
        );
    }
    open_one(&mut gate, v4(10, 0, 0, 5), 1, now + 20_000).expect("opens");
    assert!(
        matches!(
            gate.receive(v4(10, 0, 0, 1), &first, now + 20_000),
            GateOutcome::Open { .. }
        ),
        "evicted by its own network: replayable until its cookie expires"
    );
    gate.settle(v4(10, 0, 0, 1));
    assert!(
        matches!(
            gate.receive(v4(10, 0, 0, 1), &first, now + 20_000),
            GateOutcome::Drop
        ),
        "and once it opened again it is remembered again"
    );
}

/// Opens one session from `from` through `gate` and leaves it pending; what the gate evicted for it, if it opened.
fn open_pending(
    gate: &mut Gate,
    from: std::net::SocketAddr,
    nonce: u64,
    now: u32,
) -> Option<Option<std::net::SocketAddr>> {
    let opening = opening_through(gate, from, nonce, now);
    match gate.receive(from, &opening, now) {
        GateOutcome::Open { evicted, .. } => Some(evicted),
        _ => None,
    }
}

/// Networks enough to fill the pending table, none of them finishing: once they are past the evict age the oldest is
/// evicted, and a fresh client still connects all the way (a young network's only slot is never evicted).
#[test]
fn many_networks_filling_the_pending_table_still_let_a_fresh_client_connect() {
    let max = GateConfig::new(SessionConfig::new(GAME)).max_pending;
    let mut p = Pair::new(LinkFacts::PLAIN, 21);
    let now = p.now;
    let gate = p.gate.as_mut().expect("a plain pair has a gate");
    // Networks apart from the fresh client's own, which holds nothing.
    for net in 0..max {
        let from = v4(11, u8::try_from(net).expect("64 networks"), 0, 1);
        assert_eq!(
            open_pending(gate, from, 1, now),
            Some(None),
            "network {net}"
        );
    }
    assert_eq!(gate.pending(), max);
    p.now += torque_net::cap::NET_PENDING_EVICT_AGE_MS;
    assert!(p.connect(200), "the fresh client connects");

    // The one that made room names the oldest, which the gate no longer counts.
    let mut gate = Gate::new(GateConfig::new(SessionConfig::new(GAME)), KEY);
    for net in 0..max {
        let from = v4(10, u8::try_from(net).expect("64 networks"), 0, 1);
        open_pending(&mut gate, from, 1, now + u32::try_from(net).expect("small"));
    }
    assert_eq!(
        open_pending(&mut gate, v4(192, 168, 7, 7), 2, now + 1_000),
        None,
        "every network's only slot is young: refused"
    );
    let late = now + torque_net::cap::NET_PENDING_EVICT_AGE_MS;
    assert_eq!(
        open_pending(&mut gate, v4(192, 168, 7, 7), 3, late),
        Some(Some(v4(10, 0, 0, 1)))
    );
    assert_eq!(gate.pending(), max);
}

/// One /24 holds at most its share of pending sessions; other networks still open.
#[test]
fn one_network_holds_at_most_its_pending_share() {
    let mut gate = Gate::new(GateConfig::new(SessionConfig::new(GAME)), KEY);
    let now = 1_000;
    let opened = (1..=20u8)
        .filter(|&host| open_pending(&mut gate, v4(10, 0, 0, host), 1, now).is_some())
        .count();
    assert_eq!(opened, 4);
    assert_eq!(gate.pending(), 4);
    assert_eq!(
        open_pending(&mut gate, v4(10, 0, 1, 1), 1, now),
        Some(None),
        "another /24 opens"
    );

    let mut gate = Gate::new(GateConfig::new(SessionConfig::new(GAME)), KEY);
    let opened = (1..=20u16)
        .filter(|&host| {
            let from = std::net::SocketAddr::from(([0xfd00, 0, 0, 1, 0, 0, 0, host], 4000));
            open_pending(&mut gate, from, 1, now).is_some()
        })
        .count();
    assert_eq!(opened, 4, "a /64 likewise");
}

/// `a.b.c.d` as a dual-stack socket reports it: `::ffff:a.b.c.d`.
fn mapped(a: u8, b: u8, c: u8, d: u8, port: u16) -> std::net::SocketAddr {
    std::net::SocketAddr::from((std::net::Ipv4Addr::new(a, b, c, d).to_ipv6_mapped(), port))
}

/// An IPv4-mapped address is the IPv4 address it maps: one IP, one /24, one cookie; mapped clients in different /24s do
/// not share caps.
#[test]
fn mapped_ipv4_addresses_count_as_their_ipv4_form() {
    let now = 1_000;
    let plain = |d: u8, port: u16| std::net::SocketAddr::from(([10, 0, 0, d], port));

    // One IP: two pending from it in either form, then the third is refused.
    let mut gate = Gate::new(GateConfig::new(SessionConfig::new(GAME)), KEY);
    assert!(open_pending(&mut gate, plain(1, 4000), 1, now).is_some());
    assert!(open_pending(&mut gate, mapped(10, 0, 0, 1, 4001), 1, now).is_some());
    assert!(
        open_pending(&mut gate, mapped(10, 0, 0, 1, 4002), 1, now).is_none(),
        "the per-IP cap"
    );
    gate.settle(mapped(10, 0, 0, 1, 4000));
    assert_eq!(
        gate.pending(),
        1,
        "settling the mapped form settles the plain one"
    );

    // One /24: four pending across both forms, then the network is full; a mapped client elsewhere still opens.
    let mut gate = Gate::new(GateConfig::new(SessionConfig::new(GAME)), KEY);
    for d in 1..=2u8 {
        assert!(open_pending(&mut gate, plain(d, 4000), 1, now).is_some());
        assert!(open_pending(&mut gate, mapped(10, 0, 0, d + 2, 4000), 1, now).is_some());
    }
    assert!(
        open_pending(&mut gate, mapped(10, 0, 0, 9, 4000), 1, now).is_none(),
        "the /24 is full"
    );
    assert!(
        open_pending(&mut gate, plain(9, 4000), 1, now).is_none(),
        "in either form"
    );
    assert!(
        open_pending(&mut gate, mapped(10, 0, 1, 9, 4000), 1, now).is_some(),
        "another /24"
    );
    assert!(
        open_pending(&mut gate, mapped(10, 0, 2, 9, 4000), 1, now).is_some(),
        "and another"
    );

    // One cookie: one asked for in the mapped form verifies from the plain form, and the replay is caught across forms.
    let mut gate = Gate::new(GateConfig::new(SessionConfig::new(GAME)), KEY);
    let opening = opening_through(&mut gate, mapped(10, 0, 0, 1, 4000), 3, now);
    assert!(matches!(
        gate.receive(plain(1, 4000), &opening, now),
        GateOutcome::Open { .. }
    ));
    gate.settle(plain(1, 4000));
    assert!(matches!(
        gate.receive(mapped(10, 0, 0, 1, 4000), &opening, now),
        GateOutcome::Drop
    ));
}

/// A host that keeps sessions by source, settles the ones that finish and drops every `evicted` one ends up holding
/// exactly as many pending sessions as the gate counts.
#[test]
fn a_host_that_drops_evicted_sessions_matches_the_gate() {
    let config = GateConfig::new(SessionConfig::new(GAME));
    let max = config.max_pending;
    let mut gate = Gate::new(config, KEY);
    let mut host: std::collections::HashMap<std::net::SocketAddr, Session> =
        std::collections::HashMap::new();
    let mut now = 1_000;
    let mut evictions = 0;
    for i in 0..200u32 {
        let [_, _, b, c] = i.to_be_bytes();
        // Mapped and plain sources, all in different /24s.
        let from = if i % 2 == 0 {
            mapped(10, b, c, 1, 4000)
        } else {
            std::net::SocketAddr::from(([10, b, c, 1], 4000))
        };
        let opening = opening_through(&mut gate, from, u64::from(i), now);
        let GateOutcome::Open { session, evicted } = gate.receive(from, &opening, now) else {
            panic!("source {i} opens");
        };
        host.insert(from, *session);
        if let Some(gone) = evicted {
            assert!(
                host.remove(&gone).is_some(),
                "the evicted source is one the host holds"
            );
            evictions += 1;
        }
        if i % 7 == 0 {
            host.remove(&from);
            gate.settle(from);
        }
        assert_eq!(host.len(), gate.pending(), "after source {i}");
        assert!(host.len() <= max);
        // Far enough apart that the oldest is past the evict age when the table fills.
        now += 40;
    }
    assert!(evictions > 0, "the table filled");
}

/// What a run of a pair with ghost data from the server said: the stream packets the client read, and the fates the server was
/// given.
#[derive(Default)]
struct StreamFates {
    read: BTreeSet<u16>,
    arrived: BTreeSet<u16>,
    lost: BTreeSet<u16>,
    /// Stream packets the client read after the trouble was over.
    read_after: usize,
}

/// Runs a connected pair, the server writing ghost data into every packet, for `ticks`: while `stalled(tick)` the client neither
/// reads nor sends (a long frame), while `dropped(tick)` every datagram to it is lost (an outage).
fn ghost_run(
    ticks: u32,
    stalled: impl Fn(u32) -> bool,
    dropped: impl Fn(u32) -> bool,
) -> StreamFates {
    let mut p = Pair::connected(LinkFacts::MEMORY);
    p.server_streams.lanes = vec![StreamLane::Ghosts];
    let mut server = p.server.take().expect("a server");
    let mut f = StreamFates::default();
    let last_trouble = (0..ticks)
        .rev()
        .find(|&t| stalled(t) || dropped(t))
        .unwrap_or(0);
    for tick in 0..ticks {
        while let Some(d) = p.up.queue.pop_front() {
            server.receive(&d, p.now);
        }
        while let Some(e) = server.next_event() {
            if let Event::Notify { seq, delivered } = e {
                let fates = if delivered {
                    &mut f.arrived
                } else {
                    &mut f.lost
                };
                assert!(fates.insert(seq), "one fate per packet: {seq}");
            }
        }
        if dropped(tick) {
            p.down.queue.clear();
        }
        if !stalled(tick) {
            while let Some(d) = p.down.queue.pop_front() {
                p.client.receive(&d, p.now);
            }
            while let Some(e) = p.client.next_event() {
                if let Event::Stream { seq, .. } = e {
                    f.read.insert(seq);
                    if tick > last_trouble {
                        f.read_after += 1;
                    }
                }
            }
            p.client.poll(&mut p.up, &mut p.client_streams, p.now);
        }
        server.poll(&mut p.down, &mut p.server_streams, p.now);
        p.now += TICK_MS;
    }
    assert_eq!(p.client.state(), State::Established);
    assert_eq!(server.state(), State::Established);
    f
}

/// The fates the sender is given are the peer's: every stream packet the peer read is one its sender was told arrived, and
/// none it was told was lost was read.
fn assert_exact(f: &StreamFates, what: &str) {
    let both: Vec<_> = f.read.intersection(&f.lost).collect();
    assert!(both.is_empty(), "{what}: read, yet counted lost: {both:?}");
    let unread: Vec<_> = f.arrived.difference(&f.read).collect();
    assert!(
        unread.is_empty(),
        "{what}: counted arrived, yet never read: {unread:?}"
    );
    assert!(
        f.read_after > 50,
        "{what}: the stream goes on after it ({} read)",
        f.read_after
    );
}

/// A peer that stops reading for a while (a long frame) and then reads every packet that waited: none of them is counted lost,
/// however long it stopped, so the layers above never send again as new what the peer already read.
#[test]
fn a_peer_that_stalls_reads_no_packet_its_sender_counted_lost() {
    for stall in [3u32, 5, 10, 20, 40, 80, 200] {
        let f = ghost_run(stall + 200, |t| (10..10 + stall).contains(&t), |_| false);
        assert_exact(&f, &format!("a stall of {stall} ticks"));
        assert!(
            f.lost.is_empty(),
            "nothing is lost on a lossless link: {:?}",
            f.lost
        );
    }
}

/// An outage of the server's datagrams while the client goes on sending: the packets lost in it are counted lost, the others
/// arrived, and the stream goes on after it (the server waits at the edge of its window and starts again from the client's acks).
#[test]
fn an_outage_is_counted_lost_and_the_stream_goes_on() {
    for outage in [3u32, 10, 40, 200] {
        let f = ghost_run(outage + 200, |_| false, |t| (10..10 + outage).contains(&t));
        assert_exact(&f, &format!("an outage of {outage} ticks"));
        assert!(!f.lost.is_empty(), "the outage lost packets");
    }
}

/// A peer that keeps sending new packets that acknowledge nothing keeps the session from idling out, but the stream window it
/// leaves shut (32 packets with ghost data unresolved) does not stay shut: the session closes as timed out.
#[test]
fn a_peer_that_never_acks_the_stream_cannot_hold_the_window_shut() {
    let mut p = Pair::connected(LinkFacts::MEMORY);
    p.server_streams.lanes = vec![StreamLane::Ghosts];
    let mut server = p.server.take().expect("a server");
    for tick in 0..(25_000 / TICK_MS) {
        let seq = 5_000 + u16::try_from(tick).expect("a short run");
        server.receive(&packet(None, seq, [&[]; 6]), p.now);
        while server.next_event().is_some() {}
        p.down.queue.clear();
        server.poll(&mut p.down, &mut p.server_streams, p.now);
        p.now += TICK_MS;
    }
    assert_eq!(server.close_reason(), Some(&CloseReason::TimedOut));
}

/// A packet that comes three or more behind the newest one read is not read: an ack of the newest makes its sender count it
/// lost. One or two behind is still read.
#[test]
fn a_packet_three_behind_the_newest_is_not_read() {
    let mut p = Pair::connected(LinkFacts::MEMORY);
    let ghosts = [0x47; 8];
    for seq in [3_000u16, 3_004, 3_002, 3_001, 3_000] {
        p.client.receive(
            &packet(None, seq, [&[], &[], &[], &ghosts, &[], &[]]),
            p.now,
        );
    }
    let mut read = Vec::new();
    while let Some(e) = p.client.next_event() {
        if let Event::Stream { seq, .. } = e {
            read.push(seq);
        }
    }
    assert_eq!(read, [3_000, 3_004, 3_002]);
}
