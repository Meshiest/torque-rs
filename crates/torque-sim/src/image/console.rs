//! The `ShapeBase` image console methods (18 entries): the argument parsing and the state effects. The host owns the script glue (the object lookup, the
//! `%g` text of the vector results: `sprintf("%g %g %g", double, double, double)` of the floats, 6 significant digits).
//!
//! **Behaviour notes:**
//! - the argument counts count the object and the method name: `mountImage` 4..6, `setImageTrigger`/`setImageAmmo`/`setImageLoaded` 4..4,
//!   everything else 3..3;
//! - `mountImage` ALWAYS answers false, and with six arguments (a skin) the
//!   `loaded` argument is not read: `loaded` is true (with five arguments it is read); the skin is a tagged string (`"\x01<id>"`) only;
//! - a slot outside 0..3 answers: `getMountedImage`/`getPendingImage` 0, `getImageSkinTag` -1, `getImageState` the text "Error",
//!   the bool getters false, `getMuzzleVector` "0 1 0", `getMuzzlePoint` "0 0 0"; a NULL slot argument is slot 0;
//! - `getImageState` of an empty slot is NULL; `getMountSlot` of a datablock that is not mounted (or not found) is -1;
//! - `setImageTrigger` answers the trigger flag after the call, false for an empty slot; `setImageAmmo` and `setImageLoaded` answer the
//!   bool argument as parsed (`atob`: "true" matches case-insensitively, else `atof` is not 0, NaN is true);
//! - `getSlotTransform` is the mount transform of mount point `slot` (identity above 3) as `x y z ax ay az angle` text
//!   through the quaternion-to-axis-angle conversion: not modelled ([`call`] answers `None` for it).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/shapeBase.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use super::host::ImageHost;
use super::muzzle::{self, MuzzleCtx};
use super::{DbId, ImageData, SLOTS, machine};
use torque_collide::math::F3;

/// One registered method: its name and the argument counts (object and name included).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodSpec {
    pub name: &'static str,
    pub min: u8,
    pub max: u8,
}

const fn m(name: &'static str, min: u8, max: u8) -> MethodSpec {
    MethodSpec { name, min, max }
}

/// The methods, in the order of this module's docs.
pub const METHODS: [MethodSpec; 18] = [
    m("mountImage", 4, 6),
    m("unmountImage", 3, 3),
    m("getMountedImage", 3, 3),
    m("getPendingImage", 3, 3),
    m("isImageFiring", 3, 3),
    m("isImageMounted", 3, 3),
    m("getMountSlot", 3, 3),
    m("getImageSkinTag", 3, 3),
    m("getImageState", 3, 3),
    m("getImageTrigger", 3, 3),
    m("setImageTrigger", 4, 4),
    m("getImageAmmo", 3, 3),
    m("setImageAmmo", 4, 4),
    m("getImageLoaded", 3, 3),
    m("setImageLoaded", 4, 4),
    m("getMuzzleVector", 3, 3),
    m("getMuzzlePoint", 3, 3),
    m("getSlotTransform", 3, 3),
];

/// The spec of `name` (case-insensitive, as the namespace lookup is).
pub fn spec(name: &str) -> Option<&'static MethodSpec> {
    METHODS.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

/// A method's result as the console would return it.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i32),
    /// A string result; `None` is a NULL pointer (the script sees "").
    Str(Option<String>),
}

/// C `atoi` semantics: leading white space, a sign, digits; the result saturates.
pub fn atoi(s: &str) -> i32 {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
        i += 1;
    }
    let neg = match b.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let mut v: i64 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        v = (v * 10 + i64::from(b[i] - b'0')).min(1 << 40);
        i += 1;
    }
    let v = if neg { -v } else { v };
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// C `atof`: white space, a sign, decimal digits with a point and an exponent, `inf`/`infinity`/`nan`; else 0.
pub fn atof(s: &str) -> f64 {
    let t = s.trim_start_matches([' ', '\t', '\n', '\r', '\u{b}', '\u{c}']);
    let lower = t.to_ascii_lowercase();
    let (neg, rest) = match lower.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, lower.strip_prefix('+').unwrap_or(&lower)),
    };
    let sign = if neg { -1.0 } else { 1.0 };
    if rest.starts_with("inf") {
        return sign * f64::INFINITY;
    }
    if rest.starts_with("nan") {
        return f64::NAN;
    }
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    let mantissa_end = i;
    if mantissa_end == 0 || (mantissa_end == 1 && b[0] == b'.') {
        return 0.0;
    }
    if i < b.len() && b[i] == b'e' {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let ds = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > ds {
            i = j;
        }
    }
    sign * rest[..i].parse::<f64>().unwrap_or(0.0)
}

