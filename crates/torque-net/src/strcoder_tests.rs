use super::*;
use crate::huffman::tests::sample_frequencies;

fn table() -> Table {
    Table::from_frequencies(&sample_frequencies()).unwrap()
}

#[test]
fn without_a_buffer_a_string_is_the_plain_huffman_form() {
    let t = table();
    let mut a = BitWriter::new();
    write_string(&mut a, &t, b"some/path.png", 255);
    let mut b = BitWriter::new();
    t.write(&mut b, b"some/path.png", 255);
    assert_eq!(a.bytes(), b.bytes());
    let bytes = a.into_bytes();
    assert_eq!(
        read_string(&mut BitReader::new(&bytes), &t).unwrap(),
        b"some/path.png"
    );
}

#[test]
fn with_a_buffer_a_shared_prefix_of_three_or_more_is_sent_once() {
    let t = table();
    let mut w = BitWriter::new();
    w.enable_string_buffer();
    write_string(&mut w, &t, b"hello world", 255);
    let first = w.bits();
    // The first string shares nothing with the empty buffer: flag 0, then the plain form.
    assert_eq!(w.bytes()[0] & 1, 0);
    write_string(&mut w, &t, b"hello there", 255);
    // "hello " is 6 bytes in common: flag 1, int(8) 6, the form of "there".
    let mut plain = BitWriter::new();
    t.write(&mut plain, b"there", 255);
    assert_eq!(w.bits() - first, 1 + 8 + plain.bits());
    // A prefix of 2 is not worth it.
    let before = w.bits();
    write_string(&mut w, &t, b"he", 255);
    let mut plain = BitWriter::new();
    t.write(&mut plain, b"he", 255);
    assert_eq!(w.bits() - before, 1 + plain.bits());
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    r.enable_string_buffer();
    assert_eq!(read_string(&mut r, &t).unwrap(), b"hello world");
    assert_eq!(read_string(&mut r, &t).unwrap(), b"hello there");
    assert_eq!(read_string(&mut r, &t).unwrap(), b"he");
}

#[test]
fn the_same_string_twice_sends_the_whole_string_as_prefix() {
    let t = table();
    let mut w = BitWriter::new();
    w.enable_string_buffer();
    write_string(&mut w, &t, b"repeat", 255);
    let first = w.bits();
    write_string(&mut w, &t, b"repeat", 255);
    // flag, prefix 6, then the empty tail: 1 + 8 + (1 + 8).
    assert_eq!(w.bits() - first, 1 + 8 + 9);
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    r.enable_string_buffer();
    assert_eq!(read_string(&mut r, &t).unwrap(), b"repeat");
    assert_eq!(read_string(&mut r, &t).unwrap(), b"repeat");
}

#[test]
fn a_hostile_prefix_that_overruns_the_buffer_is_refused() {
    let t = table();
    let mut w = BitWriter::new();
    w.write_flag(true);
    w.write_bits(250, 8);
    t.write(&mut w, b"abcdefgh", 255);
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    r.enable_string_buffer();
    assert!(read_string(&mut r, &t).is_err());
}

#[test]
fn a_prefix_past_the_previous_string_ends_at_its_nul() {
    // The previous string was "ab"; a prefix of 5 reads the stale (zero) buffer: the C string ends at the first NUL.
    let t = table();
    let mut w = BitWriter::new();
    w.write_flag(false);
    t.write(&mut w, b"ab", 255);
    w.write_flag(true);
    w.write_bits(5, 8);
    t.write(&mut w, b"xyz", 255);
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    r.enable_string_buffer();
    assert_eq!(read_string(&mut r, &t).unwrap(), b"ab");
    assert_eq!(read_string(&mut r, &t).unwrap(), b"ab");
}
