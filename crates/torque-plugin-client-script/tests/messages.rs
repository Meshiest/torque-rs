//! Messages and calls between a realm and its server: every cap holds where the bytes arrive, a realm reaches only the RPCs
//! marked for it, and a hostile peer on either end gets nothing past the caps.

mod common;

use std::rc::Rc;

use common::*;
use torque_client_script_format::{Grants, Tier, Value, Writer, event};
use torque_engine::rpc::{Delivery, RpcCarrier, RpcLink, Side};
use torque_ghosts::codec::NetWidths;
use torque_plugin_client_script::messages::Status;
use torque_plugin_client_script::messages::{
    ByteBucket, CHUNK, DOWN, MAX_DOWN_BYTES, MAX_INBOX, MAX_UP_BYTES, OFFER, STATUS, ScriptDown,
    ScriptOffer, ScriptStatus, ScriptUp, UP, UP_BYTES_PER_SECOND,
};
use torque_plugin_client_script::{Choices, ConsentAnswer, RealmState, ScriptInbox, Setting};

fn ui_net() -> Grants {
    Grants::of(&[Tier::Ui, Tier::Net])
}

/// A world whose player allowed `ui` and `net`, with the realm running.
fn running() -> World {
    let mut w = World::new(Setting::Ask, ui_net(), Choices::in_memory());
    w.files.here.set(true);
    w.offer(&["ui", "net"]);
    w.frames(3);
    *w.asked.answer.borrow_mut() = Some(ConsentAnswer::Allow {
        ticked: vec![(Tier::Ui, true), (Tier::Net, true)],
        remember: false,
    });
    w.frames(3);
    assert_eq!(w.realms.state(), RealmState::Running);
    w
}

/// A hostile client: its own link to the server, with no limits of its own.
struct Hostile {
    link: RpcLink,
    tap: Rc<Tap>,
}

impl Hostile {
    fn new(w: &World) -> Hostile {
        let tap = Rc::new(Tap::default());
        let mut link = RpcLink::new(
            w.engine.net_rpcs().expect("the table"),
            Side::Client,
            None,
            NetWidths::engine_default(),
            Rc::clone(&tap) as Rc<dyn RpcCarrier>,
            0,
        );
        link.set_send_limits(false);
        Hostile { link, tap }
    }

    /// Sends `message` and lets the server read it.
    fn send_up(&mut self, w: &mut World, message: ScriptUp) {
        let handle = self.link.rpc::<ScriptUp, ()>(UP).expect("the rpc");
        self.link.notify(&handle, message).expect("queued");
        self.deliver(w);
    }

    fn deliver(&mut self, w: &mut World) {
        self.link.flush();
        for batch in std::mem::take(&mut *self.tap.0.borrow_mut()) {
            w.server.receive(&batch).expect("the server reads it");
        }
        w.run_server();
    }
}

fn chunks_of(bytes: &[u8], count: usize) -> Vec<[u8; 32]> {
    let mut chunks = vec![[0u8; CHUNK]; count];
    for (i, b) in bytes.iter().enumerate() {
        chunks[i / CHUNK][i % CHUNK] = *b;
    }
    chunks
}

/// The bytes of a table nested `depth` deep.
fn nested(depth: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..depth {
        out.extend_from_slice(&[5, 1, 0]);
        out.push(3);
        out.extend_from_slice(&1f64.to_le_bytes());
    }
    out.push(0);
    out
}

