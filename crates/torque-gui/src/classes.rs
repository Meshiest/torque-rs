//! The GUI classes' own methods and values (`getValue`/`setValue` per class, text, bitmaps, buttons, lists, popups), as the class
//! table lists them and TGE 1.4 implements them. Drawing and input are later steps; what they would change (a list's scroll,
//! a bitmap's offset) is accepted and kept where the tree dump shows it.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/controls/guiTextCtrl.cpp` (Copyright
//! (c) 2012 GarageGames, LLC), `gui/controls/guiBitmapCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC),
//! `gui/buttons/guiBitmapButtonCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC), `gui/buttons/guiButtonBaseCtrl.cpp` (Copyright
//! (c) 2012 GarageGames, LLC), `gui/controls/guiTextListCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC),
//! `gui/controls/guiPopUpCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC), `gui/containers/guiScrollCtrl.cpp` (Copyright (c)
//! 2012 GarageGames, LLC), `gui/controls/guiSliderCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC),
//! `gui/game/guiProgressCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC), `gui/game/guiFadeinBitmapCtrl.cpp` (Copyright (c) 2012
//! GarageGames, LLC), `gui/controls/guiMLTextCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC), `gui/core/guiTypes.cpp`
//! (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Native, NativeCall, Vm, VmError};

use crate::control::{flag, is_a};
use crate::{Gui, MAX_LIST_ROWS, MAX_TEXT_BYTES, arg, atob, atof, atoi, done, this};

/// TGE's `GuiTextCtrl::MAX_STRING_LENGTH`: `mText` keeps 255 bytes.
pub const TEXT_CTRL_MAX: usize = 255;

/// Whether the control keeps an `mText` apart from its `text` field (GuiTextCtrl and its subclasses, GuiMLTextCtrl).
pub(crate) fn has_text_state(vm: &Vm, id: ObjId) -> bool {
    is_a(vm, id, "GuiTextCtrl") || is_a(vm, id, "GuiMLTextCtrl")
}

/// `mInitialText` copied at `onAdd` (GuiTextCtrl cuts it to [`TEXT_CTRL_MAX`]).
pub(crate) fn initial_text(vm: &Vm, id: ObjId) -> Vec<u8> {
    let mut text = vm.objects().field(id, b"text", b"").unwrap_or_default();
    let cap = if is_a(vm, id, "GuiMLTextCtrl") {
        MAX_TEXT_BYTES
    } else {
        TEXT_CTRL_MAX
    };
    text.truncate(cap);
    text
}

/// The control's `mText` (a copy of that text alone).
fn text_of(gui: &Gui, id: ObjId) -> Vec<u8> {
    gui.with(id, |c| c.text.clone().unwrap_or_default())
        .unwrap_or_default()
}

/// The `variable` field (`mConsoleVariable`), with its `$` if it has one.
fn variable(vm: &Vm, id: ObjId) -> Option<String> {
    let name = vm.objects().field(id, b"variable", b"")?;
    if name.is_empty() {
        return None;
    }
    let name = String::from_utf8_lossy(&name).into_owned();
    Some(if name.starts_with('$') {
        name
    } else {
        format!("${name}")
    })
}

/// Whether the control has a profile (`mProfile`): its `profile` field names a GuiControlProfile.
fn has_profile(vm: &Vm, id: ObjId) -> bool {
    vm.objects()
        .field(id, b"profile", b"")
        .filter(|p| !p.is_empty())
        .and_then(|p| vm.objects().find(&p))
        .is_some_and(|p| is_a(vm, p, "GuiControlProfile"))
}

/// Whether the profile asks for an automatic size (`autoSizeWidth` / `autoSizeHeight`).
fn autosizes(vm: &Vm, id: ObjId) -> bool {
    let Some(profile) = vm
        .objects()
        .field(id, b"profile", b"")
        .filter(|p| !p.is_empty())
        .and_then(|p| vm.objects().find(&p))
    else {
        return false;
    };
    flag(vm, profile, b"autoSizeWidth") || flag(vm, profile, b"autoSizeHeight")
}

/// `GuiTextCtrl::setText` (and the subclasses that keep it): `mText`, the automatic size ([`autosize`]), the console
/// variable.
pub(crate) fn set_text(vm: &mut Vm, gui: &Gui, id: ObjId, text: &[u8]) {
    let ml = is_a(vm, id, "GuiMLTextCtrl");
    if ml {
        crate::mltext::set_text(vm, gui, id, text);
        return;
    }
    // Nothing without `mProfile`: a control made with no profile has none until it wakes (it then gets its class's
    // default), so `setText` / `setValue` on it change nothing.
    if !ml && !has_profile(vm, id) {
        return;
    }
    let keep = text
        .len()
        .min(if ml { MAX_TEXT_BYTES } else { TEXT_CTRL_MAX });
    // The copy (and the console variable's) is paid for; past the budget nothing changes and the run ends at its next instruction.
    if !vm.charge_work(keep as u64) {
        return;
    }
    let text = text[..keep].to_vec();
    if let Some(control) = gui.0.borrow_mut().controls.get_mut(&id) {
        control.text = Some(text.clone());
    }
    if !ml {
        autosize(vm, gui, id);
    }
    if !ml && let Some(var) = variable(vm, id) {
        vm.set_global(&var, &String::from_utf8_lossy(&text));
    }
}

/// GuiTextCtrl's automatic size (`onWake` and `setText`, after the text): with the profile's
/// `autoSizeWidth` the width becomes `getStrNWidth(mText)` of the profile's font and the height `font height + 4` with
/// `autoSizeHeight`, else stays; with `autoSizeHeight` alone the height becomes `font height + 4`; then `resize(position, extent)`
/// (the extent clamped to `minExtent`). Without such a profile nothing changes; without a font for its face and size the size is
/// left and counted ([`crate::GuiState::autosize_skipped`]).
pub(crate) fn autosize(vm: &mut Vm, gui: &Gui, id: ObjId) {
    if !autosizes(vm, id) {
        return;
    }
    let Some(profile) = vm
        .objects()
        .field(id, b"profile", b"")
        .filter(|p| !p.is_empty())
        .and_then(|p| vm.objects().find(&p))
    else {
        return;
    };
    let (by_width, by_height) = (
        flag(vm, profile, b"autoSizeWidth"),
        flag(vm, profile, b"autoSizeHeight"),
    );
    let face = String::from_utf8_lossy(
        &vm.objects()
            .field(profile, b"fontType", b"")
            .unwrap_or_default(),
    )
    .into_owned();
    let size = atoi(
        &vm.objects()
            .field(profile, b"fontSize", b"")
            .unwrap_or_default(),
    );
    let fonts = gui.0.borrow().fonts.0.clone();
    let font = {
        let read = |p: &str| match vm.file_host().read(p.as_bytes()) {
            torque_ts_vm::host::FileRead::Found(b) => Some(b),
            _ => None,
        };
        fonts.borrow_mut().font(&face, size, &read)
    };
    let Some(font) = font else {
        gui.0.borrow_mut().autosize_skipped += 1;
        return;
    };
    let text = gui
        .with(id, |c| c.text.clone())
        .flatten()
        .unwrap_or_default();
    if !vm.charge_work(text.len() as u64) {
        return;
    }
    let (position, extent) = crate::control::bounds(vm, id);
    let line = (font.height as i32).wrapping_add(4);
    let new = if by_width {
        (
            font.str_width(&text),
            if by_height { line } else { extent.1 },
        )
    } else {
        (extent.0, line)
    };
    crate::control::resize(vm, gui, id, position, new);
}

