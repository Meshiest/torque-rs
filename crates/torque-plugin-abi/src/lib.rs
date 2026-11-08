//! The C ABI between the engine and a native extension: a library (`.dll`, `.so`, `.dylib`) the engine loads from a local
//! `plugins/` folder, written in Rust with `torque-plugin-sdk` or in any language that can export C functions.
//!
//! An extension exports two symbols:
//!
//! - `torque_plugin_abi_version`, a `uint32_t` constant: the [`TORQUE_ABI_VERSION`] it was built against. The engine reads
//!   it from the library's file (its export table), before loading it, and refuses a library whose version is not its own.
//! - `torque_plugin_init`, a [`TorquePluginInitFn`]: called once with the engine's [`TorqueApi`]. It fills in a
//!   [`TorquePluginInfo`] (its id, its version and the ABI version again) and registers what it adds through the table's
//!   `register_*` functions, naming [`TorqueApi::init`]. It returns [`TORQUE_OK`], or any other status to refuse to load.
//!
//! Nothing but these `#[repr(C)]` types crosses the boundary: text is a [`TorqueStr`] (a pointer and a length, not
//! NUL-terminated), and the engine's objects are opaque handles ([`TorqueInit`], [`TorqueCall`], [`TorqueDraw`]) that are
//! live only during the call that hands them over. Every function of the table returns a [`TorqueStatus`] and checks what
//! it is given (null pointers, lengths, UTF-8, a handle that is not live), so a mistake is a status, not a crash.
//!
//! **Threads.** The engine may call into an extension (a console function, a pass) from any of its threads, but never twice
//! at once: it makes one call into an extension at a time, across all its threads, and refuses a call that would re-enter
//! the extension on the thread that is already inside it. So an extension's own state needs no lock against the engine,
//! but it may not count on one thread, or on thread-local storage, staying the same from call to call.
//!
//! **Calling scripts back.** During a call, an extension may run the script engine's functions and methods
//! ([`TorqueApi::call_function`], [`TorqueApi::call_method`]). A script run that way may call the extension again: that
//! nested call is the one re-entry the engine allows, on the same thread, at most [`TORQUE_MAX_NESTING`] deep, and the
//! extension must be ready for it (hold no lock of its own across a call back). The script engine's step limit and call
//! depth hold across the nesting: a run that reaches one ends with [`TORQUE_ERR_LIMIT`], which the extension hands back
//! from its own call so that the script that started it ends too.
//!
//! **Script languages.** An extension may run files of another language ([`TorqueApi::register_script_language`]): `exec`
//! hands a file with its extension (`.lua`, say) to it, after the same checks as a script's, so a file a server sent is
//! never run. Functions the language defines at run time ([`TorqueApi::define_function`]) come back to it by an id.
//!
//! **Versioning.** [`TORQUE_ABI_VERSION`] is one number. Any change to a type, a function's signature or the meaning of a
//! constant here raises it, and the engine loads only extensions built for the version it speaks: there is no partial
//! compatibility to get wrong. [`TorqueApi::size`] is the table's size, so a later version that only appends functions
//! can still be told apart.
//!
//! The C header `include/torque_plugin.h` is generated from this file (`just c-header`; CI checks it is up to date).

#![no_std]
#![warn(missing_docs)]

use core::ffi::c_void;

/// The version of this ABI. An extension reports the version it was built against, and the engine loads only its own.
pub const TORQUE_ABI_VERSION: u32 = 1;

/// What every function across the boundary returns: [`TORQUE_OK`] or one of the `TORQUE_ERR_` codes.
pub type TorqueStatus = u32;

