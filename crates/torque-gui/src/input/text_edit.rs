//! GuiTextEditCtrl's input (`onKeyDown`, `onMouseDown`, `onMouseUp`, `onLoseFirstResponder`, the history, `setText`).
//!
//! The text is `mText` (the control's text, at most 255 bytes); the cursor, the selection (`mBlockStart`..`mBlockEnd`, 0..0 for none)
//! and the rest are [`EditState`]. What a key does, in this order:
//! - **Shift held** (either side): Tab with `tabComplete` calls `onTabComplete("1")`, without it goes on as a character key (to
//!   the parent; with the extension's [`crate::ext::GuiRule::ShiftTabSelectsToStart`] it selects to the start, as Shift-Home);
//!   Home and End select to the start and the end; Left and Right grow or shrink the selection; other keys are characters.
//! - **No Ctrl, Left Alt or the 0x80 bit** (`modifier & 0x9c == 0`): Backspace and Delete (the selection, else one byte), Return and
//!   numpad Enter (`onLoseFirstResponder`, then `altCommand`; with the profile's `returnTab` the canvas tabs on), Escape
//!   (`escapeCommand`, else the parent), Home, End, Left, Right (the selection dropped), Up and Down (the history; consumed even
//!   without one), Insert (insert/overwrite); other keys are characters.
//! - **With one of them**: Ctrl+A/B/D/E/F/N/P/U are Home/Left/Delete/End/Right/Down/Up and Ctrl+Delete (Emacs keys: the key is sent
//!   again), Ctrl+C and Ctrl+X copy (and cut) the selection unless `password`, Ctrl+V pastes (`onPaste`), Ctrl+Z swaps the text with
//!   the undo copy (during a drag: clears the text), Ctrl+Backspace and Ctrl+Delete clear the text; other keys are characters.
//! - **A character**: Tab, Escape, Up and Down go to the parent; a byte the font has no glyph for is consumed with
//!   `sinkAllKeyEvents`, else goes to the parent; a profile with `numbersOnly` refuses what is not a digit (a `-` only at the start);
//!   the selection is replaced; a repeat inserts only after a make; within `maxLength` (overwrite mode: the cursor's place, insert
//!   mode: the length) the byte is inserted or overwritten, else refused.
//! - After a change: the variable gets the text and the `command` runs (with `$ThisControl`).
//!
//! The clipboard is the system's when the window set [`super::SYSTEM_CLIPBOARD`], else the canvas's own
//! ([`super::CanvasInput::clipboard`]).
//!
//! The above is TGE's, kept with the extension's [`GuiRule::TgeEditKeys`]. Without it the engine takes the usual desktop keys
//! first (Alt held: every key is a character's):
//! - Ctrl+A selects all; Ctrl+C and Ctrl+Insert copy, Ctrl+X and Shift+Delete cut (not a `password`), Ctrl+V and Shift+Insert
//!   paste over the selection (its first line, digits only with `numbersOnly`, as much as `maxLength` leaves room for).
//! - Left and Right move a character (with a selection and no Shift: to its start or end), with Ctrl a word ([`word_left`],
//!   [`word_right`]; a `password` is one word); Home and End to the start and the end; Shift selects from the selection's
//!   anchor to where the cursor goes.
//! - Backspace and Delete remove the selection, else a character, with Ctrl a word.
//! - Ctrl+Z undoes a step and Ctrl+Y and Ctrl+Shift+Z redo one: typing in a row is one step (as are Backspaces and Deletes in
//!   a row; a move or a click ends the row), every other change (a paste, a cut, a word deleted, Up and Down's history line)
//!   is a step of its own; at most [`MAX_UNDO_STEPS`] are kept. Undo and redo run the variable and the command as a change.
//! - Other Ctrl keys are characters (no Emacs keys, so they go on to the parent and the accelerators).
//! - A click places the cursor at the nearest gap between characters of the text as drawn (scrolled); a double click selects
//!   the word under it, a triple click all; Shift+click selects to the click; a drag after a double or triple click keeps
//!   what it selected and grows it to the mouse.
//! - Typing over a selection measures `maxLength` against the text without it; paste and cut run the variable and command.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/controls/guiTextEditCtrl.cpp`
//! (Copyright (c) 2012 GarageGames, LLC), `gui/controls/guiTextEditSliderCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC); the
//! code here is this crate's own.

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use super::{
    EditState, GuiEvent, MAX_HISTORY, exec_command, input, mouse_lock, on_key_down_at,
    parent_key_down, profile, profile_flag, set_first_responder_of, tab_next, text_field,
    variable_name,
};
use crate::control::{flag, parent, point, screen_position};
use crate::ext::GuiRule;
use crate::keys::*;
use crate::{Gui, classes};

