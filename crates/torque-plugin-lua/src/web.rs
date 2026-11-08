//! The extension as a WebAssembly module for a browser build (`wasm32-wasip1`, built by `just lua-web`): the same realms,
//! sandbox and limits as the native library, run over a function table whose functions are the page's imports.
//!
//! A page cannot load a native library, so the engine's side of the C ABI is the page's JavaScript (`web/lua.js`), which
//! hands every request on to the engine's `LuaHost` (`torque_plugin_loader::web_lua`). This file builds the [`TorqueApi`]
//! the extension's code expects from those imports, so `realm`, `api`, `require` and `value` run unchanged: the module is
//! the native extension, with the engine one bridge away.
//!
//! **Imports** (module `torque`), one per function of the table the extension uses, each naming the page's call handle:
//! `log`, `get_global`, `set_global`, `call_function`, `call_method`, `has_method`, `find_object`, `get_field`,
//! `set_field`, `define_function`, `charge`, `read_script`, and `result_len` with `result_copy`, which hand over the bytes
//! of the last answer that had any. Texts go as a pointer and a length into this module's memory. What a call carries from
//! the start (its script engine, its side, its package, its arguments) comes with the call and is answered here.
//!
//! **Exports**: `torque_lua_start` (the extension's entry point, once, after `_initialize`), `torque_lua_alloc` and
//! `torque_lua_free` (the page's texts going in), `torque_lua_run_file`, `torque_lua_call_defined` with
//! `torque_lua_result` and `torque_lua_result_len` (its result), and `torque_lua_vm_dropped`.

#![allow(unsafe_code)]

use std::cell::RefCell;

use torque_plugin_sdk::abi::{
    TORQUE_ABI_VERSION, TORQUE_ERR_FAILED, TORQUE_ERR_HANDLE, TORQUE_ERR_INVALID, TORQUE_ERR_RANGE,
    TORQUE_LOG_ERROR, TORQUE_MAX_ARGS, TORQUE_OK, TorqueApi, TorqueCall, TorqueDefineDecl,
    TorqueDraw, TorqueInit, TorqueLanguageDecl, TorqueNativeDecl, TorquePassDecl, TorquePluginInfo,
    TorqueStatus, TorqueStr, TorqueVertex, TorqueVm, TorqueVmInfo,
};

#[link(wasm_import_module = "torque")]
unsafe extern "C" {
    #[link_name = "log"]
    fn page_log(call: u32, level: u32, text: *const u8, len: usize) -> TorqueStatus;
    #[link_name = "get_global"]
    fn page_get_global(call: u32, name: *const u8, len: usize) -> TorqueStatus;
    #[link_name = "set_global"]
    fn page_set_global(
        call: u32,
        name: *const u8,
        name_len: usize,
        value: *const u8,
        value_len: usize,
    ) -> TorqueStatus;
    #[link_name = "call_function"]
    fn page_call_function(
        call: u32,
        ns: *const u8,
        ns_len: usize,
        name: *const u8,
        name_len: usize,
        args: *const TorqueStr,
        count: usize,
    ) -> TorqueStatus;
    #[link_name = "call_method"]
    fn page_call_method(
        call: u32,
        object: u32,
        name: *const u8,
        name_len: usize,
        args: *const TorqueStr,
        count: usize,
    ) -> TorqueStatus;
    #[link_name = "has_method"]
    fn page_has_method(call: u32, object: u32, name: *const u8, len: usize) -> u32;
    #[link_name = "find_object"]
    fn page_find_object(call: u32, name: *const u8, len: usize, out: *mut u32) -> TorqueStatus;
    #[link_name = "get_field"]
    fn page_get_field(
        call: u32,
        object: u32,
        name: *const u8,
        name_len: usize,
        index: *const u8,
        index_len: usize,
    ) -> TorqueStatus;
    #[link_name = "set_field"]
    fn page_set_field(
        call: u32,
        object: u32,
        name: *const u8,
        name_len: usize,
        index: *const u8,
        index_len: usize,
        value: *const u8,
        value_len: usize,
    ) -> TorqueStatus;
    #[link_name = "define_function"]
    fn page_define_function(call: u32, decl: *const TorqueDefineDecl) -> TorqueStatus;
    #[link_name = "charge"]
    fn page_charge(call: u32, steps: u64) -> TorqueStatus;
    #[link_name = "read_script"]
    fn page_read_script(call: u32, path: *const u8, len: usize) -> TorqueStatus;
    #[link_name = "result_len"]
    fn page_result_len() -> usize;
    #[link_name = "result_copy"]
    fn page_result_copy(out: *mut u8);
}

