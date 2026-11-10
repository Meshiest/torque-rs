//! Hostile modules: imports called with ranges and lists a real module never names, and calls back into the module
//! while one of its requests is being served.

use std::panic::AssertUnwindSafe;

use super::*;
use crate::wasi::{ERRNO_BADF, ERRNO_INVAL, LINES_PER_SECOND, MAX_TEXT_BYTES};

const MEMORY: usize = 128 << 16;

fn engine() -> GuestEngine {
    GuestEngine::new(Duration::from_millis(5)).expect("an engine")
}

fn limits() -> GuestLimits {
    GuestLimits {
        memory: 64 << 20,
        deadline: Duration::from_secs(10),
    }
}

/// A store over an instance of `wat` linked to the imports.
fn hostile(wat: &str) -> (Store<Data>, wasmtime::Instance) {
    let engine = engine();
    let wasm = wat::parse_str(wat).expect("text");
    let module = Module::new(engine.engine(), wasm).expect("a module");
    let mut linker = Linker::new(engine.engine());
    wasi::link(&mut linker).expect("WASI imports");
    torque(&mut linker).expect("torque imports");
    let mut store = Store::new(engine.engine(), Data::new(limits()));
    store.limiter(|data| &mut data.limits);
    store.set_epoch_deadline(engine.ticks(limits().deadline));
    let instance = linker
        .instantiate(&mut store, &module)
        .expect("an instance");
    let memory = instance.get_memory(&mut store, "memory").expect("a memory");
    store.data_mut().wasi.memory = Some(memory);
    (store, instance)
}

fn go(
    store: &mut Store<Data>,
    instance: &wasmtime::Instance,
    args: (i32, i32, i32, i32),
) -> wasmtime::Result<i32> {
    instance
        .get_typed_func::<(i32, i32, i32, i32), i32>(&mut *store, "go")
        .expect("go")
        .call(store, args)
}

fn put_u32s(store: &mut Store<Data>, at: i32, words: &[u32]) {
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    write(store, at, &bytes).expect("in memory");
}

const RANDOM: &str = r#"(module
    (import "wasi_snapshot_preview1" "random_get" (func $random (param i32 i32) (result i32)))
    (memory (export "memory") 128)
    (func (export "go") (param i32 i32 i32 i32) (result i32) (call $random (local.get 0) (local.get 1))))"#;

const WRITE: &str = r#"(module
    (import "wasi_snapshot_preview1" "fd_write" (func $write (param i32 i32 i32 i32) (result i32)))
    (memory (export "memory") 128)
    (func (export "go") (param i32 i32 i32 i32) (result i32)
      (call $write (local.get 0) (local.get 1) (local.get 2) (local.get 3))))"#;

#[test]
fn random_bytes_outside_memory_trap_before_anything_is_made() {
    let (mut store, instance) = hostile(RANDOM);
    // 4 GiB, past the end, wrapping, and all of memory but one byte too many: each traps.
    for (ptr, len) in [
        (0, -1),
        (MEMORY as i32 - 8, 16),
        (-8, 8),
        (16, MEMORY as i32),
    ] {
        let result = go(&mut store, &instance, (ptr, len, 0, 0));
        assert!(result.is_err(), "random_get({ptr}, {len}) was let through");
    }
    let memory = store.data().wasi.memory.expect("memory");
    assert!(
        memory.data(&store).iter().all(|&b| b == 0),
        "nothing was written"
    );
}

#[test]
fn random_bytes_fill_only_their_range_and_are_capped() {
    let (mut store, instance) = hostile(RANDOM);
    assert_eq!(
        go(&mut store, &instance, (100, 4096, 0, 0)).expect("in memory"),
        0
    );
    let memory = store.data().wasi.memory.expect("memory");
    let bytes = memory.data(&store);
    assert!(bytes[..100].iter().all(|&b| b == 0));
    assert!(bytes[100 + 4096..].iter().all(|&b| b == 0));
    assert!(bytes[100..100 + 4096].iter().filter(|&&b| b != 0).count() > 3000);
    // In memory but past the per-call cap: refused, nothing written.
    let big = MAX_TEXT_BYTES as i32 + 1;
    assert_eq!(
        go(&mut store, &instance, (8192, big, 0, 0)).expect("refused"),
        ERRNO_INVAL
    );
    assert!(memory.data(&store)[8192..8192 + 16].iter().all(|&b| b == 0));
}

