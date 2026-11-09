//! The host's rules without a guest: a scripted runner hands the session replies a hostile guest might send.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use torque_client_script_format::{
    Grants, MAX_EVENTS, Records, Tier, Value, Writer, abi, event, net as fnet, op,
};

use crate::content::{self, ClientMod, Refused, ServerContent};
use crate::effects::{CallRoute, Effects, LogLevel, Refusal, Stop, Violation};
use crate::events::EventQueue;
use crate::flush::{self, Action, LogRate, NetCheck, Tiers, UI_CHUNK};
use crate::grants::{GrantSet, REVIEWED_TIERS, unknown_tiers};
use crate::runner::{Entry, Outcome, Reply, Runner, Trap};
use crate::session::Session;
use crate::settings::{FrameBudget, Settings};

const UI_NET: Grants = Grants(0b111);
const FRAME: Duration = Duration::from_micros(16_667);

/// A record of `tier` and `kind` whose body `body` writes.
fn record(out: &mut Writer, tier: Tier, kind: u8, body: impl FnOnce(&mut Writer)) {
    let mark = out.begin(tier.id(), kind);
    body(out);
    out.end(mark).expect("a short record");
}

fn flush_of(records: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut out = Writer::new();
    records(&mut out);
    out.into_bytes()
}

fn log(out: &mut Writer, line: &str) {
    record(out, Tier::Realm, op::realm::LOG, |w| {
        w.u8(op::realm::LEVEL_INFO);
        w.text(line);
    });
}

fn call(out: &mut Writer, id: u32, name: &str) {
    record(out, Tier::Net, op::net::CALL, |w| {
        w.u32(id);
        w.text(name);
        Value::Nil.encode(w).expect("nil");
    });
}

fn ui_root(out: &mut Writer, handle: u32) {
    record(out, Tier::Ui, op::ui::ROOT, |w| {
        w.u32(handle);
        w.text("m");
    });
}

fn all_granted() -> Tiers {
    Tiers {
        granted: UI_NET,
        revoked: Grants(0),
        told: Grants(0),
    }
}

/// [`flush::check`] with effects that pass every `ui` record.
fn check_with(
    flush: &[u8],
    tiers: Tiers,
    now_us: u64,
    net: &mut NetCheck,
    logs: &mut LogRate,
) -> Result<Vec<Action>, Violation> {
    flush::check(flush, tiers, now_us, net, logs, &mut Seen::default())
}

fn check(flush: &[u8]) -> Result<Vec<Action>, Violation> {
    check_with(
        flush,
        all_granted(),
        0,
        &mut NetCheck::default(),
        &mut LogRate::default(),
    )
}

#[test]
fn malformed_records_unknown_tiers_and_kinds_are_violations() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("a record cut short", vec![0, 0, 9, 0, 0, 0, 1]),
        (
            "tier 10",
            flush_of(|o| record(o, Tier::Realm, 0, |_| {}))
                .into_iter()
                .enumerate()
                .map(|(i, b)| if i == 0 { 10 } else { b })
                .collect(),
        ),
        (
            "a state op",
            flush_of(|o| record(o, Tier::State, 0, |_| {})),
        ),
        (
            "realm kind 9",
            flush_of(|o| record(o, Tier::Realm, 9, |_| {})),
        ),
        ("ui kind 99", flush_of(|o| record(o, Tier::Ui, 99, |_| {}))),
        ("net kind 7", flush_of(|o| record(o, Tier::Net, 7, |_| {}))),
        (
            "a log line with a byte left over",
            flush_of(|o| {
                record(o, Tier::Realm, op::realm::LOG, |w| {
                    w.u8(0);
                    w.text("x");
                    w.u8(0);
                })
            }),
        ),
        (
            "a log level of 3",
            flush_of(|o| {
                record(o, Tier::Realm, op::realm::LOG, |w| {
                    w.u8(3);
                    w.text("x");
                })
            }),
        ),
        (
            "a hidden character",
            flush_of(|o| log(o, "fake\u{202e}line")),
        ),
        ("a newline", flush_of(|o| log(o, "two\nlines"))),
        ("a line too long", flush_of(|o| log(o, &"x".repeat(513)))),
        (
            "a limit of no kind",
            flush_of(|o| {
                record(o, Tier::Realm, op::realm::LIMIT, |w| {
                    w.u8(2);
                    w.u8(0);
                })
            }),
        ),
    ];
    let tiers = Tiers {
        granted: Grants(Grants::VALID),
        ..all_granted()
    };
    for (what, flush) in cases {
        let checked = check_with(
            &flush,
            tiers,
            0,
            &mut NetCheck::default(),
            &mut LogRate::default(),
        );
        assert!(checked.is_err(), "{what} was let through");
    }
}

