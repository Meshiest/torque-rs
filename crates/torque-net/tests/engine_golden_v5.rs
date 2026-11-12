//! Engine framing 5, pinned: the bytes of every datagram kind and control message, a packet with a section in every lane, and
//! the SHA-256 of every datagram of two scripted sessions (a validated link and a plain one through the gate), lossless.
//!
//! These bytes never change within a framing version: a change to engine wire bytes is a new framing version (`ENGINE_FRAMING`)
//! with a test and golden file named for it. The file was written once with `TORQUE_BLESS_GOLDEN=1 cargo test -p torque-net
//! --test engine_golden_v5`; blessing it again under the same version means the framing changed, which it must not.

mod engine_common;

use std::fmt::Write as _;

use engine_common::{Pair, accept, connect};
use sha2::{Digest, Sha256};
use torque_bytes::hex;
use torque_net::engine::{
    self as framing, Body, Control, Cookie, CookieValue, Disconnect, Fragment, Header, Hello,
    Reject, Unreliable, Versions,
};
use torque_net::engine::{Delivery, LinkFacts, RejectCode, StreamLane};

const GOLDEN_FILE: &str = "tests/golden/engine_v5.txt";
const GOLDEN: &str = include_str!("golden/engine_v5.txt");

fn messages() -> Vec<(&'static str, Vec<u8>)> {
    let cookie = CookieValue(*b"0123456789abcdef");
    let hello = Hello {
        versions: Versions::CURRENT,
        nonce: 0x0102_0304_0506_0708,
        game: "fixture".into(),
    };
    let mut opening = Vec::new();
    framing::Prefix::Opening {
        nonce: 0x0102_0304_0506_0708,
        cookie: Some(cookie),
    }
    .write(&mut opening);
    let mut control = Vec::new();
    Fragment {
        id: 0,
        total: 5,
        offset: 0,
        bytes: b"conn!",
    }
    .write(&mut control)
    .expect("fits");
    Body {
        header: Header {
            seq: 0,
            ack: 0,
            mask: 0,
        },
        sections: [&control, &[], &[], &[], &[], &[]],
    }
    .write(&mut opening, 64)
    .expect("a body");
    let mut opening_validated = Vec::new();
    framing::Prefix::Opening {
        nonce: 9,
        cookie: None,
    }
    .write(&mut opening_validated);
    Body {
        header: Header::default(),
        sections: [&control, &[], &[], &[], &[], &[]],
    }
    .write(&mut opening_validated, 0)
    .expect("a body");

    let mut rpc = Vec::new();
    Fragment {
        id: 7,
        total: 6,
        offset: 0,
        bytes: b"abc",
    }
    .write(&mut rpc)
    .expect("fits");
    Fragment {
        id: 7,
        total: 6,
        offset: 3,
        bytes: b"def",
    }
    .write(&mut rpc)
    .expect("fits");
    let mut unreliable = Vec::new();
    Unreliable {
        seq: 0x0203,
        bytes: b"puff",
    }
    .write(&mut unreliable)
    .expect("fits");
    let mut every_lane = Vec::new();
    framing::Prefix::Data.write(&mut every_lane);
    Body {
        header: Header {
            seq: 0x1234,
            ack: 0x1230,
            mask: 0x8000_0011,
        },
        sections: [
            &control,
            &rpc,
            &unreliable,
            &[0xaa, 0xbb],
            &[1, 2, 3],
            &[9; 4],
        ],
    }
    .write(&mut every_lane, 0)
    .expect("a body");

    let mut token_data = Vec::new();
    framing::Prefix::TokenData {
        token: 0x5e55_1011_cafe_f00d,
    }
    .write(&mut token_data);
    Body {
        header: Header {
            seq: 7,
            ack: 6,
            mask: 3,
        },
        sections: [&[], &[], &unreliable, &[], &[], &[]],
    }
    .write(&mut token_data, 0)
    .expect("a body");

    vec![
        ("hello", hello.encode().expect("a hello")),
        (
            "cookie",
            Cookie {
                version: framing::ENGINE_FRAMING,
                nonce: 0x0102_0304_0506_0708,
                cookie,
            }
            .encode(),
        ),
        ("opening-plain", opening),
        ("opening-validated", opening_validated),
        ("data-every-lane", every_lane),
        ("token-data", token_data),
        (
            "reject",
            Reject::new(
                0x0102_0304_0506_0708,
                RejectCode::Version,
                "server speaks network version 5, you have 4",
            )
            .encode()
            .expect("a reject"),
        ),
        (
            "disconnect",
            Disconnect::new(0x5e55_1011_cafe_f00d, "bye")
                .encode()
                .expect("a goodbye"),
        ),
        (
            "control-connect",
            Control::Connect(Box::new(connect()))
                .encode()
                .expect("a connect"),
        ),
        (
            "control-accept",
            Control::Accept(accept(1_200)).encode().expect("an accept"),
        ),
        (
            "control-challenge",
            Control::Challenge(0x0bad_cafe_f00d_5eed)
                .encode()
                .expect("a challenge"),
        ),
        (
            "control-ticket",
            Control::Ticket(b"ticket".to_vec())
                .encode()
                .expect("a ticket"),
        ),
    ]
}

