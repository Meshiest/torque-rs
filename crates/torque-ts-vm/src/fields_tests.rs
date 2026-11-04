//! The console types' setters and getters.
use super::*;

#[derive(Default)]
struct Env {
    strings: Vec<Rc<[u8]>>,
    messages: Vec<String>,
    levels: Vec<Level>,
    /// Work charged so far, and the most the test allows (None: no limit).
    charged: u64,
    budget: Option<u64>,
}

impl FieldEnv for Env {
    fn intern(&mut self, text: &[u8]) -> Rc<[u8]> {
        if let Some(s) = self.strings.iter().find(|s| s.eq_ignore_ascii_case(text)) {
            return Rc::clone(s);
        }
        let s: Rc<[u8]> = Rc::from(text);
        self.strings.push(Rc::clone(&s));
        s
    }
    fn expand_filename(&mut self, text: &[u8]) -> Option<Vec<u8>> {
        Some(text.to_vec())
    }
    fn find_object(&mut self, text: &[u8], _class: &str) -> Option<ObjId> {
        (text == b"Known").then_some(7)
    }
    fn message(&mut self, level: Level, text: String) {
        self.levels.push(level);
        self.messages.push(text);
    }
    fn charge(&mut self, units: u64) -> bool {
        self.charged = self.charged.saturating_add(units);
        self.budget.is_none_or(|budget| self.charged <= budget)
    }
}

/// The rules that read every float type with `%f`.
fn fixed() -> FieldRules {
    FieldRules {
        fixed_color_f: true,
        fixed_point2f: true,
        fixed_float_list: true,
        ..FieldRules::default()
    }
}

/// The getter under [`fixed`] (the answers below read the float types with `%f`).
fn get(ty: FieldType, slot: &Value, names: &dyn Fn(ObjId) -> Option<Vec<u8>>) -> Vec<u8> {
    get_with(fixed(), ty, slot, names)
}

fn names(id: ObjId) -> Option<Vec<u8>> {
    (id == 7).then(|| b"Known".to_vec())
}

/// Writes each text in turn to one element and returns what each write reads back.
fn writes(ty: FieldType, texts: &[&str]) -> Vec<String> {
    let mut env = Env::default();
    let mut slot = zero(ty);
    texts
        .iter()
        .map(|t| {
            set(ty, &mut slot, t.as_bytes(), &mut env);
            String::from_utf8(get(ty, &slot, &names)).unwrap()
        })
        .collect()
}

#[test]
fn floats_read_with_percent_g() {
    assert_eq!(
        writes(
            FieldType::F32,
            &[
                "0.5",
                "5",
                "abc",
                "1e10",
                "0.1",
                "-0",
                "nan",
                "  2.5x",
                "",
                "123456789",
                "0x10",
                "1e-5",
                "-1.5e+3",
                "3.4e39",
                "inf",
                ".5",
                "5."
            ]
        ),
        [
            "0.5",
            "5",
            "0",
            "1e+10",
            "0.1",
            "-0",
            "nan",
            "2.5",
            "0",
            "1.23457e+08",
            "16",
            "1e-05",
            "-1500",
            "inf",
            "inf",
            "0.5",
            "5"
        ]
    );
}

#[test]
fn a_bool_converts_the_number_through_a_float_before_comparing_it_with_zero() {
    // `(F32)atof(s) != 0`: a double below half the smallest float is 0 as a float.
    let read = |texts: &[&str]| writes(FieldType::Bool, texts);
    assert_eq!(
        read(&[
            "1e-50", "-1e-50", "1e-46", "7e-46", "7.1e-46", "1e-45", "0x1p-200", "1e-300"
        ]),
        ["0", "0", "0", "0", "1", "1", "0", "0"]
    );
    // Around the edges: half the smallest float (2^-150 = 7.0064923216e-46) is not reached by the first
    // and passed by the second; the smallest normal float and below it; NaN, infinity and overflow.
    assert_eq!(
        read(&[
            "7.0064923e-46",
            "7.0064924e-46",
            "1.17549435e-38",
            "1e-39",
            "nan",
            "inf",
            "1e400",
            "-1e400",
            "TrUe"
        ]),
        ["0", "1", "1", "1", "1", "1", "1", "1", "1"]
    );
}

