//! Torque's wire, pinned: five scripted sessions between the handshake server and the client over a lossless in-memory link, every
//! datagram of each recorded. A session's golden is the SHA-256 of the datagrams each way (each datagram length-prefixed, in order)
//! and the first eight datagrams as hex. A change that alters one bit torque-net writes for a peer fails here and names the session.
//!
//! The sessions: a peer that sends nothing of its own (a plain request, as Torque's own clients send it); a peer that sends a
//! manifest and a ticket and takes the bulk channels; the same with the server in a burst, as for a held download; a refusal by the
//! manifest; a refusal by the ticket. Each session that is accepted goes on to trade data packets, so the header, the acks and the
//! rate prefix are pinned as well.
//!
//! The goldens are in `golden/tge_sessions.txt`. After a change that is meant to alter the wire,
//! `TORQUE_BLESS_GOLDEN=1 cargo test -p torque-net --test tge_golden` writes the file again.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use torque_bytes::hex;
use torque_net::bits::BitWriter;
use torque_net::client::{Client, ClientEvent};
use torque_net::connect::{AuthProvider, BlockSource, ConnectBlock, ManifestGate, SharedSecret};
use torque_net::handshake::{
    ConnId, ConnectFields, Decision, Finished, Handled, Server, ServerConfig,
};
use torque_net::huffman::Table;
use torque_net::manifest::Manifest;

const GOLDEN_FILE: &str = "tests/golden/tge_sessions.txt";
const GOLDEN: &str = include_str!("golden/tge_sessions.txt");
const SECRET: [u8; 16] = [9; 16];
const KEY: [u8; 16] = [7; 16];
/// Datagrams kept verbatim.
const VERBATIM: usize = 8;
/// Rounds of data packets after an accept.
const ROUNDS: u32 = 24;

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

fn peer() -> SocketAddr {
    SocketAddr::from(([10, 0, 0, 7], 4000))
}

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

/// Every datagram of a session, in the order sent, and which way (true: to the server).
#[derive(Default)]
struct Recording {
    datagrams: Vec<(bool, Vec<u8>)>,
}

impl Recording {
    fn up(&mut self, bytes: &[u8]) {
        self.datagrams.push((true, bytes.to_vec()));
    }

    fn down(&mut self, bytes: &[u8]) {
        self.datagrams.push((false, bytes.to_vec()));
    }

    fn digest(&self, up: bool) -> String {
        let mut h = Sha256::new();
        for (_, bytes) in self.datagrams.iter().filter(|(d, _)| *d == up) {
            h.update((bytes.len() as u32).to_le_bytes());
            h.update(bytes);
        }
        hex(&h.finalize())
    }
}

/// A server that holds clients to `manifest()`, takes the bulk channels and verifies tickets under `SECRET`.
fn gated_server() -> Server {
    let gate = ManifestGate::new(manifest()).with_auth(Arc::new(SharedSecret::new(SECRET, "")));
    let mut config = ServerConfig::new(8);
    config.connect_requests_per_src_per_s = 100_000;
    config.gate = Some(Arc::new(gate));
    config.address = Some(server_addr());
    config.channels = true;
    Server::new(config, KEY)
}

fn plain_server() -> Server {
    let mut config = ServerConfig::new(8);
    config.connect_requests_per_src_per_s = 100_000;
    Server::new(config, KEY)
}

fn plain_client() -> Client {
    Client::new(5, ConnectFields::standard(b"", vec![b"Name".to_vec()]))
}

/// A client that sends `manifest` and a ticket under `key`, for the server at `server_addr()`.
fn engine_client(manifest: Manifest, key: [u8; 16]) -> Client {
    let mut client = Client::new(5, ConnectFields::standard(b"", vec![b"Name".to_vec()]));
    client.set_block_source(
        server_addr(),
        BlockSource::new(move |cx| {
            Ok(Some(ConnectBlock {
                manifest: Some(manifest.clone()),
                ticket: Some(SharedSecret::new(key, "ada").ticket(cx)?),
                ..Default::default()
            }))
        }),
    );
    client
}

/// `n` bytes of payload for `round`, whatever they are, the same every run.
fn payload(n: usize, round: u32, salt: u32) -> Vec<u8> {
    (0..n as u32)
        .map(|i| i.wrapping_mul(31).wrapping_add(round * 7 + salt) as u8)
        .collect()
}

