use crate::abi::{self, Config};
use crate::ui::{self, Kind, Prop, PropValue};
use crate::{
    FORMAT_VERSION, Grants, MAX_RECORD, Malformed, Reader, Records, Tier, Value, ValueError,
    Writer, net,
};

#[test]
fn tiers_round_trip_by_id_and_name() {
    for tier in Tier::ALL {
        assert_eq!(Tier::from_id(tier.id()), Some(tier));
        assert_eq!(Tier::from_name(tier.name()), Some(tier));
    }
    assert_eq!(Tier::from_id(10), None);
    assert_eq!(Tier::Local.table(), "world");
    assert_eq!(Grants::VALID, 0x3ff);
    let g = Grants::of(&[Tier::Ui]);
    assert!(g.has(Tier::Realm) && g.has(Tier::Ui) && !g.has(Tier::Net));
    assert!(!Grants(1 << 10).is_valid());
}

#[test]
fn records_round_trip_and_refuse_faults() {
    let mut w = Writer::new();
    let mark = w.begin(1, 2);
    w.u32(7);
    w.text("hi");
    w.end(mark).unwrap();
    let mark = w.begin(0, 0);
    w.end(mark).unwrap();
    let records = Records::new(w.bytes(), 8, 1 << 20)
        .unwrap()
        .collect_all()
        .unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!((records[0].tier, records[0].kind), (1, 2));
    let mut r = records[0].reader();
    assert_eq!(r.u32().unwrap(), 7);
    assert_eq!(r.text().unwrap(), "hi");
    r.finish().unwrap();
    assert!(records[1].body.is_empty());

    // Too many records, a short body, a length past the cap, too many bytes.
    assert!(
        Records::new(w.bytes(), 1, 1 << 20)
            .unwrap()
            .collect_all()
            .is_err()
    );
    let mut long = Writer::new();
    long.u8(0);
    long.u8(0);
    long.u32(MAX_RECORD as u32 + 1);
    assert!(
        Records::new(long.bytes(), 8, 1 << 20)
            .unwrap()
            .collect_all()
            .is_err()
    );
    assert!(Records::new(w.bytes(), 8, 3).is_err());
    let mut over = Writer::new();
    let mark = over.begin(0, 0);
    over.bytes_field(&vec![0; MAX_RECORD]);
    assert!(over.end(mark).is_err());
    assert!(over.is_empty());

    // Bools are 0 or 1; texts are UTF-8.
    assert!(Reader::new(&[2]).bool().is_err());
    assert!(Reader::new(&[1, 0, 0, 0, 0xff]).text().is_err());
}

#[test]
fn a_truncated_last_record_is_malformed() {
    let mut w = Writer::new();
    let mark = w.begin(0, 0);
    w.u32(1);
    w.end(mark).unwrap();
    let cut = &w.bytes()[..w.len() - 1];
    let result = Records::new(cut, 8, 1 << 20).unwrap().collect_all();
    assert_eq!(result, Err(Malformed("a field past the end")));
}

