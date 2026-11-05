//! `ShapeBase` as data: the shape instance an object keeps, the object box from the shape, the script threads, and the
//! console methods that touch them. The pose arithmetic is `shapes::instance`; this file is
//! the object's side of it.
//!
//! - [`new_datablock`] is `ShapeBase::onNewDataBlock` (the shape instance, the object box, the damage and visibility threads,
//!   the script threads carried over by name)
//! - the type masks ([`ctor_type_mask`]): the constructors: `GameBase` (0x400), `ShapeBase` (0x800), `StaticShape` (0x2001),
//!   `Player` (0x4000), `Vehicle`, `Projectile`, `Item` (0x8000)
//! - `StaticShape::onAdd`: the datablock's `dynamicType` joins the mask, `addToScene`, then the datablock's `onAdd` in script
//! - `playThread`, `setThreadDir`, `stopThread`, `pauseThread`: over `setThreadSequence` and `updateThread`
//! - `getSlotTransform`: over `getMountTransform`
//!
//! **Stale poses.** `getSlotTransform` reads the node matrices as they are. They are made once when the instance is built
//! and again when the engine's own queries animate the instance (`ShapeInstance::animate`: ray casts, polylists, eye and muzzle
//! lookups); `playThread` only sets the thread and marks the sub-shapes dirty.
//!
//! **Elsewhere.** The Player's object box (`Player::onNewDataBlock` builds it from `boxSize`, not from the
//! shape: `ShapeBase` skips the box for `PlayerObjectType`) and its animation threads, Item's and the vehicles' own
//! `onAdd` work, the images, and every use of the mount chain. The bins entry (`addToScene`) is made here for the classes whose
//! `onAdd` ends in it: StaticShape, Item, Player, AIPlayer and the vehicles.
//!
//! **Deviations.** The `0xf0000` thread bits are marked when a datablock is set again even if some thread bit is already
//! pending (the mask word is not readable from here). A datablock whose shape the engine holds header-only
//! (`KeptShape::shape` is `None`) gets no instance.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `T3D/shapeBase.cpp`,
//! `T3D/staticShape.cpp` and `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.);
//! `T3D/item.cpp`, `ts/tsShapeInstance.cpp` and `sim/netStringTable.cpp` (Copyright (c) 2012 GarageGames, LLC). The code here
//! is this crate's own.

use std::rc::Rc;

use shapes::instance::{ShapeInstance, ThreadId, find_sequence};
use shapes::math as fpu;
use ts_vm::classes::EngineClass;
use ts_vm::fields::atob;
use ts_vm::sim::ObjId;
use ts_vm::text::atoi;
use ts_vm::vm::{Native, NativeCall, Vm, VmError};

use super::access::{int_field, matrix_of, object_field, scale_of};
use super::camera::is_class;
use super::container::Container;
use super::ext;
use super::object as scene;
use crate::methods::register_method;
use crate::scene::ghost as ghosts;
use crate::shapes::KeptShape;

/// `GameBase | ShapeBase | StaticShape` (0x400 | 0x800 | 0x2001).
pub const STATIC_SHAPE_TYPE_MASK: u32 = 0x2c01;
/// `GameBase | ShapeBase | Player` (0x4000; `AIPlayer` adds nothing).
pub const PLAYER_TYPE_MASK: u32 = 0x4c00;
/// `GameBase | ShapeBase | Vehicle` (0x10000; `WheeledVehicle` and `FlyingVehicle` add nothing).
pub const VEHICLE_TYPE_MASK: u32 = 0x10c00;
/// `GameBase | ShapeBase | Item`.
pub const ITEM_TYPE_MASK: u32 = 0x8c00;
/// `GameBase | Projectile`.
pub const PROJECTILE_TYPE_MASK: u32 = 0x40400;
/// The `Player` bit `ShapeBase::onNewDataBlock` tests to leave the object box to the class.
pub const PLAYER_BIT: u32 = 0x4000;

/// `ShapeBase::mScriptThread`'s four slots.
pub const SCRIPT_THREADS: usize = 4;
/// The largest sequence index `setThreadSequence` takes (below `MaxSequenceIndex`, 63).
pub const MAX_THREAD_SEQUENCE: i32 = 0x3e;

/// A script thread's state (`ShapeBase::Thread::State`).
pub const THREAD_PLAY: u8 = 0;
pub const THREAD_STOP: u8 = 1;
pub const THREAD_DESTROY: u8 = 2;

/// One of the four threads `playThread` drives (`ShapeBase::Thread`).
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptThread {
    pub thread: Option<ThreadId>,
    pub state: u8,
    /// The sequence index, -1 for none.
    pub sequence: i32,
    pub at_end: bool,
    pub forward: bool,
}

impl Default for ScriptThread {
    fn default() -> ScriptThread {
        // The constructor: no thread, state Stop, sequence -1, forward.
        ScriptThread {
            thread: None,
            state: THREAD_STOP,
            sequence: -1,
            at_end: false,
            forward: true,
        }
    }
}

/// The shape instance of an object and what goes with it.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeBody {
    /// The datablock's shape resource the instance was made from.
    pub kept: Rc<KeptShape>,
    pub instance: ShapeInstance,
    /// The threads of the datablock's `Damage` and `Visibility` sequences.
    pub damage: Option<ThreadId>,
    pub visibility: Option<ThreadId>,
}

impl ShapeBody {
    /// The parsed shape.
    pub fn shape(&self) -> &shapes::Shape {
        // `body` is made only for a kept shape with a body.
        self.kept.shape.as_ref().expect("a body has a shape")
    }
}

/// What a `ShapeBase` object keeps of its shape: the instance, the threads, the fade and the name.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShapeBaseState {
    pub scripts: [ScriptThread; SCRIPT_THREADS],
    pub body: Option<ShapeBody>,
    /// How many instances [`new_datablock`] has made for the object: state a host keeps for one instance (per mesh object,
    /// say) is the current instance's while it was made at this count.
    pub instances: u32,
    /// A Player's own threads and action state (`Player::onNewDataBlock`), for the classes below `Player`.
    pub player: Option<PlayerThreads>,
    /// The net string handles `setShapeName` and `setSkinName` set: a tag of the VM's tag table on which the object
    /// holds one engine reference (let go of when the name is set again and in [`release_names`]), 0 for none.
    pub shape_name: u32,
    pub skin_name: u32,
    /// The sound slots `playAudio` fills (each: playing and the profile).
    pub sounds: [Option<ObjId>; 4],
    /// The fade `startFade` starts.
    pub fade: Fade,
}

/// A ShapeBase's fade: whether it fades out, whether it is fading, the value, the elapsed time, the fade time and the
/// delay (seconds). The constructor sets fading 0, the value 1, the time 1, the elapsed time and the delay 0 and
/// leaves the fade-out byte as allocated (the engine: 0; it is sent only while a fade runs, which `startFade` sets first).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fade {
    pub fade_out: bool,
    pub fading: bool,
    pub value: f32,
    pub elapsed: f32,
    pub time: f32,
    pub delay: f32,
}

impl Default for Fade {
    fn default() -> Fade {
        Fade {
            fade_out: false,
            fading: false,
            value: 1.0,
            elapsed: 0.0,
            time: 1.0,
            delay: 0.0,
        }
    }
}

/// The mask bit of the packer's cloak block (the cloak flag, the control flag and the fade): `startFade` and a fade's delay
/// running out mark it.
pub const FADE_MASK: u32 = 0x100;

