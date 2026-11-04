use super::*;

#[test]
fn sorting_is_charged_by_its_comparisons() {
    // n elements take about n * log2(n) comparisons: n times the bits of n, rounded up.
    assert_eq!(sort_work(0), 0);
    assert_eq!(sort_work(1), 1);
    assert_eq!(sort_work(2), 4);
    assert_eq!(sort_work(1000), 10_000);
    assert_eq!(sort_work(1 << 20), (1 << 20) * 21);
}

#[test]
fn a_dump_line_carries_the_first_4096_bytes_of_its_name_and_value() {
    let name = vec![b'n'; DUMP_PART_BYTES + 10];
    let value = vec![b'v'; DUMP_PART_BYTES + 10];
    let line = field_line(&name, &value);
    assert_eq!(line.len(), 2 + DUMP_PART_BYTES + 4 + DUMP_PART_BYTES + 1);
    // The value is expanded (a newline is `\n`, two characters); the name is not.
    assert_eq!(field_line(b"a", b"b\n"), br#"  a = "b\n""#);
}
