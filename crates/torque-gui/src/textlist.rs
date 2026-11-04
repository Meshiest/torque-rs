//! GuiArrayCtrl and GuiTextListCtrl as TGE 1.4 has them: the cells' size, the selection, the mouse; the rows
//! themselves are [`crate::Control::rows`] (`(id, text, active)`) and their script methods the classes'. Drawing is
//! [`crate::draw`]'s.
//!
//! - **`onWake`**: `GuiControl::onWake` (the script's included), then the profile's font, then `setSize(mSize)`.
//! - **`setSize`**: `mSize` kept; without a font the cell is left as it is; with one column and `fitParentWidth` the cell's
//!   width is a GuiScrollCtrl parent's content width (none: left as it is); otherwise it is the widest row plus 8, a row's width the
//!   largest `columns[c] + getStrNWidth(field c)` over its tab-separated fields (columns with a negative offset left out, at least 1;
//!   compared unsigned); the cell's height `font height + 2`; then `resize(position, (cell.x * size.x + header.x, cell.y *
//!   size.y + header.y))`.
//! - Every row change calls `setSize(1, rows)`: `addEntry`, `insertEntry` (the index clamped to the rows; the
//!   selection is not moved), `removeEntryByIndex` (the selection cleared to -1, no callback), `clear` (the rows
//!   removed one by one, so an empty list is not resized; the selection and the mouse-over cell -1).
//! - **`setSelectedCell`**: a valid cell is selected, scrolled visible in a scroll parent (`scrollRectVisible` of
//!   the cell), then `onCellSelected` (the script's `onSelect(id, text)`, then the command); an invalid one clears it.
//! - **The mouse** (down: active, awake and visible; the cell under the point selected; a double click on the same cell runs
//!   `altCommand`; move and drag, enter, leave: the mouse-over cell).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/core/guiArrayCtrl.cpp` (Copyright (c)
//! 2012 GarageGames, LLC), `gui/controls/guiTextListCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this
//! crate's own.

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use crate::control::{bounds, flag, is_a, parent};
use crate::profile::{profile_of, text};
use crate::{Gui, atoi};

/// A list's array state (GuiArrayCtrl's members).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListState {
    /// `mSize`, `mCellSize`, `mHeaderDim`.
    pub size: (i32, i32),
    pub cell: (i32, i32),
    pub header: (i32, i32),
    /// `mSelectedCell.x` (the row is [`crate::Control::selected`]) and `mMouseOverCell`.
    pub selected_x: i32,
    pub mouse_over: (i32, i32),
    /// `mFont` set (the profile's, at the first wake).
    pub font: bool,
}

impl Default for ListState {
    fn default() -> ListState {
        // TGE 1.4's GuiArrayCtrl constructor: one column, no rows, an 80 x 30 cell.
        ListState {
            size: (1, 0),
            cell: (80, 30),
            header: (0, 0),
            selected_x: -1,
            mouse_over: (-1, -1),
            font: false,
        }
    }
}

pub fn state(gui: &Gui, id: ObjId) -> ListState {
    gui.with(id, |c| c.list).flatten().unwrap_or_default()
}

fn store(gui: &Gui, id: ObjId, s: ListState) {
    gui.with_mut(id, |c| c.list = Some(s));
}

/// The `columns` field's offsets (`S32Vector`; "0" by default).
pub fn columns(vm: &Vm, id: ObjId) -> Vec<i32> {
    let t = text(vm, id, b"columns");
    let v: Vec<i32> = t
        .split(|b| *b == b' ' || *b == b'\t')
        .filter(|w| !w.is_empty())
        .take(256)
        .map(atoi)
        .collect();
    v
}

/// The profile font a list draws with.
pub fn font(vm: &Vm, gui: &Gui, id: ObjId) -> Option<std::sync::Arc<torque_fonts::Font>> {
    let p = profile_of(vm, id)?;
    let face = String::from_utf8_lossy(&text(vm, p, b"fontType")).into_owned();
    let size = crate::profile::int(vm, p, b"fontSize");
    let fonts = gui.0.borrow().fonts.0.clone();
    let read = |path: &str| match vm.file_host().read(path.as_bytes()) {
        torque_ts_vm::host::FileRead::Found(b) => Some(b),
        _ => None,
    };
    fonts.borrow_mut().font(&face, size, &read)
}

/// `GuiTextListCtrl::onWake`'s own part (after `GuiControl::onWake`): the font, then `setSize(mSize)`.
pub fn after_wake(vm: &mut Vm, gui: &Gui, id: ObjId) {
    let mut s = state(gui, id);
    s.font = true;
    store(gui, id, s);
    set_size(vm, gui, id, s.size);
}