/// The step `ShapeBase::processTick` adds to the elapsed time (0.032f as a double).
const FADE_STEP: f32 = 0.032;

/// `startFade(timeMS, delayMS, fadeOut)` (the console method): each time is `sscanf("%d")` read as an UNSIGNED number (a
/// negative one is 2^32 more), made a float and divided by 1000; the flag is `dAtob`. The ghost is
/// marked [`FADE_MASK`]; the elapsed time is 0 and fading is set; each time below 0 is 0 (never, after the unsigned read); the
/// value is the flag (1 for a fade out, 0 for a fade in). A time `sscanf` cannot read is 0.
pub fn start_fade(vm: &mut Vm, id: ObjId, time_ms: &[u8], delay_ms: &[u8], fade_out: bool) {
    let seconds = |text: &[u8]| {
        let mut v = [0i32];
        ts_vm::fields::scan_ints(text, &mut v);
        let f = v[0].cast_unsigned() as f32;
        let s = f / 1000.0;
        if 0.0 > s { 0.0 } else { s }
    };
    let (time, delay) = (seconds(time_ms), seconds(delay_ms));
    ghosts::mark(vm, id, FADE_MASK);
    if let Some(st) = name_state(vm, id) {
        st.fade = Fade {
            fade_out,
            fading: true,
            value: f32::from(u8::from(fade_out)),
            elapsed: 0.0,
            time,
            delay,
        };
    }
}

/// The fade step of `ShapeBase::processTick` (a server object; the classes whose tick runs `ShapeBase::processTick`: the
/// Player, the Camera and the vehicles, NOT the Item, whose `processTick` does not call it): while fading, the elapsed time
/// goes up by 0.032 (stored as a float); the step that takes it from below the delay to the delay or past marks [`FADE_MASK`]
/// (the ghost is then sent the fade); once it is past the fade time plus the delay the fade ends and the value is 1 for a fade
/// in, 0 for a fade out. The compares are on the floats ([`GameLibrary::fade_over`](crate::library::GameLibrary::fade_over) for
/// the end).
pub fn fade_tick(vm: &mut Vm, id: ObjId) {
    let Some(st) = state_mut(vm, id) else {
        return;
    };
    if fade_step(&mut st.fade) {
        ghosts::mark(vm, id, FADE_MASK);
    }
}

/// [`fade_tick`] on the fade itself (a Player's tick holds its shape state outside the object): true when [`FADE_MASK`] is to
/// be marked.
pub fn fade_step(f: &mut Fade) -> bool {
    if !f.fading {
        return false;
    }
    let next = FADE_STEP + f.elapsed;
    let crossed = f.elapsed < f.delay && f.delay <= next;
    f.elapsed = next;
    if (crate::library::get().fade_over)(f.time, f.delay, next) {
        f.fading = false;
        f.value = if f.fade_out { 0.0 } else { 1.0 };
    }
    crossed
}

/// The mask bit of the packer's energy block (the three tint values, the energy and the shape name).
pub const NAME_MASK: u32 = 0x10;

/// The object's fade (the constructor's for an object with no shape state).
pub fn fade_of(vm: &Vm, id: ObjId) -> Fade {
    state(vm, id).map_or_else(Fade::default, |st| st.fade)
}

/// The shape state of an object, if it has one.
pub fn state(vm: &Vm, id: ObjId) -> Option<&ShapeBaseState> {
    ext::get(vm.objects(), id)?.shape.as_deref()
}

fn state_mut(vm: &mut Vm, id: ObjId) -> Option<&mut ShapeBaseState> {
    ext::get_mut(vm.objects_mut(), id)?.shape.as_deref_mut()
}

/// The constructor's type mask for the simulation classes (see the table above), or `None` for any other class.
pub fn ctor_type_mask(vm: &Vm, id: ObjId) -> Option<u32> {
    if is_class(vm, id, "StaticShape") {
        Some(STATIC_SHAPE_TYPE_MASK)
    } else if is_class(vm, id, "Player") {
        Some(PLAYER_TYPE_MASK)
    } else if is_class(vm, id, "WheeledVehicle")
        || is_class(vm, id, "FlyingVehicle")
        || is_class(vm, id, "Vehicle")
    {
        Some(VEHICLE_TYPE_MASK)
    } else if is_class(vm, id, "Item") {
        Some(ITEM_TYPE_MASK)
    } else if is_class(vm, id, "Projectile") {
        Some(PROJECTILE_TYPE_MASK)
    } else {
        None
    }
}

fn arg(c: &NativeCall, i: usize) -> &[u8] {
    c.argv.get(i).map_or(&b""[..], Vec::as_slice)
}

fn type_mask(vm: &Vm, id: ObjId) -> u32 {
    ext::get(vm.objects(), id).map_or(0, |e| e.type_mask)
}

/// `ShapeBase::onNewDataBlock`, the shape part: for a `ShapeBaseData` whose shape resource the engine holds, a new
/// instance (nodes at the default pose), the object box from the shape's bounds (unless the object is a Player), the script
/// threads carried over by sequence name, and the damage and visibility threads. Charged by the instance.
pub fn new_datablock(vm: &mut Vm, id: ObjId, db: ObjId) -> Result<(), VmError> {
    if !is_class(vm, id, "ShapeBase") || !is_class(vm, db, "ShapeBaseData") {
        return Ok(());
    }
    let resource = ext::get(vm.objects(), db).and_then(|e| {
        e.shape_resource
            .as_ref()
            .map(crate::shapes::ShapeResource::current)
    });
    // The object holds its datablock's shape resource (header-only ones too, which the engine gives no instance) until it goes:
    // `purgeResources` frees what no one holds.
    if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
        e.shape_hold = resource.clone();
    }
    let Some(kept) = resource.filter(|k| k.shape.is_some()) else {
        return Ok(());
    };
    let previous = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.shape.take());
    let mut state = previous.unwrap_or_default();
    // The positions the script threads were at, and the shape they played in.
    let old = state.body.take();
    let positions: Vec<Option<f32>> = state
        .scripts
        .iter()
        .map(|s| {
            let thread = s.thread?;
            old.as_ref()?.instance.thread(thread).map(|t| t.pos)
        })
        .collect();
    let built = {
        let shape = kept.shape.as_ref().expect("checked above");
        ShapeInstance::new(shape, &mut |units| vm.charge_work(units))
    };
    let instance = match built {
        Ok(instance) => instance,
        Err(_) => {
            put_state(vm, id, state);
            return Err(VmError::StepLimit);
        }
    };
    let mut body = ShapeBody {
        kept: Rc::clone(&kept),
        instance,
        damage: None,
        visibility: None,
    };
    // The object box is the shape's bounds, for every object that is not a Player.
    if type_mask(vm, id) & PLAYER_BIT == 0 {
        let b = kept.shape.as_ref().map_or([0.0; 6], |s| s.bounds);
        scene::set_object_box(vm, id, [b[0], b[1], b[2]], [b[3], b[4], b[5]]);
    } else if is_class(vm, db, "PlayerData") {
        let field = crate::sim_classes::sim_classes(vm).player_box_field(vm, id);
        player_box(vm, id, db, field);
    }
    // The script threads that were playing play the same sequence of the new shape (found by name), from where they were.
    for (slot, before) in positions.iter().copied().enumerate() {
        if state.scripts[slot].sequence == -1 {
            continue;
        }
        let had = state.scripts[slot].thread.take().is_some();
        let renamed = old
            .as_ref()
            .and_then(|o| o.kept.shape.as_ref())
            .and_then(|s| {
                let seq = usize::try_from(state.scripts[slot].sequence).ok()?;
                s.names
                    .get(usize::try_from(s.sequences.get(seq)?.name).ok()?)
            })
            .and_then(|name| find_sequence(kept.shape.as_ref()?, name));
        let index = renamed.map_or(-1, |i| i32::try_from(i).unwrap_or(-1));
        state.body = Some(body);
        let _ = set_thread_sequence_in(&mut state, slot, index, had);
        body = state.body.take().expect("put just above");
        if let (Some(thread), Some(before)) = (state.scripts[slot].thread, before) {
            let shape = kept.shape.as_ref().expect("checked above");
            let now = body.instance.thread(thread).map_or(0.0, |t| t.pos);
            body.instance.advance_pos(shape, thread, before - now);
        }
    }
    {
        let shape = kept.shape.as_ref().expect("checked above");
        if let Some(i) = find_sequence(shape, b"Damage")
            && let Some(thread) = body.instance.create_thread(shape)
        {
            body.instance.set_sequence(shape, thread, i as i32, 0.0);
            body.damage = Some(thread);
        }
        if let Some(i) = find_sequence(shape, b"Visibility")
            && let Some(thread) = body.instance.create_thread(shape)
        {
            body.instance.set_sequence(shape, thread, i as i32, 0.0);
            body.visibility = Some(thread);
        }
    }
    if is_class(vm, id, "Player") && is_class(vm, db, "PlayerData") {
        let classes = crate::sim_classes::sim_classes(vm);
        let class = |body: &mut ShapeBody| classes.player_class_threads(vm, id, body);
        state.player = player_threads(
            &mut body,
            state.player.as_ref(),
            classes.action_table(),
            class,
        );
    }
    state.body = Some(body);
    state.instances = state.instances.wrapping_add(1);
    put_state(vm, id, state);
    ghosts::mark(vm, id, 0xf0000);
    Ok(())
}

