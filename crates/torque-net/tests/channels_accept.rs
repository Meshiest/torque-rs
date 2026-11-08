//! The accept of a server with bulk channels, byte for byte: Torque's own accept for a peer without channels, the channels word
//! only for a client whose channels the gate matched, the same bytes when the accept is asked for again, and nothing when the
//! host has no channels layer or the gate does not compare channels.

use std::net::SocketAddr;
use std::sync::Arc;

use torque_net::client::{Client, ClientEvent};
use torque_net::connect::{AuthContext, ConnectBlock, ConnectGate, ManifestGate};
use torque_net::handshake::{
    CHANNELS_ACCEPT_WORD, ConnectFields, Decision, Finished, Handled, Server, ServerConfig,
    parse_accept,
};
use torque_net::huffman::Table;
use torque_net::manifest::Manifest;
use torque_net::profile::HandshakeProfile;

fn table() -> Table {
    let mut f = [0u32; 256];
    for (i, slot) in f.iter_mut().enumerate().take(127) {
        *slot = match i as u8 {
            b'a'..=b'z' => 300 + (i as u32 * 37) % 900,
            b' ' => 2800,
            _ => (i as u32 * 3) % 5,
        };
    }
    Table::from_frequencies(&f).expect("a table")
}

fn from() -> SocketAddr {
    SocketAddr::from(([10, 0, 0, 7], 4000))
}

fn with_channels() -> Manifest {
    Manifest::new(12, 12).with_channels(["terrain"])
}

fn without_channels() -> Manifest {
    Manifest::new(12, 12)
}

/// A gate that accepts what the built-in one does but never says channels match (a gate that does not compare them).
#[derive(Debug)]
struct Blind(ManifestGate);

impl ConnectGate for Blind {
    fn check(
        &self,
        from: SocketAddr,
        cx: &AuthContext,
        fields: &ConnectFields,
    ) -> Result<Option<Vec<u8>>, Vec<u8>> {
        self.0.check(from, cx, fields)
    }
}

fn server(manifest: Manifest, channels: bool, blind: bool) -> Server {
    let mut config = ServerConfig::new(8);
    config.connect_requests_per_src_per_s = 100_000;
    config.channels = channels;
    let gate = ManifestGate::new(manifest);
    config.gate = Some(if blind {
        Arc::new(Blind(gate))
    } else {
        Arc::new(gate)
    });
    Server::new(config, [7; 16])
}

fn fields(manifest: Option<Manifest>) -> ConnectFields {
    let mut f = ConnectFields::standard(b"", vec![b"Name".to_vec()]);
    f.block = manifest.map(|m| {
        Box::new(ConnectBlock {
            manifest: Some(m),
            ticket: None,
        })
    });
    f
}

/// The connect request datagram of a client with `fields`, after the challenge.
fn connect_request(server: &mut Server, fields: ConnectFields, seq: u32) -> (Client, Vec<u8>) {
    let t = table();
    let mut client = Client::new(seq, fields);
    let challenge = client.challenge_request();
    let Handled::Reply(response) = server.handle_datagram(&t, from(), &challenge, 0) else {
        panic!("no challenge response")
    };
    let ClientEvent::Send(connect) = client.on_datagram(&t, &response, 0) else {
        panic!("no connect request")
    };
    (client, connect)
}

/// The accept the server sends for the request, and what the client makes of it.
fn accept(server: &mut Server, fields: ConnectFields) -> (Vec<u8>, bool) {
    let t = table();
    let (mut client, connect) = connect_request(server, fields, 5);
    let Handled::Request(request) = server.handle_datagram(&t, from(), &connect, 0) else {
        panic!("not a request")
    };
    let Finished::Accepted { reply, .. } = server.finish_request(&t, *request, Decision::Accept, 0)
    else {
        panic!("not accepted")
    };
    let ClientEvent::Accepted(accepted) = client.on_datagram(&t, &reply, 0) else {
        panic!("the client does not read an accept")
    };
    (reply, accepted.channels)
}

/// Torque's accept for sequence 5 and protocol 12: the type (36, ConnectAccept), the sequence and the protocol, little-endian words.
const TGE_ACCEPT: [u8; 9] = [36, 5, 0, 0, 0, 12, 0, 0, 0];

#[test]
fn a_gated_server_answers_a_peer_without_channels_with_exactly_torques_accept() {
    // A plain request (judged as a peer with nothing) and a manifest without channels, to servers that have none.
    let mut s = server(without_channels(), true, false);
    let (reply, channels) = accept(&mut s, fields(None));
    assert_eq!(reply, TGE_ACCEPT, "a plain request");
    assert!(!channels);
    let mut s = server(without_channels(), true, false);
    let (reply, channels) = accept(&mut s, fields(Some(without_channels())));
    assert_eq!(reply, TGE_ACCEPT, "a manifest without channels");
    assert!(!channels);
}

#[test]
fn only_a_manifest_with_channels_gets_the_word_and_a_repeated_request_gets_the_same_accept() {
    let t = table();
    let mut s = server(with_channels(), true, false);
    let (mut client, connect) = connect_request(&mut s, fields(Some(with_channels())), 5);
    let Handled::Request(request) = s.handle_datagram(&t, from(), &connect, 0) else {
        panic!("not a request")
    };
    let Finished::Accepted { reply, .. } = s.finish_request(&t, *request, Decision::Accept, 0)
    else {
        panic!("not accepted")
    };
    let mut expect = TGE_ACCEPT.to_vec();
    expect.extend_from_slice(&CHANNELS_ACCEPT_WORD.to_le_bytes());
    assert_eq!(reply, expect);
    let ClientEvent::Accepted(a) = client.on_datagram(&t, &reply, 0) else {
        panic!("no accept")
    };
    assert!(a.channels);
    // The accept was lost: the client asks again with the same sequence, and the answer is the same bytes, word and all.
    let Handled::Reply(again) = s.handle_datagram(&t, from(), &connect, 1) else {
        panic!("no accept again")
    };
    assert_eq!(again, reply);
}

#[test]
fn no_channels_layer_or_a_gate_that_does_not_compare_channels_means_no_word() {
    // The host has no channels layer.
    let mut s = server(with_channels(), false, false);
    let (reply, channels) = accept(&mut s, fields(Some(with_channels())));
    assert_eq!(reply, TGE_ACCEPT);
    assert!(!channels);
    // A gate that never says the channels match.
    let mut s = server(with_channels(), true, true);
    let (reply, channels) = accept(&mut s, fields(Some(with_channels())));
    assert_eq!(reply, TGE_ACCEPT);
    assert!(!channels);
}

#[test]
fn a_truncated_or_foreign_tail_is_not_the_word() {
    let p = HandshakeProfile::default();
    let full = {
        let mut v = TGE_ACCEPT.to_vec();
        v.extend_from_slice(&CHANNELS_ACCEPT_WORD.to_le_bytes());
        v
    };
    assert!(parse_accept(&p, &full).unwrap().channels);
    for cut in 1..=3 {
        let a = parse_accept(&p, &full[..full.len() - cut]).unwrap();
        assert!(!a.channels, "cut by {cut}");
    }
    let mut other = TGE_ACCEPT.to_vec();
    other.extend_from_slice(&[1, 2, 3, 4]);
    assert!(!parse_accept(&p, &other).unwrap().channels);
}
