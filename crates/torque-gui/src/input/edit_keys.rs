//! GuiTextEditCtrl's desktop keys and clicks, the engine's default ([`super`]'s documentation; TGE's with
//! [`crate::ext::GuiRule::TgeEditKeys`]).

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use super::{
    EditState, GuiEvent, MAX_EDIT_TEXT, MAX_UNDO_STEPS, changed, character, delete_range, denied,
    edit, font, history_key, input, profile_flag, set_raw, text, text_field,
};
use crate::Gui;
use crate::control::{flag, screen_position};
use crate::keys::*;

/// The key codes of the letters the keys use (`A` is 0x21, the rest in order).
const KEY_C: u16 = KEY_A + 2;
const KEY_V: u16 = KEY_A + 21;
const KEY_X: u16 = KEY_A + 23;
const KEY_Y: u16 = KEY_A + 24;
const KEY_Z: u16 = KEY_A + 25;

/// What a change was, for undo: changes of one kind in a row make one undo step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Merge {
    Typing,
    Backspace,
    Delete,
}

type Snapshot = (Vec<u8>, i32, i32, i32);

/// A byte's kind for the word moves: 0 space, 1 a word's (a letter, a digit, `_`, any byte from 0x80), 2 punctuation.
fn kind(b: u8) -> u8 {
    if b.is_ascii_whitespace() {
        0
    } else if b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80 {
        1
    } else {
        2
    }
}

/// Where Ctrl+Left goes from `at`: back over spaces, then over a run of word bytes or of punctuation.
pub fn word_left(t: &[u8], at: usize) -> usize {
    let mut i = at.min(t.len());
    while i > 0 && kind(t[i - 1]) == 0 {
        i -= 1;
    }
    if i > 0 {
        let k = kind(t[i - 1]);
        while i > 0 && kind(t[i - 1]) == k {
            i -= 1;
        }
    }
    i
}

/// Where Ctrl+Right goes from `at`: on over spaces, then over a run of word bytes or of punctuation.
pub fn word_right(t: &[u8], at: usize) -> usize {
    let mut i = at.min(t.len());
    while i < t.len() && kind(t[i]) == 0 {
        i += 1;
    }
    if i < t.len() {
        let k = kind(t[i]);
        while i < t.len() && kind(t[i]) == k {
            i += 1;
        }
    }
    i
}

/// The run of one kind (a word, punctuation, spaces) holding the byte at `at` (the last byte when past the end).
fn word_at(t: &[u8], at: usize) -> (usize, usize) {
    if t.is_empty() {
        return (0, 0);
    }
    let at = at.min(t.len() - 1);
    let k = kind(t[at]);
    let (mut s, mut e) = (at, at + 1);
    while s > 0 && kind(t[s - 1]) == k {
        s -= 1;
    }
    while e < t.len() && kind(t[e]) == k {
        e += 1;
    }
    (s, e)
}

/// The text, the selection and the cursor.
pub(super) fn snapshot(gui: &Gui, id: ObjId) -> Snapshot {
    let t = text(gui, id);
    edit(gui, id, |e| (t, e.block_start, e.block_end, e.cursor)).unwrap_or_default()
}

/// After a change from `before`: an undo step unless the text is the same or the change continues the last step's kind;
/// the redo steps go.
pub(super) fn commit(gui: &Gui, id: ObjId, before: Snapshot, merge: Option<Merge>) {
    if text(gui, id) == before.0 {
        return;
    }
    edit(gui, id, |e| {
        if merge.is_none() || e.undo_merge != merge {
            push_step(&mut e.undo_steps, before);
        }
        e.redo_steps.clear();
        e.undo_merge = merge;
    });
}

fn push_step(steps: &mut Vec<Snapshot>, s: Snapshot) {
    steps.push(s);
    if steps.len() > MAX_UNDO_STEPS {
        steps.remove(0);
    }
}

