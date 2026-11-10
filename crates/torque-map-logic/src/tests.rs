//! The bus (order, delays, names, caps) and each class's inputs and outputs, on synthetic maps written here.

use torque_mapcore::scene::Entity;

use crate::bus::{BrushVolumes, EntityId, Limits, MapLogic, Toucher, ToucherKind};
use crate::connection::entity_from_pairs;
use crate::defaults::Defaults;
use crate::entities::Kind;
use crate::sound::SoundAction;
use crate::volume::Volume;

fn ent(list: &[(&str, &str)]) -> Entity {
    entity_from_pairs(
        &list
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>(),
    )
}

fn logic(map: &[Entity]) -> MapLogic {
    with_volumes(map, &BrushVolumes::new())
}

fn with_volumes(map: &[Entity], v: &BrushVolumes) -> MapLogic {
    let mut ml = MapLogic::new(map, v, &mut Defaults::coded());
    ml.trace_inputs(true);
    ml
}

fn id(ml: &MapLogic, name: &str) -> EntityId {
    ml.find(name)[0]
}

/// (tick, input, value) delivered to the entities named `name`, from the trace.
fn got(trace: &[crate::Delivered], ml: &MapLogic, name: &str) -> Vec<(u64, String, String)> {
    let ids = ml.find(name);
    trace
        .iter()
        .filter(|d| ids.contains(&d.target))
        .map(|d| (d.tick, d.input.clone(), d.value.clone()))
        .collect()
}

fn probe(name: &str) -> Entity {
    ent(&[("classname", "probe"), ("targetname", name)])
}

fn counter(ml: &MapLogic, name: &str) -> f64 {
    match ml.kind(id(ml, name)) {
        Some(Kind::Counter(c)) => c.value,
        _ => f64::NAN,
    }
}

// --- The bus.

#[test]
fn equal_times_run_in_post_order_and_sub_tick_delays_by_time() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "logic_relay"),
            ("targetname", "r"),
            ("OnTrigger", "p,Second,,0.02,-1"),
            ("OnTrigger", "p,First,,0.01,-1"),
            ("OnTrigger", "p,A,,0,-1"),
            ("OnTrigger", "p,B,,0,-1"),
        ]),
        probe("p"),
    ]);
    ml.fire_input("r", "Trigger", "", 0.0, None);
    ml.run(2);
    let t = ml.take_inputs();
    let seen: Vec<String> = got(&t, &ml, "p").into_iter().map(|g| g.1).collect();
    assert_eq!(seen, ["A", "B", "First", "Second"]);
    let ticks: Vec<u64> = got(&t, &ml, "p").into_iter().map(|g| g.0).collect();
    assert_eq!(ticks, [1, 1, 2, 2]);
}

#[test]
fn zero_delay_chains_run_breadth_first_within_the_tick() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "logic_relay"),
            ("targetname", "a"),
            ("spawnflags", "2"),
            ("OnTrigger", "b,Trigger,,0,-1"),
            ("OnTrigger", "p,FromA,,0,-1"),
        ]),
        ent(&[
            ("classname", "logic_relay"),
            ("targetname", "b"),
            ("OnTrigger", "p,FromB,,0,-1"),
        ]),
        probe("p"),
    ]);
    ml.fire_input("a", "Trigger", "", 0.0, None);
    ml.tick(&[]);
    let t = ml.take_inputs();
    let seen: Vec<(u64, String)> = got(&t, &ml, "p").into_iter().map(|g| (g.0, g.1)).collect();
    assert_eq!(seen, [(1, "FromA".into()), (1, "FromB".into())]);
}

#[test]
fn a_delay_waits_for_the_first_tick_that_reaches_it() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "logic_relay"),
            ("targetname", "r"),
            ("OnTrigger", "p,Go,,0.5,-1"),
        ]),
        probe("p"),
    ]);
    ml.tick(&[]);
    ml.fire_input("r", "Trigger", "", 0.0, None);
    ml.run(20);
    let t = ml.take_inputs();
    // Fired in tick 2 (64 ms): due at 564 ms, the first tick at or past it is tick 18 (576 ms).
    assert_eq!(got(&t, &ml, "p")[0].0, 18);
}

#[test]
fn times_to_fire_run_out_and_parameters_override() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "logic_relay"),
            ("targetname", "r"),
            ("spawnflags", "2"),
            ("OnTrigger", "p,Twice,hello,0,2"),
            ("OnTrigger", "p,Always,,0,-1"),
        ]),
        probe("p"),
    ]);
    for _ in 0..3 {
        ml.fire_input("r", "Trigger", "", 0.0, None);
        ml.tick(&[]);
    }
    let t = ml.take_inputs();
    let g = got(&t, &ml, "p");
    assert_eq!(g.iter().filter(|x| x.1 == "Twice").count(), 2);
    assert_eq!(g.iter().filter(|x| x.1 == "Always").count(), 3);
    assert!(g.iter().filter(|x| x.1 == "Twice").all(|x| x.2 == "hello"));
}

#[test]
fn wildcards_match_trailing_prefixes_and_names_come_before_classes() {
    let mut ml = logic(&[
        probe("door1"),
        probe("door2"),
        probe("dor"),
        probe("probe"),
        ent(&[("classname", "other"), ("targetname", "x")]),
    ]);
    ml.fire_input("DOOR*", "A", "", 0.0, None);
    ml.fire_input("door**", "B", "", 0.0, None);
    ml.fire_input("*oor1", "C", "", 0.0, None);
    // A name wins over the class of the same spelling; a class wildcard reaches every entity of the class.
    ml.fire_input("probe", "D", "", 0.0, None);
    ml.fire_input("oth*", "E", "", 0.0, None);
    ml.tick(&[]);
    let t = ml.take_inputs();
    let to = |input: &str| {
        let mut v: Vec<u32> = t
            .iter()
            .filter(|d| d.input == input)
            .map(|d| d.target.0)
            .collect();
        v.sort_unstable();
        v
    };
    assert_eq!(to("A"), [0, 1]);
    assert_eq!(to("B"), [0, 1]);
    assert!(to("C").is_empty());
    assert_eq!(to("D"), [3]);
    assert_eq!(to("E"), [4]);
    assert_eq!(ml.stats.missing_targets, 1);
}

