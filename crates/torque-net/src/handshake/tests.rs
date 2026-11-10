use super::*;
use crate::bits::BitWriter;
use crate::cap::{
    NET_CONNS_PER_ADDR, NET_CONNS_PER_NETWORK, NET_LIMITER_ENTRIES, NET_PENDING_EVICT_AGE_MS,
    NET_PENDING_PER_NETWORK,
};
use crate::client::{Client, ClientEvent, ClientState};
use crate::huffman::tests::sample_frequencies;
use crate::profile::{HandshakeExtension, HandshakeForm, HandshakeProfile};
use crate::protocol::PacketType;
use std::net::IpAddr;
use std::sync::Arc;

// TGE's datagram types and the codecs under them.
const TYPES: HandshakeTypes = HandshakeTypes::TORQUE;
const CHALLENGE_REQUEST: u8 = TYPES.challenge_request;
const CHALLENGE_RESPONSE: u8 = TYPES.challenge_response;
const CONNECT_REQUEST: u8 = TYPES.connect_request;
const CONNECT_REJECT: u8 = TYPES.connect_reject;
const CONNECT_ACCEPT: u8 = TYPES.connect_accept;
const DISCONNECT: u8 = TYPES.disconnect;

fn challenge_request(seq: u32) -> Vec<u8> {
    TYPES.challenge_request(seq)
}

fn challenge_response(seq: u32, digest: [u32; 4]) -> Vec<u8> {
    TYPES.challenge_response(seq, digest)
}

fn parse_challenge_response(data: &[u8]) -> Result<(u32, [u32; 4]), NetError> {
    TYPES.parse_challenge_response(data)
}

fn connect_reject(t: &Table, seq: u32, reason: &[u8]) -> Vec<u8> {
    TYPES.connect_reject(t, seq, reason)
}

fn disconnect(t: &Table, seq: u32, reason: &[u8]) -> Vec<u8> {
    TYPES.disconnect(t, seq, reason)
}

/// A connect request as a client of the test profile writes it.
fn connect_request(
    t: &Table,
    seq: u32,
    digest: [u32; 4],
    class: &[u8],
    fields: &ConnectFields,
) -> Vec<u8> {
    super::connect_request(&prof(), t, seq, digest, class, fields)
}

// Torque's reject strings (the default profile's; the test profile keeps them but for the ghost-width one).
const CHR_INVALID: &[u8] = b"CHR_INVALID";
const CHR_GAME: &[u8] = b"CHR_GAME";
const CHR_PROTOCOL_LESS: &[u8] = b"CHR_PROTOCOL_LESS";
const CHR_PROTOCOL_GREATER: &[u8] = b"CHR_PROTOCOL_GREATER";
const CHR_PASSWORD: &[u8] = b"CHR_PASSWORD";
const CR_INVALID_ARGS: &[u8] = b"CR_INVALID_ARGS";
const CR_SERVERFULL: &[u8] = b"CR_SERVERFULL";
const CHR_PROTOCOL: &[u8] = b"CHR_PROTOCOL";
/// The test profile's own reject string for an unusable ghost-id width.
const CR_TEST_GHOST_BITS: &[u8] = b"CR_TEST_GHOST_BITS";

/// The test profile's game string, extension argument and accept word.
const GAME: &[u8] = b"Test Game";
const EXT_ARG: &[u8] = b"test-ext/1";
const WORD: u32 = 0x7465_7374;

/// The test profile's form: 8 raw bytes after the password (`0`s in a fresh request), the ghost-id width written after the
/// accept's protocol and read above protocol 7, widths 10 to 18.
#[derive(Debug)]
struct TestForm;

impl HandshakeForm for TestForm {
    fn write_request(&self, w: &mut BitWriter, extra: &[u8]) {
        w.write_bytes(extra);
    }

    fn read_request(&self, r: &mut BitReader) -> Result<Vec<u8>, NetError> {
        r.read_bytes(8)
    }

    fn default_request(&self) -> Vec<u8> {
        vec![b'0'; 8]
    }

    fn write_accept(&self, w: &mut BitWriter, _protocol: u32, ghost_bits: u8) {
        w.write_u32(u32::from(ghost_bits));
    }

    fn read_accept(&self, r: &mut BitReader, protocol: u32) -> Result<Option<u32>, NetError> {
        if protocol > 7 {
            Ok(Some(r.read_u32()?))
        } else {
            Ok(None)
        }
    }

    fn ghost_bits_range(&self) -> std::ops::RangeInclusive<u8> {
        10..=18
    }
}

/// A game's profile made up for the tests: protocol 9 (7 accepted), the [`TestForm`], and an extension that gives 22 bits and
/// 1,400-byte packets.
fn prof() -> HandshakeProfile {
    let rejects = crate::profile::RejectStrings {
        ghost_bits: CR_TEST_GHOST_BITS.to_vec(),
        ..Default::default()
    };
    HandshakeProfile {
        game: GAME.to_vec(),
        protocol_current: 9,
        protocol_min: 7,
        form: Arc::new(TestForm),
        rejects,
        extension: Some(HandshakeExtension {
            arg: EXT_ARG.to_vec(),
            accept_word: WORD,
            ghost_bits: Some(22),
            packet_bytes: Some(1_400),
            wide_limit: None,
        }),
        ..HandshakeProfile::default()
    }
}

/// A client of the test profile.
fn client(seq: u32) -> Client {
    Client::new(seq, standard_fields()).with_profile(prof())
}

const KEY: [u8; 16] = [
    0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef,
];

fn table() -> Table {
    Table::from_frequencies(&sample_frequencies()).expect("a table")
}

/// Host `host`'s address: each host in a network of its own, so only the tests of the network caps share one.
fn addr(host: u8, port: u16) -> SocketAddr {
    SocketAddr::from(([10, 0, host, 1], port))
}

/// Host `host` of the network 10.9.9.0/24.
fn net_addr(host: u8, port: u16) -> SocketAddr {
    SocketAddr::from(([10, 9, 9, host], port))
}

/// A request from `from` with the right digest, sent.
fn request_from(sv: &mut Server, t: &Table, from: SocketAddr, seq: u32, now: u32) -> Handled {
    let bytes = request_bytes(sv, t, from, seq, GAME_CONNECTION_CLASS, &standard_fields());
    send(sv, t, from, &bytes, now)
}

/// A request from `from`, accepted.
fn admit(
    sv: &mut Server,
    t: &Table,
    from: SocketAddr,
    seq: u32,
    now: u32,
) -> (ConnId, Option<ConnId>) {
    let h = request_from(sv, t, from, seq, now);
    accept(sv, t, h, now)
}

/// Accepts a request the server handed to the host.
fn accept(sv: &mut Server, t: &Table, h: Handled, now: u32) -> (ConnId, Option<ConnId>) {
    match sv.finish_request(t, pending(h), Decision::Accept, now) {
        Finished::Accepted { conn, evicted, .. } => (conn, evicted),
        other => panic!("expected an accept, got {other:?}"),
    }
}

/// A config whose verified-request budget is high, so a test can send many requests from one address; the tests of the
/// default budget use `ServerConfig::new`.
fn cfg(max_players: usize) -> ServerConfig {
    let mut c = ServerConfig::with_profile(max_players, prof());
    c.connect_requests_per_src_per_s = 10_000;
    c
}

fn server() -> Server {
    Server::new(cfg(8), KEY)
}

fn standard_fields() -> ConnectFields {
    ConnectFields::new(
        &prof(),
        b"",
        vec![
            b"Name".to_vec(),
            b"4242".to_vec(),
            b"Pre".to_vec(),
            b"Suf".to_vec(),
            b"nonce".to_vec(),
        ],
    )
}

/// A connect request with the right digest for `from` and `seq`.
fn request_bytes(
    sv: &Server,
    t: &Table,
    from: SocketAddr,
    seq: u32,
    class: &[u8],
    fields: &ConnectFields,
) -> Vec<u8> {
    connect_request(t, seq, challenge_digest(&sv.key, from, seq), class, fields)
}

fn send(sv: &mut Server, t: &Table, from: SocketAddr, bytes: &[u8], now: u32) -> Handled {
    sv.handle_datagram(t, from, bytes, now)
}

fn ignored(h: Handled) -> Ignore {
    match h {
        Handled::Ignored(why) => why,
        other => panic!("expected silence, got {other:?}"),
    }
}

fn reply(h: Handled) -> Vec<u8> {
    match h {
        Handled::Reply(bytes) => bytes,
        other => panic!("expected a reply, got {other:?}"),
    }
}

fn pending(h: Handled) -> PendingRequest {
    match h {
        Handled::Request(r) => *r,
        other => panic!("expected a request, got {other:?}"),
    }
}

/// The reason a reply rejects with.
fn reject_reason(t: &Table, bytes: &[u8], seq: u32) -> Vec<u8> {
    assert_eq!(bytes[0], CONNECT_REJECT);
    let (s, reason) = parse_reason(t, bytes, CONNECT_REJECT).expect("a reject");
    assert_eq!(s, seq);
    reason
}

/// A request with these fields, expected to be refused with a reason.
fn refused_with(fields: &ConnectFields) -> Vec<u8> {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, fields);
    let r = reply(send(&mut sv, &t, from, &bytes, 0));
    assert_eq!(sv.len(), 0);
    reject_reason(&t, &r, 9)
}

fn join(sv: &mut Server, t: &Table, from: SocketAddr, seq: u32, now: u32) -> ConnId {
    let bytes = request_bytes(sv, t, from, seq, GAME_CONNECTION_CLASS, &standard_fields());
    let req = pending(send(sv, t, from, &bytes, now));
    match sv.finish_request(t, req, Decision::Accept, now) {
        Finished::Accepted { conn, .. } => conn,
        other => panic!("expected an accept, got {other:?}"),
    }
}

/// The first game packet a client of connect sequence `seq` sends (a bare ack).
fn first_packet(seq: u32) -> Vec<u8> {
    let mut client = Protocol::new(Role::Client, seq, RateConfig::standard(), 0);
    client.ack_packet(0)
}

// ---- the digest ----------------------------------------------------------------------------------------------------------

