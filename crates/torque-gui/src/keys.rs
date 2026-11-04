//! TGE's input names and codes (`actionMap`'s tables and the platform's `KEY_*` codes): `gVirtualMap` (without the names
//! Torque3D added after TGE 1.4), `gAsciiMap`, the virtual-key and the scan-code tables are Torque3D's, value for value, and
//! the US layout's `AsciiTable` is computed as Torque3D's `fillAsciiTable` fills it ([`ascii_table`]). The key codes
//! (`KEY_*`), the names `bind` reads and `save`/`getBinding` write ([`parse_descriptor`], [`key_string`],
//! [`modifier_string`]; a host's own tables through [`KeyTables`]), the device names ([`device_type_and_instance`],
//! [`device_name`]), and the Win32 layer a window procedure uses ([`vk_to_key`], TGE's [`tge_vk_to_key`], [`to_ascii`], [`map_virtual_key`]: the US
//! layout's `ToAscii` and `MapVirtualKey`).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `platform/input/event.cpp` (Copyright (c)
//! 2012 GarageGames, LLC), `sim/actionMap.cpp` (Copyright (c) 2012 GarageGames, LLC), `platformWin32/winInput.cpp` (Copyright
//! (c) 2012 GarageGames, LLC), `platformWin32/winDInputDevice.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this
//! crate's own.

/// Event object types (`objType`) and devices (TGE's `SI_*`).
pub const SI_BUTTON: u16 = 0x02;
pub const SI_POV: u16 = 0x03;
pub const SI_AXIS: u16 = 0x04;
pub const SI_KEY: u16 = 0x0a;
pub const SI_XAXIS: u16 = 0x20b;
pub const SI_YAXIS: u16 = 0x20c;
pub const SI_ZAXIS: u16 = 0x20d;
pub const MOUSE_DEVICE: u16 = 1;
pub const KEYBOARD_DEVICE: u16 = 2;
pub const JOYSTICK_DEVICE: u16 = 3;

/// Actions (`SI_MAKE`...).
pub const SI_MAKE: u8 = 1;
pub const SI_BREAK: u8 = 2;
pub const SI_MOVE: u8 = 3;
pub const SI_REPEAT: u8 = 4;

/// Modifier bits of an event (left/right pairs) and their unions.
pub const SI_LSHIFT: u8 = 0x01;
pub const SI_RSHIFT: u8 = 0x02;
pub const SI_SHIFT: u8 = 0x03;
pub const SI_LCTRL: u8 = 0x04;
pub const SI_RCTRL: u8 = 0x08;
pub const SI_CTRL: u8 = 0x0c;
pub const SI_LALT: u8 = 0x10;
pub const SI_RALT: u8 = 0x20;
pub const SI_ALT: u8 = 0x30;
pub const SI_MAC_OPT: u8 = 0xc0;

/// Key codes the engine names in code.
pub const KEY_BACKSPACE: u16 = 0x01;
pub const KEY_TAB: u16 = 0x02;
pub const KEY_RETURN: u16 = 0x03;
pub const KEY_CONTROL: u16 = 0x04;
pub const KEY_ALT: u16 = 0x05;
pub const KEY_SHIFT: u16 = 0x06;
pub const KEY_CAPSLOCK: u16 = 0x08;
pub const KEY_ESCAPE: u16 = 0x09;
pub const KEY_OEM_102: u16 = 0x80;
pub const KEY_SPACE: u16 = 0x0a;
pub const KEY_PAGE_DOWN: u16 = 0x0b;
pub const KEY_PAGE_UP: u16 = 0x0c;
pub const KEY_END: u16 = 0x0d;
pub const KEY_HOME: u16 = 0x0e;
pub const KEY_LEFT: u16 = 0x0f;
pub const KEY_UP: u16 = 0x10;
pub const KEY_RIGHT: u16 = 0x11;
pub const KEY_DOWN: u16 = 0x12;
pub const KEY_PRINT: u16 = 0x13;
pub const KEY_INSERT: u16 = 0x14;
pub const KEY_DELETE: u16 = 0x15;
pub const KEY_NUMPAD0: u16 = 0x46;
pub const KEY_NUMPADENTER: u16 = 0x56;
pub const KEY_LCONTROL: u16 = 0x71;
pub const KEY_RCONTROL: u16 = 0x72;
pub const KEY_LALT: u16 = 0x73;
pub const KEY_RALT: u16 = 0x74;
pub const KEY_LSHIFT: u16 = 0x75;
pub const KEY_RSHIFT: u16 = 0x76;
pub const KEY_0: u16 = 0x17;
pub const KEY_A: u16 = 0x21;
pub const KEY_F1: u16 = 0x57;
pub const KEY_BUTTON0: u16 = 0x100;
pub const KEY_BUTTON1: u16 = 0x101;
pub const KEY_BUTTON2: u16 = 0x102;

