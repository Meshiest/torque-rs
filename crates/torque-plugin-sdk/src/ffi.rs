//! Every crossing of the C ABI on the extension's side: the calls into the engine's function table, the entry point, and
//! the trampolines the engine calls, each of which catches a panic and reports it as [`TORQUE_ERR_PANIC`].
//!
//! The engine checks everything it is handed; what this side trusts is only what the ABI promises of the engine: the
//! table stays valid while the library is loaded, and a string the engine hands over is valid until the call returns.

#![allow(unsafe_code)]

use std::any::Any;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};

use torque_plugin_abi::{
    TORQUE_ABI_VERSION, TORQUE_ERR_ABI, TORQUE_ERR_FAILED, TORQUE_ERR_INVALID, TORQUE_ERR_PANIC,
    TORQUE_LOG_ERROR, TORQUE_NATIVE_DEV, TORQUE_NATIVE_EXACT_USAGE, TORQUE_OK, TorqueApi,
    TorqueArg, TorqueCall, TorqueDraw, TorqueNativeDecl, TorquePassDecl, TorquePassFrame,
    TorquePluginInfo, TorqueStatus, TorqueStr, TorqueVertex,
};

use crate::param::Return;
use crate::render::{MakePass, Pass, PassFrame, Stage, Triangles};
use crate::script::{NativeReg, TypedNative};
use crate::{Call, Extension, Registrar, Side};

/// `bytes` as the ABI's text (borrowed: valid while `bytes` is).
fn text(bytes: &[u8]) -> TorqueStr {
    TorqueStr {
        ptr: bytes.as_ptr(),
        len: bytes.len(),
    }
}

/// The bytes of text the engine handed over, or `None` for a null pointer with a length.
///
/// # Safety
///
/// `s` is null with any length 0, or `len` bytes the engine keeps valid for `'a`.
unsafe fn bytes_of<'a>(s: TorqueStr) -> Option<&'a [u8]> {
    if s.len == 0 {
        return Some(&[]);
    }
    if s.ptr.is_null() {
        return None;
    }
    // SAFETY: not null, and the ABI says `len` bytes at `ptr` stay valid until the call that handed them returns.
    Some(unsafe { std::slice::from_raw_parts(s.ptr, s.len) })
}

/// A panic's message.
fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic with no message".to_string())
}

pub(crate) fn log(api: &TorqueApi, handle: u64, level: u32, message: &str) {
    // SAFETY: the table's functions take any values; the text is valid during the call.
    unsafe { (api.log)(handle, level, text(message.as_bytes())) };
}

pub(crate) fn get_global(api: &TorqueApi, call: TorqueCall, name: &str) -> Option<Vec<u8>> {
    let mut out = text(&[]);
    // SAFETY: `out` is a valid place for the result; the name is valid during the call.
    let status = unsafe { (api.get_global)(call, text(name.as_bytes()), &mut out) };
    if status != TORQUE_OK {
        return None;
    }
    // SAFETY: the engine keeps the value valid until the next `get_global`; it is copied here at once.
    unsafe { bytes_of(out) }.map(<[u8]>::to_vec)
}

pub(crate) fn set_global(api: &TorqueApi, call: TorqueCall, name: &str, value: &[u8]) -> bool {
    // SAFETY: both texts are valid during the call.
    unsafe { (api.set_global)(call, text(name.as_bytes()), text(value)) == TORQUE_OK }
}

pub(crate) fn draw_triangles(
    api: &TorqueApi,
    draw: TorqueDraw,
    space: u32,
    vertices: &[TorqueVertex],
) -> bool {
    // SAFETY: the slice is valid during the call, which copies it.
    unsafe { (api.draw_triangles)(draw, space, vertices.as_ptr(), vertices.len()) == TORQUE_OK }
}

impl<'a> Call<'a> {
    /// How many script arguments the call has.
    pub fn arg_count(&self) -> u32 {
        let mut n = 0;
        // SAFETY: `n` is a valid place for the count.
        let status = unsafe { (self.api.arg_count)(self.handle, &mut n) };
        if status == TORQUE_OK { n } else { 0 }
    }