#[test]
fn special_names_resolve_to_self_caller_activator_and_player() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "trigger_relay_like"),
            ("targetname", "t"),
            ("OnUser1", "r,Trigger,,0,-1"),
        ]),
        ent(&[
            ("classname", "logic_relay"),
            ("targetname", "r"),
            ("OnTrigger", "!self,FireUser2,,0,-1"),
            ("OnTrigger", "!caller,FireUser3,,0,-1"),
            ("OnTrigger", "!activator,Activated,,0,-1"),
            ("OnTrigger", "!player,Player,,0,-1"),
            ("OnTrigger", "!picker,Nothing,,0,-1"),
        ]),
    ]);
    let p = ml.add_player("bob");
    ml.fire_input("t", "FireUser1", "", 0.0, Some(p));
    ml.run(2);
    let t = ml.take_inputs();
    let (tid, rid) = (id(&ml, "t"), id(&ml, "r"));
    let find = |input: &str| t.iter().find(|d| d.input == input).map(|d| d.target);
    assert_eq!(find("FireUser2"), Some(rid));
    assert_eq!(find("FireUser3"), Some(tid));
    assert_eq!(find("Activated"), Some(p));
    assert_eq!(find("Player"), Some(p));
    assert_eq!(find("Nothing"), None);
    // The relay's caller is the entity whose output delivered its input.
    let trig = t.iter().find(|d| d.input == "Trigger").unwrap();
    assert_eq!((trig.caller, trig.activator), (Some(tid), Some(p)));
}

#[test]
fn a_zero_delay_loop_ends_at_the_chain_cap() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "logic_relay"),
            ("targetname", "ping"),
            ("spawnflags", "2"),
            ("OnTrigger", "pong,Trigger,,0,-1"),
        ]),
        ent(&[
            ("classname", "logic_relay"),
            ("targetname", "pong"),
            ("spawnflags", "2"),
            ("OnTrigger", "ping,Trigger,,0,-1"),
        ]),
    ]);
    ml.fire_input("ping", "Trigger", "", 0.0, None);
    ml.run(3);
    assert_eq!(ml.stats.dropped_depth, 1);
    assert_eq!(
        ml.stats.events,
        u64::from(Limits::default().max_chain_depth)
    );
    assert_eq!(ml.queued(), 0);
    assert!(ml.log().iter().any(|l| l.contains("deeper")));
}

#[test]
fn a_delayed_loop_keeps_running_without_a_drop() {
    let mut ml = logic(&[ent(&[
        ("classname", "logic_relay"),
        ("targetname", "tick"),
        ("spawnflags", "2"),
        ("OnTrigger", "tick,Trigger,,0.1,-1"),
    ])]);
    ml.fire_input("tick", "Trigger", "", 0.0, None);
    ml.run(400);
    assert_eq!(ml.stats.dropped_depth, 0);
    assert!(ml.stats.events >= 99, "{}", ml.stats.events);
}

#[test]
fn the_queue_and_the_tick_budget_are_capped() {
    let limits = Limits {
        max_queue: 100,
        max_events_per_tick: 10,
        ..Limits::default()
    };
    let map = [probe("p")];
    let mut ml = MapLogic::with_limits(&map, &BrushVolumes::new(), &mut Defaults::coded(), limits);
    for _ in 0..150 {
        ml.fire_input("p", "X", "", 0.0, None);
    }
    assert_eq!(ml.queued(), 99);
    assert_eq!(ml.stats.dropped_queue, 51);
    ml.tick(&[]);
    assert_eq!(ml.stats.events, 10);
    assert!(ml.stats.deferred > 0);
    ml.run(20);
    assert_eq!(ml.stats.events, 99);
    assert_eq!(ml.queued(), 0);
}

#[test]
fn a_fork_bomb_is_bounded() {
    // Each relay fires itself twice with no delay: the depth cap ends every branch and the queue cap the width.
    let mut ml = logic(&[ent(&[
        ("classname", "logic_relay"),
        ("targetname", "bomb"),
        ("spawnflags", "2"),
        ("OnTrigger", "bomb,Trigger,,0,-1"),
        ("OnTrigger", "bomb,Trigger,,0,-1"),
    ])]);
    ml.trace_inputs(false);
    ml.fire_input("bomb", "Trigger", "", 0.0, None);
    ml.run(50);
    let l = Limits::default();
    assert!(ml.stats.events <= 50 * l.max_events_per_tick as u64);
    assert!(ml.stats.max_queue_seen <= l.max_queue);
    assert!(ml.stats.dropped_queue + ml.stats.dropped_depth > 0);
}

#[test]
fn cancel_pending_drops_waiting_events_and_kill_keeps_them() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "logic_relay"),
            ("targetname", "a"),
            ("OnTrigger", "p,FromA,,1,-1"),
        ]),
        ent(&[
            ("classname", "logic_relay"),
            ("targetname", "b"),
            ("OnTrigger", "p,FromB,,1,-1"),
        ]),
        probe("p"),
    ]);
    ml.fire_input("a", "Trigger", "", 0.0, None);
    ml.fire_input("b", "Trigger", "", 0.0, None);
    ml.tick(&[]);
    ml.fire_input("a", "CancelPending", "", 0.0, None);
    ml.fire_input("b", "Kill", "", 0.0, None);
    ml.run(40);
    let t = ml.take_inputs();
    let g: Vec<String> = got(&t, &ml, "p").into_iter().map(|g| g.1).collect();
    assert_eq!(g, ["FromB"]);
    assert!(ml.find("b").is_empty());
    // The relay's output and its own re-arm.
    assert_eq!(ml.stats.canceled, 2);
}

#[test]
fn add_output_adds_connections_and_sets_keys() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "logic_relay"),
            ("targetname", "r"),
            ("spawnflags", "2"),
        ]),
        probe("p"),
    ]);
    ml.fire_input("r", "AddOutput", "OnTrigger p:Added:v:0:1", 0.0, None);
    ml.fire_input("p", "AddOutput", "targetname q", 0.0, None);
    ml.tick(&[]);
    ml.fire_input("r", "Trigger", "", 0.0, None);
    ml.fire_input("r", "Trigger", "", 0.1, None);
    ml.run(10);
    let t = ml.take_inputs();
    // Renamed before the trigger: the connection names p, which no longer exists.
    assert!(got(&t, &ml, "q").iter().all(|g| g.1 != "Added"));
    assert!(ml.find("p").is_empty());
    assert_eq!(ml.stats.missing_targets, 1);
}

#[test]
fn fire_user_fires_on_user() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "anything"),
            ("targetname", "a"),
            ("OnUser3", "p,Got,,0,-1"),
        ]),
        probe("p"),
    ]);
    ml.fire_input("a", "FireUser3", "x", 0.0, None);
    ml.tick(&[]);
    let t = ml.take_inputs();
    assert_eq!(got(&t, &ml, "p"), [(1, "Got".into(), "x".into())]);
}

#[test]
fn unhandled_inputs_are_counted_and_logged_once() {
    let mut ml = logic(&[probe("p")]);
    for _ in 0..3 {
        ml.fire_input("p", "Wobble", "", 0.0, None);
    }
    ml.tick(&[]);
    assert_eq!(ml.stats.unhandled_inputs, 3);
    assert_eq!(ml.log().iter().filter(|l| l.contains("Wobble")).count(), 1);
    assert_eq!(ml.unsupported_classes().get("probe"), Some(&1));
}

