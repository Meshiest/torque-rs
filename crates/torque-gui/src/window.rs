//! GuiWindowCtrl as TGE 1.4 has it: the title bar's buttons, the
//! frame drawn from the profile's bitmap array (23 rectangles: close, maximize, normal, minimize three states each, then the frame's
//! top-left, top-right and top with and without the key focus, left, right, bottom-left, bottom, bottom-right), and the mouse on
//! the title and the buttons. Drawing is [`crate::draw`]'s (`onRender`).
//!
//! - **`onWake`**: GuiTextCtrl's (`GuiControl::onWake`, the variable, the automatic size), the profile's texture and bitmap
//!   array; with more than 22 rectangles the title height is the close button's height plus 4, the resize margins half of it, and
//!   the buttons placed (`positionButtons`).
//! - **`positionButtons`**: a button is the close bitmap's size; from the profile's `textOffset`, right-aligned (`justify`
//!   left, the default: from the right edge, each next button to the left by its width plus 2) or left-aligned; close first, then
//!   maximize and minimize (minimize nearest the close button when going left).
//! - **`resize`**: GuiControl's, then `positionButtons`.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/containers/guiWindowCtrl.cpp`
//! (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use crate::Gui;
use crate::control::{bounds, flag, point};
use crate::profile::{Rect, bitmap_array, int, profile_of};

/// The bitmap array's rectangles.
pub const BMP_CLOSE: usize = 0;
pub const BMP_MAX: usize = 3;
pub const BMP_NORMAL: usize = 6;
pub const BMP_MIN: usize = 9;
pub const TOP_LEFT_KEY: usize = 12;
pub const TOP_LEFT_NO_KEY: usize = 15;
pub const BORDER_LEFT: usize = 18;
pub const BORDER_RIGHT: usize = 19;
pub const BORDER_BOTTOM_LEFT: usize = 20;
pub const BORDER_BOTTOM: usize = 21;
pub const BORDER_BOTTOM_RIGHT: usize = 22;

/// A window's state (GuiWindowCtrl's members).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WindowState {
    /// `mTitleHeight`, the resize margins.
    pub title_height: i32,
    pub resize_width: i32,
    pub resize_height: i32,
    /// The buttons (close, minimize, maximize).
    pub close: Rect,
    pub minimize: Rect,
    pub maximize: Rect,
    /// `mMinimized`, `mMaximized`, the presses (close, minimize, maximize).
    pub minimized: bool,
    pub maximized: bool,
    pub press_close: bool,
    pub press_minimize: bool,
    pub press_maximize: bool,
    /// Dragging the title (`mMouseMovingWin`) or an edge from the point it went down and the
    /// bounds then.
    pub moving: bool,
    pub resizing_w: bool,
    pub resizing_h: bool,
    pub down_at: (i32, i32),
    pub orig_bounds: Rect,
    /// The bounds before a maximize or minimize and a minimized window's slot.
    pub std_bounds: Rect,
    pub minimize_index: i32,
}

pub fn state(gui: &Gui, id: ObjId) -> WindowState {
    gui.with(id, |c| c.window).flatten().unwrap_or_default()
}

fn store(gui: &Gui, id: ObjId, s: WindowState) {
    gui.with_mut(id, |c| c.window = Some(s));
}

/// `positionButtons`.
pub fn position_buttons(vm: &Vm, gui: &Gui, id: ObjId) {
    let Some(p) = profile_of(vm, id) else {
        return;
    };
    let rects = bitmap_array(vm, gui, p);
    let Some(close) = rects.first().copied() else {
        return;
    };
    let mut s = state(gui, id);
    let (bw, bh) = (close[2], close[3]);
    let off = point(vm, p, b"textOffset");
    let (_, ext) = bounds(vm, id);
    let mut x = off.0;
    let mut step = bw.wrapping_add(2);
    if crate::profile::justify(vm, Some(p)) == 0 {
        step = step.wrapping_neg();
        x = ext.0.wrapping_sub(x).wrapping_sub(bw);
    }
    s.close = [x, off.1, bw, bh];
    let next = x.wrapping_add(step);
    if step < 1 {
        s.maximize = [next, off.1, bw, bh];
        s.minimize = [next.wrapping_add(step), off.1, bw, bh];
    } else {
        s.minimize = [next, off.1, bw, bh];
        s.maximize = [next.wrapping_add(step), off.1, bw, bh];
    }
    store(gui, id, s);
}

/// GuiWindowCtrl's own `onWake` part (after GuiTextCtrl's).
pub fn after_wake(vm: &mut Vm, gui: &Gui, id: ObjId) {
    let Some(p) = profile_of(vm, id) else {
        return;
    };
    let rects = bitmap_array(vm, gui, p);
    if rects.len() > 22 {
        let mut s = state(gui, id);
        s.title_height = rects[0][3].wrapping_add(4);
        s.resize_width = s.title_height / 2;
        s.resize_height = s.title_height / 2;
        store(gui, id, s);
        position_buttons(vm, gui, id);
    }
}