#[test]
fn siphash_matches_the_published_vectors() {
    let key: [u8; 16] = std::array::from_fn(|i| i as u8);
    assert_eq!(siphash24(&key, &[]), 0x726f_db47_dd0e_0e31);
    let msg: Vec<u8> = (0..15).collect();
    assert_eq!(siphash24(&key, &msg), 0xa129_ca61_49be_45e5);
    let msg: Vec<u8> = (0..8).collect();
    assert_eq!(siphash24(&key, &msg), 0x93f5_f579_9a93_2462);
}

#[test]
fn the_digest_binds_the_key_the_address_the_port_and_the_sequence() {
    let from = addr(1, 5000);
    let d = challenge_digest(&KEY, from, 77);
    assert_eq!(d, challenge_digest(&KEY, from, 77));
    assert_ne!(d, challenge_digest(&KEY, from, 78));
    assert_ne!(d, challenge_digest(&KEY, addr(1, 5001), 77));
    assert_ne!(d, challenge_digest(&KEY, addr(2, 5000), 77));
    let mut other = KEY;
    other[15] ^= 1;
    assert_ne!(d, challenge_digest(&other, from, 77));
    let v6 = SocketAddr::from(([0u16, 0, 0, 0, 0, 0, 0, 1], 5000));
    assert_ne!(d, challenge_digest(&KEY, v6, 77));
    // The four words are not all equal or zero.
    assert!(d.iter().any(|&w| w != 0) && d[0] != d[2]);
}

#[test]
fn a_challenge_digest_is_refused_once_it_is_two_periods_old() {
    use crate::handshake::{CHALLENGE_PERIOD_MS, challenge_digest_at};
    let t = table();
    let from = addr(1, 5000);
    let period = CHALLENGE_PERIOD_MS;
    // The challenge is answered 1 ms before an epoch ends; the request echoes what the server returned.
    let issued = period - 1;
    let outcome = |request_at: u32| {
        let mut sv = server();
        let response = reply(send(&mut sv, &t, from, &challenge_request(9), issued));
        let (_, digest) = parse_challenge_response(&response).expect("a response");
        let bytes = connect_request(&t, 9, digest, GAME_CONNECTION_CLASS, &standard_fields());
        send(&mut sv, &t, from, &bytes, request_at)
    };
    // An honest, slow handshake (right away, across the epoch change, and in the whole next epoch) goes through.
    for at in [issued, period + 5, 2 * period - 1] {
        assert!(matches!(outcome(at), Handled::Request(_)), "at {at}");
    }
    // Two epochs on, the same digest is refused, and stays refused.
    for at in [2 * period, 2 * period + 1, 50 * period] {
        assert_eq!(ignored(outcome(at)), Ignore::BadDigest, "at {at}");
    }
    // The digest of each epoch is its own.
    let key = KEY;
    assert_ne!(
        challenge_digest_at(&key, from, 9, 0),
        challenge_digest_at(&key, from, 9, 1)
    );
}

/// A server's challenge response for `seq` at clock reading `at`, and the connect request that echoes it.
fn challenged(sv: &mut Server, t: &Table, from: SocketAddr, seq: u32, at: u32) -> Vec<u8> {
    let response = reply(send(sv, t, from, &challenge_request(seq), at));
    let (_, digest) = parse_challenge_response(&response).expect("a response");
    connect_request(t, seq, digest, GAME_CONNECTION_CLASS, &standard_fields())
}

#[test]
fn a_challenge_digest_from_a_wrap_of_the_clock_ago_stays_refused() {
    let (t, from) = (table(), addr(1, 5000));
    // The clock is 32 bits of milliseconds; the server counts its wraps, so an epoch is never reached twice. The challenge is
    // made at `issued`, and the clock then runs through one whole wrap (2^32 ms) in steps under half a wrap, back to `issued`.
    let issued = 5 * CHALLENGE_PERIOD_MS + 7;
    let mut sv = server();
    let bytes = challenged(&mut sv, &t, from, 9, issued);
    let mut now = issued;
    for _ in 0..4 {
        now = now.wrapping_add(1 << 30);
        sv.poll_timeouts(now);
    }
    assert_eq!(now, issued, "exactly one wrap later");
    assert_eq!(
        ignored(send(&mut sv, &t, from, &bytes, now)),
        Ignore::BadDigest
    );
    // The digest is also refused a wrap and a period later, and a wrap less a period (when its epoch's u32 reading would match).
    assert_eq!(
        ignored(send(
            &mut sv,
            &t,
            from,
            &bytes,
            now.wrapping_add(CHALLENGE_PERIOD_MS)
        )),
        Ignore::BadDigest
    );
    // The current and the previous epoch still verify, on a server that has never wrapped.
    for at in [issued, issued + CHALLENGE_PERIOD_MS] {
        let mut sv = server();
        let bytes = challenged(&mut sv, &t, from, 9, issued);
        assert!(
            matches!(send(&mut sv, &t, from, &bytes, at), Handled::Request(_)),
            "at {at}"
        );
    }
}

#[test]
fn a_handshake_that_straddles_the_clocks_wrap_succeeds() {
    let (t, from) = (table(), addr(1, 5000));
    let mut sv = server();
    let bytes = challenged(&mut sv, &t, from, 9, u32::MAX - 100);
    assert!(matches!(
        send(&mut sv, &t, from, &bytes, 50),
        Handled::Request(_)
    ));
}

// ---- codecs ---------------------------------------------------------------------------------------------------------------

#[test]
fn every_handshake_message_round_trips() {
    let t = table();
    let digest = [1, 2, 0xdead_beef, u32::MAX];
    assert_eq!(
        peek_type(&challenge_request(33)),
        Ok((CHALLENGE_REQUEST, 33))
    );
    assert_eq!(
        parse_challenge_response(&challenge_response(33, digest)),
        Ok((33, digest))
    );
    let fields = standard_fields();
    let bytes = connect_request(&t, 33, digest, GAME_CONNECTION_CLASS, &fields);
    let parsed = crate::client::parse_connect_request(&t, &prof(), &bytes).expect("a request");
    assert_eq!((parsed.seq, parsed.digest), (33, digest));
    assert_eq!(parsed.class, GAME_CONNECTION_CLASS);
    assert_eq!(parsed.fields, fields);
    assert_eq!(
        parse_reason(&t, &connect_reject(&t, 5, CHR_GAME), CONNECT_REJECT),
        Ok((5, CHR_GAME.to_vec()))
    );
    assert_eq!(
        parse_reason(&t, &disconnect(&t, 6, b"Kicked"), DISCONNECT),
        Ok((6, b"Kicked".to_vec()))
    );
    let p = prof();
    assert_eq!(
        parse_accept(&p, &connect_accept(&p, 7, 9, 18, false)),
        Ok(Accept {
            seq: 7,
            protocol: 9,
            ghost_bits: Some(18),
            extension: false,
            channels: false,
        })
    );
    assert_eq!(
        parse_accept(&p, &connect_accept(&p, 7, 7, 18, false)),
        Ok(Accept {
            seq: 7,
            protocol: 7,
            ghost_bits: None,
            extension: false,
            channels: false,
        })
    );
    // The extension's accept: the same three words and the word, which a reader without the extension never reaches.
    let ext = connect_accept(&p, 7, 9, 18, true);
    assert_eq!(ext.len(), 1 + 4 + 4 + 4 + 4);
    assert_eq!(ext[..13], connect_accept(&p, 7, 9, 18, false)[..]);
    assert!(parse_accept(&p, &ext).unwrap().extension);
    let without = HandshakeProfile {
        extension: None,
        ..prof()
    };
    assert!(!parse_accept(&without, &ext).unwrap().extension);
    // Channels: the word after the extension's, or in its place, and the reader of a profile without the extension finds it.
    let both = connect_accept_with(&p, 7, 9, 18, true, true);
    assert_eq!(both.len(), 1 + 4 + 4 + 4 + 4 + 4);
    let a = parse_accept(&p, &both).unwrap();
    assert!(a.extension && a.channels);
    let a = parse_accept(&p, &connect_accept_with(&p, 7, 9, 18, false, true)).unwrap();
    assert!(!a.extension && a.channels);
    let a = parse_accept(
        &without,
        &connect_accept_with(&without, 7, 9, 18, false, true),
    )
    .unwrap();
    assert!(!a.extension && a.channels);
    assert!(!parse_accept(&p, &ext).unwrap().channels);
    // Torque's accept with no channels asked is what it always was.
    assert_eq!(connect_accept_with(&p, 7, 9, 18, true, false), ext);
    // The writer always sends the width (13 bytes); the reader takes it only above protocol 7.
    assert_eq!(connect_accept(&p, 7, 7, 18, false).len(), 1 + 4 + 4 + 4);
    // Torque's accept is the protocol alone, whatever is asked.
    let torque = HandshakeProfile::default();
    let a = connect_accept(&torque, 7, 12, 18, true);
    assert_eq!(a.len(), 1 + 4 + 4);
    assert_eq!(
        parse_accept(&torque, &a),
        Ok(Accept {
            seq: 7,
            protocol: 12,
            ghost_bits: None,
            extension: false,
            channels: false,
        })
    );
}

#[test]
fn the_ghost_width_follows_the_limit() {
    let p = prof();
    for (limit, bits) in [
        (0, 10),
        (100, 10),
        (1_024, 10),
        (1_025, 11),
        (65_536, 16),
        (65_537, 17),
        (262_144, 18),
        (262_145, 18),
        (1 << 24, 18),
        (u32::MAX, 18),
    ] {
        assert_eq!(p.ghost_bits(limit), bits, "limit {limit}");
    }
}

// ---- the handshake --------------------------------------------------------------------------------------------------------