#[test]
fn ungranted_ops_stop_and_revoked_ones_are_dropped_until_the_guest_was_told() {
    let flush = flush_of(|o| ui_root(o, 1));
    let base = Tiers {
        granted: Grants(1),
        ..all_granted()
    };
    assert!(
        check_with(
            &flush,
            base,
            0,
            &mut NetCheck::default(),
            &mut LogRate::default()
        )
        .is_err()
    );
    let revoked = Tiers {
        revoked: Grants(0).with(Tier::Ui),
        ..all_granted()
    };
    let dropped = check_with(
        &flush,
        revoked,
        0,
        &mut NetCheck::default(),
        &mut LogRate::default(),
    );
    assert_eq!(dropped, Ok(Vec::new()));
    let told = Tiers {
        told: revoked.revoked,
        ..revoked
    };
    assert!(
        check_with(
            &flush,
            told,
            0,
            &mut NetCheck::default(),
            &mut LogRate::default()
        )
        .is_err()
    );
}

#[test]
fn ui_records_pass_on_in_order_in_pieces_before_the_rest() {
    let flush = flush_of(|o| {
        for h in 1..=UI_CHUNK as u32 + 10 {
            ui_root(o, h);
        }
        log(o, "between");
        ui_root(o, 999);
    });
    let actions = check(&flush).expect("checked");
    let counts: Vec<String> = actions
        .iter()
        .map(|a| match a {
            Action::Ui(bytes) => Records::new(bytes, usize::MAX, usize::MAX)
                .expect("records")
                .count()
                .to_string(),
            Action::Log(_, line) => line.clone(),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        counts,
        [UI_CHUNK.to_string(), "11".into(), "between".into()]
    );
}

#[test]
fn calls_keep_their_order_names_and_caps() {
    let go = |flush: Vec<u8>, net: &mut NetCheck| {
        check_with(&flush, all_granted(), 0, net, &mut LogRate::default()).map(|_| ())
    };
    let mut net = NetCheck::default();
    assert!(
        go(flush_of(|o| call(o, 2, "a")), &mut net).is_err(),
        "the first id is 1"
    );
    assert!(
        go(flush_of(|o| call(o, 1, "../x")), &mut net).is_err(),
        "a name that is not plain"
    );
    for id in 1..=fnet::MAX_OUTSTANDING as u32 {
        go(flush_of(|o| call(o, id, "a")), &mut net).expect("within the cap");
    }
    let next = fnet::MAX_OUTSTANDING as u32 + 1;
    assert!(
        go(flush_of(|o| call(o, next, "a")), &mut net).is_err(),
        "17 waiting on one name"
    );
    assert!(net.answered(1));
    assert!(!net.answered(1), "answered once");
    go(flush_of(|o| call(o, next, "a")), &mut net).expect("room again");
    assert!(
        go(flush_of(|o| call(o, next, "b")), &mut net).is_err(),
        "an id used twice"
    );
}

#[test]
fn sends_keep_their_size_and_rate() {
    let send = |o: &mut Writer, bytes: usize| {
        record(o, Tier::Net, op::net::SEND, |w| {
            Value::String(vec![b'x'; bytes])
                .encode(w)
                .expect("a string");
        });
    };
    assert!(
        check(&flush_of(|o| send(o, fnet::MAX_SEND_BYTES))).is_err(),
        "past its size"
    );
    let mut net = NetCheck::default();
    let mut logs = LogRate::default();
    let burst = flush_of(|o| (0..fnet::SEND_BURST).for_each(|_| send(o, 1)));
    check_with(&burst, all_granted(), 0, &mut net, &mut logs).expect("the burst");
    let one = flush_of(|o| send(o, 1));
    assert!(
        check_with(&one, all_granted(), 0, &mut net, &mut logs).is_err(),
        "past the burst"
    );
    let mut net = NetCheck::default();
    check_with(&burst, all_granted(), 0, &mut net, &mut logs).expect("the burst");
    check_with(&one, all_granted(), 1_000_000, &mut net, &mut logs)
        .expect("refilled a second later");
}

#[test]
fn the_hosts_log_cap_drops_lines_without_stopping() {
    let flush = flush_of(|o| (0..40).for_each(|i| log(o, &format!("line {i}"))));
    let actions = check(&flush).expect("not a violation");
    assert_eq!(actions.len(), 21);
}

#[test]
fn events_keep_the_newest_state_and_drop_input_before_messages() {
    let mut queue = EventQueue::default();
    let hover = |h: u32, on: bool| {
        flush_of(|o| {
            record(o, Tier::Ui, event::ui::HOVER, |w| {
                w.u32(h);
                w.bool(on);
            })
        })
    };
    assert_eq!(queue.ui_batch(&hover(1, true)), 0);
    assert_eq!(queue.ui_batch(&hover(2, true)), 0);
    assert_eq!(queue.ui_batch(&hover(1, false)), 0);
    assert_eq!(queue.len(), 2, "one hover a control");
    let bad = flush_of(|o| record(o, Tier::Ui, event::ui::CLICK, |w| w.u8(1)));
    assert_eq!(queue.ui_batch(&bad), 1, "a click without its handle");
    let other = flush_of(|o| record(o, Tier::Net, event::net::MESSAGE, |w| w.u8(0)));
    assert_eq!(
        queue.ui_batch(&other),
        1,
        "only ui events come from the GUI"
    );
    queue.revoked(Tier::Net);
    let batch = queue.take_batch();
    let records = Records::new(&batch, usize::MAX, usize::MAX)
        .and_then(Records::collect_all)
        .expect("a batch");
    let order: Vec<(u8, u8)> = records.iter().map(|r| (r.tier, r.kind)).collect();
    assert_eq!(
        order,
        [
            (0, event::realm::REVOKED),
            (1, event::ui::HOVER),
            (1, event::ui::HOVER)
        ]
    );

    let mut queue = EventQueue::default();
    for i in 0..MAX_EVENTS {
        assert!(queue.net(event::net::MESSAGE, |_| {}, &[&Value::Number(i as f64)]));
    }
    assert_eq!(queue.ui_batch(&hover(9, true)), 0);
    assert_eq!(queue.len(), MAX_EVENTS, "a message made room");
    let batch = queue.take_batch();
    let records = Records::new(&batch, usize::MAX, usize::MAX)
        .and_then(Records::collect_all)
        .expect("a batch");
    assert_eq!(records.len(), MAX_EVENTS);
    let overflows = records
        .iter()
        .filter(|r| r.tier == Tier::Net.id() && r.kind == event::net::OVERFLOW)
        .count();
    assert_eq!(overflows, 1);
}

#[test]
fn scripts_come_only_from_the_servers_list_within_the_caps() {
    let file = |path: &str, text: &[u8]| (path.to_string(), Arc::<[u8]>::from(text));
    let content = ServerContent::new([
        file("mods/m/main.lua", b"x = 1"),
        file("mods/m/lib/util.luau", b"y = 2"),
        file("mods/m/image.png", b"\x89PNG"),
        file("mods/m/compiled.lua", b"\x1bLua"),
        file("mods/m/up/../main.lua", b"z = 3"),
        file("mods/mx/main.lua", b"other mod"),
        file("mods/n/main.lua", b"not asked for"),
    ]);
    let mods = [
        ClientMod {
            id: "m".into(),
            entry: "main.lua".into(),
        },
        ClientMod {
            id: "../m".into(),
            entry: "main.lua".into(),
        },
        ClientMod {
            id: "gone".into(),
            entry: "main.lua".into(),
        },
    ];
    let (kept, refused) = content::scripts(&content, &mods);
    let kept: Vec<&str> = kept.iter().map(|s| &*s.path).collect();
    assert_eq!(kept, ["lib/util.luau", "main.lua"]);
    assert!(refused.contains(&Refused::ModId("../m".into())));
    assert!(refused.contains(&Refused::Entry {
        mod_id: "gone".into(),
        path: "main.lua".into()
    }));
    for path in ["compiled.lua", "up/../main.lua"] {
        assert!(
            refused.contains(&Refused::File {
                mod_id: "m".into(),
                path: path.into()
            }),
            "{path}"
        );
    }
    let big: Vec<(String, Arc<[u8]>)> = (0..abi::MAX_SCRIPTS + 5)
        .map(|i| file(&format!("mods/m/f{i:03}.lua"), b"--"))
        .collect();
    let (kept, _) = content::scripts(&ServerContent::new(big), &mods[..1]);
    assert_eq!(kept.len(), abi::MAX_SCRIPTS);
}

#[test]
fn a_request_gets_only_reviewed_tiers_the_player_allowed() {
    let all = Grants(Grants::VALID);
    let asked = ["ui", "net", "telepathy"];
    assert_eq!(GrantSet::from_request(asked, all).grants(), REVIEWED_TIERS);
    assert_eq!(GrantSet::from_request(asked, Grants(0)).grants(), Grants(1));
    assert_eq!(unknown_tiers(asked), ["telepathy"]);
    let unreviewed = GrantSet::unreviewed(all).grants();
    assert_eq!(
        unreviewed, UI_NET,
        "only the tiers this build has host modules of"
    );
}

/// What a session did to the engine.
#[derive(Default)]
struct Seen {
    logs: Vec<String>,
    ui: Vec<Vec<u8>>,
    revoked: Vec<Tier>,
    teardowns: u32,
    stops: Vec<Stop>,
    refuse_ui: bool,
    refuse_check: bool,
    route: Option<CallRoute>,
    apply_cost: Duration,
    sends: usize,
}

impl Effects for Seen {
    fn log(&mut self, _level: LogLevel, line: &str) {
        self.logs.push(line.into());
    }

    fn check_ui(&mut self, _records: &[u8]) -> Result<(), Violation> {
        if self.refuse_check {
            return Err(Violation::Ui("the GUI refused it"));
        }
        Ok(())
    }

    fn apply_ui(&mut self, records: &[u8]) -> Result<(), Violation> {
        std::thread::sleep(self.apply_cost);
        self.ui.push(records.to_vec());
        if self.refuse_ui {
            return Err(Violation::Ui("the GUI could not apply it"));
        }
        Ok(())
    }

    fn send(&mut self, _message: Value) {
        self.sends += 1;
    }

    fn call(&mut self, _id: u32, _name: &str, _args: Value) -> CallRoute {
        self.route.unwrap_or(CallRoute::Sent)
    }

    fn revoke(&mut self, tier: Tier) {
        self.revoked.push(tier);
    }

    fn teardown(&mut self) {
        self.teardowns += 1;
    }

    fn stopped(&mut self, why: &Stop) {
        self.stops.push(why.clone());
    }
}

/// What the scripted guest saw.
#[derive(Default)]
struct Log {
    entries: Vec<Entry>,
    dropped: bool,
}

/// A guest whose replies are written out: each entry takes the next (an empty `OK` when there are none). With `hold`
/// it answers only when released, as a Worker does.
struct Scripted {
    replies: VecDeque<Outcome>,
    log: Rc<RefCell<Log>>,
    hold: bool,
    held: Option<Outcome>,
    released: Rc<RefCell<bool>>,
}

impl Runner for Scripted {
    fn start(&mut self, entry: Entry) {
        self.log.borrow_mut().entries.push(entry);
        self.held = Some(
            self.replies
                .pop_front()
                .unwrap_or_else(|| Ok(Reply::default())),
        );
    }

    fn poll(&mut self) -> Option<Outcome> {
        if self.hold && !std::mem::take(&mut *self.released.borrow_mut()) {
            return None;
        }
        self.held.take()
    }
}

impl Drop for Scripted {
    fn drop(&mut self) {
        self.log.borrow_mut().dropped = true;
    }
}

struct Rig {
    session: Session<Seen>,
    log: Rc<RefCell<Log>>,
    release: Rc<RefCell<bool>>,
    now: u64,
}

impl Rig {
    /// A session over a scripted guest; its replies follow the startup's (`New` and one file run).
    fn new(grants: Grants, replies: Vec<Outcome>, hold: bool, settings: Settings) -> Rig {
        let log = Rc::new(RefCell::new(Log::default()));
        let release = Rc::new(RefCell::new(false));
        let mut all = VecDeque::from([
            Ok(Reply::default()),
            Ok(Reply::default()),
            Ok(Reply::default()),
        ]);
        all.extend(replies);
        let runner = Scripted {
            replies: all,
            log: Rc::clone(&log),
            hold,
            held: None,
            released: Rc::clone(&release),
        };
        let content = ServerContent::new([(
            "mods/m/main.lua".to_string(),
            Arc::<[u8]>::from(&b"x = 1"[..]),
        )]);
        let mods = [ClientMod {
            id: "m".into(),
            entry: "main.lua".into(),
        }];
        let session = Session::open(
            Box::new(runner),
            Seen::default(),
            GrantSet::unreviewed(grants),
            &content,
            &mods,
            settings,
        );
        Rig {
            session,
            log,
            release,
            now: 0,
        }
    }

    fn frame(&mut self) {
        self.now += 16_667;
        self.session.frame(self.now, FRAME);
    }

    fn seen(&self) -> &Seen {
        self.session.effects()
    }

    fn seen_mut(&mut self) -> &mut Seen {
        self.session.effects_mut()
    }

    fn entries(&self) -> Vec<&'static str> {
        self.log
            .borrow()
            .entries
            .iter()
            .map(|e| match e {
                Entry::New(_) => "new",
                Entry::AddScript { .. } => "add",
                Entry::Run { .. } => "run",
                Entry::Frame { .. } => "frame",
            })
            .collect()
    }
}

