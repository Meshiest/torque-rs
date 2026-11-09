//! The connect block, the manifest and the gate over a whole handshake between the test client and the server: a matching peer is
//! accepted, a mismatched one is refused with the reason before the host sees the request, a garbled or oversize block is refused
//! cleanly, an auth ticket is checked after the build, and a peer without any of it keeps the handshake it always had. A small
//! seeded fuzz of the manifest and block decoders closes the file (seeded, so a failure repeats).

use std::net::SocketAddr;
use std::sync::Arc;

use torque_net::bits::{BitReader, BitWriter};
use torque_net::client::{Client, ClientEvent};
use torque_net::connect::{
    AuthContext, AuthProvider, BLOCK_MAGIC, BlockSource, ConnectBlock, MAX_TICKET_BYTES,
    ManifestGate, SharedSecret,
};
use torque_net::handshake::{
    ConnectFields, Decision, Finished, Handled, Server, ServerConfig, challenge_digest,
};
use torque_net::huffman::Table;
use torque_net::manifest::{MAX_MANIFEST_BYTES, Manifest};

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

fn from() -> SocketAddr {
    SocketAddr::from(([10, 0, 0, 7], 4000))
}

/// The address the servers of these tests say they are at, and the address a client dials to reach them.
fn server_addr() -> SocketAddr {
    SocketAddr::from(([192, 0, 2, 1], 28_000))
}

fn manifest() -> Manifest {
    Manifest::new(12, 12)
        .with_plugin("torque.rope", 2)
        .with_ghosts(["Rope"])
        .with_datablocks(["RopeData"])
        .with_channels(["terrain"])
}

fn server_with(gate: Option<ManifestGate>) -> Server {
    let mut config = ServerConfig::new(8);
    config.connect_requests_per_src_per_s = 100_000;
    config.gate = gate.map(|g| Arc::new(g) as _);
    config.address = Some(server_addr());
    Server::new(config, KEY)
}

fn fields(block: Option<ConnectBlock>) -> ConnectFields {
    let mut f = ConnectFields::standard(b"", vec![b"Name".to_vec()]);
    f.block = block.map(Box::new);
    f
}

fn block(manifest: Manifest) -> ConnectBlock {
    ConnectBlock {
        manifest: Some(manifest),
        ticket: None,
        ..Default::default()
    }
}

/// What the server made of a client's connect request.
enum Outcome {
    Accepted,
    /// The client's reading of the reject.
    Refused(Vec<u8>),
    Silence,
}

/// Drives a handshake of `client_fields` against `server`; `finish` is the host's decision on a request that reaches it.
fn join(server: &mut Server, client_fields: ConnectFields, seq: u32) -> Outcome {
    join_client(server, Client::new(seq, client_fields))
}

/// [`join`] for a client that is already made (one that makes its ticket from the challenge, say).
fn join_client(server: &mut Server, mut client: Client) -> Outcome {
    let t = table();
    let challenge = client.challenge_request();
    let Handled::Reply(response) = server.handle_datagram(&t, from(), &challenge, 0) else {
        panic!("no challenge response")
    };
    let ClientEvent::Send(connect) = client.on_datagram(&t, &response, 0) else {
        panic!("no connect request")
    };
    answer(server, client, &connect)
}

fn answer(server: &mut Server, mut client: Client, connect: &[u8]) -> Outcome {
    let t = table();
    match server.handle_datagram(&t, from(), connect, 0) {
        Handled::Request(request) => {
            let Finished::Accepted { reply, .. } =
                server.finish_request(&t, *request, Decision::Accept, 0)
            else {
                panic!("not accepted")
            };
            assert!(matches!(
                client.on_datagram(&t, &reply, 0),
                ClientEvent::Accepted(_)
            ));
            Outcome::Accepted
        }
        Handled::Reply(reject) => match client.on_datagram(&t, &reject, 0) {
            ClientEvent::Rejected(reason) => Outcome::Refused(reason),
            other => panic!("not a reject: {other:?}"),
        },
        Handled::Ignored(_) => Outcome::Silence,
        other => panic!("unexpected {other:?}"),
    }
}

fn refused(outcome: Outcome) -> String {
    match outcome {
        Outcome::Refused(reason) => String::from_utf8(reason).expect("a text reason"),
        Outcome::Accepted => panic!("accepted"),
        Outcome::Silence => panic!("silence"),
    }
}

fn gated() -> Server {
    server_with(Some(ManifestGate::new(manifest())))
}

