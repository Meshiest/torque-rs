//! The script registry and the injected parameters: every parameter kind, the argument counts and usage text a
//! signature makes, the run-time borrow errors, state kept between runs, and name collisions.

use std::cell::Cell;
use std::rc::Rc;

use torque_ts_vm::convert::{ArgKind, ScriptError};
use torque_ts_vm::vm::{Native, Vm};

use crate::script::{CLIENT_SCRIPT_STAGE, Callee, SERVER_SCRIPT_STAGE, dev_allowed_in};
use crate::{
    BuildError, ClientStage, Engine, EngineBuilder, IntoNative, Local, Plugin, PluginId,
    PluginType, Res, ResMut, Rest, ServerStage, Slot, This, VmMut, VmRef,
};

type Hook = Box<dyn Fn(&mut EngineBuilder)>;

macro_rules! test_plugins {
    ($($ty:ident = $id:literal),*) => {
        $(
            /// A test plugin that runs its hook in `build`.
            struct $ty(Hook);
            impl Plugin for $ty {
                fn build(&self, engine: &mut EngineBuilder) {
                    (self.0)(engine);
                }
            }
            impl PluginType for $ty {
                const ID: PluginId = PluginId::new($id);
            }
        )*
    };
}

test_plugins!(One = "one", Two = "two");

fn one(f: impl Fn(&mut EngineBuilder) + 'static) -> One {
    One(Box::new(f))
}

/// The engine `f` records into, built.
fn engine(f: impl Fn(&mut EngineBuilder) + 'static) -> Engine {
    EngineBuilder::new()
        .add_plugins(one(f))
        .build()
        .expect("the engine builds")
}

/// A server's script engine made from `engine`.
fn server(engine: &Engine) -> Vm {
    let mut vm = Vm::new();
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut vm).expect("the replay runs");
    }
    vm
}

/// A client's script engine made from `engine`.
fn client(engine: &Engine) -> Vm {
    let mut vm = Vm::new();
    for &stage in ClientStage::ALL {
        engine.replay(stage, &mut vm).expect("the replay runs");
    }
    vm
}

fn eval(vm: &mut Vm, text: &str) -> String {
    String::from_utf8(vm.eval(text.as_bytes()).expect("the script runs")).expect("UTF-8")
}

/// The `native_listing` line of the native `label` (`name` or `Namespace::name`).
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

struct Counter(u32);

trait Greeter {
    fn greet(&self, name: &str) -> String;
}

struct Hello;

impl Greeter for Hello {
    fn greet(&self, name: &str) -> String {
        format!("hello, {name}")
    }
}

#[test]
fn script_arguments_are_read_by_type_and_counted() {
    fn add(a: i32, b: f32, c: bool, d: &str, e: String) -> String {
        format!("{a} {b} {c} {d} {e}")
    }
    let engine = engine(|e| {
        e.script()
            .function("", "add", &["a", "b", "c", "d", "e"], "", add);
    });
    let mut vm = server(&engine);
    assert_eq!(
        eval(
            &mut vm,
            r#"return add("7x", "1.5", "true", "hi", "there");"#
        ),
        "7 1.5 true hi there"
    );
    assert_eq!(listing_line(&vm, "add"), "add 6..6 add(a, b, c, d, e)");
}

#[test]
fn optional_and_rest_arguments_set_the_counts_and_the_usage() {
    fn spread(first: i32, second: Option<i32>, rest: Rest) -> String {
        let rest: Vec<i32> = rest.parse().expect("numbers");
        format!("{first} {second:?} {rest:?}")
    }
    let engine = engine(|e| {
        e.script()
            .function(
                "Ecs",
                "spread",
                &["first", "second", "parts"],
                "spreads",
                spread,
            )
            .function(
                "",
                "maybe",
                &["a", "b"],
                "",
                |a: Option<i32>, b: Option<i32>| format!("{a:?} {b:?}"),
            );
    });
    let mut vm = server(&engine);
    assert_eq!(eval(&mut vm, "return Ecs::spread(1);"), "1 None []");
    assert_eq!(eval(&mut vm, "return Ecs::spread(1, 2);"), "1 Some(2) []");
    assert_eq!(
        eval(&mut vm, "return Ecs::spread(1, 2, 3, 4);"),
        "1 Some(2) [3, 4]"
    );
    assert_eq!(eval(&mut vm, "return maybe();"), "None None");
    assert_eq!(eval(&mut vm, "return maybe(5);"), "Some(5) None");
    assert_eq!(
        listing_line(&vm, "Ecs::spread"),
        "Ecs::spread 2..* Ecs::spread(first [, second [, parts...]]) - spreads"
    );
    assert_eq!(listing_line(&vm, "maybe"), "maybe 1..3 maybe([a [, b]])");
}

