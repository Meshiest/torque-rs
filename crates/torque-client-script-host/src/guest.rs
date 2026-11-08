//! The Lua extension's WebAssembly module under Wasmtime: the native counterpart of `web/lua.js`.
//!
//! The module's `torque` imports name the call they are made for by a handle; each call into the module pushes its
//! [`LuaHost`] under a fresh handle, and a request is served only for the innermost running call (an outer call's host is
//! borrowed further up the stack while an inner one runs). A request may run script that calls the module again (Lua and
//! TorqueScript calling each other): the store is then reached through the caller of the import being served, which is
//! kept for the time it runs.

#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::hash::{BuildHasher, RandomState};
use std::ops::Range;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::{Duration, Instant};

use torque_engine::Side;
use torque_plugin_abi::{
    TORQUE_ERR_FAILED, TORQUE_ERR_HANDLE, TORQUE_ERR_INVALID, TORQUE_ERR_LIMIT, TORQUE_ERR_PANIC,
    TORQUE_ERR_RANGE, TORQUE_LOG_ERROR, TORQUE_LOG_WARN, TORQUE_OK, TORQUE_SIDE_BOTH,
    TORQUE_SIDE_CLIENT, TORQUE_SIDE_SERVER, TorqueStatus,
};
use torque_plugin_loader::web_lua::{HostError, LuaHost, LuaModule};
use torque_ts_vm::vm::{Level, VmError};
use wasmtime::{
    AsContext, AsContextMut, Caller, Collector, Config, Engine, InstancePre, Linker, Memory,
    Module, Store, StoreContextMut, StoreLimits, StoreLimitsBuilder, Trap, TypedFunc, format_err,
};

/// WASI's error numbers the imports answer with.
const ERRNO_BADF: i32 = 8;
const ERRNO_NOTSUP: i32 = 58;
const ERRNO_NOTCAPABLE: i32 = 76;
/// WASI's clocks: the monotonic one and the process's and thread's CPU time (C's `clock()`); the wall clock is refused.
const MONOTONIC: i32 = 1;
const PROCESS_CPUTIME: i32 = 2;
const THREAD_CPUTIME: i32 = 3;
/// A line the module writes to its standard output or error is printed in pieces of this many bytes at most.
const MAX_LINE: usize = 4096;
/// How many texts an argument list (or `fd_write`'s list of buffers) may name (the C ABI's cap is far lower; the host
/// checks that one).
const MAX_TEXTS: u32 = 1024;
/// The most bytes such a list may name in all: past it the list is refused before anything is copied.
const MAX_TEXT_BYTES: usize = 4 << 20;
/// WASI's error number for a refused argument.
const ERRNO_INVAL: i32 = 28;
/// The most table elements an instance may have (the module's function table is a few thousand).
const MAX_TABLE_ELEMENTS: usize = 1 << 16;
/// What the module writes to standard output and error: at most this many lines a second are printed, and one notice
/// for those dropped.
const LINES_PER_SECOND: u32 = 20;

/// A Wasmtime engine for guests: exception handling on, epoch interruption on, ticked by a thread of its own every `tick`.
#[derive(Clone)]
pub struct GuestEngine {
    engine: Engine,
    tick: Duration,
}

impl GuestEngine {
    /// An engine whose epoch advances every `tick` (the deadlines' resolution).
    pub fn new(tick: Duration) -> wasmtime::Result<GuestEngine> {
        let mut config = Config::new();
        config
            .wasm_exceptions(true)
            .epoch_interruption(true)
            // Exception objects are collected (the null collector never frees one, and Luau throws one per error).
            .collector(Collector::DeferredReferenceCounting);
        let engine = Engine::new(&config)?;
        let weak = engine.weak();
        let tick = tick.max(Duration::from_millis(1));
        std::thread::Builder::new()
            .name("guest-epoch".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(tick);
                    match weak.upgrade() {
                        Some(engine) => engine.increment_epoch(),
                        None => break,
                    }
                }
            })
            .map_err(|e| format_err!("the epoch thread did not start: {e}"))?;
        Ok(GuestEngine { engine, tick })
    }

    /// The Wasmtime engine.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The epoch ticks a deadline of `deadline` from now takes: never fewer than it, at most one tick more.
    pub fn ticks(&self, deadline: Duration) -> u64 {
        let ticks = deadline.as_nanos().div_ceil(self.tick.as_nanos().max(1));
        u64::try_from(ticks).unwrap_or(u64::MAX - 1) + 1
    }
}

/// Whether `error` (a call's) is a trap of the epoch deadline.
pub fn is_deadline(error: &wasmtime::Error) -> bool {
    error.downcast_ref::<Trap>() == Some(&Trap::Interrupt)
}

/// The caps of one instance of a guest.
#[derive(Clone, Copy, Debug)]
pub struct GuestLimits {
    /// The most linear memory it may have, in bytes. Wasmtime holds the guest's exception objects in a GC heap that the
    /// same limit caps on its own, so an instance may hold up to twice this in all.
    pub memory: usize,
    /// How long one call into it may run, its nested calls included.
    pub deadline: Duration,
}

/// What an instance has done, for measuring: the steps the module charged and the requests it made.
#[derive(Debug, Default)]
pub struct Counters {
    steps: Cell<u64>,
    requests: Cell<u64>,
}

impl Counters {
    /// The Luau steps the module charged (one per interrupt).
    pub fn steps(&self) -> u64 {
        self.steps.get()
    }

    /// The `torque` imports the module called.
    pub fn requests(&self) -> u64 {
        self.requests.get()
    }

