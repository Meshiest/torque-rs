//! A small seeded fuzz of the packet layer, the handshake decoders and the datagram decoder, plus an in-memory session
//! between the test client and the server. The counts are bounded for `just test` (longer runs belong in a dedicated
//! fuzz job); the seed is fixed so a failure repeats.

use std::net::SocketAddr;
use torque_testkit::{Rng, fuzz, mutate};

use torque_net::bits::BitReader;
use torque_net::cap::{NET_MAX_PENDING_CONNECTS, NET_PENDING_PER_ADDR};
use torque_net::client::{Client, ClientEvent, Decoded, decode_datagram};
use torque_net::handshake::{
    self, ConnectFields, Decision, Finished, GAME_CONNECTION_CLASS, Handled, Server, ServerConfig,
    challenge_digest,
};
use torque_net::huffman::Table;
use torque_net::profile::HandshakeProfile;
use torque_net::protocol::{Protocol, RateConfig, Role};

const KEY: [u8; 16] = [7; 16];

fn addr(n: u32) -> SocketAddr {
    SocketAddr::from(([10, (n >> 8) as u8, n as u8, 1], 4000 + (n % 7) as u16))
}

fn fields() -> ConnectFields {
    ConnectFields::standard(b"", vec![b"Name".to_vec(), b"123".to_vec(), b"x".to_vec()])
}

/// Drives a join and returns the sequences' two ends.
fn joined(server: &mut Server, t: &Table, from: SocketAddr, seq: u32) -> Client {
    let mut client = Client::new(seq, fields());
    let challenge = client.challenge_request();
    let Handled::Reply(response) = server.handle_datagram(t, from, &challenge, 0) else {
        panic!("no challenge response")
    };
    let ClientEvent::Send(connect) = client.on_datagram(t, &response, 0) else {
        panic!("no connect request")
    };
    let Handled::Request(request) = server.handle_datagram(t, from, &connect, 0) else {
        panic!("no request")
    };
    let Finished::Accepted { reply, .. } = server.finish_request(t, *request, Decision::Accept, 0)
    else {
        panic!("not accepted")
    };
    assert!(matches!(
        client.on_datagram(t, &reply, 0),
        ClientEvent::Accepted(_)
    ));
    client
}

fn check_protocol(p: &Protocol) {
    assert_eq!(
        p.unacked() as u32,
        p.last_send_seq().wrapping_sub(p.highest_acked()),
        "one notify per unacked packet"
    );
    assert!((p.last_send_seq().wrapping_sub(p.highest_acked()) as i32) >= 0);
}

#[test]
fn hostile_packets_never_panic_the_packet_layer_or_break_its_invariants() {
    let mut rng = Rng::star(0xfeed_face_cafe_beef);
    let mut seeds: Vec<Vec<u8>> = Vec::new();
    // A live exchange gives the valid packets to mutate.
    let mut a = Protocol::new(Role::Server, 5, RateConfig::standard(), 0);
    let mut b = Protocol::new(Role::Client, 5, RateConfig::standard(), 0);
    for i in 0..40u32 {
        let mut w = b.begin_data_packet(i);
        w.write_bits(i, 12);
        let from_b = w.into_bytes();
        seeds.push(from_b.clone());
        let mut r = BitReader::new(&from_b);
        let _ = a.receive(&mut r, i);
        seeds.push(a.ack_packet(i));
        seeds.push(a.ping_packet(i));
        let mut w = a.begin_data_packet(i);
        w.write_bits(i, 12);
        let from_a = w.into_bytes();
        seeds.push(from_a.clone());
        let mut r = BitReader::new(&from_a);
        let _ = b.receive(&mut r, i);
    }
    let mut dropped = 0u32;
    let mut taken = 0u32;
    for round in 0..30_000u32 {
        let mut p = Protocol::new(Role::Server, 5, RateConfig::standard(), 0);
        // Put the state somewhere interesting first: some packets sent, some received.
        let warm = rng.index(25);
        for i in 0..warm {
            let mut w = p.begin_data_packet(i as u32);
            w.write_u8(1);
            if rng.index(2) == 0 {
                let seed = &seeds[rng.index(seeds.len())];
                let mut r = BitReader::new(seed);
                let _ = p.receive(&mut r, i as u32);
            }
        }
        let seed = &seeds[rng.index(seeds.len())];
        let bytes = mutate::mutant(&mut rng, seed);
        let bits = if rng.index(4) == 0 {
            rng.index(bytes.len() * 8 + 1)
        } else {
            bytes.len() * 8
        };
        let before = (
            p.last_seq_recvd(),
            p.highest_acked(),
            p.last_send_seq(),
            p.unacked(),
        );
        let mut r = BitReader::with_bit_len(&bytes, bits);
        match p.receive(&mut r, 1_000 + round) {
            Ok(got) if got.dropped.is_some() => {
                dropped += 1;
                let after = (
                    p.last_seq_recvd(),
                    p.highest_acked(),
                    p.last_send_seq(),
                    p.unacked(),
                );
                assert_eq!(
                    before, after,
                    "round {round}: a dropped packet changed state"
                );
            }
            Ok(_) => taken += 1,
            Err(_) => {}
        }
        check_protocol(&p);
        // The connection still builds and takes packets.
        let mut w = p.begin_data_packet(2_000 + round);
        w.write_u8(2);
        check_protocol(&p);
    }
    assert!(
        dropped > 1000 && taken > 1000,
        "dropped {dropped}, taken {taken}"
    );
}

