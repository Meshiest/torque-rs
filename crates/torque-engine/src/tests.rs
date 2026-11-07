use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;

use torque_ts_vm::vm::{Native, Vm};

use crate::{
    BuildError, ClientStage, Engine, EngineBuilder, Plugin, PluginGroup, PluginGroupBuilder,
    PluginId, PluginType, RecipeError, Requires, ServerStage, Stage, Warning,
};

/// A test plugin's name: each one is a type, as plugins are.
trait Name: 'static {
    const ID: &'static str;
}

macro_rules! names {
    ($($ty:ident = $id:literal),* $(,)?) => {
        $(
            struct $ty;
            impl Name for $ty {
                const ID: &'static str = $id;
            }
        )*
    };
}

names!(
    A = "a",
    B = "b",
    C = "c",
    D = "d",
    E = "e",
    G = "g",
    G1 = "g1",
    G2 = "g2",
    H = "h",
    X = "x",
    Z = "z",
    Missing = "missing",
    AlsoA = "a",
    First = "first",
    Second = "second",
    Late = "late",
    R = "r",
    Mm = "mm",
    Zz = "zz",
    Bc = "b.c",
);

type Hook = Box<dyn Fn(&mut EngineBuilder)>;

/// A test plugin: by default its `build` pushes a [`Built`] entry and records one server `Game` recipe.
struct P<N: Name> {
    requires: Vec<fn(&mut Requires)>,
    net: Option<u32>,
    build: Hook,
    finish: Hook,
    name: PhantomData<N>,
}

/// Which plugins' `build` started, in order.
struct Built(PluginId);

impl<N: Name> Default for P<N> {
    fn default() -> P<N> {
        P {
            requires: Vec::new(),
            net: None,
            build: Box::new(|engine| {
                let me = engine.current_plugin();
                engine.push(Built(me));
                engine.server(ServerStage::Game, |_, _| Ok(()));
            }),
            finish: Box::new(|_| {}),
            name: PhantomData,
        }
    }
}

fn p<N: Name>() -> P<N> {
    P::default()
}

impl<N: Name> P<N> {
    fn requires<M: Name>(mut self) -> P<N> {
        self.requires.push(|r| {
            r.plugin::<P<M>>();
        });
        self
    }

    fn net(mut self, version: u32) -> P<N> {
        self.net = Some(version);
        self
    }

    fn on_build(mut self, f: impl Fn(&mut EngineBuilder) + 'static) -> P<N> {
        self.build = Box::new(f);
        self
    }

    fn on_finish(mut self, f: impl Fn(&mut EngineBuilder) + 'static) -> P<N> {
        self.finish = Box::new(f);
        self
    }
}

impl<N: Name> Plugin for P<N> {
    fn requires(&self, requires: &mut Requires) {
        for f in &self.requires {
            f(requires);
        }
    }

    fn net_version(&self) -> Option<u32> {
        self.net
    }

    fn build(&self, engine: &mut EngineBuilder) {
        (self.build)(engine);
    }

    fn finish(&self, engine: &mut EngineBuilder) {
        (self.finish)(engine);
    }
}

impl<N: Name> PluginType for P<N> {
    const ID: PluginId = PluginId::new(N::ID);
}

fn id(id: &'static str) -> PluginId {
    PluginId::new(id)
}

fn ids(list: &[&'static str]) -> Vec<PluginId> {
    list.iter().map(|s| PluginId::new(s)).collect()
}

/// The build order, the `Game` recipe order and the [`Built`] list, which must all agree.
fn order(engine: &Engine) -> Vec<PluginId> {
    let plugins = engine.plugins().to_vec();
    let recipes: Vec<PluginId> = engine.recipes(ServerStage::Game).collect();
    let built: Vec<PluginId> = engine.list::<Built>().iter().map(|b| b.0).collect();
    assert_eq!(recipes, plugins, "the recipes replay in build order");
    assert_eq!(built, plugins, "the plugins build in order");
    plugins
}

fn build_ok(builder: &mut EngineBuilder) -> Engine {
    builder.build().expect("the engine builds")
}

fn build_err(builder: &mut EngineBuilder) -> BuildError {
    builder.build().expect_err("the build fails")
}

/// A group of three, `a`, `b`, `c`.
struct Abc;

impl PluginGroup for Abc {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::new()
            .add(p::<A>())
            .add(p::<B>())
            .add(p::<C>())
    }
}

#[test]
fn add_order_is_build_and_replay_order() {
    let engine = build_ok(EngineBuilder::new().add_plugins((p::<C>(), p::<A>(), p::<B>())));
    assert_eq!(order(&engine), ids(&["c", "a", "b"]));
}

#[test]
fn groups_and_tuples_keep_their_order_and_nest() {
    let inner = PluginGroupBuilder::new().add(p::<D>());
    let engine = build_ok(EngineBuilder::new().add_plugins((
        (p::<X>(), Abc),
        PluginGroupBuilder::new().add_group(inner),
        p::<E>(),
    )));
    assert_eq!(order(&engine), ids(&["x", "a", "b", "c", "d", "e"]));
}

