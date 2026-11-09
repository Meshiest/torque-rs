//! The client-script realm: Lua a server sends to its players' clients, run in a sandbox of its own with only the tiers
//! it was granted. It never calls into the engine: scripts change a mirror of what the realm made, which writes **ops**,
//! and the engine hands in **events**; the format of both, and every rule here, is `torque-client-script-format`'s
//! (re-exported as [`format`]), whose docs are the specification.
//!
//! A [`ClientRealm`] is one Luau state, made per server connection and dropped with it. Natively it is used by its tests;
//! the engine runs it as a WebAssembly guest (`torque_lua_client.wasm`, this crate built for wasm32-wasip1 with the
//! `client-script` feature by `just lua-client-web`), whose exports (`web`) are a thin layer over the same type.
//!
//! **The sandbox** is the mod realms' (the same standard libraries, the same removed globals, read-only built-ins, text
//! chunks only, bulk builtins charged by their bytes, the pattern cap), with none of their engine bridge: no `ts`, no
//! console, no files but the server's own scripts handed in up front, no clock but the host's time and Luau's `os.clock`.
//! Each mod gets its own global table; its base holds `realm`, `print`, its own `require` and the granted tiers' tables
//! (`ui`, `net`); an ungranted tier's table does not exist.
//!
//! **Limits**: every callback (a file run, an event, a timer, an `onFrame`) has its own step budget, counted at every
//! Luau interrupt; past it the callback is stopped and every interrupt errors until it returns. The frame's callbacks share
//! a frame budget, past which the rest waits. Luau's allocator is capped. A stop is reported as a `limit` op and a log
//! line; the realm goes on.

mod net;
mod ui;
#[cfg(target_os = "wasi")]
mod web;

pub use torque_client_script_format as format;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mlua::chunk::ChunkMode;
use mlua::{
    AnyUserData, Error, FromLuaMulti, Function, IntoLuaMulti, Lua, LuaOptions, MaybeSend,
    MultiValue, Table, UserData, UserDataMethods, Value, VmState,
};

use format::abi::{self, Config};
use format::{Grants, Malformed, NodeBudget, Reader, Records, Tier, Writer, event, op};

use crate::realm;

/// The message of the error that ends a callback past its steps.
pub const STOPPED: &str = "the script ran out of steps";

/// The tiers this guest implements: the base, `ui` and `net`.
pub const IMPLEMENTED: Grants = Grants(0b111);

/// The registry's table of the functions the realm keeps (handlers, timers, calls), by id.
const CALLBACKS: &str = "torque.client.callbacks";
/// The registry's table of each mod's global table.
const ENVS: &str = "torque.client.envs";
/// The registry's table of each mod's loaded modules.
const MODULES: &str = "torque.client.modules";
/// The registry's read-only global table.
const GLOBALS: &str = "torque.client.globals";
/// The registry's `realm` table.
const REALM: &str = "torque.client.realm";
/// The registry's `net` table.
const NET: &str = "torque.client.net";

/// Every realm's own number, so a handle of one is never taken for another's.
static SERIALS: AtomicU64 = AtomicU64::new(1);

/// A function the realm keeps (a handler, a timer's, a call's), by its key in the registry's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CallbackId(u64);

/// A live timer's number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct TimerId(u64);

/// When a timer fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Schedule {
    /// Once (`realm.after`).
    Once,
    /// Every this many microseconds (`realm.every`); at most once a frame.
    Every(u64),
    /// Every frame, after the other timers (`realm.onFrame`).
    Frame,
}

/// A timer: `realm.after`, `realm.every` or `realm.onFrame`.
struct Timer {
    /// When it fires next, in the host's microseconds (not used by [`Schedule::Frame`]).
    due: u64,
    schedule: Schedule,
    callback: CallbackId,
}

/// An event waiting for its callbacks.
pub(crate) enum Queued {
    /// `realm.revoked`.
    Revoked(Tier),
    /// A `ui` event.
    Ui(ui::UiEvent),
    /// A `net` event.
    Net(net::NetEvent),
}

impl Queued {
    fn tier(&self) -> Tier {
        match self {
            Queued::Revoked(_) => Tier::Realm,
            Queued::Ui(_) => Tier::Ui,
            Queued::Net(_) => Tier::Net,
        }
    }
}

/// The step and memory accounting of a realm's callbacks.
struct Fuel {
    /// The steps one callback may take.
    callback_steps: u64,
    /// The steps all callbacks of a frame may take before the rest waits.
    frame_steps: u64,
    /// Luau's allocator cap, in bytes.
    memory: usize,
    /// Where the running callback is.
    run: Run,
    /// The steps the running callback may take: a file run's `callback_steps`, a frame's callback at most
    /// `frame_steps` of them.
    cap: u64,
    /// The memory in use after the last collection the interrupt made.
    collected_at: usize,
    /// Steps of the frame's callbacks so far.
    frame_used: u64,
    /// The steps of the last callback that ran.
    last_steps: u64,
}

/// Where the running callback is in its step budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Run {
    /// No callback is running: the interrupt counts nothing.
    Idle,
    /// A callback is running, with this many steps so far.
    Running(u64),
    /// The running callback spent its budget (this many steps): every interrupt errors until it returns.
    Stopped(u64),
}

