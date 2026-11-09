//! The time a realm costs a frame (the justfile's `client-script-bench`): the real guest under Wasmtime, a client's GUI with its
//! realm layer, and a frame of the realms and the canvas's draw, timed.
//!
//! Runs of the same scene, each of 1,500 frames after a warm-up (the scene alone is run twice, for the noise between runs):
//! - no realm (the setting is "never"): what the scene costs alone;
//! - a hostile realm working at its caps: a thousand controls, a quarter of them moved every frame (250 `set` ops) and a message
//!   every other frame (any more steps a callback and the host stops the realm: the strike rule, tested elsewhere); its frame
//!   time at p99 must stay within 3 ms of the scene alone (its frame budget is 2 ms, which an entry that is not interrupted may
//!   overrun a little);
//! - an idle realm with 1,024 controls and nothing running per frame: its own cost at p50 at most 0.3 ms.
//!
//! The numbers are split into the realm's frame and the canvas's draw. The limits are asserted on the realm's own part; the
//! totals are printed, and the canvas's cost of drawing the realm's controls (about 0.7 ms for 1,024) is what puts them over.
//!
//! The test is ignored in an ordinary run: its numbers are wall-clock times, and `client-script-bench` runs it under the
//! machine's bench lock. It needs the guest (`just lua-client-web`) at TORQUE_CLIENT_GUEST, else the build folder's `lua-web`.

mod common;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use torque_client_script_format::{Grants, Tier};
use torque_client_script_host::{GrantSet, GuestEngine, InstanceLimits, RealmGuest, Runner, Trap};
use torque_gui::draw::{DrawCache, draw_canvas};
use torque_mods::CacheOwner;
use torque_plugin_client_script::messages::OfferedMod;
use torque_plugin_client_script::{
    Choices, ConsentAnswer, OfferSource, RealmState, Realms, RunnerFactory, Setting,
};
use torque_ts_vm::vm::Vm;

const WARM_UP: usize = 200;
const FRAMES: usize = 1_500;

struct Real(RealmGuest);

impl RunnerFactory for Real {
    fn open(&self, limits: InstanceLimits) -> Result<Box<dyn Runner>, Trap> {
        Ok(Box::new(self.0.open(limits)?))
    }
}

/// The server's files: one script.
struct Source(String);

impl OfferSource for Source {
    fn ready(&self, _vm: &Vm, _server: CacheOwner, _offered: &OfferedMod) -> bool {
        true
    }

    fn content(
        &self,
        _vm: &Vm,
        _server: CacheOwner,
        offered: &OfferedMod,
    ) -> Option<Vec<(String, Arc<[u8]>)>> {
        let script: Arc<[u8]> = Arc::from(self.0.as_bytes());
        Some(vec![(
            format!("mods/{}/{}", offered.id, offered.entry),
            script,
        )])
    }
}

/// Makes a thousand controls, a hundred a frame (a file run of all of them would be past the steps a callback may take), then
/// (when `MOVING`) moves a window of 250 of them every frame and sends a message every other one.
const SCRIPT: &str = r#"
local nodes = {}
local root = ui and ui.root()
local built = 0
local build
build = realm.every(0, function()
  if not ui then realm.cancel(build) return end
  for _ = 1, 100 do
    built += 1
    local at = { (built * 7) % 600, (built * 13) % 440 }
    if built % 4 == 0 then
      nodes[built] = ui.new("text", { pos = at, size = { 24, 12 }, text = "x" }, root)
    else
      nodes[built] = ui.new("panel", { pos = at, size = { 24, 12 } }, root)
    end
  end
  if built >= 1000 then realm.cancel(build) end
end)
if MOVING then
  local frame = 0
  realm.onFrame(function(dt)
    frame += 1
    local count = #nodes
    for k = 1, math.min(250, count) do
      local i = (frame * 250 + k) % count + 1
      nodes[i]:set({ pos = { (i * 7 + frame) % 600, (i * 13) % 440 } })
    end
    if net and frame % 2 == 0 then net.send({ frame = frame }) end
  end)
end
"#;

fn guest_bytes() -> Option<Vec<u8>> {
    let path = std::env::var_os("TORQUE_CLIENT_GUEST").map_or_else(
        || {
            let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
                || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"),
                PathBuf::from,
            );
            target.join("lua-web").join("torque_lua_client.wasm")
        },
        PathBuf::from,
    );
    match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            assert!(
                std::env::var_os("TORQUE_CLIENT_GUEST_REQUIRED").is_none(),
                "{}: {error} (build it with `just lua-client-web`)",
                path.display()
            );
            None
        }
    }
}

/// The tiers the bench grants, from TORQUE_CLIENT_SCRIPT_BENCH_TIERS (`ui,net` by default).
fn tiers() -> Vec<Tier> {
    let text =
        std::env::var("TORQUE_CLIENT_SCRIPT_BENCH_TIERS").unwrap_or_else(|_| "ui,net".into());
    text.split(',')
        .map(|name| match Tier::from_name(name.trim()) {
            Some(tier @ (Tier::Ui | Tier::Net)) => tier,
            _ => panic!("`{name}` is no tier this build has a bench for (ui, net)"),
        })
        .collect()
}