/// The end of the selection the cursor is not at (the cursor itself with no selection).
fn anchor(e: &EditState) -> i32 {
    if e.block_end > e.block_start {
        if e.cursor == e.block_start {
            e.block_end
        } else {
            e.block_start
        }
    } else {
        e.cursor
    }
}

fn select_to(e: &mut EditState, to: i32) {
    let a = anchor(e);
    e.cursor = to;
    (e.block_start, e.block_end) = if a == to {
        (0, 0)
    } else {
        (a.min(to), a.max(to))
    };
}

fn move_to(e: &mut EditState, to: i32) {
    e.cursor = to;
    e.block_start = 0;
    e.block_end = 0;
}

/// The keys of the module documentation; None: TGE's handling goes on (Return, Escape, Insert, Tab, characters).
pub(super) fn key_down(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    ev: GuiEvent,
    depth: usize,
) -> Result<Option<bool>, VmError> {
    let key = u16::from(ev.key_code);
    let m = ev.modifier;
    let shift = m & SI_SHIFT != 0;
    let len = text(gui, id).len() as i32;
    if m & (SI_ALT | 0x80) != 0 {
        return character(vm, gui, id, ev, len, depth).map(Some);
    }
    let ctrl = m & SI_CTRL != 0;
    let taken = match key {
        KEY_LEFT | KEY_RIGHT | KEY_HOME | KEY_END => {
            step(vm, gui, id, key, ctrl, shift);
            true
        }
        KEY_BACKSPACE => remove(vm, gui, id, false, ctrl)?,
        KEY_DELETE if shift && !ctrl => copy(vm, gui, id, true)?,
        KEY_DELETE => remove(vm, gui, id, true, ctrl)?,
        KEY_INSERT if shift && !ctrl => paste(vm, gui, id)?,
        KEY_INSERT if ctrl && !shift => copy(vm, gui, id, false)?,
        KEY_A if ctrl && !shift => {
            edit(gui, id, |e| {
                e.undo_merge = None;
                e.cursor = len;
                (e.block_start, e.block_end) = if len > 0 { (0, len) } else { (0, 0) };
            });
            true
        }
        KEY_C if ctrl && !shift => copy(vm, gui, id, false)?,
        KEY_X if ctrl && !shift => copy(vm, gui, id, true)?,
        KEY_V if ctrl && !shift => paste(vm, gui, id)?,
        KEY_Z if ctrl => undo(vm, gui, id, shift)?,
        KEY_Y if ctrl && !shift => undo(vm, gui, id, true)?,
        _ if ctrl => character(vm, gui, id, ev, len, depth)?,
        KEY_UP | KEY_DOWN if !shift => {
            let before = snapshot(gui, id);
            history_key(vm, gui, id, key == KEY_UP);
            commit(gui, id, before, None);
            true
        }
        _ => return Ok(None),
    };
    Ok(Some(taken))
}

/// Left, Right, Home and End: `word` with Ctrl, `select` with Shift.
fn step(vm: &Vm, gui: &Gui, id: ObjId, key: u16, word: bool, select: bool) {
    let t = text(gui, id);
    let len = t.len() as i32;
    let password = flag(vm, id, b"password");
    edit(gui, id, |e| {
        e.undo_merge = None;
        let cur = e.cursor.clamp(0, len);
        let selected = e.block_end > e.block_start;
        let to = match key {
            KEY_HOME => 0,
            KEY_END => len,
            KEY_LEFT if word && password => 0,
            KEY_RIGHT if word && password => len,
            KEY_LEFT if word => word_left(&t, cur as usize) as i32,
            KEY_RIGHT if word => word_right(&t, cur as usize) as i32,
            KEY_LEFT if selected && !select => e.block_start,
            KEY_RIGHT if selected && !select => e.block_end,
            KEY_LEFT => (cur - 1).max(0),
            _ => (cur + 1).min(len),
        };
        if select {
            e.cursor = cur;
            select_to(e, to);
        } else {
            move_to(e, to);
        }
    });
}

