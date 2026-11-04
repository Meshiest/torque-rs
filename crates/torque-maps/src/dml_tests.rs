use super::*;

#[test]
fn a_text_list_ends_at_its_first_empty_line_and_keeps_the_part_after_the_last_slash() {
    let names = parse(b"sky/front\r\nback\\b\r\nup\n\nignored\n").unwrap();
    assert_eq!(names, ["front", "b", "up"]);
    assert_eq!(parse(b"a\nb").unwrap(), ["a", "b"]);
    assert!(parse(b"").unwrap().is_empty());
}

#[test]
fn a_binary_list_reads_its_count() {
    let names = parse(b"\x01\x02\x00\x00\x00\x03a/b\x01c").unwrap();
    assert_eq!(names, ["b", "c"]);
    assert!(parse(b"\x01\x09\x00\x00\x00").is_err());
}
