use super::*;

/// A frequency table shaped like text (a made-up one, not Torque's): low letters common, a few punctuation marks.
pub(crate) fn sample_frequencies() -> [u32; 256] {
    let mut f = [0u32; 256];
    for (i, slot) in f.iter_mut().enumerate().take(127) {
        *slot = match i as u8 {
            b'a'..=b'z' => 400 + (i as u32 * 37) % 900,
            b'A'..=b'Z' => 20 + (i as u32 * 13) % 90,
            b'0'..=b'9' => 80 + (i as u32 * 7) % 60,
            b' ' => 2800,
            b'/' | b'.' | b'_' | b'-' => 500,
            _ => (i as u32 * 3) % 5,
        };
    }
    f
}

fn table() -> Table {
    Table::from_frequencies(&sample_frequencies()).unwrap()
}

#[test]
fn every_byte_gets_a_prefix_free_code() {
    let t = table();
    let mut seen: Vec<(u32, u8)> = Vec::new();
    for b in 0..=255u8 {
        let (bits, count) = t.code(b);
        assert!(count > 0 && count <= 32);
        for &(other, n) in &seen {
            let shared = count.min(n);
            let mask = if shared == 32 {
                u32::MAX
            } else {
                (1u32 << shared) - 1
            };
            assert_ne!(bits & mask, other & mask, "a code is the prefix of another");
        }
        seen.push((bits, count));
    }
    assert!(t.code(b'e').1 < t.code(0xfe).1);
    assert!(t.code(b' ').1 <= 5);
}

#[test]
fn the_default_table_is_the_balanced_tree_equal_populations_give() {
    let d = Table::default();
    let t = Table::from_frequencies(&[0; 256]).unwrap();
    for b in 0..=255u8 {
        assert_eq!(d.code(b).1, 8);
    }
    // Prefix-free and decodable: strings round trip through it.
    let mut w = BitWriter::new();
    d.write(&mut w, b"hello\xffworld", 255);
    let bytes = w.into_bytes();
    assert_eq!(
        d.read(&mut BitReader::new(&bytes)).unwrap(),
        b"hello\xffworld"
    );
    let _ = t;
    // Every byte's code decodes to the byte.
    for b in 0..=255u8 {
        let (bits, count) = d.code(b);
        let mut w = BitWriter::new();
        w.write_flag(true);
        w.write_bits(1, 8);
        w.write_bits(bits, count);
        let bytes = w.into_bytes();
        assert_eq!(d.read(&mut BitReader::new(&bytes)).unwrap(), vec![b]);
    }
}

#[test]
fn equal_populations_make_a_balanced_tree_and_strings_stay_raw() {
    let t = Table::from_frequencies(&[0; 256]).unwrap();
    assert!((0..=255u8).all(|b| t.code(b).1 == 8));
    let mut w = BitWriter::new();
    t.write(&mut w, b"abc", 255);
    // Eight bits a byte is not fewer than eight bits a byte: raw.
    assert_eq!(w.bits(), 1 + 8 + 24);
    assert_eq!(w.bytes()[0] & 1, 0);
}

#[test]
fn a_string_is_compressed_only_if_that_saves_bits() {
    let t = table();
    let mut w = BitWriter::new();
    t.write(&mut w, b"the quick brown fox", 255);
    assert_eq!(w.bytes()[0] & 1, 1);
    assert!(w.bits() < 1 + 8 + 19 * 8);
    // A string of rare bytes stays raw.
    let mut w = BitWriter::new();
    t.write(&mut w, &[0xff, 0xfe, 0x80, 0x01], 255);
    assert_eq!(w.bytes()[0] & 1, 0);
    assert_eq!(w.bits(), 1 + 8 + 32);
    // Exactly eight bits a byte is raw; one bit fewer is coded.
    let mut eight = None;
    let mut seven = None;
    for a in 0..=255u8 {
        for b in 0..=255u8 {
            match t.coded_bits(&[a, b]) {
                16 if eight.is_none() => eight = Some([a, b]),
                15 if seven.is_none() => seven = Some([a, b]),
                _ => {}
            }
        }
    }
    let mut w = BitWriter::new();
    t.write(&mut w, &eight.unwrap(), 255);
    assert_eq!((w.bytes()[0] & 1, w.bits()), (0, 1 + 8 + 16));
    let mut w = BitWriter::new();
    t.write(&mut w, &seven.unwrap(), 255);
    assert_eq!((w.bytes()[0] & 1, w.bits()), (1, 1 + 8 + 15));
}