#[test]
fn the_counts_come_from_the_script_arguments_only() {
    fn native<M>(f: impl IntoNative<M>) -> crate::TypedNative {
        f.into_native()
    }
    let function = Callee::Function {
        namespace: "",
        name: "f",
    };
    let method = Callee::Method {
        class: "Player",
        name: "f",
    };
    // Injected parameters are invisible to scripts.
    let f = native(|_: VmMut, a: i32, _: Res<Counter>, b: Option<f32>| a as f32 + b.unwrap_or(0.0));
    assert_eq!(f.args(), [ArgKind::Required, ArgKind::Optional]);
    assert_eq!(f.counts(function), (2, 3));
    assert_eq!(f.counts(method), (3, 4));
    let rest = native(|_: &Engine, _: Rest| ());
    assert_eq!(rest.counts(function), (1, 0));
    let none = native(|| 1);
    assert_eq!(none.counts(function), (1, 1));
    assert_eq!(none.counts(method), (2, 2));
}

#[test]
fn a_wrong_count_prints_the_generated_usage() {
    let engine = engine(|e| {
        e.script().function(
            "Pad",
            "setDeadZone",
            &["zone"],
            "sets the dead zone",
            |z: f32| z,
        );
    });
    let mut vm = server(&engine);
    eval(&mut vm, "Pad::setDeadZone();");
    assert!(
        logged(&vm, "usage: Pad::setDeadZone(zone) - sets the dead zone"),
        "{:?}",
        vm.log_texts()
    );
}

#[test]
fn resources_are_read_and_changed_and_shared_by_every_script_engine() {
    fn bump(mut counter: ResMut<Counter>, by: i32) -> u32 {
        counter.0 += by as u32;
        counter.0
    }
    let engine = engine(|e| {
        e.insert_resource(Counter(10));
        e.script().function("", "bump", &["by"], "", bump).function(
            "",
            "peek",
            &[],
            "",
            |c: Res<Counter>| c.0,
        );
    });
    let mut first = server(&engine);
    let mut second = client(&engine);
    assert_eq!(eval(&mut first, "return bump(2);"), "12");
    assert_eq!(eval(&mut second, "return bump(3);"), "15");
    assert_eq!(eval(&mut first, "return peek();"), "15");
    assert_eq!(engine.resource::<Counter>().map(|c| c.0), Some(15));
}

#[test]
fn a_reentrant_double_borrow_is_an_error_naming_the_resource() {
    fn outer(mut vm: VmMut, mut counter: ResMut<Counter>) -> String {
        counter.0 += 1;
        // Script that calls a native wanting the same resource, while this one holds it.
        let inner = vm.eval(b"return inner();").expect("the script runs");
        format!("{} [{}]", counter.0, String::from_utf8_lossy(&inner))
    }
    let engine = engine(|e| {
        e.insert_resource(Counter(0));
        e.script()
            .function("", "outer", &[], "", outer)
            .function("", "inner", &[], "", |mut c: ResMut<Counter>| {
                c.0 += 100;
                c.0
            })
            .function("", "reader", &[], "", |c: Res<Counter>| c.0)
            .function(
                "",
                "outerRead",
                &[],
                "",
                |mut vm: VmMut, _c: Res<Counter>| {
                    // Two readers at once are fine.
                    vm.eval(b"return reader();").expect("the script runs") == b"1"
                },
            );
    });
    let mut vm = server(&engine);
    // The inner call fails without a panic and returns the empty string; the outer one goes on.
    assert_eq!(eval(&mut vm, "return outer();"), "1 []");
    let named = format!(
        "inner: the resource `{}` is already borrowed",
        std::any::type_name::<Counter>()
    );
    assert!(logged(&vm, &named), "{:?}", vm.log_texts());
    assert_eq!(engine.resource::<Counter>().map(|c| c.0), Some(1));
    assert_eq!(eval(&mut vm, "return outerRead();"), "1");
}

