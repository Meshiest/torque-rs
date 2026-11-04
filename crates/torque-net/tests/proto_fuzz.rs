//! A small seeded fuzz of the packet layer, the handshake decoders and the datagram decoder, plus an in-memory session
//! between the test client and the server. The counts are bounded for `just test` (longer runs belong in a dedicated
//! fuzz job); the seed is fixed so a failure repeats.

use std::net::SocketAddr;

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

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        ((self.next() >> 32) as usize) % n.max(1)
    }
    fn byte(&mut self) -> u8 {
        (self.next() >> 40) as u8
    }
}

const KEY: [u8; 16] = [7; 16];

fn table() -> Table {
    let mut f = [0u32; 256];
    for (i, slot) in f.iter_mut().enumerate().take(127) {
        *slot = match i as u8 {
            b'a'..=b'z' => 300 + (i as u32 * 37) % 900,
            b' ' => 2800,
            b'A'..=b'Z' => 20 + (i as u32 * 13) % 90,
            _ => (i as u32 * 3) % 5,
        };
    }
    Table::from_frequencies(&f).expect("a table")
}

fn addr(n: u32) -> SocketAddr {
    SocketAddr::from(([10, (n >> 8) as u8, n as u8, 1], 4000 + (n % 7) as u16))
}

/// A mutant of `original`: bits flipped, bytes replaced, truncated, extended or spliced, or pure noise.
fn mutate(rng: &mut Rng, original: &[u8]) -> Vec<u8> {
    let mut bytes = original.to_vec();
    match rng.below(7) {
        0 => {
            for _ in 0..1 + rng.below(4) {
                if !bytes.is_empty() {
                    let at = rng.below(bytes.len());
                    bytes[at] ^= 1 << rng.below(8);
                }
            }
        }
        1 => {
            for _ in 0..1 + rng.below(4) {
                if !bytes.is_empty() {
                    let at = rng.below(bytes.len());
                    bytes[at] = rng.byte();
                }
            }
        }
        2 => bytes.truncate(rng.below(bytes.len() + 1)),
        3 => {
            for _ in 0..rng.below(40) {
                bytes.push(rng.byte());
            }
        }
        4 => {
            bytes = (0..rng.below(120)).map(|_| rng.byte()).collect();
        }
        5 => {
            if bytes.len() > 4 {
                let at = rng.below(bytes.len() - 1);
                bytes.remove(at);
            }
        }
        _ => {
            if !bytes.is_empty() {
                let at = rng.below(bytes.len());
                bytes[at] = 0xff;
                if at + 1 < bytes.len() {
                    bytes[at + 1] = 0xff;
                }
            }
        }
    }
    bytes
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
    let mut rng = Rng(0xfeed_face_cafe_beef);
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
        let warm = rng.below(25);
        for i in 0..warm {
            let mut w = p.begin_data_packet(i as u32);
            w.write_u8(1);
            if rng.below(2) == 0 {
                let seed = &seeds[rng.below(seeds.len())];
                let mut r = BitReader::new(seed);
                let _ = p.receive(&mut r, i as u32);
            }
        }
        let seed = &seeds[rng.below(seeds.len())];
        let bytes = mutate(&mut rng, seed);
        let bits = if rng.below(4) == 0 {
            rng.below(bytes.len() * 8 + 1)
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
            &table(),
            &HandshakeProfile::default(),
            &packet[..bits.div_ceil(8)],
        );
    }
}

#[test]
fn hostile_datagrams_never_panic_the_handshake_and_never_create_state_without_the_digest() {
    let t = table();
    let mut rng = Rng(0x0123_4567_89ab_cdef);
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
        let from = if rng.below(2) == 0 {
            real
        } else {
            addr(rng.below(400) as u32)
        };
        let seed = &seeds[rng.below(seeds.len())];
        let bytes = if rng.below(8) == 0 {
            (0..rng.below(1600)).map(|_| rng.byte()).collect()
        } else {
            mutate(&mut rng, seed)
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
                if rng.below(2) == 0 {
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

#[test]
fn the_pending_caps_hold_under_a_flood_of_valid_requests_from_many_sources() {
    let t = table();
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
        if let Handled::Request(req) = server.handle_datagram(&t, from, &bytes, 0)
            && let Finished::Accepted { .. } = server.finish_request(&t, *req, Decision::Accept, 0)
        {
            accepted += 1;
        }
        assert!(server.pending_len() <= NET_MAX_PENDING_CONNECTS);
    }
    assert_eq!(accepted, NET_MAX_PENDING_CONNECTS);
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
    let t = table();
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