fn put_state(vm: &mut Vm, id: ObjId, state: Box<ShapeBaseState>) {
    if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
        e.shape = Some(state);
    }
}

/// The server's tick step (`TickSec`, the float 0.032).
pub const THREAD_TICK_SEC: f32 = 0.032;

/// One slot of `ShapeBase::advanceThreads(dt)` (the first call of `ShapeBase::processTick`'s server branch): a
/// thread on a sequence that does not cycle and has not ended ends when its position (the transition's while in one) reaches 1
/// playing forward, 0 backwards: `atEnd` is set and `updateThread` holds it there; then the thread runs `dt` seconds at
/// its time scale. True when the slot ended now (the server then calls the datablock's `onEndSequence(obj, slot)`).
pub fn advance_thread_slot(st: &mut ShapeBaseState, slot: usize, dt: f32) -> bool {
    let Some(body) = st.body.as_mut() else {
        return false;
    };
    let Some(script) = st.scripts.get_mut(slot) else {
        return false;
    };
    let Some(thread) = script.thread else {
        return false;
    };
    let kept = Rc::clone(&body.kept);
    let Some(shape) = kept.shape.as_ref() else {
        return false;
    };
    let cyclic = usize::try_from(script.sequence)
        .ok()
        .and_then(|s| shape.sequences.get(s))
        .is_some_and(|s| s.flags & shapes::instance::FLAG_CYCLIC != 0);
    let mut ended = false;
    if !cyclic && !script.at_end {
        let pos = body.instance.thread(thread).map_or(0.0, |t| t.get_pos());
        // `pos < 1.0` forward and `0.0 < pos` backwards keep it going (a NaN ends it).
        let going = if script.forward { pos < 1.0 } else { 0.0 < pos };
        if !going {
            script.at_end = true;
            let snapshot = script.clone();
            update_thread(body, &snapshot);
            ended = true;
        }
    }
    body.instance.advance_time(shape, thread, dt);
    ended
}

/// [`advance_thread_slot`] for every slot of an object whose shape state it holds, each ended slot's `onEndSequence` called
/// before the next slot steps (a script may change the threads).
pub fn advance_threads(vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
    for slot in 0..SCRIPT_THREADS {
        let ended =
            state_mut(vm, id).is_some_and(|st| advance_thread_slot(st, slot, THREAD_TICK_SEC));
        if ended {
            on_end_sequence(vm, id, slot)?;
        }
    }
    Ok(())
}

/// The datablock's `onEndSequence(obj, slot)` (`Con::executef(mDataBlock, 3, ...)`).
pub fn on_end_sequence(vm: &mut Vm, id: ObjId, slot: usize) -> Result<(), VmError> {
    let Some(db) = super::access::object_field(vm, id, b"dataBlock") else {
        return Ok(());
    };
    vm.call_method(
        db,
        b"onEndSequence",
        &[id.to_string().into_bytes(), slot.to_string().into_bytes()],
    )?;
    Ok(())
}

/// `updateThread`: the thread's time scale and position follow its state.
fn update_thread(body: &mut ShapeBody, st: &ScriptThread) {
    let kept = Rc::clone(&body.kept);
    let Some(shape) = kept.shape.as_ref() else {
        return;
    };
    let Some(thread) = st.thread else {
        return;
    };
    let pos = body.instance.thread(thread).map_or(0.0, |t| t.pos);
    let set_scale = |body: &mut ShapeBody, scale: f32| {
        if let Some(t) = body.instance.thread_mut(thread) {
            t.time_scale = scale;
        }
    };
    match st.state {
        THREAD_PLAY => {
            if !st.at_end {
                set_scale(body, if st.forward { 1.0 } else { -1.0 });
                return;
            }
            set_scale(body, 1.0);
            let target = if st.forward { 1.0f32 } else { 0.0 };
            body.instance.advance_pos(shape, thread, target - pos);
        }
        THREAD_STOP => {
            set_scale(body, 1.0);
            body.instance.advance_pos(shape, thread, 0.0 - pos);
        }
        _ => {}
    }
    set_scale(body, 0.0);
}

/// What `setThreadSequence` did: whether it answered true, and whether it set the thread's mask bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequenceSet {
    pub ok: bool,
    pub marked: bool,
}

/// `setThreadSequence` on a state whose body is in place. It answers false only for a sequence index past the
/// limit (and then sets nothing).
fn set_thread_sequence_in(
    state: &mut ShapeBaseState,
    slot: usize,
    seq: i32,
    reset: bool,
) -> SequenceSet {
    let ShapeBaseState { scripts, body, .. } = state;
    let st = &mut scripts[slot];
    if st.thread.is_some() && st.sequence == seq && st.state == THREAD_PLAY {
        if reset && let Some(body) = body.as_mut() {
            st.at_end = false;
            if let (Some(thread), Some(shape)) = (st.thread, body.kept.shape.as_ref()) {
                let pos = body.instance.thread(thread).map_or(0.0, |t| t.pos);
                body.instance.advance_pos(shape, thread, 0.0 - pos);
            }
            let snapshot = st.clone();
            update_thread(body, &snapshot);
            return SequenceSet {
                ok: true,
                marked: true,
            };
        }
        return SequenceSet {
            ok: true,
            marked: false,
        };
    }
    if seq > MAX_THREAD_SEQUENCE {
        return SequenceSet {
            ok: false,
            marked: false,
        };
    }
    let done = SequenceSet {
        ok: true,
        marked: true,
    };
    st.sequence = seq;
    if reset {
        st.state = THREAD_PLAY;
        st.at_end = false;
        st.forward = true;
    }
    let Some(body) = body.as_mut() else {
        return done;
    };
    let Some(shape) = body.kept.shape.as_ref() else {
        return done;
    };
    if st.thread.is_none() {
        st.thread = body.instance.create_thread(shape);
    }
    if let Some(thread) = st.thread {
        body.instance.set_sequence(shape, thread, seq, 0.0);
    }
    match st.state {
        THREAD_PLAY if !st.at_end => {
            if let Some(t) = st.thread.and_then(|t| body.instance.thread_mut(t)) {
                t.time_scale = if st.forward { 1.0 } else { -1.0 };
            }
        }
        THREAD_PLAY | THREAD_STOP => {
            // Both go to a position and stop: Play-at-end to the end it faces, Stop to the start.
            let target = if st.state == THREAD_STOP || !st.forward {
                0.0f32
            } else {
                1.0
            };
            if let Some(thread) = st.thread {
                if let Some(t) = body.instance.thread_mut(thread) {
                    t.time_scale = 1.0;
                }
                let pos = body.instance.thread(thread).map_or(0.0, |t| t.pos);
                body.instance.advance_pos(shape, thread, target - pos);
                if let Some(t) = body.instance.thread_mut(thread) {
                    t.time_scale = 0.0;
                }
            }
        }
        _ => {
            if let Some(t) = st.thread.and_then(|t| body.instance.thread_mut(t)) {
                t.time_scale = 0.0;
            }
        }
    }
    done
}