/// The handle `torque_lua_start` hands the extension's entry point: past every call handle the page gives (those are 32
/// bits).
const INIT_HANDLE: TorqueInit = u64::MAX;

/// One of the page's calls into the module that is running.
struct Frame {
    handle: TorqueCall,
    vm: TorqueVm,
    side: u32,
    package: Vec<u8>,
    args: Vec<Vec<u8>>,
    /// What the last table call that answered with text handed out: valid until the next such call or the call's end, as
    /// the ABI says.
    scratch: Vec<u8>,
    /// What a defined function returned.
    result: Vec<u8>,
}

/// A language the extension registered.
#[derive(Clone, Copy)]
struct Language {
    extension: [u8; 16],
    extension_len: usize,
    decl: TorqueLanguageDecl,
}

impl Language {
    fn extension(&self) -> &[u8] {
        &self.extension[..self.extension_len]
    }
}

thread_local! {
    /// The running calls, innermost last. No borrow is held across a call into the extension or the page.
    static FRAMES: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
    static LANGUAGES: RefCell<Vec<Language>> = const { RefCell::new(Vec::new()) };
    /// The last `torque_lua_call_defined`'s result, for the page to read.
    static RESULT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// `bytes` as the ABI's text (borrowed: valid while `bytes` is).
fn text(bytes: &[u8]) -> TorqueStr {
    TorqueStr {
        ptr: bytes.as_ptr(),
        len: bytes.len(),
    }
}

/// The bytes of a text handed over.
///
/// # Safety
///
/// `s` is null with any length 0, or `len` bytes valid for `'a`.
unsafe fn bytes_of<'a>(s: TorqueStr) -> Option<&'a [u8]> {
    if s.len == 0 {
        return Some(&[]);
    }
    if s.ptr.is_null() {
        return None;
    }
    // SAFETY: the caller's promise.
    Some(unsafe { std::slice::from_raw_parts(s.ptr, s.len) })
}

/// Writes `value` to `out`.
///
/// # Safety
///
/// `out` is null, misaligned, or a valid place for a `T`.
unsafe fn put<T>(out: *mut T, value: T) -> TorqueStatus {
    if out.is_null() || !out.is_aligned() {
        return TORQUE_ERR_INVALID;
    }
    // SAFETY: not null and aligned; the caller's promise for the rest.
    unsafe { out.write(value) };
    TORQUE_OK
}

/// The page's number for the live call `call`, or `None` when no running call has it.
fn page_handle(call: TorqueCall) -> Option<u32> {
    let live = FRAMES.with(|f| f.borrow().iter().any(|f| f.handle == call));
    if live { u32::try_from(call).ok() } else { None }
}

/// Runs `f` on the live call `call`'s frame.
fn with_frame(call: TorqueCall, f: impl FnOnce(&mut Frame) -> TorqueStatus) -> TorqueStatus {
    FRAMES.with(|frames| {
        match frames
            .borrow_mut()
            .iter_mut()
            .rev()
            .find(|frame| frame.handle == call)
        {
            Some(frame) => f(frame),
            None => TORQUE_ERR_HANDLE,
        }
    })
}

/// Hands `value` out through the frame's scratch to `out`.
///
/// # Safety
///
/// `out` as [`put`]'s.
unsafe fn hand_out(call: TorqueCall, value: Vec<u8>, out: *mut TorqueStr) -> TorqueStatus {
    with_frame(call, |frame| {
        frame.scratch = value;
        // SAFETY: the caller's promise; the text points into the frame until the next hand-out or the call's end.
        unsafe { put(out, text(&frame.scratch)) }
    })
}

/// The page's answer to the last request that had one, copied out of it.
fn page_result() -> Vec<u8> {
    // SAFETY: the page writes exactly `result_len` bytes, into the place made for them here.
    unsafe {
        let len = page_result_len();
        let mut bytes = vec![0u8; len];
        page_result_copy(bytes.as_mut_ptr());
        bytes
    }
}

/// A request whose answer is text: `request` returns the page's status, and on success the text is handed out to `out`.
///
/// # Safety
///
/// `out` as [`put`]'s.
unsafe fn with_text(
    call: TorqueCall,
    out: *mut TorqueStr,
    request: impl FnOnce(u32) -> TorqueStatus,
) -> TorqueStatus {
    let Some(page) = page_handle(call) else {
        return TORQUE_ERR_HANDLE;
    };
    let status = request(page);
    if status != TORQUE_OK {
        return status;
    }
    // SAFETY: the caller's promise.
    unsafe { hand_out(call, page_result(), out) }
}

// The table's functions. Those the Lua extension never calls (natives, passes, typed arguments and results) refuse.

unsafe extern "C" fn register_native(_: TorqueInit, _: *const TorqueNativeDecl) -> TorqueStatus {
    TORQUE_ERR_FAILED
}

unsafe extern "C" fn register_global(
    init: TorqueInit,
    _side: u32,
    _name: TorqueStr,
    _default: TorqueStr,
) -> TorqueStatus {
    // The engine's side sets the extension's one global (`$Pref::Lua::MemoryLimit`) to its default itself.
    if init == INIT_HANDLE {
        TORQUE_OK
    } else {
        TORQUE_ERR_HANDLE
    }
}

unsafe extern "C" fn register_render_pass(_: TorqueInit, _: *const TorquePassDecl) -> TorqueStatus {
    TORQUE_ERR_FAILED
}

unsafe extern "C" fn register_script_language(
    init: TorqueInit,
    decl: *const TorqueLanguageDecl,
) -> TorqueStatus {
    if init != INIT_HANDLE {
        return TORQUE_ERR_HANDLE;
    }
    if decl.is_null() || !decl.is_aligned() {
        return TORQUE_ERR_INVALID;
    }
    // SAFETY: not null and aligned; valid during the call.
    let decl = unsafe { *decl };
    // SAFETY: the declaration's text is valid during the call; it is copied.
    let Some(name) = (unsafe { bytes_of(decl.extension) }) else {
        return TORQUE_ERR_INVALID;
    };
    let mut extension = [0u8; 16];
    if name.is_empty()
        || name.len() > extension.len()
        || !name.iter().all(u8::is_ascii_alphanumeric)
    {
        return TORQUE_ERR_INVALID;
    }
    for (to, from) in extension.iter_mut().zip(name) {
        *to = from.to_ascii_lowercase();
    }
    LANGUAGES.with(|l| {
        l.borrow_mut().push(Language {
            extension,
            extension_len: name.len(),
            decl,
        })
    });
    TORQUE_OK
}

unsafe extern "C" fn arg_count(call: TorqueCall, out: *mut u32) -> TorqueStatus {
    with_frame(call, |frame| {
        let n = u32::try_from(frame.args.len()).unwrap_or(u32::MAX);
        // SAFETY: the extension's place for the count.
        unsafe { put(out, n) }
    })
}

unsafe extern "C" fn arg_str(call: TorqueCall, index: u32, out: *mut TorqueStr) -> TorqueStatus {
    with_frame(call, |frame| match frame.args.get(index as usize) {
        // SAFETY: the extension's place for the text; the bytes stay in the frame until the call returns.
        Some(arg) => unsafe { put(out, text(arg)) },
        None => TORQUE_ERR_RANGE,
    })
}

unsafe extern "C" fn arg_int(_: TorqueCall, _: u32, _: *mut i32) -> TorqueStatus {
    TORQUE_ERR_FAILED
}

unsafe extern "C" fn arg_float(_: TorqueCall, _: u32, _: *mut f32) -> TorqueStatus {
    TORQUE_ERR_FAILED
}

unsafe extern "C" fn arg_bool(_: TorqueCall, _: u32, _: *mut u32) -> TorqueStatus {
    TORQUE_ERR_FAILED
}

unsafe extern "C" fn this_object(_: TorqueCall, _: *mut u32) -> TorqueStatus {
    TORQUE_ERR_FAILED
}

unsafe extern "C" fn return_str(call: TorqueCall, value: TorqueStr) -> TorqueStatus {
    // SAFETY: the text is valid during the call; it is copied.
    let Some(value) = (unsafe { bytes_of(value) }).map(<[u8]>::to_vec) else {
        return TORQUE_ERR_INVALID;
    };
    with_frame(call, |frame| {
        frame.result = value;
        TORQUE_OK
    })
}

unsafe extern "C" fn return_int(_: TorqueCall, _: i32) -> TorqueStatus {
    TORQUE_ERR_FAILED
}

unsafe extern "C" fn return_float(_: TorqueCall, _: f32) -> TorqueStatus {
    TORQUE_ERR_FAILED
}

unsafe extern "C" fn return_bool(_: TorqueCall, _: u32) -> TorqueStatus {
    TORQUE_ERR_FAILED
}

unsafe extern "C" fn return_error(call: TorqueCall, message: TorqueStr) -> TorqueStatus {
    // SAFETY: the table's own function, with the text it was given.
    unsafe { log(call, TORQUE_LOG_ERROR, message) }
}

unsafe extern "C" fn get_global(
    call: TorqueCall,
    name: TorqueStr,
    out: *mut TorqueStr,
) -> TorqueStatus {
    // SAFETY: the name is valid during the call, and `out` is the extension's place for the value.
    unsafe { with_text(call, out, |page| page_get_global(page, name.ptr, name.len)) }
}

unsafe extern "C" fn set_global(
    call: TorqueCall,
    name: TorqueStr,
    value: TorqueStr,
) -> TorqueStatus {
    let Some(page) = page_handle(call) else {
        return TORQUE_ERR_HANDLE;
    };
    // SAFETY: both texts are valid during the call; the page copies them.
    unsafe { page_set_global(page, name.ptr, name.len, value.ptr, value.len) }
}

unsafe extern "C" fn draw_triangles(
    _: TorqueDraw,
    _: u32,
    _: *const TorqueVertex,
    _: usize,
) -> TorqueStatus {
    TORQUE_ERR_FAILED
}

unsafe extern "C" fn log(handle: u64, level: u32, message: TorqueStr) -> TorqueStatus {
    // The entry point's messages (handle 0 for the page) and every live call's.
    let page = if handle == INIT_HANDLE {
        Some(0)
    } else {
        page_handle(handle)
    };
    let Some(page) = page else {
        return TORQUE_ERR_HANDLE;
    };
    // SAFETY: the text is valid during the call; the page copies it.
    unsafe { page_log(page, level, message.ptr, message.len) }
}

unsafe extern "C" fn call_function(
    call: TorqueCall,
    ns: TorqueStr,
    name: TorqueStr,
    args: *const TorqueStr,
    count: usize,
    out: *mut TorqueStr,
) -> TorqueStatus {
    if count > TORQUE_MAX_ARGS {
        return TORQUE_ERR_RANGE;
    }
    // SAFETY: the texts and the arguments are valid during the call, and the page copies them; `out` is the extension's
    // place for the result.
    unsafe {
        with_text(call, out, |page| {
            page_call_function(page, ns.ptr, ns.len, name.ptr, name.len, args, count)
        })
    }
}

unsafe extern "C" fn call_method(
    call: TorqueCall,
    object: u32,
    name: TorqueStr,
    args: *const TorqueStr,
    count: usize,
    out: *mut TorqueStr,
) -> TorqueStatus {
    if count > TORQUE_MAX_ARGS {
        return TORQUE_ERR_RANGE;
    }
    // SAFETY: as in `call_function`.
    unsafe {
        with_text(call, out, |page| {
            page_call_method(page, object, name.ptr, name.len, args, count)
        })
    }
}

unsafe extern "C" fn has_method(
    call: TorqueCall,
    object: u32,
    name: TorqueStr,
    out: *mut u32,
) -> TorqueStatus {
    let Some(page) = page_handle(call) else {
        return TORQUE_ERR_HANDLE;
    };
    // SAFETY: the name is valid during the call; `out` is the extension's place for the answer.
    unsafe {
        let has = page_has_method(page, object, name.ptr, name.len);
        put(out, u32::from(has != 0))
    }
}

unsafe extern "C" fn find_object(call: TorqueCall, name: TorqueStr, out: *mut u32) -> TorqueStatus {
    let Some(page) = page_handle(call) else {
        return TORQUE_ERR_HANDLE;
    };
    let mut id = 0;
    // SAFETY: the name is valid during the call; `id` is a place for the answer.
    let status = unsafe { page_find_object(page, name.ptr, name.len, &mut id) };
    if status != TORQUE_OK {
        return status;
    }
    // SAFETY: the extension's place for the id.
    unsafe { put(out, id) }
}

unsafe extern "C" fn get_field(
    call: TorqueCall,
    object: u32,
    name: TorqueStr,
    index: TorqueStr,
    out: *mut TorqueStr,
) -> TorqueStatus {
    // SAFETY: the texts are valid during the call; `out` is the extension's place for the value.
    unsafe {
        with_text(call, out, |page| {
            page_get_field(page, object, name.ptr, name.len, index.ptr, index.len)
        })
    }
}

unsafe extern "C" fn set_field(
    call: TorqueCall,
    object: u32,
    name: TorqueStr,
    index: TorqueStr,
    value: TorqueStr,
) -> TorqueStatus {
    let Some(page) = page_handle(call) else {
        return TORQUE_ERR_HANDLE;
    };
    // SAFETY: the texts are valid during the call; the page copies them.
    unsafe {
        page_set_field(
            page, object, name.ptr, name.len, index.ptr, index.len, value.ptr, value.len,
        )
    }
}

unsafe extern "C" fn define_function(
    call: TorqueCall,
    decl: *const TorqueDefineDecl,
) -> TorqueStatus {
    if decl.is_null() || !decl.is_aligned() {
        return TORQUE_ERR_INVALID;
    }
    let Some(page) = page_handle(call) else {
        return TORQUE_ERR_HANDLE;
    };
    // SAFETY: the declaration and its texts are valid during the call; the page copies them.
    unsafe { page_define_function(page, decl) }
}

unsafe extern "C" fn charge(call: TorqueCall, steps: u64) -> TorqueStatus {
    let Some(page) = page_handle(call) else {
        return TORQUE_ERR_HANDLE;
    };
    // SAFETY: plain values.
    unsafe { page_charge(page, steps) }
}

unsafe extern "C" fn vm_info(call: TorqueCall, out: *mut TorqueVmInfo) -> TorqueStatus {
    with_frame(call, |frame| {
        let info = TorqueVmInfo {
            vm: frame.vm,
            side: frame.side,
            // The engine's side keeps the budget (`charge`); the extension never reads the limit itself.
            step_limit: 0,
        };
        // SAFETY: the extension's place for the info.
        unsafe { put(out, info) }
    })
}

unsafe extern "C" fn script_package(call: TorqueCall, out: *mut TorqueStr) -> TorqueStatus {
    with_frame(call, |frame| {
        frame.scratch = frame.package.clone();
        // SAFETY: the extension's place for the text, which points into the frame as the ABI says.
        unsafe { put(out, text(&frame.scratch)) }
    })
}

unsafe extern "C" fn read_script(
    call: TorqueCall,
    path: TorqueStr,
    out: *mut TorqueStr,
) -> TorqueStatus {
    // SAFETY: the path is valid during the call; `out` is the extension's place for the text.
    unsafe { with_text(call, out, |page| page_read_script(page, path.ptr, path.len)) }
}

/// The table the extension runs on.
static API: TorqueApi = TorqueApi {
    abi_version: TORQUE_ABI_VERSION,
    size: std::mem::size_of::<TorqueApi>() as u32,
    init: INIT_HANDLE,
    register_function: register_native,
    register_method: register_native,
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
    call_function,
    call_method,
    has_method,
    find_object,
    get_field,
    set_field,
    define_function,
    charge,
    vm_info,
    script_package,
    read_script,
    register_script_language,
};

/// The bytes the page put at `ptr`.
///
/// # Safety
///
/// `ptr` is null with `len` 0, or `len` bytes the page wrote into this module's memory and keeps until the call returns.
unsafe fn page_bytes<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    // SAFETY: the caller's promise.
    unsafe { bytes_of(TorqueStr { ptr, len }) }
}

