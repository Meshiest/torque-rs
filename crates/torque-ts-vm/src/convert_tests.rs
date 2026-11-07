//! The conversions give what the runtime's own `atoi`, `atof` and `atob` give.
use super::*;
use crate::fields::atob;

/// Texts that exercise the number readers: partial numbers, white space, signs, bases, exponents, overflow, NUL and
/// non-ASCII bytes.
const CORPUS: &[&[u8]] = &[
    b"",
    b" ",
    b"0",
    b"1",
    b"-1",
    b"+5",
    b"-",
    b"+",
    b" 7",
    b"\t\n 7",
    b"7 ",
    b"1.5abc",
    b"1.5",
    b"-1.5",
    b".5",
    b"5.",
    b"1e3",
    b"1E3",
    b"1e",
    b"1e+",
    b"1e-50",
    b"7e-46",
    b"7.1e-46",
    b"1e-45",
    b"1e39",
    b"0x10",
    b"0X1F",
    b"0x1p4",
    b"010",
    b"true",
    b"TRUE",
    b"True",
    b"tru",
    b"false",
    b"yes",
    b"inf",
    b"-infinity",
    b"nan",
    b"NaN(12)",
    b"2147483647",
    b"2147483648",
    b"-2147483648",
    b"-2147483649",
    b"99999999999999999999",
    b"-99999999999999999999",
    b"4294967295",
    b"12abc34",
    b"abc",
    b"1\x002",
    b"\x001",
    b"true\x00x",
    b"caf\xc3\xa9",
    b"\xff\xfe",
    b"\xc3\xa9 5",
    b"5\xc2\xa0",
];

#[test]
fn the_corpus_is_large_enough() {
    assert!(CORPUS.len() >= 30, "{} cases", CORPUS.len());
}

#[test]
fn integers_are_atoi() {
    for &text in CORPUS {
        assert_eq!(i32::from_script(text), Ok(atoi(text)), "{text:?}");
    }
}

#[test]
fn unsigned_integers_are_atoi_bits() {
    for &text in CORPUS {
        assert_eq!(u32::from_script(text), Ok(atoi(text) as u32), "{text:?}");
    }
}

#[test]
fn floats_are_atof_rounded_to_a_float() {
    for &text in CORPUS {
        let got = f32::from_script(text).unwrap();
        let want = to_f32(atof(text));
        assert_eq!(got.to_bits(), want.to_bits(), "{text:?}");
    }
}

#[test]
fn truth_values_are_atob() {
    for &text in CORPUS {
        assert_eq!(bool::from_script(text), Ok(atob(text)), "{text:?}");
    }
}

#[test]
fn known_values() {
    let int = |t: &[u8]| i32::from_script(t).unwrap();
    assert_eq!(int(b"1.5abc"), 1);
    assert_eq!(int(b""), 0);
    assert_eq!(int(b" 7"), 7);
    assert_eq!(int(b"-"), 0);
    assert_eq!(int(b"0x10"), 0);
    assert_eq!(int(b"1e3"), 1);
    assert_eq!(int(b"99999999999999999999"), i32::MAX);
    assert_eq!(int(b"-99999999999999999999"), i32::MIN);
    assert_eq!(int(b"1\x002"), 1);
    assert_eq!(u32::from_script(b"-1"), Ok(u32::MAX));

    let float = |t: &[u8]| f32::from_script(t).unwrap();
    assert_eq!(float(b"1.5abc"), 1.5);
    assert_eq!(float(b""), 0.0);
    assert_eq!(float(b" 7"), 7.0);
    assert_eq!(float(b"-"), 0.0);
    assert_eq!(float(b"0x10"), 16.0);
    assert_eq!(float(b"1e3"), 1000.0);
    assert_eq!(float(b"1e39"), f32::INFINITY);
    assert_eq!(float(b"1e-50"), 0.0);
    assert!(float(b"nan").is_nan());

    let truth = |t: &[u8]| bool::from_script(t).unwrap();
    assert!(truth(b"true"));
    assert!(truth(b"TRUE"));
    assert!(truth(b"2"));
    assert!(truth(b"1e-45"));
    assert!(!truth(b""));
    assert!(!truth(b"false"));
    assert!(!truth(b"0"));
    assert!(!truth(b"1e-50"));
    assert!(!truth(b"7e-46"));
    assert!(truth(b"7.1e-46"));
}

#[test]
fn text_stops_at_a_nul() {
    assert_eq!(<&str>::from_script(b"ab\0cd"), Ok("ab"));
    assert_eq!(String::from_script(b"ab\0cd"), Ok("ab".to_string()));
    assert_eq!(<&str>::from_script(b""), Ok(""));
}

#[test]
fn text_that_is_not_utf8_is_refused_as_a_str_and_repaired_as_a_string() {
    assert_eq!(<&str>::from_script(b"caf\xc3\xa9"), Ok("caf\u{e9}"));
    assert_eq!(<&str>::from_script(b"\xff"), Err(ConvertError::NotUtf8));
    assert_eq!(String::from_script(b"a\xffb"), Ok("a\u{fffd}b".to_string()));
    let borrowed = <Cow<str>>::from_script(b"plain").unwrap();
    assert!(matches!(borrowed, Cow::Borrowed("plain")));
    assert!(ConvertError::NotUtf8.to_string().contains("UTF-8"));
}

#[test]
fn results_keep_their_values() {
    assert_eq!(().into_script(), Native::Void);
    assert_eq!((-3_i32).into_script(), Native::Int(-3));
    assert_eq!(u32::MAX.into_script(), Native::Int(-1));
    assert_eq!(0.1_f32.into_script(), Native::Float(f64::from(0.1_f32)));
    assert_eq!(true.into_script(), Native::Bool(true));
    assert_eq!("hi".into_script(), Native::Str(b"hi".to_vec()));
    assert_eq!(
        String::from("h\u{e9}").into_script(),
        Native::Str("h\u{e9}".as_bytes().to_vec())
    );
}

#[test]
fn a_float_result_reads_back_the_same() {
    for x in [0.0_f32, -0.0, 1.5, 0.1, 1e-45, f32::MAX, f32::INFINITY] {
        let Native::Float(wide) = x.into_script() else {
            panic!("not a float");
        };
        assert_eq!(to_f32(wide).to_bits(), x.to_bits());
    }
}
