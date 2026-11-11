//! Classes declared in Rust: registered below their parent with typed fields, read back as the struct, with docs and
//! ranges in the dump, a preload hook and a refresh hook.

use std::cell::RefCell;

use torque_ts_vm::classes::DatablockChange;
use torque_ts_vm::vm::Vm;

use crate::class::{ObjId, Vm as ClassVm};
use crate::{
    BuildError, ClassReadError, Engine, EngineBuilder, EngineClass, Filename, Plugin, PluginId,
    PluginType, ScriptClass, ServerStage,
};

/// A datablock with one field of each kind of rule.
#[derive(EngineClass, Debug, PartialEq)]
#[class(name = "RopeData", parent = "SimDataBlock", preload = RopeData::check)]
struct RopeData {
    /// How many pieces the rope is cut into.
    #[field(range = 2..=64, default = 8)]
    segments: i32,
    /// Metres between the ends.
    #[field(range = 0.5..=100.0, default = 2.0)]
    length: f32,
    /// The picture on the rope.
    material: Filename,
    #[field(name = "stiff", default = false)]
    rigid: bool,
    label: String,
    #[field(doc = "Where the rope hangs from.")]
    anchor: [f32; 3],
    tint: [f32; 4],
    cell: [i32; 2],
    bits: u8,
}

impl RopeData {
    fn check(&self, _vm: &mut ClassVm, _id: ObjId) -> Result<(), String> {
        if self.material.ends_with(".bad") {
            return Err(format!(
                "the material {} is not a picture",
                self.material.as_str()
            ));
        }
        Ok(())
    }
}

/// A class with no preload, below a built-in class.
#[derive(EngineClass)]
struct Marker {
    #[field(range = 0..=255)]
    shade: u8,
    note: String,
}

thread_local! {
    /// What `ChainData::rebuild` was called with: (the datablock's `links`, its users, the change).
    static REFRESHED: RefCell<Vec<(i32, Vec<ObjId>, DatablockChange)>> = const { RefCell::new(Vec::new()) };
}

/// A datablock with a refresh hook and no preload.
#[derive(EngineClass)]
#[class(parent = "SimDataBlock", refresh = ChainData::rebuild)]
struct ChainData {
    links: i32,
}

impl ChainData {
    fn rebuild(
        &self,
        _vm: &mut ClassVm,
        _id: ObjId,
        users: &[ObjId],
        change: DatablockChange,
    ) -> Result<(), String> {
        REFRESHED.with(|r| r.borrow_mut().push((self.links, users.to_vec(), change)));
        if self.links < 0 {
            return Err("a chain has at least one link".to_string());
        }
        Ok(())
    }
}

struct Plain;

impl Plugin for Plain {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .script()
            .class::<RopeData>()
            .class::<Marker>()
            .class::<ChainData>();
    }
    fn id(&self) -> Option<&str> {
        Some("plain")
    }
}

impl PluginType for Plain {
    const ID: PluginId = PluginId::new("plain");
}

fn engine() -> Engine {
    EngineBuilder::new()
        .add_plugins(Plain)
        .build()
        .expect("the engine builds")
}

fn server() -> Vm {
    let engine = engine();
    let mut vm = Vm::new();
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut vm).expect("the replay runs");
    }
    vm
}

fn eval(vm: &mut Vm, text: &str) -> String {
    String::from_utf8(vm.eval(text.as_bytes()).expect("the script runs")).expect("UTF-8")
}

fn logged(vm: &Vm, part: &str) -> bool {
    vm.log_texts().iter().any(|line| line.contains(part))
}

fn object(vm: &Vm, name: &str) -> ObjId {
    vm.objects().find(name.as_bytes()).expect("the object")
}

