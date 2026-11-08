//! Every crossing of the C ABI on the engine's side, and the only `unsafe` code of the crate: the function table handed
//! to an extension, the calls into it, and the loading of its library.
//!
//! **Trust.** What an extension hands over is checked before it is used: pointers for null and alignment, lengths
//! against the ABI's limits, names and texts for UTF-8 and their character rules, kinds, stages and flags against the
//! known ones. Handles are numbers, never pointers: each names one init, call or draw, is never reused, and is live only
//! while that call runs on this thread (a frame in a thread-local stack), so a stale or forged handle, or one from
//! another thread, is a [`TORQUE_ERR_HANDLE`]. Every function of the table catches a panic of its own and returns a
//! status, so nothing unwinds into the extension. What cannot be checked is that a pointer points at as many bytes as
//! its length says: an extension that lies there is as broken as one that writes over the game's memory, which a
//! native extension can always do.
//!
//! **Re-entrancy.** The table offers nothing that runs scripts, so a native of an extension cannot call another of its
//! natives back; the engine still makes one call into an extension at a time (`ExtensionState`'s lock).

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};

use torque_engine::Side;
use torque_plugin_abi::*;
use torque_ts_vm::convert::{ArgKind, FromScript};
use torque_ts_vm::vm::{Native, Vm};

/// A pointer the extension handed over to be handed back to it, never read by the engine.
#[derive(Clone, Copy, Debug)]
pub(crate) struct UserPtr(*mut c_void);

// SAFETY: the engine only hands the pointer back to the extension that made it; the ABI's docs say an extension's natives
// and passes may be called from the engine's threads, one call at a time (the extension's lock), which is what moving it
// between threads amounts to.
unsafe impl Send for UserPtr {}
// SAFETY: as above: shared references only copy the pointer to hand it back.
unsafe impl Sync for UserPtr {}

/// A native function of an extension.
type NativeFn = unsafe extern "C" fn(*mut c_void, TorqueCall) -> TorqueStatus;
/// `TorquePassDecl::create`.
type CreateFn = unsafe extern "C" fn(*mut c_void, TorqueDraw, *mut *mut c_void) -> TorqueStatus;
/// `TorquePassDecl::draw`.
type DrawFn = unsafe extern "C" fn(*mut c_void, *const TorquePassFrame, TorqueDraw) -> TorqueStatus;
/// `TorquePassDecl::drop`.
type DropFn = unsafe extern "C" fn(*mut c_void);

/// A console function or method an extension declared, checked and copied.
pub(crate) struct NativeSpec {
    pub(crate) side: Side,
    pub(crate) method: bool,
    pub(crate) namespace: String,
    pub(crate) name: String,
    pub(crate) args: Vec<(String, ArgKind)>,
    pub(crate) description: String,
    pub(crate) usage: Option<String>,
    pub(crate) docs: String,
    pub(crate) dev: bool,
    call: NativeFn,
    user: UserPtr,
}

/// A global an extension declared.
pub(crate) struct GlobalSpec {
    pub(crate) side: Side,
    pub(crate) name: String,
    pub(crate) default: String,
}

/// A render pass an extension declared.
#[cfg_attr(not(feature = "render"), allow(dead_code))]
pub(crate) struct PassSpec {
    /// `TORQUE_STAGE_`.
    pub(crate) stage: u32,
    create: CreateFn,
    draw: DrawFn,
    drop: Option<DropFn>,
    user: UserPtr,
}

/// What an extension's init registered, checked.
pub(crate) struct Registered {
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) net_version: Option<u32>,
    pub(crate) natives: Vec<NativeSpec>,
    pub(crate) globals: Vec<GlobalSpec>,
    pub(crate) passes: Vec<PassSpec>,
    pub(crate) log: Vec<String>,
}

/// The next handle: never 0, never reused.
fn next_handle() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The init running on this thread: what it registered so far.
struct InitFrame {
    handle: TorqueInit,
    natives: Vec<NativeSpec>,
    globals: Vec<GlobalSpec>,
    passes: Vec<PassSpec>,
    refused: Vec<String>,
    log: Vec<String>,
}

impl InitFrame {
    fn count(&self) -> usize {
        self.natives.len() + self.globals.len() + self.passes.len()
    }
}

/// A native call running on this thread.
struct CallFrame {
    handle: TorqueCall,
    /// The script engine of the call: exclusively borrowed by the host's native for as long as the frame is live.
    vm: *mut Vm,
    args: Vec<Vec<u8>>,
    this: Option<u32>,
    result: Option<Native>,
    error: Option<String>,
    /// The last `get_global`'s value, which its pointer points into.
    scratch: Vec<u8>,
}

/// One draw of a pass running on this thread.
struct DrawFrame {
    handle: TorqueDraw,
    batches: Vec<(u32, Vec<TorqueVertex>)>,
    vertices: usize,
    log: Vec<String>,
}

