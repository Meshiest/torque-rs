use super::*;

#[test]
fn reads_stop_at_the_end_and_counts_are_held_to_the_bytes() {
    let bytes = [3u8, 0, 0, 0, 1, 2];
    let mut r = Reader::new(&bytes);
    assert!(matches!(
        r.vec("x", 4, |r| r.u32("x")),
        Err(MapError::Count { .. })
    ));
    let mut r = Reader::new(&bytes);
    assert_eq!(
        r.vec("x", 1, |r| r.u8("x")),
        Err(MapError::Count {
            what: "x",
            value: 3,
            cap: 2
        })
    );
    let mut r = Reader::new(&[2u8, b'a']);
    assert!(matches!(r.string("s"), Err(MapError::Short { .. })));
    let mut r = Reader::new(&[255u8, 2, 0, b'a', b'b']);
    assert_eq!(r.long_string("s").unwrap(), "ab");
}