#[test]
fn up_messages_past_every_cap_are_refused() {
    let mut w = running();
    // A length past the cap, whatever the chunks; a length and chunks that disagree: too few, too many, none.
    let mut bad: Vec<ScriptUp> = [
        (MAX_UP_BYTES + 1, 128),
        (8191, 128),
        (100, 3),
        (10, 2),
        (5, 0),
    ]
    .into_iter()
    .map(|(len, count)| ScriptUp {
        len: len as u16,
        chunks: vec![[0; CHUNK]; count],
    })
    .collect();
    // Padding that is not zero.
    let mut padded = chunks_of(&[0], 1);
    padded[0][31] = 9;
    bad.push(ScriptUp {
        len: 1,
        chunks: padded,
    });
    // Bytes that are no value: an unknown tag, a cut-off table, a trailing byte, tables nested past the format's depth, a
    // table with more entries than it allows, a key twice.
    for bytes in [
        vec![200u8],
        vec![5, 1, 0],
        vec![0, 0],
        nested(9),
        vec![5, 0x01, 0x01],
        vec![5, 2, 0, 0, 0, 0, 0],
    ] {
        let len = bytes.len();
        bad.push(ScriptUp {
            len: len as u16,
            chunks: chunks_of(&bytes, len.div_ceil(CHUNK)),
        });
    }
    let total = bad.len();
    let mut hostile = Hostile::new(&w);
    for message in bad {
        hostile.send_up(&mut w, message);
    }
    let server = w.script_server();
    assert_eq!(server.refused(), total as u64, "every one refused");
    assert_eq!(server.delivered(), 0);
    assert!(w.got.borrow().is_empty(), "no handler heard any");
    // A message right at the cap is carried.
    let limit = Value::String(vec![b'x'; MAX_UP_BYTES - 5]);
    let fits = ScriptUp::of(&limit).expect("fits exactly");
    assert_eq!(usize::from(fits.len), MAX_UP_BYTES);
    hostile.send_up(&mut w, fits);
    assert_eq!(server.delivered(), 1);
    // One byte more does not encode at all.
    assert!(ScriptUp::of(&Value::String(vec![b'x'; MAX_UP_BYTES - 4])).is_err());
    assert!(
        ScriptUp::of(&Value::Table(
            (0..257)
                .map(|n| (Value::Number(f64::from(n)), Value::Nil))
                .collect()
        ))
        .is_err()
    );
}

#[test]
fn the_server_holds_a_client_to_its_rate_and_to_the_byte_allowance() {
    let mut w = running();
    let mut hostile = Hostile::new(&w);
    // 200 small messages in a tick: the burst (64) is carried, the rest dropped before they are decoded.
    let small = ScriptUp::of(&text("hi")).expect("fits");
    let handle = hostile.link.rpc::<ScriptUp, ()>(UP).expect("the rpc");
    for _ in 0..200 {
        hostile.link.notify(&handle, small.clone()).expect("queued");
    }
    hostile.deliver(&mut w);
    assert_eq!(w.got.borrow().len(), 64, "the burst");
    assert_eq!(w.server.drops(UP), 136, "the rest are counted as dropped");
    // Bytes: 4 KiB messages are held to 64 KiB a second whatever their number.
    let before = w.got.borrow().len();
    w.tick += 100;
    w.server.tick(w.tick);
    let big = ScriptUp::of(&Value::String(vec![b'y'; MAX_UP_BYTES - 5])).expect("fits");
    for n in 0..40 {
        hostile.link.notify(&handle, big.clone()).expect("queued");
        if n % 8 == 7 {
            hostile.deliver(&mut w);
        }
    }
    let server = w.script_server();
    let carried = w.got.borrow().len() - before;
    assert_eq!(
        carried as u64,
        UP_BYTES_PER_SECOND / MAX_UP_BYTES as u64,
        "16 of 40"
    );
    assert_eq!(server.over_bytes(), 24);
    // The allowance comes back with ticks.
    let mut bucket = ByteBucket::new(0);
    assert!(bucket.take(0, UP_BYTES_PER_SECOND as usize));
    assert!(!bucket.take(0, 1));
    assert!(bucket.take(33, 1_000));
}

#[test]
fn a_realm_sends_a_value_and_the_servers_handler_gets_it() {
    let mut w = running();
    let value = table(&[("hp", Value::Number(3.0)), ("who", text("me"))]);
    w.script.flushes.borrow_mut().push_back(send_flush(&value));
    w.frames(3);
    w.up();
    assert_eq!(w.realms.stats().sent, 1);
    let got = w.got.borrow().clone();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0], (CLIENT, value));
}

#[test]
fn a_message_past_the_size_cap_from_a_realm_stops_it_and_sends_nothing() {
    let mut w = running();
    let too_big = Value::String(vec![b'z'; MAX_UP_BYTES + 100]);
    w.script
        .flushes
        .borrow_mut()
        .push_back(send_flush(&too_big));
    w.frames(4);
    w.up();
    assert_eq!(
        w.realms.state(),
        RealmState::Off,
        "the host stopped the realm"
    );
    assert!(w.got.borrow().is_empty());
    assert!(!w.gui.client_script_active(), "and took it down");
}

