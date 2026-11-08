//! Realms: one sandboxed Lua state per script engine, made at its first Lua file, with the call it is running in, its
//! fuel and its mods' global tables.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use mlua::chunk::ChunkMode;
use mlua::{Error, Function, Lua, LuaOptions, MultiValue, StdLib, Table, Value, VmState};
use torque_plugin_sdk::abi::{TorqueApi, TorqueCall};
use torque_plugin_sdk::{Call, CallError, Limit, Side};

use crate::value::{from_text, to_text};

/// The global holding each realm's memory cap, in MiB (read when the realm is made; 64 by default).
pub const MEMORY_LIMIT_PREF: &str = "$Pref::Lua::MemoryLimit";

/// How many interrupts go by between two charges to the step budget (each interrupt is one step).
const STEPS_PER_CHARGE: u32 = 64;

/// The message of the error that ends a script a limit stopped.
pub(crate) const STOPPED: &str = "the script ran out of steps, or its calls nested too deep";

/// The registry's table of every function a realm defined, by id.
const DEFINED: &str = "torque.defined";
/// The registry's table of each mod's (or folder's) global table, by its root.
const ENVS: &str = "torque.envs";
/// The registry's table of each root's loaded modules.
const MODULES: &str = "torque.modules";
/// The read-only global table every root's table reads through to.
const GLOBALS: &str = "torque.globals";

/// The bytes of work that cost one step, as TorqueScript charges a native's work.
const BYTES_PER_STEP: u64 = 64;

/// What a realm keeps beside its Lua state (its app data).
pub(crate) struct State {
    api: &'static TorqueApi,
    side: Side,
    /// The calls of the engine's this realm is running in, innermost last.
    calls: Vec<TorqueCall>,
    /// A limit stopped the outermost call: every interrupt errors until it returns.
    stopped: bool,
    /// Interrupts since the last charge.
    ticks: u32,
    /// The next id of a defined function.
    next_id: u64,
    /// The ids of the defined functions, by `package/namespace::name` without case.
    ids: HashMap<String, u64>,
    /// The memory in use after the last collection the interrupt made.
    collected_at: usize,
    /// The memory cap, in bytes.
    limit: usize,
}

/// The realms of every script engine, by its number.
#[derive(Default)]
pub(crate) struct Realms {
    map: Mutex<HashMap<u64, Lua>>,
}

