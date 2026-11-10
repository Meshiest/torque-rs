//! The time a realm costs a frame (the justfile's `client-script-bench`): the real guest under Wasmtime, a client's GUI with its
//! realm layer, and a frame of the realms and the canvas's draw, timed.
//!
//! Runs of the same scene, each of 1,500 frames after a warm-up (the scene alone is run twice, for the noise between runs):
//! - no realm (the setting is "never"): what the scene costs alone;
//! - a hostile realm working at its caps, once for each of the attacks a review found: 1,024 controls with 16 images at the
//!   decoded cap (64 MiB) and a list of 1,024 rows, and every frame one attack of 13,000 to 18,000 steps (under a callback's
//!   20,000, past which the strike rule stops it; an op costs 25 steps and its bytes, a new control 10 more, an element of a
//!   list 6) and a message every other frame: `sizes`, 600 resizes of nested containers (moves where there are none);
//!   `churn`, 200 panels made in a panel and destroyed with it, twice; `images`, the images shown half and half in turn (each
//!   decodes once) beside 550 moves; `rows`, 2 reads of the list's rows; `select`, 600 changes of the list's selection. Its
//!   frame must stay within 3 ms of the scene alone at p99: the realm's budget holds its entries, the whole-flush checks, the
//!   applying of its ops (in pieces of 64) and the GUI's layouts and decodes for it, and an entry starts only when the budget
//!   left covers the last;
//! - an idle realm with 1,024 controls and nothing running per frame: its cost at p50 at most 0.3 ms over the scene alone.
//!
//! The four runs are made for each of four setups: with no fonts (the text draws nothing) and with a font (every text control draws
//! its glyphs), each with the controls loose in the realm's root and with them in four flex containers, laid out by the layout
//! plugin (the hostile realm then resizes items, and the layout moves the rest).
//! The numbers are split into the realm's frame and the canvas's draw, and the limits are asserted on the whole: the realm's
//! frame and the canvas drawing its controls, as a player's frame pays for both.
//!
//! What is guaranteed is the realm's work a frame, read from the GUI's counters with no frame dropped and held to limits the
//! budget gives (`MAX_FRAME_RECORDS`, `MAX_FRAME_UNITS`, `MAX_FRAME_DECODES_HERE`): a frame past one fails the bench at once,
//! as the counters are no clock and no noise moves them. The clock's p99 is asserted too: a run over a limit is made again,
//! up to three times, and the least counts, for each attack apart, since other work on the machine only adds time; a first run
//! whose p99 is over twice a limit fails at once. Each run's worst frame by the clock is printed, with how many frames a busy
//! loop of as much work stalled on the machine meanwhile, but not asserted: on a shared desktop a busy thread is stalled a
//! few times a run for 4 to 10 ms (a plain busy loop shows it), so the clock's worst frame says more of the machine than of the
//! realm.
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
use torque_fonts::{Fallback, Font, FontCache, Glyph, Remap, Tab, Units};
use torque_gui::GuiExtension;
use torque_gui::draw::{DrawCache, draw_canvas};
use torque_mods::CacheOwner;
use torque_plugin_client_script::messages::OfferedMod;
use torque_plugin_client_script::{
    Choices, ConsentAnswer, ImageReader, OfferSource, RealmState, Realms, RunnerFactory, Setting,
};
use torque_ts_vm::vm::Vm;

/// Fonts whose printable characters are 6 pixels apart, each glyph a 5 x 8 block of one sheet.
struct BlockFonts;

impl Fallback for BlockFonts {
    fn make(&self, _face: &str, _size: i32) -> Option<Font> {
        let glyph = Glyph {
            width: 5,
            height: 8,
            y_origin: 8,
            x_increment: 6,
            ..Glyph::default()
        };
        Some(Font {
            height: 10,
            baseline: 8,
            ascent: 8,
            descent: 2,
            glyphs: vec![glyph; 95],
            sheets: vec![torque_images::Bitmap {
                width: 8,
                height: 8,
                format: torque_images::Format::Alpha,
                bits: vec![255; 64],
            }],
            remap: Remap::new(32, (0..95).collect()),
            units: Units::Utf16,
            tab: Tab::Spaces,
        })
    }

