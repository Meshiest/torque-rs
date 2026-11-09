//! The realm's guest (`torque_lua_client.wasm`) under Wasmtime: one store and instance per realm, called on the engine's
//! thread. Its only imports are WASI's ([`crate::wasi`]): a module importing anything else does not link, and no import
//! can reach the instance, so no export is ever called from inside an import. Every entry runs under the epoch deadline
//! and within the store's limits; after any trap the store is dropped and nothing of it runs again.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use torque_client_script_format::{FORMAT_VERSION, MAX_FLUSH_BYTES};
use wasmtime::{
    InstancePre, Linker, Module, ResourceLimiter, Store, StoreLimits, TypedFunc, UpdateDeadline,
    format_err,
};
use web_time::Instant;

use crate::GuestEngine;
use crate::guest::store_limits;
use crate::runner::{Entry, InstanceLimits, Outcome, Reply, Runner, Trap, gone};
use crate::wasi::{self, Wasi, WasiData, read, write};

/// The store's limits, remembering whether one refused the guest memory (its trap is then a memory trap).
struct Limiter {
    limits: StoreLimits,
    refused: bool,
}

impl ResourceLimiter for Limiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if self.limits.memory_growing(current, desired, maximum)? {
            return Ok(true);
        }
        self.refused = true;
        // A growth past the cap traps: the guest's allocators have no use for a refusal there.
        Err(format_err!("the guest grew its memory past its cap"))
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        self.limits.table_growing(current, desired, maximum)
    }

    fn instances(&self) -> usize {
        self.limits.instances()
    }

    fn tables(&self) -> usize {
        self.limits.tables()
    }

    fn memories(&self) -> usize {
        self.limits.memories()
    }
}

/// A realm instance's store data.
struct RealmData {
    limiter: Limiter,
    wasi: Wasi,
    /// Whether the running entry passed the long-entry time (the epoch's first deadline).
    long: bool,
}

impl WasiData for RealmData {
    fn wasi(&self) -> &Wasi {
        &self.wasi
    }

    fn wasi_mut(&mut self) -> &mut Wasi {
        &mut self.wasi
    }
}

/// The guest compiled and linked, ready for any number of realms.
#[derive(Clone)]
pub struct RealmGuest {
    engine: GuestEngine,
    pre: InstancePre<RealmData>,
    live: Arc<AtomicUsize>,
}