#[test]
fn fd_write_refuses_too_many_or_too_large_buffers() {
    let (mut store, instance) = hostile(WRITE);
    // More buffers than the cap: refused before any is read.
    assert_eq!(
        go(&mut store, &instance, (2, 0, 1025, 16)).expect("refused"),
        ERRNO_INVAL
    );
    // Two buffers of 3 MiB each (the same bytes twice): over the total, refused.
    put_u32s(&mut store, 0, &[1 << 20, 3 << 20, 1 << 20, 3 << 20]);
    assert_eq!(
        go(&mut store, &instance, (2, 0, 2, 16)).expect("refused"),
        ERRNO_INVAL
    );
    // 1024 buffers of the whole memory: their lengths summed checked, refused.
    let list: Vec<u32> = (0..1024).flat_map(|_| [0, MEMORY as u32]).collect();
    put_u32s(&mut store, 0, &list);
    assert_eq!(
        go(&mut store, &instance, (2, 0, 1024, 16)).expect("refused"),
        ERRNO_INVAL
    );
    let printer = &store.data().wasi.printer;
    assert!(
        printer.partial.is_empty() && printer.printed == 0,
        "nothing was copied"
    );
}

#[test]
fn fd_write_buffers_outside_memory_trap() {
    let (mut store, instance) = hostile(WRITE);
    put_u32s(&mut store, 0, &[64, u32::MAX]);
    assert!(go(&mut store, &instance, (2, 0, 1, 16)).is_err());
    let (mut store, instance) = hostile(WRITE);
    // The list itself past the end.
    assert!(go(&mut store, &instance, (2, MEMORY as i32 - 4, 1, 16)).is_err());
}

#[test]
fn fd_write_streams_and_its_lines_are_rate_limited() {
    let (mut store, instance) = hostile(WRITE);
    // 3 MiB without a newline: streamed in pieces, never held whole.
    write(&mut store, 1 << 16, &vec![b'x'; 3 << 20]).expect("in memory");
    put_u32s(&mut store, 0, &[1 << 16, 3 << 20]);
    assert_eq!(
        go(&mut store, &instance, (2, 0, 1, 16)).expect("written"),
        0
    );
    let written = read(&store, 16, 4).expect("in memory");
    assert_eq!(
        u32::from_le_bytes(written.try_into().expect("4 bytes")),
        3 << 20
    );
    assert!(store.data().wasi.printer.partial.len() <= MAX_LINE);
    // 768 pieces of a line went by; at most a second's worth were printed, the rest counted.
    let printer = &store.data().wasi.printer;
    assert!(printer.printed <= u64::from(LINES_PER_SECOND));
    assert!(printer.dropped > 0);
    // 10,000 short lines: the same.
    let (mut store, instance) = hostile(WRITE);
    write(&mut store, 1 << 16, &b"spam\n".repeat(10_000)).expect("in memory");
    put_u32s(&mut store, 0, &[1 << 16, 50_000]);
    assert_eq!(
        go(&mut store, &instance, (1, 0, 1, 16)).expect("written"),
        0
    );
    assert!(store.data().wasi.printer.printed <= u64::from(LINES_PER_SECOND));
    // Another descriptor's buffers are not read at all.
    assert_eq!(
        go(&mut store, &instance, (3, 0, 1, 16)).expect("refused"),
        ERRNO_BADF
    );
}

#[test]
fn argument_lists_are_capped_by_count_and_total() {
    let (mut store, _instance) = hostile(WRITE);
    assert!(read_texts(&store, 0, 1025).expect("in memory").is_none());
    put_u32s(&mut store, 0, &[1 << 20, 3 << 20, 1 << 20, 3 << 20]);
    assert!(read_texts(&store, 0, 2).expect("in memory").is_none());
    put_u32s(&mut store, 0, &[64, 5, 128, 0]);
    let texts = read_texts(&store, 0, 2)
        .expect("in memory")
        .expect("within the caps");
    assert_eq!(texts, vec![vec![0u8; 5], Vec::new()]);
    put_u32s(&mut store, 0, &[64, u32::MAX]);
    assert!(
        read_texts(&store, 0, 1).is_err(),
        "a range outside memory traps"
    );
}

