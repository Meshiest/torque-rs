//! The authority suite: nothing a realm does changes the server except through the RPCs the server validates. A hostile realm
//! that makes no `net` calls leaves what the server sees (every byte the client sends it over 600 frames, and the server's
//! script state) the same as an idle realm of the same grants; one that makes calls changes the server only inside the
//! handlers a game registered, which the test counts.

mod common;

use common::*;
use torque_client_script_format::ui::{Kind, Prop, PropValue, write_props};
use torque_client_script_format::{Grants, Tier, Value, Writer, op};
use torque_plugin_client_script::{Choices, ConsentAnswer, RealmState, Setting};

const FRAMES: usize = 600;

/// A 64-bit FNV-1a fold of bytes.
struct Fold(u64);

impl Fold {
    fn new() -> Fold {
        Fold(0xcbf2_9ce4_8422_2325)
    }

    fn put(&mut self, bytes: &[u8]) {
        for b in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// A world whose player allowed exactly `tiers`, with the realm running.
fn running(tiers: &[Tier]) -> World {
    let offered: Vec<&str> = tiers.iter().map(|t| t.name()).collect();
    let mut w = World::new(
        Setting::Ask,
        Grants::of(&[Tier::Ui, Tier::Net]),
        Choices::in_memory(),
    );
    w.files.here.set(true);
    w.offer(&offered);
    w.frames(3);
    *w.asked.answer.borrow_mut() = Some(ConsentAnswer::Allow {
        ticked: tiers.iter().map(|t| (*t, true)).collect(),
        remember: false,
    });
    w.frames(3);
    assert_eq!(w.realms.state(), RealmState::Running, "{tiers:?}");
    w
}

/// What the server saw of one run: a fold of every byte the client sent, and the server's script state.
#[derive(Debug, PartialEq, Eq)]
struct Seen {
    upstream: u64,
    batches: usize,
    server_state: Vec<u8>,
}

/// Runs `FRAMES` frames, carrying what the client queues to the server and what the server queues back.
fn drive(w: &mut World) -> Seen {
    let mut fold = Fold::new();
    let mut batches = 0;
    for _ in 0..FRAMES {
        w.frame();
        for (batch, _) in w.lanes.take_out(None) {
            fold.put(&batch);
            batches += 1;
            w.server.receive(&batch).expect("the server reads it");
        }
        w.run_server();
        w.down();
    }
    Seen {
        upstream: fold.0,
        batches,
        server_state: w
            .server_vm
            .eval(b"return getVariableDump();")
            .expect("the dump"),
    }
}

fn record(out: &mut Writer, kind: u8, body: impl FnOnce(&mut Writer)) {
    let mark = out.begin(Tier::Ui.id(), kind);
    body(out);
    out.end(mark).expect("the record fits");
}

fn props(w: &mut Writer, list: &[(Prop, PropValue)]) {
    let mut sorted: Vec<&(Prop, PropValue)> = list.iter().collect();
    sorted.sort_by_key(|(p, _)| *p as u8);
    write_props(w, sorted.into_iter().map(|(p, v)| (*p, v)));
}

/// The flushes of a hostile realm that keeps to its caps and to `ui`: a root, then a frame's worth of creates, changes,
/// reorderings and destroys of its own controls, some carrying text that looks like script.
fn hostile_ui(frames: usize) -> Vec<Vec<u8>> {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut live: Vec<u32> = Vec::new();
    let mut next = 2u32;
    let mut flushes = Vec::new();
    for frame in 0..frames {
        let mut out = Writer::new();
        if frame == 0 {
            record(&mut out, op::ui::ROOT, |w| {
                w.u32(1);
                w.text("mod");
            });
            record(&mut out, op::ui::INTERACTIVE, |w| w.bool(true));
        }
        for _ in 0..1 + rng.below(40) {
            let pick = rng.below(8);
            if live.len() < 8 || (pick == 0 && live.len() < 400) {
                let text = if rng.below(4) == 0 {
                    "$ClientScript::Canary = 1;"
                } else {
                    "hello"
                };
                let kind = [Kind::Panel, Kind::Text, Kind::Button][rng.below(3) as usize];
                let mut list = vec![
                    (
                        Prop::Pos,
                        PropValue::Vec2(rng.below(600) as f64, rng.below(400) as f64),
                    ),
                    (Prop::Size, PropValue::Vec2(20.0, 10.0)),
                ];
                if kind != Kind::Panel {
                    list.push((Prop::Text, PropValue::Text(text.to_string())));
                }
                record(&mut out, op::ui::CREATE, |w| {
                    w.u32(next);
                    w.u32(1);
                    w.u8(kind as u8);
                    props(w, &list);
                });
                live.push(next);
                next += 1;
                continue;
            }
            let handle = live[rng.below(live.len() as u64) as usize];
            match pick {
                1..=4 => record(&mut out, op::ui::SET, |w| {
                    w.u32(handle);
                    props(
                        w,
                        &[
                            (
                                Prop::Pos,
                                PropValue::Vec2(rng.below(600) as f64, rng.below(400) as f64),
                            ),
                            (
                                Prop::Opacity,
                                PropValue::Number(rng.below(100) as f64 / 100.0),
                            ),
                        ],
                    );
                }),
                5 => record(&mut out, op::ui::ORDER, |w| {
                    w.u32(handle);
                    w.bool(rng.below(2) == 0);
                }),
                _ => {
                    record(&mut out, op::ui::DESTROY, |w| w.u32(handle));
                    live.retain(|h| *h != handle);
                }
            }
        }
        flushes.push(out.into_bytes());
    }
    flushes
}

#[test]
fn a_hostile_realm_that_makes_no_calls_changes_nothing_the_server_sees() {
    let sets: [&[Tier]; 3] = [&[Tier::Ui], &[Tier::Net], &[Tier::Ui, Tier::Net]];
    for tiers in sets {
        // The same grants, an idle realm.
        let mut idle = running(tiers);
        let baseline = drive(&mut idle);

        let mut hostile = running(tiers);
        // The run of the entry file is the first flush, then one a frame.
        hostile
            .script
            .flushes
            .borrow_mut()
            .extend(hostile_ui(FRAMES + 1));
        let seen = drive(&mut hostile);
        assert_eq!(seen, baseline, "{tiers:?}: the server saw a difference");
        // A set without `ui` stops the realm at its first op; with it the realm ran on, hostile to the end.
        if tiers.contains(&Tier::Ui) {
            assert_eq!(hostile.realms.state(), RealmState::Running, "{tiers:?}");
            assert!(
                hostile.gui.client_script_active(),
                "{tiers:?}: the realm's controls were on screen"
            );
        } else {
            assert_eq!(hostile.realms.state(), RealmState::Off, "{tiers:?}");
        }
        let server = hostile.script_server();
        assert_eq!(
            (server.delivered(), server.refused(), server.over_bytes()),
            (0, 0, 0),
            "{tiers:?}"
        );
        assert!(hostile.got.borrow().is_empty());
        assert!(hostile.spawns.borrow().is_empty());
        println!(
            "authority {tiers:?}: {FRAMES} frames, {} client batches both ways, server unchanged",
            seen.batches
        );
    }
}

#[test]
fn with_calls_the_server_changes_only_inside_the_handlers_it_registered() {
    let mut idle = running(&[Tier::Ui, Tier::Net]);
    let baseline = drive(&mut idle);

    let mut w = running(&[Tier::Ui, Tier::Net]);
    // Each frame the realm sends and calls: a message, a call of an RPC marked for it (`props.spawn`), one that is not
    // (`props.delete`), one of the plugin's own (`torque.script.up`) and one that does not exist.
    let mut flushes = Vec::new();
    // A call's id is the one after the last the realm used.
    let mut id = 0u32;
    for frame in 0..FRAMES + 1 {
        let mut out = Writer::new();
        let spawn = table(&[("data", text("crate"))]);
        let mut call = |out: &mut Writer, name: &str, args: &Value| {
            id += 1;
            let mark = out.begin(Tier::Net.id(), op::net::CALL);
            out.u32(id);
            out.text(name);
            args.encode(out).expect("a value");
            out.end(mark).expect("fits");
        };
        // At most one send and three calls a frame, within the rates over a run of frames; the host refuses what is over.
        if frame % 12 == 0 {
            let mark = out.begin(Tier::Net.id(), op::net::SEND);
            table(&[("frame", Value::Number(frame as f64))])
                .encode(&mut out)
                .expect("a value");
            out.end(mark).expect("fits");
            call(&mut out, "props.spawn", &spawn);
            call(&mut out, "props.delete", &spawn);
            call(&mut out, "torque.script.up", &spawn);
            call(&mut out, "no.such.rpc", &spawn);
        }
        flushes.push(out.into_bytes());
    }
    w.script.flushes.borrow_mut().extend(flushes);
    let seen = drive(&mut w);
    assert_eq!(w.realms.state(), RealmState::Running);

    let server = w.script_server();
    let sent = w.realms.stats().sent;
    // Every message the server delivered reached the registered handler, once, and nothing it refused did.
    assert!(sent >= 40, "the realm sent {sent}");
    assert_eq!(
        server.delivered(),
        w.got.borrow().len() as u64,
        "each delivered message ran the handler once"
    );
    assert_eq!(
        server.delivered() + server.refused() + server.over_bytes(),
        sent,
        "each message was delivered or counted refused"
    );
    // `props.spawn` ran its handler for the calls the link let through, `props.delete` and the rest never did.
    let spawned = w.spawns.borrow().len() as u64;
    assert!(spawned >= 40, "{spawned} spawns");
    assert!(spawned <= FRAMES as u64 / 12 + 1);
    assert!(
        w.realms.stats().refused >= 3 * (FRAMES as u64 / 12),
        "the three calls that are no realm's were refused: {:?}",
        w.realms.stats()
    );
    // And the server's script state is what an idle realm leaves: no handler wrote a global.
    assert_eq!(seen.server_state, baseline.server_state);
    println!(
        "authority with calls: {sent} messages sent, {} delivered to the handler, {spawned} spawns, {} calls refused",
        server.delivered(),
        w.realms.stats().refused
    );
}
