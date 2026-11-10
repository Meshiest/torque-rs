use super::*;

fn patched(old: &[u8], new: &[u8]) -> Vec<u8> {
    let patch = diff(old, new).expect("they differ");
    let mut base = old.to_vec();
    apply(&mut base, &patch, usize::MAX >> 4).expect("a patch from diff applies");
    base
}

#[test]
fn equal_bytes_have_no_patch() {
    assert_eq!(diff(b"same", b"same"), None);
    assert_eq!(diff(b"", b""), None);
}

#[test]
fn a_patch_turns_the_old_bytes_into_the_new_ones() {
    let old: Vec<u8> = (0..=255u8).cycle().take(5_000).collect();
    let mut new = old.clone();
    new[10] ^= 1;
    new[12] ^= 1;
    new[4_000..4_050].fill(7);
    assert_eq!(patched(&old, &new), new);
    // Growing and shrinking.
    let mut longer = new.clone();
    longer.extend_from_slice(b"tail");
    assert_eq!(patched(&new, &longer), longer);
    assert_eq!(patched(&longer, &new), new);
    assert_eq!(patched(&old, b"x"), b"x");
    assert_eq!(patched(b"", b"abc"), b"abc");
    assert_eq!(patched(b"abc", b""), b"");
}

#[test]
fn a_small_change_is_a_small_patch_and_near_runs_merge() {
    let old = vec![0u8; 1 << 20];
    let mut new = old.clone();
    new[500_000] = 1;
    new[500_003] = 1;
    let patch = diff(&old, &new).unwrap();
    // The length, one run header and four bytes.
    assert_eq!(patch.len(), 4 + 8 + 4);
    new[900_000] = 9;
    assert_eq!(diff(&old, &new).unwrap().len(), 4 + 8 + 4 + 8 + 1);
}

#[test]
fn a_hostile_patch_is_refused_and_changes_nothing() {
    let base = b"hello world".to_vec();
    let check = |patch: &[u8], max: usize| {
        let mut b = base.clone();
        let r = apply(&mut b, patch, max);
        assert_eq!(b, base, "unchanged after {r:?}");
        r
    };
    assert_eq!(check(&[], 100), Err(PatchError::Truncated));
    assert_eq!(check(&[1, 0], 100), Err(PatchError::Truncated));
    let mut p = 1_000_000u32.to_le_bytes().to_vec();
    assert!(matches!(check(&p, 100), Err(PatchError::TooLarge { .. })));
    // A run past the length.
    p = 4u32.to_le_bytes().to_vec();
    p.extend_from_slice(&3u32.to_le_bytes());
    p.extend_from_slice(&5u32.to_le_bytes());
    p.extend_from_slice(b"abcde");
    assert_eq!(check(&p, 100), Err(PatchError::Range));
    // A run of more bytes than the patch has.
    p = 10u32.to_le_bytes().to_vec();
    p.extend_from_slice(&0u32.to_le_bytes());
    p.extend_from_slice(&5u32.to_le_bytes());
    p.extend_from_slice(b"ab");
    assert_eq!(check(&p, 100), Err(PatchError::Truncated));
    // Offsets that overflow.
    p = 10u32.to_le_bytes().to_vec();
    p.extend_from_slice(&u32::MAX.to_le_bytes());
    p.extend_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(check(&p, 100), Err(PatchError::Range));
}