/// `mText`'s size less its NUL.
pub const MAX_EDIT_TEXT: usize = 255;
/// The most undo steps an edit keeps with the engine's keys.
pub const MAX_UNDO_STEPS: usize = 100;

#[path = "edit_keys.rs"]
mod edit_keys;
pub use edit_keys::{Merge, word_left, word_right};

fn text(gui: &Gui, id: ObjId) -> Vec<u8> {
    gui.with(id, |c| c.text.clone().unwrap_or_default())
        .unwrap_or_default()
}

fn set_raw(gui: &Gui, id: ObjId, t: Vec<u8>) {
    let mut t = t;
    t.truncate(MAX_EDIT_TEXT);
    gui.with_mut(id, |c| c.text = Some(t));
}

fn edit<R>(gui: &Gui, id: ObjId, f: impl FnOnce(&mut EditState) -> R) -> Option<R> {
    gui.with_mut(id, |c| {
        f(c.edit.get_or_insert_with(|| Box::new(EditState::new())))
    })
}

/// `GuiTextEditCtrl::setText`: `GuiTextCtrl::setText` (255 bytes, the variable), then the cursor at the end.
pub fn set_text(vm: &mut Vm, gui: &Gui, id: ObjId, t: &[u8]) {
    if crate::control::is_a(vm, id, "GuiTextEditSliderCtrl") {
        // The value read (`atof`, a float) and kept to the range (below its low end the low end, above its high end
        // the high end), then `dSprintf` of the value through the control's `format` into 20 bytes, `GuiTextCtrl::setText`
        // of that, the cursor at its end.
        let v = torque_ts_vm::text::atof(t) as f32;
        let range = vm.objects().field(id, b"range", b"").unwrap_or_default();
        let mut lohi = [0f32; 2];
        let n = torque_ts_vm::text::scan_floats(&range, &mut lohi);
        let (lo, hi) = if n >= 2 {
            (lohi[0], lohi[1])
        } else {
            (0.0, 1.0)
        };
        let v = if v < lo {
            lo
        } else if hi < v {
            hi
        } else {
            v
        };
        gui.with_mut(id, |c| c.progress = v);
        let format = vm.objects().field(id, b"format", b"").unwrap_or_default();
        let shown = torque_ts_vm::text::sprintf_double(&format, f64::from(v), 19);
        classes::set_text(vm, gui, id, &shown);
    } else {
        classes::set_text(vm, gui, id, t);
    }
    let len = text(gui, id).len() as i32;
    edit(gui, id, |e| {
        e.cursor = len;
        e.undo_merge = None;
    });
}

/// The font the edit measures with.
fn font(vm: &Vm, gui: &Gui, id: ObjId) -> Option<std::sync::Arc<torque_fonts::Font>> {
    let p = profile(vm, id)?;
    let face = String::from_utf8_lossy(&text_field(vm, p, b"fontType")).into_owned();
    let size = torque_ts_vm::text::atoi(&text_field(vm, p, b"fontSize"));
    let fonts = gui.0.borrow().fonts.0.clone();
    let read = |path: &str| match vm.file_host().read(path.as_bytes()) {
        torque_ts_vm::host::FileRead::Found(b) => Some(b),
        _ => None,
    };
    fonts.borrow_mut().font(&face, size, &read)
}

