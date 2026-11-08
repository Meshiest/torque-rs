use super::*;
use crate::handshake::{self, ConnectFields};
use crate::huffman::tests::sample_frequencies;
use crate::profile::HandshakeProfile;

fn table() -> Table {
    Table::from_frequencies(&sample_frequencies()).expect("a table")
}

/// Torque's profile: the client and the decoder default to it.
fn tp() -> HandshakeProfile {
    HandshakeProfile::default()
}

fn fields() -> ConnectFields {
    ConnectFields::standard(b"pw", vec![b"a".to_vec(), b"b".to_vec()])
}

#[test]
fn the_datagram_decoder_takes_every_handshake_datagram_apart() {
    let t = table();
    let digest = [9, 8, 7, 6];
    let f = fields();
    assert_eq!(
        decode_datagram(
            &t,
            &tp(),
            &handshake::HandshakeTypes::TORQUE.challenge_request(5)
        ),
        Ok(Decoded::ChallengeRequest { seq: 5 })
    );
    assert_eq!(
        decode_datagram(
            &t,
            &tp(),
            &handshake::HandshakeTypes::TORQUE.challenge_response(5, digest)
        ),
        Ok(Decoded::ChallengeResponse { seq: 5, digest })
    );
    let request = handshake::connect_request(
        &HandshakeProfile::default(),
        &t,
        5,
        digest,
        GAME_CONNECTION_CLASS,
        &f,
    );
    assert_eq!(
        decode_datagram(&t, &tp(), &request),
        Ok(Decoded::ConnectRequest(Box::new(ParsedRequest {
            seq: 5,
            digest,
            class: GAME_CONNECTION_CLASS.to_vec(),
            fields: f,
        })))
    );
    assert_eq!(
        decode_datagram(
            &t,
            &tp(),
            &handshake::HandshakeTypes::TORQUE.connect_reject(&t, 5, b"CHR_GAME")
        ),
        Ok(Decoded::Reject {
            seq: 5,
            reason: b"CHR_GAME".to_vec()
        })
    );
    assert_eq!(
        decode_datagram(
            &t,
            &tp(),
            &handshake::connect_accept(&tp(), 5, 12, 18, false)
        ),
        Ok(Decoded::Accept(Accept {
            seq: 5,
            protocol: 12,
            ghost_bits: None,
            extension: false,
            channels: false,
        }))
    );
    assert_eq!(
        decode_datagram(
            &t,
            &tp(),
            &handshake::HandshakeTypes::TORQUE.disconnect(&t, 5, b"bye")
        ),
        Ok(Decoded::Disconnect {
            seq: 5,
            reason: b"bye".to_vec()
        })
    );
    assert_eq!(
        decode_datagram(&t, &tp(), &[42, 0, 0]),
        Ok(Decoded::Other(42))
    );
    assert_eq!(decode_datagram(&t, &tp(), &[]), Err(NetError::Overrun));
}

#[test]
fn the_datagram_decoder_takes_game_packets_apart_with_nothing_left_but_padding() {
    let t = table();
    let mut p = Protocol::new(Role::Client, 3, RateConfig::new(32, 10, 450), 0);
    let mut w = p.begin_data_packet(0);
    let head = C2sHead {
        camera_flag: true,
        control_checksum: 0xcafe_f00d,
        move_start: 77,
        move_count: 0,
    };
    head.write(&mut w);
    write_fov(&mut w, Some(90));
    let bytes = w.into_bytes();
    let Ok(Decoded::Game {
        header,
        rate,
        payload_bit,
    }) = decode_datagram(&t, &tp(), &bytes)
    else {
        panic!("not a game packet")
    };
    assert_eq!(header.ptype, PacketType::Data);
    assert!(header.connect_bit);
    assert_eq!(
        rate,
        Some(RatePrefix {
            cur: None,
            max: Some((102, 450))
        })
    );
    let mut r = crate::protocol::reader_at(&bytes, payload_bit);
    assert_eq!(C2sHead::read(&mut r), Ok(head));
    assert_eq!(read_fov(&mut r), Ok(Some(90)));
    assert!(r.bits_left() < 8);

    // An ack packet has no rate prefix.
    let ack = p.ack_packet(1);
    let Ok(Decoded::Game { rate, .. }) = decode_datagram(&t, &tp(), &ack) else {
        panic!("not a game packet")
    };
    assert_eq!(rate, None);
}

#[test]
fn the_head_and_the_fov_round_trip() {
    for head in [
        C2sHead::default(),
        C2sHead {
            camera_flag: true,
            control_checksum: u32::MAX,
            move_start: u32::MAX,
            move_count: 31,
        },
    ] {
        let mut w = BitWriter::new();
        head.write(&mut w);
        assert_eq!(w.bits(), 1 + 32 + 32 + 5);
        let bytes = w.into_bytes();
        assert_eq!(C2sHead::read(&mut BitReader::new(&bytes)), Ok(head));
    }
    for fov in [None, Some(0), Some(255)] {
        let mut w = BitWriter::new();
        write_fov(&mut w, fov);
        let bytes = w.into_bytes();
        assert_eq!(read_fov(&mut BitReader::new(&bytes)), Ok(fov));
    }
}

