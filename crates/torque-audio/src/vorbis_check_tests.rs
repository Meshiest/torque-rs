use super::*;

/// Vorbis bit packing (least significant bit first), for building setup headers.
#[derive(Default)]
pub struct BitWriter {
    pub bytes: Vec<u8>,
    bits: u64,
}

impl BitWriter {
    pub fn put(&mut self, v: u64, n: u32) {
        for i in 0..n {
            if self.bits.is_multiple_of(8) {
                self.bytes.push(0);
            }
            let last = self.bytes.len() - 1;
            self.bytes[last] |= ((v.checked_shr(i).unwrap_or(0) & 1) as u8) << (self.bits % 8);
            self.bits += 1;
        }
    }
}

fn setup(body: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut w = BitWriter::default();
    body(&mut w);
    let mut p = vec![5];
    p.extend_from_slice(b"vorbis");
    p.extend(w.bytes);
    p
}

/// One codebook header up to its lookup type.
fn codebook(w: &mut BitWriter, dimensions: u64, entries: u64) {
    w.put(u64::from(CODEBOOK_SYNC), 24);
    w.put(dimensions, 16);
    w.put(entries, 24);
}

#[test]
fn lookup1_values_is_lewtons() {
    assert_eq!(lookup1_values(1025, 10), 2);
    assert_eq!(lookup1_values(1024, 10), 2);
    assert_eq!(lookup1_values(1023, 10), 1);
    assert_eq!(lookup1_values(3126, 5), 5);
    assert_eq!(lookup1_values(3125, 5), 5);
    assert_eq!(lookup1_values(3124, 5), 4);
    assert_eq!(lookup1_values(1, 1), 1);
    assert_eq!(lookup1_values(0, 15), 0);
    assert_eq!(lookup1_values(0, 0), 0);
    assert_eq!(lookup1_values(1, 0), u32::MAX);
    assert_eq!(lookup1_values(400, 0), u32::MAX);
    assert_eq!(lookup1_values((1 << 24) - 1, 1), (1 << 24) - 1);
    assert_eq!(lookup1_values(1 << 24, 40), 1);
}

#[test]
fn a_fuzz_found_comment_count_is_refused_without_allocating() {
    // A comment header found by the fuzz: vendor "test", then 0xa49c8800 comments in a packet of a few dozen bytes.
    let mut p = vec![3];
    p.extend_from_slice(b"vorbis");
    p.extend_from_slice(&4u32.to_le_bytes());
    p.extend_from_slice(b"test");
    p.extend_from_slice(&0xa49c_8800u32.to_le_bytes());
    p.extend_from_slice(&[0x86, 0x0f, 0x60, 0x95, 0x2c, 0xa6, 0x44, 0x36]);
    assert!(check_comment(&p).is_err());
    // A vendor length past the packet, a comment longer than what is left, and a good header.
    let mut good = vec![3];
    good.extend_from_slice(b"vorbis");
    good.extend_from_slice(&4u32.to_le_bytes());
    good.extend_from_slice(b"test");
    good.extend_from_slice(&1u32.to_le_bytes());
    good.extend_from_slice(&3u32.to_le_bytes());
    good.extend_from_slice(b"a=b");
    good.push(1);
    assert_eq!(check_comment(&good), Ok(()));
    let mut vendor = good.clone();
    vendor[7..11].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(check_comment(&vendor).is_err());
    let mut long = good.clone();
    long[19..23].copy_from_slice(&0x7fff_ffffu32.to_le_bytes());
    assert!(check_comment(&long).is_err());
    let mut framing = good.clone();
    *framing.last_mut().unwrap() = 0;
    assert!(check_comment(&framing).is_err());
    let mut audio = good;
    audio[0] = 2;
    assert!(check_comment(&audio).is_err());
}

#[test]
fn codebooks_that_would_reserve_more_than_their_bits_are_refused() {
    // Lookup type 1 with 0 dimensions: lewton's lookup1_values is u32::MAX multiplicands (16 GiB reserved).
    let zero_dims = setup(|w| {
        w.put(0, 8);
        codebook(w, 0, 1);
        w.put(0, 1); // unordered
        w.put(0, 1); // not sparse
        w.put(0, 5);
        w.put(1, 4); // lookup type 1
        w.put(0, 64);
        w.put(0, 4);
        w.put(0, 1);
    });
    assert!(
        check_setup(&zero_dims)
            .unwrap_err()
            .contains("lookup values")
    );
    // An ordered codebook of 2^24 - 1 entries in a few bytes, its lookup vectors 2^24 x 65535 floats (4 TiB reserved).
    let huge = setup(|w| {
        w.put(0, 8);
        codebook(w, 65_535, (1 << 24) - 1);
        w.put(1, 1); // ordered
        w.put(0, 5);
        w.put((1 << 24) - 1, 24);
        w.put(1, 4);
    });
    assert!(check_setup(&huge).unwrap_err().contains("entries"));
    // Entries summed over codebooks with no lookup table (type 0): 32 ordered codebooks of 2^16 - 1 entries each, a few bytes
    // apiece, pass the total cap.
    let many = setup(|w| {
        w.put(31, 8);
        for _ in 0..32 {
            codebook(w, 1, (1 << 16) - 1);
            w.put(1, 1); // ordered
            w.put(0, 5);
            w.put((1 << 16) - 1, 16);
            w.put(0, 4); // no lookup
        }
    });
    assert!(check_setup(&many).unwrap_err().contains("entries"));
    // At the cap they pass: 16 such codebooks are 1,048,560 entries.
    let sixteen = setup(|w| {
        w.put(15, 8);
        for _ in 0..16 {
            codebook(w, 1, (1 << 16) - 1);
            w.put(1, 1);
            w.put(0, 5);
            w.put((1 << 16) - 1, 16);
            w.put(0, 4);
        }
    });
    assert_eq!(
        check_setup(&sixteen).map(|t| t.entries),
        Ok(16 * ((1 << 16) - 1))
    );
    // An unordered codebook needs at least a bit per entry: 2^20 entries in a few bytes end the header early.
    let short = setup(|w| {
        w.put(0, 8);
        codebook(w, 1, 1 << 20);
        w.put(0, 1); // unordered
        w.put(1, 1); // sparse
        w.put(0, 64);
    });
    assert!(check_setup(&short).unwrap_err().contains("ends early"));
    // Type 2 lookup values past the packet.
    let past = setup(|w| {
        w.put(0, 8);
        codebook(w, 4, 8);
        w.put(1, 1);
        w.put(0, 5);
        w.put(8, 4);
        w.put(2, 4);
        w.put(0, 64);
        w.put(7, 4);
        w.put(0, 1);
    });
    assert!(check_setup(&past).unwrap_err().contains("lookup values"));
    // The same codebook with its 32 values of 8 bits present.
    let ok = setup(|w| {
        w.put(0, 8);
        codebook(w, 4, 8);
        w.put(1, 1);
        w.put(0, 5);
        w.put(8, 4);
        w.put(2, 4);
        w.put(0, 64);
        w.put(7, 4);
        w.put(0, 1);
        w.put(0, 32 * 8);
    });
    assert_eq!(
        check_setup(&ok),
        Ok(CodebookTotals {
            codebooks: 1,
            entries: 8,
            vq_values: 32
        })
    );
}