thread_local! {
    static INITS: RefCell<Vec<InitFrame>> = const { RefCell::new(Vec::new()) };
    static CALLS: RefCell<Vec<CallFrame>> = const { RefCell::new(Vec::new()) };
    static DRAWS: RefCell<Vec<DrawFrame>> = const { RefCell::new(Vec::new()) };
}

/// Runs `f` on the live init frame `handle`; [`TORQUE_ERR_HANDLE`] when there is none.
fn with_init(handle: TorqueInit, f: impl FnOnce(&mut InitFrame) -> TorqueStatus) -> TorqueStatus {
    INITS.with(|inits| {
        let mut inits = inits.borrow_mut();
        match inits.iter_mut().rev().find(|i| i.handle == handle) {
            Some(frame) => f(frame),
            None => TORQUE_ERR_HANDLE,
        }
    })
}

/// Runs `f` on the live call frame `handle`; [`TORQUE_ERR_HANDLE`] when there is none.
fn with_call(handle: TorqueCall, f: impl FnOnce(&mut CallFrame) -> TorqueStatus) -> TorqueStatus {
    CALLS.with(|calls| {
        let mut calls = calls.borrow_mut();
        match calls.iter_mut().rev().find(|c| c.handle == handle) {
            Some(frame) => f(frame),
            None => TORQUE_ERR_HANDLE,
        }
    })
}

/// Runs `f` on the live draw frame `handle`; [`TORQUE_ERR_HANDLE`] when there is none.
fn with_draw(handle: TorqueDraw, f: impl FnOnce(&mut DrawFrame) -> TorqueStatus) -> TorqueStatus {
    DRAWS.with(|draws| {
        let mut draws = draws.borrow_mut();
        match draws.iter_mut().rev().find(|d| d.handle == handle) {
            Some(frame) => f(frame),
            None => TORQUE_ERR_HANDLE,
        }
    })
}

/// A table function's body, with a panic of the engine's own turned into a status instead of unwinding into the
/// extension.
fn guard(f: impl FnOnce() -> TorqueStatus) -> TorqueStatus {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(TORQUE_ERR_PANIC)
}

/// The bytes of `s`, at most `max` of them.
///
/// # Safety
///
/// When `s.ptr` is not null, it points at `s.len` readable bytes that stay valid and unchanged for `'a` (the ABI's
/// promise of an extension handing text over for one call).
unsafe fn bytes<'a>(s: TorqueStr, max: usize) -> Result<&'a [u8], TorqueStatus> {
    if s.len > max {
        return Err(TORQUE_ERR_RANGE);
    }
    if s.len == 0 {
        return Ok(&[]);
    }
    if s.ptr.is_null() {
        return Err(TORQUE_ERR_INVALID);
    }
    // SAFETY: not null, at most `max` long, and valid for `'a` by the caller's promise.
    Ok(unsafe { std::slice::from_raw_parts(s.ptr, s.len) })
}

/// `s` as UTF-8 text of at most `max` bytes.
///
/// # Safety
///
/// As [`bytes`].
unsafe fn utf8(s: TorqueStr, max: usize) -> Result<String, TorqueStatus> {
    // SAFETY: the caller's promise.
    let b = unsafe { bytes(s, max) }?;
    std::str::from_utf8(b)
        .map(str::to_string)
        .map_err(|_| TORQUE_ERR_INVALID)
}

/// Writes `value` to `out`: [`TORQUE_ERR_INVALID`] when it is null or misaligned.
///
/// # Safety
///
/// When `out` is not null and aligned, it is a place for a `T` the extension owns, valid during the call.
unsafe fn put<T>(out: *mut T, value: T) -> TorqueStatus {
    if out.is_null() || !out.is_aligned() {
        return TORQUE_ERR_INVALID;
    }
    // SAFETY: not null, aligned, and a valid place by the caller's promise.
    unsafe { out.write(value) };
    TORQUE_OK
}

/// A console name: ASCII letters, digits and `_`, not starting with a digit.
fn is_script_name(name: &str) -> bool {
    name.bytes().next().is_some_and(|b| !b.is_ascii_digit())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// A global's name: `$` and then letters, digits, `_` and `::` separators.
fn is_global_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('$') else {
        return false;
    };
    !rest.is_empty() && rest.split("::").all(is_script_name)
}

fn side_of(code: u32) -> Option<Side> {
    match code {
        TORQUE_SIDE_BOTH => Some(Side::Both),
        TORQUE_SIDE_SERVER => Some(Side::Server),
        TORQUE_SIDE_CLIENT => Some(Side::Client),
        _ => None,
    }
}