/// How much of a variable's value an edit takes at wake: `mMaxStrLen` (the `maxLength` field, compared unsigned, so a negative
/// one cuts nothing) and never past [`TEXT_CTRL_MAX`].
fn wake_max(vm: &Vm, id: ObjId) -> usize {
    match vm.objects().field(id, b"maxLength", b"") {
        Some(v) if !v.is_empty() => {
            usize::try_from(atoi(&v)).map_or(TEXT_CTRL_MAX, |m| m.min(TEXT_CTRL_MAX))
        }
        _ => TEXT_CTRL_MAX,
    }
}

/// A class's constructor state (beside `GuiControl`'s): a scroll control's sizes, a list's cells.
pub(crate) fn construct(vm: &Vm, id: ObjId, control: &mut crate::Control) {
    if is_a(vm, id, "GuiScrollCtrl") {
        control.scroll = Some(Box::default());
    }
    if is_a(vm, id, "GuiConsole") {
        // GuiConsole's constructor: the extent, the cell and the size (1, 1), (1, 1), (1, 0).
        control.list = Some(crate::textlist::ListState {
            cell: (1, 1),
            ..Default::default()
        });
    } else if is_a(vm, id, "GuiArrayCtrl") {
        control.list = Some(crate::textlist::ListState::default());
    }
    if is_a(vm, id, "GuiMLTextCtrl") {
        control.ml = Some(Box::default());
        // `onAdd`: `setText(text)`, which cuts the text to `maxChars - 1`.
        let m = atoi(&vm.objects().field(id, b"maxChars", b"").unwrap_or_default());
        if m > 0
            && let Some(t) = control.text.as_mut()
        {
            t.truncate((m - 1) as usize);
        }
    }
}

/// A class's own `resize` when it has one: true when it handled the call. GuiScrollCtrl: `GuiControl::
/// resize`, then `computeSizes`. GuiMLTextEditCtrl: the height at least the parent's, then `GuiControl::resize`.
pub(crate) fn resize(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    position: (i32, i32),
    extent: (i32, i32),
    depth: usize,
) -> bool {
    if is_a(vm, id, "GuiScrollCtrl") {
        crate::control::base_resize(vm, gui, id, position, extent, depth);
        crate::scroll::compute_sizes(vm, gui, id);
        return true;
    }
    if is_a(vm, id, "GuiWindowCtrl") {
        crate::window::resize(vm, gui, id, position, extent, depth);
        return true;
    }
    if is_a(vm, id, "GuiMLTextEditCtrl") {
        let mut extent = extent;
        if let Some(p) = crate::control::parent(vm, id) {
            let ph = crate::control::point(vm, p, b"extent").1;
            if extent.1 < ph {
                extent.1 = ph;
            }
        }
        crate::control::base_resize(vm, gui, id, position, extent, depth);
        return true;
    }
    false
}

/// A class's own `parentResized` when it has one: true when it handled the call. (GuiTextEditCtrl's and GuiMLTextCtrl's run
/// GuiControl's and mark their own drawing state, which this crate recomputes when it draws.)
pub(crate) fn parent_resized(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    old: (i32, i32),
    new: (i32, i32),
    depth: usize,
) -> bool {
    if is_a(vm, id, "GuiMLTextCtrl") {
        crate::control::base_parent_resized(vm, gui, id, old, new, depth);
        crate::mltext::set_dirty(gui, id);
        return true;
    }
    if is_a(vm, id, "GuiTextEditCtrl") {
        // GuiControl's, then the text's scroll offset reset (placed afresh at the next draw).
        crate::control::base_parent_resized(vm, gui, id, old, new, depth);
        gui.with_mut(id, |c| {
            if let Some(e) = c.edit.as_mut() {
                e.text_offset = None;
            }
        });
        return true;
    }
    false
}

/// What a class's `onWake` does after `GuiControl::onWake` (whose end runs the script's `onWake`): GuiScrollCtrl's sizes,
/// GuiTextListCtrl's font and `setSize`.
pub(crate) fn after_wake(vm: &mut Vm, gui: &Gui, id: ObjId) {
    if is_a(vm, id, "GuiScrollCtrl") {
        crate::scroll::after_wake(vm, gui, id);
    }
    if is_a(vm, id, "GuiTextListCtrl") {
        crate::textlist::after_wake(vm, gui, id);
    }
    if is_a(vm, id, "GuiMLTextCtrl") {
        crate::mltext::set_dirty(gui, id);
    }
    if is_a(vm, id, "GuiWindowCtrl") {
        crate::window::after_wake(vm, gui, id);
    }
    if is_a(vm, id, "GuiSliderCtrl") {
        crate::slider::after_wake(vm, gui, id);
    }
}

/// What a class does at wake beyond `GuiControl::onWake` (TGE 1.4, before the script `onWake`): a text control reads its console
/// variable, a button its check state.
pub(crate) fn on_wake(vm: &mut Vm, gui: &Gui, id: ObjId) {
    if is_a(vm, id, "GuiFadeinBitmapCtrl") {
        reset_fade(vm, gui, id);
    }
    if is_a(vm, id, "GuiTextCtrl") && !is_a(vm, id, "GuiTextEditCtrl") {
        if let Some(var) = variable(vm, id) {
            let value = vm.global(&var).unwrap_or_default();
            set_text(vm, gui, id, &value);
        } else {
            autosize(vm, gui, id);
        }
    } else if is_a(vm, id, "GuiTextEditCtrl") {
        // GuiTextCtrl's onWake in full: the variable's value (cut to `mMaxStrLen`, the
        // `maxLength` field) through the edit's setText, then the profile's automatic size, as for a text control.
        if let Some(var) = variable(vm, id) {
            let mut value = vm.global(&var).unwrap_or_default();
            value.truncate(wake_max(vm, id));
            let len = value.len() as i32;
            if let Some(control) = gui.0.borrow_mut().controls.get_mut(&id) {
                control.text = Some(value);
                if let Some(e) = control.edit.as_mut() {
                    e.cursor = len;
                }
            }
        }
        autosize(vm, gui, id);
    } else if is_a(vm, id, "GuiButtonBaseCtrl")
        && let Some(var) = variable(vm, id)
    {
        let on = atob(&vm.global(&var).unwrap_or_default());
        if let Some(control) = gui.0.borrow_mut().controls.get_mut(&id) {
            control.state_on = on;
        }
    }
}

/// GuiFadeinBitmapCtrl's `onWake` (after the parent's) and `reset`: the clock starts now, `onWait` and
/// `onDone` may run again.
fn reset_fade(vm: &Vm, gui: &Gui, id: ObjId) {
    let now = vm.sim_time();
    gui.with_mut(id, |c| {
        c.fade = crate::Fade {
            wake: now,
            waited: false,
            done: false,
        };
    });
}