#[test]
fn bools_are_datob() {
    assert_eq!(
        writes(
            FieldType::Bool,
            &[
                "5", "0", "abc", "true", "TRUE", "yes", "-1", "0.5", "", " 1", "false", "0.0001",
                "1e-9"
            ]
        ),
        [
            "1", "0", "0", "1", "1", "0", "1", "1", "0", "1", "0", "1", "1"
        ]
    );
}

#[test]
fn ints_are_atoi_saturating() {
    assert_eq!(
        writes(
            FieldType::S32,
            &[
                "7",
                "-3",
                "2147483648",
                "99999999999",
                "1.9",
                "abc",
                "0x10",
                "",
                " -4",
                "+5",
                "-2147483649"
            ]
        ),
        [
            "7",
            "-3",
            "2147483647",
            "2147483647",
            "1",
            "0",
            "0",
            "0",
            "-4",
            "5",
            "-2147483648"
        ]
    );
}

#[test]
fn point3f_keeps_what_the_text_does_not_overwrite() {
    assert_eq!(
        writes(
            FieldType::Point3F,
            &[
                "1 2 3 4",
                "1 2",
                "1",
                "",
                "a b c",
                "1,2,3",
                " 1  2   3",
                "1.5 2.25 1e3",
                "1 2 x"
            ]
        ),
        [
            "1 2 3",
            "1 2 3",
            "1 2 3",
            "1 2 3",
            "1 2 3",
            "1 2 3",
            "1 2 3",
            "1.5 2.25 1000",
            "1 2 1000"
        ]
    );
}

#[test]
fn colors_take_defaults_for_what_is_missing() {
    assert_eq!(
        writes(
            FieldType::ColorF,
            &[
                "1 0.5 0.25 1",
                "1 0.5",
                "2 3 4 5",
                "a",
                "",
                "1 2 3 4 5",
                "0.1 0.2 0.3",
                "1e10 0 0 0"
            ]
        ),
        [
            "1.000000 0.500000 0.250000 1.000000",
            "1.000000 0.500000 0.000000 1.000000",
            "2.000000 3.000000 4.000000 5.000000",
            "0.000000 0.000000 0.000000 1.000000",
            "0.000000 0.000000 0.000000 1.000000",
            "1.000000 2.000000 3.000000 4.000000",
            "0.100000 0.200000 0.300000 1.000000",
            "10000000000.000000 0.000000 0.000000 0.000000"
        ]
    );
    assert_eq!(
        writes(
            FieldType::ColorI,
            &[
                "1 2 3",
                "300 -5 7 9",
                "a b c",
                "1 2 3 4 5",
                "1 2",
                "255 255 255 255",
                "1.9 2.9 3.9 4.9",
                "256 512 -1 -256"
            ]
        ),
        [
            "1 2 3 255",
            "44 251 7 9",
            "0 0 0 255",
            "1 2 3 4",
            "1 2 0 255",
            "255 255 255 255",
            "1 0 0 255",
            "0 0 255 0"
        ]
    );
}

#[test]
fn two_dimensional_points_read_in_place() {
    let mut env = Env::default();
    let mut p = Value::Ints2([0, 0]);
    let mut seen = Vec::new();
    for t in ["1 2", "1", "1 2 3", "-5 x", "1.9 2.9"] {
        set(FieldType::Point2I, &mut p, t.as_bytes(), &mut env);
        seen.push(String::from_utf8(get(FieldType::Point2I, &p, &names)).unwrap());
    }
    assert_eq!(seen, ["1 2", "1 2", "1 2", "-5 2", "1 2"]);
    assert_eq!(
        writes(FieldType::Point2F, &["1 2", "1", "1 2 3", "0.5 x"]),
        [
            "1.000000 2.000000",
            "1.000000 2.000000",
            "1.000000 2.000000",
            "0.500000 2.000000"
        ]
    );
}

