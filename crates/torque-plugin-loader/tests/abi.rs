//! The boundary's checks, on extensions linked into the test (their entry points called directly, as a game links one
//! in): a wrong ABI version is refused with a message before any of the extension's code runs; a panic inside an
//! extension disables it and the engine goes on; what an extension hands over is checked (null pointers, ranges, names,
//! handles that are no longer live).

// The hand-written extension below speaks the C ABI as a C one would, through raw pointers.
#![allow(unsafe_code)]

mod common;

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

use common::server_vm;
use torque_engine::EngineBuilder;
use torque_plugin_loader::ExtensionPlugin;
use torque_plugin_loader::abi::*;
use torque_plugin_sdk::{Extension, Registrar, This};

#[derive(Default)]
struct Boom;

impl Extension for Boom {
    const ID: &'static str = "test.boom";
    const VERSION: &'static str = "1.0.0";

    fn build(&self, ext: &mut Registrar) {
        ext.script()
            .function("", "boomFine", &[], "answers fine", || "fine")
            .method(
                "SimObject",
                "boomId",
                &["plus"],
                "its id plus a number",
                |this: This, plus: i32| this.0 as i32 + plus,
            )
            .function("", "boomPanic", &["why"], "panics", |why: &str| -> String {
                panic!("{why}")
            });
    }
}

torque_plugin_sdk::init_fn!(boom_init, Boom);

#[test]
fn a_wrong_abi_version_is_refused_with_a_message() {
    let Err(message) = ExtensionPlugin::from_init(TORQUE_ABI_VERSION + 1, boom_init) else {
        panic!("refused");
    };
    assert!(
        message.contains(&format!("ABI version {}", TORQUE_ABI_VERSION + 1))
            && message.contains(&format!("speaks version {TORQUE_ABI_VERSION}"))
            && message.contains("rebuild it"),
        "{message}"
    );
}

/// An entry point that reports another ABI version than the one it was exported as.
unsafe extern "C" fn liar_init(_api: *const TorqueApi, out: *mut TorquePluginInfo) -> TorqueStatus {
    // SAFETY: the engine hands a valid place for the info.
    unsafe { (*out).abi_version = 99 };
    TORQUE_OK
}

#[test]
fn an_init_that_reports_another_version_is_refused() {
    let Err(message) = ExtensionPlugin::from_init(TORQUE_ABI_VERSION, liar_init) else {
        panic!("refused");
    };
    assert!(message.contains("ABI version 99"), "{message}");
}

#[test]
fn a_panic_disables_the_extension_and_the_engine_goes_on() {
    let plugin = ExtensionPlugin::from_init(TORQUE_ABI_VERSION, boom_init).unwrap();
    let extension = plugin.extension();
    let engine = EngineBuilder::new().add_plugins(plugin).build().unwrap();
    assert!(engine.is_plugin_added_id("test.boom"));
    let mut vm = server_vm(&engine);
    assert_eq!(vm.eval(b"return boomFine();").unwrap(), b"fine");
    vm.eval(b"new SimObject(Thing);").unwrap();
    let id = vm.objects().find(b"Thing").expect("the object");
    assert_eq!(
        vm.eval(b"return Thing.boomId(5);").unwrap(),
        (id + 5).to_string().as_bytes()
    );
    assert!(vm.native_listing().contains(
        "SimObject::boomId 3..3 simObject.boomId(plus) - its id plus a number
"
    ));
    assert!(!extension.is_disabled());

    // The panic is caught in the extension, reported, and the call returns the empty string.
    assert_eq!(vm.eval(b"return boomPanic(\"on purpose\");").unwrap(), b"");
    assert!(extension.is_disabled());
    let reason = extension.disabled_reason().unwrap();
    assert!(reason.contains("boomPanic"), "{reason}");

    // Disabled: its functions are not called any more; the script engine runs on.
    assert_eq!(vm.eval(b"return boomFine();").unwrap(), b"");
    assert_eq!(vm.eval(b"return 1 + 1;").unwrap(), b"2");
}

#[derive(Default)]
struct BadStart;

impl Extension for BadStart {
    const ID: &'static str = "test.bad-start";
    const VERSION: &'static str = "1";

    fn build(&self, _ext: &mut Registrar) {
        panic!("no start today");
    }
}

torque_plugin_sdk::init_fn!(bad_start_init, BadStart);

#[derive(Default)]
struct BadName;

impl Extension for BadName {
    const ID: &'static str = "test.bad-name";
    const VERSION: &'static str = "1";

    fn build(&self, ext: &mut Registrar) {
        ext.script().function("", "two words", &[], "", || 1);
    }
}

torque_plugin_sdk::init_fn!(bad_name_init, BadName);

#[test]
fn a_panic_while_it_starts_or_a_refused_declaration_refuses_it() {
    let Err(message) = ExtensionPlugin::from_init(TORQUE_ABI_VERSION, bad_start_init) else {
        panic!("refused");
    };
    assert!(
        message.contains("panicked") && message.contains("no start today"),
        "{message}"
    );
    let Err(message) = ExtensionPlugin::from_init(TORQUE_ABI_VERSION, bad_name_init) else {
        panic!("refused");
    };
    assert!(message.contains("not a console name"), "{message}");
}

