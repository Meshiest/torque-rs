//! The GUI's work for a realm is the realm's: applying its `ui` ops is counted in its frame budget, its slow rule and its
//! strikes, a flush past the budget is spread over frames, a flush is checked whole before any of it applies, and the layouts
//! its ops ask for are laid out once a frame, not once an op.

mod common;

use std::time::Duration;

use common::*;
use torque_client_script_format::ui::{Kind, Prop, PropValue, write_props};
use torque_client_script_format::{Grants, Tier, Writer, op};
use torque_client_script_host::FrameBudget;
use torque_plugin_client_script::{Choices, ConsentAnswer, RealmState, Setting};

fn record(out: &mut Writer, kind: u8, body: impl FnOnce(&mut Writer)) {
    let mark = out.begin(Tier::Ui.id(), kind);
    body(out);
    out.end(mark).expect("the record fits");
}

fn create(out: &mut Writer, handle: u32, parent: u32, kind: Kind, props: &[(Prop, PropValue)]) {
    record(out, op::ui::CREATE, |w| {
        w.u32(handle);
        w.u32(parent);
        w.u8(kind as u8);
        let mut sorted: Vec<&(Prop, PropValue)> = props.iter().collect();
        sorted.sort_by_key(|(p, _)| *p as u8);
        write_props(w, sorted.into_iter().map(|(p, v)| (*p, v)));
    });
}

fn size(w: f64, h: f64) -> (Prop, PropValue) {
    (Prop::Size, PropValue::Vec2(w, h))
}

fn root(out: &mut Writer) {
    record(out, op::ui::ROOT, |w| {
        w.u32(1);
        w.text("hud");
    });
}

/// A world whose realm was allowed `ui` and `net`, laid out by the layout plugin when `layout`, and whose guest's first
/// flushes are `flushes`.
fn running(layout: bool, flushes: Vec<Vec<u8>>, setup: impl FnOnce(&mut World)) -> World {
    let mut w = World::with_layout(
        Setting::Ask,
        Grants::of(&[Tier::Ui, Tier::Net]),
        Choices::in_memory(),
        layout,
    );
    setup(&mut w);
    w.files.here.set(true);
    w.script.flushes.borrow_mut().extend(flushes);
    w.offer(&["ui", "net"]);
    w.frames(3);
    *w.asked.answer.borrow_mut() = Some(ConsentAnswer::Allow {
        ticked: vec![(Tier::Ui, true), (Tier::Net, true)],
        remember: false,
    });
    w.frame();
    w
}

/// One client frame: the realms, then the GUI's own frame (the layers and the layouts).
fn client_frame(w: &mut World) {
    w.frame();
    w.gui.frame(&mut w.vm);
}

/// The objects of the realm's control classes alive.
fn realm_objects(w: &World) -> usize {
    let objects = w.vm.objects();
    objects
        .iter()
        .filter(|(_, o)| objects.classes.name(o.class()).starts_with(b"ClientScript"))
        .count()
}

/// A frame's churn: a panel with `count` panels in it, made and destroyed, `rounds` times; handles from `next`.
fn churn(next: &mut u32, rounds: u32, count: u32) -> Vec<u8> {
    let mut out = Writer::new();
    for _ in 0..rounds {
        let parent = *next;
        create(&mut out, parent, 1, Kind::Panel, &[size(600.0, 400.0)]);
        for k in 1..=count {
            create(&mut out, parent + k, parent, Kind::Panel, &[size(4.0, 4.0)]);
        }
        record(&mut out, op::ui::DESTROY, |w| w.u32(parent));
        *next += count + 1;
    }
    out.into_bytes()
}

fn first_flush() -> Vec<u8> {
    let mut out = Writer::new();
    root(&mut out);
    out.into_bytes()
}