    fn add_request(&self) {
        self.requests.set(self.requests.get().saturating_add(1));
    }
}

/// The caller of the import being served, for a call into the module made while it runs.
type ActiveCaller = Rc<Cell<Option<NonNull<Caller<'static, Data>>>>>;

/// One running call into the module.
struct Running {
    handle: u32,
    /// The call's host: a pointer to the `&mut dyn LuaHost` its entry holds for the time the call runs. Taken out while
    /// a request of the call is being served (a request made meanwhile with the same handle finds none), and `None` for
    /// a call that serves no requests (`torque_lua_vm_dropped`).
    host: Option<NonNull<&'static mut dyn LuaHost>>,
    /// The limit that stopped one of its requests, handed back when the call returns.
    stopped: Option<VmError>,
}

/// A store's data.
struct Data {
    limits: StoreLimits,
    memory: Option<Memory>,
    /// The running calls, innermost last.
    running: Vec<Running>,
    /// The answer the module copies out next (`result_len`, `result_copy`).
    pending: Vec<u8>,
    /// What the module writes to standard output and error.
    printer: Printer,
    /// Set when a call into the module trapped or panicked, shared with the module: an import serving a request whose
    /// nested call broke the module traps too, so that the outer call stops.
    broken: Rc<Cell<bool>>,
    started: Instant,
    random: RandomState,
    draws: u64,
    active: ActiveCaller,
    counters: Rc<Counters>,
}

impl Data {
    fn new(limits: GuestLimits) -> Data {
        Data {
            limits: StoreLimitsBuilder::new()
                .memory_size(limits.memory)
                .table_elements(MAX_TABLE_ELEMENTS)
                .instances(1)
                .tables(1)
                .memories(1)
                .build(),
            memory: None,
            running: Vec::new(),
            pending: Vec::new(),
            printer: Printer::default(),
            broken: Rc::new(Cell::new(false)),
            started: Instant::now(),
            random: RandomState::new(),
            draws: 0,
            active: Rc::new(Cell::new(None)),
            counters: Rc::new(Counters::default()),
        }
    }
}

/// The Lua module compiled and linked, ready to be instantiated any number of times.
#[derive(Clone)]
pub struct LuaGuest {
    engine: GuestEngine,
    pre: InstancePre<Data>,
}

/// `torque_lua_run_file`'s parameters: the call, the script engine, its side, then the path, the source and the package
/// (a pointer and a length each).
type RunFileParams = (i32, i64, i32, i32, i32, i32, i32, i32, i32);

/// The module's exports a call uses.
#[derive(Clone)]
struct Exports {
    alloc: TypedFunc<i32, i32>,
    free: TypedFunc<(i32, i32), ()>,
    run_file: TypedFunc<RunFileParams, i32>,
    call_defined: TypedFunc<(i32, i64, i32, i64, i32, i32), i32>,
    result: TypedFunc<(), i32>,
    result_len: TypedFunc<(), i32>,
    vm_dropped: TypedFunc<i64, ()>,
}

impl LuaGuest {
    /// Compiles the module's bytes (`torque_lua.wasm`) and links its imports.
    pub fn new(engine: &GuestEngine, wasm: &[u8]) -> wasmtime::Result<LuaGuest> {
        LuaGuest::link(engine, Module::new(engine.engine(), wasm)?)
    }

    /// Links the imports of `module`, compiled by `engine`.
    pub fn link(engine: &GuestEngine, module: Module) -> wasmtime::Result<LuaGuest> {
        let mut linker = Linker::new(engine.engine());
        wasi(&mut linker)?;
        torque(&mut linker)?;
        let pre = linker.instantiate_pre(&module)?;
        Ok(LuaGuest {
            engine: engine.clone(),
            pre,
        })
    }

    /// The compiled module.
    pub fn module(&self) -> &Module {
        self.pre.module()
    }

    /// A fresh instance, started (its constructors and the extension's entry point run).
    pub fn instantiate(&self, limits: GuestLimits) -> wasmtime::Result<WasmtimeLuaModule> {
        let data = Data::new(limits);
        let active = Rc::clone(&data.active);
        let counters = Rc::clone(&data.counters);
        let broken = Rc::clone(&data.broken);
        let deadline = self.engine.ticks(limits.deadline);
        let mut store = Store::new(self.engine.engine(), data);
        store.limiter(|data| &mut data.limits);
        store.set_epoch_deadline(deadline);
        let instance = self.pre.instantiate(&mut store)?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| format_err!("the module exports no memory"))?;
        store.data_mut().memory = Some(memory);
        let exports = Exports {
            alloc: instance.get_typed_func(&mut store, "torque_lua_alloc")?,
            free: instance.get_typed_func(&mut store, "torque_lua_free")?,
            run_file: instance.get_typed_func(&mut store, "torque_lua_run_file")?,
            call_defined: instance.get_typed_func(&mut store, "torque_lua_call_defined")?,
            result: instance.get_typed_func(&mut store, "torque_lua_result")?,
            result_len: instance.get_typed_func(&mut store, "torque_lua_result_len")?,
            vm_dropped: instance.get_typed_func(&mut store, "torque_lua_vm_dropped")?,
        };
        instance
            .get_typed_func::<(), ()>(&mut store, "_initialize")?
            .call(&mut store, ())?;
        let started = instance
            .get_typed_func::<(), i32>(&mut store, "torque_lua_start")?
            .call(&mut store, ())?;
        if started != TORQUE_OK as i32 {
            return Err(format_err!("the module did not start (status {started})"));
        }
        Ok(WasmtimeLuaModule {
            store: RefCell::new(store),
            active,
            counters,
            exports,
            deadline,
            broken,
            next_handle: Cell::new(1),
        })
    }
}

