//! Input as the canvas dispatches it (TGE 1.4's design: `GuiCanvas`, `GuiControl`, `GuiButtonBaseCtrl`, `GuiTextEditCtrl`).
//!
//! **The canvas** (`GuiCanvas::processInputEvent`), for one engine input event ([`InputEvent`]):
//! - The canvas's first responder's `onInputEvent` first (only GuiInputCtrl answers).
//! - Keyboard: the event's ascii, modifiers, key code (its low byte) and action become the canvas's last event. A make goes to the
//!   first responder's `onKeyDown`; then, with the cursor on and Tab: Shift-Tab tabs back (the top dialog's `findPrevTabable`, else its
//!   `findLastTabable`), Tab without modifiers tabs on (`tabNext`) and both are consumed; then the accelerators (key and normalised
//!   modifiers) press. A break goes to `onKeyUp`, then releases an accelerator. A repeat goes to `onKeyRepeat` and is consumed even
//!   with no first responder. Anything else is not consumed.
//! - Mouse (only with the cursor on): the X and Y axes move the cursor by `value * pixelsPerMickey` (1), clamped to the canvas; an
//!   unmoved axis is consumed and does nothing; a move goes to the captured control (`onMouseDragged` with the left button down,
//!   `onRightMouseDragged` with the right one, else `onMouseMove`, after the captured control's enter/leave check), else to the
//!   control under the cursor (`findMouseControl`: leave/enter as it changes). The wheel goes to the captured control or the one under
//!   the cursor and is NOT consumed (the maps see it too). Button 0: a make counts clicks (the last press was the left one and at most
//!   500 ms ago: one more, else 1), and goes to the captured control, else to the first canvas child from the top whose hit control
//!   (`findHitControl`) is active or has a modal profile (that control's window comes to the front first); no such
//!   child: nobody. A break goes to the captured control, else to the one under the cursor. Button 1 the same with `onRightMouse*`
//!   and a 50 ms double-click window. Both are consumed.
//! - The window's absolute mouse position becomes an X and a Y axis event of the difference ([`process_mouse_move`]), straight
//!   to the canvas (never to the action maps).
//!
//! **Controls** (by class, the first that applies): GuiTextEditCtrl ([`text_edit`]), GuiButtonBaseCtrl and its subclasses (check
//! boxes, radios, bitmap buttons), GuiMouseEventCtrl (the script callbacks with the modifier, the point, the click count),
//! GuiInputCtrl (the remap dialog's `onInputEvent(device, action, make)`), else GuiControl (keys and the wheel go to the parent).
//!
//! **First responders and tabbing**: every control remembers one (`mFirstResponder`); the canvas's is the one keys go to.
//! `setFirstResponder` needs the control awake and visible and its profile `canKeyFocus`; `setContent` and `pushDialog` take the
//! content's or dialog's `findFirstTabable` directly; `popDialog` the new top's remembered one; the old one gets
//! `onLoseFirstResponder`. Accelerators are rebuilt from the top child (`pushDialog`, `popDialog`) or from the top down to the first
//! modal child (`setContent`).
//!
//! **Script callbacks run with no borrow held** (each is a `Vm` call of its own after the state was written back), and the tree is
//! read again after every one. Every walk is capped ([`crate::MAX_TREE_DEPTH`], [`MAX_TAB_VISITS`]) and charged; an event from the
//! host runs under a host budget of its own ([`MAX_EVENT_WORK`], [`MAX_EVENT_STEPS`]).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/core/guiCanvas.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.), `gui/core/guiControl.cpp` (Copyright (c) 2012 GarageGames, LLC;
//! Copyright (C) 2015 Faust Logic, Inc.), `gui/buttons/guiButtonBaseCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC),
//! `gui/utility/guiMouseEventCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC), `gui/utility/guiInputCtrl.cpp` (Copyright (c)
//! 2012 GarageGames, LLC), `gui/core/guiArrayCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's
//! own.

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use crate::control::{children, flag, is_a, parent};
use crate::keys::{self, *};
use crate::{Gui, MAX_TREE_DEPTH};

/// The work one input event from the host may do outside the scripts it calls (`Vm::charge_work` units).
pub const MAX_EVENT_WORK: u64 = 16_000_000;
/// The steps the scripts one input event calls may take together.
pub const MAX_EVENT_STEPS: u64 = 50_000_000;
/// The most controls one tab search visits.
pub const MAX_TAB_VISITS: u32 = 1 << 16;
/// The double-click windows: the left button's and the right button's, in milliseconds.
pub const LEFT_DOUBLE_CLICK_MS: u32 = 500;
pub const RIGHT_DOUBLE_CLICK_MS: u32 = 50;
/// The caret's flip: more than 500 ms and more than 3 frames since the last flip.
pub const CARET_FLIP_MS: u32 = 500;
pub const CARET_FLIP_FRAMES: u32 = 3;
/// The most accelerators the canvas keeps (a safety cap).
pub const MAX_ACCELERATORS: usize = 4096;
/// A text edit's history lines (a safety cap on `historySize`).
pub const MAX_HISTORY: usize = 1024;

/// One engine input event (TGE's `InputEvent`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct InputEvent {
    pub device: u16,
    pub device_inst: u32,
    pub obj_type: u16,
    pub obj_inst: u16,
    pub action: u8,
    pub modifier: u8,
    pub ascii: u16,
    pub value: f32,
}

/// What a control's handler is given (TGE's `GuiEvent`; the key code is a byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GuiEvent {
    pub ascii: u16,
    pub modifier: u8,
    pub key_code: u8,
    pub point: (i32, i32),
    pub clicks: u8,
    pub action: u8,
}

/// An accelerator of the canvas's map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accelerator {
    pub control: ObjId,
    pub index: u32,
    pub key: u16,
    pub modifier: u8,
}

/// The canvas's input state (GuiCanvas's members).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CanvasInput {
    /// `mFirstResponder`.
    pub first_responder: Option<ObjId>,
    /// `mMouseCapturedControl` and `mMouseControl`, `mMouseControlClicked`.
    pub capture: Option<ObjId>,
    pub mouse_control: Option<ObjId>,
    pub mouse_control_clicked: bool,
    /// `mMouseButtonDown`, `mMouseRightButtonDown`, `mLeftMouseLast`.
    pub left_down: bool,
    pub right_down: bool,
    pub left_last: bool,
    /// `mLastMouseDownTime`, `mLastMouseClickCount`, `mPrevMouseTime`.
    pub last_down_time: u32,
    pub click_count: u8,
    pub prev_mouse_time: u32,
    /// `mLastEvent`.
    pub last: GuiEvent,
    pub accelerators: Vec<Accelerator>,
    /// The tab searches' globals (`smCurResponder`, `smPrevResponder`).
    tab_cur: Option<ObjId>,
    tab_prev: Option<ObjId>,
    /// The canvas's own clipboard, used when no [`SYSTEM_CLIPBOARD`] is set (tests and headless runs).
    pub clipboard: Vec<u8>,
}

/// The system clipboard the text edits use: the window sets it at start; unset, the canvas keeps its own.
pub static SYSTEM_CLIPBOARD: std::sync::OnceLock<SystemClipboard> = std::sync::OnceLock::new();

/// Reads and writes the system clipboard's text.
#[derive(Debug, Clone, Copy)]
pub struct SystemClipboard {
    pub get: fn() -> Option<String>,
    pub set: fn(&str),
}

/// Text from the system clipboard as a text edit takes it: its first line, each character as one byte (Latin-1; others become `?`).
pub fn clipboard_bytes(text: &str) -> Vec<u8> {
    text.split(['\r', '\n'])
        .next()
        .unwrap_or("")
        .chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
        .collect()
}