    fn name(&self) -> &'static str {
        "block"
    }
}

/// The host's side of the GUI with a font: nothing else of an extension's.
struct Fonts;

impl GuiExtension for Fonts {
    fn into_any(self: Rc<Self>) -> Rc<dyn std::any::Any> {
        self
    }

    fn fonts(&self) -> Option<Box<dyn torque_gui::FontSource>> {
        Some(Box::new(FontCache::new("fonts", Box::new(BlockFonts))))
    }

    fn layer_stamp(&self, _gui: &torque_gui::Gui, _now: u32) -> Option<u64> {
        Some(0)
    }
}

const WARM_UP: usize = 200;
const FRAMES: usize = 1_500;

struct Real(RealmGuest);

impl RunnerFactory for Real {
    fn open(&self, limits: InstanceLimits) -> Result<Box<dyn Runner>, Trap> {
        Ok(Box::new(self.0.open(limits)?))
    }
}

/// The server's files: one script, and the images `img<n>.png`.
struct Source(String, Arc<[u8]>);

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

    fn images(&self, _vm: &Vm, _server: CacheOwner, _offered: &OfferedMod) -> ImageReader {
        let image = Arc::clone(&self.1);
        Box::new(move |path| path.starts_with("img").then(|| Arc::clone(&image)))
    }
}

/// A 2048 x 512 PNG of one color: 4 MiB decoded, so 16 of them are a realm's decoded cap.
fn image() -> Arc<[u8]> {
    let (w, h) = (2048, 512);
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        let mut wr = e.write_header().expect("the header");
        wr.write_image_data(&vec![90; (w * h * 3) as usize])
            .expect("the pixels");
    }
    out.into()
}