impl RealmGuest {
    /// Compiles a guest's bytes and links its imports.
    ///
    /// # Errors
    ///
    /// When the bytes are not a module, or it imports anything but the WASI functions this crate answers.
    pub fn new(engine: &GuestEngine, wasm: &[u8]) -> wasmtime::Result<RealmGuest> {
        let module = Module::new(engine.engine(), wasm)?;
        let mut linker = Linker::new(engine.engine());
        wasi::link(&mut linker)?;
        let pre = linker.instantiate_pre(&module)?;
        Ok(RealmGuest {
            engine: engine.clone(),
            pre,
            live: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// The instances of this guest that exist.
    pub fn live(&self) -> usize {
        self.live.load(Ordering::Relaxed)
    }

    /// A fresh instance within `limits`, started (its constructors run) and checked to speak this format.
    ///
    /// # Errors
    ///
    /// The trap that stopped it starting, or [`Trap::Fault`] for a module that is not a realm guest of this format.
    pub fn open(&self, limits: InstanceLimits) -> Result<NativeRealm, Trap> {
        let data = RealmData {
            limiter: Limiter {
                limits: store_limits(limits.memory),
                refused: false,
            },
            wasi: Wasi::new("torque_lua_client.wasm"),
            long: false,
        };
        let mut store = Store::new(self.engine.engine(), data);
        store.limiter(|data| &mut data.limiter);
        let ticks = EntryTicks::new(&self.engine, limits);
        // The first deadline marks the entry long while it runs; the second stops it.
        let rest = ticks.deadline - ticks.long;
        store.epoch_deadline_callback(move |mut state| {
            let data = state.data_mut();
            if data.long || rest == 0 {
                data.long = true;
                return Ok(UpdateDeadline::Interrupt);
            }
            data.long = true;
            Ok(UpdateDeadline::Continue(rest))
        });
        store.set_epoch_deadline(ticks.long);
        let started = (|| {
            let instance = self.pre.instantiate(&mut store)?;
            let memory = instance
                .get_memory(&mut store, "memory")
                .ok_or_else(|| format_err!("the guest exports no memory"))?;
            store.data_mut().wasi.memory = Some(memory);
            let exports = Exports::of(&instance, &mut store)?;
            if let Ok(init) = instance.get_typed_func::<(), ()>(&mut store, "_initialize") {
                init.call(&mut store, ())?;
            }
            let format = instance
                .get_typed_func::<(), i32>(&mut store, "torque_realm_format")?
                .call(&mut store, ())?;
            if format != FORMAT_VERSION as i32 {
                return Err(format_err!("the guest speaks format {format}"));
            }
            Ok(exports)
        })();
        let exports = started.map_err(|error| trap_of(&store, &error))?;
        self.live.fetch_add(1, Ordering::Relaxed);
        Ok(NativeRealm {
            live: Some(Live {
                store,
                exports,
                ticks,
                count: Arc::clone(&self.live),
            }),
            outcome: None,
        })
    }
}

/// The guest's exports an entry uses.
struct Exports {
    alloc: TypedFunc<i32, i32>,
    free: TypedFunc<(i32, i32), ()>,
    new: TypedFunc<(i32, i32), i32>,
    add_script: TypedFunc<(i32, i32, i32, i32, i32, i32), i32>,
    run: TypedFunc<(i64, i32, i32, i32, i32), i32>,
    frame: TypedFunc<(i64, i32, i32), i32>,
    ops: TypedFunc<(), i32>,
    ops_len: TypedFunc<(), i32>,
    pending: TypedFunc<(), i32>,
}

impl Exports {
    fn of(
        instance: &wasmtime::Instance,
        store: &mut Store<RealmData>,
    ) -> wasmtime::Result<Exports> {
        Ok(Exports {
            alloc: instance.get_typed_func(&mut *store, "torque_realm_alloc")?,
            free: instance.get_typed_func(&mut *store, "torque_realm_free")?,
            new: instance.get_typed_func(&mut *store, "torque_realm_new")?,
            add_script: instance.get_typed_func(&mut *store, "torque_realm_add_script")?,
            run: instance.get_typed_func(&mut *store, "torque_realm_run")?,
            frame: instance.get_typed_func(&mut *store, "torque_realm_frame")?,
            ops: instance.get_typed_func(&mut *store, "torque_realm_ops")?,
            ops_len: instance.get_typed_func(&mut *store, "torque_realm_ops_len")?,
            pending: instance.get_typed_func(&mut *store, "torque_realm_pending")?,
        })
    }
}

/// The trap `error` of a call into `store`'s instance is.
fn trap_of(store: &Store<RealmData>, error: &wasmtime::Error) -> Trap {
    if store.data().limiter.refused {
        Trap::Memory
    } else if crate::is_deadline(error) {
        Trap::Deadline
    } else {
        Trap::Fault(format!("{error:#}"))
    }
}

/// The epoch ticks of an entry: to the long-entry time, and to its deadline.
#[derive(Clone, Copy)]
struct EntryTicks {
    long: u64,
    deadline: u64,
}

impl EntryTicks {
    fn new(engine: &GuestEngine, limits: InstanceLimits) -> EntryTicks {
        let deadline = engine.ticks(limits.deadline);
        EntryTicks {
            long: engine.ticks(limits.long_entry).min(deadline),
            deadline,
        }
    }
}

/// A live instance.
struct Live {
    store: Store<RealmData>,
    exports: Exports,
    ticks: EntryTicks,
    count: Arc<AtomicUsize>,
}

impl Drop for Live {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Live {
    /// Copies `bytes` into the guest: their pointer and length (`None` when it has no room).
    fn put(&mut self, bytes: &[u8]) -> wasmtime::Result<Option<(i32, i32)>> {
        let len = i32::try_from(bytes.len()).map_err(|_| format_err!("an argument too long"))?;
        let ptr = self.exports.alloc.call(&mut self.store, len)?;
        if ptr == 0 {
            return Ok(None);
        }
        write(&mut self.store, ptr, bytes)?;
        Ok(Some((ptr, len)))
    }

    /// Runs `entry`: `Ok(None)` when the guest had no room for its arguments.
    fn run(&mut self, entry: &Entry) -> wasmtime::Result<Option<Reply>> {
        self.store.data_mut().long = false;
        self.store.set_epoch_deadline(self.ticks.long);
        let started = Instant::now();
        let config;
        let texts: Vec<&[u8]> = match entry {
            Entry::New(c) => {
                config = c.to_bytes();
                vec![&config]
            }
            Entry::AddScript {
                mod_id,
                path,
                source,
            } => vec![mod_id.as_bytes(), path.as_bytes(), &**source],
            Entry::Run { mod_id, path, .. } => vec![mod_id.as_bytes(), path.as_bytes()],
            Entry::Frame { events, .. } => vec![events],
        };
        let mut placed = Vec::with_capacity(texts.len());
        for text in texts {
            match self.put(text)? {
                Some(place) => placed.push(place),
                None => return Ok(None),
            }
        }
        let ex = &self.exports;
        let store = &mut self.store;
        let status = match (entry, placed.as_slice()) {
            (Entry::New(_), &[(c, cl)]) => ex.new.call(&mut *store, (c, cl))?,
            (Entry::AddScript { .. }, &[(m, ml), (p, pl), (s, sl)]) => {
                ex.add_script.call(&mut *store, (m, ml, p, pl, s, sl))?
            }
            (Entry::Run { now_us, .. }, &[(m, ml), (p, pl)]) => {
                ex.run.call(&mut *store, (micros(*now_us), m, ml, p, pl))?
            }
            (Entry::Frame { now_us, .. }, &[(e, el)]) => {
                ex.frame.call(&mut *store, (micros(*now_us), e, el))?
            }
            _ => return Err(format_err!("an entry's arguments did not match it")),
        };
        for (ptr, len) in placed {
            ex.free.call(&mut *store, (ptr, len))?;
        }
        let mut reply = Reply {
            status,
            elapsed: started.elapsed(),
            long: store.data().long,
            ..Reply::default()
        };
        if entry.has_flush() {
            let len = ex.ops_len.call(&mut *store, ())?;
            if len as u32 as usize > MAX_FLUSH_BYTES {
                return Err(format_err!("a flush longer than the format allows"));
            }
            let ptr = ex.ops.call(&mut *store, ())?;
            reply.ops = read(&*store, ptr, len)?;
            reply.pending = u32::try_from(ex.pending.call(&mut *store, ())?).unwrap_or(0);
        }
        Ok(Some(reply))
    }
}

/// The time an entry carries.
fn micros(now_us: u64) -> i64 {
    i64::try_from(now_us).unwrap_or(i64::MAX)
}

/// One realm's instance, called on the engine's thread: each entry's outcome is ready when [`Runner::start`] returns.
pub struct NativeRealm {
    live: Option<Live>,
    outcome: Option<Outcome>,
}

impl NativeRealm {
    /// Whether the instance still exists (it is thrown away after any trap).
    pub fn is_live(&self) -> bool {
        self.live.is_some()
    }
}

impl Runner for NativeRealm {
    fn start(&mut self, entry: Entry) {
        let Some(live) = self.live.as_mut() else {
            self.outcome = Some(Err(gone()));
            return;
        };
        let outcome = match live.run(&entry) {
            Ok(Some(reply)) => Ok(reply),
            Ok(None) => Err(Trap::Memory),
            Err(error) => Err(trap_of(&live.store, &error)),
        };
        if outcome.is_err() {
            self.live = None;
        }
        self.outcome = Some(outcome);
    }

    fn poll(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use torque_client_script_format::abi::Config;

    use super::*;

    fn engine() -> GuestEngine {
        GuestEngine::new(Duration::from_millis(5)).expect("an engine")
    }

    fn limits() -> InstanceLimits {
        InstanceLimits {
            memory: 4 << 20,
            deadline: Duration::from_millis(200),
            long_entry: Duration::from_millis(100),
        }
    }

    /// A guest with every export: `frame` runs `frame_body`, `torque_realm_format` answers `format`, the flush is
    /// `ops_len` bytes long.
    fn guest(frame_body: &str, format: i32, ops_len: u32) -> Vec<u8> {
        wat::parse_str(format!(
            r#"(module
  (memory (export "memory") 1)
  (func (export "torque_realm_format") (result i32) i32.const {format})
  (func (export "torque_realm_tiers") (result i32) i32.const 7)
  (func (export "torque_realm_alloc") (param i32) (result i32) i32.const 4096)
  (func (export "torque_realm_free") (param i32 i32))
  (func (export "torque_realm_new") (param i32 i32) (result i32) i32.const 0)
  (func (export "torque_realm_add_script") (param i32 i32 i32 i32 i32 i32) (result i32) i32.const 0)
  (func (export "torque_realm_run") (param i64 i32 i32 i32 i32) (result i32) i32.const 0)
  (func (export "torque_realm_frame") (param i64 i32 i32) (result i32) {frame_body} i32.const 0)
  (func (export "torque_realm_ops") (result i32) i32.const 0)
  (func (export "torque_realm_ops_len") (result i32) i32.const {ops_len})
  (func (export "torque_realm_pending") (result i32) i32.const 0))"#
        ))
        .expect("text")
    }

    fn frame() -> Entry {
        Entry::Frame {
            now_us: 0,
            events: Vec::new(),
        }
    }

    #[test]
    fn a_guest_importing_anything_but_wasi_does_not_link() {
        let wasm = wat::parse_str(
            r#"(module (import "torque" "call" (func)) (memory (export "memory") 1))"#,
        )
        .expect("text");
        assert!(RealmGuest::new(&engine(), &wasm).is_err());
    }

    #[test]
    fn a_guest_of_another_format_does_not_open() {
        let guest = RealmGuest::new(&engine(), &guest("", 1, 0)).expect("links");
        assert!(matches!(guest.open(limits()), Err(Trap::Fault(_))));
        assert_eq!(guest.live(), 0);
    }

    #[test]
    fn after_a_trap_the_instance_is_thrown_away_and_runs_nothing() {
        let guest = RealmGuest::new(&engine(), &guest("unreachable", 2, 0)).expect("links");
        let mut realm = guest.open(limits()).expect("opens");
        assert_eq!(guest.live(), 1);
        realm.start(Entry::New(Config::default()));
        assert!(realm.poll().expect("an outcome").is_ok());
        realm.start(frame());
        assert!(matches!(realm.poll(), Some(Err(Trap::Fault(_)))));
        assert!(!realm.is_live());
        assert_eq!(guest.live(), 0);
        realm.start(frame());
        assert_eq!(realm.poll(), Some(Err(gone())));
    }

    #[test]
    fn a_flush_past_the_formats_cap_is_a_fault_before_it_is_read() {
        let guest =
            RealmGuest::new(&engine(), &guest("", 2, (MAX_FLUSH_BYTES + 1) as u32)).expect("links");
        let mut realm = guest.open(limits()).expect("opens");
        realm.start(frame());
        assert!(matches!(realm.poll(), Some(Err(Trap::Fault(_)))));
    }

    #[test]
    fn growing_past_the_cap_is_a_memory_trap() {
        let grow = "(drop (memory.grow (i32.const 128)))";
        let guest = RealmGuest::new(&engine(), &guest(grow, 2, 0)).expect("links");
        let mut realm = guest.open(limits()).expect("opens");
        realm.start(frame());
        assert_eq!(realm.poll(), Some(Err(Trap::Memory)));
    }
}
