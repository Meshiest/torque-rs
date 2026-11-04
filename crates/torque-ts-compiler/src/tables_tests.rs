use super::*;

#[test]
fn strings_dedupe_by_case_and_tag_and_pad_short_tags() {
    let mut t = StringTable::default();
    assert_eq!(t.add(b"Foo", true, false), 0);
    assert_eq!(t.add(b"foo", true, false), 4);
    assert_eq!(
        t.add(b"FOO", false, false),
        0,
        "case-insensitive finds the first"
    );
    assert_eq!(t.add(b"foo", true, true), 8, "a tag is its own entry");
    assert_eq!(t.add(b"x", true, false), 15);
    assert_eq!(t.bytes(), b"Foo\0foo\0foo\0\0\0\0x\0");
    assert_eq!(t.add(b"longer!", true, true), 17);
    assert_eq!(t.len(), 25);
}

#[test]
fn floats_compare_with_equality() {
    let mut t = FloatTable::default();
    assert_eq!(t.add(1.5), 0);
    assert_eq!(t.add(-0.0), 1);
    assert_eq!(t.add(0.0), 1);
    assert_eq!(t.add(f64::NAN), 2);
    assert_eq!(t.add(f64::NAN), 3);
    assert_eq!(t.add(1.5), 0);
    assert!(t.values()[1].is_sign_negative());
}

#[test]
fn identifiers_list_newest_offset_first_and_first_position_first() {
    let mut t = IdentTable::default();
    for (offset, ip) in [(0, 1), (5, 3), (0, 7), (0, 9), (5, 11), (9, 12)] {
        t.add(offset, ip);
    }
    let e = t.entries();
    assert_eq!(e.len(), 3);
    assert_eq!((e[0].string_index, e[0].positions.clone()), (9, vec![12]));
    assert_eq!(
        (e[1].string_index, e[1].positions.clone()),
        (5, vec![3, 11])
    );
    assert_eq!(
        (e[2].string_index, e[2].positions.clone()),
        (0, vec![1, 9, 7])
    );
}