/// `ShapeBase::setThreadSequence` for an object (the thread's mask bit is marked).
pub fn set_thread_sequence(vm: &mut Vm, id: ObjId, slot: usize, seq: i32, reset: bool) -> bool {
    let Some(mut st) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.shape.take()) else {
        return false;
    };
    let done = set_thread_sequence_in(&mut st, slot, seq, reset);
    put_state(vm, id, st);
    if done.marked {
        ghosts::mark(vm, id, 0x10000 << slot);
    }
    done.ok
}

/// `playThread(slot [, name])`.
fn play_thread(vm: &mut Vm, c: &NativeCall) -> Native {
    let Some(id) = c.this_object else {
        return Native::Bool(false);
    };
    let slot = atoi(arg(c, 2));
    let Ok(slot) = usize::try_from(slot) else {
        return Native::Bool(false);
    };
    if slot >= SCRIPT_THREADS {
        return Native::Bool(false);
    }
    if c.argv.len() == 4 {
        let index = state(vm, id)
            .and_then(|s| s.body.as_ref())
            .and_then(|b| find_sequence(b.shape(), arg(c, 3)));
        let Some(index) = index else {
            return Native::Bool(false);
        };
        let ok = set_thread_sequence(vm, id, slot, i32::try_from(index).unwrap_or(i32::MAX), true);
        return Native::Bool(ok);
    }
    // No name: resume what the slot holds.
    let Some(mut st) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.shape.take()) else {
        return Native::Bool(false);
    };
    let resumes = st.scripts[slot].sequence != -1 && st.scripts[slot].state != THREAD_PLAY;
    if resumes {
        st.scripts[slot].state = THREAD_PLAY;
        let snapshot = st.scripts[slot].clone();
        if let Some(body) = st.body.as_mut() {
            update_thread(body, &snapshot);
        }
    }
    put_state(vm, id, st);
    if resumes {
        ghosts::mark(vm, id, 0x10000 << slot);
    }
    Native::Bool(resumes)
}

/// `stopThread` and `pauseThread`: the slot goes to `state` if it holds a sequence and is not there already.
fn set_thread_state(vm: &mut Vm, c: &NativeCall, to: u8) -> Native {
    let Some(id) = c.this_object else {
        return Native::Bool(false);
    };
    let slot = atoi(arg(c, 2));
    let Some(slot) = usize::try_from(slot).ok().filter(|s| *s < SCRIPT_THREADS) else {
        return Native::Bool(false);
    };
    let Some(mut st) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.shape.take()) else {
        return Native::Bool(false);
    };
    let changes = st.scripts[slot].sequence != -1 && st.scripts[slot].state != to;
    if changes {
        st.scripts[slot].state = to;
        let snapshot = st.scripts[slot].clone();
        if let Some(body) = st.body.as_mut() {
            update_thread(body, &snapshot);
        }
    }
    put_state(vm, id, st);
    if changes {
        ghosts::mark(vm, id, 0x10000 << slot);
    }
    Native::Bool(changes)
}

/// `setThreadDir(slot, forward)`.
fn set_thread_dir(vm: &mut Vm, c: &NativeCall) -> Native {
    let Some(id) = c.this_object else {
        return Native::Bool(false);
    };
    let slot = atoi(arg(c, 2));
    let Some(slot) = usize::try_from(slot).ok().filter(|s| *s < SCRIPT_THREADS) else {
        return Native::Bool(false);
    };
    let forward = atob(arg(c, 3));
    let Some(mut st) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.shape.take()) else {
        return Native::Bool(false);
    };
    let held = st.scripts[slot].sequence != -1;
    let changes = held && st.scripts[slot].forward != forward;
    if changes {
        st.scripts[slot].forward = forward;
        st.scripts[slot].at_end = false;
        let snapshot = st.scripts[slot].clone();
        if let Some(body) = st.body.as_mut() {
            update_thread(body, &snapshot);
        }
    }
    put_state(vm, id, st);
    if changes {
        ghosts::mark(vm, id, 0x10000 << slot);
    }
    Native::Bool(held)
}

/// The mount transform of slot `slot` (`getMountTransform`): the object's transform, times the mount node's matrix with
/// its translation scaled by the object's scale, when the datablock has that mount node and the object an instance.
pub fn mount_transform(vm: &Vm, id: ObjId, slot: usize) -> fpu::Matrix {
    let object = matrix_of(vm, id).unwrap_or(fpu::IDENTITY);
    let Some(body) = state(vm, id).and_then(|s| s.body.as_ref()) else {
        return object;
    };
    let node = body
        .kept
        .facts
        .as_ref()
        .and_then(|f| f.mount_nodes.get(slot).copied())
        .and_then(|n| usize::try_from(n).ok());
    let Some(node) = node else {
        return object;
    };
    let Some(mut m) = body.instance.node_transform(node).copied() else {
        return object;
    };
    let s = scale_of(vm, id);
    m[3] *= s[0];
    m[7] *= s[1];
    m[11] *= s[2];
    fpu::mat_mul(&object, &m)
}

/// `getSlotTransform(slot)`: the slot's mount transform as `x y z axis angle`; a slot past 3 gives the identity's.
fn get_slot_transform(vm: &mut Vm, c: &NativeCall) -> Native {
    let Some(id) = c.this_object else {
        return Native::Str(Vec::new());
    };
    let slot = atoi(arg(c, 2)) as u32;
    let m = if slot <= 3 {
        mount_transform(vm, id, slot as usize)
    } else {
        fpu::IDENTITY
    };
    Native::Str(scene::transform_text(crate::geometry::of(vm), &m))
}

/// `StaticShape::onAdd` after `GameBase::onAdd`: the datablock's `dynamicType` (low 24 bits) joins the type mask, the object
/// enters the container by its box, and the datablock's script `onAdd(%obj)` runs.
struct StaticShapeClass {
    container: Rc<Container>,
}

impl EngineClass for StaticShapeClass {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        let Some(db) = object_field(vm, id, b"dataBlock") else {
            return Ok(false);
        };
        let dynamic = int_field(vm, db, b"dynamicType") as u32 & 0xff_ffff;
        if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
            e.type_mask |= dynamic;
        }
        scene::add_to_scene(vm, &self.container, id);
        vm.call_method(db, b"onAdd", &[id.to_string().into_bytes()])?;
        Ok(true)
    }

    /// `StaticShape::onRemove`: the datablock's script `onRemove(%obj)` first.
    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        script_on_remove(vm, id)
    }
}