#[test]
fn a_matching_manifest_is_accepted() {
    let mut server = gated();
    assert!(matches!(
        join(&mut server, fields(Some(block(manifest()))), 5),
        Outcome::Accepted
    ));
    assert_eq!(server.len(), 1);
}

#[test]
fn a_missing_plugin_is_refused_with_the_plugin_named() {
    let client = Manifest::new(12, 12)
        .with_ghosts(["Rope"])
        .with_datablocks(["RopeData"])
        .with_channels(["terrain"]);
    let reason = refused(join(&mut gated(), fields(Some(block(client))), 5));
    assert_eq!(
        reason,
        "server needs plugin torque.rope 2, you do not have it"
    );
}

#[test]
fn a_plugin_of_another_version_is_refused_with_both_versions() {
    let client = Manifest::new(12, 12)
        .with_plugin("torque.rope", 1)
        .with_ghosts(["Rope"])
        .with_datablocks(["RopeData"])
        .with_channels(["terrain"]);
    let mut server = gated();
    let reason = refused(join(&mut server, fields(Some(block(client))), 5));
    assert_eq!(reason, "server needs plugin torque.rope 2, you have 1");
    assert!(server.is_empty(), "nothing was kept for the refused client");
}

#[test]
fn name_tables_that_differ_are_refused_with_the_name() {
    let client = Manifest::new(12, 12)
        .with_plugin("torque.rope", 2)
        .with_ghosts(["Rope", "Extra"])
        .with_datablocks(["RopeData"])
        .with_channels(["terrain"]);
    let reason = refused(join(&mut gated(), fields(Some(block(client))), 5));
    assert_eq!(
        reason,
        "you have ghost class Extra, which this server does not"
    );
    let client = Manifest::new(12, 12)
        .with_plugin("torque.rope", 2)
        .with_ghosts(["Rope"])
        .with_datablocks(["RopeData"]);
    let reason = refused(join(&mut gated(), fields(Some(block(client))), 6));
    assert_eq!(reason, "server has channel terrain, you do not");
}

#[test]
fn a_plain_client_is_told_what_the_server_needs() {
    let reason = refused(join(&mut gated(), fields(None), 5));
    assert_eq!(
        reason,
        "server needs plugin torque.rope 2, you do not have it"
    );
}

#[test]
fn a_server_without_a_gate_never_reads_the_block() {
    let mut server = server_with(None);
    let Outcome::Accepted = join(&mut server, fields(Some(block(Manifest::new(1, 1)))), 5) else {
        panic!("a server without a gate refuses nothing")
    };
}

#[test]
fn a_request_without_a_block_is_the_request_it_always_was() {
    let t = table();
    let digest = challenge_digest(&KEY, from(), 5);
    let plain = torque_net::handshake::connect_request(
        &Default::default(),
        &t,
        5,
        digest,
        b"GameConnection",
        &fields(None),
    );
    let with = torque_net::handshake::connect_request(
        &Default::default(),
        &t,
        5,
        digest,
        b"GameConnection",
        &fields(Some(block(manifest()))),
    );
    // Torque's request is the first part of the longer one, bit for bit up to the last byte the arguments end in.
    assert!(with.len() > plain.len());
    assert_eq!(plain[..plain.len() - 1], with[..plain.len() - 1]);
    // And a client without a block asks the same of a gate-less server as before: nothing of it is kept.
    let mut server = server_with(None);
    let Handled::Request(request) = server.handle_datagram(&t, from(), &plain, 0) else {
        panic!("a request")
    };
    assert_eq!(request.fields.block, None);
    assert_eq!(request.identity, None);
}

/// A connect request of a plain client whose arguments are followed by whatever `tail` writes (the block's place), with the
/// right digest for `seq`.
fn request_with_tail(tail: impl FnOnce(&mut BitWriter), seq: u32) -> Vec<u8> {
    let t = table();
    let types = torque_net::handshake::HandshakeTypes::TORQUE;
    let mut w = BitWriter::new();
    w.write_u8(types.connect_request);
    w.write_u32(seq);
    for word in challenge_digest(&KEY, from(), seq) {
        w.write_u32(word);
    }
    t.write(&mut w, b"GameConnection", 255);
    w.write_u32(torque_net::handshake::CLASS_GROUP);
    w.write_u32(torque_net::handshake::CLASS_CRC);
    t.write(
        &mut w,
        &torque_net::profile::HandshakeProfile::default().game,
        255,
    );
    w.write_u32(12);
    w.write_u32(12);
    t.write(&mut w, b"", 255);
    w.write_u32(1);
    t.write(&mut w, b"Name", 255);
    tail(&mut w);
    w.into_bytes()
}