impl Fuel {
    /// Adds `steps` to the running callback: `false` once it is past its budget, and from then on until it returns.
    fn spend(&mut self, steps: u64) -> bool {
        match self.run {
            Run::Idle => true,
            Run::Stopped(_) => false,
            Run::Running(spent) => {
                let spent = spent.saturating_add(steps);
                if spent > self.cap {
                    self.run = Run::Stopped(spent);
                    false
                } else {
                    self.run = Run::Running(spent);
                    true
                }
            }
        }
    }

    /// Ends the running callback: whether it was stopped. Its steps count for the frame.
    fn finish(&mut self) -> bool {
        let (steps, stopped) = match self.run {
            Run::Idle => (0, false),
            Run::Running(steps) => (steps, false),
            Run::Stopped(steps) => (steps, true),
        };
        self.run = Run::Idle;
        self.frame_used = self.frame_used.saturating_add(steps);
        self.last_steps = steps;
        stopped
    }
}

/// The running entry's flush and its log-line count.
#[derive(Default)]
struct Flush {
    ops: Writer,
    count: usize,
    /// The second the log lines are counted in, and how many it had.
    log_second: u64,
    log_lines: u32,
}

/// The script files handed in, by mod and path, shared with the chunks loading them.
#[derive(Default)]
struct Scripts {
    files: HashMap<String, HashMap<String, Arc<[u8]>>>,
    count: usize,
    bytes: usize,
}

impl Scripts {
    fn get(&self, mod_id: &str, path: &str) -> Option<Arc<[u8]>> {
        self.files.get(mod_id)?.get(path).cloned()
    }

    /// Keeps a file within the caps, once per path: `false` when it is refused.
    fn add(&mut self, mod_id: &str, path: &str, source: &[u8]) -> bool {
        if self.count >= abi::MAX_SCRIPTS
            || self.bytes + source.len() > abi::MAX_SCRIPTS_BYTES
            || self.get(mod_id, path).is_some()
        {
            return false;
        }
        self.count += 1;
        self.bytes += source.len();
        self.files
            .entry(mod_id.to_string())
            .or_default()
            .insert(path.to_string(), source.into());
        true
    }
}

/// What a realm keeps beside its Lua state (its app data).
pub(crate) struct State {
    serial: u64,
    /// The tiers granted, implemented and not revoked.
    grants: Grants,
    fuel: Fuel,
    out: Flush,
    /// The host's time of the running entry, in microseconds.
    pub(crate) now: u64,
    /// The last entry's time (`now` never goes back).
    last_now: Option<u64>,
    last_frame: Option<u64>,
    scripts: Scripts,
    next_callback: u64,
    timers: BTreeMap<TimerId, Timer>,
    next_timer: u64,
    queue: VecDeque<Queued>,
    /// A net message was dropped from the full queue: one `overflow` callback is due.
    overflowed: bool,
    revoked_handlers: Vec<CallbackId>,
    pub(crate) ui: ui::UiState,
    pub(crate) net: net::NetState,
}

impl State {
    /// Appends an op: an error a script sees when the flush is full.
    pub(crate) fn emit(
        &mut self,
        tier: Tier,
        kind: u8,
        body: impl FnOnce(&mut Writer),
    ) -> mlua::Result<()> {
        if self.out.count >= format::MAX_OPS {
            return Err(Error::runtime(
                "too many changes in one frame (at most 4096): the rest waits for a later frame",
            ));
        }
        let before = self.out.ops.len();
        let mark = self.out.ops.begin(tier.id(), kind);
        body(&mut self.out.ops);
        if self.out.ops.end(mark).is_err() || self.out.ops.len() > format::MAX_FLUSH_BYTES {
            self.out.ops.truncate(before);
            return Err(Error::runtime("too much changed in one frame"));
        }
        self.out.count += 1;
        Ok(())
    }

    /// Appends a log line, made plain ([`plain`]) and at most [`format::LOG_LINES_PER_SECOND`] a second, then one
    /// warning.
    pub(crate) fn log(&mut self, level: u8, text: &str) {
        let second = self.now / 1_000_000;
        if second != self.out.log_second {
            self.out.log_second = second;
            self.out.log_lines = 0;
        }
        self.out.log_lines = self.out.log_lines.saturating_add(1);
        let limit = format::LOG_LINES_PER_SECOND;
        let (level, line) = if self.out.log_lines <= limit {
            (level, plain(text))
        } else if self.out.log_lines == limit + 1 {
            (
                op::realm::LEVEL_WARNING,
                "more log lines this second were dropped".to_string(),
            )
        } else {
            return;
        };
        let _ = self.emit(Tier::Realm, op::realm::LOG, |w| {
            w.u8(level);
            w.text(&line);
        });
    }

    /// Whether `tier` is granted and not revoked.
    pub(crate) fn granted(&self, tier: Tier) -> bool {
        self.grants.has(tier)
    }

    /// Whether `serial` is this realm's.
    pub(crate) fn is_own(&self, serial: u64) -> bool {
        serial == self.serial
    }

    pub(crate) fn serial(&self) -> u64 {
        self.serial
    }
}

/// `text` as one plain log line: every character [`format::is_hidden`] names (controls, newlines, line and paragraph
/// separators, direction controls) as a space, at most [`format::MAX_LOG_LINE`] bytes.
fn plain(text: &str) -> String {
    let mut line = String::with_capacity(text.len().min(format::MAX_LOG_LINE));
    for c in text.chars() {
        let c = if format::is_hidden(c) { ' ' } else { c };
        if line.len() + c.len_utf8() > format::MAX_LOG_LINE {
            break;
        }
        line.push(c);
    }
    line
}