/// It worked.
pub const TORQUE_OK: TorqueStatus = 0;
/// The other side speaks another ABI version.
pub const TORQUE_ERR_ABI: TorqueStatus = 1;
/// A value was refused: a null pointer, a length over the limit, text that is not UTF-8, an unknown kind, stage or flag.
pub const TORQUE_ERR_INVALID: TorqueStatus = 2;
/// The handle is not live: it belongs to a call that has returned, or to another thread.
pub const TORQUE_ERR_HANDLE: TorqueStatus = 3;
/// The code that was called panicked (the extension is disabled when its own code does).
pub const TORQUE_ERR_PANIC: TorqueStatus = 4;
/// The request was understood and refused: a name already taken, an extension that is disabled, an init that failed.
pub const TORQUE_ERR_FAILED: TorqueStatus = 5;
/// An index past the end, or more than a limit allows (the arguments of a call, the vertices of a frame).
pub const TORQUE_ERR_RANGE: TorqueStatus = 6;
/// The run stopped at a limit: the script engine's step budget or call depth, or [`TORQUE_MAX_NESTING`]. Hand it back from
/// the extension's own call, so the script that called it stops too.
pub const TORQUE_ERR_LIMIT: TorqueStatus = 7;

/// The handle of one `torque_plugin_init` call: what the `register_*` functions name. Live only during that call.
pub type TorqueInit = u64;
/// The handle of one call of a console function or method: what the `arg_*`, `return_*` and global functions name. Live
/// only during that call.
pub type TorqueCall = u64;
/// The handle of one pass's drawing in one frame: what [`TorqueApi::draw_triangles`] names. Live only during that call.
pub type TorqueDraw = u64;
/// A script engine (one server's or one client's): the same number for its whole life, never reused for another.
pub type TorqueVm = u64;

/// Text: `len` bytes at `ptr`, not NUL-terminated. `ptr` may be null only when `len` is 0. Names, ids, versions and usage
/// texts must be UTF-8; script values are bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TorqueStr {
    /// The first byte.
    pub ptr: *const u8,
    /// How many bytes.
    pub len: usize,
}

/// The longest name, id, version or namespace the engine takes (bytes).
pub const TORQUE_MAX_NAME: usize = 256;
/// The longest usage text, description, docs or log message the engine takes (bytes).
pub const TORQUE_MAX_TEXT: usize = 64 * 1024;
/// The longest script value (a return value, a global's value) the engine takes (bytes).
pub const TORQUE_MAX_VALUE: usize = 1024 * 1024;
/// The most script arguments a console function or method declares.
pub const TORQUE_MAX_ARGS: usize = 32;
/// The most console functions, methods, globals and passes one extension registers.
pub const TORQUE_MAX_REGISTRATIONS: usize = 1024;
/// The most vertices one pass draws in one frame (the rest are refused with [`TORQUE_ERR_RANGE`]).
pub const TORQUE_MAX_PASS_VERTICES: usize = 1 << 20;
/// How deep calls back into one extension may nest on one thread ([`TorqueApi::call_function`] running a script that
/// calls the extension again); the call past it fails with [`TORQUE_ERR_LIMIT`].
pub const TORQUE_MAX_NESTING: usize = 64;
/// The steps an outermost call into an extension may [`charge`](TorqueApi::charge) when its script engine has no step
/// limit of its own.
pub const TORQUE_DEFAULT_STEPS: u64 = 50_000_000;
/// The longest script file [`TorqueApi::read_script`] reads (bytes).
pub const TORQUE_MAX_SCRIPT: usize = 16 * 1024 * 1024;

/// Which script engines a registration goes into: every server's and every client's.
pub const TORQUE_SIDE_BOTH: u32 = 0;
/// Every server's script engine.
pub const TORQUE_SIDE_SERVER: u32 = 1;
/// Every client's script engine.
pub const TORQUE_SIDE_CLIENT: u32 = 2;

/// A script argument the script must pass.
pub const TORQUE_ARG_REQUIRED: u32 = 0;
/// A script argument the script may leave out: only after the required ones.
pub const TORQUE_ARG_OPTIONAL: u32 = 1;
/// Every argument left: the last one.
pub const TORQUE_ARG_REST: u32 = 2;

