//! The client-script realm natively (`--features client-script`): the escape suite, fuel and memory, tiers and
//! revocation, the ui mirror and its ops, net messages and calls, and the frame's order and budget. The guest
//! (`just lua-client-web`) is this same code built for wasm32-wasip1.
#![cfg(feature = "client-script")]

use torque_lua::client_script::format::abi::{self, Config};
use torque_lua::client_script::format::ui::{self, Kind, Prop, PropValue};
use torque_lua::client_script::format::{
    self, Grants, Reader, Records, Tier, Value, Writer, event, op,
};
use torque_lua::client_script::{ClientRealm, STOPPED};

/// The escape suite's script.
const ESCAPE: &str = include_str!("scripts/client_script_escape.lua");

const MOD: &str = "m";

/// One op of a flush.
#[derive(Clone, Debug)]
struct Op {
    tier: u8,
    kind: u8,
    body: Vec<u8>,
}

fn realm_of(config: Config) -> ClientRealm {
    ClientRealm::new(config).expect("the realm is made")
}

fn realm(tiers: &[Tier]) -> ClientRealm {
    realm_of(Config {
        grants: Grants::of(tiers),
        ..Config::default()
    })
}

/// A realm with the base, `ui` and `net`.
fn full() -> ClientRealm {
    realm(&[Tier::Ui, Tier::Net])
}

fn ops(realm: &ClientRealm) -> Vec<Op> {
    realm.with_ops(|bytes| {
        Records::new(bytes, format::MAX_OPS, format::MAX_FLUSH_BYTES)
            .unwrap()
            .map(|r| {
                let r = r.unwrap();
                Op {
                    tier: r.tier,
                    kind: r.kind,
                    body: r.body.to_vec(),
                }
            })
            .collect()
    })
}

/// The log lines of the last entry.
fn logs(realm: &ClientRealm) -> Vec<String> {
    ops(realm)
        .iter()
        .filter(|o| o.tier == Tier::Realm.id() && o.kind == op::realm::LOG)
        .map(|o| {
            let mut r = Reader::new(&o.body);
            r.u8().unwrap();
            r.text().unwrap().to_string()
        })
        .collect()
}

/// The limit ops of the last entry: (which, entry).
fn limits(realm: &ClientRealm) -> Vec<(u8, u8)> {
    ops(realm)
        .iter()
        .filter(|o| o.tier == Tier::Realm.id() && o.kind == op::realm::LIMIT)
        .map(|o| (o.body[0], o.body[1]))
        .collect()
}

/// Keeps `source` as the mod's `path` and runs it at `now`.
fn run_at(realm: &mut ClientRealm, now: u64, path: &str, source: &str) {
    realm.add_script(MOD, path, source.as_bytes()).unwrap();
    realm.run(now, MOD, path).unwrap();
}

fn run(realm: &mut ClientRealm, source: &str) {
    run_at(realm, 0, "main.lua", source);
}

/// A batch of events.
#[derive(Default)]
struct Batch(Writer);

impl Batch {
    fn record(mut self, tier: Tier, kind: u8, body: impl FnOnce(&mut Writer)) -> Batch {
        let mark = self.0.begin(tier.id(), kind);
        body(&mut self.0);
        self.0.end(mark).unwrap();
        self
    }

    fn click(self, handle: u32) -> Batch {
        self.record(Tier::Ui, event::ui::CLICK, |w| w.u32(handle))
    }

    fn press(self, handle: u32) -> Batch {
        self.record(Tier::Ui, event::ui::PRESS, |w| w.u32(handle))
    }

    fn change(self, handle: u32, value: &Value) -> Batch {
        self.record(Tier::Ui, event::ui::CHANGE, |w| {
            w.u32(handle);
            value.encode(w).unwrap();
        })
    }

    fn message(self, value: &Value) -> Batch {
        self.record(Tier::Net, event::net::MESSAGE, |w| value.encode(w).unwrap())
    }

    fn revoke(self, tier: Tier) -> Batch {
        self.record(Tier::Realm, event::realm::REVOKED, |w| w.u8(tier.id()))
    }

    fn bytes(&self) -> &[u8] {
        self.0.bytes()
    }
}

fn frame(realm: &mut ClientRealm, now: u64, batch: Batch) {
    realm.frame(now, batch.bytes()).unwrap();
}

fn string(s: &str) -> Value {
    Value::String(s.as_bytes().to_vec())
}

#[test]
fn the_escape_suite_is_all_refused() {
    let mut r = realm_of(Config {
        grants: Grants::of(&[Tier::Ui, Tier::Net]),
        callback_steps: 5_000_000,
        ..Config::default()
    });
    r.add_script("other", "otherlib.lua", b"return { evil = true }")
        .unwrap();
    run(&mut r, ESCAPE);
    let logs = logs(&r);
    let summary = logs
        .iter()
        .find(|l| l.starts_with("escape: "))
        .unwrap_or_else(|| panic!("the suite ran to its end: {logs:?}"));
    let words: Vec<&str> = summary.split_whitespace().collect();
    let refused: usize = words[1].parse().unwrap();
    let allowed: usize = words[3].parse().unwrap();

    // The cases only the host's side can drive: each counted with the script's.
    let host = host_side_escapes();
    println!(
        "escape suite: {} refused ({refused} in the script, {host} through the entry points), {allowed} allowed",
        refused + host
    );
    assert_eq!(allowed, 0, "{logs:?}");
    assert!(refused + host >= 45, "{refused} + {host}");
}

