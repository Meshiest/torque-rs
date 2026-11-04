use super::*;

#[test]
fn a_reservation_is_cut_to_what_the_bytes_could_hold_and_recorded() {
    reset_parse_requests();
    let v: Vec<u32> = reserved(1_000_000, 40, 4);
    assert!(v.capacity() >= 10 && v.capacity() < 1_000);
    assert!(parse_requests() >= 40 && parse_requests() < 4_000);
    assert!(largest_request() >= 40);
}

#[test]
fn latin1_keeps_every_byte() {
    assert_eq!(latin1(b"ab\xe9"), "ab\u{e9}");
}

#[test]
fn the_readers_reserve_only_through_the_helper() {
    for (name, text) in [
        ("dif.rs", include_str!("dif.rs")),
        ("ter.rs", include_str!("ter.rs")),
        ("dml.rs", include_str!("dml.rs")),
        ("png.rs", include_str!("png.rs")),
        ("read.rs", include_str!("read.rs")),
    ] {
        let needle = concat!("with_", "capacity");
        assert!(!text.contains(needle), "{name} sizes a vector itself");
        let needle = concat!("vec!", "[");
        assert!(!text.contains(needle), "{name} sizes a vector itself");
    }
}