#[test]
fn add_before_pulls_the_plugin_up_to_its_target() {
    let game = PluginGroupBuilder::new()
        .add(p::<G1>())
        .add_before::<P<B>>(p::<G2>());
    let engine = build_ok(EngineBuilder::new().add_plugins((Abc, game)));
    assert_eq!(order(&engine), ids(&["a", "g2", "b", "c", "g1"]));
}

#[test]
fn add_after_holds_for_earlier_and_later_targets() {
    // The target is earlier: the plugin stays where it was added.
    let after_a = PluginGroupBuilder::new().add_after::<P<A>>(p::<G>());
    let engine = build_ok(EngineBuilder::new().add_plugins((Abc, after_a)));
    assert_eq!(order(&engine), ids(&["a", "b", "c", "g"]));

    // The target is added later: it is pulled up before the plugin.
    let after_c = PluginGroupBuilder::new().add_after::<P<C>>(p::<G>());
    let engine = build_ok(EngineBuilder::new().add_plugins((after_c, Abc)));
    assert_eq!(order(&engine), ids(&["c", "g", "a", "b"]));
}

#[test]
fn a_required_plugin_builds_first_and_keeps_its_settings() {
    let engine = build_ok(EngineBuilder::new().add_plugins((
        p::<A>().requires::<C>(),
        p::<B>(),
        p::<C>().net(5),
    )));
    assert_eq!(order(&engine), ids(&["c", "a", "b"]));
    // The one added, not a default.
    assert_eq!(engine.net_versions(), [(id("c"), 5)]);
}

#[test]
fn a_missing_requirement_is_added_once_as_its_default() {
    let engine = build_ok(
        EngineBuilder::new().add_plugins((p::<A>().requires::<X>(), p::<B>().requires::<X>())),
    );
    assert_eq!(order(&engine), ids(&["x", "a", "b"]));
    assert!(engine.warnings().is_empty());
}

#[test]
fn requiring_a_disabled_plugin_turns_the_plugin_off_with_a_warning() {
    let engine = build_ok(
        EngineBuilder::new()
            .add_plugins((Abc, p::<G>().requires::<B>(), p::<H>().requires::<G>()))
            .disable::<P<B>>(),
    );
    assert_eq!(order(&engine), ids(&["a", "c"]));
    assert_eq!(
        engine.warnings(),
        [
            Warning::RequiresDisabled {
                plugin: id("g"),
                requires: id("b")
            },
            Warning::RequiresDisabled {
                plugin: id("h"),
                requires: id("g")
            },
        ]
    );
    assert_eq!(
        engine.warnings()[0].to_string(),
        "the plugin `g` is off: it requires `b`, which is disabled"
    );

    // A disabled plugin's own requirements are not added.
    let engine = build_ok(
        EngineBuilder::new()
            .add_plugins((p::<A>(), p::<G>().requires::<X>()))
            .disable::<P<G>>(),
    );
    assert_eq!(order(&engine), ids(&["a"]));
}

#[test]
fn an_ordering_cycle_fails_naming_it() {
    let group = PluginGroupBuilder::new()
        .add_before::<P<B>>(p::<A>())
        .add_before::<P<A>>(p::<B>());
    let err = build_err(EngineBuilder::new().add_plugins(group));
    assert_eq!(
        err,
        BuildError::OrderCycle {
            cycle: ids(&["b", "a", "b"])
        }
    );
    assert_eq!(err.to_string(), "the plugin order has a cycle: b -> a -> b");

    // Through `requires` alone, and through `requires` and a group constraint together.
    let err = build_err(
        EngineBuilder::new().add_plugins((p::<A>().requires::<B>(), p::<B>().requires::<A>())),
    );
    assert!(matches!(err, BuildError::OrderCycle { .. }), "{err}");
    let group = PluginGroupBuilder::new()
        .add(p::<A>().requires::<C>())
        .add(p::<B>())
        .add_after::<P<A>>(p::<C>());
    let err = build_err(EngineBuilder::new().add_plugins(group));
    assert!(matches!(err, BuildError::OrderCycle { .. }), "{err}");
}

#[test]
fn constraints_on_absent_or_disabled_plugins_are_ignored() {
    let group = PluginGroupBuilder::new()
        .add(p::<A>())
        .add_before::<P<Missing>>(p::<B>())
        .add_after::<P<C>>(p::<D>())
        .add(p::<C>())
        .disable::<P<C>>();
    let engine = build_ok(EngineBuilder::new().add_plugins(group));
    assert_eq!(order(&engine), ids(&["a", "b", "d"]));
}