/// A development command: it runs in a debug build of the engine, or once `$Pref::Dev` is true.
pub const TORQUE_NATIVE_DEV: u32 = 1;
/// [`TorqueNativeDecl::usage`] is the whole usage text, kept as written (none is made from the arguments).
pub const TORQUE_NATIVE_EXACT_USAGE: u32 = 2;

/// A message for the console or the engine's log.
pub const TORQUE_LOG_INFO: u32 = 0;
/// A warning.
pub const TORQUE_LOG_WARN: u32 = 1;
/// An error.
pub const TORQUE_LOG_ERROR: u32 = 2;

/// One script argument of a console function or method: its name in the usage text and its kind (`TORQUE_ARG_`).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TorqueArg {
    /// The name the usage text shows.
    pub name: TorqueStr,
    /// [`TORQUE_ARG_REQUIRED`], [`TORQUE_ARG_OPTIONAL`] or [`TORQUE_ARG_REST`].
    pub kind: u32,
}

/// A console function or method of the extension: called with the `user` pointer it was registered with and the call's
/// handle. It reads its arguments and sets its result through the [`TorqueApi`], and returns [`TORQUE_OK`] (or
/// [`TORQUE_ERR_PANIC`] when it caught a panic of its own: the engine then disables the extension).
pub type TorqueNativeFn = unsafe extern "C" fn(user: *mut c_void, call: TorqueCall) -> TorqueStatus;

/// A console function (`register_function`) or method (`register_method`) the extension declares.
///
/// The engine copies everything it needs before the register call returns, except `user`, which it hands back to `call`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TorqueNativeDecl {
    /// [`TORQUE_SIDE_BOTH`], [`TORQUE_SIDE_SERVER`] or [`TORQUE_SIDE_CLIENT`].
    pub side: u32,
    /// A function's namespace (empty for the global one), or a method's class.
    pub ns: TorqueStr,
    /// The name scripts call.
    pub name: TorqueStr,
    /// The script arguments, `arg_count` of them (may be null when `arg_count` is 0).
    pub args: *const TorqueArg,
    /// How many script arguments.
    pub arg_count: usize,
    /// The usage text's description, after the signature made from the arguments (empty for none).
    pub description: TorqueStr,
    /// With [`TORQUE_NATIVE_EXACT_USAGE`]: the whole usage text. Otherwise ignored.
    pub usage: TorqueStr,
    /// The whole doc comment, for the script reference (empty for none).
    pub docs: TorqueStr,
    /// [`TORQUE_NATIVE_DEV`] and [`TORQUE_NATIVE_EXACT_USAGE`], or 0.
    pub flags: u32,
    /// The function the engine calls (a [`TorqueNativeFn`]).
    pub call: Option<unsafe extern "C" fn(user: *mut c_void, call: TorqueCall) -> TorqueStatus>,
    /// Handed back to `call` as it is; never read by the engine.
    pub user: *mut c_void,
}

/// The frame's stage after the opaque pass: world-space triangles go into the HDR frame, depth tested and written.
pub const TORQUE_STAGE_OPAQUE: u32 = 2;
/// The stage after the translucent passes: world-space triangles go into the HDR frame, depth tested and written.
pub const TORQUE_STAGE_TRANSLUCENT: u32 = 3;
/// The stage after the tonemap: triangles go over the finished picture, with no depth.
pub const TORQUE_STAGE_OVERLAY: u32 = 6;

/// Vertices in world units (the engine's z-up world), seen through [`TorquePassFrame::view_proj`].
pub const TORQUE_SPACE_WORLD: u32 = 0;
/// Vertices in clip space (x and y from -1 to 1 across the stage's viewport, y up; z is ignored at the overlay).
pub const TORQUE_SPACE_CLIP: u32 = 1;

/// What a pass is told about the frame it draws in.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TorquePassFrame {
    /// The stage (`TORQUE_STAGE_`).
    pub stage: u32,
    /// The size of the stage's target, in pixels.
    pub size: [u32; 2],
    /// Where the world is in the target: left, top, width, height.
    pub viewport: [u32; 4],
    /// The camera's view and projection, world to clip, column major.
    pub view_proj: [f32; 16],
    /// The camera's position in the world.
    pub eye: [f32; 3],
    /// The scene's effects time, in seconds.
    pub time: f32,
}