/// `setSize`.
pub fn set_size(vm: &mut Vm, gui: &Gui, id: ObjId, size: (i32, i32)) {
    if !gui.is_control(id) {
        return;
    }
    let mut s = state(gui, id);
    s.size = size;
    let f = if s.font { font(vm, gui, id) } else { None };
    if let Some(f) = f {
        if size.0 == 1 && flag(vm, id, b"fitParentWidth") {
            if let Some(p) = parent(vm, id).filter(|p| is_a(vm, *p, "GuiScrollCtrl")) {
                s.cell.0 = crate::scroll::state(gui, p).content_ext.0;
            }
        } else {
            let cols = columns(vm, id);
            let rows: Vec<Vec<u8>> = gui
                .with(id, |c| c.rows.iter().map(|r| r.1.clone()).collect())
                .unwrap_or_default();
            vm.charge_work(rows.iter().map(|r| r.len() as u64 + 16).sum());
            let mut widest: u32 = 1;
            for row in &rows {
                let mut row_w: u32 = 1;
                let mut rest: &[u8] = row;
                for &off in &cols {
                    let (field, next) = match rest.iter().position(|b| *b == b'\t') {
                        Some(t) => (&rest[..t], Some(&rest[t + 1..])),
                        None => (rest, None),
                    };
                    if off >= 0 {
                        let w = (off as u32).wrapping_add(f.str_width(field) as u32);
                        row_w = row_w.max(w);
                    }
                    match next {
                        Some(n) => rest = n,
                        None => break,
                    }
                }
                widest = widest.max(row_w);
            }
            s.cell.0 = (widest as i32).wrapping_add(8);
        }
        s.cell.1 = (f.height as i32).wrapping_add(2);
        // An open popup's list with the extension's list pad ([`crate::GuiExtension::list_pad`]) keeps its padded cell when it
        // wakes (the rows that much taller and wider), and spans the scroll's width so a hovered row's fill reaches both sides.
        if let Some((_, crate::popup::Part::List)) = crate::popup::part_of(gui, id) {
            let profile = vm.objects().field(id, b"profile", b"").unwrap_or_default();
            if let Some(pad) = crate::popup::list_pad(vm, gui, &profile) {
                s.cell.1 += 2 * pad.1;
                s.cell.0 += (2 * pad.0 - 8).max(0);
                if let Some(p) = parent(vm, id).filter(|p| is_a(vm, *p, "GuiScrollCtrl")) {
                    s.cell.0 = s.cell.0.max(crate::scroll::state(gui, p).content_ext.0);
                }
            }
        }
    }
    store(gui, id, s);
    let (pos, _) = bounds(vm, id);
    let ext = (
        s.cell.0.wrapping_mul(size.0).wrapping_add(s.header.0),
        s.cell.1.wrapping_mul(size.1).wrapping_add(s.header.1),
    );
    crate::control::resize(vm, gui, id, pos, ext);
}

/// The rows changed: `setSize(1, rows)`.
pub fn rows_changed(vm: &mut Vm, gui: &Gui, id: ObjId) {
    let n = gui.with(id, |c| c.rows.len() as i32).unwrap_or(0);
    set_size(vm, gui, id, (1, n));
}

/// `setSelectedCell` of row `row` (column 0), with `onCellSelected`'s callbacks when `callbacks`.
pub fn set_selected(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    row: i32,
    callbacks: bool,
) -> Result<(), VmError> {
    let s = state(gui, id);
    let valid = row >= 0 && row < s.size.1 && s.size.0 > 0;
    let picked = gui
        .with_mut(id, |c| {
            if valid {
                c.selected = row;
                if let Some(l) = c.list.as_mut() {
                    l.selected_x = 0;
                }
                c.rows.get(row as usize).map(|r| (r.0, r.1.clone()))
            } else {
                c.selected = -1;
                if let Some(l) = c.list.as_mut() {
                    l.selected_x = -1;
                }
                None
            }
        })
        .flatten();
    if !valid {
        return Ok(());
    }
    // `scrollCellVisible`.
    if let Some(p) = parent(vm, id).filter(|p| is_a(vm, *p, "GuiScrollCtrl")) {
        crate::scroll::scroll_rect_visible(
            vm,
            gui,
            p,
            [0, s.cell.1.wrapping_mul(row), s.cell.0, s.cell.1],
        );
    }
    if !callbacks {
        return Ok(());
    }
    // `onCellSelected`: `onSelect(id, text)` (the id `%d`), then the command.
    let (row_id, text) = picked.unwrap_or((0, Vec::new()));
    vm.call_method(id, b"onSelect", &[row_id.to_string().into_bytes(), text])?;
    let command = vm.objects().field(id, b"command", b"").unwrap_or_default();
    if !command.is_empty() {
        vm.eval(&command)?;
    }
    Ok(())
}

/// The cell under a point in the control's coordinates (`-1` for a negative coordinate, else the quotient).
fn cell_at(s: &ListState, p: (i32, i32)) -> (i32, i32) {
    let x = p.0.wrapping_sub(s.header.0);
    let y = p.1.wrapping_sub(s.header.1);
    let cx = if x < 0 || s.cell.0 == 0 {
        -1
    } else {
        x / s.cell.0
    };
    let cy = if y < 0 || s.cell.1 == 0 {
        -1
    } else {
        y / s.cell.1
    };
    (cx, cy)
}