#[test]
fn a_client_joins_through_the_full_handshake_and_exchanges_packets() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let mut client = client(0x1234_5679);

    let challenge = client.challenge_request();
    let response = reply(send(&mut sv, &t, from, &challenge, 0));
    assert_eq!(response[0], CHALLENGE_RESPONSE);
    assert_eq!(sv.len(), 0, "no state before the digest returns");
    let connect = match client.on_datagram(&t, &response, 1) {
        ClientEvent::Send(bytes) => bytes,
        other => panic!("{other:?}"),
    };
    assert_eq!(client.state(), &ClientState::Connecting);
    let request = pending(send(&mut sv, &t, from, &connect, 2));
    assert_eq!(request.fields.args.len(), 5);
    assert_eq!(request.fields.args[0], b"Name");
    assert_eq!(request.protocol_version, 9);
    assert_eq!(request.fields.extra, vec![b'0'; 8]);
    assert_eq!(sv.len(), 0, "the script has not decided yet");

    let (conn, accept) = match sv.finish_request(&t, request, Decision::Accept, 3) {
        Finished::Accepted { conn, reply, .. } => (conn, reply),
        other => panic!("{other:?}"),
    };
    assert_eq!(sv.pending_len(), 1);
    match client.on_datagram(&t, &accept, 4) {
        ClientEvent::Accepted(a) => {
            assert_eq!((a.protocol, a.ghost_bits), (9, Some(12)));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(client.state(), &ClientState::Connected);

    // The client's first data packet moves the connection out of pending and carries its rate.
    let mut w = client
        .begin_data_packet(32, false)
        .expect("the gate is open");
    w.write_u8(0x42);
    let packet = w.into_bytes();
    match send(&mut sv, &t, from, &packet, 6) {
        Handled::Game {
            conn: c,
            received,
            payload_bit,
        } => {
            assert_eq!(c, conn);
            assert!(received.payload);
            let mut r = crate::protocol::reader_at(&packet, payload_bit);
            assert_eq!(r.read_u8(), Ok(0x42));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(sv.pending_len(), 0);
    let server_conn = sv.connection_mut(conn).expect("a connection");
    assert_eq!(server_conn.protocol.cur_rate().delay, 102);
    let mut w = server_conn.protocol.begin_data_packet(40);
    w.write_u8(0x77);
    let back = w.into_bytes();
    match client.on_datagram(&t, &back, 41) {
        ClientEvent::Game {
            received,
            payload_bit,
        } => {
            assert!(received.payload);
            assert_eq!(received.notifies.len(), 1);
            assert!(received.notifies[0].delivered);
            let mut r = crate::protocol::reader_at(&back, payload_bit);
            assert_eq!(r.read_u8(), Ok(0x77));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_accept_carries_min_of_the_clients_protocol_and_the_servers() {
    for (current, min, protocol) in [(9, 7, 9), (20, 7, 9), (8, 7, 8), (7, 7, 7), (99, 9, 9)] {
        let (mut sv, t, from) = (server(), table(), addr(1, 5000));
        let mut fields = standard_fields();
        fields.current_protocol = current;
        fields.min_protocol = min;
        let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, &fields);
        let req = pending(send(&mut sv, &t, from, &bytes, 0));
        assert_eq!(req.protocol_version, protocol);
        let Finished::Accepted { reply, .. } = sv.finish_request(&t, req, Decision::Accept, 0)
        else {
            panic!("not accepted")
        };
        let a = parse_accept(&prof(), &reply).expect("an accept");
        assert_eq!(a.protocol, protocol, "current {current}");
        assert_eq!(a.ghost_bits, (protocol > 7).then_some(12));
    }
}

#[test]
fn the_ghost_width_in_the_accept_follows_the_servers_limit() {
    for (limit, bits) in [(262_144, 18), (1_024, 10), (1 << 20, 18), (100_000, 17)] {
        let mut config = cfg(8);
        config.ghost_limit = limit;
        let (mut sv, t, from) = (Server::new(config, KEY), table(), addr(1, 5000));
        let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, &standard_fields());
        let req = pending(send(&mut sv, &t, from, &bytes, 0));
        let Finished::Accepted { reply, .. } = sv.finish_request(&t, req, Decision::Accept, 0)
        else {
            panic!("not accepted")
        };
        assert_eq!(
            parse_accept(&prof(), &reply).expect("an accept").ghost_bits,
            Some(bits)
        );
    }
}

#[test]
fn the_client_refuses_an_accept_it_cannot_use() {
    let t = table();
    let p = prof();
    for (protocol, bits, reason) in [
        (6, 18, CHR_PROTOCOL),
        (10, 18, CHR_PROTOCOL),
        (9, 9, CR_TEST_GHOST_BITS),
        (9, 19, CR_TEST_GHOST_BITS),
        (8, 0, CR_TEST_GHOST_BITS),
    ] {
        let mut client = client(9);
        client.challenge_request();
        client.connect_request(&t, Some([0; 4]));
        match client.on_datagram(&t, &connect_accept(&p, 9, protocol, bits, false), 0) {
            ClientEvent::Rejected(r) => assert_eq!(r, reason, "protocol {protocol} bits {bits}"),
            other => panic!("{other:?}"),
        }
    }
    // Protocol 7 sends no width, so none is checked.
    let mut c = client(9);
    c.connect_request(&t, Some([0; 4]));
    assert!(matches!(
        c.on_datagram(&t, &connect_accept(&p, 9, 7, 0, false), 0),
        ClientEvent::Accepted(_)
    ));
    // An accept for another sequence is ignored.
    let mut c = client(9);
    c.connect_request(&t, Some([0; 4]));
    assert!(matches!(
        c.on_datagram(&t, &connect_accept(&p, 10, 9, 18, false), 0),
        ClientEvent::Nothing
    ));
}

#[test]
fn a_client_and_server_of_the_default_profile_speak_torques_handshake() {
    let (t, from) = (table(), addr(1, 5000));
    let mut sv = Server::new(ServerConfig::new(8), KEY);
    let fields = ConnectFields::standard(b"", vec![b"Name".to_vec()]);
    assert_eq!(fields.game, b"Torque Game Engine Demo");
    assert_eq!((fields.current_protocol, fields.min_protocol), (12, 12));
    assert!(fields.extra.is_empty());
    let mut c = Client::new(5, fields);
    let response = reply(send(&mut sv, &t, from, &c.challenge_request(), 0));
    let ClientEvent::Send(connect) = c.on_datagram(&t, &response, 1) else {
        panic!("no connect request");
    };
    let req = pending(send(&mut sv, &t, from, &connect, 2));
    assert_eq!(req.protocol_version, 12);
    assert!(!req.extension);
    let Finished::Accepted {
        reply: accept,
        conn,
        ..
    } = sv.finish_request(&t, req, Decision::Accept, 3)
    else {
        panic!("not accepted");
    };
    // The accept is the protocol alone; the connection takes the width of the limit, TGE's 12 bits.
    assert_eq!(accept.len(), 1 + 4 + 4);
    assert_eq!(sv.connection(conn).map(|c| c.ghost_bits), Some(12));
    match c.on_datagram(&t, &accept, 4) {
        ClientEvent::Accepted(a) => assert_eq!((a.protocol, a.ghost_bits), (12, None)),
        other => panic!("{other:?}"),
    }
    // A request of another game is refused with Torque's string.
    let mut other = ConnectFields::standard(b"", vec![]);
    other.game = GAME.to_vec();
    let bytes = request_bytes(&sv, &t, addr(2, 5000), 9, GAME_CONNECTION_CLASS, &other);
    let r = reply(send(&mut sv, &t, addr(2, 5000), &bytes, 5));
    assert_eq!(reject_reason(&t, &r, 9), CHR_GAME);
}

// ---- the reject table: one test per row ----------------------------------------------------------------------------------

#[test]
fn reject_the_wrong_net_class_group() {
    let mut f = standard_fields();
    f.group = 1;
    assert_eq!(refused_with(&f), CHR_INVALID);
}

#[test]
fn reject_the_wrong_class_crc() {
    let mut f = standard_fields();
    f.crc = 0xffff_fffe;
    assert_eq!(refused_with(&f), CHR_INVALID);
    f.crc = 0;
    assert_eq!(refused_with(&f), CHR_INVALID);
}

#[test]
fn reject_the_wrong_game_string() {
    let mut f = standard_fields();
    f.game = GAME.to_ascii_lowercase();
    assert_eq!(refused_with(&f), CHR_GAME);
    f.game = Vec::new();
    assert_eq!(refused_with(&f), CHR_GAME);
}

#[test]
fn reject_a_current_protocol_below_the_oldest_accepted() {
    let mut f = standard_fields();
    f.current_protocol = 6;
    assert_eq!(refused_with(&f), CHR_PROTOCOL_LESS);
    f.current_protocol = 0;
    assert_eq!(refused_with(&f), CHR_PROTOCOL_LESS);
}

#[test]
fn reject_a_minimum_protocol_above_the_newest_spoken() {
    let mut f = standard_fields();
    f.min_protocol = 10;
    assert_eq!(refused_with(&f), CHR_PROTOCOL_GREATER);
    f.min_protocol = u32::MAX;
    assert_eq!(refused_with(&f), CHR_PROTOCOL_GREATER);
}

#[test]
fn reject_the_wrong_password() {
    let (mut config, t, from) = (cfg(8), table(), addr(1, 5000));
    config.password = b"hunter2".to_vec();
    let mut sv = Server::new(config, KEY);
    for wrong in [&b""[..], b"hunter", b"Hunter2", b"hunter22"] {
        let f = ConnectFields::new(&prof(), wrong, vec![]);
        let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, &f);
        let r = reply(send(&mut sv, &t, from, &bytes, 0));
        assert_eq!(reject_reason(&t, &r, 9), CHR_PASSWORD, "{wrong:?}");
    }
    let f = ConnectFields::new(&prof(), b"hunter2", vec![]);
    let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, &f);
    pending(send(&mut sv, &t, from, &bytes, 0));
    // With no server password any password passes.
    let (mut open, t, from) = (server(), table(), addr(1, 5000));
    let f = ConnectFields::new(&prof(), b"anything", vec![]);
    let bytes = request_bytes(&open, &t, from, 9, GAME_CONNECTION_CLASS, &f);
    pending(send(&mut open, &t, from, &bytes, 0));
}

#[test]
fn reject_more_than_16_arguments() {
    let mut f = standard_fields();
    f.args = (0..17).map(|i| vec![b'a' + i as u8]).collect();
    assert_eq!(refused_with(&f), CR_INVALID_ARGS);
    f.args = (0..40).map(|_| b"x".to_vec()).collect();
    assert_eq!(refused_with(&f), CR_INVALID_ARGS);
    // Exactly 16 pass, and 0.
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    for n in [0usize, 16] {
        let mut f = standard_fields();
        f.args = (0..n).map(|_| b"arg".to_vec()).collect();
        let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, &f);
        let req = pending(send(&mut sv, &t, from, &bytes, 0));
        assert_eq!(req.fields.args.len(), n);
    }
}

#[test]
fn an_argument_count_of_billions_is_refused_without_reading_them() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    // A request written by hand with a lying argument count.
    let mut w = BitWriter::new();
    w.write_u8(CONNECT_REQUEST);
    w.write_u32(9);
    for word in challenge_digest(&KEY, from, 9) {
        w.write_u32(word);
    }
    t.write(&mut w, GAME_CONNECTION_CLASS, 255);
    w.write_u32(CLASS_GROUP);
    w.write_u32(CLASS_CRC);
    t.write(&mut w, GAME, 255);
    w.write_u32(9);
    w.write_u32(7);
    t.write(&mut w, b"", 255);
    w.write_bytes(&[0; 8]);
    w.write_u32(0xffff_ffff);
    let bytes = w.into_bytes();
    let r = reply(send(&mut sv, &t, from, &bytes, 0));
    assert_eq!(reject_reason(&t, &r, 9), CR_INVALID_ARGS);
}

