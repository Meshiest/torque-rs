//! The image state machine on hand-worked chains (each expected tick count and value is worked from `delay - 0.032f` in single
//! precision, with an independent float32 script for the numbers), the console methods, the muzzle chain over a mock ray cast,
//! determinism, and a hostile fuzz (NaN and infinite timers, zero-timeout cycles, out-of-range transitions, scripts that re-enter).

use std::collections::VecDeque;

use torque_collide::math::{Box3, F3, Matrix};
use torque_collide::{ObjectId, TypeMasks};

use super::console::{self, Value};
use super::driver::{Action, CallbackKind, Event, ScriptedHost, TickRecord, run};
use super::host::{AimClient, RayCaster};
use super::muzzle::{self, MuzzleCtx};
use super::*;
use crate::RayHit;
use crate::player::{NoSkeleton, PlayerData, PlayerState, TGE_ACTION_NUMBERS};

const SLOT0: u32 = 0x10_0000;

fn st(name: &str) -> ImageState {
    ImageState {
        name: name.to_string(),
        ..ImageState::default()
    }
}

fn data(id: DbId, states: Vec<ImageState>) -> ImageData {
    let mut d = ImageData::new(id);
    d.states = states;
    d.finish();
    d
}

/// A basic gun: Activate (0.1 s) -> Ready -(trigger down)-> Fire (0.12 s, fire, `onFire`) -> Reload (0.3 s) -> Ready.
fn gun(id: DbId) -> ImageData {
    let mut activate = st("Activate");
    activate.timeout_value = 0.1;
    activate.wait_for_timeout = true;
    activate.timeout = 1;
    let mut ready = st("Ready");
    ready.trigger[1] = 2;
    let mut fire = st("Fire");
    fire.fire = true;
    fire.timeout_value = 0.12;
    fire.wait_for_timeout = true;
    fire.timeout = 3;
    fire.script = Some("onFire".to_string());
    let mut reload = st("Reload");
    reload.timeout_value = 0.3;
    reload.wait_for_timeout = true;
    reload.timeout = 1;
    data(id, vec![activate, ready, fire, reload])
}

fn mount(db: DbId, slot: usize) -> Event {
    Event::Do(Action::Mount {
        db,
        slot,
        loaded: true,
        skin: 0,
    })
}

fn trig(slot: usize, down: bool) -> Event {
    Event::Do(Action::Trigger { slot, down })
}

fn names(recs: &[TickRecord], slot: usize) -> Vec<Option<&str>> {
    recs.iter()
        .map(|r| r.slots[slot].state.as_deref())
        .collect()
}

#[test]
fn the_step_is_one_tick_and_the_slot_masks_are_bits_20_to_23() {
    assert_eq!(IMAGE_DT, crate::DT);
    assert_eq!(slot_mask(0), 0x10_0000);
    assert_eq!(slot_mask(3), 0x80_0000);
}

#[test]
fn a_plain_gun_activates_fires_reloads_and_refires_on_a_held_trigger() {
    let mut h = ScriptedHost::new();
    h.add(gun(1));
    // The mount at tick 0; the trigger down at tick 5 and up at tick 20.
    let ev = vec![(0, mount(1, 0)), (5, trig(0, true)), (20, trig(0, false))];
    let recs = run(&mut h, &ev, 36, 0);
    let n = names(&recs, 0);
    // Activate: 0.1 s is four ticks (0.068, 0.036, 0.004, -0.028): Ready from tick 3.
    assert_eq!(&n[0..3], &[Some("Activate"); 3]);
    assert_eq!(n[3], Some("Ready"));
    assert_eq!(n[4], Some("Ready"));
    // The trigger at tick 5 enters Fire at once (the 0-dt update); 0.12 s is four ticks: Reload at tick 8.
    assert_eq!(&n[5..8], &[Some("Fire"); 3]);
    // Reload 0.3 s is ten ticks: Ready at tick 18, where the held trigger enters Fire in the same call (an immediate transition of
    // Ready: no Ready delay, no script).
    assert_eq!(&n[8..18], &[Some("Reload"); 10]);
    assert_eq!(&n[18..22], &[Some("Fire"); 4]);
    // The trigger released at tick 20 does not end Fire early (waitForTimeout); Reload at tick 22, Ready at tick 32.
    assert_eq!(&n[22..32], &[Some("Reload"); 10]);
    assert_eq!(n[32], Some("Ready"));
    assert_eq!(n[35], Some("Ready"));
    // Two shots: the counter and the callbacks with their sim times (32 ms per tick).
    assert_eq!(recs[35].slots[0].fire_count, 2);
    let all: Vec<_> = recs
        .iter()
        .flat_map(|r| r.callbacks.iter())
        .map(|c| (c.time_ms, c.kind, c.name.as_str()))
        .collect();
    assert_eq!(
        all,
        vec![
            (0, CallbackKind::OnMount, "onMount"),
            (5 * 32, CallbackKind::Script, "onFire"),
            (18 * 32, CallbackKind::Script, "onFire"),
        ]
    );
    // The mask: the mount (tick 0), the trigger and the shot (5), the shot (18), the trigger release (20); state changes alone raise
    // nothing.
    let masked: Vec<u32> = recs
        .iter()
        .filter(|r| r.mask != 0)
        .map(|r| r.tick)
        .collect();
    assert_eq!(masked, vec![0, 5, 18, 20]);
    assert!(recs.iter().all(|r| r.mask == 0 || r.mask == SLOT0));
    // The delay counted down in float32: 0.1 - 4 * 0.032f at the Ready entry.
    assert_eq!(h.images.cap_hits, 0);
}

#[test]
fn each_pass_through_the_fire_state_animates_the_owner_s_shape() {
    // Every `setImageState` pass whose state is `fireState` animates the shape (two shots, one animation each).
    let mut h = ScriptedHost::new();
    h.add(gun(1));
    let ev = vec![(0, mount(1, 0)), (5, trig(0, true)), (20, trig(0, false))];
    let _ = run(&mut h, &ev, 36, 0);
    assert_eq!(h.fire_animations, 2);
}

#[test]
fn the_timer_counts_down_in_single_precision() {
    let mut h = ScriptedHost::new();
    h.add(gun(1));
    let recs = run(&mut h, &[(0, mount(1, 0))], 4, 0);
    // 0.1f - 0.032f = 0.068, then 0.036, 0.004 (float32 values, not decimal).
    let want = [0.1_f32 - IMAGE_DT, 0.1_f32 - IMAGE_DT - IMAGE_DT];
    assert_eq!(recs[0].slots[0].delay, want[0]);
    assert!((recs[1].slots[0].delay - want[1]).abs() < 1e-6);
    assert!((recs[2].slots[0].delay - 0.004).abs() < 1e-6);
    // The state entered at tick 3 sets the delay of Ready, 0.
    assert_eq!(recs[3].slots[0].delay, 0.0);
}