/// Backspace (`forward` false) and Delete: the selection, else a character or (`word`) a word.
fn remove(vm: &mut Vm, gui: &Gui, id: ObjId, forward: bool, word: bool) -> Result<bool, VmError> {
    let before = snapshot(gui, id);
    let (t, bs, be, cur) = (&before.0, before.1, before.2, before.3);
    let len = t.len();
    let cur = cur.clamp(0, len as i32) as usize;
    let password = flag(vm, id, b"password");
    let (s, e, merge) = if be > bs {
        (bs.max(0) as usize, (be.max(0) as usize).min(len), None)
    } else if forward {
        let e = match (word, password) {
            (true, true) => len,
            (true, false) => word_right(t, cur),
            _ => (cur + 1).min(len),
        };
        (cur, e, (!word).then_some(Merge::Delete))
    } else {
        let s = match (word, password) {
            (true, true) => 0,
            (true, false) => word_left(t, cur),
            _ => cur.saturating_sub(1),
        };
        (s, cur, (!word).then_some(Merge::Backspace))
    };
    if s >= e {
        return Ok(true);
    }
    delete_range(gui, id, s as i32, e as i32);
    commit(gui, id, before, merge);
    changed(vm, gui, id)
}

/// Copy, and with `cut` the selection removed.
fn copy(vm: &mut Vm, gui: &Gui, id: ObjId, cut: bool) -> Result<bool, VmError> {
    let before = snapshot(gui, id);
    let (t, bs, be) = (&before.0, before.1, before.2);
    if flag(vm, id, b"password") || be <= bs {
        return Ok(true);
    }
    let s = (bs.max(0) as usize).min(t.len());
    let e = (be.max(0) as usize).min(t.len()).max(s);
    input(gui).clipboard = t[s..e].to_vec();
    if let Some(c) = super::super::SYSTEM_CLIPBOARD.get() {
        (c.set)(&super::super::clipboard_text(&t[s..e]));
    }
    if !cut {
        return Ok(true);
    }
    delete_range(gui, id, bs, be);
    commit(gui, id, before, None);
    changed(vm, gui, id)
}