#[test]
fn the_reject_strings_are_checked_in_the_readings_order() {
    let mut f = standard_fields();
    f.group = 5;
    f.game = b"x".to_vec();
    f.current_protocol = 1;
    f.min_protocol = 99;
    f.password = b"x".to_vec();
    f.args = (0..30).map(|_| b"x".to_vec()).collect();
    let mut config = cfg(8);
    config.password = b"pw".to_vec();
    let (t, from) = (table(), addr(1, 5000));
    let expect = |f: &ConnectFields, want: &[u8]| {
        let mut sv = Server::new(config.clone(), KEY);
        let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, f);
        let r = reply(send(&mut sv, &t, from, &bytes, 0));
        assert_eq!(reject_reason(&t, &r, 9), want);
    };
    expect(&f, CHR_INVALID);
    f.group = 0;
    expect(&f, CHR_GAME);
    f.game = GAME.to_vec();
    expect(&f, CHR_PROTOCOL_LESS);
    f.current_protocol = 9;
    expect(&f, CHR_PROTOCOL_GREATER);
    f.min_protocol = 7;
    expect(&f, CHR_PASSWORD);
    f.password = b"pw".to_vec();
    expect(&f, CR_INVALID_ARGS);
}

#[test]
fn the_scripts_reject_string_is_sent_cut_at_255_bytes() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, &standard_fields());
    let req = pending(send(&mut sv, &t, from, &bytes, 0));
    let long = vec![b'z'; 400];
    let Finished::Rejected { reply } = sv.finish_request(&t, req, Decision::Reject(long), 0) else {
        panic!("not rejected")
    };
    assert_eq!(reject_reason(&t, &reply, 9), vec![b'z'; 255]);
    assert_eq!(sv.len(), 0);

    let req = pending(send(&mut sv, &t, from, &bytes, 1));
    let Finished::Rejected { reply } = sv.finish_request(
        &t,
        req,
        Decision::Reject(b"CR_BANNED soon\0hidden".to_vec()),
        1,
    ) else {
        panic!("not rejected")
    };
    assert_eq!(reject_reason(&t, &reply, 9), b"CR_BANNED soon");
}

// ---- silence ---------------------------------------------------------------------------------------------------------------

#[test]
fn silence_a_bad_digest() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let mut bad = challenge_digest(&KEY, from, 9);
    bad[3] ^= 1;
    let bytes = connect_request(&t, 9, bad, GAME_CONNECTION_CLASS, &standard_fields());
    assert_eq!(
        ignored(send(&mut sv, &t, from, &bytes, 0)),
        Ignore::BadDigest
    );
    let zero = connect_request(&t, 9, [0; 4], GAME_CONNECTION_CLASS, &standard_fields());
    assert_eq!(
        ignored(send(&mut sv, &t, from, &zero, 0)),
        Ignore::BadDigest
    );
    assert_eq!(sv.len(), 0);
}

#[test]
fn silence_a_stale_digest() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    // A digest for another sequence, another port, another address, or from before a rekey.
    let cases = [
        (challenge_digest(&KEY, from, 8), from),
        (challenge_digest(&KEY, addr(1, 5001), 9), from),
        (challenge_digest(&KEY, addr(2, 5000), 9), from),
    ];
    for (digest, source) in cases {
        let bytes = connect_request(&t, 9, digest, GAME_CONNECTION_CLASS, &standard_fields());
        assert_eq!(
            ignored(send(&mut sv, &t, source, &bytes, 0)),
            Ignore::BadDigest
        );
    }
    let old = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, &standard_fields());
    sv.set_key([9; 16]);
    assert_eq!(ignored(send(&mut sv, &t, from, &old, 0)), Ignore::BadDigest);
}

#[test]
fn silence_an_unknown_class_name() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    for class in [
        &b"NetConnection"[..],
        b"gameconnection",
        b"",
        b"GameConnection2",
        b"SimObject",
    ] {
        let bytes = request_bytes(&sv, &t, from, 9, class, &standard_fields());
        assert_eq!(
            ignored(send(&mut sv, &t, from, &bytes, 0)),
            Ignore::UnknownClass
        );
    }
}

#[test]
fn silence_while_connections_are_off() {
    let mut config = cfg(8);
    config.allow_connections = false;
    let (mut sv, t, from) = (Server::new(config, KEY), table(), addr(1, 5000));
    assert_eq!(
        ignored(send(&mut sv, &t, from, &challenge_request(9), 0)),
        Ignore::Refused
    );
    let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, &standard_fields());
    assert_eq!(ignored(send(&mut sv, &t, from, &bytes, 0)), Ignore::Refused);
    // Turned back on, the same request works.
    sv.config_mut().allow_connections = true;
    pending(send(&mut sv, &t, from, &bytes, 0));
}

#[test]
fn silence_a_request_with_a_smaller_sequence_than_the_connection() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let conn = join(&mut sv, &t, from, 50, 0);
    let bytes = request_bytes(&sv, &t, from, 49, GAME_CONNECTION_CLASS, &standard_fields());
    assert_eq!(
        ignored(send(&mut sv, &t, from, &bytes, 1)),
        Ignore::StaleSequence
    );
    assert_eq!(sv.lookup(from), Some(conn), "the connection stays");
}

#[test]
fn silence_a_truncated_request_at_every_length() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, &standard_fields());
    for len in 0..bytes.len() {
        match send(&mut sv, &t, from, &bytes[..len], 0) {
            Handled::Ignored(_) => {}
            other => panic!("{len} bytes: {other:?}"),
        }
    }
    assert_eq!(sv.len(), 0);
    pending(send(&mut sv, &t, from, &bytes, 0));
}

#[test]
fn silence_for_unknown_types_oversize_and_empty_datagrams() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    assert_eq!(ignored(send(&mut sv, &t, from, &[], 0)), Ignore::Empty);
    assert_eq!(
        ignored(send(&mut sv, &t, from, &vec![CHALLENGE_REQUEST; 1501], 0)),
        Ignore::TooLong
    );
    for ty in (0..=255u8).filter(|ty| ty & 1 == 0) {
        let mut bytes = vec![ty];
        bytes.extend_from_slice(&[0; 40]);
        let h = send(&mut sv, &t, from, &bytes, 0);
        if matches!(ty, CHALLENGE_REQUEST | CONNECT_REQUEST | DISCONNECT) {
            continue;
        }
        assert_eq!(ignored(h), Ignore::UnknownType, "type {ty}");
    }
    // Client-side types sent to the server do nothing.
    for ty in [CHALLENGE_RESPONSE, CONNECT_REJECT, CONNECT_ACCEPT] {
        assert_eq!(
            ignored(send(&mut sv, &t, from, &[ty, 0, 0, 0, 0, 0, 0, 0, 0], 0)),
            Ignore::UnknownType
        );
    }
    // A game packet from an address with no connection.
    assert_eq!(
        ignored(send(&mut sv, &t, from, &first_packet(9), 0)),
        Ignore::UnknownSource
    );
    assert_eq!(sv.len(), 0);
}

// ---- idempotence and replacement -------------------------------------------------------------------------------------------

#[test]
fn a_repeated_request_with_the_same_sequence_gets_the_same_accept_and_makes_no_second_connection() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, &standard_fields());
    let req = pending(send(&mut sv, &t, from, &bytes, 0));
    let Finished::Accepted {
        conn, reply: first, ..
    } = sv.finish_request(&t, req, Decision::Accept, 0)
    else {
        panic!("not accepted")
    };
    for now in 1..5 {
        let again = reply(send(&mut sv, &t, from, &bytes, now));
        assert_eq!(again, first);
    }
    assert_eq!(sv.len(), 1);
    assert_eq!(sv.lookup(from), Some(conn));
    // A repeat with a bad digest is silence (the digest is checked before anything is re-sent).
    let mut bad = challenge_digest(&KEY, from, 9);
    bad[0] ^= 1;
    let forged = connect_request(&t, 9, bad, GAME_CONNECTION_CLASS, &standard_fields());
    assert_eq!(
        ignored(send(&mut sv, &t, from, &forged, 6)),
        Ignore::BadDigest
    );
}

#[test]
fn a_newer_request_replaces_the_older_connection_once_it_passes() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let old = join(&mut sv, &t, from, 50, 0);
    // A newer request that fails a check leaves the old connection alone.
    let mut bad = standard_fields();
    bad.game = b"x".to_vec();
    let bytes = request_bytes(&sv, &t, from, 51, GAME_CONNECTION_CLASS, &bad);
    reply(send(&mut sv, &t, from, &bytes, 1));
    assert_eq!(sv.lookup(from), Some(old));
    // A good one replaces it.
    let bytes = request_bytes(&sv, &t, from, 52, GAME_CONNECTION_CLASS, &standard_fields());
    let req = pending(send(&mut sv, &t, from, &bytes, 2));
    assert_eq!(req.replaced, Some(old));
    // The old one stays in the table until the host drops it (a host that does not admit the request keeps its session).
    assert_eq!(sv.lookup(from), Some(old));
    let Finished::Accepted { conn, replaced, .. } = sv.finish_request(&t, req, Decision::Accept, 2)
    else {
        panic!("not accepted")
    };
    assert_eq!(
        replaced,
        Some(old),
        "a host that left it there has it removed now"
    );
    assert_eq!(sv.len(), 1);
    assert_ne!(conn, old, "ids are not reused");
    assert_eq!(sv.lookup(from), Some(conn));
    assert_eq!(sv.connection(conn).map(|c| c.sequence), Some(52));
    assert!(sv.connection(old).is_none());
}