/// `gAsciiMap`: names of characters (`bind(keyboard, "comma",...)`), in the table's order.
pub const ASCII_MAP: [(&str, u8); 11] = [
    ("space", b' '),
    ("doublequote", b'"'),
    ("apostrophe", b'\''),
    ("comma", b','),
    ("minus", b'-'),
    ("period", b'.'),
    ("backslash", b'\\'),
    ("grave", b'`'),
    ("super2", 0xb2),
    ("super3", 0xb3),
    ("acute", 0xb4),
];

/// `gVirtualMap`: `(name, event type, code)` in the table's order (the first match by name wins; a code's name is its first entry).
pub const VIRTUAL_MAP: [(&str, u16, u16); 143] = [
    ("backspace", SI_KEY, 0x01),
    ("tab", SI_KEY, 0x02),
    ("return", SI_KEY, 0x03),
    ("enter", SI_KEY, 0x03),
    ("shift", SI_KEY, 0x06),
    ("ctrl", SI_KEY, 0x04),
    ("alt", SI_KEY, 0x05),
    ("pause", SI_KEY, 0x07),
    ("capslock", SI_KEY, 0x08),
    ("escape", SI_KEY, 0x09),
    ("space", SI_KEY, 0x0a),
    ("pagedown", SI_KEY, 0x0b),
    ("pageup", SI_KEY, 0x0c),
    ("end", SI_KEY, 0x0d),
    ("home", SI_KEY, 0x0e),
    ("left", SI_KEY, 0x0f),
    ("up", SI_KEY, 0x10),
    ("right", SI_KEY, 0x11),
    ("down", SI_KEY, 0x12),
    ("print", SI_KEY, 0x13),
    ("insert", SI_KEY, 0x14),
    ("delete", SI_KEY, 0x15),
    ("help", SI_KEY, 0x16),
    ("win_lwindow", SI_KEY, 0x77),
    ("win_rwindow", SI_KEY, 0x78),
    ("win_apps", SI_KEY, 0x79),
    ("cmd", SI_KEY, 0x05),
    ("opt", SI_KEY, 0x90),
    ("lopt", SI_KEY, 0x91),
    ("ropt", SI_KEY, 0x92),
    ("numpad0", SI_KEY, 0x46),
    ("numpad1", SI_KEY, 0x47),
    ("numpad2", SI_KEY, 0x48),
    ("numpad3", SI_KEY, 0x49),
    ("numpad4", SI_KEY, 0x4a),
    ("numpad5", SI_KEY, 0x4b),
    ("numpad6", SI_KEY, 0x4c),
    ("numpad7", SI_KEY, 0x4d),
    ("numpad8", SI_KEY, 0x4e),
    ("numpad9", SI_KEY, 0x4f),
    ("numpadmult", SI_KEY, 0x50),
    ("numpadadd", SI_KEY, 0x51),
    ("numpadsep", SI_KEY, 0x52),
    ("numpadminus", SI_KEY, 0x53),
    ("numpaddecimal", SI_KEY, 0x54),
    ("numpaddivide", SI_KEY, 0x55),
    ("numpadenter", SI_KEY, 0x56),
    ("f1", SI_KEY, 0x57),
    ("f2", SI_KEY, 0x58),
    ("f3", SI_KEY, 0x59),
    ("f4", SI_KEY, 0x5a),
    ("f5", SI_KEY, 0x5b),
    ("f6", SI_KEY, 0x5c),
    ("f7", SI_KEY, 0x5d),
    ("f8", SI_KEY, 0x5e),
    ("f9", SI_KEY, 0x5f),
    ("f10", SI_KEY, 0x60),
    ("f11", SI_KEY, 0x61),
    ("f12", SI_KEY, 0x62),
    ("f13", SI_KEY, 0x63),
    ("f14", SI_KEY, 0x64),
    ("f15", SI_KEY, 0x65),
    ("f16", SI_KEY, 0x66),
    ("f17", SI_KEY, 0x67),
    ("f18", SI_KEY, 0x68),
    ("f19", SI_KEY, 0x69),
    ("f20", SI_KEY, 0x6a),
    ("f21", SI_KEY, 0x6b),
    ("f22", SI_KEY, 0x6c),
    ("f23", SI_KEY, 0x6d),
    ("f24", SI_KEY, 0x6e),
    ("numlock", SI_KEY, 0x6f),
    ("scrolllock", SI_KEY, 0x70),
    ("lshift", SI_KEY, 0x75),
    ("rshift", SI_KEY, 0x76),
    ("lcontrol", SI_KEY, 0x71),
    ("rcontrol", SI_KEY, 0x72),
    ("lalt", SI_KEY, 0x73),
    ("ralt", SI_KEY, 0x74),
    ("tilde", SI_KEY, 0x3b),
    ("minus", SI_KEY, 0x3c),
    ("equals", SI_KEY, 0x3d),
    ("lbracket", SI_KEY, 0x3e),
    ("rbracket", SI_KEY, 0x3f),
    ("backslash", SI_KEY, 0x40),
    ("semicolon", SI_KEY, 0x41),
    ("apostrophe", SI_KEY, 0x42),
    ("comma", SI_KEY, 0x43),
    ("period", SI_KEY, 0x44),
    ("slash", SI_KEY, 0x45),
    ("lessthan", SI_KEY, 0x80),
    ("button0", SI_BUTTON, 0x100),
    ("button1", SI_BUTTON, 0x101),
    ("button2", SI_BUTTON, 0x102),
    ("button3", SI_BUTTON, 0x103),
    ("button4", SI_BUTTON, 0x104),
    ("button5", SI_BUTTON, 0x105),
    ("button6", SI_BUTTON, 0x106),
    ("button7", SI_BUTTON, 0x107),
    ("button8", SI_BUTTON, 0x108),
    ("button9", SI_BUTTON, 0x109),
    ("button10", SI_BUTTON, 0x10a),
    ("button11", SI_BUTTON, 0x10b),
    ("button12", SI_BUTTON, 0x10c),
    ("button13", SI_BUTTON, 0x10d),
    ("button14", SI_BUTTON, 0x10e),
    ("button15", SI_BUTTON, 0x10f),
    ("button16", SI_BUTTON, 0x110),
    ("button17", SI_BUTTON, 0x111),
    ("button18", SI_BUTTON, 0x112),
    ("button19", SI_BUTTON, 0x113),
    ("button20", SI_BUTTON, 0x114),
    ("button21", SI_BUTTON, 0x115),
    ("button22", SI_BUTTON, 0x116),
    ("button23", SI_BUTTON, 0x117),
    ("button24", SI_BUTTON, 0x118),
    ("button25", SI_BUTTON, 0x119),
    ("button26", SI_BUTTON, 0x11a),
    ("button27", SI_BUTTON, 0x11b),
    ("button28", SI_BUTTON, 0x11c),
    ("button29", SI_BUTTON, 0x11d),
    ("button30", SI_BUTTON, 0x11e),
    ("button31", SI_BUTTON, 0x11f),
    ("xaxis", SI_AXIS, 0x20b),
    ("yaxis", SI_AXIS, 0x20c),
    ("zaxis", SI_AXIS, 0x20d),
    ("rxaxis", SI_AXIS, 0x20e),
    ("ryaxis", SI_AXIS, 0x20f),
    ("rzaxis", SI_AXIS, 0x210),
    ("slider", SI_AXIS, 0x211),
    ("xpov", SI_POV, 0x204),
    ("ypov", SI_POV, 0x205),
    ("upov", SI_POV, 0x206),
    ("dpov", SI_POV, 0x207),
    ("lpov", SI_POV, 0x208),
    ("rpov", SI_POV, 0x209),
    ("xpov2", SI_POV, 0x212),
    ("ypov2", SI_POV, 0x213),
    ("upov2", SI_POV, 0x214),
    ("dpov2", SI_POV, 0x215),
    ("lpov2", SI_POV, 0x216),
    ("rpov2", SI_POV, 0x217),
    ("anykey", SI_KEY, 0xfffe),
];