/// `GameBase::scriptOnRemove`: `%db.onRemove(%obj)` for a server object that is ghostable and has a datablock. The
/// leaf classes call it first thing in their `onRemove` (Player, Item, StaticShape and FlyingVehicle), before
/// `ShapeBase::onRemove` unmounts. Every object of the engine's server is a server object, and the shape classes are ghostable.
pub fn script_on_remove(vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
    if let Some(db) = object_field(vm, id, b"dataBlock") {
        vm.call_method(db, b"onRemove", &[id.to_string().into_bytes()])?;
    }
    Ok(())
}

/// The classes whose `onAdd` ends in `addToScene` and whose other work is not modelled here: the object enters the container.
struct SceneEntryClass {
    container: Rc<Container>,
}

impl EngineClass for SceneEntryClass {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        scene::add_to_scene(vm, &self.container, id);
        Ok(true)
    }
}

/// Gives the shape classes their behaviours and the `ShapeBase` console methods. After `attach::register` (which owns
/// `GameBase`).
pub fn register(vm: &mut Vm, container: &Rc<Container>) {
    vm.set_class_behaviour(
        "StaticShape",
        Rc::new(StaticShapeClass {
            container: Rc::clone(container),
        }),
    );
    for class in ["Item", "Player", "WheeledVehicle", "FlyingVehicle"] {
        vm.set_class_behaviour(
            class,
            Rc::new(SceneEntryClass {
                container: Rc::clone(container),
            }),
        );
    }
    register_method(vm, "ShapeBase", "playThread", play_thread);
    register_method(vm, "ShapeBase", "setThreadDir", set_thread_dir);
    register_method(vm, "ShapeBase", "stopThread", |vm, c| {
        set_thread_state(vm, c, THREAD_STOP)
    });
    register_method(vm, "ShapeBase", "pauseThread", |vm, c| {
        set_thread_state(vm, c, THREAD_DESTROY)
    });
    register_method(vm, "ShapeBase", "getSlotTransform", get_slot_transform);
    register_method(vm, "ShapeBase", "setShapeName", set_shape_name);
    register_method(vm, "ShapeBase", "setSkinName", set_skin_name);
    register_method(vm, "ShapeBase", "getShapeName", |vm, c| {
        get_name(vm, c, false)
    });
    register_method(vm, "ShapeBase", "getSkinName", |vm, c| {
        get_name(vm, c, true)
    });
    let w = Rc::clone(container);
    register_method(vm, "ShapeBase", "setHidden", move |vm, c| {
        if let Some(id) = c.this_object {
            set_hidden(vm, &w, id, ts_vm::fields::atob(arg(c, 2)));
        }
        Native::Void
    });
    register_method(vm, "ShapeBase", "isHidden", |vm, c| {
        let hidden = c
            .this_object
            .and_then(|id| ext::get(vm.objects(), id))
            .is_some_and(|e| e.hidden);
        Native::Bool(hidden)
    });
    register_method(vm, "ShapeBase", "startFade", |vm, c| {
        if let Some(id) = c.this_object {
            let fade_out = c.argv.get(4).is_some_and(|a| atob(a));
            start_fade(vm, id, arg(c, 2), arg(c, 3), fade_out);
        }
        Native::Void
    });
    register_method(vm, "Player", "setArmThread", set_arm_thread);
    register_method(vm, "Player", "setActionThread", set_action_thread);
}

/// The mask bit `setHidden` marks (the packer's cloak block: its `cloaked` flag is the hidden byte for a StaticShape).
pub const HIDDEN_MASK: u32 = 0x100;

/// `ShapeBase::setHidden(hidden)` (the console method reads its argument with `dAtob`; the same for ShapeBase, StaticShape,
/// Player, Item and both vehicles): nothing when the hidden byte already has the value;
/// else the ghost is marked [`HIDDEN_MASK`], a shown object leaves the scene (`removeFromScene`: out of the container's bins,
/// so no ray, search, collision or scope walk finds it) and a hidden one enters it again (`addToScene`, by its world box
/// now), and the byte takes the value. Both moves are charged to the run (`Bins::{remove,insert}_charged`, always carried out
/// whole). Not modelled: the object's listener list (each told 1), which no server object the engine makes registers in.
pub fn set_hidden(vm: &mut Vm, container: &Container, id: ObjId, hidden: bool) {
    let Some(was) = ext::get(vm.objects(), id).map(|e| e.hidden) else {
        return;
    };
    if hidden == was {
        return;
    }
    ghosts::mark(vm, id, HIDDEN_MASK);
    if was {
        scene::add_to_scene(vm, container, id);
    } else {
        let vm_ref: &Vm = vm;
        container
            .bins_mut()
            .remove_charged(id, &mut |u| vm_ref.charge_work(u));
    }
    if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
        e.hidden = hidden;
    }
}

/// `TSShapeInstance::animate(detail)` for an object: what the engine's queries run before they read the node matrices.
/// A host function for the engine's queries (ray casts, polylists, the eye and muzzle lookups) and the tests; no script method
/// calls it. Charged by the instance.
pub fn animate(vm: &mut Vm, id: ObjId, detail: usize) -> Result<(), VmError> {
    let Some(mut st) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.shape.take()) else {
        return Ok(());
    };
    let mut result = Ok(());
    if let Some(body) = st.body.as_mut() {
        let kept = Rc::clone(&body.kept);
        if let Some(shape) = kept.shape.as_ref()
            && body
                .instance
                .animate(shape, detail, None, &mut |units| vm.charge_work(units))
                .is_err()
        {
            result = Err(VmError::StepLimit);
        }
    }
    put_state(vm, id, st);
    result
}

// ------------------------------------------------------------------------------------------------------- the Player's skeleton

/// A Player class's action table (`PlayerData::ActionAnimationList` and its size): the fixed actions, whose sequences are
/// looked up by name at the datablock's preload, with their table directions; every other sequence of the shape follows as an
/// action of its own name. A host's Player class gives its own through
/// [`SimClasses::action_table`](crate::sim_classes::SimClasses::action_table).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActionTable {
    /// The fixed actions' names. The first four are the root and the moving actions `pickActionAnimation` chooses by
    /// velocity: run, back and side.
    pub fixed: &'static [&'static str],
    /// The fixed actions' table directions, in the same order (zero for those without one).
    pub dirs: &'static [[f32; 3]],
    /// The table's size: `setActionThread` refuses an action at or past it, and the preload keeps no more actions.
    pub max: usize,
    /// The fixed actions chosen by number: falling, and landing (an action that does not hold the thread); the jumps, which a
    /// run surface stops.
    pub fall: u32,
    pub land: u32,
    pub jump: u32,
    pub stand_jump: u32,
    /// No action chosen (`NullAnimation`): what the arm and action numbers hold before one is.
    pub none: u32,
    /// The first of a crouching run, back and side, when the class has them: a crouched Player's moving action is moved onto
    /// them.
    pub crouch: Option<u32>,
}