/// `dAtob`.
pub fn atob(s: Option<&str>) -> bool {
    let Some(s) = s else {
        return false;
    };
    if s.eq_ignore_ascii_case("true") {
        return true;
    }
    (atof(s) as f32) != 0.0
}

/// A slot argument: NULL is slot 0; `atoi` in 0..3; anything else is `Err`.
fn slot_arg(a: Option<Option<&str>>) -> Result<usize, ()> {
    match a.flatten() {
        None => Ok(0),
        Some(s) => {
            let n = atoi(s);
            if (0..SLOTS as i32).contains(&n) {
                Ok(n as usize)
            } else {
                Err(())
            }
        }
    }
}

/// Runs the state method `name` on the object `host` is bound to. `args` are argv[2..] (the object and the method name are not in
/// it). `None`: not one of the methods this answers, or the argument count is outside its range (the console prints the usage and
/// returns "").
pub fn call(host: &mut dyn ImageHost, name: &str, args: &[Option<&str>]) -> Option<Value> {
    let sp = spec(name)?;
    let argc = args.len() + 2;
    if argc < usize::from(sp.min) || argc > usize::from(sp.max) {
        return None;
    }
    let a = |i: usize| args.get(i).copied();
    let slot = slot_arg(a(0));
    let v = match sp.name {
        "mountImage" => {
            if !host.client_side_call()
                && let Some(db) = a(0).flatten().and_then(|n| host.find_image_data(n))
            {
                // argv[3] is the slot, argv[4] `loaded` (argc 5 only), argv[5] the skin tag (argc 6 only).
                let slot = a(1).flatten().map_or(0, atoi);
                let loaded = if argc == 5 {
                    atob(a(2).flatten())
                } else {
                    true
                };
                let skin = if argc == 6 {
                    a(3).flatten()
                        .and_then(|s| s.strip_prefix('\u{1}'))
                        .map_or(0, atoi) as u32
                } else {
                    0
                };
                if (0..SLOTS as i32).contains(&slot) {
                    machine::mount_image(host, db, slot as usize, loaded, skin);
                }
            }
            Value::Bool(false)
        }
        "unmountImage" => Value::Bool(match slot {
            Ok(s) => machine::unmount_image(host, s),
            Err(()) => false,
        }),
        "getMountedImage" => Value::Int(match slot {
            Ok(s) => host.images().slots[s].db.map_or(0, |d| d as i32),
            Err(()) => 0,
        }),
        "getPendingImage" => Value::Int(match slot {
            Ok(s) => host.images().slots[s].pending.map_or(0, |d| d as i32),
            Err(()) => 0,
        }),
        "isImageFiring" => Value::Bool(match slot {
            Ok(s) => current_state(host, s).is_some_and(|(_, st)| st.fire),
            Err(()) => false,
        }),
        "isImageMounted" => {
            let db = a(0).flatten().and_then(|n| host.find_image_data(n));
            Value::Bool(db.is_some_and(|d| mounted_slot(host, d).is_some()))
        }
        "getMountSlot" => {
            let db = a(0).flatten().and_then(|n| host.find_image_data(n));
            Value::Int(
                db.and_then(|d| mounted_slot(host, d))
                    .map_or(-1, |s| s as i32),
            )
        }
        "getImageSkinTag" => Value::Int(match slot {
            Ok(s) => {
                let sl = &host.images().slots[s];
                if sl.db.is_some() { sl.skin as i32 } else { 0 }
            }
            Err(()) => -1,
        }),
        "getImageState" => Value::Str(match slot {
            Ok(s) => current_state(host, s).map(|(_, st)| st.name.clone()),
            Err(()) => Some("Error".to_string()),
        }),
        "getImageTrigger" => Value::Bool(match slot {
            Ok(s) => {
                let sl = &host.images().slots[s];
                sl.db.is_some() && sl.trigger_down
            }
            Err(()) => false,
        }),
        "setImageTrigger" => Value::Bool(match slot {
            Ok(s) => machine::set_image_trigger(host, s, atob(a(1).flatten())),
            Err(()) => false,
        }),
        "getImageAmmo" => Value::Bool(match slot {
            Ok(s) => {
                let sl = &host.images().slots[s];
                sl.db.is_some() && sl.ammo
            }
            Err(()) => false,
        }),
        "setImageAmmo" => Value::Bool(match slot {
            Ok(s) => {
                let on = atob(a(1).flatten());
                machine::set_image_ammo(host, s, on);
                on
            }
            Err(()) => false,
        }),
        "getImageLoaded" => Value::Bool(match slot {
            Ok(s) => {
                let sl = &host.images().slots[s];
                sl.db.is_some() && sl.loaded
            }
            Err(()) => false,
        }),
        "setImageLoaded" => Value::Bool(match slot {
            Ok(s) => {
                let on = atob(a(1).flatten());
                machine::set_image_loaded(host, s, on);
                on
            }
            Err(()) => false,
        }),
        _ => return None,
    };
    Some(v)
}

