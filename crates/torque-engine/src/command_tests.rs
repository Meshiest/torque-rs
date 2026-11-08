//! Server commands and tick hooks as an engine records them: the specs' checks, the token bucket, the declarations'
//! errors, and the hooks' parameters.

use torque_ts_vm::vm::Vm;

use crate::command::{
    ArgType, CommandRefusal, CommandSpec, ServerCommand, TokenBucket, command_function,
};
use crate::{
    BuildError, Engine, EngineBuilder, Local, Plugin, PluginId, PluginType, ResMut, ServerStage,
    ServerTick, TickInfo, TickPhase, VmMut,
};

type Hook = Box<dyn Fn(&mut EngineBuilder)>;

macro_rules! test_plugins {
    ($($ty:ident = $id:literal),*) => {
        $(
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

fn args(list: &[&str]) -> Vec<Vec<u8>> {
    list.iter().map(|a| a.as_bytes().to_vec()).collect()
}

fn server(engine: &Engine) -> Vm {
    let mut vm = Vm::new();
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut vm).expect("the stage replays");
    }
    vm
}

fn ping(_client: i32, _n: i32) {}

#[test]
fn a_bucket_holds_its_burst_and_refills_by_the_tick() {
    // 10 a second, a burst of 5: a thousand calls in one tick get five.
    let mut bucket = TokenBucket::new(10, 5, 0);
    assert_eq!((0..1000).filter(|_| bucket.take(0)).count(), 5);
    // One tick (32 ms) gains 0.32 of a token: three ticks are not yet one, four are.
    assert!(!bucket.take(3));
    assert!(bucket.take(4));
    assert!(!bucket.take(4));
    // 600 ticks (19.2 s) at 10 a second refill 192 tokens, held to the burst.
    assert_eq!((0..1000).filter(|_| bucket.take(604)).count(), 5);
    // Over a long run it lets through exactly `rate` a second: drained, a call every tick for 600 ticks gets 0.32 of a token
    // a tick, 192 in all.
    let mut bucket = TokenBucket::new(10, 5, 0);
    assert_eq!((0..5).filter(|_| bucket.take(0)).count(), 5);
    let ran = (1..=600).filter(|tick| bucket.take(*tick)).count();
    assert_eq!(ran, 192);
}

#[test]
fn a_tick_that_goes_backward_adds_nothing() {
    let mut bucket = TokenBucket::new(1000, 2, 100);
    assert!(bucket.take(100) && bucket.take(100));
    assert!(!bucket.take(50));
    assert!(!bucket.take(100));
}

#[test]
fn a_spec_checks_the_count_the_size_and_the_type_of_each_argument() {
    let spec = CommandSpec::new()
        .arg("count", ArgType::Int)
        .arg("scale", ArgType::Float)
        .optional("on", ArgType::Bool)
        .optional("label", ArgType::Text)
        .max_arg_bytes(16);
    assert_eq!(spec.check(&args(&["3", "0.5"])), Ok(()));
    assert_eq!(spec.check(&args(&["-3", "1e2", "TRUE", "hi"])), Ok(()));
    assert_eq!(
        spec.check(&args(&["3"])),
        Err(CommandRefusal::TooFewArguments {
            given: 1,
            needed: 2
        })
    );
    assert_eq!(
        spec.check(&args(&["3", "1", "0", "x", "extra"])),
        Err(CommandRefusal::TooManyArguments { given: 5, max: 4 })
    );
    assert_eq!(
        spec.check(&args(&["3", "1", "1", "12345678901234567"])),
        Err(CommandRefusal::ArgumentTooLong {
            number: 4,
            len: 17,
            max: 16
        })
    );
    for bad in ["", "-", "3.5", "x", "99999999999", "+3", " 3"] {
        assert_eq!(
            spec.check(&args(&[bad, "1"])),
            Err(CommandRefusal::BadArgument {
                number: 1,
                expected: ArgType::Int
            }),
            "{bad:?}"
        );
    }
    for bad in ["inf", "NaN", "", "1,5"] {
        assert!(spec.check(&args(&["1", bad])).is_err(), "{bad:?}");
    }
    assert!(spec.check(&args(&["1", "1", "yes"])).is_err());
    // A NUL ends an argument as it does for the script engine.
    assert_eq!(spec.check(&[b"3\0junk".to_vec(), b"1".to_vec()]), Ok(()));
}

#[test]
fn the_function_of_a_command_has_the_prefix_once() {
    assert_eq!(command_function("Ping"), "serverCmdPing");
    assert_eq!(command_function("serverCmdPing"), "serverCmdPing");
    assert_eq!(command_function("SERVERCMDPing"), "SERVERCMDPing");
    assert_eq!(command_function("serverCmd"), "serverCmdserverCmd");
}

#[test]
fn a_declared_command_is_a_native_and_a_spec_in_the_engines_list() {
    let mut builder = EngineBuilder::new();
    builder.add_plugins(One(Box::new(|engine| {
        let spec = CommandSpec::new().arg("n", ArgType::Int).rate(4, 8);
        engine.server_command("Ping", spec, |client: i32, n: i32| client + n);
    })));
    let engine = builder.build().expect("builds");
    let commands = engine.list::<ServerCommand>();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].function, "serverCmdPing");
    assert_eq!(commands[0].plugin, "one");
    assert_eq!((commands[0].spec.rate, commands[0].spec.burst), (4, 8));
    let mut vm = server(&engine);
    assert_eq!(vm.eval(b"return serverCmdPing(3, 4);").unwrap(), b"7");
    assert!(
        vm.native_listing()
            .contains("serverCmdPing 3..3 serverCmdPing(client, n)")
    );
}