/// One instance of the Lua module, as the engine's [`LuaModule`]: what `web/lua.js` is to a page.
pub struct WasmtimeLuaModule {
    store: RefCell<Store<Data>>,
    active: ActiveCaller,
    counters: Rc<Counters>,
    exports: Exports,
    /// Epoch ticks per outermost call.
    deadline: u64,
    broken: Rc<Cell<bool>>,
    next_handle: Cell<u32>,
}

/// What a call into the module came to.
enum Outcome {
    /// It returned this status, with the call's result (`call_defined`'s) and the limit that stopped one of its requests.
    Returned(TorqueStatus, Vec<u8>, Option<VmError>),
    /// It trapped: the module is broken from now on.
    Trapped(wasmtime::Error),
    /// The module was broken before.
    Broken,
}

impl WasmtimeLuaModule {
    /// What the instance has done so far.
    pub fn counters(&self) -> Rc<Counters> {
        Rc::clone(&self.counters)
    }

    /// Whether the module trapped (and runs nothing more).
    pub fn is_broken(&self) -> bool {
        self.broken.get()
    }

    /// Runs `f` with the store: the outermost call's own (its deadline set again), or the one of the import that is
    /// being served, for a call made while it runs. `None` when neither is at hand.
    fn with_store<R>(&self, f: impl FnOnce(StoreContextMut<'_, Data>) -> R) -> Option<R> {
        if let Ok(mut store) = self.store.try_borrow_mut() {
            // Nothing runs: a call a panic cut short left nothing behind.
            store.data_mut().running.clear();
            self.active.set(None);
            store.set_epoch_deadline(self.deadline);
            return Some(f(store.as_context_mut()));
        }
        let caller = self.active.get()?;
        // SAFETY: `active` holds the caller of the import being served, set by `serve` for exactly the time that import
        // runs and restored when it returns (or unwinds). This call is made from within that import (the store is
        // borrowed only while a call runs, and the module reaches the engine only through its imports), which uses its
        // caller no more until this returns: the one live use of it is here.
        let caller = unsafe { &mut *caller.as_ptr() };
        Some(f(caller.as_context_mut()))
    }

    /// A fresh call handle.
    fn handle(&self) -> u32 {
        let handle = self.next_handle.get();
        self.next_handle.set(if handle >= u32::MAX - 1 {
            1
        } else {
            handle + 1
        });
        handle
    }

    /// Runs `f` as a call into the module whose requests `host` serves.
    fn call(
        &self,
        host: &mut dyn LuaHost,
        f: impl FnOnce(
            &mut StoreContextMut<'_, Data>,
            &Exports,
            u32,
        ) -> wasmtime::Result<(i32, Vec<u8>)>,
    ) -> Outcome {
        if self.broken.get() {
            return Outcome::Broken;
        }
        let handle = self.handle();
        let mut host: &mut dyn LuaHost = host;
        let pointer = NonNull::from(&mut host).cast::<&'static mut dyn LuaHost>();
        let _unwind = BreakOnUnwind(&self.broken);
        let outcome = self.with_store(|mut store| {
            store.data_mut().running.push(Running {
                handle,
                host: Some(pointer),
                stopped: None,
            });
            let result = f(&mut store, &self.exports, handle);
            let running = &mut store.data_mut().running;
            let stopped = running
                .iter()
                .rposition(|r| r.handle == handle)
                .and_then(|at| running.remove(at).stopped);
            (result, stopped)
        });
        match outcome {
            None => Outcome::Returned(TORQUE_ERR_HANDLE, Vec::new(), None),
            Some((Ok((status, result)), stopped)) => {
                Outcome::Returned(status as TorqueStatus, result, stopped)
            }
            Some((Err(error), _)) => {
                self.broken.set(true);
                Outcome::Trapped(error)
            }
        }
    }

    /// What a call came to for the engine, as `web_lua`'s page bridge reports it.
    fn finish(host: &mut dyn LuaHost, outcome: Outcome) -> Result<Vec<u8>, VmError> {
        let (status, result, stopped) = match outcome {
            Outcome::Returned(status, result, stopped) => (status, result, stopped),
            Outcome::Trapped(error) => {
                let why = if is_deadline(&error) {
                    "it ran past its deadline".to_string()
                } else {
                    format!("it trapped: {error}")
                };
                host.print(
                    Level::Error,
                    &format!("the Lua module stopped working ({why}); Lua does not run again in this engine"),
                );
                return Ok(Vec::new());
            }
            Outcome::Broken => (TORQUE_ERR_PANIC, Vec::new(), None),
        };
        match status {
            TORQUE_OK => Ok(result),
            TORQUE_ERR_LIMIT => Err(stopped.unwrap_or(VmError::StepLimit)),
            TORQUE_ERR_PANIC => {
                host.print(
                    Level::Error,
                    "the Lua module stopped working (it trapped or panicked); Lua does not run again in this engine",
                );
                Ok(Vec::new())
            }
            status => {
                host.print(
                    Level::Error,
                    &format!("the Lua module refused the call (status {status})"),
                );
                Ok(Vec::new())
            }
        }
    }
}

impl LuaModule for WasmtimeLuaModule {
    fn run_file(&self, host: &mut dyn LuaHost, path: &str, source: &[u8]) -> Result<(), VmError> {
        let (realm, side) = host.realm();
        let package = host.package().as_bytes().to_vec();
        let outcome = self.call(host, |store, exports, handle| {
            let texts = [
                put(store, exports, path.as_bytes())?,
                put(store, exports, source)?,
                put(store, exports, &package)?,
            ];
            let [
                (path, path_len),
                (source, source_len),
                (package, package_len),
            ] = texts;
            let status = exports.run_file.call(
                &mut *store,
                (
                    handle as i32,
                    realm as i64,
                    side_code(side) as i32,
                    path,
                    path_len,
                    source,
                    source_len,
                    package,
                    package_len,
                ),
            )?;
            for (ptr, len) in texts {
                exports.free.call(&mut *store, (ptr, len))?;
            }
            Ok((status, Vec::new()))
        });
        Self::finish(host, outcome).map(|_| ())
    }