/// The `AsciiTable` of the US layout as `fillAsciiTable` makes it at start, for every key code from `KEY_ESCAPE` to
/// `KEY_OEM_102`: `(key code, lower, upper, goofy)`, the characters [`to_ascii`] gives the key's virtual key with no modifier,
/// with Shift and with Ctrl-Alt (0 for none). The virtual key is the one of the key's scan code (the inverses of the scan-code
/// table and of [`map_virtual_key`]); a numeric-keypad digit takes its own virtual key, with Num Lock on, and has no upper or
/// goofy character.
pub fn ascii_table() -> &'static [(u16, u8, u8, u8)] {
    static TABLE: std::sync::OnceLock<Vec<(u16, u8, u8, u8)>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let none = |_: u8| false;
        let shift = |k: u8| k == 0x10;
        let ctrl_alt = |k: u8| k == 0x11 || k == 0x12;
        (KEY_ESCAPE..=KEY_OEM_102)
            .map(|code| {
                if (KEY_NUMPAD0..=KEY_NUMPAD0 + 9).contains(&code) {
                    let vk = (0..=255u8).find(|&vk| u16::from(VK_TABLE[usize::from(vk)]) == code);
                    return (code, vk.map_or(0, |vk| to_ascii(vk, &none, false)), 0, 0);
                }
                let scan = (1..=255u8).find(|&s| dik_to_key(s) == code);
                let vk = scan.and_then(|s| (1..=255u8).find(|&vk| map_virtual_key(vk) == s));
                match vk {
                    Some(vk) => (
                        code,
                        to_ascii(vk, &none, false),
                        to_ascii(vk, &shift, false),
                        to_ascii(vk, &ctrl_alt, false),
                    ),
                    None => (code, 0, 0, 0),
                }
            })
            .collect()
    })
}