impl Realms {
    /// The realm of the call's script engine, made at its first use.
    fn realm(&self, call: Call<'_>) -> Result<Lua, String> {
        let info = call
            .vm_info()
            .ok_or("the engine did not say which script engine this is")?;
        if let Some(lua) = self.lock().get(&info.vm) {
            return Ok(lua.clone());
        }
        let lua = new_realm(call, info.side)
            .map_err(|e| format!("the Lua realm could not be made: {e}"))?;
        self.lock().insert(info.vm, lua.clone());
        Ok(lua)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Lua>> {
        self.map.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The script engine `vm` is gone, and so is its realm.
    pub(crate) fn drop_realm(&self, vm: u64) {
        let gone = self.lock().remove(&vm);
        drop(gone);
    }

    /// Runs the file `path` in the call's realm.
    pub(crate) fn run_file(&self, call: Call<'_>, path: &str, source: &[u8]) -> Result<(), Limit> {
        let lua = match self.realm(call) {
            Ok(lua) => lua,
            Err(message) => {
                call.error(&message);
                return Ok(());
            }
        };
        if is_binary(source) {
            call.error(&format!(
                "{path}: compiled Lua is never loaded; ship the source"
            ));
            return Ok(());
        }
        let package = call.script_package();
        let root = root_of(path);
        enter(&lua, call.handle());
        let ran = env_for(&lua, &root, &package).and_then(|env| {
            lua.load(source)
                .set_name(format!("@{path}"))
                .set_mode(ChunkMode::Text)
                .set_environment(env)
                .exec()
        });
        finish(&lua, call, ran).map(|_| ())
    }

    /// Calls the function the call's realm defined with `id`, with the call's arguments.
    pub(crate) fn call_defined(&self, call: Call<'_>, id: u64) -> Result<Vec<u8>, Limit> {
        let lua = match self.realm(call) {
            Ok(lua) => lua,
            Err(message) => {
                call.error(&message);
                return Ok(Vec::new());
            }
        };
        enter(&lua, call.handle());
        let ran = (|| {
            let defined: Table = lua.named_registry_value(DEFINED)?;
            let function: Function = defined.raw_get(id)?;
            let args = (0..call.arg_count())
                .map(|i| from_text(&lua, call.arg_bytes(i).unwrap_or_default()))
                .collect::<mlua::Result<MultiValue>>()?;
            let result: Value = function.call(args)?;
            to_text(&result)
        })();
        Ok(finish(&lua, call, ran)?.unwrap_or_default())
    }
}

/// Starts a call in the realm: the outermost one starts with fresh fuel.
fn enter(lua: &Lua, handle: TorqueCall) {
    if let Some(mut state) = lua.app_data_mut::<State>() {
        if state.calls.is_empty() {
            state.stopped = false;
            state.ticks = 0;
        }
        state.calls.push(handle);
    }
}

/// Ends a call in the realm: `Err(Limit)` when a limit stopped it (whatever the script did with the error), else its
/// result, with a script error printed and `None`.
fn finish<T>(lua: &Lua, call: Call<'_>, ran: mlua::Result<T>) -> Result<Option<T>, Limit> {
    let (stopped, outermost) = match lua.app_data_mut::<State>() {
        Some(mut state) => {
            state.calls.pop();
            (state.stopped, state.calls.is_empty())
        }
        None => (false, true),
    };
    // What a script that hit the memory cap left behind is garbage now: collected at once, so the realm's next script
    // has its memory back.
    if outermost && matches!(&ran, Err(e) if is_memory_error(e)) {
        let _ = lua.gc_collect();
    }
    if stopped {
        return Err(Limit);
    }
    match ran {
        Ok(value) => Ok(Some(value)),
        Err(error) => {
            call.error(&error.to_string());
            Ok(None)
        }
    }
}

/// Whether `error` is (or wraps) the memory cap's.
fn is_memory_error(error: &Error) -> bool {
    match error {
        Error::MemoryError(_) => true,
        Error::CallbackError { cause, .. } => is_memory_error(cause),
        Error::WithContext { cause, .. } => is_memory_error(cause),
        _ => error.to_string().contains("not enough memory"),
    }
}

/// Runs `f` with the call the realm is running in.
pub(crate) fn with_call<R>(lua: &Lua, f: impl FnOnce(Call<'_>) -> R) -> mlua::Result<R> {
    let (api, handle) = {
        let state = lua
            .app_data_ref::<State>()
            .ok_or_else(|| Error::runtime("this Lua state is not a realm"))?;
        let handle = *state
            .calls
            .last()
            .ok_or_else(|| Error::runtime("no call of the engine is running"))?;
        (state.api, handle)
    };
    #[allow(unsafe_code)]
    // SAFETY: `handle` is the innermost call the realm is running in: Lua code runs only during one of the engine's calls
    // into the extension, which is pushed before and popped after it, so it is live; and the `Call` is used within `f`,
    // which returns before that call does (nothing of it is kept).
    let call = unsafe { Call::from_handle(api, handle) };
    Ok(f(call))
}

/// The error that ends a script a limit stopped, marking the realm stopped until its outermost call returns.
pub(crate) fn stop(lua: &Lua) -> Error {
    if let Some(mut state) = lua.app_data_mut::<State>() {
        state.stopped = true;
    }
    Error::runtime(STOPPED)
}

/// `result` of a request to the engine, as Lua sees it: a limit stops the script, a refusal is an error naming `what`.
pub(crate) fn engine<T>(lua: &Lua, what: &str, result: Result<T, CallError>) -> mlua::Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(CallError::Limit) => Err(stop(lua)),
        Err(CallError::Refused(status)) => Err(Error::runtime(format!(
            "{what}: the engine refused it (status {status})"
        ))),
    }
}

/// The realm's side.
pub(crate) fn side(lua: &Lua) -> Side {
    lua.app_data_ref::<State>().map_or(Side::Both, |s| s.side)
}

/// The id of the function defined as `key` (`namespace::name`), a new one when it has none.
pub(crate) fn defined_id(lua: &Lua, key: &str) -> u64 {
    let Some(mut state) = lua.app_data_mut::<State>() else {
        return 0;
    };
    let key = key.to_ascii_lowercase();
    if let Some(&id) = state.ids.get(&key) {
        return id;
    }
    state.next_id += 1;
    let id = state.next_id;
    state.ids.insert(key, id);
    id
}

/// Keeps `function` as the defined function `id`.
pub(crate) fn keep_defined(lua: &Lua, id: u64, function: Function) -> mlua::Result<()> {
    let defined: Table = lua.named_registry_value(DEFINED)?;
    defined.raw_set(id, function)
}

/// A new realm for a script engine of `side`, its memory capped by the global [`MEMORY_LIMIT_PREF`].
fn new_realm(call: Call<'_>, side: Side) -> mlua::Result<Lua> {
    let libs = StdLib::COROUTINE
        | StdLib::TABLE
        | StdLib::STRING
        | StdLib::UTF8
        | StdLib::MATH
        | StdLib::BIT
        | StdLib::BUFFER
        | StdLib::OS;
    let lua = Lua::new_with(libs, LuaOptions::new())?;
    let mib = String::from_utf8_lossy(&call.global(MEMORY_LIMIT_PREF))
        .trim()
        .parse::<usize>()
        .unwrap_or(64)
        .clamp(1, 4096);
    lua.set_memory_limit(mib << 20)?;

    let globals = lua.globals();
    // What reaches outside the realm or loads code from text: gone.
    for name in [
        "collectgarbage",
        "debug",
        "dofile",
        "gcinfo",
        "getfenv",
        "io",
        "load",
        "loadfile",
        "loadstring",
        "module",
        "package",
        "require",
        "setfenv",
    ] {
        globals.raw_set(name, Value::Nil)?;
    }
    // `os` keeps only its clock.
    let os: Table = globals.raw_get("os")?;
    let clock: Function = os.raw_get("clock")?;
    let safe_os = lua.create_table()?;
    safe_os.raw_set("clock", clock)?;
    globals.raw_set("os", safe_os)?;
    globals.raw_set("print", lua.create_function(print)?)?;
    globals.raw_set("SERVER", side == Side::Server)?;
    globals.raw_set("CLIENT", side == Side::Client)?;

    charge_builtins(&lua, &globals)?;
    lua.set_named_registry_value(GLOBALS, globals.clone())?;
    lua.set_named_registry_value(DEFINED, lua.create_table()?)?;
    lua.set_named_registry_value(ENVS, lua.create_table()?)?;
    lua.set_named_registry_value(MODULES, lua.create_table()?)?;
    lua.set_app_data(State {
        api: call.api(),
        side,
        calls: Vec::new(),
        stopped: false,
        ticks: 0,
        next_id: 0,
        ids: HashMap::new(),
        collected_at: 0,
        limit: mib << 20,
    });
    lua.set_interrupt(interrupt);
    // Read-only built-ins and globals from here on.
    lua.sandbox(true)?;
    Ok(lua)
}

/// Luau's interrupt (at calls and loop edges, in every coroutine): one step each, charged in batches; once a limit is
/// reached every interrupt errors, so neither `pcall` nor a coroutine can go on running.
fn interrupt(lua: &Lua) -> mlua::Result<VmState> {
    let (charge, collected_at, limit) = {
        let Some(mut state) = lua.app_data_mut::<State>() else {
            return Ok(VmState::Continue);
        };
        if state.stopped {
            return Err(Error::runtime(STOPPED));
        }
        state.ticks += 1;
        let charge = state.ticks >= STEPS_PER_CHARGE && !state.calls.is_empty();
        if state.ticks >= STEPS_PER_CHARGE {
            state.ticks = 0;
        }
        (charge, state.collected_at, state.limit)
    };
    collect_under_pressure(lua, collected_at, limit);
    if !charge {
        return Ok(VmState::Continue);
    }
    match with_call(lua, |call| call.charge(u64::from(STEPS_PER_CHARGE)))? {
        Ok(()) => Ok(VmState::Continue),
        Err(Limit) => Err(stop(lua)),
    }
}

/// A full collection when the realm is past half its cap and has grown by an eighth of it since the last one: a loop
/// over large temporaries gets its garbage back before it reaches the cap, and a realm whose memory is all live is not
/// collected again and again.
fn collect_under_pressure(lua: &Lua, collected_at: usize, limit: usize) {
    let used = lua.used_memory();
    if limit == 0 || used < limit / 2 || used < collected_at.saturating_add(limit / 8) {
        return;
    }
    let _ = lua.gc_collect();
    if let Some(mut state) = lua.app_data_mut::<State>() {
        state.collected_at = lua.used_memory();
    }
}

/// Charges `bytes` of work to the step budget at once: the builtins that do work by the byte.
fn charge_bytes(lua: &Lua, bytes: u64) -> mlua::Result<()> {
    let steps = bytes / BYTES_PER_STEP;
    if steps == 0 {
        return Ok(());
    }
    match with_call(lua, |call| call.charge(steps))? {
        Ok(()) => Ok(()),
        Err(Limit) => Err(stop(lua)),
    }
}

/// The bytes a table slot counts as, for the table functions' work.
const SLOT_BYTES: u64 = 8;

/// A builtin's arguments, read for the work it will do.
struct Args<'a>(&'a MultiValue);