    fn call_defined(
        &self,
        host: &mut dyn LuaHost,
        id: u64,
        args: &[Vec<u8>],
    ) -> Result<Vec<u8>, VmError> {
        let (realm, side) = host.realm();
        let outcome = self.call(host, |store, exports, handle| {
            let mut texts = Vec::with_capacity(args.len());
            for arg in args {
                texts.push(put(store, exports, arg)?);
            }
            let size =
                i32::try_from(8 * texts.len()).map_err(|_| format_err!("too many arguments"))?;
            let list = exports.alloc.call(&mut *store, size)?;
            let mut bytes = Vec::with_capacity(8 * texts.len());
            for &(ptr, len) in &texts {
                bytes.extend_from_slice(&ptr.to_le_bytes());
                bytes.extend_from_slice(&len.to_le_bytes());
            }
            write(store, list, &bytes)?;
            let status = exports.call_defined.call(
                &mut *store,
                (
                    handle as i32,
                    realm as i64,
                    side_code(side) as i32,
                    id as i64,
                    list,
                    texts.len() as i32,
                ),
            )?;
            let mut result = Vec::new();
            if status == TORQUE_OK as i32 {
                let ptr = exports.result.call(&mut *store, ())?;
                let len = exports.result_len.call(&mut *store, ())?;
                result = read(store, ptr, len)?;
            }
            texts.push((list, size));
            for (ptr, len) in texts {
                exports.free.call(&mut *store, (ptr, len))?;
            }
            Ok((status, result))
        });
        Self::finish(host, outcome)
    }

    fn realm_dropped(&self, realm: u64) {
        if self.broken.get() {
            return;
        }
        let handle = self.handle();
        let _unwind = BreakOnUnwind(&self.broken);
        let dropped = self.with_store(|mut store| {
            // A call of its own that serves no requests: every handle the module names meanwhile is refused.
            store.data_mut().running.push(Running {
                handle,
                host: None,
                stopped: None,
            });
            let dropped = self
                .exports
                .vm_dropped
                .call(&mut store, realm as i64)
                .is_ok();
            let running = &mut store.data_mut().running;
            if let Some(at) = running.iter().rposition(|r| r.handle == handle) {
                running.remove(at);
            }
            dropped
        });
        if dropped == Some(false) {
            self.broken.set(true);
        }
    }
}

/// Marks the module broken when a call into it is cut short by a panic (of a host callback or the engine): its shadow
/// stack and its running calls are left half done.
struct BreakOnUnwind<'a>(&'a Cell<bool>);

impl Drop for BreakOnUnwind<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.set(true);
        }
    }
}

/// The C ABI's code for `side`.
fn side_code(side: Side) -> u32 {
    match side {
        Side::Both => TORQUE_SIDE_BOTH,
        Side::Server => TORQUE_SIDE_SERVER,
        Side::Client => TORQUE_SIDE_CLIENT,
    }
}

/// The module's memory.
fn memory(store: &impl AsContext<Data = Data>) -> wasmtime::Result<Memory> {
    store
        .as_context()
        .data()
        .memory
        .ok_or_else(|| format_err!("the module has no memory yet"))
}

/// The range of the `len` bytes at `ptr`, checked to lie in the module's memory (a range out of it traps the module).
fn range(
    store: &impl AsContext<Data = Data>,
    ptr: i32,
    len: i32,
) -> wasmtime::Result<Range<usize>> {
    let size = memory(store)?.data_size(store);
    let start = ptr as u32 as usize;
    start
        .checked_add(len as u32 as usize)
        .filter(|&end| end <= size)
        .map(|end| start..end)
        .ok_or_else(|| format_err!("the module named bytes outside its memory"))
}

/// The `len` bytes at `ptr` in the module's memory (a pointer out of it traps the module).
fn read(store: &impl AsContext<Data = Data>, ptr: i32, len: i32) -> wasmtime::Result<Vec<u8>> {
    let range = range(store, ptr, len)?;
    Ok(memory(store)?.data(store)[range].to_vec())
}

/// Writes `bytes` at `ptr` in the module's memory.
fn write(
    store: &mut impl AsContextMut<Data = Data>,
    ptr: i32,
    bytes: &[u8],
) -> wasmtime::Result<()> {
    let memory = memory(store)?;
    let start = ptr as u32 as usize;
    let end = start.checked_add(bytes.len());
    end.and_then(|end| memory.data_mut(&mut *store).get_mut(start..end))
        .ok_or_else(|| format_err!("the module named bytes outside its memory"))?
        .copy_from_slice(bytes);
    Ok(())
}