/// The tables the key names are read and written with ([`VIRTUAL_MAP`], [`ascii_table`]); a host may give its own.
#[derive(Debug, Clone, Copy)]
pub struct KeyTables {
    pub names: &'static [(&'static str, u16, u16)],
    pub ascii: &'static [(u16, u8, u8, u8)],
}

impl KeyTables {
    /// TGE's: [`VIRTUAL_MAP`] and the US layout's [`ascii_table`].
    pub fn engine() -> KeyTables {
        KeyTables {
            names: &VIRTUAL_MAP,
            ascii: ascii_table(),
        }
    }
}

/// Whether `c` passes the engine's "decent character" test (printable ASCII past the space, 0x91, 0x92, or 0xa0..=0xff).
fn decent(c: u8) -> bool {
    (0x21..0x7f).contains(&c) || c >= 0xa0 || c == 0x91 || c == 0x92
}

/// `Input::getKeyCode(ascii)`: the first code whose lower, then upper, then goofy character is `c` (codes ascending), 0 for none.
pub fn key_code_of_ascii(c: u8) -> u16 {
    key_code_of_ascii_in(&KeyTables::engine(), c)
}

/// [`key_code_of_ascii`] in `t`.
pub fn key_code_of_ascii_in(t: &KeyTables, c: u8) -> u16 {
    for pick in 0..3 {
        for &(code, lo, up, gf) in t.ascii {
            let ch = [lo, up, gf][pick];
            if ch != 0 && ch == c {
                return code;
            }
        }
    }
    0
}

/// `Input::getAscii(code, STATE_LOWER)`.
pub fn lower_ascii(code: u16) -> u8 {
    lower_ascii_in(&KeyTables::engine(), code)
}

/// [`lower_ascii`] in `t`.
pub fn lower_ascii_in(t: &KeyTables, code: u16) -> u8 {
    t.ascii.iter().find(|r| r.0 == code).map_or(0, |r| r.1)
}

/// `Input::getAscii(code, STATE_UPPER)`.
pub fn upper_ascii(code: u16) -> u8 {
    upper_ascii_in(&KeyTables::engine(), code)
}

/// [`upper_ascii`] in `t`.
pub fn upper_ascii_in(t: &KeyTables, code: u16) -> u8 {
    t.ascii.iter().find(|r| r.0 == code).map_or(0, |r| r.2)
}

/// `ActionMap::getKeyString(code)`: the character's name (`gAsciiMap`) or the character of a key with a decent lower
/// character, a digit or letter for 0x17..=0x3a, else the first `gVirtualMap` name. None for a code with no name (the caller
/// prints that the action has no string).
pub fn key_string(code: u16) -> Option<String> {
    key_string_in(&KeyTables::engine(), code)
}