#[test]
fn a_garbled_or_oversize_block_is_refused_with_a_reason_not_dropped() {
    let t = table();
    let mut server = gated();
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "unknown manifest format",
            request_with_tail(
                |w| {
                    w.write_u32(BLOCK_MAGIC);
                    w.write_u8(1);
                    w.write_u8(1);
                    w.write_bits(1, 16);
                    w.write_u8(9);
                },
                1,
            ),
            "manifest format 9 is not supported (this side speaks 1 and 2)",
        ),
        (
            "truncated",
            request_with_tail(
                |w| {
                    w.write_u32(BLOCK_MAGIC);
                    w.write_u8(2);
                },
                2,
            ),
            "bad connect block",
        ),
        (
            "oversize manifest",
            request_with_tail(
                |w| {
                    w.write_u32(BLOCK_MAGIC);
                    w.write_u8(1);
                    w.write_u8(1);
                    w.write_bits(MAX_MANIFEST_BYTES as u32 + 1, 16);
                    w.write_bytes(&vec![0; MAX_MANIFEST_BYTES + 1]);
                },
                3,
            ),
            "bad connect block",
        ),
        (
            "oversize ticket",
            request_with_tail(
                |w| {
                    w.write_u32(BLOCK_MAGIC);
                    w.write_u8(1);
                    w.write_u8(2);
                    w.write_bits(MAX_TICKET_BYTES as u32 + 1, 16);
                    w.write_bytes(&vec![0; MAX_TICKET_BYTES + 1]);
                },
                4,
            ),
            "bad connect block",
        ),
        (
            "garbled manifest",
            request_with_tail(
                |w| {
                    w.write_u32(BLOCK_MAGIC);
                    w.write_u8(1);
                    w.write_u8(1);
                    w.write_bits(5, 16);
                    w.write_bytes(&[1, 255, 255, 255, 255]);
                },
                5,
            ),
            "invalid manifest",
        ),
    ];
    for (name, datagram, expected) in cases {
        let Handled::Reply(reply) = server.handle_datagram(&t, from(), &datagram, 0) else {
            panic!("{name}: expected a reject")
        };
        let (_, reason) = torque_net::handshake::HandshakeTypes::TORQUE
            .parse_reject(&t, &reply)
            .expect("a reject");
        let reason = String::from_utf8(reason).expect("text");
        assert!(reason.starts_with(expected), "{name}: {reason}");
        assert!(reason.len() <= 255, "{name}");
    }
    assert!(server.is_empty());
}

/// A client that dials `dialed` and makes a block with a ticket for `name` under `key` from the challenge it is given.
fn signing_client(seq: u32, key: [u8; 16], name: &'static str, dialed: SocketAddr) -> Client {
    let mut client = Client::new(seq, fields(None));
    client.set_block_source(
        dialed,
        BlockSource::new(move |cx| {
            Ok(Some(ConnectBlock {
                manifest: Some(manifest()),
                ticket: Some(SharedSecret::new(key, name).ticket(cx)?),
                ..Default::default()
            }))
        }),
    );
    client
}

/// A server that holds clients to `manifest()` and verifies tickets under `key`, says it is at `address` and draws its challenges
/// with `server_key`.
fn secret_server(key: [u8; 16], address: SocketAddr, server_key: [u8; 16]) -> Server {
    let gate = ManifestGate::new(manifest()).with_auth(Arc::new(SharedSecret::new(key, "")));
    let mut config = ServerConfig::new(8);
    config.connect_requests_per_src_per_s = 100_000;
    config.gate = Some(Arc::new(gate));
    config.address = Some(address);
    Server::new(config, server_key)
}

/// The connect request `client` sends `server` once it has the challenge response.
fn connect_datagram(server: &mut Server, client: &mut Client) -> Vec<u8> {
    let t = table();
    let challenge = client.challenge_request();
    let Handled::Reply(response) = server.handle_datagram(&t, from(), &challenge, 0) else {
        panic!("challenge")
    };
    let ClientEvent::Send(connect) = client.on_datagram(&t, &response, 0) else {
        panic!("connect")
    };
    connect
}