/// The escape cases that go through the entry points rather than a script: each must be refused; the count of them.
fn host_side_escapes() -> usize {
    let mut count = 0;
    let mut refused = |ok: bool, what: &str| {
        assert!(ok, "{what} was allowed");
        count += 1;
    };
    let mut r = full();
    // Script files: compiled code, paths out of the mod, caps.
    refused(
        r.add_script(MOD, "a.lua", b"\x1bLua").is_err(),
        "a bytecode chunk",
    );
    refused(
        r.add_script(MOD, "../a.lua", b"").is_err(),
        "a script path with ..",
    );
    refused(
        r.add_script(MOD, "C:/a.lua", b"").is_err(),
        "a script path on a drive",
    );
    refused(
        r.add_script(MOD, "/a.lua", b"").is_err(),
        "an absolute script path",
    );
    refused(
        r.add_script(MOD, "a\\b.lua", b"").is_err(),
        "a script path with a backslash",
    );
    refused(
        r.add_script(MOD, "a.cs", b"").is_err(),
        "a TorqueScript file",
    );
    refused(r.add_script("..", "a.lua", b"").is_err(), "a mod id of ..");
    refused(
        r.add_script(MOD, "con.lua", b"").is_err(),
        "a script named after a device",
    );
    refused(
        r.add_script(MOD, ".../a.lua", b"").is_err(),
        "a script folder of only dots",
    );
    refused(
        r.add_script("aux", "a.lua", b"").is_err(),
        "a mod id named after a device",
    );
    refused(
        r.add_script("mod.", "a.lua", b"").is_err(),
        "a mod id with a trailing dot",
    );
    refused(
        r.add_script(MOD, "big.lua", &vec![b' '; abi::MAX_SCRIPT_BYTES + 1])
            .is_err(),
        "a script past its cap",
    );
    refused(
        r.run(0, MOD, "missing.lua") == Err(torque_lua::client_script::Status::Refused),
        "running a file not kept",
    );

    // Markup in plain text is shown literally: the op carries the bytes and the text kind, never rich text.
    let mut r = full();
    run(
        &mut r,
        r#"ui.new("text", { text = "<a:gotoWebPage>x</a>$ClientScript::Canary = 1;" })"#,
    );
    let create = ops(&r)
        .into_iter()
        .find(|o| o.tier == Tier::Ui.id() && o.kind == op::ui::CREATE)
        .unwrap();
    let mut reader = Reader::new(&create.body);
    reader.u32().unwrap();
    reader.u32().unwrap();
    let kind = reader.u8().unwrap();
    let props = ui::read_props(&mut reader, ui::kind_from_id(kind).unwrap()).unwrap();
    refused(
        kind == Kind::Text as u8
            && props
                == vec![(
                    Prop::Text,
                    PropValue::Text("<a:gotoWebPage>x</a>$ClientScript::Canary = 1;".into()),
                )],
        "markup in plain text taken as markup",
    );

    // Events naming another realm's control, or one never made, reach nothing.
    let mut a = full();
    run(
        &mut a,
        r#"local b = ui.new("button", {}); b:on("press", function() realm.log("a pressed") end)"#,
    );
    let mut b = full();
    run(&mut b, r#"realm.log("b ready")"#);
    frame(&mut b, 1, Batch::default().press(2).press(99));
    refused(logs(&b).is_empty(), "another realm's control");
    frame(&mut a, 1, Batch::default().press(99).click(2));
    refused(
        logs(&a).is_empty(),
        "a control never made, or an event its kind lacks",
    );

    // Focus is refused without a click in the last second, granted after one, refused again a second later.
    let mut r = full();
    run(
        &mut r,
        r#"field = ui.new("textfield", {}); go = ui.new("button", {}); go:on("press", function() end)
        realm.every(0, function() realm.log(tostring(ui.focus(field))) end)"#,
    );
    frame(&mut r, 10, Batch::default());
    refused(logs(&r) == ["false"], "focus without a click");
    frame(&mut r, 20, Batch::default().press(3));
    assert_eq!(logs(&r), ["true"], "focus within a second of a press");
    frame(&mut r, 1_000_100, Batch::default());
    refused(
        logs(&r) == ["false"],
        "focus more than a second after the press",
    );

    // A batch that does not follow the format is refused whole.
    let mut r = full();
    run(
        &mut r,
        r#"b = ui.new("button", {}); b:on("press", function() realm.log("pressed") end)"#,
    );
    let mut bad = Batch::default().press(2).0.into_bytes();
    bad.extend_from_slice(&[Tier::State.id(), 0, 0, 0, 0, 0]);
    refused(
        r.frame(1, &bad).is_err() && r.pending() == 0,
        "an event of a tier format 1 has none of",
    );
    refused(r.frame(2, &[1, 0, 4, 0]).is_err(), "a truncated batch");
    let revoke_base = Batch::default().record(Tier::Realm, event::realm::REVOKED, |w| w.u8(0));
    refused(
        r.frame(3, revoke_base.bytes()).is_err(),
        "revoking the base",
    );
    refused(
        r.frame(1, &[]).is_ok() && r.frame(0, &[]).is_err(),
        "time going back",
    );

    // A config naming tiers that do not exist is refused.
    let mut config = Config::default().to_bytes();
    config[4..8].copy_from_slice(&(1u32 << 11).to_le_bytes());
    refused(
        ClientRealm::from_config(&config).is_err(),
        "grant bits that name no tier",
    );
    count
}

#[test]
fn an_ungranted_tier_has_no_table() {
    // One case per tier name: the realm is granted every other tier (this build's and the rest), and the tier's own
    // table is absent, `realm.has` says no, and using it is a plain Lua error.
    for tier in Tier::ALL.into_iter().filter(|&t| t != Tier::Realm) {
        let others: Vec<Tier> = Tier::ALL.into_iter().filter(|&t| t != tier).collect();
        let mut r = realm(&others);
        let table = tier.table();
        run(
            &mut r,
            &format!(
                r#"realm.log(tostring({table} == nil), tostring(realm.has("{name}")),
                    tostring(pcall(function() return {table}.anything end)))"#,
                name = tier.name()
            ),
        );
        assert_eq!(logs(&r), ["true false false"], "{}", tier.name());
    }
    // The tiers this build does not have stay absent even when granted.
    let mut r = realm(&Tier::ALL);
    run(
        &mut r,
        r#"realm.log(tostring(ui ~= nil and net ~= nil), tostring(state == nil and draw == nil and effects == nil
            and input == nil and camera == nil and world == nil and predict == nil), tostring(realm.has("draw")))"#,
    );
    assert_eq!(logs(&r), ["true true false"]);
    assert_eq!(ClientRealm::tiers(), Grants::of(&[Tier::Ui, Tier::Net]));
}