#[test]
fn two_clients_behind_different_ports_have_separate_connections() {
    let (mut sv, t) = (server(), table());
    let a = join(&mut sv, &t, addr(1, 5000), 10, 0);
    let b = join(&mut sv, &t, addr(1, 5001), 10, 0);
    assert_ne!(a, b);
    assert_eq!(sv.len(), 2);
}

// ---- the caps --------------------------------------------------------------------------------------------------------------

#[test]
fn at_most_2_connections_are_pending_per_address_and_4_per_network() {
    let (mut sv, t) = (server(), table());
    join(&mut sv, &t, addr(1, 5000), 10, 0);
    join(&mut sv, &t, addr(1, 5001), 10, 0);
    assert_eq!(
        ignored(request_from(&mut sv, &t, addr(1, 5002), 10, 0)),
        Ignore::PendingPerAddr
    );
    let mut sv = Server::new(cfg(100), KEY);
    for host in 1..=NET_PENDING_PER_NETWORK as u8 {
        join(&mut sv, &t, net_addr(host, 5000), 10, 0);
    }
    assert_eq!(
        ignored(request_from(&mut sv, &t, net_addr(99, 5000), 10, 0)),
        Ignore::PendingPerNetwork
    );
    // A first packet from one frees its network's slot.
    assert!(matches!(
        send(&mut sv, &t, net_addr(1, 5000), &first_packet(10), 1),
        Handled::Game { .. }
    ));
    pending(request_from(&mut sv, &t, net_addr(99, 5000), 10, 2));
}

#[test]
fn ten_addresses_holding_every_pending_slot_cannot_lock_out_another_network() {
    // Ten addresses of one /24 hold 2 pending connections each (the caps raised so they can): another network's request is
    // still accepted and established, the oldest pending connection making room.
    let t = table();
    let mut config = cfg(100);
    config.caps.pending_per_network = 2 * 10;
    config.caps.per_network = 2 * 10;
    let mut sv = Server::new(config, KEY);
    let mut first = None;
    for host in 1..=10u8 {
        for port in [5000, 5001] {
            let (conn, evicted) = admit(&mut sv, &t, net_addr(host, port), 10, 0);
            assert_eq!(evicted, None);
            first.get_or_insert(conn);
        }
    }
    assert_eq!(sv.pending_len(), NET_MAX_PENDING_CONNECTS);
    let honest = addr(1, 7000);
    let (conn, evicted) = admit(&mut sv, &t, honest, 10, 1);
    assert_eq!(evicted, first, "the oldest pending connection made room");
    assert_eq!(sv.pending_len(), NET_MAX_PENDING_CONNECTS);
    assert!(matches!(
        send(&mut sv, &t, honest, &first_packet(10), 2),
        Handled::Game { conn: c, .. } if c == conn
    ));
    assert_eq!(
        sv.connection(conn).map(Connection::stage),
        Some(Stage::Established)
    );
    // At the default caps, an 11th address of a network that holds 4 pending is refused.
    let mut sv = Server::new(cfg(100), KEY);
    for host in 1..=10u8 {
        let h = request_from(&mut sv, &t, net_addr(host, 5000), 10, 0);
        if host as usize <= NET_PENDING_PER_NETWORK {
            accept(&mut sv, &t, h, 0);
        } else {
            assert_eq!(ignored(h), Ignore::PendingPerNetwork);
        }
    }
    assert_eq!(
        ignored(request_from(&mut sv, &t, net_addr(11, 5000), 10, 0)),
        Ignore::PendingPerNetwork
    );
    pending(request_from(&mut sv, &t, addr(2, 5000), 10, 0));
}

#[test]
fn an_eviction_takes_from_a_network_over_its_share_never_an_honest_client_elsewhere() {
    let t = table();
    let mut config = cfg(100);
    config.caps = SourceCaps::unlimited();
    let mut sv = Server::new(config, KEY);
    // An honest client is the oldest pending; one network holds the other 19 slots.
    let (honest, _) = admit(&mut sv, &t, addr(1, 5000), 10, 0);
    let mut hostile = Vec::new();
    for host in 1..=19u8 {
        hostile.push(admit(&mut sv, &t, net_addr(host, 5000), 10, 1).0);
    }
    let (_, evicted) = admit(&mut sv, &t, addr(2, 5000), 10, 2);
    assert_eq!(
        evicted,
        hostile.first().copied(),
        "the network over its share pays"
    );
    assert!(sv.connection(honest).is_some(), "the honest client stays");
}

#[test]
fn a_requester_whose_network_holds_its_share_is_refused_and_nothing_is_evicted() {
    let t = table();
    let mut sv = Server::new(cfg(100), KEY);
    // Twenty networks of one each: the share is one, and the requester's network holds it.
    for h in 1..=20u8 {
        admit(&mut sv, &t, addr(h, 5000), 10, 0);
    }
    let req = pending(request_from(&mut sv, &t, addr(1, 6000), 10, 1));
    let Finished::Rejected { reply } = sv.finish_request(&t, req, Decision::Accept, 1) else {
        panic!("not refused")
    };
    assert_eq!(reject_reason(&t, &reply, 10), CR_SERVERFULL);
    assert_eq!(
        sv.pending_len(),
        NET_MAX_PENDING_CONNECTS,
        "nothing was evicted"
    );
}

#[test]
fn an_honest_networks_only_pending_slot_is_never_evicted_while_young() {
    // Twenty networks of one each and a newcomer: the share comes to 0, taken as 1, so no network is over it. The young slots
    // stay and the newcomer is refused; once they are past the evict age, the oldest makes room (and is reported).
    let t = table();
    let mut sv = Server::new(cfg(100), KEY);
    let held: Vec<ConnId> = (1..=20u8)
        .map(|h| admit(&mut sv, &t, addr(h, 5000), 10, 0).0)
        .collect();
    let req = pending(request_from(&mut sv, &t, addr(99, 5000), 10, 2));
    assert!(matches!(
        sv.finish_request(&t, req, Decision::Accept, 2),
        Finished::Rejected { .. }
    ));
    assert!(
        held.iter().all(|c| sv.connection(*c).is_some()),
        "no honest slot was evicted"
    );
    let (_, evicted) = admit(&mut sv, &t, addr(99, 5000), 11, NET_PENDING_EVICT_AGE_MS);
    assert_eq!(
        evicted,
        held.first().copied(),
        "an eviction is always reported"
    );
}

#[test]
fn a_table_full_of_evenly_spread_pending_connections_still_admits_a_new_network() {
    let t = table();
    let mut sv = Server::new(cfg(100), KEY);
    // Ten networks of two each: none is over its share, and the newcomer's holds none.
    let mut held = Vec::new();
    for host in 1..=10u8 {
        held.push(admit(&mut sv, &t, addr(host, 5000), 10, u32::from(host)).0);
        held.push(admit(&mut sv, &t, addr(host, 5001), 10, u32::from(host)).0);
    }
    assert_eq!(sv.pending_len(), NET_MAX_PENDING_CONNECTS);
    let (conn, evicted) = admit(&mut sv, &t, addr(50, 5000), 10, 20);
    assert_eq!(
        evicted,
        held.first().copied(),
        "the oldest of all made room"
    );
    assert!(sv.connection(conn).is_some());
}

#[test]
fn pending_connections_fill_a_small_table_only_until_a_new_one_evicts_one_past_the_evict_age() {
    // A table of 4 (no players and the margin), all pending: a new network's accept evicts the oldest once it is past the
    // evict age, instead of "full".
    let t = table();
    let mut sv = Server::new(cfg(0), KEY);
    let limit = sv.config().max_connections();
    let held: Vec<ConnId> = (0..limit as u8)
        .map(|h| admit(&mut sv, &t, addr(h + 10, 5000), 10, 0).0)
        .collect();
    let (_, evicted) = admit(&mut sv, &t, addr(1, 5000), 10, NET_PENDING_EVICT_AGE_MS);
    assert_eq!(evicted, held.first().copied());
    assert_eq!(sv.len(), limit);
}

#[test]
fn a_source_holds_at_most_4_connections_per_address_and_8_per_network() {
    let t = table();
    let mut sv = Server::new(cfg(100), KEY);
    let established = |sv: &mut Server, from: SocketAddr| {
        join(sv, &t, from, 10, 0);
        assert!(matches!(
            send(sv, &t, from, &first_packet(10), 1),
            Handled::Game { .. }
        ));
    };
    for port in 0..NET_CONNS_PER_ADDR as u16 {
        established(&mut sv, addr(1, 5000 + port));
    }
    let r = reply(request_from(&mut sv, &t, addr(1, 6000), 10, 2));
    assert_eq!(reject_reason(&t, &r, 10), CR_SERVERFULL);
    // Another network is still accepted.
    pending(request_from(&mut sv, &t, addr(2, 5000), 10, 2));
    let mut sv = Server::new(cfg(100), KEY);
    for host in 1..=NET_CONNS_PER_NETWORK as u8 {
        established(&mut sv, net_addr(host, 5000));
    }
    let r = reply(request_from(&mut sv, &t, net_addr(99, 5000), 10, 2));
    assert_eq!(reject_reason(&t, &r, 10), CR_SERVERFULL);
    pending(request_from(&mut sv, &t, addr(2, 5000), 10, 2));
    // A reconnect from a held address replaces its own connection and is not refused.
    pending(request_from(&mut sv, &t, net_addr(1, 5000), 11, 3));
}

#[test]
fn a_replaced_pending_connection_does_not_count_against_the_caps_twice() {
    let (mut sv, t) = (server(), table());
    join(&mut sv, &t, addr(1, 5000), 10, 0);
    join(&mut sv, &t, addr(1, 5001), 10, 0);
    // The address at port 5000 reconnects: it replaces its own pending slot.
    let bytes = request_bytes(
        &sv,
        &t,
        addr(1, 5000),
        11,
        GAME_CONNECTION_CLASS,
        &standard_fields(),
    );
    let req = pending(send(&mut sv, &t, addr(1, 5000), &bytes, 1));
    assert!(req.replaced.is_some());
}

#[test]
fn a_pending_connection_that_never_speaks_expires() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let conn = join(&mut sv, &t, from, 10, 0);
    assert!(
        sv.poll_timeouts(NET_PENDING_TIMEOUT_MS)
            .iter()
            .all(|e| !matches!(e, TimeoutEvent::PendingExpired { .. }))
    );
    let events = sv.poll_timeouts(NET_PENDING_TIMEOUT_MS + 1);
    assert!(matches!(events.as_slice(), [TimeoutEvent::PendingExpired { conn: c }] if *c == conn));
    assert_eq!(sv.len(), 0);
    assert_eq!(sv.lookup(from), None);
}

