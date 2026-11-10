//! A host's [`TextRules`] replace the defaults for the whole process (this test binary installs its own).

use torque_ts_text::{
    TextRules, atof, f64_to_u32, format_g, install_rules, scan_floats, sort_by, sprintf_double,
};

/// Rules that differ from every default in a way a test can see.
struct Marked;

impl TextRules for Marked {
    fn nan(&self, negative: bool, _tail: &[u8]) -> f64 {
        f64::from_bits(if negative {
            0xFFF8_0000_0000_0001
        } else {
            0x7FF8_0000_0000_0001
        })
    }

    fn scan_nan(&self, _negative: bool, _tail: &[u8]) -> Option<(f32, usize)> {
        None
    }

    fn nan_text(&self, _v: f64) -> String {
        "NaN!".to_string()
    }

    fn f64_to_u32(&self, _x: f64) -> u32 {
        7
    }

    fn double_as_word(&self, _v: f64) -> Option<u32> {
        Some(42)
    }

    fn sort(&self, order: &mut [usize], _cmp: &mut dyn FnMut(usize, usize) -> i32) {
        order.reverse();
    }
}

#[test]
fn installed_rules_are_used_and_the_first_installation_wins() {
    assert!(install_rules(&Marked));
    assert!(!install_rules(&torque_ts_text::DefaultRules));
    assert_eq!(atof(b"nan").to_bits(), 0x7FF8_0000_0000_0001);
    assert_eq!(atof(b"-nan").to_bits(), 0xFFF8_0000_0000_0001);
    assert_eq!(format_g(f64::NAN), "NaN!");
    assert_eq!(f64_to_u32(1e30), 7);
    assert_eq!(
        f64_to_u32(-1.0),
        0xFFFF_FFFF,
        "in range: the rules are not asked"
    );
    assert_eq!(sprintf_double(b"%d", 1.0, 19), b"42");
    let mut out = [0.0f32; 2];
    assert_eq!(scan_floats(b"1 nan", &mut out), 1);
    let mut v = [1, 2, 3];
    sort_by(&mut v, |a, b| a - b);
    assert_eq!(v, [3, 2, 1]);
}

#[test]
fn atoi_stops_at_a_nul_as_at_any_other_byte_and_reads_only_what_it_takes() {
    use torque_ts_text::atoi;
    assert_eq!(atoi(b"12\x0034"), 12);
    assert_eq!(atoi(b" \t-7x"), -7);
    assert_eq!(atoi(b"\x00 5"), 0);
    assert_eq!(atoi(b"+"), 0);
    assert_eq!(atoi(b"99999999999"), i32::MAX);
    // The rest of a megabyte's text after the digits is not read: 65,536 calls over it stay quick.
    let mut text = b"42>".to_vec();
    text.resize(1 << 20, b'x');
    let start = std::time::Instant::now();
    let sum: i64 = (0..65_536).map(|_| i64::from(atoi(&text))).sum();
    assert_eq!(sum, 42 * 65_536);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}
