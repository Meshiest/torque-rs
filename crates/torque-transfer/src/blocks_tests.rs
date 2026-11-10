use super::*;

#[test]
fn counts_and_runs_follow_the_block_size() {
    let b = BLOCK_BYTES as u64;
    assert_eq!(block_count(0), 0);
    assert_eq!(block_count(1), 1);
    assert_eq!(block_count(b), 1);
    assert_eq!(block_count(b + 1), 2);
    assert_eq!(run_bytes(3 * b + 5, 1, 2), (b, 3 * b));
    assert_eq!(run_bytes(3 * b + 5, 3, 4), (3 * b, 3 * b + 5));
    assert_eq!(run_bytes(10, 5, 1), (10, 10));
}

#[test]
fn hostile_run_numbers_saturate() {
    assert_eq!(run_bytes(100, u64::MAX, u64::MAX), (100, 100));
    assert_eq!(run_bytes(u64::MAX, u64::MAX, 1), (u64::MAX, u64::MAX));
}

#[test]
fn hashes_are_prefixes_of_sha256() {
    let data = vec![7u8; BLOCK_BYTES + 3];
    let list = block_hashes(&data);
    assert_eq!(list.len(), 2);
    assert_eq!(list[0], block_hash(&data[..BLOCK_BYTES]));
    assert_ne!(list[0], list[1]);
    assert_eq!(content_hash(&data)[..BLOCK_HASH_LEN], block_hash(&data));
}