#[test]
fn a_plugin_added_twice_fails() {
    let err = build_err(EngineBuilder::new().add_plugins((p::<A>(), p::<B>(), p::<A>())));
    assert_eq!(err, BuildError::DuplicatePlugin { id: id("a") });
    assert_eq!(
        err.to_string(),
        "the plugin `a` was added twice (or its id is taken)"
    );

    // Across groups, when one is disabled, and two types with one id.
    let err = build_err(EngineBuilder::new().add_plugins((Abc, Abc.disable::<P<B>>())));
    assert_eq!(err, BuildError::DuplicatePlugin { id: id("a") });
    let err = build_err(EngineBuilder::new().add_plugins((p::<A>(), p::<AlsoA>())));
    assert_eq!(err, BuildError::DuplicatePlugin { id: id("a") });
}

#[test]
fn a_plugin_may_add_plugins_that_build_at_once() {
    let a = p::<A>().on_build(|engine| {
        engine.server(ServerStage::Game, |_, _| Ok(()));
        engine.add_plugins(p::<D>().requires::<E>());
        engine.server(ServerStage::Game, |_, _| Ok(()));
    });
    let engine = build_ok(EngineBuilder::new().add_plugins((a, p::<B>())));
    assert_eq!(
        engine.recipes(ServerStage::Game).collect::<Vec<_>>(),
        ids(&["a", "e", "d", "a", "b"])
    );
    // A plugin added by another one's build finishes building first.
    assert_eq!(engine.plugins(), ids(&["e", "d", "a", "b"]));
}

#[test]
fn a_plugin_added_during_build_pulls_its_requirement_forward() {
    let a = p::<A>().on_build(|engine| {
        engine.push(Built(engine.current_plugin()));
        engine.add_plugins(p::<D>().requires::<C>());
    });
    let engine = build_ok(EngineBuilder::new().add_plugins((a, p::<B>(), p::<C>())));
    let built: Vec<PluginId> = engine.list::<Built>().iter().map(|b| b.0).collect();
    assert_eq!(
        built,
        ids(&["a", "c", "d", "b"]),
        "`c` builds once, before `d`"
    );
}

#[test]
fn adding_an_added_plugin_from_build_fails_unless_checked() {
    let careless = p::<G>().on_build(|engine| {
        engine.add_plugins(p::<A>());
    });
    let err = build_err(EngineBuilder::new().add_plugins((p::<A>(), careless)));
    assert_eq!(err, BuildError::DuplicatePlugin { id: id("a") });

    let careful = p::<G>().on_build(|engine| {
        if !engine.is_plugin_added::<P<A>>() {
            engine.add_plugins(p::<A>());
        }
    });
    let engine = build_ok(EngineBuilder::new().add_plugins((p::<A>(), careful)));
    assert_eq!(engine.plugins(), ids(&["a", "g"]));
    assert!(engine.is_plugin_added::<P<G>>());
    assert!(!engine.is_plugin_added::<P<B>>());
}

trait Rule {
    fn name(&self) -> &'static str;
}

struct Named(&'static str);

impl Rule for Named {
    fn name(&self) -> &'static str {
        self.0
    }
}

fn sets_rule<N: Name>() -> P<N> {
    p::<N>().on_build(|engine| {
        engine.set_slot::<dyn Rule>(Rc::new(Named(N::ID)));
    })
}

#[test]
fn a_slot_set_twice_fails_naming_both_plugins() {
    let err = build_err(EngineBuilder::new().add_plugins((
        sets_rule::<First>(),
        p::<X>(),
        sets_rule::<Second>(),
    )));
    let BuildError::DuplicateSlot {
        slot,
        first,
        second,
    } = &err
    else {
        panic!("not a slot error: {err}");
    };
    assert_eq!((*first, *second), (id("first"), id("second")));
    assert!(slot.contains("Rule"), "{slot}");
    let text = err.to_string();
    assert!(text.contains("`first` and `second`"), "{text}");

    // Set in a `finish` counts too.
    let late = p::<Late>().on_finish(|engine| {
        engine.set_slot::<dyn Rule>(Rc::new(Named("late")));
    });
    let err = build_err(EngineBuilder::new().add_plugins((sets_rule::<First>(), late)));
    assert!(matches!(err, BuildError::DuplicateSlot { second, .. } if second == id("late")));
}

#[test]
fn slots_are_exclusive_and_lists_additive() {
    let engine = build_ok(EngineBuilder::new().add_plugins((
        p::<A>().on_build(|engine| {
            engine.push(1u32).push(2u32);
        }),
        sets_rule::<R>(),
        p::<B>().on_build(|engine| {
            engine.push(3u32);
        }),
    )));
    assert_eq!(engine.list::<u32>(), [1, 2, 3]);
    assert!(engine.list::<u64>().is_empty());
    assert_eq!(engine.slot::<dyn Rule>().unwrap().name(), "r");
    assert!(engine.slot::<Named>().is_none());
}

/// What `a`'s `finish` saw of `b`, which builds after it.
#[derive(Default)]
struct Seen {
    rule: Option<&'static str>,
    items: Vec<u32>,
    has_b: bool,
}