fn arg_kind(code: u32) -> Option<ArgKind> {
    match code {
        TORQUE_ARG_REQUIRED => Some(ArgKind::Required),
        TORQUE_ARG_OPTIONAL => Some(ArgKind::Optional),
        TORQUE_ARG_REST => Some(ArgKind::Rest),
        _ => None,
    }
}

/// Checks a native declaration and copies it.
///
/// # Safety
///
/// `decl` is null or points at a declaration whose pointers keep the ABI's promises for the call.
unsafe fn read_native(
    decl: *const TorqueNativeDecl,
    method: bool,
) -> Result<NativeSpec, (TorqueStatus, String)> {
    let bad = |what: &str| (TORQUE_ERR_INVALID, what.to_string());
    if decl.is_null() || !decl.is_aligned() {
        return Err(bad("a null or misaligned declaration"));
    }
    // SAFETY: not null and aligned; the extension keeps it valid during the call.
    let d = unsafe { *decl };
    let side = side_of(d.side).ok_or_else(|| bad("an unknown side"))?;
    // SAFETY: the declaration's texts keep the ABI's promise for the call.
    let namespace = unsafe { utf8(d.ns, TORQUE_MAX_NAME) }
        .map_err(|s| (s, "the namespace is not UTF-8 or too long".into()))?;
    // SAFETY: as above.
    let name = unsafe { utf8(d.name, TORQUE_MAX_NAME) }
        .map_err(|s| (s, "the name is not UTF-8 or too long".into()))?;
    if !is_script_name(&name) {
        return Err(bad(&format!("{name:?} is not a console name")));
    }
    if (method || !namespace.is_empty()) && !is_script_name(&namespace) {
        return Err(bad(&format!(
            "{namespace:?} is not a namespace or class name"
        )));
    }
    if d.arg_count > TORQUE_MAX_ARGS {
        return Err((
            TORQUE_ERR_RANGE,
            format!("{name}: more than {TORQUE_MAX_ARGS} arguments"),
        ));
    }
    if d.arg_count > 0 && (d.args.is_null() || !d.args.is_aligned()) {
        return Err(bad("the arguments are null or misaligned"));
    }
    let raw_args = if d.arg_count == 0 {
        &[][..]
    } else {
        // SAFETY: not null, aligned, at most TORQUE_MAX_ARGS long, and valid during the call by the ABI's promise.
        unsafe { std::slice::from_raw_parts(d.args, d.arg_count) }
    };
    let mut args = Vec::with_capacity(raw_args.len());
    for arg in raw_args {
        // SAFETY: the argument's name keeps the ABI's promise.
        let arg_name = unsafe { utf8(arg.name, TORQUE_MAX_NAME) }
            .map_err(|s| (s, "an argument's name is not UTF-8 or too long".into()))?;
        if arg_name.is_empty() {
            return Err(bad("an argument has no name"));
        }
        let kind = arg_kind(arg.kind).ok_or_else(|| bad("an unknown argument kind"))?;
        args.push((arg_name, kind));
    }
    let kinds: Vec<torque_engine::ParamKind> = args
        .iter()
        .map(|(_, k)| torque_engine::ParamKind::Script(*k))
        .collect();
    if let Some(error) = torque_engine::param::native_shape_error(&kinds) {
        return Err(bad(&format!("{name}: {error}")));
    }
    let text = |s: TorqueStr, what: &str| {
        // SAFETY: the declaration's texts keep the ABI's promise for the call.
        unsafe { utf8(s, TORQUE_MAX_TEXT) }.map_err(|status| {
            (
                status,
                format!("{name}: the {what} is not UTF-8 or too long"),
            )
        })
    };
    let description = text(d.description, "description")?;
    let docs = text(d.docs, "docs")?;
    if d.flags & !(TORQUE_NATIVE_DEV | TORQUE_NATIVE_EXACT_USAGE) != 0 {
        return Err(bad("unknown flags"));
    }
    let usage = if d.flags & TORQUE_NATIVE_EXACT_USAGE != 0 {
        Some(text(d.usage, "usage")?)
    } else {
        None
    };
    let call = d
        .call
        .ok_or_else(|| bad(&format!("{name}: no function to call")))?;
    Ok(NativeSpec {
        side,
        method,
        namespace,
        name,
        args,
        description,
        usage,
        docs,
        dev: d.flags & TORQUE_NATIVE_DEV != 0,
        call,
        user: UserPtr(d.user),
    })
}

fn register(init: TorqueInit, decl: *const TorqueNativeDecl, method: bool) -> TorqueStatus {
    guard(|| {
        with_init(init, |frame| {
            if frame.count() >= TORQUE_MAX_REGISTRATIONS {
                frame
                    .refused
                    .push(format!("more than {TORQUE_MAX_REGISTRATIONS} declarations"));
                return TORQUE_ERR_RANGE;
            }
            // SAFETY: the extension hands its declaration over for this call, as the ABI says.
            match unsafe { read_native(decl, method) } {
                Ok(spec) => {
                    frame.natives.push(spec);
                    TORQUE_OK
                }
                Err((status, reason)) => {
                    frame.refused.push(reason);
                    status
                }
            }
        })
    })
}

