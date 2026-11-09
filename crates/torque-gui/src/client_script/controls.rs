//! The `ClientScript*` control classes. They are the only controls a realm makes: each is handled by its class alone (draw,
//! mouse, keys), keeps what the realm set in its state rather than in console fields, and answers the player by queueing an
//! event for the realm. None of them runs a command, reads a variable or calls a script method.

use torque_client_script_format::is_hidden;
use torque_client_script_format::ui::{Kind, Prop, PropValue};
use torque_engine::EngineClass;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use super::events::{Changed, UiEvent};
use super::layers::effectively_visible;
use super::paint::{Laid, draw_node, row_height};
use super::props::Props;
use crate::control::{self, children};
use crate::custom::{ControlCx, GuiControlClass, KeyEvent, Place, PointerEvent};
use crate::draw::{DrawCtx, RealmImage};
use crate::keys::{
    KEY_BACKSPACE, KEY_DELETE, KEY_END, KEY_ESCAPE, KEY_HOME, KEY_LEFT, KEY_NUMPADENTER,
    KEY_RETURN, KEY_RIGHT, SI_ALT, SI_CTRL, normalise_modifiers,
};
use crate::{Gui, MAX_TREE_DEPTH};

/// How far one wheel notch moves a scrolling area.
const WHEEL_STEP: i32 = 40;
/// The least alpha of a panel's fill, a text's color or an image's tint at which acting on the control is the player's doing.
const MIN_SHOWN_ALPHA: u8 = 64;

/// What a realm control keeps: its kind (none for a mod root), the props the realm set, and the player's doing.
#[derive(Debug, Default)]
pub(crate) struct NodeState {
    /// The realm's handle for the control; the events name it.
    pub(super) handle: u32,
    pub(super) kind: Option<Kind>,
    pub(super) props: Props,
    pub(super) hover: bool,
    pub(super) pressed: bool,
    /// A scrolling area's or a list's offset, in whole units.
    pub(super) scroll: (i32, i32),
    /// A text field's caret, in characters.
    pub(super) caret: usize,
    /// What the server's store gave for the `image` prop.
    pub(super) image: Option<RealmImage>,
    /// The text laid out in lines, kept until the text, the font or the width changes.
    pub(super) laid: Option<Laid>,
}

impl NodeState {
    pub(super) fn new(handle: u32, kind: Option<Kind>) -> NodeState {
        NodeState {
            handle,
            kind,
            ..NodeState::default()
        }
    }

    /// Takes the props the realm set, and the image the store gave for them.
    pub(super) fn set_props(&mut self, props: Props, image: Option<RealmImage>) {
        self.props = props;
        self.image = image;
        self.laid = None;
    }

    /// Whether the control draws something the player sees, so that acting on it is the player's doing: a panel's fill, a
    /// text, an image, or a control with a look of its own (a button, toggle, field, list or slider).
    fn shows_something(&self) -> bool {
        let props = &self.props;
        match self.kind {
            Some(Kind::Panel) => props.color(Prop::Fill)[3] >= MIN_SHOWN_ALPHA,
            Some(Kind::Text) => {
                !props.text(Prop::Text).trim().is_empty()
                    && props.color(Prop::Color)[3] >= MIN_SHOWN_ALPHA
            }
            Some(Kind::RichText) => !props.text(Prop::RichText).trim().is_empty(),
            Some(Kind::Image) => {
                self.image.is_some() && props.color(Prop::Tint)[3] >= MIN_SHOWN_ALPHA
            }
            Some(Kind::Button | Kind::Toggle | Kind::TextField | Kind::List | Kind::Slider) => true,
            Some(Kind::Scroll | Kind::Progress) | None => false,
        }
    }

    fn child_shift(&self) -> (i32, i32) {
        match self.kind {
            Some(Kind::Scroll) => (-self.scroll.0, -self.scroll.1),
            _ => (0, 0),
        }
    }

    /// The most the control scrolls on y, given its own height.
    fn scroll_limit(&self, height: i32) -> i32 {
        let content = match self.kind {
            Some(Kind::List) => {
                self.props.rows().len() as i32 * row_height(self.props.num(Prop::FontSize) as i32)
            }
            _ => self.props.vec2(Prop::ContentSize).1 as i32,
        };
        (content - height).max(0)
    }
}

fn emit(gui: &Gui, event: UiEvent) {
    gui.0.borrow_mut().client_script.emit(event);
}

