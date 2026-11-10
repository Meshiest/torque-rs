//! The guest's exports (`torque_lua_client.wasm`, as the format crate's docs list them): one realm per instance, over
//! [`ClientRealm`]. The host writes its inputs into places `torque_realm_alloc` made and reads each entry's flush from
//! `torque_realm_ops` before its next call. Nothing here calls the host.

#![expect(
    unsafe_code,
    reason = "the exports read what the host wrote into this module's memory"
)]

use std::cell::RefCell;

use super::format::{FORMAT_VERSION, abi};
use super::{ClientRealm, Status};

thread_local! {
    static REALM: RefCell<Option<ClientRealm>> = const { RefCell::new(None) };
    /// The last entry's flush, for the host to read.
    static OPS: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// The bytes the host put at `ptr`.
///
/// # Safety
///
/// `ptr` is null with `len` 0, or `len` bytes of this module's memory the host wrote and keeps until the call returns.
unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if len == 0 {
        return Some(&[]);
    }
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the caller's promise.
    Some(unsafe { std::slice::from_raw_parts(ptr, len) })
}

/// The text the host put at `ptr`.
///
/// # Safety
///
/// As [`bytes`].
unsafe fn text<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
    // SAFETY: the caller's promise.
    std::str::from_utf8(unsafe { bytes(ptr, len) }?).ok()
}

/// Runs `f` on the realm, then keeps its flush: the entry's status.
fn entry(f: impl FnOnce(&mut ClientRealm) -> Result<(), Status>) -> i32 {
    REALM.with(|realm| {
        let mut realm = realm.borrow_mut();
        let Some(realm) = realm.as_mut() else {
            return abi::STATE;
        };
        let status = match f(realm) {
            Ok(()) => abi::OK,
            Err(status) => status.code(),
        };
        OPS.with(|ops| *ops.borrow_mut() = realm.take_ops());
        status
    })
}

/// The time an entry carries, or `None` when it is negative.
fn micros(now_us: i64) -> Option<u64> {
    u64::try_from(now_us).ok()
}

/// This format's version.
#[unsafe(no_mangle)]
pub extern "C" fn torque_realm_format() -> i32 {
    FORMAT_VERSION as i32
}

/// The grant bits of the tiers this guest implements.
#[unsafe(no_mangle)]
pub extern "C" fn torque_realm_tiers() -> i32 {
    ClientRealm::tiers().0 as i32
}

/// A place of `len` bytes for the host to write into, or null when there is no room.
#[unsafe(no_mangle)]
pub extern "C" fn torque_realm_alloc(len: usize) -> *mut u8 {
    let mut place: Vec<u8> = Vec::new();
    if place.try_reserve_exact(len.max(1)).is_err() {
        return std::ptr::null_mut();
    }
    place.resize(len.max(1), 0);
    Box::into_raw(place.into_boxed_slice()).cast::<u8>()
}

/// Frees a place [`torque_realm_alloc`] made.
///
/// # Safety
///
/// `ptr` and `len` are one place `torque_realm_alloc(len)` returned, not freed before.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn torque_realm_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: the caller's promise: a boxed slice of `len.max(1)` bytes made by `torque_realm_alloc`.
    drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len.max(1))) });
}

/// Makes the instance's realm from a config record.
///
/// # Safety
///
/// The record is `len` bytes the host wrote at `config`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn torque_realm_new(config: *const u8, len: usize) -> i32 {
    // SAFETY: the caller's promise.
    let Some(record) = (unsafe { bytes(config, len) }) else {
        return abi::MALFORMED;
    };
    REALM.with(|realm| {
        let mut realm = realm.borrow_mut();
        if realm.is_some() {
            return abi::STATE;
        }
        match ClientRealm::from_config(record) {
            Ok(made) => {
                *realm = Some(made);
                abi::OK
            }
            Err(status) => status.code(),
        }
    })
}

/// Keeps one script file of a mod.
///
/// # Safety
///
/// Each pointer and length is bytes the host wrote.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn torque_realm_add_script(
    mod_id: *const u8,
    mod_len: usize,
    path: *const u8,
    path_len: usize,
    source: *const u8,
    source_len: usize,
) -> i32 {
    // SAFETY: the caller's promise.
    let (Some(mod_id), Some(path), Some(source)) = (unsafe {
        (
            text(mod_id, mod_len),
            text(path, path_len),
            bytes(source, source_len),
        )
    }) else {
        return abi::MALFORMED;
    };
    REALM.with(|realm| match realm.borrow_mut().as_mut() {
        Some(realm) => match realm.add_script(mod_id, path, source) {
            Ok(()) => abi::OK,
            Err(status) => status.code(),
        },
        None => abi::STATE,
    })
}

/// Runs a kept file in its mod's global table.
///
/// # Safety
///
/// Each pointer and length is bytes the host wrote.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn torque_realm_run(
    now_us: i64,
    mod_id: *const u8,
    mod_len: usize,
    path: *const u8,
    path_len: usize,
) -> i32 {
    // SAFETY: the caller's promise.
    let (Some(mod_id), Some(path), Some(now)) =
        (unsafe { (text(mod_id, mod_len), text(path, path_len), micros(now_us)) })
    else {
        return abi::MALFORMED;
    };
    entry(|realm| realm.run(now, mod_id, path))
}

/// One frame with the event batch the host wrote.
///
/// # Safety
///
/// The batch is `len` bytes the host wrote at `events`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn torque_realm_frame(now_us: i64, events: *const u8, len: usize) -> i32 {
    // SAFETY: the caller's promise.
    let (Some(events), Some(now)) = (unsafe { bytes(events, len) }, micros(now_us)) else {
        return abi::MALFORMED;
    };
    entry(|realm| realm.frame(now, events))
}

/// Where the last entry's flush is.
#[unsafe(no_mangle)]
pub extern "C" fn torque_realm_ops() -> *const u8 {
    OPS.with(|ops| ops.borrow().as_ptr())
}

/// How long the last entry's flush is.
#[unsafe(no_mangle)]
pub extern "C" fn torque_realm_ops_len() -> usize {
    OPS.with(|ops| ops.borrow().len())
}

/// Events still queued for a later frame.
#[unsafe(no_mangle)]
pub extern "C" fn torque_realm_pending() -> i32 {
    REALM.with(|realm| {
        realm
            .borrow()
            .as_ref()
            .map_or(0, |r| i32::try_from(r.pending()).unwrap_or(i32::MAX))
    })
}