fn write_u32(
    store: &mut impl AsContextMut<Data = Data>,
    ptr: i32,
    value: u32,
) -> wasmtime::Result<()> {
    write(store, ptr, &value.to_le_bytes())
}

fn write_u64(
    store: &mut impl AsContextMut<Data = Data>,
    ptr: i32,
    value: u64,
) -> wasmtime::Result<()> {
    write(store, ptr, &value.to_le_bytes())
}

/// The ranges of the `count` texts (a pointer and a length each) at `ptr`, each checked to lie in the module's memory
/// (one out of it traps the module): `None` when there are more than [`MAX_TEXTS`] or they name more than
/// [`MAX_TEXT_BYTES`] in all.
fn text_ranges(
    store: &impl AsContext<Data = Data>,
    ptr: i32,
    count: i32,
) -> wasmtime::Result<Option<Vec<Range<usize>>>> {
    let count = count as u32;
    if count > MAX_TEXTS {
        return Ok(None);
    }
    let list = read(store, ptr, (count * 8) as i32)?;
    let mut ranges = Vec::with_capacity(count as usize);
    let mut total = 0usize;
    for pair in list.as_chunks::<8>().0 {
        let ptr = i32::from_le_bytes([pair[0], pair[1], pair[2], pair[3]]);
        let len = i32::from_le_bytes([pair[4], pair[5], pair[6], pair[7]]);
        let range = range(store, ptr, len)?;
        match total.checked_add(range.len()) {
            Some(sum) if sum <= MAX_TEXT_BYTES => total = sum,
            _ => return Ok(None),
        }
        ranges.push(range);
    }
    Ok(Some(ranges))
}

/// The `count` texts (a pointer and a length each) at `ptr`: `None` when [`text_ranges`] refuses them.
fn read_texts(
    store: &impl AsContext<Data = Data>,
    ptr: i32,
    count: i32,
) -> wasmtime::Result<Option<Vec<Vec<u8>>>> {
    let Some(ranges) = text_ranges(store, ptr, count)? else {
        return Ok(None);
    };
    let bytes = memory(store)?.data(store);
    Ok(Some(
        ranges.into_iter().map(|r| bytes[r].to_vec()).collect(),
    ))
}

/// Copies `bytes` into the module: its pointer and length, freed with the module's `torque_lua_free`.
fn put(
    store: &mut StoreContextMut<'_, Data>,
    exports: &Exports,
    bytes: &[u8],
) -> wasmtime::Result<(i32, i32)> {
    let len =
        i32::try_from(bytes.len()).map_err(|_| format_err!("a text too long for the module"))?;
    let ptr = exports.alloc.call(&mut *store, len)?;
    write(store, ptr, bytes)?;
    Ok((ptr, len))
}

