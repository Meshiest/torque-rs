//! Lenient readers of keyvalue and parameter text: a map's values arrive as strings and each handler reads them by its own
//! type. A number is read from its leading part (`"12abc"` is 12, `"abc"` is 0, as C's `atof` and `atoi` do); a vector takes
//! the first three numbers; a value that does not read gives the default the caller names. Nothing here panics or allocates
//! past the text's own length.

/// The leading decimal number of `s` (after white space): sign, digits, a fraction and an exponent; 0 when there is none.
/// Never NaN or infinite.
pub fn atof(s: &str) -> f64 {
    let b = s.trim_start().as_bytes();
    let mut end = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        end = 1;
    }
    let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    let int = digits(end);
    end += int;
    let mut frac = 0;
    if b.get(end) == Some(&b'.') {
        frac = digits(end + 1);
        if int + frac > 0 {
            end += 1 + frac;
        }
    }
    if int + frac == 0 {
        return 0.0;
    }
    if matches!(b.get(end), Some(b'e' | b'E')) {
        let mut e = end + 1;
        if matches!(b.get(e), Some(b'+' | b'-')) {
            e += 1;
        }
        let n = digits(e);
        if n > 0 {
            end = e + n;
        }
    }
    let text = std::str::from_utf8(&b[..end]).unwrap_or("0");
    match text.parse::<f64>() {
        Ok(v) if v.is_finite() => v,
        Ok(v) if v.is_infinite() => {
            if v > 0.0 {
                f64::MAX
            } else {
                f64::MIN
            }
        }
        _ => 0.0,
    }
}

/// [`atof`] as an `f32` (clamped to the finite range).
pub fn atof32(s: &str) -> f32 {
    atof(s).clamp(f64::from(f32::MIN), f64::from(f32::MAX)) as f32
}

/// The leading integer of `s` (after white space), saturating; 0 when there is none. A fraction is cut (`"2.9"` is 2).
pub fn atoi(s: &str) -> i64 {
    let b = s.trim_start().as_bytes();
    let (neg, rest) = match b.first() {
        Some(b'-') => (true, &b[1..]),
        Some(b'+') => (false, &b[1..]),
        _ => (false, b),
    };
    let mut v: i64 = 0;
    for c in rest.iter().take_while(|c| c.is_ascii_digit()) {
        v = v.saturating_mul(10).saturating_add(i64::from(c - b'0'));
    }
    if neg { v.saturating_neg() } else { v }
}

/// [`atoi`] clamped to `i32`.
pub fn atoi32(s: &str) -> i32 {
    atoi(s).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// A truth value: a number other than 0, or `true` / `yes` (any case); anything else false.
pub fn boolean(s: &str) -> bool {
    let t = s.trim();
    if t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("yes") {
        return true;
    }
    atof(t) != 0.0
}

/// The first three numbers of `s` (`"1 2 3"`), missing ones 0; None when `s` has no number at all.
pub fn vec3(s: &str) -> Option<[f32; 3]> {
    let mut out = [0.0f32; 3];
    let mut n = 0;
    for (k, t) in s.split_whitespace().take(3).enumerate() {
        out[k] = atof32(t);
        n += 1;
    }
    (n > 0).then_some(out)
}

/// A color `"r g b"` (0..255 each, a fourth number ignored); missing channels 255.
pub fn color255(s: &str) -> [u8; 3] {
    let mut out = [255u8; 3];
    for (k, t) in s.split_whitespace().take(3).enumerate() {
        out[k] = atoi(t).clamp(0, 255) as u8;
    }
    out
}

/// A number as a parameter's text: an integer without a fraction, else the shortest form that reads back.
pub fn format_number(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v}");
    if s.len() <= 24 { s } else { format!("{v:.6}") }
}

/// The unit direction of Source angles (pitch, yaw, roll in degrees): x forward, y left, z up; pitch is positive downward.
pub fn angles_dir(a: [f32; 3]) -> [f32; 3] {
    let (p, y) = (a[0].to_radians(), a[1].to_radians());
    [p.cos() * y.cos(), p.cos() * y.sin(), -p.sin()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_from_their_leading_part() {
        assert_eq!(atof("12abc"), 12.0);
        assert_eq!(atof("  -0.5"), -0.5);
        assert_eq!(atof(".25"), 0.25);
        assert_eq!(atof("1e3x"), 1000.0);
        assert_eq!(atof("1e"), 1.0);
        assert_eq!(atof("abc"), 0.0);
        assert_eq!(atof("-"), 0.0);
        assert_eq!(atof("."), 0.0);
        assert_eq!(atof("1e999"), f64::MAX);
        assert_eq!(atoi("2.9"), 2);
        assert_eq!(atoi("-17 apples"), -17);
        assert_eq!(atoi("99999999999999999999999"), i64::MAX);
        assert_eq!(atoi32("-99999999999"), i32::MIN);
        assert!(boolean("1") && boolean("TRUE") && !boolean("0") && !boolean("no"));
        assert_eq!(vec3("1 2"), Some([1.0, 2.0, 0.0]));
        assert_eq!(vec3(""), None);
        assert_eq!(color255("300 -4 7 9"), [255, 0, 7]);
        assert_eq!(format_number(3.0), "3");
        assert_eq!(format_number(0.5), "0.5");
        assert_eq!(format_number(f64::NAN), "0");
    }

    #[test]
    fn angles_point_along_yaw_and_down_by_pitch() {
        let d = angles_dir([0.0, 90.0, 0.0]);
        assert!(d[0].abs() < 1e-6 && (d[1] - 1.0).abs() < 1e-6);
        let d = angles_dir([90.0, 0.0, 0.0]);
        assert!((d[2] + 1.0).abs() < 1e-6);
    }
}