fn valid(s: &ListState, c: (i32, i32)) -> bool {
    c.0 >= 0 && c.0 < s.size.0 && c.1 >= 0 && c.1 < s.size.1
}

/// `onMouseDown`.
pub fn mouse_down(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    pt: (i32, i32),
    clicks: u8,
) -> Result<(), VmError> {
    if !crate::input::active(gui, id) || !gui.is_awake(id) || !flag(vm, id, b"visible") {
        return Ok(());
    }
    let s = state(gui, id);
    let c = cell_at(&s, crate::scroll::local(vm, id, pt));
    if !valid(&s, c) {
        return Ok(());
    }
    let old = (s.selected_x, gui.with(id, |c| c.selected).unwrap_or(-1));
    set_selected(vm, gui, id, c.1, true)?;
    let new = (
        state(gui, id).selected_x,
        gui.with(id, |c| c.selected).unwrap_or(-1),
    );
    if clicks > 1 && old == new {
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

/// `onMouseMove` / `onMouseDragged`: the mouse-over cell follows.
pub fn mouse_move(vm: &Vm, gui: &Gui, id: ObjId, pt: (i32, i32)) {
    let mut s = state(gui, id);
    let c = cell_at(&s, crate::scroll::local(vm, id, pt));
    if c != s.mouse_over {
        s.mouse_over = if valid(&s, c) { c } else { (-1, -1) };
        store(gui, id, s);
    }
}

/// `onMouseEnter`.
pub fn mouse_enter(vm: &Vm, gui: &Gui, id: ObjId, pt: (i32, i32)) {
    let mut s = state(gui, id);
    let c = cell_at(&s, crate::scroll::local(vm, id, pt));
    if valid(&s, c) {
        s.mouse_over = c;
        store(gui, id, s);
    }
}

/// `onMouseLeave`.
pub fn mouse_leave(gui: &Gui, id: ObjId) {
    let mut s = state(gui, id);
    s.mouse_over = (-1, -1);
    store(gui, id, s);
}

/// GuiTextListCtrl's `onKeyDown` over GuiArrayCtrl's: `None` sends the key to the parent. Not visible, active
/// and awake: taken. Return runs `altCommand`; Delete on a selected row calls `onDeleteKey(id)`; the arrows move the selection (from
/// row 0 when none), Page Up and Down by the parent's height in rows less one (at least 1), Home and End select the first and the
/// last row; each a `setSelectedCell` with its callbacks.
pub fn key_down(vm: &mut Vm, gui: &Gui, id: ObjId, key: u16) -> Result<Option<bool>, VmError> {
    use crate::keys::{
        KEY_DELETE, KEY_DOWN, KEY_END, KEY_HOME, KEY_LEFT, KEY_PAGE_DOWN, KEY_PAGE_UP, KEY_RETURN,
        KEY_RIGHT, KEY_UP,
    };
    if !flag(vm, id, b"visible") || !crate::input::active(gui, id) || !gui.is_awake(id) {
        return Ok(Some(true));
    }
    let s = state(gui, id);
    let row = gui.with(id, |c| c.selected).unwrap_or(-1);
    if key == KEY_RETURN {
        let alt = vm
            .objects()
            .field(id, b"altCommand", b"")
            .unwrap_or_default();
        if !alt.is_empty() {
            vm.eval(&alt)?;
        }
        return Ok(Some(true));
    }
    if key == KEY_DELETE && row >= 0 && (row as usize) < gui.with(id, |c| c.rows.len()).unwrap_or(0)
    {
        let rid = gui.with(id, |c| c.rows[row as usize].0).unwrap_or(0);
        vm.call_method(id, b"onDeleteKey", &[rid.to_string().into_bytes()])?;
        return Ok(Some(true));
    }
    let page = match parent(vm, id) {
        Some(p) if s.cell.1 > 0 => (bounds(vm, p).1.1 / s.cell.1 - 1).max(1),
        _ => 1,
    };
    let delta = match key {
        KEY_PAGE_UP => (0, -page),
        KEY_PAGE_DOWN => (0, page),
        KEY_LEFT => (-1, 0),
        KEY_RIGHT => (1, 0),
        KEY_UP => (0, -1),
        KEY_DOWN => (0, 1),
        KEY_HOME => {
            set_selected(vm, gui, id, 0, true)?;
            return Ok(Some(true));
        }
        KEY_END => {
            set_selected(vm, gui, id, s.size.1 - 1, true)?;
            return Ok(Some(true));
        }
        _ => return Ok(None),
    };
    if s.size.0 > 0 && s.size.1 > 0 {
        if s.selected_x == -1 || row == -1 {
            set_selected(vm, gui, id, 0, true)?;
        } else {
            // One column: the x stays 0 (`setSelectedCell` keeps the row's).
            let y = (row + delta.1).min(s.size.1 - 1).max(0);
            set_selected(vm, gui, id, y, true)?;
        }
    }
    Ok(Some(true))
}