fn append_global(vm: &mut Vm, name: &[u8], text: &[u8]) {
    let before = vm.global_of(name).unwrap_or_default();
    vm.set_global_text(name, &[before.as_slice(), text].concat());
}

#[test]
fn finish_sees_every_build_and_its_recipes_come_last() {
    let a = p::<A>()
        .on_build(|engine| {
            engine.init_resource::<Seen>();
            engine.server(ServerStage::Game, |vm, _| {
                append_global(vm, b"$Order", b"a-build");
                Ok(())
            });
        })
        .on_finish(|engine| {
            let rule = engine.slot::<dyn Rule>().map(|r| r.name());
            let items = engine.list::<u32>().to_vec();
            let has_b = engine.is_plugin_added::<P<B>>();
            *engine.resource_mut::<Seen>().unwrap() = Seen { rule, items, has_b };
            engine.server(ServerStage::Game, |vm, _| {
                append_global(vm, b"$Order", b" a-finish");
                Ok(())
            });
        });
    let b = p::<B>().on_build(|engine| {
        engine.set_slot::<dyn Rule>(Rc::new(Named("b's")));
        engine.push(7u32);
        engine.server(ServerStage::Game, |vm, _| {
            append_global(vm, b"$Order", b" b-build");
            Ok(())
        });
    });
    let engine = build_ok(EngineBuilder::new().add_plugins((a, b)));
    let seen = engine.resource::<Seen>().unwrap();
    assert_eq!(seen.rule, Some("b's"));
    assert_eq!(seen.items, [7]);
    assert!(seen.has_b);
    assert_eq!(
        engine.recipes(ServerStage::Game).collect::<Vec<_>>(),
        ids(&["a", "b", "a"])
    );
    let mut vm = Vm::new();
    engine.replay(ServerStage::Game, &mut vm).unwrap();
    assert_eq!(vm.global("$Order").unwrap(), b"a-build b-build a-finish");
}

#[test]
fn a_disabled_plugin_neither_builds_nor_replays() {
    let engine = build_ok(EngineBuilder::new().add_plugins(Abc.disable::<P<B>>()));
    assert_eq!(order(&engine), ids(&["a", "c"]));
    assert!(!engine.is_plugin_added::<P<B>>());
    assert!(engine.is_plugin_added::<P<A>>());
}

#[test]
fn disable_applies_after_every_add_by_type_or_config_id() {
    let engine = build_ok(
        EngineBuilder::new()
            .disable::<P<C>>()
            .add_plugins(Abc)
            .disable_id("a"),
    );
    assert_eq!(order(&engine), ids(&["b"]));
    assert!(engine.warnings().is_empty());
}

#[test]
fn enable_turns_a_disabled_plugin_back_on_and_calls_apply_in_order() {
    let engine = build_ok(
        EngineBuilder::new()
            .add_plugins(Abc.disable::<P<B>>())
            .enable_id("b"),
    );
    assert_eq!(order(&engine), ids(&["a", "b", "c"]));

    let engine = build_ok(
        EngineBuilder::new()
            .add_plugins(Abc)
            .enable::<P<A>>()
            .disable::<P<A>>()
            .disable_id("c")
            .enable::<P<C>>(),
    );
    assert_eq!(order(&engine), ids(&["b", "c"]));
}

#[test]
fn unknown_names_are_skipped_with_warnings() {
    let engine = build_ok(
        EngineBuilder::new()
            .add_plugins(Abc.enable::<P<Z>>().set(p::<X>()))
            .disable_id("nope")
            .disable::<P<Missing>>(),
    );
    assert_eq!(order(&engine), ids(&["a", "b", "c"]));
    let unknown = |id: &str, action| Warning::UnknownPlugin {
        id: id.into(),
        action,
    };
    assert_eq!(
        engine.warnings(),
        [
            unknown("z", "enable"),
            unknown("x", "set"),
            unknown("nope", "disable"),
            unknown("missing", "disable"),
        ]
    );
    assert_eq!(
        engine.warnings()[2].to_string(),
        "disable names `nope`, which no plugin added has; skipped"
    );
}

#[test]
fn a_toggle_made_while_building_is_refused() {
    let a = p::<A>().on_build(|engine| {
        engine.push(Built(engine.current_plugin()));
        engine.server(ServerStage::Game, |_, _| Ok(()));
        engine.disable::<P<C>>();
        engine.enable_id("a");
    });
    let engine = build_ok(EngineBuilder::new().add_plugins((
        a,
        p::<B>(),
        p::<C>(),
        p::<G>().requires::<C>(),
    )));
    // `c` still builds, and so does `g`, which requires it.
    assert_eq!(order(&engine), ids(&["a", "b", "c", "g"]));
    let refused = |id: &str, action| Warning::ToggleDuringBuild {
        id: id.into(),
        action,
    };
    assert_eq!(
        engine.warnings(),
        [refused("c", "disable"), refused("a", "enable")]
    );
}