fn flush_reply(records: impl FnOnce(&mut Writer)) -> Outcome {
    Ok(Reply {
        status: abi::OK,
        ops: flush_of(records),
        ..Reply::default()
    })
}

#[test]
fn startup_makes_the_realm_keeps_the_scripts_runs_the_entry_then_frames() {
    let mut rig = Rig::new(Grants(1), Vec::new(), false, Settings::native());
    rig.frame();
    rig.frame();
    assert_eq!(rig.entries(), ["new", "add", "run", "frame", "frame"]);
    let times: Vec<u64> = rig
        .log
        .borrow()
        .entries
        .iter()
        .filter_map(|e| match e {
            Entry::Run { now_us, .. } | Entry::Frame { now_us, .. } => Some(*now_us),
            _ => None,
        })
        .collect();
    assert!(
        times.windows(2).all(|w| w[0] <= w[1]) && times[0] == 0,
        "{times:?}"
    );
}

#[test]
fn a_violation_stops_the_realm_and_throws_its_instance_away() {
    let mut rig = Rig::new(
        Grants(1),
        vec![flush_reply(|o| ui_root(o, 1))],
        false,
        Settings::native(),
    );
    rig.frame();
    rig.frame();
    let (stop, _) = rig.session.stop().expect("stopped");
    assert!(matches!(stop, Stop::Violation(_)));
    assert!(rig.log.borrow().dropped, "the instance is gone");
    assert_eq!((rig.seen().teardowns, rig.seen().stops.len()), (1, 1));
    assert!(rig.seen().ui.is_empty(), "nothing of the flush applied");
    let count = rig.entries().len();
    rig.frame();
    assert_eq!(rig.entries().len(), count, "nothing runs again");
}