#[test]
fn every_truncation_of_a_valid_game_packet_is_safe_at_every_bit_length() {
    let mut a = Protocol::new(Role::Server, 5, RateConfig::standard(), 0);
    let mut b = Protocol::new(Role::Client, 5, RateConfig::standard(), 0);
    for i in 0..5 {
        let mut w = b.begin_data_packet(i);
        w.write_bits(0x2a5, 11);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        assert!(a.receive(&mut r, i).is_ok());
    }
    let mut w = a.begin_data_packet(9);
    w.write_bits(0x2a5, 11);
    let packet = w.into_bytes();
    for bits in 0..=packet.len() * 8 {
        let mut copy = b.clone();
        let mut r = BitReader::with_bit_len(&packet, bits);
        let _ = copy.receive(&mut r, 10);
        check_protocol(&copy);
        let _ = decode_datagram(
            &Table::torque(),
            &HandshakeProfile::default(),
            &packet[..bits.div_ceil(8)],
        );
    }
}

#[test]
fn hostile_datagrams_never_panic_the_handshake_and_never_create_state_without_the_digest() {
    let t = Table::torque();
    let mut rng = Rng::star(0x0123_4567_89ab_cdef);
    let mut config = ServerConfig::new(8);
    config.connect_requests_per_src_per_s = 10_000;
    let mut server = Server::new(config, KEY);
    let real = addr(1);
    let seq = 0x4000_0001;
    let digest = challenge_digest(&KEY, real, seq);
    let request = handshake::connect_request(
        &HandshakeProfile::default(),
        &t,
        seq,
        digest,
        GAME_CONNECTION_CLASS,
        &fields(),
    );
    let seeds = [
        handshake::HandshakeTypes::TORQUE.challenge_request(seq),
        request.clone(),
        handshake::HandshakeTypes::TORQUE.disconnect(&t, seq, b"bye"),
        handshake::HandshakeTypes::TORQUE.connect_reject(&t, seq, b"CHR_GAME"),
        handshake::connect_accept(&HandshakeProfile::default(), seq, 12, 18, false),
        handshake::HandshakeTypes::TORQUE.challenge_response(seq, digest),
    ];
    let mut requests = 0u32;
    let mut game = 0u32;
    for round in 0..30_000u32 {
        // Spoofed sources too: the digest is for `real` only.
        let from = if rng.index(2) == 0 {
            real
        } else {
            addr(rng.index(400) as u32)
        };
        let seed = &seeds[rng.index(seeds.len())];
        let bytes = if rng.index(8) == 0 {
            (0..rng.index(1600)).map(|_| rng.byte()).collect()
        } else {
            mutate::mutant(&mut rng, seed)
        };
        let now = round / 3;
        match server.handle_datagram(&t, from, &bytes, now) {
            Handled::Request(req) => {
                requests += 1;
                // A request is only ever produced for the digest of its own source and sequence.
                let expect = challenge_digest(&KEY, from, req.seq);
                let mut r = BitReader::new(&bytes);
                r.read_u8().expect("type");
                assert_eq!(r.read_u32(), Ok(req.seq));
                for word in expect {
                    assert_eq!(
                        r.read_u32(),
                        Ok(word),
                        "round {round}: no digest, no request"
                    );
                }
                // The host accepts about half of them.
                if rng.index(2) == 0 {
                    let _ = server.finish_request(&t, *req, Decision::Accept, now);
                }
            }
            Handled::Game { .. } => game += 1,
            _ => {}
        }
        assert!(server.len() <= server.config().max_connections());
        assert!(server.pending_len() <= NET_MAX_PENDING_CONNECTS);
        let _ = server.poll_timeouts(now);
        // The datagram decoder sees the same bytes.
        let _ = decode_datagram(&t, &HandshakeProfile::default(), &bytes);
    }
    assert!(
        requests > 50,
        "{requests} requests got through the mutation"
    );
    let _ = game;
}