impl Args<'_> {
    /// The length of the string or buffer at `at` (a table's array length, in slots).
    fn len(&self, at: usize) -> u64 {
        match self.0.get(at) {
            Some(Value::String(s)) => s.as_bytes().len() as u64,
            Some(Value::Buffer(b)) => b.len() as u64,
            Some(Value::Table(t)) => t.raw_len() as u64,
            _ => 0,
        }
    }

    /// The whole number at `at`, if there is one.
    fn int(&self, at: usize) -> Option<i64> {
        match self.0.get(at) {
            // `Integer` is 32 bits on wasm32.
            #[allow(clippy::useless_conversion)]
            Some(Value::Integer(i)) => Some(i64::from(*i)),
            Some(Value::Number(n)) if n.is_finite() => Some(*n as i64),
            _ => None,
        }
    }

    /// A count at `at`: a whole number, at least 0, or `missing`.
    fn count(&self, at: usize, missing: u64) -> u64 {
        self.int(at).map_or(missing, |n| n.max(0) as u64)
    }

    /// How many of `len` places the 1-based range `i..=j` at `i_at` and `j_at` covers (negative counts from the end, as
    /// the string functions do), with `i` and `j` defaulting to `i_default` and `j_default`.
    fn span(&self, len: u64, i_at: usize, j_at: usize, i_default: i64, j_default: i64) -> u64 {
        let len = len as i64;
        let place = |n: i64| if n < 0 { len + n + 1 } else { n };
        let i = place(self.int(i_at).unwrap_or(i_default)).max(1);
        let j = place(self.int(j_at).unwrap_or(j_default)).min(len);
        (j - i + 1).max(0) as u64
    }

    /// How much of the string at 0 a search from the `init` at `at` scans.
    fn scanned(&self, at: usize) -> u64 {
        let len = self.len(0);
        self.span(len, at, usize::MAX, 1, -1)
    }

    /// The pattern at 1's backtracking quantifiers ([`backtracking`]); none for a plain `string.find`.
    fn quantifiers(&self, name: &str) -> u64 {
        let plain = name == "string.find"
            && self
                .0
                .get(3)
                .is_some_and(|v| !matches!(v, Value::Nil | Value::Boolean(false)));
        match self.0.get(1) {
            Some(Value::String(p)) if !plain => backtracking(&p.as_bytes()),
            _ => 0,
        }
    }
}