#[test]
fn the_same_map_runs_the_same_way() {
    let map = || {
        vec![
            ent(&[
                ("classname", "logic_timer"),
                ("RefireTime", "0.1"),
                ("UseRandomTime", "1"),
                ("LowerRandomBound", "0.1"),
                ("UpperRandomBound", "0.5"),
                ("OnTimer", "c,PickRandom,,0,-1"),
            ]),
            ent(&[
                ("classname", "logic_case"),
                ("targetname", "c"),
                ("Case01", "a"),
                ("Case02", "b"),
                ("OnCase01", "p,A,,0,-1"),
                ("OnCase02", "p,B,,0,-1"),
            ]),
            probe("p"),
        ]
    };
    let run = || {
        let mut ml = logic(&map());
        ml.run(300);
        ml.take_inputs()
    };
    let (a, b) = (run(), run());
    assert!(a.len() > 20);
    assert_eq!(a, b);
}

// --- Logic entities.

#[test]
fn a_relay_triggers_spawns_disables_and_fires_once() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "logic_relay"),
            ("targetname", "r"),
            ("OnSpawn", "p,Spawned,,0,-1"),
            ("OnTrigger", "p,T,,0,-1"),
        ]),
        ent(&[
            ("classname", "logic_relay"),
            ("targetname", "once"),
            ("spawnflags", "1"),
            ("OnTrigger", "p,Once,,0,-1"),
        ]),
        ent(&[
            ("classname", "logic_relay"),
            ("targetname", "off"),
            ("StartDisabled", "1"),
            ("OnTrigger", "p,Off,,0,-1"),
        ]),
        probe("p"),
    ]);
    ml.tick(&[]);
    for _ in 0..2 {
        for n in ["r", "once", "off"] {
            ml.fire_input(n, "Trigger", "", 0.0, None);
        }
        ml.tick(&[]);
    }
    ml.fire_input("off", "Enable", "", 0.0, None);
    ml.fire_input("off", "Trigger", "", 0.0, None);
    ml.tick(&[]);
    let t = ml.take_inputs();
    let g: Vec<String> = got(&t, &ml, "p").into_iter().map(|g| g.1).collect();
    assert_eq!(g, ["Spawned", "T", "Once", "T", "Off"]);
}

#[test]
fn a_relay_with_delays_is_busy_until_they_have_fired() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "logic_relay"),
            ("targetname", "slow"),
            ("OnTrigger", "p,Slow,,0.5,-1"),
        ]),
        ent(&[
            ("classname", "logic_relay"),
            ("targetname", "fast"),
            ("spawnflags", "2"),
            ("OnTrigger", "p,Fast,,0.5,-1"),
        ]),
        probe("p"),
    ]);
    ml.tick(&[]);
    for _ in 0..3 {
        ml.fire_input("slow", "Trigger", "", 0.0, None);
        ml.fire_input("fast", "Trigger", "", 0.0, None);
        ml.tick(&[]);
    }
    ml.run(40);
    ml.fire_input("slow", "Trigger", "", 0.0, None);
    ml.run(40);
    let t = ml.take_inputs();
    let g = got(&t, &ml, "p");
    assert_eq!(g.iter().filter(|x| x.1 == "Fast").count(), 3);
    assert_eq!(g.iter().filter(|x| x.1 == "Slow").count(), 2);
}

#[test]
fn logic_auto_fires_on_spawn_removes_itself_and_its_delays_still_run() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "logic_auto"),
            ("targetname", "auto"),
            ("OnMapSpawn", "p,Now,,0,-1"),
            ("OnMapSpawn", "p,Later,,1,-1"),
            ("OnNewGame", "p,New,,0,-1"),
        ]),
        ent(&[
            ("classname", "logic_auto"),
            ("spawnflags", "0"),
            ("globalstate", "some_state"),
            ("OnMapSpawn", "p,Never,,0,-1"),
        ]),
        probe("p"),
    ]);
    ml.run(40);
    let t = ml.take_inputs();
    let g: Vec<(u64, String)> = got(&t, &ml, "p").into_iter().map(|g| (g.0, g.1)).collect();
    assert_eq!(g[0], (1, "Now".into()));
    assert_eq!(g[1], (1, "New".into()));
    assert_eq!(g[2].1, "Later");
    assert_eq!(g.len(), 3);
    assert!(ml.find("auto").is_empty());
}

#[test]
fn a_timer_fires_at_its_interval_and_takes_its_inputs() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "logic_timer"),
            ("targetname", "t"),
            ("RefireTime", "0.32"),
            ("OnTimer", "p,Tick,,0,-1"),
        ]),
        ent(&[
            ("classname", "logic_timer"),
            ("targetname", "osc"),
            ("spawnflags", "1"),
            ("RefireTime", "0.32"),
            ("OnTimerHigh", "p,High,,0,-1"),
            ("OnTimerLow", "p,Low,,0,-1"),
        ]),
        probe("p"),
    ]);
    ml.run(32);
    let t = ml.take_inputs();
    let g = got(&t, &ml, "p");
    let ticks: Vec<u64> = g.iter().filter(|x| x.1 == "Tick").map(|x| x.0).collect();
    assert_eq!(ticks, [11, 21, 31]);
    let osc: Vec<&str> = g
        .iter()
        .filter(|x| x.1 != "Tick")
        .map(|x| x.1.as_str())
        .collect();
    assert_eq!(osc, ["High", "Low", "High"]);
    ml.fire_input("t", "Disable", "", 0.0, None);
    ml.fire_input("osc", "Disable", "", 0.0, None);
    ml.run(40);
    assert!(got(&ml.take_inputs(), &ml, "p").is_empty());
    ml.fire_input("t", "FireTimer", "", 0.0, None);
    ml.tick(&[]);
    assert_eq!(got(&ml.take_inputs(), &ml, "p").len(), 1);
    ml.fire_input("t", "Enable", "", 0.0, None);
    ml.fire_input("t", "AddToTimer", "0.32", 0.0, None);
    ml.run(15);
    assert!(
        got(&ml.take_inputs(), &ml, "p").is_empty(),
        "the add delayed it"
    );
    ml.run(10);
    assert_eq!(got(&ml.take_inputs(), &ml, "p").len(), 1);
}

