//! `jpeg_idct_islow` (jidctint.c): the slow-but-accurate integer IDCT, 13-bit constants, 2 extra bits between the passes, and the
//! range-limit table libjpeg indexes its result with. libjpeg computes in `INT32` and `int` and lets them wrap (C `int` does on common targets), so
//! every operation here wraps: a hostile coefficient overflows the same way. The all-zero-AC shortcuts of both passes are kept: they
//! differ from the full computation exactly when the full one would overflow.
//! Based in part on the work of the Independent JPEG Group; see [`super`] for the credits.

const CONST_BITS: u32 = 13;
const PASS1_BITS: u32 = 2;

const FIX_0_298631336: i32 = 2446;
const FIX_0_390180644: i32 = 3196;
const FIX_0_541196100: i32 = 4433;
const FIX_0_765366865: i32 = 6270;
const FIX_0_899976223: i32 = 7373;
const FIX_1_175875602: i32 = 9633;
const FIX_1_501321110: i32 = 12299;
const FIX_1_847759065: i32 = 15137;
const FIX_1_961570560: i32 = 16069;
const FIX_2_053119869: i32 = 16819;
const FIX_2_562915447: i32 = 20995;
const FIX_3_072711026: i32 = 25172;

/// `DESCALE(x, n)`: add half and shift right arithmetically.
fn descale(x: i32, n: u32) -> i32 {
    x.wrapping_add(1 << (n - 1)) >> n
}

/// The post-IDCT range limit: libjpeg's `sample_range_limit + CENTERJSAMPLE`, indexed by `x & RANGE_MASK` (1023). 0..127 map to
/// 128..255, 128..511 saturate to 255, 512..895 to 0, 896..1023 map to 0..127.
pub(super) fn idct_limit(x: i32) -> u8 {
    let i = x & 1023;
    match i {
        0..=127 => (i + 128) as u8,
        128..=511 => 255,
        512..=895 => 0,
        _ => (i - 896) as u8,
    }
}

