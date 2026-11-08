//! Prefs: defaults set before scripts, clamped on read, exported, and read back typed.

use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

use crate::{
    BuildError, Engine, EngineBuilder, Plugin, PluginId, PluginType, Pref, Prefs, ServerStage,
    VmPrefs, function, natives,
};

/// Prefs of every type, with and without a range.
#[derive(Prefs, Debug, PartialEq)]
#[prefs(prefix = "$Pref::Pad")]
struct PadPrefs {
    /// How far the stick must move before it counts.
    #[pref(default = 0.15, range = 0.0..=0.9)]
    dead_zone: f32,
    #[pref(default = true)]
    invert_y: bool,
    #[pref(default = 2, range = 1..=4)]
    sensitivity: i32,
    #[pref(default = 10, range = 5..=20)]
    repeat: u32,
    #[pref(default = "default", name = "Profile")]
    active_profile: String,
    #[pref(default = -3)]
    offset: i32,
}

struct Plain;

impl Plugin for Plain {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.script().prefs::<PadPrefs>();
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

fn server(engine: &Engine) -> Vm {
    let mut vm = Vm::new();
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut vm).expect("the replay runs");
    }
    vm
}

fn global(vm: &Vm, name: &str) -> String {
    String::from_utf8_lossy(&vm.global(name).unwrap_or_default()).into_owned()
}

#[test]
fn the_declarations_name_the_globals_and_keep_the_defaults_as_text() {
    let names: Vec<_> = PadPrefs::PREFS
        .iter()
        .map(|p| (p.name, p.default))
        .collect();
    assert_eq!(
        names,
        [
            ("$Pref::Pad::DeadZone", "0.15"),
            ("$Pref::Pad::InvertY", "1"),
            ("$Pref::Pad::Sensitivity", "2"),
            ("$Pref::Pad::Repeat", "10"),
            ("$Pref::Pad::Profile", "default"),
            ("$Pref::Pad::Offset", "-3"),
        ]
    );
    let zone = &PadPrefs::PREFS[0];
    assert_eq!(zone.doc, "How far the stick must move before it counts.");
    assert_eq!(zone.range_text().as_deref(), Some("0 to 0.9"));
    assert_eq!(PadPrefs::PREFS[1].range_text(), None);
    assert_eq!(PadPrefs::PREFIX, "$Pref::Pad");
}

#[test]
fn defaults_are_set_before_any_script_runs() {
    // The script engine is made and replayed; nothing has evaluated a script yet.
    let vm = server(&engine());
    assert_eq!(global(&vm, "$Pref::Pad::DeadZone"), "0.15");
    assert_eq!(global(&vm, "$Pref::Pad::InvertY"), "1");
    assert_eq!(global(&vm, "$Pref::Pad::Profile"), "default");
    assert_eq!(global(&vm, "$Pref::Pad::Offset"), "-3");
    // A script can read them straight away, and a name matches without case.
    let mut vm = vm;
    assert_eq!(
        vm.eval(b"return $pref::pad::sensitivity + 1;")
            .expect("runs"),
        b"3"
    );
}

#[test]
fn a_script_run_after_the_defaults_overrides_them() {
    let mut vm = server(&engine());
    vm.eval(b"$Pref::Pad::DeadZone = 0.3; $Pref::Pad::Profile = \"pro\";")
        .expect("runs");
    let pad: PadPrefs = vm.prefs();
    assert_eq!(pad.dead_zone, 0.3);
    assert_eq!(pad.active_profile, "pro");
}

#[test]
fn the_defaults_read_back_as_the_struct() {
    let vm = server(&engine());
    let expected = PadPrefs {
        dead_zone: 0.15,
        invert_y: true,
        sensitivity: 2,
        repeat: 10,
        active_profile: "default".into(),
        offset: -3,
    };
    assert_eq!(vm.prefs::<PadPrefs>(), expected);
    assert_eq!(PadPrefs::defaults(), expected);
    // On a script engine that never had them set, a read gives the defaults too.
    assert_eq!(Vm::new().prefs::<PadPrefs>(), expected);
}

#[test]
fn a_read_clamps_to_the_range_and_reads_scripts_text_by_the_console_rules() {
    let mut vm = server(&engine());
    vm.eval(
        b"$Pref::Pad::DeadZone = 4; $Pref::Pad::Sensitivity = -7; $Pref::Pad::Repeat = 99;
          $Pref::Pad::InvertY = \"false\"; $Pref::Pad::Offset = \"12abc\";",
    )
    .expect("runs");
    let pad = vm.prefs::<PadPrefs>();
    assert_eq!(pad.dead_zone, 0.9, "above the range");
    assert_eq!(pad.sensitivity, 1, "below the range");
    assert_eq!(pad.repeat, 20);
    assert!(!pad.invert_y);
    assert_eq!(pad.offset, 12, "no range, and dAtoi's reading");
    vm.eval(b"$Pref::Pad::DeadZone = -1; $Pref::Pad::Repeat = 0;")
        .expect("runs");
    let pad = vm.prefs::<PadPrefs>();
    assert_eq!((pad.dead_zone, pad.repeat), (0.0, 5));
    // The global itself is what the script wrote.
    assert_eq!(global(&vm, "$Pref::Pad::DeadZone"), "-1");
}

