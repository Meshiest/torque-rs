use super::*;

/// An Ogg page (CRC filled in) of one logical stream.
pub fn page(serial: u32, sequence: u32, granule: u64, kind: u8, packets: &[&[u8]]) -> Vec<u8> {
    let mut table = Vec::new();
    let mut body = Vec::new();
    for p in packets {
        let mut n = p.len();
        while n >= 255 {
            table.push(255u8);
            n -= 255;
        }
        table.push(n as u8);
        body.extend_from_slice(p);
    }
    let mut out = b"OggS".to_vec();
    out.push(0);
    out.push(kind);
    out.extend_from_slice(&granule.to_le_bytes());
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&sequence.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.push(table.len() as u8);
    out.extend_from_slice(&table);
    out.extend_from_slice(&body);
    let crc = crc(&out);
    out[22..26].copy_from_slice(&crc.to_le_bytes());
    out
}

/// Ogg's CRC-32 (polynomial 0x04c11db7, no reflection, no final xor).
pub fn crc(bytes: &[u8]) -> u32 {
    let mut c: u32 = 0;
    for b in bytes {
        c ^= u32::from(*b) << 24;
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 {
                (c << 1) ^ 0x04c1_1db7
            } else {
                c << 1
            };
        }
    }
    c
}

/// A Vorbis identification header.
pub fn ident(channels: u8, rate: u32) -> Vec<u8> {
    let mut p = vec![1];
    p.extend_from_slice(b"vorbis");
    p.extend_from_slice(&0u32.to_le_bytes());
    p.push(channels);
    p.extend_from_slice(&rate.to_le_bytes());
    p.extend_from_slice(&[0; 12]);
    p.push(0xb8); // blocksizes 256 and 2048
    p.push(1);
    p
}

#[test]
fn the_last_granule_of_the_first_stream_is_the_total() {
    let mut file = page(7, 0, 0, 2, &[&ident(1, 22_050)]);
    file.extend(page(7, 1, 4_410, 0, &[b"x"]));
    file.extend(page(7, 2, u64::MAX, 0, &[b"y"]));
    file.extend(page(7, 3, 9_000, 4, &[b"z"]));
    file.extend(page(8, 0, 99_999, 2, &[b"other"]));
    assert_eq!(last_granule(&file), Some(9_000));
    assert_eq!(last_granule(b"no pages"), None);
}

/// A file the audio fuzz found (mutated from its own seeds): a comment header claiming 0xa49c8800 comments in 342
/// bytes. `lewton::header::read_header_comment` reserved 48 bytes per comment (132,562,649,088) before reading any, which aborted
/// the process.
const FUZZ_FOUND: [u8; 342] = [
    0x4f, 0x67, 0x67, 0x53, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2a, 0x34, 0xbb, 0x06, 0x01, 0x1e, 0x01, 0x76, 0x6f, 0x72,
    0x62, 0x69, 0x73, 0x00, 0x00, 0x00, 0x00, 0x01, 0x22, 0x56, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xb8, 0x01, 0x4f, 0x67, 0x67, 0x53, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
    0xc5, 0x40, 0x37, 0xe0, 0x02, 0x14, 0x10, 0x03, 0x76, 0x6f, 0x72, 0x62, 0x69, 0x73, 0x04, 0x00,
    0x00, 0x00, 0x70, 0x6f, 0x72, 0x74, 0x00, 0x88, 0x9c, 0xa4, 0x7e, 0x86, 0x0f, 0x60, 0x95, 0x2c,
    0xa6, 0x44, 0x36, 0xcb, 0x61, 0x43, 0x04, 0x59, 0x92, 0xfd, 0xa4, 0x1f, 0x9a, 0xd8, 0x89, 0x25,
    0x31, 0x29, 0x77, 0xd0, 0x44, 0x27, 0xfa, 0x6a, 0xeb, 0x53, 0x57, 0x12, 0x15, 0x06, 0xa9, 0xbb,
    0x43, 0x95, 0x47, 0x72, 0xb7, 0xc6, 0xfb, 0x38, 0x96, 0xa3, 0x36, 0x4b, 0x8d, 0xd8, 0x98, 0xbf,
    0xb2, 0xd1, 0xb1, 0x54, 0xd4, 0x71, 0x00, 0x00, 0x00, 0x01, 0x05, 0x76, 0x6f, 0x72, 0x62, 0x69,
    0x73, 0x00, 0x42, 0x43, 0x56, 0x01, 0x00, 0x01, 0x00, 0x00, 0x4f, 0x67, 0x67, 0x53, 0x00, 0x04,
    0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00,
    0x59, 0xc2, 0x8d, 0x11, 0x01, 0x04, 0x00, 0x01, 0x02, 0x03, 0x04, 0x00, 0x00, 0x00, 0x70, 0x6f,
    0x72, 0x74, 0x00, 0x88, 0x9c, 0xa4, 0x7e, 0x86, 0x0f, 0x60, 0x95, 0x2c, 0xa6, 0x44, 0x36, 0xcb,
    0x61, 0x43, 0x04, 0x59, 0x92, 0xfd, 0xa4, 0x1f, 0x9a, 0xd8, 0x89, 0x25, 0x31, 0x29, 0x77, 0xd0,
    0x44, 0x27, 0xfa, 0x6a, 0xeb, 0x53, 0x57, 0x12, 0x15, 0x06, 0xa9, 0xbb, 0x43, 0x95, 0x47, 0x72,
    0xb7, 0xc6, 0xfb, 0x38, 0x96, 0xa3, 0x36, 0x4b, 0x8d, 0xd8, 0x98, 0xbf, 0xb2, 0xd1, 0xb1, 0x54,
    0xd4, 0x71, 0x00, 0x00, 0x00, 0x01, 0x05, 0x76, 0x6f, 0x72, 0x62, 0x69, 0x73, 0x00, 0x42, 0x43,
    0x56, 0x01, 0x00, 0x01, 0x00, 0x00, 0x4f, 0x67, 0x67, 0x53, 0x00, 0x04, 0x00, 0x04, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x59, 0xc2, 0x8d, 0x11,
    0x01, 0x04, 0x00, 0x01, 0x02, 0x03,
];

