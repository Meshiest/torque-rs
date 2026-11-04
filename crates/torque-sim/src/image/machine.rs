//! The state machine: `updateImageState`, `setImageState`, `setImage`, `setImageTrigger`, `mountImage` and `unmountImage`,
//! `setImageAmmo` and `setImageLoaded`, server side.
//!
//! **Notes on the order of the calls:**
//! - the script callbacks are `state_script` (the state's `script` pointer, from `setImageState`), and `setImage` passes the
//!   literals `onUnmount` and `onMount`; the object is the slot's datablock read at the call, and `onMount` is
//!   called whether or not the first state's script left the slot mounted (Torque then reads a NULL `SimObject`; this crate skips it);
//! - the same-state re-entry tests only the script pointer, so an empty script name still calls the callback there, while the
//!   normal path also tests that the name is non-empty; this crate passes both to [`ImageHost::state_script`], and the host finds no
//!   function named "";
//! - `setImageAmmo` and `setImageLoaded` take the bool argument parsed as `dAtob`: `stricmp(s, "true") == 0` or `atof(s) != 0` as a float, NaN true.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/shapeImage.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

use std::sync::Arc;

use super::{
    DbId, IMAGE_DT, ImageData, ImageHost, MAX_IMAGE_CHAIN, MAX_IMAGE_DEPTH, SLOTS, slot_mask,
};

/// Takes one level of nesting; false (and a counted cap trip) past [`MAX_IMAGE_DEPTH`]. The outermost call resets the chain budget.
fn enter(host: &mut dyn ImageHost) -> bool {
    let s = host.images();
    if s.depth >= MAX_IMAGE_DEPTH {
        s.cap_hits += 1;
        return false;
    }
    if s.depth == 0 {
        s.steps = 0;
    }
    s.depth += 1;
    true
}

fn leave(host: &mut dyn ImageHost) {
    let s = host.images();
    s.depth = s.depth.saturating_sub(1);
}

/// One transition of the chain; false (and a counted cap trip) past [`MAX_IMAGE_CHAIN`].
fn step(host: &mut dyn ImageHost) -> bool {
    let s = host.images();
    if s.steps >= MAX_IMAGE_CHAIN {
        s.cap_hits += 1;
        return false;
    }
    s.steps += 1;
    true
}

/// The `processTick` loop: every slot, in order, with the 0.032f step. A slot that is empty when its turn comes
/// (a script of an earlier slot may have changed it) does nothing.
pub fn tick_images(host: &mut dyn ImageHost) {
    for slot in 0..SLOTS {
        update_image_state(host, slot, IMAGE_DT);
    }
}

/// `updateImageState(slot, dt)`: the timer, the energy and ammo flip, then the transition scan (loaded, ammo, target, wet,
/// trigger, timeout; the first hit wins and goes through [`set_image_state`]).
pub fn update_image_state(host: &mut dyn ImageHost, slot: usize, dt: f32) {
    if slot >= SLOTS || !enter(host) {
        return;
    }
    update_inner(host, slot, dt);
    leave(host);
}

fn update_inner(host: &mut dyn ImageHost, slot: usize, dt: f32) {
    let Some(db) = host.images().slots[slot].db else {
        return;
    };
    let Some(data) = host.image_data(db) else {
        return;
    };
    // The state record is the one read at the top, whatever happens to the slot before the scan.
    let Some(st) = data.states.get(host.images().slots[slot].state) else {
        return;
    };
    // `delay - dt`, rounded to a float at the store.
    {
        let s = &mut host.images().slots[slot];
        s.delay -= dt;
    }
    if data.uses_energy {
        // `energy - drain * dt` (the library table's `image_energy`); below zero is zero (NaN stays).
        let e = host.energy();
        let mut p = (crate::library::get().image_energy)(e, st.energy_drain, dt);
        if 0.0 > p {
            p = 0.0;
        }
        host.set_energy(p);
        let a = data.min_energy < p;
        if a != host.images().slots[slot].ammo {
            host.raise_mask(slot_mask(slot));
            host.images().slots[slot].ammo = a;
        }
    }
    let s = &host.images().slots[slot];
    let delay = s.delay;
    // `delay <= 0.0` (ordered), or the state does not wait for its timeout.
    // Not `delay > 0.0 && wait`: a NaN delay is not `<= 0.0` and not `> 0.0`.
    #[allow(clippy::nonminimal_bool)]
    let waiting = !(delay <= 0.0 || !st.wait_for_timeout);
    if waiting {
        return;
    }
    let next = [
        st.loaded[usize::from(s.loaded)],
        st.ammo[usize::from(s.ammo)],
        st.target[usize::from(s.target)],
        st.wet[usize::from(s.wet)],
        st.trigger[usize::from(s.trigger_down)],
    ]
    .into_iter()
    .find(|&n| n != -1)
    .or_else(|| (delay <= 0.0 && st.timeout != -1).then_some(st.timeout));
    if let Some(n) = next {
        set_image_state(host, slot, n as usize, false);
    }
}