    /// Script argument `index` (from 0) as bytes, up to its first NUL.
    pub fn arg_bytes(&self, index: u32) -> Option<&'a [u8]> {
        let mut out = text(&[]);
        // SAFETY: `out` is a valid place for the argument.
        let status = unsafe { (self.api.arg_str)(self.handle, index, &mut out) };
        if status != TORQUE_OK {
            return None;
        }
        // SAFETY: the engine keeps an argument valid until the call returns, and a `Call` lives no longer.
        unsafe { bytes_of(out) }
    }

    /// Script argument `index` as an integer (`dAtoi`).
    pub fn arg_int(&self, index: u32) -> Option<i32> {
        let mut out = 0;
        // SAFETY: `out` is a valid place for the value.
        let status = unsafe { (self.api.arg_int)(self.handle, index, &mut out) };
        (status == TORQUE_OK).then_some(out)
    }

    /// Script argument `index` as a float (`dAtof`, rounded to 32 bits).
    pub fn arg_float(&self, index: u32) -> Option<f32> {
        let mut out = 0.0;
        // SAFETY: `out` is a valid place for the value.
        let status = unsafe { (self.api.arg_float)(self.handle, index, &mut out) };
        (status == TORQUE_OK).then_some(out)
    }

    /// Script argument `index` as a truth value (`dAtob`).
    pub fn arg_bool(&self, index: u32) -> Option<bool> {
        let mut out = 0;
        // SAFETY: `out` is a valid place for the value.
        let status = unsafe { (self.api.arg_bool)(self.handle, index, &mut out) };
        (status == TORQUE_OK).then_some(out != 0)
    }

    /// A method call's object id; `None` for a function.
    pub fn this_object(&self) -> Option<u32> {
        let mut out = 0;
        // SAFETY: `out` is a valid place for the id.
        let status = unsafe { (self.api.this_object)(self.handle, &mut out) };
        (status == TORQUE_OK).then_some(out)
    }

    fn set_result(&self, result: Return) -> TorqueStatus {
        let (api, call) = (self.api, self.handle);
        // SAFETY: each function takes the live call and a value (text valid during the call).
        unsafe {
            match result {
                Return::Void => TORQUE_OK,
                Return::Int(v) => (api.return_int)(call, v),
                Return::Float(v) => (api.return_float)(call, v),
                Return::Bool(v) => (api.return_bool)(call, u32::from(v)),
                Return::Text(v) => (api.return_str)(call, text(&v)),
                Return::Error(message) => (api.return_error)(call, text(message.as_bytes())),
            }
        }
    }
}

/// What a native's `user` pointer holds.
struct NativeUser {
    api: &'static TorqueApi,
    native: TypedNative,
}

/// What a pass declaration's `user` pointer holds.
struct PassUser {
    api: &'static TorqueApi,
    make: MakePass,
}

/// What a pass instance's pointer holds.
struct PassInstance {
    api: &'static TorqueApi,
    pass: Box<dyn Pass>,
}

/// The engine calls a native here.
unsafe extern "C" fn native_trampoline(user: *mut c_void, call: TorqueCall) -> TorqueStatus {
    if user.is_null() {
        return TORQUE_ERR_INVALID;
    }
    // SAFETY: `user` is the `NativeUser` this side registered with the native, never freed while the library is loaded.
    let user = unsafe { &*(user as *const NativeUser) };
    let call = Call {
        api: user.api,
        handle: call,
    };
    match catch_unwind(AssertUnwindSafe(|| (user.native.run)(call))) {
        Ok(result) => call.set_result(result),
        Err(payload) => {
            call.error(&format!("panicked: {}", panic_text(&*payload)));
            TORQUE_ERR_PANIC
        }
    }
}