#[test]
fn applying_past_the_budget_waits_for_later_frames_and_makes_the_realm_slow() {
    // A budget that one piece of ops uses up: each frame applies one piece, and the guest runs again only once its
    // flush has applied whole.
    let mut next = 2;
    let mut flushes = vec![first_flush()];
    flushes.extend((0..200).map(|_| churn(&mut next, 1, 254)));
    let mut w = running(false, flushes, |w| {
        w.realms.settings.budget = FrameBudget::Shared {
            max: Duration::from_nanos(1),
            share: 1.0,
        };
        w.realms.settings.long_entry = Duration::from_secs(5);
    });
    assert_eq!(w.realms.state(), RealmState::Running);
    let ran = |w: &World| w.script.events.borrow().len();
    let started = ran(&w);
    for _ in 0..30 {
        client_frame(&mut w);
    }
    let early = ran(&w) - started;
    // 256 ops a flush are four pieces: at most one guest frame every four client frames.
    assert!(
        early <= 30 / 4 + 1,
        "the guest ran {early} times in 30 frames"
    );
    assert!(early >= 3, "the realm still runs ({early})");
    for _ in 0..60 {
        client_frame(&mut w);
    }
    let late = ran(&w) - started - early;
    // Over its budget every frame, the realm is slow and applies every other frame: half the pace.
    assert!(
        late <= 60 / 8 + 1,
        "the guest ran {late} times in 60 slow frames"
    );
    assert_eq!(w.realms.state(), RealmState::Running);
}

#[test]
fn applying_that_holds_the_frame_too_long_is_a_strike_and_turns_the_realm_off() {
    let mut next = 2;
    let mut flushes = vec![first_flush()];
    flushes.extend((0..60).map(|_| churn(&mut next, 5, 760)));
    let mut w = running(false, flushes, |w| {
        w.realms.settings.long_entry = Duration::from_nanos(1);
    });
    for _ in 0..10 {
        client_frame(&mut w);
    }
    assert_eq!(w.realms.state(), RealmState::Off, "three strikes");
    assert_eq!(realm_objects(&w), 0, "and everything it made is gone");
    assert!(!w.gui.client_script_active());
}

#[test]
fn a_flush_whose_last_op_is_forged_applies_nothing_and_sends_nothing() {
    let mut out = Writer::new();
    let mark = out.begin(Tier::Net.id(), op::net::SEND);
    text("before the ui").encode(&mut out).expect("a value");
    out.end(mark).expect("fits");
    root(&mut out);
    for k in 0..600 {
        create(&mut out, 2 + k, 1, Kind::Panel, &[]);
    }
    record(&mut out, op::ui::DESTROY, |w| w.u32(90_000));
    let mut w = running(false, vec![out.into_bytes()], |_| {});
    for _ in 0..5 {
        client_frame(&mut w);
    }
    w.up();
    assert_eq!(w.realms.state(), RealmState::Off);
    assert_eq!(realm_objects(&w), 0, "no control of the flush was made");
    assert!(
        w.got.borrow().is_empty(),
        "the send of that flush never left"
    );
    assert_eq!(w.realms.stats().sent, 0);
}

