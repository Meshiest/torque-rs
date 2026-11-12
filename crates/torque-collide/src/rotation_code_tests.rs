use super::*;

/// The loop the stable pack replaced, written out as the reference.
fn settled(q: [f32; 4]) -> (u8, [u16; 3]) {
    let (mut index, mut parts) = pack(q);
    for _ in 0..4 {
        let (i, p) = pack(unpack(index, parts));
        if (i, p) == (index, parts) {
            break;
        }
        (index, parts) = (i, p);
    }
    (index, parts)
}

#[test]
fn a_stable_pack_matches_the_repeated_pack() {
    let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    };
    for _ in 0..20_000 {
        let q = [next(), next(), next(), next()];
        assert_eq!(pack_stable(q), settled(q), "{q:?}");
    }
}

#[test]
fn near_ties_and_degenerate_rotations_settle() {
    let tie = 0.5f32;
    for q in [
        [tie, tie, tie, tie],
        [tie + 0.004, tie, tie, tie - 0.004],
        [0.0; 4],
        [f32::NAN, 0.0, 0.0, 1.0],
        [0.0, 0.0, 0.0, 1.0],
    ] {
        assert_eq!(pack_stable(q), settled(q), "{q:?}");
    }
}