/// Runs `f` as the page's call `call`: its frame is pushed for the time `f` runs.
fn as_call(frame: Frame, f: impl FnOnce() -> TorqueStatus) -> (TorqueStatus, Frame) {
    let handle = frame.handle;
    FRAMES.with(|frames| frames.borrow_mut().push(frame));
    let status = f();
    let frame = FRAMES.with(|frames| {
        let mut frames = frames.borrow_mut();
        let at = frames.iter().rposition(|f| f.handle == handle);
        at.map(|at| frames.remove(at))
    });
    let frame = frame.unwrap_or_else(|| Frame::new(handle, 0, 0));
    (status, frame)
}

impl Frame {
    fn new(handle: TorqueCall, vm: TorqueVm, side: u32) -> Frame {
        Frame {
            handle,
            vm,
            side,
            package: Vec::new(),
            args: Vec::new(),
            scratch: Vec::new(),
            result: Vec::new(),
        }
    }
}

/// The extension's entry point: registers its languages. Called once, after the module's `_initialize`.
#[unsafe(no_mangle)]
pub extern "C" fn torque_lua_start() -> TorqueStatus {
    if LANGUAGES.with(|l| !l.borrow().is_empty()) {
        return TORQUE_OK;
    }
    let mut info = TorquePluginInfo {
        abi_version: 0,
        id: text(&[]),
        version: text(&[]),
        net_version: 0,
        has_net_version: 0,
    };
    // SAFETY: the table is this module's own, valid for its life; `info` is a valid place for the extension's info.
    unsafe { crate::INIT(&API, &mut info) }
}