unsafe extern "C" fn register_function(
    init: TorqueInit,
    decl: *const TorqueNativeDecl,
) -> TorqueStatus {
    register(init, decl, false)
}

unsafe extern "C" fn register_method(
    init: TorqueInit,
    decl: *const TorqueNativeDecl,
) -> TorqueStatus {
    register(init, decl, true)
}

unsafe extern "C" fn register_global(
    init: TorqueInit,
    side: u32,
    name: TorqueStr,
    default_value: TorqueStr,
) -> TorqueStatus {
    guard(|| {
        with_init(init, |frame| {
            // SAFETY: the texts keep the ABI's promise for the call.
            let read = unsafe {
                (
                    utf8(name, TORQUE_MAX_NAME),
                    utf8(default_value, TORQUE_MAX_VALUE),
                )
            };
            let (Ok(name), Ok(default), Some(side)) = (read.0, read.1, side_of(side)) else {
                frame
                    .refused
                    .push("a global with a bad name, value or side".to_string());
                return TORQUE_ERR_INVALID;
            };
            if !is_global_name(&name) {
                frame
                    .refused
                    .push(format!("{name:?} is not a global's name"));
                return TORQUE_ERR_INVALID;
            }
            if frame.count() >= TORQUE_MAX_REGISTRATIONS {
                frame
                    .refused
                    .push(format!("more than {TORQUE_MAX_REGISTRATIONS} declarations"));
                return TORQUE_ERR_RANGE;
            }
            frame.globals.push(GlobalSpec {
                side,
                name,
                default,
            });
            TORQUE_OK
        })
    })
}

unsafe extern "C" fn register_render_pass(
    init: TorqueInit,
    decl: *const TorquePassDecl,
) -> TorqueStatus {
    guard(|| {
        with_init(init, |frame| {
            if decl.is_null() || !decl.is_aligned() {
                frame
                    .refused
                    .push("a null or misaligned pass declaration".to_string());
                return TORQUE_ERR_INVALID;
            }
            // SAFETY: not null and aligned; valid during the call by the ABI's promise.
            let d = unsafe { *decl };
            if !matches!(
                d.stage,
                TORQUE_STAGE_OPAQUE | TORQUE_STAGE_TRANSLUCENT | TORQUE_STAGE_OVERLAY
            ) {
                frame
                    .refused
                    .push(format!("a pass at the unknown stage {}", d.stage));
                return TORQUE_ERR_INVALID;
            }
            let (Some(create), Some(draw)) = (d.create, d.draw) else {
                frame
                    .refused
                    .push("a pass with no create or draw function".to_string());
                return TORQUE_ERR_INVALID;
            };
            if frame.count() >= TORQUE_MAX_REGISTRATIONS {
                frame
                    .refused
                    .push(format!("more than {TORQUE_MAX_REGISTRATIONS} declarations"));
                return TORQUE_ERR_RANGE;
            }
            frame.passes.push(PassSpec {
                stage: d.stage,
                create,
                draw,
                drop: d.drop,
                user: UserPtr(d.user),
            });
            TORQUE_OK
        })
    })
}

unsafe extern "C" fn arg_count(call: TorqueCall, out: *mut u32) -> TorqueStatus {
    guard(|| {
        with_call(call, |frame| {
            let n = u32::try_from(frame.args.len()).unwrap_or(u32::MAX);
            // SAFETY: the extension's place for the count, valid during the call.
            unsafe { put(out, n) }
        })
    })
}

/// Script argument `index` of the live call `call`, through `read`.
fn with_arg(
    call: TorqueCall,
    index: u32,
    read: impl FnOnce(&[u8]) -> TorqueStatus,
) -> TorqueStatus {
    with_call(call, |frame| match frame.args.get(index as usize) {
        Some(arg) => read(arg),
        None => TORQUE_ERR_RANGE,
    })
}

unsafe extern "C" fn arg_str(call: TorqueCall, index: u32, out: *mut TorqueStr) -> TorqueStatus {
    guard(|| {
        with_arg(call, index, |arg| {
            let text = <&[u8]>::from_script(arg).unwrap_or_default();
            // SAFETY: the extension's place for the text; the bytes stay in the call's frame until the call returns.
            unsafe {
                put(
                    out,
                    TorqueStr {
                        ptr: text.as_ptr(),
                        len: text.len(),
                    },
                )
            }
        })
    })
}