#[test]
fn two_plugins_naming_one_command_fail_the_build_naming_both() {
    let mut builder = EngineBuilder::new();
    builder.add_plugins((
        One(Box::new(|engine| {
            engine.server_command("Ping", CommandSpec::new().arg("n", ArgType::Int), ping);
        })),
        Two(Box::new(|engine| {
            engine.server_command_limits("serverCmdPING", CommandSpec::new());
        })),
    ));
    let error = builder.build().unwrap_err();
    assert!(
        matches!(&error, BuildError::DuplicateCommand { first, second, .. }
            if *first == "one" && *second == "two"),
        "{error:?}"
    );
    let text = error.to_string();
    assert!(
        text.contains("one") && text.contains("two") && text.contains("PING"),
        "{text}"
    );
}

#[test]
fn a_handler_that_does_not_match_its_spec_or_a_bad_name_fails_the_build() {
    let one = |f: fn(&mut EngineBuilder)| {
        EngineBuilder::new()
            .add_plugins(One(Box::new(f)))
            .build()
            .unwrap_err()
    };
    // The handler takes no argument for the spec's.
    let error = one(|engine| {
        engine.server_command(
            "Ping",
            CommandSpec::new().arg("n", ArgType::Int),
            |_: i32| {},
        );
    });
    assert!(
        matches!(error, BuildError::ScriptSignature { .. }),
        "{error:?}"
    );
    // A required argument where the spec's is optional.
    let error = one(|engine| {
        let spec = CommandSpec::new().optional("n", ArgType::Int);
        engine.server_command("Ping", spec, ping);
    });
    assert!(
        matches!(error, BuildError::ScriptSignature { .. }),
        "{error:?}"
    );
    // A name that cannot be a function, and an optional argument before a required one.
    let error = one(|engine| {
        engine.server_command_limits("not a name", CommandSpec::new());
    });
    assert!(matches!(error, BuildError::Rejected { .. }), "{error:?}");
    let error = one(|engine| {
        let spec = CommandSpec::new()
            .optional("a", ArgType::Int)
            .arg("b", ArgType::Int);
        engine.server_command_limits("Ping", spec);
    });
    assert!(matches!(error, BuildError::Rejected { .. }), "{error:?}");
}