/// The most backtracking quantifiers (`*`, `+`, `-`, `?`) a pattern may have: each can make a match go back over the
/// subject again, so the work of one search grows with the subject's length to their number.
pub(crate) const MAX_BACKTRACKING: u64 = 6;

/// How many of `pattern`'s quantifiers can backtrack: `*`, `+`, `-` and `?` after an item, not inside a set (`[a-z]`) or
/// escaped (`%-`).
fn backtracking(pattern: &[u8]) -> u64 {
    let mut count = 0;
    let mut at = 0;
    let mut after_item = false;
    while let Some(&b) = pattern.get(at) {
        match b {
            b'%' => {
                at += 2;
                after_item = true;
                continue;
            }
            b'[' => {
                at += 1;
                if pattern.get(at) == Some(&b'^') {
                    at += 1;
                }
                // A `]` first in a set is the character itself.
                if pattern.get(at) == Some(&b']') {
                    at += 1;
                }
                while let Some(&c) = pattern.get(at) {
                    at += if c == b'%' { 2 } else { 1 };
                    if c == b']' {
                        break;
                    }
                }
                after_item = true;
                continue;
            }
            b'*' | b'+' | b'-' if after_item => {
                count += 1;
                after_item = false;
            }
            b'?' if after_item => {
                count += 1;
                after_item = false;
            }
            b'(' | b')' => after_item = false,
            _ => after_item = true,
        }
        at += 1;
    }
    count
}