#[test]
fn the_position_and_rotation_views_share_one_matrix() {
    let mut env = Env::default();
    let mut m = zero(FieldType::MatrixPosition);
    let read = |m: &Value, ty| String::from_utf8(get(ty, m, &names)).unwrap();
    let mut rot = Vec::new();
    for t in [
        "0 0 1 90",
        "1 0 0 0",
        "0 0 0 0",
        "1 2 3",
        "0 0 1 3.14159",
        "0 0 1 -90",
        "0 1 0 45",
        "2 0 0 30",
        "0 0 1 720",
        "1 1 1 120",
        "0 0 1 180",
        "0 0 -1 90",
        "0.5 0.5 0.5 60",
    ] {
        set(FieldType::MatrixRotation, &mut m, t.as_bytes(), &mut env);
        rot.push(read(&m, FieldType::MatrixRotation));
    }
    assert_eq!(
        rot,
        [
            "0 0 1 90",
            "1 0 0 0",
            "1 0 0 0",
            "1 0 0 0",
            "0 0 1 3.14169",
            "0 0 -1 90",
            "0 1 0 45",
            "1 0 0 62.3479",
            "1 0 0 0",
            "0.57735 0.57735 0.57735 120",
            "0 0 1 180",
            "0 0 -1 90",
            "0.57735 0.57735 0.57735 51.3178"
        ]
    );
    let mut pos = Vec::new();
    for t in ["1 2 3", "1 2 3 4", "1", "", "0.1 0.2 0.3", "5 6 7"] {
        set(FieldType::MatrixPosition, &mut m, t.as_bytes(), &mut env);
        pos.push(read(&m, FieldType::MatrixPosition));
    }
    assert_eq!(
        pos,
        ["1 2 3", "1 2 3 4", "1 0 0", "0 0 0", "0.1 0.2 0.3", "5 6 7"]
    );
    // The rotation written last is still there.
    assert_eq!(
        read(&m, FieldType::MatrixRotation),
        "0.57735 0.57735 0.57735 51.3178"
    );
    // A rotation written over a position keeps the translation.
    set(FieldType::MatrixPosition, &mut m, b"5 6 7", &mut env);
    set(FieldType::MatrixRotation, &mut m, b"0 0 1 90", &mut env);
    assert_eq!(read(&m, FieldType::MatrixPosition), "5 6 7");
    // Rotations about each axis and a skewed one.
    let mut m = zero(FieldType::MatrixRotation);
    for (t, want) in [
        ("0 0 1 1e-7", "1 0 0 0"),
        ("0 0 1 359.9", "0 0 -1 0.104678"),
    ] {
        set(FieldType::MatrixRotation, &mut m, t.as_bytes(), &mut env);
        assert_eq!(read(&m, FieldType::MatrixRotation), want, "{t}");
    }
}

/// A rotation write keeps the whole translation column, the fourth
/// element too (`position = "1 2 3 4"`, then a rotation, still reads `1 2 3 4`).
#[test]
fn a_rotation_written_over_a_position_keeps_its_fourth_element() {
    let mut env = Env::default();
    let mut m = zero(FieldType::MatrixPosition);
    set(FieldType::MatrixPosition, &mut m, b"1 2 3 4", &mut env);
    set(FieldType::MatrixRotation, &mut m, b"0 1 0 30", &mut env);
    assert_eq!(get(FieldType::MatrixPosition, &m, &names), b"1 2 3 4");
    assert_eq!(get(FieldType::MatrixRotation, &m, &names), b"0 1 0 30");
}

/// An `int` field (a team number, say) is `atoi` and `%d`, not text.
#[test]
fn an_int_field_is_atoi_and_percent_d() {
    let mut env = Env::default();
    let ty = FieldType::S32;
    let mut slot = zero(ty);
    assert_eq!(get(ty, &slot, &names), b"0");
    for (text, want) in [
        ("3 4", "3"),
        ("x", "0"),
        ("", "0"),
        (" 5 ", "5"),
        ("-3", "-3"),
    ] {
        set(ty, &mut slot, text.as_bytes(), &mut env);
        assert_eq!(get(ty, &slot, &names), want.as_bytes(), "{text:?}");
    }
}

#[test]
fn degrees_and_radians_convert_in_doubles_and_round_once() {
    // `mDegToRad`/`mRadToDeg` compute in doubles and store a float; the same product in floats differs in
    // the last digit at these inputs.
    assert_eq!(deg_to_rad(1.25), 2.1816615e-2_f32);
    assert_eq!(rad_to_deg(1.0), 5.729578e1_f32);
    assert_eq!(rad_to_deg(2.5), 1.4323946e2_f32);
    assert_eq!(rad_to_deg(0.75), 4.2971836e1_f32);
}

