use super::*;

#[test]
fn a_block_list_round_trips() {
    let hashes = [[1u8; BLOCK_HASH_LEN], [2u8; BLOCK_HASH_LEN]];
    let bytes = encode_block_list(7, &hashes);
    let ClientMessage::BlockList { id, hashes: got } = decode(&bytes).unwrap() else {
        panic!("not a block list");
    };
    assert_eq!(id, 7);
    assert_eq!(got, [hashes[0], hashes[1]].concat());
}

#[test]
fn data_round_trips() {
    let bytes = encode_data(9, 3, &[5, 6, 7]);
    assert_eq!(
        decode(&bytes).unwrap(),
        ClientMessage::Data {
            id: 9,
            start: 3,
            bytes: &[5, 6, 7]
        }
    );
}

#[test]
fn hostile_messages_are_errors() {
    let long = vec![0u8; MAX_RUN_BLOCKS as usize * BLOCK_BYTES + 1];
    let cases: Vec<Vec<u8>> = vec![
        vec![],
        vec![TAG_DATA],
        vec![TAG_DATA, 1, 0, 0],
        encode_data(1, 0, &[]),
        encode_data(1, 0, &long),
        vec![0, 1, 0, 0, 0],
        [encode_block_list(1, &[]), vec![1]].concat(),
    ];
    for case in cases {
        assert!(decode(&case).is_err(), "{case:?}");
    }
}

#[test]
fn the_largest_run_fits_the_message_limit() {
    let run = vec![0u8; MAX_RUN_BLOCKS as usize * BLOCK_BYTES];
    assert_eq!(encode_data(1, 0, &run).len(), MAX_MESSAGE);
    assert!(decode(&encode_data(1, 0, &run)).is_ok());
}