/// The engine makes a pass instance here.
unsafe extern "C" fn pass_create(
    user: *mut c_void,
    draw: TorqueDraw,
    out: *mut *mut c_void,
) -> TorqueStatus {
    if user.is_null() || out.is_null() || !out.is_aligned() {
        return TORQUE_ERR_INVALID;
    }
    // SAFETY: `user` is the `PassUser` registered with the pass, never freed while the library is loaded.
    let user = unsafe { &*(user as *const PassUser) };
    match catch_unwind(AssertUnwindSafe(|| (user.make)())) {
        Ok(pass) => {
            let made = Box::into_raw(Box::new(PassInstance {
                api: user.api,
                pass,
            }));
            // SAFETY: `out` is not null and aligned: the engine's place for the instance during the call.
            unsafe { out.write(made as *mut c_void) };
            TORQUE_OK
        }
        Err(payload) => {
            let message = format!("a pass panicked as it was made: {}", panic_text(&*payload));
            log(user.api, draw, TORQUE_LOG_ERROR, &message);
            TORQUE_ERR_PANIC
        }
    }
}

/// The engine draws a pass instance here.
unsafe extern "C" fn pass_draw(
    pass: *mut c_void,
    frame: *const TorquePassFrame,
    draw: TorqueDraw,
) -> TorqueStatus {
    if pass.is_null() || frame.is_null() {
        return TORQUE_ERR_INVALID;
    }
    // SAFETY: `pass` is an instance `pass_create` made and `pass_drop` has not freed; the engine draws one instance on
    // one thread at a time.
    let instance = unsafe { &mut *(pass as *mut PassInstance) };
    // SAFETY: `frame` is not null, and valid during the call.
    let raw = unsafe { *frame };
    let Some(stage) = Stage::of_code(raw.stage) else {
        return TORQUE_ERR_INVALID;
    };
    let frame = PassFrame {
        stage,
        size: raw.size,
        viewport: raw.viewport,
        view_proj: raw.view_proj,
        eye: raw.eye,
        time: raw.time,
    };
    let api = instance.api;
    let mut out = Triangles { api, draw };
    match catch_unwind(AssertUnwindSafe(|| instance.pass.draw(&frame, &mut out))) {
        Ok(()) => TORQUE_OK,
        Err(payload) => {
            let message = format!("a pass panicked: {}", panic_text(&*payload));
            log(api, draw, TORQUE_LOG_ERROR, &message);
            TORQUE_ERR_PANIC
        }
    }
}

/// The engine frees a pass instance here.
unsafe extern "C" fn pass_drop(pass: *mut c_void) {
    if pass.is_null() {
        return;
    }
    // SAFETY: `pass` is an instance `pass_create` made, freed once (the engine drops each instance once).
    let instance = unsafe { Box::from_raw(pass as *mut PassInstance) };
    // A panicking `Drop` is caught and dropped: there is nobody to tell.
    let _ = catch_unwind(AssertUnwindSafe(move || drop(instance)));
}