/// A flood of requests 110 ms apart (within the digest's life), every one from its own network: once the table is full each is accepted by evicting a
/// pending connection past the evict age, so the pending table stays at its cap, the newest requests hold it and a flood
/// never locks a new client out.
#[test]
fn the_pending_caps_hold_under_a_flood_of_valid_requests_from_many_sources() {
    let t = Table::torque();
    let mut server = Server::new(ServerConfig::new(500), KEY);
    let mut accepted = 0;
    for i in 0..2000u32 {
        let from = addr(i);
        let seq = 100 + i;
        let digest = challenge_digest(&KEY, from, seq);
        let bytes = handshake::connect_request(
            &HandshakeProfile::default(),
            &t,
            seq,
            digest,
            GAME_CONNECTION_CLASS,
            &fields(),
        );
        let now = i * 110;
        if let Handled::Request(req) = server.handle_datagram(&t, from, &bytes, now)
            && let Finished::Accepted { .. } =
                server.finish_request(&t, *req, Decision::Accept, now)
        {
            accepted += 1;
        }
        assert!(server.pending_len() <= NET_MAX_PENDING_CONNECTS);
    }
    assert_eq!(accepted, 2000);
    assert_eq!(server.pending_len(), NET_MAX_PENDING_CONNECTS);
    let newest = addr(1999);
    assert!(
        server.lookup(newest).is_some(),
        "the newest request holds a slot"
    );
    // None of the sources holds more than the per-address cap.
    let mut per_ip = std::collections::HashMap::new();
    for id in server.ids() {
        if let Some(c) = server.connection(id) {
            *per_ip.entry(c.addr.ip()).or_insert(0usize) += 1;
        }
    }
    assert!(per_ip.values().all(|&n| n <= NET_PENDING_PER_ADDR));
}

