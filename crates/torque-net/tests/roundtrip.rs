//! 100,000 random round trips per primitive of the bit layer and the string coder, each written at a random bit offset
//! into a stream that already holds other bits, and the readers over hostile bytes at every bit offset (no panic, an `Err` where the
//! stream is too short). Seeded, so a failure repeats.

use torque_net::NetError;
use torque_net::bits::{Affine, BitReader, BitWriter, ColorI, bit_length};
use torque_net::huffman::Table;
use torque_net::strcoder::{read_string, write_string};
use torque_testkit::Rng;

const ROUNDS: usize = 100_000;

/// The test floats: any kind, and only finite ones.
trait FloatRng {
    /// A float of any kind: finite of any scale, a small one, zero, NaN or infinite.
    fn float(&mut self) -> f32;
    fn finite(&mut self) -> f32;
}

impl FloatRng for Rng {
    fn float(&mut self) -> f32 {
        match self.below(8) {
            0 => f32::from_bits(self.next_u32()),
            1 => 0.0,
            2 => f32::NAN,
            3 => f32::INFINITY,
            4 => (self.next_u32() as f32 / u32::MAX as f32) * 2.0 - 1.0,
            5 => (self.below(2000) as f32 - 1000.0) / 8.0,
            6 => (self.below(200_000) as f32 - 100_000.0) * 0.37,
            _ => (self.next_u32() as f32 / u32::MAX as f32) * 100.0,
        }
    }

    fn finite(&mut self) -> f32 {
        loop {
            let f = self.float();
            if f.is_finite() {
                return f;
            }
        }
    }
}

/// Some junk bits to write before a field so it starts at any bit offset.
fn pad(w: &mut BitWriter, rng: &mut Rng) -> u8 {
    let n = rng.below(8) as u8;
    w.write_bits(rng.next_u32(), n);
    n
}

fn skip(r: &mut BitReader, n: u8) {
    r.read_bits(n).expect("the padding is there");
}

fn frequencies() -> [u32; 256] {
    let mut f = [0u32; 256];
    for (i, slot) in f.iter_mut().enumerate().take(127) {
        *slot = match i as u8 {
            b'a'..=b'z' => 300 + (i as u32 * 37) % 900,
            b'A'..=b'Z' => 20 + (i as u32 * 13) % 90,
            b'0'..=b'9' => 80 + (i as u32 * 7) % 60,
            b' ' => 2800,
            b'/' | b'.' | b'_' | b'-' => 500,
            _ => (i as u32 * 3) % 5,
        };
    }
    f
}

#[test]
fn int_n_flags_and_raw_words_round_trip() {
    let mut rng = Rng::star(0x1234_5678_9abc_def1);
    for _ in 0..ROUNDS {
        let mut w = BitWriter::new();
        let before = pad(&mut w, &mut rng);
        let n = rng.below(33) as u8;
        let v = rng.next_u32();
        w.write_bits(v, n);
        let flag = rng.flip();
        w.write_flag(flag);
        let byte = rng.next_u32() as u8;
        w.write_u8(byte);
        let word = rng.next_u32();
        w.write_u32(word);
        let f = rng.float();
        w.write_f32(f);
        assert!(!w.is_overflowed());
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        skip(&mut r, before);
        let mask = if n == 32 { u32::MAX } else { (1u32 << n) - 1 };
        assert_eq!(r.read_bits(n), Ok(v & mask));
        assert_eq!(r.read_flag(), Ok(flag));
        assert_eq!(r.read_u8(), Ok(byte));
        assert_eq!(r.read_u32(), Ok(word));
        assert_eq!(r.read_f32().map(f32::to_bits), Ok(f.to_bits()));
        assert!(r.bits_left() < 8);
    }
}

#[test]
fn ranged_integers_round_trip_in_range_and_wrap_out_of_it() {
    let mut rng = Rng::star(0x0bad_cafe_dead_beef);
    for _ in 0..ROUNDS {
        let lo = rng.next_u32() >> rng.below(32);
        let span = rng.next_u32() >> rng.below(32);
        let hi = lo.saturating_add(span);
        let v = lo + rng.below(u64::from(hi - lo) + 1) as u32;
        let mut w = BitWriter::new();
        let before = pad(&mut w, &mut rng);
        w.write_ranged_u32(v, lo, hi);
        assert_eq!(
            w.bits(),
            usize::from(before) + usize::from(bit_length(hi - lo))
        );
        // Out of range: the low bits survive, nothing panics.
        let wild = rng.next_u32();
        w.write_ranged_u32(wild, lo, hi);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        skip(&mut r, before);
        assert_eq!(r.read_ranged_u32(lo, hi), Ok(v));
        let bits = bit_length(hi - lo);
        let mask = if bits == 32 {
            u32::MAX
        } else {
            (1u32 << bits) - 1
        };
        assert_eq!(
            r.read_ranged_u32(lo, hi),
            Ok((wild.wrapping_sub(lo) & mask).wrapping_add(lo))
        );
    }
}