#[test]
fn a_case_matches_text_and_numbers_and_picks_among_its_wired_cases() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "logic_case"),
            ("targetname", "c"),
            ("Case01", "red"),
            ("Case02", "2"),
            ("Case03", "unwired"),
            ("OnCase01", "p,Red,,0,-1"),
            ("OnCase02", "p,Two,,0,-1"),
            ("OnDefault", "p,Default,,0,-1"),
        ]),
        probe("p"),
    ]);
    for v in ["red", "2.0", "blue"] {
        ml.fire_input("c", "InValue", v, 0.0, None);
    }
    ml.tick(&[]);
    let t = ml.take_inputs();
    let g: Vec<(String, String)> = got(&t, &ml, "p").into_iter().map(|g| (g.1, g.2)).collect();
    assert_eq!(
        g,
        [
            ("Red".into(), "red".into()),
            ("Two".into(), "2.0".into()),
            ("Default".into(), "blue".into())
        ]
    );
    // The caller of a case's outputs is the case.
    let cid = id(&ml, "c");
    assert!(
        t.iter()
            .filter(|d| d.input == "Red")
            .all(|d| d.caller == Some(cid))
    );
    for _ in 0..20 {
        ml.fire_input("c", "PickRandom", "", 0.0, None);
    }
    ml.tick(&[]);
    let g = got(&ml.take_inputs(), &ml, "p");
    assert!(g.iter().all(|x| x.1 == "Red" || x.1 == "Two"));
    for _ in 0..4 {
        ml.fire_input("c", "PickRandomShuffle", "", 0.0, None);
    }
    ml.tick(&[]);
    let g = got(&ml.take_inputs(), &ml, "p");
    assert_eq!(g.iter().filter(|x| x.1 == "Red").count(), 2);
    assert_eq!(g.iter().filter(|x| x.1 == "Two").count(), 2);
}

#[test]
fn a_compare_and_a_branch_fire_by_their_values() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "logic_compare"),
            ("targetname", "cmp"),
            ("InitialValue", "3"),
            ("CompareValue", "5"),
            ("OnLessThan", "p,Less,,0,-1"),
            ("OnEqualTo", "p,Equal,,0,-1"),
            ("OnNotEqualTo", "p,NotEqual,,0,-1"),
            ("OnGreaterThan", "p,Greater,,0,-1"),
        ]),
        ent(&[
            ("classname", "logic_branch"),
            ("targetname", "br"),
            ("InitialValue", "0"),
            ("OnTrue", "p,True,,0,-1"),
            ("OnFalse", "p,False,,0,-1"),
        ]),
        probe("p"),
    ]);
    ml.fire_input("cmp", "Compare", "", 0.0, None);
    ml.fire_input("cmp", "SetValueCompare", "5", 0.0, None);
    ml.fire_input("cmp", "SetCompareValue", "1", 0.0, None);
    ml.fire_input("cmp", "Compare", "", 0.0, None);
    ml.fire_input("br", "Test", "", 0.0, None);
    ml.fire_input("br", "ToggleTest", "", 0.0, None);
    ml.fire_input("br", "SetValueTest", "0", 0.0, None);
    ml.tick(&[]);
    let t = ml.take_inputs();
    let g: Vec<String> = got(&t, &ml, "p")
        .into_iter()
        .map(|g| format!("{} {}", g.1, g.2))
        .collect();
    assert_eq!(
        g,
        [
            "Less 3",
            "NotEqual 3",
            "Equal 5",
            "NotEqual 5",
            "Greater 5",
            "False 0",
            "True 1",
            "False 0"
        ]
    );
}

#[test]
fn a_counter_counts_holds_its_bounds_and_hits_once() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "math_counter"),
            ("targetname", "c"),
            ("startvalue", "1"),
            ("min", "0"),
            ("max", "3"),
            ("OutValue", "p,Out,,0,-1"),
            ("OnHitMax", "p,Max,,0,-1"),
            ("OnHitMin", "p,Min,,0,-1"),
            ("OnGetValue", "p,Get,,0,-1"),
        ]),
        probe("p"),
    ]);
    ml.tick(&[]);
    for (i, v) in [
        ("Add", "1"),
        ("Add", "5"),
        ("Add", "1"),
        ("Subtract", "10"),
        ("Divide", "0"),
        ("Multiply", "2"),
        ("SetValueNoFire", "2"),
        ("GetValue", ""),
    ] {
        ml.fire_input("c", i, v, 0.0, None);
    }
    ml.tick(&[]);
    let t = ml.take_inputs();
    let g: Vec<String> = got(&t, &ml, "p")
        .into_iter()
        .map(|g| format!("{} {}", g.1, g.2))
        .collect();
    assert_eq!(
        g,
        [
            "Out 2", "Out 3", "Max 3", "Out 3", "Out 0", "Min 0", "Out 0", "Get 2"
        ]
    );
    ml.fire_input("c", "Disable", "", 0.0, None);
    ml.fire_input("c", "Add", "1", 0.0, None);
    ml.tick(&[]);
    assert_eq!(counter(&ml, "c"), 2.0);
}

// --- Triggers and filters.

fn box_volumes() -> BrushVolumes {
    let mut v = BrushVolumes::new();
    v.insert(1, vec![Volume::of_box([-32.0; 3], [32.0; 3])]);
    // A door: 8 thick, 64 wide, 96 tall.
    v.insert(
        2,
        vec![Volume::of_box([-4.0, -32.0, 0.0], [4.0, 32.0, 96.0])],
    );
    // A button: a small box.
    v.insert(3, vec![Volume::of_box([-2.0, -4.0, -4.0], [2.0, 4.0, 4.0])]);
    v
}

fn player_at(id: EntityId, x: f32) -> Toucher {
    Toucher {
        id,
        kind: ToucherKind::Player,
        min: [x - 16.0, -16.0, 0.0],
        max: [x + 16.0, 16.0, 72.0],
    }
}

#[test]
fn a_trigger_multiple_fires_its_touch_outputs_and_waits() {
    let mut ml = with_volumes(
        &[
            ent(&[
                ("classname", "trigger_multiple"),
                ("targetname", "tm"),
                ("model", "*1"),
                ("origin", "0 0 0"),
                ("wait", "1"),
                ("OnStartTouch", "p,Start,,0,-1"),
                ("OnStartTouchAll", "p,StartAll,,0,-1"),
                ("OnEndTouch", "p,End,,0,-1"),
                ("OnEndTouchAll", "p,EndAll,,0,-1"),
                ("OnTrigger", "p,Trigger,,0,-1"),
            ]),
            probe("p"),
        ],
        &box_volumes(),
    );
    let a = ml.add_player("a");
    let b = ml.add_player("b");
    let away = [player_at(a, 500.0), player_at(b, 500.0)];
    let inside = [player_at(a, 0.0), player_at(b, 500.0)];
    let both = [player_at(a, 0.0), player_at(b, 10.0)];
    ml.tick(&away);
    ml.tick(&inside);
    ml.tick(&both);
    for _ in 0..40 {
        ml.tick(&both);
    }
    ml.tick(&inside);
    ml.tick(&away);
    let t = ml.take_inputs();
    let g: Vec<String> = got(&t, &ml, "p").into_iter().map(|g| g.1).collect();
    assert_eq!(&g[..3], ["Start", "StartAll", "Trigger"]);
    assert_eq!(g[3], "Start");
    // Re-armed after one second while someone stays inside.
    assert_eq!(g.iter().filter(|s| *s == "Trigger").count(), 2);
    let tail: Vec<&str> = g.iter().rev().take(3).rev().map(String::as_str).collect();
    assert_eq!(tail, ["End", "End", "EndAll"]);
    let start_all = t.iter().find(|d| d.input == "StartAll").unwrap();
    assert_eq!(start_all.activator, Some(a));
}