/// A text edit's bytes as system clipboard text (each byte as its Latin-1 character).
pub fn clipboard_text(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// A text edit's editing state (GuiTextEditCtrl's members).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EditState {
    pub cursor: i32,
    pub block_start: i32,
    pub block_end: i32,
    /// `mInsertOn`: true inserts, false overwrites (TGE's constructor: true).
    pub insert_on: bool,
    pub changes_made: bool,
    /// A make seen, so repeats insert.
    pub key_seen: bool,
    pub drag_hit: bool,
    pub drag_start: i32,
    pub history: Vec<Vec<u8>>,
    pub history_last: i32,
    pub history_index: i32,
    /// The undo buffer: the text, the block and the cursor.
    pub undo: (Vec<u8>, i32, i32, i32),
    /// The caret: on, when it last flipped, frames since.
    pub caret_on: bool,
    pub caret_flipped: u32,
    pub caret_frames: u32,
    /// `mTextOffset.x` where the text starts while it scrolls; `None`: placed afresh at the next draw.
    pub text_offset: Option<i32>,
    /// The engine's keys only ([`text_edit`]): the undo steps (oldest first, at most [`text_edit::MAX_UNDO_STEPS`]), the
    /// redo steps, and the kind of change the last step may still take more of.
    pub undo_steps: Vec<(Vec<u8>, i32, i32, i32)>,
    pub redo_steps: Vec<(Vec<u8>, i32, i32, i32)>,
    pub undo_merge: Option<text_edit::Merge>,
    /// The engine's keys only: the word or the whole text a double or triple click selected, kept while the drag goes on.
    pub drag_range: Option<(i32, i32)>,
}

impl EditState {
    pub fn new() -> EditState {
        EditState {
            insert_on: true,
            history_last: -1,
            ..EditState::default()
        }
    }
}

// ---- small readers

fn alive(vm: &Vm, id: ObjId) -> bool {
    vm.objects().contains(id) && is_a(vm, id, "GuiControl")
}

fn text_field(vm: &Vm, id: ObjId, field: &[u8]) -> Vec<u8> {
    vm.objects().field(id, field, b"").unwrap_or_default()
}

/// The control's profile object.
pub(crate) fn profile(vm: &Vm, id: ObjId) -> Option<ObjId> {
    let name = text_field(vm, id, b"profile");
    if name.is_empty() {
        return None;
    }
    vm.objects()
        .find(&name)
        .filter(|p| is_a(vm, *p, "GuiControlProfile"))
}

fn profile_flag(vm: &Vm, id: ObjId, field: &[u8]) -> bool {
    profile(vm, id).is_some_and(|p| flag(vm, p, field))
}

fn awake(gui: &Gui, id: ObjId) -> bool {
    gui.is_awake(id)
}

fn visible(vm: &Vm, id: ObjId) -> bool {
    flag(vm, id, b"visible")
}

fn enabled(vm: &Vm, id: ObjId) -> bool {
    flag(vm, id, b"enabled")
}

/// `mActive`.
pub(crate) fn active(gui: &Gui, id: ObjId) -> bool {
    gui.with(id, |c| c.active).unwrap_or(false)
}

fn canvas_of(vm: &Vm, gui: &Gui) -> Option<ObjId> {
    gui.canvas().filter(|c| vm.objects().contains(*c))
}

fn class_name(vm: &Vm, id: ObjId) -> Vec<u8> {
    vm.objects()
        .get(id)
        .map(|o| vm.objects().classes.name(o.class()).to_vec())
        .unwrap_or_default()
}

fn input(gui: &Gui) -> std::cell::RefMut<'_, CanvasInput> {
    std::cell::RefMut::map(gui.0.borrow_mut(), |s| &mut s.input)
}

/// A value read out of the canvas's input state, the borrow dropped before it is returned.
fn peek<R>(gui: &Gui, f: impl FnOnce(&CanvasInput) -> R) -> R {
    let s = gui.0.borrow();
    f(&s.input)
}

fn now(vm: &Vm) -> u32 {
    vm.sim_time()
}

/// `$ThisControl` and `Con::evaluate(command)`, as `execConsoleCallback`.
fn exec_command(vm: &mut Vm, id: ObjId, command: &[u8]) -> Result<(), VmError> {
    vm.set_global("$ThisControl", &id.to_string());
    vm.eval(command).map(|_| ())
}

/// The variable a control names, with its `$`.
fn variable_name(vm: &Vm, id: ObjId) -> Option<String> {
    let name = text_field(vm, id, b"variable");
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

// ---- the sound hook (the audio plays the profile's sounds; the GUI only names them)

/// What the GUI asks the audio layer to play: the AudioProfile a button profile names (`soundButtonDown`, `soundButtonOver`).
pub type SoundHook = std::rc::Rc<dyn Fn(&mut Vm, ObjId)>;

/// The hook's place in the GUI's state.
#[derive(Clone, Default)]
pub struct SoundHookSlot(pub Option<SoundHook>);

impl std::fmt::Debug for SoundHookSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "SoundHook(set)"
        } else {
            "SoundHook(none)"
        })
    }
}

/// `$Pref::Audio::MenuSounds` on and the profile's field naming an AudioProfile: the hook is called with it.
fn button_sound(vm: &mut Vm, gui: &Gui, id: ObjId, field: &[u8]) {
    let Some(p) = profile(vm, id) else {
        return;
    };
    let name = text_field(vm, p, field);
    if name.is_empty() {
        return;
    }
    if !torque_ts_vm::fields::atob(&vm.global("$Pref::Audio::MenuSounds").unwrap_or_default()) {
        return;
    }
    let Some(sound) = vm.objects().find(&name) else {
        return;
    };
    let hook = gui.0.borrow().sound_hook.0.clone();
    if let Some(hook) = hook {
        hook(vm, sound);
    }
}

// ---- hit tests, the mouse control, capture

/// `findHitControl(pt, -1)` from `root` (canvas coordinates relative to `root`'s parent space: the canvas's are the screen's).
fn hit(vm: &Vm, gui: &Gui, root: ObjId, pt: (i32, i32)) -> ObjId {
    crate::canvas::hit_test_gui(vm, gui, root, pt, crate::canvas::MAX_HIT_VISITS)
}

/// `GuiCanvas::findMouseControl`: the control under the cursor becomes the mouse control, the old one leaving and the
/// new one entering; with no child the mouse control is cleared.
fn find_mouse_control(vm: &mut Vm, gui: &Gui, ev: GuiEvent) -> Result<(), VmError> {
    let Some(canvas) = canvas_of(vm, gui) else {
        return Ok(());
    };
    if children(vm, canvas).is_empty() {
        input(gui).mouse_control = None;
        sync_hover(gui);
        return Ok(());
    }
    let h = hit(vm, gui, canvas, ev.point);
    let old = input(gui).mouse_control;
    if Some(h) != old {
        if let Some(o) = old.filter(|o| alive(vm, *o)) {
            on_mouse_leave(vm, gui, o, ev)?;
        }
        input(gui).mouse_control = Some(h);
        on_mouse_enter(vm, gui, h, ev)?;
    }
    Ok(())
}

/// `checkLockMouseMove`: with a capture, the captured control leaves or enters as the cursor leaves or enters it.
fn check_lock_mouse_move(vm: &mut Vm, gui: &Gui, ev: GuiEvent) -> Result<(), VmError> {
    let Some(canvas) = canvas_of(vm, gui) else {
        return Ok(());
    };
    let h = hit(vm, gui, canvas, ev.point);
    let (old, capture) = {
        let i = input(gui);
        (i.mouse_control, i.capture)
    };
    if Some(h) != old {
        if let Some(c) = capture.filter(|c| alive(vm, *c)) {
            if old == Some(c) {
                on_mouse_leave(vm, gui, c, ev)?;
            } else if h == c {
                on_mouse_enter(vm, gui, c, ev)?;
            }
        }
        input(gui).mouse_control = Some(h);
    }
    Ok(())
}

/// The canvas's first responder (`GuiCanvas`), the control keys go to.
pub fn canvas_first_responder(gui: &Gui) -> Option<ObjId> {
    peek(gui, |i| i.first_responder)
}

/// `GuiCanvas::mouseLock`.
pub fn mouse_lock(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    let (capture, mc) = peek(gui, |i| (i.capture, i.mouse_control));
    let pt = gui.0.borrow().cursor_pos;
    if capture.is_some() {
        return Ok(());
    }
    input(gui).capture = Some(id);
    if let Some(m) = mc.filter(|m| *m != id && alive(vm, *m)) {
        let ev = GuiEvent {
            point: pt,
            ..input(gui).last
        };
        on_mouse_leave(vm, gui, m, ev)?;
    }
    Ok(())
}