#[test]
fn a_default_no_plugin_still_needs_is_dropped() {
    // `a` requires `x` (not added: made as its default) and `b`, which is disabled: `a` is off, and `x` with it.
    let engine = build_ok(
        EngineBuilder::new()
            .add_plugins((p::<A>().requires::<X>().requires::<B>(), p::<B>()))
            .disable::<P<B>>(),
    );
    assert!(engine.plugins().is_empty());
    assert_eq!(
        engine.warnings(),
        [Warning::RequiresDisabled {
            plugin: id("a"),
            requires: id("b")
        }]
    );

    // The same through a missing id: the extension is off, and `d`, made as its default, is dropped.
    let lost = Extension {
        id: "ext.lost",
        needs: "torque.gui",
        default: true,
    };
    let engine = build_ok(EngineBuilder::new().add_plugins((p::<C>(), lost)));
    assert_eq!(order(&engine), ids(&["c"]));

    // A default another plugin that is on still requires stays.
    let engine = build_ok(
        EngineBuilder::new()
            .add_plugins((
                p::<A>().requires::<X>().requires::<B>(),
                p::<B>(),
                p::<C>().requires::<X>(),
            ))
            .disable::<P<B>>(),
    );
    assert_eq!(order(&engine), ids(&["x", "c"]));
}

#[test]
fn contains_and_ids_list_what_was_added() {
    let mut builder = EngineBuilder::new();
    builder.add_plugins((Abc.disable::<P<B>>(), p::<D>()));
    assert!(builder.contains("b"));
    assert!(!builder.contains("e"));
    assert!(builder.is_plugin_added::<P<B>>());
    assert_eq!(
        builder.ids().collect::<Vec<_>>(),
        ids(&["a", "b", "c", "d"])
    );
    // A build leaves the builder empty.
    let engine = build_ok(&mut builder);
    assert_eq!(engine.plugins(), ids(&["a", "c", "d"]));
    assert_eq!(builder.ids().count(), 0);
}

#[test]
fn set_replaces_a_plugin_in_place_keeping_its_state() {
    let engine = build_ok(EngineBuilder::new().add_plugins(Abc.set(p::<A>().net(2))));
    assert_eq!(order(&engine), ids(&["a", "b", "c"]));
    assert_eq!(engine.net_versions(), [(id("a"), 2)]);

    let engine =
        build_ok(EngineBuilder::new().add_plugins(Abc.disable::<P<B>>().set(p::<B>().net(1))));
    assert_eq!(order(&engine), ids(&["a", "c"]));
    assert!(engine.net_versions().is_empty());
}

#[test]
fn net_versions_are_sorted_by_id() {
    let engine = build_ok(EngineBuilder::new().add_plugins((
        p::<Zz>().net(3),
        p::<A>(),
        p::<Mm>().net(1),
        p::<Bc>().net(9),
    )));
    assert_eq!(
        engine.net_versions(),
        [(id("b.c"), 9), (id("mm"), 1), (id("zz"), 3)]
    );
}

#[test]
fn resources_insert_init_and_change() {
    let engine = build_ok(EngineBuilder::new().add_plugins((
        p::<A>().on_build(|engine| {
            engine
                .insert_resource(1u32)
                .insert_resource(String::from("x"));
        }),
        p::<B>().on_build(|engine| {
            engine.init_resource::<u32>().init_resource::<u8>();
            *engine.resource_mut::<String>().unwrap() += "y";
            let replaced = String::from("replaced ") + engine.resource::<String>().unwrap();
            engine.insert_resource(replaced);
        }),
    )));
    assert_eq!(engine.resource::<u32>(), Some(&1));
    assert_eq!(engine.resource::<u8>(), Some(&0));
    assert_eq!(
        engine.resource::<String>().map(String::as_str),
        Some("replaced xy")
    );
    assert_eq!(engine.resource::<i64>(), None);
}

/// A plugin whose server recipe registers `count()`, which counts calls in that Vm only, and bumps a shared resource.
fn counter() -> P<X> {
    p::<X>().on_build(|engine| {
        engine.insert_resource(Cell::new(0u32));
        engine.server(ServerStage::Game, |vm, engine| {
            engine.resource::<Cell<u32>>().unwrap().update(|n| n + 1);
            let calls = Rc::new(Cell::new(0));
            vm.register("count", 1, 1, "() calls so far", move |_, _| {
                calls.set(calls.get() + 1);
                Native::Int(calls.get())
            });
            Ok(())
        });
        engine.client(ClientStage::Natives, |vm, _| {
            vm.set_global_text(b"$Client::Made", b"1");
            Ok(())
        });
    })
}

fn replay_server(engine: &Engine) -> Vm {
    let mut vm = Vm::new();
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut vm).unwrap();
    }
    vm
}