/// The window's `resize`.
pub fn resize(vm: &mut Vm, gui: &Gui, id: ObjId, pos: (i32, i32), ext: (i32, i32), depth: usize) {
    crate::control::base_resize(vm, gui, id, pos, ext, depth);
    if gui.is_awake(id) {
        position_buttons(vm, gui, id);
    }
}

fn in_rect(r: Rect, p: (i32, i32)) -> bool {
    r[0] <= p.0 && p.0 < r[0].wrapping_add(r[2]) && r[1] <= p.1 && p.1 < r[1].wrapping_add(r[3])
}

/// Whether the point (in the window) is on a button the window has, and which: `(close, minimize, maximize)`.
pub fn button_at(vm: &Vm, gui: &Gui, id: ObjId, p: (i32, i32)) -> (bool, bool, bool) {
    let s = state(gui, id);
    (
        flag(vm, id, b"canClose") && in_rect(s.close, p),
        flag(vm, id, b"canMinimize") && in_rect(s.minimize, p),
        flag(vm, id, b"canMaximize") && in_rect(s.maximize, p),
    )
}

/// `selectWindow`: the window last of its parent's children (drawn on top), the script's `onWindowFocus`, its first
/// responder made the canvas's.
fn select_window(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    if let Some(p) = crate::control::parent(vm, id) {
        let kids = crate::control::children(vm, p);
        if kids.contains(&id) && kids.last() != Some(&id) {
            vm.objects_mut().move_member(p, id, false);
        }
    }
    vm.call_method(id, b"onWindowFocus", &[])?;
    if let Some(fr) = gui.with(id, |c| c.first_responder).flatten()
        && vm.objects().contains(fr)
    {
        crate::input::set_first_responder(vm, gui, fr)?;
    }
    Ok(())
}

/// `onMouseDown`: the bounds and the point kept, the window selected; in the title bar a button its flag allows is
/// pressed, else the window moves (`canMove`); below it the right and bottom edges resize (`resizeWidth`, `resizeHeight`). A press,
/// a move or a resize locks the mouse; anything else goes to the control under the point.
pub fn mouse_down(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    ev: crate::input::GuiEvent,
) -> Result<(), VmError> {
    let pt = ev.point;
    let (pos, ext) = bounds(vm, id);
    let mut s = state(gui, id);
    s.orig_bounds = [pos.0, pos.1, ext.0, ext.1];
    s.down_at = pt;
    store(gui, id, s);
    let p = crate::scroll::local(vm, id, pt);
    select_window(vm, gui, id)?;
    let mut s = state(gui, id);
    if p.1 < s.title_height {
        let (close, min, max) = button_at(vm, gui, id, p);
        if close {
            s.press_close = true;
        } else if max {
            s.press_maximize = true;
        } else if min {
            s.press_minimize = true;
        } else {
            s.moving = flag(vm, id, b"canMove");
            s.resizing_w = false;
            s.resizing_h = false;
        }
    } else {
        s.moving = false;
        if flag(vm, id, b"resizeWidth") && ext.0 - s.resize_width < p.0 {
            s.resizing_w = true;
        }
        if flag(vm, id, b"resizeHeight") && ext.1 - s.resize_height < p.1 {
            s.resizing_h = true;
        }
    }
    store(gui, id, s);
    if s.moving
        || s.resizing_w
        || s.resizing_h
        || s.press_close
        || s.press_maximize
        || s.press_minimize
    {
        return crate::input::mouse_lock(vm, gui, id);
    }
    // findHitControl over the window's children (the point in the window's coordinates).
    let hit = crate::canvas::hit_test_gui(vm, gui, id, p, crate::canvas::MAX_HIT_VISITS);
    if hit != id && vm.objects().contains(hit) {
        crate::input::mouse_down_on(vm, gui, hit, ev)?;
    }
    Ok(())
}

/// `onMouseDragged`: a move keeps the window inside its parent; a resize keeps the size within the parent's and at least
/// `minSize`; a pressed button only redraws.
pub fn mouse_dragged(vm: &mut Vm, gui: &Gui, id: ObjId, pt: (i32, i32)) {
    let Some(parent) = crate::control::parent(vm, id) else {
        return;
    };
    let s = state(gui, id);
    let (pos, ext) = bounds(vm, id);
    let pext = bounds(vm, parent).1;
    let (dx, dy) = (
        pt.0.wrapping_sub(s.down_at.0),
        pt.1.wrapping_sub(s.down_at.1),
    );
    let (mut np, mut ne) = (pos, ext);
    if s.moving {
        np.0 = (s.orig_bounds[0] + dx).min(pext.0 - ext.0).max(0);
        np.1 = (s.orig_bounds[1] + dy).min(pext.1 - ext.1).max(0);
    } else {
        if s.press_close || s.press_maximize || s.press_minimize {
            return;
        }
        let min = point(vm, id, b"minSize");
        let mut changed = false;
        if s.resizing_w {
            ne.0 = (s.orig_bounds[2] + dx).min(pext.0).max(min.0).max(0);
            changed = true;
        }
        if s.resizing_h {
            ne.1 = (s.orig_bounds[3] + dy).min(pext.1).max(min.1).max(0);
            changed = true;
        }
        if !changed {
            return;
        }
    }
    crate::control::resize(vm, gui, id, np, ne);
}

