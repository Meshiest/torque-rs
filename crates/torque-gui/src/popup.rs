//! GuiPopUpMenuCtrl's list as TGE 1.4 opens and closes it.
//!
//! - **`onMouseDown`**: an active popup runs `onAction`.
//! - **`onAction`** (`addChildren`): an enabled popup makes three controls: the list (a GuiPopUpTextListCtrl,
//!   which has no console class of its own, so script sees a `GuiTextListCtrl`; the popup's profile, `noDuplicates` false), a
//!   GuiScrollCtrl (the popup's profile, `hScrollBar` `AlwaysOff`, `vScrollBar` `dynamic`) and a background (a
//!   GuiPopUpBackgroundCtrl: a `GuiControl` over the whole canvas). The cell is the popup's width (the widest entry plus 10 when it
//!   is wider) by the font's height plus 2; the entries are added; the scroll control goes under the popup (one pixel up), or above
//!   it when there is no room below and there is above, with a vertical bar (the width plus 20) when neither holds the list; the
//!   background is pushed on the popup's parent's layer; the list scrolled to the selected entry when it has a bar; the list the
//!   first responder; `mInAction`.
//! - **`closePopUp`**: the list's selected row becomes the selection (the entry's text, the variable the entry's id;
//!   none: the old selection kept); the list unlocked, the background popped, the three deleted, the popup the first responder;
//!   then `onSelect(id, text)` or `onCancel()`, then the `command`.
//! - **The background's `onMouseDown`**: the list's selection cleared, closed. **The list's**: the array's
//!   (the row under the mouse selected), closed. **Its `onKeyDown`**: Return closes, Escape clears and closes, other keys
//!   are the text list's.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/controls/guiPopUpCtrl.cpp` (Copyright
//! (c) 2012 GarageGames, LLC); the code here is this crate's own.

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use crate::Gui;
use crate::control::{bounds, flag, parent, screen_position};

/// What a popup's part is: its list or its background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    List,
    Background,
}

/// An open popup's controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parts {
    pub list: ObjId,
    pub scroll: ObjId,
    pub background: ObjId,
}

/// The popup a control belongs to, and as what.
pub fn part_of(gui: &Gui, id: ObjId) -> Option<(ObjId, Part)> {
    gui.with(id, |c| c.popup_of).flatten()
}