/// The clipboard over the selection, as much as `maxLength` leaves room for; then `onPaste`, the variable and the command.
fn paste(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<bool, VmError> {
    let mut clip = match super::super::SYSTEM_CLIPBOARD.get() {
        Some(c) => (c.get)()
            .map(|t| super::super::clipboard_bytes(&t))
            .unwrap_or_default(),
        None => input(gui).clipboard.clone(),
    };
    let before = snapshot(gui, id);
    let (bs, be) = (before.1, before.2);
    let start = if be > bs { bs } else { before.3 };
    if profile_flag(vm, id, b"numbersOnly") {
        let minus_ok = start == 0 && before.0.get(be.max(0) as usize) != Some(&b'-');
        let lead = usize::from(minus_ok && clip.first() == Some(&b'-'));
        let digits = clip[lead..].iter().copied().filter(u8::is_ascii_digit);
        clip = clip[..lead].iter().copied().chain(digits).collect();
    }
    if clip.is_empty() {
        return Ok(true);
    }
    if be > bs {
        delete_range(gui, id, bs, be);
    }
    let max = torque_ts_vm::text::atoi(&text_field(vm, id, b"maxLength")).max(0) as usize;
    let mut t = text(gui, id);
    let room = max.min(MAX_EDIT_TEXT).saturating_sub(t.len());
    let take = clip.len().min(room);
    let cursor = edit(gui, id, |e| e.cursor)
        .unwrap_or(0)
        .clamp(0, t.len() as i32) as usize;
    t.splice(cursor..cursor, clip[..take].iter().copied());
    set_raw(gui, id, t);
    edit(gui, id, |e| {
        e.cursor = (cursor + take) as i32;
        e.undo_merge = None;
    });
    if take < clip.len() {
        denied(vm, gui, id);
    }
    commit(gui, id, before, None);
    vm.call_method(id, b"onPaste", &[])?;
    changed(vm, gui, id)
}

/// Ctrl+Z (`redo` false), Ctrl+Y and Ctrl+Shift+Z.
fn undo(vm: &mut Vm, gui: &Gui, id: ObjId, redo: bool) -> Result<bool, VmError> {
    let now = snapshot(gui, id);
    let step = edit(gui, id, |e| {
        e.undo_merge = None;
        let (from, to) = if redo {
            (&mut e.redo_steps, &mut e.undo_steps)
        } else {
            (&mut e.undo_steps, &mut e.redo_steps)
        };
        let s = from.pop()?;
        push_step(to, now);
        Some(s)
    })
    .flatten();
    let Some((t, bs, be, cur)) = step else {
        return Ok(true);
    };
    set_raw(gui, id, t);
    edit(gui, id, |e| {
        e.block_start = bs;
        e.block_end = be;
        e.cursor = cur;
    });
    changed(vm, gui, id)
}

/// Where a click at `pt` falls in the text as drawn: the nearest gap between characters and the character under it.
fn hit(vm: &Vm, gui: &Gui, id: ObjId, pt: (i32, i32)) -> (i32, i32) {
    let t = text(gui, id);
    let len = t.len() as i32;
    let start = edit(gui, id, |e| e.text_offset)
        .flatten()
        .unwrap_or_else(|| screen_position(vm, id).0 + 3);
    let x = pt.0.wrapping_sub(start);
    if x < 0 {
        return (0, 0);
    }
    let Some(f) = font(vm, gui, id) else {
        return (len, len);
    };
    let password = flag(vm, id, b"password");
    let mut width = 0i32;
    for (i, &b) in t.iter().enumerate() {
        let c = if password { b'*' } else { b };
        if !password && !f.is_valid_char(c) {
            continue;
        }
        let adv = f.glyph(c).map_or(0, |g| i32::from(g.x_increment));
        if x < width + adv {
            let gap = if (x - width) * 2 >= adv { i + 1 } else { i };
            return (gap as i32, i as i32);
        }
        width += adv;
    }
    (len, len)
}

/// A press of the left button: a click, a double click (the word), a triple click (all) or Shift+click (to the click).
pub(super) fn mouse_down(vm: &Vm, gui: &Gui, id: ObjId, ev: GuiEvent) {
    let (gap, under) = hit(vm, gui, id, ev.point);
    let t = text(gui, id);
    let len = t.len() as i32;
    let password = flag(vm, id, b"password");
    edit(gui, id, |e| {
        e.drag_hit = false;
        e.undo_merge = None;
        e.drag_range = None;
        if ev.modifier & SI_SHIFT != 0 {
            e.drag_start = anchor(e);
            select_to(e, gap);
            return;
        }
        let range = match ev.clicks {
            2 if !password => {
                let (s, en) = word_at(&t, under as usize);
                Some((s as i32, en as i32))
            }
            2.. => Some((0, len)),
            _ => None,
        };
        match range {
            Some((s, en)) if en > s => {
                e.block_start = s;
                e.block_end = en;
                e.cursor = en;
                e.drag_start = s;
                e.drag_range = Some((s, en));
            }
            _ => {
                move_to(e, gap);
                e.drag_start = gap;
            }
        }
    });
}

/// A drag with the left button: from the press (or what a double or triple click selected) to the mouse.
pub(super) fn mouse_dragged(vm: &Vm, gui: &Gui, id: ObjId, ev: GuiEvent) {
    let (p, _) = hit(vm, gui, id, ev.point);
    edit(gui, id, |e| {
        let (s, en) = e.drag_range.unwrap_or((e.drag_start, e.drag_start));
        if p < s {
            (e.block_start, e.block_end, e.cursor) = (p, en, p);
        } else if p > en {
            (e.block_start, e.block_end, e.cursor) = (s, p, p);
        } else if en > s {
            (e.block_start, e.block_end, e.cursor) = (s, en, en);
        } else {
            move_to(e, p);
        }
    });
}