#[test]
fn ranged_floats_come_back_within_one_step_and_re_encode_to_the_same_bits() {
    let mut rng = Rng::star(0x7777_1111_2222_3333);
    for _ in 0..ROUNDS {
        let min = rng.finite().clamp(-1.0e6, 1.0e6);
        let max = min + rng.finite().abs().clamp(0.001, 1.0e6);
        let bits = 1 + rng.below(24) as u8;
        let v = rng.float();
        let mut w = BitWriter::new();
        let before = pad(&mut w, &mut rng);
        w.write_ranged_f32(v, min, max, bits);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        skip(&mut r, before);
        let back = r.read_ranged_f32(min, max, bits).expect("bits are there");
        if v.is_finite() {
            let step = (max - min) / (((1u64 << bits) - 1) as f32);
            let clamped = v.clamp(min, max);
            let tolerance = step * 1.01 + (max - min) * 1.0e-5 + max.abs().max(min.abs()) * 1.0e-6;
            assert!(
                (back - clamped).abs() <= tolerance
                    && back >= min - tolerance
                    && back <= max + tolerance,
                "v {v} in {min}..{max} at {bits} bits came back {back}"
            );
        }
        // Whatever came back, writing it again gives the same field (the stream is a fixed point).
        let mut again = BitWriter::new();
        pad_same(&mut again, before);
        again.write_ranged_f32(back, min, max, bits);
        let again = again.into_bytes();
        let mut r2 = BitReader::new(&again);
        skip(&mut r2, before);
        let back2 = r2.read_ranged_f32(min, max, bits).expect("bits are there");
        assert!(
            (back2 - back).abs()
                <= (max - min) / (((1u64 << bits) - 1) as f32) * 1.01 + 1.0e-3 * (1.0 + max.abs()),
            "{back} then {back2}"
        );
    }
}

fn pad_same(w: &mut BitWriter, n: u8) {
    w.write_bits(0, n);
}

#[test]
fn signed_ints_and_unit_floats_round_trip() {
    let mut rng = Rng::star(0x5555_aaaa_5555_aaaa);
    for _ in 0..ROUNDS {
        let bits = 2 + rng.below(31) as u8;
        let limit = if bits == 32 {
            i32::MAX
        } else {
            (1i32 << (bits - 1)) - 1
        };
        let v = (rng.next_u32() as i64 % (2 * i64::from(limit) + 1) - i64::from(limit)) as i32;
        let mut w = BitWriter::new();
        let before = pad(&mut w, &mut rng);
        w.write_signed_int(v, bits);
        let fbits = 1 + rng.below(24) as u8;
        let f = (rng.below(1_000_001) as f32) / 1_000_000.0;
        w.write_float(f, fbits);
        let sbits = 1 + rng.below(23) as u8;
        let s = (rng.below(2_000_001) as f32) / 1_000_000.0 - 1.0;
        w.write_signed_float(s, sbits);
        assert_eq!(
            w.bits(),
            usize::from(before) + usize::from(bits) + usize::from(fbits) + usize::from(sbits)
        );
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        skip(&mut r, before);
        assert_eq!(r.read_signed_int(bits), Ok(v));
        let step = 1.0 / (((1u64 << fbits) - 1) as f32);
        let back = r.read_float(fbits).unwrap();
        assert!(
            (back - f).abs() <= step * 1.01 + 1.0e-6,
            "{f} at {fbits} -> {back}"
        );
        let sstep = 2.0 / (((1u64 << sbits) - 1) as f32);
        let back = r.read_signed_float(sbits).unwrap();
        assert!(
            (back - s).abs() <= sstep * 1.01 + 1.0e-6,
            "{s} at {sbits} -> {back}"
        );
    }
}