/// What a GuiFadeinBitmapCtrl's `onRender` calls at the tick count `now`: with `elapsed = now - wake` (unsigned, as
/// the engine's `GetTickCount` arithmetic), in `[fadein, fadein + wait)` `onWait` once, at or past `fadein + wait + fadeout`
/// `onDone` once (a frame that skips the wait's window never calls `onWait`). The times are the fields (`S32`, compared as
/// unsigned). Marks what it returns as called.
pub(crate) fn fade_callback(vm: &Vm, gui: &Gui, id: ObjId, now: u32) -> Option<&'static [u8]> {
    let time = |name: &[u8]| atoi(&vm.objects().field(id, name, b"").unwrap_or_default()) as u32;
    let (fadein, wait, fadeout) = (time(b"fadeinTime"), time(b"waitTime"), time(b"fadeoutTime"));
    gui.with_mut(id, |c| {
        let elapsed = now.wrapping_sub(c.fade.wake);
        if elapsed < fadein {
            None
        } else if elapsed < wait.wrapping_add(fadein) {
            (!std::mem::replace(&mut c.fade.waited, true)).then_some(&b"onWait"[..])
        } else if elapsed < fadeout.wrapping_add(wait).wrapping_add(fadein) {
            None
        } else {
            (!std::mem::replace(&mut c.fade.done, true)).then_some(&b"onDone"[..])
        }
    })
    .flatten()
}

/// `getScriptValue` by class. Copies only the value itself (the selected row's text, never the list).
pub(crate) fn get_value(vm: &Vm, gui: &Gui, id: ObjId) -> Vec<u8> {
    if is_a(vm, id, "GuiTextListCtrl") {
        return gui
            .with(id, |c| {
                usize::try_from(c.selected)
                    .ok()
                    .and_then(|r| c.rows.get(r))
                    .map(|r| r.1.clone())
            })
            .flatten()
            .unwrap_or_default();
    }
    if is_a(vm, id, "GuiButtonBaseCtrl") {
        return if gui.with(id, |c| c.state_on).unwrap_or(false) {
            b"1".to_vec()
        } else {
            b"0".to_vec()
        };
    }
    if is_a(vm, id, "GuiSliderCtrl") {
        return vm.objects().field(id, b"value", b"").unwrap_or_default();
    }
    if is_a(vm, id, "GuiProgressCtrl") {
        // GuiProgressCtrl's `getScriptValue` prints `%f` (`0.250000`).
        let progress = gui.with(id, |c| c.progress).unwrap_or(0.0);
        return torque_ts_vm::text::format_fixed(f64::from(progress), 6).into_bytes();
    }
    if has_text_state(vm, id) {
        return text_of(gui, id);
    }
    if is_a(vm, id, "GuiConsoleTextCtrl") {
        return vm
            .objects()
            .field(id, b"expression", b"")
            .unwrap_or_default();
    }
    Vec::new()
}

/// `setScriptValue` by class.
pub(crate) fn set_value(vm: &mut Vm, gui: &Gui, id: ObjId, value: &[u8]) {
    if is_a(vm, id, "GuiTextListCtrl") {
        // A scan by text, charged; past the budget the selection stays as it was (the run ends at its next instruction).
        // The row whose text equals the value (`_stricmp`) selected, with `onCellSelected`.
        if let Ok(found) = scan(vm, gui, id, |rows| {
            rows.iter().position(|r| r.1.eq_ignore_ascii_case(value))
        }) {
            let _ = crate::textlist::set_selected(
                vm,
                gui,
                id,
                found.flatten().map_or(-1, |r| r as i32),
                true,
            );
        }
    } else if is_a(vm, id, "GuiButtonBaseCtrl") {
        let on = atob(value);
        if let Some(control) = gui.0.borrow_mut().controls.get_mut(&id) {
            control.state_on = on;
        }
        if let Some(var) = variable(vm, id) {
            vm.set_global(&var, if on { "1" } else { "0" });
        }
    } else if is_a(vm, id, "GuiSliderCtrl") {
        // `atof`, then `updateThumb(value, true)` (clamped; the variable set; no command).
        let _ = crate::slider::update_thumb(vm, gui, id, atof(value) as f32, true);
    } else if is_a(vm, id, "GuiConsoleTextCtrl") {
        // `setScriptValue` is `setText`: the expression the control shows.
        vm.set_object_field(id, b"expression", b"", value);
    } else if is_a(vm, id, "GuiProgressCtrl") {
        // Clamped to 0..1 (NaN and -0 kept), or the extension's rule.
        let v = atof(value) as f32;
        let v = if gui.ext().rule(crate::ext::GuiRule::ProgressNanIsFull) {
            if v <= 1.0 {
                if v <= 0.0 { 0.0 } else { v }
            } else {
                1.0
            }
        } else {
            v.clamp(0.0, 1.0)
        };
        if let Some(control) = gui.0.borrow_mut().controls.get_mut(&id) {
            control.progress = v;
        }
    } else if has_text_state(vm, id) {
        if is_a(vm, id, "GuiTextEditCtrl") {
            // `GuiTextCtrl::setText` (255 bytes, the variable), the cursor at the end.
            crate::input::text_edit::set_text(vm, gui, id, value);
        } else {
            set_text(vm, gui, id, value);
        }
    }
}