#[test]
fn down_messages_past_every_cap_are_dropped() {
    let mut w = running();
    let inbox = w.vm.extension::<ScriptInbox>().expect("the inbox");
    let mut server = hostile_server(&w);
    let mut bad: Vec<ScriptDown> = vec![
        ScriptDown {
            len: (MAX_DOWN_BYTES + 1) as u16,
            chunks: vec![[0; CHUNK]; 480],
        },
        ScriptDown {
            len: 16_383,
            chunks: vec![[0; CHUNK]; 480],
        },
        ScriptDown {
            len: 64,
            chunks: vec![[0; CHUNK]; 1],
        },
        ScriptDown {
            len: 1,
            chunks: vec![[1; CHUNK]; 1],
        },
    ];
    for bytes in [vec![99u8], nested(9), vec![5, 0x01, 0x01]] {
        let len = bytes.len();
        bad.push(ScriptDown {
            len: len as u16,
            chunks: chunks_of(&bytes, len.div_ceil(CHUNK)),
        });
    }
    let total = bad.len() as u64;
    for message in bad {
        server.send(&mut w, message);
    }
    assert_eq!(inbox.refused(), total, "every one refused");
    assert!(
        inbox.take_values(w.rpc.epoch()).is_empty(),
        "none reached the realm"
    );
    // At the cap one is carried; past the queue the oldest go.
    let limit = ScriptDown::of(&Value::String(vec![b'q'; MAX_DOWN_BYTES - 5])).expect("fits");
    server.send(&mut w, limit);
    assert_eq!(inbox.take_values(w.rpc.epoch()).len(), 1);
    for round in 0..3u32 {
        w.tick += 40;
        w.server.tick(w.tick);
        for n in 0..100 {
            let value = Value::Number(f64::from(round * 100 + n));
            server.send(&mut w, ScriptDown::of(&value).expect("fits"));
        }
    }
    assert_eq!(inbox.dropped(), 300 - MAX_INBOX as u64);
    let kept = inbox.take_values(w.rpc.epoch());
    assert_eq!(kept.len(), MAX_INBOX);
    assert_eq!(kept[0], Value::Number(44.0), "the oldest 44 were dropped");
}

/// The server's end that can send what an honest one would not.
struct HostileServer;

fn hostile_server(_w: &World) -> HostileServer {
    HostileServer
}

impl HostileServer {
    fn send(&mut self, w: &mut World, message: ScriptDown) {
        // The server's own link would refuse a message past its type's caps; ones within them but wrong are the hostile ones.
        w.server.set_send_limits(false);
        let handle = w.server.rpc::<ScriptDown, ()>(DOWN).expect("the rpc");
        w.server.notify(&handle, message).expect("queued");
        w.server.flush();
        w.down();
    }
}

#[test]
fn the_client_holds_the_server_to_its_down_rate() {
    let mut w = running();
    let mut server = hostile_server(&w);
    let inbox = w.vm.extension::<ScriptInbox>().expect("the inbox");
    for n in 0..300 {
        let value = Value::Number(f64::from(n));
        w.server.set_send_limits(false);
        let handle = w.server.rpc::<ScriptDown, ()>(DOWN).expect("the rpc");
        w.server
            .notify(&handle, ScriptDown::of(&value).expect("fits"))
            .expect("queued");
    }
    w.server.flush();
    w.down();
    let _ = &mut server;
    // 64 a second with a burst of 128: the rest of the tick's messages were dropped by the client's link.
    assert_eq!(inbox.take_values(w.rpc.epoch()).len(), 128);
    assert!(w.rpc.with_link(|link| link.drops(DOWN)).expect("a link") >= 172);
}

#[test]
fn a_server_value_reaches_a_realm_that_holds_net_and_only_that_one() {
    let mut w = running();
    w.send_down(&text("hello"));
    w.frames(2);
    let saw = |w: &World| {
        w.script.events.borrow().iter().any(|batch| {
            batch.contains(&event::net::MESSAGE) && batch.windows(5).any(|s| s == b"hello")
        })
    };
    assert!(saw(&w), "the realm was told");
    // A realm that was not given `net` is told nothing.
    let mut ui_only = World::new(Setting::Ask, ui_net(), Choices::in_memory());
    ui_only.files.here.set(true);
    ui_only.offer(&["ui", "net"]);
    ui_only.frames(3);
    *ui_only.asked.answer.borrow_mut() = Some(ConsentAnswer::Allow {
        ticked: vec![(Tier::Ui, true), (Tier::Net, false)],
        remember: false,
    });
    ui_only.frames(3);
    assert_eq!(ui_only.realms.state(), RealmState::Running);
    ui_only.send_down(&text("hello"));
    ui_only.frames(2);
    assert!(!saw(&ui_only));
    // And its own sends are an op of a tier it does not hold: the host stops it.
    ui_only
        .script
        .flushes
        .borrow_mut()
        .push_back(send_flush(&text("x")));
    ui_only.frames(4);
    assert_eq!(ui_only.realms.state(), RealmState::Off);
    ui_only.up();
    assert!(ui_only.got.borrow().is_empty());
}

