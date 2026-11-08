//! The bridge to the page's Lua module: a [`LuaModule`] over `globalThis.torqueLua`, which `web/lua.js` sets once it has
//! loaded the module.
//!
//! Each call into the module hands the page a function that serves the module's requests for that call, as long as it
//! runs: `host(op, args)`, with the texts as `Uint8Array`s and numbers as numbers. It answers with the C ABI's status, or
//! with the text (a `Uint8Array`) or the number a request has; [`LuaHost`] makes every check. A limit that stopped a request
//! is kept here and handed back when the module's call returns ([`torque_plugin_abi::TORQUE_ERR_LIMIT`]), as a native
//! extension's call hands it back to the engine.
//!
//! Without `globalThis.torqueLua` (a page that loaded no Lua module) a Lua file prints that it cannot run, as
//! [`MissingLuaModule`] does.

use std::cell::Cell;

use js_sys::{Array, Reflect, Uint8Array};
use torque_engine::Side;
use torque_plugin_abi::{
    TORQUE_ERR_FAILED, TORQUE_ERR_INVALID, TORQUE_ERR_LIMIT, TORQUE_ERR_PANIC, TORQUE_LOG_ERROR,
    TORQUE_LOG_WARN, TORQUE_OK, TORQUE_SIDE_BOTH, TORQUE_SIDE_CLIENT, TORQUE_SIDE_SERVER,
    TorqueStatus,
};
use torque_ts_vm::vm::{Level, VmError};
use wasm_bindgen::prelude::*;

use super::{HostError, LuaHost, LuaModule, MissingLuaModule};

/// The requests the page's module makes during a call (`host(op, args)`), one per function of the C ABI it uses.
mod op {
    /// `(level, text)`: prints.
    pub const LOG: u32 = 0;
    /// `(name)`: a global's value.
    pub const GET_GLOBAL: u32 = 1;
    /// `(name, value)`: sets a global.
    pub const SET_GLOBAL: u32 = 2;
    /// `(namespace, name, args)`: runs a console function; its result.
    pub const CALL_FUNCTION: u32 = 3;
    /// `(object, name, args)`: runs a method; its result.
    pub const CALL_METHOD: u32 = 4;
    /// `(object, name)`: 1 when the object has the method, else 0.
    pub const HAS_METHOD: u32 = 5;
    /// `(name)`: the object's id, or `null`.
    pub const FIND_OBJECT: u32 = 6;
    /// `(object, name, index)`: a field's value.
    pub const GET_FIELD: u32 = 7;
    /// `(object, name, index, value)`: sets a field.
    pub const SET_FIELD: u32 = 8;
    /// `(namespace, name, package, usage, id)`: defines a console function.
    pub const DEFINE_FUNCTION: u32 = 9;
    /// `(steps)`: charges the step budget.
    pub const CHARGE: u32 = 10;
    /// `(path)`: a Lua file, after `exec`'s checks.
    pub const READ_SCRIPT: u32 = 11;
}

#[wasm_bindgen]
extern "C" {
    /// The page's Lua module, as `web/lua.js` offers it.
    type Bridge;

    #[wasm_bindgen(method, catch, js_name = runFile)]
    fn run_file(
        this: &Bridge,
        host: &mut dyn FnMut(u32, Array) -> JsValue,
        vm: f64,
        side: u32,
        path: &[u8],
        source: &[u8],
        package: &[u8],
    ) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(method, catch, js_name = callDefined)]
    fn call_defined(
        this: &Bridge,
        host: &mut dyn FnMut(u32, Array) -> JsValue,
        vm: f64,
        side: u32,
        id: f64,
        args: Array,
    ) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(method, catch, js_name = realmDropped)]
    fn realm_dropped(this: &Bridge, vm: f64) -> Result<JsValue, JsValue>;
}

/// The page's module, when it loaded one.
fn bridge() -> Option<Bridge> {
    let value = Reflect::get(&js_sys::global(), &JsValue::from_str("torqueLua")).ok()?;
    (!value.is_undefined() && !value.is_null()).then(|| value.unchecked_into())
}

/// The page's Lua module ([`WebLuaPlugin`](super::WebLuaPlugin)'s default in a browser build).
#[derive(Clone, Copy, Debug, Default)]
pub struct PageLuaModule;