#[test]
fn recipes_replay_into_every_new_vm_with_fresh_state() {
    let engine = build_ok(EngineBuilder::new().add_plugins(counter()));
    let mut first = replay_server(&engine);
    assert_eq!(first.eval(b"count(); return count();").unwrap(), b"2");
    // A restart: the same recipes, a new Vm, nothing carried over but the resource.
    let mut second = replay_server(&engine.clone());
    assert_eq!(second.eval(b"return count();").unwrap(), b"1");
    assert_eq!(first.native_listing(), second.native_listing());
    assert_eq!(engine.resource::<Cell<u32>>().unwrap().get(), 2);
}

#[test]
fn a_recipe_can_call_back_into_the_engine_and_replay_again() {
    let host = p::<H>().on_build(|engine| {
        // Straight away: the client's own recipe replays the server's `Game` stage into the same Vm.
        engine.client(ClientStage::Natives, |vm, engine| {
            engine
                .replay(ServerStage::Game, vm)
                .map_err(|err| err.to_string())
        });
        // Later: a native that starts a whole server from the engine it keeps.
        engine.client(ClientStage::Natives, |vm, engine| {
            let engine = engine.clone();
            vm.register("hostServer", 1, 1, "() starts a server", move |_, _| {
                let mut server = replay_server(&engine);
                let answer = server.eval(b"count(); return count();").unwrap();
                Native::Str(answer)
            });
            Ok(())
        });
    });
    let engine = build_ok(EngineBuilder::new().add_plugins((counter(), host)));
    let mut client = Vm::new();
    for &stage in ClientStage::ALL {
        engine.replay(stage, &mut client).unwrap();
    }
    assert_eq!(client.eval(b"return count();").unwrap(), b"1");
    assert_eq!(client.eval(b"return hostServer();").unwrap(), b"2");
    assert_eq!(client.eval(b"return hostServer();").unwrap(), b"2");
    // Three server replays: the client's, then one per `hostServer()`.
    assert_eq!(engine.resource::<Cell<u32>>().unwrap().get(), 3);
}

#[test]
fn a_server_replays_no_client_recipe() {
    let engine = build_ok(EngineBuilder::new().add_plugins(counter()));
    let server = replay_server(&engine);
    assert_eq!(server.global("$Client::Made"), None);
    assert_eq!(engine.recipes(ClientStage::Natives).count(), 1);
    assert_eq!(engine.recipes(ClientStage::Gui).count(), 0);

    let mut client = Vm::new();
    for &stage in ClientStage::ALL {
        engine.replay(stage, &mut client).unwrap();
    }
    assert_eq!(client.global("$Client::Made").as_deref(), Some(&b"1"[..]));
    assert_eq!(client.native_listing(), Vm::new().native_listing());
}

#[test]
fn a_failing_recipe_stops_the_replay_naming_plugin_and_stage() {
    let fails = p::<A>().on_build(|engine| {
        engine.server(ServerStage::World, |_, _| Err("no terrain file".into()));
    });
    let after = p::<B>().on_build(|engine| {
        engine.server(ServerStage::World, |vm, _| {
            vm.set_global_text(b"$After", b"ran");
            Ok(())
        });
    });
    let engine = build_ok(EngineBuilder::new().add_plugins((fails, after)));
    let mut vm = Vm::new();
    let err = engine.replay(ServerStage::World, &mut vm).unwrap_err();
    assert_eq!(
        err,
        RecipeError {
            plugin: id("a"),
            stage: Stage::Server(ServerStage::World),
            message: "no terrain file".into()
        }
    );
    assert_eq!(
        err.to_string(),
        "plugin `a` failed at the server stage World: no terrain file"
    );
    assert_eq!(vm.global("$After"), None);
}

#[test]
fn what_is_recorded_outside_plugins_is_filed_under_engine() {
    let mut builder = EngineBuilder::new();
    assert_eq!(builder.current_plugin(), id("engine"));
    builder.server(ServerStage::Net, |_, _| Ok(()));
    builder.add_plugins(p::<A>().on_build(|engine| {
        engine.server(ServerStage::Net, |_, _| Ok(()));
    }));
    let engine = build_ok(&mut builder);
    assert_eq!(
        engine.recipes(ServerStage::Net).collect::<Vec<_>>(),
        ids(&["engine", "a"])
    );
}

#[test]
fn plugin_ids_are_checked() {
    for good in [
        "torque.gamepad",
        "a",
        "my-game_2.props",
        "torque_audio::AudioPlugin",
    ] {
        assert!(PluginId::is_valid(good), "{good}");
    }
    for bad in ["", "torque gamepad", "torque/pad", "caf\u{e9}"] {
        assert!(!PluginId::is_valid(bad), "{bad}");
    }
    assert_eq!(id("torque.gamepad").to_string(), "torque.gamepad");
    assert_eq!(format!("{:?}", id("a")), "PluginId(\"a\")");
}

#[test]
#[should_panic(expected = "a plugin id is ASCII")]
fn a_bad_plugin_id_panics() {
    let _ = PluginId::new(std::hint::black_box("Bad Id"));
}