/// A place of `len` bytes for the page to write a text into, freed with [`torque_lua_free`].
#[unsafe(no_mangle)]
pub extern "C" fn torque_lua_alloc(len: usize) -> *mut u8 {
    Box::into_raw(vec![0u8; len.max(1)].into_boxed_slice()).cast::<u8>()
}

/// Frees a place [`torque_lua_alloc`] made.
///
/// # Safety
///
/// `ptr` and `len` are one place `torque_lua_alloc(len)` returned, not freed before.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn torque_lua_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: the caller's promise: a boxed slice of `len.max(1)` bytes made by `torque_lua_alloc`.
    drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len.max(1))) });
}

/// Runs the file `path` (its text `source`) as the page's call `call`, in the script engine `vm` of `side`, its
/// definitions going into `package`: the status of the extension's `run_file`.
///
/// # Safety
///
/// Each pointer and length is a text the page wrote into this module's memory, kept until the call returns.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn torque_lua_run_file(
    call: u32,
    vm: TorqueVm,
    side: u32,
    path: *const u8,
    path_len: usize,
    source: *const u8,
    source_len: usize,
    package: *const u8,
    package_len: usize,
) -> TorqueStatus {
    // SAFETY: the caller's promise.
    let (Some(path), Some(source), Some(package)) = (unsafe {
        (
            page_bytes(path, path_len),
            page_bytes(source, source_len),
            page_bytes(package, package_len),
        )
    }) else {
        return TORQUE_ERR_INVALID;
    };
    let extension = path
        .iter()
        .rposition(|&b| b == b'.')
        .map(|at| path[at + 1..].to_ascii_lowercase())
        .unwrap_or_default();
    let Some(language) = LANGUAGES.with(|l| {
        l.borrow()
            .iter()
            .find(|l| l.extension() == extension.as_slice())
            .copied()
    }) else {
        return TORQUE_ERR_FAILED;
    };
    let Some(run) = language.decl.run_file else {
        return TORQUE_ERR_FAILED;
    };
    let mut frame = Frame::new(TorqueCall::from(call), vm, side);
    frame.package = package.to_vec();
    let (status, _) = as_call(frame, || {
        // SAFETY: the extension's own function and pointer, with texts valid during the call.
        unsafe {
            run(
                language.decl.user,
                TorqueCall::from(call),
                text(path),
                text(source),
            )
        }
    });
    status
}