/// `GuiCanvas::mouseUnlock`.
pub fn mouse_unlock(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    if peek(gui, |i| i.capture) != Some(id) {
        return Ok(());
    }
    let pt = gui.0.borrow().cursor_pos;
    if let Some(canvas) = canvas_of(vm, gui) {
        let h = hit(vm, gui, canvas, pt);
        if h != id {
            {
                let mut i = input(gui);
                i.mouse_control = Some(h);
                i.mouse_control_clicked = false;
            }
            let ev = GuiEvent {
                point: pt,
                ..input(gui).last
            };
            on_mouse_enter(vm, gui, h, ev)?;
        }
    }
    input(gui).capture = None;
    Ok(())
}

/// The canvas's mouse control at the cursor again (`refreshMouseControl`).
pub fn refresh_mouse_control(vm: &mut Vm, gui: &Gui) -> Result<(), VmError> {
    let pt = gui.0.borrow().cursor_pos;
    let ev = GuiEvent {
        point: pt,
        ..input(gui).last
    };
    find_mouse_control(vm, gui, ev)
}

/// The buttons' `mMouseOver` shown by drawing follows the canvas's mouse control (the hover).
fn sync_hover(_gui: &Gui) {}

/// The shape of the system cursor over the control under the mouse ([`hover_cursor`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CursorShape {
    /// The default arrow.
    #[default]
    Arrow,
    /// The hand: the control under the mouse takes a click.
    Pointer,
    /// The text cursor: a text edit.
    Text,
}

/// The cursor shape for the control under the mouse (the canvas's mouse control, the one [`find_mouse_control`] keeps): a
/// button of any kind, a popup menu, a slider or a row of a text list that is active (and a button enabled) is a pointer, an
/// active text edit the text cursor, anything else the arrow.
pub fn hover_cursor(vm: &Vm, gui: &Gui) -> CursorShape {
    let (mouse, pt) = {
        let s = gui.0.borrow();
        (s.input.mouse_control, s.cursor_pos)
    };
    let Some(id) = mouse.filter(|m| alive(vm, *m)) else {
        return CursorShape::Arrow;
    };
    if !active(gui, id) || !gui.is_awake(id) {
        return CursorShape::Arrow;
    }
    let pointer = if is_a(vm, id, "GuiTextEditCtrl") {
        return CursorShape::Text;
    } else if is_a(vm, id, "GuiButtonBaseCtrl") {
        enabled(vm, id)
    } else if is_a(vm, id, "GuiTextListCtrl") {
        crate::textlist::row_under(vm, gui, id, pt)
    } else {
        is_a(vm, id, "GuiPopUpMenuCtrl") || is_a(vm, id, "GuiSliderCtrl")
    };
    if pointer {
        CursorShape::Pointer
    } else {
        CursorShape::Arrow
    }
}

// ---- first responders

/// `GuiControl::setFirstResponder(GuiControl*)` up the parents (the canvas's tells the old one it lost it).
fn set_first_responder_of(
    vm: &mut Vm,
    gui: &Gui,
    at: ObjId,
    fr: ObjId,
    depth: usize,
) -> Result<(), VmError> {
    if depth > MAX_TREE_DEPTH || !vm.charge_work(32) {
        return Ok(());
    }
    let key_focus = profile_flag(vm, fr, b"canKeyFocus");
    if Some(at) == canvas_of(vm, gui) {
        let old = input(gui).first_responder;
        if key_focus {
            input(gui).first_responder = Some(fr);
            gui.with_mut(at, |c| c.first_responder = Some(fr));
        }
        let new = input(gui).first_responder;
        if let Some(o) = old.filter(|o| Some(*o) != new && alive(vm, *o)) {
            on_lose_first_responder(vm, gui, o)?;
        }
        return Ok(());
    }
    if key_focus {
        gui.with_mut(at, |c| c.first_responder = Some(fr));
    }
    if let Some(p) = parent(vm, at) {
        set_first_responder_of(vm, gui, p, fr, depth + 1)?;
    }
    Ok(())
}

/// `GuiControl::setFirstResponder()`: awake and visible, the profile `canKeyFocus`: the parents make it theirs.
pub fn set_first_responder(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    if !awake(gui, id) || !visible(vm, id) {
        return Ok(());
    }
    if profile_flag(vm, id, b"canKeyFocus")
        && let Some(p) = parent(vm, id)
    {
        let old = input(gui).first_responder;
        set_first_responder_of(vm, gui, p, id, 0)?;
        if old != Some(id) && input(gui).first_responder == Some(id) {
            gui.ext().focused(vm, gui, id)?;
        }
    }
    Ok(())
}

/// `GuiControl::clearFirstResponder`: every parent up the chain whose first responder it is forgets it.
pub fn clear_first_responder(vm: &Vm, gui: &Gui, id: ObjId) {
    let mut at = id;
    let canvas = canvas_of(vm, gui);
    for _ in 0..=MAX_TREE_DEPTH {
        let Some(p) = vm.objects().get(at).and_then(|o| o.group()) else {
            break;
        };
        if !is_a(vm, p, "GuiControl") {
            break;
        }
        let mine = if Some(p) == canvas {
            input(gui).first_responder == Some(id)
        } else {
            gui.with(p, |c| c.first_responder == Some(id))
                .unwrap_or(false)
        };
        if !mine {
            break;
        }
        gui.with_mut(p, |c| c.first_responder = None);
        if Some(p) == canvas {
            input(gui).first_responder = None;
        }
        at = p;
    }
}

/// `onLoseFirstResponder`: a text edit's history and `validate`; others nothing.
pub fn on_lose_first_responder(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    if is_a(vm, id, "GuiTextEditCtrl") {
        text_edit::lose_first_responder(vm, gui, id)?;
    }
    Ok(())
}

fn tabable(vm: &Vm, gui: &Gui, id: ObjId) -> bool {
    profile_flag(vm, id, b"tab") && awake(gui, id) && visible(vm, id)
}

/// `findFirstTabable`: the first child's first tabable (remembered as the control's first responder), else the control
/// when it is tabable.
pub fn find_first_tabable(vm: &Vm, gui: &Gui, id: ObjId) -> Option<ObjId> {
    let mut seen = 0u32;
    first_tabable(vm, gui, id, 0, &mut seen)
}

fn first_tabable(vm: &Vm, gui: &Gui, id: ObjId, depth: usize, seen: &mut u32) -> Option<ObjId> {
    if depth > MAX_TREE_DEPTH || *seen >= MAX_TAB_VISITS || !vm.charge_work(16) {
        return None;
    }
    *seen += 1;
    for child in children(vm, id) {
        if !alive(vm, child) {
            continue;
        }
        if let Some(found) = first_tabable(vm, gui, child, depth + 1, seen) {
            gui.with_mut(id, |c| c.first_responder = Some(found));
            return Some(found);
        }
    }
    tabable(vm, gui, id).then_some(id)
}

/// `findLastTabable(true)` (TGE 1.4): every tabable control in order; the last one.
fn find_last_tabable(vm: &Vm, gui: &Gui, id: ObjId) -> Option<ObjId> {
    input(gui).tab_prev = None;
    let mut seen = 0u32;
    last_tabable(vm, gui, id, 0, &mut seen);
    let prev = input(gui).tab_prev;
    gui.with_mut(id, |c| c.first_responder = prev);
    prev
}

fn last_tabable(vm: &Vm, gui: &Gui, id: ObjId, depth: usize, seen: &mut u32) {
    if depth > MAX_TREE_DEPTH || *seen >= MAX_TAB_VISITS || !vm.charge_work(16) {
        return;
    }
    *seen += 1;
    if profile_flag(vm, id, b"tab") {
        input(gui).tab_prev = Some(id);
    }
    for child in children(vm, id) {
        if alive(vm, child) {
            last_tabable(vm, gui, child, depth + 1, seen);
        }
    }
    let prev = input(gui).tab_prev;
    gui.with_mut(id, |c| c.first_responder = prev);
}

/// `findNextTabable(cur, first)`.
fn next_tabable(
    vm: &Vm,
    gui: &Gui,
    id: ObjId,
    cur: Option<ObjId>,
    first: bool,
    depth: usize,
    seen: &mut u32,
) -> Option<ObjId> {
    if depth > MAX_TREE_DEPTH || *seen >= MAX_TAB_VISITS || !vm.charge_work(16) {
        return None;
    }
    *seen += 1;
    if first {
        input(gui).tab_cur = None;
    }
    if cur == Some(id) {
        input(gui).tab_cur = Some(id);
    } else if peek(gui, |i| i.tab_cur.is_some()) && tabable(vm, gui, id) && active(gui, id) {
        return Some(id);
    }
    let mut found = None;
    for child in children(vm, id) {
        if !alive(vm, child) {
            continue;
        }
        found = next_tabable(vm, gui, child, cur, false, depth + 1, seen);
        if found.is_some() {
            break;
        }
    }
    gui.with_mut(id, |c| c.first_responder = found);
    found
}