#[test]
fn the_guis_refusal_and_a_trap_stop_the_realm() {
    let mut rig = Rig::new(
        UI_NET,
        vec![flush_reply(|o| ui_root(o, 1))],
        false,
        Settings::native(),
    );
    rig.seen_mut().refuse_ui = true;
    rig.frame();
    rig.frame();
    assert!(matches!(
        rig.session.stop(),
        Some((
            Stop::Violation(Violation::Ui("the GUI could not apply it")),
            _
        ))
    ));
    let mut rig = Rig::new(
        Grants(1),
        vec![Err(Trap::Memory)],
        false,
        Settings::native(),
    );
    rig.frame();
    rig.frame();
    assert_eq!(
        rig.session.stop().map(|(s, _)| s.clone()),
        Some(Stop::Trapped(Trap::Memory))
    );
}

#[test]
fn a_guest_refusing_the_hosts_calls_stops_the_realm() {
    let refused = Ok(Reply {
        status: abi::MALFORMED,
        ..Reply::default()
    });
    let mut rig = Rig::new(Grants(1), vec![refused], false, Settings::native());
    rig.frame();
    rig.frame();
    assert!(matches!(
        rig.session.stop(),
        Some((Stop::Refused(Refusal::Frame), _))
    ));
}

#[test]
fn three_limit_stops_within_ten_seconds_stop_the_realm() {
    let limit = || {
        flush_reply(|o| {
            record(o, Tier::Realm, op::realm::LIMIT, |w| {
                w.u8(op::realm::LIMIT_STEPS);
                w.u8(op::realm::ENTRY_FRAME);
            })
        })
    };
    let mut rig = Rig::new(
        Grants(1),
        vec![limit(), limit(), limit()],
        false,
        Settings::native(),
    );
    for _ in 0..3 {
        rig.frame();
    }
    assert_eq!(
        rig.session.stop().map(|(s, _)| s.clone()),
        Some(Stop::Strikes)
    );
}