/// [`key_string`] in `t`.
pub fn key_string_in(t: &KeyTables, code: u16) -> Option<String> {
    if code < 0x81 {
        let lo = lower_ascii_in(t, code);
        if lo != 0 && decent(lo) {
            if let Some((name, _)) = ASCII_MAP.iter().find(|(_, c)| *c == lo) {
                return Some((*name).to_string());
            }
            return Some(char::from(lo).to_string());
        }
    }
    if (0x17..0x21).contains(&code) {
        return Some(char::from(b'0' + (code - KEY_0) as u8).to_string());
    }
    if (0x21..0x3b).contains(&code) {
        return Some(char::from(b'a' + (code - KEY_A) as u8).to_string());
    }
    t.names
        .iter()
        .find(|(_, _, c)| *c == code)
        .map(|(n, _, _)| (*n).to_string())
}

/// Every side of a pair present counts as the pair (how the maps and the canvas compare an event's modifiers).
pub fn normalise_modifiers(modifiers: u8) -> u8 {
    let mut m = modifiers;
    for pair in [SI_SHIFT, SI_CTRL, SI_ALT, SI_MAC_OPT] {
        if m & pair != 0 {
            m |= pair;
        }
    }
    m
}

/// `ActionMap::getModifierString`: the text a binding's key follows.
pub fn modifier_string(modifiers: u8) -> &'static str {
    match normalise_modifiers(modifiers) {
        0x03 => "shift ",
        0x0c => "ctrl ",
        0x0f => "shift-ctrl ",
        0x30 => "alt ",
        0x33 => "shift-alt ",
        0x3c => "ctrl-alt ",
        0x3f => "shift-ctrl-alt ",
        0xc0 => "opt ",
        0xc3 => "shift-opt ",
        0xcc => "ctrl-opt ",
        0xcf => "shift-ctrl-opt ",
        _ => "",
    }
}

/// What `bind` makes of its key text (`createEventDescriptor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Descriptor {
    pub modifiers: u8,
    /// `SI_KEY`, `SI_BUTTON` or `SI_AXIS`.
    pub kind: u16,
    pub code: u16,
}

/// `createEventDescriptor`: the text cut to 255 bytes; with a space, the words before the first space split at `-`
/// are modifiers (`shift`, `ctrl`, `alt`/`cmd`, `opt`, any case; others ignored) and the rest is the key; one decent character is a
/// key by `getKeyCode`, else a letter or digit; else a `gAsciiMap` name whose character has a code past 0x16, else a `gVirtualMap`
/// name. None when nothing matches.
pub fn parse_descriptor(text: &[u8]) -> Option<Descriptor> {
    parse_descriptor_with(text, &KeyTables::engine())
}

/// [`parse_descriptor`] with a host's own tables.
pub fn parse_descriptor_with(text: &[u8], t: &KeyTables) -> Option<Descriptor> {
    let text = &text[..text.len().min(255)];
    let text = &text[..text.iter().position(|b| *b == 0).unwrap_or(text.len())];
    let (mods, key) = match text.iter().position(|b| *b == b' ') {
        Some(at) => (&text[..at], &text[at + 1..]),
        None => (&b""[..], text),
    };
    let mut modifiers = 0u8;
    for word in mods.split(|b| *b == b'-').filter(|w| !w.is_empty()) {
        if word.eq_ignore_ascii_case(b"shift") {
            modifiers |= SI_SHIFT;
        } else if word.eq_ignore_ascii_case(b"ctrl") {
            modifiers |= SI_CTRL;
        } else if word.eq_ignore_ascii_case(b"alt") || word.eq_ignore_ascii_case(b"cmd") {
            modifiers |= SI_ALT;
        } else if word.eq_ignore_ascii_case(b"opt") {
            modifiers |= SI_MAC_OPT;
        }
    }
    let key_desc = |code: u16| Descriptor {
        modifiers,
        kind: SI_KEY,
        code,
    };
    if key.len() == 1 {
        let c = key[0];
        if !decent(c) {
            return None;
        }
        let code = key_code_of_ascii_in(t, c);
        if code > 0x16 {
            return Some(key_desc(code));
        }
        if c.is_ascii_alphabetic() {
            return Some(key_desc(u16::from(c.to_ascii_lowercase()) - 0x40));
        }
        if c.is_ascii_digit() {
            return Some(key_desc(u16::from(c) - 0x19));
        }
        return None;
    }
    if let Some((_, ch)) = ASCII_MAP
        .iter()
        .find(|(n, _)| n.as_bytes().eq_ignore_ascii_case(key))
    {
        let code = key_code_of_ascii_in(t, *ch);
        if code >= 0x17 {
            return Some(key_desc(code));
        }
    }
    t.names
        .iter()
        .find(|(n, _, _)| n.as_bytes().eq_ignore_ascii_case(key))
        .map(|&(_, kind, code)| Descriptor {
            modifiers,
            kind,
            code,
        })
}