/// `findPrevTabable(cur, first)`.
fn prev_tabable(
    vm: &Vm,
    gui: &Gui,
    id: ObjId,
    cur: Option<ObjId>,
    first: bool,
    depth: usize,
    seen: &mut u32,
) -> Option<ObjId> {
    if depth > MAX_TREE_DEPTH || *seen >= MAX_TAB_VISITS || !vm.charge_work(16) {
        return None;
    }
    *seen += 1;
    if first {
        input(gui).tab_prev = None;
    }
    if cur == Some(id) {
        return input(gui).tab_prev;
    }
    if tabable(vm, gui, id) && active(gui, id) {
        input(gui).tab_prev = Some(id);
    }
    let mut found = None;
    for child in children(vm, id) {
        if !alive(vm, child) {
            continue;
        }
        found = prev_tabable(vm, gui, child, cur, false, depth + 1, seen);
        if found.is_some() {
            break;
        }
    }
    gui.with_mut(id, |c| c.first_responder = found);
    found
}

/// `GuiCanvas::tabNext`.
pub fn tab_next(vm: &mut Vm, gui: &Gui) -> Result<bool, VmError> {
    let Some(canvas) = canvas_of(vm, gui) else {
        return Ok(false);
    };
    let Some(top) = children(vm, canvas)
        .last()
        .copied()
        .filter(|t| alive(vm, *t))
    else {
        return Ok(false);
    };
    let old = input(gui).first_responder;
    let mut seen = 0;
    let new = next_tabable(vm, gui, top, old, true, 0, &mut seen)
        .or_else(|| find_first_tabable(vm, gui, top));
    let Some(new) = new else {
        return Ok(false);
    };
    if Some(new) != old {
        set_first_responder(vm, gui, new)?;
        if let Some(o) = old.filter(|o| alive(vm, *o)) {
            on_lose_first_responder(vm, gui, o)?;
        }
    }
    Ok(true)
}

/// Shift-Tab (in `processInputEvent`): the top dialog's previous tabable (else its last), made the first responder.
fn tab_prev(vm: &mut Vm, gui: &Gui) -> Result<(), VmError> {
    let Some(canvas) = canvas_of(vm, gui) else {
        return Ok(());
    };
    let Some(top) = children(vm, canvas)
        .last()
        .copied()
        .filter(|t| alive(vm, *t))
    else {
        return Ok(());
    };
    let old = input(gui).first_responder;
    let mut seen = 0;
    let new = prev_tabable(vm, gui, top, old, true, 0, &mut seen)
        .or_else(|| find_last_tabable(vm, gui, top));
    if let Some(new) = new.filter(|n| Some(*n) != old) {
        set_first_responder(vm, gui, new)?;
        if let Some(o) = old.filter(|o| alive(vm, *o)) {
            on_lose_first_responder(vm, gui, o)?;
        }
    }
    Ok(())
}

// ---- accelerators

/// `buildAcceleratorMap` of `id` and its children: a control whose `accelerator` field parses to a key adds itself.
fn build_accelerators(vm: &Vm, gui: &Gui, id: ObjId, depth: usize, seen: &mut u32) {
    if depth > MAX_TREE_DEPTH || *seen >= MAX_TAB_VISITS || !vm.charge_work(16) {
        return;
    }
    *seen += 1;
    let accel = text_field(vm, id, b"accelerator");
    if !accel.is_empty()
        && let Some(d) = keys::parse_descriptor_with(&accel, &gui.ext().key_tables())
        && d.code != 0
    {
        let mut i = input(gui);
        if i.accelerators.len() < MAX_ACCELERATORS {
            i.accelerators.push(Accelerator {
                control: id,
                index: 0,
                key: d.code,
                modifier: d.modifiers,
            });
        }
    }
    for child in children(vm, id) {
        if alive(vm, child) {
            build_accelerators(vm, gui, child, depth + 1, seen);
        }
    }
}

/// The canvas's map again: from the top child only (`push`, `pop`), or from the top down to the first child with a modal profile
/// (`setContent`).
pub fn rebuild_accelerators(vm: &Vm, gui: &Gui, to_modal: bool) {
    input(gui).accelerators.clear();
    let Some(canvas) = canvas_of(vm, gui) else {
        return;
    };
    let kids = children(vm, canvas);
    let mut seen = 0;
    for child in kids.into_iter().rev() {
        if !alive(vm, child) {
            continue;
        }
        build_accelerators(vm, gui, child, 0, &mut seen);
        if !to_modal || profile_flag(vm, child, b"modal") {
            break;
        }
    }
}

fn accelerator_press(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    if is_a(vm, id, "GuiButtonBaseCtrl") {
        // Active and enabled: depressed, and the first responder when its profile is tabable.
        if active(gui, id) && enabled(vm, id) {
            gui.with_mut(id, |c| c.depressed = true);
            if profile_flag(vm, id, b"tab") {
                set_first_responder(vm, gui, id)?;
            }
        }
        return Ok(());
    }
    // GuiControl::acceleratorKeyPress: onAction.
    on_action(vm, gui, id)
}

fn accelerator_release(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    if is_a(vm, id, "GuiButtonBaseCtrl") {
        // When depressed, not any more, and the action.
        if active(gui, id) && enabled(vm, id) && gui.with(id, |c| c.depressed).unwrap_or(false) {
            gui.with_mut(id, |c| c.depressed = false);
            on_action(vm, gui, id)?;
        }
    }
    Ok(())
}

// ---- actions

/// `onAction`: a button's toggle or radio first, then `GuiControl::onAction` (inactive does
/// nothing; the command with `$ThisControl`, else the script `onAction`).
pub fn on_action(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    if is_a(vm, id, "GuiButtonBaseCtrl") {
        if !active(gui, id) || !enabled(vm, id) {
            return Ok(());
        }
        let kind = text_field(vm, id, b"buttonType");
        if kind.eq_ignore_ascii_case(b"ToggleButton") {
            let on = !gui.with(id, |c| c.state_on).unwrap_or(false);
            gui.with_mut(id, |c| c.state_on = on);
            if let Some(var) = variable_name(vm, id) {
                vm.set_global(&var, if on { "1" } else { "0" });
            }
        } else if kind.eq_ignore_ascii_case(b"RadioButton") {
            gui.with_mut(id, |c| c.state_on = true);
            let group = torque_ts_vm::text::atoi(&text_field(vm, id, b"groupNum"));
            message_siblings(vm, gui, id, group);
        }
    }
    control_on_action(vm, gui, id)
}

fn control_on_action(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    if !active(gui, id) {
        return Ok(());
    }
    let command = text_field(vm, id, b"command");
    if command.is_empty() {
        vm.call_method(id, b"onAction", &[]).map(|_| ())
    } else {
        exec_command(vm, id, &command)
    }
}

/// `messageSiblings(group)`: every other child of the parent gets `onMessage(sender, group)`; a radio button of that
/// group turns off (TGE 1.4's button `onMessage`).
fn message_siblings(vm: &mut Vm, gui: &Gui, id: ObjId, group: i32) {
    let Some(p) = parent(vm, id) else {
        return;
    };
    for sibling in children(vm, p) {
        if sibling == id || !alive(vm, sibling) || !is_a(vm, sibling, "GuiButtonBaseCtrl") {
            continue;
        }
        let kind = text_field(vm, sibling, b"buttonType");
        if kind.eq_ignore_ascii_case(b"RadioButton")
            && torque_ts_vm::text::atoi(&text_field(vm, sibling, b"groupNum")) == group
        {
            gui.with_mut(sibling, |c| c.state_on = false);
        }
    }
}

// ---- the controls' mouse handlers

/// A control's `onMouseDown` (a window passes a click on its client area to the control under it).
pub(crate) fn mouse_down_on(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    ev: GuiEvent,
) -> Result<(), VmError> {
    on_mouse_down(vm, gui, id, ev)
}

