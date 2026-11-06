use std::time::Instant;

use super::*;

/// The unkeyed multiply-fold this module once used: `h = (rotl(h, 23) ^ block) * K` over 8-byte blocks from a seed.
fn old_hash(seed: u64, bytes: &[u8]) -> u64 {
    const K: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut h = seed;
    let mut mix = |v: u64| h = (h.rotate_left(23) ^ v).wrapping_mul(K);
    let (chunks, rest) = bytes.as_chunks::<8>();
    for c in chunks {
        mix(u64::from_le_bytes(*c));
    }
    let mut w = [0u8; 8];
    w[..rest.len()].copy_from_slice(rest);
    mix(u64::from_le_bytes(w) ^ ((rest.len() as u64) << 56));
    let h = h ^ (h >> 32);
    h.wrapping_mul(K) ^ (h >> 29)
}

/// 2^bits names that collide whole under [`old_hash`] whatever its seed: each pair of blocks is either `(a, b)` or
/// `(a ^ 2^63, b ^ 2^22)`. A top-bit change survives the multiply as a top-bit change (K is odd), the rotation moves it to bit
/// 22, and the next block cancels it.
fn colliding_names(bits: u32) -> Vec<Vec<u8>> {
    (0..1u32 << bits)
        .map(|n| {
            let mut name = Vec::new();
            for k in 0..bits {
                let (mut a, mut b) = (
                    0x6161_6161_6161_6161u64 + u64::from(k),
                    0x6262_6262_6262_6262u64,
                );
                if n >> k & 1 == 1 {
                    a ^= 1 << 63;
                    b ^= 1 << 22;
                }
                name.extend_from_slice(&a.to_le_bytes());
                name.extend_from_slice(&b.to_le_bytes());
            }
            name
        })
        .collect()
}

#[test]
fn names_built_to_collide_under_the_old_hash_spread_under_the_keyed_one() {
    let names = colliding_names(12);
    assert_eq!(names.len(), 4_096);
    for seed in [1u64, 0xdead_beef, u64::MAX] {
        let first = old_hash(seed, &names[0]);
        assert!(
            names.iter().all(|n| old_hash(seed, n) == first),
            "the old hash collides"
        );
    }
    let t = Instant::now();
    let mut m: NameMap<Vec<u8>, usize> = NameMap::default();
    for (i, n) in names.iter().enumerate() {
        m.insert(n.clone(), i);
    }
    for (i, n) in names.iter().enumerate() {
        assert_eq!(m.get(n), Some(&i));
    }
    // Colliding keys would cost 4,096^2 / 2 comparisons of 192-byte names; spread, a few milliseconds even unoptimised.
    assert!(t.elapsed().as_millis() < 500, "{:?}", t.elapsed());
    let b = NameBuild::default();
    let distinct: std::collections::HashSet<u64> = names
        .iter()
        .map(|n| std::hash::BuildHasher::hash_one(&b, n))
        .collect();
    assert!(distinct.len() > 4_000, "{} distinct hashes", distinct.len());
}