/// The longest name a function takes (a kind, an event, an RPC, a module): longer ones are refused before they are read.
pub(crate) const MAX_NAME: usize = 200;

/// `s` as text of at most `max` bytes, or `None`: the length is checked before anything is copied or scanned, so a huge
/// string costs nothing unmetered.
pub(crate) fn short_text(s: &mlua::LuaString, max: usize) -> Option<String> {
    let bytes = s.as_bytes();
    if bytes.len() > max {
        return None;
    }
    std::str::from_utf8(&bytes).ok().map(str::to_string)
}

/// A name argument as text (empty when too long or not UTF-8, which no name check accepts).
pub(crate) fn name_of(s: &mlua::LuaString) -> String {
    short_text(s, MAX_NAME).unwrap_or_default()
}

/// Runs `f` on `args` converted, charging a refusal ([`realm::REFUSED_BYTES`]) when the conversion or `f` fails.
pub(crate) fn guarded<A: FromLuaMulti, R>(
    lua: &Lua,
    args: MultiValue,
    f: impl FnOnce(&Lua, A) -> mlua::Result<R>,
) -> mlua::Result<R> {
    realm::charged(lua, charge, args, f)
}

/// A function of the realm's tables: its arguments are converted inside it, and a refusal is charged ([`guarded`]).
pub(crate) fn function<A, R, F>(lua: &Lua, f: F) -> mlua::Result<Function>
where
    A: FromLuaMulti,
    R: IntoLuaMulti,
    F: Fn(&Lua, A) -> mlua::Result<R> + MaybeSend + 'static,
{
    realm::charged_function(lua, charge, f)
}

/// Whether the realm's `tier` is granted and not revoked (a revoked tier's functions answer `false`).
pub(crate) fn granted(lua: &Lua, tier: Tier) -> mlua::Result<bool> {
    Ok(state(lua)?.granted(tier))
}

/// The realm's state. Never held across a call into Lua.
pub(crate) fn state(lua: &Lua) -> mlua::Result<mlua::AppDataRefMut<'_, State>> {
    lua.try_app_data_mut::<State>()
        .ok()
        .flatten()
        .ok_or_else(|| Error::runtime("this Lua state is not a client realm"))
}

/// Keeps `f` for later, by a new id.
pub(crate) fn keep(lua: &Lua, f: Function) -> mlua::Result<CallbackId> {
    let id = {
        let mut st = state(lua)?;
        st.next_callback += 1;
        CallbackId(st.next_callback)
    };
    let callbacks: Table = lua.named_registry_value(CALLBACKS)?;
    callbacks.raw_set(id.key(), f)?;
    Ok(id)
}

impl CallbackId {
    /// Its key in the registry's table: a number, exact below 2^53 (ids count up by one).
    fn key(self) -> f64 {
        self.0 as f64
    }
}

/// Lets the function kept as `id` go.
pub(crate) fn release(lua: &Lua, id: CallbackId) {
    if let Ok(callbacks) = lua.named_registry_value::<Table>(CALLBACKS) {
        let _ = callbacks.raw_set(id.key(), Value::Nil);
    }
}

/// The function kept as `id`.
fn kept(lua: &Lua, id: CallbackId) -> Option<Function> {
    let callbacks: Table = lua.named_registry_value(CALLBACKS).ok()?;
    callbacks
        .raw_get::<Option<Function>>(id.key())
        .ok()
        .flatten()
}

/// Adds `f` to an `on` list, at most [`format::MAX_HANDLERS`].
pub(crate) fn add_handler(
    lua: &Lua,
    f: Function,
    list: fn(&mut State) -> &mut Vec<CallbackId>,
) -> mlua::Result<()> {
    if list(&mut *state(lua)?).len() >= format::MAX_HANDLERS {
        return Err(Error::runtime("at most 16 functions listen for one event"));
    }
    let id = keep(lua, f)?;
    list(&mut *state(lua)?).push(id);
    Ok(())
}

/// Runs the kept function `id` as a callback of `entry`.
pub(crate) fn invoke_kept(lua: &Lua, entry: u8, id: CallbackId, args: impl IntoLuaMulti) {
    if let Some(f) = kept(lua, id) {
        invoke(lua, entry, &f, args);
    }
}

/// Runs `f` as one callback of `entry`, with its own step budget; an error or a stop becomes ops.
pub(crate) fn invoke(lua: &Lua, entry: u8, f: &Function, args: impl IntoLuaMulti) {
    begin_callback(lua, entry);
    let result = f.call::<()>(args);
    end_callback(lua, entry, result);
}

fn begin_callback(lua: &Lua, entry: u8) {
    if let Ok(mut st) = state(lua) {
        let fuel = &mut st.fuel;
        fuel.cap = if entry == op::realm::ENTRY_RUN {
            fuel.callback_steps
        } else {
            fuel.callback_steps.min(fuel.frame_steps)
        };
        fuel.run = Run::Running(0);
    }
}