#[test]
fn revoking_takes_effect_at_once_and_drops_what_waits() {
    let mut rig = Rig::new(UI_NET, Vec::new(), false, Settings::native());
    rig.frame();
    let click = flush_of(|o| record(o, Tier::Ui, event::ui::CLICK, |w| w.u32(1)));
    assert_eq!(rig.session.ui_events(&click), 0);
    assert!(rig.session.revoke(Tier::Ui));
    assert_eq!(rig.seen().revoked, [Tier::Ui], "in the same frame");
    assert!(!rig.session.revoke(Tier::Ui), "only once");
    assert!(!rig.session.revoke(Tier::Realm), "never the base");
    assert_eq!(rig.session.ui_events(&click), 0);
    rig.frame();
    let log = rig.log.borrow();
    let Some(Entry::Frame { events, .. }) = log.entries.last() else {
        panic!("a frame ran");
    };
    let records = Records::new(events, usize::MAX, usize::MAX)
        .and_then(Records::collect_all)
        .expect("a batch");
    let kinds: Vec<(u8, u8)> = records.iter().map(|r| (r.tier, r.kind)).collect();
    assert_eq!(
        kinds,
        [(Tier::Realm.id(), event::realm::REVOKED)],
        "the click went with the tier"
    );
}

#[test]
fn ops_of_an_entry_that_started_before_a_revocation_are_dropped_not_violations() {
    let mut rig = Rig::new(
        UI_NET,
        vec![flush_reply(|o| ui_root(o, 1))],
        true,
        Settings::web(),
    );
    // The startup, one entry a frame as on the web.
    for _ in 0..3 {
        rig.frame();
        *rig.release.borrow_mut() = true;
        rig.session.poll();
    }
    rig.frame();
    assert!(rig.session.is_waiting(), "a frame is in flight");
    rig.session.revoke(Tier::Ui);
    *rig.release.borrow_mut() = true;
    rig.frame();
    assert!(rig.session.stop().is_none(), "{:?}", rig.session.stop());
    assert!(rig.seen().ui.is_empty());
}

