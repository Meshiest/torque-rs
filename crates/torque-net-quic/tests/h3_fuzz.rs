//! Hostile HTTP/3 at both ends of a WebTransport session: the server's and the client's session state machines fed random
//! bytes, bit-flipped and cut copies of a real exchange, over-long frames and field sections, streams of every type in every
//! order and split at every byte. Nothing panics; the bytes held for a stream never pass the cap; a session is established
//! only by a CONNECT that judges well; a server's answer is never more than a few hundred bytes to any byte sequence; and
//! the QPACK and Huffman decoders take everything without a panic.
//!
//! `TORQUE_FUZZ_H3=<n> cargo test -p torque-net-quic --test main h3_fuzz::` runs `n` rounds (10,000 by default).

use crate::common;

use common::Rng;
use quinn_proto::{Dir, Side, StreamId};
use torque_net_quic::h3::session::{Action, End, Origins, Session, Setup};
use torque_net_quic::h3::{Dial, MAX_STREAM_BUFFER, frames, huffman, qpack};

fn rounds() -> usize {
    std::env::var("TORQUE_FUZZ_H3")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000)
}

fn uni(side: Side, index: u64) -> StreamId {
    StreamId::new(side, Dir::Uni, index)
}

fn bi(side: Side, index: u64) -> StreamId {
    StreamId::new(side, Dir::Bi, index)
}

fn setup() -> Setup {
    Setup {
        origins: Origins::List(vec!["https://good.example".into()]),
        path: "/".into(),
        hosts: Vec::new(),
    }
}

/// The bytes a well-behaved client puts on its control stream and its request stream.
fn good_client() -> (Vec<u8>, Vec<u8>) {
    let mut control = vec![0x00];
    frames::put_frame(
        &mut control,
        frames::frame::SETTINGS,
        &frames::put_settings(&[
            (frames::setting::ENABLE_CONNECT_PROTOCOL, 1),
            (frames::setting::H3_DATAGRAM, 1),
            (frames::setting::WT_MAX_SESSIONS, 1),
        ]),
    );
    let mut request = Vec::new();
    frames::put_frame(
        &mut request,
        frames::frame::HEADERS,
        &qpack::encode(&[
            (":method", "CONNECT"),
            (":protocol", "webtransport"),
            (":scheme", "https"),
            (":authority", "10.0.0.1:28000"),
            (":path", "/"),
            ("origin", "https://good.example"),
        ]),
    );
    (control, request)
}

/// The bytes a well-behaved server puts on its control stream and its answer.
fn good_server() -> (Vec<u8>, Vec<u8>) {
    let mut control = vec![0x00];
    frames::put_frame(
        &mut control,
        frames::frame::SETTINGS,
        &frames::put_settings(&[
            (frames::setting::ENABLE_CONNECT_PROTOCOL, 1),
            (frames::setting::H3_DATAGRAM, 1),
            (frames::setting::ENABLE_WEBTRANSPORT, 1),
        ]),
    );
    let mut answer = Vec::new();
    frames::put_frame(
        &mut answer,
        frames::frame::HEADERS,
        &qpack::encode(&[(":status", "200")]),
    );
    (control, answer)
}

/// `bytes` mangled one way or another: flips, cuts, inserts, repeats, or replaced by noise.
fn mangle(rng: &mut Rng, bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    match rng.below(8) {
        0 => {
            for _ in 0..=rng.below(4) {
                if !out.is_empty() {
                    let i = rng.below(out.len() as u64) as usize;
                    out[i] ^= 1 << rng.below(8);
                }
            }
        }
        1 => {
            let cut = rng.below(out.len() as u64 + 1) as usize;
            out.truncate(cut);
        }
        2 => {
            let at = rng.below(out.len() as u64 + 1) as usize;
            let junk = {
                let n = rng.below(16) as usize + 1;
                rng.bytes(n)
            };
            out.splice(at..at, junk);
        }
        3 => {
            let copy = out.clone();
            out.extend(copy);
        }
        4 => {
            out = {
                let n = rng.below(64) as usize;
                rng.bytes(n)
            }
        }
        5 => {
            // A frame that claims far more than it brings.
            let mut long = Vec::new();
            frames::put_varint(&mut long, rng.below(0x80));
            frames::put_varint(&mut long, 1 << rng.below(62));
            out.splice(..0, long);
        }
        6 => {
            // A header block of many fields, some over the caps.
            let n = rng.below(48) as usize;
            let values: Vec<String> = (0..n).map(|i| "v".repeat(1 + (i * 97) % 1200)).collect();
            let fields: Vec<(&str, &str)> = values.iter().map(|v| ("x-f", v.as_str())).collect();
            let mut block = Vec::new();
            frames::put_frame(&mut block, frames::frame::HEADERS, &qpack::encode(&fields));
            out = block;
        }
        _ => {}
    }
    out
}

