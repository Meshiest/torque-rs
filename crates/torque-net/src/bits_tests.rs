use super::*;
use torque_testkit::Rng;

#[test]
fn bits_run_low_first_and_a_word_is_its_little_endian_bytes() {
    let mut w = BitWriter::new();
    w.write_bits(0b101, 3);
    w.write_bits(0b1, 1);
    w.write_u32(0x1234_5678);
    assert_eq!(w.bits(), 36);
    let bytes = w.into_bytes();
    assert_eq!(bytes.len(), 5);
    assert_eq!(bytes[0] & 0xf, 0b1101);
    let mut aligned = BitWriter::new();
    aligned.write_u32(0x1234_5678);
    assert_eq!(aligned.bytes(), &0x1234_5678u32.to_le_bytes());
}

#[test]
fn the_unused_bits_of_the_last_byte_are_zero() {
    let mut w = BitWriter::new();
    w.write_bits(0xffff_ffff, 11);
    assert_eq!(w.bytes(), &[0xff, 0x07]);
}

#[test]
fn a_write_past_the_limit_is_dropped_whole() {
    let mut w = BitWriter::with_limit(12);
    w.write_bits(0xff, 8);
    assert!(!w.is_overflowed());
    w.write_bits(0xff, 8);
    assert!(w.is_overflowed());
    assert_eq!(w.bits(), 8);
    w.write_bits(0x7, 4);
    assert_eq!(w.bits(), 12);
    assert_eq!(w.remaining_bits(), 0);
}

#[test]
fn the_reader_fails_on_overrun_and_stays_put() {
    let mut r = BitReader::new(&[0xab]);
    assert_eq!(r.read_bits(5), Ok(0xab & 0x1f));
    assert_eq!(r.read_bits(4), Err(NetError::Overrun));
    assert_eq!(r.position(), 5);
    assert_eq!(r.read_bits(3), Ok(0xab >> 5));
    assert_eq!(r.bits_left(), 0);
    assert_eq!(r.read_flag(), Err(NetError::Overrun));
    assert_eq!(r.read_bits(0), Ok(0));
    let mut short = BitReader::with_bit_len(&[0xff, 0xff], 9);
    assert_eq!(short.read_bits(10), Err(NetError::Overrun));
    assert_eq!(short.read_bits(9), Ok(0x1ff));
    assert_eq!(BitReader::with_bit_len(&[1], 1000).bits_left(), 8);
}

#[test]
fn ranged_integers_take_the_width_of_the_range_and_do_not_clamp() {
    // 0..1024 is 11 bits; 3..8194 is 13 bits; 0..0 is no bits.
    let mut w = BitWriter::new();
    w.write_ranged_u32(1000, 0, 1024);
    assert_eq!(w.bits(), 11);
    w.write_ranged_u32(8194, 3, 8194);
    assert_eq!(w.bits(), 24);
    w.write_ranged_u32(7, 7, 7);
    assert_eq!(w.bits(), 24);
    // Out of range: the low bits go out.
    let mut w = BitWriter::new();
    w.write_ranged_u32(5000, 0, 1023);
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    assert_eq!(r.read_ranged_u32(0, 1023), Ok(5000 & 1023));
    // hi < lo wraps like C's unsigned subtraction and does not panic.
    let mut w = BitWriter::new();
    w.write_ranged_u32(1, 5, 2);
    assert_eq!(w.bits(), 32);
}

#[test]
fn signed_ints_carry_a_sign_flag_and_the_magnitude() {
    let mut w = BitWriter::new();
    w.write_signed_int(-5, 8);
    w.write_signed_int(100, 8);
    assert_eq!(w.bits(), 16);
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    assert_eq!(r.read_signed_int(8), Ok(-5));
    assert_eq!(r.read_signed_int(8), Ok(100));
}

#[test]
fn ranged_floats_clamp_and_quantise() {
    let mut w = BitWriter::new();
    w.write_ranged_f32(0.5, 0.0, 1.0, 8);
    w.write_ranged_f32(-3.0, 0.0, 1.0, 8);
    w.write_ranged_f32(9.0, 0.0, 1.0, 8);
    assert_eq!(w.bytes(), &[127, 0, 255]);
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    assert!((r.read_ranged_f32(0.0, 1.0, 8).unwrap() - 127.0 / 255.0).abs() < 1e-6);
    assert_eq!(r.read_ranged_f32(0.0, 1.0, 8), Ok(0.0));
    assert_eq!(r.read_ranged_f32(0.0, 1.0, 8), Ok(1.0));
    // NaN, infinities and a zero span do not panic.
    let mut w = BitWriter::new();
    w.write_ranged_f32(f32::NAN, 0.0, 1.0, 8);
    w.write_ranged_f32(f32::INFINITY, 0.0, 1.0, 8);
    w.write_ranged_f32(0.5, 1.0, 1.0, 8);
    w.write_ranged_f32(0.5, 0.0, 1.0, 0);
    assert_eq!(w.bits(), 24);
}

