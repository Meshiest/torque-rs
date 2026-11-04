use super::*;

#[test]
fn names_parse_and_print_as_the_action_maps_do() {
    let k = |t: &str| parse_descriptor(t.as_bytes()).map(|d| (d.modifiers, d.code));
    assert_eq!(k("w"), Some((0, 0x37)));
    assert_eq!(k("W"), Some((0, 0x37)));
    assert_eq!(k("ctrl w"), Some((SI_CTRL, 0x37)));
    assert_eq!(k("shift-ctrl p"), Some((SI_SHIFT | SI_CTRL, 0x30)));
    assert_eq!(k("shift ctrl p"), None);
    assert_eq!(k("space"), Some((0, 0x0a)));
    assert_eq!(k("+"), Some((0, 0x51)));
    assert_eq!(k("!"), Some((0, 0x18)));
    assert_eq!(k("comma"), Some((0, 0x43)));
    assert_eq!(k("tilde"), Some((0, 0x3b)));
    assert_eq!(k("button0"), Some((0, 0x100)));
    assert_eq!(k("xaxis"), Some((0, 0x20b)));
    assert_eq!(k("nope"), None);
    assert_eq!(key_string(0x37).as_deref(), Some("w"));
    assert_eq!(key_string(0x0a).as_deref(), Some("space"));
    assert_eq!(key_string(0x51).as_deref(), Some("+"));
    assert_eq!(key_string(0x3b).as_deref(), Some("grave"));
    assert_eq!(key_string(0x3d).as_deref(), Some("="));
    assert_eq!(key_string(0x53).as_deref(), Some("minus"));
    assert_eq!(key_string(0x56).as_deref(), Some("numpadenter"));
    assert_eq!(key_string(0x75).as_deref(), Some("lshift"));
    assert_eq!(key_string(0x20d).as_deref(), Some("zaxis"));
    assert_eq!(key_string(0x18).as_deref(), Some("1"));
    assert_eq!(key_string(0x9999), None);
    assert_eq!(modifier_string(SI_LCTRL), "ctrl ");
    assert_eq!(modifier_string(SI_SHIFT | SI_CTRL), "shift-ctrl ");
    assert_eq!(device_type_and_instance(b"MOUSE"), Some((MOUSE_DEVICE, 0)));
    assert_eq!(device_type_and_instance(b"mouse2"), Some((MOUSE_DEVICE, 2)));
    assert_eq!(
        device_type_and_instance(b"keyboard0"),
        Some((KEYBOARD_DEVICE, 0))
    );
    assert_eq!(device_type_and_instance(b"toaster"), None);
    assert_eq!(device_name(KEYBOARD_DEVICE, 3).as_deref(), Some("keyboard"));
    assert_eq!(device_name(MOUSE_DEVICE, 0).as_deref(), Some("mouse0"));
}

#[test]
fn virtual_keys_map_as_the_window_procedure_maps_them() {
    assert_eq!(vk_to_key(0x41, 0x1e), 0x21);
    assert_eq!(vk_to_key(0x0d, 0x1c), KEY_RETURN);
    assert_eq!(vk_to_key(0x10, 0x2a), KEY_SHIFT);
    assert_eq!(vk_to_key(0x25, 0x4b), KEY_LEFT);
    assert_eq!(vk_to_key(0xbc, 0x33), 0x43);
    assert_eq!(vk_to_key(0x60, 0x52), 0x46);
    // F13..F15 reach their codes through the scan code; F16 and up have none.
    assert_eq!(vk_to_key(0x7c, map_virtual_key(0x7c)), 0x63);
    assert_eq!(vk_to_key(0x7e, map_virtual_key(0x7e)), 0x65);
    assert_eq!(vk_to_key(0x7f, map_virtual_key(0x7f)), 0);
    assert_eq!(map_virtual_key(0x87), 0x76);
    // The ISO key is out of this table (its scan code is the window's to give), so the ascii table leaves its code without
    // characters and it keeps its own name rather than the backslash key's.
    assert_eq!(map_virtual_key(0xe2), 0);
    assert_eq!(key_string(KEY_OEM_102).as_deref(), Some("lessthan"));
    assert_eq!(key_string(0x40).as_deref(), Some("backslash"));
    let none = |_: u8| false;
    let shift = |k: u8| k == 0x10;
    assert_eq!(to_ascii(0x41, &none, false), b'a');
    assert_eq!(to_ascii(0x41, &shift, false), b'A');
    assert_eq!(to_ascii(0x41, &shift, true), b'a');
    assert_eq!(to_ascii(0x31, &shift, false), b'!');
    assert_eq!(to_ascii(0x25, &none, false), 0);
    for vk in 0..=255u8 {
        let _ = vk_to_key(vk, map_virtual_key(vk));
        let _ = to_ascii(vk, &none, false);
    }
}