#[test]
fn an_rpc_without_the_realm_mark_is_refused() {
    let mut w = running();
    // The player's answer went to the server; from here on, anything on the wire would be the realm's.
    w.up();
    let args = table(&[("data", text("crate"))]);
    // Four calls the realm may not make: an RPC that is not marked, one of this crate's own, a server-to-client one, one that is
    // not there. Each is answered `refused` locally and none reaches the wire.
    let mut all = Writer::new();
    for (id, name) in [
        (1u32, "props.delete"),
        (2, "torque.script.up"),
        (3, "hud.note"),
        (4, "props.nothing"),
    ] {
        let mark = all.begin(Tier::Net.id(), torque_client_script_format::op::net::CALL);
        all.u32(id);
        all.text(name);
        args.encode(&mut all).expect("a value");
        all.end(mark).expect("fits");
    }
    w.script.flushes.borrow_mut().push_back(all.into_bytes());
    w.frames(4);
    assert_eq!(w.realms.state(), RealmState::Running);
    assert_eq!(w.realms.stats().refused, 4, "all four refused");
    let _ = &args;
    assert!(w.lanes.take_out(None).is_empty(), "nothing went out");
    assert!(w.spawns.borrow().is_empty());
    // The realm was told, for each call, that it was refused.
    w.frames(2);
    let replies = w
        .script
        .events
        .borrow()
        .iter()
        .filter(|b| b.windows(7).any(|s| s == b"refused"))
        .count();
    assert!(replies >= 1, "the guest is told `refused`");
}

#[test]
fn a_marked_rpc_is_called_with_arguments_that_fit_and_answers_with_its_reply() {
    let mut w = running();
    let args = table(&[
        ("data", text("crate")),
        (
            "at",
            Value::Table(vec![
                (Value::Number(1.0), Value::Number(1.0)),
                (Value::Number(2.0), Value::Number(2.0)),
                (Value::Number(3.0), Value::Number(4.5)),
            ]),
        ),
    ]);
    w.script
        .flushes
        .borrow_mut()
        .push_back(call_flush(1, "props.spawn", &args));
    w.frames(4);
    w.up();
    assert_eq!(
        *w.spawns.borrow(),
        [Spawn {
            data: "crate".into(),
            at: [1.0, 2.0, 4.5],
        }]
    );
    // The server's reply goes back to the realm as the answer to call 1.
    w.down();
    w.frames(3);
    assert!(
        w.script
            .events
            .borrow()
            .iter()
            .any(|b| b.contains(&event::net::REPLY) && b.windows(5).any(|s| s == b"crate")),
        "the guest got the reply"
    );
}

#[test]
fn calls_are_refused_unless_they_fit_the_rpc() {
    let mut w = running();
    let nan = Value::Number(f64::NAN);
    let long = Value::String(vec![b'a'; 300]);
    let vector17 = Value::Table(
        (1..=17)
            .map(|n| (Value::Number(f64::from(n)), Value::Number(1.0)))
            .collect(),
    );
    let nested_table = Value::Table(vec![(
        Value::Number(1.0),
        Value::Table(vec![(Value::Number(1.0), Value::Number(1.0))]),
    )]);
    let bad: Vec<Value> = vec![
        table(&[("color", text("red"))]),
        table(&[("data", long)]),
        table(&[("data", text("a\0b"))]),
        table(&[("at", vector17)]),
        table(&[("at", nested_table)]),
        table(&[("at", Value::Table(vec![(Value::Number(1.0), nan.clone())]))]),
        table(&[("data", nan)]),
        Value::Number(1.0),
        Value::String(b"props".to_vec()),
        Value::Table(vec![(Value::Number(1.0), text("x"))]),
    ];
    let mut all = Writer::new();
    for (id, args) in bad.iter().enumerate() {
        let mark = all.begin(Tier::Net.id(), torque_client_script_format::op::net::CALL);
        all.u32(id as u32 + 1);
        all.text("props.spawn");
        args.encode(&mut all).expect("a value");
        all.end(mark).expect("fits");
    }
    w.script.flushes.borrow_mut().push_back(all.into_bytes());
    w.frames(4);
    assert_eq!(w.realms.state(), RealmState::Running);
    assert_eq!(w.realms.stats().refused, bad.len() as u64, "all refused");
    w.up();
    assert!(w.spawns.borrow().is_empty());
}