unsafe extern "C" fn arg_int(call: TorqueCall, index: u32, out: *mut i32) -> TorqueStatus {
    guard(|| {
        with_arg(call, index, |arg| {
            let value = i32::from_script(arg).unwrap_or(0);
            // SAFETY: the extension's place for the value.
            unsafe { put(out, value) }
        })
    })
}

unsafe extern "C" fn arg_float(call: TorqueCall, index: u32, out: *mut f32) -> TorqueStatus {
    guard(|| {
        with_arg(call, index, |arg| {
            let value = f32::from_script(arg).unwrap_or(0.0);
            // SAFETY: the extension's place for the value.
            unsafe { put(out, value) }
        })
    })
}

unsafe extern "C" fn arg_bool(call: TorqueCall, index: u32, out: *mut u32) -> TorqueStatus {
    guard(|| {
        with_arg(call, index, |arg| {
            let value = u32::from(bool::from_script(arg).unwrap_or(false));
            // SAFETY: the extension's place for the value.
            unsafe { put(out, value) }
        })
    })
}

unsafe extern "C" fn this_object(call: TorqueCall, out: *mut u32) -> TorqueStatus {
    guard(|| {
        with_call(call, |frame| match frame.this {
            // SAFETY: the extension's place for the id.
            Some(id) => unsafe { put(out, id) },
            None => TORQUE_ERR_FAILED,
        })
    })
}

/// Sets the live call's result.
fn set_result(call: TorqueCall, result: Native) -> TorqueStatus {
    guard(|| {
        with_call(call, |frame| {
            frame.result = Some(result);
            TORQUE_OK
        })
    })
}

unsafe extern "C" fn return_str(call: TorqueCall, value: TorqueStr) -> TorqueStatus {
    // SAFETY: the text keeps the ABI's promise for the call; it is copied.
    match unsafe { bytes(value, TORQUE_MAX_VALUE) } {
        Ok(text) => set_result(call, Native::Str(text.to_vec())),
        Err(status) => status,
    }
}

unsafe extern "C" fn return_int(call: TorqueCall, value: i32) -> TorqueStatus {
    set_result(call, Native::Int(value))
}

unsafe extern "C" fn return_float(call: TorqueCall, value: f32) -> TorqueStatus {
    use torque_ts_vm::convert::IntoScript;
    set_result(call, value.into_script())
}

unsafe extern "C" fn return_bool(call: TorqueCall, value: u32) -> TorqueStatus {
    set_result(call, Native::Bool(value != 0))
}

unsafe extern "C" fn return_error(call: TorqueCall, message: TorqueStr) -> TorqueStatus {
    guard(|| {
        // SAFETY: the text keeps the ABI's promise for the call; it is copied.
        let message = match unsafe { bytes(message, TORQUE_MAX_TEXT) } {
            Ok(text) => String::from_utf8_lossy(text).into_owned(),
            Err(status) => return status,
        };
        with_call(call, |frame| {
            frame.error = Some(message);
            TORQUE_OK
        })
    })
}

unsafe extern "C" fn get_global(
    call: TorqueCall,
    name: TorqueStr,
    out: *mut TorqueStr,
) -> TorqueStatus {
    guard(|| {
        // SAFETY: the text keeps the ABI's promise for the call.
        let name = match unsafe { utf8(name, TORQUE_MAX_NAME) } {
            Ok(name) if is_global_name(&name) => name,
            Ok(_) => return TORQUE_ERR_INVALID,
            Err(status) => return status,
        };
        with_call(call, |frame| {
            // SAFETY: the frame is live, so the host's native that made it still holds the script engine exclusively
            // and is blocked in this call: nothing else touches it.
            let vm = unsafe { &*frame.vm };
            frame.scratch = vm.global(&name).unwrap_or_default();
            let text = TorqueStr {
                ptr: frame.scratch.as_ptr(),
                len: frame.scratch.len(),
            };
            // SAFETY: the extension's place for the text; it points into the frame until the next `get_global` or the
            // call's end, as the ABI says.
            unsafe { put(out, text) }
        })
    })
}

unsafe extern "C" fn set_global(
    call: TorqueCall,
    name: TorqueStr,
    value: TorqueStr,
) -> TorqueStatus {
    guard(|| {
        // SAFETY: the texts keep the ABI's promise for the call; they are copied.
        let (name, value) =
            match unsafe { (utf8(name, TORQUE_MAX_NAME), bytes(value, TORQUE_MAX_VALUE)) } {
                (Ok(name), Ok(value)) if is_global_name(&name) => (name, value.to_vec()),
                (Err(status), _) | (_, Err(status)) => return status,
                _ => return TORQUE_ERR_INVALID,
            };
        with_call(call, |frame| {
            // SAFETY: as in `get_global`: the frame is live, so the script engine is the call's alone.
            let vm = unsafe { &mut *frame.vm };
            vm.set_global_text(name.as_bytes(), &value);
            TORQUE_OK
        })
    })
}