/// Checks a callback that loops forever stops within its budget and says so.
fn assert_stopped(r: &ClientRealm, entry: u8, cap: u64) {
    assert_eq!(limits(r), [(op::realm::LIMIT_STEPS, entry)]);
    assert!(logs(r).iter().any(|l| l == STOPPED), "{:?}", logs(r));
    let steps = r.last_steps();
    assert!(
        steps > cap && steps <= cap + 64,
        "{steps} steps for a cap of {cap}"
    );
}

#[test]
fn a_runaway_loop_stops_in_every_entry() {
    let cap = u64::from(Config::default().callback_steps);
    // A file run.
    let mut r = full();
    run(&mut r, "while true do end");
    assert_stopped(&r, op::realm::ENTRY_RUN, cap);

    // An event, a timer, a message handler, an onFrame callback.
    let mut r = full();
    run(
        &mut r,
        r#"b = ui.new("button", {}); b:on("press", function() while true do end end)"#,
    );
    frame(&mut r, 1, Batch::default().press(2));
    assert_stopped(&r, op::realm::ENTRY_EVENT, cap);

    let mut r = full();
    run(&mut r, "realm.after(0.5, function() while true do end end)");
    frame(&mut r, 400_000, Batch::default());
    assert!(limits(&r).is_empty());
    frame(&mut r, 500_000, Batch::default());
    assert_stopped(&r, op::realm::ENTRY_TIMER, cap);

    let mut r = full();
    run(&mut r, "net.on(function(v) while true do end end)");
    frame(&mut r, 1, Batch::default().message(&string("hi")));
    assert_stopped(&r, op::realm::ENTRY_EVENT, cap);

    let mut r = full();
    run(&mut r, "realm.onFrame(function(dt) while true do end end)");
    frame(&mut r, 1, Batch::default());
    assert_stopped(&r, op::realm::ENTRY_FRAME, cap);

    // A coroutine.
    let mut r = full();
    run(&mut r, "coroutine.wrap(function() while true do end end)()");
    assert_stopped(&r, op::realm::ENTRY_RUN, cap);

    // The realm goes on: the next callback has its own budget.
    run_at(&mut r, 2, "next.lua", r#"realm.log("next")"#);
    assert_eq!(logs(&r), ["next"]);
}

#[test]
fn neither_pcall_nor_a_coroutine_goes_on_after_a_stop() {
    let mut r = full();
    run(
        &mut r,
        r#"pcall(function() while true do end end)
        realm.log("went on after pcall")"#,
    );
    assert!(
        !logs(&r).iter().any(|l| l.contains("went on")),
        "{:?}",
        logs(&r)
    );
    assert_eq!(limits(&r).len(), 1);

    let mut r = full();
    run(
        &mut r,
        r#"while true do pcall(function() while true do end end) end"#,
    );
    assert_eq!(limits(&r).len(), 1);

    let mut r = full();
    run(
        &mut r,
        r#"local co = coroutine.create(function() while true do end end)
        coroutine.resume(co)
        realm.log("went on after resume")
        while true do coroutine.resume(coroutine.create(function() end)) end"#,
    );
    assert!(
        !logs(&r).iter().any(|l| l.contains("went on")),
        "{:?}",
        logs(&r)
    );
    assert_eq!(limits(&r).len(), 1);

    // An error object whose __tostring loops is made text under the same budget.
    let mut r = full();
    run(
        &mut r,
        r#"error(setmetatable({}, { __tostring = function() while true do end end }))"#,
    );
    assert_eq!(limits(&r), [(op::realm::LIMIT_STEPS, op::realm::ENTRY_RUN)]);
    run_at(&mut r, 1, "after.lua", r#"realm.log("after")"#);
    assert_eq!(logs(&r), ["after"]);
}

#[test]
fn a_callback_storm_waits_for_later_frames() {
    let config = Config {
        grants: Grants::of(&[Tier::Ui]),
        ..Config::default()
    };
    let (per_callback, per_frame) = (
        u64::from(config.callback_steps),
        u64::from(config.frame_steps),
    );
    let mut r = realm_of(config);
    run(
        &mut r,
        r#"b = ui.new("button", {}); b:on("press", function() while true do end end)"#,
    );
    let mut batch = Batch::default();
    for _ in 0..100 {
        batch = batch.press(2);
    }
    frame(&mut r, 1, batch);
    let ran = limits(&r).len();
    assert!(
        r.frame_steps() <= per_frame + per_callback + 64,
        "{}",
        r.frame_steps()
    );
    assert_eq!(ran as u64, per_frame.div_ceil(per_callback));
    assert_eq!(r.pending(), 100 - ran);
    // The rest runs in later frames.
    let mut frames = 1;
    while r.pending() > 0 {
        frame(&mut r, 1 + frames, Batch::default());
        assert!(r.frame_steps() <= per_frame + per_callback + 64);
        frames += 1;
    }
    assert_eq!(frames, 10);

    // The queue keeps at most 256 events: the oldest input goes.
    let mut batch = Batch::default();
    for _ in 0..1000 {
        batch = batch.press(2);
    }
    frame(&mut r, 100, batch);
    assert_eq!(r.pending() + limits(&r).len(), format::MAX_QUEUED);
}

#[test]
fn memory_bombs_stop_at_the_cap_and_the_realm_goes_on() {
    // With the default budget, bulk builtins are charged by their bytes before they allocate.
    let mut r = full();
    run(&mut r, r#"local s = string.rep("x", 2^30)"#);
    assert_eq!(limits(&r), [(op::realm::LIMIT_STEPS, op::realm::ENTRY_RUN)]);
    run_at(&mut r, 1, "buffer.lua", "local b = buffer.create(2^30)");
    assert_eq!(limits(&r), [(op::realm::LIMIT_STEPS, op::realm::ENTRY_RUN)]);
    run_at(
        &mut r,
        2,
        "create.lua",
        "local t = table.create(2^20, 0) t = table.create(2^20, 0)",
    );
    assert_eq!(limits(&r), [(op::realm::LIMIT_STEPS, op::realm::ENTRY_RUN)]);

    // With steps to spare, the memory cap stops them.
    let cap = 8 << 20;
    let bombs = [
        ("rep.lua", r#"local s = string.rep("x", 64 * 2^20)"#),
        ("table.lua", "local t = {} for i = 1, 1e9 do t[i] = i end"),
        (
            "strings.lua",
            r#"local t = {} for i = 1, 1e9 do t[i] = string.rep("y", 1000) .. i end"#,
        ),
        ("buffer.lua", "local b = buffer.create(64 * 2^20)"),
        (
            "concat.lua",
            r#"local s = "z" for i = 1, 40 do s = s .. s end"#,
        ),
    ];
    let mut r = realm_of(Config {
        grants: Grants::of(&[Tier::Ui, Tier::Net]),
        memory: cap,
        callback_steps: 1_000_000_000,
        ..Config::default()
    });
    for (now, (path, source)) in bombs.into_iter().enumerate() {
        run_at(&mut r, now as u64, path, source);
        assert_eq!(
            limits(&r),
            [(op::realm::LIMIT_MEMORY, op::realm::ENTRY_RUN)],
            "{path}: {:?}",
            logs(&r)
        );
        assert!(
            r.used_memory() < cap as usize / 2,
            "{path}: {} bytes kept",
            r.used_memory()
        );
    }
    // Deep recursion is an error, not a crash.
    run_at(
        &mut r,
        10,
        "deep.lua",
        "local function f(n) return f(n + 1) + 1 end f(1)",
    );
    assert!(!logs(&r).is_empty());
    run_at(&mut r, 11, "after.lua", r#"realm.log("after")"#);
    assert_eq!(logs(&r), ["after"]);

    // Another realm never noticed.
    let mut other = full();
    run(&mut other, r#"realm.log("fine")"#);
    assert_eq!(logs(&other), ["fine"]);
}

#[test]
fn patterns_with_too_many_quantifiers_are_refused() {
    let mut r = full();
    run(
        &mut r,
        r#"realm.log(tostring(pcall(string.find, string.rep("a", 100), "a*a*a*a*a*a*a*b")),
            tostring(pcall(string.find, "aaa", "a*a*b")))"#,
    );
    assert_eq!(logs(&r), ["false true"]);
}

/// The ui ops of the last entry, as (kind, body).
fn ui_ops(r: &ClientRealm) -> Vec<(u8, Vec<u8>)> {
    ops(r)
        .into_iter()
        .filter(|o| o.tier == Tier::Ui.id())
        .map(|o| (o.kind, o.body))
        .collect()
}

#[test]
fn ui_ops_follow_the_mirror() {
    let mut r = full();
    run(
        &mut r,
        r##"local root = ui.root()
        panel = ui.new("panel", { pos = { 10, 20 }, size = { x = 100, y = 50 }, fill = "#ff000080" })
        button = ui.new("button", { text = "Go", style = "primary" }, panel)
        list = ui.new("list", { rows = { "a", "b" } })
        realm.log(button:get("text"), button:get("style"), tostring(panel:get("pos")), tostring(list:get("selected")))
        button:set({ text = "Stop" })
        button:front()
        button:parent(root)
        panel:destroy()
        realm.log(tostring(pcall(panel.set, panel, {})), tostring(button:get("visible")))
        ui.interactive(true)"##,
    );
    assert_eq!(logs(&r), ["Go primary 10 0", "false true"]);
    let kinds: Vec<u8> = ui_ops(&r).iter().map(|(k, _)| *k).collect();
    use op::ui::*;
    assert_eq!(
        kinds,
        [
            ROOT,
            CREATE,
            CREATE,
            CREATE,
            SET,
            ORDER,
            PARENT,
            DESTROY,
            INTERACTIVE
        ]
    );
    let ops = ui_ops(&r);
    // The root names its mod; the panel is control 2 under root 1, its props in id order.
    let mut root = Reader::new(&ops[0].1);
    assert_eq!((root.u32().unwrap(), root.text().unwrap()), (1, MOD));
    let mut create = Reader::new(&ops[1].1);
    assert_eq!((create.u32().unwrap(), create.u32().unwrap()), (2, 1));
    let kind = ui::kind_from_id(create.u8().unwrap()).unwrap();
    assert_eq!(kind.kind, Kind::Panel);
    assert_eq!(
        ui::read_props(&mut create, kind).unwrap(),
        [
            (Prop::Pos, PropValue::Vec2(10.0, 20.0)),
            (Prop::Size, PropValue::Vec2(100.0, 50.0)),
            (Prop::Fill, PropValue::Color([255, 0, 0, 128])),
        ]
    );
    create.finish().unwrap();
    // The button (3) went under the panel (2), then to the root.
    let mut button = Reader::new(&ops[2].1);
    assert_eq!((button.u32().unwrap(), button.u32().unwrap()), (3, 2));
    let mut parent = Reader::new(&ops[6].1);
    assert_eq!((parent.u32().unwrap(), parent.u32().unwrap()), (3, 1));
    // Handles are never reused.
    run_at(&mut r, 1, "more.lua", r#"ui.new("panel", {})"#);
    let more = ui_ops(&r);
    assert_eq!(Reader::new(&more[0].1).u32().unwrap(), 5);
}

#[test]
fn ui_caps_hold() {
    let mut r = full();
    run(
        &mut r,
        r#"for i = 1, 1024 do ui.new("panel", {}) end
        realm.log(tostring(pcall(ui.new, "panel", {})))
        local t = {}
        for i = 1, 300 do t[i] = ui.new("text", { text = string.rep("x", 4000) }) end"#,
    );
    let lines = logs(&r);
    assert_eq!(lines[0], "false");
    assert!(lines[1].contains("at most 1024 controls"), "{lines:?}");

    // Text in all: at most 1 MiB.
    let mut r = full();
    run(
        &mut r,
        r#"local ok, n = true, 0
        while ok do ok = pcall(ui.new, "text", { text = string.rep("x", 4000) }); if ok then n += 1 end end
        realm.log(tostring(n))"#,
    );
    assert_eq!(logs(&r), [(ui::MAX_TEXT / 4000).to_string()]);

    // A list's rows: 16 KiB in all (beside the 4 KiB of its other text), 4 KiB a row.
    let mut r = full();
    run(
        &mut r,
        r#"local row = string.rep("r", 4096)
        local tip = string.rep("t", 256)
        local list = ui.new("list", { rows = { row, row, row, row }, tooltip = tip })
        realm.log(tostring(#list:get("rows")),
            tostring(pcall(ui.new, "list", { rows = { row, row, row, row, "x" } })),
            tostring(pcall(ui.new, "list", { rows = { row .. "x" } })),
            tostring(pcall(list.set, list, { rows = { row, row, row, row, "x" } })),
            tostring(#list:get("rows")))"#,
    );
    assert_eq!(logs(&r), ["4 false false false 4"]);

    // Images: at most 256 distinct.
    let mut r = full();
    run(
        &mut r,
        r#"for i = 1, 256 do ui.new("image", { image = `mods/m/img{i}.png` }) end
        ui.new("image", { image = "mods/m/img1.png" })
        realm.log(tostring(pcall(ui.new, "image", { image = "mods/m/img257.png" })))"#,
    );
    assert_eq!(logs(&r), ["false"]);
}

#[test]
fn events_update_the_mirror_and_reach_their_controls() {
    let mut r = full();
    run(
        &mut r,
        r#"toggle = ui.new("toggle", { text = "On?" })
        toggle:on("change", function(on) realm.log("toggle", tostring(on), tostring(toggle:get("on"))) end)
        field = ui.new("textfield", { maxLength = 4 })
        field:on("change", function(text) realm.log("field", text) end)
        slider = ui.new("slider", { range = { 0, 10 } })
        slider:on("change", function(v) realm.log("slider", tostring(v)) end)
        list = ui.new("list", { rows = { "a", "b" } })
        list:on("select", function(row) realm.log("list", tostring(row)) end)
        ui.on("screen", function(w, h, s) realm.log("screen", w, h, s) end)"#,
    );
    // Handles: root 1, toggle 2, field 3, slider 4, list 5.
    let batch = Batch::default()
        .change(2, &Value::Bool(true))
        .change(3, &string("abcd"))
        .change(3, &string("too long"))
        .change(3, &Value::Number(1.0))
        .change(4, &Value::Number(7.0))
        .change(4, &Value::Number(11.0))
        .record(Tier::Ui, event::ui::SELECT, |w| {
            w.u32(5);
            w.u32(2);
        })
        .record(Tier::Ui, event::ui::SELECT, |w| {
            w.u32(5);
            w.u32(3);
        })
        .record(Tier::Ui, event::ui::SCREEN, |w| {
            w.f64(800.0);
            w.f64(600.0);
            w.f64(1.0);
        });
    frame(&mut r, 1, batch);
    assert_eq!(
        logs(&r),
        [
            "toggle true true",
            "field abcd",
            "slider 7",
            "list 2",
            "screen 800 600 1"
        ]
    );
}

#[test]
fn revocation_answers_false_and_drops_the_tier() {
    let mut r = full();
    run(
        &mut r,
        r#"b = ui.new("button", {})
        b:on("press", function() realm.log("pressed") end)
        realm.on("revoked", function(name)
            realm.log("revoked " .. name, tostring(realm.has(name)), tostring(b:set({ text = "x" })),
                tostring(ui.new("panel", {})), tostring(net.send(1)))
        end)"#,
    );
    // The press queued in the same batch as the revocation never runs.
    frame(
        &mut r,
        1,
        Batch::default().press(2).revoke(Tier::Ui).revoke(Tier::Net),
    );
    assert_eq!(
        logs(&r),
        [
            "revoked ui false false false false",
            "revoked net false false false false"
        ]
    );
    assert!(ops(&r).iter().all(|o| o.tier == Tier::Realm.id()));
    assert_eq!(r.grants(), Grants::of(&[]));
    // Later events of the tier are dropped.
    frame(&mut r, 2, Batch::default().press(2).message(&Value::Nil));
    assert!(logs(&r).is_empty());
}

#[test]
fn net_messages_calls_and_rates() {
    let mut r = full();
    run(
        &mut r,
        r#"net.on(function(v) realm.log("message", v.kind, tostring(v.n)) end)
        net.handle("game.score", function(v) realm.log("score", tostring(v)) end)
        net.send({ kind = "hello", list = { 1, 2, 3 } })
        net.call("props.spawn", { data = "crate" }, function(ok, v) realm.log("reply", tostring(ok), tostring(v)) end)"#,
    );
    let net: Vec<Op> = ops(&r)
        .into_iter()
        .filter(|o| o.tier == Tier::Net.id())
        .collect();
    assert_eq!(net.len(), 2);
    assert_eq!(net[0].kind, op::net::SEND);
    let sent = Value::from_bytes(&net[0].body).unwrap();
    let Value::Table(pairs) = sent else { panic!() };
    assert_eq!(pairs.len(), 2);
    let mut call = Reader::new(&net[1].body);
    assert_eq!(
        (call.u32().unwrap(), call.text().unwrap()),
        (1, "props.spawn")
    );
    let args = Value::decode(&mut call, &mut format::NodeBudget::default()).unwrap();
    assert_eq!(args, Value::Table(vec![(string("data"), string("crate"))]));

    let message = Value::Table(vec![
        (string("kind"), string("chat")),
        (string("n"), Value::Number(3.0)),
    ]);
    let batch = Batch::default()
        .message(&message)
        .record(Tier::Net, event::net::RPC, |w| {
            w.text("game.score");
            Value::Number(10.0).encode(w).unwrap();
        })
        .record(Tier::Net, event::net::REPLY, |w| {
            w.u32(1);
            w.bool(true);
            string("done").encode(w).unwrap();
        })
        .record(Tier::Net, event::net::REPLY, |w| {
            w.u32(1);
            w.bool(true);
            string("twice").encode(w).unwrap();
        });
    frame(&mut r, 1, batch);
    assert_eq!(logs(&r), ["message chat 3", "score 10", "reply true done"]);

    // The burst is 64 sends; then 32 a second.
    let mut r = full();
    run(
        &mut r,
        r#"local n = 0
        for i = 1, 100 do if net.send(i) then n += 1 end end
        realm.log(tostring(n))"#,
    );
    assert_eq!(logs(&r).last().unwrap(), "64");
    run_at(
        &mut r,
        1_000_000,
        "later.lua",
        r#"local n = 0
        for i = 1, 100 do if net.send(i) then n += 1 end end
        realm.log(tostring(n))"#,
    );
    assert_eq!(logs(&r).last().unwrap(), "32");

    // At most 16 calls of one RPC wait for replies.
    let mut r = full();
    run(
        &mut r,
        r#"for i = 1, 16 do net.call("a.b", i, function() end) end
        realm.log(tostring(pcall(net.call, "a.b", 17, function() end)), tostring(net.call("a.c", 1, function() end)))"#,
    );
    assert_eq!(logs(&r), ["false true"]);
}

#[test]
fn timers_run_by_due_time_and_frames_get_their_seconds() {
    let mut r = full();
    run(
        &mut r,
        r#"realm.after(0.2, function() realm.log("b") end)
        realm.after(0.1, function() realm.log("a") end)
        local t = realm.every(0.1, function() realm.log("every", realm.time()) end)
        local frames = 0
        realm.onFrame(function(dt) frames += 1; if frames == 3 then realm.log("dt", dt); realm.cancel(t) end end)"#,
    );
    frame(&mut r, 50_000, Batch::default());
    assert!(logs(&r).is_empty());
    frame(&mut r, 250_000, Batch::default());
    assert_eq!(logs(&r), ["a", "every 0.25", "b"]);
    frame(&mut r, 300_000, Batch::default());
    assert_eq!(logs(&r), ["dt 0.05"]);
    frame(&mut r, 400_000, Batch::default());
    assert!(logs(&r).is_empty());

    // At most 64 timers.
    let mut r = full();
    run(
        &mut r,
        r#"for i = 1, 64 do realm.after(1, function() end) end
        realm.log(tostring(pcall(realm.after, 1, function() end)))"#,
    );
    assert_eq!(logs(&r), ["false"]);
}

#[test]
fn require_loads_the_mods_own_files_once() {
    let mut r = full();
    r.add_script(
        MOD,
        "lib/util.lua",
        b"count = (count or 0) + 1; return { twice = function(x) return x * 2 end }",
    )
    .unwrap();
    r.add_script(MOD, "loop.lua", b"return require(\"loop\")")
        .unwrap();
    r.add_script("other", "secret.lua", b"return 42").unwrap();
    run(
        &mut r,
        r#"local u = require("lib.util"); local v = require("lib/util")
        realm.log(tostring(u.twice(4)), tostring(u == v), tostring(count))
        realm.log(tostring(pcall(require, "loop")), tostring(pcall(require, "secret")))"#,
    );
    assert_eq!(logs(&r), ["8 true 1", "false false"]);
}

#[test]
fn log_lines_are_plain_capped_and_rate_limited() {
    let mut r = full();
    run(
        &mut r,
        r#"print("a\nb\27[31m\tc")
        realm.log(string.rep("x", 600))
        for i = 1, 30 do realm.log(i) end"#,
    );
    let logs = logs(&r);
    assert_eq!(logs[0], "a b [31m c");
    assert_eq!(logs[1].len(), format::MAX_LOG_LINE);
    assert_eq!(logs.len(), format::LOG_LINES_PER_SECOND as usize + 1);
    assert!(logs.last().unwrap().contains("dropped"));
}

/// Runs `source` in a fresh realm: it must stop by its step budget, and soon (the work it hands a builtin is charged
/// before the builtin runs).
fn assert_runs_out(name: &str, source: &str) {
    let mut r = full();
    let started = std::time::Instant::now();
    run(&mut r, source);
    let took = started.elapsed().as_secs_f64();
    assert_eq!(
        limits(&r),
        [(op::realm::LIMIT_STEPS, op::realm::ENTRY_RUN)],
        "{name}: {:?}",
        logs(&r)
    );
    assert!(took < 2.0, "{name} took {took} s");
    // The realm goes on.
    run_at(&mut r, 1, "after.lua", r#"realm.log("after")"#);
    assert_eq!(logs(&r), ["after"], "{name}");
}

#[test]
fn string_searches_are_charged_by_pattern_and_needle() {
    for (name, source) in [
        (
            "a plain find of a long needle",
            r#"string.find(string.rep("a", 2^20), string.rep("a", 2^19) .. "b", 1, true)"#,
        ),
        (
            "a balanced match",
            r#"string.find(string.rep("(", 2^20), "%b()")"#,
        ),
        (
            "a gsub of a long literal",
            r#"local s, p = string.rep("a", 2^20), string.rep("a", 1023) .. "b"
            while true do s:gsub(p, "x") end"#,
        ),
        (
            "a split by a long needle",
            r#"local s = string.rep("a", 2^20) s:split(string.rep("a", 2^19) .. "b")"#,
        ),
    ] {
        assert_runs_out(name, source);
    }
    // A pattern longer than 1 KiB is refused; one of 1 KiB is not.
    let mut r = full();
    run(
        &mut r,
        r#"realm.log(tostring(pcall(string.find, "x", string.rep("a", 1025))),
            tostring(pcall(string.find, "x", string.rep("a", 1024))))"#,
    );
    assert_eq!(logs(&r), ["false true"]);
}

#[test]
fn table_work_is_charged_by_its_slots() {
    // A table of a million empty slots: each of these goes through all of them, so each is charged them all.
    let sparse = "local t = table.create(1e6) ";
    for (name, body) in [
        ("next", "while true do next(t) end"),
        ("pairs", "while true do for _ in pairs(t) do end end"),
        ("table.clear", "while true do table.clear(t) end"),
        ("table.maxn", "while true do table.maxn(t) end"),
        ("table.foreach", "while true do table.foreach(t, print) end"),
        ("table.clone", "while true do local c = table.clone(t) end"),
        ("net.send", "while true do pcall(net.send, t) end"),
        ("ui.new", r#"while true do pcall(ui.new, "panel", t) end"#),
    ] {
        assert_runs_out(name, &format!("{sparse}{body}"));
    }
    // Sending it or using it as props is refused before it is walked; table.create has a cap.
    let mut r = full();
    run(
        &mut r,
        r#"local t = table.create(5000)
        realm.log(tostring(pcall(net.send, t)), tostring(pcall(ui.new, "panel", t)),
            tostring(pcall(table.create, 2^20 + 1)))"#,
    );
    assert_eq!(logs(&r), ["false false false"]);
}

#[test]
fn a_huge_tostring_is_read_only_as_far_as_a_line_goes() {
    let mut r = realm_of(Config {
        grants: Grants::of(&[Tier::Ui, Tier::Net]),
        memory: 64 << 20,
        ..Config::default()
    });
    let started = std::time::Instant::now();
    run(
        &mut r,
        r#"local big = "x" for _ = 1, 24 do big = big .. big end
        local o = setmetatable({}, { __tostring = function() return big end })
        realm.log(o)
        while true do realm.log(o) print(o) end"#,
    );
    let took = started.elapsed().as_secs_f64();
    assert_eq!(limits(&r), [(op::realm::LIMIT_STEPS, op::realm::ENTRY_RUN)]);
    assert_eq!(logs(&r)[0].len(), format::MAX_LOG_LINE);
    assert!(took < 2.0, "{took} s");
    run_at(&mut r, 2_000_000, "after.lua", r#"realm.log("after")"#);
    assert_eq!(logs(&r), ["after"]);
}

#[test]
fn log_lines_cannot_forge_a_line_or_reorder_text() {
    let mut r = full();
    run(
        &mut r,
        r#"realm.log("a\u{2028}[engine] x")
        realm.log("b\u{2029}c\u{202e}d\u{2066}e\u{200f}f")"#,
    );
    assert_eq!(logs(&r), ["a [engine] x", "b c d e f"]);
}

#[test]
fn calls_wait_at_most_64_in_all() {
    let mut r = full();
    run(
        &mut r,
        r#"for n = 1, 4 do for i = 1, 16 do net.call("rpc.n" .. n, i, function() end) end end
        realm.log(tostring(pcall(net.call, "rpc.other", 1, function() end)))"#,
    );
    assert_eq!(logs(&r), ["false"]);
}

#[test]
fn a_batch_past_its_node_budget_is_refused_whole() {
    let mut r = full();
    run(&mut r, r#"net.on(function() realm.log("message") end)"#);
    let table = Value::Table(
        (0..200)
            .map(|i| (Value::Number(f64::from(i)), Value::Nil))
            .collect(),
    );
    let mut batch = Batch::default();
    for _ in 0..(format::MAX_BATCH_NODES / 401 + 1) {
        batch = batch.message(&table);
    }
    assert!(r.frame(1, batch.bytes()).is_err());
    assert_eq!(r.pending(), 0);
}

#[test]
fn pairs_hands_out_the_metered_next() {
    // The iterator `pairs` gives is `next` itself, charged as it goes.
    let sparse = "local t = table.create(1e6) t[1e6] = 1 ";
    for (name, body) in [
        (
            "pairs' iterator from the start",
            "local f = pairs(t) while true do f(t) end",
        ),
        (
            "pairs' iterator from a key out of order",
            "local f, k = pairs(t), next(t) while true do f(t, k) end",
        ),
    ] {
        assert_runs_out(name, &format!("{sparse}{body}"));
    }
    // A whole traversal is charged its slots once: 20,000 keys go through within one callback's budget.
    let mut r = full();
    run(
        &mut r,
        r#"local t = {} for i = 1, 20000 do t["k" .. i] = i end
        local n = 0
        for _ in pairs(t) do n += 1 end
        local m, k = 0, next(t)
        while k ~= nil do m += 1 k = next(t, k) end
        realm.log(tostring(n), tostring(m), tostring(rawequal((pairs(t)), next)))"#,
    );
    assert_eq!(logs(&r), ["20000 20000 true"], "{:?}", limits(&r));
}

#[test]
fn numeric_strings_are_read_as_luau_reads_them() {
    // A size given as text is the number Luau reads from it: capped and charged as that number.
    for (name, source) in [
        (
            "string.rep of a text count",
            r#"local s = string.rep("a", "1e9")"#,
        ),
        (
            "buffer.create of a text size",
            r#"local b = buffer.create("1e9")"#,
        ),
        (
            "string.rep of a number's text",
            "local s = string.rep(1e300, 1e7)",
        ),
        (
            "table.create of text sizes in a loop",
            r#"while true do local t = table.create("100000") end"#,
        ),
    ] {
        assert_runs_out(name, source);
    }
    let mut r = full();
    run(
        &mut r,
        r#"realm.log(tostring(pcall(table.create, "1e7")), tostring(pcall(table.create, " 0x200000 ")),
            tostring(pcall(table.create, string.rep("0", 100) .. "5")), tostring(#table.create("5", 1)))"#,
    );
    assert_eq!(logs(&r), ["false false false 5"]);
}

#[test]
fn utf8_codes_iterator_is_charged_the_bytes_it_skips() {
    let mut r = realm_of(Config {
        grants: Grants::of(&[Tier::Ui, Tier::Net]),
        memory: 256 << 20,
        ..Config::default()
    });
    let started = std::time::Instant::now();
    // One character, then 64 MiB of continuation bytes the iterator's second call skips at once.
    run(
        &mut r,
        r#"local c = "\x80" for _ = 1, 26 do c = c .. c end
        local s = "a" .. c
        c = nil
        while true do for _ in utf8.codes(s) do end end"#,
    );
    assert_eq!(
        limits(&r),
        [(op::realm::LIMIT_STEPS, op::realm::ENTRY_RUN)],
        "{:?}",
        logs(&r)
    );
    assert!(started.elapsed().as_secs_f64() < 5.0);
    // Valid text iterates as before.
    let mut r = full();
    run(
        &mut r,
        r#"local n = 0 for _, c in utf8.codes("h\u{e9}llo") do n += 1 end realm.log(tostring(n))"#,
    );
    assert_eq!(logs(&r), ["5"]);
}

#[test]
fn ranges_are_charged_whole_and_never_overflow() {
    for (name, source) in [
        (
            "unpack past the table's length",
            "while true do pcall(unpack, {}, 1, 7000) end",
        ),
        (
            "table.unpack past the table's length",
            "while true do pcall(table.unpack, {}, 1, 7000) end",
        ),
        (
            "table.move to a huge end",
            "local t = {1} table.move(t, 0, 1e300, 1)",
        ),
        (
            "table.remove at a huge negative place",
            "local t = {1} table.remove(t, -1e300)",
        ),
        (
            "table.insert at a huge negative place",
            "local t = {1} table.insert(t, -1e300, 1)",
        ),
        (
            "string.sub of huge places",
            r#"while true do string.sub("abc", -1e300, 1e300) end"#,
        ),
        (
            "table.move by text",
            r#"local t = {1} table.move(t, "1", "1e9", 2)"#,
        ),
        (
            "buffer.readstring by text",
            r#"local b = buffer.create(8) pcall(buffer.readstring, b, 0, "1e9")"#,
        ),
        (
            "tonumber of a long hex text",
            r#"local s = string.rep("f", 1e6) while true do tonumber(s, 16) end"#,
        ),
    ] {
        assert_runs_out(name, source);
    }
}

#[test]
fn caught_errors_are_charged() {
    for (name, source) in [
        ("pcall of error", r#"while true do pcall(error, "x") end"#),
        (
            "xpcall of error",
            r#"while true do xpcall(error, function(e) return e end, "x") end"#,
        ),
        (
            "pcall of a failing builtin",
            r#"while true do pcall(string.rep) end"#,
        ),
    ] {
        assert_runs_out(name, source);
    }
    // A coroutine still yields inside a pcall, and a caught error is still caught.
    let mut r = full();
    run(
        &mut r,
        r#"local co = coroutine.wrap(function() local ok = pcall(function() coroutine.yield(1) end) return ok and 2 end)
        local a = co()
        local b = co()
        local ok, e = pcall(error, "boom")
        local ok2, e2 = xpcall(error, function(m) return "handled " .. m end, "x")
        realm.log(tostring(a), tostring(b), tostring(ok), e, tostring(ok2), e2)"#,
    );
    assert_eq!(logs(&r), ["1 2 false boom false handled x"]);
}

#[test]
fn zero_width_and_tag_characters_are_hidden_in_log_lines() {
    let mut r = full();
    run(&mut r, r#"realm.log("a\u{200b}b\u{feff}c\u{e0041}d")"#);
    assert_eq!(logs(&r), ["a b c d"]);
}