/// One vertex of a pass's triangles: a position (in the space the draw call names) and a linear RGBA color.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TorqueVertex {
    /// The position.
    pub position: [f32; 3],
    /// The color, red, green, blue and alpha from 0 to 1 (alpha blends over what is there).
    pub color: [f32; 4],
}

/// Makes one instance of a pass, for one view of the world: its state, written to `out` and handed to `draw` and `drop`.
/// `draw` is a live handle for [`TorqueApi::log`] during the call. [`TORQUE_OK`] with the instance; [`TORQUE_ERR_PANIC`]
/// when it caught a panic of its own (the engine disables the extension); any other status when it made none.
pub type TorquePassCreateFn = unsafe extern "C" fn(
    user: *mut c_void,
    draw: TorqueDraw,
    out: *mut *mut c_void,
) -> TorqueStatus;
/// Draws one frame of a pass instance: triangles through [`TorqueApi::draw_triangles`] with `draw`.
pub type TorquePassDrawFn = unsafe extern "C" fn(
    pass: *mut c_void,
    frame: *const TorquePassFrame,
    draw: TorqueDraw,
) -> TorqueStatus;
/// Frees a pass instance when its view goes.
pub type TorquePassDropFn = unsafe extern "C" fn(pass: *mut c_void);

/// A render pass the extension declares (`register_render_pass`): each view of the world makes an instance with `create`
/// and draws it at `stage` of every frame.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TorquePassDecl {
    /// [`TORQUE_STAGE_OPAQUE`], [`TORQUE_STAGE_TRANSLUCENT`] or [`TORQUE_STAGE_OVERLAY`].
    pub stage: u32,
    /// Makes an instance (a [`TorquePassCreateFn`]).
    pub create: Option<
        unsafe extern "C" fn(
            user: *mut c_void,
            draw: TorqueDraw,
            out: *mut *mut c_void,
        ) -> TorqueStatus,
    >,
    /// Draws an instance (a [`TorquePassDrawFn`]).
    pub draw: Option<
        unsafe extern "C" fn(
            pass: *mut c_void,
            frame: *const TorquePassFrame,
            draw: TorqueDraw,
        ) -> TorqueStatus,
    >,
    /// Frees an instance (a [`TorquePassDropFn`]; may be null).
    pub drop: Option<unsafe extern "C" fn(pass: *mut c_void)>,
    /// Handed to `create` as it is; never read by the engine.
    pub user: *mut c_void,
}

/// What [`TorqueApi::vm_info`] tells about the script engine of a call.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TorqueVmInfo {
    /// The script engine.
    pub vm: TorqueVm,
    /// [`TORQUE_SIDE_SERVER`] or [`TORQUE_SIDE_CLIENT`]; [`TORQUE_SIDE_BOTH`] for one the engine made outside a server or a
    /// client (a tool's).
    pub side: u32,
    /// Its step limit for one outermost run, or 0 when it has none ([`TORQUE_DEFAULT_STEPS`] then bounds an outermost
    /// call into the extension).
    pub step_limit: u64,
}

/// Runs the file `path` of the language, whose text is `source`: called by `exec`, inside a run of the script engine, with
/// a live call handle. [`TORQUE_OK`] when it ran (a script error it printed is still a run); [`TORQUE_ERR_LIMIT`] when a
/// limit stopped it; [`TORQUE_ERR_PANIC`] when it caught a panic of its own.
pub type TorqueRunFileFn = unsafe extern "C" fn(
    user: *mut c_void,
    call: TorqueCall,
    path: TorqueStr,
    source: TorqueStr,
) -> TorqueStatus;
/// Calls the function the language defined with `id` ([`TorqueApi::define_function`]): a console function's call, with
/// its arguments and result as a native's.
pub type TorqueCallDefinedFn =
    unsafe extern "C" fn(user: *mut c_void, call: TorqueCall, id: u64) -> TorqueStatus;
