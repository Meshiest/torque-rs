use super::*;

#[test]
fn halves_decode() {
    assert_eq!(f16_to_f32(0x3c00), 1.0);
    assert_eq!(f16_to_f32(0xc000), -2.0);
    assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
    assert!(f16_to_f32(0x7c00).is_infinite() && f16_to_f32(0x7e00).is_nan());
    assert_eq!(non_finite(&[[0.0; 4], [f32::NAN, 0.0, 0.0, 0.0]]), 1);
}

#[test]
fn halves_encode_round_to_nearest() {
    // Every half that is a finite number survives the round trip.
    for h in 0u16..=0xffff {
        let f = f16_to_f32(h);
        if f.is_finite() {
            assert_eq!(f16_to_f32(f32_to_f16(f)).to_bits(), f.to_bits(), "{h:#x}");
        }
    }
    // Between two halves the nearer one; past the range infinity.
    let (a, b) = (f16_to_f32(0x3c00), f16_to_f32(0x3c01));
    assert_eq!(f32_to_f16(a + 0.4 * (b - a)), 0x3c00);
    assert_eq!(f32_to_f16(a + 0.6 * (b - a)), 0x3c01);
    assert_eq!(f32_to_f16(1e6), 0x7c00);
    assert_eq!(f32_to_f16(1e-9), 0);
}

#[test]
fn the_inverse_is_an_inverse() {
    #[rustfmt::skip]
    let m = [2.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0, 0.0, 0.5, 0.25, 0.001, -1.0, 0.0, 0.0, 0.1, 0.0];
    let inv = invert(&m);
    for r in 0..4 {
        for c in 0..4 {
            let v: f32 = (0..4).map(|k| m[k * 4 + r] * inv[c * 4 + k]).sum();
            assert!(
                (v - if r == c { 1.0 } else { 0.0 }).abs() < 1e-4,
                "{r} {c} {v}"
            );
        }
    }
}

#[test]
fn a_cell_s_indices_go_to_runs_of_16_bits_that_draw_every_triangle_as_it_was() {
    use torque_render_scene::hd::clusters::Cluster;
    // 40,000 quads in one batch drawn as one cluster (a cell built without clusters): 160,000 vertices.
    let quads = 40_000u32;
    let indices: Vec<u32> = (0..quads)
        .flat_map(|q| [0, 1, 2, 0, 2, 3].map(|i| 4 * q + i))
        .collect();
    let one = Cluster {
        slot: 0,
        translucent: false,
        first: 0,
        count: indices.len() as u32,
        bounds: [[0.0; 3], [1.0; 3]],
    };
    let (short, runs) = short_indices(&indices, &[one]);
    assert_eq!(short.len(), indices.len());
    assert_eq!(runs.iter().map(|r| r.2).sum::<u32>(), indices.len() as u32);
    for &(_, first, count, base) in &runs {
        for i in first..first + count {
            assert_eq!(u32::from(short[i as usize]) + base, indices[i as usize]);
        }
    }
    // A triangle whose own vertices are too far apart for 16 bits is left out, the rest kept.
    let far = [0u32, 1, 70_000, 4, 5, 6];
    let cl = Cluster {
        slot: 0,
        translucent: false,
        first: 0,
        count: 6,
        bounds: [[0.0; 3], [1.0; 3]],
    };
    let (_, runs) = short_indices(&far, &[cl]);
    assert_eq!(runs.iter().map(|r| r.2).sum::<u32>(), 3);
}