#[test]
fn the_declarations_name_the_console_fields_and_types() {
    use torque_ts_vm::fields::FieldType;
    let fields: Vec<_> = RopeData::FIELDS.iter().map(|f| (f.name, f.ty)).collect();
    assert_eq!(
        fields,
        [
            ("segments", FieldType::S32),
            ("length", FieldType::F32),
            ("material", FieldType::Filename),
            ("stiff", FieldType::Bool),
            ("label", FieldType::String),
            ("anchor", FieldType::Point3F),
            ("tint", FieldType::ColorF),
            ("cell", FieldType::Point2I),
            ("bits", FieldType::U8),
        ]
    );
    assert_eq!(RopeData::FIELDS[0].default, Some("8"));
    assert_eq!(RopeData::FIELDS[0].range_text().as_deref(), Some("2 to 64"));
    assert_eq!(RopeData::FIELDS[2].range_text(), None);
    assert_eq!(
        (RopeData::NAME, RopeData::PARENT),
        ("RopeData", "SimDataBlock")
    );
    assert_eq!((Marker::NAME, Marker::PARENT), ("Marker", "SimObject"));
    const { assert!(RopeData::HAS_PRELOAD && !Marker::HAS_PRELOAD) };
}

#[test]
fn the_class_is_registered_below_its_parent_and_listed() {
    let engine = engine();
    let classes: Vec<_> = engine
        .classes()
        .map(|c| (c.name, c.parent, c.fields.len()))
        .collect();
    assert_eq!(
        classes,
        [
            ("RopeData", "SimDataBlock", 9),
            ("Marker", "SimObject", 2),
            ("ChainData", "SimDataBlock", 1)
        ]
    );
    let mut vm = server();
    assert_eq!(
        eval(
            &mut vm,
            "datablock RopeData(Cable) {}; return Cable.getClassName();"
        ),
        "RopeData"
    );
    let classes = &vm.objects().classes;
    let rope = classes.find(b"RopeData").expect("the class");
    let block = classes.find(b"SimDataBlock").expect("the parent");
    assert!(classes.is_a(rope, block));
}

#[test]
fn a_fresh_object_reads_the_defaults_and_the_types_zero() {
    let mut vm = server();
    vm.eval(b"datablock RopeData(Cable) {};").expect("runs");
    let rope = RopeData::read(&vm, object(&vm, "Cable")).expect("reads");
    assert_eq!(
        rope,
        RopeData {
            segments: 8,
            length: 2.0,
            material: Filename::default(),
            rigid: false,
            label: String::new(),
            anchor: [0.0; 3],
            tint: [0.0, 0.0, 0.0, 1.0],
            cell: [0, 0],
            bits: 0,
        }
    );
}

#[test]
fn fields_follow_the_console_rules_for_their_type() {
    let mut vm = server();
    vm.eval(
        br#"datablock RopeData(Cable) {
            segments = "12abc"; length = "3.5x"; material = "art/rope.png"; stiff = "TRUE";
            label = "Hello"; anchor = "1 2 3"; tint = "0.5 0.25 1"; cell = "4 5"; bits = 300;
        };"#,
    )
    .expect("runs");
    let rope = RopeData::read(&vm, object(&vm, "Cable")).expect("reads");
    assert_eq!(rope.segments, 12, "dAtoi");
    assert_eq!(rope.length, 3.5, "dAtof");
    assert_eq!(rope.material.as_str(), "art/rope.png");
    assert!(rope.rigid, "dAtob");
    assert_eq!(rope.label, "Hello");
    assert_eq!(rope.anchor, [1.0, 2.0, 3.0]);
    assert_eq!(
        rope.tint,
        [0.5, 0.25, 1.0, 1.0],
        "alpha 1 when three are given"
    );
    assert_eq!(rope.cell, [4, 5]);
    assert_eq!(rope.bits, 44, "U8 keeps the low byte");
    // And the text reads back as the console prints each type.
    assert_eq!(eval(&mut vm, "return Cable.anchor;"), "1 2 3");
    assert_eq!(eval(&mut vm, "return Cable.stiff;"), "1");
}