/// The script engine `vm` is gone: what the language kept for it can go too. No call names it again.
pub type TorqueVmDroppedFn = unsafe extern "C" fn(user: *mut c_void, vm: TorqueVm);

/// A script language the extension runs (`register_script_language`).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TorqueLanguageDecl {
    /// [`TORQUE_SIDE_BOTH`], [`TORQUE_SIDE_SERVER`] or [`TORQUE_SIDE_CLIENT`]: which script engines run its files.
    pub side: u32,
    /// The file extension it runs, without the dot (`lua`): ASCII letters and digits, not one of TorqueScript's (`cs`,
    /// `dso`, `gui`, `mis`).
    pub extension: TorqueStr,
    /// Runs a file (a [`TorqueRunFileFn`]).
    pub run_file: Option<
        unsafe extern "C" fn(
            user: *mut c_void,
            call: TorqueCall,
            path: TorqueStr,
            source: TorqueStr,
        ) -> TorqueStatus,
    >,
    /// Calls a function it defined (a [`TorqueCallDefinedFn`]; may be null when it defines none).
    pub call_defined:
        Option<unsafe extern "C" fn(user: *mut c_void, call: TorqueCall, id: u64) -> TorqueStatus>,
    /// Told when a script engine goes (a [`TorqueVmDroppedFn`]; may be null).
    pub vm_dropped: Option<unsafe extern "C" fn(user: *mut c_void, vm: TorqueVm)>,
    /// Handed to the functions as it is; never read by the engine.
    pub user: *mut c_void,
}

/// A console function a script language defines at run time, in the call's script engine only (`define_function`).
/// Calls of it go to the language's `call_defined` with `id`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TorqueDefineDecl {
    /// The namespace (empty for the global one) or class: `Player` makes `Player::name`, which a method call on a player
    /// finds, with the object first among the arguments.
    pub ns: TorqueStr,
    /// The name scripts call.
    pub name: TorqueStr,
    /// The package it goes into, so that deactivating the package hides it again: empty for none, else one
    /// [`TorqueApi::script_package`] gave the language in this script engine.
    pub package: TorqueStr,
    /// Its usage text (empty for none).
    pub usage: TorqueStr,
    /// Handed back to `call_defined`.
    pub id: u64,
}

/// `register_function` and `register_method`.
pub type TorqueRegisterNativeFn =
    unsafe extern "C" fn(init: TorqueInit, decl: *const TorqueNativeDecl) -> TorqueStatus;
/// `register_global`.
pub type TorqueRegisterGlobalFn = unsafe extern "C" fn(
    init: TorqueInit,
    side: u32,
    name: TorqueStr,
    default_value: TorqueStr,
) -> TorqueStatus;
/// `register_render_pass`.
pub type TorqueRegisterPassFn =
    unsafe extern "C" fn(init: TorqueInit, decl: *const TorquePassDecl) -> TorqueStatus;
/// `arg_count`.
pub type TorqueArgCountFn = unsafe extern "C" fn(call: TorqueCall, out: *mut u32) -> TorqueStatus;
/// `arg_str`.
pub type TorqueArgStrFn =
    unsafe extern "C" fn(call: TorqueCall, index: u32, out: *mut TorqueStr) -> TorqueStatus;
/// `arg_int`.
pub type TorqueArgIntFn =
    unsafe extern "C" fn(call: TorqueCall, index: u32, out: *mut i32) -> TorqueStatus;
/// `arg_float`.
pub type TorqueArgFloatFn =
    unsafe extern "C" fn(call: TorqueCall, index: u32, out: *mut f32) -> TorqueStatus;
/// `arg_bool`.
pub type TorqueArgBoolFn =
    unsafe extern "C" fn(call: TorqueCall, index: u32, out: *mut u32) -> TorqueStatus;