/// `setImageState(slot, state, force)`, the server path. A chained transition (an immediate one on entry, or the timeout
/// of a state whose timer is 0) restarts the loop with `force` false; see the module docs for the order of the steps.
pub fn set_image_state(host: &mut dyn ImageHost, slot: usize, state: usize, force: bool) {
    if slot >= SLOTS || !enter(host) {
        return;
    }
    state_loop(host, slot, state, force);
    leave(host);
}

fn state_loop(host: &mut dyn ImageHost, slot: usize, mut new: usize, mut force: bool) {
    loop {
        // The loop head: the datablock is read again; a script may have unmounted the slot.
        let Some(db) = host.images().slots[slot].db else {
            return;
        };
        let Some(data) = host.image_data(db) else {
            return;
        };
        let Some(st) = data.states.get(new) else {
            return;
        };
        if !step(host) {
            return;
        }
        // The fire state animates the owner's shape (every pass, the re-entry included).
        if new as i32 == data.fire_state {
            host.fire_animate();
        }
        // The same state again, not forced: the timer restarts and the script runs (the pointer test only).
        if !force && host.images().slots[slot].state == new {
            host.images().slots[slot].delay = st.timeout_value;
            if let Some(script) = &st.script {
                host.state_script(db, script, slot);
            }
            return;
        }
        // The new state is current from here on.
        host.images().slots[slot].state = new;
        // A pending image waits for a state that allows the change.
        if st.allow_image_change {
            let s = &host.images().slots[slot];
            if let Some(p) = s.pending {
                let (skin, loaded) = (s.pending_skin, s.pending_loaded);
                set_image(host, slot, Some(p), skin, loaded, false, false, false);
                return;
            }
        }
        // The immediate transitions: no timer, no waitForTimeout, no timeout transition.
        let s = &host.images().slots[slot];
        let immediate = [
            st.loaded[usize::from(s.loaded)],
            st.ammo[usize::from(s.ammo)],
            st.target[usize::from(s.target)],
            st.wet[usize::from(s.wet)],
            st.trigger[usize::from(s.trigger_down)],
        ]
        .into_iter()
        .find(|&n| n != -1);
        if let Some(n) = immediate {
            new = n as usize;
            force = false;
            continue;
        }
        // The timer and the loaded flag.
        let s = &mut host.images().slots[slot];
        s.delay = st.timeout_value;
        if st.loaded_flag != 0 {
            s.loaded = st.loaded_flag == 1;
        }
        // The fire state counts a shot.
        if new as i32 == data.fire_state {
            host.raise_mask(slot_mask(slot));
            let s = &mut host.images().slots[slot];
            s.fire_count = (s.fire_count + 1) & 7;
        }
        // Recoil.
        if st.recoil != 0 {
            host.on_recoil(slot, st.recoil);
        }
        // The state's script, synchronously.
        if let Some(script) = st.script.as_deref().filter(|s| !s.is_empty()) {
            host.state_script(db, script, slot);
        }
        // A state whose timer is not exactly 0 stays (the field is read again after the script); a timer of 0 leaves at once by
        // the timeout transition of the record read before the script.
        if host.images().slots[slot].delay != 0.0 {
            return;
        }
        if st.timeout == -1 {
            return;
        }
        new = st.timeout as usize;
        force = false;
    }
}

/// `setImage(slot, newDb, skin, loaded, ammo, trigger, target)`, the server path.
#[allow(clippy::too_many_arguments)]
pub fn set_image(
    host: &mut dyn ImageHost,
    slot: usize,
    new_db: Option<DbId>,
    skin: u32,
    loaded: bool,
    ammo: bool,
    trigger: bool,
    target: bool,
) {
    if slot >= SLOTS || !enter(host) {
        return;
    }
    set_image_inner(host, slot, new_db, skin, loaded, ammo, trigger, target);
    leave(host);
}