#[test]
fn the_trigger_goes_up_and_down_with_a_zero_step_update() {
    let mut idle = st("Idle");
    idle.trigger[1] = 1;
    let mut held = st("Held");
    held.trigger[0] = 0;
    let mut h = ScriptedHost::new();
    h.add(data(7, vec![idle, held]));
    machine::mount_image(&mut h, 7, 2, true, 0);
    assert_eq!(h.images.slots[2].state, 0);
    h.mask = 0;
    // Down: at once, without a tick (the 0-dt update).
    assert!(machine::set_image_trigger(&mut h, 2, true));
    assert_eq!(h.images.slots[2].state, 1);
    assert_eq!(h.mask, slot_mask(2));
    // The same value again: no event.
    h.mask = 0;
    assert!(machine::set_image_trigger(&mut h, 2, true));
    assert_eq!(h.mask, 0);
    // Up: back to Idle.
    assert!(!machine::set_image_trigger(&mut h, 2, false));
    assert_eq!(h.images.slots[2].state, 0);
    // An empty slot answers false and raises nothing.
    h.mask = 0;
    assert!(!machine::set_image_trigger(&mut h, 3, true));
    assert_eq!(h.mask, 0);
}

#[test]
fn a_waiting_state_does_not_leave_on_the_trigger() {
    let mut a = st("A");
    a.timeout_value = 0.5;
    a.wait_for_timeout = true;
    a.trigger[1] = 1;
    a.timeout = 2;
    let b = st("B");
    let c = st("C");
    let mut h = ScriptedHost::new();
    h.add(data(1, vec![a, b, c]));
    machine::mount_image(&mut h, 1, 0, true, 0);
    machine::set_image_trigger(&mut h, 0, true);
    // delay 0.5 > 0 and waitForTimeout: the trigger transition waits.
    assert_eq!(h.images.slots[0].state, 0);
    for _ in 0..16 {
        machine::tick_images(&mut h);
    }
    // 0.5 s = 16 ticks of 0.032 = 0.512: the scan on the last tick finds delay <= 0 and takes the trigger transition (before timeout).
    assert_eq!(h.images.slots[0].state, 1);
}

#[test]
fn ammo_flips_the_state_on_the_next_tick_and_energy_images_own_the_byte() {
    let mut ready = st("Ready");
    ready.ammo[0] = 1;
    let mut empty = st("Empty");
    empty.ammo[1] = 0;
    let mut h = ScriptedHost::new();
    h.add(data(1, vec![ready, empty]));
    let recs = run(
        &mut h,
        &[
            (0, mount(1, 0)),
            (2, Event::Do(Action::Ammo { slot: 0, on: true })),
            (4, Event::Do(Action::Ammo { slot: 0, on: false })),
        ],
        6,
        0,
    );
    // Mounted without ammo: Ready's immediate scan (ammo false) goes to Empty during the mount.
    assert_eq!(recs[0].slots[0].state.as_deref(), Some("Empty"));
    assert_eq!(recs[1].slots[0].state.as_deref(), Some("Empty"));
    // setImageAmmo at tick 2 raises the mask but changes no state until the tick's update (same tick here: events come first).
    assert_eq!(recs[2].slots[0].state.as_deref(), Some("Ready"));
    assert_eq!(recs[2].mask, SLOT0);
    assert_eq!(recs[3].slots[0].state.as_deref(), Some("Ready"));
    assert_eq!(recs[4].slots[0].state.as_deref(), Some("Empty"));

    // An image that uses energy: setImageAmmo is ignored; the energy step sets the flag (and the mask) itself.
    let mut e = st("Ready");
    e.energy_drain = 5.0;
    let mut d = data(2, vec![e]);
    d.uses_energy = true;
    d.min_energy = 9.8;
    let mut h = ScriptedHost::new();
    h.add(d);
    h.energy = 10.0;
    machine::mount_image(&mut h, 2, 1, true, 0);
    h.mask = 0;
    machine::set_image_ammo(&mut h, 1, true);
    assert!(!h.images.slots[1].ammo);
    assert_eq!(h.mask, 0);
    machine::update_image_state(&mut h, 1, IMAGE_DT);
    // 10 - 5 * 0.032f = 9.84 (float32 9.84000015258789) > 9.8: ammo on.
    assert_eq!(h.energy, 9.84_f32);
    assert!(h.images.slots[1].ammo);
    assert_eq!(h.mask, slot_mask(1));
    h.mask = 0;
    machine::update_image_state(&mut h, 1, IMAGE_DT);
    // 9.68 is not above 9.8: ammo off again.
    assert_eq!(h.energy, 9.68_f32);
    assert!(!h.images.slots[1].ammo);
    assert_eq!(h.mask, slot_mask(1));
    // The energy never goes below zero.
    h.energy = 0.1;
    machine::update_image_state(&mut h, 1, IMAGE_DT);
    assert_eq!(h.energy, 0.0);
}

#[test]
fn a_wet_flip_changes_state_and_the_wet_byte_survives_a_remount() {
    let mut dry = st("Dry");
    dry.wet[1] = 1;
    let mut wet = st("Wet");
    wet.wet[0] = 0;
    let mut h = ScriptedHost::new();
    h.add(data(1, vec![dry, wet]));
    let recs = run(
        &mut h,
        &[
            (0, mount(1, 0)),
            (2, Event::Wet(true)),
            (4, Event::Do(Action::Unmount { slot: 0 })),
            (5, mount(1, 0)),
        ],
        7,
        0,
    );
    assert_eq!(recs[1].slots[0].state.as_deref(), Some("Dry"));
    assert_eq!(recs[2].slots[0].state.as_deref(), Some("Wet"));
    assert!(recs[2].slots[0].wet);
    assert_eq!(recs[4].slots[0].db, None);
    // The teardown does not write the wet flag: the new mount is wet and its first state's immediate scan goes to Wet.
    assert!(recs[5].slots[0].wet);
    assert_eq!(recs[5].slots[0].state.as_deref(), Some("Wet"));
}

/// The transition order when several conditions hold at once, in the tick's scan and the immediate
/// scan on entry: loaded, ammo, target, wet, trigger, the first that names a state wins (the order is observable). Hub goes to
/// L on not loaded, A on no ammo, G on a target, W on wet, T on the trigger down.
#[test]
fn loaded_ammo_target_wet_and_trigger_together_go_by_the_scan_order() {
    let mut hub = st("Hub");
    hub.loaded[0] = 1;
    hub.ammo[0] = 2;
    hub.target[1] = 3;
    hub.wet[1] = 4;
    hub.trigger[1] = 5;
    hub.timeout_value = 1.0;
    let states = vec![hub, st("L"), st("A"), st("G"), st("W"), st("T")];
    let mut h = ScriptedHost::new();
    h.add(data(1, states));
    machine::mount_image(&mut h, 1, 0, true, 0);
    // Sets the five conditions: `on[k]` makes condition k hold.
    let set = |h: &mut ScriptedHost, on: [bool; 5]| {
        let s = &mut h.images.slots[0];
        s.loaded = !on[0];
        s.ammo = !on[1];
        s.target = on[2];
        s.wet = on[3];
        s.trigger_down = on[4];
    };
    let name = |h: &ScriptedHost| ["Hub", "L", "A", "G", "W", "T"][h.images.slots[0].state];
    let expect = ["L", "A", "G", "W", "T"];
    for first in 0..5 {
        // Every condition from `first` on holds at once.
        let mut on = [false; 5];
        on[first..].fill(true);
        // The tick's scan: in Hub with nothing holding, then all of them at once.
        set(&mut h, [false; 5]);
        machine::set_image_state(&mut h, 0, 0, true);
        assert_eq!(name(&h), "Hub");
        set(&mut h, on);
        machine::update_image_state(&mut h, 0, IMAGE_DT);
        assert_eq!(name(&h), expect[first], "tick scan from {first}");
        // The immediate scan: Hub entered with all of them holding.
        machine::set_image_state(&mut h, 0, 0, true);
        assert_eq!(name(&h), expect[first], "entry scan from {first}");
        // Only the last condition and the first: the first still wins (each pair against the trigger, the scan's last).
        if first < 4 {
            let mut pair = [false; 5];
            pair[first] = true;
            pair[4] = true;
            set(&mut h, pair);
            machine::set_image_state(&mut h, 0, 0, true);
            assert_eq!(name(&h), expect[first], "pair {first} with the trigger");
        }
    }
    // Nothing holding: Hub stays, on entry and at the tick.
    set(&mut h, [false; 5]);
    machine::set_image_state(&mut h, 0, 0, true);
    machine::update_image_state(&mut h, 0, IMAGE_DT);
    assert_eq!(name(&h), "Hub");
}