#[test]
fn an_in_memory_session_runs_for_a_minute_of_virtual_time_without_a_drop() {
    let t = Table::torque();
    let mut server = Server::new(ServerConfig::new(8), KEY);
    let from = addr(3);
    let mut client = joined(&mut server, &t, from, 0x1234);
    let conn = server.lookup(from).expect("a connection");
    let mut delivered_to_server = 0u32;
    let mut delivered_to_client = 0u32;
    let mut notified = 0usize;
    for now in (0..60_000u32).step_by(8) {
        // The client sends when its gate allows (TGE's 32 a second); the server checks on its 32 ms tick (TGE's 102 ms rate).
        if let Some(mut w) = client.begin_data_packet(now, false) {
            w.write_bits(now, 24);
            let bytes = w.into_bytes();
            match server.handle_datagram(&t, from, &bytes, now) {
                Handled::Game { received, .. } => {
                    assert!(received.payload);
                    delivered_to_server += 1;
                }
                other => panic!("at {now}: {other:?}"),
            }
        }
        if now % 32 == 0
            && let Some(c) = server.connection_mut(conn)
            && c.protocol.may_send(now, false)
        {
            let mut w = c.protocol.begin_data_packet(now);
            w.write_bits(now, 24);
            let bytes = w.into_bytes();
            match client.on_datagram(&t, &bytes, now) {
                ClientEvent::Game { received, .. } => {
                    assert!(received.payload);
                    notified += received.notifies.len();
                    delivered_to_client += 1;
                }
                other => panic!("at {now}: {other:?}"),
            }
        }
        assert!(server.poll_timeouts(now).is_empty());
    }
    assert!(delivered_to_server > 1_800, "{delivered_to_server}");
    assert!(delivered_to_client > 500, "{delivered_to_client}");
    assert!(notified > 300);
    let server_side = server.connection(conn).expect("still connected");
    assert_eq!(server_side.protocol.cur_rate().delay, 102);
    assert_eq!(server_side.protocol.cur_rate().size, 200);
    assert!(server_side.protocol.unacked() <= 30);
    assert!(matches!(
        decode_datagram(
            &t,
            &HandshakeProfile::default(),
            &handshake::HandshakeTypes::TORQUE.challenge_request(1)
        ),
        Ok(Decoded::ChallengeRequest { seq: 1 })
    ));
}