#[test]
fn the_table_holds_max_players_plus_four_and_then_says_the_server_is_full() {
    let t = table();
    let mut sv = Server::new(cfg(2), KEY);
    let limit = sv.config().max_connections();
    assert_eq!(limit, 6);
    for i in 0..limit as u8 {
        let from = addr(i + 1, 5000);
        join(&mut sv, &t, from, 10, 0);
        // Activate it so pending never limits.
        assert!(matches!(
            send(&mut sv, &t, from, &first_packet(10), 1),
            Handled::Game { .. }
        ));
    }
    assert_eq!(sv.len(), limit);
    let from = addr(100, 5000);
    let bytes = request_bytes(&sv, &t, from, 10, GAME_CONNECTION_CLASS, &standard_fields());
    let r = reply(send(&mut sv, &t, from, &bytes, 2));
    assert_eq!(reject_reason(&t, &r, 10), CR_SERVERFULL);
    assert_eq!(sv.len(), limit);
    // A reconnect from a connected address replaces its slot and is not "full".
    let again = request_bytes(
        &sv,
        &t,
        addr(1, 5000),
        11,
        GAME_CONNECTION_CLASS,
        &standard_fields(),
    );
    pending(send(&mut sv, &t, addr(1, 5000), &again, 3));
}

#[test]
fn challenge_replies_are_limited_per_address_and_in_all() {
    let (mut sv, t) = (server(), table());
    let from = addr(1, 5000);
    for i in 0..NET_CHALLENGES_PER_ADDR_PER_S {
        reply(send(&mut sv, &t, from, &challenge_request(i), 100));
    }
    assert_eq!(
        ignored(send(&mut sv, &t, from, &challenge_request(99), 500)),
        Ignore::RateLimited
    );
    // Another port of the same address shares the budget.
    assert_eq!(
        ignored(send(
            &mut sv,
            &t,
            addr(1, 6000),
            &challenge_request(99),
            500
        )),
        Ignore::RateLimited
    );
    // Another address does not; the second after the first, the budget is back.
    reply(send(&mut sv, &t, addr(2, 5000), &challenge_request(1), 500));
    reply(send(&mut sv, &t, from, &challenge_request(99), 1100));

    // One network: its addresses share the network's budget.
    let mut sv = server();
    let mut answered = 0u32;
    for host in 0..=255u8 {
        if let Handled::Reply(_) = send(
            &mut sv,
            &t,
            net_addr(host, 4000),
            &challenge_request(1),
            5000,
        ) {
            answered += 1;
        }
    }
    assert_eq!(answered, NET_CHALLENGES_PER_NETWORK_PER_S);
    reply(send(
        &mut sv,
        &t,
        net_addr(1, 4000),
        &challenge_request(1),
        6001,
    ));
}

#[test]
fn a_spoofed_flood_of_challenges_still_lets_an_honest_client_through_at_once() {
    // 20,000 spoofed challenge requests a second from random sources: an honest client's first or second try is answered.
    let (mut sv, t) = (server(), table());
    let mut rng = crate::testrng::TestRng::new(0x2545_f491_4f6c_dd1d);
    let honest = addr(1, 5000);
    for second in 0..3u32 {
        let mut tries = 0;
        let mut answered = false;
        for i in 0..20_000u32 {
            let bits = rng.next_u64().to_le_bytes();
            let spoofed = SocketAddr::from((
                [bits[0], bits[1], bits[2], bits[3]],
                u16::from_le_bytes([bits[4], bits[5]]),
            ));
            let now = second * 1_000 + i / 20;
            let _ = send(&mut sv, &t, spoofed, &challenge_request(i), now);
            if i % 10_000 == 5_000 && !answered {
                tries += 1;
                answered = matches!(
                    send(&mut sv, &t, honest, &challenge_request(7), now),
                    Handled::Reply(_)
                );
            }
        }
        assert!(answered && tries <= 2, "second {second}: {tries} tries");
    }
}

#[test]
fn the_per_address_counters_stay_bounded_under_a_spoofed_flood() {
    let (mut sv, t) = (server(), table());
    for i in 0..20_000u32 {
        let from = SocketAddr::from(([(i >> 16) as u8 + 11, (i >> 8) as u8, i as u8, 1], 4000));
        let _ = send(&mut sv, &t, from, &challenge_request(i), i / 4);
        assert_eq!(sv.len(), 0);
    }
    assert_eq!(sv.len(), 0, "a challenge stores no connection");
}

#[test]
fn a_connection_reads_at_most_512_packets_a_second() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    join(&mut sv, &t, from, 10, 0);
    let ack = first_packet(10);
    let mut ok = 0;
    let mut limited = 0;
    for i in 0..1200u32 {
        match send(&mut sv, &t, from, &ack, 10 + i / 40) {
            Handled::Game { .. } => ok += 1,
            Handled::Ignored(Ignore::RateLimited) => limited += 1,
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(ok, NET_PACKETS_PER_CONN_PER_S as usize);
    assert_eq!(limited, 1200 - ok);
}

// ---- disconnect ------------------------------------------------------------------------------------------------------------

#[test]
fn a_disconnect_counts_only_from_the_connections_address_with_its_sequence() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let conn = join(&mut sv, &t, from, 10, 0);
    let kick = disconnect(&t, 10, b"bye");
    // Another source, another port: silence.
    assert_eq!(
        ignored(send(&mut sv, &t, addr(2, 5000), &kick, 1)),
        Ignore::UnknownSource
    );
    assert_eq!(
        ignored(send(&mut sv, &t, addr(1, 5001), &kick, 1)),
        Ignore::UnknownSource
    );
    // The right address, a wrong sequence: silence.
    for seq in [0, 9, 11, u32::MAX] {
        assert_eq!(
            ignored(send(&mut sv, &t, from, &disconnect(&t, seq, b"x"), 1)),
            Ignore::BadSequence
        );
    }
    // Truncated: silence.
    assert_eq!(
        ignored(send(&mut sv, &t, from, &kick[..6], 1)),
        Ignore::Malformed
    );
    assert_eq!(sv.lookup(from), Some(conn));
    match send(&mut sv, &t, from, &kick, 2) {
        Handled::Disconnected { conn: c, reason } => {
            assert_eq!(c, conn);
            assert_eq!(reason, b"bye");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(sv.len(), 0);
    // And the connection is gone for a second one.
    assert_eq!(
        ignored(send(&mut sv, &t, from, &kick, 3)),
        Ignore::UnknownSource
    );
}

#[test]
fn the_server_tells_a_client_it_is_disconnected() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let mut client = client(10);
    let challenge = client.challenge_request();
    let response = reply(send(&mut sv, &t, from, &challenge, 0));
    let ClientEvent::Send(connect) = client.on_datagram(&t, &response, 0) else {
        panic!("no connect request")
    };
    let req = pending(send(&mut sv, &t, from, &connect, 0));
    let Finished::Accepted {
        conn,
        reply: accept,
        ..
    } = sv.finish_request(&t, req, Decision::Accept, 0)
    else {
        panic!("not accepted")
    };
    client.on_datagram(&t, &accept, 0);
    let (to, datagram) = sv
        .disconnect_datagram(&t, conn, b"Kicked")
        .expect("a connection");
    assert_eq!(to, from);
    match client.on_datagram(&t, &datagram, 1) {
        ClientEvent::Disconnected(reason) => assert_eq!(reason, b"Kicked"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        client.state(),
        &ClientState::Disconnected(b"Kicked".to_vec())
    );
}

// ---- game packets and time -------------------------------------------------------------------------------------------------

#[test]
fn a_bad_rate_prefix_after_a_valid_header_is_a_game_error() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let conn = join(&mut sv, &t, from, 10, 0);
    let mut w = BitWriter::new();
    crate::protocol::Header {
        connect_bit: false,
        seq: 1,
        ack: 0,
        ptype: PacketType::Data,
        ack_bytes: 0,
        ack_mask: 0,
    }
    .write(&mut w);
    // The cur pair flag is set but its 20 bits are missing.
    w.write_flag(true);
    w.write_bits(5, 6);
    let bytes = w.into_bytes();
    match send(&mut sv, &t, from, &bytes, 1) {
        Handled::GameError { conn: c, error } => {
            assert_eq!(c, conn);
            assert_eq!(error, NetError::Overrun);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_dropped_game_packet_is_silence() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    join(&mut sv, &t, from, 10, 0);
    // The wrong connect bit (seq 10 is even; this one is for an odd sequence).
    let wrong = first_packet(11);
    assert_eq!(
        ignored(send(&mut sv, &t, from, &wrong, 1)),
        Ignore::Packet(Dropped::ConnectBit)
    );
    assert_eq!(
        sv.pending_len(),
        1,
        "a dropped packet does not activate the connection"
    );
    // Garbage with the game bit set.
    assert!(matches!(
        send(&mut sv, &t, from, &[0xff; 3], 1),
        Handled::Ignored(Ignore::Packet(Dropped::Malformed))
    ));
}

#[test]
fn silence_pings_then_times_out_a_connection() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let conn = join(&mut sv, &t, from, 10, 0);
    assert!(matches!(
        send(&mut sv, &t, from, &first_packet(10), 1),
        Handled::Game { .. }
    ));
    assert!(sv.poll_timeouts(4000).is_empty());
    let mut now = 1 + 4501;
    let mut pings = 0;
    loop {
        let events = sv.poll_timeouts(now);
        if events.is_empty() {
            break;
        }
        match &events[0] {
            TimeoutEvent::Ping {
                conn: c,
                to,
                datagram,
            } => {
                assert_eq!((*c, *to), (conn, from));
                assert_eq!(datagram[0] & 1, 1);
                pings += 1;
            }
            TimeoutEvent::TimedOut { conn: c } => {
                assert_eq!(*c, conn);
                break;
            }
            other => panic!("{other:?}"),
        }
        now += 4501;
    }
    assert_eq!(pings, 15);
    assert_eq!(sv.len(), 0);
}

// ---- review fixes ---------------------------------------------------------------------------------------------------------

#[test]
fn forged_packets_cost_the_real_peer_nothing_of_its_packet_budget() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    join(&mut sv, &t, from, 10, 0);
    // Forged game-bit datagrams: the wrong connect bit, garbage, a sequence far ahead.
    let wrong_bit = first_packet(11);
    let mut far = Vec::new();
    {
        let mut w = BitWriter::new();
        crate::protocol::Header {
            connect_bit: false,
            seq: 200,
            ack: 0,
            ptype: PacketType::Ack,
            ack_bytes: 0,
            ack_mask: 0,
        }
        .write(&mut w);
        far.extend(w.into_bytes());
    }
    for i in 0..3000u32 {
        let bytes = match i % 3 {
            0 => wrong_bit.clone(),
            1 => vec![0xff; 3],
            _ => far.clone(),
        };
        assert!(matches!(
            send(&mut sv, &t, from, &bytes, 100),
            Handled::Ignored(Ignore::Packet(_))
        ));
    }
    // The real peer's whole budget is still there in the same millisecond.
    let ack = first_packet(10);
    let mut ok = 0;
    for _ in 0..NET_PACKETS_PER_CONN_PER_S {
        if matches!(send(&mut sv, &t, from, &ack, 100), Handled::Game { .. }) {
            ok += 1;
        }
    }
    assert_eq!(ok, NET_PACKETS_PER_CONN_PER_S);
}

#[test]
fn guessing_disconnect_sequences_is_not_free_and_does_not_starve_game_packets() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let conn = join(&mut sv, &t, from, 10, 0);
    for guess in 0..NET_DISCONNECTS_PER_CONN_PER_S {
        assert_eq!(
            ignored(send(
                &mut sv,
                &t,
                from,
                &disconnect(&t, 1000 + guess, b"x"),
                5
            )),
            Ignore::BadSequence
        );
    }
    // The budget is spent: even the right sequence waits for the next second...
    assert_eq!(
        ignored(send(&mut sv, &t, from, &disconnect(&t, 10, b"x"), 5)),
        Ignore::RateLimited
    );
    assert_eq!(sv.lookup(from), Some(conn));
    // ...but the connection's game packets are untouched.
    assert!(matches!(
        send(&mut sv, &t, from, &first_packet(10), 5),
        Handled::Game { .. }
    ));
    assert!(matches!(
        send(&mut sv, &t, from, &disconnect(&t, 10, b"x"), 1100),
        Handled::Disconnected { .. }
    ));
}