fn on_mouse_down(vm: &mut Vm, gui: &Gui, id: ObjId, ev: GuiEvent) -> Result<(), VmError> {
    if crate::popup::part_of(gui, id).is_some() {
        return crate::popup::part_mouse_down(vm, gui, id, ev.point, ev.clicks);
    }
    if is_a(vm, id, "GuiPopUpMenuCtrl") {
        // An active popup opens.
        return if active(gui, id) {
            crate::popup::open(vm, gui, id)
        } else {
            Ok(())
        };
    }
    if is_a(vm, id, "GuiScrollCtrl") {
        return crate::scroll::mouse_down(vm, gui, id, ev.point);
    }
    if is_a(vm, id, "GuiArrayCtrl") {
        return crate::textlist::mouse_down(vm, gui, id, ev.point, ev.clicks);
    }
    if is_a(vm, id, "GuiSliderCtrl") {
        return crate::slider::mouse_down(vm, gui, id, ev.point, ev.modifier);
    }
    if is_a(vm, id, "GuiWindowCtrl") {
        return crate::window::mouse_down(vm, gui, id, ev);
    }
    if is_a(vm, id, "GuiMLTextCtrl") {
        return crate::mltext::mouse_down(vm, gui, id, ev.point);
    }
    if is_a(vm, id, "GuiTextEditCtrl") {
        return text_edit::mouse_down(vm, gui, id, ev);
    }
    if is_a(vm, id, "GuiButtonBaseCtrl") {
        // `onMouseDown`.
        if !active(gui, id) || !enabled(vm, id) {
            return Ok(());
        }
        if profile_flag(vm, id, b"canKeyFocus") {
            set_first_responder(vm, gui, id)?;
        }
        button_sound(vm, gui, id, b"soundButtonDown");
        mouse_lock(vm, gui, id)?;
        gui.with_mut(id, |c| c.depressed = true);
        return Ok(());
    }
    if is_a(vm, id, "GuiMouseEventCtrl") {
        if flag(vm, id, b"lockMouse") {
            mouse_lock(vm, gui, id)?;
        }
        return mouse_event(vm, id, b"onMouseDown", ev);
    }
    Ok(())
}

fn on_mouse_up(vm: &mut Vm, gui: &Gui, id: ObjId, ev: GuiEvent) -> Result<(), VmError> {
    if is_a(vm, id, "GuiScrollCtrl") {
        return crate::scroll::mouse_up(vm, gui, id);
    }
    if is_a(vm, id, "GuiSliderCtrl") {
        return crate::slider::mouse_up(vm, gui, id);
    }
    if is_a(vm, id, "GuiWindowCtrl") {
        return crate::window::mouse_up(vm, gui, id, ev.point);
    }
    if is_a(vm, id, "GuiMLTextCtrl") {
        return crate::mltext::mouse_up(vm, gui, id, ev.point);
    }
    if is_a(vm, id, "GuiTextEditCtrl") {
        gui.with_mut(id, |c| {
            if let Some(e) = c.edit.as_mut() {
                e.drag_hit = false;
            }
        });
        return mouse_unlock(vm, gui, id);
    }
    if is_a(vm, id, "GuiButtonBaseCtrl") {
        // `onMouseUp`.
        if !active(gui, id) || !enabled(vm, id) {
            return Ok(());
        }
        mouse_unlock(vm, gui, id)?;
        let was = gui.with(id, |c| c.depressed).unwrap_or(false);
        if was {
            on_action(vm, gui, id)?;
        }
        gui.with_mut(id, |c| c.depressed = false);
        return Ok(());
    }
    if is_a(vm, id, "GuiMouseEventCtrl") {
        if flag(vm, id, b"lockMouse") {
            mouse_unlock(vm, gui, id)?;
        }
        return mouse_event(vm, id, b"onMouseUp", ev);
    }
    Ok(())
}

fn on_mouse_move(vm: &mut Vm, gui: &Gui, id: ObjId, ev: GuiEvent) -> Result<(), VmError> {
    if is_a(vm, id, "GuiMouseEventCtrl") {
        return mouse_event(vm, id, b"onMouseMove", ev);
    }
    if is_a(vm, id, "GuiArrayCtrl") {
        // The mouse-over cell follows.
        crate::textlist::mouse_move(vm, gui, id, ev.point);
        return Ok(());
    }
    if is_a(vm, id, "GuiButtonBaseCtrl") || is_a(vm, id, "GuiTextEditCtrl") {
        return Ok(());
    }
    // GuiControl::onMouseMove: visible and awake, to the parent.
    if visible(vm, id)
        && awake(gui, id)
        && let Some(p) = parent(vm, id)
        && vm.charge_work(16)
    {
        return on_mouse_move(vm, gui, p, ev);
    }
    Ok(())
}

fn on_mouse_dragged(vm: &mut Vm, gui: &Gui, id: ObjId, ev: GuiEvent) -> Result<(), VmError> {
    if is_a(vm, id, "GuiTextEditCtrl") {
        return text_edit::mouse_dragged(vm, gui, id, ev);
    }
    if is_a(vm, id, "GuiScrollCtrl") {
        crate::scroll::mouse_dragged(vm, gui, id, ev.point);
        return Ok(());
    }
    if is_a(vm, id, "GuiArrayCtrl") {
        crate::textlist::mouse_move(vm, gui, id, ev.point);
        return Ok(());
    }
    if is_a(vm, id, "GuiSliderCtrl") {
        return crate::slider::mouse_dragged(vm, gui, id, ev.point, ev.modifier);
    }
    if is_a(vm, id, "GuiWindowCtrl") {
        crate::window::mouse_dragged(vm, gui, id, ev.point);
        return Ok(());
    }
    if is_a(vm, id, "GuiMouseEventCtrl") {
        return mouse_event(vm, id, b"onMouseDragged", ev);
    }
    Ok(())
}

fn on_mouse_enter(vm: &mut Vm, gui: &Gui, id: ObjId, ev: GuiEvent) -> Result<(), VmError> {
    if is_a(vm, id, "GuiArrayCtrl") {
        crate::textlist::mouse_enter(vm, gui, id, ev.point);
        return Ok(());
    }
    if is_a(vm, id, "GuiButtonBaseCtrl") {
        // Captured: depressed again; else (active) the over sound; over; then the script `onMouseEnter`.
        let captured = input(gui).capture == Some(id);
        if captured {
            gui.with_mut(id, |c| c.depressed = true);
        } else if active(gui, id) {
            button_sound(vm, gui, id, b"soundButtonOver");
        }
        gui.with_mut(id, |c| c.mouse_over = true);
        return vm.call_method(id, b"onMouseEnter", &[]).map(|_| ());
    }
    if is_a(vm, id, "GuiMouseEventCtrl") {
        return mouse_event(vm, id, b"onMouseEnter", ev);
    }
    Ok(())
}

fn on_mouse_leave(vm: &mut Vm, gui: &Gui, id: ObjId, ev: GuiEvent) -> Result<(), VmError> {
    if is_a(vm, id, "GuiArrayCtrl") {
        crate::textlist::mouse_leave(gui, id);
        return Ok(());
    }
    if is_a(vm, id, "GuiButtonBaseCtrl") {
        // Captured: neither depressed nor over; else not over.
        let captured = input(gui).capture == Some(id);
        gui.with_mut(id, |c| {
            if captured {
                c.depressed = false;
            }
            c.mouse_over = false;
        });
        return Ok(());
    }
    if is_a(vm, id, "GuiMouseEventCtrl") {
        return mouse_event(vm, id, b"onMouseLeave", ev);
    }
    Ok(())
}

fn on_right_mouse_down(vm: &mut Vm, gui: &Gui, id: ObjId, ev: GuiEvent) -> Result<(), VmError> {
    // The buttons: the left button's handler.
    if is_a(vm, id, "GuiButtonBaseCtrl") {
        return on_mouse_down(vm, gui, id, ev);
    }
    if is_a(vm, id, "GuiMouseEventCtrl") {
        return mouse_event(vm, id, b"onRightMouseDown", ev);
    }
    Ok(())
}