#[test]
fn a_missing_resource_or_slot_fails_the_call_or_the_replay_with_its_name() {
    let engine = engine(|e| {
        e.script()
            .function("", "peek", &[], "", |c: Res<Counter>| c.0)
            .function("", "greet", &[], "", |g: Slot<dyn Greeter>| g.greet("x"));
    });
    let mut vm = server(&engine);
    assert_eq!(eval(&mut vm, "return peek();"), "");
    assert!(logged(&vm, "peek: the resource `"), "{:?}", vm.log_texts());
    assert_eq!(eval(&mut vm, "return greet();"), "");
    assert!(logged(&vm, "greet: no plugin set the slot `"));

    let mut builder = EngineBuilder::new();
    builder.add_plugins(one(|e| {
        e.recipe(ServerStage::Game, |_: Res<Counter>| {});
    }));
    let engine = builder.build().expect("the engine builds");
    let error = engine
        .replay(ServerStage::Game, &mut Vm::new())
        .expect_err("the resource is missing");
    assert_eq!(error.plugin, "one");
    assert!(error.message.contains("Counter"), "{}", error.message);
}

#[test]
fn slots_the_engine_and_the_called_object_are_injected() {
    fn greet(greeter: Slot<dyn Greeter>, engine: &Engine, name: &str) -> String {
        format!(
            "{} ({} plugin)",
            greeter.greet(name),
            engine.plugins().len()
        )
    }
    let engine = engine(|e| {
        e.set_slot::<dyn Greeter>(Rc::new(Hello));
        e.script()
            .function("", "greet", &["name"], "", greet)
            .function("", "keep", &[], "", |kept: Engine| {
                kept.plugins().len() as i32
            })
            .method(
                "ScriptObject",
                "whoAmI",
                &["extra"],
                "says its id",
                |this: This, extra: i32| this.0 as i32 + extra,
            );
    });
    let mut vm = server(&engine);
    assert_eq!(
        eval(&mut vm, "return greet(\"you\");"),
        "hello, you (1 plugin)"
    );
    assert_eq!(eval(&mut vm, "return keep();"), "1");
    assert_eq!(
        eval(
            &mut vm,
            "%o = new ScriptObject(); return %o.whoAmI(1) - %o.getId();"
        ),
        "1"
    );
    assert_eq!(
        listing_line(&vm, "ScriptObject::whoAmI"),
        "ScriptObject::whoAmI 3..3 scriptObject.whoAmI(extra) - says its id"
    );
}

#[test]
fn a_natives_local_state_lasts_for_its_script_engine() {
    fn tick(mut calls: Local<u32>) -> u32 {
        *calls += 1;
        *calls
    }
    let engine = engine(|e| {
        e.script()
            .function("", "tick", &[], "", tick)
            .function("", "other", &[], "", tick);
    });
    let mut vm = server(&engine);
    assert_eq!(eval(&mut vm, "tick(); tick(); return tick();"), "3");
    // Each native keeps its own; a new script engine starts fresh.
    assert_eq!(eval(&mut vm, "return other();"), "1");
    assert_eq!(eval(&mut server(&engine), "return tick();"), "1");
}

#[test]
fn a_natives_local_state_in_use_is_an_error_when_it_calls_itself() {
    fn again(mut vm: VmMut, mut depth: Local<u32>) -> String {
        *depth += 1;
        let inner = vm.eval(b"return again();").expect("the script runs");
        format!("{} [{}]", *depth, String::from_utf8_lossy(&inner))
    }
    let engine = engine(|e| {
        e.script().function("", "again", &[], "", again);
    });
    let mut vm = server(&engine);
    assert_eq!(eval(&mut vm, "return again();"), "1 []");
    assert!(logged(&vm, "again: the state `Local<u32>` is in use"));
}

#[test]
fn a_recipes_local_state_lasts_across_replays() {
    fn count(mut vm: VmMut, mut starts: Local<u32>, counter: Res<Counter>) -> Result<(), String> {
        *starts += 1;
        vm.set_global("$Starts", &format!("{} {}", *starts, counter.0));
        Ok(())
    }
    let engine = engine(|e| {
        e.insert_resource(Counter(7));
        e.recipe(ServerStage::Game, count);
    });
    for expected in ["1 7", "2 7", "3 7"] {
        let vm = server(&engine);
        assert_eq!(vm.global("$Starts").as_deref(), Some(expected.as_bytes()));
    }
}