/// The module's WASI imports: nothing reaches anything but the monotonic clock, random bytes and standard error.
fn wasi(linker: &mut Linker<Data>) -> wasmtime::Result<()> {
    const W: &str = "wasi_snapshot_preview1";
    linker.func_wrap(W, "args_get", |_: Caller<'_, Data>, _: i32, _: i32| {
        TORQUE_OK as i32
    })?;
    linker.func_wrap(
        W,
        "args_sizes_get",
        |mut c: Caller<'_, Data>, count: i32, size: i32| {
            write_u32(&mut c, count, 0)?;
            write_u32(&mut c, size, 0)?;
            Ok(TORQUE_OK as i32)
        },
    )?;
    linker.func_wrap(W, "environ_get", |_: Caller<'_, Data>, _: i32, _: i32| {
        TORQUE_OK as i32
    })?;
    linker.func_wrap(
        W,
        "environ_sizes_get",
        |mut c: Caller<'_, Data>, count: i32, size: i32| {
            write_u32(&mut c, count, 0)?;
            write_u32(&mut c, size, 0)?;
            Ok(TORQUE_OK as i32)
        },
    )?;
    let clock = |id: i32| matches!(id, MONOTONIC | PROCESS_CPUTIME | THREAD_CPUTIME);
    linker.func_wrap(
        W,
        "clock_res_get",
        move |mut c: Caller<'_, Data>, id: i32, out: i32| {
            if !clock(id) {
                return Ok(ERRNO_NOTSUP);
            }
            write_u64(&mut c, out, 1000)?;
            Ok(TORQUE_OK as i32)
        },
    )?;
    linker.func_wrap(
        W,
        "clock_time_get",
        move |mut c: Caller<'_, Data>, id: i32, _precision: i64, out: i32| {
            if !clock(id) {
                return Ok(ERRNO_NOTSUP);
            }
            let nanos = u64::try_from(c.data().started.elapsed().as_nanos()).unwrap_or(u64::MAX);
            write_u64(&mut c, out, nanos)?;
            Ok(TORQUE_OK as i32)
        },
    )?;
    linker.func_wrap(
        W,
        "fd_write",
        |mut c: Caller<'_, Data>, fd: i32, iovs: i32, count: i32, written: i32| {
            if fd != 1 && fd != 2 {
                return Ok(ERRNO_BADF);
            }
            let Some(ranges) = text_ranges(&c, iovs, count)? else {
                return Ok(ERRNO_INVAL);
            };
            let mut n = 0u32;
            let memory = memory(&c)?;
            let (bytes, data) = memory.data_and_store_mut(&mut c);
            for range in ranges {
                n = n.saturating_add(range.len() as u32);
                // Streamed a line's length at a time: the output never holds more than a line and a piece.
                for piece in bytes[range].chunks(MAX_LINE) {
                    data.printer.write(piece);
                }
            }
            write_u32(&mut c, written, n)?;
            Ok(TORQUE_OK as i32)
        },
    )?;
    linker.func_wrap(W, "fd_close", |_: Caller<'_, Data>, _: i32| ERRNO_BADF)?;
    linker.func_wrap(W, "fd_fdstat_get", |_: Caller<'_, Data>, _: i32, _: i32| {
        ERRNO_BADF
    })?;
    linker.func_wrap(
        W,
        "fd_filestat_get",
        |_: Caller<'_, Data>, _: i32, _: i32| ERRNO_BADF,
    )?;
    linker.func_wrap(
        W,
        "fd_prestat_get",
        |_: Caller<'_, Data>, _: i32, _: i32| ERRNO_BADF,
    )?;
    linker.func_wrap(
        W,
        "fd_prestat_dir_name",
        |_: Caller<'_, Data>, _: i32, _: i32, _: i32| ERRNO_BADF,
    )?;
    linker.func_wrap(
        W,
        "fd_read",
        |_: Caller<'_, Data>, _: i32, _: i32, _: i32, _: i32| ERRNO_BADF,
    )?;
    linker.func_wrap(
        W,
        "fd_seek",
        |_: Caller<'_, Data>, _: i32, _: i64, _: i32, _: i32| ERRNO_BADF,
    )?;
    linker.func_wrap(
        W,
        "path_filestat_get",
        |_: Caller<'_, Data>, _: i32, _: i32, _: i32, _: i32, _: i32| ERRNO_NOTCAPABLE,
    )?;
    linker.func_wrap(
        W,
        "path_open",
        |_: Caller<'_, Data>,
         _: i32,
         _: i32,
         _: i32,
         _: i32,
         _: i32,
         _: i64,
         _: i64,
         _: i32,
         _: i32| ERRNO_NOTCAPABLE,
    )?;
    linker.func_wrap(
        W,
        "proc_exit",
        |_: Caller<'_, Data>, code: i32| -> wasmtime::Result<()> {
            Err(format_err!("the module exited ({code})"))
        },
    )?;
    linker.func_wrap(
        W,
        "random_get",
        |mut c: Caller<'_, Data>, ptr: i32, len: i32| {
            // The range is checked first, then filled in place: nothing of its size is made on this side.
            let range = range(&c, ptr, len)?;
            if range.len() > MAX_TEXT_BYTES {
                return Ok(ERRNO_INVAL);
            }
            let memory = memory(&c)?;
            let (bytes, data) = memory.data_and_store_mut(&mut c);
            for chunk in bytes[range].chunks_mut(8) {
                data.draws = data.draws.wrapping_add(1);
                let word = data.random.hash_one(data.draws).to_le_bytes();
                chunk.copy_from_slice(&word[..chunk.len()]);
            }
            Ok(TORQUE_OK as i32)
        },
    )?;
    linker.func_wrap(W, "sched_yield", |_: Caller<'_, Data>| TORQUE_OK as i32)?;
    Ok(())
}

/// What the module writes to standard output and error, printed on this process's standard error a line at a time (a
/// line that never ends in pieces of [`MAX_LINE`]), at most [`LINES_PER_SECOND`] lines a second.
#[derive(Default)]
struct Printer {
    /// What was written since the last full line.
    partial: Vec<u8>,
    /// When the current second started, and the lines printed and dropped in it.
    second: Option<Instant>,
    lines: u32,
    dropped: u64,
    /// Lines printed in all.
    printed: u64,
}

impl Printer {
    /// Takes `bytes` of output (of at most [`MAX_LINE`]): every full line in them is printed.
    fn write(&mut self, mut bytes: &[u8]) {
        while let Some(at) = bytes.iter().position(|&b| b == b'\n') {
            self.partial.extend_from_slice(&bytes[..at]);
            let line = std::mem::take(&mut self.partial);
            self.line(&line);
            bytes = &bytes[at + 1..];
        }
        self.partial.extend_from_slice(bytes);
        while self.partial.len() > MAX_LINE {
            let rest = self.partial.split_off(MAX_LINE);
            let line = std::mem::replace(&mut self.partial, rest);
            self.line(&line);
        }
    }

    /// Prints one line, unless this second's lines are spent.
    fn line(&mut self, text: &[u8]) {
        let now = Instant::now();
        if self
            .second
            .is_none_or(|start| now.duration_since(start) >= Duration::from_secs(1))
        {
            if self.dropped > 0 {
                eprintln!("torque_lua.wasm: ({} lines dropped)", self.dropped);
            }
            self.second = Some(now);
            self.lines = 0;
            self.dropped = 0;
        }
        if self.lines >= LINES_PER_SECOND {
            self.dropped += 1;
            return;
        }
        self.lines += 1;
        self.printed += 1;
        eprintln!("torque_lua.wasm: {}", String::from_utf8_lossy(text));
    }
}

/// What a request comes to: a status, or a text that becomes the pending answer.
enum Answer {
    Status(TorqueStatus),
    Text(Vec<u8>),
    /// A limit stopped it: kept for the call's end.
    Stopped(VmError),
}

impl Answer {
    fn of(result: Result<Vec<u8>, HostError>) -> Answer {
        match result {
            Ok(text) => Answer::Text(text),
            Err(HostError::Refused) => Answer::Status(TORQUE_ERR_FAILED),
            Err(HostError::Stopped(error)) => Answer::Stopped(error),
        }
    }

    fn ok_or(ok: bool, otherwise: TorqueStatus) -> Answer {
        Answer::Status(if ok { TORQUE_OK } else { otherwise })
    }
}