#[test]
fn immediate_transitions_skip_the_delay_and_the_script_of_the_state_entered() {
    // A -(loaded)-> B -(no ammo)-> C. Mounted loaded without ammo: A and B are passed through; only C sets its timer, flags and script.
    let mut a = st("A");
    a.loaded[1] = 1;
    a.timeout_value = 9.0;
    a.script = Some("scriptA".to_string());
    a.fire = true;
    let mut b = st("B");
    b.ammo[0] = 2;
    b.timeout_value = 8.0;
    b.script = Some("scriptB".to_string());
    let mut c = st("C");
    c.timeout_value = 0.25;
    c.script = Some("scriptC".to_string());
    c.loaded_flag = 2;
    let mut h = ScriptedHost::new();
    h.add(data(1, vec![a, b, c]));
    machine::mount_image(&mut h, 1, 0, true, 0);
    let s = &h.images.slots[0];
    assert_eq!(s.state, 2);
    assert_eq!(s.delay, 0.25);
    // C's loaded flag (anything but 0 and 1 clears it) ran; A's fire state (the first state with fire) was never entered for real.
    assert!(!s.loaded);
    assert_eq!(s.fire_count, 0);
    let log: Vec<_> = h.log.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(log, vec!["scriptC", "onMount"]);
}

#[test]
fn a_zero_timer_chains_through_timeouts_and_runs_each_script() {
    let mut a = st("A");
    a.timeout = 1;
    a.script = Some("sa".to_string());
    let mut b = st("B");
    b.timeout = 2;
    b.script = Some("sb".to_string());
    let mut c = st("C");
    c.timeout_value = 0.5;
    c.script = Some("sc".to_string());
    let mut h = ScriptedHost::new();
    h.add(data(1, vec![a, b, c]));
    machine::mount_image(&mut h, 1, 0, true, 0);
    assert_eq!(h.images.slots[0].state, 2);
    let log: Vec<_> = h.log.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(log, vec!["sa", "sb", "sc", "onMount"]);
    // NaN is not equal to 0: a state with a NaN timer stays.
    let mut n = st("N");
    n.timeout_value = f32::NAN;
    n.timeout = 0;
    let mut h = ScriptedHost::new();
    h.add(data(2, vec![n]));
    machine::mount_image(&mut h, 2, 0, true, 0);
    assert!(h.images.slots[0].delay.is_nan());
    machine::tick_images(&mut h);
    assert_eq!(h.images.slots[0].state, 0);
}

#[test]
fn a_zero_timeout_cycle_stops_at_the_chain_cap() {
    let mut a = st("A");
    a.timeout = 1;
    let mut b = st("B");
    b.timeout = 0;
    let mut h = ScriptedHost::new();
    h.add(data(1, vec![a, b]));
    machine::mount_image(&mut h, 1, 0, true, 0);
    assert_eq!(h.images.cap_hits, 1);
    assert_eq!(h.images.depth, 0);
    assert!(h.images.slots[0].db.is_some());
}

#[test]
fn the_same_state_again_restarts_the_timer_and_runs_the_script_only() {
    let mut a = st("Loop");
    a.timeout_value = 0.1;
    a.timeout = 0;
    a.fire = true;
    a.script = Some("tick".to_string());
    let mut h = ScriptedHost::new();
    h.add(data(1, vec![a]));
    let recs = run(&mut h, &[(0, mount(1, 0))], 9, 0);
    // The mount enters the fire state (count 1); the timeout transition to itself is the same-state path: delay reset, script, and no
    // second shot.
    assert_eq!(recs[8].slots[0].fire_count, 1);
    let scripts: Vec<u32> = recs
        .iter()
        .flat_map(|r| r.callbacks.iter())
        .filter(|c| c.name == "tick")
        .map(|c| c.time_ms)
        .collect();
    // Mount (tick 0), then the timeouts at ticks 3 and 7 (0.1 s is four ticks).
    assert_eq!(scripts, vec![0, 3 * 32, 7 * 32]);
}

#[test]
fn the_fire_counter_wraps_at_eight() {
    let mut ready = st("Ready");
    ready.trigger[1] = 1;
    let mut fire = st("Fire");
    fire.fire = true;
    fire.trigger[0] = 0;
    let mut h = ScriptedHost::new();
    h.add(data(1, vec![ready, fire]));
    machine::mount_image(&mut h, 1, 0, true, 0);
    for _ in 0..9 {
        machine::set_image_trigger(&mut h, 0, true);
        machine::set_image_trigger(&mut h, 0, false);
    }
    assert_eq!(h.images.slots[0].fire_count, 1);
}

#[test]
fn a_mount_over_a_state_that_forbids_the_change_waits_for_one_that_allows_it() {
    // Image 1: Busy (no change) -(timeout 0.1)-> Free (change allowed). Image 2: one state.
    let mut busy = st("Busy");
    busy.timeout_value = 0.1;
    busy.wait_for_timeout = true;
    busy.timeout = 1;
    let mut free = st("Free");
    free.allow_image_change = true;
    let mut h = ScriptedHost::new();
    h.add(data(1, vec![busy, free]));
    h.add(data(2, vec![st("Two")]));
    let ev = vec![
        (0, mount(1, 0)),
        (1, mount(2, 0)),
        (2, mount(1, 0)),
        (3, mount(2, 0)),
    ];
    let recs = run(&mut h, &ev, 8, 0);
    // Tick 1: image 2 is deferred: no mask, no callbacks, image 1 still there with image 2 pending.
    assert_eq!(recs[1].mask, 0);
    assert!(recs[1].callbacks.is_empty());
    assert_eq!(recs[1].slots[0].db, Some(1));
    // Tick 2: mounting image 1 again cancels the pending change (the same image and skin).
    assert_eq!(recs[2].mask, 0);
    // Tick 3: pending again; Busy's 0.1 s run out at tick 3 (four ticks from the mount): Free is entered, and the pending image is
    // swapped in by that entry, in the same call: onUnmount(1) then onMount(2).
    assert_eq!(recs[3].slots[0].db, Some(2));
    let log: Vec<_> = recs[3].callbacks.iter().map(|c| (c.kind, c.db)).collect();
    assert_eq!(
        log,
        vec![(CallbackKind::OnUnmount, 1), (CallbackKind::OnMount, 2)]
    );
    assert_eq!(recs[3].mask, SLOT0);
    assert_eq!(recs[3].slots[0].state.as_deref(), Some("Two"));
    // The deferral is also cancelled by an unmount: unmountImage ignores allowImageChange.
    let mut busy = st("Busy");
    busy.timeout_value = 5.0;
    busy.wait_for_timeout = true;
    let mut h = ScriptedHost::new();
    h.add(data(1, vec![busy]));
    h.add(data(2, vec![st("Two")]));
    machine::mount_image(&mut h, 1, 0, true, 0);
    machine::mount_image(&mut h, 2, 0, true, 0);
    assert_eq!(h.images.slots[0].pending, Some(2));
    assert!(machine::unmount_image(&mut h, 0));
    assert_eq!(h.images.slots[0].db, None);
    assert_eq!(h.images.slots[0].pending, None);
    assert!(!machine::unmount_image(&mut h, 0));
}