/// `this_object`.
pub type TorqueThisObjectFn = unsafe extern "C" fn(call: TorqueCall, out: *mut u32) -> TorqueStatus;
/// `return_str` and `return_error`.
pub type TorqueReturnStrFn =
    unsafe extern "C" fn(call: TorqueCall, value: TorqueStr) -> TorqueStatus;
/// `return_int`.
pub type TorqueReturnIntFn = unsafe extern "C" fn(call: TorqueCall, value: i32) -> TorqueStatus;
/// `return_float`.
pub type TorqueReturnFloatFn = unsafe extern "C" fn(call: TorqueCall, value: f32) -> TorqueStatus;
/// `return_bool`.
pub type TorqueReturnBoolFn = unsafe extern "C" fn(call: TorqueCall, value: u32) -> TorqueStatus;
/// `get_global`.
pub type TorqueGetGlobalFn =
    unsafe extern "C" fn(call: TorqueCall, name: TorqueStr, out: *mut TorqueStr) -> TorqueStatus;
/// `set_global`.
pub type TorqueSetGlobalFn =
    unsafe extern "C" fn(call: TorqueCall, name: TorqueStr, value: TorqueStr) -> TorqueStatus;
/// `draw_triangles`.
pub type TorqueDrawTrianglesFn = unsafe extern "C" fn(
    draw: TorqueDraw,
    space: u32,
    vertices: *const TorqueVertex,
    count: usize,
) -> TorqueStatus;
/// `log`.
pub type TorqueLogFn =
    unsafe extern "C" fn(handle: u64, level: u32, message: TorqueStr) -> TorqueStatus;
/// `call_function`.
pub type TorqueCallFunctionFn = unsafe extern "C" fn(
    call: TorqueCall,
    ns: TorqueStr,
    name: TorqueStr,
    args: *const TorqueStr,
    count: usize,
    out: *mut TorqueStr,
) -> TorqueStatus;
/// `call_method`.
pub type TorqueCallMethodFn = unsafe extern "C" fn(
    call: TorqueCall,
    object: u32,
    name: TorqueStr,
    args: *const TorqueStr,
    count: usize,
    out: *mut TorqueStr,
) -> TorqueStatus;
/// `has_method`.
pub type TorqueHasMethodFn = unsafe extern "C" fn(
    call: TorqueCall,
    object: u32,
    name: TorqueStr,
    out: *mut u32,
) -> TorqueStatus;
/// `find_object`.
pub type TorqueFindObjectFn =
    unsafe extern "C" fn(call: TorqueCall, name: TorqueStr, out: *mut u32) -> TorqueStatus;
/// `get_field`.
pub type TorqueGetFieldFn = unsafe extern "C" fn(
    call: TorqueCall,
    object: u32,
    name: TorqueStr,
    index: TorqueStr,
    out: *mut TorqueStr,
) -> TorqueStatus;
/// `set_field`.
pub type TorqueSetFieldFn = unsafe extern "C" fn(
    call: TorqueCall,
    object: u32,
    name: TorqueStr,
    index: TorqueStr,
    value: TorqueStr,
) -> TorqueStatus;
/// `define_function`.
pub type TorqueDefineFunctionFn =
    unsafe extern "C" fn(call: TorqueCall, decl: *const TorqueDefineDecl) -> TorqueStatus;
/// `charge`.
pub type TorqueChargeFn = unsafe extern "C" fn(call: TorqueCall, steps: u64) -> TorqueStatus;
/// `vm_info`.
pub type TorqueVmInfoFn =
    unsafe extern "C" fn(call: TorqueCall, out: *mut TorqueVmInfo) -> TorqueStatus;
/// `script_package`.
pub type TorqueScriptPackageFn =
    unsafe extern "C" fn(call: TorqueCall, out: *mut TorqueStr) -> TorqueStatus;
/// `read_script`.
pub type TorqueReadScriptFn =
    unsafe extern "C" fn(call: TorqueCall, path: TorqueStr, out: *mut TorqueStr) -> TorqueStatus;
