use super::*;

/// A smooth color ramp with a soft alpha.
fn ramp(w: u32, h: u32) -> Vec<u8> {
    (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            [
                (x * 255 / w.max(2)) as u8,
                (y * 255 / h.max(2)) as u8,
                ((x + y) * 127 / (w + h)) as u8,
                (255 - x * 128 / w.max(1)) as u8,
            ]
        })
        .collect()
}

fn psnr(a: &[u8], b: &[u8], channels: &[usize]) -> f64 {
    let (mut sum, mut n) = (0.0f64, 0usize);
    for (p, q) in a.chunks(4).zip(b.chunks(4)) {
        for &c in channels {
            let d = f64::from(p[c]) - f64::from(q[c]);
            sum += d * d;
            n += 1;
        }
    }
    if sum == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0f64 * 255.0 / (sum / n as f64)).log10()
}

#[test]
fn each_format_round_trips_a_smooth_ramp() {
    let (w, h) = (64, 32);
    let rgba = ramp(w, h);
    for (format, channels) in [
        (BlockFormat::Bc4, &[0][..]),
        (BlockFormat::Bc5, &[0, 1][..]),
        (BlockFormat::Bc7, &[0, 1, 2, 3][..]),
    ] {
        let blocks = encode(format, &rgba, w, h).unwrap();
        assert_eq!(blocks.len() as u64, format.level_bytes(w, h));
        let back = decode(format, &blocks, w, h).unwrap();
        let p = psnr(&rgba, &back, channels);
        assert!(p > 40.0, "{}: {p:.1} dB", format.name());
    }
}

#[test]
fn a_decoded_bc4_or_bc5_texel_is_what_a_gpu_samples() {
    let rgba = ramp(8, 8);
    for format in [BlockFormat::Bc4, BlockFormat::Bc5] {
        let back = decode(format, &encode(format, &rgba, 8, 8).unwrap(), 8, 8).unwrap();
        for p in back.chunks(4) {
            assert_eq!(p[2], 0);
            assert_eq!(p[3], 255);
            if format == BlockFormat::Bc4 {
                assert_eq!(p[1], 0);
            }
        }
    }
}

#[test]
fn levels_of_any_size_encode_with_their_edges_repeated() {
    for (w, h) in [(1, 1), (2, 1), (3, 5), (5, 3), (17, 9), (4, 20)] {
        let rgba = ramp(w, h);
        for format in [BlockFormat::Bc4, BlockFormat::Bc5, BlockFormat::Bc7] {
            let blocks = encode(format, &rgba, w, h).unwrap();
            assert_eq!(blocks.len() as u64, format.level_bytes(w, h), "{w} x {h}");
            let back = decode(format, &blocks, w, h).unwrap();
            assert_eq!(back.len(), rgba.len());
        }
    }
    // A flat 1 x 1 comes back exactly.
    let one = [200u8, 100, 50, 255];
    let back = decode(
        BlockFormat::Bc7,
        &encode(BlockFormat::Bc7, &one, 1, 1).unwrap(),
        1,
        1,
    )
    .unwrap();
    assert_eq!(back, one);
}

#[test]
fn sizes_that_do_not_match_their_bytes_are_refused() {
    assert!(encode(BlockFormat::Bc7, &[0; 15], 2, 2).is_err());
    assert!(encode(BlockFormat::Bc7, &[], 0, 0).is_err());
    assert!(decode(BlockFormat::Bc4, &[0; 7], 4, 4).is_err());
    assert!(decode(BlockFormat::Bc5, &[0; 16], 5, 4).is_err());
    assert!(decode(BlockFormat::Bc7, &[0; 16], 0, 4).is_err());
}

#[test]
fn any_block_bytes_decode_without_a_panic() {
    // Every BC7 mode byte (including the reserved all-zero one) and noise.
    let mut x = 0x9e37_79b9u32;
    let mut noise = || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x as u8
    };
    for first in 0..=255u8 {
        let mut block = [0u8; 16];
        block[0] = first;
        for b in &mut block[1..] {
            *b = noise();
        }
        for format in [BlockFormat::Bc4, BlockFormat::Bc5, BlockFormat::Bc7] {
            decode(format, &block[..format.block_bytes()], 4, 4).unwrap();
        }
    }
}