#[test]
fn colors_normals_and_transforms_round_trip() {
    let mut rng = Rng::star(0x0123_4567_89ab_cdef);
    for _ in 0..ROUNDS {
        let mut w = BitWriter::new();
        let before = pad(&mut w, &mut rng);
        let c = ColorI {
            r: rng.next_u32() as u8,
            g: rng.next_u32() as u8,
            b: rng.next_u32() as u8,
            a: rng.next_u32() as u8,
        };
        w.write_color_i(c);
        let v = [rng.finite(), rng.finite(), rng.finite()];
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let unit = if len > 1.0e-3 && len.is_finite() {
            [v[0] / len, v[1] / len, v[2] / len]
        } else {
            [0.0, 0.0, 1.0]
        };
        w.write_normal_vector(unit, 8);
        let q = [rng.finite() % 1.0, rng.finite() % 1.0, rng.finite() % 1.0];
        let sum: f32 = q.iter().map(|x| x * x).sum();
        let wsign = if rng.flip() { 1.0f32 } else { -1.0 };
        let t = Affine {
            position: [rng.float(), rng.float(), rng.float()],
            quaternion: [q[0], q[1], q[2], wsign * (1.0 - sum.min(1.0)).sqrt()],
        };
        w.write_affine_transform(&t);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        skip(&mut r, before);
        assert_eq!(r.read_color_i(), Ok(c));
        let back = r.read_normal_vector(8).unwrap();
        let dot = unit[0] * back[0] + unit[1] * back[1] + unit[2] * back[2];
        assert!(dot > 0.999, "{unit:?} came back {back:?}");
        let got = r.read_affine_transform().unwrap();
        for (a, b) in got.position.iter().zip(t.position) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
        assert_eq!(got.quaternion[..3], t.quaternion[..3]);
        assert_eq!(got.quaternion[3].is_sign_negative(), t.quaternion[3] < 0.0);
        assert!((got.quaternion[3] - t.quaternion[3]).abs() < 1.0e-3);
    }
}

#[test]
fn compressed_points_round_trip_near_and_far() {
    let mut rng = Rng::star(0xfeed_face_cafe_beef);
    for _ in 0..ROUNDS {
        let scale = 0.5f32;
        let point = [
            rng.finite() % 1000.0,
            rng.finite() % 1000.0,
            rng.finite() % 1000.0,
        ];
        let reach = match rng.below(4) {
            0 => 50.0,
            1 => 10_000.0,
            2 => 50_000.0,
            _ => 1.0e6,
        };
        let p = [
            point[0] + (rng.below(2001) as f32 / 1000.0 - 1.0) * reach,
            point[1] + (rng.below(2001) as f32 / 1000.0 - 1.0) * reach,
            point[2] + (rng.below(2001) as f32 / 1000.0 - 1.0) * reach,
        ];
        let mut w = BitWriter::new();
        w.set_compression_point(point);
        let before = pad(&mut w, &mut rng);
        w.write_compressed_point(p, scale);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        r.set_compression_point(point);
        skip(&mut r, before);
        let back = r.read_compressed_point(scale).unwrap();
        for k in 0..3 {
            // Near points are quantised to the scale; far ones are exact. f32 rounding of `point + d * scale` adds a few ulps.
            let tolerance = scale * 1.01 + p[k].abs().max(point[k].abs()) * 1.0e-6;
            assert!(
                (back[k] - p[k]).abs() <= tolerance,
                "{p:?} against {point:?} came back {back:?}"
            );
        }
    }
}

#[test]
fn strings_round_trip_with_and_without_the_packet_buffer() {
    let table = Table::from_frequencies(&frequencies()).unwrap();
    let mut rng = Rng::star(0x1357_9bdf_2468_ace0);
    let alphabet: Vec<u8> = (b'a'..=b'z').chain(b'0'..=b'9').chain(*b"/._ A").collect();
    for round in 0..ROUNDS {
        let buffered = round % 2 == 0;
        let mut w = BitWriter::new();
        if buffered {
            w.enable_string_buffer();
        }
        let before = pad(&mut w, &mut rng);
        let count = 1 + rng.below(6);
        let mut strings: Vec<Vec<u8>> = Vec::new();
        for _ in 0..count {
            let s: Vec<u8> = match (strings.last(), rng.below(4)) {
                // Share a prefix with the last one, as paths in a packet do.
                (Some(last), 0 | 1) if !last.is_empty() => {
                    let keep = rng.below(last.len() as u64 + 1) as usize;
                    let mut s = last[..keep].to_vec();
                    for _ in 0..rng.below(12) {
                        s.push(alphabet[rng.below(alphabet.len() as u64) as usize]);
                    }
                    s
                }
                (_, 2) => (0..rng.below(300))
                    .map(|_| (rng.next_u32() as u8).max(1))
                    .collect(),
                _ => (0..rng.below(40))
                    .map(|_| alphabet[rng.below(alphabet.len() as u64) as usize])
                    .collect(),
            };
            write_string(&mut w, &table, &s, 255);
            strings.push(s);
        }
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        if buffered {
            r.enable_string_buffer();
        }
        skip(&mut r, before);
        for s in &strings {
            let want: Vec<u8> = s.iter().copied().take(255).collect();
            assert_eq!(read_string(&mut r, &table).unwrap(), want, "round {round}");
        }
        assert!(r.bits_left() < 8);
    }
}