/// The derive, used from inside this crate.
#[derive(crate::PluginId)]
struct Derived;

impl Plugin for Derived {
    fn build(&self, _engine: &mut EngineBuilder) {}
}

#[test]
fn the_derive_names_the_package_and_type() {
    assert_eq!(Derived::ID, id("torque_engine::Derived"));
}

#[test]
fn an_empty_engine_builds_and_replays_nothing() {
    let engine = build_ok(&mut EngineBuilder::new());
    assert!(engine.plugins().is_empty());
    let vm = replay_server(&engine);
    assert_eq!(vm.native_listing(), Vm::new().native_listing());
}

#[test]
fn the_first_error_found_is_returned() {
    // A duplicate is found when it is added, before any plugin builds (and so before the slot set twice).
    let err = build_err(EngineBuilder::new().add_plugins((
        sets_rule::<First>(),
        sets_rule::<Second>(),
        p::<First>(),
    )));
    assert_eq!(err, BuildError::DuplicatePlugin { id: id("first") });
    // A cycle stops the build before any plugin builds.
    let group = PluginGroupBuilder::new()
        .add_before::<P<B>>(sets_rule::<A>())
        .add_before::<P<A>>(sets_rule::<B>());
    let err = build_err(EngineBuilder::new().add_plugins(group));
    assert!(matches!(err, BuildError::OrderCycle { .. }), "{err}");
}

#[test]
fn disable_and_enable_take_a_tuple_of_types() {
    let engine = build_ok(
        EngineBuilder::new()
            .add_plugins((Abc, p::<D>(), p::<E>()))
            .disable::<(P<A>, (P<C>, P<E>))>(),
    );
    assert_eq!(order(&engine), ids(&["b", "d"]));

    let group = PluginGroupBuilder::new()
        .add(p::<A>())
        .add(p::<B>())
        .add(p::<C>())
        .disable::<(P<A>, P<B>, P<C>)>()
        .enable::<(P<C>,)>();
    let engine = build_ok(
        EngineBuilder::new()
            .add_plugins(group)
            .enable::<(P<A>, P<Z>)>(),
    );
    assert_eq!(order(&engine), ids(&["a", "c"]));
    assert_eq!(
        engine.warnings(),
        [Warning::UnknownPlugin {
            id: "z".into(),
            action: "enable"
        }]
    );
}

/// One adapter type standing for many loaded extensions, each with the id it reports and the plugin it needs by id
/// (and, with `default`, a plugin `d` that requires its default `e`).
struct Extension {
    id: &'static str,
    needs: &'static str,
    default: bool,
}

impl PluginType for Extension {
    const ID: PluginId = PluginId::new("test.extension");
}

impl Plugin for Extension {
    fn id(&self) -> Option<&str> {
        Some(self.id)
    }

    fn requires(&self, requires: &mut Requires) {
        if self.default {
            requires.plugin::<NeedsE>();
        }
        requires.id(self.needs);
    }

    fn build(&self, engine: &mut EngineBuilder) {
        engine.push(Built(engine.current_plugin()));
        engine.server(ServerStage::Game, |_, _| Ok(()));
    }
}

fn ext(id: &'static str, needs: &'static str) -> Extension {
    Extension {
        id,
        needs,
        default: false,
    }
}

/// `d`, requiring `e`, as a default.
#[derive(Default)]
struct NeedsE(P<E>);

impl Plugin for NeedsE {
    fn requires(&self, requires: &mut Requires) {
        requires.plugin::<P<E>>();
    }

    fn build(&self, engine: &mut EngineBuilder) {
        self.0.build(engine);
    }
}

impl PluginType for NeedsE {
    const ID: PluginId = PluginId::new("d");
}

#[test]
fn many_extensions_share_one_adapter_type_by_their_ids() {
    let loaded = PluginGroupBuilder::new()
        .add(ext("ext.one", "b"))
        .add_before_id("ext.one", ext("ext.two", "c"))
        .add_after_id("a", ext("ext.three", "c"));
    let engine = build_ok(EngineBuilder::new().add_plugins((loaded, Abc)));
    // Each extension's string requirement builds first (`ext.one` places its requirement, then `ext.two`, which must
    // come before it); `ext.three` waits for `a`.
    assert_eq!(
        order(&engine),
        ids(&["b", "c", "ext.two", "ext.one", "a", "ext.three"])
    );
    assert!(engine.is_plugin_added_id("ext.two"));
    assert!(
        !engine.is_plugin_added::<Extension>(),
        "the adapter's own id is not used"
    );

    // A repeated id is a duplicate, whatever the type.
    let err = build_err(EngineBuilder::new().add_plugins((
        Abc,
        ext("ext.one", "b"),
        ext("ext.two", "b"),
        ext("ext.one", "c"),
    )));
    assert_eq!(err, BuildError::DuplicatePlugin { id: id("ext.one") });
}