unsafe extern "C" fn draw_triangles(
    draw: TorqueDraw,
    space: u32,
    vertices: *const TorqueVertex,
    count: usize,
) -> TorqueStatus {
    guard(|| {
        with_draw(draw, |frame| {
            if !matches!(space, TORQUE_SPACE_WORLD | TORQUE_SPACE_CLIP) || !count.is_multiple_of(3)
            {
                return TORQUE_ERR_INVALID;
            }
            if count > TORQUE_MAX_PASS_VERTICES - frame.vertices {
                return TORQUE_ERR_RANGE;
            }
            if count == 0 {
                return TORQUE_OK;
            }
            if vertices.is_null() || !vertices.is_aligned() {
                return TORQUE_ERR_INVALID;
            }
            // SAFETY: not null, aligned, at most the frame's limit long, and valid during the call by the ABI's promise.
            let given = unsafe { std::slice::from_raw_parts(vertices, count) };
            if given
                .iter()
                .any(|v| !v.position.iter().chain(&v.color).all(|x| x.is_finite()))
            {
                return TORQUE_ERR_INVALID;
            }
            frame.vertices += count;
            frame.batches.push((space, given.to_vec()));
            TORQUE_OK
        })
    })
}

unsafe extern "C" fn log(handle: u64, level: u32, message: TorqueStr) -> TorqueStatus {
    guard(|| {
        if level > TORQUE_LOG_ERROR {
            return TORQUE_ERR_INVALID;
        }
        // SAFETY: the text keeps the ABI's promise for the call; it is copied.
        let message = match unsafe { bytes(message, TORQUE_MAX_TEXT) } {
            Ok(text) => String::from_utf8_lossy(text).into_owned(),
            Err(status) => return status,
        };
        // The call's script engine, taken out of the frame first: printing may run the console's own hooks, which must
        // find this thread's frames free.
        let vm = CALLS.with(|calls| {
            calls
                .borrow()
                .iter()
                .rev()
                .find(|c| c.handle == handle)
                .map(|c| c.vm)
        });
        if let Some(vm) = vm {
            // SAFETY: as in `get_global`: the frame is live, so the script engine is the call's alone.
            let vm = unsafe { &mut *vm };
            match level {
                TORQUE_LOG_INFO => vm.print(message.as_bytes()),
                TORQUE_LOG_WARN => vm.warn(message),
                _ => vm.error(message),
            }
            return TORQUE_OK;
        }
        let line = format!("{}{message}", ["", "warning: ", "error: "][level as usize]);
        let status = with_init(handle, |frame| {
            frame.log.push(line.clone());
            TORQUE_OK
        });
        if status != TORQUE_ERR_HANDLE {
            return status;
        }
        with_draw(handle, |frame| {
            frame.log.push(line);
            TORQUE_OK
        })
    })
}

/// The function table for one init.
fn table(init: TorqueInit) -> TorqueApi {
    TorqueApi {
        abi_version: TORQUE_ABI_VERSION,
        size: std::mem::size_of::<TorqueApi>() as u32,
        init,
        register_function,
        register_method,
        register_global,
        register_render_pass,
        arg_count,
        arg_str,
        arg_int,
        arg_float,
        arg_bool,
        this_object,
        return_str,
        return_int,
        return_float,
        return_bool,
        return_error,
        get_global,
        set_global,
        draw_triangles,
        log,
    }
}

/// Err with the message when `version` is not this engine's ABI version.
pub(crate) fn check_abi(version: u32) -> Result<(), String> {
    if version == TORQUE_ABI_VERSION {
        return Ok(());
    }
    Err(format!(
        "it was built for native extension ABI version {version}, and this engine speaks version {TORQUE_ABI_VERSION}: \
         rebuild it against this engine's torque-plugin-sdk (or its torque_plugin.h)"
    ))
}