pub fn color_f_text(vm: &Vm, id: ObjId, field: &[u8]) -> String {
    let text = vm.objects().field(id, field, b"").unwrap_or_default();
    let words: Vec<&[u8]> = text
        .split(|b| *b == b' ' || *b == b'\t')
        .filter(|w| !w.is_empty())
        .collect();
    (0..4)
        .map(|i| {
            let byte = match words.get(i) {
                Some(w) => atoi(w) as u8,
                None if i == 3 => 255,
                None => 0,
            };
            torque_ts_vm::text::format_fixed(f64::from(f32::from(byte) / 255.0), 6)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `sscanf(s, "%d %d %d %d")`: the ints it converts in order (it stops at the first that does not convert), the others 0.
pub fn scan_ints4(s: &[u8]) -> [i32; 4] {
    let mut out = [0i32; 4];
    let mut at = 0usize;
    for slot in out.iter_mut() {
        while s
            .get(at)
            .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
        {
            at += 1;
        }
        let start = at;
        if matches!(s.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        let digits = s[at.min(s.len())..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digits == 0 {
            break;
        }
        at += digits;
        *slot = atoi(&s[start..at]);
    }
    out
}

/// ColorF to ColorI: each channel `x * 255 + 0.5`, truncated.
pub fn color_f_to_i(f: [f32; 4]) -> [u8; 4] {
    f.map(|x| (x * 255.0 + 0.5) as i32 as u8)
}

/// A colour as a ColorI field holds it (`%d %d %d %d`).
pub fn color_i_text(c: [u8; 4]) -> String {
    format!("{} {} {} {}", c[0], c[1], c[2], c[3])
}

/// GuiBitmapCtrl's and GuiBitmapButtonCtrl's `setColor`: `%f %f %f %f` (a missing channel 0, the alpha too),
/// each clamped to 0..1, as a ColorI.
fn float_color(s: &[u8]) -> [u8; 4] {
    let mut f = [0f32; 4];
    torque_ts_vm::text::scan_floats(s, &mut f);
    color_f_to_i(f.map(|x| {
        if x > 1.0 {
            1.0
        } else if x > 0.0 {
            x
        } else {
            0.0
        }
    }))
}

/// C `%g` as the engine prints it (`torque_ts_vm::text::format_g`).
pub(crate) fn format_g(v: f64) -> String {
    torque_ts_vm::text::format_g(v)
}

/// `performClick`: the button's `onAction` ([`crate::input::on_action`]: inactive or disabled does nothing; a check box toggles and
/// sets its variable, a radio turns on and its siblings of the group off; then the command with `$ThisControl`, or the script
/// `onAction` when there is none).
fn perform_click(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    crate::input::on_action(vm, gui, id)
}

/// `GuiPopUpMenuCtrl::setSelected(id)`: the entry's text and `onSelect(id, text)`, then the control's `command`
/// (compiled and run when it is not empty); for an id with no entry the text empties, the selection is -1, `onCancel` and the
/// command run. TGE runs the command only in the second case; a host's [`crate::ext::GuiRule::PopupSelectRunsCommand`] runs
/// it in both.
fn popup_select(vm: &mut Vm, gui: &Gui, id: ObjId, entry: i32) -> Result<(), VmError> {
    let found = scan(vm, gui, id, |rows| {
        rows.iter()
            .position(|r| r.0 == entry)
            .map(|i| (i, rows[i].1.clone()))
    })?
    .flatten();
    match found {
        Some((index, text)) => {
            if let Some(control) = gui.0.borrow_mut().controls.get_mut(&id) {
                control.selected = index as i32;
            }
            set_text(vm, gui, id, &text);
            vm.call_method(id, b"onSelect", &[entry.to_string().into_bytes(), text])?;
            if gui.ext().rule(crate::ext::GuiRule::PopupSelectRunsCommand)
                && vm.objects().contains(id)
            {
                let command = vm.objects().field(id, b"command", b"").unwrap_or_default();
                if !command.is_empty() {
                    vm.eval(&command)?;
                }
            }
        }
        None => {
            set_text(vm, gui, id, b"");
            if let Some(control) = gui.0.borrow_mut().controls.get_mut(&id) {
                control.selected = -1;
            }
            vm.call_method(id, b"onCancel", &[])?;
            let command = vm.objects().field(id, b"command", b"").unwrap_or_default();
            if !command.is_empty() {
                vm.eval(&command)?;
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn reg(
    vm: &mut Vm,
    gui: &Gui,
    class: &str,
    name: &str,
    min: i32,
    max: i32,
    usage: &str,
    f: impl Fn(&mut Vm, &Gui, ObjId, &NativeCall) -> Native + 'static,
) {
    let gui = gui.clone();
    vm.register_in(class, name, min, max, usage, move |vm, c| {
        match this(vm, c) {
            Some(id) if gui.is_control(id) => f(vm, &gui, id, c),
            _ => Native::Void,
        }
    });
}

/// Sets field `field` from argument 2 (`setBitmap`, `setColor`).
fn set_field_from(vm: &mut Vm, id: ObjId, field: &[u8], c: &NativeCall) -> Native {
    vm.set_object_field(id, field, b"", arg(c, 2));
    Native::Void
}

fn field(vm: &Vm, id: ObjId, name: &[u8]) -> Native {
    Native::Str(vm.objects().field(id, name, b"").unwrap_or_default())
}

/// What reading one row of a list costs (`Vm::charge_work` units: a step for every four rows), beside the bytes of any text copied.
const ROW_WORK: u64 = 16;

/// The work of visiting `rows` rows.
fn rows_work(rows: usize) -> u64 {
    (rows as u64).saturating_mul(ROW_WORK)
}

/// A list row: `(id, text, active)`.
type Row = (i32, Vec<u8>, bool);

/// `f` over the control's rows under a short borrow, every row's visit charged to the step budget first (a scan of a list a script
/// made as long as it liked is paid for by that script). `Ok(None)` for an unknown control; `Err(StepLimit)` when the budget cannot
/// pay, before any row is read.
fn scan<R>(
    vm: &Vm,
    gui: &Gui,
    id: ObjId,
    f: impl FnOnce(&[Row]) -> R,
) -> Result<Option<R>, VmError> {
    let len = gui.with(id, |c| c.rows.len()).unwrap_or(0);
    if !vm.charge_work(rows_work(len)) {
        return Err(VmError::StepLimit);
    }
    Ok(gui.with(id, |c| f(&c.rows)))
}

/// [`scan`] with the rows mutable.
fn scan_mut<R>(
    vm: &Vm,
    gui: &Gui,
    id: ObjId,
    f: impl FnOnce(&mut [Row]) -> R,
) -> Result<Option<R>, VmError> {
    let len = gui.with(id, |c| c.rows.len()).unwrap_or(0);
    if !vm.charge_work(rows_work(len)) {
        return Err(VmError::StepLimit);
    }
    Ok(gui.with_mut(id, |c| f(&mut c.rows)))
}

/// A text a method returns, its copy charged by its length ("" for none).
fn text_result(vm: &Vm, found: Result<Option<Option<Vec<u8>>>, VmError>) -> Native {
    match found {
        Ok(text) => {
            let text = text.flatten().unwrap_or_default();
            if vm.charge_work(text.len() as u64) {
                Native::Str(text)
            } else {
                Native::Fail(VmError::StepLimit)
            }
        }
        Err(e) => Native::Fail(e),
    }
}

/// An integer a scan found (`missing` for an unknown control).
fn int_result(found: Result<Option<i32>, VmError>, missing: i32) -> Native {
    match found {
        Ok(v) => Native::Int(v.unwrap_or(missing)),
        Err(e) => Native::Fail(e),
    }
}

/// `removeEntryByIndex`: row `at` taken out, `setSize(1, rows)`, the selection cleared (no callback).
fn remove_row(vm: &mut Vm, gui: &Gui, id: ObjId, at: usize) {
    let removed = gui
        .with_mut(id, |control| {
            if at < control.rows.len() {
                control.rows.remove(at);
                true
            } else {
                false
            }
        })
        .unwrap_or(false);
    if removed {
        crate::textlist::rows_changed(vm, gui, id);
        gui.with_mut(id, |control| {
            control.selected = -1;
            if let Some(l) = control.list.as_mut() {
                l.selected_x = -1;
            }
        });
    }
}

/// A text list row's text from its `col`-th column on, as the sort comparators find it (past `col` tabs with
/// `strchr`, the rest of the row; a row with fewer tabs, or a negative column, gives "").
fn column_rest(text: &[u8], col: i32) -> &[u8] {
    if col < 0 {
        return b"";
    }
    let mut rest = text;
    for _ in 0..col {
        match rest.iter().position(|b| *b == b'\t') {
            Some(at) => rest = &rest[at + 1..],
            None => return b"",
        }
    }
    rest
}

/// `_stricmp` of two texts, clamped to -1, 0, 1 as the comparator clamps it (the bytes lower-cased as the C locale does).
fn stricmp(a: &[u8], b: &[u8]) -> i32 {
    let a = torque_ts_vm::text::until_nul(a);
    let b = torque_ts_vm::text::until_nul(b);
    for i in 0.. {
        let x = a.get(i).map_or(0, u8::to_ascii_lowercase);
        let y = b.get(i).map_or(0, u8::to_ascii_lowercase);
        if x != y {
            return if x < y { -1 } else { 1 };
        }
        if x == 0 {
            return 0;
        }
    }
    0
}

/// The numeric comparator's value of a column: `atof` as a float, and 999999 for a 0 whose text does not start with `0`.
fn numeric_key(text: &[u8]) -> f32 {
    let v = atof(text) as f32;
    if v == 0.0 && text.first() != Some(&b'0') {
        f32::from_bits(0x4974_23f0)
    } else {
        v
    }
}

/// `sort` / `sortNumerical(col, increasing)`: the rows through `qsort` ([`torque_ts_vm::text::sort_by`]) with the column's
/// comparator, negated when not increasing; the keys' bytes and `n log n` comparisons are charged first. The selected row
/// stays selected. No script runs.
fn sort_rows(
    vm: &Vm,
    gui: &Gui,
    id: ObjId,
    col: i32,
    numeric: bool,
    increasing: bool,
) -> Result<(), VmError> {
    let (len, bytes) = gui
        .with(id, |c| {
            (
                c.rows.len(),
                c.rows.iter().map(|r| r.1.len() as u64).sum::<u64>(),
            )
        })
        .unwrap_or((0, 0));
    let log = u64::from(usize::BITS - len.leading_zeros()).max(1);
    if !vm.charge_work(rows_work(len).saturating_mul(log).saturating_add(bytes)) {
        return Err(VmError::StepLimit);
    }
    gui.with_mut(id, |control| {
        let selected = usize::try_from(control.selected)
            .ok()
            .and_then(|r| control.rows.get(r))
            .map(|r| r.0);
        let mut rows = std::mem::take(&mut control.rows);
        torque_ts_vm::text::sort_by(&mut rows, |a, b| {
            let (x, y) = (column_rest(&a.1, col), column_rest(&b.1, col));
            let r = if numeric {
                let d = numeric_key(x) - numeric_key(y);
                if 0.0 <= d { i32::from(0.0 < d) } else { -1 }
            } else {
                stricmp(x, y)
            };
            if increasing { r } else { -r }
        });
        control.rows = rows;
        control.selected = selected
            .and_then(|s| control.rows.iter().position(|r| r.0 == s))
            .map_or(-1, |i| i as i32);
    });
    Ok(())
}

pub(crate) fn register(vm: &mut Vm, gui: &Gui) {
    register_profile(vm, gui);
    crate::hud::register(vm, gui);
    crate::msgvector::register(vm, gui);
    // Text controls.
    reg(
        vm,
        gui,
        "GuiTextCtrl",
        "setText",
        3,
        3,
        "(string text)",
        // The virtual `setText`: a text edit's puts the cursor at the end.
        // The script method clears the width the last render kept: `getPixelWidth` is 0 until the next frame draws it.
        |vm, gui, id, c| {
            if is_a(vm, id, "GuiTextEditCtrl") {
                crate::input::text_edit::set_text(vm, gui, id, arg(c, 2));
            } else {
                set_text(vm, gui, id, arg(c, 2));
            }
            gui.with_mut(id, |c| c.pixel_width = 0);
            Native::Void
        },
    );
    // The width of the text as the control's last render drew it (see `Control::pixel_width`): 0 before the first frame
    // after a `setText`, so a script that places a control after the text waits for the frame.
    reg(
        vm,
        gui,
        "GuiTextCtrl",
        "getPixelWidth",
        2,
        2,
        "",
        |_, gui, id, _| Native::Int(gui.with(id, |c| c.pixel_width).unwrap_or(0)),
    );
    reg(
        vm,
        gui,
        "GuiMLTextCtrl",
        "setText",
        3,
        3,
        "(string text)",
        |vm, gui, id, c| {
            set_text(vm, gui, id, arg(c, 2));
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        "GuiMLTextCtrl",
        "getText",
        2,
        2,
        "",
        |vm, gui, id, _| text_result(vm, Ok(Some(Some(text_of(gui, id))))),
    );
    reg(
        vm,
        gui,
        "GuiMLTextCtrl",
        "addText",
        4,
        4,
        "(string text, bool reformat)",
        // The text so far and the new piece are copied: both paid for.
        |vm, gui, id, c| {
            crate::mltext::add_text(vm, gui, id, arg(c, 2), atob(arg(c, 3)));
            Native::Void
        },
    );
    // TGE 1.4's `setCursorPosition`: the cursor clamped to the buffer; true when it was clamped.
    reg(
        vm,
        gui,
        "GuiMLTextCtrl",
        "setCursorPosition",
        3,
        3,
        "(int newPos)",
        |_, gui, id, c| {
            let len = text_of(gui, id).len() as i32;
            let want = atoi(arg(c, 2));
            let (pos, clamped) = if want < 0 {
                (0, true)
            } else if want >= len {
                (len, true)
            } else {
                (want, false)
            };
            gui.with_mut(id, |c| {
                if let Some(s) = c.ml.as_deref_mut() {
                    s.cursor = pos as u32;
                }
            });
            Native::Bool(clamped)
        },
    );
    reg(
        vm,
        gui,
        "GuiMLTextCtrl",
        "scrollToTag",
        3,
        3,
        "(int tagID)",
        |vm, gui, id, c| {
            let Some(p) = crate::control::parent(vm, id).filter(|p| is_a(vm, *p, "GuiScrollCtrl"))
            else {
                return Native::Void;
            };
            let want = atoi(arg(c, 2)) as u32;
            let y = crate::mltext::state(gui, id)
                .and_then(|s| s.layout.tags.iter().find(|t| t.0 == want).map(|t| t.1));
            match y {
                Some(y) => crate::scroll::scroll_rect_visible(vm, gui, p, [0, y as i32, 1, 1]),
                None => vm.warn(format!(
                    "GuiMLTextCtrl::scrollToTag - tag id {want} not found!"
                )),
            }
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        "GuiMLTextCtrl",
        "scrollToTop",
        2,
        2,
        "()",
        |vm, gui, id, _| {
            if let Some(p) =
                crate::control::parent(vm, id).filter(|p| is_a(vm, *p, "GuiScrollCtrl"))
            {
                crate::scroll::scroll_rect_visible(vm, gui, p, [0, 0, 0, 0]);
            }
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        "GuiMLTextCtrl",
        "forceReflow",
        2,
        2,
        "()",
        |vm, gui, id, _| {
            if !gui.is_awake(id) {
                // The message between two empty lines, with a BackTrace before the second (`backtrace();` run).
                return match vm.error_framed(
                    b"GuiMLTextCtrl::forceReflow can only be called on visible controls.",
                ) {
                    Ok(()) => Native::Void,
                    Err(e) => Native::Fail(e),
                };
            }
            done(crate::mltext::reflow(vm, gui, id))
        },
    );
    reg(
        vm,
        gui,
        "GuiMLTextCtrl",
        "setAlpha",
        3,
        3,
        "",
        |_, gui, id, c| {
            let a = atof(arg(c, 2)) as f32;
            gui.with_mut(id, |c| {
                if let Some(s) = c.ml.as_deref_mut() {
                    s.alpha = a;
                }
            });
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        "GuiTextEditCtrl",
        "getCursorPos",
        2,
        2,
        "",
        |_, gui, id, _| Native::Int(crate::input::text_edit::cursor(gui, id)),
    );
    reg(
        vm,
        gui,
        "GuiTextEditCtrl",
        "setCursorPos",
        3,
        3,
        "",
        |_, gui, id, c| {
            crate::input::text_edit::set_cursor(gui, id, atoi(arg(c, 2)));
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        "GuiTextEditCtrl",
        "updateHistorySize",
        2,
        2,
        "",
        |_, _, _, _| Native::Void,
    );

    // Buttons.
    reg(
        vm,
        gui,
        "GuiButtonBaseCtrl",
        "setText",
        3,
        3,
        "(string text)",
        |vm, _, id, c| set_field_from(vm, id, b"text", c),
    );
    reg(
        vm,
        gui,
        "GuiButtonBaseCtrl",
        "getText",
        2,
        2,
        "",
        |vm, _, id, _| field(vm, id, b"text"),
    );
    reg(
        vm,
        gui,
        "GuiButtonBaseCtrl",
        "performClick",
        2,
        2,
        "",
        |vm, gui, id, _| done(perform_click(vm, gui, id)),
    );

    // Bitmaps and colours: the fields (drawing comes later).
    // A subclass that loads frames of the name itself registers its own `setBitmap` over this one.
    for class in [
        "GuiBitmapCtrl",
        "GuiBitmapButtonCtrl",
        "GuiChunkedBitmapCtrl",
    ] {
        reg(
            vm,
            gui,
            class,
            "setBitmap",
            3,
            3,
            "(string filename)",
            |vm, _, id, c| set_field_from(vm, id, b"bitmap", c),
        );
    }
    for class in ["GuiBitmapCtrl", "GuiBitmapButtonCtrl"] {
        reg(
            vm,
            gui,
            class,
            "setColor",
            3,
            3,
            "(color)",
            |vm, _, id, c| {
                let text = color_i_text(float_color(arg(c, 2)));
                vm.set_object_field(id, b"mColor", b"", text.as_bytes());
                Native::Void
            },
        );
        reg(vm, gui, class, "getColor", 2, 2, "()", |vm, _, id, _| {
            Native::Str(color_f_text(vm, id, b"mColor").into_bytes())
        });
    }
    // A bitmap's start offset (`setValue(x, y)`): drawing only.
    reg(
        vm,
        gui,
        "GuiBitmapCtrl",
        "setValue",
        4,
        4,
        "(int xAxis, int yAxis)",
        |_, _, _, _| Native::Void,
    );
    reg(
        vm,
        gui,
        "GuiFadeinBitmapCtrl",
        "reset",
        2,
        2,
        "",
        |vm, gui, id, _| {
            reset_fade(vm, gui, id);
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        "GuiSliderCtrl",
        "getValue",
        2,
        2,
        "()",
        |vm, _, id, _| {
            Native::Float(atof(
                &vm.objects().field(id, b"value", b"").unwrap_or_default(),
            ))
        },
    );
    // `scrollTo(0, 0)` and `scrollTo(0, 2^31 - 1)`.
    reg(
        vm,
        gui,
        "GuiScrollCtrl",
        "scrollToTop",
        2,
        2,
        "()",
        |vm, gui, id, _| {
            crate::scroll::scroll_to(vm, gui, id, 0, 0);
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        "GuiScrollCtrl",
        "scrollToBottom",
        2,
        2,
        "()",
        |vm, gui, id, _| {
            crate::scroll::scroll_to(vm, gui, id, 0, i32::MAX);
            Native::Void
        },
    );

    register_text_list(vm, gui);
    register_popup(vm, gui);
}

fn register_text_list(vm: &mut Vm, gui: &Gui) {
    const C: &str = "GuiTextListCtrl";
    reg(
        vm,
        gui,
        C,
        "addRow",
        4,
        5,
        "(int id, string text, int index=0)",
        |vm, gui, id, c| {
            let text = arg(c, 3);
            if !vm.charge_work(ROW_WORK.saturating_add(text.len() as u64)) {
                return Native::Fail(VmError::StepLimit);
            }
            let row = (atoi(arg(c, 2)), text.to_vec(), true);
            let at = (c.argv.len() >= 5).then(|| usize::try_from(atoi(arg(c, 4))).unwrap_or(0));
            let len = gui.with(id, |c| c.rows.len()).unwrap_or(0);
            // An insert moves the rows after it: paid for like a scan.
            if at.is_some() && !vm.charge_work(rows_work(len)) {
                return Native::Fail(VmError::StepLimit);
            }
            let added = gui.with_mut(id, |control| {
                let len = control.rows.len();
                if len >= MAX_LIST_ROWS {
                    return len as i32;
                }
                // `insertEntry` clamps the index to the rows and leaves the selection where it was.
                match at {
                    Some(index) => control.rows.insert(index.min(len), row),
                    None => control.rows.push(row),
                }
                len as i32
            });
            crate::textlist::rows_changed(vm, gui, id);
            Native::Int(added.unwrap_or(-1))
        },
    );
    reg(
        vm,
        gui,
        C,
        "setRowById",
        4,
        4,
        "(int id, string text)",
        |vm, gui, id, c| {
            let row_id = atoi(arg(c, 2));
            let text = arg(c, 3);
            if !vm.charge_work(text.len() as u64) {
                return Native::Fail(VmError::StepLimit);
            }
            // The row's text replaced, else a new row (`addEntry`); either way `setSize(1, rows)`.
            let found = match scan_mut(vm, gui, id, |rows| {
                rows.iter_mut()
                    .find(|r| r.0 == row_id)
                    .map(|row| row.1 = text.to_vec())
                    .is_some()
            }) {
                Ok(f) => f.unwrap_or(false),
                Err(e) => return Native::Fail(e),
            };
            if !found {
                gui.with_mut(id, |control| {
                    if control.rows.len() < MAX_LIST_ROWS {
                        control.rows.push((row_id, text.to_vec(), true));
                    }
                });
            }
            crate::textlist::rows_changed(vm, gui, id);
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        C,
        "clear",
        2,
        2,
        "()",
        // The rows removed one by one (each `setSize`), so an empty list is not resized; the selection and the mouse-over
        // cell cleared.
        |vm, gui, id, _| {
            let had = gui
                .with_mut(id, |control| {
                    let had = !control.rows.is_empty();
                    control.rows.clear();
                    control.selected = -1;
                    if let Some(l) = control.list.as_mut() {
                        l.selected_x = -1;
                        l.mouse_over = (-1, -1);
                    }
                    had
                })
                .unwrap_or(false);
            if had {
                crate::textlist::rows_changed(vm, gui, id);
            }
            Native::Void
        },
    );
    reg(vm, gui, C, "rowCount", 2, 2, "()", |_, gui, id, _| {
        Native::Int(gui.with(id, |c| c.rows.len() as i32).unwrap_or(0))
    });
    reg(
        vm,
        gui,
        C,
        "getRowId",
        3,
        3,
        "(int index)",
        |_, gui, id, c| {
            let index = usize::try_from(atoi(arg(c, 2))).ok();
            Native::Int(
                gui.with(id, |c| {
                    index.and_then(|i| c.rows.get(i)).map_or(-1, |r| r.0)
                })
                .unwrap_or(-1),
            )
        },
    );
    reg(
        vm,
        gui,
        C,
        "getRowTextById",
        3,
        3,
        "(int id)",
        |vm, gui, id, c| {
            let row_id = atoi(arg(c, 2));
            text_result(
                vm,
                scan(vm, gui, id, |rows| {
                    rows.iter().find(|r| r.0 == row_id).map(|r| r.1.clone())
                }),
            )
        },
    );
    reg(
        vm,
        gui,
        C,
        "getRowNumById",
        3,
        3,
        "(int id)",
        |vm, gui, id, c| {
            let row_id = atoi(arg(c, 2));
            int_result(
                scan(vm, gui, id, |rows| {
                    rows.iter()
                        .position(|r| r.0 == row_id)
                        .map_or(-1, |i| i as i32)
                }),
                -1,
            )
        },
    );
    reg(
        vm,
        gui,
        C,
        "getRowText",
        3,
        3,
        "(int index)",
        |vm, gui, id, c| {
            let index = usize::try_from(atoi(arg(c, 2))).ok();
            let text = gui.with(id, |c| {
                index.and_then(|i| c.rows.get(i)).map(|r| r.1.clone())
            });
            text_result(vm, Ok(text))
        },
    );
    reg(
        vm,
        gui,
        C,
        "removeRowById",
        3,
        3,
        "(int id)",
        |vm, gui, id, c| {
            let row_id = atoi(arg(c, 2));
            match scan(vm, gui, id, |rows| rows.iter().position(|r| r.0 == row_id)) {
                Ok(Some(Some(at))) => remove_row(vm, gui, id, at),
                Ok(_) => {}
                Err(e) => return Native::Fail(e),
            }
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        C,
        "removeRow",
        3,
        3,
        "(int index)",
        |vm, gui, id, c| {
            let Ok(at) = usize::try_from(atoi(arg(c, 2))) else {
                return Native::Void;
            };
            let len = gui.with(id, |c| c.rows.len()).unwrap_or(0);
            if at < len {
                // The rows after it move up: paid for like a scan.
                if !vm.charge_work(rows_work(len - at)) {
                    return Native::Fail(VmError::StepLimit);
                }
                remove_row(vm, gui, id, at);
            }
            Native::Void
        },
    );
    reg(vm, gui, C, "getSelectedId", 2, 2, "()", |_, gui, id, _| {
        let selected = gui.with(id, |c| {
            usize::try_from(c.selected)
                .ok()
                .and_then(|r| c.rows.get(r))
                .map_or(-1, |r| r.0)
        });
        Native::Int(selected.unwrap_or(-1))
    });
    reg(
        vm,
        gui,
        C,
        "setSelectedById",
        3,
        3,
        "(int id)",
        |vm, gui, id, c| {
            let row_id = atoi(arg(c, 2));
            match scan(vm, gui, id, |rows| rows.iter().position(|r| r.0 == row_id)) {
                Ok(Some(Some(r))) => {
                    done(crate::textlist::set_selected(vm, gui, id, r as i32, true))
                }
                Ok(_) => Native::Void,
                Err(e) => Native::Fail(e),
            }
        },
    );
    reg(
        vm,
        gui,
        C,
        "setSelectedRow",
        3,
        3,
        "(int rowNum)",
        |vm, gui, id, c| {
            done(crate::textlist::set_selected(
                vm,
                gui,
                id,
                atoi(arg(c, 2)),
                true,
            ))
        },
    );
    reg(
        vm,
        gui,
        C,
        "clearSelection",
        2,
        2,
        "()",
        // The selection -1, no callback.
        |vm, gui, id, _| done(crate::textlist::set_selected(vm, gui, id, -1, false)),
    );
    reg(
        vm,
        gui,
        C,
        "findTextIndex",
        3,
        3,
        "(string needle)",
        |vm, gui, id, c| {
            let needle = arg(c, 2);
            int_result(
                scan(vm, gui, id, |rows| {
                    rows.iter()
                        .position(|r| r.1.eq_ignore_ascii_case(needle))
                        .map_or(-1, |i| i as i32)
                }),
                -1,
            )
        },
    );
    reg(
        vm,
        gui,
        C,
        "setRowActive",
        4,
        4,
        "(int rowNum, bool active)",
        // The row by id; making the selected row inactive clears the selection.
        |vm, gui, id, c| {
            let row_id = atoi(arg(c, 2));
            let active = atob(arg(c, 3));
            if !vm.charge_work(rows_work(gui.with(id, |c| c.rows.len()).unwrap_or(0))) {
                return Native::Fail(VmError::StepLimit);
            }
            gui.with_mut(id, |control| {
                if let Some(at) = control.rows.iter().position(|r| r.0 == row_id)
                    && control.rows[at].2 != active
                {
                    control.rows[at].2 = active;
                    if !active && control.selected == at as i32 {
                        control.selected = -1;
                        if let Some(l) = control.list.as_mut() {
                            l.selected_x = -1;
                        }
                    }
                }
            });
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        C,
        "isRowActive",
        3,
        3,
        "(int rowNum)",
        // The row by id.
        |vm, gui, id, c| {
            let row_id = atoi(arg(c, 2));
            match scan(vm, gui, id, |rows| {
                rows.iter().find(|r| r.0 == row_id).is_some_and(|r| r.2)
            }) {
                Ok(v) => Native::Bool(v.unwrap_or(false)),
                Err(e) => Native::Fail(e),
            }
        },
    );
    reg(
        vm,
        gui,
        C,
        "scrollVisible",
        3,
        3,
        "(int rowNum)",
        // The row's cell scrolled into a scroll parent's view.
        |vm, gui, id, c| {
            let row = atoi(arg(c, 2));
            if row >= 0
                && let Some(p) =
                    crate::control::parent(vm, id).filter(|p| is_a(vm, *p, "GuiScrollCtrl"))
            {
                let cell = crate::textlist::state(gui, id).cell;
                crate::scroll::scroll_rect_visible(
                    vm,
                    gui,
                    p,
                    [0, cell.1.wrapping_mul(row), cell.0, cell.1],
                );
            }
            Native::Void
        },
    );
    for (name, numeric) in [("sort", false), ("sortNumerical", true)] {
        reg(
            vm,
            gui,
            C,
            name,
            3,
            4,
            "(int columnID, bool increasing=false)",
            move |vm, gui, id, c| {
                let col = atoi(arg(c, 2));
                let increasing = c.argv.len() < 4 || atob(arg(c, 3));
                match sort_rows(vm, gui, id, col, numeric, increasing) {
                    Ok(()) => Native::Void,
                    Err(e) => Native::Fail(e),
                }
            },
        );
    }
}

fn register_popup(vm: &mut Vm, gui: &Gui) {
    const C: &str = "GuiPopUpMenuCtrl";
    for (name, front) in [("add", false), ("addFront", true)] {
        reg(
            vm,
            gui,
            C,
            name,
            4,
            5,
            "(string name, int idNum, int scheme=0)",
            move |vm, gui, id, c| {
                let text = arg(c, 2);
                let len = gui.with(id, |c| c.rows.len()).unwrap_or(0);
                // `addFront` moves every entry: paid for like a scan.
                let work = ROW_WORK
                    .saturating_add(text.len() as u64)
                    .saturating_add(if front { rows_work(len) } else { 0 });
                if !vm.charge_work(work) {
                    return Native::Fail(VmError::StepLimit);
                }
                let entry = (atoi(arg(c, 3)), text.to_vec(), true);
                gui.with_mut(id, |control| {
                    if control.rows.len() < MAX_LIST_ROWS {
                        if front {
                            control.rows.insert(0, entry);
                        } else {
                            control.rows.push(entry);
                        }
                    }
                });
                Native::Void
            },
        );
    }
    reg(vm, gui, C, "addScheme", 6, 6, "", |_, _, _, _| Native::Void);
    reg(
        vm,
        gui,
        C,
        "setEnumContent",
        4,
        4,
        "(string class, string enum)",
        // The class found by name ignoring case, then its field (its parents' too, as the class's field list holds
        // them) by name ignoring case; an enum field adds each of its table's labels with its value (scheme 0), in the table's
        // order. Each refusal is a warning: `failed to locate class rep for '%s'`, `failed to locate field '%s' for class '%s'`,
        // `field '%s' is not an enumeration for class '%s'`.
        |vm, gui, id, c| {
            let (class, field) = (arg(c, 2).to_vec(), arg(c, 3).to_vec());
            let shown = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
            let classes = &vm.objects().classes;
            let Some(def) = classes.find(&class).and_then(|k| classes.get(k)) else {
                vm.warn(format!(
                    "failed to locate class rep for '{}'",
                    shown(&class)
                ));
                return Native::Void;
            };
            let found = def.layout.field(&field).map(|(f, _)| f.ty);
            match found {
                None => vm.warn(format!(
                    "failed to locate field '{}' for class '{}'",
                    shown(&field),
                    shown(&class)
                )),
                Some(torque_ts_vm::fields::FieldType::Enum(table)) => {
                    if !vm.charge_work(rows_work(table.entries.len())) {
                        return Native::Fail(VmError::StepLimit);
                    }
                    gui.with_mut(id, |control| {
                        for (value, label) in table.entries {
                            if control.rows.len() < MAX_LIST_ROWS {
                                control.rows.push((*value, label.as_bytes().to_vec(), true));
                            }
                        }
                    });
                }
                Some(_) => vm.warn(format!(
                    "field '{}' is not an enumeration for class '{}'",
                    shown(&field),
                    shown(&class)
                )),
            }
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        C,
        "setText",
        3,
        3,
        "(string text)",
        |vm, gui, id, c| {
            set_text(vm, gui, id, arg(c, 2));
            Native::Void
        },
    );
    reg(vm, gui, C, "getText", 2, 2, "", |vm, gui, id, _| {
        text_result(vm, Ok(Some(Some(text_of(gui, id)))))
    });
    reg(vm, gui, C, "clear", 2, 2, "()", |vm, gui, id, _| {
        gui.with_mut(id, |control| {
            control.rows.clear();
            control.selected = -1;
        });
        set_text(vm, gui, id, b"");
        Native::Void
    });
    reg(vm, gui, C, "sort", 2, 2, "()", |vm, gui, id, _| {
        // The runtime's qsort of the entries by `stricmp` of their whole text, as the list's alphabetical sort of column 0.
        match sort_rows(vm, gui, id, 0, false, true) {
            Ok(()) => Native::Void,
            Err(e) => Native::Fail(e),
        }
    });
    reg(vm, gui, C, "forceOnAction", 2, 2, "", |vm, _, id, _| {
        let command = vm.objects().field(id, b"command", b"").unwrap_or_default();
        if command.is_empty() {
            Native::Void
        } else {
            match vm.eval(&command) {
                Ok(_) => Native::Void,
                Err(e) => Native::Fail(e),
            }
        }
    });
    reg(vm, gui, C, "forceClose", 2, 2, "", |vm, gui, id, _| {
        done(crate::popup::close(vm, gui, id))
    });
    reg(vm, gui, C, "getSelected", 2, 2, "", |_, gui, id, _| {
        let selected = gui.with(id, |c| {
            usize::try_from(c.selected)
                .ok()
                .and_then(|r| c.rows.get(r))
                .map_or(0, |r| r.0)
        });
        Native::Int(selected.unwrap_or(0))
    });
    reg(
        vm,
        gui,
        C,
        "setSelected",
        3,
        3,
        "(int id)",
        |vm, gui, id, c| done(popup_select(vm, gui, id, atoi(arg(c, 2)))),
    );
    reg(
        vm,
        gui,
        C,
        "getTextById",
        3,
        3,
        "(int id)",
        |vm, gui, id, c| {
            let entry = atoi(arg(c, 2));
            text_result(
                vm,
                scan(vm, gui, id, |rows| {
                    rows.iter().find(|r| r.0 == entry).map(|r| r.1.clone())
                }),
            )
        },
    );
    reg(
        vm,
        gui,
        C,
        "findText",
        3,
        3,
        "(string text)",
        |vm, gui, id, c| {
            let needle = arg(c, 2);
            int_result(
                scan(vm, gui, id, |rows| {
                    rows.iter().find(|r| r.1 == needle).map_or(-1, |r| r.0)
                }),
                -1,
            )
        },
    );
    reg(vm, gui, C, "size", 2, 2, "", |_, gui, id, _| {
        Native::Int(gui.with(id, |c| c.rows.len() as i32).unwrap_or(0))
    });
    reg(
        vm,
        gui,
        C,
        "replaceText",
        3,
        3,
        "(bool doReplaceText)",
        |_, _, _, _| Native::Void,
    );
}

/// `GuiControlProfile::updateFont`: the font made again from `fontType` and `fontSize` (`Failed to load/create
/// profile font (%s/%d)` when there is none), the bitmap loaded again (a message,
/// [`crate::ext::GuiMessage::ProfileBitmapMissing`], when it names a file that does not load) and its rectangle array
/// cleared, to be made again from it. A profile's font and bitmap are read from
/// its fields at every use, so what changes is the cached array and the errors.
fn register_profile(vm: &mut Vm, gui: &Gui) {
    let gui = gui.clone();
    vm.register_in(
        "GuiControlProfile",
        "updateFont",
        2,
        2,
        "()",
        move |vm, c| {
            let Some(id) = crate::this(vm, c) else {
                return Native::Void;
            };
            let face = vm.objects().field(id, b"fontType", b"").unwrap_or_default();
            let size = atoi(&vm.objects().field(id, b"fontSize", b"").unwrap_or_default());
            let fonts = gui.0.borrow().fonts.0.clone();
            let found = {
                let read = |path: &str| match vm.file_host().read(path.as_bytes()) {
                    torque_ts_vm::host::FileRead::Found(b) => Some(b),
                    _ => None,
                };
                fonts
                    .borrow_mut()
                    .font(&String::from_utf8_lossy(&face), size, &read)
                    .is_some()
            };
            if !found {
                vm.error(format!(
                    "Failed to load/create profile font ({}/{size})",
                    String::from_utf8_lossy(&face)
                ));
            }
            let bitmap = vm.objects().field(id, b"bitmap", b"").unwrap_or_default();
            if !bitmap.is_empty() && crate::profile::profile_bitmap(vm, id).is_none() {
                let profile = crate::name_of(vm, id);
                let bitmap = String::from_utf8_lossy(&bitmap);
                let m = crate::ext::GuiMessage::ProfileBitmapMissing {
                    profile: &profile,
                    bitmap: &bitmap,
                };
                vm.error(crate::message_text(&gui, &m));
            }
            gui.0.borrow_mut().bitmap_arrays.remove(&id);
            Native::Void
        },
    );
}