/// Every reader over hostile bytes, at every bit offset and bit length: an `Ok` or an `Err`, never a panic.
#[test]
fn readers_survive_hostile_bytes_at_every_bit_offset() {
    let table = Table::from_frequencies(&frequencies()).unwrap();
    let mut rng = Rng::star(0xdead_0000_beef_1111);
    for round in 0..ROUNDS {
        let len = rng.below(48) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| rng.next_u32() as u8).collect();
        let offset = (round % 8) as u8;
        let bit_len = if rng.flip() {
            bytes.len() * 8
        } else {
            rng.below(bytes.len() as u64 * 8 + 9) as usize
        };
        let mut r = BitReader::with_bit_len(&bytes, bit_len);
        if round % 3 == 0 {
            r.enable_string_buffer();
        }
        let _ = r.read_bits(offset);
        r.set_compression_point([rng.float(), rng.float(), rng.float()]);
        for _ in 0..12 {
            match rng.below(16) {
                0 => drop(r.read_bits(rng.below(40) as u8)),
                1 => drop(r.read_flag()),
                2 => drop(r.read_u8()),
                3 => drop(r.read_u32()),
                4 => drop(r.read_f32()),
                5 => drop(r.read_bytes(rng.below(100) as usize)),
                6 => drop(r.read_ranged_u32(rng.next_u32(), rng.next_u32())),
                7 => drop(r.read_ranged_f32(rng.float(), rng.float(), rng.below(40) as u8)),
                8 => drop(r.read_signed_int(rng.below(40) as u8)),
                9 => drop(r.read_float(rng.below(40) as u8)),
                10 => drop(r.read_signed_float(rng.below(40) as u8)),
                11 => drop(r.read_color_i()),
                12 => drop(r.read_normal_vector(rng.below(40) as u8)),
                13 => drop(r.read_affine_transform()),
                14 => drop(r.read_compressed_point(rng.float())),
                _ => drop(read_string(&mut r, &table)),
            }
        }
        // A read that fails leaves the position alone: asking for more than is left is an Overrun, always.
        let left = r.bits_left();
        if left < 32 {
            assert_eq!(r.read_u32(), Err(NetError::Overrun));
            assert_eq!(r.bits_left(), left);
        }
    }
}

#[test]
fn a_writer_at_its_limit_drops_writes_and_never_panics() {
    let table = Table::from_frequencies(&frequencies()).unwrap();
    let mut rng = Rng::star(0x2222_3333_4444_5555);
    for _ in 0..10_000 {
        let limit = rng.below(400) as usize;
        let mut w = BitWriter::with_limit(limit);
        w.enable_string_buffer();
        for _ in 0..30 {
            match rng.below(8) {
                0 => w.write_bits(rng.next_u32(), rng.below(40) as u8),
                1 => w.write_f32(rng.float()),
                2 => w.write_ranged_f32(rng.float(), rng.float(), rng.float(), rng.below(40) as u8),
                3 => w.write_normal_vector(
                    [rng.float(), rng.float(), rng.float()],
                    rng.below(20) as u8,
                ),
                4 => w.write_compressed_point([rng.float(), rng.float(), rng.float()], rng.float()),
                5 => w.write_signed_int(rng.next_u32() as i32, rng.below(40) as u8),
                6 => w.write_signed_float(rng.float(), rng.below(40) as u8),
                _ => write_string(&mut w, &table, &[b'a'; 40], 255),
            }
        }
        assert!(w.bits() <= limit);
    }
}
