use super::*;

/// A datablock's field as a test writes it, and its fields.
struct Entry {
    name: &'static str,
    index: u32,
    value: Value,
}

struct Record {
    entries: Vec<Entry>,
}

impl ImageStates {
    fn of(r: &Record) -> Option<ImageStates> {
        Some(ImageStates::of_fields(
            r.entries.iter().map(|e| (e.name, e.index, &e.value)),
        ))
    }
}

/// A test state: name, transitions, timeout, waits, fires, spin.
type TestState<'a> = (&'a str, [i32; 11], f32, bool, bool, u32);

fn record(states: &[TestState<'_>]) -> Record {
    let mut r = Record {
        entries: Vec::new(),
    };
    for (i, (_, trans, timeout, wait, fire, spin)) in states.iter().enumerate() {
        let i = i as u32;
        let mut push = |name: &'static str, index: u32, value: Value| {
            r.entries.push(Entry { name, index, value })
        };
        push("stateUsed", i, Value::Bool(true));
        for (k, t) in trans.iter().enumerate() {
            push(
                "stateTrans",
                11 * i + k as u32,
                Value::Int(i64::from(t + 1)),
            );
        }
        push("stateTimeoutValue", i, Value::F32(*timeout));
        push("stateWaitForTimeout", i, Value::Bool(*wait));
        push("stateFire", i, Value::Bool(*fire));
        push("stateSpinThread", i, Value::Int(i64::from(*spin)));
    }
    r
}

const N: [i32; 11] = [-1; 11];

fn ap(
    slots: &mut Slots,
    sent: &[Option<ImageSlot>; IMAGE_SLOTS],
    added: bool,
    now: Option<u32>,
    f: &dyn Fn(u32) -> Option<ImageStates>,
) {
    let mut counts = [0, 1, 2, 3].map(|i| slots[i].as_ref().map_or(0, |m| m.fire_count));
    apply(
        &TgeImageRules,
        slots,
        &mut counts,
        sent,
        added,
        now,
        f,
        &mut TICK_BUDGET.clone(),
        &mut TickStats::default(),
        &mut Vec::new(),
    );
}

impl MountedImage {
    /// The slot run to `now` on a copy, with no budget.
    fn at(&self, states: Option<&ImageStates>, now: Option<u32>) -> MountedImage {
        let mut m = self.clone();
        if let Some(st) = states {
            m.advance_to(
                &TgeImageRules,
                st,
                now,
                &mut u32::MAX.clone(),
                false,
                &mut TickStats::default(),
            );
        }
        m
    }
}

#[test]
fn many_held_images_after_a_long_gap_stay_within_the_tick_budget() {
    let f = |_: u32| Some(tool());
    let mut images = std::collections::BTreeMap::new();
    // 1,000 ghosts with four images each, mounted at 0.
    for g in 0..1000u32 {
        let mut slots: Slots = Default::default();
        let mut b = TICK_BUDGET;
        let mut stats = TickStats::default();
        let four = [0, 1, 2, 3].map(|_| Some(sent(70, 0, false)));
        apply(
            &TgeImageRules,
            &mut slots,
            &mut [0; IMAGE_SLOTS],
            &four,
            true,
            Some(0),
            &f,
            &mut b,
            &mut stats,
            &mut Vec::new(),
        );
        images.insert(g, slots);
    }
    let mut cursor = 0;
    // An hour later, then a step a tick later: each step runs at most the budget, the rest is skipped or deferred.
    let mut total = TickStats::default();
    for now in [3_600_000u32, 3_600_032] {
        let mut budget = TICK_BUDGET;
        let mut stats = TickStats::default();
        advance_all(
            &TgeImageRules,
            &mut images,
            &mut cursor,
            &f,
            Some(now),
            &mut budget,
            &mut stats,
        );
        assert!(stats.run <= u64::from(TICK_BUDGET), "{stats:?}");
        total.run += stats.run;
        total.skipped += stats.skipped;
        total.deferred += stats.deferred;
    }
    assert!(total.skipped > 0 && total.deferred > 0, "{total:?}");
    // Enough steps bring every slot to the clock, each settled in Ready.
    for _ in 0..4096 {
        let mut budget = TICK_BUDGET;
        let mut stats = TickStats::default();
        advance_all(
            &TgeImageRules,
            &mut images,
            &mut cursor,
            &f,
            Some(3_600_032),
            &mut budget,
            &mut stats,
        );
        if stats.deferred == 0 && stats.run == 0 {
            break;
        }
    }
    for slots in images.values() {
        for m in slots.iter().flatten() {
            assert_eq!((m.at_ms, m.state), (Some(3_600_032), 1));
        }
    }
}