/// Runs the handshake and, when the server accepts, `ROUNDS` rounds of one data packet each way (the server's as large as its
/// packet layer lets it when `burst`).
fn play(mut server: Server, mut client: Client, burst: bool) -> Recording {
    let t = table();
    let mut wire = Recording::default();
    let challenge = client.challenge_request();
    wire.up(&challenge);
    let Handled::Reply(response) = server.handle_datagram(&t, peer(), &challenge, 0) else {
        panic!("no challenge response")
    };
    wire.down(&response);
    let ClientEvent::Send(connect) = client.on_datagram(&t, &response, 0) else {
        panic!("no connect request")
    };
    wire.up(&connect);
    let id: ConnId = match server.handle_datagram(&t, peer(), &connect, 0) {
        Handled::Request(request) => {
            let Finished::Accepted { conn, reply, .. } =
                server.finish_request(&t, *request, Decision::Accept, 0)
            else {
                panic!("not accepted")
            };
            wire.down(&reply);
            assert!(matches!(
                client.on_datagram(&t, &reply, 0),
                ClientEvent::Accepted(_)
            ));
            conn
        }
        Handled::Reply(reject) => {
            wire.down(&reject);
            assert!(matches!(
                client.on_datagram(&t, &reject, 0),
                ClientEvent::Rejected(_)
            ));
            return wire;
        }
        other => panic!("unexpected {other:?}"),
    };
    if burst {
        server
            .connection_mut(id)
            .expect("the connection")
            .protocol
            .set_burst(true);
    }
    for round in 1..=ROUNDS {
        let now = round * 32;
        if let Some(mut w) = client.begin_data_packet(now, true) {
            w.write_bytes(&payload(40, round, 1));
            let bytes = w.into_bytes();
            wire.up(&bytes);
            assert!(matches!(
                server.handle_datagram(&t, peer(), &bytes, now),
                Handled::Game { .. }
            ));
        }
        let conn = server.connection_mut(id).expect("the connection");
        if conn.protocol.may_send(now, true) {
            let want = if burst {
                (conn.protocol.packet_limit_bits() / 8).saturating_sub(24)
            } else {
                40
            };
            let mut w: BitWriter = conn.protocol.begin_data_packet(now);
            w.write_bytes(&payload(want, round, 2));
            let bytes = w.into_bytes();
            wire.down(&bytes);
            assert!(matches!(
                client.on_datagram(&t, &bytes, now),
                ClientEvent::Game { .. }
            ));
        }
        // A bare ack and a bare ping now and then.
        if round == 10 {
            let ack = client.protocol_mut().expect("protocol").ack_packet(now);
            wire.up(&ack);
            server.handle_datagram(&t, peer(), &ack, now);
        }
        if round == 12 {
            let ping = server
                .connection_mut(id)
                .expect("the connection")
                .protocol
                .ping_packet(now);
            wire.down(&ping);
            client.on_datagram(&t, &ping, now);
        }
    }
    wire
}

fn sessions() -> Vec<(&'static str, Recording)> {
    let other = manifest().with_plugin("torque.other", 1);
    vec![
        ("plain", play(plain_server(), plain_client(), false)),
        (
            "gated",
            play(gated_server(), engine_client(manifest(), SECRET), false),
        ),
        (
            "burst",
            play(gated_server(), engine_client(manifest(), SECRET), true),
        ),
        (
            "manifest_reject",
            play(gated_server(), engine_client(other, SECRET), false),
        ),
        (
            "ticket_reject",
            play(gated_server(), engine_client(manifest(), [1; 16]), false),
        ),
    ]
}

/// The golden file's text for the sessions: the count and a digest line for each way, and a line for each of the first datagrams.
fn render(sessions: &[(&str, Recording)]) -> String {
    let mut out = String::new();
    for (name, wire) in sessions {
        let _ = writeln!(out, "{name} datagrams {}", wire.datagrams.len());
        let _ = writeln!(out, "{name} to-server {}", wire.digest(true));
        let _ = writeln!(out, "{name} to-client {}", wire.digest(false));
        for (i, (up, bytes)) in wire.datagrams.iter().take(VERBATIM).enumerate() {
            let way = if *up { "to-server" } else { "to-client" };
            let _ = writeln!(out, "{name} {i} {way} {}", hex(bytes));
        }
    }
    out
}

#[test]
fn the_wire_is_the_bytes_it_was() {
    let sessions = sessions();
    let text = render(&sessions);
    if std::env::var_os("TORQUE_BLESS_GOLDEN").is_some() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN_FILE);
        std::fs::write(path, &text).expect("writes the goldens");
        return;
    }
    let mut differing = Vec::new();
    for (name, _) in &sessions {
        let pick = |s: &str| {
            s.lines()
                .filter(|l| l.split(' ').next() == Some(name))
                .collect::<Vec<_>>()
                .join("\n")
        };
        if pick(&text) != pick(GOLDEN) {
            differing.push(*name);
        }
    }
    assert!(
        differing.is_empty(),
        "sessions whose bytes differ from the goldens: {differing:?}\n--- now\n{text}"
    );
    assert_eq!(text.lines().count(), GOLDEN.lines().count());
}

#[test]
fn the_sessions_do_what_they_say() {
    let by_name: std::collections::BTreeMap<_, _> = sessions().into_iter().collect();
    // A refused session ends with the answer to the connect request; an accepted one trades packets.
    assert_eq!(by_name["manifest_reject"].datagrams.len(), 4);
    assert_eq!(by_name["ticket_reject"].datagrams.len(), 4);
    assert!(by_name["plain"].datagrams.len() > 40);
    let biggest = |name: &str| {
        by_name[name]
            .datagrams
            .iter()
            .filter(|(up, _)| !*up)
            .map(|(_, b)| b.len())
            .max()
            .unwrap()
    };
    assert!(biggest("burst") > 1_000, "{}", biggest("burst"));
    assert!(biggest("gated") < 200, "{}", biggest("gated"));
    // A plain request is answered with Torque's own accept.
    assert_eq!(by_name["plain"].datagrams[3].1.len(), 9);
}