/// A page with its own lacing table (so a packet can be left unfinished at its end), CRC filled in.
fn raw_page(serial: u32, sequence: u32, kind: u8, lacing: &[u8], body: &[u8]) -> Vec<u8> {
    let mut out = b"OggS".to_vec();
    out.push(0);
    out.push(kind);
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&sequence.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.push(lacing.len() as u8);
    out.extend_from_slice(lacing);
    out.extend_from_slice(body);
    let c = crc(&out);
    out[22..26].copy_from_slice(&c.to_le_bytes());
    out
}

/// A comment header claiming `count` comments, padded to `len` bytes (no framing byte where the count says it is).
fn comment_claiming(count: u32, len: usize) -> Vec<u8> {
    let mut p = vec![3];
    p.extend_from_slice(b"vorbis");
    p.extend_from_slice(&4u32.to_le_bytes());
    p.extend_from_slice(b"test");
    p.extend_from_slice(&count.to_le_bytes());
    p.resize(len, 0x41);
    p
}

fn good_comment() -> Vec<u8> {
    let mut p = comment_claiming(0, 19);
    p.push(1);
    p
}

/// The headers the decode checks are the packets `ogg::PacketReader` (lewton's own demuxer) delivers, and lewton never parses the
/// comment header, so no page trick can show the check one packet and lewton another. Each crafted file must be refused (or read)
/// without the test process aborting on a huge allocation.
#[test]
fn crafted_pages_cannot_get_a_hostile_header_past_the_check() {
    let head = page(5, 0, 0, 2, &[&ident(1, 22_050)]);
    let hostile = comment_claiming(0xa49c_8800, 300);
    // (1) The hostile comment split over two pages (continued properly).
    let mut split = head.clone();
    split.extend(raw_page(5, 1, 0, &[255], &hostile[..255]));
    split.extend(raw_page(
        5,
        2,
        1,
        &[(hostile.len() - 255) as u8],
        &hostile[255..],
    ));
    // (2) A page that restarts mid-packet: an unfinished harmless fragment, then a page without the continuation flag that
    // starts the hostile comment afresh (the demuxer drops the fragment or refuses the stream).
    let mut restart = head.clone();
    let mut decoy = good_comment();
    decoy.resize(255, 0);
    restart.extend(raw_page(5, 1, 0, &[255], &decoy));
    restart.extend(raw_page(
        5,
        2,
        0,
        &[255, (hostile.len() - 255) as u8],
        &hostile,
    ));
    // (3) A continued page with nothing pending: a leading fragment that looks like a good comment, then the hostile one.
    let mut orphan = head.clone();
    let good = good_comment();
    let mut body = good.clone();
    body.extend_from_slice(&hostile);
    orphan.extend(raw_page(
        5,
        1,
        1,
        &[good.len() as u8, 255, (hostile.len() - 255) as u8],
        &body,
    ));
    // (4) The hostile comment on a page whose CRC is wrong.
    let mut bad_crc = head.clone();
    let mut p = raw_page(5, 1, 0, &[255, (hostile.len() - 255) as u8], &hostile);
    p[22] ^= 0xff;
    bad_crc.extend(p);
    // (5) A second logical stream whose packet is the hostile comment, between the first stream's headers.
    let mut other = head.clone();
    other.extend(raw_page(
        6,
        0,
        2,
        &[255, (hostile.len() - 255) as u8],
        &hostile,
    ));
    other.extend(raw_page(
        5,
        1,
        0,
        &[255, (hostile.len() - 255) as u8],
        &hostile,
    ));
    for (name, file) in [
        ("split", split),
        ("restart", restart),
        ("orphan", orphan),
        ("bad crc", bad_crc),
        ("other stream", other),
    ] {
        assert!(read_ogg(&file).is_err(), "{name}");
    }
}

#[test]
fn a_fuzz_found_file_is_refused_without_a_huge_allocation() {
    let r = read_ogg(&FUZZ_FOUND);
    assert!(
        matches!(&r, Err(OggError::Open(e)) if e.contains("comment")),
        "{r:?}"
    );
}

#[test]
fn a_header_only_stream_or_garbage_fails_cleanly() {
    let file = page(1, 0, 0, 2, &[&ident(2, 44_100)]);
    assert!(read_ogg(&file).is_err());
    assert!(read_ogg(b"OggS\0\0\0\0").is_err());
    assert!(read_ogg(&[]).is_err());
}
