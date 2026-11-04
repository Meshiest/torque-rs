use super::*;

/// A list of `entries` (name, flags), written as the exporter does.
pub(crate) fn encode(entries: &[(&[u8], u32)]) -> Vec<u8> {
    let mut out = vec![1u8];
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (name, _) in entries {
        out.push(name.len() as u8);
        out.extend_from_slice(name);
    }
    for (_, flags) in entries {
        out.extend_from_slice(&flags.to_le_bytes());
    }
    for _ in 0..entries.len() * 5 {
        out.extend_from_slice(&0u32.to_le_bytes());
    }
    out
}

fn free(_: u64) -> bool {
    true
}

#[test]
fn names_lose_their_path_and_ifl_materials_lose_their_name() {
    let bytes = encode(&[
        (b"plain", 3),
        (b"dir/sub\\leaf", 0),
        (b"anim.ifl", IFL_MATERIAL),
        (b"frame0", IFL_FRAME),
    ]);
    let list = read(&bytes, &mut free).expect("reads");
    assert_eq!(list.bytes, bytes.len());
    let names: Vec<Option<&[u8]>> = list.materials.iter().map(|m| m.name.as_deref()).collect();
    assert_eq!(
        names,
        [
            Some(&b"plain"[..]),
            Some(&b"leaf"[..]),
            None,
            Some(&b"frame0"[..])
        ]
    );
    assert_eq!(list.materials[3].flags, IFL_FRAME);
}

#[test]
fn an_empty_list_is_five_bytes() {
    let bytes = encode(&[]);
    assert_eq!(bytes.len(), 5);
    let list = read(&bytes, &mut free).expect("reads");
    assert!(list.materials.is_empty());
    assert_eq!(list.bytes, 5);
}

#[test]
fn hostile_lists_are_errors_not_panics() {
    assert!(read(&[], &mut free).is_err());
    assert!(read(&[2, 0, 0, 0, 0], &mut free).is_err());
    assert!(read(&[1, 0, 0], &mut free).is_err());
    // A count over the cap, and a count the bytes cannot hold.
    let mut huge = vec![1u8];
    huge.extend_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        read(&huge, &mut free),
        Err(ShapeError::Cap { .. })
    ));
    let mut short = vec![1u8];
    short.extend_from_slice(&(MAX_MATERIALS as u32).to_le_bytes());
    assert!(read(&short, &mut free).is_err());
    // A name that runs past the end.
    assert!(read(&[1, 1, 0, 0, 0, 9, b'a'], &mut free).is_err());
    // The flag arrays cut short.
    let mut cut = encode(&[(b"a", 0)]);
    cut.pop();
    assert!(read(&cut, &mut free).is_err());
}

#[test]
fn an_empty_name_ends_the_list_with_the_names_read_so_far() {
    // Torque stops at the first name whose first byte is NUL, keeps the names before it and reads no flags.
    for second in [&[0u8][..], &[1u8, 0][..]] {
        let mut bytes = vec![1u8, 3, 0, 0, 0, 1, b'a'];
        bytes.extend_from_slice(second);
        bytes.extend_from_slice(&[1, b'c', 9, 9, 9]);
        let list = read(&bytes, &mut free).expect("reads");
        assert_eq!(list.materials.len(), 1);
        assert_eq!(list.materials[0].name.as_deref(), Some(&b"a"[..]));
        assert_eq!(list.materials[0].flags, 0);
        assert_eq!(list.bytes, 7 + second.len());
    }
    // An empty first name: no materials.
    let list = read(&[1, 1, 0, 0, 0, 0], &mut free).expect("reads");
    assert!(list.materials.is_empty());
    assert_eq!(list.bytes, 6);
}

#[test]
fn a_budget_that_cannot_pay_ends_the_read() {
    let bytes = encode(&[(b"a", 0)]);
    assert_eq!(read(&bytes, &mut |_| false), Err(ShapeError::OutOfBudget));
}