/// Every datagram of a lossless session, both ways, hashed (direction byte, length, bytes), and how many there were.
fn session(facts: LinkFacts) -> (String, usize) {
    let mut p = Pair::new(facts, 0);
    p.client_streams.lanes = vec![StreamLane::Moves];
    p.server_streams.lanes = vec![StreamLane::Ghosts, StreamLane::Channels];
    assert!(p.connect(100));
    p.client
        .send(vec![0x11; 3_000], Delivery::Reliable)
        .expect("queued");
    p.server()
        .send(vec![0x22; 2_000], Delivery::Reliable)
        .expect("queued");
    p.client
        .send(vec![0x33; 100], Delivery::Unreliable)
        .expect("queued");
    for _ in 0..30 {
        p.tick();
    }
    let mut h = Sha256::new();
    for (dir, wire) in [(b'u', &p.up), (b'd', &p.down)] {
        for (d, _) in &wire.log {
            h.update([dir]);
            h.update((d.len() as u32).to_le_bytes());
            h.update(d);
        }
    }
    (hex(&h.finalize()), p.up.log.len() + p.down.log.len())
}

fn render() -> String {
    let mut out = String::new();
    for (name, bytes) in messages() {
        let _ = writeln!(out, "{name} {}", hex(&bytes));
    }
    for (name, facts) in [
        ("session-memory", LinkFacts::MEMORY),
        ("session-plain", LinkFacts::PLAIN),
    ] {
        let (digest, count) = session(facts);
        let _ = writeln!(out, "{name} {count} {digest}");
    }
    out
}

#[test]
fn engine_framing_5_bytes_are_pinned() {
    let now = render();
    if std::env::var_os("TORQUE_BLESS_GOLDEN").is_some() {
        std::fs::write(GOLDEN_FILE, &now).expect("write the golden");
        return;
    }
    let mut differ = Vec::new();
    for (want, got) in GOLDEN.lines().zip(now.lines()) {
        if want != got {
            differ.push(format!("want {want}\n got {got}"));
        }
    }
    assert_eq!(
        GOLDEN.lines().count(),
        now.lines().count(),
        "the golden has another set of entries"
    );
    assert!(
        differ.is_empty(),
        "{} entries differ:\n{}",
        differ.len(),
        differ.join("\n")
    );
}

/// Every pinned datagram reads back as what wrote it.
#[test]
fn the_pinned_datagrams_read_back() {
    for (name, bytes) in messages() {
        match name {
            "control-connect" | "control-accept" | "control-challenge" | "control-ticket" => {
                let back = Control::decode(&bytes).expect(name).encode().expect(name);
                assert_eq!(back, bytes, "{name}");
            }
            _ => {
                let d = framing::parse(&bytes).expect(name);
                if let framing::Datagram::Opening { body, .. }
                | framing::Datagram::Data { body, .. } = d
                {
                    Body::read(body).expect(name);
                }
            }
        }
    }
}