#[test]
fn a_write_outside_the_range_is_clamped_with_a_warning() {
    let mut vm = server();
    vm.eval(b"datablock RopeData(Cable) { segments = 100; length = 0.1; };")
        .expect("runs");
    let rope = RopeData::read(&vm, object(&vm, "Cable")).expect("reads");
    assert_eq!((rope.segments, rope.length), (64, 0.5));
    assert!(logged(&vm, "invalid value for segments"));
    assert!(logged(
        &vm,
        "invalid value for length: Must be between 0.5 and 100"
    ));
    vm.eval(b"Cable.segments = 1;").expect("runs");
    assert_eq!(eval(&mut vm, "return Cable.segments;"), "2");
}

#[test]
fn the_dump_lists_the_docs_and_ranges_of_the_documented_fields() {
    let mut vm = server();
    vm.eval(b"datablock RopeData(Cable) { segments = 5; };")
        .expect("runs");
    vm.eval(b"Cable.dump();").expect("runs");
    let lines = vm.log_texts();
    let at = lines
        .iter()
        .position(|l| l == "Field Docs:")
        .expect("the section");
    assert_eq!(
        &lines[at + 1..at + 5],
        [
            "  anchor - Where the rope hangs from.",
            "  length - Metres between the ends. (0.5 to 100)",
            "  material - The picture on the rope.",
            "  segments - How many pieces the rope is cut into. (2 to 64)",
        ]
    );
    // The values still print as before, and the section comes before the tagged fields.
    assert!(lines.iter().any(|l| l == "  segments = \"5\""));
    let tagged = lines
        .iter()
        .position(|l| l == "Tagged Fields:")
        .expect("tagged");
    assert!(at < tagged);
}

#[test]
fn a_console_class_without_docs_dumps_as_it_always_did() {
    let mut vm = Vm::new();
    vm.eval(b"new SimObject(Plain);").expect("runs");
    vm.eval(b"Plain.dump();").expect("runs");
    assert!(!logged(&vm, "Field Docs:"));
}

#[test]
fn a_class_without_docs_for_its_fields_has_no_section_either() {
    let mut vm = server();
    vm.eval(b"new Marker(M) { shade = 7; };").expect("runs");
    vm.eval(b"M.dump();").expect("runs");
    assert!(!logged(&vm, "Field Docs:"));
    let marker = Marker::read(&vm, object(&vm, "M")).expect("reads");
    assert_eq!((marker.shade, marker.note.as_str()), (7, ""));
}

#[test]
fn preload_runs_with_the_fields_read_and_a_failure_deletes_the_object() {
    let mut vm = server();
    vm.eval(b"datablock RopeData(Good) { material = \"art/rope.png\"; };")
        .expect("runs");
    assert!(vm.objects().find(b"Good").is_some());
    vm.eval(b"datablock RopeData(Bad) { material = \"art/rope.bad\"; };")
        .expect("runs");
    assert!(
        vm.objects().find(b"Bad").is_none(),
        "the failed preload deleted it"
    );
    assert!(logged(
        &vm,
        "preload failed for Bad: the material art/rope.bad is not a picture."
    ));
}

/// The class chain's `datablock_changed` for `db`, as a server's drain runs it: whether a behavior took it.
fn changed(vm: &mut Vm, db: ObjId, users: &[ObjId], change: DatablockChange) -> bool {
    let class = vm.objects().get(db).expect("the datablock").class();
    let behaviors = vm.objects().classes.behaviours(class);
    behaviors.iter().fold(false, |taken, b| {
        taken | b.datablock_changed(vm, db, users, change).expect("runs")
    })
}