#[test]
fn a_new_skin_on_the_same_image_raises_the_mask_only() {
    let mut h = ScriptedHost::new();
    h.add(data(1, vec![st("A")]));
    machine::mount_image(&mut h, 1, 0, true, 0);
    h.mask = 0;
    h.log.clear();
    machine::mount_image(&mut h, 1, 0, true, 7);
    assert_eq!(h.mask, SLOT0);
    assert_eq!(h.images.slots[0].skin, 7);
    assert!(h.log.is_empty());
}

#[test]
fn a_script_that_releases_the_trigger_changes_state_under_the_loop() {
    // Fire (no waitForTimeout, 0.1 s -> Reload) with `onFire` releasing the trigger. The nested update takes Fire's trigger-up
    // transition to Ready; the outer loop then reads the delay again (Ready's 0) and takes the timeout of the Fire record it holds:
    // Reload.
    let mut ready = st("Ready");
    ready.trigger[1] = 1;
    let mut fire = st("Fire");
    fire.fire = true;
    fire.timeout_value = 0.1;
    fire.timeout = 2;
    fire.trigger[0] = 0;
    fire.script = Some("onFire".to_string());
    let reload = st("Reload");
    let mut h = ScriptedHost::new();
    h.add(data(1, vec![ready, fire, reload]));
    h.hook(
        1,
        "onFire",
        vec![Action::Trigger {
            slot: 0,
            down: false,
        }],
    );
    machine::mount_image(&mut h, 1, 0, true, 0);
    assert_eq!(h.images.slots[0].state, 0);
    machine::set_image_trigger(&mut h, 0, true);
    assert_eq!(
        h.images.slots[0].state, 2,
        "Reload, via the stale Fire record"
    );
    assert!(!h.images.slots[0].trigger_down);
    assert_eq!(h.images.slots[0].fire_count, 1);
    assert_eq!(h.images.depth, 0);
}

#[test]
fn a_script_that_unmounts_its_own_image_ends_the_chain() {
    let mut fire = st("Fire");
    fire.timeout = 0;
    fire.script = Some("gone".to_string());
    let mut h = ScriptedHost::new();
    h.add(data(1, vec![fire]));
    h.hook(1, "gone", vec![Action::Unmount { slot: 0 }]);
    machine::mount_image(&mut h, 1, 0, true, 0);
    assert_eq!(h.images.slots[0].db, None);
    // onUnmount ran inside the script; onMount is not called on a slot that is no longer mounted (the object is NULL there).
    let kinds: Vec<_> = h.log.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, vec![CallbackKind::Script, CallbackKind::OnUnmount]);
}

#[test]
fn an_unmount_script_that_mounts_again_is_cut_off_by_the_depth_cap() {
    let mut h = ScriptedHost::new();
    let mut a = st("A");
    a.allow_image_change = true;
    let mut b = st("B");
    b.allow_image_change = true;
    h.add(data(1, vec![a]));
    h.add(data(2, vec![b]));
    h.hook(
        1,
        "onUnmount",
        vec![Action::Mount {
            db: 2,
            slot: 0,
            loaded: true,
            skin: 0,
        }],
    );
    h.hook(
        2,
        "onUnmount",
        vec![Action::Mount {
            db: 1,
            slot: 0,
            loaded: true,
            skin: 0,
        }],
    );
    machine::mount_image(&mut h, 1, 0, true, 0);
    machine::mount_image(&mut h, 2, 0, true, 0);
    assert!(h.images.cap_hits > 0);
    assert_eq!(h.images.depth, 0);
}

#[test]
fn mount_and_unmount_order_the_callbacks_and_clear_state() {
    let mut a = st("A");
    a.script = Some("enter".to_string());
    a.allow_image_change = true;
    let mut h = ScriptedHost::new();
    h.add(data(1, vec![a]));
    h.add(data(2, vec![st("B")]));
    h.images.slots[1].target = true;
    h.images.slots[1].wet = true;
    machine::mount_image(&mut h, 1, 1, true, 0);
    // Mount: the first state's script, then onMount.
    let log: Vec<_> = h.log.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(log, vec!["enter", "onMount"]);
    // The mount sets target to the argument (false); the teardown leaves wet alone.
    assert!(!h.images.slots[1].target);
    assert!(h.images.slots[1].wet);
    h.images.slots[1].ammo = true;
    h.images.slots[1].trigger_down = true;
    h.log.clear();
    machine::mount_image(&mut h, 2, 1, true, 0);
    // A different image over a state that allows the change: onUnmount of the old one first, then the new one's entry and onMount.
    let log: Vec<_> = h.log.iter().map(|c| (c.kind, c.db)).collect();
    assert_eq!(
        log,
        vec![(CallbackKind::OnUnmount, 1), (CallbackKind::OnMount, 2)]
    );
    assert!(!h.images.slots[1].ammo && !h.images.slots[1].trigger_down);
}

#[test]
fn light_images_tell_the_registry() {
    let mut d = data(1, vec![st("A")]);
    d.light_type = 2;
    let mut h = ScriptedHost::new();
    h.add(d);
    h.add(data(2, vec![st("B")]));
    machine::mount_image(&mut h, 1, 0, true, 0);
    machine::mount_image(&mut h, 2, 1, true, 0);
    machine::unmount_image(&mut h, 1);
    machine::unmount_image(&mut h, 0);
    assert_eq!(h.light_calls, vec![true, false]);
}

#[test]
fn the_image_table_resolves_transition_names_as_onadd_does() {
    let raw = vec![
        RawState {
            name: Some("Ready".into()),
            on_trigger_down: Some("FIRE".into()),
            ..RawState::default()
        },
        RawState {
            name: Some("Fire".into()),
            fire: true,
            on_timeout: Some("Nowhere".into()),
            timeout_value: 0.1,
            ..RawState::default()
        },
    ];
    let (d, errors) = ImageData::from_raw(5, &raw);
    // Case-insensitive; an empty name is none; an unknown name is an error and state 0.
    assert_eq!(d.states[0].trigger, [-1, 1]);
    assert_eq!(d.states[1].timeout, 0);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("Could not resolve state \"Nowhere\""));
    assert_eq!(d.fire_state, 1);
    // More than 31 states: the rest are not read.
    let many: Vec<RawState> = (0..40)
        .map(|i| RawState {
            name: Some(format!("S{i}")),
            ..RawState::default()
        })
        .collect();
    let (d, _) = ImageData::from_raw(6, &many);
    assert_eq!(d.states.len(), MAX_STATES);
}