/// TGE's table: root, run, back, side, fall, jump, standjump and land (`NumTableActionAnims`), with 512 extra actions
/// (`NumActionAnims`, 520 in all); no action is 511 (`NullAnimation`, the largest of `ActionAnimBits`' 9 bits).
pub static TGE_ACTIONS: ActionTable = ActionTable {
    fixed: &[
        "root",
        "run",
        "back",
        "side",
        "fall",
        "jump",
        "standjump",
        "land",
    ],
    dirs: &[
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0; 3],
        [0.0; 3],
        [0.0; 3],
        [0.0; 3],
    ],
    max: 520,
    fall: 4,
    land: 7,
    jump: 5,
    stand_jump: 6,
    none: 511,
    crouch: None,
};

impl ActionTable {
    /// The numbers the step reads ([`PlayerData::actions`](::sim::player::PlayerData::actions)).
    pub fn numbers(&self) -> ::sim::player::ActionNumbers {
        ::sim::player::ActionNumbers {
            table: u32::try_from(self.fixed.len()).unwrap_or(u32::MAX),
            jump: self.jump,
            stand_jump: self.stand_jump,
            land: self.land,
            none: self.none,
        }
    }
}

/// The `Bip01` bones the server pins to the default pose (found by name at the `PlayerData` preload).
pub const MASKED_BONES: [&str; 6] = [
    "Bip01 Pelvis",
    "Bip01 Spine",
    "Bip01 Spine1",
    "Bip01 Spine2",
    "Bip01 Neck",
    "Bip01 Head",
];

/// One entry of a PlayerData's action list.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerAction {
    pub name: Vec<u8>,
    /// The sequence index, -1 for an action the shape has no sequence for.
    pub sequence: i32,
}

/// `PlayerData::preload`'s action list for a shape: the table's fixed actions (their sequences by name), then each sequence not
/// already an action's, named as the shape names it. Cut at the table's size (it has no room for more).
pub fn player_actions(shape: &shapes::Shape, table: &ActionTable) -> Vec<PlayerAction> {
    let mut out: Vec<PlayerAction> = table
        .fixed
        .iter()
        .map(|n| PlayerAction {
            name: n.as_bytes().to_vec(),
            sequence: find_sequence(shape, n.as_bytes())
                .map_or(-1, |i| i32::try_from(i).unwrap_or(-1)),
        })
        .collect();
    for (i, s) in shape.sequences.iter().enumerate() {
        let index = i32::try_from(i).unwrap_or(i32::MAX);
        if out.iter().any(|a| a.sequence == index) || out.len() >= table.max {
            continue;
        }
        let name = usize::try_from(s.name)
            .ok()
            .and_then(|n| shape.names.get(n))
            .cloned()
            .unwrap_or_default();
        out.push(PlayerAction {
            name,
            sequence: index,
        });
    }
    out
}

/// What `Player::onNewDataBlock` builds on a server: the arm thread (the action named `look`), the `head` and `headside`
/// threads, the threads the Player class adds and the action thread, with the action state `setArmThread` and `setActionThread`
/// set.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerThreads {
    /// The class's action table and the datablock's action list from it (see [`player_actions`]).
    pub table: &'static ActionTable,
    pub actions: Vec<PlayerAction>,
    /// The arm action chosen and its thread.
    pub arm_action: u32,
    pub arm: Option<ThreadId>,
    pub head: Option<ThreadId>,
    pub head_side: Option<ThreadId>,
    /// The threads the Player class adds, by their numbers (`Thread::Class(n)` is the n-th; see
    /// [`SimClasses::player_class_threads`](crate::sim_classes::SimClasses::player_class_threads)). None for TGE's Player.
    pub class_threads: Vec<Option<ThreadId>>,
    /// The action chosen and its thread.
    pub action: u32,
    pub action_thread: Option<ThreadId>,
    /// `setActionThread`'s arguments, as it stores them: forward, the first-person-suppress flag (also), hold
    /// and hold-or-wait.
    pub forward: bool,
    pub fsp: bool,
    pub hold: bool,
    pub wait: bool,
    /// The action thread reached its end (`updateAnimation` sets it, `setActionThread` clears it).
    pub at_end: bool,
    /// The counter `setActionThread` sets to 4 and `updateAnimation` counts down.
    pub action_counter: i32,
    /// A synced Player's position and upward speed at the end of its last step
    /// ([`synced`](crate::sim::player::synced)).
    pub synced_from: Option<([f32; 3], f32)>,
}

/// `updateActionThread(true)` on a server: each of the six bones is pinned to the default pose (the rotation and the three
/// translation axes are the Player's) unless the action says the first-person suppress flag, when they are let go; the other
/// node marks of the bone are cleared. A bone the shape lacks is left.
pub fn update_node_masks(body: &mut ShapeBody, fsp: bool) {
    let wanted: u8 = if fsp { 0 } else { 0xf };
    let kept = Rc::clone(&body.kept);
    let Some(shape) = kept.shape.as_ref() else {
        return;
    };
    for bone in MASKED_BONES {
        let Some(name) = shape.find_name(bone.as_bytes()) else {
            continue;
        };
        let Some(node) = shape.nodes.iter().position(|n| n.name == name) else {
            continue;
        };
        if node >= shapes::instance::SET_BITS {
            continue;
        }
        let m = &body.instance.masks;
        let has = |set: &shapes::instance::NodeSet| shapes::instance::set_has(set, node);
        let current = u8::from(has(&m.mask_rotation))
            | u8::from(has(&m.mask_pos_x)) << 1
            | u8::from(has(&m.mask_pos_y)) << 2
            | u8::from(has(&m.mask_pos_z)) << 3
            | u8::from(has(&m.disable_blend)) << 4
            | u8::from(has(&m.hands_off)) << 5
            | u8::from(has(&m.callback)) << 6;
        if wanted == current {
            continue;
        }
        body.instance.set_all_dirty();
        let (word, bit) = (node / 32, 1u32 << (node % 32));
        let m = &mut body.instance.masks;
        let sets = [
            (&mut m.mask_rotation, 1u8),
            (&mut m.mask_pos_x, 2),
            (&mut m.mask_pos_y, 4),
            (&mut m.mask_pos_z, 8),
        ];
        for (set, flag) in sets {
            if wanted & flag != 0 {
                set[word] |= bit;
            } else {
                set[word] &= !bit;
            }
        }
        m.disable_blend[word] &= !bit;
        m.hands_off[word] &= !bit;
        m.callback[word] &= !bit;
    }
}

