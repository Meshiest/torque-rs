//! What `collect` reads from script engines made by a small engine: natives declared by the attributes (with their docs),
//! natives registered straight into the script engine, classes and prefs.

use torque_engine::{
    ClientStage, EngineBuilder, EngineClass, Plugin, PluginId, Prefs, ServerStage, This, function,
    methods, natives,
};
use torque_ts_vm::vm::{Native, Vm};

use crate::collect::{SideVm, collect};
use crate::model::{Reference, Side};

/// sets the stick dead zone
///
/// The zone is kept between 0 and 0.75.
#[function(ns = "Pad")]
fn set_dead_zone(zone: f32) -> f32 {
    zone.clamp(0.0, 0.75)
}

struct Crate;

#[methods(class = "Container")]
impl Crate {
    /// the object's id
    pub fn id_of(this: This) -> u32 {
        this.0
    }
}

#[derive(Prefs)]
#[prefs(prefix = "$Pref::Pad")]
#[allow(dead_code, reason = "the prefs are only declared")]
struct PadPrefs {
    /// How far the stick must move before it counts.
    #[pref(default = 0.15, range = 0.0..=0.9)]
    dead_zone: f32,
}

#[derive(EngineClass)]
#[class(name = "RopeData", parent = "SimDataBlock")]
#[allow(dead_code, reason = "the class is only declared")]
struct RopeData {
    /// How many pieces the rope is cut into.
    #[field(range = 2..=64, default = 8)]
    segments: i32,
}

#[derive(PluginId)]
#[plugin(id = "test.reference")]
struct TestPlugin;

impl Plugin for TestPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .script()
            .natives(natives![set_dead_zone, Crate])
            .prefs::<PadPrefs>()
            .class::<RopeData>();
        engine.server(ServerStage::Game, |vm, _| {
            vm.register(
                "serverOnly",
                1,
                1,
                "serverOnly() - only on the server",
                |_, _| Native::Void,
            );
            vm.register_in(
                "SimObject",
                "raw",
                2,
                0,
                "obj.raw(args...)\nsecond line",
                |_, _| Native::Void,
            );
            Ok(())
        });
        engine.client(ClientStage::Natives, |vm, _| {
            vm.register("clientOnly", 1, 1, "", |_, _| Native::Void);
            Ok(())
        });
    }
}

fn read() -> Reference {
    let engine = EngineBuilder::new()
        .add_plugins(TestPlugin)
        .build()
        .expect("the engine builds");
    let mut server = Vm::new();
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut server).expect("server stage");
    }
    let mut client = Vm::new();
    for &stage in ClientStage::ALL {
        engine.replay(stage, &mut client).expect("client stage");
    }
    collect(
        &engine,
        &[
            SideVm {
                side: Side::Server,
                vm: &server,
            },
            SideVm {
                side: Side::Client,
                vm: &client,
            },
        ],
    )
}

fn native<'a>(reference: &'a Reference, namespace: &str, name: &str) -> &'a crate::model::Native {
    reference
        .natives
        .iter()
        .find(|n| n.namespace == namespace && n.name == name)
        .unwrap_or_else(|| panic!("{namespace}::{name} is listed"))
}

#[test]
fn a_declared_native_keeps_its_usage_arity_and_the_rest_of_its_docs() {
    let reference = read();
    let pad = native(&reference, "Pad", "setDeadZone");
    assert_eq!(
        pad.usage,
        "Pad::setDeadZone(zone) - sets the stick dead zone"
    );
    assert_eq!((pad.arity.min, pad.arity.max), (1, Some(1)));
    assert_eq!(pad.docs, "The zone is kept between 0 and 0.75.");
    assert!(!pad.method);
    assert!(pad.sides.both());
}

#[test]
fn a_method_counts_neither_its_name_nor_its_object() {
    let reference = read();
    let id = native(&reference, "Container", "idOf");
    assert!(id.method);
    assert_eq!((id.arity.min, id.arity.max), (0, Some(0)));
    let raw = native(&reference, "SimObject", "raw");
    assert_eq!((raw.arity.min, raw.arity.max), (0, None));
    assert_eq!(raw.usage, "obj.raw(args...)\nsecond line");
}

#[test]
fn a_native_of_one_side_says_so() {
    let reference = read();
    let server = native(&reference, "", "serverOnly");
    assert_eq!(server.sides.text(), "server");
    let client = native(&reference, "", "clientOnly");
    assert_eq!(client.sides.text(), "client");
    assert_eq!(client.usage, "");
}

#[test]
fn prefs_and_classes_carry_their_defaults_ranges_and_docs() {
    let reference = read();
    let pref = &reference.prefs[0];
    assert_eq!(pref.name, "$Pref::Pad::DeadZone");
    assert_eq!(pref.ty, "float");
    assert_eq!(pref.default, "0.15");
    assert_eq!(pref.range.as_deref(), Some("0 to 0.9"));
    assert_eq!(pref.doc, "How far the stick must move before it counts.");
    assert!(pref.sides.both());

    let rope = reference
        .classes
        .iter()
        .find(|c| c.name == "RopeData")
        .expect("the class");
    assert_eq!(rope.parent, "SimDataBlock");
    assert!(rope.declared);
    assert!(
        !reference
            .classes
            .iter()
            .any(|c| c.name == "SimObject" && c.declared)
    );
    let segments = &rope.fields[0];
    assert_eq!(segments.name, "segments");
    assert_eq!(segments.ty, "int");
    assert_eq!(segments.default, "8");
    assert_eq!(segments.range.as_deref(), Some("2 to 64"));
    assert_eq!(segments.doc, "How many pieces the rope is cut into.");
}

#[test]
fn a_messages_layout_text_gives_its_fields_even_when_a_type_holds_a_semicolon() {
    let fields = crate::collect::message_fields(
        "text:String:0:32:0;at:[f32;3]:0:0:1;ids:Vec<u8>:8:16:0;target:torque_engine::net_classes::GhostRef:0:0:0",
    )
    .expect("every entry reads");
    let shown: Vec<(&str, &str, u8, usize, bool)> = fields
        .iter()
        .map(|f| (f.name.as_str(), f.ty.as_str(), f.bits, f.max_len, f.finite))
        .collect();
    assert_eq!(
        shown,
        [
            ("text", "String", 0, 32, false),
            ("at", "[f32;3]", 0, 0, true),
            ("ids", "Vec<u8>", 8, 16, false),
            (
                "target",
                "torque_engine::net_classes::GhostRef",
                0,
                0,
                false
            ),
        ]
    );
    assert!(crate::collect::message_fields("").unwrap().is_empty());
    // An entry that does not read is named, not dropped.
    assert_eq!(
        crate::collect::message_fields("a:u8:0:0:0;broken;b:u8:0:0:0"),
        Err("broken".to_string())
    );
}