impl Registrar<'_> {
    pub(crate) fn native(&mut self, reg: NativeReg) {
        let args: Vec<TorqueArg> = reg
            .args
            .iter()
            .map(|(name, kind)| TorqueArg {
                name: text(name.as_bytes()),
                kind: kind.code(),
            })
            .collect();
        let mut flags = 0;
        if reg.dev {
            flags |= TORQUE_NATIVE_DEV;
        }
        if reg.usage.is_some() {
            flags |= TORQUE_NATIVE_EXACT_USAGE;
        }
        let usage = reg.usage.clone().unwrap_or_default();
        let user = Box::into_raw(Box::new(NativeUser {
            api: self.api,
            native: reg.native,
        }));
        let decl = TorqueNativeDecl {
            side: reg.side.code(),
            ns: text(reg.namespace.as_bytes()),
            name: text(reg.name.as_bytes()),
            args: args.as_ptr(),
            arg_count: args.len(),
            description: text(reg.description.as_bytes()),
            usage: text(usage.as_bytes()),
            docs: text(reg.docs.as_bytes()),
            flags,
            call: Some(native_trampoline),
            user: user as *mut c_void,
        };
        let register = if reg.method {
            self.api.register_method
        } else {
            self.api.register_function
        };
        // SAFETY: the declaration and everything it points at are valid during the call, which copies them.
        let status = unsafe { register(self.api.init, &decl) };
        if status != TORQUE_OK {
            // SAFETY: the engine refused it, so it keeps no copy of `user`: this side still owns it.
            drop(unsafe { Box::from_raw(user) });
            let kind = if reg.method { "method" } else { "function" };
            self.refused.push(format!(
                "the engine refused the {kind} {}::{} (status {status})",
                reg.namespace, reg.name
            ));
        }
    }

    pub(crate) fn global(&mut self, side: Side, name: &str, default: &str) {
        // SAFETY: both texts are valid during the call, which copies them.
        let status = unsafe {
            (self.api.register_global)(
                self.api.init,
                side.code(),
                text(name.as_bytes()),
                text(default.as_bytes()),
            )
        };
        if status != TORQUE_OK {
            self.refused.push(format!(
                "the engine refused the global {name} (status {status})"
            ));
        }
    }

    pub(crate) fn pass(&mut self, stage: Stage, make: MakePass) {
        let user = Box::into_raw(Box::new(PassUser {
            api: self.api,
            make,
        }));
        let decl = TorquePassDecl {
            stage: stage.code(),
            create: Some(pass_create),
            draw: Some(pass_draw),
            drop: Some(pass_drop),
            user: user as *mut c_void,
        };
        // SAFETY: the declaration is valid during the call, which copies it.
        let status = unsafe { (self.api.register_render_pass)(self.api.init, &decl) };
        if status != TORQUE_OK {
            // SAFETY: refused, so the engine keeps no copy of `user`.
            drop(unsafe { Box::from_raw(user) });
            self.refused.push(format!(
                "the engine refused a pass at {stage:?} (status {status})"
            ));
        }
    }
}

/// The entry point of the extension `E`: what [`export!`](crate::export) and [`init_fn!`](crate::init_fn) call.
///
/// # Safety
///
/// `api` is null or the engine's function table, valid while the library is loaded; `out` is null or a valid place for
/// the extension's info.
pub unsafe fn init<E: Extension>(
    api: *const TorqueApi,
    out: *mut TorquePluginInfo,
) -> TorqueStatus {
    if api.is_null() || out.is_null() {
        return TORQUE_ERR_INVALID;
    }
    // SAFETY: not null; the first two fields are the same in every version of the table, so they are read alone first.
    let (version, size) = unsafe { ((*api).abi_version, (*api).size) };
    if version != TORQUE_ABI_VERSION || (size as usize) < std::mem::size_of::<TorqueApi>() {
        return TORQUE_ERR_ABI;
    }
    // SAFETY: the table is this version's, and the engine keeps it valid while the library is loaded.
    let api: &'static TorqueApi = unsafe { &*api };
    // SAFETY: `out` is not null and a valid place for the info; the id and version are `'static`.
    unsafe {
        *out = TorquePluginInfo {
            abi_version: TORQUE_ABI_VERSION,
            id: text(E::ID.as_bytes()),
            version: text(E::VERSION.as_bytes()),
            net_version: E::NET_VERSION.unwrap_or(0),
            has_net_version: u32::from(E::NET_VERSION.is_some()),
        };
    }
    let built = catch_unwind(AssertUnwindSafe(|| {
        let mut registrar = Registrar {
            api,
            refused: Vec::new(),
            _life: std::marker::PhantomData,
        };
        // Kept for the process: what `build` hands the engine may borrow from it.
        let extension: &'static E = Box::leak(Box::new(E::default()));
        extension.build(&mut registrar);
        registrar.refused
    }));
    match built {
        Ok(refused) if refused.is_empty() => TORQUE_OK,
        Ok(refused) => {
            for message in refused {
                log(api, api.init, TORQUE_LOG_ERROR, &message);
            }
            TORQUE_ERR_FAILED
        }
        Err(payload) => {
            let message = format!("panicked while it was built: {}", panic_text(&*payload));
            log(api, api.init, TORQUE_LOG_ERROR, &message);
            TORQUE_ERR_PANIC
        }
    }
}