#[test]
fn the_ticket_is_verified_before_the_host_sees_the_request() {
    let secret = [9; 16];
    let mut server = secret_server(secret, server_addr(), KEY);
    // A good ticket reaches the host, with the identity attached.
    let t = table();
    let mut client = signing_client(5, secret, "ada", server_addr());
    let connect = connect_datagram(&mut server, &mut client);
    let Handled::Request(request) = server.handle_datagram(&t, from(), &connect, 0) else {
        panic!("a verified request reaches the host")
    };
    assert_eq!(request.identity.as_deref(), Some(&b"ada"[..]));
    let Finished::Accepted { conn, .. } = server.finish_request(&t, *request, Decision::Accept, 0)
    else {
        panic!("accepted")
    };
    assert_eq!(
        server.connection(conn).unwrap().identity.as_deref(),
        Some(&b"ada"[..])
    );
    // A forged ticket, and no ticket, are refused with the provider's reason.
    let forged = refused(join_client(
        &mut server,
        signing_client(6, [1; 16], "ada", server_addr()),
    ));
    assert_eq!(forged, "your sign-in was not accepted");
    let none = refused(join(&mut server, fields(Some(block(manifest()))), 7));
    assert_eq!(none, "sign in to join this server");
}

#[test]
fn a_ticket_for_another_server_is_refused_even_when_a_server_relays_it() {
    let secret = [9; 16];
    let (a, b) = (
        SocketAddr::from(([198, 51, 100, 1], 28_000)),
        SocketAddr::from(([198, 51, 100, 2], 28_000)),
    );
    let t = table();
    // The client dials A. A is hostile: it asks B for the challenge and hands B's answer to the client, then passes the client's
    // request on to B, which shares the secret. The challenge is B's own, so only the address tells the ticket is not for B.
    let mut server_b = secret_server(secret, b, KEY);
    let mut client = signing_client(5, secret, "ada", a);
    let connect = connect_datagram(&mut server_b, &mut client);
    let reason = refused(answer(&mut server_b, client, &connect));
    assert_eq!(reason, "your sign-in was not accepted");
    assert!(server_b.is_empty());

    // A relay that did not get B's challenge sends B a digest B never made: silence.
    let mut server_a = secret_server(secret, a, [8; 16]);
    let mut client = signing_client(5, secret, "ada", a);
    let connect = connect_datagram(&mut server_a, &mut client);
    assert!(matches!(
        server_b.handle_datagram(&t, from(), &connect, 0),
        Handled::Ignored(_)
    ));
    // And the server it was made for takes it.
    assert!(matches!(
        server_a.handle_datagram(&t, from(), &connect, 0),
        Handled::Request(_)
    ));
}

#[test]
fn a_ticket_replayed_on_another_connection_is_refused() {
    let secret = [9; 16];
    let t = table();
    let mut server = secret_server(secret, server_addr(), KEY);
    // The first connection's request, as a sniffer sees it.
    let mut client = signing_client(5, secret, "ada", server_addr());
    let connect = connect_datagram(&mut server, &mut client);
    let sniffed = torque_net::client::parse_connect_request(
        &t,
        &torque_net::profile::HandshakeProfile::default(),
        &connect,
    )
    .expect("a request")
    .fields
    .block
    .expect("a block");
    assert!(matches!(
        server.handle_datagram(&t, from(), &connect, 0),
        Handled::Request(_)
    ));

    // A new connection from the same address (another sequence) carrying that ticket: refused.
    let replay = Client::new(6, fields(Some((*sniffed).clone())));
    let again = refused(join_client(&mut server, replay));
    assert_eq!(again, "your sign-in was not accepted");

    // The same sequence from another source address has another challenge: refused too.
    let other = SocketAddr::from(([10, 0, 0, 8], 4000));
    let mut replay = Client::new(5, fields(Some((*sniffed).clone())));
    let challenge = replay.challenge_request();
    let Handled::Reply(response) = server.handle_datagram(&t, other, &challenge, 0) else {
        panic!("challenge")
    };
    let ClientEvent::Send(connect) = replay.on_datagram(&t, &response, 0) else {
        panic!("connect")
    };
    let Handled::Reply(reject) = server.handle_datagram(&t, other, &connect, 0) else {
        panic!("a reject")
    };
    let (_, reason) = torque_net::handshake::HandshakeTypes::TORQUE
        .parse_reject(&t, &reject)
        .expect("a reject");
    assert_eq!(reason, b"your sign-in was not accepted");
}