/// The server half of `Player::onNewDataBlock`: the threads, in the order the Player makes them, and the node marks. `class`
/// makes the class's own threads, after `headside` and before the action thread.
fn player_threads(
    body: &mut ShapeBody,
    previous: Option<&PlayerThreads>,
    table: &'static ActionTable,
    class: impl FnOnce(&mut ShapeBody) -> Vec<Option<ThreadId>>,
) -> Option<PlayerThreads> {
    let kept = Rc::clone(&body.kept);
    let shape = kept.shape.as_ref()?;
    let actions = player_actions(shape, table);
    let look = actions.iter().position(|a| a.name == b"look").unwrap_or(0);
    let mut p = PlayerThreads {
        table,
        actions,
        arm_action: table.none,
        arm: None,
        head: None,
        head_side: None,
        class_threads: Vec::new(),
        action: table.none,
        action_thread: None,
        forward: true,
        fsp: false,
        hold: false,
        wait: false,
        at_end: false,
        action_counter: 0,
        synced_from: None,
    };
    let still = |body: &mut ShapeBody, t: Option<ThreadId>| {
        if let Some(t) = t.and_then(|t| body.instance.thread_mut(t)) {
            t.time_scale = 0.0;
        }
    };
    if look != 0 {
        p.arm = body.instance.create_thread(shape);
        still(body, p.arm);
        // The arm action the old datablock had is chosen again by name; else the `look` action.
        let old = previous.and_then(|o| {
            let n = usize::try_from(o.arm_action).ok()?;
            o.actions.get(n).map(|a| a.name.clone())
        });
        let mut chosen = look;
        if let Some(name) = old
            && let Some(i) = p
                .actions
                .iter()
                .enumerate()
                .skip(1)
                .find(|(_, a)| a.name.eq_ignore_ascii_case(&name))
                .map(|(i, _)| i)
        {
            chosen = i;
        }
        p.arm_action = u32::try_from(chosen).unwrap_or(table.none);
        if let (Some(t), Some(a)) = (p.arm, p.actions.get(chosen)) {
            body.instance.set_sequence(shape, t, a.sequence, 0.0);
        }
    }
    for (slot, name) in [(0, "head"), (1, "headside")] {
        if let Some(i) = find_sequence(shape, name.as_bytes()) {
            let t = body.instance.create_thread(shape);
            if let Some(t) = t {
                body.instance
                    .set_sequence(shape, t, i32::try_from(i).unwrap_or(-1), 0.0);
            }
            still(body, t);
            if slot == 0 {
                p.head = t;
            } else {
                p.head_side = t;
            }
        }
    }
    p.class_threads = class(body);
    p.action_thread = body.instance.create_thread(shape);
    update_node_masks(body, p.fsp);
    Some(p)
}

/// `Player::setArmThread(action)`: the arm thread plays the action's sequence unless it plays it already.
fn arm_action(st: &mut ShapeBaseState, action: usize) -> bool {
    let (Some(body), Some(player)) = (st.body.as_mut(), st.player.as_mut()) else {
        return false;
    };
    let Some(sequence) = player
        .actions
        .get(action)
        .map(|a| a.sequence)
        .filter(|s| *s != -1)
    else {
        return false;
    };
    let kept = Rc::clone(&body.kept);
    let Some(shape) = kept.shape.as_ref() else {
        return false;
    };
    let Some(arm) = player.arm else {
        return false;
    };
    let current = body.instance.thread(arm).and_then(|t| t.sequence);
    if current == usize::try_from(sequence).ok() {
        return false;
    }
    body.instance.set_sequence(shape, arm, sequence, 0.0);
    player.arm_action = u32::try_from(action).unwrap_or(player.table.none);
    true
}

/// What `Player::setActionThread` asked for (the arguments of).
#[derive(Debug, Clone, Copy)]
pub struct ActionRequest {
    pub forward: bool,
    pub hold: bool,
    pub wait: bool,
    pub fsp: bool,
    pub force: bool,
    /// The action blends in over [`TRANSITION`](crate::sim::player::synced::TRANSITION) seconds (a synced Player's; TGE's
    /// server sets the sequence).
    pub blend: bool,
    /// Where in its sequence the action starts, when not at its start (forward) or end (backward): the engine's walk and run
    /// keep their place in the loop ([`keeps_phase`](crate::sim::player::synced::keeps_phase)).
    pub start: Option<f32>,
}

/// `Player::setActionThread` on a server: the action's sequence on the action thread, from its start when the action plays
/// forward and from its end when it plays back, at the rate +1 or -1 (blended in when the request says so, as a client does).
/// The error line when the action is out of range.
pub fn action_thread(st: &mut ShapeBaseState, action: usize, r: ActionRequest) -> Option<String> {
    let (Some(body), Some(player)) = (st.body.as_mut(), st.player.as_mut()) else {
        return None;
    };
    if player.action == u32::try_from(action).unwrap_or(player.table.none) && !r.force {
        return None;
    }
    if action >= player.table.max {
        return Some(format!(
            "Player::setActionThread({action}): Player action out of range"
        ));
    }
    let sequence = player
        .actions
        .get(action)
        .map(|a| a.sequence)
        .filter(|s| *s != -1)?;
    player.hold = r.hold;
    player.wait = r.hold | r.wait;
    player.action = u32::try_from(action).unwrap_or(player.table.none);
    player.forward = r.forward;
    player.fsp = r.fsp;
    player.action_counter = 4;
    player.at_end = false;
    let kept = Rc::clone(&body.kept);
    if let (Some(shape), Some(thread)) = (kept.shape.as_ref(), player.action_thread) {
        let pos = r.start.unwrap_or(if r.forward { 0.0 } else { 1.0 });
        let scale = if r.forward { 1.0 } else { -1.0 };
        match usize::try_from(sequence) {
            // The direction first: a transition runs the way the thread plays when it starts.
            Ok(seq) if r.blend => {
                if let Some(t) = body.instance.thread_mut(thread) {
                    t.time_scale = scale;
                }
                body.instance.transition_to_sequence(
                    shape,
                    thread,
                    seq,
                    pos,
                    crate::sim::player::synced::TRANSITION,
                );
            }
            _ => {
                body.instance.set_sequence(shape, thread, sequence, pos);
                if let Some(t) = body.instance.thread_mut(thread) {
                    t.time_scale = scale;
                }
            }
        }
    }
    None
}

/// `setArmThread(name)`: the first action after `root` named `name` (ignoring case), if it is a new sequence.
fn set_arm_thread(vm: &mut Vm, c: &NativeCall) -> Native {
    let Some(id) = c.this_object else {
        return Native::Bool(false);
    };
    let Some(mut st) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.shape.take()) else {
        return Native::Bool(false);
    };
    let name = arg(c, 2);
    let found = st.player.as_ref().and_then(|p| {
        p.actions
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, a)| a.name.eq_ignore_ascii_case(name))
            .map(|(i, _)| i)
    });
    let done = found.is_some_and(|i| arm_action(&mut st, i));
    put_state(vm, id, st);
    if done {
        ghosts::mark(vm, id, 0x100_0000);
    }
    Native::Bool(done)
}

/// `setActionThread(name [, hold [, fsp]])`: the first action named `name` (ignoring case) plays forward.
fn set_action_thread(vm: &mut Vm, c: &NativeCall) -> Native {
    let Some(id) = c.this_object else {
        return Native::Bool(false);
    };
    let Some(mut st) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.shape.take()) else {
        return Native::Bool(false);
    };
    // The wrapper's defaults: not held, and the first-person flag on unless the fifth argument says otherwise.
    let (hold, fsp) = if c.argv.len() >= 4 {
        (atob(arg(c, 3)), c.argv.len() <= 4 || atob(arg(c, 4)))
    } else {
        (false, true)
    };
    let name = arg(c, 2);
    let found = st.player.as_ref().and_then(|p| {
        p.actions
            .iter()
            .position(|a| a.name.eq_ignore_ascii_case(name))
    });
    let mut message = None;
    if let Some(i) = found {
        let request = ActionRequest {
            forward: true,
            hold,
            wait: true,
            fsp,
            force: true,
            blend: crate::scene::hitboxes::wanted(vm, id),
            start: None,
        };
        message = action_thread(&mut st, i, request);
    }
    let none = crate::sim_classes::sim_classes(vm).action_table().none;
    let now = st.player.as_ref().map_or(none, |p| p.action);
    put_state(vm, id, st);
    if let Some(s) = crate::sim::player::state_mut(vm, id) {
        s.action_anim = now;
    }
    if let Some(message) = message {
        vm.error(message);
    }
    if found.is_some() {
        ghosts::mark(vm, id, 0x100_0000);
    }
    Native::Bool(found.is_some())
}

// --------------------------------------------------------------------------------------------------- Item and Player specifics