#[test]
fn a_requirement_by_an_unknown_id_turns_the_plugin_off() {
    let engine = build_ok(EngineBuilder::new().add_plugins((
        Abc,
        ext("ext.lost", "torque.gui"),
        p::<G>().requires::<B>(),
    )));
    assert_eq!(order(&engine), ids(&["a", "b", "c", "g"]));
    assert_eq!(
        engine.warnings(),
        [Warning::RequiresMissing {
            plugin: id("ext.lost"),
            requires: "torque.gui".into()
        }]
    );
    assert_eq!(
        engine.warnings()[0].to_string(),
        "the plugin `ext.lost` is off: it requires `torque.gui`, which no plugin added has"
    );
}

#[test]
fn an_invalid_reported_id_is_a_warning_and_the_plugin_is_not_added() {
    let group = PluginGroupBuilder::new()
        .add(ext("ext one", "a"))
        .add_after_id("a", ext("ext/two", "a"));
    let engine = build_ok(EngineBuilder::new().add_plugins((p::<A>(), group)));
    assert_eq!(order(&engine), ids(&["a"]));
    assert!(
        !engine.is_plugin_added::<Extension>(),
        "no fallback to the adapter's id"
    );
    assert_eq!(
        engine.warnings(),
        [
            Warning::InvalidId {
                id: "ext one".into()
            },
            Warning::InvalidId {
                id: "ext/two".into()
            },
        ]
    );
    assert_eq!(
        engine.warnings()[0].to_string(),
        "a plugin reported the invalid id `ext one`; not added"
    );
}

#[test]
fn ids_known_only_at_run_time() {
    assert_eq!(PluginId::try_new("ext.one"), Some(id("ext.one")));
    assert_eq!(PluginId::try_new("ext one"), None);
    let reported = String::from("ext.reported");
    assert_eq!(
        PluginId::leak(&reported).map(PluginId::as_str),
        Some("ext.reported")
    );
    assert_eq!(PluginId::leak("ext reported"), None);
    let mut builder = EngineBuilder::new();
    builder.add_plugins((Abc, ext("ext.one", "a")));
    assert!(builder.is_plugin_added_id("ext.one"));
    assert!(!builder.is_plugin_added_id("ext.two"));
}

/// A runner that records the plugins it was given, or fails with `fail`.
struct Recording {
    seen: Rc<std::cell::RefCell<Vec<PluginId>>>,
    fail: Option<&'static str>,
}

impl crate::Runner for Recording {
    fn run(&self, engine: &Engine) -> Result<(), String> {
        self.seen.borrow_mut().extend_from_slice(engine.plugins());
        self.fail.map_or(Ok(()), |m| Err(m.to_string()))
    }
}

fn runner_plugin(runner: Recording) -> P<R> {
    p::<R>().on_build(move |engine| {
        engine.set_slot::<dyn crate::Runner>(Rc::new(Recording {
            seen: Rc::clone(&runner.seen),
            fail: runner.fail,
        }));
    })
}

#[test]
fn run_builds_the_engine_and_hands_it_to_the_runner() {
    let seen = Rc::default();
    let mut builder = EngineBuilder::new();
    builder.add_plugins((
        p::<A>(),
        runner_plugin(Recording {
            seen: Rc::clone(&seen),
            fail: None,
        }),
    ));
    builder.run().expect("runs");
    assert_eq!(*seen.borrow(), [id("a"), id("r")]);
}

#[test]
fn run_without_a_runner_says_so() {
    let err = EngineBuilder::new().add_plugins(p::<A>()).run();
    assert_eq!(err, Err(crate::RunError::NoRunner));
    assert!(err.unwrap_err().to_string().contains("no runner"));
}

#[test]
fn a_build_error_comes_out_of_run_and_the_runner_never_runs() {
    let seen = Rc::default();
    let mut builder = EngineBuilder::new();
    builder.add_plugins((
        p::<A>(),
        p::<AlsoA>(),
        runner_plugin(Recording {
            seen: Rc::clone(&seen),
            fail: None,
        }),
    ));
    let err = builder.run().unwrap_err();
    assert!(matches!(
        err,
        crate::RunError::Build(BuildError::DuplicatePlugin { .. })
    ));
    assert!(
        err.to_string().starts_with("the engine did not build:"),
        "{err}"
    );
    assert!(seen.borrow().is_empty());
}

#[test]
fn the_runners_error_comes_out_of_run_as_it_stands() {
    let mut builder = EngineBuilder::new();
    builder.add_plugins(runner_plugin(Recording {
        seen: Rc::default(),
        fail: Some("the window did not open"),
    }));
    let err = builder.run().unwrap_err();
    assert_eq!(
        err,
        crate::RunError::Failed("the window did not open".to_string())
    );
    assert_eq!(err.to_string(), "the window did not open");
}

#[test]
fn run_exit_is_a_failure_code_for_an_error() {
    use std::process::ExitCode;
    assert_eq!(EngineBuilder::new().run_exit(), ExitCode::FAILURE);
}