/// Runs an extension's init, after its ABI version was checked, and checks what it reports and registered.
pub(crate) fn run_init(init: TorquePluginInitFn) -> Result<Registered, String> {
    let handle = next_handle();
    // The table stays valid while the extension is loaded, which is for the rest of the process.
    let api: &'static TorqueApi = Box::leak(Box::new(table(handle)));
    let empty = TorqueStr {
        ptr: std::ptr::null(),
        len: 0,
    };
    let mut info = TorquePluginInfo {
        abi_version: 0,
        id: empty,
        version: empty,
        net_version: 0,
        has_net_version: 0,
    };
    INITS.with(|inits| {
        inits.borrow_mut().push(InitFrame {
            handle,
            natives: Vec::new(),
            globals: Vec::new(),
            passes: Vec::new(),
            refused: Vec::new(),
            log: Vec::new(),
        })
    });
    // SAFETY: the extension's entry point, called once as the ABI says, with a table that lives for the process and a
    // place for its info on this stack.
    let status = unsafe { init(api, &mut info) };
    let frame = INITS
        .with(|inits| inits.borrow_mut().pop())
        .expect("the init's frame is the last one pushed");
    let said = |frame: &InitFrame| {
        let mut lines: Vec<String> = frame.refused.clone();
        lines.extend(
            frame
                .log
                .iter()
                .filter(|l| l.starts_with("error: "))
                .cloned(),
        );
        if lines.is_empty() {
            String::new()
        } else {
            format!(" ({})", lines.join("; "))
        }
    };
    match status {
        TORQUE_OK => {}
        TORQUE_ERR_ABI => {
            return Err(format!(
                "it does not speak this engine's ABI version {TORQUE_ABI_VERSION}{}",
                said(&frame)
            ));
        }
        TORQUE_ERR_PANIC => return Err(format!("it panicked as it started{}", said(&frame))),
        other => {
            return Err(format!(
                "its init failed with status {other}{}",
                said(&frame)
            ));
        }
    }
    check_abi(info.abi_version)?;
    // SAFETY: the info's texts are the extension's own, valid while it is loaded; they are copied here.
    let (id, version) = unsafe {
        (
            utf8(info.id, TORQUE_MAX_NAME),
            utf8(info.version, TORQUE_MAX_NAME),
        )
    };
    let id = id.ok().filter(|id| torque_engine::PluginId::is_valid(id)).ok_or(
        "it reports an id that is not a plugin id (ASCII letters, digits, '_', '-', '.' and ':')",
    )?;
    let version = version
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or("it reports no version, or one that is not UTF-8")?;
    if !frame.refused.is_empty() {
        return Err(format!(
            "the engine refused what it declared: {}",
            frame.refused.join("; ")
        ));
    }
    Ok(Registered {
        id,
        version,
        net_version: (info.has_net_version != 0).then_some(info.net_version),
        natives: frame.natives,
        globals: frame.globals,
        passes: frame.passes,
        log: frame.log,
    })
}

/// Loads the library at the absolute `path` (its version already read from the file), checks the exported ABI version
/// again, and runs its init. The library stays
/// loaded for the rest of the process (an extension's threads or callbacks may outlive any engine).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn load_library(path: &std::path::Path) -> Result<Registered, String> {
    if !path.is_absolute() {
        return Err(format!("{} is not an absolute path", path.display()));
    }
    let library = open_library(path).map_err(|e| format!("it could not be loaded: {e}"))?;
    let version = {
        // SAFETY: the symbol, when present, is the `uint32_t` the ABI says; reading it runs none of the library's code.
        let symbol =
            unsafe { library.get::<*const u32>(b"torque_plugin_abi_version\0") }.map_err(|_| {
                "it exports no torque_plugin_abi_version: it is not a native extension".to_string()
            })?;
        let at: *const u32 = *symbol;
        if at.is_null() || !at.is_aligned() {
            return Err("its torque_plugin_abi_version is not a uint32_t".to_string());
        }
        // SAFETY: not null and aligned: the address of the library's exported constant, mapped while it is loaded.
        unsafe { at.read() }
    };
    check_abi(version)?;
    // SAFETY: the symbol is the ABI's entry point; its type is the one the ABI declares.
    let init = unsafe { library.get::<TorquePluginInitFn>(b"torque_plugin_init\0") }
        .map(|symbol| *symbol)
        .map_err(|_| {
            "it exports no torque_plugin_init: it is not a native extension".to_string()
        })?;
    // Kept loaded from here on: its init is about to run, and what it registers points into it.
    let _ = Box::leak(Box::new(library));
    run_init(init)
}