/// `setCursorPos(point)` (TGE 1.4): -1 left of the control, -2 at or past `ctrlOffset.x + extent.x` from it (TGE's measure), else the
/// byte whose advances first pass the point from the text's start (`mTextOffset.x`: the control's left plus the profile's text
/// offset; scrolled text is not drawn yet).
fn cursor_at(vm: &Vm, gui: &Gui, id: ObjId, pt: (i32, i32)) -> i32 {
    let at = screen_position(vm, id);
    let ext = point(vm, id, b"extent");
    let cur_x = pt.0.wrapping_sub(at.0);
    if cur_x < 0 {
        return -1;
    }
    if cur_x >= at.0.wrapping_add(ext.0) {
        return -2;
    }
    let offset = profile(vm, id).map_or(0, |p| point(vm, p, b"textOffset").0);
    let cur_x = pt.0.wrapping_sub(at.0.wrapping_add(offset));
    let t = text(gui, id);
    let password = flag(vm, id, b"password");
    let Some(f) = font(vm, gui, id) else {
        return t.len() as i32;
    };
    let mut width = 0i32;
    let mut count = 0usize;
    while count < t.len() {
        let c = if password { b'*' } else { t[count] };
        if !password && !f.is_valid_char(c) {
            count += 1;
            continue;
        }
        width = width.wrapping_add(f.glyph(c).map_or(0, |g| i32::from(g.x_increment)));
        if width > cur_x {
            break;
        }
        count += 1;
    }
    count as i32
}

/// `onMouseDown`: the selection dropped, the cursor where the click is, the mouse locked, the first responder.
pub fn mouse_down(vm: &mut Vm, gui: &Gui, id: ObjId, ev: GuiEvent) -> Result<(), VmError> {
    if !gui.ext().rule(GuiRule::TgeEditKeys) {
        edit_keys::mouse_down(vm, gui, id, ev);
        return take_mouse(vm, gui, id);
    }
    let pos = cursor_at(vm, gui, id, ev.point);
    let len = text(gui, id).len() as i32;
    let pos = match pos {
        -1 => 0,
        -2 => len,
        p => p,
    };
    edit(gui, id, |e| {
        e.drag_hit = false;
        e.block_start = 0;
        e.block_end = 0;
        e.cursor = pos;
        e.drag_start = pos;
    });
    take_mouse(vm, gui, id)
}

/// The tail of `onMouseDown`: the mouse locked, the first responder.
fn take_mouse(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    mouse_lock(vm, gui, id)?;
    edit(gui, id, |e| e.drag_hit = true);
    if gui.is_awake(id)
        && flag(vm, id, b"visible")
        && profile_flag(vm, id, b"canKeyFocus")
        && let Some(p) = parent(vm, id)
    {
        set_first_responder_of(vm, gui, p, id, 0)?;
    }
    Ok(())
}

/// `onMouseDragged` (TGE 1.4): the selection from the drag's start to the cursor.
pub fn mouse_dragged(vm: &mut Vm, gui: &Gui, id: ObjId, ev: GuiEvent) -> Result<(), VmError> {
    if !gui.ext().rule(GuiRule::TgeEditKeys) {
        edit_keys::mouse_dragged(vm, gui, id, ev);
        return Ok(());
    }
    let pos = cursor_at(vm, gui, id, ev.point);
    let len = text(gui, id).len() as i32;
    edit(gui, id, |e| {
        match pos {
            -1 => {
                e.block_start = 0;
                e.block_end = e.drag_start;
                e.cursor = 0;
            }
            -2 => {
                e.block_start = e.drag_start;
                e.block_end = len;
                e.cursor = len;
            }
            p => {
                e.cursor = p;
                if p <= e.drag_start {
                    e.block_start = p;
                    e.block_end = e.drag_start;
                } else {
                    e.block_start = e.drag_start;
                    e.block_end = p;
                }
            }
        }
        if e.block_start == e.block_end {
            e.block_start = 0;
            e.block_end = 0;
        }
    });
    Ok(())
}

