use super::*;
use std::rc::Rc;

fn call(args: &[&[u8]]) -> NativeCall {
    NativeCall {
        argv: std::iter::once(&b"f"[..])
            .chain(args.iter().copied())
            .map(<[u8]>::to_vec)
            .collect(),
        this_object: None,
        from_bytecode: false,
        name: b"f".to_vec(),
        namespace: Rc::from(&b""[..]),
        file: None,
    }
}

#[test]
fn a_missing_argument_reads_as_empty_and_zero() {
    let c = call(&[b"7"]);
    assert_eq!(c.argc(), 2);
    for i in [2, 3, usize::MAX] {
        assert_eq!(c.arg(i), b"");
        assert_eq!(c.cstr(i), b"");
        assert_eq!(c.int(i), 0);
        assert_eq!(c.float(i).to_bits(), 0);
        assert!(!c.boolean(i));
        assert_eq!(c.vec3(i), [0.0; 3]);
        assert!(c.rest(i).is_empty());
    }
    assert_eq!(c.arg(0), b"f");
}

#[test]
fn arg_keeps_a_nul_and_cstr_cuts_there() {
    let c = call(&[b"ab\0cd", b"\0x"]);
    assert_eq!(c.arg(1), b"ab\0cd");
    assert_eq!(c.cstr(1), b"ab");
    assert_eq!(c.arg(2), b"\0x");
    assert_eq!(c.cstr(2), b"");
}

#[test]
fn numbers_stop_at_a_nul() {
    let c = call(&[b"12\x0034", b"1.5\x002.5", b"true\0x", b"1 2\x003"]);
    assert_eq!(c.int(1), 12);
    assert_eq!(c.float(2), 1.5);
    assert!(c.boolean(3));
    assert_eq!(c.vec3(4), [1.0, 2.0, 0.0]);
}

#[test]
fn int_reads_like_atoi() {
    for (text, want) in [
        (&b"  -42abc"[..], -42),
        (b"+7", 7),
        (b"\x0b\x0c5", 5),
        (b"0x10", 0),
        (b"1.9", 1),
        (b"99999999999", i32::MAX),
        (b"-99999999999", i32::MIN),
        (b"abc", 0),
        (b"", 0),
    ] {
        assert_eq!(call(&[text]).int(1), want, "{text:?}");
    }
}

#[test]
fn float_is_atof_rounded_to_a_float() {
    for (text, want) in [
        (&b" -2.5e1x"[..], -25.0),
        (b"0x1p4", 16.0),
        (b".5", 0.5),
        (b"1e400", f32::INFINITY),
        (b"-inf", f32::NEG_INFINITY),
        (b"1e-50", 0.0),
        (b"abc", 0.0),
    ] {
        assert_eq!(call(&[text]).float(1), want, "{text:?}");
    }
    // 16777217 is not a float: the double rounds to nearest even.
    assert_eq!(call(&[b"16777217"]).float(1), 16_777_216.0);
}

#[test]
fn a_nan_is_quiet_and_keeps_its_sign_and_payload() {
    for text in [&b"nan"[..], b"-nan", b"NaN(123)", b"nan(0x7ffff)"] {
        let c = call(&[text]);
        let wide = atof(text);
        assert!(wide.is_nan(), "{text:?}");
        let got = c.float(1);
        assert!(got.is_nan(), "{text:?}");
        assert_eq!(got.to_bits(), to_f32(wide).to_bits(), "{text:?}");
        assert_eq!(got.to_bits() & 0x7fc0_0000, 0x7fc0_0000, "{text:?}: quiet");
        assert_eq!(got.is_sign_negative(), wide.is_sign_negative(), "{text:?}");
    }
}

#[test]
fn boolean_is_atob() {
    for (text, want) in [
        (&b"true"[..], true),
        (b"TrUe", true),
        (b"1", true),
        (b"0.5", true),
        (b"nan", true),
        (b"1e-45", true),
        (b"1e-50", false),
        (b"false", false),
        (b"yes", false),
        (b"0", false),
        (b"", false),
    ] {
        assert_eq!(call(&[text]).boolean(1), want, "{text:?}");
    }
}

#[test]
fn vec3_fills_what_the_text_gives() {
    for (text, want) in [
        (&b"1 2 3"[..], [1.0, 2.0, 3.0]),
        (b" 1.5\t-2 ", [1.5, -2.0, 0.0]),
        (b"1 x 3", [1.0, 0.0, 0.0]),
        (b"1 2 3 4", [1.0, 2.0, 3.0]),
        (b"", [0.0; 3]),
        (b"x", [0.0; 3]),
    ] {
        assert_eq!(call(&[text]).vec3(1), want, "{text:?}");
    }
}

#[test]
fn rest_is_the_arguments_from_an_index() {
    let c = call(&[b"a", b"b", b"c"]);
    assert_eq!(c.rest(0).len(), 4);
    assert_eq!(c.rest(2), [b"b".to_vec(), b"c".to_vec()]);
    assert_eq!(c.rest(4), [] as [Vec<u8>; 0]);
    assert!(c.rest(5).is_empty());
}

#[test]
fn this_needs_the_object_to_exist() {
    let mut vm = Vm::new();
    let mut c = call(&[]);
    assert_eq!(c.this(&vm), None);
    c.this_object = Some(987_654);
    assert_eq!(c.this(&vm), None);
    vm.eval(b"new ScriptObject(Probe);").expect("created");
    let id = vm.objects().find(b"Probe").expect("the object exists");
    c.this_object = Some(id);
    assert_eq!(c.this(&vm), Some(id));
    vm.eval(b"Probe.delete();").expect("deleted");
    assert_eq!(c.this(&vm), None);
}