const GET_GLOBAL: &str = r#"(module
    (import "torque" "get_global" (func $get_global (param i32 i32 i32) (result i32)))
    (import "torque" "set_field" (func $set_field (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
    (memory (export "memory") 128)
    (func (export "go") (param i32 i32 i32 i32) (result i32)
      (if (result i32) (local.get 3)
        (then (call $set_field (local.get 0) (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 0) (i32.const 0)
          (local.get 1) (local.get 2)))
        (else (call $get_global (local.get 0) (local.get 1) (local.get 2))))))"#;

#[test]
fn texts_past_the_abi_s_caps_are_refused_before_they_are_copied() {
    let (mut store, instance) = hostile(GET_GLOBAL);
    let counters = Rc::clone(&store.data().counters);
    // A name longer than the module's whole memory: refused by its length, not trapped reading it.
    let status = go(&mut store, &instance, (1, 0, 1 << 24, 0)).expect("no trap");
    assert_eq!(status, TORQUE_ERR_RANGE as i32);
    // One past the name cap, and a value one past the value cap: refused too.
    let status = go(&mut store, &instance, (1, 0, TORQUE_MAX_NAME as i32 + 1, 0)).expect("no trap");
    assert_eq!(status, TORQUE_ERR_RANGE as i32);
    let status = go(
        &mut store,
        &instance,
        (1, 0, TORQUE_MAX_VALUE as i32 + 1, 1),
    )
    .expect("no trap");
    assert_eq!(status, TORQUE_ERR_RANGE as i32);
    assert_eq!(counters.requests(), 0, "no request reached the host");
    // Within the caps, the request goes on to be served (here: no call is running, so the handle is refused).
    let status = go(&mut store, &instance, (1, 0, 8, 0)).expect("no trap");
    assert_eq!(status, TORQUE_ERR_HANDLE as i32);
    assert_eq!(counters.requests(), 1);
}

/// A Lua module that, asked to run a file, keeps the call's handle and calls a console function (the engine's host
/// decides what that does), then marks that it went on (the word at 4). Its `vm_dropped` makes a request with the kept
/// handle and stores the status at 0; its `call_defined` traps.
const REENTRANT: &str = r#"(module
    (import "torque" "call_function" (func $call_function (param i32 i32 i32 i32 i32 i32 i32) (result i32)))
    (import "torque" "log" (func $log (param i32 i32 i32 i32) (result i32)))
    (memory (export "memory") 2)
    (global $handle (mut i32) (i32.const 0))
    (global $heap (mut i32) (i32.const 1024))
    (func (export "_initialize"))
    (func (export "torque_lua_start") (result i32) (i32.const 0))
    (func (export "torque_lua_alloc") (param $n i32) (result i32)
      (local $at i32)
      (local.set $at (global.get $heap))
      (global.set $heap (i32.add (local.get $at) (i32.and (i32.add (local.get $n) (i32.const 15)) (i32.const -16))))
      (local.get $at))
    (func (export "torque_lua_free") (param i32 i32))
    (func (export "torque_lua_run_file") (param i32 i64 i32 i32 i32 i32 i32 i32 i32) (result i32)
      (global.set $handle (local.get 0))
      (i32.store (i32.const 0) (i32.const -1))
      (drop (call $call_function (local.get 0) (i32.const 0) (i32.const 0) (local.get 3) (local.get 4) (i32.const 0) (i32.const 0)))
      (i32.store (i32.const 4) (i32.const 1))
      (i32.const 0))
    (func (export "torque_lua_call_defined") (param i32 i64 i32 i64 i32 i32) (result i32) unreachable)
    (func (export "torque_lua_result") (result i32) (i32.const 0))
    (func (export "torque_lua_result_len") (result i32) (i32.const 0))
    (func (export "torque_lua_vm_dropped") (param i64)
      (i32.store (i32.const 0) (call $log (global.get $handle) (i32.const 0) (i32.const 16) (i32.const 4)))))"#;

/// What the engine's host does when the module calls a console function.
#[derive(Clone, Copy)]
enum Then {
    /// Drops a realm: the module's `vm_dropped` runs inside the request.
    DropRealm,
    /// Calls a function the module defined (which traps) inside the request.
    CallBack,
    /// Panics.
    Panic,
}

struct Host {
    module: Rc<WasmtimeLuaModule>,
    then: Then,
    prints: Rc<Cell<u32>>,
}

impl LuaHost for Host {
    fn realm(&self) -> (u64, Side) {
        (1, Side::Client)
    }
    fn package(&self) -> &str {
        ""
    }
    fn call_function(&mut self, _: &str, _: &str, _: &[Vec<u8>]) -> Result<Vec<u8>, HostError> {
        match self.then {
            Then::DropRealm => self.module.realm_dropped(7),
            Then::CallBack => {
                let mut inner = Host {
                    module: Rc::clone(&self.module),
                    then: Then::DropRealm,
                    prints: Rc::clone(&self.prints),
                };
                let _ = self.module.call_defined(&mut inner, 1, &[]);
            }
            Then::Panic => panic!("a host callback panics"),
        }
        Ok(Vec::new())
    }
    fn call_method(&mut self, _: u32, _: &str, _: &[Vec<u8>]) -> Result<Vec<u8>, HostError> {
        Err(HostError::Refused)
    }
    fn has_method(&self, _: u32, _: &str) -> bool {
        false
    }
    fn find_object(&self, _: &[u8]) -> Option<u32> {
        None
    }
    fn field(&self, _: u32, _: &[u8], _: &[u8]) -> Option<Vec<u8>> {
        None
    }
    fn set_field(&mut self, _: u32, _: &[u8], _: &[u8], _: &[u8]) -> bool {
        false
    }
    fn global(&self, _: &str) -> Vec<u8> {
        Vec::new()
    }
    fn set_global(&mut self, _: &str, _: &[u8]) -> bool {
        false
    }
    fn define_function(&mut self, _: &str, _: &str, _: &str, _: &str, _: u64) -> bool {
        false
    }
    fn charge(&mut self, _: u64) -> Result<(), VmError> {
        Ok(())
    }
    fn read_script(&mut self, _: &str) -> Result<Option<Vec<u8>>, VmError> {
        Ok(None)
    }
    fn print(&mut self, _: Level, _: &str) {
        self.prints.set(self.prints.get() + 1);
    }
}

fn reentrant(then: Then) -> (Rc<WasmtimeLuaModule>, Host) {
    let wasm = wat::parse_str(REENTRANT).expect("text");
    let guest = LuaGuest::new(&engine(), &wasm).expect("linked");
    let module = Rc::new(guest.instantiate(limits()).expect("started"));
    let host = Host {
        module: Rc::clone(&module),
        then,
        prints: Rc::new(Cell::new(0)),
    };
    (module, host)
}

/// The word at `at` in the module's memory.
fn word(module: &WasmtimeLuaModule, at: usize) -> i32 {
    let store = module.store.borrow();
    let memory = store.data().wasi.memory.expect("memory");
    let bytes = &memory.data(&*store)[at..at + 4];
    i32::from_le_bytes(bytes.try_into().expect("4 bytes"))
}

#[test]
fn a_realm_dropped_inside_a_request_serves_no_request_with_the_outer_handle() {
    let (module, mut host) = reentrant(Then::DropRealm);
    assert_eq!(module.run_file(&mut host, "f.lua", b""), Ok(()));
    // The request `vm_dropped` made with the served call's handle was refused, and never reached the host.
    assert_eq!(word(&module, 0), TORQUE_ERR_HANDLE as i32);
    assert_eq!(host.prints.get(), 0);
    assert_eq!(word(&module, 4), 1, "the outer call went on");
    assert!(!module.is_broken());
    // The call's entries are gone, the host put back for the next one.
    assert!(module.store.borrow().data().running.is_empty());
    assert_eq!(module.run_file(&mut host, "f.lua", b""), Ok(()));
}

#[test]
fn a_nested_call_that_traps_stops_the_outer_call_too() {
    let (module, mut host) = reentrant(Then::CallBack);
    assert_eq!(module.run_file(&mut host, "f.lua", b""), Ok(()));
    assert!(module.is_broken());
    assert_eq!(
        word(&module, 4),
        0,
        "the outer call did not go on after its request"
    );
    // Both calls said why they stopped.
    assert_eq!(host.prints.get(), 2);
}

#[test]
fn a_panic_in_a_host_callback_breaks_the_module() {
    let (module, mut host) = reentrant(Then::Panic);
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        module.run_file(&mut host, "f.lua", b"")
    }));
    assert!(outcome.is_err(), "the panic goes on to the caller");
    assert!(module.is_broken());
    // Nothing runs again: the next call is refused, the half-used module untouched.
    host.then = Then::DropRealm;
    assert_eq!(module.run_file(&mut host, "f.lua", b""), Ok(()));
    assert_eq!(word(&module, 4), 0);
    assert_eq!(host.prints.get(), 1);
}

#[test]
fn the_store_holds_one_memory_and_one_table() {
    let engine = engine();
    let wasm = wat::parse_str(r#"(module (memory (export "memory") 1) (table 70000 funcref))"#)
        .expect("text");
    let module = Module::new(engine.engine(), wasm).expect("a module");
    let mut store = Store::new(engine.engine(), Data::new(limits()));
    store.limiter(|data| &mut data.limits);
    assert!(
        wasmtime::Instance::new(&mut store, &module, &[]).is_err(),
        "a table past the element cap"
    );
}
