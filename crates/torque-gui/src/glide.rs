//! Smooth scrolling: off by default (TGE's stepped scroll), on while `$Pref::Gui::SmoothScroll` is true (`setSmoothScroll`
//! writes it). A scroll control's content still moves to where the wheel puts it at once; only its drawing lags, by a
//! [`Glide`] that shrinks to nothing over [`GLIDE_MS`] of the simulation clock ([`Vm::sim_time`]), so it takes the same time at
//! any frame rate.
//!
//! - **What scripts and hit tests see** is the final place at all times: the children's `position`, `scrollTo`, the list rows
//!   under the mouse. Only the picture lags.
//! - **The wheel** ([`crate::scroll::wheel`]) and a scroll that reveals the focused control ([`crate::scroll::reveal`], on while
//!   `$Pref::Gui::RevealFocus` is true; `setRevealFocus` writes it; off by default) add what
//!   they moved to the control's glide: one that arrives while the content still glides goes on from the place drawn. The
//!   picture never leaves the scroll range ([`crate::scroll::shown`]).
//! - **Parts of a notch** ([`wheel_part`]): a high-resolution wheel or a touchpad reports less than a whole notch, which the window
//!   gathers into whole ones. The scroll control the wheel reaches moves by that share of a row as it comes, and back by it when the
//!   whole notch arrives, so a slow swipe moves the content all the way and whole notches land where a plain wheel's do.
//! - **A press ends every glide** ([`finish_all`]), so what is clicked is what is drawn and a dragged thumb follows the mouse.
//!   Keyboard and script scrolls ([`crate::scroll::scroll_to`] and its callers) move at once and leave the glide as it was.

use std::collections::HashMap;

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Native, Vm, VmError};

use crate::control::{children, flag};
use crate::scroll::{report_moved, scroll_delta, state};
use crate::{Gui, atob};

/// The GUI's prefs (`GuiPlugin` declares them): the script reference and the prefs file take them from here.
#[derive(torque_engine::Prefs)]
#[prefs(prefix = "$Pref::Gui")]
pub struct GuiPrefs {
    /// Whether the scroll controls draw their content gliding to where the wheel puts it (`setSmoothScroll` writes it). Off, a
    /// wheel notch steps the content as TGE's does.
    #[pref(default = false)]
    pub smooth_scroll: bool,
    /// Whether a control that takes the key focus (a click, Tab, `makeFirstResponder`) scrolls its nearest scroll control to show it
    /// (`setRevealFocus` writes it). Off, focus moves no content, as TGE's does.
    #[pref(default = false)]
    pub reveal_focus: bool,
}

/// The global [`GuiPrefs::smooth_scroll`] is.
pub const PREF: &str = "$Pref::Gui::SmoothScroll";
/// The global [`GuiPrefs::reveal_focus`] is.
pub const REVEAL_PREF: &str = "$Pref::Gui::RevealFocus";
/// The most glides kept before the ones at rest are dropped from the map on a push.
const GLIDES_KEPT: usize = 16;
/// How long a glide takes from its last push to rest (milliseconds of the simulation clock).
pub const GLIDE_MS: u32 = 150;
/// The farthest the picture is drawn from the content's place, each way (a broken device's huge wheel).
pub const MAX_GLIDE_PX: f32 = 8192.0;
/// The row a part of a notch is a share of when the content has no line size of its own.
const DEFAULT_ROW: u32 = 30;

/// The displacement a scroll control's children are drawn at (pixels added to their places): `from` at `start`, none at
/// `start + GLIDE_MS`, an ease-out (cubic).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glide {
    pub from: (f32, f32),
    pub start: u32,
}

impl Glide {
    /// The displacement at `now` (none from `start + GLIDE_MS` on, and for a clock behind `start`).
    pub fn at(&self, now: u32) -> (f32, f32) {
        let elapsed = now.wrapping_sub(self.start);
        if elapsed >= GLIDE_MS {
            return (0.0, 0.0);
        }
        let left = 1.0 - elapsed as f32 / GLIDE_MS as f32;
        let k = left * left * left;
        (self.from.0 * k, self.from.1 * k)
    }
}

/// The part of a wheel notch gathered so far, as the scroll controls took it ([`wheel_part`]).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct WheelPart {
    /// The control that last took a part, and the pixels it moved for it.
    ctrl: Option<ObjId>,
    applied: i32,
    /// The last part the window gave: a control the mouse moves onto starts from it, so it does not jump by it.
    last: f32,
}

/// The glides in progress and the wheel's gathered part.
#[derive(Debug, Default)]
pub struct Glides {
    by_control: HashMap<ObjId, Glide>,
    part: WheelPart,
}

/// Whether smooth scrolling is on.
pub fn enabled(vm: &Vm) -> bool {
    atob(&vm.global(PREF).unwrap_or_default())
}

/// Whether a control that takes the key focus is scrolled into view ([`crate::scroll::reveal`]).
pub fn reveal_enabled(vm: &Vm) -> bool {
    atob(&vm.global(REVEAL_PREF).unwrap_or_default())
}

/// Turns smooth scrolling on or off (the pref); off ends every glide.
pub fn set_enabled(vm: &mut Vm, gui: &Gui, on: bool) {
    vm.set_global(PREF, if on { "1" } else { "0" });
    if !on {
        finish_all(gui);
    }
}