/// Puts `active` back as it was when it is dropped.
struct Restore<'a> {
    active: &'a Cell<Option<NonNull<Caller<'static, Data>>>>,
    before: Option<NonNull<Caller<'static, Data>>>,
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        self.active.set(self.before);
    }
}

/// Serves a request of the module's for the call `call`: `f` runs with that call's host, if it is the innermost running
/// call and no other request of it is being served, and its answer is handed to the module (a text as the pending answer,
/// a limit kept for the call's end). A trap when the module broke while `f` ran (a nested call trapped), so that this
/// call stops too.
fn serve(
    caller: &mut Caller<'_, Data>,
    call: i32,
    f: impl FnOnce(&mut dyn LuaHost) -> Answer,
) -> wasmtime::Result<TorqueStatus> {
    caller.data().counters.add_request();
    let Some(running) = caller.data_mut().running.last_mut() else {
        return Ok(TORQUE_ERR_HANDLE);
    };
    if running.handle != call as u32 {
        return Ok(TORQUE_ERR_HANDLE);
    }
    // Taken out for the time it is used: a request made meanwhile with this handle finds no host.
    let Some(host) = running.host.take() else {
        return Ok(TORQUE_ERR_HANDLE);
    };
    let active = Rc::clone(&caller.data().active);
    let answer = {
        let restore = Restore {
            active: &active,
            before: active.get(),
        };
        restore.active.set(Some(
            NonNull::from(&mut *caller).cast::<Caller<'static, Data>>(),
        ));
        // SAFETY: `host` points at the `&mut dyn LuaHost` of the innermost running call, which its entry (`call`) holds,
        // unused, for the time the call runs; the call runs (its entry popped only when it returns), and the pointer was
        // taken out of the entry, so no other request (an outer call's, or this call's own made while `f` runs) can
        // reach it until it is put back: this is the one live use of it. After a panic the entry is never served again
        // (the module is broken, and its running calls are cleared on the next outermost call).
        let host = unsafe { &mut **host.as_ptr() };
        f(host)
    };
    let data = caller.data_mut();
    if let Some(running) = data.running.iter_mut().rfind(|r| r.handle == call as u32) {
        running.host = Some(host);
        if let Answer::Stopped(error) = &answer {
            running.stopped = Some(error.clone());
        }
    }
    if data.broken.get() {
        return Err(format_err!("a call the request made stopped the module"));
    }
    Ok(match answer {
        Answer::Status(status) => status,
        Answer::Text(text) => {
            data.pending = text;
            TORQUE_OK
        }
        Answer::Stopped(_) => TORQUE_ERR_LIMIT,
    })
}

/// UTF-8 text, or `None`.
fn utf8(bytes: Vec<u8>) -> Option<String> {
    String::from_utf8(bytes).ok()
}