#[test]
fn values_round_trip_and_keep_their_rules() {
    let value = Value::Table(vec![
        (Value::String(b"a".to_vec()), Value::Number(1.5)),
        (Value::Number(1.0), Value::Bool(true)),
        (
            Value::Number(2.0),
            Value::Table(vec![(Value::Number(1.0), Value::Nil)]),
        ),
    ]);
    let bytes = value.to_bytes().unwrap();
    assert_eq!(Value::from_bytes(&bytes).unwrap(), value);

    // NaN and -0 have one form.
    let nan = Value::Number(f64::from_bits(0x7ff0_0000_0000_0001))
        .to_bytes()
        .unwrap();
    assert_eq!(&nan[1..], &0x7FF8_0000_0000_0000u64.to_le_bytes());
    let zero = Value::Number(-0.0).to_bytes().unwrap();
    assert_eq!(&zero[1..], &0u64.to_le_bytes());
    let mut bad = vec![3];
    bad.extend_from_slice(&0x8000_0000_0000_0000u64.to_le_bytes());
    assert!(Value::from_bytes(&bad).is_err());
    let mut bad = vec![3];
    bad.extend_from_slice(&0x7ff0_0000_0000_0001u64.to_le_bytes());
    assert!(Value::from_bytes(&bad).is_err());

    // Depth, entries, keys.
    let mut deep = Value::Nil;
    for _ in 0..9 {
        deep = Value::Table(vec![(Value::Number(1.0), deep)]);
    }
    assert_eq!(deep.to_bytes(), Err(ValueError::TooDeep));
    let mut eight = Value::Nil;
    for _ in 0..8 {
        eight = Value::Table(vec![(Value::Number(1.0), eight)]);
    }
    assert!(Value::from_bytes(&eight.to_bytes().unwrap()).is_ok());
    let wide = Value::Table(
        (0..257)
            .map(|i| (Value::Number(f64::from(i)), Value::Nil))
            .collect(),
    );
    assert_eq!(wide.to_bytes(), Err(ValueError::TooManyEntries));
    let dup = Value::Table(vec![
        (Value::Number(0.0), Value::Nil),
        (Value::Number(-0.0), Value::Nil),
    ]);
    assert_eq!(dup.to_bytes(), Err(ValueError::DuplicateKey));
    let bad_key = Value::Table(vec![(Value::Bool(true), Value::Nil)]);
    assert_eq!(bad_key.to_bytes(), Err(ValueError::BadKey));
    assert!(Value::from_bytes(&[9]).is_err());
    assert!(Value::from_bytes(&[0, 0]).is_err());
    // A table that claims more pairs than its bytes hold.
    assert!(Value::from_bytes(&[5, 0xff, 0x00]).is_err());
}

/// Props as `write_props` takes them.
fn pairs(props: &[(Prop, PropValue)]) -> impl ExactSizeIterator<Item = (Prop, &PropValue)> {
    props.iter().map(|(p, v)| (*p, v))
}

#[test]
fn props_round_trip_and_refuse_what_a_kind_lacks() {
    let button = ui::kind_def(Kind::Button);
    let props = vec![
        (Prop::Pos, PropValue::Vec2(10.0, 20.0)),
        (Prop::Text, PropValue::Text("Go".into())),
        (Prop::Style, PropValue::Enum(1)),
    ];
    let mut w = Writer::new();
    ui::write_props(&mut w, pairs(&props));
    let read = ui::read_props(&mut Reader::new(w.bytes()), button).unwrap();
    assert_eq!(read, props);

    // A prop the kind lacks, ids out of order, a value out of range.
    let mut w = Writer::new();
    ui::write_props(&mut w, pairs(&[(Prop::Rows, PropValue::Rows(vec![]))]));
    assert!(ui::read_props(&mut Reader::new(w.bytes()), button).is_err());
    let mut w = Writer::new();
    ui::write_props(
        &mut w,
        pairs(&[
            (Prop::Text, PropValue::Text("a".into())),
            (Prop::Pos, PropValue::Vec2(0.0, 0.0)),
        ]),
    );
    assert!(ui::read_props(&mut Reader::new(w.bytes()), button).is_err());
    let mut w = Writer::new();
    ui::write_props(&mut w, pairs(&[(Prop::Style, PropValue::Enum(9))]));
    assert!(ui::read_props(&mut Reader::new(w.bytes()), button).is_err());
    let mut w = Writer::new();
    ui::write_props(
        &mut w,
        pairs(&[(Prop::Pos, PropValue::Vec2(f64::INFINITY, 0.0))]),
    );
    assert!(ui::read_props(&mut Reader::new(w.bytes()), button).is_err());

    // Every prop's default is in its own range, and every kind's props are known and named once.
    for (i, def) in ui::PROPS.iter().enumerate() {
        assert_eq!(
            ui::prop_from_id(def.prop as u8),
            Some(def),
            "{} is listed once",
            def.name
        );
        assert!(
            i == 0 || ui::PROPS[i - 1].prop < def.prop,
            "PROPS is in id order"
        );
        ui::check_value(def, &def.default.value()).unwrap();
    }
    for (i, event) in ui::Event::ALL.into_iter().enumerate() {
        assert_eq!(event as usize, i);
        assert_eq!(ui::Event::from_id(i as u8), Some(event));
    }
    assert_eq!(ui::Event::Scroll as u8, crate::event::ui::SCROLL);
    for (i, kind) in ui::KINDS.iter().enumerate() {
        assert_eq!(kind.kind as usize, i);
        let mut names: Vec<_> = kind.props().map(|p| ui::prop_def(p).name).collect();
        names.sort_unstable();
        let len = names.len();
        names.dedup();
        assert_eq!(names.len(), len, "{} names a prop twice", kind.name);
    }
}

