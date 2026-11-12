use super::*;

#[test]
fn sequential_ids_spread_over_the_buckets() {
    let mut m: IdMap<u32, u32> = IdMap::default();
    for i in 0..100_000 {
        m.insert(i, i);
    }
    assert!((0..100_000).all(|i| m.get(&i) == Some(&i)));
    let low: std::collections::HashSet<u64> = (0..4096u32)
        .map(|i| {
            let mut h = IdHasher::default();
            h.write_u32(i);
            h.finish() & 4095
        })
        .collect();
    // A random map of 4,096 keys onto 4,096 slots fills about 63 % of them.
    assert!(low.len() > 2300, "{} distinct low parts", low.len());
}