/// The module's `torque` imports: each the C ABI's function of the same name, answered by the running call's host.
fn torque(linker: &mut Linker<Data>) -> wasmtime::Result<()> {
    const T: &str = "torque";
    let invalid = || TORQUE_ERR_INVALID as i32;
    linker.func_wrap(
        T,
        "log",
        move |mut c: Caller<'_, Data>, call: i32, level: i32, ptr: i32, len: i32| {
            let text = String::from_utf8_lossy(&read(&c, ptr, len)?).into_owned();
            let level = match level as u32 {
                TORQUE_LOG_WARN => Level::Warning,
                TORQUE_LOG_ERROR => Level::Error,
                _ => Level::Normal,
            };
            if call == 0 {
                let text: Vec<u8> = text.bytes().take(MAX_LINE).collect();
                c.data_mut().printer.line(&text);
                return Ok(TORQUE_OK as i32);
            }
            Ok(serve(&mut c, call, |host| {
                host.print(level, &text);
                Answer::Status(TORQUE_OK)
            })? as i32)
        },
    )?;
    linker.func_wrap(
        T,
        "get_global",
        move |mut c: Caller<'_, Data>, call: i32, ptr: i32, len: i32| {
            let Some(name) = utf8(read(&c, ptr, len)?) else {
                return Ok(invalid());
            };
            Ok(serve(&mut c, call, |host| Answer::Text(host.global(&name)))? as i32)
        },
    )?;
    linker.func_wrap(
        T,
        "set_global",
        move |mut c: Caller<'_, Data>,
              call: i32,
              name: i32,
              name_len: i32,
              value: i32,
              value_len: i32| {
            let (Some(name), value) =
                (utf8(read(&c, name, name_len)?), read(&c, value, value_len)?)
            else {
                return Ok(invalid());
            };
            Ok(serve(&mut c, call, |host| {
                Answer::ok_or(host.set_global(&name, &value), TORQUE_ERR_INVALID)
            })? as i32)
        },
    )?;
    linker.func_wrap(
        T,
        "call_function",
        move |mut c: Caller<'_, Data>,
              call: i32,
              ns: i32,
              ns_len: i32,
              name: i32,
              name_len: i32,
              args: i32,
              count: i32| {
            let namespace = utf8(read(&c, ns, ns_len)?);
            let name = utf8(read(&c, name, name_len)?);
            let Some(args) = read_texts(&c, args, count)? else {
                return Ok(TORQUE_ERR_RANGE as i32);
            };
            let (Some(namespace), Some(name)) = (namespace, name) else {
                return Ok(invalid());
            };
            Ok(serve(&mut c, call, |host| {
                Answer::of(host.call_function(&namespace, &name, &args))
            })? as i32)
        },
    )?;
    linker.func_wrap(
        T,
        "call_method",
        move |mut c: Caller<'_, Data>,
              call: i32,
              object: i32,
              name: i32,
              name_len: i32,
              args: i32,
              count: i32| {
            let name = utf8(read(&c, name, name_len)?);
            let Some(args) = read_texts(&c, args, count)? else {
                return Ok(TORQUE_ERR_RANGE as i32);
            };
            let Some(name) = name else {
                return Ok(invalid());
            };
            Ok(serve(&mut c, call, |host| {
                Answer::of(host.call_method(object as u32, &name, &args))
            })? as i32)
        },
    )?;
    linker.func_wrap(
        T,
        "has_method",
        |mut c: Caller<'_, Data>, call: i32, object: i32, name: i32, len: i32| {
            let Some(name) = utf8(read(&c, name, len)?) else {
                return Ok(0);
            };
            let status = serve(&mut c, call, |host| {
                Answer::ok_or(host.has_method(object as u32, &name), TORQUE_ERR_FAILED)
            })?;
            Ok(i32::from(status == TORQUE_OK))
        },
    )?;
    linker.func_wrap(
        T,
        "find_object",
        |mut c: Caller<'_, Data>, call: i32, name: i32, len: i32, out: i32| {
            let name = read(&c, name, len)?;
            let mut found = None;
            let status = serve(&mut c, call, |host| {
                found = host.find_object(&name);
                Answer::ok_or(found.is_some(), TORQUE_ERR_FAILED)
            })?;
            if let Some(id) = found {
                write_u32(&mut c, out, id)?;
            }
            Ok(status as i32)
        },
    )?;
    linker.func_wrap(
        T,
        "get_field",
        |mut c: Caller<'_, Data>,
         call: i32,
         object: i32,
         name: i32,
         name_len: i32,
         index: i32,
         index_len: i32| {
            let name = read(&c, name, name_len)?;
            let index = read(&c, index, index_len)?;
            Ok(serve(&mut c, call, |host| {
                match host.field(object as u32, &name, &index) {
                    Some(value) => Answer::Text(value),
                    None => Answer::Status(TORQUE_ERR_FAILED),
                }
            })? as i32)
        },
    )?;
    linker.func_wrap(
        T,
        "set_field",
        |mut c: Caller<'_, Data>,
         call: i32,
         object: i32,
         name: i32,
         name_len: i32,
         index: i32,
         index_len: i32,
         value: i32,
         value_len: i32| {
            let name = read(&c, name, name_len)?;
            let index = read(&c, index, index_len)?;
            let value = read(&c, value, value_len)?;
            Ok(serve(&mut c, call, |host| {
                Answer::ok_or(
                    host.set_field(object as u32, &name, &index, &value),
                    TORQUE_ERR_FAILED,
                )
            })? as i32)
        },
    )?;
    linker.func_wrap(
        T,
        "define_function",
        move |mut c: Caller<'_, Data>, call: i32, decl: i32| {
            // A `TorqueDefineDecl`: four texts (namespace, name, package, usage), then the id (a u64 at 32).
            let bytes = read(&c, decl, 40)?;
            let mut texts = Vec::with_capacity(4);
            for at in (0..32).step_by(8) {
                let ptr =
                    i32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
                let len = i32::from_le_bytes([
                    bytes[at + 4],
                    bytes[at + 5],
                    bytes[at + 6],
                    bytes[at + 7],
                ]);
                texts.push(utf8(read(&c, ptr, len)?));
            }
            let mut id = [0u8; 8];
            id.copy_from_slice(&bytes[32..40]);
            let id = u64::from_le_bytes(id);
            let [Some(namespace), Some(name), Some(package), Some(usage)] =
                <[Option<String>; 4]>::try_from(texts).unwrap_or_default()
            else {
                return Ok(invalid());
            };
            Ok(serve(&mut c, call, |host| {
                Answer::ok_or(
                    host.define_function(&namespace, &name, &package, &usage, id),
                    TORQUE_ERR_FAILED,
                )
            })? as i32)
        },
    )?;
    linker.func_wrap(
        T,
        "charge",
        |mut c: Caller<'_, Data>, call: i32, steps: i64| {
            let steps = steps as u64;
            let counters = Rc::clone(&c.data().counters);
            counters
                .steps
                .set(counters.steps.get().saturating_add(steps));
            serve(&mut c, call, |host| match host.charge(steps) {
                Ok(()) => Answer::Status(TORQUE_OK),
                Err(error) => Answer::Stopped(error),
            })
            .map(|status| status as i32)
        },
    )?;
    linker.func_wrap(
        T,
        "read_script",
        move |mut c: Caller<'_, Data>, call: i32, ptr: i32, len: i32| {
            let Some(path) = utf8(read(&c, ptr, len)?) else {
                return Ok(invalid());
            };
            Ok(serve(&mut c, call, |host| match host.read_script(&path) {
                Ok(Some(source)) => Answer::Text(source),
                Ok(None) => Answer::Status(TORQUE_ERR_FAILED),
                Err(error) => Answer::Stopped(error),
            })? as i32)
        },
    )?;
    linker.func_wrap(T, "result_len", |c: Caller<'_, Data>| {
        c.data().pending.len() as i32
    })?;
    linker.func_wrap(T, "result_copy", |mut c: Caller<'_, Data>, ptr: i32| {
        let pending = std::mem::take(&mut c.data_mut().pending);
        write(&mut c, ptr, &pending)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests;