/// Calls the function the extension defined with `id` as the page's call `call` (in the script engine `vm` of `side`),
/// with the `count` texts at `args`: the status of the extension's `call_defined`, its result in
/// [`torque_lua_result`].
///
/// # Safety
///
/// `args` is null with `count` 0, or `count` texts the page wrote into this module's memory, kept until the call
/// returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn torque_lua_call_defined(
    call: u32,
    vm: TorqueVm,
    side: u32,
    id: u64,
    args: *const TorqueStr,
    count: usize,
) -> TorqueStatus {
    if count > TORQUE_MAX_ARGS {
        return TORQUE_ERR_RANGE;
    }
    let given: &[TorqueStr] = if count == 0 {
        &[]
    } else if args.is_null() || !args.is_aligned() {
        return TORQUE_ERR_INVALID;
    } else {
        // SAFETY: not null, aligned, `count` texts by the caller's promise.
        unsafe { std::slice::from_raw_parts(args, count) }
    };
    let mut frame = Frame::new(TorqueCall::from(call), vm, side);
    for arg in given {
        // SAFETY: the caller's promise; copied.
        let Some(bytes) = (unsafe { bytes_of(*arg) }) else {
            return TORQUE_ERR_INVALID;
        };
        // An argument up to its first NUL, as the engine hands a native's.
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        frame.args.push(bytes[..end].to_vec());
    }
    let Some(language) = LANGUAGES.with(|l| l.borrow().first().copied()) else {
        return TORQUE_ERR_FAILED;
    };
    let Some(defined) = language.decl.call_defined else {
        return TORQUE_ERR_FAILED;
    };
    let (status, frame) = as_call(frame, || {
        // SAFETY: the extension's own function and pointer.
        unsafe { defined(language.decl.user, TorqueCall::from(call), id) }
    });
    RESULT.with(|r| *r.borrow_mut() = frame.result);
    status
}

/// Where the last [`torque_lua_call_defined`]'s result is.
#[unsafe(no_mangle)]
pub extern "C" fn torque_lua_result() -> *const u8 {
    RESULT.with(|r| r.borrow().as_ptr())
}

/// How long the last [`torque_lua_call_defined`]'s result is.
#[unsafe(no_mangle)]
pub extern "C" fn torque_lua_result_len() -> usize {
    RESULT.with(|r| r.borrow().len())
}

/// The script engine `vm` is gone, and so is its realm.
#[unsafe(no_mangle)]
pub extern "C" fn torque_lua_vm_dropped(vm: TorqueVm) {
    let languages = LANGUAGES.with(|l| l.borrow().clone());
    for language in languages {
        if let Some(dropped) = language.decl.vm_dropped {
            // SAFETY: the extension's own function and pointer.
            unsafe { dropped(language.decl.user, vm) };
        }
    }
}