#[test]
fn a_trigger_once_fires_once_and_filters_and_flags_choose_who_counts() {
    let mut ml = with_volumes(
        &[
            ent(&[
                ("classname", "trigger_once"),
                ("targetname", "once"),
                ("model", "*1"),
                ("filtername", "only_bob"),
                ("OnTrigger", "p,Once,,0,-1"),
            ]),
            ent(&[
                ("classname", "trigger_multiple"),
                ("targetname", "npc_only"),
                ("model", "*1"),
                ("spawnflags", "2"),
                ("OnStartTouch", "p,Npc,,0,-1"),
            ]),
            ent(&[
                ("classname", "filter_activator_name"),
                ("targetname", "only_bob"),
                ("filtername", "bob"),
            ]),
            probe("p"),
        ],
        &box_volumes(),
    );
    let alice = ml.add_player("alice");
    let bob = ml.add_player("bob");
    ml.tick(&[player_at(alice, 0.0)]);
    ml.tick(&[player_at(alice, 0.0), player_at(bob, 0.0)]);
    ml.tick(&[player_at(bob, 500.0)]);
    ml.tick(&[player_at(bob, 0.0)]);
    let t = ml.take_inputs();
    let g: Vec<String> = got(&t, &ml, "p").into_iter().map(|g| g.1).collect();
    assert_eq!(g, ["Once"]);
    assert!(ml.find("once").is_empty());
}

#[test]
fn a_disabled_trigger_ignores_touches_and_touch_test_reports() {
    let mut ml = with_volumes(
        &[
            ent(&[
                ("classname", "trigger_multiple"),
                ("targetname", "tm"),
                ("model", "*1"),
                ("StartDisabled", "1"),
                ("OnStartTouch", "p,Start,,0,-1"),
                ("OnTouching", "p,Touching,,0,-1"),
                ("OnNotTouching", "p,NotTouching,,0,-1"),
            ]),
            probe("p"),
        ],
        &box_volumes(),
    );
    let a = ml.add_player("a");
    ml.tick(&[player_at(a, 0.0)]);
    ml.fire_input("tm", "TouchTest", "", 0.0, None);
    ml.tick(&[player_at(a, 0.0)]);
    ml.fire_input("tm", "Enable", "", 0.0, None);
    ml.tick(&[player_at(a, 0.0)]);
    ml.tick(&[player_at(a, 0.0)]);
    ml.fire_input("tm", "TouchTest", "", 0.0, None);
    ml.tick(&[player_at(a, 0.0)]);
    let t = ml.take_inputs();
    let g: Vec<String> = got(&t, &ml, "p").into_iter().map(|g| g.1).collect();
    assert_eq!(g, ["NotTouching", "Start", "Touching"]);
}

#[test]
fn filters_test_names_classes_and_other_filters() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "filter_activator_name"),
            ("targetname", "f_name"),
            ("filtername", "bo*"),
        ]),
        ent(&[
            ("classname", "filter_activator_class"),
            ("targetname", "f_class"),
            ("filterclass", "player"),
            ("Negated", "1"),
        ]),
        ent(&[
            ("classname", "filter_multi"),
            ("targetname", "f_any"),
            ("filtertype", "1"),
            ("Filter01", "f_name"),
            ("Filter02", "f_class"),
        ]),
        ent(&[
            ("classname", "filter_multi"),
            ("targetname", "f_all"),
            ("Filter01", "f_name"),
            ("Filter02", "f_class"),
        ]),
        ent(&[
            ("classname", "filter_multi"),
            ("targetname", "f_self"),
            ("Filter01", "f_self"),
        ]),
    ]);
    let bob = ml.add_player("bob");
    for f in ["f_name", "f_class", "f_any", "f_all", "f_self"] {
        ml.fire_input(
            f,
            "AddOutput",
            &format!("OnPass {f}:Passed::0:-1"),
            0.0,
            None,
        );
        ml.fire_input(
            f,
            "AddOutput",
            &format!("OnFail {f}:Failed::0:-1"),
            0.0,
            None,
        );
        ml.fire_input(f, "TestActivator", "", 0.0, Some(bob));
    }
    ml.tick(&[]);
    let t = ml.take_inputs();
    let result = |f: &str| {
        got(&t, &ml, f)
            .into_iter()
            .find(|g| g.1 == "Passed" || g.1 == "Failed")
            .map(|g| g.1)
            .unwrap()
    };
    assert_eq!(result("f_name"), "Passed");
    assert_eq!(result("f_class"), "Failed");
    assert_eq!(result("f_any"), "Passed");
    assert_eq!(result("f_all"), "Failed");
    // A filter that names itself ends at the nesting cap and fails.
    assert_eq!(result("f_self"), "Failed");
}

// --- Movers.

fn door_map(extra: &[(&str, &str)]) -> Vec<Entity> {
    let mut d = vec![
        ("classname", "func_door"),
        ("targetname", "door"),
        ("model", "*2"),
        ("origin", "64 0 0"),
        ("movedir", "0 90 0"),
        ("speed", "64"),
        ("wait", "1"),
        ("OnOpen", "p,Open,,0,-1"),
        ("OnFullyOpen", "p,FullyOpen,,0,-1"),
        ("OnClose", "p,Close,,0,-1"),
        ("OnFullyClosed", "p,FullyClosed,,0,-1"),
        ("OnBlockedClosing", "p,BlockedClosing,,0,-1"),
        ("OnLockedUse", "p,LockedUse,,0,-1"),
    ];
    d.extend_from_slice(extra);
    vec![ent(&d), probe("p")]
}

fn frac(ml: &MapLogic, name: &str) -> f32 {
    ml.kind(id(ml, name))
        .and_then(|k| k.as_mover())
        .unwrap()
        .frac
}