// A hand-written extension: what the engine's checks return to it is recorded, and read back after the call (a panic
// inside an `extern "C"` function would end the process).

static API: AtomicPtr<TorqueApi> = AtomicPtr::new(std::ptr::null_mut());
static STALE: AtomicU64 = AtomicU64::new(0);
static SEEN: Mutex<Vec<(&'static str, TorqueStatus)>> = Mutex::new(Vec::new());

fn api() -> &'static TorqueApi {
    // SAFETY: set by `raw_init` to the engine's table, which lives for the process.
    unsafe { &*API.load(Ordering::SeqCst) }
}

fn text(s: &'static str) -> TorqueStr {
    TorqueStr {
        ptr: s.as_ptr(),
        len: s.len(),
    }
}

unsafe extern "C" fn raw_count(_user: *mut c_void, call: TorqueCall) -> TorqueStatus {
    let api = api();
    STALE.store(call, Ordering::SeqCst);
    let mut seen = SEEN.lock().unwrap();
    let mut n = 0;
    // SAFETY (each call below): the table's functions take any values and check them.
    unsafe {
        seen.push(("null out", (api.arg_count)(call, std::ptr::null_mut())));
        seen.push(("count", (api.arg_count)(call, &mut n)));
        let mut s = text("");
        seen.push(("past the end", (api.arg_str)(call, n + 5, &mut s)));
        let dangling = TorqueStr {
            ptr: std::ptr::null(),
            len: 3,
        };
        seen.push(("null text", (api.return_str)(call, dangling)));
        seen.push((
            "bad global",
            (api.set_global)(call, text("no dollar"), text("1")),
        ));
        seen.push(("not an object", (api.this_object)(call, &mut n)));
        seen.push(("wrong handle", (api.return_int)(call + 1_000_000, 7)));
        let mut n = 0;
        (api.arg_count)(call, &mut n);
        (api.return_int)(call, n as i32)
    }
}

fn decl(name: &'static str, args: &[TorqueArg], description: &'static str) -> TorqueNativeDecl {
    TorqueNativeDecl {
        side: TORQUE_SIDE_BOTH,
        ns: text(""),
        name: text(name),
        args: args.as_ptr(),
        arg_count: args.len(),
        description: text(description),
        usage: text(""),
        docs: text(""),
        flags: 0,
        call: Some(raw_count),
        user: std::ptr::null_mut(),
    }
}

unsafe extern "C" fn raw_init(api: *const TorqueApi, out: *mut TorquePluginInfo) -> TorqueStatus {
    API.store(api as *mut TorqueApi, Ordering::SeqCst);
    let api = self::api();
    let args = [TorqueArg {
        name: text("values"),
        kind: TORQUE_ARG_REST,
    }];
    let decl = decl("rawCount", &args, "counts its arguments");
    // SAFETY: the engine's table and a valid place for the info.
    unsafe {
        *out = TorquePluginInfo {
            abi_version: TORQUE_ABI_VERSION,
            id: text("test.raw"),
            version: text("1"),
            net_version: 0,
            has_net_version: 0,
        };
        (api.register_function)(api.init, &decl)
    }
}

#[test]
fn what_an_extension_hands_over_is_checked() {
    let plugin = ExtensionPlugin::from_init(TORQUE_ABI_VERSION, raw_init).unwrap();
    let engine = EngineBuilder::new().add_plugins(plugin).build().unwrap();
    let mut vm = server_vm(&engine);
    assert!(
        vm.native_listing()
            .contains("rawCount 1..* rawCount(values...) - counts its arguments\n")
    );
    assert_eq!(vm.eval(b"return rawCount(1, 2, 3);").unwrap(), b"3");
    let seen = SEEN.lock().unwrap().clone();
    assert_eq!(
        seen,
        [
            ("null out", TORQUE_ERR_INVALID),
            ("count", TORQUE_OK),
            ("past the end", TORQUE_ERR_RANGE),
            ("null text", TORQUE_ERR_INVALID),
            ("bad global", TORQUE_ERR_INVALID),
            ("not an object", TORQUE_ERR_FAILED),
            ("wrong handle", TORQUE_ERR_HANDLE),
        ]
    );

    // Once the call has returned, its handle is dead, and so is the init's.
    let api = api();
    let mut n = 0;
    let late = decl("late", &[], "");
    // SAFETY: the table's functions take any values and check them.
    unsafe {
        assert_eq!(
            (api.arg_count)(STALE.load(Ordering::SeqCst), &mut n),
            TORQUE_ERR_HANDLE
        );
        assert_eq!((api.register_function)(api.init, &late), TORQUE_ERR_HANDLE);
        assert_eq!((api.log)(0, 9, text("x")), TORQUE_ERR_INVALID);
    }
}