/// The IDCT of one block of coefficients (natural order) with the multipliers `quant` (the latched quantisation table, as `int`),
/// into an 8 x 8 sample block.
pub(super) fn islow(coef: &[i16; 64], quant: &[u16; 64], out: &mut [[u8; 8]; 8]) {
    let mut ws = [0i32; 64];
    let deq = |i: usize| i32::from(coef[i]).wrapping_mul(i32::from(quant[i]));
    let mul = |a: i32, b: i32| a.wrapping_mul(b);
    // Pass 1: columns.
    for col in 0..8 {
        let at = |row: usize| col + row * 8;
        if (1..8).all(|row| coef[at(row)] == 0) {
            let dc = deq(at(0)) << PASS1_BITS;
            for row in 0..8 {
                ws[at(row)] = dc;
            }
            continue;
        }
        let z2 = deq(at(2));
        let z3 = deq(at(6));
        let z1 = mul(z2.wrapping_add(z3), FIX_0_541196100);
        let tmp2 = z1.wrapping_add(mul(z3, -FIX_1_847759065));
        let tmp3 = z1.wrapping_add(mul(z2, FIX_0_765366865));
        let z2 = deq(at(0));
        let z3 = deq(at(4));
        let tmp0 = z2.wrapping_add(z3) << CONST_BITS;
        let tmp1 = z2.wrapping_sub(z3) << CONST_BITS;
        let tmp10 = tmp0.wrapping_add(tmp3);
        let tmp13 = tmp0.wrapping_sub(tmp3);
        let tmp11 = tmp1.wrapping_add(tmp2);
        let tmp12 = tmp1.wrapping_sub(tmp2);
        let (o0, o1, o2, o3) = odd(deq(at(7)), deq(at(5)), deq(at(3)), deq(at(1)));
        let n = CONST_BITS - PASS1_BITS;
        ws[at(0)] = descale(tmp10.wrapping_add(o3), n);
        ws[at(7)] = descale(tmp10.wrapping_sub(o3), n);
        ws[at(1)] = descale(tmp11.wrapping_add(o2), n);
        ws[at(6)] = descale(tmp11.wrapping_sub(o2), n);
        ws[at(2)] = descale(tmp12.wrapping_add(o1), n);
        ws[at(5)] = descale(tmp12.wrapping_sub(o1), n);
        ws[at(3)] = descale(tmp13.wrapping_add(o0), n);
        ws[at(4)] = descale(tmp13.wrapping_sub(o0), n);
    }
    // Pass 2: rows.
    for (row, line) in out.iter_mut().enumerate() {
        let w = &ws[row * 8..row * 8 + 8];
        if w[1..].iter().all(|&v| v == 0) {
            let v = idct_limit(descale(w[0], PASS1_BITS + 3));
            *line = [v; 8];
            continue;
        }
        let z2 = w[2];
        let z3 = w[6];
        let z1 = mul(z2.wrapping_add(z3), FIX_0_541196100);
        let tmp2 = z1.wrapping_add(mul(z3, -FIX_1_847759065));
        let tmp3 = z1.wrapping_add(mul(z2, FIX_0_765366865));
        let tmp0 = w[0].wrapping_add(w[4]) << CONST_BITS;
        let tmp1 = w[0].wrapping_sub(w[4]) << CONST_BITS;
        let tmp10 = tmp0.wrapping_add(tmp3);
        let tmp13 = tmp0.wrapping_sub(tmp3);
        let tmp11 = tmp1.wrapping_add(tmp2);
        let tmp12 = tmp1.wrapping_sub(tmp2);
        let (o0, o1, o2, o3) = odd(w[7], w[5], w[3], w[1]);
        let n = CONST_BITS + PASS1_BITS + 3;
        line[0] = idct_limit(descale(tmp10.wrapping_add(o3), n));
        line[7] = idct_limit(descale(tmp10.wrapping_sub(o3), n));
        line[1] = idct_limit(descale(tmp11.wrapping_add(o2), n));
        line[6] = idct_limit(descale(tmp11.wrapping_sub(o2), n));
        line[2] = idct_limit(descale(tmp12.wrapping_add(o1), n));
        line[5] = idct_limit(descale(tmp12.wrapping_sub(o1), n));
        line[3] = idct_limit(descale(tmp13.wrapping_add(o0), n));
        line[4] = idct_limit(descale(tmp13.wrapping_sub(o0), n));
    }
}

/// The odd part (figure 8 of the IJG notes): inputs y7, y5, y3, y1; outputs tmp0..tmp3.
fn odd(t0: i32, t1: i32, t2: i32, t3: i32) -> (i32, i32, i32, i32) {
    let mul = |a: i32, b: i32| a.wrapping_mul(b);
    let z1 = t0.wrapping_add(t3);
    let z2 = t1.wrapping_add(t2);
    let z3 = t0.wrapping_add(t2);
    let z4 = t1.wrapping_add(t3);
    let z5 = mul(z3.wrapping_add(z4), FIX_1_175875602);
    let mut tmp0 = mul(t0, FIX_0_298631336);
    let mut tmp1 = mul(t1, FIX_2_053119869);
    let mut tmp2 = mul(t2, FIX_3_072711026);
    let mut tmp3 = mul(t3, FIX_1_501321110);
    let z1 = mul(z1, -FIX_0_899976223);
    let z2 = mul(z2, -FIX_2_562915447);
    let z3 = mul(z3, -FIX_1_961570560).wrapping_add(z5);
    let z4 = mul(z4, -FIX_0_390180644).wrapping_add(z5);
    tmp0 = tmp0.wrapping_add(z1.wrapping_add(z3));
    tmp1 = tmp1.wrapping_add(z2.wrapping_add(z4));
    tmp2 = tmp2.wrapping_add(z2.wrapping_add(z3));
    tmp3 = tmp3.wrapping_add(z1.wrapping_add(z4));
    (tmp0, tmp1, tmp2, tmp3)
}