/// Whether the object is a `Player` or an `AIPlayer` (its `setTransform` is its own).
pub fn is_player(vm: &Vm, id: ObjId) -> bool {
    is_class(vm, id, "Player")
}

/// `Player::setTransform`: the rotation kept is about z alone, `rot.z = -f32(atan2(-m[1], m[5]))` (a double `atan2`
/// stored as a float), and `Player::setPosition` (no mount) makes the matrix of the Euler angles `(0, 0, rot.z)` with the
/// position of the incoming matrix, stores it, inverts the copy, resets the world box and checks the bins (the caller); the
/// mask bits `0x2000040`. The rotation `setPosition` keeps is the Player glue's.
pub fn player_set_transform(vm: &mut Vm, id: ObjId, m: &fpu::Matrix) {
    let yaw = collide::math::transcendental::atan2(-f64::from(m[1]), f64::from(m[5])) as f32;
    let mut out = collide::math::set_euler([0.0, 0.0, -yaw]);
    out[3] = m[3];
    out[7] = m[7];
    out[11] = m[11];
    super::access::set_matrix(vm, id, &out);
    scene::reset_world_box(vm, id);
    ghosts::mark(vm, id, 0x200_0040);
}

/// Whether the object is an `Item` (its `setTransform` is its own).
pub fn is_item(vm: &Vm, id: ObjId) -> bool {
    is_class(vm, id, "Item")
}

/// `Item::setTransform`: only the position of the matrix is kept; the rotation is about z alone, the yaw `atan2(-m[1], m[5])`
/// of the incoming matrix, and none at all for an item that rotates (its `rotate` field); the world box follows and the
/// container is told by the caller.
pub fn item_set_transform(vm: &mut Vm, id: ObjId, m: &fpu::Matrix) {
    let rotates = super::access::bool_field(vm, id, b"rotate");
    let mut out = if rotates {
        fpu::IDENTITY
    } else {
        let yaw = collide::math::transcendental::atan2(-f64::from(m[1]), f64::from(m[5])) as f32;
        collide::math::set_euler([0.0, 0.0, -yaw])
    };
    out[3] = m[3];
    out[7] = m[7];
    out[11] = m[11];
    super::access::set_matrix(vm, id, &out);
    scene::reset_world_box(vm, id);
    ghosts::mark(vm, id, 0xc00_0040);
}

/// The Player's own object box (`Player::onNewDataBlock`): from the datablock's `field` (the class's,
/// [`SimClasses::player_box_field`](crate::sim_classes::SimClasses::player_box_field): TGE's `boundingBox`) as
/// `(-x/2, -y/2, 0)` to `(x/2, y/2, z)`, set only when the current extents differ from it by more than 0.1 (the gate before
/// it). The world box is NOT reset there (the box fields are written and the world box waits for the next `setTransform` or
/// `setScale`: the `getWorldBox` of a fresh Player is the point at its position).
fn player_box(vm: &mut Vm, id: ObjId, db: ObjId, field: &'static [u8]) {
    let Some(ts_vm::fields::Value::Floats3(size)) = super::access::value_of(vm, db, field) else {
        return;
    };
    let Some(current) = ext::get(vm.objects(), id).and_then(|e| e.scene) else {
        return;
    };
    if !(crate::library::get().box_differs)(current.object_min, current.object_max, size) {
        return;
    }
    let half = |v: f32| v * 0.5;
    scene::set_object_box_only(
        vm,
        id,
        [-half(size[0]), -half(size[1]), 0.0],
        [half(size[0]), half(size[1]), size[2]],
    );
}

// ------------------------------------------------------------------------------------------------------------------- names

/// The shape state an object's name handles live in (made when the object has none yet).
fn name_state(vm: &mut Vm, id: ObjId) -> Option<&mut ShapeBaseState> {
    let e = ext::get_mut(vm.objects_mut(), id)?;
    Some(&mut **e.shape.get_or_insert_with(Box::default))
}

/// The net string handle a `setShapeName`/`setSkinName` argument makes, holding one engine reference: none for
/// an empty argument, the tag `<id>` names for a tagged form `\x01<id>` (`NetStringHandle(U32)`), else the text entered in the
/// tag table (`addString`). `Err` when the table cannot take a new string. Engine-only: a tagged form that names no live tag
/// makes no handle.
fn name_handle(vm: &mut Vm, text: &[u8]) -> Result<u32, VmError> {
    match text.first() {
        None => Ok(0),
        Some(1) => {
            let id = atoi(text.get(1..).unwrap_or(&[])) as u32;
            let Some(live) = vm.tags().lookup(id).map(<[u8]>::to_vec) else {
                return Ok(0);
            };
            vm.tags_mut().add_string(&live).ok_or(VmError::TagTableFull)
        }
        Some(_) => vm.tags_mut().add_string(text).ok_or(VmError::TagTableFull),
    }
}

/// Lets go of the engine reference a name handle holds.
fn release_handle(vm: &mut Vm, tag: u32) {
    if tag != 0
        && let Some(message) = vm.tags_mut().remove_string(tag, false)
    {
        vm.error(message);
    }
}

/// Assigns a name handle (`NetStringHandle::operator=`): the new one is taken first, then the old one released.
fn set_name(vm: &mut Vm, id: ObjId, text: &[u8], skin: bool) -> Result<(), VmError> {
    let handle = name_handle(vm, text)?;
    let old = match name_state(vm, id) {
        Some(s) if skin => std::mem::replace(&mut s.skin_name, handle),
        Some(s) => std::mem::replace(&mut s.shape_name, handle),
        None => handle,
    };
    release_handle(vm, old);
    Ok(())
}

/// The ShapeBase destructor's release of the two name handles (the engine runs it from `ShapeBase::onRemove`, as the object
/// leaves).
pub fn release_names(vm: &mut Vm, id: ObjId) {
    let names = state_mut(vm, id).map(|s| {
        (
            std::mem::take(&mut s.shape_name),
            std::mem::take(&mut s.skin_name),
        )
    });
    if let Some((shape, skin)) = names {
        release_handle(vm, shape);
        release_handle(vm, skin);
    }
}

/// `setShapeName(tag)`: the name's handle; mask 0x10.
pub fn set_shape_name(vm: &mut Vm, c: &NativeCall) -> Native {
    let Some(id) = c.this_object else {
        return Native::Void;
    };
    if let Err(e) = set_name(vm, id, arg(c, 2), false) {
        return Native::Fail(e);
    }
    ghosts::mark(vm, id, 0x10);
    Native::Void
}

/// `setSkinName(tag)`: mask 0x800.
fn set_skin_name(vm: &mut Vm, c: &NativeCall) -> Native {
    let Some(id) = c.this_object else {
        return Native::Void;
    };
    if let Err(e) = set_name(vm, id, arg(c, 2), true) {
        return Native::Fail(e);
    }
    ghosts::mark(vm, id, 0x800);
    Native::Void
}

/// `getShapeName()` and `getSkinName()`: the handle's text (`lookupString`), empty when there is none.
fn get_name(vm: &Vm, c: &NativeCall, skin: bool) -> Native {
    let tag = c
        .this_object
        .and_then(|id| state(vm, id))
        .map_or(0, |s| if skin { s.skin_name } else { s.shape_name });
    let text = match tag {
        0 => Vec::new(),
        t => vm.tags().lookup(t).map(<[u8]>::to_vec).unwrap_or_default(),
    };
    Native::Str(text)
}

#[cfg(test)]
#[path = "shapebase_tests.rs"]
mod tests;