#[test]
fn a_call_the_server_did_not_mark_is_answered_refused_and_replies_need_a_waiting_call() {
    let mut rig = Rig::new(
        UI_NET,
        vec![flush_reply(|o| call(o, 1, "nope"))],
        false,
        Settings::native(),
    );
    rig.seen_mut().route = Some(CallRoute::Refused);
    rig.frame();
    rig.frame();
    assert!(!rig.session.reply(1, true, &Value::Nil), "already answered");
    rig.frame();
    let log = rig.log.borrow();
    let refused = log.entries.iter().any(|e| {
        let Entry::Frame { events, .. } = e else {
            return false;
        };
        Records::new(events, usize::MAX, usize::MAX)
            .expect("records")
            .filter_map(Result::ok)
            .any(|r| r.tier == Tier::Net.id() && r.kind == event::net::REPLY)
    });
    assert!(refused, "the guest got its refusal");
}

#[test]
fn applying_past_the_web_budget_waits_for_the_next_frame() {
    let ui = flush_reply(|o| {
        for h in 1..=(UI_CHUNK * 3) as u32 {
            ui_root(o, h);
        }
    });
    let settings = Settings {
        budget: FrameBudget::ApplyOnly {
            cap: Duration::from_millis(1),
        },
        ..Settings::native()
    };
    let mut rig = Rig::new(UI_NET, vec![ui], false, settings);
    rig.seen_mut().apply_cost = Duration::from_millis(2);
    rig.frame();
    rig.frame();
    let applied = rig.seen().ui.len();
    assert!(applied < 3, "the budget held ({applied} pieces)");
    for _ in 0..3 {
        rig.frame();
    }
    assert_eq!(rig.seen().ui.len(), 3, "the rest came later");
}