/// Hostile bytes where a client's upstream section goes, read by several connections of one server under small caps: random
/// bytes, mutants of sections an honest client wrote, and chunks with crafted fields (far offsets, huge totals, lengths past the
/// message). Nothing panics, no connection holds more than its cap, all of them together never hold more than the server's, the
/// shared count is exactly what the connections hold, and a connection that is refused is dropped (its bytes go back).
#[test]
fn upstream_section_fuzz_never_panics_and_the_caps_hold() {
    use torque_net::bits::{BitReader, BitWriter, bit_length};
    use torque_net::chunk::upstream::{UpstreamCaps, UpstreamReader, UpstreamServer};
    use torque_net::chunk::{CHUNK_LEN_BITS, ChannelLimits, ChunkSender, Kind, id_bits};

    const CONNS: usize = 6;
    let limits = [
        ChannelLimits::new(300).with_max_message(1 << 20),
        ChannelLimits::new(100).with_max_message(40_000),
        ChannelLimits::new(100).with_max_message(2_000),
    ];
    let caps = UpstreamCaps {
        hold_per_connection: 128 << 10,
        hold_total: 600 << 10,
        max_message: 512 << 10,
        max_partial: Some(5),
        message_age_ms: 10_000,
        message_min_rate: 4_096,
        bytes_per_second: 200_000,
        byte_burst: 100_000,
        messages_per_second: 200,
        message_burst: 50,
    };
    let server = UpstreamServer::new(caps);
    let mut rng = Rng::star(0x5eed_u64);
    let mut readers: Vec<UpstreamReader> = (0..CONNS).map(|_| server.connection(&limits)).collect();
    // An honest client of each connection, which sends a packet now and then among the hostile ones.
    let mut senders: Vec<ChunkSender> = (0..CONNS).map(|_| ChunkSender::new(&limits)).collect();
    let (mut ok, mut refused, mut delivered) = (0usize, 0usize, 0usize);
    let mut now = 0u32;

    // Sections an honest client wrote, to mutate.
    let mut honest: Vec<Vec<u8>> = Vec::new();
    let mut tx = ChunkSender::new(&limits);
    for c in 0..3 {
        for _ in 0..3 {
            let n = 1 + rng.index([5_000, 5_000, 1_900][c]);
            let data: Vec<u8> = (0..n).map(|_| rng.byte()).collect();
            tx.enqueue(c, Kind::Delta, data).unwrap();
        }
    }
    for seq in 1..=120u32 {
        let mut w = BitWriter::new();
        tx.write(&mut w, seq, (50 + rng.index(1_000)) * 8);
        honest.push(w.into_bytes());
        tx.on_notify(seq, rng.index(100) < 70);
    }

    let rounds = fuzz::rounds("TORQUE_FUZZ_UPSTREAM", 20_000) as usize;
    for round in 0..rounds {
        now = now.wrapping_add(rng.index(40) as u32);
        let at = rng.index(CONNS);
        let bytes = match rng.index(5) {
            0 => (0..rng.index(300)).map(|_| rng.byte()).collect(),
            1 | 2 => {
                let pick = rng.index(honest.len());
                mutate::mutant(&mut rng, &honest[pick])
            }
            3 => {
                let tx = &mut senders[at];
                if tx.is_idle() {
                    let channel = 1 + rng.index(2);
                    let n = 1 + rng.index(if channel == 1 { 30_000 } else { 1_500 });
                    let data: Vec<u8> = (0..n).map(|_| rng.byte()).collect();
                    tx.enqueue(channel, Kind::Delta, data).unwrap();
                }
                let mut w = BitWriter::new();
                tx.write(&mut w, round as u32 + 1, (100 + rng.index(900)) * 8);
                tx.on_notify(round as u32 + 1, true);
                w.into_bytes()
            }
            _ => {
                // Chunks with crafted fields: every one is well formed to the reader and then judged by its caps.
                let channel = rng.index(3);
                let l = limits[channel];
                let size_bits = bit_length(l.max_message as u32);
                let mut w = BitWriter::new();
                w.write_flag(true);
                for k in 0..1 + rng.index(6) {
                    let total = match rng.index(4) {
                        0 => 1 + rng.index(l.max_message),
                        1 => l.max_message,
                        2 => 1 + rng.index(2_000),
                        _ => rng.next_u64() as usize & ((1 << size_bits) - 1),
                    };
                    let offset = if rng.index(2) == 0 {
                        rng.index(total.max(1))
                    } else {
                        rng.next_u64() as usize & ((1 << size_bits) - 1)
                    };
                    let len = 1 + rng.index(40);
                    w.write_bits(channel as u32, id_bits(3));
                    w.write_flag(rng.index(2) == 0);
                    w.write_bits(rng.index(7) as u32, 16);
                    w.write_bits(total as u32, size_bits);
                    w.write_bits(offset as u32, size_bits);
                    w.write_bits(len as u32, CHUNK_LEN_BITS);
                    for _ in 0..len {
                        w.write_u8(rng.byte());
                    }
                    w.write_flag(k % 7 != 6 && rng.index(3) != 0);
                }
                w.into_bytes()
            }
        };
        let mut out = Vec::new();
        match readers[at].read(&mut BitReader::new(&bytes), now, &mut out) {
            Ok(()) => {
                ok += 1;
                delivered += out.len();
            }
            Err(_) => {
                refused += 1;
                assert!(out.is_empty(), "a refused packet delivers nothing");
                // The host drops a connection that sent a bad packet, and a new one may come.
                readers[at] = server.connection(&limits);
                senders[at] = ChunkSender::new(&limits);
            }
        }
        if rng.index(7) == 0 {
            // The host's tick: a connection that was evicted or sat on a stale message is dropped.
            let at = rng.index(CONNS);
            if readers[at].sweep(now).is_err() {
                readers[at] = server.connection(&limits);
                senders[at] = ChunkSender::new(&limits);
            }
        }
        if rng.index(500) == 0 {
            let at = rng.index(CONNS);
            readers[at] = server.connection(&limits);
            senders[at] = ChunkSender::new(&limits);
        }
        let held: usize = readers.iter().map(UpstreamReader::held_bytes).sum();
        assert!(
            readers
                .iter()
                .all(|r| r.held_bytes() <= caps.hold_per_connection),
            "round {round}: a connection holds more than its cap"
        );
        assert!(
            held <= caps.hold_total,
            "round {round}: {held} bytes held in all"
        );
        assert_eq!(
            held,
            server.held_total(),
            "round {round}: the shared count is what the connections hold"
        );
    }
    eprintln!(
        "upstream fuzz: {rounds} packets, {ok} read, {refused} refused, {delivered} messages delivered"
    );
    drop(readers);
    assert_eq!(server.held_total(), 0);
}
