//! GuiSliderCtrl as TGE 1.4 has it. Drawing is [`crate::draw`]'s
//! (`onRender`).
//!
//! - **Constructor**: active; range 0..1, 10 ticks, value 0.5, the thumb 8 x 20, the shift point 5, the shift extent 10.
//! - **`onWake`**: `GuiControl::onWake`, then `mDisplayValue` (the value shown under the thumb) when the thumb's height
//!   less 4 plus the font's height fits the extent (compared unsigned), then `updateThumb(value, true)`.
//! - **`updateThumb`**: the value clamped to the range; the thumb placed (horizontal: at `extent.x - (shiftExtent +
//!   thumb.x) / 2` times the value's fraction, less half the thumb's width; vertical: the same down the height); the variable set
//!   (`Con::setFloatVariable`, `%f`); outside a wake, `altCommand` evaluated.
//! - **`setScriptValue`** (`setValue`): `atof`, then `updateThumb(value, true)`: the variable, no command.
//! - **The mouse** (down: active, awake and visible; the mouse locked, the first responder; drag, the value clamped
//!   first; both snap to the ticks with Shift or `snap` when there are more than two; up: unlocked, the `command` with
//!   `$ThisControl`).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/controls/guiSliderCtrl.cpp` (Copyright
//! (c) 2012 GarageGames, LLC); the code here is this crate's own.

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use crate::Gui;
use crate::control::{bounds, flag};
use crate::profile::Rect;

/// The thumb's size, the shift point and extent (the constructor's; no fields set them).
pub const THUMB_SIZE: (i32, i32) = (8, 20);
pub const SHIFT_POINT: i32 = 5;
pub const SHIFT_EXTENT: i32 = 10;

/// The members the drawing reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliderState {
    pub value: f32,
    pub range: (f32, f32),
    pub ticks: i32,
    /// `mThumb` (in the control).
    pub thumb: Rect,
    pub thumb_size: (i32, i32),
    pub shift_point: i32,
    pub shift_extent: i32,
    pub display_value: bool,
}

/// The kept part of a slider's state (the rest is its fields).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SliderMembers {
    pub thumb: Rect,
    pub display_value: bool,
}

fn words(vm: &Vm, id: ObjId, field: &[u8]) -> Vec<f32> {
    vm.objects()
        .field(id, field, b"")
        .unwrap_or_default()
        .split(|b| *b == b' ' || *b == b'\t')
        .filter(|w| !w.is_empty())
        .take(2)
        .map(|w| torque_ts_vm::text::atof(w) as f32)
        .collect()
}

fn range(vm: &Vm, id: ObjId) -> (f32, f32) {
    let w = words(vm, id, b"range");
    (
        w.first().copied().unwrap_or(0.0),
        w.get(1).copied().unwrap_or(1.0),
    )
}

fn ticks(vm: &Vm, id: ObjId) -> i32 {
    match vm.objects().field(id, b"ticks", b"") {
        Some(t) if !t.is_empty() => crate::atoi(&t),
        _ => 10,
    }
}

fn value(vm: &Vm, id: ObjId) -> f32 {
    match vm.objects().field(id, b"value", b"") {
        Some(t) if !t.is_empty() => torque_ts_vm::text::atof(&t) as f32,
        _ => 0.5,
    }
}

pub fn state(vm: &Vm, gui: &Gui, id: ObjId) -> SliderState {
    let m = gui.with(id, |c| c.slider).flatten().unwrap_or_default();
    SliderState {
        value: value(vm, id),
        range: range(vm, id),
        ticks: ticks(vm, id),
        thumb: m.thumb,
        thumb_size: THUMB_SIZE,
        shift_point: SHIFT_POINT,
        shift_extent: SHIFT_EXTENT,
        display_value: m.display_value,
    }
}

/// `onWake`'s own part.
pub fn after_wake(vm: &mut Vm, gui: &Gui, id: ObjId) {
    let font_h = crate::textlist::font(vm, gui, id).map_or(0, |f| f.height as i32);
    let (_, ext) = bounds(vm, id);
    let display = (THUMB_SIZE.1 - 4 + font_h) as u32 <= ext.1 as u32;
    gui.with_mut(id, |c| {
        c.slider
            .get_or_insert_with(SliderMembers::default)
            .display_value = display
    });
    let v = value(vm, id);
    let _ = update_thumb(vm, gui, id, v, true);
}