/// Whether the control `cx` is for, whose own opacity is `st`'s, is on screen to be seen ([`effectively_visible`]).
fn seen(cx: &ControlCx<'_>, st: &NodeState) -> bool {
    effectively_visible(cx.vm, cx.gui, cx.id, Some(st.props.num(Prop::Opacity)))
}

/// Queues `event` from the control `cx` is for. An activating event counts as the player's action, which lets `ui.focus`
/// through, only when the control can be seen and draws something: the realm cannot earn focus by clicks on something
/// invisible or clear.
fn emit_from(cx: &ControlCx<'_>, st: &NodeState, event: UiEvent) {
    let acting = event.is_activation() && st.shows_something() && seen(cx, st);
    let l = &mut cx.gui.0.borrow_mut().client_script;
    if acting {
        l.activate();
    }
    l.emit(event);
}

/// Implements [`GuiControlClass`] for a class whose controls keep a [`NodeState`].
macro_rules! node_behavior {
    ($ty:ident) => {
        impl GuiControlClass for $ty {
            type State = NodeState;

            fn draw(&self, st: &mut NodeState, d: &mut DrawCtx<'_, '_>, at: &Place) {
                draw_node(st, d, at);
            }

            fn child_shift(st: &NodeState) -> (i32, i32) {
                st.child_shift()
            }

            fn mouse_down(
                &self,
                st: &mut NodeState,
                cx: &mut ControlCx<'_>,
                ev: &PointerEvent,
            ) -> Result<(), VmError> {
                mouse_down(st, cx, ev);
                Ok(())
            }

            fn mouse_up(
                &self,
                st: &mut NodeState,
                cx: &mut ControlCx<'_>,
                ev: &PointerEvent,
            ) -> Result<(), VmError> {
                mouse_up(st, cx, ev);
                Ok(())
            }

            fn mouse_dragged(
                &self,
                st: &mut NodeState,
                cx: &mut ControlCx<'_>,
                ev: &PointerEvent,
            ) -> Result<(), VmError> {
                if st.kind == Some(Kind::Slider) && st.pressed {
                    slide(st, cx, ev.local.0);
                }
                Ok(())
            }

            fn mouse_enter(
                &self,
                st: &mut NodeState,
                cx: &mut ControlCx<'_>,
                _ev: &PointerEvent,
            ) -> Result<(), VmError> {
                hover(st, cx, true);
                Ok(())
            }

            fn mouse_leave(
                &self,
                st: &mut NodeState,
                cx: &mut ControlCx<'_>,
                _ev: &PointerEvent,
            ) -> Result<(), VmError> {
                hover(st, cx, false);
                st.pressed = false;
                Ok(())
            }

            fn key_down(
                &self,
                st: &mut NodeState,
                cx: &mut ControlCx<'_>,
                ev: &KeyEvent,
            ) -> Result<bool, VmError> {
                Ok(key_down(st, cx, ev))
            }
        }
    };
}

/// The mod root a realm makes for each mod: a container over the server area.
#[derive(EngineClass)]
#[class(name = "ClientScriptRoot", parent = "GuiControl")]
pub(crate) struct RootCtrl {}
node_behavior!(RootCtrl);

/// A `panel`.
#[derive(EngineClass)]
#[class(name = "ClientScriptPanel", parent = "GuiControl")]
pub(crate) struct PanelCtrl {}
node_behavior!(PanelCtrl);

/// A `text`.
#[derive(EngineClass)]
#[class(name = "ClientScriptText", parent = "GuiControl")]
pub(crate) struct TextCtrl {}
node_behavior!(TextCtrl);

/// A `richtext`.
#[derive(EngineClass)]
#[class(name = "ClientScriptRichText", parent = "GuiControl")]
pub(crate) struct RichTextCtrl {}
node_behavior!(RichTextCtrl);

/// An `image`.
#[derive(EngineClass)]
#[class(name = "ClientScriptImage", parent = "GuiControl")]
pub(crate) struct ImageCtrl {}
node_behavior!(ImageCtrl);

/// A `button`.
#[derive(EngineClass)]
#[class(name = "ClientScriptButton", parent = "GuiControl")]
pub(crate) struct ButtonCtrl {}
node_behavior!(ButtonCtrl);

/// A `toggle`.
#[derive(EngineClass)]
#[class(name = "ClientScriptToggle", parent = "GuiControl")]
pub(crate) struct ToggleCtrl {}
node_behavior!(ToggleCtrl);

