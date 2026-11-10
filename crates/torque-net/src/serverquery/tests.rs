use super::*;
use crate::huffman::tests::sample_frequencies;
use std::net::Ipv4Addr;

fn table() -> Table {
    Table::from_frequencies(&sample_frequencies()).expect("a table")
}

/// A game's version fields: the ping answer carries the profile's, not Torque's.
fn prof() -> QueryProfile {
    QueryProfile {
        version_string: b"VER1".to_vec(),
        protocol_current: 14,
        protocol_min: 12,
        version: 1800,
        info_type_max: GAME_HEARTBEAT,
        request_bytes: 0,
    }
}

fn req(kind: Kind, flags: u8, key: u32) -> Request {
    Request { kind, flags, key }
}

/// A seeded xorshift, so a failure repeats.
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
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| (self.next() >> 40) as u8).collect()
    }
}

#[test]
fn the_info_handler_gets_every_even_type_up_to_the_heartbeat() {
    let q = QueryProfile::default();
    let info: Vec<u8> = (0..=255u8).filter(|&t| q.is_info_type(t)).collect();
    assert_eq!(info, vec![0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22]);
    // The handshake's types are not info types, nor is the unused 24.
    for ty in [24u8, 26, 28, 30, 32, 34, 36, 38, 40] {
        assert!(!q.is_info_type(ty));
    }
    // A profile may move the bound.
    let wide = QueryProfile {
        info_type_max: 26,
        ..QueryProfile::default()
    };
    assert!(wide.is_info_type(24) && wide.is_info_type(26) && !wide.is_info_type(28));
}

#[test]
fn a_request_is_type_flags_and_key_and_a_short_one_is_silence() {
    // What a client sends (`sendPacket`): flags 0, `(session << 16) | (key & 0xffff)`.
    let bytes = request(GAME_PING_REQUEST, 0, 3, 0x1_0005);
    assert_eq!(bytes, [14, 0, 0x05, 0x00, 0x03, 0x00]);
    assert_eq!(
        parse_request(&bytes),
        Parsed::Request(req(Kind::Ping, 0, 0x0003_0005))
    );
    assert_eq!(
        parse_request(&request(GAME_INFO_REQUEST, 2, 1, 7)),
        Parsed::Request(req(Kind::Info, 2, 0x0001_0007))
    );
    // A type the server has no handler for comes back with its fields.
    assert_eq!(
        parse_request(&request(GAME_HEARTBEAT, 1, 0, 9)),
        Parsed::Other {
            ty: GAME_HEARTBEAT,
            flags: 1,
            key: 9
        }
    );
    // Trailing bytes are not read (only six are read).
    let mut long = request(GAME_PING_REQUEST, 0, 0, 1);
    long.extend_from_slice(&[0xff; 40]);
    assert_eq!(parse_request(&long), Parsed::Request(req(Kind::Ping, 0, 1)));
    for n in 0..REQUEST_BYTES {
        assert_eq!(parse_request(&bytes[..n]), Parsed::Short, "{n} bytes");
    }
    // The client's own types and the unused ones do nothing on a server.
    for ty in [0u8, 2, 4, 6, 8, 10, 12, 16, 20, 22, 24] {
        assert_eq!(
            parse_request(&request(ty, 0, 0, 0)),
            Parsed::Other {
                ty,
                flags: 0,
                key: 0
            }
        );
    }
}

#[test]
fn who_answers_follows_the_rule() {
    for kind in [Kind::Ping, Kind::Info] {
        let r = req(kind, 0, 1);
        assert!(answers(&r, true, false));
        assert!(!answers(&r, false, false), "allowConnections off");
        assert!(!answers(&req(kind, FLAG_OFFLINE_QUERY, 1), true, false));
        assert!(!answers(&req(kind, 3, 1), true, false));
        // The string flag does not stop an answer.
        assert!(answers(&req(kind, FLAG_NO_STRING_COMPRESS, 1), true, false));
        // SinglePlayer stops the pings, not the info request.
        assert_eq!(answers(&r, true, true), kind == Kind::Info);
    }
}