/// `register_script_language`.
pub type TorqueRegisterLanguageFn =
    unsafe extern "C" fn(init: TorqueInit, decl: *const TorqueLanguageDecl) -> TorqueStatus;

/// The engine's function table, handed to `torque_plugin_init`. It stays valid while the extension is loaded, which is
/// until the process ends: keep the pointer.
///
/// `abi_version` and `size` come first in every version: read them before anything else, and give up on a version that is
/// not the one the extension was built for.
///
/// **Threads.** The engine may call into an extension (a console function, a pass) from any of its threads, but never twice
/// at once: it makes one call into an extension at a time, across all its threads, and refuses a call that would re-enter
/// the extension on the thread that is already inside it. So an extension's own state needs no lock against the engine,
/// but it may not count on one thread, or on thread-local storage, staying the same from call to call.
#[repr(C)]
pub struct TorqueApi {
    /// The engine's [`TORQUE_ABI_VERSION`].
    pub abi_version: u32,
    /// The size of this table in bytes.
    pub size: u32,
    /// This init's handle, for the `register_*` functions.
    pub init: TorqueInit,

    /// Declares a console function (only during `torque_plugin_init`).
    pub register_function: TorqueRegisterNativeFn,
    /// Declares a console method of the class `decl.ns`: its object is `this_object` (only during init).
    pub register_method: TorqueRegisterNativeFn,
    /// Declares a global variable (`name` with its `$`) set to `default_value` before scripts run (only during init).
    pub register_global: TorqueRegisterGlobalFn,
    /// Declares a render pass (only during init).
    pub register_render_pass: TorqueRegisterPassFn,

    /// How many script arguments the call has (not counting the name or a method's object).
    pub arg_count: TorqueArgCountFn,
    /// Script argument `index` (from 0) as bytes, valid until the call returns.
    pub arg_str: TorqueArgStrFn,
    /// Script argument `index` as an integer, by the script engine's rules (`dAtoi`).
    pub arg_int: TorqueArgIntFn,
    /// Script argument `index` as a float, by the script engine's rules (`dAtof`, rounded to 32 bits).
    pub arg_float: TorqueArgFloatFn,
    /// Script argument `index` as a truth value (1 or 0), by the script engine's rules (`dAtob`).
    pub arg_bool: TorqueArgBoolFn,
    /// A method call's object id.
    pub this_object: TorqueThisObjectFn,
    /// Sets the call's result to text (copied).
    pub return_str: TorqueReturnStrFn,
    /// Sets the call's result to an integer.
    pub return_int: TorqueReturnIntFn,
    /// Sets the call's result to a float.
    pub return_float: TorqueReturnFloatFn,
    /// Sets the call's result to a truth value (any value but 0 is true).
    pub return_bool: TorqueReturnBoolFn,
    /// Fails the call: the console prints `message` as an error with the function's name and its usage, and the call
    /// returns the empty string.
    pub return_error: TorqueReturnStrFn,
    /// A global variable's value (`name` with its `$`), valid until the next `get_global` or the call's end; empty when
    /// it is not set.
    pub get_global: TorqueGetGlobalFn,
    /// Sets a global variable (`name` with its `$`).
    pub set_global: TorqueSetGlobalFn,

    /// Draws triangles in a pass's frame: `count` vertices (a multiple of 3) in `space` (`TORQUE_SPACE_`), copied.
    pub draw_triangles: TorqueDrawTrianglesFn,

    /// A message (`TORQUE_LOG_`): to the running script engine's console during a call, to the engine's log otherwise.
    /// `handle` is the live init, call or draw handle, or 0.
    pub log: TorqueLogFn,