/// Whether the first frame of a request stream's bytes is a HEADERS frame whose CONNECT the setup takes.
fn judges_well(data: &[u8]) -> bool {
    let frames::FrameRead::Frame { kind, payload, .. } = frames::read_frame(data) else {
        return false;
    };
    kind == frames::frame::HEADERS
        && qpack::decode(payload)
            .is_ok_and(|fields| torque_net_quic::h3::judge(&fields, &setup()).is_ok())
}

/// Feeds `data` to `session` on `id` in pieces of random size.
fn feed(rng: &mut Rng, session: &mut Session, id: StreamId, data: &[u8], fin: bool) {
    let mut at = 0;
    while at < data.len() {
        let n = (rng.below(32) as usize + 1).min(data.len() - at);
        session.data(id, &data[at..at + n], false);
        at += n;
    }
    if fin {
        session.data(id, &[], true);
    }
}

/// Drains `session`'s actions; the bytes it wrote, and whether it closed the connection.
fn drain(session: &mut Session) -> (usize, bool) {
    let mut wrote = 0;
    let mut closed = false;
    while let Some(action) = session.poll() {
        match action {
            Action::Write { data, .. } => wrote += data.len(),
            Action::Close { .. } => closed = true,
            Action::Finish(_) | Action::Stop { .. } => {}
        }
    }
    (wrote, closed)
}