/// The content of `id` moved by `moved` (pixels: a scroll down moves it up, `(0, -row)`): its picture starts where it was drawn,
/// so the glide grows by the opposite. Nothing while smooth scrolling is off.
pub fn push(vm: &Vm, gui: &Gui, id: ObjId, moved: (i32, i32)) {
    if moved == (0, 0) || !gui.is_control(id) || !enabled(vm) {
        return;
    }
    let now = vm.sim_time();
    let mut state = gui.0.borrow_mut();
    let glides = &mut state.glides.by_control;
    // The ones at rest are dropped once there are many: a few at rest cost nothing and every one is asked for its place.
    if glides.len() >= GLIDES_KEPT {
        glides.retain(|_, g| g.at(now) != (0.0, 0.0));
    }
    let shown = glides.get(&id).map_or((0.0, 0.0), |g| g.at(now));
    let from = (
        (shown.0 - moved.0 as f32).clamp(-MAX_GLIDE_PX, MAX_GLIDE_PX),
        (shown.1 - moved.1 as f32).clamp(-MAX_GLIDE_PX, MAX_GLIDE_PX),
    );
    if from.0.abs() >= 0.5 || from.1.abs() >= 0.5 {
        glides.insert(id, Glide { from, start: now });
    } else {
        glides.remove(&id);
    }
}

/// The displacement `id`'s children are drawn at `now`, in whole pixels.
pub fn shift(gui: &Gui, id: ObjId, now: u32) -> (i32, i32) {
    gui.0
        .borrow()
        .glides
        .by_control
        .get(&id)
        .map_or((0, 0), |g| {
            let d = g.at(now);
            (d.0.round() as i32, d.1.round() as i32)
        })
}

/// Whether any control still glides `now`.
pub fn any_gliding(gui: &Gui, now: u32) -> bool {
    gui.0
        .borrow()
        .glides
        .by_control
        .values()
        .any(|g| g.at(now) != (0.0, 0.0))
}

/// Every glide ended: the children are drawn at their places (a press, smooth scrolling turned off).
pub fn finish_all(gui: &Gui) {
    gui.0.borrow_mut().glides.by_control.clear();
}

/// The control was removed.
pub(crate) fn forget(gui: &Gui, id: ObjId) {
    let mut state = gui.0.borrow_mut();
    state.glides.by_control.remove(&id);
    if state.glides.part.ctrl == Some(id) {
        state.glides.part.ctrl = None;
    }
}

/// The wheel's part of a notch now gathered (`part`, positive away from the user, within one notch either way: what the window
/// has not yet sent as a whole notch). The scroll control the wheel would reach moves by the change of that share of a row since
/// the last part, then each of its children gets an `onMouseMove`, as for a notch. Nothing while smooth scrolling is off.
///
/// # Errors
///
/// What a child's `onMouseMove` ended with.
pub fn wheel_part(vm: &mut Vm, gui: &Gui, part: f32) -> Result<(), VmError> {
    if !enabled(vm) || !part.is_finite() {
        return Ok(());
    }
    let part = part.clamp(-1.0, 1.0);
    let reached =
        crate::input::wheel_route(vm, gui, &mut |vm, gui, id| scroll_part(vm, gui, id, part))?;
    if !reached {
        gui.0.borrow_mut().glides.part.last = part;
    }
    Ok(())
}

/// `part` of a notch gathered, taken by the scroll control `id`: the content moves by the change of `part * row` since this
/// control's last part, so the whole notch's row, which comes as the part falls back by one, lands where a plain wheel's does.
/// Awake and visible, as the wheel's notch. Whether it was taken.
fn scroll_part(vm: &mut Vm, gui: &Gui, id: ObjId, part: f32) -> bool {
    if !gui.is_awake(id) || !flag(vm, id, b"visible") {
        return false;
    }
    let row = gui
        .ext()
        .scroll_line_sizes(vm, id)
        .map_or(DEFAULT_ROW, |(row, _)| row)
        .min(1 << 16) as f32;
    let target = (part * row).round() as i32;
    let had = gui.0.borrow().glides.part;
    let applied = if had.ctrl == Some(id) {
        had.applied
    } else {
        (had.last * row).round() as i32
    };
    let mut moved = 0;
    let delta = target.wrapping_sub(applied);
    if delta != 0 && !children(vm, id).is_empty() && state(gui, id).v_enabled {
        let before = state(gui, id).child_rel;
        scroll_delta(vm, gui, id, 0, delta.wrapping_neg());
        report_moved(vm, gui, id, before);
        // The pixels it did move: a content at the end of its range stays where it is, and so does what is owed.
        moved = before.1.wrapping_sub(state(gui, id).child_rel.1);
    }
    gui.0.borrow_mut().glides.part = WheelPart {
        ctrl: Some(id),
        applied: applied.wrapping_add(moved),
        last: part,
    };
    true
}

pub(crate) fn register(vm: &mut Vm, gui: &Gui) {
    vm.register(
        "getRevealFocus",
        1,
        1,
        "() - whether a control that takes the key focus scrolls its scroll control to show it ($Pref::Gui::RevealFocus)",
        |vm, _| Native::Bool(reveal_enabled(vm)),
    );
    vm.register(
        "setRevealFocus",
        2,
        2,
        "(bool on) - turns on or off scrolling the nearest scroll control to show the control that takes the key focus",
        |vm, c| {
            vm.set_global(REVEAL_PREF, if atob(crate::arg(c, 1)) { "1" } else { "0" });
            Native::Void
        },
    );
    vm.register(
        "getSmoothScroll",
        1,
        1,
        "() - whether the scroll controls glide to where the wheel puts them ($Pref::Gui::SmoothScroll)",
        |vm, _| Native::Bool(enabled(vm)),
    );
    let gui = gui.clone();
    vm.register(
        "setSmoothScroll",
        2,
        2,
        "(bool on) - turns smooth scrolling on or off: with it on, the scroll controls draw their content gliding to where the wheel puts it",
        move |vm, c| {
            set_enabled(vm, &gui, atob(crate::arg(c, 1)));
            Native::Void
        },
    );
}

#[cfg(test)]
#[path = "glide_tests.rs"]
mod tests;