#[test]
fn a_source_gets_a_few_verified_connect_requests_a_second() {
    let (mut sv, t) = (Server::new(ServerConfig::new(8), KEY), table());
    let from = addr(1, 5000);
    let mut wrong = standard_fields();
    wrong.game = b"x".to_vec();
    let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, &wrong);
    for _ in 0..NET_CONNECT_SCRIPT_PER_SRC_PER_S {
        reply(send(&mut sv, &t, from, &bytes, 100));
    }
    assert_eq!(
        ignored(send(&mut sv, &t, from, &bytes, 900)),
        Ignore::RateLimited
    );
    // Another port of the same address shares the budget; another address does not.
    let other_port = request_bytes(&sv, &t, addr(1, 5001), 9, GAME_CONNECTION_CLASS, &wrong);
    assert_eq!(
        ignored(send(&mut sv, &t, addr(1, 5001), &other_port, 900)),
        Ignore::RateLimited
    );
    let other = request_bytes(&sv, &t, addr(2, 5000), 9, GAME_CONNECTION_CLASS, &wrong);
    reply(send(&mut sv, &t, addr(2, 5000), &other, 900));
    // An unverified request does not use the budget up.
    let mut bad = challenge_digest(&KEY, addr(3, 5000), 9);
    bad[0] ^= 1;
    let forged = connect_request(&t, 9, bad, GAME_CONNECTION_CLASS, &wrong);
    for _ in 0..50 {
        assert_eq!(
            ignored(send(&mut sv, &t, addr(3, 5000), &forged, 100)),
            Ignore::BadDigest
        );
    }
    // The next second the budget is back.
    reply(send(&mut sv, &t, from, &bytes, 1200));
}

#[test]
fn the_repeated_accept_resend_is_rate_limited() {
    let (mut sv, t, from) = (
        Server::new(ServerConfig::with_profile(8, prof()), KEY),
        table(),
        addr(1, 5000),
    );
    let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, &standard_fields());
    let req = pending(send(&mut sv, &t, from, &bytes, 0));
    assert!(matches!(
        sv.finish_request(&t, req, Decision::Accept, 0),
        Finished::Accepted { .. }
    ));
    // One verified request was used by the first; three resends fit the burst of four, the fourth does not.
    for _ in 0..NET_CONNECT_SCRIPT_PER_SRC_PER_S - 1 {
        reply(send(&mut sv, &t, from, &bytes, 10));
    }
    assert_eq!(
        ignored(send(&mut sv, &t, from, &bytes, 10)),
        Ignore::RateLimited
    );
    reply(send(&mut sv, &t, from, &bytes, 1100));
}

#[test]
fn pending_per_address_is_a_config_knob() {
    let t = table();
    let mut config = cfg(100);
    config.caps.pending_per_ip = 5;
    config.caps.pending_per_network = 5;
    config.caps.per_ip = 10;
    let mut sv = Server::new(config, KEY);
    for port in 0..5 {
        join(&mut sv, &t, addr(1, 5000 + port), 10, 0);
    }
    let bytes = request_bytes(
        &sv,
        &t,
        addr(1, 5005),
        10,
        GAME_CONNECTION_CLASS,
        &standard_fields(),
    );
    assert_eq!(
        ignored(send(&mut sv, &t, addr(1, 5005), &bytes, 0)),
        Ignore::PendingPerAddr
    );
    assert_eq!(cfg(1).caps, SourceCaps::default());
}

#[test]
fn finish_request_rechecks_the_table_capacity() {
    let t = table();
    let mut sv = Server::new(cfg(0), KEY);
    assert_eq!(sv.config().max_connections(), NET_CONNECTIONS_MARGIN);
    let late = addr(100, 5000);
    let bytes = request_bytes(&sv, &t, late, 10, GAME_CONNECTION_CLASS, &standard_fields());
    let req = pending(send(&mut sv, &t, late, &bytes, 0));
    // The table fills with established connections while the script runs (pending ones would be evicted).
    for i in 0..NET_CONNECTIONS_MARGIN as u8 {
        join(&mut sv, &t, addr(i + 1, 5000), 10, 0);
        assert!(matches!(
            send(&mut sv, &t, addr(i + 1, 5000), &first_packet(10), 0),
            Handled::Game { .. }
        ));
    }
    let Finished::Rejected { reply } = sv.finish_request(&t, req, Decision::Accept, 1) else {
        panic!("accepted into a full table")
    };
    assert_eq!(reject_reason(&t, &reply, 10), CR_SERVERFULL);
    assert_eq!(sv.len(), NET_CONNECTIONS_MARGIN);
    assert_eq!(sv.lookup(late), None);
}

#[test]
fn a_late_accept_never_replaces_a_newer_connection_and_reports_what_it_replaced() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let first = request_bytes(&sv, &t, from, 5, GAME_CONNECTION_CLASS, &standard_fields());
    let second = request_bytes(&sv, &t, from, 6, GAME_CONNECTION_CLASS, &standard_fields());
    let r1 = pending(send(&mut sv, &t, from, &first, 0));
    let r2 = pending(send(&mut sv, &t, from, &second, 0));
    // The older request is accepted first, the newer one replaces it and says so.
    let Finished::Accepted { conn: c1, .. } =
        sv.finish_request(&t, r1.clone(), Decision::Accept, 1)
    else {
        panic!("not accepted")
    };
    let Finished::Accepted {
        conn: c2, replaced, ..
    } = sv.finish_request(&t, r2, Decision::Accept, 2)
    else {
        panic!("not accepted")
    };
    assert_eq!(replaced, Some(c1));
    assert_eq!(sv.lookup(from), Some(c2));
    // A late accept of the older sequence, or a repeat of the same one, changes nothing and sends nothing.
    assert!(matches!(
        sv.finish_request(&t, r1, Decision::Accept, 3),
        Finished::Refused
    ));
    assert_eq!(sv.lookup(from), Some(c2));
    assert_eq!(sv.len(), 1);
}

#[test]
fn the_limiter_forgets_the_least_recent_source_and_stays_bounded() {
    let mut l = SourceLimiter::default();
    for i in 0..(NET_LIMITER_ENTRIES as u32 + 10) {
        let ip = IpAddr::from((0x0a00_0000 + i).to_be_bytes());
        assert!(l.allow(ip, 0, 1));
        assert!(l.len() <= NET_LIMITER_ENTRIES);
    }
    // The first ten sources were forgotten (their budget is back); the newest still holds its count.
    assert!(l.allow(IpAddr::from([10, 0, 0, 0]), 0, 1));
    let newest = 0x0a00_0000 + NET_LIMITER_ENTRIES as u32 + 9;
    assert!(!l.allow(IpAddr::from(newest.to_be_bytes()), 0, 1));
}

#[test]
fn a_local_connection_is_established_at_once_and_never_times_out() {
    let mut s = Server::new(ServerConfig::new(8), KEY);
    let local = SocketAddr::from(([0, 0, 0, 0], 0));
    let id = s.insert_local(local, 12, 0);
    let c = s.connection(id).expect("in the table");
    assert!(c.local && !c.pending && !c.extension);
    assert_eq!((c.sequence, c.protocol_version), (0, 12));
    assert_eq!(s.lookup(local), Some(id));
    // Long silences: no ping, no time-out, no pending expiry.
    for now in [10_000, 100_000, 10_000_000] {
        assert!(s.poll_timeouts(now).is_empty());
    }
    assert!(s.connection(id).is_some());
    // The protocol is the client's, at most the profile's; a second local connection replaces the first.
    let again = s.insert_local(local, 20, 5);
    assert_ne!(again, id);
    assert!(s.connection(id).is_none());
    assert_eq!(s.connection(again).map(|c| c.protocol_version), Some(12));
    // The client half connects with sequence 0 and the accept the server half wrote.
    let fields = ConnectFields::standard(b"", vec![b"a".to_vec()]);
    let client = Client::local(
        fields,
        RateConfig::standard(),
        12,
        u32::from(s.connection(again).unwrap().ghost_bits),
        0,
    );
    assert_eq!(*client.state(), ClientState::Connected);
    assert_eq!(client.sequence(), 0);
    assert!(client.protocol().is_some());
}