static LIGHT: EnumTable = EnumTable {
    entries: &[
        (0, "NoLight"),
        (1, "ConstantLight"),
        (2, "PulsingLight"),
        (3, "WeaponFireLight"),
    ],
};

#[test]
fn enums_match_labels_ignoring_case_and_store_zero_otherwise() {
    assert_eq!(
        writes(
            FieldType::Enum(&LIGHT),
            &[
                "NoLight",
                "ConstantLight",
                "PulsingLight",
                "WeaponFireLight",
                "abc",
                "1",
                "constantlight",
                "",
                "SpotLight"
            ]
        ),
        [
            "NoLight",
            "ConstantLight",
            "PulsingLight",
            "WeaponFireLight",
            "NoLight",
            "NoLight",
            "ConstantLight",
            "NoLight",
            "NoLight"
        ]
    );
}

#[test]
fn strings_take_the_first_spelling_and_case_strings_their_own() {
    let mut env = Env::default();
    let mut a = zero(FieldType::String);
    set(FieldType::String, &mut a, b"MixedCase", &mut env);
    let mut b = zero(FieldType::String);
    set(FieldType::String, &mut b, b"mixedcase", &mut env);
    assert_eq!(get(FieldType::String, &b, &names), b"MixedCase");
    let mut c = zero(FieldType::CaseString);
    set(FieldType::CaseString, &mut c, b"mixedcase", &mut env);
    assert_eq!(get(FieldType::CaseString, &c, &names), b"mixedcase");
}

#[test]
fn a_datablock_pointer_is_cleared_then_looked_up() {
    let mut env = Env::default();
    let ty = FieldType::DataBlock("ProjectileData");
    let mut p = zero(ty);
    set(ty, &mut p, b"Known", &mut env);
    assert_eq!(get(ty, &p, &names), b"Known");
    set(ty, &mut p, b"NoSuchData", &mut env);
    assert_eq!(get(ty, &p, &names), b"");
    set(ty, &mut p, b"", &mut env);
    set(ty, &mut p, b"0", &mut env);
    assert_eq!(
        env.messages,
        [
            "'NoSuchData' is not a datablock of class ProjectileData",
            "'0' is not a datablock of class ProjectileData"
        ]
    );
    // The line is a normal-level console line, not a warning.
    assert_eq!(env.levels, [Level::Normal, Level::Normal]);
}

#[test]
fn list_types_stop_at_the_first_piece_that_is_not_a_number() {
    assert_eq!(
        writes(
            FieldType::S32Vector,
            &["1 2 x 4", " 7 8", "", "5", "1  2", "-3 +4 9z 6", "x"]
        ),
        ["1 2", "7 7 8", "", "5", "1 2 2", "-3 4 9 6", ""]
    );
    assert_eq!(
        writes(
            FieldType::F32Vector,
            &["1.5 2e1", "1.5 x 3", "", "0.1 inf", "  .5"]
        ),
        [
            "1.500000 20.000000",
            "1.500000",
            "",
            "0.100000 inf",
            "0.500000 0.500000 0.500000"
        ]
    );
}

/// TGE's walk, piece by piece over the rest of the text (quadratic; `list` is linear).
fn naive_ints(text: &[u8]) -> Vec<i32> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut v = [0];
        if scan_ints(rest, &mut v) != 1 {
            break;
        }
        out.push(v[0]);
        match rest.iter().position(|&b| b == b' ') {
            Some(space) => rest = &rest[space + 1..],
            None => break,
        }
    }
    out
}

#[test]
fn the_linear_list_walk_gives_the_naive_walks_pieces() {
    let alphabet = [b' ', b'1', b'x', b'\t', b'-', b'7'];
    let mut words: Vec<Vec<u8>> = vec![Vec::new()];
    for _ in 0..7 {
        let longer: Vec<Vec<u8>> = words
            .iter()
            .flat_map(|w| {
                alphabet.iter().map(move |&c| {
                    let mut n = w.clone();
                    n.push(c);
                    n
                })
            })
            .collect();
        for w in &longer {
            let mut slot = zero(FieldType::S32Vector);
            set(FieldType::S32Vector, &mut slot, w, &mut Env::default());
            let Value::IntList(got) = slot else {
                panic!("not a list")
            };
            assert_eq!(
                &*got,
                &naive_ints(w)[..],
                "{:?}",
                String::from_utf8_lossy(w)
            );
        }
        words = longer;
    }
}