/// `ActionMap::getDeviceTypeAndInstance`: `keyboard`, `mouse` or `joystick` as a prefix in any case, then an
/// instance (`atoi` of the rest, a negative one 0; none: 0).
pub fn device_type_and_instance(text: &[u8]) -> Option<(u16, u32)> {
    let starts = |p: &[u8]| text.len() >= p.len() && text[..p.len()].eq_ignore_ascii_case(p);
    let (kind, len) = if starts(b"keyboard") {
        (KEYBOARD_DEVICE, 8)
    } else if starts(b"mouse") {
        (MOUSE_DEVICE, 5)
    } else if starts(b"joystick") {
        (JOYSTICK_DEVICE, 8)
    } else {
        return None;
    };
    let inst = if text.len() <= len {
        0
    } else {
        torque_ts_vm::text::atoi(&text[len..]).max(0) as u32
    };
    Some((kind, inst))
}

/// `ActionMap::getDeviceName`: `keyboard` (no instance), `mouse%d`, `joystick%d`.
pub fn device_name(kind: u16, inst: u32) -> Option<String> {
    match kind {
        KEYBOARD_DEVICE => Some("keyboard".to_string()),
        MOUSE_DEVICE => Some(format!("mouse{inst}")),
        JOYSTICK_DEVICE => Some(format!("joystick{inst}")),
        _ => None,
    }
}

/// The virtual-key table: the key code of a Windows virtual key in the ranges the window procedure reads from it.
const VK_TABLE: [u8; 256] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x00, 0x00, 0x00, 0x03, 0x00,
    0x00, //
    0x06, 0x04, 0x05, 0x07, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00,
    0x00, //
    0x0a, 0x0c, 0x0b, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x00, 0x13, 0x00, 0x00, 0x14, 0x15,
    0x16, //
    0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, //
    0x00, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e,
    0x2f, //
    0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x77, 0x78, 0x79, 0x00,
    0x00, //
    0x46, 0x47, 0x48, 0x49, 0x4a, 0x4b, 0x4c, 0x4d, 0x4e, 0x4f, 0x50, 0x51, 0x52, 0x53, 0x54,
    0x55, //
    0x57, 0x58, 0x59, 0x5a, 0x5b, 0x5c, 0x5d, 0x5e, 0x5f, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65,
    0x66, //
    0x67, 0x68, 0x69, 0x6a, 0x6b, 0x6c, 0x6d, 0x6e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, //
    0x6f, 0x70, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, //
    0x75, 0x76, 0x71, 0x72, 0x73, 0x74, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x41, 0x3d, 0x43, 0x3c, 0x44,
    0x45, //
    0x3b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3e, 0x40, 0x3f, 0x42,
    0x00, //
    0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, //
];