/// The history: `text` added unless it is the last line (the oldest goes when full); with `move_index` the index past it.
fn update_history(vm: &Vm, gui: &Gui, id: ObjId, t: &[u8], move_index: bool) {
    let cap = torque_ts_vm::text::atoi(&text_field(vm, id, b"historySize"))
        .clamp(0, MAX_HISTORY as i32) as usize;
    if cap == 0 {
        return;
    }
    let mut t = t.to_vec();
    t.truncate(MAX_EDIT_TEXT);
    edit(gui, id, |e| {
        let last = e.history_last;
        let same = last >= 0 && e.history.get(last as usize).is_some_and(|h| *h == t);
        if !same {
            if last == cap as i32 - 1 && !e.history.is_empty() {
                e.history.remove(0);
                e.history.push(t);
                if e.history_index > 0 {
                    e.history_index -= 1;
                }
            } else {
                e.history_last = last + 1;
                let at = e.history_last as usize;
                if e.history.len() <= at {
                    e.history.resize(at + 1, Vec::new());
                }
                e.history[at] = t;
            }
        }
        if move_index {
            e.history_index = e.history_last + 1;
        }
    });
}

/// `onLoseFirstResponder`: the text into the history, then `validate` (no `$ThisControl`).
pub fn lose_first_responder(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    let t = text(gui, id);
    update_history(vm, gui, id, &t, true);
    let validate = text_field(vm, id, b"validate");
    if !validate.is_empty() {
        vm.eval(&validate)?;
    }
    Ok(())
}