/// A `textfield`.
#[derive(EngineClass)]
#[class(name = "ClientScriptTextField", parent = "GuiControl")]
pub(crate) struct TextFieldCtrl {}
node_behavior!(TextFieldCtrl);

/// A `list`.
#[derive(EngineClass)]
#[class(name = "ClientScriptList", parent = "GuiControl")]
pub(crate) struct ListCtrl {}
node_behavior!(ListCtrl);

/// A `scroll`.
#[derive(EngineClass)]
#[class(name = "ClientScriptScroll", parent = "GuiControl")]
pub(crate) struct ScrollCtrl {}
node_behavior!(ScrollCtrl);

/// A `slider`.
#[derive(EngineClass)]
#[class(name = "ClientScriptSlider", parent = "GuiControl")]
pub(crate) struct SliderCtrl {}
node_behavior!(SliderCtrl);

/// A `progress`.
#[derive(EngineClass)]
#[class(name = "ClientScriptProgress", parent = "GuiControl")]
pub(crate) struct ProgressCtrl {}
node_behavior!(ProgressCtrl);

/// The class of controls of `kind` (`None`: a mod root).
pub(crate) fn class_name(kind: Option<Kind>) -> &'static str {
    match kind {
        None => "ClientScriptRoot",
        Some(Kind::Panel) => "ClientScriptPanel",
        Some(Kind::Text) => "ClientScriptText",
        Some(Kind::RichText) => "ClientScriptRichText",
        Some(Kind::Image) => "ClientScriptImage",
        Some(Kind::Button) => "ClientScriptButton",
        Some(Kind::Toggle) => "ClientScriptToggle",
        Some(Kind::TextField) => "ClientScriptTextField",
        Some(Kind::List) => "ClientScriptList",
        Some(Kind::Scroll) => "ClientScriptScroll",
        Some(Kind::Slider) => "ClientScriptSlider",
        Some(Kind::Progress) => "ClientScriptProgress",
    }
}

fn enabled(st: &NodeState) -> bool {
    st.props.flag(Prop::Enabled)
}

fn inside(local: (i32, i32), extent: (i32, i32)) -> bool {
    (0..extent.0).contains(&local.0) && (0..extent.1).contains(&local.1)
}

fn mouse_down(st: &mut NodeState, cx: &mut ControlCx<'_>, ev: &PointerEvent) {
    let Some(kind) = st.kind else {
        return;
    };
    if !enabled(st) {
        return;
    }
    match kind {
        Kind::Panel | Kind::Text | Kind::Image | Kind::Button | Kind::Toggle => st.pressed = true,
        Kind::TextField => {
            if seen(cx, st) {
                let l = &mut cx.gui.0.borrow_mut().client_script;
                l.focus = Some(cx.id);
                l.activate();
                st.caret = st.props.text(Prop::Text).chars().count();
            }
        }
        Kind::List => pick_row(st, cx, ev.local.1),
        Kind::Slider => {
            st.pressed = true;
            slide(st, cx, ev.local.0);
        }
        Kind::RichText | Kind::Scroll | Kind::Progress => {}
    }
}

fn mouse_up(st: &mut NodeState, cx: &mut ControlCx<'_>, ev: &PointerEvent) {
    let was = std::mem::take(&mut st.pressed);
    let Some(kind) = st.kind else {
        return;
    };
    if !was || !enabled(st) || !inside(ev.local, cx.bounds().1) {
        return;
    }
    match kind {
        Kind::Panel | Kind::Text | Kind::Image => emit_from(cx, st, UiEvent::Click(st.handle)),
        Kind::Button => emit_from(cx, st, UiEvent::Press(st.handle)),
        Kind::Toggle => {
            let on = !st.props.flag(Prop::On);
            st.props.set(Prop::On, PropValue::Bool(on));
            emit_from(cx, st, UiEvent::Change(st.handle, Changed::Bool(on)));
        }
        Kind::RichText
        | Kind::TextField
        | Kind::List
        | Kind::Scroll
        | Kind::Slider
        | Kind::Progress => {}
    }
}

fn hover(st: &mut NodeState, cx: &ControlCx<'_>, on: bool) {
    st.hover = on;
    if matches!(st.kind, Some(Kind::Panel | Kind::Image)) && enabled(st) {
        emit(cx.gui, UiEvent::Hover(st.handle, on));
    }
}

