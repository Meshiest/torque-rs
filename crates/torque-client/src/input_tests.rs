use super::*;

#[test]
fn keys_carry_their_codes_modifiers_and_ascii() {
    let mut w = Win32::default();
    let ev = |e: Option<Engine>| match e {
        Some(Engine::Input(i)) => i,
        other => panic!("{other:?}"),
    };
    let a = ev(w.message(WM_KEYDOWN, 0x41, key_lparam(0x41, false, false)));
    assert_eq!(
        (a.obj_inst, a.ascii, a.modifier, a.action),
        (0x21, u16::from(b'a'), 0, SI_MAKE)
    );
    let s = ev(w.message(WM_KEYDOWN, 0x10, key_lparam(0x10, false, false)));
    assert_eq!((s.obj_inst, s.modifier), (KEY_LSHIFT, 0));
    let a2 = ev(w.message(WM_KEYDOWN, 0x41, key_lparam(0x41, false, true)));
    assert_eq!(
        (a2.ascii, a2.modifier, a2.action),
        (u16::from(b'A'), SI_LSHIFT, SI_REPEAT)
    );
    let s_up = ev(w.message(WM_KEYUP, 0x10, key_lparam(0x10, true, false)));
    assert_eq!((s_up.action, s_up.modifier), (SI_BREAK, 0));
    assert_eq!(w.modifiers, 0);
    assert_eq!(
        w.message(WM_MOUSEMOVE, 0, coords(400, 300)),
        Some(Engine::Move((400, 300), 0))
    );
    assert!(w.message(WM_MOUSEWHEEL, 60 << 16, 0).is_none());
    let wheel = ev(w.message(WM_MOUSEWHEEL, (((-120i32) as u32) & 0xffff) << 16, 0));
    assert_eq!((wheel.obj_type, wheel.value), (SI_ZAXIS, -120.0));
}

#[test]
fn an_injection_reads_its_words() {
    let items =
        parse_injection("move 1 2; down 1 2 # click\nsleep 100\nkey 65\nwheel -120").unwrap();
    assert_eq!(items.len(), 6);
    assert!(parse_injection("jump 1").is_err());
    assert!(parse_injection("sleep 999999").is_err());
    assert!(parse_injection("key 0").is_err());
}