/// The bytes a builtin works through before it returns, from its arguments: what it reads and writes, not the size of
/// what it is handed (`s:sub(i, i)` of a long string touches one byte, `table.insert` at the end shifts nothing). A
/// search is charged the part of the subject it scans from its start, once more for each quantifier of its pattern.
fn work_of(name: &str, args: &MultiValue) -> u64 {
    let a = Args(args);
    let slots = |n: u64| n.saturating_mul(SLOT_BYTES);
    match name {
        "string.sub" => a.span(a.len(0), 1, 2, 1, -1),
        "string.byte" => {
            let i = a.int(1).unwrap_or(1);
            a.span(a.len(0), 1, 2, i, i)
        }
        "string.find" | "string.match" | "string.gmatch" | "string.gsub" => {
            let init = if name == "string.gsub" { usize::MAX } else { 2 };
            a.scanned(init).saturating_mul(1 + a.quantifiers(name))
        }
        "string.rep" => (a.len(0) + a.len(2)).saturating_mul(a.count(1, 0)),
        "string.lower" | "string.upper" | "string.reverse" | "string.split" | "string.len" => {
            a.len(0)
        }
        "string.unpack" => a.len(1),
        "table.insert" => {
            if args.len() >= 3 {
                let n = a.len(0) as i64;
                slots((n - a.int(1).unwrap_or(n + 1) + 1).max(0) as u64)
            } else {
                0
            }
        }
        "table.remove" => {
            let n = a.len(0) as i64;
            slots((n - a.int(1).unwrap_or(n)).max(0) as u64)
        }
        "table.unpack" => slots(a.span(a.len(0), 1, 2, 1, a.len(0) as i64)),
        "table.concat" => slots(a.span(a.len(0), 2, 3, 1, a.len(0) as i64)),
        "table.move" => {
            let (f, e) = (a.int(1).unwrap_or(1), a.int(2).unwrap_or(0));
            slots((e - f + 1).max(0) as u64)
        }
        "table.find" => slots(a.span(a.len(0), 2, usize::MAX, 1, a.len(0) as i64)),
        "table.clone" => slots(a.len(0)),
        "table.sort" => {
            let n = a.len(0);
            slots(n.saturating_mul(u64::from(64 - n.leading_zeros())))
        }
        "table.create" => slots(a.count(0, 0)),
        "buffer.create" => a.count(0, 0),
        "buffer.fromstring" | "buffer.tostring" => a.len(0),
        "buffer.readstring" => a.count(2, 0),
        "buffer.writestring" => a.count(3, a.len(2)),
        "buffer.fill" => {
            let offset = a.count(1, 0);
            a.count(3, a.len(0).saturating_sub(offset))
        }
        "buffer.copy" => {
            let offset = a.count(3, 0);
            a.count(4, a.len(2).saturating_sub(offset))
        }
        "utf8.codepoint" => a.span(a.len(0), 1, 2, 1, a.int(1).unwrap_or(1)),
        "utf8.len" => a.span(a.len(0), 1, 2, 1, -1),
        "utf8.codes" | "utf8.offset" => a.len(0),
        _ => 0,
    }
}

/// The builtins whose result is the work: charged by the bytes of the text they make, after they make it.
const CHARGED_BY_RESULT: &[&str] = &[
    "string.char",
    "string.format",
    "string.gsub",
    "string.pack",
    "table.concat",
    "utf8.char",
];