fn trans(timeout: i32, trigger_down: i32, trigger_up: i32) -> [i32; 11] {
    let mut t = N;
    t[10] = timeout;
    t[9] = trigger_down;
    t[8] = trigger_up;
    t
}

/// A tool's states: Activate (0.15 s) -> Ready; Ready waits for the trigger; Fire (0.2 s) -> Ready.
fn tool() -> ImageStates {
    let r = record(&[
        ("Activate", trans(1, -1, -1), 0.15, true, false, 0),
        ("Ready", trans(-1, 2, -1), 0.0, false, false, 0),
        ("Fire", trans(1, -1, -1), 0.2, true, true, 0),
    ]);
    ImageStates::of(&r).unwrap()
}

fn sent(db: u32, count: u32, trigger: bool) -> ImageSlot {
    ImageSlot {
        datablock: Some(db),
        skin: NetStr::Null,
        flags: [false, false, true, false, trigger],
        fire_count: count,
        firing: false,
    }
}

fn one(s: ImageSlot) -> [Option<ImageSlot>; IMAGE_SLOTS] {
    [Some(s), None, None, None]
}

#[test]
fn a_state_that_names_a_sequence_puts_it_on_the_animation_thread() {
    // Activate plays sequence 0 forward at speed 1; Ready names none; Fire plays 1 backward over its timeout
    // (stateScaleAnimation), and a flash state's sequence is not the animation thread's.
    let mut r = record(&[
        ("Activate", trans(1, -1, -1), 0.15, true, false, 0),
        ("Ready", trans(-1, 2, -1), 0.0, false, false, 0),
        ("Fire", trans(1, -1, -1), 0.2, true, true, 0),
        ("Flash", N, 0.0, false, false, 0),
    ]);
    let mut push =
        |name: &'static str, index: u32, value: Value| r.entries.push(Entry { name, index, value });
    // The record keeps the wire's signed int: the sign in bit 0, the magnitude above it.
    push("stateSequence", 0, Value::Int(0));
    push("stateSequence", 2, Value::Int(1 << 1));
    push("stateDirection", 2, Value::Bool(false));
    push("stateScaleAnimation", 2, Value::Bool(true));
    push("stateSequence", 3, Value::Int(3 << 1));
    push("stateFlashSequence", 3, Value::Bool(true));
    push("stateSequence", 4, Value::Int(1 | 9 << 1));
    let st = ImageStates::of(&r).unwrap();
    assert_eq!(
        [0, 1, 2, 3, 4].map(|i| st.states[i].sequence),
        [0, -1, 1, 3, -1],
        "decoded; a negative is none"
    );
    let f = move |_: u32| Some(st.clone());
    let mut slots: Slots = Default::default();
    ap(&mut slots, &one(sent(70, 0, false)), true, Some(0), &f);
    let m = slots[0].as_ref().unwrap().clone();
    let a = m.anim.unwrap();
    assert_eq!(
        (a.sequence, a.forward, a.over, a.start()),
        (0, true, None, 0.0)
    );
    assert_eq!(a.time_scale(2.0), 1.0);
    // Ready leaves the thread as Activate put it, running on.
    let ready = m.at(f(70).as_ref(), Some(10 * 32));
    assert_eq!(ready.state, 1);
    let a = ready.anim.unwrap();
    assert_eq!(a.sequence, 0);
    assert!((a.time - 10.0 * TICK_SEC).abs() < 1e-5, "{a:?}");
    ap(
        &mut slots,
        &one(sent(70, 1, false)),
        true,
        Some(10 * 32),
        &f,
    );
    let fire = slots[0].as_ref().unwrap();
    assert_eq!(fire.state, 2);
    let a = fire.anim.unwrap();
    assert_eq!(
        (a.sequence, a.forward, a.over, a.start()),
        (1, false, Some(0.2), 1.0)
    );
    assert!(
        (a.time_scale(0.5) + 2.5).abs() < 1e-6,
        "backward, the duration over the timeout"
    );
}

#[test]
fn a_mounted_tool_activates_then_waits_ready() {
    let st = tool();
    assert_eq!(st.fire_state, 2);
    let mut slots: Slots = Default::default();
    let f = |_: u32| Some(tool());
    ap(&mut slots, &one(sent(70, 0, false)), true, Some(1000), &f);
    let m = slots[0].as_ref().unwrap();
    assert_eq!((m.state, m.delay), (0, 0.15));
    // 0.15 s is five ticks (0.16 s): Ready from then on.
    assert_eq!(m.at(Some(&st), Some(1000 + 4 * 32)).state, 0);
    let later = m.at(Some(&st), Some(1000 + 5 * 32));
    assert_eq!(later.state, 1);
    assert!((later.ambient_time - 5.0 * TICK_SEC).abs() < 1e-6);
}