#[test]
fn recipes_take_the_closure_form_and_report_their_errors() {
    fn plain(vm: &mut Vm, _: &Engine) -> Result<(), String> {
        vm.set_global("$Plain", "1");
        Ok(())
    }
    let engine = engine(|e| {
        e.recipe(ServerStage::Game, plain);
        e.recipe(ClientStage::Gui, |mut vm: VmMut| vm.set_global("$Gui", "1"));
        e.recipe(ServerStage::World, |_: VmMut| -> Result<(), ScriptError> {
            Err(ScriptError::new("no world"))
        });
    });
    let mut vm = Vm::new();
    engine.replay(ServerStage::Game, &mut vm).expect("runs");
    assert_eq!(vm.global("$Plain").as_deref(), Some(&b"1"[..]));
    let error = engine
        .replay(ServerStage::World, &mut vm)
        .expect_err("fails");
    assert_eq!(error.message, "no world");
    assert_eq!(client(&engine).global("$Gui").as_deref(), Some(&b"1"[..]));
}

#[test]
fn a_script_error_prints_with_the_usage_and_returns_empty() {
    fn half(n: i32) -> Result<i32, ScriptError> {
        if n % 2 != 0 {
            return Err(ScriptError::new(format!("{n} is odd")));
        }
        Ok(n / 2)
    }
    let engine = engine(|e| {
        e.script()
            .function("", "half", &["n"], "half of an even number", half);
    });
    let mut vm = server(&engine);
    assert_eq!(eval(&mut vm, "return half(8);"), "4");
    assert_eq!(eval(&mut vm, "return half(3);"), "");
    assert!(logged(&vm, "half: 3 is odd"));
    assert!(logged(&vm, "half: usage: half(n) - half of an even number"));
}

#[test]
fn a_text_argument_that_is_not_utf8_is_an_error_naming_it() {
    let engine = engine(|e| {
        e.script()
            .function("", "text", &["a", "b"], "", |_: i32, b: &str| {
                b.len() as i32
            });
    });
    let mut vm = server(&engine);
    vm.set_global_bytes("$Bad", b"\xff");
    assert_eq!(eval(&mut vm, "return text(1, $Bad);"), "");
    assert!(logged(
        &vm,
        "text: argument 2: the argument is not valid UTF-8 text"
    ));
}

#[test]
fn entries_go_to_their_side_after_the_stage_recipes() {
    let engine = engine(|e| {
        e.server(SERVER_SCRIPT_STAGE, |vm, _| {
            vm.register("which", 1, 1, "", |_, _| Native::Str(b"recipe".to_vec()));
            vm.set_global("$Seen", "recipe");
            Ok(())
        });
        e.script()
            .function("", "which", &[], "", || "registry")
            .global("$Both", "1");
        e.server_script().function("", "onServer", &[], "", || 1);
        e.client_script().function("", "onClient", &[], "", || 1);
    });
    let mut vm = server(&engine);
    assert_eq!(eval(&mut vm, "return which();"), "registry");
    assert_eq!(vm.global("$Both").as_deref(), Some(&b"1"[..]));
    assert_eq!(
        eval(
            &mut vm,
            "return isFunction(onServer) @ isFunction(onClient);"
        ),
        "10"
    );
    let mut c = client(&engine);
    assert_eq!(
        eval(
            &mut c,
            "return isFunction(onServer) @ isFunction(onClient);"
        ),
        "01"
    );
    assert_eq!(c.global("$Both").as_deref(), Some(&b"1"[..]));
    // Only those two stages register anything.
    let mut early = Vm::new();
    engine.replay(ServerStage::Game, &mut early).expect("runs");
    engine
        .replay(CLIENT_SCRIPT_STAGE, &mut early)
        .expect("runs");
    assert_eq!(early.global("$Both").as_deref(), Some(&b"1"[..]));
    assert_eq!(eval(&mut early, "return isFunction(onServer);"), "0");
}