impl LuaModule for PageLuaModule {
    fn run_file(&self, host: &mut dyn LuaHost, path: &str, source: &[u8]) -> Result<(), VmError> {
        let Some(bridge) = bridge() else {
            return MissingLuaModule.run_file(host, path, source);
        };
        let (realm, side) = host.realm();
        let package = host.package().as_bytes().to_vec();
        let stopped = Cell::new(None);
        let mut serve = |op: u32, args: Array| serve(&mut *host, &stopped, op, &args);
        let outcome = bridge.run_file(
            &mut serve,
            realm as f64,
            side_code(side),
            path.as_bytes(),
            source,
            &package,
        );
        finish(host, &stopped, outcome).map(|_| ())
    }

    fn call_defined(
        &self,
        host: &mut dyn LuaHost,
        id: u64,
        args: &[Vec<u8>],
    ) -> Result<Vec<u8>, VmError> {
        let Some(bridge) = bridge() else {
            return Ok(Vec::new());
        };
        let (realm, side) = host.realm();
        let texts: Array = args
            .iter()
            .map(|a| JsValue::from(Uint8Array::from(&a[..])))
            .collect();
        let stopped = Cell::new(None);
        let mut serve = |op: u32, args: Array| serve(&mut *host, &stopped, op, &args);
        let outcome =
            bridge.call_defined(&mut serve, realm as f64, side_code(side), id as f64, texts);
        finish(host, &stopped, outcome)
    }