/// A game's rules that differ from TGE's in every answer.
struct Other;

impl ImageRules for Other {
    fn same_state_emitter(&self) -> bool {
        true
    }

    fn emitter_without_datablock(&self) -> bool {
        true
    }

    fn remount_on_skin(&self) -> bool {
        true
    }
}

/// `apply` of one slot under `rules`.
fn ap_with(
    rules: &dyn ImageRules,
    slots: &mut Slots,
    s: ImageSlot,
    now: u32,
    f: &dyn Fn(u32) -> Option<ImageStates>,
) {
    let mut counts = [0, 1, 2, 3].map(|i| slots[i].as_ref().map_or(0, |m| m.fire_count));
    apply(
        rules,
        slots,
        &mut counts,
        &one(s),
        true,
        Some(now),
        f,
        &mut TICK_BUDGET.clone(),
        &mut TickStats::default(),
        &mut Vec::new(),
    );
}

#[test]
fn tick_is_tges() {
    assert_eq!(TICK_SEC, 0.032);
    assert_eq!(TICK_SEC.to_bits(), 0x3d03_126f);
}

#[test]
fn a_state_that_times_out_into_itself_restarts_its_timer_and_its_emitter_only_by_a_games_rule() {
    // Fire: 0.04 s, its timeout back into Fire, its trigger up to Ready; its emitter for 0.07 s at the muzzle.
    let mut r = record(&[
        ("Activate", trans(1, -1, -1), 0.15, true, false, 0),
        ("Ready", trans(-1, 2, -1), 0.0, false, false, 0),
        ("Fire", trans(2, -1, 1), 0.04, false, true, 0),
    ]);
    for (name, value) in [
        ("stateEmitter", Value::Id(Some(77))),
        ("stateEmitterTime", Value::F32(0.07)),
        ("stateEmitterNode", Value::Int(-1)),
    ] {
        r.entries.push(Entry {
            name,
            index: 2,
            value,
        });
    }
    let st = ImageStates::of(&r).unwrap();
    let f = |_: u32| ImageStates::of(&r);
    let emits = |events: &[ImageEvent]| {
        events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ImageEvent::Emitter {
                        datablock: Some(77),
                        node: -1,
                        ..
                    }
                )
            })
            .count()
    };
    for (rules, game) in [(&TgeImageRules as &dyn ImageRules, false), (&Other, true)] {
        let mut slots: Slots = Default::default();
        ap_with(rules, &mut slots, sent(70, 0, false), 0, &f);
        // Activate and Ready name no emitter: TGE asks for none, the game's rule for a dead one each.
        let dead = slots[0]
            .as_ref()
            .unwrap()
            .events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ImageEvent::Emitter {
                        datablock: None,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(dead > 0, game);
        ap_with(rules, &mut slots, sent(70, 1, true), 320, &f);
        let m = slots[0].as_mut().unwrap();
        assert_eq!(m.state, 2);
        assert_eq!(emits(&m.events), 1, "entering Fire starts its emitter");
        m.events.clear();
        // Ten ticks held: Fire re-entered on its timeouts; an emitter event each only by the game's rule.
        let mut later = m.clone();
        later.advance_to(
            rules,
            &st,
            Some(320 + 10 * 32),
            &mut u32::MAX.clone(),
            false,
            &mut TickStats::default(),
        );
        assert_eq!(later.state, 2);
        if game {
            assert!(emits(&later.events) >= 5, "{:?}", later.events);
        } else {
            assert_eq!(emits(&later.events), 0, "{:?}", later.events);
        }
        // No state sound or loop stop on a same-state re-entry.
        assert!(!later.events.contains(&ImageEvent::StopLoop));
    }
}

#[test]
fn a_new_skin_reskins_the_image_and_remounts_it_only_by_a_games_rule() {
    let f = |_: u32| Some(tool());
    for (rules, game) in [(&TgeImageRules as &dyn ImageRules, false), (&Other, true)] {
        let mut slots: Slots = Default::default();
        ap_with(rules, &mut slots, sent(70, 0, false), 0, &f);
        ap_with(rules, &mut slots, sent(70, 0, false), 320, &f);
        assert_eq!(slots[0].as_ref().unwrap().state, 1, "Ready");
        let mut skinned = sent(70, 0, false);
        skinned.skin = NetStr::Text(b"blue".to_vec());
        ap_with(rules, &mut slots, skinned.clone(), 352, &f);
        let m = slots[0].as_ref().unwrap();
        assert_eq!(m.skin, skinned.skin);
        assert_eq!(m.state, if game { 0 } else { 1 });
    }
}