fn end_callback(lua: &Lua, entry: u8, result: mlua::Result<()>) {
    let stopped = state(lua).is_ok_and(|mut st| st.fuel.finish());
    let memory = matches!(&result, Err(e) if realm::is_memory_error(e));
    if memory {
        let _ = lua.gc_collect();
    }
    let Ok(mut st) = state(lua) else {
        return;
    };
    if stopped || memory {
        let which = if stopped {
            op::realm::LIMIT_STEPS
        } else {
            op::realm::LIMIT_MEMORY
        };
        let _ = st.emit(Tier::Realm, op::realm::LIMIT, |w| {
            w.u8(which);
            w.u8(entry);
        });
    }
    if stopped {
        st.log(op::realm::LEVEL_ERROR, STOPPED);
    } else if let Err(error) = result {
        st.log(op::realm::LEVEL_ERROR, &error_text(&error));
    }
}

/// An error as one line: its message, without mlua's wrapping or Luau's traceback.
fn error_text(error: &Error) -> String {
    match error {
        Error::RuntimeError(s) => s
            .split("\nstack traceback:")
            .next()
            .unwrap_or(s)
            .to_string(),
        Error::CallbackError { cause, .. } => error_text(cause),
        Error::WithContext { context, cause } => format!("{context}: {}", error_text(cause)),
        Error::MemoryError(_) => "not enough memory".to_string(),
        Error::SyntaxError { message, .. } => message.clone(),
        other => other.to_string(),
    }
}

/// Luau's interrupt (every call and loop edge, in every coroutine): one step, and once the budget is spent an error at
/// every interrupt until the callback returns.
fn interrupt(lua: &Lua) -> mlua::Result<VmState> {
    let (collected_at, limit) = {
        let Ok(Some(mut st)) = lua.try_app_data_mut::<State>() else {
            return Ok(VmState::Continue);
        };
        if st.fuel.run == Run::Idle {
            return Ok(VmState::Continue);
        }
        if !st.fuel.spend(1) {
            return Err(Error::runtime(STOPPED));
        }
        (st.fuel.collected_at, st.fuel.memory)
    };
    if let Some(at) = realm::collect_under_pressure(lua, collected_at, limit)
        && let Ok(mut st) = state(lua)
    {
        st.fuel.collected_at = at;
    }
    Ok(VmState::Continue)
}

/// Charges a bulk builtin's bytes of work to the running callback.
fn charge(lua: &Lua, bytes: u64) -> mlua::Result<()> {
    let steps = bytes / abi::BYTES_PER_STEP;
    if steps == 0 {
        return Ok(());
    }
    if state(lua)?.fuel.spend(steps) {
        Ok(())
    } else {
        Err(Error::runtime(STOPPED))
    }
}

/// A realm's timer, as scripts hold it.
pub(crate) struct TimerHandle {
    id: TimerId,
    serial: u64,
}

impl UserData for TimerHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(mlua::MetaMethod::ToString, |_, _, ()| Ok("timer"));
    }
}

/// The `realm` table.
fn realm_table(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.raw_set(
        "time",
        function(lua, |lua, ()| Ok(state(lua)?.now as f64 / 1e6))?,
    )?;
    t.raw_set(
        "after",
        function(lua, |lua, (seconds, f): (f64, Function)| {
            add_timer(lua, f, seconds, |_| Schedule::Once)
        })?,
    )?;
    t.raw_set(
        "every",
        function(lua, |lua, (seconds, f): (f64, Function)| {
            add_timer(lua, f, seconds, Schedule::Every)
        })?,
    )?;
    t.raw_set(
        "onFrame",
        function(lua, |lua, f: Function| {
            add_timer(lua, f, 0.0, |_| Schedule::Frame)
        })?,
    )?;
    t.raw_set("cancel", function(lua, cancel)?)?;
    let log = function(lua, log_function)?;
    t.raw_set("log", log)?;
    t.raw_set(
        "has",
        function(lua, |lua, name: mlua::LuaString| {
            let granted = state(lua)?.grants;
            Ok(Tier::from_name(&name_of(&name))
                .is_some_and(|tier| tier != Tier::Realm && granted.has(tier)))
        })?,
    )?;
    t.raw_set(
        "on",
        function(lua, |lua, (name, f): (mlua::LuaString, Function)| {
            if name.as_bytes().as_ref() != b"revoked" {
                return Err(Error::runtime(
                    "realm.on: the realm's only event is \"revoked\"",
                ));
            }
            add_handler(lua, f, |st| &mut st.revoked_handlers)
        })?,
    )?;
    t.set_readonly(true);
    Ok(t)
}

/// `realm.cancel(timer)`: whether it was still live.
fn cancel(lua: &Lua, timer: AnyUserData) -> mlua::Result<bool> {
    let handle = timer
        .borrow::<TimerHandle>()
        .map_err(|_| Error::runtime("realm.cancel takes a timer"))?;
    let removed = {
        let mut st = state(lua)?;
        if !st.is_own(handle.serial) {
            return Err(Error::runtime("realm.cancel takes a timer of this realm"));
        }
        st.timers.remove(&handle.id)
    };
    let Some(timer) = removed else {
        return Ok(false);
    };
    release(lua, timer.callback);
    Ok(true)
}