#[test]
fn the_registered_prefs_are_listed_and_exported() {
    let engine = engine();
    let listed: Vec<_> = engine.prefs().map(|p| p.field).collect();
    assert_eq!(
        listed,
        [
            "dead_zone",
            "invert_y",
            "sensitivity",
            "repeat",
            "active_profile",
            "offset"
        ]
    );
    let mut vm = server(&engine);
    vm.eval(b"$Pref::Pad::DeadZone = 0.5; $Pref::Other = 1;")
        .expect("runs");
    engine.export_prefs(&mut vm, None).expect("exports");
    let lines: Vec<String> = vm
        .log_texts()
        .into_iter()
        .filter(|line| line.starts_with("$Pref::"))
        .collect();
    assert_eq!(
        lines,
        [
            "$Pref::Pad::DeadZone = 0.5;",
            "$Pref::Pad::InvertY = 1;",
            "$Pref::Pad::Offset = -3;",
            "$Pref::Pad::Profile = \"default\";",
            "$Pref::Pad::Repeat = 10;",
            "$Pref::Pad::Sensitivity = 2;",
        ],
        "the registered prefs only, sorted by name ignoring case, as export sorts"
    );
    // A game's own export of the family finds them too.
    let before = vm.log_texts().len();
    vm.eval(b"export(\"$Pref::Pad::*\");").expect("runs");
    assert_eq!(vm.log_texts().len() - before, 6);
}

/// The file `export_prefs` writes is the one `export` of the same family writes, byte for byte, and it replaces what
/// was there.
#[test]
fn the_exported_file_is_what_the_consoles_export_writes() {
    let engine = engine();
    let mut vm = server(&engine);
    let files = MemFiles::new();
    files.insert("old.cs", b"stale\r\n");
    vm.set_file_host(files.clone());
    vm.eval(
        b"$Pref::Pad::DeadZone = 0.5; $Pref::Pad::profile = \"Mixed Case\"; $Pref::Pad::Other = 1;",
    )
    .expect("runs");
    // `Other` is not a registered pref: the console's export of the family has it, the engine's does not.
    engine
        .export_prefs(&mut vm, Some(b"old.cs"))
        .expect("exports");
    vm.eval(b"deleteVariables(\"$Pref::Pad::Other\"); export(\"$Pref::Pad::*\", \"family.cs\");")
        .expect("runs");
    let ours = files.get("old.cs").expect("written");
    assert_eq!(ours, files.get("family.cs").expect("written"));
    assert!(!ours.starts_with(b"stale"), "the file is replaced");
    assert!(
        ours.ends_with(b"$Pref::Pad::Sensitivity = 2;\r\n"),
        "{ours:?}"
    );
}

#[test]
fn a_pref_name_taken_twice_fails_the_build() {
    struct Again;
    impl Plugin for Again {
        fn build(&self, engine: &mut EngineBuilder) {
            engine.script().prefs::<PadPrefs>();
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
        .expect_err("the second declaration of the same globals");
    assert!(
        matches!(error, BuildError::DuplicateScriptName { kind: "pref", .. }),
        "{error:?}"
    );
}

#[derive(Prefs)]
#[prefs(prefix = "$Pref::Hud")]
struct HudPrefs {
    #[pref(default = 1.0, range = 0.5..=2.0)]
    scale: f32,
}

/// the HUD scale as the player set it
#[function(ns = "Hud")]
fn hud_scale(prefs: Pref<HudPrefs>) -> f32 {
    prefs.scale
}

#[test]
fn a_prefs_parameter_reads_the_current_values_at_each_call() {
    let mut builder = EngineBuilder::new();
    builder
        .script()
        .prefs::<HudPrefs>()
        .natives(natives![hud_scale]);
    let engine = builder.build().expect("the engine builds");
    let mut vm = server(&engine);
    assert_eq!(vm.eval(b"return Hud::hudScale();").expect("runs"), b"1");
    vm.eval(b"$Pref::Hud::Scale = 0.1;").expect("runs");
    assert_eq!(vm.eval(b"return Hud::hudScale();").expect("runs"), b"0.5");
    // The usage text does not show the injected parameter.
    assert!(
        vm.native_listing()
            .contains("Hud::hudScale 1..1 Hud::hudScale() - the HUD scale as the player set it\n"),
        "{}",
        vm.native_listing()
    );
}