#[test]
fn the_ping_answer_byte_for_byte_with_c_strings() {
    let r = req(Kind::Ping, FLAG_NO_STRING_COMPRESS, 0xa1b2_c3d4);
    let bytes = ping_response(&table(), &r, b"My Server", &prof());
    let mut want = vec![16u8, 2, 0xd4, 0xc3, 0xb2, 0xa1, 4, b'V', b'E', b'R', b'1'];
    want.extend_from_slice(&14u32.to_le_bytes());
    want.extend_from_slice(&12u32.to_le_bytes());
    want.extend_from_slice(&1800u32.to_le_bytes());
    want.push(9);
    want.extend_from_slice(b"My Server");
    assert_eq!(bytes, want);
    // The name is cut at 24 bytes (`strncpy(buf, name, 24)`), and at its NUL.
    let long = ping_response(&table(), &r, b"abcdefghijklmnopqrstuvwxyz0123", &prof());
    assert_eq!(long[long.len() - 25], 24);
    assert_eq!(&long[long.len() - 24..], b"abcdefghijklmnopqrstuvwx");
    let nul = ping_response(&table(), &r, b"ab\0cd", &prof());
    assert_eq!(&nul[nul.len() - 3..], &[2, b'a', b'b']);
}

#[test]
fn the_default_query_profile_is_torques_version_string_and_protocol() {
    let p = QueryProfile::default();
    assert_eq!(p.version_string, b"VER1");
    assert_eq!((p.protocol_current, p.protocol_min, p.version), (12, 12, 0));
    let t = table();
    let r = req(Kind::Ping, FLAG_NO_STRING_COMPRESS, 1);
    let got = parse_ping_response(&t, &ping_response(&t, &r, b"x", &p)).unwrap();
    assert_eq!(
        (
            got.version,
            got.protocol,
            got.min_protocol,
            got.version_number
        ),
        (b"VER1".to_vec(), 12, 12, 0)
    );
}

#[test]
fn the_ping_answer_with_huffman_strings_reads_back() {
    let t = table();
    let r = req(Kind::Ping, 0, 0x0007_0003);
    let bytes = ping_response(&t, &r, b"a dedicated server named at length", &prof());
    assert_eq!(&bytes[..6], &[16, 0, 0x03, 0x00, 0x07, 0x00]);
    let got = parse_ping_response(&t, &bytes).unwrap();
    assert_eq!(
        got,
        PingResponse {
            flags: 0,
            key: 0x0007_0003,
            version: b"VER1".to_vec(),
            protocol: 14,
            min_protocol: 12,
            version_number: 1800,
            name: b"a dedicated server named".to_vec(),
        }
    );
    // The Huffman strings are the handshake's form: the flag, 8 bits of length, the codes (or the raw bytes).
    let mut w = BitWriter::new();
    w.write_u8(16);
    w.write_u8(0);
    w.write_u32(0x0007_0003);
    t.write(&mut w, b"VER1", 255);
    w.write_u32(14);
    w.write_u32(12);
    w.write_u32(1800);
    t.write(&mut w, b"a dedicated server named", 255);
    assert_eq!(bytes, w.into_bytes());
}

fn sample_fields() -> InfoFields {
    InfoFields {
        game_type: b"Test App".to_vec(),
        mission_type: b"Deathmatch".to_vec(),
        mission_name: b"Arena".to_vec(),
        status: STATUS_DEDICATED,
        players: 3,
        max_players: 32,
        bots: 0,
        cpu_mhz: 200,
        info: b"This is a Torque Game Engine Test Server.".to_vec(),
        content: b"Doing Ok".to_vec(),
    }
}

#[test]
fn the_info_answer_byte_for_byte_with_c_strings() {
    let r = req(Kind::Info, FLAG_NO_STRING_COMPRESS, 0x0102_0304);
    let f = sample_fields();
    let bytes = info_response(&table(), &r, &f);
    let mut want = vec![20u8, 2, 4, 3, 2, 1];
    for s in [&f.game_type, &f.mission_type, &f.mission_name] {
        want.push(s.len() as u8);
        want.extend_from_slice(s);
    }
    want.extend_from_slice(&[1, 3, 32, 0]);
    want.extend_from_slice(&200u16.to_le_bytes());
    want.push(f.info.len() as u8);
    want.extend_from_slice(&f.info);
    want.extend_from_slice(&8u16.to_le_bytes());
    want.extend_from_slice(b"Doing Ok");
    assert_eq!(bytes, want);
    assert_eq!(
        parse_info_response(&table(), &bytes).unwrap(),
        InfoResponse {
            flags: 2,
            key: 0x0102_0304,
            fields: f
        }
    );
}