#[test]
fn calls_past_the_rpcs_rate_get_rate_limited_and_the_rest_are_carried() {
    let mut w = running();
    let mut all = Writer::new();
    for id in 1..=16u32 {
        let mark = all.begin(Tier::Net.id(), torque_client_script_format::op::net::CALL);
        all.u32(id);
        all.text("props.spawn");
        table(&[("data", text("c"))])
            .encode(&mut all)
            .expect("a value");
        all.end(mark).expect("fits");
    }
    w.script.flushes.borrow_mut().push_back(all.into_bytes());
    w.frames(4);
    w.up();
    // The RPC's own bucket is 4 a second with a burst of 8: 8 of the 16 went out.
    assert_eq!(w.spawns.borrow().len(), 8);
    w.frames(3);
    let limited = w
        .script
        .events
        .borrow()
        .iter()
        .map(|b| b.windows(12).filter(|s| *s == b"rate limited").count())
        .sum::<usize>();
    assert_eq!(limited, 8, "the other 8 were told `rate limited`");
}

#[test]
fn a_realm_cannot_spend_more_than_the_connections_other_rpcs_allow() {
    // The realm's sends share the connection's bucket with every other RPC, so a flood of them leaves the rest their share: the
    // server link counts what it drops by RPC.
    let mut w = running();
    let mut hostile = Hostile::new(&w);
    let handle = hostile.link.rpc::<ScriptUp, ()>(UP).expect("the rpc");
    let small = ScriptUp::of(&Value::Nil).expect("fits");
    for _ in 0..1000 {
        hostile.link.notify(&handle, small.clone()).expect("queued");
    }
    hostile.deliver(&mut w);
    assert!(w.server.drops(UP) >= 900);
    // Another RPC of the same connection still gets through.
    let status = hostile
        .link
        .rpc::<ScriptStatus, ()>(STATUS)
        .expect("the rpc");
    hostile
        .link
        .notify(
            &status,
            ScriptStatus::of(&torque_plugin_client_script::messages::Status::Declined),
        )
        .expect("queued");
    hostile.deliver(&mut w);
    assert_eq!(w.status(), Some(Status::Declined));
}

#[test]
fn a_hostile_status_says_nothing_it_should_not() {
    let mut w = running();
    let mut hostile = Hostile::new(&w);
    let handle = hostile
        .link
        .rpc::<ScriptStatus, ()>(STATUS)
        .expect("the rpc");
    // A state no client sends is refused; tiers that are not plain words are left out of an allowed answer.
    hostile
        .link
        .notify(
            &handle,
            ScriptStatus {
                state: 3,
                tiers: "ui".into(),
            },
        )
        .expect("queued");
    hostile.deliver(&mut w);
    assert_eq!(w.status(), None);
    hostile
        .link
        .notify(
            &handle,
            ScriptStatus {
                state: 2,
                tiers: "ui,;drop table,NET,,net".into(),
            },
        )
        .expect("queued");
    hostile.deliver(&mut w);
    assert_eq!(w.status(), Some(Status::Allowed(vec![Tier::Ui, Tier::Net])));
}