/// The DirectInput scan-code table, read for the other virtual keys.
fn dik_to_key(scan: u8) -> u16 {
    match scan {
        0x01 => 0x09,
        // `DIK_1`..`DIK_9`.
        0x02..=0x0a => KEY_0 + u16::from(scan - 1),
        0x0b => 0x17,
        0x0c => 0x3c,
        0x0d => 0x3d,
        0x0e => 0x01,
        0x0f => 0x02,
        0x10 => 0x31,
        0x11 => 0x37,
        0x12 => 0x25,
        0x13 => 0x32,
        0x14 => 0x34,
        0x15 => 0x39,
        0x16 => 0x35,
        0x17 => 0x29,
        0x18 => 0x2f,
        0x19 => 0x30,
        0x1a => 0x3e,
        0x1b => 0x3f,
        0x1c => 0x03,
        0x1d => 0x71,
        0x1e => 0x21,
        0x1f => 0x33,
        0x20 => 0x24,
        0x21 => 0x26,
        0x22 => 0x27,
        0x23 => 0x28,
        0x24 => 0x2a,
        0x25 => 0x2b,
        0x26 => 0x2c,
        0x27 => 0x41,
        0x28 => 0x42,
        0x29 => 0x3b,
        0x2a => 0x75,
        0x2b => 0x40,
        0x2c => 0x3a,
        0x2d => 0x38,
        0x2e => 0x23,
        0x2f => 0x36,
        0x30 => 0x22,
        0x31 => 0x2e,
        0x32 => 0x2d,
        0x33 => 0x43,
        0x34 => 0x44,
        0x35 => 0x45,
        0x36 => 0x76,
        0x37 => 0x50,
        0x38 => 0x73,
        0x39 => 0x0a,
        0x3a => 0x08,
        // `DIK_F1`..`DIK_F10`.
        0x3b..=0x44 => KEY_F1 + u16::from(scan - 0x3b),
        0x45 => 0x6f,
        0x46 => 0x70,
        0x47 => 0x4d,
        0x48 => 0x4e,
        0x49 => 0x4f,
        0x4a => 0x53,
        0x4b => 0x4a,
        0x4c => 0x4b,
        0x4d => 0x4c,
        0x4e => 0x51,
        0x4f => 0x47,
        0x50 => 0x48,
        0x51 => 0x49,
        0x52 => 0x46,
        0x53 => 0x54,
        0x56 => 0x80,
        0x57 => 0x61,
        0x58 => 0x62,
        0x64 => 0x63,
        0x65 => 0x64,
        0x66 => 0x65,
        0x9c => 0x56,
        0x9d => 0x72,
        0xb3 => 0x52,
        0xb5 => 0x55,
        0xb7 => 0x13,
        0xb8 => 0x74,
        0xc5 => 0x07,
        0xc7 => 0x0e,
        0xc8 => 0x10,
        0xc9 => 0x0c,
        0xcb => 0x0f,
        0xcd => 0x11,
        0xcf => 0x0d,
        0xd0 => 0x12,
        0xd1 => 0x0b,
        0xd2 => 0x14,
        0xd3 => 0x15,
        0xdb => 0x77,
        0xdc => 0x78,
        0xdd => 0x79,
        _ => 0,
    }
}

/// The key code the engine gives a virtual key: TGE's ([`tge_vk_to_key`]), but every key its own: Pause is `pause` (TGE
/// reads Num Lock's scan code for it), Print Screen `print` (TGE has none), the Windows and menu keys `win_lwindow`,
/// `win_rwindow` and `win_apps` (TGE has none), the numpad's divide `numpaddivide` (TGE reads the slash's scan code) and F16
/// to F24 their own (TGE has none).
pub fn vk_to_key(vk: u8, scan: u8) -> u16 {
    match vk {
        0x13 | 0x5b..=0x5d | 0x6f | 0x7c..=0x87 => u16::from(VK_TABLE[usize::from(vk)]),
        0x2c => KEY_PRINT,
        _ => tge_vk_to_key(vk, scan),
    }
}

/// The key code TGE's window procedure gives a virtual key: the table for 0x08..=0x12, 0x1b, 0x20..=0x2e, 0x30..=0x39,
/// 0x41..=0x5a and 0x70..=0x7b, the scan code's otherwise.
pub fn tge_vk_to_key(vk: u8, scan: u8) -> u16 {
    let direct =
        matches!(vk, 0x08..=0x12 | 0x1b | 0x20..=0x2e | 0x30..=0x39 | 0x41..=0x5a | 0x70..=0x7b);
    if direct {
        u16::from(VK_TABLE[usize::from(vk)])
    } else {
        dik_to_key(scan)
    }
}

/// `VK_F1` and the scan code of F1.
const VK_F1: u8 = 0x70;
const SCAN_F1: u8 = 0x3b;