#[test]
fn the_info_answer_with_huffman_strings_reads_back() {
    let t = table();
    let r = req(Kind::Info, 0, 0x0003_0001);
    let mut f = sample_fields();
    f.status = STATUS_DEDICATED | STATUS_PASSWORDED;
    let bytes = info_response(&t, &r, &f);
    let got = parse_info_response(&t, &bytes).unwrap();
    assert_eq!(got.fields, f);
    assert_eq!((got.flags, got.key), (0, 0x0003_0001));
}

#[test]
fn a_c_string_length_is_its_low_byte_as_the_writer_writes_it() {
    // `writeCString` takes `strlen & 0xff`: a 300-byte string writes 44 and its first 44 bytes.
    let mut w = BitWriter::new();
    write_c_string(&mut w, &[b'x'; 300]);
    let bytes = w.into_bytes();
    assert_eq!(bytes.len(), 45);
    assert_eq!(bytes[0], 44);
    // The long form: `strlen & 0xffff`.
    let mut w = BitWriter::new();
    write_long_c_string(&mut w, &[b'y'; 70_000]);
    let bytes = w.into_bytes();
    assert_eq!(&bytes[..2], &(70_000u32 as u16).to_le_bytes());
    assert_eq!(bytes.len(), 2 + (70_000 & 0xffff));
}

#[test]
fn a_reply_is_cut_at_1500_bytes() {
    let t = table();
    let mut f = sample_fields();
    f.content = vec![b'z'; 4_000];
    let bytes = info_response(&t, &req(Kind::Info, 0, 1), &f);
    // The writes past 12,000 bits are dropped; the length field says what the script returned.
    assert_eq!(bytes.len(), NET_MAX_DATAGRAM);
    assert_eq!(&bytes[..6], &[20, 0, 1, 0, 0, 0]);
    // A reader sees a long string longer than the datagram: an error, never a panic.
    assert!(parse_info_response(&t, &bytes).is_err());
    // Under the limit every byte is there.
    f.content = vec![b'z'; 1_000];
    let bytes = info_response(&t, &req(Kind::Info, 0, 1), &f);
    assert_eq!(
        parse_info_response(&t, &bytes)
            .unwrap()
            .fields
            .content
            .len(),
        1_000
    );
}

fn ask(ip: IpAddr, request_len: usize) -> InfoAsk {
    InfoAsk {
        ip,
        request_len,
        max_reply: NET_MAX_DATAGRAM,
    }
}

#[test]
fn the_limiter_counts_per_source_and_per_network() {
    let padded = |ip| ask(ip, PADDED_REQUEST_BYTES);
    let a = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
    let mut l = InfoLimiter::default();
    for _ in 0..NET_INFO_REPLIES_PER_SRC_PER_S {
        assert_eq!(l.allow(&padded(a), 0), Ok(()));
    }
    assert_eq!(l.allow(&padded(a), 500), Err(Refused::Source));
    // The next second.
    assert_eq!(l.allow(&padded(a), 1_000), Ok(()));
    // One network: its addresses share the network's budget.
    let mut l = InfoLimiter::default();
    let answered = (0..=255u8)
        .filter(|&h| {
            l.allow(&padded(IpAddr::V4(Ipv4Addr::new(10, 1, 1, h))), 0)
                .is_ok()
        })
        .count();
    assert_eq!(answered, NET_INFO_REPLIES_PER_NETWORK_PER_S as usize);
}

#[test]
fn the_info_scripts_output_is_answered_from_for_a_second() {
    let mut cache = InfoCache::default();
    assert_eq!(cache.fresh(0), None);
    cache.store(100, b"players 3".to_vec());
    assert_eq!(
        cache.fresh(100 + NET_INFO_CACHE_MS - 1),
        Some(&b"players 3"[..])
    );
    assert_eq!(
        cache.fresh(100 + NET_INFO_CACHE_MS),
        None,
        "stale: the script runs again"
    );
}