fn on_right_mouse_up(vm: &mut Vm, gui: &Gui, id: ObjId, ev: GuiEvent) -> Result<(), VmError> {
    if is_a(vm, id, "GuiButtonBaseCtrl") {
        // Not the left button's: still depressed, active and enabled, a button WITH a command runs its altCommand (with
        // `$ThisControl`; none runs nothing) and one without calls the script's `onAltAction`: a right click on a button with a
        // command and no altCommand runs nothing.
        if !active(gui, id) || !enabled(vm, id) {
            return Ok(());
        }
        mouse_unlock(vm, gui, id)?;
        let was = gui.with(id, |c| c.depressed).unwrap_or(false);
        if was && active(gui, id) && enabled(vm, id) {
            if text_field(vm, id, b"command").is_empty() {
                vm.call_method(id, b"onAltAction", &[])?;
            } else {
                let alt = text_field(vm, id, b"altCommand");
                exec_command(vm, id, &alt)?;
            }
        }
        gui.with_mut(id, |c| c.depressed = false);
        return Ok(());
    }
    if is_a(vm, id, "GuiMouseEventCtrl") {
        return mouse_event(vm, id, b"onRightMouseUp", ev);
    }
    Ok(())
}

fn on_right_mouse_dragged(vm: &mut Vm, _gui: &Gui, id: ObjId, ev: GuiEvent) -> Result<(), VmError> {
    if is_a(vm, id, "GuiMouseEventCtrl") {
        return mouse_event(vm, id, b"onRightMouseDragged", ev);
    }
    Ok(())
}

/// The wheel (`onMouseWheelUp`/`Down`): a GuiControl passes it to its parent; a GuiScrollCtrl scrolls (a row, a page with
/// Ctrl), then gives each child an `onMouseMove` (a list's mouse-over cell follows). The canvas never
/// consumes it, so the maps see it.
fn on_wheel(vm: &mut Vm, gui: &Gui, id: ObjId, ev: GuiEvent, up: bool) -> Result<(), VmError> {
    let mut at = Some(id);
    let canvas = canvas_of(vm, gui);
    for _ in 0..MAX_TREE_DEPTH {
        let Some(c) = at.filter(|c| alive(vm, *c) && Some(*c) != canvas) else {
            return Ok(());
        };
        if !vm.charge_work(16) {
            return Ok(());
        }
        if is_a(vm, c, "GuiScrollCtrl") {
            if crate::scroll::wheel(vm, gui, c, up, ev.modifier) {
                for child in children(vm, c) {
                    if alive(vm, child) {
                        on_mouse_move(vm, gui, child, ev)?;
                    }
                }
            }
            return Ok(());
        }
        if !visible(vm, c) || !awake(gui, c) {
            return Ok(());
        }
        at = parent(vm, c);
    }
    Ok(())
}

/// The scroll control the wheel would reach now (the mouse's capture, else the control under the mouse, then its parents while
/// they are visible and awake): `take` of it, and when it took it, each of its children an `onMouseMove`, as for a notch.
/// Whether one was reached.
pub fn wheel_route(
    vm: &mut Vm,
    gui: &Gui,
    take: &mut dyn FnMut(&mut Vm, &Gui, ObjId) -> bool,
) -> Result<bool, VmError> {
    let pt = gui.0.borrow().cursor_pos;
    input(gui).last.point = pt;
    let gev = input(gui).last;
    let capture = input(gui).capture.filter(|c| alive(vm, *c));
    let start = match capture {
        Some(c) => Some(c),
        None => {
            find_mouse_control(vm, gui, gev)?;
            peek(gui, |i| i.mouse_control).filter(|m| alive(vm, *m))
        }
    };
    let canvas = canvas_of(vm, gui);
    let mut at = start;
    for _ in 0..MAX_TREE_DEPTH {
        let Some(c) = at.filter(|c| alive(vm, *c) && Some(*c) != canvas) else {
            break;
        };
        if !vm.charge_work(16) {
            break;
        }
        if is_a(vm, c, "GuiScrollCtrl") {
            if take(vm, gui, c) {
                for child in children(vm, c) {
                    if alive(vm, child) {
                        on_mouse_move(vm, gui, child, gev)?;
                    }
                }
            }
            return Ok(true);
        }
        if !visible(vm, c) || !awake(gui, c) {
            break;
        }
        at = parent(vm, c);
    }
    Ok(false)
}

/// GuiMouseEventCtrl's callbacks: `(modifier, "x y", clicks)`, the point the canvas's.
fn mouse_event(vm: &mut Vm, id: ObjId, name: &[u8], ev: GuiEvent) -> Result<(), VmError> {
    vm.call_method(
        id,
        name,
        &[
            ev.modifier.to_string().into_bytes(),
            format!("{} {}", ev.point.0, ev.point.1).into_bytes(),
            ev.clicks.to_string().into_bytes(),
        ],
    )
    .map(|_| ())
}

// ---- the controls' key handlers

/// `GuiControl::onKeyDown`: to the parent (the canvas answers false).
fn parent_key_down(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    ev: GuiEvent,
    depth: usize,
) -> Result<bool, VmError> {
    match parent(vm, id) {
        Some(p) if Some(p) != canvas_of(vm, gui) => on_key_down_at(vm, gui, p, ev, depth + 1),
        _ => Ok(false),
    }
}

fn parent_key_up(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    ev: GuiEvent,
    depth: usize,
) -> Result<bool, VmError> {
    match parent(vm, id) {
        Some(p) if Some(p) != canvas_of(vm, gui) => on_key_up_at(vm, gui, p, ev, depth + 1),
        _ => Ok(false),
    }
}

pub(crate) fn on_key_down(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    ev: GuiEvent,
) -> Result<bool, VmError> {
    on_key_down_at(vm, gui, id, ev, 0)
}

fn on_key_down_at(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    ev: GuiEvent,
    depth: usize,
) -> Result<bool, VmError> {
    if depth > MAX_TREE_DEPTH || !vm.charge_work(32) || !alive(vm, id) {
        return Ok(false);
    }
    if is_a(vm, id, "GuiTextEditCtrl") {
        return text_edit::key_down(vm, gui, id, ev, depth);
    }
    if is_a(vm, id, "GuiTextListCtrl") {
        let k = u16::from(ev.key_code);
        if let Some(taken) = crate::popup::list_key_down(vm, gui, id, k, ev.modifier)? {
            return Ok(taken);
        }
        if let Some(taken) = crate::textlist::key_down(vm, gui, id, k)? {
            return Ok(taken);
        }
        return parent_key_down(vm, gui, id, ev, depth);
    }
    if is_a(vm, id, "GuiButtonBaseCtrl") {
        // Return or Space with no modifier: the down sound, consumed; else to the parent. Inactive: consumed.
        if !active(gui, id) || !enabled(vm, id) {
            return Ok(true);
        }
        let k = u16::from(ev.key_code);
        if (k == KEY_RETURN || k == KEY_SPACE) && ev.modifier == 0 {
            button_sound(vm, gui, id, b"soundButtonDown");
            if gui.ext().key_press(vm, id) {
                gui.with_mut(id, |c| c.depressed = true);
            }
            return Ok(true);
        }
        return parent_key_down(vm, gui, id, ev, depth);
    }
    parent_key_down(vm, gui, id, ev, depth)
}

fn on_key_up_at(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    ev: GuiEvent,
    depth: usize,
) -> Result<bool, VmError> {
    if depth > MAX_TREE_DEPTH || !vm.charge_work(32) || !alive(vm, id) {
        return Ok(false);
    }
    if is_a(vm, id, "GuiButtonBaseCtrl") {
        // Inactive: consumed; depressed and Return or Space with no modifier: the action; else to the parent.
        if !active(gui, id) || !enabled(vm, id) {
            return Ok(true);
        }
        let k = u16::from(ev.key_code);
        if gui.with(id, |c| c.depressed).unwrap_or(false)
            && (k == KEY_RETURN || k == KEY_SPACE)
            && ev.modifier == 0
        {
            if gui.ext().key_press(vm, id) {
                gui.with_mut(id, |c| c.depressed = false);
            }
            on_action(vm, gui, id)?;
            return Ok(true);
        }
        return parent_key_up(vm, gui, id, ev, depth);
    }
    parent_key_up(vm, gui, id, ev, depth)
}

/// `onKeyRepeat`: what the control's `onKeyDown` does (a text edit's repeats insert once a make was seen).
fn on_key_repeat(vm: &mut Vm, gui: &Gui, id: ObjId, ev: GuiEvent) -> Result<bool, VmError> {
    on_key_down(vm, gui, id, ev)
}

