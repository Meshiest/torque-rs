//! A `.glb` whose import does not fit the room a staged shape read begins with fails once for good: it is not read again on
//! later ticks.

use std::cell::Cell;
use std::rc::Rc;

use torque_client::predict::FileReader;
use torque_client::predict::drive::{MAX_SHAPE_BYTES, SHAPE_WORK_PER_BYTE, ShapeCache, Staged};

/// A `.glb` with no nodes and one buffer of `n` zero bytes: its read, its import and its buffer are each charged by its size.
fn glb(n: usize) -> Vec<u8> {
    let mut json =
        format!(r#"{{"asset":{{"version":"2.0"}},"buffers":[{{"byteLength":{n}}}]}}"#).into_bytes();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut out = b"glTF".to_vec();
    out.extend(2u32.to_le_bytes());
    out.extend(((28 + json.len() + n) as u32).to_le_bytes());
    out.extend((json.len() as u32).to_le_bytes());
    out.extend(b"JSON");
    out.extend(json);
    out.extend((n as u32).to_le_bytes());
    out.extend(b"BIN\0");
    out.extend(vec![0u8; n]);
    out
}

#[test]
fn a_glb_too_costly_for_a_ticks_room_fails_once_and_is_not_read_again() {
    let file = glb(6 << 20);
    let reads = Rc::new(Cell::new(0));
    let counted = Rc::clone(&reads);
    let reader: FileReader = Rc::new(move |_: &[u8]| {
        counted.set(counted.get() + 1);
        Some(file.clone())
    });
    let mut cache = ShapeCache::default();
    let budget = SHAPE_WORK_PER_BYTE * MAX_SHAPE_BYTES as u64;
    for tick in 0..4 {
        let mut work = 0;
        let got = cache.get_staged(b"kit/big.glb", Some(&reader), &mut work, budget, tick, 16);
        assert!(matches!(got, Staged::Failed), "tick {tick}: {got:?}");
    }
    assert_eq!(reads.get(), 1);
}