// ---- the handshake extension -----------------------------------------------------------------------------------------

#[test]
fn a_request_ending_with_the_extension_argument_gets_it_and_the_scripts_never_see_the_argument() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let mut f = standard_fields();
    f.args.push(EXT_ARG.to_vec());
    let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, &f);
    let req = pending(send(&mut sv, &t, from, &bytes, 0));
    assert!(req.extension);
    assert_eq!(req.fields.args, standard_fields().args);
    let Finished::Accepted { conn, .. } = sv.finish_request(&t, req, Decision::Accept, 0) else {
        panic!("not accepted");
    };
    let c = sv.connection(conn).expect("the connection");
    assert!(c.extension);
    assert_eq!(c.protocol.packet_limit_bits(), 1_400 * 8);
    assert_eq!(c.ghost_bits, 22);
}

#[test]
fn a_standard_request_is_served_at_the_standard_rates_unless_the_host_fills_packets() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let conn = join(&mut sv, &t, from, 9, 0);
    let c = sv.connection(conn).expect("the connection");
    assert!(!c.extension);
    assert_eq!(c.protocol.packet_limit_bits(), 200 * 8);
    assert_eq!(c.protocol.max_rate().delay, 1024 / 10);
    sv.config_mut().packet_bytes = Some(1_400);
    sv.config_mut().rates = RateConfig::new(32, 32, 200);
    let conn = join(&mut sv, &t, addr(2, 5000), 9, 0);
    let c = sv.connection(conn).expect("the connection");
    assert_eq!(c.protocol.packet_limit_bits(), 1_400 * 8);
    assert_eq!(c.protocol.max_rate().delay, 1024 / 32);
}

#[test]
fn the_extension_argument_counts_against_the_sixteen_and_is_taken_only_from_the_end() {
    // Sixteen and the argument: seventeen, refused before anything is taken off (the standard check).
    let mut f = standard_fields();
    f.args = (0..16).map(|i| format!("a{i}").into_bytes()).collect();
    f.args.push(EXT_ARG.to_vec());
    assert_eq!(refused_with(&f), CR_INVALID_ARGS);
    // Anywhere but last it is an ordinary argument.
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let mut f = standard_fields();
    f.args.insert(0, EXT_ARG.to_vec());
    let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, &f);
    let req = pending(send(&mut sv, &t, from, &bytes, 0));
    assert!(!req.extension);
    assert_eq!(req.fields.args.len(), 6);
    let mut args = vec![b"x".to_vec(), EXT_ARG.to_vec(), EXT_ARG.to_vec()];
    assert!(prof().take_extension_arg(&mut args));
    assert_eq!(args.len(), 2);
    // A server without an extension leaves the argument to the scripts.
    let mut config = cfg(8);
    config.profile.extension = None;
    let mut plain = Server::new(config, KEY);
    let mut f = standard_fields();
    f.args.push(EXT_ARG.to_vec());
    let bytes = request_bytes(&plain, &t, from, 9, GAME_CONNECTION_CLASS, &f);
    let req = pending(send(&mut plain, &t, from, &bytes, 0));
    assert!(!req.extension);
    assert_eq!(req.fields.args.last().map(Vec::as_slice), Some(EXT_ARG));
}

#[test]
fn only_a_client_that_asked_is_sent_the_extension_word() {
    let (mut sv, t, from) = (server(), table(), addr(1, 5000));
    let mut f = standard_fields();
    f.args.push(EXT_ARG.to_vec());
    let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, &f);
    let req = pending(send(&mut sv, &t, from, &bytes, 0));
    let Finished::Accepted { reply, .. } = sv.finish_request(&t, req, Decision::Accept, 0) else {
        panic!("not accepted");
    };
    assert!(parse_accept(&prof(), &reply).unwrap().extension);
    // A repeated request is answered with the same accept.
    let again = reply_of(send(&mut sv, &t, from, &bytes, 1));
    assert!(parse_accept(&prof(), &again).unwrap().extension);
    // A standard request's accept is thirteen bytes.
    let (from2, f2) = (addr(2, 5000), standard_fields());
    let bytes = request_bytes(&sv, &t, from2, 9, GAME_CONNECTION_CLASS, &f2);
    let req = pending(send(&mut sv, &t, from2, &bytes, 0));
    let Finished::Accepted { reply, .. } = sv.finish_request(&t, req, Decision::Accept, 0) else {
        panic!("not accepted");
    };
    assert_eq!(reply.len(), 13);
    assert!(!parse_accept(&prof(), &reply).unwrap().extension);
}

fn reply_of(h: Handled) -> Vec<u8> {
    reply(h)
}

/// The accept a request gets from a server whose extension gives `ext_bits`, as `(ghost bits, extension word)`.
fn accept_of(ext_bits: Option<u8>, ghost_limit: u32, ask: bool) -> (Option<u32>, bool) {
    let mut config = cfg(8);
    config.ghost_limit = ghost_limit;
    if let Some(ext) = config.profile.extension.as_mut() {
        ext.ghost_bits = ext_bits;
    }
    let (mut sv, t, from) = (Server::new(config, KEY), table(), addr(1, 5000));
    let mut f = standard_fields();
    if ask {
        f.args.push(EXT_ARG.to_vec());
    }
    let bytes = request_bytes(&sv, &t, from, 9, GAME_CONNECTION_CLASS, &f);
    let req = pending(send(&mut sv, &t, from, &bytes, 0));
    let Finished::Accepted { reply, conn, .. } = sv.finish_request(&t, req, Decision::Accept, 0)
    else {
        panic!("not accepted")
    };
    let accept = parse_accept(&prof(), &reply).expect("an accept");
    assert_eq!(
        Some(u32::from(sv.connection(conn).unwrap().ghost_bits)),
        accept.ghost_bits
    );
    (accept.ghost_bits, accept.extension)
}

#[test]
fn the_extension_width_is_given_only_to_a_peer_that_asked() {
    // A peer that asked: the extension's width, never below the standard width, never past the crate's bound.
    assert_eq!(accept_of(Some(22), 262_144, true), (Some(22), true));
    assert_eq!(accept_of(Some(16), 262_144, true), (Some(18), true));
    assert_eq!(accept_of(Some(40), 262_144, true), (Some(24), true));
    // A standard client of the same server: the standard width, the profile's 18 at most.
    assert_eq!(accept_of(Some(22), 262_144, false), (Some(18), false));
    assert_eq!(accept_of(Some(22), u32::MAX, false), (Some(18), false));
    // An extension without a width: the standard width.
    assert_eq!(accept_of(None, 262_144, true), (Some(18), true));
    // The local connection has the extension.
    let mut sv = Server::new(cfg(8), KEY);
    let local = sv.insert_local(addr(9, 9), 9, 0);
    let c = sv.connection(local).unwrap();
    assert!(c.extension);
    assert_eq!(c.ghost_bits, 22);
}

/// SECURITY (resource cap): any client may ask for the extension, so the wide tables strangers can make a server keep are bounded:
/// past the limit's count or slots a client that asked gets the standard width (and keeps the extension), and a freed wide table
/// makes room again. The local connection is not counted.
#[test]
fn wide_tables_are_granted_only_within_the_extensions_limit() {
    let with_limit = |clients: usize, slots: u64| {
        let mut config = cfg(16);
        if let Some(ext) = config.profile.extension.as_mut() {
            ext.wide_limit = Some(crate::profile::WideLimit { clients, slots });
        }
        Server::new(config, KEY)
    };
    let t = table();
    let ext_join = |sv: &mut Server, host: u8| {
        let from = addr(host, 5000);
        let mut f = standard_fields();
        f.args.push(EXT_ARG.to_vec());
        let bytes = request_bytes(sv, &t, from, 9, GAME_CONNECTION_CLASS, &f);
        let req = pending(send(sv, &t, from, &bytes, 0));
        let Finished::Accepted { reply, conn, .. } =
            sv.finish_request(&t, req, Decision::Accept, 0)
        else {
            panic!("not accepted")
        };
        (conn, parse_accept(&prof(), &reply).unwrap().ghost_bits)
    };
    let mut sv = with_limit(2, u64::MAX);
    let local = sv.insert_local(addr(9, 9), 9, 0);
    assert_eq!(sv.connection(local).unwrap().ghost_bits, 22);
    let (first, bits) = ext_join(&mut sv, 1);
    assert_eq!(bits, Some(22));
    assert_eq!(ext_join(&mut sv, 2).1, Some(22));
    // Two wide tables held: the third gets the standard width, and still has the extension.
    let (third, bits) = ext_join(&mut sv, 3);
    assert_eq!(bits, Some(12));
    assert!(sv.connection(third).unwrap().extension);
    sv.remove(first);
    assert_eq!(ext_join(&mut sv, 4).1, Some(22));
    // The slots: one 22-bit table and room for less than another.
    let mut sv = with_limit(16, (1 << 22) + (1 << 20));
    assert_eq!(ext_join(&mut sv, 1).1, Some(22));
    assert_eq!(ext_join(&mut sv, 2).1, Some(12));
    // A limit of none: the standard width; a width within the standard maximum needs no room.
    let mut sv = with_limit(0, u64::MAX);
    assert_eq!(ext_join(&mut sv, 1).1, Some(12));
}

#[test]
fn the_client_takes_the_extension_width_only_from_an_accept_that_granted_it() {
    let t = table();
    for (bits, granted, ok) in [
        (18, false, true),
        (19, false, false),
        (22, false, false),
        (19, true, true),
        (22, true, true),
        (23, true, false),
        (32, true, false),
        (u32::from(u8::MAX), true, false),
        (9, true, false),
    ] {
        let mut client = client(9);
        client.challenge_request();
        client.connect_request(&t, Some([0; 4]));
        let mut w = BitWriter::new();
        w.write_u8(CONNECT_ACCEPT);
        w.write_u32(9);
        w.write_u32(9);
        w.write_u32(bits);
        if granted {
            w.write_u32(WORD);
        }
        match client.on_datagram(&t, &w.into_bytes(), 0) {
            ClientEvent::Accepted(a) if ok => assert_eq!(a.ghost_bits, Some(bits)),
            ClientEvent::Rejected(r) if !ok => {
                assert_eq!(r, CR_TEST_GHOST_BITS, "{bits} {granted}")
            }
            other => panic!("{bits} bits, extension word {granted}: {other:?}"),
        }
    }
}