#[test]
fn a_new_fire_count_forces_the_fire_state_and_a_trigger_does_not() {
    let st = tool();
    let f = |_: u32| Some(tool());
    let mut slots: Slots = Default::default();
    ap(&mut slots, &one(sent(70, 0, false)), true, Some(0), &f);
    // The trigger held in Ready: the client does not go to Fire by the transition.
    ap(&mut slots, &one(sent(70, 0, true)), true, Some(320), &f);
    assert_eq!(slots[0].as_ref().unwrap().state, 1);
    ap(&mut slots, &one(sent(70, 1, true)), true, Some(352), &f);
    let m = slots[0].as_ref().unwrap();
    assert_eq!((m.state, m.fire_count), (2, 1));
    assert_eq!(m.at(Some(&st), Some(352 + 7 * 32)).state, 1);
}

#[test]
fn the_fire_count_is_the_slots_through_image_changes_and_an_empty_slot() {
    // `setImage` never writes the slot's count: an image mounted after a shot comes with the count the client holds and goes to
    // Ready, not Fire.
    let f = |_: u32| Some(tool());
    let mut slots: Slots = Default::default();
    let mut counts = [0; IMAGE_SLOTS];
    let run = |slots: &mut Slots, counts: &mut [u32; IMAGE_SLOTS], s: ImageSlot, now: u32| {
        apply(
            &TgeImageRules,
            slots,
            counts,
            &one(s),
            true,
            Some(now),
            &f,
            &mut TICK_BUDGET.clone(),
            &mut TickStats::default(),
            &mut Vec::new(),
        );
    };
    run(&mut slots, &mut counts, sent(70, 0, false), 0);
    run(&mut slots, &mut counts, sent(70, 1, false), 320);
    assert_eq!(slots[0].as_ref().unwrap().state, 2, "the shot");
    // Another tool in the slot, the count unchanged: it activates, never fires.
    run(&mut slots, &mut counts, sent(71, 1, false), 960);
    let m = slots[0].as_ref().unwrap();
    assert_eq!((m.datablock, m.state, m.fire_count), (71, 0, 1));
    // The slot emptied, then a tool again.
    let mut empty = sent(0, 1, false);
    empty.datablock = None;
    run(&mut slots, &mut counts, empty, 1280);
    assert!(slots[0].is_none());
    assert_eq!(counts[0], 1);
    run(&mut slots, &mut counts, sent(70, 1, false), 1600);
    assert_eq!(slots[0].as_ref().unwrap().state, 0);
    // A shot the empty slot saw counts too; a new shot fires.
    let mut empty = sent(0, 2, false);
    empty.datablock = None;
    run(&mut slots, &mut counts, empty, 1920);
    run(&mut slots, &mut counts, sent(71, 2, false), 2240);
    assert_eq!(slots[0].as_ref().unwrap().state, 0);
    run(&mut slots, &mut counts, sent(71, 3, false), 2560);
    assert_eq!(slots[0].as_ref().unwrap().state, 2);
}

#[test]
fn hostile_states_never_loop_or_panic() {
    // A cycle of zero timeouts, transitions past the table and a NaN timeout.
    let mut t0 = N;
    t0[10] = 1;
    let mut t1 = N;
    t1[10] = 0;
    let r = record(&[
        ("a", t0, 0.0, true, false, 0),
        ("b", t1, 0.0, true, false, 0),
        ("d", t0, 0.0, true, false, 2),
        ("c", [30; 11], f32::NAN, false, true, 7),
    ]);
    let st = ImageStates::of(&r).unwrap();
    let f = move |_: u32| Some(st.clone());
    let mut slots: Slots = Default::default();
    ap(&mut slots, &one(sent(9, 5, true)), false, Some(0), &f);
    let m = slots[0].as_ref().unwrap();
    assert!(m.cap_hits > 0);
    let later = m.at(f(9).as_ref(), Some(u32::MAX));
    assert!(later.ambient_time.is_finite() && later.spin_time.is_finite());
    // An unknown datablock clears the slot.
    let none = |_: u32| None;
    ap(&mut slots, &one(sent(9, 5, true)), true, Some(10), &none);
    assert!(slots[0].is_none());
}