#[test]
fn a_function_another_plugin_registered_is_taken_too() {
    let mut builder = EngineBuilder::new();
    builder.add_plugins((
        One(Box::new(|engine| {
            engine
                .script()
                .function("", "serverCmdPing", &["client", "n"], "", ping);
        })),
        Two(Box::new(|engine| {
            engine.server_command("Ping", CommandSpec::new().arg("n", ArgType::Int), ping);
        })),
    ));
    let error = builder.build().unwrap_err();
    assert!(
        matches!(error, BuildError::DuplicateScriptName { .. }),
        "{error:?}"
    );
}

#[derive(Default)]
struct Log(Vec<String>);

#[test]
fn tick_hooks_take_injected_parameters_and_keep_the_order_pushed() {
    let mut builder = EngineBuilder::new();
    builder.insert_resource(Log::default());
    builder.add_plugins((
        One(Box::new(|engine| {
            engine.push(ServerTick::after(|mut log: ResMut<Log>, tick: TickInfo| {
                log.0.push(format!("one.after {}", tick.number));
            }));
            engine.push(ServerTick::before(
                |mut log: ResMut<Log>, mut seen: Local<u32>| {
                    *seen += 1;
                    log.0.push(format!("one.before #{}", *seen));
                },
            ));
        })),
        Two(Box::new(|engine| {
            engine.push(ServerTick::before(|mut vm: VmMut, tick: TickInfo| {
                vm.set_global("$Time", &tick.time_ms.to_string());
            }));
            engine.push(ServerTick::after(
                |_: &mut Vm, engine: &Engine, tick: TickInfo| -> Result<(), String> {
                    engine
                        .resource_mut::<Log>()
                        .expect("log")
                        .0
                        .push(format!("two.after {}", tick.number));
                    Ok(())
                },
            ));
        })),
    ));
    let engine = builder.build().expect("builds");
    let hooks = engine.list::<ServerTick>();
    let phases: Vec<TickPhase> = hooks.iter().map(ServerTick::phase).collect();
    assert_eq!(
        phases,
        [
            TickPhase::After,
            TickPhase::Before,
            TickPhase::Before,
            TickPhase::After
        ]
    );
    let mut vm = Vm::new();
    for n in 1..=2 {
        for hook in hooks {
            hook.run(&mut vm, &engine, TickInfo::new(n)).expect("runs");
        }
    }
    assert_eq!(
        engine.resource::<Log>().unwrap().0,
        [
            "one.after 1",
            "one.before #1",
            "two.after 1",
            "one.after 2",
            "one.before #2",
            "two.after 2"
        ]
    );
    assert_eq!(vm.global("$Time").as_deref(), Some(&b"64"[..]));
}

#[test]
fn a_hook_that_cannot_get_its_parameters_says_why() {
    struct Missing;
    let mut builder = EngineBuilder::new();
    builder.push(ServerTick::before(|_: ResMut<Missing>| {}));
    let engine = builder.build().expect("builds");
    let error = engine.list::<ServerTick>()[0]
        .run(&mut Vm::new(), &engine, TickInfo::new(1))
        .unwrap_err();
    assert!(error.contains("Missing"), "{error}");
    // A recipe that asks for the tick is told it is not in one.
    let mut builder = EngineBuilder::new();
    builder.recipe(ServerStage::Game, |_: TickInfo| {});
    let engine = builder.build().expect("builds");
    let error = engine
        .replay(ServerStage::Game, &mut Vm::new())
        .unwrap_err();
    assert!(error.message.contains("tick hook"), "{error}");
}

#[test]
fn a_tick_knows_its_time() {
    let tick = TickInfo::new(631);
    assert_eq!((tick.number, tick.time_ms), (631, 20_192));
    assert_eq!(TickInfo::new(1).time_ms, TickInfo::MS);
}