fn is_name(t: &[u8]) -> bool {
    !t.is_empty() && t.len() < 256 && t.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// A new control of `class` with `fields` (`new <class>() {... };`), its id.
fn make(vm: &mut Vm, class: &str, fields: &[(&str, &[u8])]) -> Result<Option<ObjId>, VmError> {
    let mut code = format!("return new {class}() {{");
    for (name, value) in fields {
        code.push_str(&format!(
            " {name} = \"{}\";",
            String::from_utf8_lossy(value)
        ));
    }
    code.push_str(" };");
    let out = vm.eval(code.as_bytes())?;
    let id = crate::atoi(&out);
    Ok((id > 0)
        .then_some(id as ObjId)
        .filter(|o| vm.objects().contains(*o)))
}

/// `onAction`.
pub fn open(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    if !flag(vm, id, b"enabled") || gui.with(id, |c| c.in_action).unwrap_or(true) {
        return Ok(());
    }
    let Some(canvas) = gui.canvas() else {
        return Ok(());
    };
    let profile = vm.objects().field(id, b"profile", b"").unwrap_or_default();
    if !is_name(&profile) {
        return Ok(());
    }
    let Some(font) = crate::textlist::font(vm, gui, id) else {
        return Ok(());
    };
    let Some(list) = make(
        vm,
        "GuiTextListCtrl",
        &[("profile", &profile), ("noDuplicates", b"false")],
    )?
    else {
        return Ok(());
    };
    let Some(scroll) = make(
        vm,
        "GuiScrollCtrl",
        &[
            ("profile", &profile),
            ("hScrollBar", b"AlwaysOff"),
            ("vScrollBar", b"dynamic"),
        ],
    )?
    else {
        return Ok(());
    };
    let Some(background) = make(vm, "GuiControl", &[])? else {
        return Ok(());
    };
    // Made by the engine, not by script: script reads their fields as "" (see `SimObject::engine_made`).
    for o in [list, scroll, background] {
        if let Some(ob) = vm.objects_mut().get_mut(o) {
            ob.engine_made = true;
        }
    }
    gui.with_mut(list, |c| c.popup_of = Some((id, Part::List)));
    gui.with_mut(background, |c| c.popup_of = Some((id, Part::Background)));
    let (_, root) = bounds(vm, canvas);
    vm.set_object_field(background, b"position", b"", b"0 0");
    vm.set_object_field(
        background,
        b"extent",
        b"",
        format!("{} {}", root.0, root.1).as_bytes(),
    );
    let (_, ext) = bounds(vm, id);
    let entries: Vec<(i32, Vec<u8>)> = gui
        .with(id, |c| c.rows.iter().map(|r| (r.0, r.1.clone())).collect())
        .unwrap_or_default();
    vm.charge_work(entries.iter().map(|e| e.1.len() as u64 + 16).sum());
    let text_w = entries
        .iter()
        .map(|e| font.str_width(&e.1))
        .max()
        .unwrap_or(0)
        .max(0);
    // The extension's list pad ([`crate::GuiExtension::list_pad`], "x y") puts each row's text x in from the cell's left
    // (else 4) and y above and below it (the cell that much taller), and the list wide enough for it.
    let pad = list_pad(vm, gui, &profile);
    let extra = pad.map_or(0, |p| (2 * p.0 - 8).max(0));
    let mut width = if text_w + extra <= ext.0 {
        ext.0
    } else {
        text_w + 10 + extra
    };
    let cell_h = font.height as i32 + 2 + pad.map_or(0, |p| 2 * p.1);
    let set_cell = |gui: &Gui, w: i32| {
        gui.with_mut(list, |c| {
            let l = c
                .list
                .get_or_insert_with(crate::textlist::ListState::default);
            l.cell = (w, cell_h);
        });
    };
    set_cell(gui, width);
    gui.with_mut(list, |c| {
        c.rows = entries
            .iter()
            .take(crate::MAX_LIST_ROWS)
            .map(|e| (e.0, e.1.clone(), true))
            .collect();
    });
    crate::textlist::rows_changed(vm, gui, list);
    let at = screen_position(vm, id);
    let mut point = (at.0, at.1 + ext.1 - 1);
    let mut max_y = root.1 - ext.1 - at.1 - 5;
    let list_h = bounds(vm, list).1.1 + 2;
    let mut set_scroll = false;
    if max_y < list_h {
        let above = at.1 - 5;
        if max_y < above {
            if list_h <= above {
                point.1 = at.1 + (-1 - list_h);
                max_y = list_h;
            } else {
                if root.1 >= width {
                    width += 20;
                }
                set_cell(gui, width);
                point = (at.0, 4);
                max_y = above;
                set_scroll = true;
            }
        } else {
            if root.1 >= width {
                width += 20;
            }
            set_cell(gui, width);
            set_scroll = true;
        }
    } else {
        max_y = list_h;
    }
    let mut sx = point.0;
    if root.0 < width + sx && width != ext.0 && width - ext.0 >= 0 {
        sx = ext.0 + (sx - width);
    }
    vm.set_object_field(
        scroll,
        b"position",
        b"",
        format!("{} {}", sx, point.1).as_bytes(),
    );
    vm.set_object_field(
        scroll,
        b"extent",
        b"",
        format!("{} {}", width, max_y).as_bytes(),
    );
    crate::control::add_child(vm, gui, scroll, list)?;
    crate::control::add_child(vm, gui, background, scroll)?;
    let layer = parent(vm, id)
        .and_then(|p| gui.with(p, |c| c.layer))
        .unwrap_or(0);
    gui.with_mut(id, |c| {
        c.in_action = true;
        c.popup = Some(Parts {
            list,
            scroll,
            background,
        });
    });
    crate::canvas::push_dialog(vm, gui, background, layer)?;
    if set_scroll {
        let sel = gui.with(id, |c| c.selected).unwrap_or(-1);
        let cell = crate::textlist::state(gui, list).cell;
        if sel >= 0 {
            crate::scroll::scroll_rect_visible(vm, gui, scroll, [0, sel * cell.1, cell.0, cell.1]);
        }
    }
    if gui.is_awake(list) && flag(vm, list, b"visible") {
        crate::input::set_first_responder(vm, gui, list)?;
    }
    Ok(())
}

/// A popup list's text inset for the profile named `profile` ([`crate::GuiExtension::list_pad`]), None: the engine's.
pub fn list_pad(vm: &Vm, gui: &Gui, profile: &[u8]) -> Option<(i32, i32)> {
    gui.ext().list_pad(vm, profile)
}

/// `closePopUp`.
pub fn close(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    let Some(parts) = gui.with(id, |c| c.popup.filter(|_| c.in_action)).flatten() else {
        return Ok(());
    };
    let old = gui.with(id, |c| c.selected).unwrap_or(-1);
    let sel = gui.with(parts.list, |c| c.selected).unwrap_or(-1);
    let entry = gui
        .with(id, |c| {
            usize::try_from(sel)
                .ok()
                .and_then(|i| c.rows.get(i))
                .map(|r| (r.0, r.1.clone()))
        })
        .flatten();
    let sel = if entry.is_some() { sel } else { -1 };
    gui.with_mut(id, |c| c.selected = sel);
    if let Some((entry_id, text)) = &entry {
        crate::classes::set_text(vm, gui, id, text);
        let var = vm.objects().field(id, b"variable", b"").unwrap_or_default();
        if !var.is_empty() {
            let name = String::from_utf8_lossy(&var).into_owned();
            let name = if name.starts_with('$') {
                name
            } else {
                format!("${name}")
            };
            vm.set_global(&name, &entry_id.to_string());
        }
    }
    gui.with_mut(id, |c| {
        c.in_action = false;
        c.popup = None;
    });
    crate::input::mouse_unlock(vm, gui, parts.list)?;
    crate::canvas::pop_dialog(vm, gui, Some(parts.background))?;
    for o in [parts.list, parts.scroll, parts.background] {
        if vm.objects().contains(o) {
            vm.delete_object(o)?;
        }
    }
    crate::input::set_first_responder(vm, gui, id)?;
    match entry {
        None => {
            gui.with_mut(id, |c| c.selected = old);
            vm.call_method(id, b"onCancel", &[])?;
        }
        Some((entry_id, text)) => {
            vm.call_method(id, b"onSelect", &[entry_id.to_string().into_bytes(), text])?;
        }
    }
    let command = vm.objects().field(id, b"command", b"").unwrap_or_default();
    if !command.is_empty() {
        vm.eval(&command)?;
    }
    Ok(())
}

/// The list's selection cleared (`mSelectedCell = (-1, -1)`).
fn clear_list(gui: &Gui, list: ObjId) {
    gui.with_mut(list, |c| {
        c.selected = -1;
        if let Some(l) = c.list.as_mut() {
            l.selected_x = -1;
        }
    });
}

/// A part's mouse going down: the background's or the list's.
pub fn part_mouse_down(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    pt: (i32, i32),
    clicks: u8,
) -> Result<(), VmError> {
    let Some((popup, part)) = part_of(gui, id) else {
        return Ok(());
    };
    match part {
        Part::Background => {
            if let Some(p) = gui.with(popup, |c| c.popup).flatten() {
                clear_list(gui, p.list);
            }
        }
        Part::List => crate::textlist::mouse_down(vm, gui, id, pt, clicks)?,
    }
    close(vm, gui, popup)
}

/// The list's `onKeyDown`: whether it was taken.
pub fn list_key_down(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    key: u16,
    modifier: u8,
) -> Result<Option<bool>, VmError> {
    let Some((popup, Part::List)) = part_of(gui, id) else {
        return Ok(None);
    };
    if !flag(vm, id, b"visible") || !crate::input::active(gui, id) || !gui.is_awake(id) {
        return Ok(Some(false));
    }
    if modifier == 0 && (key == crate::keys::KEY_RETURN || key == crate::keys::KEY_ESCAPE) {
        if key == crate::keys::KEY_ESCAPE {
            clear_list(gui, id);
        }
        close(vm, gui, popup)?;
        return Ok(Some(true));
    }
    Ok(None)
}