    // The functions below run scripts and script languages; `size` tells a table that has them.
    /// Runs the console function `ns::name` (`ns` empty for a global one) with `count` arguments (at most
    /// [`TORQUE_MAX_ARGS`]) and writes its result to `out`, valid until the next function of this table the extension
    /// calls with this call handle, or the call's end. A function that does not exist gives the empty string (a global one
    /// warns, as a script's call does). The run shares the call's budgets: [`TORQUE_ERR_LIMIT`] when it stopped at one.
    pub call_function: TorqueCallFunctionFn,
    /// Runs the method `name` of `object` (its `%this` first, then the arguments), as
    /// [`call_function`](Self::call_function) does; the empty string when the object or the method does not exist.
    pub call_method: TorqueCallMethodFn,
    /// Whether `object` has a method `name` (its class's or its namespace's, script or native): 1 or 0 to `out`.
    pub has_method: TorqueHasMethodFn,
    /// The object `name` names (a name, an id, or a `/` path, as a script's `nameToID`) to `out`; [`TORQUE_ERR_FAILED`]
    /// when there is none.
    pub find_object: TorqueFindObjectFn,
    /// The field `name[index]` of `object` (`index` empty for a plain field), as a script reads it, to `out` (valid as
    /// [`call_function`](Self::call_function)'s result); [`TORQUE_ERR_FAILED`] when the object does not exist.
    pub get_field: TorqueGetFieldFn,
    /// Sets the field `name[index]` of `object`, as a script's assignment does; [`TORQUE_ERR_FAILED`] when the object
    /// does not exist.
    pub set_field: TorqueSetFieldFn,
    /// Defines a console function in the call's script engine whose calls go to the call's script language
    /// ([`TorqueLanguageDecl::call_defined`]): only during a call of a language's `run_file` or `call_defined`. Defining
    /// a name again replaces it.
    pub define_function: TorqueDefineFunctionFn,
    /// Charges `steps` of work to the run's step budget: [`TORQUE_OK`] while it can pay, [`TORQUE_ERR_LIMIT`] once it is
    /// spent (stop, and hand it back). A script language calls it as its code runs (at its interrupts, say).
    pub charge: TorqueChargeFn,
    /// What the call's script engine is.
    pub vm_info: TorqueVmInfoFn,
    /// During a language's `run_file`: the package the file's definitions go into (a mod's, so that switching the mod
    /// off hides them), or the empty string. Valid as [`call_function`](Self::call_function)'s result.
    pub script_package: TorqueScriptPackageFn,
    /// Reads the script file `path` (a relative path in the game's files, with no `..`, ending in the extension of one of
    /// the extension's languages) after the checks `exec` makes: a file a server sent is refused ([`TORQUE_ERR_FAILED`]),
    /// and so is a missing one. Valid as [`call_function`](Self::call_function)'s result, at most [`TORQUE_MAX_SCRIPT`]
    /// bytes.
    pub read_script: TorqueReadScriptFn,
    /// Declares a script language (only during init).
    pub register_script_language: TorqueRegisterLanguageFn,
}

/// What an extension says about itself, filled in by `torque_plugin_init`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TorquePluginInfo {
    /// The [`TORQUE_ABI_VERSION`] the extension was built against.
    pub abi_version: u32,
    /// Its stable id, such as `example.native-sample`: ASCII letters, digits, `_`, `-`, `.` and `:`.
    pub id: TorqueStr,
    /// Its version, such as `0.1.0`.
    pub version: TorqueStr,
    /// Its network version when [`has_net_version`](Self::has_net_version) is 1: a server and a client must then both have
    /// it, at the same version.
    pub net_version: u32,
    /// 1 when it changes what goes over the network, else 0.
    pub has_net_version: u32,
}

/// `torque_plugin_init`: the extension's entry point.
pub type TorquePluginInitFn =
    unsafe extern "C" fn(api: *const TorqueApi, out: *mut TorquePluginInfo) -> TorqueStatus;

/// The name of the exported `uint32_t` holding the ABI version an extension was built against.
pub const TORQUE_ABI_VERSION_SYMBOL: &str = "torque_plugin_abi_version";
/// The name of the exported entry point ([`TorquePluginInitFn`]).
pub const TORQUE_INIT_SYMBOL: &str = "torque_plugin_init";