#[test]
fn a_provider_that_cannot_make_a_ticket_ends_the_connect_with_its_reason() {
    let t = table();
    let mut server = secret_server([9; 16], server_addr(), KEY);
    let mut client = signing_client(5, [9; 16], "", server_addr());
    let challenge = client.challenge_request();
    let Handled::Reply(response) = server.handle_datagram(&t, from(), &challenge, 0) else {
        panic!("challenge")
    };
    let ClientEvent::Rejected(reason) = client.on_datagram(&t, &response, 0) else {
        panic!("the client refuses to connect without a ticket")
    };
    assert_eq!(reason, b"not signed in");
    assert!(server.is_empty());
}

#[test]
fn a_server_never_told_its_address_checks_no_tickets() {
    let secret = [9; 16];
    let mut server = secret_server(secret, server_addr(), KEY);
    server.config_mut().address = None;
    let reason = refused(join_client(
        &mut server,
        signing_client(5, secret, "ada", server_addr()),
    ));
    assert_eq!(
        reason,
        "this server cannot check sign-ins (it does not know its own address)"
    );
}

/// A provider that counts its verifications and accepts everyone.
#[derive(Debug, Default)]
struct Counting(std::sync::atomic::AtomicUsize);

impl AuthProvider for Counting {
    fn ticket(&self, _cx: &AuthContext) -> Result<Vec<u8>, String> {
        Ok(b"t".to_vec())
    }

    fn verify(
        &self,
        _from: SocketAddr,
        _cx: &AuthContext,
        _ticket: &[u8],
    ) -> Result<Vec<u8>, String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(b"who".to_vec())
    }
}

#[derive(Debug)]
struct Shared(Arc<Counting>);

impl AuthProvider for Shared {
    fn ticket(&self, cx: &AuthContext) -> Result<Vec<u8>, String> {
        self.0.ticket(cx)
    }

    fn verify(&self, from: SocketAddr, cx: &AuthContext, ticket: &[u8]) -> Result<Vec<u8>, String> {
        self.0.verify(from, cx, ticket)
    }
}

