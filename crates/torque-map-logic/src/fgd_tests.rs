//! The FGD reader on definitions written here (none from a game): the grammar, includes, inheritance and the caps.

use crate::fgd::{
    EntityDefs, Fgd, KeyType, MAX_CLASSES, MAX_FILE_BYTES, MemorySource, Typed, plain_name,
};

const BASE: &str = r#"
// A comment line.
@mapsize(-16384, 16384)

@BaseClass = Named
[
    targetname(target_source) : "Name" : : "What others call it"
    input Kill(void) : "Removes it"
    output OnUser1(void) : "Fired by FireUser1"
]

@BaseClass base(Named) = Switchable
[
    StartDisabled(choices) : "Start off" : 0 =
    [
        0 : "No"
        1 : "Yes"
    ]
    input Enable(void) : "On"
    input Disable(void) : "Off"
]

@BaseClass base(Named, Switchable) = Volume
[
    spawnflags(Flags) =
    [
        1 : "Players" : 1
        2 : "Others" : 0
    ]
    wait(integer) readonly : "Wait" : 1 : "Seconds between "
        + "triggers"
    output OnTrigger(void) : "Fired"
]

@AutoVisGroup = "Tools"
[
    "Volumes" [ "box_volume" "other_volume" ]
]
@MaterialExclusion [ "dev" "tools" ]
"#;

const GAME: &str = r#"
@include "base.fgd"
@include "base.fgd"

@SolidClass base(Volume) = box_volume : "A box"
[
    spawnflags(flags) = [ 4 : "Props" : 1  1 : "Players" : 0 ]
    wait(float) report : "Wait" : "0.5"
    color(color255) : "Tint" : "10 20 30"
    dir(angle) : "Direction" : "0 90 0"
    input Toggle(void) : "Flip"
    output OnTrigger(string) : "Fired with a word"
]

@PointClass base(Named) iconsprite("x.vmt") size(-8 -8 -8, 8 8 8) halfgridsnap = point_thing : "First"
[
    radius(float) : "Radius" : 64
]

@PointClass base(Named) = point_thing : "Second, which replaces the first"
[
    height(integer) : "Height" : 8
]

@ExtendClass = point_thing
[
    extra(boolean) : "Extra" : 1
]

@ExtendClass = not_defined [ nothing(string) : "x" ]

@UnknownSection(1, 2) [ [ nested ] ]

@PointClass base(Loop1) = looped [ k(integer) : "k" : 1 ]
@BaseClass base(Loop2) = Loop1 [ a(integer) : "a" : 1 ]
@BaseClass base(Loop1) = Loop2 [ b(integer) : "b" : 2 ]
"#;

fn game() -> MemorySource {
    MemorySource(vec![
        ("base.fgd".into(), BASE.as_bytes().to_vec()),
        ("game.fgd".into(), GAME.as_bytes().to_vec()),
    ])
}

#[test]
fn classes_flatten_their_bases_in_order_without_repeats() {
    let fgd = Fgd::load(&game(), "game.fgd");
    assert_eq!(fgd.files, ["game.fgd", "base.fgd"]);
    let defs = EntityDefs::new(fgd);
    let c = defs.class("BOX_VOLUME").unwrap();
    assert_eq!(c.kind, "solidclass");
    let names: Vec<&str> = c.keys.iter().map(|k| k.name.as_str()).collect();
    // Named reached twice (directly and through Switchable) adds its key once.
    assert_eq!(
        names,
        [
            "targetname",
            "StartDisabled",
            "spawnflags",
            "wait",
            "color",
            "dir"
        ]
    );
    // The class's own wait replaces the base's in its place.
    let wait = c.key("wait").unwrap();
    assert_eq!(
        (wait.ty.clone(), wait.default.as_deref()),
        (KeyType::Float, Some("0.5"))
    );
    // Flags merge bit by bit; the class's own default for bit 1 wins.
    assert_eq!(c.default_spawnflags(), Some(4));
    assert_eq!(c.key("spawnflags").unwrap().choices.len(), 3);
    assert_eq!(c.key("StartDisabled").unwrap().choices.len(), 2);
    assert_eq!(c.key("targetname").unwrap().default, None);
    let inputs: Vec<&str> = c.inputs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(inputs, ["Kill", "Enable", "Disable", "Toggle"]);
    assert!(c.has_output("ontrigger") && c.has_output("OnUser1"));
    assert_eq!(
        c.outputs.iter().find(|o| o.name == "OnTrigger").unwrap().ty,
        "string"
    );
    assert!(
        defs.class("Named").is_none(),
        "base classes are not entities"
    );
}