/// `MapVirtualKey(vk, MAPVK_VK_TO_VSC)` of the US layout for the keys a driver posts (0 for one this table does not know).
pub fn map_virtual_key(vk: u8) -> u8 {
    const LETTERS: [u8; 26] = [
        0x1e, 0x30, 0x2e, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18,
        0x19, 0x10, 0x13, 0x1f, 0x14, 0x16, 0x2f, 0x11, 0x2d, 0x15, 0x2c,
    ];
    match vk {
        0x08 => 0x0e,
        0x09 => 0x0f,
        0x0d => 0x1c,
        0x10 | 0xa0 => 0x2a,
        0xa1 => 0x36,
        0x11 | 0xa2 | 0xa3 => 0x1d,
        0x12 | 0xa4 | 0xa5 => 0x38,
        0x13 => 0x45,
        0x14 => 0x3a,
        0x1b => 0x01,
        0x20 => 0x39,
        0x21 => 0x49,
        0x22 => 0x51,
        0x23 => 0x4f,
        0x24 => 0x47,
        0x25 => 0x4b,
        0x26 => 0x48,
        0x27 => 0x4d,
        0x28 => 0x50,
        0x2d => 0x52,
        0x2e => 0x53,
        0x30 => 0x0b,
        0x31..=0x39 => vk - 0x2f,
        0x41..=0x5a => LETTERS[usize::from(vk - 0x41)],
        0x60 => 0x52,
        0x61 => 0x4f,
        0x62 => 0x50,
        0x63 => 0x51,
        0x64 => 0x4b,
        0x65 => 0x4c,
        0x66 => 0x4d,
        0x67 => 0x47,
        0x68 => 0x48,
        0x69 => 0x49,
        0x6a => 0x37,
        0x6b => 0x4e,
        0x6d => 0x4a,
        0x6e => 0x53,
        0x6f => 0x35,
        // `VK_F1`..`VK_F10`: the scan codes of F1..F10 in order.
        0x70..=0x79 => SCAN_F1 + (vk - VK_F1),
        0x7a => 0x57,
        0x7b => 0x58,
        // `VK_F13`..`VK_F23`, then `VK_F24`.
        0x7c..=0x86 => 0x64 + (vk - 0x7c),
        0x87 => 0x76,
        0x90 => 0x45,
        0x91 => 0x46,
        0xba => 0x27,
        0xbb => 0x0d,
        0xbc => 0x33,
        0xbd => 0x0c,
        0xbe => 0x34,
        0xbf => 0x35,
        0xc0 => 0x29,
        0xdb => 0x1a,
        0xdc => 0x2b,
        0xdd => 0x1b,
        0xde => 0x28,
        _ => 0,
    }
}

/// `ToAscii` of the US layout over the window procedure's own key-state array (`down` reads a virtual key's state; `caps` the Caps
/// Lock toggle): the character a key gives, 0 for none.
pub fn to_ascii(vk: u8, down: &dyn Fn(u8) -> bool, caps: bool) -> u8 {
    let shift = down(0x10) || down(0xa0) || down(0xa1);
    let ctrl = down(0x11) || down(0xa2) || down(0xa3);
    let alt = down(0x12) || down(0xa4) || down(0xa5);
    if ctrl && alt {
        return 0;
    }
    if ctrl {
        return match vk {
            0x41..=0x5a => vk - 0x40,
            0xdb => 0x1b,
            0xdc => 0x1c,
            0xdd => 0x1d,
            0x20 => 0x20,
            0x08 => 0x7f,
            0x0d => 0x0a,
            0x1b => 0x1b,
            0x03 => 0x03,
            0x36 if shift => 0x1e,
            0xbd if shift => 0x1f,
            _ => 0,
        };
    }
    let pick = |plain: u8, shifted: u8| if shift { shifted } else { plain };
    match vk {
        0x41..=0x5a => {
            if shift != caps {
                vk
            } else {
                vk.to_ascii_lowercase()
            }
        }
        0x30..=0x39 => pick(vk, b")!@#$%^&*("[usize::from(vk - 0x30)]),
        0x20 => b' ',
        0x08 => 0x08,
        0x09 => 0x09,
        0x0d => 0x0d,
        0x1b => 0x1b,
        0x60..=0x69 => vk - 0x30,
        0x6a => b'*',
        0x6b => b'+',
        0x6d => b'-',
        0x6e => b'.',
        0x6f => b'/',
        0xba => pick(b';', b':'),
        0xbb => pick(b'=', b'+'),
        0xbc => pick(b',', b'<'),
        0xbd => pick(b'-', b'_'),
        0xbe => pick(b'.', b'>'),
        0xbf => pick(b'/', b'?'),
        0xc0 => pick(b'`', b'~'),
        0xdb => pick(b'[', b'{'),
        0xdc | 0xe2 => pick(b'\\', b'|'),
        0xdd => pick(b']', b'}'),
        0xde => pick(b'\'', b'"'),
        _ => 0,
    }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