#[test]
fn a_door_opens_by_its_size_and_closes_after_its_wait() {
    let mut ml = with_volumes(&door_map(&[("spawnflags", "0")]), &box_volumes());
    ml.tick(&[]);
    ml.fire_input("door", "Open", "", 0.0, None);
    ml.run(16);
    assert!(
        (frac(&ml, "door") - 0.5).abs() < 0.05,
        "{}",
        frac(&ml, "door")
    );
    ml.run(20);
    assert_eq!(frac(&ml, "door"), 1.0);
    let m = ml
        .kind(id(&ml, "door"))
        .and_then(|k| k.as_mover())
        .unwrap()
        .clone();
    // 64 wide along y, no lip.
    assert!(
        (m.offset[1] - 64.0).abs() < 1e-3 && m.offset[0].abs() < 1e-3,
        "{:?}",
        m.offset
    );
    ml.run(80);
    assert_eq!(frac(&ml, "door"), 0.0);
    let t = ml.take_inputs();
    let g: Vec<String> = got(&t, &ml, "p").into_iter().map(|g| g.1).collect();
    assert_eq!(g, ["Open", "FullyOpen", "Close", "FullyClosed"]);
}

#[test]
fn a_door_is_blocked_reverses_and_a_locked_door_ignores_use() {
    let mut ml = with_volumes(
        &door_map(&[("spawnflags", "256"), ("spawnpos", "1")]),
        &box_volumes(),
    );
    let pl = ml.add_player("pl");
    // Standing in the doorway (the door closes along -y back over x = 64).
    let blocker = Toucher {
        id: pl,
        kind: ToucherKind::Player,
        min: [48.0, -16.0, 0.0],
        max: [80.0, 16.0, 72.0],
    };
    ml.tick(&[blocker]);
    ml.fire_input("door", "Close", "", 0.0, None);
    for _ in 0..40 {
        ml.tick(&[blocker]);
    }
    let t = ml.take_inputs();
    let g: Vec<String> = got(&t, &ml, "p").into_iter().map(|g| g.1).collect();
    assert_eq!(g[..2], ["Close".to_string(), "BlockedClosing".into()]);
    assert!(frac(&ml, "door") > 0.5, "it went back open");
    ml.fire_input("door", "Lock", "", 0.0, None);
    ml.tick(&[]);
    ml.use_entity(id(&ml, "door"), Some(pl));
    let t = ml.take_inputs();
    assert!(got(&t, &ml, "p").iter().any(|g| g.1 == "LockedUse"));
}

#[test]
fn a_door_opens_when_a_player_touches_it() {
    let mut ml = with_volumes(&door_map(&[]), &box_volumes());
    let pl = ml.add_player("pl");
    ml.tick(&[player_at(pl, 500.0)]);
    assert_eq!(frac(&ml, "door"), 0.0);
    // Against the door's face (x = 60) from the -x side.
    let at = Toucher {
        id: pl,
        kind: ToucherKind::Player,
        min: [27.5, -16.0, 0.0],
        max: [59.5, 16.0, 72.0],
    };
    ml.tick(&[at]);
    ml.run(5);
    assert!(frac(&ml, "door") > 0.0);
}

#[test]
fn a_rotating_door_turns_about_its_axis() {
    let mut ml = with_volumes(
        &[ent(&[
            ("classname", "func_door_rotating"),
            ("targetname", "rd"),
            ("model", "*2"),
            ("distance", "90"),
            ("speed", "90"),
            ("spawnflags", "2"),
            ("wait", "-1"),
        ])],
        &box_volumes(),
    );
    ml.tick(&[]);
    ml.fire_input("rd", "Open", "", 0.0, None);
    ml.run(40);
    let m = ml.kind(id(&ml, "rd")).and_then(|k| k.as_mover()).unwrap();
    assert_eq!(m.frac, 1.0);
    assert_eq!(m.pose_at(m.frac).angles, [0.0, -90.0, 0.0]);
    let (lo, hi) = m.volumes_now()[0].bounds();
    // Turned a quarter: the 64-wide leaf now runs along x.
    assert!((hi[0] - lo[0] - 64.0).abs() < 1e-2, "{lo:?} {hi:?}");
}

#[test]
fn a_movelinear_goes_to_a_position() {
    let mut ml = with_volumes(
        &[
            ent(&[
                ("classname", "func_movelinear"),
                ("targetname", "lift"),
                ("model", "*1"),
                ("movedir", "-90 0 0"),
                ("movedistance", "64"),
                ("speed", "640"),
                ("OnFullyOpen", "p,Top,,0,-1"),
                ("OnFullyClosed", "p,Bottom,,0,-1"),
            ]),
            probe("p"),
        ],
        &box_volumes(),
    );
    ml.tick(&[]);
    ml.fire_input("lift", "SetPosition", "0.5", 0.0, None);
    ml.run(10);
    assert_eq!(frac(&ml, "lift"), 0.5);
    let m = ml.kind(id(&ml, "lift")).and_then(|k| k.as_mover()).unwrap();
    assert!((m.pose_at(m.frac).origin[2] - 32.0).abs() < 1e-3);
    ml.fire_input("lift", "Open", "", 0.0, None);
    ml.run(10);
    ml.fire_input("lift", "Close", "", 0.0, None);
    ml.run(10);
    let t = ml.take_inputs();
    let g: Vec<String> = got(&t, &ml, "p").into_iter().map(|g| g.1).collect();
    assert_eq!(g, ["Top", "Bottom"]);
}

#[test]
fn a_button_presses_moves_in_and_comes_back_out() {
    let mut ml = with_volumes(
        &[
            ent(&[
                ("classname", "func_button"),
                ("targetname", "b"),
                ("model", "*3"),
                ("movedir", "0 0 0"),
                ("speed", "40"),
                ("wait", "0.5"),
                ("OnPressed", "p,Pressed,,0,-1"),
                ("OnIn", "p,In,,0,-1"),
                ("OnOut", "p,Out,,0,-1"),
                ("OnUseLocked", "p,UseLocked,,0,-1"),
            ]),
            probe("p"),
        ],
        &box_volumes(),
    );
    let pl = ml.add_player("pl");
    ml.tick(&[]);
    ml.use_entity(id(&ml, "b"), Some(pl));
    ml.use_entity(id(&ml, "b"), Some(pl));
    ml.run(40);
    ml.fire_input("b", "Lock", "", 0.0, None);
    ml.tick(&[]);
    ml.use_entity(id(&ml, "b"), Some(pl));
    let t = ml.take_inputs();
    let g: Vec<(u64, String)> = got(&t, &ml, "p").into_iter().map(|g| (g.0, g.1)).collect();
    let names: Vec<&str> = g.iter().map(|g| g.1.as_str()).collect();
    assert_eq!(names, ["Pressed", "In", "Out", "UseLocked"]);
    // 4 units at 40 a second: in after 0.1 s; out 0.5 s after that, back in another 0.1 s.
    assert!(g[1].0 >= 4 && g[1].0 <= 6, "{g:?}");
    assert!(g[2].0 >= g[1].0 + 18 && g[2].0 <= g[1].0 + 22, "{g:?}");
    let pressed = t.iter().find(|d| d.input == "Pressed").unwrap();
    assert_eq!(pressed.activator, Some(pl));
}

