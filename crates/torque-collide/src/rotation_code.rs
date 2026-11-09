//! A rotation as three whole numbers: the quaternion's three smallest components in 15 bits each, with the index of the one
//! left out. Shared by everything that puts a rotation on the wire at this precision.
//!
//! Only IEEE operations (a square root, no transcendental), so the codes and what they decode to are the same bits on every
//! target. [`pack`] settles a near tie between two components on the lowest index; a caller that wants a code that packs back to
//! itself packs the decoded rotation again until the code stops changing (a round or two).

/// The middle of a component's range: the code of 0.
pub const ROT_MID: i32 = 16_383;
/// The bits of a component.
pub const ROT_BITS: u8 = 15;

/// How near the largest a component counts as tied with it.
const ROT_TIE: f64 = 5.0e-3;
/// The largest a component that is left in may be: a little past 1/sqrt 2, for the one a near tie keeps.
const ROT_RANGE: f64 = 0.7125;

/// The largest component of `q` (the lowest index of a tie), made positive by turning the whole quaternion, and the other three
/// coded as `ROT_MID + c / 0.7125 * ROT_MID` rounded. A quaternion that is not finite or is zero is the identity.
pub fn pack(q: [f32; 4]) -> (u8, [u16; 3]) {
    let mut v = q.map(f64::from);
    let len = v.iter().map(|c| c * c).sum::<f64>().sqrt();
    if !(len.is_finite() && len > 1.0e-12) {
        return (3, [ROT_MID as u16; 3]);
    }
    for c in &mut v {
        *c /= len;
    }
    // The lowest index of the components within a tolerance of the largest: a near tie keeps one choice from round to round
    // (two components that close are both below `ROT_RANGE`, so either may be left out).
    let most = v.iter().fold(0.0f64, |m, c| m.max(c.abs()));
    let big = v
        .iter()
        .position(|c| c.abs() >= most - ROT_TIE)
        .unwrap_or(3);
    if v[big] < 0.0 {
        for c in &mut v {
            *c = -*c;
        }
    }
    let mut out = [0u16; 3];
    let mut j = 0;
    for (k, c) in v.iter().enumerate() {
        if k == big {
            continue;
        }
        let x = (c / ROT_RANGE * f64::from(ROT_MID)).round() as i64 + i64::from(ROT_MID);
        out[j] = x.clamp(0, 2 * i64::from(ROT_MID)) as u16;
        j += 1;
    }
    (big as u8, out)
}

/// The quaternion `x, y, z, w` of a code. A value past its range (a hostile record's) is held to it.
pub fn unpack(index: u8, rot: [u16; 3]) -> [f32; 4] {
    let small = rot.map(|r| {
        let r = i64::from(r).min(2 * i64::from(ROT_MID));
        (r - i64::from(ROT_MID)) as f64 / f64::from(ROT_MID) * ROT_RANGE
    });
    let rest = small.iter().map(|c| c * c).sum::<f64>();
    let big = (1.0 - rest).max(0.0).sqrt();
    let index = usize::from(index.min(3));
    let mut q = [0.0f64; 4];
    let mut j = 0;
    for (k, c) in q.iter_mut().enumerate() {
        if k == index {
            *c = big;
        } else {
            *c = small[j];
            j += 1;
        }
    }
    // Normalized in doubles before the floats are taken: the three small components were rounded to 15 bits.
    let len = q.iter().map(|c| c * c).sum::<f64>().sqrt();
    if len > 0.0 {
        for c in &mut q {
            *c /= len;
        }
    }
    q.map(|c| c as f32)
}