#[test]
fn small_integers_keep_the_low_bits_of_atoi() {
    assert_eq!(
        writes(
            FieldType::U8,
            &["300", "-1", "255", "256", "70000", "abc", "1.9"]
        ),
        ["44", "255", "255", "0", "112", "0", "1"]
    );
    assert_eq!(
        writes(
            FieldType::U16,
            &["300", "70000", "-1", "65535", "65536", "abc", "99999999999"]
        ),
        ["300", "4464", "65535", "65535", "0", "0", "65535"]
    );
}

#[test]
fn deprecated_fields_store_nothing_and_read_empty() {
    let mut env = Env::default();
    let mut slot = zero(FieldType::Deprecated);
    set(FieldType::Deprecated, &mut slot, b"anything", &mut env);
    assert_eq!(get(FieldType::Deprecated, &slot, &names), b"");
    assert_eq!(slot, zero(FieldType::Deprecated));
    assert!(env.messages.is_empty());
}

#[test]
fn opaque_and_filename_keep_text_and_a_refused_filename_warns() {
    struct Refuse(Vec<String>);
    impl FieldEnv for Refuse {
        fn intern(&mut self, text: &[u8]) -> Rc<[u8]> {
            Rc::from(text)
        }
        fn expand_filename(&mut self, text: &[u8]) -> Option<Vec<u8>> {
            (text != b"bad").then(|| text.to_vec())
        }
        fn find_object(&mut self, _: &[u8], _: &str) -> Option<ObjId> {
            None
        }
        fn message(&mut self, level: Level, text: String) {
            assert_eq!(level, Level::Warning);
            self.0.push(text);
        }
        fn charge(&mut self, _: u64) -> bool {
            true
        }
    }
    let mut env = Refuse(Vec::new());
    let mut slot = zero(FieldType::Filename);
    set(FieldType::Filename, &mut slot, b"good.txt", &mut env);
    set(FieldType::Filename, &mut slot, b"bad", &mut env);
    assert_eq!(get(FieldType::Filename, &slot, &names), b"good.txt");
    assert_eq!(env.0, ["(TypeFilename) illegal filename detected: bad"]);
    let ty = FieldType::Opaque("TriggerPolyhedron");
    let mut slot = zero(ty);
    set(ty, &mut slot, b"0 0 0 1", &mut env);
    assert_eq!(get(ty, &slot, &names), b"0 0 0 1");
}

#[test]
fn a_gui_profile_keeps_the_old_one_when_the_name_is_unknown() {
    struct Profiles;
    impl FieldEnv for Profiles {
        fn intern(&mut self, text: &[u8]) -> Rc<[u8]> {
            Rc::from(text)
        }
        fn expand_filename(&mut self, text: &[u8]) -> Option<Vec<u8>> {
            Some(text.to_vec())
        }
        fn find_object(&mut self, text: &[u8], class: &str) -> Option<ObjId> {
            assert_eq!(class, "GuiControlProfile");
            (text == b"Known").then_some(7)
        }
        fn message(&mut self, _: Level, _: String) {}
        fn charge(&mut self, _: u64) -> bool {
            true
        }
    }
    let mut slot = zero(FieldType::GuiProfile);
    set(FieldType::GuiProfile, &mut slot, b"Known", &mut Profiles);
    set(FieldType::GuiProfile, &mut slot, b"Nope", &mut Profiles);
    assert_eq!(get(FieldType::GuiProfile, &slot, &names), b"Known");
}

#[test]
fn a_slot_of_the_wrong_shape_is_replaced_by_the_zero() {
    let mut env = Env::default();
    let mut slot = Value::Float(1.0);
    set(FieldType::Bool, &mut slot, b"1", &mut env);
    assert_eq!(slot, Value::Bool(true));
    let mut slot = Value::Int(3);
    set(FieldType::Point3F, &mut slot, b"1 x", &mut env);
    assert_eq!(get(FieldType::Point3F, &slot, &names), b"1 0 0");
}