/// `realm.log(...)` and `print(...)`: the values as `tostring` writes them, separated by spaces.
fn log_function(lua: &Lua, values: MultiValue) -> mlua::Result<()> {
    let tostring: Function = lua
        .named_registry_value::<Table>(GLOBALS)?
        .raw_get("tostring")?;
    let mut line = String::new();
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            line.push(' ');
        }
        // `tostring`'s string as Lua holds it (a `__tostring` may return a huge one): only the part a line can show is
        // read.
        let text = match value {
            Value::String(s) => s.clone(),
            other => tostring.call::<mlua::LuaString>(other)?,
        };
        let bytes = text.as_bytes();
        let shown = &bytes[..bytes.len().min(format::MAX_LOG_LINE + 4)];
        line.push_str(&String::from_utf8_lossy(shown));
        if line.len() > format::MAX_LOG_LINE {
            break;
        }
    }
    state(lua)?.log(op::realm::LEVEL_INFO, &line);
    Ok(())
}

/// The longest wait or period a timer takes, in seconds (a year).
const MAX_TIMER_SECONDS: f64 = 365.0 * 86400.0;

/// A new timer firing `f` after `seconds`, on the schedule `schedule` makes of them (in microseconds).
fn add_timer(
    lua: &Lua,
    f: Function,
    seconds: f64,
    schedule: fn(u64) -> Schedule,
) -> mlua::Result<TimerHandle> {
    if !(seconds.is_finite() && (0.0..=MAX_TIMER_SECONDS).contains(&seconds)) {
        return Err(Error::runtime(
            "a timer's seconds are a number from 0 to a year",
        ));
    }
    if state(lua)?.timers.len() >= format::MAX_TIMERS {
        return Err(Error::runtime("at most 64 timers are live at once"));
    }
    let callback = keep(lua, f)?;
    let mut st = state(lua)?;
    let micros = (seconds * 1e6) as u64;
    st.next_timer += 1;
    let id = TimerId(st.next_timer);
    let due = st.now.saturating_add(micros);
    st.timers.insert(
        id,
        Timer {
            due,
            schedule: schedule(micros),
            callback,
        },
    );
    Ok(TimerHandle {
        id,
        serial: st.serial,
    })
}

/// The global table of `mod_id` (made at its first file): writes stay in it; reads go to its base (`realm`, `print`,
/// `require` and the granted tiers' tables), then to the read-only built-ins.
fn env_for(lua: &Lua, mod_id: &str) -> mlua::Result<Table> {
    let envs: Table = lua.named_registry_value(ENVS)?;
    if let Some(env) = envs.raw_get::<Option<Table>>(mod_id)? {
        return Ok(env);
    }
    let grants = state(lua)?.grants;
    let base = lua.create_table()?;
    let realm: Table = lua.named_registry_value(REALM)?;
    base.raw_set("print", realm.raw_get::<Function>("log")?)?;
    base.raw_set("realm", realm)?;
    base.raw_set("require", require_function(lua, mod_id)?)?;
    if grants.has(Tier::Ui) {
        base.raw_set("ui", ui::table(lua, mod_id)?)?;
    }
    if grants.has(Tier::Net) {
        base.raw_set("net", lua.named_registry_value::<Table>(NET)?)?;
    }
    lock(lua, &base, Value::Table(lua.named_registry_value(GLOBALS)?))?;
    let env = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.raw_set("__index", base)?;
    meta.raw_set("__metatable", "locked")?;
    meta.set_readonly(true);
    env.set_metatable(Some(meta))?;
    envs.raw_set(mod_id, env.clone())?;
    Ok(env)
}

/// Makes `table` read only, reading through to `fallback`.
fn lock(lua: &Lua, table: &Table, fallback: Value) -> mlua::Result<()> {
    let meta = lua.create_table()?;
    meta.raw_set("__index", fallback)?;
    meta.raw_set("__metatable", "locked")?;
    meta.set_readonly(true);
    table.set_metatable(Some(meta))?;
    table.set_readonly(true);
    Ok(())
}

/// `require` of `mod_id`: a module among the mod's own kept files, run once in its global table and kept.
fn require_function(lua: &Lua, mod_id: &str) -> mlua::Result<Function> {
    let mod_id = mod_id.to_string();
    function(lua, move |lua, name: mlua::LuaString| {
        let name = name_of(&name);
        if !crate::require::is_module_name(&name) {
            return Err(Error::runtime(
                "require: a module name is parts of letters, digits, `_` and `-`, separated by `.`: a module is a \
                 file of this mod",
            ));
        }
        let relative = name.replace('.', "/");
        let modules: Table = lua.named_registry_value(MODULES)?;
        let loaded = match modules.raw_get::<Option<Table>>(mod_id.as_str())? {
            Some(t) => t,
            None => {
                let t = lua.create_table()?;
                modules.raw_set(mod_id.as_str(), t.clone())?;
                t
            }
        };
        match loaded.raw_get::<Value>(relative.as_str())? {
            Value::Nil => {}
            Value::LightUserData(_) => {
                return Err(Error::runtime(format!(
                    "require: `{name}` requires itself (through the modules it requires)"
                )));
            }
            value => return Ok(value),
        }
        let found = {
            let st = state(lua)?;
            [
                format!("{relative}.lua"),
                format!("{relative}.luau"),
                format!("{relative}/init.lua"),
            ]
            .into_iter()
            .find_map(|path| st.scripts.get(&mod_id, &path).map(|source| (path, source)))
        };
        let Some((path, source)) = found else {
            return Err(Error::runtime(format!(
                "require: this mod has no module `{name}`"
            )));
        };
        let env = env_for(lua, &mod_id)?;
        // Marked while it runs, so a cycle is an error rather than a loop.
        loaded.raw_set(
            relative.as_str(),
            Value::LightUserData(mlua::LightUserData(std::ptr::null_mut())),
        )?;
        let ran = lua
            .load(&*source)
            .set_name(format!("@{mod_id}/{path}"))
            .set_mode(ChunkMode::Text)
            .set_environment(env)
            .call::<Value>(());
        let value = match ran {
            Ok(Value::Nil) => Value::Boolean(true),
            Ok(value) => value,
            Err(error) => {
                loaded.raw_set(relative.as_str(), Value::Nil)?;
                return Err(error);
            }
        };
        loaded.raw_set(relative.as_str(), value.clone())?;
        Ok(value)
    })
}