#[test]
fn a_shared_replay_registers_only_the_clients_own_entries_over_the_servers() {
    let engine = engine(|e| {
        e.script().global("$Both", "default");
        e.client_script()
            .function("", "onClient", &[], "", || 1)
            .global("$ClientOnly", "1");
    });
    let mut vm = server(&engine);
    vm.set_global("$Both", "the server's");
    for &stage in ClientStage::ALL {
        engine.replay_shared(stage, &mut vm).expect("runs");
    }
    assert_eq!(vm.global("$Both").as_deref(), Some(&b"the server's"[..]));
    assert_eq!(vm.global("$ClientOnly").as_deref(), Some(&b"1"[..]));
    assert_eq!(eval(&mut vm, "return isFunction(onClient);"), "1");
    // A replay of its own puts the default back, as on a client's own script engine.
    engine.replay(CLIENT_SCRIPT_STAGE, &mut vm).expect("runs");
    assert_eq!(vm.global("$Both").as_deref(), Some(&b"default"[..]));
}

#[test]
fn a_name_declared_twice_fails_the_build_naming_both_plugins() {
    let build = |a: Hook, b: Hook| {
        EngineBuilder::new()
            .add_plugins((One(a), Two(b)))
            .build()
            .map(drop)
    };
    // Names compare without case, as scripts call them.
    let err = build(
        Box::new(|e| {
            e.script().function("Pad", "setZone", &[], "", || 1);
        }),
        Box::new(|e| {
            e.server_script().function("pad", "SETZONE", &[], "", || 2);
        }),
    )
    .unwrap_err();
    assert_eq!(
        err,
        BuildError::DuplicateScriptName {
            kind: "function",
            name: "pad::SETZONE".into(),
            first: PluginId::new("one"),
            second: PluginId::new("two"),
        }
    );
    assert!(err.to_string().contains("`one` and `two`"), "{err}");
    // A method takes the same name a function in that namespace would.
    let err = build(
        Box::new(|e| {
            e.script().function("Player", "jump", &[], "", || 1);
        }),
        Box::new(|e| {
            e.script().method("Player", "jump", &[], "", || 1);
        }),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        BuildError::DuplicateScriptName { kind: "method", .. }
    ));
    let err = build(
        Box::new(|e| {
            e.script().global("$Pref::Pad::Zone", "1");
        }),
        Box::new(|e| {
            e.client_script().global("$pref::pad::zone", "2");
        }),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        BuildError::DuplicateScriptName { kind: "global", .. }
    ));
    // One server's name and one client's do not meet.
    build(
        Box::new(|e| {
            e.server_script().function("", "hit", &[], "", || 1);
        }),
        Box::new(|e| {
            e.client_script().function("", "hit", &[], "", || 2);
        }),
    )
    .expect("the sides are apart");
}

#[test]
fn argument_names_that_do_not_match_fail_the_build() {
    let err = EngineBuilder::new()
        .add_plugins(one(|e| {
            e.script()
                .function("", "f", &["a"], "", |a: i32, b: i32| a + b);
        }))
        .build()
        .unwrap_err();
    assert_eq!(
        err,
        BuildError::ScriptSignature {
            plugin: PluginId::new("one"),
            message: "`f` names 1 script argument(s), but its function takes 2".into(),
        }
    );
}

#[test]
fn a_development_command_runs_in_a_debug_build_or_with_the_pref() {
    let engine = engine(|e| {
        e.script().dev("", "cheat", &[], "", || 1);
    });
    let mut vm = server(&engine);
    assert_eq!(
        eval(&mut vm, "return cheat();"),
        if cfg!(debug_assertions) { "1" } else { "" }
    );
    assert!(dev_allowed_in(&vm, true));
    assert!(!dev_allowed_in(&vm, false));
    vm.set_global(crate::script::DEV_PREF, "1");
    assert!(dev_allowed_in(&vm, false));
}