#[test]
fn a_brush_shows_hides_and_follows_its_solidity() {
    let mut ml = with_volumes(
        &[
            ent(&[
                ("classname", "func_brush"),
                ("targetname", "wall"),
                ("model", "*1"),
                ("StartDisabled", "1"),
            ]),
            ent(&[
                ("classname", "func_brush"),
                ("targetname", "ghost"),
                ("model", "*1"),
                ("Solidity", "1"),
            ]),
        ],
        &box_volumes(),
    );
    let state = |ml: &MapLogic, n: &str| {
        let m = ml.kind(id(ml, n)).and_then(|k| k.as_mover()).unwrap();
        (m.visible, m.solid)
    };
    assert_eq!(state(&ml, "wall"), (false, false));
    assert_eq!(state(&ml, "ghost"), (true, false));
    ml.fire_input("wall", "Toggle", "", 0.0, None);
    ml.tick(&[]);
    assert_eq!(state(&ml, "wall"), (true, true));
    assert!(
        ml.mover_hit([-10.0; 3], [10.0; 3])
            .is_some_and(|(i, _)| i == id(&ml, "wall"))
    );
    ml.fire_input("wall", "Disable", "", 0.0, None);
    ml.tick(&[]);
    assert!(ml.mover_hit([-10.0; 3], [10.0; 3]).is_none());
}

#[test]
fn a_mover_ghost_gives_the_pose_the_server_has() {
    let mut ml = with_volumes(&door_map(&[("wait", "-1")]), &box_volumes());
    ml.tick(&[]);
    ml.fire_input("door", "Open", "", 0.0, None);
    ml.tick(&[]);
    let (_, ghost) = ml.mover_ghosts()[0].clone();
    for _ in 0..40 {
        ml.tick(&[]);
        let f = frac(&ml, "door");
        let g = ghost.fraction_at(ml.tick_count() as f64);
        assert!(
            (f - g).abs() < 0.04,
            "server {f} ghost {g} at {}",
            ml.tick_count()
        );
    }
    assert_eq!(ghost.fraction_at(1e6), 1.0);
    assert_eq!(ghost.model, 2);
}

// --- Sound.

#[test]
fn an_ambient_sound_starts_silent_by_default_and_follows_its_inputs() {
    let mut ml = logic(&[
        ent(&[
            ("classname", "ambient_generic"),
            ("targetname", "hum"),
            ("message", "ambient/hum.wav"),
            ("spawnflags", "16"),
            ("health", "5"),
        ]),
        ent(&[
            ("classname", "ambient_generic"),
            ("targetname", "beep"),
            ("message", "beep.wav"),
        ]),
        ent(&[
            ("classname", "ambient_generic"),
            ("targetname", "loud"),
            ("message", "loud.wav"),
            ("spawnflags", "0"),
        ]),
    ]);
    ml.tick(&[]);
    let first = ml.take_sounds();
    assert_eq!(first.len(), 1);
    assert_eq!(
        (first[0].sound.as_str(), first[0].looped),
        ("loud.wav", true)
    );
    ml.fire_input("hum", "PlaySound", "", 0.0, None);
    ml.fire_input("hum", "Volume", "10", 0.0, None);
    ml.fire_input("hum", "Pitch", "200", 0.0, None);
    ml.fire_input("beep", "PlaySound", "", 0.0, None);
    ml.tick(&[]);
    let s = ml.take_sounds();
    let acts: Vec<(String, SoundAction)> = s.iter().map(|e| (e.sound.clone(), e.action)).collect();
    assert_eq!(acts[0], ("ambient/hum.wav".into(), SoundAction::Play));
    assert_eq!(s[0].volume, 0.5);
    assert_eq!(s[2].pitch, 2.0);
    assert!(!s[3].looped);
    let playing: Vec<String> = ml.playing_sounds().into_iter().map(|e| e.sound).collect();
    assert_eq!(playing, ["ambient/hum.wav", "loud.wav"]);
    ml.fire_input("hum", "Volume", "0", 0.0, None);
    ml.tick(&[]);
    assert_eq!(ml.take_sounds()[0].action, SoundAction::Stop);
}

// --- The FGD over the coded defaults.

#[test]
fn an_fgd_default_replaces_a_coded_one_and_the_difference_is_reported() {
    let text = br#"
        @BaseClass = Targetname [ targetname(target_source) : "Name" ]
        @SolidClass base(Targetname) = func_button : "x"
        [
            speed(integer) : "Speed" : 200
            wait(float) : "Wait" : "0.5"
            spawnflags(flags) = [ 1024 : "Use" : 1  16384 : "Other" : 1 ]
        ]
    "#;
    let defs = crate::fgd::EntityDefs::new(crate::fgd::Fgd::parse(text));
    let mut d = Defaults::with_fgd(defs);
    let map = [ent(&[
        ("classname", "func_button"),
        ("targetname", "b"),
        ("model", "*3"),
    ])];
    let ml = MapLogic::new(&map, &box_volumes(), &mut d);
    let m = ml.kind(id(&ml, "b")).and_then(|k| k.as_mover()).unwrap();
    assert_eq!((m.speed, m.wait), (200.0, 0.5));
    assert_eq!(ml.entity(id(&ml, "b")).unwrap().spawnflags, 1024 | 16384);
    assert_eq!(d.differences.len(), 3, "{:?}", d.differences);
    // Coded alone: the older games' numbers.
    let ml = MapLogic::new(&map, &box_volumes(), &mut Defaults::coded());
    let m = ml.kind(id(&ml, "b")).and_then(|k| k.as_mover()).unwrap();
    assert_eq!((m.speed, m.wait), (5.0, 3.0));
}

// --- The plugin.

#[test]
fn the_plugin_adds_the_ghost_class_and_the_script_drives_the_map() {
    use torque_engine::{EngineBuilder, ServerStage};
    use torque_ts_vm::vm::Vm;

    let engine = EngineBuilder::new()
        .add_plugins(crate::MapLogicPlugin)
        .build()
        .unwrap();
    let names: Vec<String> = engine
        .net_classes()
        .ghosts()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    assert_eq!(names, ["MapMover"]);
    let mut vm = Vm::new();
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut vm).unwrap();
    }
    assert_eq!(vm.eval(b"return mapLogicTick();").unwrap(), b"0");
    let map = door_map(&[("wait", "-1")]);
    let ml = MapLogic::new(&map, &box_volumes(), &mut Defaults::coded());
    install(&mut vm, ml);
    vm.eval(
        b"$outs = \"\"; function MapLogic::onOutput(%e, %o, %a) { $outs = $outs SPC %e @ \".\" @ %o; }",
    )
    .unwrap();
    vm.eval(b"mapLogicPlayer(\"me\", \"500 0 0\", \"530 30 70\");")
        .unwrap();
    assert_eq!(
        vm.eval(b"return mapLogicFire(\"door\", \"Open\");")
            .unwrap(),
        b"1"
    );
    vm.eval(b"mapLogicTick(40);").unwrap();
    assert_eq!(
        vm.eval(b"return mapLogicFraction(\"door\");").unwrap(),
        b"1"
    );
    let outs = String::from_utf8(vm.eval(b"return $outs;").unwrap()).unwrap();
    assert!(
        outs.contains("door.OnOpen") && outs.contains("door.OnFullyOpen"),
        "{outs}"
    );
    assert_eq!(
        vm.eval(b"return mapLogicCount(\"missing\");").unwrap(),
        b"0"
    );
}