#[test]
fn the_last_class_of_a_name_wins_and_extend_adds_to_it() {
    let defs = EntityDefs::new(Fgd::load(&game(), "game.fgd"));
    let p = defs.class("point_thing").unwrap();
    assert!(p.key("radius").is_none());
    assert_eq!(p.key("height").unwrap().default.as_deref(), Some("8"));
    assert_eq!(p.key("extra").unwrap().ty, KeyType::Boolean);
    assert!(defs.class("not_defined").is_none());
}

#[test]
fn a_base_cycle_is_cut_and_reported() {
    let defs = EntityDefs::new(Fgd::load(&game(), "game.fgd"));
    let l = defs.class("looped").unwrap();
    let names: Vec<&str> = l.keys.iter().map(|k| k.name.as_str()).collect();
    assert_eq!(names, ["b", "a", "k"]);
    assert!(defs.warnings().iter().any(|w| w.contains("cycle")));
}

#[test]
fn typed_values_read_by_the_declared_type() {
    let defs = EntityDefs::new(Fgd::load(&game(), "game.fgd"));
    assert_eq!(defs.typed("box_volume", "wait", "2.5x"), Typed::Float(2.5));
    assert_eq!(
        defs.typed("box_volume", "color", "1 2"),
        Typed::Color([1, 2, 255])
    );
    assert_eq!(defs.typed("box_volume", "spawnflags", "3"), Typed::Int(3));
    assert_eq!(defs.typed("point_thing", "extra", "0"), Typed::Bool(false));
    assert_eq!(
        defs.typed("box_volume", "dir", "0 90 0"),
        Typed::Str("0 90 0".into())
    );
    assert_eq!(defs.typed("nope", "x", "y"), Typed::Str("y".into()));
    assert_eq!(KeyType::parse("Target_Destination"), KeyType::Target);
    assert_eq!(KeyType::parse("*Sound"), KeyType::Other("*sound".into()));
}

#[test]
fn includes_stay_in_the_folder_and_missing_ones_are_warnings() {
    for bad in [
        "../base.fgd",
        "sub/base.fgd",
        "sub\\base.fgd",
        "C:base.fgd",
        "",
        "a\u{1}",
    ] {
        assert!(!plain_name(bad), "{bad:?}");
    }
    assert!(plain_name("base.fgd"));
    let text = br#"@include "../escape.fgd" @include "missing.fgd" @include "/etc/x" @PointClass = ok [ ]"#;
    let fgd = Fgd::parse(text);
    assert!(fgd.classes.contains_key("ok"));
    assert_eq!(fgd.files, ["main.fgd"]);
    assert_eq!(fgd.warnings.len(), 3, "{:?}", fgd.warnings);
}

#[test]
fn includes_nest_to_a_cap() {
    let mut files = Vec::new();
    for i in 0..20 {
        let next = format!("@include \"f{}.fgd\" @PointClass = class{i} [ ]", i + 1);
        files.push((format!("f{i}.fgd"), next.into_bytes()));
    }
    let fgd = Fgd::load(&MemorySource(files), "f0.fgd");
    assert_eq!(fgd.files.len(), crate::fgd::MAX_INCLUDE_DEPTH + 1);
    assert!(fgd.warnings.iter().any(|w| w.contains("nest")));
}

#[test]
fn caps_stop_a_file_and_keep_what_was_read() {
    let big = vec![b' '; MAX_FILE_BYTES + 1];
    let fgd = Fgd::parse(&big);
    assert!(fgd.files.is_empty() && !fgd.warnings.is_empty());
    let mut many = String::new();
    for i in 0..MAX_CLASSES + 10 {
        many.push_str(&format!("@PointClass = c{i} [ ]\n"));
    }
    let fgd = Fgd::parse(many.as_bytes());
    assert_eq!(fgd.classes.len(), MAX_CLASSES);
    assert!(fgd.warnings.iter().any(|w| w.contains("classes")));
    // Deep brackets in a skipped section end at the nesting cap.
    let deep = format!(
        "@Odd {}{} @PointClass = after [ ]",
        "[".repeat(10_000),
        "]".repeat(10_000)
    );
    let fgd = Fgd::parse(deep.as_bytes());
    assert!(fgd.warnings.iter().any(|w| w.contains("step")) || !fgd.classes.contains_key("after"));
}