/// What a change does after it (the tail of `onKeyDown`): the variable, then the command.
fn changed(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<bool, VmError> {
    edit(gui, id, |e| e.changes_made = true);
    if let Some(var) = variable_name(vm, id) {
        let t = text(gui, id);
        vm.set_global(&var, &String::from_utf8_lossy(&t));
    }
    let command = text_field(vm, id, b"command");
    if !command.is_empty() {
        exec_command(vm, id, &command)?;
    }
    Ok(true)
}

/// The undo copy (before a change).
fn save_undo(gui: &Gui, id: ObjId) {
    let t = text(gui, id);
    edit(gui, id, |e| {
        e.undo = (t, e.block_start, e.block_end, e.cursor)
    });
}

/// Deletes `[start, end)` and puts the cursor at `start`, the selection dropped.
fn delete_range(gui: &Gui, id: ObjId, start: i32, end: i32) {
    let mut t = text(gui, id);
    let s = (start.max(0) as usize).min(t.len());
    let e = (end.max(0) as usize).min(t.len()).max(s);
    t.drain(s..e);
    set_raw(gui, id, t);
    edit(gui, id, |st| {
        st.cursor = start;
        st.block_start = 0;
        st.block_end = 0;
    });
}

fn denied(vm: &mut Vm, gui: &Gui, id: ObjId) {
    // `playDeniedSound`: the `deniedSound` field's profile, whatever `$Pref::Audio::MenuSounds` says (only the buttons'
    // sounds read it); the audio plays it.
    let name = text_field(vm, id, b"deniedSound");
    if name.is_empty() {
        return;
    }
    if let Some(sound) = vm.objects().find(&name) {
        let hook = gui.0.borrow().sound_hook.0.clone();
        if let Some(hook) = hook {
            hook(vm, sound);
        }
    }
}

/// `onKeyDown`: see the module documentation. Whether the key was consumed.
pub fn key_down(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    ev: GuiEvent,
    depth: usize,
) -> Result<bool, VmError> {
    if !flag(vm, id, b"enabled") {
        return Ok(false);
    }
    let len = text(gui, id).len() as i32;
    let st = edit(gui, id, |e| e.clone()).unwrap_or_else(EditState::new);
    if !gui.ext().rule(GuiRule::TgeEditKeys)
        && let Some(taken) = edit_keys::key_down(vm, gui, id, ev, depth)?
    {
        return Ok(taken);
    }
    let key = u16::from(ev.key_code);
    let m = ev.modifier;
    if m & SI_SHIFT != 0 {
        match key {
            KEY_TAB if flag(vm, id, b"tabComplete") => {
                vm.call_method(id, b"onTabComplete", &[b"1".to_vec()])?;
                return Ok(true);
            }
            KEY_TAB if !gui.ext().rule(GuiRule::ShiftTabSelectsToStart) => {
                return character(vm, gui, id, ev, len, depth);
            }
            KEY_TAB | KEY_HOME => {
                edit(gui, id, |e| {
                    e.block_end = e.cursor;
                    e.block_start = 0;
                    e.cursor = 0;
                });
                return Ok(true);
            }
            KEY_END => {
                edit(gui, id, |e| {
                    e.block_start = e.cursor;
                    e.block_end = len;
                    e.cursor = len;
                });
                return Ok(true);
            }
            KEY_LEFT => {
                edit(gui, id, |e| {
                    if e.cursor > 0 && len > 0 {
                        if e.cursor == e.block_end {
                            e.cursor -= 1;
                            e.block_end -= 1;
                            if e.block_end == e.block_start {
                                e.block_start = 0;
                                e.block_end = 0;
                            }
                        } else {
                            e.cursor -= 1;
                            e.block_start = e.cursor;
                            if e.block_end == 0 {
                                e.block_end = e.cursor + 1;
                            }
                        }
                    }
                });
                return Ok(true);
            }
            KEY_RIGHT => {
                edit(gui, id, |e| {
                    if e.cursor < len {
                        if e.cursor == e.block_start && e.block_end > 0 {
                            e.cursor += 1;
                            e.block_start += 1;
                            if e.block_start == e.block_end {
                                e.block_start = 0;
                                e.block_end = 0;
                            }
                        } else {
                            if e.block_end == 0 {
                                e.block_start = e.cursor;
                                e.block_end = e.cursor;
                            }
                            e.cursor += 1;
                            e.block_end += 1;
                        }
                    }
                });
                return Ok(true);
            }
            _ => return character(vm, gui, id, ev, len, depth),
        }
    }
    if m & 0x9c == 0 {
        match key {
            KEY_BACKSPACE => {
                save_undo(gui, id);
                if st.block_end > 0 {
                    delete_range(gui, id, st.block_start, st.block_end);
                } else if st.cursor > 0 {
                    delete_range(gui, id, st.cursor - 1, st.cursor);
                } else {
                    return Ok(true);
                }
                return changed(vm, gui, id);
            }
            KEY_DELETE => {
                save_undo(gui, id);
                if st.block_end > 0 {
                    delete_range(gui, id, st.block_start, st.block_end);
                } else if st.cursor < len {
                    delete_range(gui, id, st.cursor, st.cursor + 1);
                } else {
                    return Ok(true);
                }
                return changed(vm, gui, id);
            }
            KEY_RETURN | KEY_NUMPADENTER => {
                super::on_lose_first_responder(vm, gui, id)?;
                edit(gui, id, |e| e.changes_made = false);
                let alt = text_field(vm, id, b"altCommand");
                if !alt.is_empty() {
                    exec_command(vm, id, &alt)?;
                }
                if profile_flag(vm, id, b"returnTab") {
                    tab_next(vm, gui)?;
                }
                return Ok(true);
            }
            KEY_ESCAPE => {
                let esc = text_field(vm, id, b"escapeCommand");
                if !esc.is_empty() {
                    vm.eval(&esc)?;
                    return Ok(true);
                }
                return parent_key_down(vm, gui, id, ev, depth);
            }
            KEY_END => {
                edit(gui, id, |e| {
                    e.block_start = 0;
                    e.block_end = 0;
                    e.cursor = len;
                });
                return Ok(true);
            }
            KEY_HOME => {
                edit(gui, id, |e| {
                    e.block_start = 0;
                    e.block_end = 0;
                    e.cursor = 0;
                });
                return Ok(true);
            }
            KEY_LEFT => {
                edit(gui, id, |e| {
                    e.block_start = 0;
                    e.block_end = 0;
                    if e.cursor > 0 {
                        e.cursor -= 1;
                    }
                });
                return Ok(true);
            }
            KEY_RIGHT => {
                edit(gui, id, |e| {
                    e.block_start = 0;
                    e.block_end = 0;
                    if e.cursor < len {
                        e.cursor += 1;
                    }
                });
                return Ok(true);
            }
            KEY_UP | KEY_DOWN => {
                history_key(vm, gui, id, key == KEY_UP);
                return Ok(true);
            }
            KEY_INSERT => {
                edit(gui, id, |e| e.insert_on = !e.insert_on);
                return Ok(true);
            }
            _ => return character(vm, gui, id, ev, len, depth),
        }
    }
    // Ctrl, Left Alt or the 0x80 bit.
    let emacs = match key {
        0x21 => Some((KEY_HOME, 0)),
        0x22 => Some((KEY_LEFT, 0)),
        0x24 => Some((KEY_DELETE, 0)),
        0x25 => Some((KEY_END, 0)),
        0x26 => Some((KEY_RIGHT, 0)),
        0x2e => Some((KEY_DOWN, 0)),
        0x30 => Some((KEY_UP, 0)),
        0x35 => Some((KEY_DELETE, SI_CTRL)),
        _ => None,
    };
    if let Some((k, mods)) = emacs {
        let again = GuiEvent {
            key_code: k as u8,
            modifier: mods,
            ..ev
        };
        return on_key_down_at(vm, gui, id, again, depth + 1);
    }
    match key {
        0x23 | 0x38 => {
            if !flag(vm, id, b"password") && st.block_end > 0 {
                save_undo(gui, id);
                let t = text(gui, id);
                let s = (st.block_start.max(0) as usize).min(t.len());
                let e = (st.block_end.max(0) as usize).min(t.len()).max(s);
                input(gui).clipboard = t[s..e].to_vec();
                if let Some(c) = super::SYSTEM_CLIPBOARD.get() {
                    (c.set)(&super::clipboard_text(&t[s..e]));
                }
                if key == 0x38 {
                    delete_range(gui, id, st.block_start, st.block_end);
                } else {
                    edit(gui, id, |e| {
                        e.block_start = 0;
                        e.block_end = 0;
                    });
                }
            }
            Ok(true)
        }
        0x36 => {
            let clip = match super::SYSTEM_CLIPBOARD.get() {
                Some(c) => (c.get)()
                    .map(|t| super::clipboard_bytes(&t))
                    .unwrap_or_default(),
                None => input(gui).clipboard.clone(),
            };
            if clip.is_empty() {
                return Ok(true);
            }
            save_undo(gui, id);
            if st.block_end > 0 {
                delete_range(gui, id, st.block_start, st.block_end);
            }
            let max = torque_ts_vm::text::atoi(&text_field(vm, id, b"maxLength")).max(0) as usize;
            let mut t = text(gui, id);
            let room = max.min(MAX_EDIT_TEXT).saturating_sub(len as usize);
            let take = clip.len().min(room);
            let cursor = edit(gui, id, |e| e.cursor)
                .unwrap_or(0)
                .clamp(0, t.len() as i32) as usize;
            for (k, b) in clip[..take].iter().enumerate() {
                t.insert(cursor + k, *b);
            }
            set_raw(gui, id, t);
            edit(gui, id, |e| e.cursor += take as i32);
            vm.call_method(id, b"onPaste", &[])?;
            Ok(true)
        }
        0x3a if !st.drag_hit => {
            let cur = (text(gui, id), st.block_start, st.block_end, st.cursor);
            let (ut, us, ue, uc) = st.undo.clone();
            set_raw(gui, id, ut);
            edit(gui, id, |e| {
                e.block_start = us;
                e.block_end = ue;
                e.cursor = uc;
                e.undo = cur;
            });
            Ok(true)
        }
        0x3a | KEY_BACKSPACE | KEY_DELETE => {
            save_undo(gui, id);
            set_raw(gui, id, Vec::new());
            edit(gui, id, |e| {
                e.cursor = 0;
                e.block_start = 0;
                e.block_end = 0;
            });
            changed(vm, gui, id)
        }
        _ => character(vm, gui, id, ev, len, depth),
    }
}

/// Up and Down (no modifier): the history.
fn history_key(vm: &mut Vm, gui: &Gui, id: ObjId, up: bool) {
    let cap = torque_ts_vm::text::atoi(&text_field(vm, id, b"historySize")).max(0);
    if cap <= 0 {
        return;
    }
    if edit(gui, id, |e| std::mem::replace(&mut e.changes_made, false)).unwrap_or(false) {
        let t = text(gui, id);
        update_history(vm, gui, id, &t, false);
    }
    let line = edit(gui, id, |e| {
        if up {
            e.history_index -= 1;
            if e.history_index < 0 {
                e.history_index = 0;
                None
            } else if e.history_index <= e.history_last {
                Some(
                    e.history
                        .get(e.history_index as usize)
                        .cloned()
                        .unwrap_or_default(),
                )
            } else {
                None
            }
        } else {
            e.history_index += 1;
            if e.history_index > e.history_last {
                e.history_index = e.history_last + 1;
                Some(Vec::new())
            } else {
                Some(
                    e.history
                        .get(e.history_index as usize)
                        .cloned()
                        .unwrap_or_default(),
                )
            }
        }
    })
    .flatten();
    if let Some(l) = line {
        set_text(vm, gui, id, &l);
    }
}

/// A character key (`switchD_..._caseD_3`).
fn character(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    ev: GuiEvent,
    len: i32,
    depth: usize,
) -> Result<bool, VmError> {
    let key = u16::from(ev.key_code);
    match key {
        KEY_TAB if flag(vm, id, b"tabComplete") => {
            vm.call_method(id, b"onTabComplete", &[b"0".to_vec()])?;
            return Ok(true);
        }
        KEY_TAB | KEY_ESCAPE | KEY_UP | KEY_DOWN => return parent_key_down(vm, gui, id, ev, depth),
        _ => {}
    }
    let c = ev.ascii as u8;
    // The font says which characters it has; with no font for the profile (no font file and no system font of its face,
    // such as Arial on most Linux systems) a printable byte is typed all the same, so the box still takes what the player types.
    let valid = match font(vm, gui, id) {
        Some(f) => f.is_valid_char(c),
        None => c >= b' ' && c != 0x7f,
    };
    if !valid {
        if flag(vm, id, b"sinkAllKeyEvents") {
            return Ok(true);
        }
        return parent_key_down(vm, gui, id, ev, depth);
    }
    let st = edit(gui, id, |e| e.clone()).unwrap_or_else(EditState::new);
    if profile_flag(vm, id, b"numbersOnly") {
        let t = text(gui, id);
        let refused = if c == b'-' {
            st.cursor != 0 || (st.insert_on && t.first() == Some(&b'-'))
        } else {
            !c.is_ascii_digit()
        };
        if refused {
            denied(vm, gui, id);
            return Ok(true);
        }
    }
    let tge = gui.ext().rule(GuiRule::TgeEditKeys);
    let before = edit_keys::snapshot(gui, id);
    if tge {
        save_undo(gui, id);
    }
    if st.block_end > 0 {
        delete_range(gui, id, st.block_start, st.block_end);
    }
    if ev.action == SI_REPEAT {
        if !edit(gui, id, |e| e.key_seen).unwrap_or(false) {
            if !tge {
                edit_keys::commit(gui, id, before, None);
            }
            return Ok(false);
        }
    } else {
        edit(gui, id, |e| e.key_seen = true);
    }
    let st = edit(gui, id, |e| e.clone()).unwrap_or_else(EditState::new);
    let max = torque_ts_vm::text::atoi(&text_field(vm, id, b"maxLength"));
    // TGE's compares with the length read before the selection went (`len`); the engine's with the text left.
    let len = if tge { len } else { text(gui, id).len() as i32 };
    let measure = if st.insert_on { len } else { st.cursor };
    if measure < max && (text(gui, id).len()) < MAX_EDIT_TEXT {
        let mut t = text(gui, id);
        let cur = (st.cursor.max(0) as usize).min(t.len());
        if cur >= t.len() || !st.insert_on {
            if cur < t.len() {
                t[cur] = c;
            } else {
                t.push(c);
            }
        } else {
            t.insert(cur, c);
        }
        set_raw(gui, id, t);
        edit(gui, id, |e| e.cursor = cur as i32 + 1);
    } else {
        denied(vm, gui, id);
    }
    if !tge {
        edit_keys::commit(gui, id, before, Some(Merge::Typing));
    }
    changed(vm, gui, id)
}

/// `getCursorPos` and `setCursorPos(pos)` (clamped to the text).
pub fn cursor(gui: &Gui, id: ObjId) -> i32 {
    edit(gui, id, |e| e.cursor).unwrap_or(0)
}

pub fn set_cursor(gui: &Gui, id: ObjId, pos: i32) {
    let len = text(gui, id).len() as i32;
    edit(gui, id, |e| e.cursor = pos.clamp(0, len));
}

#[cfg(test)]
#[path = "text_edit_tests.rs"]
mod tests;