use crate::plugin::install;

#[test]
fn wildcard_relays_growing_their_outputs_stay_within_the_work_budget() {
    // 300 relays, each wired to every relay by a wildcard and adding a further wildcard connection to every relay each time it
    // fires: the fan-out and the outputs grow without end unless the caps hold them.
    let limits = Limits {
        max_total_outputs: 20_000,
        ..Limits::default()
    };
    let map: Vec<Entity> = (0..300)
        .map(|i| {
            ent(&[
                ("classname", "logic_relay"),
                ("targetname", &format!("r{i}")),
                ("spawnflags", "2"),
                ("OnTrigger", "r*,Trigger,,0,-1"),
                ("OnTrigger", "r*,AddOutput,OnTrigger r*:Trigger::0:-1,0,-1"),
            ])
        })
        .collect();
    let mut ml = MapLogic::with_limits(&map, &BrushVolumes::new(), &mut Defaults::coded(), limits);
    ml.fire_input("r0", "Trigger", "", 0.0, None);
    let mut most = 0;
    for _ in 0..30 {
        ml.tick(&[]);
        most = most.max(ml.work_used());
        assert!(ml.total_outputs() <= limits.max_total_outputs);
        assert!(ml.queued() <= limits.max_queue);
    }
    // One delivery's own posts (at most an entity's outputs) may run past the budget before the next check.
    let slack = 2 * limits.max_outputs_per_entity as u64 + 16;
    assert!(most <= limits.max_work_per_tick + slack, "{most}");
    assert_eq!(ml.stats.max_work_seen, most);
    assert!(ml.stats.deferred > 0);
    assert!(ml.stats.dropped_outputs > 0);
}

#[test]
fn a_received_mover_record_is_held_to_usable_numbers() {
    use crate::mover::{MAX_GHOST_COORD, MoverGhost};
    let hostile = MoverGhost {
        model: 3,
        origin: [f32::NAN, f32::INFINITY, -1e30],
        offset: [f32::NEG_INFINITY, 5.0, f32::NAN],
        turn: [1e9, f32::NAN, -1e9],
        from: f32::NAN,
        to: 7.0,
        rate: f32::INFINITY,
        since_tick: u32::MAX,
        visible: true,
        solid: true,
    };
    assert!(hostile.sanitized(3).is_none(), "model 3 of 3");
    assert!(
        MoverGhost {
            model: 0,
            ..hostile.clone()
        }
        .sanitized(9)
        .is_none(),
        "the world is no mover"
    );
    let g = hostile.sanitized(4).unwrap();
    let all = g
        .origin
        .iter()
        .chain(&g.offset)
        .chain(&g.turn)
        .chain([&g.from, &g.to, &g.rate]);
    assert!(all.clone().all(|v| v.is_finite()));
    assert_eq!(g.origin, [0.0, MAX_GHOST_COORD, -MAX_GHOST_COORD]);
    assert_eq!((g.from, g.to), (0.0, 1.0));
    for t in [f64::NAN, f64::INFINITY, -1e300, 0.0, 1e300] {
        let p = g.pose_at(t);
        assert!(
            p.origin.iter().chain(&p.angles).all(|v| v.is_finite()),
            "{t}"
        );
    }
}

#[test]
fn a_tick_called_from_an_output_callback_is_refused() {
    use torque_ts_vm::vm::Vm;
    let mut vm = Vm::new();
    crate::plugin::register(&mut vm);
    let map = [
        ent(&[
            ("classname", "logic_timer"),
            ("targetname", "t"),
            ("RefireTime", "0.032"),
            ("OnTimer", "p,X,,0,-1"),
        ]),
        probe("p"),
    ];
    install(
        &mut vm,
        MapLogic::new(&map, &BrushVolumes::new(), &mut Defaults::coded()),
    );
    vm.eval(b"$nested = \"\"; $calls = 0; function MapLogic::onOutput(%e, %o, %a) { $calls++; $nested = $nested @ mapLogicTick(5); }")
        .unwrap();
    assert_eq!(vm.eval(b"return mapLogicTick(10);").unwrap(), b"10");
    let calls: i32 = String::from_utf8(vm.eval(b"return $calls;").unwrap())
        .unwrap()
        .parse()
        .unwrap();
    assert!(calls >= 5);
    let nested = String::from_utf8(vm.eval(b"return $nested;").unwrap()).unwrap();
    assert!(
        nested.chars().all(|c| c == '0') && nested.len() == calls as usize,
        "{nested}"
    );
    // Not stuck: the next tick from outside runs.
    assert_eq!(vm.eval(b"return mapLogicTick(1);").unwrap(), b"1");
}

#[test]
fn kills_and_renames_in_a_huge_shared_name_stay_cheap() {
    // 100,000 entities share one name; every tick an event kills up to the target cap of them and another renames as many.
    let mut map: Vec<Entity> = (0..100_000).map(|_| probe("crowd")).collect();
    map.push(ent(&[
        ("classname", "logic_timer"),
        ("RefireTime", "0.032"),
        ("OnTimer", "crowd,Kill,,0,-1"),
        ("OnTimer", "crowd,AddOutput,targetname moved,0,-1"),
    ]));
    let mut ml = MapLogic::new(&map, &BrushVolumes::new(), &mut Defaults::coded());
    let t0 = std::time::Instant::now();
    for _ in 0..40 {
        ml.tick(&[]);
        assert!(ml.work_used() <= Limits::default().max_work_per_tick + 64);
    }
    let crowd = ml
        .entities()
        .filter(|(_, e)| e.alive && e.name == "crowd")
        .count();
    let moved = ml
        .entities()
        .filter(|(_, e)| e.alive && e.name == "moved")
        .count();
    assert_eq!(crowd, 0);
    assert!(moved > 0);
    // A removal and a rename are one set operation each, not a scan of the group: the whole run is quick even unoptimized.
    assert!(t0.elapsed().as_secs() < 30, "{:?}", t0.elapsed());
    assert!(
        ml.entities()
            .filter(|(_, e)| e.alive && e.class == "probe")
            .count()
            > 0
    );
}