/// What a run took a frame, sorted: the realms' frame (the guest's entries, applying their ops, the messages) and the canvas's
/// draw, each alone and together.
struct Times {
    realm: Vec<Duration>,
    draw: Vec<Duration>,
    total: Vec<Duration>,
}

/// The frame times of `FRAMES` frames of the scene with a realm running `script` (`None`: no realm).
fn run(guest: &RealmGuest, script: Option<String>, tiers: &[Tier]) -> Times {
    let mut w = World::new(
        Setting::Never,
        Grants::of(&[Tier::Ui, Tier::Net]),
        Choices::in_memory(),
    );
    let running = script.is_some();
    if let Some(script) = script {
        let mut realms = Realms::new(Choices::in_memory(), Some(Box::new(Real(guest.clone()))));
        realms.setting = Setting::Ask;
        realms.offerable = Grants::of(tiers);
        realms.set_prompt(Box::new(TestPrompt(Rc::clone(&w.asked))));
        realms.set_source(Box::new(Source(script)));
        realms.set_grant_set(|_, allowed| GrantSet::unreviewed(allowed));
        w.realms = realms;
        let names: Vec<&str> = tiers.iter().map(|t| t.name()).collect();
        w.offer(&names);
        w.frames(3);
        *w.asked.answer.borrow_mut() = Some(ConsentAnswer::Allow {
            ticked: tiers.iter().map(|t| (*t, true)).collect(),
            remember: false,
        });
        w.frames(3);
        assert_eq!(w.realms.state(), RealmState::Running);
    }
    let mut cache = DrawCache::for_gui(&w.gui);
    let mut times = Times {
        realm: Vec::with_capacity(FRAMES),
        draw: Vec::with_capacity(FRAMES),
        total: Vec::with_capacity(FRAMES),
    };
    for n in 0..WARM_UP + FRAMES {
        let started = Instant::now();
        w.frame();
        let framed = started.elapsed();
        let frame = draw_canvas(&w.vm, &w.gui, &mut cache);
        let took = started.elapsed();
        std::hint::black_box(&frame);
        if n >= WARM_UP {
            times.realm.push(framed);
            times.draw.push(took - framed);
            times.total.push(took);
        }
    }
    if running {
        assert_eq!(
            w.realms.state(),
            RealmState::Running,
            "the realm was not stopped"
        );
    }
    for list in [&mut times.realm, &mut times.draw, &mut times.total] {
        list.sort();
    }
    times
}

fn at(times: &[Duration], percent: usize) -> Duration {
    times[(times.len() * percent / 100).min(times.len() - 1)]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000.0
}

#[test]
#[ignore = "a timing run: the justfile's `client-script-bench` runs it under the bench lock"]
fn a_realm_keeps_the_frame_within_the_budgets() {
    let Some(bytes) = guest_bytes() else {
        println!("no guest built: skipped");
        return;
    };
    let tiers = tiers();
    let engine = GuestEngine::new(Duration::from_millis(5)).expect("an engine");
    let guest = RealmGuest::new(&engine, &bytes).expect("the pinned guest links");

    let alone = run(&guest, None, &tiers);
    let hostile = run(
        &guest,
        Some(format!(
            "MOVING = true
{SCRIPT}"
        )),
        &tiers,
    );
    let alone_again = run(&guest, None, &tiers);
    let idle = run(
        &guest,
        Some(format!(
            "MOVING = false
{SCRIPT}"
        )),
        &tiers,
    );

    let line = |name: &str, t: &Times| {
        let part = |what: &str, t: &[Duration]| {
            format!(
                "{what} p50 {:.3} p99 {:.3} max {:.3}",
                ms(at(t, 50)),
                ms(at(t, 99)),
                ms(*t.last().expect("frames"))
            )
        };
        println!(
            "bench {name}: total {} | {} | {} (ms)",
            part("", &t.total),
            part("realm", &t.realm),
            part("draw", &t.draw)
        );
    };
    line("scene alone", &alone);
    line("scene alone again", &alone_again);
    line("hostile realm", &hostile);
    line("idle realm, 1,024 controls", &idle);

    // The realm's own cost (its guest, applying its ops, its messages) against the limits; the canvas drawing a thousand controls is
    // the scene's and is reported beside it, not hidden in it.
    let hostile_realm = ms(at(&hostile.realm, 99));
    let idle_realm = ms(at(&idle.realm, 50));
    println!(
        "bench hostile realm's own cost: p99 {hostile_realm:.3} ms (limit 3 over the scene alone; its frame budget is 2)"
    );
    println!("bench idle realm's own cost: p50 {idle_realm:.3} ms (limit 0.3)");
    let alone_p99 = at(&alone.total, 99).min(at(&alone_again.total, 99));
    let alone_p50 = at(&alone.total, 50).min(at(&alone_again.total, 50));
    let hostile_over = ms(at(&hostile.total, 99)) - ms(alone_p99);
    let idle_over = ms(at(&idle.total, 50)) - ms(alone_p50);
    println!(
        "bench with the canvas drawing the realm's controls: hostile p99 {hostile_over:+.3} ms over the scene alone (limit 3), idle p50 {idle_over:+.3} ms (limit 0.3)"
    );
    assert!(hostile_realm <= 3.0, "{hostile_realm:.3} ms");
    assert!(idle_realm <= 0.3, "{idle_realm:.3} ms");
}