#[allow(clippy::too_many_arguments)]
fn set_image_inner(
    host: &mut dyn ImageHost,
    slot: usize,
    new_db: Option<DbId>,
    skin: u32,
    loaded: bool,
    ammo: bool,
    trigger: bool,
    target: bool,
) {
    let cur = host.images().slots[slot].db;
    if cur == new_db {
        // The same image: it cancels a pending change; a new skin goes to the clients.
        let s = &mut host.images().slots[slot];
        s.pending = None;
        if s.skin != skin {
            s.skin = skin;
            host.raise_mask(slot_mask(slot));
        }
        return;
    }
    if let (Some(n), Some(c)) = (new_db, cur) {
        // A different image over a mounted one waits for a state that allows the change.
        let idx = host.images().slots[slot].state;
        let allow = host
            .image_data(c)
            .and_then(|dd: Arc<ImageData>| dd.states.get(idx).map(|s| s.allow_image_change))
            .unwrap_or(true);
        if !allow {
            let s = &mut host.images().slots[slot];
            s.pending = Some(n);
            s.pending_skin = skin;
            s.pending_loaded = loaded;
            return;
        }
    }
    host.raise_mask(slot_mask(slot));
    if let Some(c) = host.images().slots[slot].db {
        host.on_unmount(c, slot);
    }
    // The teardown: the target and the wet flag are not written.
    {
        let s = &mut host.images().slots[slot];
        s.db = None;
        s.pending = None;
        s.skin = 0;
        s.pending_skin = 0;
        s.state = 0;
        s.delay = 0.0;
        s.trigger_down = false;
        s.ammo = false;
        s.loaded = false;
    }
    if let Some(n) = new_db {
        {
            let s = &mut host.images().slots[slot];
            s.db = Some(n);
            s.state = 0;
            s.skin = skin;
            s.loaded = loaded;
            s.ammo = ammo;
            s.trigger_down = trigger;
            s.target = target;
        }
        set_image_state(host, slot, 0, true);
        if let Some(live) = host.images().slots[slot].db {
            host.on_mount(live, slot);
        }
    }
}

/// `mountImage(db, slot, loaded, skin)`: the same image with the same skin only cancels a pending change; else the image
/// is mounted with ammo, trigger and target clear. Always true.
pub fn mount_image(
    host: &mut dyn ImageHost,
    db: DbId,
    slot: usize,
    loaded: bool,
    skin: u32,
) -> bool {
    if slot >= SLOTS {
        return false;
    }
    let s = &mut host.images().slots[slot];
    if s.db == Some(db) && s.skin == skin {
        s.pending = None;
        return true;
    }
    set_image(host, slot, Some(db), skin, loaded, false, false, false);
    if host.image_data(db).is_some_and(|dd| dd.light_type != 0) {
        host.light_registry(true);
    }
    true
}

/// `unmountImage(slot)`: an occupied slot is emptied (whatever `allowImageChange` says) and the answer is true; then the
/// light registry is told when no mounted image has a `lightType`.
pub fn unmount_image(host: &mut dyn ImageHost, slot: usize) -> bool {
    if slot >= SLOTS {
        return false;
    }
    let mut r = false;
    if host.images().slots[slot].db.is_some() {
        set_image(host, slot, None, 0, true, false, false, false);
        r = true;
    }
    let mut any_light = false;
    for i in 0..SLOTS {
        if let Some(db) = host.images().slots[i].db
            && host.image_data(db).is_some_and(|dd| dd.light_type != 0)
        {
            any_light = true;
            break;
        }
    }
    if !any_light {
        host.light_registry(false);
    }
    r
}

/// `setImageTrigger(slot, down)`: a change raises the mask and runs `updateImageState(slot, 0.0)` at once (the timer is
/// unchanged; a state that waits for its timeout does not leave early). Returns the trigger flag afterwards, false for an empty slot
/// (what the console method answers).
pub fn set_image_trigger(host: &mut dyn ImageHost, slot: usize, down: bool) -> bool {
    if slot >= SLOTS {
        return false;
    }
    let s = &mut host.images().slots[slot];
    if s.db.is_none() {
        return false;
    }
    if s.trigger_down != down {
        s.trigger_down = down;
        host.raise_mask(slot_mask(slot));
        update_image_state(host, slot, 0.0);
    }
    let s = &host.images().slots[slot];
    s.db.is_some() && s.trigger_down
}

/// `setImageAmmo(slot, on)`: written only when the image does not use energy (the energy step owns the flag then) and the
/// value differs. No immediate update: the next tick sees it.
pub fn set_image_ammo(host: &mut dyn ImageHost, slot: usize, on: bool) {
    if slot >= SLOTS {
        return;
    }
    let Some(db) = host.images().slots[slot].db else {
        return;
    };
    let uses_energy = host.image_data(db).is_some_and(|dd| dd.uses_energy);
    let s = &mut host.images().slots[slot];
    if !uses_energy && s.ammo != on {
        s.ammo = on;
        host.raise_mask(slot_mask(slot));
    }
}

/// `setImageLoaded(slot, on)`: written when the value differs (no energy gate). No immediate update.
pub fn set_image_loaded(host: &mut dyn ImageHost, slot: usize, on: bool) {
    if slot >= SLOTS {
        return;
    }
    let s = &mut host.images().slots[slot];
    if s.db.is_some() && s.loaded != on {
        s.loaded = on;
        host.raise_mask(slot_mask(slot));
    }
}

/// `updateImageState`'s energy step: `energy - drain * dt`. [`PLAIN`](crate::library::PLAIN)'s `image_energy`.
pub fn plain_energy(energy: f32, drain: f32, dt: f32) -> f32 {
    energy - drain * dt
}