/// The proof: `getWords` registered through the registry has the `native_listing` line the runtime's own, hand-registered one
/// has, byte for byte, and answers as it does.
#[test]
fn a_typed_native_matches_its_hand_registered_twin() {
    fn get_words(text: &[u8], first: i32, last: Option<i32>) -> String {
        let words = torque_ts_vm::vm::textfns::get_words(text, first, last.unwrap_or(1_000_000));
        String::from_utf8_lossy(&words).into_owned()
    }
    let hand = Vm::new();
    let hand_line = listing_line(&hand, "getWords");
    assert_eq!(
        hand_line,
        "getWords 3..4 getWords(text, first [, last]) - the words from first to last"
    );
    let engine = engine(|e| {
        e.script().function(
            "",
            "getWords",
            &["text", "first", "last"],
            "the words from first to last",
            get_words,
        );
    });
    let mut typed = server(&engine);
    assert_eq!(listing_line(&typed, "getWords"), hand_line);
    let mut hand = Vm::new();
    for script in [
        r#"return getWords("a b c d", 1);"#,
        r#"return getWords("a b c d", 1, 2);"#,
        r#"return getWords("a b ", 1);"#,
        r#"return getWords("a b c", "x", "1");"#,
    ] {
        assert_eq!(
            eval(&mut typed, script),
            eval(&mut hand, script),
            "{script}"
        );
    }
}

#[test]
fn a_typed_native_registers_by_hand_in_a_recipe() {
    let calls = Rc::new(Cell::new(0));
    let seen = Rc::clone(&calls);
    let engine = engine(move |e| {
        let seen = Rc::clone(&seen);
        e.server(ServerStage::Game, move |vm, engine| {
            let seen = Rc::clone(&seen);
            (move |n: i32| {
                seen.set(seen.get() + 1);
                n * 2
            })
            .into_native()
            .register(
                vm,
                engine,
                Callee::Function {
                    namespace: "",
                    name: "twice",
                },
                &["n"],
                "doubles",
            )
        });
    });
    let mut vm = server(&engine);
    assert_eq!(eval(&mut vm, "return twice(4);"), "8");
    assert_eq!(calls.get(), 1);
    assert_eq!(listing_line(&vm, "twice"), "twice 2..2 twice(n) - doubles");
}

#[test]
fn twelve_parameters_are_the_most() {
    #[allow(clippy::too_many_arguments)]
    fn twelve(
        mut vm: VmMut,
        a: i32,
        b: i32,
        c: i32,
        d: i32,
        e: i32,
        f: i32,
        g: i32,
        h: i32,
        i: i32,
        j: i32,
        rest: Rest,
    ) -> i32 {
        vm.set_global("$Twelve", "1");
        a + b + c + d + e + f + g + h + i + j + rest.len() as i32
    }
    let names = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "rest"];
    let engine = engine(move |e| {
        e.script().function("", "twelve", &names, "", twelve);
    });
    let mut vm = server(&engine);
    assert_eq!(
        eval(
            &mut vm,
            "return twelve(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 0, 0);"
        ),
        "57"
    );
    assert_eq!(vm.global("$Twelve").as_deref(), Some(&b"1"[..]));
}

#[test]
fn any_number_of_readers_share_the_script_engine() {
    fn read_twice(first: VmRef, second: VmRef, this: This, name: &str) -> String {
        let value = first.global(name).unwrap_or_default();
        let has = second.objects().contains(this.0);
        format!("{} {has}", String::from_utf8_lossy(&value))
    }
    fn note(vm: VmRef, other: VmRef) -> Result<(), String> {
        match (vm.global("$Seen"), other.global("$Seen")) {
            (Some(a), Some(b)) if a == b => Ok(()),
            seen => Err(format!("the readers saw {seen:?}")),
        }
    }
    let engine = engine(|e| {
        e.server(ServerStage::Game, |vm, _| {
            vm.set_global("$Seen", "yes");
            Ok(())
        });
        e.recipe(ServerStage::World, note);
        e.script()
            .method("ScriptObject", "readTwice", &["name"], "", read_twice);
    });
    let mut vm = server(&engine);
    assert_eq!(
        eval(
            &mut vm,
            "%o = new ScriptObject(); return %o.readTwice(\"$Seen\");"
        ),
        "yes true"
    );
}

#[test]
fn a_reader_holds_the_script_engine_from_a_writer() {
    let engine = engine(|_| {});
    let mut vm = Vm::new();
    let cx = crate::Cx::recipe(&mut vm, &engine);
    let first = cx.share_vm().expect("a reader gets it");
    let second = cx.share_vm().expect("so does another");
    assert!(std::ptr::eq(first, second));
    assert!(cx.take_vm().is_none(), "a writer does not, after a reader");
}