/// Why an entry did not run: one of [`format::abi`]'s statuses but `OK`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The arguments or a batch do not follow the format; nothing was applied.
    Malformed,
    /// A script file was refused, or an entry file is not there.
    Refused,
    /// The realm is not one (or not yet).
    State,
}

impl Status {
    /// The status's number at the guest's exports.
    #[must_use]
    pub fn code(self) -> i32 {
        match self {
            Status::Malformed => abi::MALFORMED,
            Status::Refused => abi::REFUSED,
            Status::State => abi::STATE,
        }
    }
}

/// One client-script realm: a Luau state with its mirror, its queue and its flush.
pub struct ClientRealm {
    lua: Lua,
}

impl ClientRealm {
    /// A realm made from `config` (its grants left to the tiers this guest implements).
    ///
    /// # Errors
    ///
    /// Grant bits that name no tier, or Luau could not make the state (no memory).
    pub fn new(config: Config) -> mlua::Result<ClientRealm> {
        if !config.grants.is_valid() {
            return Err(Error::runtime("grant bits that name no tier"));
        }
        let lua = Lua::new_with(realm::libs(), LuaOptions::new())?;
        Self::set_up(&lua, &config)?;
        Ok(ClientRealm { lua })
    }

    /// A realm made from a config record.
    ///
    /// # Errors
    ///
    /// [`Status::Malformed`] when the record does not follow the format or the realm could not be made.
    pub fn from_config(record: &[u8]) -> Result<ClientRealm, Status> {
        let config = Config::from_bytes(record).map_err(|_| Status::Malformed)?;
        ClientRealm::new(config).map_err(|_| Status::Malformed)
    }

    fn set_up(lua: &Lua, config: &Config) -> mlua::Result<()> {
        lua.set_memory_limit(config.memory as usize)?;
        let globals = lua.globals();
        // `math.random` from the host's seed, before the libraries are read-only.
        let math: Table = globals.raw_get("math")?;
        math.raw_get::<Function>("randomseed")?
            .call::<()>(f64::from(config.seed))?;
        realm::strip_globals(lua, &globals)?;
        globals.raw_set("print", Value::Nil)?;
        // Before the builtins are charged: the probe runs outside any callback.
        realm::check_table_slots(lua)?;
        realm::charge_builtins(lua, &globals, charge)?;
        lua.set_named_registry_value(GLOBALS, globals)?;
        lua.set_named_registry_value(CALLBACKS, lua.create_table()?)?;
        lua.set_named_registry_value(ENVS, lua.create_table()?)?;
        lua.set_named_registry_value(MODULES, lua.create_table()?)?;
        lua.set_app_data(State {
            serial: SERIALS.fetch_add(1, Ordering::Relaxed),
            grants: config.grants.and(IMPLEMENTED).with(Tier::Realm),
            fuel: Fuel {
                callback_steps: u64::from(config.callback_steps),
                frame_steps: u64::from(config.frame_steps),
                memory: config.memory as usize,
                run: Run::Idle,
                cap: u64::from(config.callback_steps),
                collected_at: 0,
                frame_used: 0,
                last_steps: 0,
            },
            out: Flush::default(),
            now: 0,
            last_now: None,
            last_frame: None,
            scripts: Scripts::default(),
            next_callback: 0,
            timers: BTreeMap::new(),
            next_timer: 0,
            queue: VecDeque::new(),
            overflowed: false,
            revoked_handlers: Vec::new(),
            ui: ui::UiState::default(),
            net: net::NetState::default(),
        });
        lua.set_named_registry_value(REALM, realm_table(lua)?)?;
        lua.set_named_registry_value(NET, net::table(lua)?)?;
        lua.set_interrupt(interrupt);
        lua.sandbox(true)?;
        Ok(())
    }

    /// The tiers this guest implements.
    pub fn tiers() -> Grants {
        IMPLEMENTED
    }

    /// The tiers granted and not revoked.
    pub fn grants(&self) -> Grants {
        state(&self.lua).map_or(Grants(0), |st| st.grants)
    }

    /// Keeps a script file of a mod for `torque_realm_run` and `require`, or [`Status::Refused`] (a path or mod id out
    /// of the format's rules, compiled code, a file past its caps, or a path kept twice).
    pub fn add_script(&mut self, mod_id: &str, path: &str, source: &[u8]) -> Result<(), Status> {
        let refused = Err(Status::Refused);
        if abi::check_mod_id(mod_id).is_err()
            || abi::check_script_path(path).is_err()
            || abi::is_compiled(source)
            || source.len() > abi::MAX_SCRIPT_BYTES
        {
            return refused;
        }
        let Ok(mut st) = state(&self.lua) else {
            return Err(Status::State);
        };
        if !st.scripts.add(mod_id, path, source) {
            return refused;
        }
        Ok(())
    }