#[test]
fn spoofed_short_info_requests_reflect_at_most_three_times_their_bytes_and_padded_ones_are_answered()
 {
    // 10,000 spoofed 6-byte info requests from 1,000 sources in a second, each answered with a 600-byte reply when the
    // limiter lets it: the reply bytes stay within 3 times the request bytes. A padded request is still answered in full.
    let reply_len = 600;
    let mut l = InfoLimiter::default();
    let (mut asked, mut sent) = (0usize, 0usize);
    for i in 0..10_000u32 {
        let spoofed = ask(
            IpAddr::V4(Ipv4Addr::from(0xc612_0000 + (i % 1_000) * 61)),
            REQUEST_BYTES,
        );
        let now = i / 10;
        asked += REQUEST_BYTES;
        if l.allow(&spoofed, now).is_ok() && l.allow_reply(&spoofed, reply_len, now).is_ok() {
            sent += reply_len;
        }
    }
    assert!(sent > 0, "a stock client still gets some answers");
    assert!(
        sent <= 3 * asked,
        "{sent} reply bytes for {asked} request bytes"
    );
    let honest = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7));
    let now = 999;
    let padded = ask(honest, PADDED_REQUEST_BYTES);
    assert_eq!(l.allow(&padded, now), Ok(()));
    assert_eq!(l.allow_reply(&padded, NET_MAX_DATAGRAM, now), Ok(()));
    // A reply within the factor costs the budget nothing.
    let small = ask(honest, REQUEST_BYTES);
    assert_eq!(l.allow_reply(&small, 18, now), Ok(()));
}

#[test]
fn a_padded_request_reads_as_the_request_it_pads() {
    let padded = padded_request(GAME_INFO_REQUEST, 0, 3, 9, PADDED_REQUEST_BYTES);
    assert_eq!(padded.len(), PADDED_REQUEST_BYTES);
    const { assert!(PADDED_REQUEST_BYTES * NET_INFO_REPLY_FACTOR >= NET_MAX_DATAGRAM) };
    assert_eq!(
        &padded[..REQUEST_BYTES],
        request(GAME_INFO_REQUEST, 0, 3, 9).as_slice()
    );
    assert_eq!(
        parse_request(&padded),
        parse_request(&padded[..REQUEST_BYTES])
    );
    // Torque's profile sends the bare request; a profile with the extension pads.
    assert_eq!(
        QueryProfile::default()
            .request(GAME_PING_REQUEST, 0, 3, 9)
            .len(),
        REQUEST_BYTES
    );
    let handshake = crate::profile::HandshakeProfile {
        extension: Some(crate::profile::HandshakeExtension {
            arg: b"x".to_vec(),
            accept_word: 1,
            ghost_bits: None,
            packet_bytes: None,
            wide_limit: None,
        }),
        ..Default::default()
    };
    let profile = QueryProfile::for_handshake(&handshake);
    assert_eq!(
        profile.request(GAME_PING_REQUEST, 0, 3, 9).len(),
        PADDED_REQUEST_BYTES
    );
}

#[test]
fn fuzz_the_decoders_never_panic_and_the_writers_round_trip() {
    let t = table();
    let mut rng = Rng(0x05ee_d1a7);
    for _ in 0..20_000 {
        let len = rng.below(64);
        let mut bytes = rng.bytes(len);
        if let Some(first) = bytes.first_mut() {
            *first = [14u8, 16, 18, 20, 22, 24][rng.below(6)];
        }
        let _ = parse_request(&bytes);
        let _ = parse_ping_response(&t, &bytes);
        let _ = parse_info_response(&t, &bytes);
    }
    for _ in 0..2_000 {
        let flags = (rng.next() >> 50) as u8 & FLAG_NO_STRING_COMPRESS;
        let r = req(Kind::Info, flags, rng.next() as u32);
        // Strings without NULs (a C string ends at one), at most 255 bytes (Huffman's `writeString(s, 255)` cuts there; a C string's
        // length is its low byte, so a longer one would not read back the same).
        let mut s = |n: usize| -> Vec<u8> {
            let len = rng.below(n);
            rng.bytes(len).into_iter().map(|b| b.max(1)).collect()
        };
        let f = InfoFields {
            game_type: s(256),
            mission_type: s(256),
            mission_name: s(256),
            status: 3,
            players: 1,
            max_players: 2,
            bots: 3,
            cpu_mhz: 4,
            info: s(256),
            content: s(300),
        };
        let bytes = info_response(&t, &r, &f);
        assert!(bytes.len() <= NET_MAX_DATAGRAM);
        if let Ok(got) = parse_info_response(&t, &bytes) {
            assert_eq!(got.fields, f);
        } else {
            // Only a reply cut at 1,500 bytes fails to read back.
            assert_eq!(bytes.len(), NET_MAX_DATAGRAM);
        }
        let name = s(40);
        let p = parse_ping_response(
            &t,
            &ping_response(&t, &req(Kind::Ping, flags, 5), &name, &prof()),
        )
        .unwrap();
        assert_eq!(p.name, name[..name.len().min(SERVER_NAME_BYTES)]);
    }
}