#[test]
fn text_that_is_not_utf8_reads_byte_by_byte() {
    let mut text = b"@PointClass = caf".to_vec();
    text.push(0xE9);
    text.extend_from_slice(b" [ k(string) : \"\xA9 label\" : \"d\xE9f\" ]");
    let fgd = Fgd::parse(&text);
    let c = &fgd.classes["caf\u{e9}"];
    assert_eq!(c.keys[0].default.as_deref(), Some("d\u{e9}f"));
}

#[test]
fn broken_text_reads_what_it_can() {
    for text in [
        "@",
        "@PointClass",
        "@PointClass =",
        "@PointClass = x",
        "@PointClass = x [",
        "@PointClass = x [ k(",
        "@PointClass = x [ k(integer) : ",
        "@PointClass = x [ k(choices) : \"a\" : 0 = [ 0 : ",
        "@PointClass = x [ input",
        "\"unterminated",
        "]]]]",
        "@PointClass base(a, , b,) = y [ ] @PointClass = z [ ]",
    ] {
        let fgd = Fgd::parse(text.as_bytes());
        let _ = EntityDefs::new(fgd);
    }
    let fgd = Fgd::parse(b"@PointClass base(a, , b,) = y [ ] @PointClass = z [ ]");
    assert_eq!(fgd.classes["y"].bases, ["a", "b"]);
    assert!(fgd.classes.contains_key("z"));
}

#[test]
fn a_huge_base_under_many_classes_is_flattened_lazily_within_its_caps() {
    use crate::fgd::{MAX_ENTRIES, MAX_FLAT_ENTRIES};
    // A base class of nearly 4 MB (the most keys a class keeps, each with a long label) under the most classes a file keeps.
    let label = "x".repeat(3_700);
    let mut text = String::from("@BaseClass = Big\n[\n");
    for k in 0..MAX_ENTRIES {
        text.push_str(&format!("    key{k}(integer) : \"{label}\" : {k}\n"));
    }
    text.push_str("]\n");
    assert!(text.len() > 3 << 20 && text.len() < MAX_FILE_BYTES);
    for i in 0..MAX_CLASSES - 1 {
        text.push_str(&format!("@PointClass base(Big) = c{i} [ ]\n"));
    }
    let t0 = std::time::Instant::now();
    let defs = EntityDefs::new(Fgd::parse(text.as_bytes()));
    assert_eq!(defs.len(), MAX_CLASSES - 1);
    assert_eq!(
        defs.flat_entries(),
        0,
        "nothing is flattened before it is asked for"
    );
    let first = defs.class("c0").unwrap();
    assert_eq!(first.keys.len(), MAX_ENTRIES);
    assert_eq!(first.key("key7").unwrap().default.as_deref(), Some("7"));
    // Asking for every class stops at the cap on flattened entries; the rest read as unknown.
    let known = (0..MAX_CLASSES - 1)
        .filter(|i| defs.class(&format!("c{i}")).is_some())
        .count();
    assert!(defs.flat_entries() <= MAX_FLAT_ENTRIES);
    assert_eq!(known, MAX_FLAT_ENTRIES / MAX_ENTRIES);
    assert!(defs.warnings().iter().any(|w| w.contains("entries")));
    // A class asked for again comes from the cache.
    assert!(std::rc::Rc::ptr_eq(&first, &defs.class("C0").unwrap()));
    assert!(t0.elapsed().as_secs() < 30, "{:?}", t0.elapsed());
}

