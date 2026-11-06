use super::*;

#[test]
fn a_datagram_over_the_most_bytes_is_refused() {
    let mut held = HeldDatagrams::default();
    assert!(held.push(&[7; NET_MAX_DATAGRAM]));
    assert!(!held.push(&[7; NET_MAX_DATAGRAM + 1]));
    assert!(!held.push(&vec![7; 256 * 1024]));
    assert_eq!(
        (held.len(), held.bytes(), held.dropped),
        (1, NET_MAX_DATAGRAM, 2)
    );
    assert_eq!(held.pop().map(|d| d.len()), Some(NET_MAX_DATAGRAM));
    assert!(held.is_empty());
    assert_eq!(held.bytes(), 0);
}

#[test]
fn a_flood_stays_within_the_count_and_the_bytes() {
    // small datagrams: the count holds
    let mut held = HeldDatagrams::default();
    let taken = (0..MAX_INBOX * 2).filter(|_| held.push(&[1; 8])).count();
    assert_eq!(taken, MAX_INBOX);
    assert_eq!(held.dropped, MAX_INBOX as u64);
    // the longest datagrams: the bytes hold
    let mut held = HeldDatagrams::default();
    let taken = (0..MAX_INBOX)
        .filter(|_| held.push(&[1; NET_MAX_DATAGRAM]))
        .count();
    assert_eq!(taken, MAX_HELD_BYTES / NET_MAX_DATAGRAM);
    assert!(held.bytes() <= MAX_HELD_BYTES);
    // reading frees room
    assert!(held.pop().is_some());
    assert!(held.push(&[1; NET_MAX_DATAGRAM]));
    assert!(!held.push(&[1; NET_MAX_DATAGRAM]));
}