fn pick_row(st: &mut NodeState, cx: &ControlCx<'_>, y: i32) {
    let rh = row_height(st.props.num(Prop::FontSize).round() as i32);
    let row = (y + st.scroll.1).max(0) / rh.max(1);
    if usize::try_from(row).is_ok_and(|r| r < st.props.rows().len()) {
        let selected = row as u32 + 1;
        st.props.set(Prop::Selected, PropValue::Int(selected));
        emit_from(cx, st, UiEvent::Select(st.handle, selected));
    }
}

/// Moves a slider's value to where the pointer is (`x` from the control's left), by its step, and tells the realm.
fn slide(st: &mut NodeState, cx: &ControlCx<'_>, x: i32) {
    let width = cx.bounds().1.0.max(1);
    let (lo, hi) = st.props.vec2(Prop::Range);
    let t = (f64::from(x) / f64::from(width)).clamp(0.0, 1.0);
    let mut value = lo + t * (hi - lo);
    let step = st.props.num(Prop::Step);
    if step > 0.0 {
        value = (lo + ((value - lo) / step).round() * step).min(hi);
    }
    let value = value.clamp(lo, hi);
    if (value - st.props.num(Prop::SliderValue)).abs() > f64::EPSILON {
        st.props.set(Prop::SliderValue, PropValue::Number(value));
        emit_from(cx, st, UiEvent::Change(st.handle, Changed::Number(value)));
    }
}

/// What a key does in a text field.
enum Typed {
    Nothing,
    Edited,
    Submitted,
}

fn key_down(st: &mut NodeState, cx: &mut ControlCx<'_>, ev: &KeyEvent) -> bool {
    if u16::from(ev.key_code) == KEY_ESCAPE {
        cx.gui.0.borrow_mut().client_script.stats.escape_keys += 1;
        return false;
    }
    if st.kind != Some(Kind::TextField) || !enabled(st) {
        return false;
    }
    if !seen(cx, st) {
        // A field that cannot be seen takes no keys and loses the focus.
        let l = &mut cx.gui.0.borrow_mut().client_script;
        if l.focus == Some(cx.id) {
            l.blur();
        }
        return false;
    }
    let (consumed, typed) = type_key(st, ev);
    match typed {
        Typed::Nothing => {}
        Typed::Edited => {
            let text = st.props.text(Prop::Text).to_string();
            emit_from(cx, st, UiEvent::Change(st.handle, Changed::Text(text)));
        }
        Typed::Submitted => {
            let text = st.props.text(Prop::Text).to_string();
            emit_from(cx, st, UiEvent::Submit(st.handle, text));
        }
    }
    consumed
}

/// Applies a key to a text field's text and caret: whether the field took the key, and what it did.
fn type_key(st: &mut NodeState, ev: &KeyEvent) -> (bool, Typed) {
    let key = u16::from(ev.key_code);
    let modifier = normalise_modifiers(ev.modifier);
    if modifier & (SI_CTRL | SI_ALT) != 0 {
        return (false, Typed::Nothing);
    }
    let mut text = st.props.text(Prop::Text).to_string();
    let len = text.chars().count();
    st.caret = st.caret.min(len);
    let at = |text: &str, n: usize| text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
    let typed = match key {
        KEY_LEFT => {
            st.caret = st.caret.saturating_sub(1);
            Typed::Nothing
        }
        KEY_RIGHT => {
            st.caret = (st.caret + 1).min(len);
            Typed::Nothing
        }
        KEY_HOME => {
            st.caret = 0;
            Typed::Nothing
        }
        KEY_END => {
            st.caret = len;
            Typed::Nothing
        }
        KEY_BACKSPACE if st.caret > 0 => {
            text.remove(at(&text, st.caret - 1));
            st.caret -= 1;
            Typed::Edited
        }
        KEY_DELETE if st.caret < len => {
            text.remove(at(&text, st.caret));
            Typed::Edited
        }
        KEY_BACKSPACE | KEY_DELETE => Typed::Nothing,
        KEY_RETURN | KEY_NUMPADENTER => return (true, Typed::Submitted),
        _ => match char::from_u32(u32::from(ev.ascii)) {
            Some(c) if insertable(st, c, len) => {
                text.insert(at(&text, st.caret), c);
                st.caret += 1;
                Typed::Edited
            }
            // Any other character the key made is taken and dropped (a hidden one never reaches the text, or the game).
            Some(_) if ev.ascii != 0 => Typed::Nothing,
            _ => return (false, Typed::Nothing),
        },
    };
    if matches!(typed, Typed::Edited) {
        st.props.set(Prop::Text, PropValue::Text(text));
    }
    (true, typed)
}