/// `onMouseUp`: the presses, the move and the resize end, the mouse unlocked; a press released on its button acts: close
/// runs `closeCommand`; maximize fills the parent (or puts the window back); minimize makes it a title bar at the parent's bottom
/// left, in the first free slot of rows of 100 pixels (or puts it back).
pub fn mouse_up(vm: &mut Vm, gui: &Gui, id: ObjId, pt: (i32, i32)) -> Result<(), VmError> {
    let mut s = state(gui, id);
    let (close, max, min) = (s.press_close, s.press_maximize, s.press_minimize);
    s.press_close = false;
    s.press_minimize = false;
    s.press_maximize = false;
    s.moving = false;
    s.resizing_w = false;
    s.resizing_h = false;
    store(gui, id, s);
    crate::input::mouse_unlock(vm, gui, id)?;
    let Some(parent) = crate::control::parent(vm, id) else {
        return Ok(());
    };
    let p = crate::scroll::local(vm, id, pt);
    let pext = bounds(vm, parent).1;
    if close && in_rect(s.close, p) {
        let cmd = vm
            .objects()
            .field(id, b"closeCommand", b"")
            .unwrap_or_default();
        if !cmd.is_empty() {
            vm.eval(&cmd)?;
        }
        return Ok(());
    }
    let (pos, ext) = bounds(vm, id);
    let restore = |s: &WindowState| {
        let y = (pext.1 - s.std_bounds[3]).min(s.std_bounds[1]).max(0);
        let x = (pext.0 - s.std_bounds[2]).min(s.std_bounds[0]).max(0);
        ((x, y), (s.std_bounds[2], s.std_bounds[3]))
    };
    if max && in_rect(s.maximize, p) {
        let mut s = state(gui, id);
        if s.maximized {
            let (np, ne) = restore(&s);
            s.maximized = false;
            store(gui, id, s);
            crate::control::resize(vm, gui, id, np, ne);
        } else {
            if s.minimized {
                s.minimized = false;
            } else {
                s.std_bounds = [pos.0, pos.1, ext.0, ext.1];
            }
            s.maximized = true;
            store(gui, id, s);
            crate::control::resize(vm, gui, id, (0, 0), pext);
        }
        return Ok(());
    }
    if min && in_rect(s.minimize, p) {
        let mut s = state(gui, id);
        if s.minimized {
            let (np, ne) = restore(&s);
            s.minimized = false;
            store(gui, id, s);
            crate::control::resize(vm, gui, id, np, ne);
            return Ok(());
        }
        if pext.0 < 100 || pext.1 < s.title_height + 3 {
            return Ok(());
        }
        if s.maximized {
            s.maximized = false;
        } else {
            s.std_bounds = [pos.0, pos.1, ext.0, ext.1];
        }
        // The slots the parent's other minimized windows hold (its first 32 children).
        let mut taken = 0u32;
        for c in crate::control::children(vm, parent).into_iter().take(32) {
            if crate::control::is_a(vm, c, "GuiWindowCtrl") && flag(vm, c, b"visible") {
                let w = state(gui, c);
                if w.minimized {
                    taken |= 1 << (w.minimize_index & 31);
                }
            }
        }
        let index = (0..32).find(|i| taken & (1 << i) == 0).unwrap_or(32);
        let per_row = (pext.0 / 98 + 2).max(1);
        let x = (index % per_row) * 100 + 2;
        let y = pext.1 - (s.title_height + 2) * (index / per_row + 1) - 2;
        s.minimize_index = index;
        s.minimized = true;
        store(gui, id, s);
        crate::control::resize(vm, gui, id, (x, y), (98, s.title_height));
    }
    Ok(())
}

/// The window's title bar height for hit tests elsewhere (0 for a window that has not woken).
pub fn title_height(gui: &Gui, id: ObjId) -> i32 {
    state(gui, id).title_height
}

/// An S32 of the profile, for the drawing (`int` re-exported for the draw module).
pub fn pint(vm: &Vm, profile: ObjId, name: &[u8]) -> i32 {
    int(vm, profile, name)
}