/// `onInputEvent`: GuiInputCtrl's (TGE 1.4 `guiInputCtrl.cc`): a make of a button or a key that is no modifier
/// key calls `onInputEvent(device, action, 1)`; a modifier key's break `onInputEvent("keyboard", key, 0)`; both consumed.
fn on_input_event(vm: &mut Vm, gui: &Gui, id: ObjId, ev: &InputEvent) -> Result<bool, VmError> {
    if !is_a(vm, id, "GuiInputCtrl") {
        return Ok(false);
    }
    let modifier_key = |k: u16| {
        matches!(
            k,
            KEY_LALT
                | KEY_RALT
                | KEY_LSHIFT
                | KEY_RSHIFT
                | KEY_LCONTROL
                | KEY_RCONTROL
                | KEY_ALT
                | KEY_SHIFT
                | KEY_CONTROL
        )
    };
    if ev.action == SI_MAKE {
        if ev.obj_type == SI_BUTTON || (ev.obj_type == SI_KEY && !modifier_key(ev.obj_inst)) {
            let Some(device) = keys::device_name(ev.device, ev.device_inst) else {
                return Ok(false);
            };
            let Some(key) = keys::key_string_in(&gui.ext().key_tables(), ev.obj_inst) else {
                return Ok(false);
            };
            let action = format!("{}{key}", keys::modifier_string(ev.modifier));
            vm.call_method(
                id,
                b"onInputEvent",
                &[device.into_bytes(), action.into_bytes(), b"1".to_vec()],
            )?;
            return Ok(true);
        }
    } else if ev.action == SI_BREAK && ev.obj_type == SI_KEY && modifier_key(ev.obj_inst) {
        let Some(key) = keys::key_string_in(&gui.ext().key_tables(), ev.obj_inst) else {
            return Ok(false);
        };
        vm.call_method(
            id,
            b"onInputEvent",
            &[b"keyboard".to_vec(), key.into_bytes(), b"0".to_vec()],
        )?;
        return Ok(true);
    }
    Ok(false)
}

// ---- the canvas

/// `GuiCanvas::processInputEvent`: whether the canvas consumed the event (see the module documentation).
pub fn process_input_event(vm: &mut Vm, gui: &Gui, ev: &InputEvent) -> Result<bool, VmError> {
    let Some(canvas) = canvas_of(vm, gui) else {
        return Ok(false);
    };
    if let Some(fr) = peek(gui, |i| i.first_responder).filter(|f| alive(vm, *f))
        && on_input_event(vm, gui, fr, ev)?
    {
        return Ok(true);
    }
    if ev.device == KEYBOARD_DEVICE {
        let gev = {
            let mut i = input(gui);
            i.last.ascii = ev.ascii;
            i.last.modifier = ev.modifier;
            i.last.key_code = ev.obj_inst as u8;
            i.last.action = ev.action;
            i.last
        };
        let modifier = keys::normalise_modifiers(ev.modifier) & (SI_SHIFT | SI_CTRL | SI_ALT);
        let fr = input(gui).first_responder.filter(|f| alive(vm, *f));
        match ev.action {
            SI_MAKE => {
                if let Some(f) = fr
                    && on_key_down(vm, gui, f, gev)?
                {
                    return Ok(true);
                }
                let cursor_on = gui.0.borrow().cursor_on;
                if cursor_on && ev.obj_inst == KEY_TAB && !children(vm, canvas).is_empty() {
                    if ev.modifier & SI_SHIFT != 0 {
                        tab_prev(vm, gui)?;
                        return Ok(true);
                    }
                    if ev.modifier == 0 {
                        tab_next(vm, gui)?;
                        return Ok(true);
                    }
                }
                return accelerator(vm, gui, ev.obj_inst, modifier, true);
            }
            SI_BREAK => {
                if let Some(f) = fr
                    && on_key_up_at(vm, gui, f, gev, 0)?
                {
                    return Ok(true);
                }
                return accelerator(vm, gui, ev.obj_inst, modifier, false);
            }
            SI_REPEAT => {
                if let Some(f) = fr {
                    on_key_repeat(vm, gui, f, gev)?;
                }
                return Ok(true);
            }
            _ => return Ok(false),
        }
    }
    let cursor_on = gui.0.borrow().cursor_on;
    if ev.device != MOUSE_DEVICE || !cursor_on {
        return Ok(false);
    }
    input(gui).last.modifier = ev.modifier;
    match ev.obj_type {
        SI_XAXIS | SI_YAXIS => {
            let (old, size) = {
                let s = gui.0.borrow();
                (s.cursor_pos, s.canvas_size)
            };
            let mut pt = old;
            if ev.obj_type == SI_XAXIS {
                pt.0 = clamp_axis(old.0 as f32 + ev.value, size.0);
                if pt.0 == old.0 {
                    return Ok(true);
                }
            } else {
                pt.1 = clamp_axis(old.1 as f32 + ev.value, size.1);
                if pt.1 == old.1 {
                    return Ok(true);
                }
            }
            gui.0.borrow_mut().cursor_pos = pt;
            let (left, right, capture) = {
                let mut i = input(gui);
                i.last.point = pt;
                (i.left_down, i.right_down, i.capture)
            };
            let gev = input(gui).last;
            let capture = capture.filter(|c| alive(vm, *c));
            if right && !left {
                input(gui).prev_mouse_time = now(vm);
            }
            match capture {
                Some(c) => {
                    check_lock_mouse_move(vm, gui, gev)?;
                    if left {
                        on_mouse_dragged(vm, gui, c, gev)?;
                    } else if right {
                        on_right_mouse_dragged(vm, gui, c, gev)?;
                    } else {
                        on_mouse_move(vm, gui, c, gev)?;
                    }
                }
                None => {
                    find_mouse_control(vm, gui, gev)?;
                    if let Some(m) = peek(gui, |i| i.mouse_control).filter(|m| alive(vm, *m)) {
                        if left {
                            on_mouse_dragged(vm, gui, m, gev)?;
                        } else if right {
                            on_right_mouse_dragged(vm, gui, m, gev)?;
                        } else {
                            on_mouse_move(vm, gui, m, gev)?;
                        }
                    }
                }
            }
            Ok(true)
        }
        SI_ZAXIS => {
            let pt = gui.0.borrow().cursor_pos;
            input(gui).last.point = pt;
            let gev = input(gui).last;
            let up = ev.value >= 0.0;
            let capture = input(gui).capture.filter(|c| alive(vm, *c));
            match capture {
                Some(c) => on_wheel(vm, gui, c, gev, up)?,
                None => {
                    find_mouse_control(vm, gui, gev)?;
                    if let Some(m) = peek(gui, |i| i.mouse_control).filter(|m| alive(vm, *m)) {
                        on_wheel(vm, gui, m, gev, up)?;
                    }
                }
            }
            Ok(false)
        }
        SI_BUTTON => {
            let pt = gui.0.borrow().cursor_pos;
            input(gui).last.point = pt;
            if ev.action == SI_MAKE {
                gui.ext().mouse_pressed(gui);
            }
            match ev.obj_inst {
                KEY_BUTTON0 => left_button(vm, gui, canvas, ev.action == SI_MAKE),
                KEY_BUTTON1 => right_button(vm, gui, ev.action == SI_MAKE),
                _ => Ok(false),
            }
        }
        _ => Ok(false),
    }
}

fn clamp_axis(v: f32, extent: i32) -> i32 {
    // `ftol` truncates toward zero; the canvas keeps the cursor inside [0, extent - 1].
    let t = v as i32;
    t.min(extent.wrapping_sub(1)).max(0)
}

fn accelerator(
    vm: &mut Vm,
    gui: &Gui,
    key: u16,
    modifier: u8,
    press: bool,
) -> Result<bool, VmError> {
    let found = input(gui)
        .accelerators
        .iter()
        .find(|a| {
            a.key == key
                && keys::normalise_modifiers(a.modifier) & (SI_SHIFT | SI_CTRL | SI_ALT) == modifier
        })
        .copied();
    let Some(a) = found else {
        return Ok(false);
    };
    if alive(vm, a.control) {
        if press {
            accelerator_press(vm, gui, a.control)?;
        } else {
            accelerator_release(vm, gui, a.control)?;
        }
    }
    Ok(true)
}