/// Whether the field takes `c` as its next character with `len` characters in it.
fn insertable(st: &NodeState, c: char, len: usize) -> bool {
    if is_hidden(c) || len >= st.props.int(Prop::MaxLength) as usize {
        return false;
    }
    !st.props.flag(Prop::Numeric) || c.is_ascii_digit() || c == '.' || c == '-'
}

/// The control under `pt` in the tree below `root` (not a mod root), from the top: only where every ancestor's rectangle holds
/// the point, as drawing clips it.
pub(crate) fn hit(vm: &Vm, gui: &Gui, root: ObjId, pt: (i32, i32)) -> Option<(ObjId, (i32, i32))> {
    fn go(
        vm: &Vm,
        gui: &Gui,
        id: ObjId,
        origin: (i32, i32),
        pt: (i32, i32),
        depth: usize,
    ) -> Option<(ObjId, (i32, i32))> {
        if depth > MAX_TREE_DEPTH {
            return None;
        }
        let shift = gui
            .with_custom_state::<NodeState, _>(id, |s| s.child_shift())
            .unwrap_or_default();
        for child in children(vm, id).into_iter().rev() {
            if !control::flag(vm, child, b"visible") {
                continue;
            }
            let (pos, ext) = control::bounds(vm, child);
            let at = (origin.0 + shift.0 + pos.0, origin.1 + shift.1 + pos.1);
            let local = (pt.0 - at.0, pt.1 - at.1);
            if !inside(local, ext) {
                continue;
            }
            if let Some(found) = go(vm, gui, child, at, pt, depth + 1) {
                return Some(found);
            }
            let is_node = gui
                .with_custom_state::<NodeState, _>(child, |s| s.kind.is_some())
                .unwrap_or(false);
            if is_node {
                return Some((child, at));
            }
        }
        None
    }
    let (pos, _) = control::bounds(vm, root);
    go(vm, gui, root, pos, pt, 0)
}

/// Which way a wheel notch turned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Notch {
    Up,
    Down,
}

/// A wheel notch over `id`: the nearest scrolling area or list around it moves. Whether one did.
pub(crate) fn wheel(vm: &Vm, gui: &Gui, id: ObjId, notch: Notch) -> bool {
    let mut at = Some(id);
    for _ in 0..=MAX_TREE_DEPTH {
        let Some(c) = at else {
            return false;
        };
        let height = control::bounds(vm, c).1.1;
        let moved = gui.with_custom_state::<NodeState, _>(c, |s| {
            if !matches!(s.kind, Some(Kind::Scroll | Kind::List)) {
                return None;
            }
            let limit = s.scroll_limit(height);
            let step = match notch {
                Notch::Up => -WHEEL_STEP,
                Notch::Down => WHEEL_STEP,
            };
            s.scroll.1 = (s.scroll.1 + step).clamp(0, limit);
            Some((s.handle, s.kind, s.scroll))
        });
        if let Some(Some((handle, kind, scroll))) = moved {
            if kind == Some(Kind::Scroll) {
                emit(
                    gui,
                    UiEvent::Scroll(handle, f64::from(scroll.0), f64::from(scroll.1)),
                );
            }
            return true;
        }
        at = control::parent(vm, c);
    }
    false
}

/// Where the control `id` is on the canvas: its position and every ancestor's, with the shift of each scrolling area it is in.
pub(crate) fn origin(vm: &Vm, gui: &Gui, id: ObjId) -> (i32, i32) {
    let mut at = (0, 0);
    let mut current = Some(id);
    for _ in 0..=MAX_TREE_DEPTH {
        let Some(c) = current else {
            break;
        };
        let (pos, _) = control::bounds(vm, c);
        at = (at.0 + pos.0, at.1 + pos.1);
        current = control::parent(vm, c);
        if let Some(p) = current {
            let shift = gui
                .with_custom_state::<NodeState, _>(p, |s| s.child_shift())
                .unwrap_or_default();
            at = (at.0 + shift.0, at.1 + shift.1);
        }
    }
    at
}