/// `updateThumb`.
pub fn update_thumb(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    v: f32,
    on_wake: bool,
) -> Result<(), VmError> {
    let (a, b) = range(vm, id);
    let (lo, hi) = if b < a { (b, a) } else { (a, b) };
    let mut v = v;
    if v < lo {
        v = lo;
    }
    if hi < v {
        v = hi;
    }
    let (_, ext) = bounds(vm, id);
    let mut m = gui.with(id, |c| c.slider).flatten().unwrap_or_default();
    let ext_x = ext.0 - (SHIFT_EXTENT + THUMB_SIZE.0) / 2;
    let frac = (v - lo) / (hi - lo);
    if ext.0 < ext.1 {
        let y = (ext.1 as f32 * frac) as i32 - THUMB_SIZE.0 / 2;
        let x = ext_x / 2 - THUMB_SIZE.1 / 2;
        m.thumb = [x, y, THUMB_SIZE.1, THUMB_SIZE.0];
    } else {
        let mid = if m.display_value { THUMB_SIZE.1 } else { ext.1 };
        let y = mid / 2 - THUMB_SIZE.1 / 2;
        let x = (ext_x as f32 * frac) as i32 - THUMB_SIZE.0 / 2;
        m.thumb = [x, y, THUMB_SIZE.0, THUMB_SIZE.1];
    }
    gui.with_mut(id, |c| c.slider = Some(m));
    vm.set_object_field(
        id,
        b"value",
        b"",
        crate::classes::format_g(f64::from(v)).as_bytes(),
    );
    let var = vm.objects().field(id, b"variable", b"").unwrap_or_default();
    if !var.is_empty() {
        let name = if var.first() == Some(&b'$') {
            String::from_utf8_lossy(&var).into_owned()
        } else {
            format!("${}", String::from_utf8_lossy(&var))
        };
        vm.set_global(&name, &torque_ts_vm::text::format_fixed(f64::from(v), 6));
    }
    if !on_wake {
        let alt = vm
            .objects()
            .field(id, b"altCommand", b"")
            .unwrap_or_default();
        if !alt.is_empty() {
            vm.eval(&alt)?;
        }
    }
    Ok(())
}

/// The value under the mouse (down and drag), snapped with Shift or `snap`.
fn value_at(vm: &Vm, id: ObjId, pt: (i32, i32), modifier: u8, clamp: bool) -> f32 {
    let local = crate::scroll::local(vm, id, pt);
    let (a, b) = range(vm, id);
    let (lo, hi) = if b < a { (b, a) } else { (a, b) };
    let (_, ext) = bounds(vm, id);
    let frac = if ext.0 < ext.1 {
        local.1 as f32 / ext.1 as f32
    } else {
        (local.0 - SHIFT_POINT) as f32 / (ext.0 - SHIFT_EXTENT) as f32
    };
    let mut v = frac * (b - a).abs() + lo;
    if b < a {
        v = (hi - v) + lo;
    }
    if clamp {
        v = v.clamp(lo, hi);
    }
    let t = ticks(vm, id);
    if (modifier & 3 != 0 || flag(vm, id, b"snap")) && t as u32 > 2 {
        let step = (hi - lo) / (t + 1) as f32;
        let n = ((v - lo) / step + 0.5) as i32;
        v = (n as f32 * step + lo).clamp(lo, hi);
    }
    v
}

fn usable(vm: &Vm, gui: &Gui, id: ObjId) -> bool {
    crate::input::active(gui, id) && gui.is_awake(id) && flag(vm, id, b"visible")
}

pub fn mouse_down(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    pt: (i32, i32),
    modifier: u8,
) -> Result<(), VmError> {
    if !usable(vm, gui, id) {
        return Ok(());
    }
    crate::input::mouse_lock(vm, gui, id)?;
    crate::input::set_first_responder(vm, gui, id)?;
    let v = value_at(vm, id, pt, modifier, false);
    update_thumb(vm, gui, id, v, false)
}

pub fn mouse_dragged(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    pt: (i32, i32),
    modifier: u8,
) -> Result<(), VmError> {
    if !usable(vm, gui, id) {
        return Ok(());
    }
    let v = value_at(vm, id, pt, modifier, true);
    update_thumb(vm, gui, id, v, false)
}

pub fn mouse_up(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    if !usable(vm, gui, id) {
        return Ok(());
    }
    crate::input::mouse_unlock(vm, gui, id)?;
    let command = vm.objects().field(id, b"command", b"").unwrap_or_default();
    if !command.is_empty() {
        vm.set_global("$ThisControl", &id.to_string());
        vm.eval(&command)?;
    }
    Ok(())
}