#[test]
fn strings_round_trip_cut_at_255_bytes_and_at_a_nul() {
    let t = table();
    let long = vec![b'a'; 300];
    let mut w = BitWriter::new();
    t.write(&mut w, &long, 255);
    t.write(&mut w, b"ab\0cd", 255);
    t.write(&mut w, b"", 255);
    t.write(&mut w, b"abcdef", 3);
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    assert_eq!(t.read(&mut r).unwrap(), vec![b'a'; 255]);
    assert_eq!(t.read(&mut r).unwrap(), b"ab");
    assert_eq!(t.read(&mut r).unwrap(), b"");
    assert_eq!(t.read(&mut r).unwrap(), b"abc");
}

#[test]
fn a_truncated_or_hostile_string_is_an_error_not_a_panic() {
    let t = table();
    let mut w = BitWriter::new();
    t.write(&mut w, b"some/file/name.png", 255);
    let bytes = w.into_bytes();
    for cut in 0..bytes.len() {
        assert!(t.read(&mut BitReader::new(&bytes[..cut])).is_err());
    }
    // A length of 255 with nothing after it.
    assert!(t.read(&mut BitReader::new(&[0xff, 0x01])).is_err());
    for seed in 0..2000u32 {
        let junk: Vec<u8> = (0..20u32)
            .map(|i| (seed.wrapping_mul(2_654_435_761).rotate_left(i) >> 3) as u8)
            .collect();
        for skip in 0..8 {
            let mut r = BitReader::new(&junk);
            let _ = r.read_bits(skip);
            let _ = t.read(&mut r);
        }
    }
}

#[test]
fn extreme_tables_are_refused_or_prefix_free_never_a_panic() {
    let mut tables: Vec<[u32; 256]> = vec![[u32::MAX; 256], [0; 256]];
    let mut fib = [0u32; 256];
    let (mut a, mut b) = (1u32, 1u32);
    for slot in fib.iter_mut() {
        *slot = a;
        let next = a.saturating_add(b);
        a = b;
        b = next;
    }
    tables.push(fib);
    let mut powers = [0u32; 256];
    for (i, slot) in powers.iter_mut().enumerate() {
        *slot = 1u32.checked_shl(i as u32).unwrap_or(u32::MAX);
    }
    tables.push(powers);
    for f in &tables {
        if let Ok(t) = Table::from_frequencies(f) {
            let mut w = BitWriter::new();
            t.write(&mut w, &[b'a', b'b', 0xff, b'z'], 255);
            let bytes = w.into_bytes();
            assert!(t.read(&mut BitReader::new(&bytes)).is_ok());
        }
    }
}

#[test]
fn torques_standard_table_builds_and_compresses_text() {
    let t = Table::torque();
    // The frequent letters get short codes, a byte the table never saw a long one.
    assert!(t.code(b'e').1 <= 4 && t.code(b' ').1 <= 4);
    assert!(t.code(0xff).1 > 8);
    let text = b"the quick brown fox jumps over the lazy dog";
    assert!(t.coded_bits(text) < text.len() * 8);
    let mut w = BitWriter::new();
    t.write(&mut w, text, 255);
    let bytes = w.into_bytes();
    assert_eq!(t.read(&mut BitReader::new(&bytes)), Ok(text.to_vec()));
    // The same tree whatever builds it.
    let again = Table::from_frequencies(&TORQUE_CHAR_FREQS).unwrap();
    assert!((0..=255u8).all(|b| again.code(b) == t.code(b)));
}