#[test]
fn long_names_and_defaults_are_shared_not_copied_into_each_class() {
    use crate::fgd::{MAX_ENTRIES, MAX_FLAT_BYTES};
    // A base of the most keys, each with a name and a default of about 1,800 bytes, in its own file; the most classes under it.
    let long = "n".repeat(1_800);
    let mut base = String::from("@BaseClass = Big\n[\n");
    for k in 0..MAX_ENTRIES {
        base.push_str(&format!("    {long}{k}(string) : \"\" : \"{long}{k}\"\n"));
    }
    base.push_str("]\n");
    assert!(base.len() > 3 << 20 && base.len() < MAX_FILE_BYTES);
    let mut main = String::from("@include \"base.fgd\"\n");
    for i in 0..MAX_CLASSES - 1 {
        main.push_str(&format!("@PointClass base(Big) = c{i} [ ]\n"));
    }
    let src = MemorySource(vec![
        ("main.fgd".into(), main.into_bytes()),
        ("base.fgd".into(), base.into_bytes()),
    ]);
    let defs = EntityDefs::new(Fgd::load(&src, "main.fgd"));
    let known = (0..MAX_CLASSES - 1)
        .filter(|i| defs.class(&format!("c{i}")).is_some())
        .count();
    assert!(known > 0);
    let (a, b) = (defs.class("c0").unwrap(), defs.class("c1").unwrap());
    assert!(
        std::rc::Rc::ptr_eq(&a.keys[5], &b.keys[5]),
        "one entry, shared"
    );
    assert_eq!(a.keys[5].default.as_deref().map(str::len), Some(1_801));
    // Pointers only: about 8 bytes an entry, far under the cap (copies would be gigabytes).
    assert!(
        defs.flat_bytes() <= known * MAX_ENTRIES * 8,
        "{}",
        defs.flat_bytes()
    );
    assert!(defs.flat_bytes() <= MAX_FLAT_BYTES);
}

#[test]
fn flags_merges_copy_within_the_byte_cap() {
    use crate::fgd::{MAX_CHOICES, MAX_FLAT_BYTES};
    // A flags key of all but one of the most choices, each a long value; every class adds a flag of its own, so each copies
    // the key.
    let mut text = String::from("@BaseClass = Flagged [ spawnflags(flags) = [\n");
    for k in 0..MAX_CHOICES - 1 {
        text.push_str(&format!("{}{k} : \"\" : 0\n", "7".repeat(3_000)));
    }
    text.push_str("] ]\n");
    for i in 0..300 {
        text.push_str(&format!(
            "@PointClass base(Flagged) = f{i} [ spawnflags(flags) = [ {} : \"\" : 1 ] ]\n",
            i + 1
        ));
    }
    assert!(text.len() < MAX_FILE_BYTES);
    let defs = EntityDefs::new(Fgd::parse(text.as_bytes()));
    let known = (0..300)
        .filter(|i| defs.class(&format!("f{i}")).is_some())
        .count();
    let f0 = defs.class("f0").unwrap();
    assert_eq!(f0.key("spawnflags").unwrap().choices.len(), MAX_CHOICES);
    assert_eq!(f0.default_spawnflags(), Some(1));
    assert!(known < 300, "the cap stopped the copies");
    assert!(defs.flat_bytes() <= MAX_FLAT_BYTES);
    assert!(defs.warnings().iter().any(|w| w.contains("bytes")));
}

#[test]
fn classes_changing_a_large_shared_flags_key_pay_before_copying_and_stop_at_the_caps() {
    use crate::fgd::{MAX_CHOICES, MAX_FLAT_BYTES, MAX_FLATTEN_WORK};
    // A shared flags key of about 3 MB; each of 2,000 classes adds a flag, so flattening it copies the key.
    let mut text = String::from("@BaseClass = Flagged [ spawnflags(flags) = [\n");
    for k in 0..MAX_CHOICES - 1 {
        text.push_str(&format!("{}{k} : \"\" : 0\n", "3".repeat(3_000)));
    }
    text.push_str("] ]\n");
    for i in 0..2_000 {
        text.push_str(&format!(
            "@PointClass base(Flagged) = g{i} [ spawnflags(flags) = [ {} : \"\" : 1 ] ]\n",
            i + 1
        ));
    }
    assert!(text.len() < MAX_FILE_BYTES);
    let defs = EntityDefs::new(Fgd::parse(text.as_bytes()));
    let mut known = 0;
    let mut work_when_full = None;
    for i in 0..2_000 {
        if defs.class(&format!("g{i}")).is_some() {
            known += 1;
        } else if work_when_full.is_none() {
            work_when_full = Some(defs.flatten_work());
        }
    }
    let bytes_per_copy = 3_000 * (MAX_CHOICES - 1);
    assert!(
        known > 0 && known <= MAX_FLAT_BYTES / bytes_per_copy,
        "{known}"
    );
    assert!(defs.flat_bytes() <= MAX_FLAT_BYTES);
    assert!(defs.flatten_work() <= MAX_FLATTEN_WORK + MAX_CHOICES);
    // Once full, the later classes are refused without being flattened: no more work.
    assert_eq!(Some(defs.flatten_work()), work_when_full);
    assert!(defs.warnings().iter().any(|w| w.contains("bytes")));
}