#[test]
fn no_prop_reaches_code_names_or_profiles() {
    for name in [
        "command",
        "altCommand",
        "variable",
        "accelerator",
        "class",
        "superClass",
        "name",
        "profile",
        "closeCommand",
        "validate",
        "escapeCommand",
        "expression",
        "bitmap",
        "font_file",
    ] {
        assert!(ui::PROPS.iter().all(|d| d.name != name), "{name}");
    }
}

#[test]
fn rich_text_takes_its_subset_only() {
    for ok in [
        "plain",
        "<b>bold</b> and <i>it</i>",
        "<color:ff0000>red</color><color:00ff00AA>x</color>",
        "<size:8>s</size><size:72>l</size>",
        "<align:center>mid<br>next",
        "a << b > c",
        "<b>left open",
    ] {
        ui::check_rich(ok, 1000).unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
    for bad in [
        "<a:http://x>link</a>",
        "<bitmap:base/x.png>",
        "<font:Arial:12>",
        "<size:7>",
        "<size:73>",
        "<color:red>",
        "</b>",
        "<b></i>",
        "<b",
        "<B>",
        "<b><b><b><b><b><b><b><b><b>",
        "nul\u{0}",
    ] {
        assert!(ui::check_rich(bad, 1000).is_err(), "{bad}");
    }
}

#[test]
fn content_paths_are_plain_mod_files() {
    ui::check_content_path("mods/myserver/img/logo.png", ui::IMAGE_EXTENSIONS).unwrap();
    for bad in [
        "mods/x/../../config/prefs.cs",
        "config/prefs.cs",
        "mods/x/client/main.lua",
        "C:/x.png",
        "mods\\x\\a.png",
        "/mods/x/a.png",
        "mods/x/a.png\u{0}",
        "mods/x//a.png",
        "mods/a.png",
        "mods/x/./a.png",
        "mods/x/a.PNG",
    ] {
        assert!(
            ui::check_content_path(bad, ui::IMAGE_EXTENSIONS).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn config_and_names() {
    let config = Config {
        grants: Grants::of(&[Tier::Ui, Tier::Net]),
        ..Config::default()
    };
    assert_eq!(Config::from_bytes(&config.to_bytes()), Ok(config));
    let mut bad = config.to_bytes();
    bad[..4].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    assert!(Config::from_bytes(&bad).is_err(), "another version");
    let bad = Config {
        grants: Grants(1 << 12),
        ..config
    }
    .to_bytes();
    assert!(Config::from_bytes(&bad).is_err());
    let bad = Config {
        memory: 1,
        ..config
    }
    .to_bytes();
    assert!(Config::from_bytes(&bad).is_err());

    abi::check_script_path("client/main.lua").unwrap();
    for bad in [
        "../x.lua",
        "/x.lua",
        "C:/x.lua",
        "a\\b.lua",
        "x.cs",
        "a//b.lua",
        "x.lua\u{0}",
    ] {
        assert!(abi::check_script_path(bad).is_err(), "{bad}");
    }
    abi::check_mod_id("my-mod_1.2").unwrap();
    assert!(abi::check_mod_id("..").is_err() && abi::check_mod_id("a/b").is_err());
    net::check_rpc_name("props.spawn").unwrap();
    assert!(net::check_rpc_name("a..b").is_err() && net::check_rpc_name("1.2.3.4:28000").is_err());
    assert!(abi::is_compiled(&[3, 0]) && !abi::is_compiled(b"\n--"));
}

#[test]
fn a_lists_rows_have_their_own_cap() {
    let rows = ui::prop_def(Prop::Rows);
    let full = |n: usize, len: usize| PropValue::Rows(vec!["x".repeat(len); n]);
    ui::check_value(rows, &full(4, ui::MAX_ROW_TEXT)).unwrap();
    assert_eq!(4 * ui::MAX_ROW_TEXT, ui::MAX_LIST_TEXT);
    assert!(ui::check_value(rows, &full(5, ui::MAX_ROW_TEXT)).is_err());
    let mut over = vec!["x".repeat(ui::MAX_ROW_TEXT); 4];
    over.push("y".into());
    assert!(ui::check_value(rows, &PropValue::Rows(over)).is_err());
    assert!(ui::check_value(rows, &full(1, ui::MAX_ROW_TEXT + 1)).is_err());
}

#[test]
fn a_batch_decodes_within_its_node_budget() {
    // A table of 256 tables of 255 entries is past the budget, however few bytes it takes.
    let inner = Value::Table(
        (0..255)
            .map(|i| (Value::Number(f64::from(i)), Value::Nil))
            .collect(),
    );
    let wide = Value::Table(
        (0..256)
            .map(|i| (Value::Number(f64::from(i)), inner.clone()))
            .collect(),
    );
    let bytes = wide.to_bytes().unwrap();
    assert!(Value::from_bytes(&bytes).is_err());
    // The budget is shared by every value of a batch: a table of 200 pairs is 401 nodes.
    let small = Value::Table(
        (0..200)
            .map(|i| (Value::Number(f64::from(i)), Value::Nil))
            .collect(),
    );
    let one = small.to_bytes().unwrap();
    let mut budget = crate::NodeBudget::default();
    let mut decoded = 0;
    while Value::decode(&mut Reader::new(&one), &mut budget).is_ok() {
        decoded += 1;
    }
    assert_eq!(decoded, crate::MAX_BATCH_NODES / 401);
}

#[test]
fn hidden_characters_are_refused_in_text() {
    for c in [
        '\u{0}',
        '\u{1b}',
        '\u{7f}',
        '\u{85}',
        '\u{2028}',
        '\u{2029}',
        '\u{202a}',
        '\u{202e}',
        '\u{2066}',
        '\u{2069}',
        '\u{200f}',
        '\u{61c}',
        '\u{200b}',
        '\u{200d}',
        '\u{2060}',
        '\u{2064}',
        '\u{feff}',
        '\u{e0000}',
        '\u{e0041}',
        '\u{e007f}',
    ] {
        assert!(crate::is_hidden(c), "{c:?}");
        assert!(ui::check_text(&format!("a{c}b"), 100).is_err(), "{c:?}");
    }
    ui::check_text("line\nnext\ttab \u{e9} \u{65e5}", 100).unwrap();
    for shown in ['\u{200a}', '\u{2065}', '\u{e0080}', 'a'] {
        assert!(!crate::is_hidden(shown), "{shown:?}");
    }
}

#[test]
fn names_and_paths_refuse_dots_and_device_names() {
    for bad in [
        "...", "....", "a.", "a..", "CON", "con.png", "Nul.txt", "aux", "PRN.lua", "COM1",
        "com9.png", "LPT3.x", "",
    ] {
        assert!(!crate::is_plain_part(bad), "{bad}");
        assert!(abi::check_mod_id(bad).is_err(), "{bad}");
    }
    for ok in [
        "a.b", "com", "com10", "lpt0", "console", "nul2", ".hidden", "my-mod_1",
    ] {
        assert!(crate::is_plain_part(ok), "{ok}");
    }
    for bad in [
        "mods/x/.../a.png",
        "mods/x/con.png",
        "mods/x/a./b.png",
        "mods/CON/a.png",
        "mods/x/LPT1.png",
    ] {
        assert!(
            ui::check_content_path(bad, ui::IMAGE_EXTENSIONS).is_err(),
            "{bad}"
        );
    }
    for bad in ["con.lua", "a/aux.lua", ".../a.lua", "a./b.lua"] {
        assert!(abi::check_script_path(bad).is_err(), "{bad}");
    }
}

#[test]
fn containers_have_no_look_and_every_kind_has_the_item_props() {
    for kind in ui::KINDS {
        for item in [Prop::Grow, Prop::Column, Prop::Margin, Prop::MaxSize] {
            assert!(kind.has(item), "{} lacks {item:?}", kind.name);
        }
        let container = kind.kind.container().is_some();
        for look in [Prop::Enabled, Prop::Tooltip, Prop::Opacity] {
            assert_eq!(kind.has(look), !container, "{} and {look:?}", kind.name);
        }
        assert!(kind.events.is_empty() || !container);
    }
    assert_eq!(ui::kind_named("flex").map(|k| k.kind), Some(Kind::Flex));
    assert_eq!(ui::kind_named("grid").map(|k| k.kind), Some(Kind::Grid));
    assert!(ui::kind_named("flex").unwrap().has(Prop::Direction));
    assert!(!ui::kind_named("flex").unwrap().has(Prop::Columns));
    assert!(ui::kind_named("grid").unwrap().has(Prop::Columns));
    assert!(!ui::kind_named("panel").unwrap().has(Prop::Gap));
}

/// One prop written and read back by a kind that has it.
fn round_trip(
    kind: Kind,
    prop: Prop,
    value: PropValue,
) -> Result<Vec<(Prop, PropValue)>, Malformed> {
    let mut w = Writer::new();
    ui::write_props(&mut w, pairs(&[(prop, value)]));
    let mut r = Reader::new(w.bytes());
    let props = ui::read_props(&mut r, ui::kind_def(kind))?;
    r.finish()?;
    Ok(props)
}

#[test]
fn layout_props_round_trip_and_hold_their_ranges() {
    use ui::Track;
    let tracks = PropValue::Tracks(vec![Track::Px(120.0), Track::Fr(1.5), Track::Auto]);
    assert_eq!(
        round_trip(Kind::Grid, Prop::Columns, tracks.clone()).unwrap(),
        vec![(Prop::Columns, tracks)]
    );
    let margin = PropValue::Sides([1.0, 2.0, 3.0, 4.0]);
    assert_eq!(
        round_trip(Kind::Button, Prop::Margin, margin.clone()).unwrap(),
        vec![(Prop::Margin, margin)]
    );
    let gap = PropValue::Vec2(8.0, 16.0);
    assert_eq!(
        round_trip(Kind::Flex, Prop::Gap, gap.clone()).unwrap(),
        vec![(Prop::Gap, gap)]
    );

    let too_many = PropValue::Tracks(vec![Track::Auto; ui::MAX_TRACKS + 1]);
    assert!(round_trip(Kind::Grid, Prop::Columns, too_many).is_err());
    let full = PropValue::Tracks(vec![Track::Auto; ui::MAX_TRACKS]);
    assert!(round_trip(Kind::Grid, Prop::TrackRows, full).is_ok());
    for bad in [
        Track::Px(-1.0),
        Track::Px(ui::MAX_LENGTH + 1.0),
        Track::Px(f64::NAN),
        Track::Fr(f64::INFINITY),
        Track::Fr(ui::MAX_SHARE + 1.0),
    ] {
        let value = PropValue::Tracks(vec![bad]);
        assert!(
            round_trip(Kind::Grid, Prop::Columns, value).is_err(),
            "{bad:?}"
        );
    }
    for bad in [
        PropValue::Sides([0.0, 0.0, 0.0, -1.0]),
        PropValue::Sides([0.0, ui::MAX_SPACING + 1.0, 0.0, 0.0]),
        PropValue::Sides([0.0, 0.0, f64::NAN, 0.0]),
    ] {
        assert!(round_trip(Kind::Flex, Prop::Padding, bad).is_err());
    }
    assert!(
        round_trip(
            Kind::Flex,
            Prop::Gap,
            PropValue::Vec2(0.0, ui::MAX_SPACING + 1.0)
        )
        .is_err()
    );
    assert!(
        round_trip(
            Kind::Panel,
            Prop::Grow,
            PropValue::Number(ui::MAX_SHARE + 1.0)
        )
        .is_err()
    );
    assert!(round_trip(Kind::Panel, Prop::Basis, PropValue::Number(-2.0)).is_err());
    assert!(
        round_trip(
            Kind::Panel,
            Prop::Row,
            PropValue::Int(ui::MAX_TRACKS as u32 + 1)
        )
        .is_err()
    );
    assert!(round_trip(Kind::Panel, Prop::ColumnSpan, PropValue::Int(0)).is_err());
    // A prop of the other container, and a look a container lacks.
    assert!(round_trip(Kind::Flex, Prop::Columns, PropValue::Tracks(vec![])).is_err());
    assert!(round_trip(Kind::Grid, Prop::Direction, PropValue::Enum(0)).is_err());
    assert!(round_trip(Kind::Flex, Prop::Opacity, PropValue::Number(1.0)).is_err());

    // A track count past the cap is refused before any track is read, and an unknown tag is malformed.
    let mut w = Writer::new();
    w.u8(1);
    w.u8(Prop::Columns as u8);
    w.u8(255);
    assert!(ui::read_props(&mut Reader::new(w.bytes()), ui::kind_def(Kind::Grid)).is_err());
    let mut w = Writer::new();
    w.u8(1);
    w.u8(Prop::Columns as u8);
    w.u8(1);
    w.u8(9);
    assert!(ui::read_props(&mut Reader::new(w.bytes()), ui::kind_def(Kind::Grid)).is_err());
}

#[test]
fn layout_props_read_as_the_words_of_css() {
    use ui::Track;
    let text = |prop, value: PropValue| ui::field_text(ui::prop_def(prop), &value);
    assert_eq!(text(Prop::Direction, PropValue::Enum(2)), "row-reverse");
    assert_eq!(text(Prop::Gap, PropValue::Vec2(8.0, 16.5)), "8 16.5");
    assert_eq!(
        text(Prop::Padding, PropValue::Sides([1.0, 2.0, 3.0, 4.0])),
        "1 2 3 4"
    );
    assert_eq!(text(Prop::Grow, PropValue::Number(0.25)), "0.25");
    assert_eq!(text(Prop::Basis, PropValue::Number(-1.0)), "auto");
    assert_eq!(text(Prop::Basis, PropValue::Number(0.0)), "0");
    assert_eq!(text(Prop::Row, PropValue::Int(3)), "3");
    assert_eq!(
        text(
            Prop::Columns,
            PropValue::Tracks(vec![Track::Px(120.0), Track::Fr(1.0), Track::Auto])
        ),
        "120 1fr auto"
    );
    assert_eq!(text(Prop::Columns, PropValue::Tracks(vec![])), "");
    assert_eq!(text(Prop::AutoFlow, PropValue::Enum(3)), "column dense");
    // Numbers keep four decimals, never a long tail, and never a negative zero.
    assert_eq!(text(Prop::Grow, PropValue::Number(1e-300)), "0");
    assert_eq!(text(Prop::Grow, PropValue::Number(-1e-300)), "0");
    assert_eq!(text(Prop::Grow, PropValue::Number(-0.0)), "0");
    assert_eq!(text(Prop::Grow, PropValue::Number(0.12345678)), "0.1235");
    assert_eq!(text(Prop::Gap, PropValue::Vec2(-0.0, 2.50)), "0 2.5");
    assert_eq!(ui::number_text(16384.0), "16384");
    // No value of any prop is longer than its numbers and spaces allow.
    for value in [
        f64::MIN_POSITIVE,
        1e-300,
        1e9,
        -1e9,
        16384.00004,
        0.00005,
        123456789.123456,
    ] {
        assert!(
            ui::number_text(value).len() <= ui::MAX_NUMBER_TEXT,
            "{value}"
        );
    }
    let longest = PropValue::Tracks(vec![ui::Track::Fr(-1e9); ui::MAX_TRACKS]);
    assert!(text(Prop::Columns, longest).len() <= ui::MAX_TRACKS * (ui::MAX_NUMBER_TEXT + 3));
    // An index past the names reads as nothing, never a panic.
    assert_eq!(text(Prop::Direction, PropValue::Enum(200)), "");
}

#[test]
fn hostile_bytes_never_panic_the_props_reader() {
    // A small xorshift: the same bytes every run.
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let cases = if std::env::var_os("TORQUE_CLIENT_SCRIPT_FUZZ").is_some() {
        300_000
    } else {
        20_000
    };
    for _ in 0..cases {
        let len = (next() % 80) as usize;
        let mut bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        // Mostly well-shaped starts: one to three props of the layout ids, so the payload readers run.
        if len >= 2 && next() % 4 != 0 {
            bytes[0] = (next() % 3 + 1) as u8;
            bytes[1] = (Prop::Grow as u8) + (next() % 23) as u8;
        }
        let kind = &ui::KINDS[(next() % ui::KINDS.len() as u64) as usize];
        // Whatever it reads is within the type's range, and never more than the bytes could hold.
        if let Ok(props) = ui::read_props(&mut Reader::new(&bytes), kind) {
            for (prop, value) in &props {
                ui::check_value(ui::prop_def(*prop), value).unwrap();
            }
            assert!(props.len() <= bytes.len());
        }
    }
}