#[test]
fn the_console_table_and_argument_parsing() {
    assert_eq!(console::METHODS.len(), 18);
    let sp = console::spec("MOUNTIMAGE").unwrap();
    assert_eq!((sp.min, sp.max), (4, 6));
    assert_eq!(
        console::spec("setImageAmmo").map(|s| (s.min, s.max)),
        Some((4, 4))
    );
    assert_eq!(
        console::spec("getImageState").map(|s| (s.min, s.max)),
        Some((3, 3))
    );
    assert!(console::spec("nope").is_none());
    assert_eq!(console::atoi("  -12abc"), -12);
    assert_eq!(console::atoi("99999999999"), i32::MAX);
    assert_eq!(console::atoi(""), 0);
    assert!(console::atob(Some("TRUE")));
    assert!(console::atob(Some("1")));
    assert!(console::atob(Some("0.5")));
    assert!(!console::atob(Some("0")));
    assert!(!console::atob(Some("false")));
    assert!(!console::atob(Some("")));
    assert!(!console::atob(None));
    assert!(console::atob(Some("nan")));
    assert_eq!(console::atof("1.5e2x"), 150.0);
    assert_eq!(console::atof(".x"), 0.0);
}

struct ConsoleHost {
    inner: ScriptedHost,
}

impl ImageHost for ConsoleHost {
    fn images(&mut self) -> &mut ImageSet {
        self.inner.images()
    }
    fn image_data(&mut self, db: DbId) -> Option<std::sync::Arc<ImageData>> {
        self.inner.image_data(db)
    }
    fn raise_mask(&mut self, bits: u32) {
        self.inner.raise_mask(bits);
    }
    fn energy(&mut self) -> f32 {
        self.inner.energy()
    }
    fn set_energy(&mut self, e: f32) {
        self.inner.set_energy(e);
    }
    fn state_script(&mut self, db: DbId, script: &str, slot: usize) {
        self.inner.state_script(db, script, slot);
    }
    fn on_mount(&mut self, db: DbId, slot: usize) {
        self.inner.on_mount(db, slot);
    }
    fn on_unmount(&mut self, db: DbId, slot: usize) {
        self.inner.on_unmount(db, slot);
    }
    fn find_image_data(&mut self, name: &str) -> Option<DbId> {
        match name {
            "GunImage" | "1" => Some(1),
            "OtherImage" | "2" => Some(2),
            _ => None,
        }
    }
}

fn call(h: &mut ConsoleHost, name: &str, args: &[&str]) -> Option<Value> {
    let a: Vec<Option<&str>> = args.iter().map(|s| Some(*s)).collect();
    console::call(h, name, &a)
}