#[test]
fn colors_are_four_bytes_rounded_half_up() {
    let c = ColorI::from_f32([0.0, 1.0, 0.5, 0.2]);
    assert_eq!(
        c,
        ColorI {
            r: 0,
            g: 255,
            b: 128,
            a: 51
        }
    );
    let mut w = BitWriter::new();
    w.write_color_i(c);
    assert_eq!(w.bytes(), &[0, 255, 128, 51]);
    let bytes = w.into_bytes();
    assert_eq!(BitReader::new(&bytes).read_color_i(), Ok(c));
    // NaN and huge values do not panic.
    let _ = ColorI::from_f32([f32::NAN, f32::INFINITY, -1.0e30, 1.0e30]);
}

#[test]
fn a_normal_vector_is_17_bits_and_comes_back_close() {
    let mut w = BitWriter::new();
    w.write_normal_vector([0.0, 0.0, 1.0], 8);
    assert_eq!(w.bits(), 17);
    let bytes = w.into_bytes();
    let n = BitReader::new(&bytes).read_normal_vector(8).unwrap();
    assert!(
        (n[2] - 1.0).abs() < 1e-3 && n[0].abs() < 0.05 && n[1].abs() < 0.05,
        "{n:?}"
    );
    for v in [
        [0.6f32, 0.0, 0.8],
        [-0.3, 0.9, -0.3],
        [1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
    ] {
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let v = [v[0] / len, v[1] / len, v[2] / len];
        let mut w = BitWriter::new();
        w.write_normal_vector(v, 8);
        let bytes = w.into_bytes();
        let back = BitReader::new(&bytes).read_normal_vector(8).unwrap();
        let dot = v[0] * back[0] + v[1] * back[1] + v[2] * back[2];
        assert!(dot > 0.999, "{v:?} {back:?} {dot}");
    }
}

#[test]
fn an_affine_transform_is_193_bits() {
    let t = Affine {
        position: [1.5, -2.0, 3.25],
        quaternion: [0.5, 0.5, 0.5, -0.5],
    };
    let mut w = BitWriter::new();
    w.write_affine_transform(&t);
    assert_eq!(w.bits(), 193);
    let bytes = w.into_bytes();
    let back = BitReader::new(&bytes).read_affine_transform().unwrap();
    assert_eq!(back.position, t.position);
    for (a, b) in back.quaternion.iter().zip(t.quaternion) {
        assert!((a - b).abs() < 1e-6, "{back:?}");
    }
}

#[test]
fn compressed_points_pick_the_smallest_form_and_go_absolute_when_far() {
    let mut w = BitWriter::new();
    w.set_compression_point([10.0, 10.0, 10.0]);
    w.write_compressed_point([12.0, 9.0, 10.5], 0.5);
    assert_eq!(w.bits(), 2 + 3 * 16);
    w.write_compressed_point([10.0 + 20_000.0, 10.0, 10.0], 0.5);
    assert_eq!(w.bits(), 50 + 2 + 3 * 18);
    w.write_compressed_point([10.0 + 70_000.0, 10.0, 10.0], 0.5);
    assert_eq!(w.bits(), 106 + 2 + 3 * 20);
    w.write_compressed_point([10.0 + 1.0e7, 10.0, 10.0], 0.5);
    assert_eq!(w.bits(), 168 + 2 + 96);
    w.write_compressed_point([f32::NAN, 0.0, 0.0], 0.5);
    assert_eq!(w.bits(), 266 + 2 + 96);
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    r.set_compression_point([10.0, 10.0, 10.0]);
    assert_eq!(r.read_compressed_point(0.5), Ok([12.0, 9.0, 10.5]));
    assert_eq!(r.read_compressed_point(0.5), Ok([20_010.0, 10.0, 10.0]));
    assert_eq!(r.read_compressed_point(0.5), Ok([70_010.0, 10.0, 10.0]));
    assert_eq!(r.read_compressed_point(0.5), Ok([1.0e7 + 10.0, 10.0, 10.0]));
    assert!(r.read_compressed_point(0.5).unwrap()[0].is_nan());
}

/// The form (0..=3) a point is written in, from the first two bits.
fn point_form(p: [f32; 3], scale: f32) -> u32 {
    let mut w = BitWriter::new();
    w.write_compressed_point(p, scale);
    BitReader::new(w.bytes()).read_bits(2).unwrap()
}

#[test]
fn the_form_changes_exactly_at_32768_131072_and_524288_units() {
    // Scale 1: r is the distance. Strictly below picks the smaller form.
    for (x, form) in [
        (0.0f32, 0),
        (32_767.0, 0),
        (32_767.996, 0),
        (32_768.0, 1),
        (131_071.0, 1),
        (131_072.0, 2),
        (524_287.0, 2),
        (524_288.0, 3),
        (1.0e9, 3),
        (f32::INFINITY, 3),
        (f32::NAN, 3),
    ] {
        assert_eq!(point_form([x, 0.0, 0.0], 1.0), form, "x = {x}");
    }
    // The distance is the vector's length, and negative components count alike.
    assert_eq!(point_form([-32_768.0, 0.0, 0.0], 1.0), 1);
    assert_eq!(point_form([23_170.0, 23_170.0, 0.0], 1.0), 0);
    assert_eq!(point_form([23_171.0, 23_171.0, 0.0], 1.0), 1);
    // The widths: 16, 18 and 20 bits a component after the 2-bit form, and 96 bits (absolute) beyond.
    for (x, bits) in [
        (1.0f32, 2 + 48),
        (40_000.0, 2 + 54),
        (200_000.0, 2 + 60),
        (9.0e6, 2 + 96),
    ] {
        let mut w = BitWriter::new();
        w.write_compressed_point([x, 0.0, 0.0], 1.0);
        assert_eq!(w.bits(), bits, "x = {x}");
    }
}

#[test]
fn components_go_out_in_x_y_z_order_with_a_sign_flag_and_truncation_toward_zero() {
    let mut w = BitWriter::new();
    w.write_compressed_point([1.5, -2.25, 3.0], 0.5);
    // inv = 2: x = 3, y = (S32)-4.5 = -4, z = 6.
    let mut e = BitWriter::new();
    e.write_bits(0, 2);
    e.write_flag(false);
    e.write_bits(3, 15);
    e.write_flag(true);
    e.write_bits(4, 15);
    e.write_flag(false);
    e.write_bits(6, 15);
    assert_eq!(w.bits(), e.bits());
    assert_eq!(w.bytes(), e.bytes());
    // The difference is taken against the compression point, in f32.
    let mut w = BitWriter::new();
    w.set_compression_point([100.0, 100.0, 100.0]);
    w.write_compressed_point([101.5, 97.75, 103.0], 0.5);
    let mut r = BitReader::new(w.bytes());
    assert_eq!(r.read_bits(2), Ok(0));
    assert_eq!(r.read_signed_int(16), Ok(3));
    assert_eq!(r.read_signed_int(16), Ok(-4));
    assert_eq!(r.read_signed_int(16), Ok(6));
}

#[test]
fn form_three_is_the_absolute_position_whatever_the_compression_point() {
    let mut w = BitWriter::new();
    w.set_compression_point([1.0e6, 0.0, 0.0]);
    w.write_compressed_point([2.0e6, 5.0, 6.0], 1.0);
    let mut r = BitReader::new(w.bytes());
    r.set_compression_point([-77.0, 0.0, 0.0]);
    assert_eq!(r.read_compressed_point(1.0), Ok([2.0e6, 5.0, 6.0]));
}

#[test]
fn compressed_points_round_trip_within_one_unit_of_the_scale() {
    let mut rng = Rng::new(0x1234_5678_9abc_def0);
    let mut rnd = || rng.next_u64();
    for _ in 0..3000 {
        let cp = [
            (rnd() % 2000) as f32 - 1000.0,
            (rnd() % 2000) as f32 - 1000.0,
            (rnd() % 2000) as f32 - 1000.0,
        ];
        let range = [100.0f32, 5_000.0, 50_000.0, 500_000.0][(rnd() % 4) as usize];
        let p = [
            cp[0] + ((rnd() % 20001) as f32 - 10000.0) / 10000.0 * range,
            cp[1] + ((rnd() % 20001) as f32 - 10000.0) / 10000.0 * range,
            cp[2] + ((rnd() % 20001) as f32 - 10000.0) / 10000.0 * range,
        ];
        let scale = [0.01f32, 0.5, 1.0][(rnd() % 3) as usize];
        let mut w = BitWriter::new();
        w.set_compression_point(cp);
        w.write_compressed_point(p, scale);
        let bits = w.bits();
        let bytes = w.into_bytes();
        let mut r = BitReader::with_bit_len(&bytes, bits);
        r.set_compression_point(cp);
        let back = r.read_compressed_point(scale).unwrap();
        assert_eq!(r.bits_left(), 0);
        for k in 0..3 {
            let tol = scale + p[k].abs().max(cp[k].abs()) * 1.0e-6;
            assert!((back[k] - p[k]).abs() <= tol, "{p:?} {back:?} {scale}");
        }
    }
}

#[test]
fn unit_and_signed_floats_use_their_formulas() {
    let mut w = BitWriter::new();
    w.write_float(1.0, 6);
    w.write_float(0.0, 6);
    w.write_signed_float(1.0, 8);
    w.write_signed_float(-1.0, 8);
    w.write_signed_float(0.0, 8);
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    assert_eq!(r.read_bits(6), Ok(63));
    assert_eq!(r.read_bits(6), Ok(0));
    assert_eq!(r.read_bits(8), Ok(255));
    assert_eq!(r.read_bits(8), Ok(0));
    // 0.0 is 127.5 -> 127: truncation (f + 1) * 0.5 * 255.
    assert_eq!(r.read_bits(8), Ok(127));
    let mut r = BitReader::new(&bytes);
    assert_eq!(r.read_float(6), Ok(1.0));
    assert_eq!(r.read_float(6), Ok(0.0));
    assert_eq!(r.read_signed_float(8), Ok(1.0));
    assert_eq!(r.read_signed_float(8), Ok(-1.0));
}

#[test]
fn bytes_are_checked_against_the_bits_left_before_they_are_reserved() {
    let mut r = BitReader::new(&[1, 2, 3]);
    assert!(r.read_bytes(usize::MAX).is_err());
    assert_eq!(r.read_bytes(3), Ok(vec![1, 2, 3]));
    assert_eq!(r.read_bytes(0), Ok(vec![]));
}

#[test]
fn the_string_buffer_is_per_stream_and_cleared() {
    let mut w = BitWriter::new();
    assert!(w.string_buffer_ref().is_none());
    w.enable_string_buffer();
    assert_eq!(w.string_buffer_ref().map(|b| b.len()), Some(256));
    assert!(
        w.string_buffer_ref()
            .is_some_and(|b| b.iter().all(|&x| x == 0))
    );
}

#[test]
fn the_normal_vector_writer_follows_tges_expressions() {
    let pair = |v: [f32; 3]| {
        let mut w = BitWriter::new();
        w.write_normal_vector(v, 8);
        let mut r = BitReader::new(w.bytes());
        (r.read_bits(9).unwrap(), r.read_bits(8).unwrap())
    };
    // (0, 0, 1): phi 0 -> (0 + 1) * 0.5 * 511 = 255.5 -> 255; theta 1 -> 255.
    assert_eq!(pair([0.0, 0.0, 1.0]), (255, 255));
    assert_eq!(pair([0.0, 0.0, -1.0]), (255, 0));
    // (1, 0, 0): phi 0.5 -> 1.5 * 0.5 * 511 = 383.25 -> 383; theta 0 -> 127.5 -> 127.
    assert_eq!(pair([1.0, 0.0, 0.0]), (383, 127));
    assert_eq!(pair([0.0, 1.0, 0.0]), (255, 127));
    // (0, -1, 0): atan2(0, -1) is pi; over the double pi it is 1 -> 511.
    assert_eq!(pair([0.0, -1.0, 0.0]), (511, 127));
    assert_eq!(pair([-1.0, 0.0, 0.0]), (127, 127));
    // A NaN converts to 0.
    assert_eq!(pair([f32::NAN, 0.0, 1.0]), (0, 0));
}

#[test]
fn a_component_is_the_f32_product_converted_toward_zero() {
    // 0.29f32 * 100 rounds to 29.0 in f32: 29.
    let mut w = BitWriter::new();
    w.write_compressed_point([0.29, 0.0, 0.0], 0.01);
    let mut r = BitReader::new(w.bytes());
    assert_eq!(r.read_bits(2), Ok(0));
    assert_eq!(r.read_signed_int(16), Ok(29));
}

#[test]
fn the_compressed_point_reader_adds_an_f32_product() {
    let (scale, cp) = (0.01f32, 1234.5678f32);
    for i in [1i32, 77, 3999, -250] {
        let mut w = BitWriter::new();
        w.write_bits(0, 2);
        w.write_signed_int(i, 16);
        w.write_signed_int(0, 16);
        w.write_signed_int(0, 16);
        let mut r = BitReader::new(w.bytes());
        r.set_compression_point([cp, 0.0, 0.0]);
        let got = r.read_compressed_point(scale).unwrap()[0];
        assert_eq!(got.to_bits(), (cp + i as f32 * scale).to_bits(), "i = {i}");
    }
}

#[test]
fn the_signed_float_reader_is_an_f32_quotient_less_one() {
    for (raw, want) in [(255u32, 1.0f32), (0, -1.0), (127, 254.0f32 / 255.0 - 1.0)] {
        let mut w = BitWriter::new();
        w.write_bits(raw, 8);
        assert_eq!(BitReader::new(w.bytes()).read_signed_float(8), Ok(want));
    }
}

#[test]
fn a_reversed_or_empty_range_does_not_panic() {
    let mut w = BitWriter::new();
    w.write_ranged_f32(0.5, 1.0, 0.0, 8);
    w.write_ranged_f32(f32::NAN, 1.0, 0.0, 8);
    w.write_ranged_f32(0.5, 0.5, 0.5, 8);
    assert_eq!(w.bits(), 24);
}

#[test]
fn colour_channels_keep_the_low_byte_of_the_converted_integer() {
    // NaN converts to 0, the infinities saturate, 2.0 is 510.5 -> 510 -> 254.
    let c = ColorI::from_f32([f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 2.0]);
    assert_eq!(
        c,
        ColorI {
            r: 0,
            g: 255,
            b: 0,
            a: 254
        }
    );
}

#[test]
fn bytes_and_fields_written_and_read_in_bulk_equal_the_same_written_bit_by_bit() {
    let blob: Vec<u8> = (0..40u32).map(|i| (i * 37 + 11) as u8).collect();
    for offset in 0..9u8 {
        for limit in [0usize, 7, 8, 100, 8 * 20 + 3, 400, 4_000] {
            for len in [0usize, 1, 2, 17, 40] {
                let mut bulk = BitWriter::with_limit(limit);
                let mut slow = BitWriter::with_limit(limit);
                bulk.write_bits(0x1ff, offset);
                slow.write_bits(0x1ff, offset);
                bulk.write_bytes(&blob[..len]);
                for &b in &blob[..len] {
                    slow.write_u8(b);
                }
                assert_eq!(bulk.bits(), slow.bits(), "{offset} {limit} {len}");
                assert_eq!(bulk.is_overflowed(), slow.is_overflowed());
                assert_eq!(bulk.bytes(), slow.bytes());
                let bytes = bulk.bytes().to_vec();
                let mut r = BitReader::with_bit_len(&bytes, bulk.bits());
                if r.read_bits(offset).is_ok() && r.bits_left() >= len * 8 {
                    let got = r.read_bytes(len).unwrap();
                    assert_eq!(got, &blob[..len]);
                    assert_eq!(r.position(), usize::from(offset) + len * 8);
                }
            }
        }
    }
}

#[test]
fn a_mark_keeps_the_strings_the_stream_had_whatever_is_written_after_it() {
    let mut w = BitWriter::new();
    w.enable_string_buffer();
    w.string_buffer().unwrap()[0] = 7;
    let mark = w.mark();
    w.string_buffer().unwrap()[0] = 9;
    assert_eq!(w.string_buffer().unwrap()[0], 9);
    w.rewind(mark);
    assert_eq!(w.string_buffer_ref().unwrap()[0], 7);
    // Reading never copies: a mark taken now still shares the buffer after a read.
    let mark = w.mark();
    assert_eq!(w.string_buffer_ref().unwrap()[0], 7);
    assert!(Arc::ptr_eq(
        w.string_buffer.as_ref().unwrap(),
        mark.string_buffer.as_ref().unwrap()
    ));
}