/// The slot's datablock and current state record, as `getImageState` and `isImageFiring` read them.
fn current_state(
    host: &mut dyn ImageHost,
    slot: usize,
) -> Option<(std::sync::Arc<ImageData>, super::ImageState)> {
    let sl = &host.images().slots[slot];
    let (db, idx) = (sl.db?, sl.state);
    let data = host.image_data(db)?;
    let st = data.states.get(idx)?.clone();
    Some((data, st))
}

/// The first slot that holds `db` (`getMountSlot`, `isImageMounted`).
fn mounted_slot(host: &mut dyn ImageHost, db: DbId) -> Option<usize> {
    (0..SLOTS).find(|&i| host.images().slots[i].db == Some(db))
}

/// `getMuzzleVector(slot)`: the vector of [`muzzle::muzzle_vector`] normalised by [`muzzle::console_normalize`]; "0 1 0"
/// for a slot outside 0..3. The host prints it with `%g %g %g`. `player` picks `Player`'s overrides.
pub fn get_muzzle_vector(
    host: &mut dyn ImageHost,
    ctx: &mut MuzzleCtx,
    slot_arg_: Option<&str>,
    player: bool,
) -> F3 {
    let Ok(slot) = slot_arg(Some(slot_arg_)) else {
        return [0.0, 1.0, 0.0];
    };
    let img = slot_image(host, slot);
    let v = muzzle::muzzle_vector(ctx, img.as_deref(), slot, player);
    muzzle::console_normalize(v)
}

/// `getMuzzlePoint(slot)`: `None` for a client-side call (the script gets NULL); "0 0 0" for a slot outside 0..3.
pub fn get_muzzle_point(
    host: &mut dyn ImageHost,
    ctx: &mut MuzzleCtx,
    slot_arg_: Option<&str>,
    player: bool,
) -> Option<F3> {
    if host.client_side_call() {
        return None;
    }
    let Ok(slot) = slot_arg(Some(slot_arg_)) else {
        return Some([0.0; 3]);
    };
    let img = slot_image(host, slot);
    Some(if player {
        muzzle::player_muzzle_point(ctx, img.as_deref(), slot)
    } else {
        muzzle::shape_muzzle_point(ctx, img.as_deref(), slot)
    })
}

fn slot_image(host: &mut dyn ImageHost, slot: usize) -> Option<std::sync::Arc<ImageData>> {
    let db = host.images().slots[slot].db?;
    host.image_data(db)
}