#[test]
fn the_console_methods_answer_as_documented() {
    let mut inner = ScriptedHost::new();
    inner.add(gun(1));
    inner.add(data(2, vec![st("Two")]));
    let mut h = ConsoleHost { inner };
    // mountImage always answers false; with six arguments `loaded` is not read (true) and the skin is a tagged string.
    assert_eq!(
        call(&mut h, "mountImage", &["GunImage", "1", "0", "\u{1}77"]),
        Some(Value::Bool(false))
    );
    assert_eq!(h.inner.images.slots[1].db, Some(1));
    assert!(h.inner.images.slots[1].loaded);
    assert_eq!(h.inner.images.slots[1].skin, 77);
    // Five arguments read `loaded`.
    call(&mut h, "mountImage", &["OtherImage", "2", "false"]);
    assert_eq!(h.inner.images.slots[2].db, Some(2));
    assert!(!h.inner.images.slots[2].loaded);
    // A slot outside 0..3 and an unknown datablock mount nothing.
    call(&mut h, "mountImage", &["GunImage", "4"]);
    call(&mut h, "mountImage", &["Missing", "0"]);
    assert_eq!(h.inner.images.slots[0].db, None);
    // The argument count is checked by the caller's table; out of range answers None.
    assert_eq!(call(&mut h, "mountImage", &["GunImage"]), None);
    assert_eq!(call(&mut h, "getMountedImage", &[]), None);
    assert_eq!(call(&mut h, "getMountedImage", &["1"]), Some(Value::Int(1)));
    assert_eq!(call(&mut h, "getMountedImage", &["0"]), Some(Value::Int(0)));
    assert_eq!(call(&mut h, "getMountedImage", &["9"]), Some(Value::Int(0)));
    assert_eq!(
        call(&mut h, "getMountSlot", &["GunImage"]),
        Some(Value::Int(1))
    );
    assert_eq!(
        call(&mut h, "getMountSlot", &["Missing"]),
        Some(Value::Int(-1))
    );
    assert_eq!(
        call(&mut h, "isImageMounted", &["OtherImage"]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(&mut h, "getImageSkinTag", &["1"]),
        Some(Value::Int(77))
    );
    assert_eq!(
        call(&mut h, "getImageSkinTag", &["-1"]),
        Some(Value::Int(-1))
    );
    assert_eq!(
        call(&mut h, "getImageState", &["1"]),
        Some(Value::Str(Some("Activate".into())))
    );
    assert_eq!(
        call(&mut h, "getImageState", &["0"]),
        Some(Value::Str(None))
    );
    assert_eq!(
        call(&mut h, "getImageState", &["7"]),
        Some(Value::Str(Some("Error".into())))
    );
    assert_eq!(
        call(&mut h, "isImageFiring", &["1"]),
        Some(Value::Bool(false))
    );
    // Trigger: set answers the flag after; get; the bool argument is dAtob.
    assert_eq!(
        call(&mut h, "setImageTrigger", &["1", "true"]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(&mut h, "getImageTrigger", &["1"]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(&mut h, "setImageTrigger", &["0", "1"]),
        Some(Value::Bool(false))
    );
    assert_eq!(
        call(&mut h, "setImageTrigger", &["8", "1"]),
        Some(Value::Bool(false))
    );
    // Ammo and loaded answer the argument; getters read the flags.
    assert_eq!(
        call(&mut h, "setImageAmmo", &["1", "yes"]),
        Some(Value::Bool(false))
    );
    assert_eq!(
        call(&mut h, "setImageAmmo", &["1", "2"]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(&mut h, "getImageAmmo", &["1"]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(&mut h, "setImageLoaded", &["1", "0"]),
        Some(Value::Bool(false))
    );
    assert_eq!(
        call(&mut h, "getImageLoaded", &["1"]),
        Some(Value::Bool(false))
    );
    assert_eq!(call(&mut h, "getPendingImage", &["1"]), Some(Value::Int(0)));
    // unmountImage.
    assert_eq!(
        call(&mut h, "unmountImage", &["1"]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(&mut h, "unmountImage", &["1"]),
        Some(Value::Bool(false))
    );
    assert_eq!(
        call(&mut h, "unmountImage", &["5"]),
        Some(Value::Bool(false))
    );
    // getSlotTransform is registered but not modelled here.
    assert_eq!(call(&mut h, "getSlotTransform", &["0"]), None);
}

// ---- the muzzle chain ----

struct MockRays {
    hits: VecDeque<Option<RayHit>>,
    calls: Vec<(F3, F3, u32, bool)>,
}

impl MockRays {
    fn new(hits: Vec<Option<RayHit>>) -> MockRays {
        MockRays {
            hits: hits.into(),
            calls: Vec::new(),
        }
    }
}

impl RayCaster for MockRays {
    fn cast_ray(&mut self, start: F3, end: F3, mask: u32, exclude_mount: bool) -> Option<RayHit> {
        self.calls.push((start, end, mask, exclude_mount));
        self.hits.pop_front().flatten()
    }
}

fn hit_at(p: F3, type_mask: u32) -> Option<RayHit> {
    Some(RayHit {
        object: ObjectId(99),
        point: p,
        normal: [0.0, 0.0, 1.0],
        type_mask,
    })
}

fn player_at(pos: F3) -> PlayerState {
    let mut s = PlayerState::new(ObjectId(1), &PlayerData::default(), pos, 0.0);
    s.world_box = Box3::new(
        [pos[0] - 1.0, pos[1] - 1.0, pos[2] - 1.0],
        [pos[0] + 1.0, pos[1] + 1.0, pos[2] + 3.0],
    );
    s
}

fn image_with_offset() -> ImageData {
    let mut d = ImageData::new(1);
    d.mount_transform[3] = 1.0;
    d.mount_transform[7] = 2.0;
    d.mount_transform[11] = 3.0;
    d
}

#[test]
fn the_muzzle_point_is_the_muzzle_transforms_translation_and_casts_no_ray() {
    let s = player_at([10.0, 20.0, 30.0]);
    let img = image_with_offset();
    for player in [true, false] {
        let mut skel = NoSkeleton;
        let mut rays = MockRays::new(vec![hit_at([10.0, 20.0, 30.25], 0)]);
        let mut ctx = MuzzleCtx {
            s: &s,
            skel: &mut skel,
            rays: &mut rays,
            first_person: false,
            client: None,
            actions: TGE_ACTION_NUMBERS,
        };
        let p = if player {
            muzzle::player_muzzle_point(&mut ctx, Some(&img), 0)
        } else {
            muzzle::shape_muzzle_point(&mut ctx, Some(&img), 0)
        };
        // Torque's getMuzzlePoint: column 3 of the muzzle transform (10 + 1, 20 + 2, 30 + 3).
        assert_eq!(p, [11.0, 22.0, 33.0], "player {player}");
        assert!(rays.calls.is_empty());
    }
}

/// A ray caster that moves the muzzle point halfway back to the mount point.
struct Halfway(MockRays, Vec<(F3, F3, bool)>);

impl RayCaster for Halfway {
    fn cast_ray(&mut self, start: F3, end: F3, mask: u32, exclude_mount: bool) -> Option<RayHit> {
        self.0.cast_ray(start, end, mask, exclude_mount)
    }

    fn muzzle_point(&mut self, _s: &PlayerState, mount: F3, muzzle: F3, player: bool) -> F3 {
        self.1.push((mount, muzzle, player));
        std::array::from_fn(|i| (mount[i] + muzzle[i]) * 0.5)
    }
}

#[test]
fn a_host_may_move_the_muzzle_point_from_the_mount_and_muzzle_translations() {
    let s = player_at([10.0, 20.0, 30.0]);
    let img = image_with_offset();
    let mut skel = NoSkeleton;
    let mut rays = Halfway(MockRays::new(vec![]), Vec::new());
    let mut ctx = MuzzleCtx {
        s: &s,
        skel: &mut skel,
        rays: &mut rays,
        first_person: false,
        client: None,
        actions: TGE_ACTION_NUMBERS,
    };
    let p = muzzle::shape_muzzle_point(&mut ctx, Some(&img), 0);
    assert_eq!(p, [10.5, 21.0, 31.5]);
    assert_eq!(
        rays.1,
        vec![([10.0, 20.0, 30.0], [11.0, 22.0, 33.0], false)]
    );
}

#[test]
fn the_player_muzzle_transform_rebuilds_the_rotation_from_the_pitch() {
    let mut s = player_at([10.0, 20.0, 30.0]);
    let img = image_with_offset();
    let mut skel = NoSkeleton;
    let mut rays = MockRays::new(vec![]);
    s.action_anim = 0;
    s.head = [0.0, 0.0, 0.0];
    let mut ctx = MuzzleCtx {
        s: &s,
        skel: &mut skel,
        rays: &mut rays,
        first_person: false,
        client: None,
        actions: TGE_ACTION_NUMBERS,
    };
    let m = muzzle::player_muzzle_transform(&mut ctx, Some(&img), 0);
    // Pitch 0: the rotation is the object's (identity); the translation is the base transform's.
    assert_eq!([m[0], m[5], m[10]], [1.0, 1.0, 1.0]);
    assert_eq!([m[3], m[7], m[11]], [11.0, 22.0, 33.0]);
    // An action past the table's fixed ones leaves the base transform: TGE's table has eight.
    s.action_anim = 8;
    let mut skel = NoSkeleton;
    let mut ctx = MuzzleCtx {
        s: &s,
        skel: &mut skel,
        rays: &mut rays,
        first_person: false,
        client: None,
        actions: TGE_ACTION_NUMBERS,
    };
    let m = muzzle::player_muzzle_transform(&mut ctx, Some(&img), 0);
    assert_eq!([m[3], m[7], m[11]], [11.0, 22.0, 33.0]);
}

#[test]
fn the_muzzle_node_and_the_first_person_eye_offset_shape_the_transform() {
    let s = player_at([10.0, 20.0, 30.0]);
    let mut img = image_with_offset();
    // A muzzle node 0.5 above the image origin: translation (0, 0, 0.5) in the image shape.
    let mut node = torque_collide::math::IDENTITY;
    node[11] = 0.5;
    img.muzzle_node = 3;
    img.muzzle_node_transform = Some(node);
    let mut skel = NoSkeleton;
    let mut rays = MockRays::new(vec![]);
    let mut ctx = MuzzleCtx {
        s: &s,
        skel: &mut skel,
        rays: &mut rays,
        first_person: false,
        client: None,
        actions: TGE_ACTION_NUMBERS,
    };
    let m = muzzle::image_node_transform(&mut ctx, &img);
    assert_eq!([m[3], m[7], m[11]], [11.0, 22.0, 33.5]);
    // First person with an image that asks for the eye: the eye transform times the eye offset (0, 0, 1) instead of the mount.
    img.first_person = true;
    img.eye_offset[11] = 1.0;
    let mut ctx = MuzzleCtx {
        s: &s,
        skel: &mut skel,
        rays: &mut rays,
        first_person: true,
        client: None,
        actions: TGE_ACTION_NUMBERS,
    };
    let t = muzzle::image_transform(&mut ctx, Some(&img));
    assert_eq!([t[3], t[7], t[11]], [10.0, 20.0, 31.0]);
    // The owner not in first person: the mount path again.
    let mut ctx = MuzzleCtx {
        s: &s,
        skel: &mut skel,
        rays: &mut rays,
        first_person: false,
        client: None,
        actions: TGE_ACTION_NUMBERS,
    };
    let t = muzzle::image_transform(&mut ctx, Some(&img));
    assert_eq!([t[3], t[7], t[11]], [11.0, 22.0, 33.0]);
}

#[test]
fn a_slot_mounted_by_an_earlier_slots_script_is_ticked_in_the_same_pass() {
    // Slot 0: a state whose timeout is itself (0.01 s): each timeout is a same-state re-entry that runs the script.
    let mut a = st("A");
    a.timeout_value = 0.01;
    a.timeout = 0;
    a.script = Some("mountTwo".to_string());
    let mut b = st("B");
    b.timeout_value = 0.1;
    let mut h = ScriptedHost::new();
    h.add(data(1, vec![a]));
    h.add(data(2, vec![b]));
    machine::mount_image(&mut h, 1, 0, true, 0);
    h.hook(
        1,
        "mountTwo",
        vec![Action::Mount {
            db: 2,
            slot: 2,
            loaded: true,
            skin: 0,
        }],
    );
    machine::tick_images(&mut h);
    // Slot 2 was empty when the pass began and was mounted by slot 0's script: its own turn comes after and counts its timer down once.
    assert_eq!(h.images.slots[2].db, Some(2));
    assert!((h.images.slots[2].delay - 0.068).abs() < 1e-6);
}

/// TGE's aim: the unit vector `to * (1 / |to|)`.
fn tge_aim(to: F3) -> F3 {
    let inv = 1.0 / (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
    std::array::from_fn(|i| to[i] * inv)
}

#[test]
fn the_muzzle_vector_is_corrected_toward_what_lies_ahead_of_the_eye() {
    let s = player_at([10.0, 20.0, 30.0]);
    let mut img = image_with_offset();
    img.correct_muzzle_vector = true;
    let client = Some(AimClient {
        freelook_value: 0.0,
        ai: false,
    });
    // The muzzle is (11, 22, 33); the eye ray runs from (10, 20, 30) along +y.
    let run_vec = |img: &ImageData, hits: Vec<Option<RayHit>>, client: Option<AimClient>| {
        let mut skel = NoSkeleton;
        let mut rays = MockRays::new(hits);
        let mut ctx = MuzzleCtx {
            s: &s,
            skel: &mut skel,
            rays: &mut rays,
            first_person: false,
            client,
            actions: TGE_ACTION_NUMBERS,
        };
        let v = muzzle::muzzle_vector(&mut ctx, Some(img), 0, true);
        (v, rays.calls)
    };
    let (v, calls) = run_vec(&img, vec![hit_at([10.0, 120.0, 30.0], 0)], client);
    assert_eq!(v, tge_aim([-1.0, 98.0, -3.0]));
    // The aim ray: 500 units along the eye's forward axis, every type, only the object itself left out.
    assert_eq!(
        calls[0],
        (
            [10.0, 20.0, 30.0],
            [10.0, 520.0, 30.0],
            TypeMasks::TGE.muzzle_aim,
            false
        )
    );
    assert_eq!(TypeMasks::TGE.muzzle_aim, u32::MAX);
    // No hit: the aim is at the end of the ray.
    assert_eq!(
        run_vec(&img, vec![None], client).0,
        tge_aim([-1.0, 498.0, -3.0])
    );
    // A hit nearer than 6 (of any type) is pushed out to 6 from the eye: (10, 26, 30).
    let (v, _) = run_vec(&img, vec![hit_at([10.0, 22.0, 30.0], 0x4000)], client);
    assert_eq!(v, tge_aim([-1.0, 4.0, -3.0]));
    // One within 0.2 of the eye is not: the aim goes back to the eye.
    let (v, _) = run_vec(&img, vec![hit_at([10.0, 20.1, 30.0], 0)], client);
    assert_eq!(v, tge_aim([-1.0, 20.1 - 22.0, -3.0]));
    // A third-person client, an AI client, no client, or an image that does not ask: the y axis.
    let third = Some(AimClient {
        freelook_value: 1.0,
        ai: false,
    });
    let ai = Some(AimClient {
        freelook_value: 0.0,
        ai: true,
    });
    for c in [third, ai, None] {
        assert_eq!(
            run_vec(&img, vec![hit_at([10.0, 120.0, 30.0], 0)], c).0,
            [0.0, 1.0, 0.0]
        );
    }
    img.correct_muzzle_vector = false;
    assert_eq!(
        run_vec(&img, vec![hit_at([10.0, 120.0, 30.0], 0)], client).0,
        [0.0, 1.0, 0.0]
    );
}

#[test]
fn the_corrected_aim_declines_when_the_muzzle_is_at_the_aim_point() {
    let s = player_at([10.0, 20.0, 30.0]);
    let eye = crate::player::eye_transform(&s, &mut NoSkeleton);
    // The hit at 2 from the eye is pushed out to (10, 26, 30), where the muzzle is.
    let mut rays = MockRays::new(vec![hit_at([10.0, 22.0, 30.0], 0)]);
    assert_eq!(
        muzzle::corrected_aim(&mut rays, &eye, [10.0, 26.0, 30.0]),
        None
    );
    let mut rays = MockRays::new(vec![hit_at([10.0, 22.0, 30.0], 0)]);
    assert_eq!(
        muzzle::corrected_aim(&mut rays, &eye, [10.0, 26.0, 30.25]),
        Some(tge_aim([0.0, 0.0, -0.25]))
    );
}

/// A ray caster with an aim rule of its own: it records what the muzzle chain hands it.
struct OwnAim(MockRays, Vec<(F3, F3)>);

impl RayCaster for OwnAim {
    fn cast_ray(&mut self, start: F3, end: F3, mask: u32, exclude_mount: bool) -> Option<RayHit> {
        self.0.cast_ray(start, end, mask, exclude_mount)
    }

    fn muzzle_point(&mut self, _s: &PlayerState, _mount: F3, muzzle: F3, _player: bool) -> F3 {
        [muzzle[0], muzzle[1], muzzle[2] + 1.0]
    }

    fn corrected_aim(&mut self, _eye: &Matrix, muzzle: F3, point: F3) -> Option<F3> {
        self.1.push((muzzle, point));
        Some([1.0, 0.0, 0.0])
    }
}

#[test]
fn a_ray_caster_may_replace_the_corrected_aim() {
    let s = player_at([10.0, 20.0, 30.0]);
    let mut img = image_with_offset();
    img.correct_muzzle_vector = true;
    let mut skel = NoSkeleton;
    let mut rays = OwnAim(MockRays::new(vec![]), Vec::new());
    let mut ctx = MuzzleCtx {
        s: &s,
        skel: &mut skel,
        rays: &mut rays,
        first_person: false,
        client: Some(AimClient::default()),
        actions: TGE_ACTION_NUMBERS,
    };
    assert_eq!(
        muzzle::muzzle_vector(&mut ctx, Some(&img), 0, true),
        [1.0, 0.0, 0.0]
    );
    assert_eq!(rays.1, vec![([11.0, 22.0, 33.0], [11.0, 22.0, 34.0])]);
    assert!(rays.0.calls.is_empty());
}

#[test]
fn the_console_vector_normalises_only_above_1e_4() {
    assert_eq!(muzzle::console_normalize([0.0, 2.0, 0.0]), [0.0, 1.0, 0.0]);
    assert_eq!(
        muzzle::console_normalize([1.0, 2.0, 2.0]),
        [0.333_333_34, 0.666_666_7, 0.666_666_7]
    );
    // A component squared below 1e-4 everywhere: the length is 0 and nothing changes.
    assert_eq!(muzzle::fast_len([0.005, 0.0, 0.0]), 0.0);
    assert_eq!(
        muzzle::console_normalize([0.005, 0.0, 0.0]),
        [0.005, 0.0, 0.0]
    );
    let nan = muzzle::console_normalize([f32::NAN, 0.0, 0.0]);
    assert!(nan[0].is_nan());
}

// ---- determinism and the hostile fuzz ----

fn determinism_events() -> Vec<(u32, Event)> {
    vec![
        (0, mount(1, 0)),
        (3, mount(1, 1)),
        (5, trig(0, true)),
        (5, trig(1, true)),
        (9, Event::Wet(true)),
        (14, trig(0, false)),
        (15, Event::Do(Action::Unmount { slot: 1 })),
    ]
}

#[test]
fn the_same_events_give_the_same_records() {
    let build = || {
        let mut h = ScriptedHost::new();
        h.add(gun(1));
        h
    };
    let a = run(&mut build(), &determinism_events(), 60, 1000);
    let b = run(&mut build(), &determinism_events(), 60, 1000);
    assert_eq!(a, b);
    // The records carry the sim time of the start.
    assert_eq!(a[0].time_ms, 1000);
    assert_eq!(a[59].time_ms, 1000 + 59 * 32);
}

/// xorshift64*.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, n: u64) -> bool {
        self.below(n) == 0
    }
}

fn hostile_float(r: &mut Rng) -> f32 {
    const POOL: [f32; 14] = [
        0.0,
        -0.0,
        0.032,
        0.1,
        0.5,
        -1.0,
        1e30,
        -1e30,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::MIN_POSITIVE,
        1e-45,
        3.0,
    ];
    POOL[r.below(POOL.len() as u64) as usize]
}

fn hostile_action(r: &mut Rng) -> Action {
    let slot = r.below(6) as usize;
    match r.below(5) {
        0 => Action::Mount {
            db: 1 + r.below(4) as u32,
            slot,
            loaded: r.chance(2),
            skin: r.below(3) as u32,
        },
        1 => Action::Unmount { slot },
        2 => Action::Trigger {
            slot,
            down: r.chance(2),
        },
        3 => Action::Ammo {
            slot,
            on: r.chance(2),
        },
        _ => Action::Loaded {
            slot,
            on: r.chance(2),
        },
    }
}

fn hostile_host(seed: u64) -> (ScriptedHost, Vec<(u32, Event)>) {
    let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut h = ScriptedHost::new();
    for id in 1..=3u32 {
        let mut d = ImageData::new(id);
        let n = r.below(45) as usize;
        for i in 0..n {
            let mut s = st(&format!("S{i}"));
            let t = |r: &mut Rng| -> i32 {
                if r.chance(3) {
                    r.below(50) as i32 - 5
                } else {
                    -1
                }
            };
            s.loaded = [t(&mut r), t(&mut r)];
            s.ammo = [t(&mut r), t(&mut r)];
            s.target = [t(&mut r), t(&mut r)];
            s.trigger = [t(&mut r), t(&mut r)];
            s.wet = [t(&mut r), t(&mut r)];
            s.timeout = t(&mut r);
            s.fire = r.chance(5);
            s.allow_image_change = r.chance(2);
            s.wait_for_timeout = r.chance(2);
            s.timeout_value = hostile_float(&mut r);
            s.energy_drain = hostile_float(&mut r);
            s.loaded_flag = r.below(4) as i32;
            s.recoil = r.below(3) as i32;
            if r.chance(2) {
                s.script = Some(format!("sc{}", r.below(4)));
            }
            d.states.push(s);
        }
        d.uses_energy = r.chance(3);
        d.min_energy = hostile_float(&mut r);
        d.light_type = r.below(2) as u32;
        // Half of the tables skip `finish`: transitions beyond the table, negative ones, more than 31 states.
        if r.chance(2) {
            d.finish();
        } else {
            d.fire_state = r.below(60) as i32 - 5;
        }
        h.add(d);
        for k in 0..4 {
            if r.chance(2) {
                let n = 1 + r.below(3) as usize;
                let actions = (0..n).map(|_| hostile_action(&mut r)).collect();
                h.hook(id, &format!("sc{k}"), actions);
            }
        }
        if r.chance(3) {
            h.hook(id, "onMount", vec![hostile_action(&mut r)]);
        }
        if r.chance(3) {
            h.hook(id, "onUnmount", vec![hostile_action(&mut r)]);
        }
    }
    h.energy = hostile_float(&mut r);
    let mut events = Vec::new();
    for t in 0..120u32 {
        for _ in 0..r.below(3) {
            let ev = match r.below(8) {
                0 => Event::Wet(r.chance(2)),
                1 => Event::Energy(hostile_float(&mut r)),
                _ => Event::Do(hostile_action(&mut r)),
            };
            events.push((t, ev));
        }
    }
    (h, events)
}

#[test]
fn a_hostile_fuzz_never_panics_and_holds_the_caps() {
    let mut cap_trips = 0;
    for seed in 0..400u64 {
        let (mut h, events) = hostile_host(seed);
        let recs = run(&mut h, &events, 120, 0);
        assert_eq!(recs.len(), 120);
        assert_eq!(h.images.depth, 0, "seed {seed}");
        assert!(h.images.steps <= MAX_IMAGE_CHAIN, "seed {seed}");
        for r in &recs {
            assert!(r.slots.iter().all(|s| s.fire_count < 8), "seed {seed}");
            assert_eq!(r.mask & !0x00f0_0000, 0, "seed {seed}");
        }
        cap_trips += h.images.cap_hits;
        // The same seed again gives the same records.
        let (mut h2, events2) = hostile_host(seed);
        // (NaN timers compare unequal, so the records are compared as text.)
        assert_eq!(
            format!("{:?}", run(&mut h2, &events2, 120, 0)),
            format!("{recs:?}"),
            "seed {seed}"
        );
    }
    // The generator does build cycles and re-entrant scripts: the caps are exercised.
    assert!(cap_trips > 0);
}

#[test]
fn huge_tables_are_cut_to_31_states() {
    let mut d = ImageData::new(1);
    d.states = (0..10_000)
        .map(|i| {
            let mut s = st(&format!("S{i}"));
            s.timeout = (i + 1) % 10_000;
            s
        })
        .collect();
    d.finish();
    assert_eq!(d.states.len(), MAX_STATES);
    assert_eq!(d.states[30].timeout, -1);
    let mut h = ScriptedHost::new();
    h.add(d);
    machine::mount_image(&mut h, 1, 0, true, 0);
    machine::tick_images(&mut h);
}