#[test]
fn an_offer_with_anything_wrong_is_refused_whole() {
    let good = ScriptOffer {
        ids: vec!["hud".into()],
        hashes: vec![[1; 32]],
        entries: vec!["client/main.lua".into()],
        tiers: vec!["ui,net".into()],
    };
    assert_eq!(good.mods().expect("fine").len(), 1);
    let mut cases: Vec<ScriptOffer> = Vec::new();
    for edit in 0..9 {
        let mut offer = good.clone();
        match edit {
            0 => offer.ids[0] = "../etc".into(),
            1 => offer.entries[0] = "../main.lua".into(),
            2 => offer.entries[0] = "client/main.cs".into(),
            3 => offer.entries[0] = "C:/main.lua".into(),
            4 => offer.tiers[0] = "ui,UI".into(),
            5 => offer.hashes.clear(),
            6 => {
                offer.ids.push("hud".into());
                offer.hashes.push([2; 32]);
                offer.entries.push("a.lua".into());
                offer.tiers.push(String::new());
            }
            7 => offer.entries[0] = "client\\main.lua".into(),
            _ => offer.tiers[0] = vec!["a"; 17].join(","),
        }
        cases.push(offer);
    }
    for offer in &cases {
        assert!(offer.mods().is_err(), "{offer:?}");
    }
    // 17 mods is past the offer.
    let many = ScriptOffer {
        ids: (0..17).map(|n| format!("m{n}")).collect(),
        hashes: vec![[0; 32]; 17],
        entries: vec!["a.lua".into(); 17],
        tiers: vec![String::new(); 17],
    };
    assert!(many.mods().is_err());
    // A client takes one offer a connection and none that is wrong.
    let mut w = World::new(Setting::Ask, ui_net(), Choices::in_memory());
    let inbox = w.vm.extension::<ScriptInbox>().expect("the inbox");
    let handle = w.server.rpc::<ScriptOffer, ()>(OFFER).expect("the rpc");
    w.server.notify(&handle, cases[0].clone()).expect("queued");
    w.server.flush();
    w.down();
    assert!(inbox.take_offer(w.rpc.epoch()).is_none());
    assert_eq!(inbox.refused(), 1);
}

fn random_value(next: &mut impl FnMut() -> u64, depth: usize) -> Value {
    match next() % if depth >= 3 { 4 } else { 6 } {
        0 => Value::Nil,
        1 => Value::Bool(next() & 1 == 1),
        2 => Value::Number((next() % 1000) as f64 / 8.0),
        3 => Value::String(vec![b'a' + (next() % 26) as u8; (next() % 40) as usize]),
        _ => Value::Table(
            (0..(next() % 5) as usize)
                .map(|n| (Value::Number(n as f64 + 1.0), random_value(next, depth + 1)))
                .collect(),
        ),
    }
}

#[test]
fn message_decoding_never_panics_and_what_it_accepts_is_within_caps() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let (mut accepted, mut refused) = (0u32, 0u32);
    for case in 0..200_000u32 {
        let bytes = if next() % 2 == 0 {
            // A value that is right, then wronged: bytes changed, cut, extended, the length off by one.
            let mut bytes = random_value(&mut next, 0)
                .to_bytes()
                .expect("a value encodes");
            for _ in 0..(next() % 3) {
                if !bytes.is_empty() {
                    let at = (next() as usize) % bytes.len();
                    bytes[at] = next() as u8;
                }
            }
            match next() % 4 {
                0 => bytes.truncate((next() as usize) % (bytes.len() + 1)),
                1 => bytes.extend((0..(next() % 5)).map(|_| next() as u8)),
                _ => {}
            }
            bytes
        } else {
            (0..(next() % 300)).map(|_| next() as u8).collect()
        };
        let mut len = bytes.len();
        match next() % 8 {
            0 => len += 1,
            1 => len = len.saturating_sub(1),
            2 => len = (next() % 9000) as usize,
            _ => {}
        }
        let count = if next() % 8 == 0 {
            (next() % 140) as usize
        } else {
            len.div_ceil(CHUNK).min(500)
        };
        let mut chunks = vec![[0u8; CHUNK]; count];
        for (i, b) in bytes.iter().enumerate() {
            if i / CHUNK < count {
                chunks[i / CHUNK][i % CHUNK] = *b;
            }
        }
        let up = ScriptUp {
            len: len.min(8191) as u16,
            chunks: chunks.iter().copied().take(128).collect(),
        };
        match up.value() {
            Ok(value) => {
                accepted += 1;
                let again = value.to_bytes().expect("an accepted value encodes again");
                assert!(again.len() <= MAX_UP_BYTES, "case {case}");
            }
            Err(_) => refused += 1,
        }
        let down = ScriptDown {
            len: len.min(16_383) as u16,
            chunks: chunks.iter().copied().take(480).collect(),
        };
        if let Ok(value) = down.value() {
            assert!(
                value.to_bytes().expect("encodes").len() <= MAX_DOWN_BYTES,
                "case {case}"
            );
        }
    }
    assert!(
        accepted > 10_000 && refused > 10_000,
        "{accepted} accepted, {refused} refused: both paths are reached"
    );
    let _ = Delivery::Reliable;
}