/// Opens the library at the absolute `path`: on Windows its own dependencies are looked for only beside it and in
/// System32 (never the working directory or `PATH`).
#[cfg(windows)]
fn open_library(path: &std::path::Path) -> Result<libloading::Library, libloading::Error> {
    use libloading::os::windows::{
        LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, Library,
    };
    // SAFETY: loading a library runs its platform initializers. This happens only for the private copy of a file the
    // player approved by its hash, whose exported ABI version was checked in the file: running its code is what that
    // approval is for.
    unsafe {
        Library::load_with_flags(
            path,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    }
    .map(Into::into)
}

/// Opens the library at the absolute `path`, its symbols bound at once and kept to itself.
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn open_library(path: &std::path::Path) -> Result<libloading::Library, libloading::Error> {
    use libloading::os::unix::{Library, RTLD_LOCAL, RTLD_NOW};
    // SAFETY: as on Windows: only the private copy of an approved, checked file.
    unsafe { Library::open(Some(path), RTLD_NOW | RTLD_LOCAL) }.map(Into::into)
}

/// What a call into an extension's native came to.
pub(crate) enum CallOutcome {
    /// Its result.
    Done(Native),
    /// It failed the call with this message (`return_error`, or a status).
    Failed(String),
    /// It reported a panic of its own.
    Panicked,
}

/// Calls the native `spec` with the script arguments `args` (and a method's object `this`) on `vm`.
pub(crate) fn call_native(
    spec: &NativeSpec,
    vm: &mut Vm,
    args: &[Vec<u8>],
    this: Option<u32>,
) -> CallOutcome {
    let handle = next_handle();
    CALLS.with(|calls| {
        calls.borrow_mut().push(CallFrame {
            handle,
            vm: vm as *mut Vm,
            args: args.to_vec(),
            this,
            result: None,
            error: None,
            scratch: Vec::new(),
        })
    });
    // SAFETY: the extension's own function and pointer, as it declared them; the call's handle is live in this
    // thread's frames until it returns, and `vm` is not touched here until then.
    let status = unsafe { (spec.call)(spec.user.0, handle) };
    let frame = CALLS
        .with(|calls| calls.borrow_mut().pop())
        .expect("the call's frame is the last one pushed");
    match status {
        TORQUE_OK => match frame.error {
            Some(message) => CallOutcome::Failed(message),
            None => CallOutcome::Done(frame.result.unwrap_or(Native::Void)),
        },
        TORQUE_ERR_PANIC => CallOutcome::Panicked,
        other => CallOutcome::Failed(
            frame
                .error
                .unwrap_or_else(|| format!("the extension's function failed with status {other}")),
        ),
    }
}

/// A pass instance of an extension.
#[cfg_attr(not(feature = "render"), allow(dead_code))]
pub(crate) struct PassInstance(UserPtr);

/// What making a pass instance came to.
#[cfg_attr(not(feature = "render"), allow(dead_code))]
pub(crate) struct CreateOutcome {
    pub(crate) status: TorqueStatus,
    /// The instance, when the status is [`TORQUE_OK`] and it made one.
    pub(crate) made: Option<PassInstance>,
    pub(crate) log: Vec<String>,
}

/// Makes an instance of the pass `spec`, with a draw handle live for its log.
#[cfg_attr(not(feature = "render"), allow(dead_code))]
pub(crate) fn create_pass(spec: &PassSpec) -> CreateOutcome {
    let handle = next_handle();
    DRAWS.with(|draws| {
        draws.borrow_mut().push(DrawFrame {
            handle,
            batches: Vec::new(),
            vertices: 0,
            log: Vec::new(),
        })
    });
    let mut made: *mut c_void = std::ptr::null_mut();
    // SAFETY: the extension's own function and pointer, as it declared them, a place for the instance on this stack, and
    // a handle live in this thread's frames until it returns.
    let status = unsafe { (spec.create)(spec.user.0, handle, &mut made) };
    let done = DRAWS
        .with(|draws| draws.borrow_mut().pop())
        .expect("the frame is the last one pushed");
    CreateOutcome {
        status,
        made: (status == TORQUE_OK && !made.is_null()).then_some(PassInstance(UserPtr(made))),
        log: done.log,
    }
}

/// What one draw of a pass came to.
#[cfg_attr(not(feature = "render"), allow(dead_code))]
pub(crate) struct DrawOutcome {
    pub(crate) status: TorqueStatus,
    /// The triangles: a space (`TORQUE_SPACE_`) and its vertices, in the order drawn.
    pub(crate) batches: Vec<(u32, Vec<TorqueVertex>)>,
    pub(crate) log: Vec<String>,
}

/// Draws one frame of the instance `pass` of `spec`.
#[cfg_attr(not(feature = "render"), allow(dead_code))]
pub(crate) fn draw_pass(
    spec: &PassSpec,
    pass: &PassInstance,
    frame: &TorquePassFrame,
) -> DrawOutcome {
    let handle = next_handle();
    DRAWS.with(|draws| {
        draws.borrow_mut().push(DrawFrame {
            handle,
            batches: Vec::new(),
            vertices: 0,
            log: Vec::new(),
        })
    });
    // SAFETY: the extension's own function and the instance it made; the frame is on this stack for the call, and the
    // draw handle is live in this thread's frames until it returns.
    let status = unsafe { (spec.draw)(pass.0.0, frame, handle) };
    let done = DRAWS
        .with(|draws| draws.borrow_mut().pop())
        .expect("the draw's frame is the last one pushed");
    DrawOutcome {
        status,
        batches: done.batches,
        log: done.log,
    }
}

/// Frees the instance `pass` of `spec`.
#[cfg_attr(not(feature = "render"), allow(dead_code))]
pub(crate) fn drop_pass(spec: &PassSpec, pass: PassInstance) {
    if let Some(drop) = spec.drop {
        // SAFETY: the extension's own function and the instance it made, freed once (the instance is moved in).
        unsafe { drop(pass.0.0) };
    }
}