/// Makes its controls a hundred a frame (a file run of all of them would be past the steps a callback may take): 992 for the idle
/// realm; for the hostile one (`PROBE`, the attack it makes every frame) 16 images at the decoded cap, a list of 1,024 rows,
/// 600 controls and room for the 200 it makes and destroys. With `CONTAINERS` the first 512 are loose in the root and the rest
/// are the items of 16 flex containers, a column each, four of which hold a nested flex.
const SCRIPT: &str = r#"
local nodes = {}
local root = ui and ui.root()
local boxes, inner, images = {}, {}, {}
local list, rows = nil, {}
if CONTAINERS and ui then
  for c = 1, 16 do
    boxes[c] = ui.new("flex", { pos = { ((c - 1) % 8) * 75, 300 + ((c - 1) // 8) * 110 }, size = { 70, 100 }, direction = "column", gap = { 0, 1 } }, root)
  end
  if PROBE then
    for c = 1, 4 do inner[c] = ui.new("flex", { size = { 40, 20 } }, boxes[c]) end
  end
end
if PROBE and ui then
  for k = 1, 16 do
    images[k] = ui.new("image", { image = `mods/hud/img{k}.png`, pos = { (k - 1) * 36, 4 }, size = { 32, 32 } }, root)
  end
  for i = 1, 1024 do rows[i] = string.format("row %011d", i) end
  list = ui.new("list", { rows = rows, pos = { 600, 40 }, size = { 40, 200 } }, root)
end
local TARGET = PROBE and 600 or 992
local built = 0
local build
build = realm.every(0, function()
  if not ui then realm.cancel(build) return end
  for _ = 1, math.min(100, TARGET - built) do
    built += 1
    local at = { (built * 7) % 600, (built * 13) % 440 }
    local kind, parent = "panel", root
    if built % 4 == 0 then kind = "text" end
    local props = { pos = at, size = { 24, 12 }, text = "x" }
    if CONTAINERS and built > 512 then
      parent = boxes[built % 16 + 1]
      props.pos = nil
    end
    if kind == "panel" then props.text = nil end
    nodes[built] = ui.new(kind, props, parent)
  end
  if built >= TARGET then realm.cancel(build) end
end)
local function moves(frame, count)
  local loose = math.min(512, #nodes)
  for k = 1, count do
    local i = (frame * count + k) % loose + 1
    nodes[i]:set({ pos = { (i * 7 + frame) % 600, (i * 13) % 440 } })
  end
end
if PROBE then
  local frame = 0
  realm.onFrame(function(dt)
    frame += 1
    if #nodes < TARGET then return end
    if PROBE == "sizes" then
      if CONTAINERS then
        local side = 40 + (frame % 2) * 10
        for k = 1, 600 do inner[k % 4 + 1]:set({ size = { side, 20 } }) end
      else
        moves(frame, 600)
      end
    elseif PROBE == "churn" then
      for _ = 1, 2 do
        local holder = ui.new("panel", { size = { 600, 400 } }, root)
        for _ = 1, 200 do ui.new("panel", { size = { 4, 4 } }, holder) end
        holder:destroy()
      end
    elseif PROBE == "images" then
      for k = 1, 16 do images[k]:set({ visible = k % 2 == frame % 2 }) end
      moves(frame, 550)
    elseif PROBE == "rows" then
      for _ = 1, 2 do local _ = list:get("rows") end
    elseif PROBE == "select" then
      for k = 1, 600 do list:set({ selected = (frame + k) % 2 + 1 }) end
    end
    if net and frame % 2 == 0 then net.send({ frame = frame }) end
  end)
end
"#;

/// The attacks of the hostile realm, one a run.
const PROBES: [&str; 5] = ["sizes", "churn", "images", "rows", "select"];

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
    /// The realm's work a frame by the GUI's counters, not the clock: its `ui` records applied, the work units of applying
    /// them and of laying its containers out (each sorted), and the most images a frame decoded on its own thread.
    records: Vec<u64>,
    work: Vec<u64>,
    decoded_here: u32,
    realm: Vec<Duration>,
    /// The part of the realm's frame that is the GUI's own frame.
    gui: Vec<Duration>,
    draw: Vec<Duration>,
    total: Vec<Duration>,
}

/// How a run is set up: the GUI with a font or not, and with the layout plugin or not.
#[derive(Clone, Copy)]
struct Setup {
    fonts: bool,
    containers: bool,
}

impl Setup {
    fn label(self) -> String {
        let fonts = if self.fonts { "block font" } else { "no fonts" };
        let scene = if self.containers {
            "containers"
        } else {
            "flat"
        };
        format!("{scene}, {fonts}")
    }

    /// The script of a realm that moves things, or not.
    /// The script of a realm that makes the attack `probe` every frame, or of an idle one.
    fn script(self, probe: Option<&str>) -> String {
        let probe = probe.map_or_else(|| "nil".to_string(), |p| format!("{p:?}"));
        format!(
            "PROBE = {probe}
CONTAINERS = {}
{SCRIPT}",
            self.containers
        )
    }
}

/// The frame times of `FRAMES` frames of the scene with a realm running `script` (`None`: no realm).
fn run(
    guest: &RealmGuest,
    script: Option<String>,
    tiers: &[Tier],
    setup: Setup,
    image: &Arc<[u8]>,
) -> Times {
    let fonts = setup.fonts;
    let mut w = World::with_layout(
        Setting::Never,
        Grants::of(&[Tier::Ui, Tier::Net]),
        Choices::in_memory(),
        setup.containers,
    );
    if fonts {
        w.gui.set_extension(Rc::new(Fonts));
    }
    let running = script.is_some();
    if let Some(script) = script {
        let mut realms = Realms::new(Choices::in_memory(), Some(Box::new(Real(guest.clone()))));
        realms.setting = Setting::Ask;
        realms.offerable = Grants::of(tiers);
        realms.set_prompt(Box::new(TestPrompt(Rc::clone(&w.asked))));
        realms.set_source(Box::new(Source(script, Arc::clone(image))));
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
    let mut decodes = 0u32;
    let mut times = Times {
        records: Vec::with_capacity(FRAMES),
        work: Vec::with_capacity(FRAMES),
        decoded_here: 0,
        realm: Vec::with_capacity(FRAMES),
        gui: Vec::with_capacity(FRAMES),
        draw: Vec::with_capacity(FRAMES),
        total: Vec::with_capacity(FRAMES),
    };
    let counted = |w: &World| {
        let stats = w.gui.client_script_stats();
        (stats.applied_records, stats.apply_work + stats.layout_work)
    };
    for n in 0..WARM_UP + FRAMES {
        let before = counted(&w);
        let started = Instant::now();
        w.frame();
        // The client's own frame of the GUI: the layers, the chip, and the layout of the realm's containers.
        let gui_started = Instant::now();
        w.gui.frame(&mut w.vm);
        let gui_took = gui_started.elapsed();
        let framed = started.elapsed();
        let frame = draw_canvas(&w.vm, &w.gui, &mut cache);
        let took = started.elapsed();
        decodes += frame.draws.report.realm_decodes;
        std::hint::black_box(&frame);
        let after = counted(&w);
        if n >= WARM_UP {
            times.records.push(after.0 - before.0);
            times.work.push(after.1 - before.1);
            times.decoded_here = times
                .decoded_here
                .max(frame.draws.report.realm_decoded_here);
            times.realm.push(framed);
            times.gui.push(gui_took);
            times.draw.push(took - framed);
            times.total.push(took);
        }
    }
    if running && w.realms.state() != RealmState::Running {
        let log: Vec<String> =
            w.vm.drain_log()
                .iter()
                .map(|l| String::from_utf8_lossy(&l.text).into_owned())
                .filter(|l| l.contains("scripts"))
                .collect();
        panic!("[{}] the realm was stopped: {log:?}", setup.label());
    }
    if running {
        let stats = w.gui.client_script_stats();
        println!(
            "bench [{}] in {} frames: the layout ran {} times ({} work), {} resizes deferred to it, {} images decoded",
            setup.label(),
            WARM_UP + FRAMES,
            stats.layout_updates,
            stats.layout_work,
            stats.deferred_layouts,
            decodes
        );
        if setup.containers {
            assert!(stats.layout_updates > 0, "the containers were laid out");
        }
        assert!(decodes <= 16, "each image decoded once ({decodes})");
    }
    times.records.sort_unstable();
    times.work.sort_unstable();
    for list in [
        &mut times.realm,
        &mut times.gui,
        &mut times.draw,
        &mut times.total,
    ] {
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

/// The least time a `ui` record takes to check and apply: a frame applies pieces until its budget is spent, and one more.
const LEAST_RECORD: Duration = Duration::from_nanos(500);
/// The most records a frame applies: its budget's worth at [`LEAST_RECORD`] each, and the piece past it.
const MAX_FRAME_RECORDS: u64 = (REALM_BUDGET.as_nanos() / LEAST_RECORD.as_nanos()) as u64 + PIECE;
/// The records of a piece (the host's pieces of a flush).
const PIECE: u64 = 64;
/// The most work units (`Vm::charge_work`) a frame's applying and layouts take: its budget's worth of Luau steps, at the units
/// a step stands for.
const MAX_FRAME_UNITS: u64 = (REALM_BUDGET.as_nanos() as u64
    / torque_client_script_host::STEP_NANOS)
    * torque_ts_vm::vm::WORK_PER_STEP;
/// The most of the bench's images (4 MiB decoded each) a frame decodes on its own thread: what of its allowance for that one
/// takes, none.
const MAX_FRAME_DECODES_HERE: u32 =
    (torque_gui::draw::REALM_DECODE_HERE_FRAME / (4 * 2048 * 512)) as u32;

/// What a run's work counters say is past the budget's limits, if anything.
fn work_over(times: &Times) -> Option<String> {
    let records = times.records.last().copied().unwrap_or(0);
    if records > MAX_FRAME_RECORDS {
        return Some(format!(
            "a frame applied {records} records (at most {MAX_FRAME_RECORDS})"
        ));
    }
    let units = times.work.last().copied().unwrap_or(0);
    if units > MAX_FRAME_UNITS {
        return Some(format!(
            "a frame took {units} work units (at most {MAX_FRAME_UNITS})"
        ));
    }
    (times.decoded_here > MAX_FRAME_DECODES_HERE).then(|| {
        format!(
            "a frame decoded {} images on its own thread (at most {MAX_FRAME_DECODES_HERE})",
            times.decoded_here
        )
    })
}

/// How many of `frames` frames busy for `busy` each (a run's mean) the machine stalls past 2 ms more: a busy loop of as much
/// work, run now.
fn stalls(busy: Duration, frames: usize) -> usize {
    let mut x = 1u64;
    let stalled = (0..frames)
        .filter(|_| {
            let started = Instant::now();
            while started.elapsed() < busy {
                x = std::hint::black_box(x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1));
            }
            started.elapsed() > busy + Duration::from_millis(2)
        })
        .count();
    std::hint::black_box(x);
    stalled
}

/// The most times a setup's four runs are made when the limits are not met: a machine doing other work only ever adds time to a
/// frame, so the least of the runs is the one that says what the frame costs.
const ATTEMPTS: usize = 3;

/// A first run this many times over a limit is no noise: the bench fails at once.
const FAILS_AT_ONCE: f64 = 2.0;

const HOSTILE_LIMIT_MS: f64 = 3.0;
/// The realm's frame budget natively ([`torque_client_script_host::Settings::native`] at 60 frames a second).
const REALM_BUDGET: Duration = Duration::from_micros(1500);
const IDLE_LIMIT_MS: f64 = 0.3;

/// How far over the scene alone a hostile realm's frame is at p99 (limit 3 ms) and an idle realm's at p50 (limit 0.3 ms), in
/// milliseconds, from four runs of `FRAMES` frames: the scene alone twice, the hostile realm and the idle one. The whole frame:
/// the realm's and the canvas drawing its controls.
fn measure(
    guest: &RealmGuest,
    tiers: &[Tier],
    setup: Setup,
    attempt: usize,
    image: &Arc<[u8]>,
) -> ([f64; PROBES.len()], f64, Vec<String>) {
    let label = setup.label();
    let alone = run(guest, None, tiers, setup, image);
    let alone_again = run(guest, None, tiers, setup, image);
    let idle = run(guest, Some(setup.script(None)), tiers, setup, image);

    let line = |name: &str, t: &Times| {
        let part = |what: &str, t: &[Duration]| {
            format!(
                "{what} min {:.3} p50 {:.3} p99 {:.3} max {:.3}",
                ms(t[0]),
                ms(at(t, 50)),
                ms(at(t, 99)),
                ms(*t.last().expect("frames"))
            )
        };
        println!(
            "bench [{label}, run {attempt}] {name}: total {} | {} | {} (ms)",
            part("", &t.total),
            part("realm", &t.realm),
            part("draw", &t.draw)
        );
        println!(
            "bench [{label}, run {attempt}] {name}: of the realm's frame, the GUI's own: {}",
            part("gui", &t.gui)
        );
    };
    line("scene alone", &alone);
    line("scene alone again", &alone_again);
    line("idle realm, 1,024 controls", &idle);
    let alone_p99 = at(&alone.total, 99).min(at(&alone_again.total, 99));
    let alone_p50 = at(&alone.total, 50).min(at(&alone_again.total, 50));
    let idle_over = ms(at(&idle.total, 50)) - ms(alone_p50);
    // Each attack: its work counters against the budget's limits (nothing dropped), its p99 by the clock, and its worst frame
    // by the clock beside the stalls a busy loop of as much work had meanwhile (printed, not asserted).
    let mut p99s = [f64::MIN; PROBES.len()];
    let mut over = Vec::new();
    for (slot, probe) in p99s.iter_mut().zip(PROBES) {
        let hostile = run(guest, Some(setup.script(Some(probe))), tiers, setup, image);
        line(&format!("hostile realm, {probe}"), &hostile);
        let work = work_over(&hostile);
        println!(
            "bench [{label}, run {attempt}] hostile realm, {probe}: work a frame at most {} records (limit {MAX_FRAME_RECORDS}), {} units (limit {MAX_FRAME_UNITS}), {} images decoded on its thread (limit {MAX_FRAME_DECODES_HERE}){}",
            hostile.records.last().copied().unwrap_or(0),
            hostile.work.last().copied().unwrap_or(0),
            hostile.decoded_here,
            work.as_deref().map_or(String::new(), |w| format!(": {w}"))
        );
        if let Some(work) = work {
            over.push(format!("[{label}] {probe}: {work}"));
        }
        let busy = hostile.total.iter().sum::<Duration>() / hostile.total.len() as u32;
        let stalls = stalls(busy, hostile.total.len());
        let worst = ms(*hostile.total.last().expect("frames")) - ms(alone_p99);
        let p99 = ms(at(&hostile.total, 99)) - ms(alone_p99);
        println!(
            "bench [{label}, run {attempt}] hostile realm, {probe}: p99 {p99:+.3} ms over the scene alone's (limit {HOSTILE_LIMIT_MS}); worst frame {worst:+.3} ms, with {stalls} frames a busy loop of as much work was stalled for (not asserted)"
        );
        *slot = p99;
    }
    println!(
        "bench [{label}, run {attempt}] idle realm: p50 {idle_over:+.3} ms (limit {IDLE_LIMIT_MS})"
    );
    (p99s, idle_over, over)
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
    let image = image();

    let mut failures = Vec::new();
    for containers in [false, true] {
        for fonts in [false, true] {
            let setup = Setup { fonts, containers };
            let label = setup.label();
            // Each attack's p99 is the least of its attempts; its work counters fail it at once.
            let (mut hostile, mut idle) = ([f64::MAX; PROBES.len()], f64::MAX);
            for attempt in 1..=ATTEMPTS {
                let (h, i, work) = measure(&guest, &tiers, setup, attempt, &image);
                assert!(work.is_empty(), "{}", work.join("; "));
                if attempt == 1 {
                    let p99 = h.iter().copied().fold(f64::MIN, f64::max);
                    assert!(
                        p99 <= HOSTILE_LIMIT_MS * FAILS_AT_ONCE
                            && i <= IDLE_LIMIT_MS * FAILS_AT_ONCE,
                        "[{label}] the first run is over twice a limit: hostile p99 {p99:.3} ms (limit {HOSTILE_LIMIT_MS}), idle {i:.3} ms (limit {IDLE_LIMIT_MS})"
                    );
                }
                for (best, p99) in hostile.iter_mut().zip(h) {
                    *best = best.min(p99);
                }
                idle = idle.min(i);
                if hostile.iter().all(|p| *p <= HOSTILE_LIMIT_MS) && idle <= IDLE_LIMIT_MS {
                    break;
                }
                println!("bench [{label}] over a limit in run {attempt} of {ATTEMPTS}");
            }
            for (probe, p99) in PROBES.iter().zip(hostile) {
                println!(
                    "bench [{label}] hostile realm, {probe}: p99 {p99:+.3} ms at best (limit {HOSTILE_LIMIT_MS})"
                );
                if p99 > HOSTILE_LIMIT_MS {
                    failures.push(format!(
                        "[{label}] the hostile realm's frame at p99 under {probe} is {p99:.3} ms over the scene alone at best"
                    ));
                }
            }
            if idle > IDLE_LIMIT_MS {
                failures.push(format!(
                    "[{label}] the idle realm's frame is {idle:.3} ms over the scene alone at p50 at best"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}