#[test]
fn a_request_the_server_would_drop_anyway_costs_no_verification() {
    let counter = Arc::new(Counting::default());
    let gate =
        ManifestGate::new(Manifest::new(12, 12)).with_auth(Arc::new(Shared(Arc::clone(&counter))));
    let mut config = ServerConfig::new(8);
    config.connect_requests_per_src_per_s = 100_000;
    config.pending_per_addr = 1;
    config.gate = Some(Arc::new(gate));
    let mut server = Server::new(config, KEY);
    let t = table();
    let handshake = |server: &mut Server, port: u16, seq: u32| {
        let addr = SocketAddr::from(([10, 0, 0, 7], port));
        let mut client = Client::new(seq, fields(None));
        let challenge = client.challenge_request();
        let Handled::Reply(response) = server.handle_datagram(&t, addr, &challenge, 0) else {
            panic!("challenge")
        };
        let ClientEvent::Send(connect) = client.on_datagram(&t, &response, 0) else {
            panic!("connect")
        };
        server.handle_datagram(&t, addr, &connect, 0)
    };
    // The first request from the address is verified and taken (it stays pending until its first game packet).
    let Handled::Request(request) = handshake(&mut server, 4001, 1) else {
        panic!("a request")
    };
    assert_eq!(counter.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(matches!(
        server.finish_request(&t, *request, Decision::Accept, 0),
        Finished::Accepted { .. }
    ));
    // A second from the same IP is over its pending limit: dropped before the gate, so the provider is not asked.
    for (port, seq) in [(4002, 2), (4003, 3), (4004, 4)] {
        assert!(matches!(
            handshake(&mut server, port, seq),
            Handled::Ignored(_)
        ));
    }
    assert_eq!(counter.0.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// A provider that refuses everyone with a reason longer than a reject can carry.
#[derive(Debug)]
struct Wordy;

impl AuthProvider for Wordy {
    fn ticket(&self, _cx: &AuthContext) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }

    fn verify(
        &self,
        _from: SocketAddr,
        _cx: &AuthContext,
        _ticket: &[u8],
    ) -> Result<Vec<u8>, String> {
        Err("no ".repeat(300))
    }
}

#[test]
fn a_long_refusal_is_cut_to_what_a_reject_carries() {
    let gate = ManifestGate::new(Manifest::new(12, 12)).with_auth(Arc::new(Wordy));
    let reason = refused(join(&mut server_with(Some(gate)), fields(None), 5));
    assert!(reason.len() <= 255);
    assert!(reason.starts_with("no no"));
}

// ---- fuzz --------------------------------------------------------------------------------------------------------------------

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

fn random_manifest(rng: &mut Rng) -> Manifest {
    let mut m = Manifest::new(rng.below(20) as u32, rng.below(20) as u32);
    for i in 0..rng.below(6) {
        m = m.with_plugin(format!("plugin.{}.{i}", rng.below(50)), rng.below(5) as u32);
    }
    let names = |rng: &mut Rng| -> Vec<String> {
        (0..rng.below(30))
            .map(|_| format!("Name{}", rng.below(60)))
            .collect()
    };
    m = m.with_ghosts(names(rng));
    m = m.with_datablocks(names(rng));
    m = m.with_channels(names(rng));
    for i in 0..rng.below(3) {
        m = m.with_mod(format!("mod{i}"), [rng.byte(); 32]);
    }
    m
}

fn mutate(rng: &mut Rng, original: &[u8]) -> Vec<u8> {
    let mut bytes = original.to_vec();
    match rng.below(6) {
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
        3 => bytes.extend((0..rng.below(40)).map(|_| rng.byte())),
        4 => bytes = (0..rng.below(200)).map(|_| rng.byte()).collect(),
        _ => {
            if !bytes.is_empty() {
                let at = rng.below(bytes.len());
                bytes[at] = 0xff;
            }
        }
    }
    bytes
}

#[test]
fn the_manifest_decoder_survives_a_hostile_stream() {
    let mut rng = Rng(0x6d61_6e69_6665_7374);
    let mut decoded = 0usize;
    for _ in 0..30_000 {
        let m = random_manifest(&mut rng);
        let Ok(bytes) = m.encode() else { continue };
        // What was written reads back.
        let back = Manifest::decode(&bytes).expect("a written manifest reads back");
        assert_eq!(back.plugins, m.plugins);
        assert!(back.ghosts.same_as(&m.ghosts));
        assert_eq!(back.mods, m.mods);
        // Every mutant is a value or an error, never a panic; a value is canonical (it encodes and reads back the same).
        for _ in 0..4 {
            let bad = mutate(&mut rng, &bytes);
            if let Ok(read) = Manifest::decode(&bad) {
                decoded += 1;
                let again = read.encode().expect("a decoded manifest encodes");
                assert_eq!(Manifest::decode(&again).expect("reads"), read);
            }
        }
    }
    assert!(decoded > 0, "some mutants are still valid manifests");
}

#[test]
fn the_block_decoder_survives_a_hostile_stream() {
    let mut rng = Rng(0x626c_6f63_6b21_2121);
    for _ in 0..30_000 {
        let block = ConnectBlock {
            manifest: Some(random_manifest(&mut rng)),
            ticket: (rng.below(2) == 0).then(|| (0..rng.below(40)).map(|_| rng.byte()).collect()),
            ..Default::default()
        };
        let mut w = BitWriter::new();
        if block.write(&mut w).is_err() {
            continue;
        }
        let bytes = w.into_bytes();
        assert_eq!(
            ConnectBlock::read(&mut BitReader::new(&bytes)).expect("reads"),
            Some(block)
        );
        for _ in 0..4 {
            let bad = mutate(&mut rng, &bytes);
            let mut r = BitReader::new(&bad);
            let _ = ConnectBlock::read(&mut r);
        }
    }
}

#[test]
fn hostile_requests_against_a_gated_server_never_panic_and_never_slip_through() {
    let mut rng = Rng(0x6761_7465_6761_7465);
    let t = table();
    let mut server = gated();
    let good = {
        let digest = challenge_digest(&KEY, from(), 5);
        torque_net::handshake::connect_request(
            &Default::default(),
            &t,
            5,
            digest,
            b"GameConnection",
            &fields(Some(block(manifest()))),
        )
    };
    for _ in 0..20_000 {
        let bad = mutate(&mut rng, &good);
        // Whatever the bytes, the answer is a reply, silence or a request for the host: and a request that reaches the host has
        // passed the gate, so its manifest is the server's.
        if let Handled::Request(request) = server.handle_datagram(&t, from(), &bad, 0) {
            let block = request
                .fields
                .block
                .expect("the gate saw a block or judged none");
            let ok = block.manifest.map(|m| manifest().mismatch(&m).is_none());
            assert_eq!(ok, Some(true));
        }
    }
}