#[test]
fn closing_tears_down_once_and_drops_the_instance() {
    let mut rig = Rig::new(UI_NET, Vec::new(), false, Settings::native());
    rig.frame();
    let log = Rc::clone(&rig.log);
    let seen = rig.session.close();
    assert_eq!(seen.teardowns, 1);
    assert!(log.borrow().dropped);
}

/// A send, then a `ui` op.
fn send_then_ui() -> Outcome {
    flush_reply(|o| {
        record(o, Tier::Net, op::net::SEND, |w| {
            Value::Nil.encode(w).expect("nil");
        });
        ui_root(o, 1);
    })
}

#[test]
fn a_flush_whose_ui_is_refused_sends_nothing() {
    let mut rig = Rig::new(UI_NET, vec![send_then_ui()], false, Settings::native());
    rig.seen_mut().refuse_check = true;
    rig.frame();
    rig.frame();
    assert!(matches!(
        rig.session.stop(),
        Some((Stop::Violation(Violation::Ui(_)), _))
    ));
    assert_eq!((rig.seen().sends, rig.seen().ui.len()), (0, 0));
}

#[test]
fn a_flush_whose_ui_fails_to_apply_sends_nothing() {
    let mut rig = Rig::new(UI_NET, vec![send_then_ui()], false, Settings::native());
    rig.seen_mut().refuse_ui = true;
    rig.frame();
    rig.frame();
    assert!(rig.session.stop().is_some());
    assert_eq!(rig.seen().sends, 0, "the send waited for the ui ops");
}