    /// Starts an entry at `now_us`: the flush emptied; `Err` when time went back.
    fn begin(&mut self, now_us: u64) -> Result<(), Status> {
        let mut st = state(&self.lua).map_err(|_| Status::State)?;
        if st.last_now.is_some_and(|last| now_us < last) {
            return Err(Status::Malformed);
        }
        st.last_now = Some(now_us);
        st.now = now_us;
        st.out.ops.clear();
        st.out.count = 0;
        Ok(())
    }

    /// Runs the kept file `path` of `mod_id` in the mod's global table, as one callback; [`abi::REFUSED`] when it was
    /// not kept.
    pub fn run(&mut self, now_us: u64, mod_id: &str, path: &str) -> Result<(), Status> {
        self.begin(now_us)?;
        let source = state(&self.lua)
            .ok()
            .and_then(|st| st.scripts.get(mod_id, path))
            .ok_or(Status::Refused)?;
        let lua = &self.lua;
        let chunk = env_for(lua, mod_id).and_then(|env| {
            lua.load(&*source)
                .set_name(format!("@{mod_id}/{path}"))
                .set_mode(ChunkMode::Text)
                .set_environment(env)
                .into_function()
        });
        match chunk {
            Ok(f) => invoke(lua, op::realm::ENTRY_RUN, &f, ()),
            Err(error) => {
                if let Ok(mut st) = state(lua) {
                    st.log(op::realm::LEVEL_ERROR, &error_text(&error));
                }
            }
        }
        Ok(())
    }

    /// One frame at `now_us`: the event batch `events`, then the queue, the due timers and the `onFrame` callbacks
    /// within the frame's budget, in the order the format's docs give. [`Status::Malformed`] (nothing applied) when the batch does not follow the
    /// format.
    pub fn frame(&mut self, now_us: u64, events: &[u8]) -> Result<(), Status> {
        let incoming = decode_events(events).map_err(|_| Status::Malformed)?;
        self.begin(now_us)?;
        let lua = &self.lua;
        for item in incoming {
            match item {
                Incoming::Revoked(tier) => revoke(lua, tier),
                Incoming::Ui(e) => {
                    if let Some(queued) = ui::arrive(lua, e) {
                        enqueue(lua, Queued::Ui(queued));
                    }
                }
                Incoming::Net(e) => {
                    if state(lua).is_ok_and(|st| st.granted(Tier::Net)) {
                        enqueue(lua, Queued::Net(e));
                    }
                }
            }
        }
        if let Ok(mut st) = state(lua) {
            st.fuel.frame_used = 0;
        }
        run_queue(lua);
        run_timers(lua, now_us);
        if let Ok(mut st) = state(lua) {
            st.last_frame = Some(now_us);
        }
        Ok(())
    }

    /// The last entry's ops, taken: the next call has none until another entry runs.
    pub fn take_ops(&mut self) -> Vec<u8> {
        state(&self.lua).map_or_else(
            |_| Vec::new(),
            |mut st| std::mem::take(&mut st.out.ops).into_bytes(),
        )
    }

    /// Runs `f` on the last entry's ops, without a copy.
    pub fn with_ops<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        match state(&self.lua) {
            Ok(st) => f(st.out.ops.bytes()),
            Err(_) => f(&[]),
        }
    }

    /// Events still queued for a later frame.
    pub fn pending(&self) -> usize {
        state(&self.lua).map_or(0, |st| st.queue.len())
    }

    /// The steps of the last callback that ran.
    pub fn last_steps(&self) -> u64 {
        state(&self.lua).map_or(0, |st| st.fuel.last_steps)
    }

    /// The steps the last frame's callbacks took.
    pub fn frame_steps(&self) -> u64 {
        state(&self.lua).map_or(0, |st| st.fuel.frame_used)
    }

    /// The bytes Luau's allocator holds.
    pub fn used_memory(&self) -> usize {
        self.lua.used_memory()
    }
}

/// An event as decoded, before the realm's state judges it.
enum Incoming {
    Revoked(Tier),
    Ui(ui::Arrived),
    Net(net::NetEvent),
}

/// The batch's events, every one well-formed, or the first fault.
fn decode_events(bytes: &[u8]) -> Result<Vec<Incoming>, Malformed> {
    let records =
        Records::new(bytes, format::MAX_EVENTS, format::MAX_BATCH_BYTES)?.collect_all()?;
    let mut out = Vec::with_capacity(records.len());
    let mut budget = NodeBudget::default();
    for record in records {
        let mut r = record.reader();
        let item = match Tier::from_id(record.tier) {
            Some(Tier::Realm) => match record.kind {
                event::realm::REVOKED => {
                    let tier = Tier::from_id(r.u8()?).ok_or(Malformed("an unknown tier"))?;
                    if tier == Tier::Realm {
                        return Err(Malformed("the base is never revoked"));
                    }
                    Incoming::Revoked(tier)
                }
                _ => return Err(Malformed("an unknown realm event")),
            },
            Some(Tier::Ui) => Incoming::Ui(ui::decode(record.kind, &mut r, &mut budget)?),
            Some(Tier::Net) => Incoming::Net(net::decode(record.kind, &mut r, &mut budget)?),
            _ => return Err(Malformed("an event of a tier format 1 has none of")),
        };
        r.finish()?;
        out.push(item);
    }
    Ok(out)
}

