//! Parity: each case's observable stream (the log lines, the `ui` records and messages the host applied, the calls, the
//! revocations and the stop) is folded into one number, and that number must be the same on every runner: natively under
//! Wasmtime, under Node and in Chrome. The realm guest is one file and these cases run it as such, so a difference is a
//! runner that changed what a script saw (a float printed another way, a step counted differently, an op in another order).
//!
//! `parity.txt` holds the numbers. A case that depends on the wall clock (a deadline, a time budget) or on how far behind a
//! runner's results arrive (a web runner's frame has its ops applied in the next one, so a revocation between frames drops one
//! frame's ops there and none natively) is listed in [`RUNNER_DEPENDENT`] and has none. A change that moves a number on purpose is written back by running the native cases with
//! `TORQUE_PARITY_BLESS=1`.

use std::time::Duration;

use torque_client_script_format::{Grants, Tier};

use super::{Action, Guest, expect};
use crate::{FrameBudget, Settings};

/// The numbers of the cases that have one: a name and 16 lower-case hexadecimal digits a line.
pub const TABLE: &str = include_str!("parity.txt");

/// The cases whose behavior reads the wall clock or the depth of a runner's pipeline, so their streams may differ between runs and
/// runners; each passes its own checks on every runner.
pub const RUNNER_DEPENDENT: &[&str] = &[
    "the_deadline_stops_a_long_shared_prefix_compare",
    "the_deadline_stops_concatenating_onto_a_mib",
    "the_deadline_stops_iterating_a_sparse_table",
    "the_deadline_stops_pattern_backtracking",
    "the_deadline_stops_a_loop_without_imports",
    "a_guest_past_its_memory_traps_and_only_it_stops",
    "a_realm_over_budget_too_often_runs_every_other_frame",
    "os_clock_is_the_instances_own",
    "a_guest_at_its_step_caps_every_frame_is_struck_off",
    "a_guest_holding_its_entries_past_the_long_entry_time_is_struck_off",
    "the_ui_functions_cost_steps_for_what_they_handle",
    "three_limit_stops_in_ten_seconds_turn_the_realm_off",
    "limit_stops_further_apart_do_not",
    "an_op_of_a_revoked_tier_stops_the_realm_once_it_was_told",
    "revoking_removes_the_tier_in_the_same_frame",
];

/// The number `name` must have, if it has one.
pub fn expected(name: &str) -> Option<u64> {
    TABLE
        .lines()
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| {
            let (case, digits) = line.split_once(' ')?;
            (case == name).then(|| u64::from_str_radix(digits.trim(), 16).ok())?
        })
}

/// A 64-bit FNV-1a fold of what a realm did, tagged and length-prefixed so that two streams that split the same bytes
/// differently differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trace(u64);

impl Default for Trace {
    fn default() -> Trace {
        Trace(0xcbf2_9ce4_8422_2325)
    }
}

impl Trace {
    /// Folds in one item of kind `tag`.
    pub fn put(&mut self, tag: u8, bytes: &[u8]) {
        let mut fold = |b: u8| self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        fold(tag);
        // A fixed width, not `usize`'s, which a 32-bit target (the web's) does not share.
        (bytes.len() as u64)
            .to_le_bytes()
            .into_iter()
            .for_each(&mut fold);
        bytes.iter().copied().for_each(fold);
    }

    /// Folds in another realm's trace.
    pub fn merge(&mut self, other: Trace) {
        self.put(0xff, &other.0.to_le_bytes());
    }

    /// The number.
    pub fn digest(self) -> u64 {
        self.0
    }
}

/// Settings that make a realm's run a function of its script alone: no wall-clock rule can fire.
fn steady(settings: &mut Settings) {
    settings.budget = FrameBudget::ApplyOnly {
        cap: Duration::from_secs(3600),
    };
    settings.long_entry = Duration::from_secs(3600);
    settings.deadline = Duration::from_secs(30);
}

