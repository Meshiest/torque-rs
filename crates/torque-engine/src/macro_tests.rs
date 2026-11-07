//! Natives declared with `#[function]` and `#[methods]`: what the attributes make registers as the hand-written
//! registry calls do, methods reach their object's data, and the declarations' mistakes fail the build.

use torque_ts_vm::vm::Vm;

use crate::script::{NativeDecl, Natives, TypedNative};
use crate::{
    BuildError, Engine, EngineBuilder, IntoNative, Plugin, PluginId, PluginType, ResMut,
    ServerStage, This, VmMut, function, methods, natives,
};

/// A test plugin that runs its hook in `build`.
struct Hooked(&'static str, Box<dyn Fn(&mut EngineBuilder)>);

impl Plugin for Hooked {
    fn build(&self, engine: &mut EngineBuilder) {
        (self.1)(engine);
    }
    fn id(&self) -> Option<&str> {
        Some(self.0)
    }
}

impl PluginType for Hooked {
    const ID: PluginId = PluginId::new("hooked");
}

fn engine(f: impl Fn(&mut EngineBuilder) + 'static) -> Engine {
    EngineBuilder::new()
        .add_plugins(Hooked("one", Box::new(f)))
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

fn eval(vm: &mut Vm, text: &str) -> String {
    String::from_utf8(vm.eval(text.as_bytes()).expect("the script runs")).expect("UTF-8")
}

fn listing_line(vm: &Vm, label: &str) -> String {
    let start = format!("{label} ");
    vm.native_listing()
        .lines()
        .find(|line| line.starts_with(&start))
        .unwrap_or_else(|| panic!("no native {label}"))
        .to_string()
}

fn logged(vm: &Vm, part: &str) -> bool {
    vm.log_texts().iter().any(|line| line.contains(part))
}

/// the words from first to last
#[function(name = "getWords")]
fn get_words(text: &[u8], first: i32, last: Option<i32>) -> String {
    let words = torque_ts_vm::vm::textfns::get_words(text, first, last.unwrap_or(1_000_000));
    String::from_utf8_lossy(&words).into_owned()
}

/// The runtime's `getWords` and the attribute's have one `native_listing` line, byte for byte, and answer alike.
#[test]
fn a_declared_native_matches_its_hand_registered_twin() {
    let hand = Vm::new();
    let engine = engine(|e| {
        e.script().natives(natives![get_words]);
    });
    let mut declared = server(&engine);
    assert_eq!(
        listing_line(&declared, "getWords"),
        listing_line(&hand, "getWords")
    );
    let mut hand = Vm::new();
    for script in [
        r#"return getWords("a b c d", 1);"#,
        r#"return getWords("a b c d", 1, 2);"#,
        r#"return getWords("a b c", "x", "1");"#,
    ] {
        assert_eq!(
            eval(&mut declared, script),
            eval(&mut hand, script),
            "{script}"
        );
    }
}

struct Zone(f32);

/// sets the zone, returns the zone kept
///
/// The rest of the comment is kept for the reference.
#[function(ns = "Pad")]
fn set_dead_zone(mut kept: ResMut<Zone>, dead_zone: f32) -> f32 {
    kept.0 = dead_zone.clamp(0.0, 0.75);
    kept.0
}

/// The same native through `function` and through the attribute: the same line, the declaration keeping the docs.
#[test]
fn the_attribute_gives_the_registrys_names_counts_and_usage() {
    let by_hand = engine(|e| {
        e.insert_resource(Zone(0.0));
        e.script().function(
            "Pad",
            "setDeadZone",
            &["deadZone"],
            "sets the zone, returns the zone kept",
            set_dead_zone,
        );
    });
    let declared = engine(|e| {
        e.insert_resource(Zone(0.0));
        e.script().natives(natives![set_dead_zone]);
    });
    let line = listing_line(&server(&by_hand), "Pad::setDeadZone");
    assert_eq!(
        line,
        "Pad::setDeadZone 2..2 Pad::setDeadZone(deadZone) - sets the zone, returns the zone kept"
    );
    let mut vm = server(&declared);
    assert_eq!(listing_line(&vm, "Pad::setDeadZone"), line);
    assert_eq!(eval(&mut vm, "return Pad::setDeadZone(5);"), "0.75");
    let decl = <set_dead_zone as Natives>::NATIVES[0];
    assert_eq!(decl.params, ["kept", "deadZone"]);
    assert!(
        decl.docs
            .ends_with("\n\nThe rest of the comment is kept for the reference."),
        "{}",
        decl.docs
    );
}

/// cheats
#[function(dev)]
fn cheat() -> i32 {
    1
}

#[function(usage = "kept(a); - as it was")]
fn kept(a: i32) -> i32 {
    a
}

#[test]
fn a_kept_usage_and_a_development_command() {
    let engine = engine(|e| {
        e.script().natives(natives![cheat, kept]);
    });
    let mut vm = server(&engine);
    assert_eq!(listing_line(&vm, "kept"), "kept 2..2 kept(a); - as it was");
    assert_eq!(listing_line(&vm, "cheat"), "cheat 1..1 cheat() - cheats");
    assert_eq!(
        eval(&mut vm, "return cheat();"),
        if cfg!(debug_assertions) { "1" } else { "" }
    );
}

/// A thing with rounds, kept as an object's Rust data.
struct Crate {
    rounds: i32,
}

#[methods(class = "SimObject")]
impl Crate {
    /// takes rounds out, returns the rounds left
    pub fn take_rounds(&mut self, amount: i32) -> i32 {
        self.rounds -= amount;
        self.rounds
    }

    /// runs `script` while the crate's data is out (its script reaching the crate again finds none)
    pub fn while_held(&mut self, mut vm: VmMut, script: &str) -> String {
        self.rounds += 100;
        let out = vm.eval(script.as_bytes()).unwrap_or_default();
        String::from_utf8_lossy(&out).into_owned()
    }

    /// the object
    #[torque(name = "self")]
    pub fn object(this: This) -> u32 {
        this.0
    }
}

fn crate_vm() -> (Vm, u32) {
    let engine = engine(|e| {
        e.script().natives(natives![Crate]);
    });
    let mut vm = server(&engine);
    eval(&mut vm, "new SimObject(Box);");
    let id = vm.objects().find(b"Box").expect("the object");
    vm.objects_mut().get_mut(id).expect("the object").ext = Some(Box::new(Crate { rounds: 10 }));
    (vm, id)
}

#[test]
fn a_method_with_self_is_called_on_its_objects_data() {
    let (mut vm, id) = crate_vm();
    assert_eq!(
        listing_line(&vm, "SimObject::takeRounds"),
        "SimObject::takeRounds 3..3 simObject.takeRounds(amount) - takes rounds out, returns the rounds left"
    );
    assert_eq!(eval(&mut vm, "return Box.takeRounds(3);"), "7");
    assert_eq!(eval(&mut vm, "return Box.takeRounds(3);"), "4");
    assert_eq!(eval(&mut vm, "return Box.self();"), id.to_string());
}

#[test]
fn the_data_is_out_for_the_call_and_back_after() {
    let (mut vm, _) = crate_vm();
    // Inside the call, the crate's own method finds no data: an error, not a panic, and "".
    let inner = eval(
        &mut vm,
        r#"return Box.whileHeld("return Box.takeRounds(1);");"#,
    );
    assert_eq!(inner, "");
    assert!(logged(&vm, "has no `"), "{:?}", vm.log_texts());
    // After it, the data is back, changed by the outer call.
    assert_eq!(eval(&mut vm, "return Box.takeRounds(0);"), "110");
}

#[test]
fn an_object_without_the_data_is_an_error_naming_the_type() {
    let (mut vm, _) = crate_vm();
    eval(&mut vm, "new SimObject(Empty);");
    assert_eq!(eval(&mut vm, "return Empty.takeRounds(1);"), "");
    assert!(
        logged(&vm, "SimObject::takeRounds: object")
            && logged(&vm, "has no `torque_engine::macro_tests::Crate` data"),
        "{:?}",
        vm.log_texts()
    );
}

#[test]
fn a_declaration_registers_by_hand() {
    let engine = EngineBuilder::new().build().expect("the engine builds");
    let mut vm = Vm::new();
    for decl in natives![get_words, Crate] {
        decl.register(&mut vm, &engine).expect("it registers");
    }
    assert!(
        vm.native_listing()
            .contains("SimObject::takeRounds 3..3 simObject.takeRounds(amount)")
    );
}

#[test]
fn one_name_in_two_plugins_lists_fails_the_build() {
    let err = EngineBuilder::new()
        .add_plugins((
            Hooked(
                "one",
                Box::new(|e| {
                    e.script().natives(natives![get_words]);
                }),
            ),
            Hooked(
                "two",
                Box::new(|e| {
                    e.script().function("", "GETWORDS", &[], "", || 1);
                }),
            ),
        ))
        .build()
        .map(drop)
        .unwrap_err();
    assert!(
        matches!(
            err,
            BuildError::DuplicateScriptName {
                kind: "function",
                ..
            }
        ),
        "{err}"
    );
}

fn two() -> TypedNative {
    (|a: i32, b: i32| a + b).into_native()
}

#[test]
fn a_declaration_whose_names_do_not_match_fails_the_build() {
    let decl = NativeDecl {
        namespace: "",
        name: "add",
        method: false,
        dev: false,
        params: &["a"],
        description: "",
        usage: None,
        docs: "",
        native: two,
    };
    let err = EngineBuilder::new()
        .add_plugins(Hooked(
            "one",
            Box::new(move |e| {
                e.script().natives([decl]);
            }),
        ))
        .build()
        .map(drop)
        .unwrap_err();
    assert!(
        matches!(&err, BuildError::ScriptSignature { message, .. } if message.contains("names 1 parameter(s), but its function takes 2")),
        "{err}"
    );
}