/// Reads one value of the format from a body.
pub(crate) fn read_value(
    r: &mut Reader<'_>,
    budget: &mut NodeBudget,
) -> Result<format::Value, Malformed> {
    format::Value::decode(r, budget)
}

/// Queues an event: past [`format::MAX_QUEUED`], the oldest `ui` input event goes, else the oldest event (a net message
/// dropped makes one `overflow` callback due).
fn enqueue(lua: &Lua, item: Queued) {
    let Ok(mut st) = state(lua) else {
        return;
    };
    if st.queue.len() >= format::MAX_QUEUED {
        let at = st
            .queue
            .iter()
            .position(|q| matches!(q, Queued::Ui(_)))
            .or_else(|| {
                st.queue
                    .iter()
                    .position(|q| !matches!(q, Queued::Revoked(_)))
            });
        match at.and_then(|at| st.queue.remove(at)) {
            Some(Queued::Net(net::NetEvent::Message(_) | net::NetEvent::Rpc { .. })) => {
                st.overflowed = true;
            }
            Some(_) => {}
            None => return,
        }
    }
    st.queue.push_back(item);
}

/// Revokes `tier` at once: its table's functions answer `false` from now on, its queued events and kept functions go,
/// and `realm.on("revoked")` hears of it first thing.
fn revoke(lua: &Lua, tier: Tier) {
    let released = {
        let Ok(mut st) = state(lua) else {
            return;
        };
        if !st.granted(tier) {
            return;
        }
        st.grants = st.grants.without(tier);
        st.queue.retain(|q| q.tier() != tier);
        if tier == Tier::Net {
            st.overflowed = false;
        }
        let released = match tier {
            Tier::Ui => st.ui.revoke(),
            Tier::Net => st.net.revoke(),
            _ => Vec::new(),
        };
        let at = st
            .queue
            .iter()
            .take_while(|q| matches!(q, Queued::Revoked(_)))
            .count();
        st.queue.insert(at, Queued::Revoked(tier));
        released
    };
    for id in released {
        release(lua, id);
    }
}

/// Whether the frame's steps are spent.
fn spent(lua: &Lua) -> bool {
    state(lua).map_or(true, |st| st.fuel.frame_used >= st.fuel.frame_steps)
}

/// Runs queued events in order while the frame's budget lasts.
fn run_queue(lua: &Lua) {
    loop {
        if spent(lua) {
            return;
        }
        let overflow = state(lua).is_ok_and(|mut st| std::mem::take(&mut st.overflowed));
        if overflow {
            net::overflow(lua);
            continue;
        }
        let Some(item) = state(lua).ok().and_then(|mut st| st.queue.pop_front()) else {
            return;
        };
        match item {
            Queued::Revoked(tier) => {
                let handlers = state(lua)
                    .map(|st| st.revoked_handlers.clone())
                    .unwrap_or_default();
                for id in handlers {
                    invoke_kept(lua, op::realm::ENTRY_EVENT, id, tier.name());
                }
            }
            Queued::Ui(e) => ui::dispatch(lua, e),
            Queued::Net(e) => net::dispatch(lua, e),
        }
    }
}

/// Runs the due timers (by due time, then age), then the `onFrame` callbacks, while the frame's budget lasts.
fn run_timers(lua: &Lua, now: u64) {
    let Some((due, frames, dt)) = state(lua).ok().map(|st| {
        let mut due: Vec<(u64, TimerId)> = st
            .timers
            .iter()
            .filter(|(_, t)| t.schedule != Schedule::Frame && t.due <= now)
            .map(|(&id, t)| (t.due, id))
            .collect();
        due.sort_unstable();
        let frames: Vec<TimerId> = st
            .timers
            .iter()
            .filter(|(_, t)| t.schedule == Schedule::Frame)
            .map(|(&id, _)| id)
            .collect();
        let dt = st.last_frame.map_or(0, |last| now.saturating_sub(last)) as f64 / 1e6;
        (due, frames, dt)
    }) else {
        return;
    };
    for (_, id) in due {
        if spent(lua) {
            return;
        }
        fire(lua, id, now);
    }
    for id in frames {
        if spent(lua) {
            return;
        }
        let callback = state(lua)
            .ok()
            .and_then(|st| st.timers.get(&id).map(|t| t.callback));
        if let Some(callback) = callback {
            invoke_kept(lua, op::realm::ENTRY_FRAME, callback, dt);
        }
    }
}

/// Fires the due timer `id`: a repeating one is due again a period on (or a period from `now` when it fell behind),
/// before its callback runs, so the callback may cancel it; a one-off one is gone.
fn fire(lua: &Lua, id: TimerId, now: u64) {
    let fired = state(lua).ok().and_then(|mut st| {
        let timer = st.timers.get_mut(&id)?;
        let callback = timer.callback;
        let Schedule::Every(period) = timer.schedule else {
            st.timers.remove(&id);
            return Some((callback, true));
        };
        let next = timer.due.saturating_add(period);
        timer.due = if next <= now {
            now.saturating_add(period)
        } else {
            next
        };
        Some((callback, false))
    });
    let Some((callback, once)) = fired else {
        return;
    };
    invoke_kept(lua, op::realm::ENTRY_TIMER, callback, ());
    if once {
        release(lua, callback);
    }
}