/// [`steady`], with step caps small enough that a few thousand steps meet them.
fn small_caps(settings: &mut Settings) {
    steady(settings);
    settings.callback_steps = 6_000;
    settings.frame_steps = 9_000;
}

/// A run of one script for `frames`, which must end with `ERRORS` script errors in its log.
fn run_script<const ERRORS: usize>(
    scripts: &'static [(&'static str, &'static str)],
    tune: fn(&mut Settings),
    grants: Grants,
    frames: u32,
) -> Vec<Action> {
    vec![
        Action::Open {
            slot: 0,
            guest: Guest::Realm,
            grants,
            scripts,
            tune,
            apply_cost: Duration::ZERO,
        },
        Action::Frames {
            slot: 0,
            count: frames,
        },
        Action::Check(|w| {
            let effects = w.slot(0).effects();
            if std::env::var_os("TORQUE_PARITY_LOGS").is_some() {
                println!("{:#?}", effects.logs);
            }
            expect(!effects.logs.is_empty(), "the script said what it saw")?;
            expect(
                effects.errors.len() == ERRORS,
                &format!(
                    "the script's errors were {:?}, not {ERRORS}",
                    effects.errors
                ),
            )
        }),
    ]
}

const UI_NET: Grants = Grants(0b111);

/// The head of a script that has more to say than the log's 20 lines a second take: `say` queues a line and a timer logs
/// one every 0.06 s.
macro_rules! prelude {
    () => {
        r##"
local pending = {}
local function say(line) pending[#pending + 1] = line end
realm.every(0.06, function() local line = table.remove(pending, 1) if line then realm.log(line) end end)
"##
    };
}

/// Frames for the queued lines of the `say` scripts to come out, at one every 0.06 s.
const FRAMES_FOR_SAYING: u32 = 300;

/// Floating-point arithmetic, its printing and `math`'s functions.
pub fn numbers() -> Vec<Action> {
    run_script::<0>(
        &[(
            "main.lua",
            concat!(
                prelude!(),
                r##"
local samples = { 0.1, 1 / 3, 2 ^ 53, 2 ^ -1074, 1e308, -0.0, 123456789.123456789, 5e-324, 0.30000000000000004 }
for _, x in samples do
  say(string.format("%.17g|%g|%8.3f|%e", x, x, x, x))
  say(tostring(x) .. " " .. tostring(x * x) .. " " .. tostring(x / 3) .. " " .. tostring(-x))
end
for i = 1, 12 do
  local x = i * 0.37
  say(table.concat({
    tostring(math.sin(x)), tostring(math.cos(x)), tostring(math.tan(x)), tostring(math.exp(x)), tostring(math.log(x)),
    tostring(math.sqrt(x)), tostring(x ^ 1.5), tostring(math.fmod(x, 0.7)), tostring(math.atan(x)), tostring(math.sinh(x)),
    tostring(math.log10(x)), tostring(math.noise(x, x * 2, x * 3)),
  }, " "))
end
local z = 0
say(tostring(z / z) .. " " .. tostring(1 / z) .. " " .. tostring(-1 / z) .. " " .. tostring(math.huge - math.huge))
say(tostring(math.floor(-0.5)) .. " " .. tostring(math.ceil(-0.5)) .. " " .. tostring(math.round(2.5)) .. " " .. tostring(2 ^ 63) .. " " .. tostring(-2 ^ 63))
say(tostring(7 // 2) .. " " .. tostring(-7 // 2) .. " " .. tostring(7 % -3) .. " " .. tostring(-7 % 3) .. " " .. tostring(5.5 // 2))
say(tostring(bit32.band(0xff00, 0x0ff0)) .. " " .. tostring(bit32.lshift(1, 31)) .. " " .. tostring(bit32.rrotate(0x12345678, 8)) .. " " .. tostring(bit32.countlz(1)))
math.randomseed(42)
local r = {}
for i = 1, 16 do r[i] = math.random(1, 1000) end
say(table.concat(r, ","))
say(tostring(math.random()) .. " " .. tostring(math.random(5)))
"##
            ),
        )],
        steady,
        Grants(1),
        FRAMES_FOR_SAYING,
    )
}

/// Strings, patterns, sorting and the tables' iteration order.
pub fn strings() -> Vec<Action> {
    run_script::<0>(
        &[(
            "main.lua",
            concat!(
                prelude!(),
                r##"
local text = "The quick brown fox jumps over the lazy dog, 12 times; 3.5 or -7e2 maybe."
say(text:upper() .. "|" .. text:lower() .. "|" .. text:reverse() .. "|" .. #text)
say(tostring(text:find("quick")) .. " " .. tostring(text:find("%d+")) .. " " .. tostring(text:match("(%a+) fox")) .. " " .. tostring(text:find("o", 20, true)))
say((text:gsub("%w+", function(w) return #w end)))
say((text:gsub("(%a)(%a*)", "%2%1ay")))
for number in text:gmatch("-?%d+%.?%d*e?%d*") do say("number " .. number) end
say(string.format("%q|%5d|%-5d|%05d|%x|%X|%o|%c|%s|%10.4s|%%", "a\n\"b\0c", 42, 42, 42, 255, 255, 8, 65, "hello", "truncated"))
say(string.format("%i %u %5.1f %g %g", 12, 12, 3.14159, 1e20, 1e-5))
say(tostring(utf8.len("héllo wörld ✓")) .. " " .. utf8.char(72, 228, 8364, 128512) .. " " .. tostring(utf8.codepoint("€", 1)))
for p, c in utf8.codes("aé✓") do say("codepoint " .. p .. " " .. c) end
say(string.rep("ab", 5, "-") .. " " .. ("x"):rep(3) .. " " .. table.concat({ 1, 2, 3 }, "+") .. " " .. string.byte("A") .. " " .. string.char(104, 105))
say(tostring(string.pack("<i4", 1234567):byte(1, -1)) .. " " .. tostring(string.unpack("<i4", string.pack("<i4", -2))))
local items = {}
for i = 1, 40 do items[i] = { key = (i * 7919) % 13, id = i, name = "n" .. ((i * 31) % 17) } end
table.sort(items, function(a, b) if a.key ~= b.key then return a.key < b.key end return a.id < b.id end)
local ids = {}
for i, item in items do ids[i] = item.id end
say(table.concat(ids, ","))
local words = { "pear", "apple", "fig", "banana", "cherry", "date", "elderberry", "grape" }
table.sort(words)
say(table.concat(words, " "))
table.sort(words, function(a, b) return #a < #b or (#a == #b and a < b) end)
say(table.concat(words, " "))
local keys = {}
for k in { alpha = 1, beta = 2, gamma = 3, delta = 4, epsilon = 5, zeta = 6, eta = 7, theta = 8 } do keys[#keys + 1] = k end
table.sort(keys)
say(table.concat(keys, " "))
local sparse = {}
for i = 1, 30 do sparse[i * i] = i end
local order = {}
for k, v in sparse do order[#order + 1] = k .. "=" .. v end
table.sort(order)
say(table.concat(order, " "))
say(tostring(#{ 1, 2, 3, nil, 5 }) .. " " .. tostring(select("#", table.unpack({ 1, nil, 3 }, 1, 3))) .. " " .. tostring(next({})) .. " " .. type(table.clone({ 1 })))
say(tostring(table.find({ 5, 6, 7 }, 7)) .. " " .. tostring(table.pack(1, 2, 3).n) .. " " .. table.concat(table.move({ 1, 2, 3 }, 1, 3, 2, {}), ",", 2, 4))
local b = buffer.create(16)
buffer.writeu32(b, 0, 0xdeadbeef) buffer.writef64(b, 8, 1 / 3)
say(tostring(buffer.readu32(b, 0)) .. " " .. tostring(buffer.readf64(b, 8)) .. " " .. buffer.tostring(b):byte(1, 4))
"##
            ),
        )],
        steady,
        Grants(1),
        FRAMES_FOR_SAYING,
    )
}

/// Controls of every kind, their changes and a few timers.
pub fn ui_tree() -> Vec<Action> {
    run_script::<0>(
        &[(
            "main.lua",
            r##"
local root = ui.root()
local panel = ui.new("panel", { pos = { 10, 10 }, size = { 300, 200 }, fill = { 0.1, 0.2, 0.3, 0.5 } }, root)
local title = ui.new("text", { text = "Title <b>not markup</b>", pos = { 4, 4 }, size = { 200, 20 }, fontSize = 18 }, panel)
local rich = ui.new("richtext", { text = "<b>bold</b> and <color:ff8800>orange</color>", pos = { 4, 30 }, size = { 200, 40 } }, panel)
local button = ui.new("button", { text = "Go", pos = { 4, 80 }, size = { 80, 24 } }, panel)
local toggle = ui.new("toggle", { text = "On", pos = { 100, 80 }, size = { 80, 24 } }, panel)
local field = ui.new("textfield", { placeholder = "name", pos = { 4, 110 }, size = { 120, 24 }, maxLength = 16 }, panel)
local slider = ui.new("slider", { range = { 0, 10 }, value = 3, pos = { 4, 140 }, size = { 120, 16 } }, panel)
local list = ui.new("list", { rows = { "one", "two", "three" }, selected = 2, pos = { 130, 110 }, size = { 100, 60 } }, panel)
button:on("press", function() realm.log("pressed") end)
local count = 0
realm.every(0.1, function()
  count += 1
  title:set({ text = "Ticks " .. count })
  slider:set({ value = count % 11 })
  if count == 3 then toggle:set({ visible = false }) end
  if count == 5 then list:set({ rows = { "a", "b" }, selected = 1 }) end
  if count == 7 then rich:destroy() end
  realm.log("tick " .. count)
end)
realm.after(0.35, function() panel:set({ opacity = 0.5 }) realm.log("after") end)
realm.onFrame(function(dt) if dt > 1 then realm.log("slow frame") end end)
realm.log("built")
"##,
        )],
        steady,
        UI_NET,
        60,
    )
}

/// Layout containers, their items and the numbers their props are made of (lengths, shares, sides, tracks).
pub fn layout_tree() -> Vec<Action> {
    run_script::<0>(
        &[(
            "main.lua",
            r##"
local root = ui.root()
local menu = ui.new("flex", { size = { 300, 200 }, direction = "column", gap = { 4, 8.5 }, padding = { 6, 10 },
  justifyContent = "space-between", alignItems = "center", wrap = "wrap" }, root)
local grid = ui.new("grid", { size = { 280, 100 }, columns = { 100, "1fr", "auto", "2.5fr" }, rows = { 30, "1fr" },
  autoFlow = "row dense", justifyItems = "start" }, menu)
local a = ui.new("button", { text = "A", row = 1, column = 2, columnSpan = 2, margin = 3 }, grid)
local b = ui.new("panel", { grow = 2.5, shrink = 0, basis = 40, alignSelf = "end", minSize = { 10, 20 }, maxSize = { 90, 0 } }, menu)
local inner = ui.new("flex", { size = { 50, 50 }, padding = { 1, 2, 3, 4 } })
ui.new("text", { text = "in", grow = 1 }, inner)
realm.log(tostring(#grid:get("columns")), grid:get("columns")[4], tostring(a:get("margin")[2]), tostring(b:get("basis")), menu:get("padding")[3])
realm.after(0.25, function()
  inner:parent(menu)
  menu:set({ direction = "row", gap = { 1, 2 } })
  grid:set({ columns = { "auto" } })
  realm.log(pcall(grid.set, grid, { columns = { 1e9 } }) and "accepted" or "refused")
  realm.log(pcall(ui.new, "flex", { opacity = 1 }) and "accepted" or "refused")
end)
realm.log("built")
"##,
        )],
        steady,
        UI_NET,
        20,
    )
}

/// Messages and calls to the server.
pub fn net() -> Vec<Action> {
    run_script::<0>(
        &[(
            "main.lua",
            r##"
net.send({ hp = 3, name = "me", tags = { "a", "b" }, nested = { x = 1.5, y = { true, false } }, nul = "a\0b" })
net.send("plain text")
net.send(12345.678)
net.send(true)
net.call("nope", { 1, 2, 3 }, function(ok, value) realm.log("reply " .. tostring(ok) .. " " .. tostring(value)) end)
local n = 0
realm.every(0.2, function()
  n += 1
  net.send({ n = n, half = n / 2 })
  if n == 4 then net.call("also.nope", {}, function(ok) realm.log("second " .. tostring(ok)) end) end
end)
net.on(function(v) realm.log("got " .. tostring(v)) end)
realm.log("sent")
"##,
        )],
        steady,
        UI_NET,
        60,
    )
}

/// Steps: where the caps cut a callback (a plain loop, and one that first spent steps on bulk builtins), and a crowd of timers
/// due together.
pub fn steps() -> Vec<Action> {
    run_script::<2>(
        &[(
            "main.lua",
            r##"
n = 0
realm.after(0, function() while true do n = n + 1 end end)
realm.after(0.1, function() realm.log("loops before the callback cap: " .. n) end)
local frame = 0
realm.onFrame(function() frame += 1 end)
for id = 1, 12 do
  realm.every(0.5, function()
    local x = 0
    for i = 1, 1000 do x = x + i end
    realm.log("storm " .. id .. " frame " .. frame .. " x " .. x)
  end)
end
realm.after(0.2, function()
  local t = {}
  for i = 1, 300 do t[i] = tostring(i) .. "-" end
  realm.log("gathered " .. #table.concat(t))
end)
m = 0
realm.after(0.3, function()
  local s = string.rep("ab", 64 * 1000)
  local found = string.find(s, "b", 1, true)
  local numbers = {}
  for i = 1, 300 do numbers[i] = (i * 37) % 101 end
  table.sort(numbers)
  while true do m = m + 1 end
end)
realm.after(0.5, function() realm.log("after the bulk charges: " .. m) end)
"##,
        )],
        small_caps,
        UI_NET,
        90,
    )
}

/// The player revoking `ui` after the script has gone quiet (so the web runner, which applies a frame's ops in the next one, has
/// nothing in flight to drop), and the script's own answer to being told.
pub fn revoked() -> Vec<Action> {
    vec![
        Action::Open {
            slot: 0,
            guest: Guest::Realm,
            grants: UI_NET,
            scripts: &[(
                "main.lua",
                r##"
local made = 0
realm.on("revoked", function(name) realm.log("revoked " .. name .. " " .. tostring(ui.new("panel", {}))) end)
local timer
timer = realm.every(0, function()
  made += 1
  ui.new("panel", { pos = { made, 0 }, size = { 5, 5 } })
  if made == 10 then realm.log("made " .. made) realm.cancel(timer) end
end)
"##,
            )],
            tune: steady,
            apply_cost: Duration::ZERO,
        },
        Action::Frames { slot: 0, count: 25 },
        Action::Revoke {
            slot: 0,
            tier: Tier::Ui,
        },
        Action::Frames { slot: 0, count: 25 },
        Action::Check(|w| {
            let logs = &w.slot(0).effects().logs;
            expect(
                logs == &["made 10", "revoked ui false"],
                &format!("it said what it saw ({logs:?})"),
            )
        }),
    ]
}