fn left_button(vm: &mut Vm, gui: &Gui, canvas: ObjId, make: bool) -> Result<bool, VmError> {
    let t = now(vm);
    let capture = input(gui).capture.filter(|c| alive(vm, *c));
    if !make {
        let gev = {
            let mut i = input(gui);
            i.prev_mouse_time = t;
            i.left_down = false;
            i.last
        };
        match capture {
            Some(c) => on_mouse_up(vm, gui, c, gev)?,
            None => {
                find_mouse_control(vm, gui, gev)?;
                if let Some(m) = peek(gui, |i| i.mouse_control).filter(|m| alive(vm, *m)) {
                    on_mouse_up(vm, gui, m, gev)?;
                }
            }
        }
        return Ok(true);
    }
    let gev = {
        let mut i = input(gui);
        if !i.left_last {
            i.left_last = true;
            i.click_count = 1;
        } else if t.wrapping_sub(i.last_down_time) > LEFT_DOUBLE_CLICK_MS {
            i.click_count = 1;
        } else {
            i.click_count = i.click_count.wrapping_add(1);
        }
        i.last_down_time = t;
        i.last.clicks = i.click_count;
        i.prev_mouse_time = t;
        i.left_down = true;
        i.last
    };
    match capture {
        Some(c) => on_mouse_down(vm, gui, c, gev)?,
        None => {
            let mut target = None;
            for child in children(vm, canvas).into_iter().rev() {
                if !alive(vm, child) || !vm.charge_work(16) {
                    continue;
                }
                // `findHitControl(mousePoint)` of the child in the canvas's coordinates (a canvas child sits at 0,0).
                let h = hit(vm, gui, child, gev.point);
                if active(gui, h) || profile_flag(vm, h, b"modal") {
                    target = Some(h);
                    break;
                }
            }
            if let Some(h) = target {
                select_window(vm, h);
                on_mouse_down(vm, gui, h, gev)?;
            }
        }
    }
    if peek(gui, |i| i.mouse_control.is_some()) {
        input(gui).mouse_control_clicked = true;
    }
    Ok(true)
}

fn right_button(vm: &mut Vm, gui: &Gui, make: bool) -> Result<bool, VmError> {
    let t = now(vm);
    let capture = input(gui).capture.filter(|c| alive(vm, *c));
    let gev = {
        let mut i = input(gui);
        if make {
            if i.left_last {
                i.left_last = false;
                i.click_count = 1;
            } else if t.wrapping_sub(i.last_down_time) > RIGHT_DOUBLE_CLICK_MS {
                i.click_count = 1;
            } else {
                i.click_count = i.click_count.wrapping_add(1);
            }
            i.last_down_time = t;
            i.last.clicks = i.click_count;
        }
        i.prev_mouse_time = t;
        i.right_down = make;
        i.last
    };
    match capture {
        Some(c) => {
            if make {
                on_right_mouse_down(vm, gui, c, gev)?;
            } else {
                on_right_mouse_up(vm, gui, c, gev)?;
            }
        }
        None => {
            find_mouse_control(vm, gui, gev)?;
            if let Some(m) = peek(gui, |i| i.mouse_control).filter(|m| alive(vm, *m)) {
                if make {
                    on_right_mouse_down(vm, gui, m, gev)?;
                } else {
                    on_right_mouse_up(vm, gui, m, gev)?;
                }
            }
        }
    }
    Ok(true)
}

/// Before a mouse down: the GuiWindowCtrl the control is in comes to the front of its parent.
fn select_window(vm: &mut Vm, id: ObjId) {
    let mut at = id;
    for _ in 0..=MAX_TREE_DEPTH {
        let Some(p) = parent(vm, at) else {
            return;
        };
        if class_name(vm, p).eq_ignore_ascii_case(b"GuiWindowCtrl") {
            if let Some(grand) = parent(vm, p) {
                let kids = children(vm, grand);
                if kids.contains(&p) && kids.last() != Some(&p) {
                    vm.objects_mut().move_member(grand, p, false);
                }
            }
            return;
        }
        at = p;
    }
}

/// The window's mouse move: its position `(x, y)` as an X and a Y axis event of the difference, to the canvas only.
pub fn process_mouse_move(
    vm: &mut Vm,
    gui: &Gui,
    pt: (i32, i32),
    modifier: u8,
) -> Result<(), VmError> {
    let old = gui.0.borrow().cursor_pos;
    let mut ev = InputEvent {
        device: MOUSE_DEVICE,
        obj_type: SI_XAXIS,
        action: SI_MOVE,
        modifier,
        value: pt.0 as f32 - old.0 as f32,
        ..InputEvent::default()
    };
    process_input_event(vm, gui, &ev)?;
    let now_at = gui.0.borrow().cursor_pos;
    ev.obj_type = SI_YAXIS;
    ev.value = pt.1 as f32 - now_at.1 as f32;
    process_input_event(vm, gui, &ev)?;
    Ok(())
}

/// The canvas's part when a control leaves the model (deleted): every reference to it is forgotten (its `SimObjectPtr`s).
pub fn forget(gui: &Gui, id: ObjId) {
    let mut s = gui.0.borrow_mut();
    let i = &mut s.input;
    for slot in [
        &mut i.first_responder,
        &mut i.capture,
        &mut i.mouse_control,
        &mut i.tab_cur,
        &mut i.tab_prev,
    ] {
        if *slot == Some(id) {
            *slot = None;
        }
    }
    i.accelerators.retain(|a| a.control != id);
    for c in s.controls.values_mut() {
        if c.first_responder == Some(id) {
            c.first_responder = None;
        }
    }
}

/// `GuiControl::onSleep`'s input part (`clearFirstResponder`, `mouseUnlock`), before the script's `onSleep`.
pub fn on_sleep(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    clear_first_responder(vm, gui, id);
    mouse_unlock(vm, gui, id)
}

/// `GuiControl::onWake`'s input part: no remembered first responder: its first tabable; a tabable control whose profile
/// `canKeyFocus` becomes its parents'.
pub fn on_wake(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    if gui
        .with(id, |c| c.first_responder.is_none())
        .unwrap_or(false)
    {
        let found = find_first_tabable(vm, gui, id);
        gui.with_mut(id, |c| c.first_responder = found);
    }
    if profile_flag(vm, id, b"tab")
        && profile_flag(vm, id, b"canKeyFocus")
        && awake(gui, id)
        && visible(vm, id)
        && let Some(p) = parent(vm, id)
    {
        set_first_responder_of(vm, gui, p, id, 0)?;
    }
    Ok(())
}

/// The caret of a first-responder text edit, per drawn frame: it flips after more than [`CARET_FLIP_MS`] and more than
/// [`CARET_FLIP_FRAMES`] frames. Whether it is shown now.
pub fn caret_frame(vm: &Vm, gui: &Gui, id: ObjId) -> bool {
    let t = now(vm);
    gui.with_mut(id, |c| {
        let Some(e) = c.edit.as_mut() else {
            return false;
        };
        e.caret_frames = e.caret_frames.saturating_add(1);
        if t.wrapping_sub(e.caret_flipped) > CARET_FLIP_MS && e.caret_frames > CARET_FLIP_FRAMES {
            e.caret_on = !e.caret_on;
            e.caret_frames = 0;
            e.caret_flipped = t;
        }
        e.caret_on
    })
    .unwrap_or(false)
}

/// The host's entry for one input event: under a host budget of its own; the error a callback ended with, if any.
pub fn host_event(vm: &mut Vm, gui: &Gui, ev: &InputEvent) -> (bool, Option<VmError>) {
    let saved = vm.begin_host_budget_with_runs(MAX_EVENT_WORK, MAX_EVENT_STEPS);
    let result = process_input_event(vm, gui, ev);
    vm.end_host_budget(saved);
    match result {
        Ok(consumed) => (consumed, None),
        Err(e) => (true, Some(e)),
    }
}

/// The host's entry for one absolute mouse position.
pub fn host_mouse_move(vm: &mut Vm, gui: &Gui, pt: (i32, i32), modifier: u8) -> Option<VmError> {
    let saved = vm.begin_host_budget_with_runs(MAX_EVENT_WORK, MAX_EVENT_STEPS);
    let result = process_mouse_move(vm, gui, pt, modifier);
    vm.end_host_budget(saved);
    result.err()
}

pub mod text_edit;