#[test]
fn nul_and_megabyte_values_parse_for_every_type() {
    let types = [
        FieldType::String,
        FieldType::CaseString,
        FieldType::Filename,
        FieldType::U8,
        FieldType::U16,
        FieldType::S32,
        FieldType::F32,
        FieldType::Bool,
        FieldType::S32Vector,
        FieldType::F32Vector,
        FieldType::Enum(&LIGHT),
        FieldType::ColorI,
        FieldType::ColorF,
        FieldType::Point2I,
        FieldType::Point2F,
        FieldType::Point3F,
        FieldType::MatrixPosition,
        FieldType::MatrixRotation,
        FieldType::DataBlock("ProjectileData"),
        FieldType::GuiProfile,
        FieldType::Opaque("TriggerPolyhedron"),
        FieldType::Deprecated,
    ];
    let mega = vec![b'9'; 1 << 20];
    let spaced = b"1 ".repeat(1 << 19);
    let mut blank = vec![b' '; 1 << 20];
    blank.push(b'1');
    let inputs: [&[u8]; 7] = [
        &mega,
        &spaced,
        &blank,
        b"1\x02 3",
        b"\xff\xfe",
        b"",
        b"7\0 8 9",
    ];
    for ty in types {
        let mut env = Env::default();
        let mut slot = zero(ty);
        for input in inputs {
            set(ty, &mut slot, input, &mut env);
            set(ty, &mut slot, input, &mut env);
            let _ = get(ty, &slot, &names);
        }
    }
}

#[test]
fn scan_ints_saturates_at_the_i32_range() {
    let read = |text: &str| {
        let mut out = [7];
        assert_eq!(scan_ints(text.as_bytes(), &mut out), 1, "{text}");
        out[0]
    };
    for (text, want) in [
        ("99999999999", i32::MAX),
        ("-99999999999", i32::MIN),
        ("2147483647", i32::MAX),
        ("-2147483648", i32::MIN),
        ("99999999999999999999999", i32::MAX),
        ("00000000000000000000000000000000000005", 5),
        ("-00000000000000000000000000000000000005", -5),
    ] {
        assert_eq!(read(text), want, "{text}");
    }
    let mut out = [0; 3];
    assert_eq!(scan_ints(b"5 12345678901234567890 6", &mut out), 3);
    assert_eq!(out, [5, i32::MAX, 6]);
}

#[test]
fn scan_ints_reports_the_count_and_leaves_the_rest() {
    let mut out = [0; 3];
    assert_eq!(scan_ints(b" -4 +5 6x", &mut out), 3);
    assert_eq!(out, [-4, 5, 6]);
    let mut out = [9; 2];
    assert_eq!(scan_ints(b"7 -", &mut out), 1);
    assert_eq!(out, [7, 9]);
}

#[test]
fn a_list_is_charged_by_the_element_before_it_is_stored() {
    // `1`, a megabyte of spaces, `2`: the first space ends the `1`, so the 2 is repeated 2^20 times.
    let text = format!("1{}2", " ".repeat(1 << 20));
    for ty in [FieldType::S32Vector, FieldType::F32Vector] {
        let mut env = Env::default();
        let mut slot = zero(ty);
        set(ty, &mut slot, text.as_bytes(), &mut env);
        assert_eq!(env.charged, 1 + (1 << 20));
        let read = get(ty, &slot, &names);
        // An int reads as one digit, a float as `2.000000`.
        let width = if ty == FieldType::F32Vector { 8 } else { 1 };
        assert_eq!(read.len(), (width + 1) * ((1 << 20) + 1) - 1);

        // A budget that covers the `1` but not the run of 2s: the run is refused whole, nothing of it
        // is stored (the charge came first), and the work is still counted.
        let mut env = Env {
            budget: Some(100),
            ..Env::default()
        };
        let mut slot = zero(ty);
        set(ty, &mut slot, text.as_bytes(), &mut env);
        let one: &[u8] = if ty == FieldType::F32Vector {
            b"1.000000"
        } else {
            b"1"
        };
        assert_eq!(get(ty, &slot, &names), one);
        assert_eq!(env.charged, 1 + (1 << 20));
    }
    // Without repeats the charge is one per element.
    let mut env = Env::default();
    let mut slot = zero(FieldType::S32Vector);
    set(FieldType::S32Vector, &mut slot, b"1 2 3 4 x 5", &mut env);
    assert_eq!(env.charged, 4);
    // A budget of 2 lets two elements in: the third is refused.
    let mut env = Env {
        budget: Some(2),
        ..Env::default()
    };
    set(FieldType::S32Vector, &mut slot, b"1 2 3 4", &mut env);
    assert_eq!(get(FieldType::S32Vector, &slot, &names), b"1 2");
}