#[test]
fn many_resizes_of_a_nested_container_are_laid_out_once_a_frame_and_charged_to_the_realm() {
    let mut out = Writer::new();
    root(&mut out);
    create(&mut out, 2, 1, Kind::Flex, &[size(600.0, 400.0)]);
    for k in 0..250 {
        create(&mut out, 3 + k, 2, Kind::Panel, &[size(20.0, 10.0)]);
    }
    create(&mut out, 253, 2, Kind::Flex, &[size(40.0, 40.0)]);
    create(&mut out, 254, 253, Kind::Panel, &[size(10.0, 10.0)]);
    let mut flushes = vec![out.into_bytes()];
    for frame in 0..60 {
        let mut out = Writer::new();
        for k in 0..2_000 {
            let side = if (frame + k) % 2 == 0 { 50.0 } else { 60.0 };
            record(&mut out, op::ui::SET, |w| {
                w.u32(253);
                write_props(w, [(Prop::Size, &PropValue::Vec2(side, side))].into_iter());
            });
        }
        flushes.push(out.into_bytes());
    }
    // The whole frame for the realm, so that a debug build applies a flush in a few frames.
    let mut w = running(true, flushes, |w| {
        w.realms.settings.budget = FrameBudget::Shared {
            max: Duration::from_secs(1),
            share: 1.0,
        };
    });
    assert_eq!(w.realms.state(), RealmState::Running);
    client_frame(&mut w);
    let mut before = w.gui.client_script_stats();
    let mut most = 0;
    for _ in 0..60 {
        client_frame(&mut w);
        let now = w.gui.client_script_stats();
        most = most.max(now.layout_updates - before.layout_updates);
        before = now;
    }
    assert!(
        most <= 2,
        "the outer and the inner container checked at most once a frame ({most})"
    );
    // Every flush the guest made before its last applied whole, each of its resizes deferred.
    let flushes = w.script.events.borrow().len() as u64;
    assert!(flushes >= 4, "the realm ran ({flushes} frames)");
    assert!(
        before.deferred_layouts >= 2_000 * (flushes - 2),
        "{} resizes deferred in {flushes} flushes",
        before.deferred_layouts
    );
    assert_eq!(w.realms.state(), RealmState::Running);
    // The realms' next frame charges the GUI's time for the realm (its layouts) to it.
    w.frame();
    assert_eq!(w.gui.take_client_script_owed(), Duration::ZERO);
}

/// A frame's realm time (the realms' frame: the entry, the whole-flush check and the applying) for `frames` frames.
fn realm_times(w: &mut World, frames: usize) -> Vec<Duration> {
    (0..frames)
        .map(|_| {
            let started = std::time::Instant::now();
            w.frame();
            let took = started.elapsed();
            w.gui.frame(&mut w.vm);
            took
        })
        .collect()
}

/// 1,500 `set{selected}` a frame on a list of 1,024 rows: a set costs the host what it carries, not the list's rows, so no
/// frame holds the realm past three budgets, and once the slow rule is on it averages under one. Natively and with the web's
/// budget (the guest's time is the scripted runner's, none). The budgets are a debug build's.
#[test]
fn a_storm_of_selections_on_a_full_list_stays_within_its_budget() {
    let budget = Duration::from_millis(15);
    for setting in [
        FrameBudget::Shared {
            max: budget,
            share: 1.0,
        },
        FrameBudget::ApplyOnly { cap: budget },
    ] {
        let mut out = Writer::new();
        root(&mut out);
        let rows: Vec<String> = (0..1024).map(|i| format!("row {i:011}")).collect();
        create(
            &mut out,
            2,
            1,
            Kind::List,
            &[(Prop::Rows, PropValue::Rows(rows))],
        );
        let mut flushes = vec![out.into_bytes()];
        for frame in 0..120 {
            let mut out = Writer::new();
            for k in 0..1_500u32 {
                record(&mut out, op::ui::SET, |w| {
                    w.u32(2);
                    let selected = PropValue::Int(1 + (frame + k) % 2);
                    write_props(w, [(Prop::Selected, &selected)].into_iter());
                });
            }
            flushes.push(out.into_bytes());
        }
        let mut w = running(false, flushes, |w| {
            w.realms.settings.budget = setting;
            w.realms.settings.long_entry = Duration::from_secs(1);
        });
        assert_eq!(w.realms.state(), RealmState::Running);
        let times = realm_times(&mut w, 120);
        assert_eq!(w.realms.state(), RealmState::Running, "{setting:?}");
        let worst = times.iter().max().copied().unwrap_or_default();
        assert!(
            worst <= budget * 3,
            "{setting:?}: a frame held the realm {worst:?}"
        );
        let late = &times[60..];
        let mean = late.iter().sum::<Duration>() / late.len() as u32;
        assert!(mean <= budget, "{setting:?}: {mean:?} a frame on average");
    }
}