    fn realm_dropped(&self, realm: u64) {
        if let Some(bridge) = bridge() {
            let _ = bridge.realm_dropped(realm as f64);
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

/// What a call into the module came to: its text (empty for a status of success), the limit that stopped it, or nothing
/// after an error the module or the page printed.
fn finish(
    host: &mut dyn LuaHost,
    stopped: &Cell<Option<VmError>>,
    outcome: Result<JsValue, JsValue>,
) -> Result<Vec<u8>, VmError> {
    let answer = match outcome {
        Ok(answer) => answer,
        Err(error) => {
            host.print(
                Level::Error,
                &format!("the page's Lua module failed: {}", describe(&error)),
            );
            return Ok(Vec::new());
        }
    };
    if let Some(text) = answer.dyn_ref::<Uint8Array>() {
        return Ok(text.to_vec());
    }
    let status = answer
        .as_f64()
        .map_or(TORQUE_ERR_FAILED, |s| s as TorqueStatus);
    match status {
        TORQUE_OK => Ok(Vec::new()),
        TORQUE_ERR_LIMIT => Err(stopped.take().unwrap_or(VmError::StepLimit)),
        TORQUE_ERR_PANIC => {
            host.print(
                Level::Error,
                "the page's Lua module stopped working (it trapped or panicked); Lua does not run again until the page is \
                 loaded again",
            );
            Ok(Vec::new())
        }
        status => {
            host.print(
                Level::Error,
                &format!("the page's Lua module refused the call (status {status})"),
            );
            Ok(Vec::new())
        }
    }
}

/// An error the page threw, as text.
fn describe(error: &JsValue) -> String {
    error
        .dyn_ref::<js_sys::Error>()
        .map(|e| String::from(e.message()))
        .or_else(|| error.as_string())
        .unwrap_or_else(|| format!("{error:?}"))
}

/// The bytes of the text at `at`.
fn bytes(args: &Array, at: u32) -> Option<Vec<u8>> {
    args.get(at)
        .dyn_into::<Uint8Array>()
        .ok()
        .map(|a| a.to_vec())
}

/// The UTF-8 name at `at`.
fn name(args: &Array, at: u32) -> Option<String> {
    String::from_utf8(bytes(args, at)?).ok()
}

/// The whole number at `at`.
fn number(args: &Array, at: u32) -> Option<f64> {
    args.get(at).as_f64().filter(|n| n.is_finite() && *n >= 0.0)
}

/// The texts of the array at `at`.
fn texts(args: &Array, at: u32) -> Option<Vec<Vec<u8>>> {
    let list = args.get(at).dyn_into::<Array>().ok()?;
    (0..list.length()).map(|i| bytes(&list, i)).collect()
}

fn status(code: TorqueStatus) -> JsValue {
    JsValue::from(code)
}

fn text(bytes: &[u8]) -> JsValue {
    JsValue::from(Uint8Array::from(bytes))
}

/// A result of the engine's: its text, or the status of a refusal or of a limit (which is kept for the call's end).
fn answer(stopped: &Cell<Option<VmError>>, result: Result<Vec<u8>, HostError>) -> JsValue {
    match result {
        Ok(bytes) => text(&bytes),
        Err(HostError::Refused) => status(TORQUE_ERR_FAILED),
        Err(HostError::Stopped(error)) => {
            stopped.set(Some(error));
            status(TORQUE_ERR_LIMIT)
        }
    }
}

/// Serves one request of the module's, as the C ABI's functions do.
fn serve(
    host: &mut dyn LuaHost,
    stopped: &Cell<Option<VmError>>,
    op: u32,
    args: &Array,
) -> JsValue {
    let invalid = || status(TORQUE_ERR_INVALID);
    match op {
        op::LOG => {
            let (Some(level), Some(text)) = (number(args, 0), bytes(args, 1)) else {
                return invalid();
            };
            let level = match level as u32 {
                TORQUE_LOG_WARN => Level::Warning,
                TORQUE_LOG_ERROR => Level::Error,
                _ => Level::Normal,
            };
            host.print(level, &String::from_utf8_lossy(&text));
            status(TORQUE_OK)
        }
        op::GET_GLOBAL => match name(args, 0) {
            Some(name) => text(&host.global(&name)),
            None => invalid(),
        },
        op::SET_GLOBAL => match (name(args, 0), bytes(args, 1)) {
            (Some(name), Some(value)) if host.set_global(&name, &value) => status(TORQUE_OK),
            (Some(_), Some(_)) => invalid(),
            _ => invalid(),
        },
        op::CALL_FUNCTION => match (name(args, 0), name(args, 1), texts(args, 2)) {
            (Some(namespace), Some(function), Some(values)) => {
                answer(stopped, host.call_function(&namespace, &function, &values))
            }
            _ => invalid(),
        },
        op::CALL_METHOD => match (number(args, 0), name(args, 1), texts(args, 2)) {
            (Some(object), Some(method), Some(values)) => {
                answer(stopped, host.call_method(object as u32, &method, &values))
            }
            _ => invalid(),
        },
        op::HAS_METHOD => match (number(args, 0), name(args, 1)) {
            (Some(object), Some(method)) => {
                JsValue::from(u32::from(host.has_method(object as u32, &method)))
            }
            _ => JsValue::from(0),
        },
        op::FIND_OBJECT => match bytes(args, 0).and_then(|name| host.find_object(&name)) {
            Some(id) => JsValue::from(id),
            None => JsValue::NULL,
        },
        op::GET_FIELD => match (number(args, 0), bytes(args, 1), bytes(args, 2)) {
            (Some(object), Some(field), Some(index)) => {
                match host.field(object as u32, &field, &index) {
                    Some(value) => text(&value),
                    None => status(TORQUE_ERR_FAILED),
                }
            }
            _ => invalid(),
        },
        op::SET_FIELD => match (
            number(args, 0),
            bytes(args, 1),
            bytes(args, 2),
            bytes(args, 3),
        ) {
            (Some(object), Some(field), Some(index), Some(value)) => {
                if host.set_field(object as u32, &field, &index, &value) {
                    status(TORQUE_OK)
                } else {
                    status(TORQUE_ERR_FAILED)
                }
            }
            _ => invalid(),
        },
        op::DEFINE_FUNCTION => {
            let read = (
                name(args, 0),
                name(args, 1),
                name(args, 2),
                name(args, 3),
                number(args, 4),
            );
            let (Some(namespace), Some(function), Some(package), Some(usage), Some(id)) = read
            else {
                return invalid();
            };
            if host.define_function(&namespace, &function, &package, &usage, id as u64) {
                status(TORQUE_OK)
            } else {
                status(TORQUE_ERR_FAILED)
            }
        }
        op::CHARGE => match number(args, 0) {
            Some(steps) => match host.charge(steps as u64) {
                Ok(()) => status(TORQUE_OK),
                Err(error) => {
                    stopped.set(Some(error));
                    status(TORQUE_ERR_LIMIT)
                }
            },
            None => invalid(),
        },
        op::READ_SCRIPT => match name(args, 0) {
            Some(path) => match host.read_script(&path) {
                Ok(Some(source)) => text(&source),
                Ok(None) => status(TORQUE_ERR_FAILED),
                Err(error) => {
                    stopped.set(Some(error));
                    status(TORQUE_ERR_LIMIT)
                }
            },
            None => invalid(),
        },
        _ => invalid(),
    }
}