#[test]
fn the_refresh_hook_runs_with_the_fields_as_they_are_now_and_a_failure_is_an_error() {
    let mut vm = server();
    REFRESHED.with(|r| r.borrow_mut().clear());
    vm.eval(b"datablock ChainData(Links) { links = 3; };")
        .expect("runs");
    let db = object(&vm, "Links");
    assert!(
        changed(&mut vm, db, &[7, 9], DatablockChange::Written),
        "the class takes the refresh"
    );
    vm.eval(b"Links.links = -1;").expect("runs");
    assert!(changed(&mut vm, db, &[7], DatablockChange::Preloaded));
    assert_eq!(
        REFRESHED.with(|r| r.borrow().clone()),
        [
            (3, vec![7, 9], DatablockChange::Written),
            (-1, vec![7], DatablockChange::Preloaded),
        ]
    );
    assert!(logged(
        &vm,
        "ChainData: refresh failed: a chain has at least one link"
    ));
    // A class without the hook takes no refresh (a host warns about its users).
    vm.eval(b"datablock RopeData(Plain) { material = \"art/rope.png\"; };")
        .expect("runs");
    let plain = object(&vm, "Plain");
    assert!(!changed(&mut vm, plain, &[7], DatablockChange::Written));
    const { assert!(ChainData::HAS_REFRESH && !RopeData::HAS_REFRESH && !ChainData::HAS_PRELOAD) };
}

#[test]
fn reading_the_wrong_object_says_why() {
    let mut vm = server();
    vm.eval(b"new SimObject(Plain); new Marker(M) {};")
        .expect("runs");
    assert_eq!(
        RopeData::read(&vm, object(&vm, "Plain")),
        Err(ClassReadError::NotAnInstance {
            id: object(&vm, "Plain"),
            class: "RopeData"
        })
    );
    assert_eq!(
        RopeData::read(&vm, 99_999),
        Err(ClassReadError::Gone(99_999))
    );
    assert!(RopeData::read(&vm, object(&vm, "M")).is_err());
    let error = RopeData::read(&vm, 99_999).expect_err("gone").to_string();
    assert_eq!(error, "object 99999 is gone");
}

#[test]
fn a_class_name_taken_twice_fails_the_build() {
    struct Again;
    impl Plugin for Again {
        fn build(&self, engine: &mut EngineBuilder) {
            engine.script().class::<Marker>();
        }
        fn id(&self) -> Option<&str> {
            Some("again")
        }
    }
    impl PluginType for Again {
        const ID: PluginId = PluginId::new("again");
    }
    let error = EngineBuilder::new()
        .add_plugins((Plain, Again))
        .build()
        .expect_err("the same class twice");
    assert!(
        matches!(error, BuildError::DuplicateScriptName { kind: "class", .. }),
        "{error:?}"
    );
}

#[test]
fn a_parent_that_does_not_exist_is_a_console_error() {
    #[derive(EngineClass)]
    #[class(parent = "NoSuchClass")]
    struct Orphan {
        _a: i32,
    }
    let mut builder = EngineBuilder::new();
    builder.script().class::<Orphan>();
    let engine = builder.build().expect("the engine builds");
    let mut vm = Vm::new();
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut vm).expect("the replay runs");
    }
    assert!(logged(&vm, "class Orphan: not registered"));
}

#[test]
fn a_typed_read_does_not_give_back_a_nan_the_console_range_let_through() {
    use crate::class::{FieldValue, read_field};
    // The console's range check clamps what is out of range, but a NaN is neither below nor above it.
    let length = &RopeData::FIELDS[1];
    let mut nan = |_: &crate::FieldDecl| Some(FieldValue::Float(f32::NAN));
    assert_eq!(read_field::<f32>(length, &mut nan), Ok(0.5));
    let mut seven = |_: &crate::FieldDecl| Some(FieldValue::Float(7.0));
    assert_eq!(read_field::<f32>(length, &mut seven), Ok(7.0));
    // A float field with only the types default bounds (none) has nothing to put in its place.
    assert!(
        read_field::<f32>(
            &crate::FieldDecl {
                min: None,
                max: None,
                ..*length
            },
            &mut nan
        )
        .is_ok_and(f32::is_nan)
    );
}