#[test]
fn tges_state_members_are_read_as_the_state_arrays() {
    use crate::tge::tge_state_fields;
    let t = Value::Bool(true);
    let f = Value::Bool(false);
    let to = |s: i64| Value::Int(s + 1);
    let none = Value::Int(0);
    let quarter = Value::F32(0.25);
    let seq = Value::Int(0);
    let (into0, into1) = (to(0), to(1));
    // State 0 times out into state 1 after 0.25 s; state 1 fires, plays sequence 0 and goes to state 0 when the trigger is up.
    let fields: Vec<(&str, u32, &Value)> = vec![
        ("state", 0, &t),
        ("transition.loaded", 0, &none),
        ("transition.loaded", 1, &none),
        ("transition.trigger", 0, &none),
        ("transition.trigger", 1, &none),
        ("transition.timeout", 0, &into1),
        ("timeoutValue", 0, &quarter),
        ("fire", 0, &f),
        ("state", 1, &t),
        ("transition.loaded", 2, &none),
        ("transition.loaded", 3, &none),
        ("transition.trigger", 2, &into0),
        ("transition.trigger", 3, &none),
        ("transition.timeout", 1, &none),
        ("fire", 1, &t),
        ("sequence", 0, &seq),
        ("state", 2, &f),
    ];
    let states = ImageStates::of_fields(tge_state_fields(fields));
    assert_eq!(states.states[0].trans[10], 1);
    assert_eq!(states.states[0].timeout, 0.25);
    assert_eq!(states.states[0].sequence, -1);
    assert_eq!(states.states[1].trans[8], 0, "trigger up");
    assert_eq!(states.states[1].trans[10], -1);
    assert!(states.states[1].fire);
    assert_eq!(states.states[1].sequence, 0);
    assert_eq!(states.fire_state, 1);
}

#[test]
fn a_state_with_a_recoil_counts_one_on_the_slot() {
    let mut r = record(&[
        ("Activate", trans(1, -1, -1), 0.15, true, false, 0),
        ("Ready", trans(-1, 2, -1), 0.0, false, false, 0),
        ("Fire", trans(1, -1, -1), 0.2, true, true, 0),
    ]);
    r.entries.push(Entry {
        name: "stateRecoil",
        index: 2,
        value: Value::Int(1),
    });
    let st = ImageStates::of(&r).unwrap();
    assert_eq!(st.states[2].recoil, 1);
    let f = move |_: u32| Some(st.clone());
    let mut slots: Slots = Default::default();
    ap(&mut slots, &one(sent(70, 0, false)), true, Some(0), &f);
    assert_eq!(slots[0].as_ref().unwrap().recoil, None, "no recoil yet");
    // A shot: Fire's recoil, once.
    ap(
        &mut slots,
        &one(sent(70, 1, false)),
        true,
        Some(10 * 32),
        &f,
    );
    assert_eq!(
        slots[0].as_ref().unwrap().recoil,
        Some(ImageRecoil { kind: 1, count: 1 })
    );
}

#[test]
fn looping_state_chains_are_charged_to_the_tick_budget() {
    // Two states with no timeout that point at each other, on 4,096 ghosts of four slots each.
    let mut t0 = N;
    t0[10] = 1;
    let mut t1 = N;
    t1[10] = 0;
    let r = record(&[
        ("a", t0, 0.0, false, false, 0),
        ("b", t1, 0.0, false, false, 0),
    ]);
    let st = ImageStates::of(&r).unwrap();
    let f = move |_: u32| Some(st.clone());
    let mut images = std::collections::BTreeMap::new();
    for g in 0..4_096u32 {
        let mut slots: Slots = Default::default();
        let four = [0, 1, 2, 3].map(|_| Some(sent(70, 0, false)));
        apply(
            &TgeImageRules,
            &mut slots,
            &mut [0; IMAGE_SLOTS],
            &four,
            true,
            Some(0),
            &f,
            &mut u32::MAX.clone(),
            &mut TickStats::default(),
            &mut Vec::new(),
        );
        images.insert(g, slots);
    }
    let mut cursor = 0;
    for now in [32u32, 64, 96] {
        let mut budget = TICK_BUDGET;
        let mut stats = TickStats::default();
        advance_all(
            &TgeImageRules,
            &mut images,
            &mut cursor,
            &f,
            Some(now),
            &mut budget,
            &mut stats,
        );
        let work = stats.run + stats.chain;
        // A step's work stays within the budget and one chain past it.
        assert!(
            work <= u64::from(TICK_BUDGET + MAX_STATE_CHAIN),
            "{stats:?}"
        );
        assert!(stats.chain > 0, "{stats:?}");
    }
}