#[test]
fn a_huge_list_reads_back_no_longer_than_the_string_stack_holds() {
    let big: Vec<i32> = vec![2_147_483_647; (crate::vm::MAX_STRING_BYTES / 11) + 100];
    let slot = Value::IntList(Rc::from(big));
    let text = get(FieldType::S32Vector, &slot, &names);
    assert!(text.len() > crate::vm::MAX_STRING_BYTES);
    assert!(text.len() < crate::vm::MAX_STRING_BYTES + 12);
}

#[test]
fn a_read_is_charged_by_the_bytes_it_makes_and_stops_when_the_budget_does() {
    // 10,000 sevens read back as 19,999 bytes: charged in full, by the bytes formatted.
    let ints = Value::IntList(Rc::from(vec![7; 10_000]));
    let mut charged = 0u64;
    let all = get_charged(fixed(), FieldType::S32Vector, &ints, &names, &mut |units| {
        charged += units;
        true
    });
    assert_eq!(all.len(), 19_999);
    assert_eq!(charged, 19_999);
    assert_eq!(get(FieldType::S32Vector, &ints, &names), all);
    // Floats read as `%f`: 9 bytes with the separator.
    let floats = Value::FloatList(Rc::from(vec![0.5; 1_000]));
    let mut charged = 0u64;
    let all = get_charged(
        fixed(),
        FieldType::F32Vector,
        &floats,
        &names,
        &mut |units| {
            charged += units;
            true
        },
    );
    assert_eq!(all.len(), 9 * 1_000 - 1);
    assert_eq!(charged, all.len() as u64);
    // A budget of 10,000 units: the third chunk (the charges are 4097, 4096, 4096) is refused and the
    // formatting stops there, with the bytes it had made charged and nothing more made.
    let mut charged = 0u64;
    let cut = get_charged(fixed(), FieldType::S32Vector, &ints, &names, &mut |units| {
        charged += units;
        charged <= 10_000
    });
    assert_eq!(charged, 4097 + 4096 + 4096);
    assert_eq!(cut.len() as u64, charged);
    // An empty list charges nothing.
    let none = Value::IntList(Rc::from(Vec::<i32>::new()));
    assert!(
        get_charged(
            fixed(),
            FieldType::S32Vector,
            &none,
            &names,
            &mut |_| panic!("charged")
        )
        .is_empty()
    );
}

#[test]
fn a_text_is_charged_by_its_length_before_it_is_copied() {
    let text = Value::Text(Rc::from(&b"abcdef"[..]));
    let mut asked = Vec::new();
    let read = get_charged(fixed(), FieldType::String, &text, &names, &mut |units| {
        asked.push(units);
        true
    });
    assert_eq!(
        (read.as_slice(), asked.as_slice()),
        (&b"abcdef"[..], &[6][..])
    );
    // Refused: nothing is copied.
    assert!(get_charged(fixed(), FieldType::String, &text, &names, &mut |_| false).is_empty());
    // An object's name is a text too.
    let object = Value::Object(Some(7));
    let mut asked = Vec::new();
    let read = get_charged(
        fixed(),
        FieldType::DataBlock("X"),
        &object,
        &names,
        &mut |units| {
            asked.push(units);
            true
        },
    );
    assert_eq!(
        (read.as_slice(), asked.as_slice()),
        (&b"Known"[..], &[5][..])
    );
    assert!(
        get_charged(
            fixed(),
            FieldType::DataBlock("X"),
            &object,
            &names,
            &mut |_| false
        )
        .is_empty()
    );
    // The small types are not charged.
    let int = Value::Int(5);
    assert_eq!(
        get_charged(fixed(), FieldType::S32, &int, &names, &mut |_| panic!(
            "charged"
        )),
        b"5"
    );
}