#[test]
fn entries_past_the_long_entry_time_are_strikes() {
    let long = || {
        Ok(Reply {
            elapsed: Duration::from_millis(30),
            ..Reply::default()
        })
    };
    let flagged = || {
        Ok(Reply {
            long: true,
            ..Reply::default()
        })
    };
    let mut rig = Rig::new(
        Grants(1),
        vec![long(), flagged()],
        false,
        Settings::native(),
    );
    rig.frame();
    rig.frame();
    assert!(rig.session.stop().is_none(), "two strikes");
    let mut rig = Rig::new(
        Grants(1),
        vec![long(), flagged(), long()],
        false,
        Settings::native(),
    );
    for _ in 0..3 {
        rig.frame();
    }
    assert_eq!(
        rig.session.stop().map(|(s, _)| s.clone()),
        Some(Stop::Strikes)
    );
    assert_eq!(rig.session.longest_entry(), Duration::from_millis(30));
}

/// What a dropped session left behind, seen from outside it.
#[derive(Default)]
struct Left {
    controls: usize,
    waiting_calls: usize,
}

/// Effects that keep their state where a test can see it after the session is gone.
struct Probe(Rc<RefCell<Left>>);

impl Effects for Probe {
    fn log(&mut self, _level: LogLevel, _line: &str) {}

    fn check_ui(&mut self, _records: &[u8]) -> Result<(), Violation> {
        Ok(())
    }

    fn apply_ui(&mut self, records: &[u8]) -> Result<(), Violation> {
        let count = Records::new(records, usize::MAX, usize::MAX)
            .expect("records")
            .count();
        self.0.borrow_mut().controls += count;
        Ok(())
    }

    fn send(&mut self, _message: Value) {}

    fn call(&mut self, _id: u32, _name: &str, _args: Value) -> CallRoute {
        self.0.borrow_mut().waiting_calls += 1;
        CallRoute::Sent
    }

    fn revoke(&mut self, _tier: Tier) {}

    fn teardown(&mut self) {
        *self.0.borrow_mut() = Left::default();
    }

    fn stopped(&mut self, _why: &Stop) {}
}

#[test]
fn dropping_a_session_tears_it_down() {
    let left = Rc::new(RefCell::new(Left::default()));
    let runner = Scripted {
        replies: VecDeque::from([
            Ok(Reply::default()),
            flush_reply(|o| {
                ui_root(o, 1);
                call(o, 1, "wait");
            }),
        ]),
        log: Rc::default(),
        hold: false,
        held: None,
        released: Rc::default(),
    };
    let mut session = Session::open(
        Box::new(runner),
        Probe(Rc::clone(&left)),
        GrantSet::unreviewed(UI_NET),
        &ServerContent::default(),
        &[],
        Settings::native(),
    );
    session.frame(16_667, FRAME);
    session.frame(33_333, FRAME);
    assert_eq!(
        (left.borrow().controls, left.borrow().waiting_calls),
        (1, 1)
    );
    drop(session);
    assert_eq!(
        (left.borrow().controls, left.borrow().waiting_calls),
        (0, 0)
    );
}

#[test]
fn the_hosts_own_calls_are_never_strikes() {
    let slow = || {
        Ok(Reply {
            elapsed: Duration::from_millis(400),
            long: true,
            ..Reply::default()
        })
    };
    let runner = Scripted {
        // Making the realm and keeping its file, each slow three times over: neither is the script's time.
        replies: VecDeque::from([slow(), slow(), Ok(Reply::default())]),
        log: Rc::default(),
        hold: false,
        held: None,
        released: Rc::default(),
    };
    let content = ServerContent::new([(
        "mods/m/main.lua".to_string(),
        Arc::<[u8]>::from(&b"x = 1"[..]),
    )]);
    let mods = [ClientMod {
        id: "m".into(),
        entry: "main.lua".into(),
    }];
    let mut session = Session::open(
        Box::new(runner),
        Seen::default(),
        GrantSet::unreviewed(Grants(1)),
        &content,
        &mods,
        Settings::native(),
    );
    for frame in 1..=5 {
        session.frame(frame * 16_667, FRAME);
    }
    assert!(session.stop().is_none());
    assert_eq!(session.longest_entry(), Duration::ZERO);
}