#[test]
fn every_truncation_of_every_datagram_is_an_error_never_a_panic() {
    let t = table();
    let f = fields();
    let samples = [
        handshake::HandshakeTypes::TORQUE.challenge_request(5),
        handshake::HandshakeTypes::TORQUE.challenge_response(5, [1, 2, 3, 4]),
        handshake::connect_request(
            &HandshakeProfile::default(),
            &t,
            5,
            [1, 2, 3, 4],
            GAME_CONNECTION_CLASS,
            &f,
        ),
        handshake::HandshakeTypes::TORQUE.connect_reject(&t, 5, b"CHR_PASSWORD"),
        handshake::connect_accept(&tp(), 5, 12, 18, false),
        handshake::HandshakeTypes::TORQUE.disconnect(&t, 5, b"reason"),
    ];
    for sample in &samples {
        for len in 0..sample.len() {
            // A short datagram either fails or decodes as something shorter, and either way does not panic.
            let _ = decode_datagram(&t, &tp(), &sample[..len]);
        }
        // A bit-exact prefix of a request is not a request.
    }
    let request = &samples[2];
    for len in 0..request.len() - 1 {
        assert!(
            !matches!(
                decode_datagram(&t, &tp(), &request[..len]),
                Ok(Decoded::ConnectRequest(_))
            ),
            "{len} bytes"
        );
    }
}

#[test]
fn the_decoder_refuses_a_hostile_argument_count() {
    let t = table();
    let mut w = BitWriter::new();
    w.write_u8(handshake::HandshakeTypes::TORQUE.connect_request);
    w.write_u32(1);
    for _ in 0..4 {
        w.write_u32(0);
    }
    t.write(&mut w, GAME_CONNECTION_CLASS, 255);
    w.write_u32(0);
    w.write_u32(0xffff_ffff);
    t.write(&mut w, &tp().game, 255);
    w.write_u32(12);
    w.write_u32(12);
    t.write(&mut w, b"", 255);
    w.write_u32(1 << 30);
    let bytes = w.into_bytes();
    assert!(matches!(
        parse_connect_request(&t, &tp(), &bytes),
        Err(NetError::Cap {
            what: "connect arguments",
            ..
        })
    ));
}

#[test]
fn the_client_ignores_datagrams_out_of_state() {
    let t = table();
    let mut c = Client::new(5, fields());
    // Nothing is expected yet.
    assert!(matches!(
        c.on_datagram(
            &t,
            &handshake::HandshakeTypes::TORQUE.challenge_response(5, [1, 2, 3, 4]),
            0
        ),
        ClientEvent::Nothing
    ));
    assert!(matches!(
        c.on_datagram(&t, &handshake::connect_accept(&tp(), 5, 12, 18, false), 0),
        ClientEvent::Nothing
    ));
    c.challenge_request();
    // A response for another sequence is ignored; the right one is answered.
    assert!(matches!(
        c.on_datagram(
            &t,
            &handshake::HandshakeTypes::TORQUE.challenge_response(6, [1, 2, 3, 4]),
            0
        ),
        ClientEvent::Nothing
    ));
    let ClientEvent::Send(bytes) = c.on_datagram(
        &t,
        &handshake::HandshakeTypes::TORQUE.challenge_response(5, [1, 2, 3, 4]),
        0,
    ) else {
        panic!("no connect request")
    };
    let Ok(Decoded::ConnectRequest(req)) = decode_datagram(&t, &tp(), &bytes) else {
        panic!("not a request")
    };
    assert_eq!(req.digest, [1, 2, 3, 4]);
    // A game packet before the accept is nothing.
    assert!(matches!(
        c.on_datagram(&t, &[0xff, 0xff, 0xff], 0),
        ClientEvent::Nothing
    ));
    // A reject ends the attempt.
    let reject = handshake::HandshakeTypes::TORQUE.connect_reject(&t, 5, b"CHR_GAME");
    assert!(matches!(c.on_datagram(&t, &reject, 0), ClientEvent::Rejected(r) if r == b"CHR_GAME"));
    assert_eq!(c.state(), &ClientState::Rejected(b"CHR_GAME".to_vec()));
}

#[test]
fn a_hostile_class_name_goes_out_as_told() {
    let t = table();
    let mut c = Client::new(5, fields()).with_class(b"SimObject");
    let bytes = c.connect_request(&t, Some([1, 2, 3, 4]));
    let Ok(Decoded::ConnectRequest(req)) = decode_datagram(&t, &tp(), &bytes) else {
        panic!("not a request")
    };
    assert_eq!(req.class, b"SimObject");
}

/// A Disconnect written into a global packet stream that still held a string buffer: `writeString`'s flag bit before the text.
/// A reader drops the connection whatever the reason reads as; this one reads the reason the writer meant.
#[test]
fn a_disconnect_written_with_a_stale_string_buffer_still_drops_the_connection() {
    let t = table();
    let mut w = BitWriter::new();
    w.write_u8(handshake::HandshakeTypes::TORQUE.disconnect);
    w.write_u32(5);
    // The string buffer shared fewer than three bytes with the reason: the flag is 0, then the reason as usual.
    w.write_flag(false);
    t.write(&mut w, b"The server is closing.", 255);
    let stale = w.into_bytes();
    assert_eq!(
        handshake::HandshakeTypes::TORQUE
            .parse_disconnect(&t, &stale)
            .unwrap(),
        (5, b"The server is closing.".to_vec())
    );
    // The plain form still reads as it is written.
    assert_eq!(
        handshake::HandshakeTypes::TORQUE
            .parse_disconnect(
                &t,
                &handshake::HandshakeTypes::TORQUE.disconnect(&t, 5, b"TimedOut")
            )
            .unwrap(),
        (5, b"TimedOut".to_vec())
    );
    // A reason that reads as nothing at all still drops the connection: only the type and the sequence are needed.
    let mut w = BitWriter::new();
    w.write_u8(handshake::HandshakeTypes::TORQUE.disconnect);
    w.write_u32(5);
    let bare = w.into_bytes();
    assert_eq!(
        handshake::HandshakeTypes::TORQUE
            .parse_disconnect(&t, &bare)
            .unwrap(),
        (5, Vec::new())
    );
}