/// Wraps the builtins that work through a whole string, table or buffer in one call, which Luau's interrupts do not see:
/// each is charged by the bytes it reads and writes ([`work_of`], before it runs; [`CHARGED_BY_RESULT`], after), and one
/// that hits the memory cap gets a full collection and a second try.
fn charge_builtins(lua: &Lua, globals: &Table) -> mlua::Result<()> {
    const BULK: &[(&str, &[&str])] = &[
        (
            "string",
            &[
                "byte", "char", "find", "format", "gmatch", "gsub", "len", "lower", "match",
                "pack", "rep", "reverse", "split", "sub", "unpack", "upper",
            ],
        ),
        (
            "table",
            &[
                "clone", "concat", "create", "find", "insert", "move", "remove", "sort", "unpack",
            ],
        ),
        (
            "buffer",
            &[
                "copy",
                "create",
                "fill",
                "fromstring",
                "readstring",
                "tostring",
                "writestring",
            ],
        ),
        ("utf8", &["char", "codepoint", "codes", "len", "offset"]),
    ];
    for (library, names) in BULK {
        let Some(table) = globals.raw_get::<Option<Table>>(*library)? else {
            continue;
        };
        for name in *names {
            let Some(original) = table.raw_get::<Option<Function>>(*name)? else {
                continue;
            };
            let full = format!("{library}.{name}");
            let by_result = CHARGED_BY_RESULT.contains(&full.as_str());
            let search = matches!(
                full.as_str(),
                "string.find" | "string.match" | "string.gmatch" | "string.gsub"
            );
            let wrapped = lua.create_function(move |lua, args: MultiValue| {
                if search && Args(&args).quantifiers(&full) > MAX_BACKTRACKING {
                    return Err(Error::runtime(format!(
                        "{full}: the pattern has more than {MAX_BACKTRACKING} of `*`, `+`, `-` and `?`, which can take too long"
                    )));
                }
                charge_bytes(lua, work_of(&full, &args))?;
                let result = match original.call::<MultiValue>(args.clone()) {
                    Err(error) if is_memory_error(&error) => {
                        let _ = lua.gc_collect();
                        original.call::<MultiValue>(args)
                    }
                    other => other,
                }?;
                if by_result {
                    let made: u64 = result
                        .iter()
                        .map(|v| match v {
                            Value::String(s) => s.as_bytes().len() as u64,
                            _ => 0,
                        })
                        .fold(0, u64::saturating_add);
                    charge_bytes(lua, made)?;
                }
                Ok(result)
            })?;
            table.raw_set(*name, wrapped)?;
        }
    }
    Ok(())
}

/// `print(...)`: the values as `tostring` writes them, separated by spaces (the console shows a tab as `^`), on the
/// console.
fn print(lua: &Lua, values: MultiValue) -> mlua::Result<()> {
    let mut line = String::new();
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            line.push(' ');
        }
        line.push_str(&value.to_string()?);
    }
    with_call(lua, |call| call.print(&line))
}

/// Whether `source` is compiled code rather than text: Luau's bytecode starts with its version byte.
fn is_binary(source: &[u8]) -> bool {
    source
        .first()
        .is_some_and(|&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0c))
}

/// The folder whose files a script's `require` loads: a mod's own (`mods/<id>`, where the engine runs its scripts), or
/// for another file its own folder.
pub(crate) fn root_of(path: &str) -> String {
    let mut parts = path.split('/');
    if let (Some(first), Some(id), Some(_)) = (parts.next(), parts.next(), parts.next())
        && first.eq_ignore_ascii_case("mods")
    {
        return format!("mods/{id}");
    }
    path.rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_default()
}

/// The global table of `root` in the realm (made at its first file, with its own `ts` and `require`; its definitions go
/// into `package`): writes stay in it, reads go on to the read-only built-ins.
pub(crate) fn env_for(lua: &Lua, root: &str, package: &str) -> mlua::Result<Table> {
    let envs: Table = lua.named_registry_value(ENVS)?;
    let key = root.to_ascii_lowercase();
    if let Some(env) = envs.raw_get::<Option<Table>>(key.as_str())? {
        return Ok(env);
    }
    let env = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.raw_set("__index", lua.named_registry_value::<Table>(GLOBALS)?)?;
    meta.raw_set("__metatable", "locked")?;
    meta.set_readonly(true);
    env.set_metatable(Some(meta))?;
    env.raw_set("ts", crate::api::ts_table(lua, package)?)?;
    env.raw_set("require", crate::require::require_function(lua, root)?)?;
    envs.raw_set(key.as_str(), env.clone())?;
    Ok(env)
}

/// The loaded modules of `root`.
pub(crate) fn modules_of(lua: &Lua, root: &str) -> mlua::Result<Table> {
    let modules: Table = lua.named_registry_value(MODULES)?;
    let key = root.to_ascii_lowercase();
    if let Some(table) = modules.raw_get::<Option<Table>>(key.as_str())? {
        return Ok(table);
    }
    let table = lua.create_table()?;
    modules.raw_set(key.as_str(), table.clone())?;
    Ok(table)
}