#[test]
fn hostile_h3_never_panics_and_the_caps_hold() {
    let rounds = rounds();
    let mut rng = Rng::new(0x4833);
    let (good_control, good_request) = good_client();
    let (server_control, server_answer) = good_server();
    let mut established = 0u64;
    let mut closed = 0u64;
    let mut client_established = 0u64;
    let mut most_written = 0usize;
    for round in 0..rounds {
        // The server fed a client's streams in a random order, some of them mangled.
        let mut server = Session::server(setup());
        server.local_streams(
            uni(Side::Server, 0),
            uni(Side::Server, 1),
            uni(Side::Server, 2),
        );
        let (hello, _) = drain(&mut server);
        assert!(hello < 128, "the server's opening is {hello} bytes");
        let mut streams: Vec<(StreamId, Vec<u8>, bool)> = Vec::new();
        let n_uni = rng.below(5) as usize;
        for i in 0..n_uni {
            let data = match rng.below(6) {
                0 => good_control.clone(),
                1 => mangle(&mut rng, &good_control),
                2 => vec![0x02],
                3 => vec![0x03, 0x40, 0x41],
                4 => {
                    let mut d = vec![0x54];
                    d.extend({
                        let n = rng.below(40) as usize;
                        rng.bytes(n)
                    });
                    d
                }
                _ => {
                    let n = rng.below(24) as usize;
                    rng.bytes(n)
                }
            };
            streams.push((uni(Side::Client, i as u64), data, rng.percent(20)));
        }
        let n_bi = rng.below(3) as usize;
        for i in 0..n_bi {
            let data = match rng.below(4) {
                0 => good_request.clone(),
                1 | 2 => mangle(&mut rng, &good_request),
                _ => {
                    let n = rng.below(64) as usize;
                    rng.bytes(n)
                }
            };
            streams.push((bi(Side::Client, i as u64), data, rng.percent(20)));
        }
        // Register, then feed in a shuffled order, interleaving the streams.
        for (id, _, _) in &streams {
            server.peer_stream(*id, id.dir() == Dir::Bi);
        }
        let mut cursors: Vec<usize> = vec![0; streams.len()];
        let mut left = streams.len();
        while left > 0 {
            let i = rng.below(streams.len() as u64) as usize;
            let (id, data, fin) = &streams[i];
            if cursors[i] > data.len() {
                continue;
            }
            if cursors[i] == data.len() {
                if *fin {
                    server.data(*id, &[], true);
                }
                cursors[i] = data.len() + 1;
                left -= 1;
                continue;
            }
            let n = (rng.below(24) as usize + 1).min(data.len() - cursors[i]);
            server.data(*id, &data[cursors[i]..cursors[i] + n], false);
            cursors[i] += n;
            assert!(
                server.buffered() <= MAX_STREAM_BUFFER * 4,
                "round {round}: {} bytes held",
                server.buffered()
            );
        }
        let (wrote, did_close) = drain(&mut server);
        most_written = most_written.max(wrote);
        assert!(
            wrote <= 512,
            "round {round}: the server wrote {wrote} bytes"
        );
        if did_close {
            closed += 1;
            assert!(matches!(server.end(), Some(End::Protocol(_))));
            assert_eq!(server.established(), None);
        }
        if server.established().is_some() {
            established += 1;
            // Only a CONNECT that judges well establishes anything (a mangled one may still judge well: a flip in the
            // authority, or the request repeated).
            let had_good = streams
                .iter()
                .any(|(id, d, _)| id.dir() == Dir::Bi && judges_well(d));
            assert!(
                had_good,
                "round {round}: established without a good CONNECT"
            );
        }
        // The client fed a server's streams, mangled.
        let mut client = Session::client(Dial {
            authority: "10.0.0.1:28000".into(),
            path: "/".into(),
            origin: None,
        });
        client.local_streams(
            uni(Side::Client, 0),
            uni(Side::Client, 1),
            uni(Side::Client, 2),
        );
        client.request_stream(bi(Side::Client, 0));
        drain(&mut client);
        let control = if rng.percent(50) {
            server_control.clone()
        } else {
            mangle(&mut rng, &server_control)
        };
        let answer = if rng.percent(50) {
            server_answer.clone()
        } else {
            mangle(&mut rng, &server_answer)
        };
        client.peer_stream(uni(Side::Server, 0), false);
        let (control_fin, answer_fin) = (rng.percent(10), rng.percent(30));
        if rng.percent(50) {
            feed(
                &mut rng,
                &mut client,
                uni(Side::Server, 0),
                &control,
                control_fin,
            );
            drain(&mut client);
            feed(
                &mut rng,
                &mut client,
                bi(Side::Client, 0),
                &answer,
                answer_fin,
            );
        } else {
            feed(
                &mut rng,
                &mut client,
                bi(Side::Client, 0),
                &answer,
                answer_fin,
            );
            drain(&mut client);
            feed(
                &mut rng,
                &mut client,
                uni(Side::Server, 0),
                &control,
                control_fin,
            );
        }
        if rng.percent(30) {
            client.peer_stream(uni(Side::Server, 1), false);
            let n = rng.below(32) as usize;
            let junk = rng.bytes(n);
            feed(&mut rng, &mut client, uni(Side::Server, 1), &junk, false);
        }
        let (wrote, _) = drain(&mut client);
        assert!(
            wrote <= 512,
            "round {round}: the client wrote {wrote} bytes"
        );
        assert!(client.buffered() <= MAX_STREAM_BUFFER * 4);
        if client.established().is_some() {
            client_established += 1;
        }
        // The decoders alone, on noise and on mangled valid input.
        let noise = {
            let n = rng.below(96) as usize;
            rng.bytes(n)
        };
        let _ = qpack::decode(&noise);
        let _ = qpack::decode(&mangle(
            &mut rng,
            &qpack::encode(&[(":status", "200"), ("origin", "https://x")]),
        ));
        let mut out = Vec::new();
        let _ = huffman::decode(&noise, 64, &mut out);
        let _ = frames::read_settings(&noise);
        let _ = frames::read_capsules(&noise);
        let _ = frames::read_frame(&noise);
    }
    assert!(established > 0, "some rounds establish a session");
    assert!(closed > 0, "some rounds close the connection");
    assert!(client_established > 0);
    println!(
        "{rounds} rounds: {established} server sessions established, {closed} connections closed, {client_established} client sessions, the most a server wrote {most_written} bytes"
    );
}
