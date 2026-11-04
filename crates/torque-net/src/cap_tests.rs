use super::*;

#[test]
fn a_count_is_bounded_by_the_cap_and_by_the_bits_left() {
    assert_eq!(count("n", 10, 100, 1000, 10), Ok(10));
    assert_eq!(count("n", 100, 100, 100_000, 1), Ok(100));
    assert!(matches!(
        count("n", 101, 100, 100_000, 1),
        Err(NetError::Cap { cap: 100, .. })
    ));
    // 1000 bits hold 100 elements of 10 bits, not 101.
    assert!(count("n", 101, 1000, 1000, 10).is_err());
    assert_eq!(count("n", 100, 1000, 1000, 10), Ok(100));
    // Zero-bit elements are charged one bit.
    assert!(count("n", 9, 1000, 8, 0).is_err());
    assert_eq!(count("n", 8, 1000, 8, 0), Ok(8));
    assert!(count("n", u64::MAX, 1000, usize::MAX, 1).is_err());
}

#[test]
fn vec_for_reserves_only_what_was_allowed() {
    let v: Vec<u64> = vec_for("n", 4, 10, 4096, 64).unwrap();
    assert!(v.capacity() >= 4 && v.capacity() < 1000);
    assert!(vec_for::<u64>("n", u64::from(u32::MAX), usize::MAX, 64, 64).is_err());
}
