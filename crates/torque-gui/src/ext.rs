//! The GUI's seams for a host: where its fonts come from ([`FontSource`]) and an extension ([`GuiExtension`]) that adds
//! control classes of its own (their state, their drawing) and extra looks to the engine's controls (profile fields read by
//! the draw, a pass before each control draws, a displacement of the children drawn, key and focus rules). Without one the
//! GUI is TGE's: [`NoExtension`] and the `torque-fonts` cache.

use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

use torque_fonts::Font;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use crate::Gui;
use crate::draw::DrawCtx;

/// The folder the default font source reads its cache files from.
pub const DEFAULT_FONT_DIR: &str = "common/ui/cache";

/// Where the GUI's fonts come from: a face at a size, loaded on first use.
pub trait FontSource {
    /// The font for `face` at `size`, read on first use with `read` (a path relative to the root folder).
    fn font(
        &mut self,
        face: &str,
        size: i32,
        read: &dyn Fn(&str) -> Option<Vec<u8>>,
    ) -> Option<Arc<Font>>;

    /// The cache under it (its counts, the faces it could not find).
    fn cache(&self) -> &torque_fonts::FontCache;

    fn as_any(&self) -> &dyn Any;

    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl FontSource for torque_fonts::FontCache {
    fn font(
        &mut self,
        face: &str,
        size: i32,
        read: &dyn Fn(&str) -> Option<Vec<u8>>,
    ) -> Option<Arc<Font>> {
        self.get(face, size, read).map(|(f, _)| f)
    }

    fn cache(&self) -> &torque_fonts::FontCache {
        self
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// The default font source: the `torque-fonts` cache over [`DEFAULT_FONT_DIR`], no fallback.
pub fn default_fonts() -> Box<dyn FontSource> {
    Box::new(torque_fonts::FontCache::new(
        DEFAULT_FONT_DIR,
        Box::new(torque_fonts::NoFallback),
    ))
}

/// A profile colour an extension may add to the engine's lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListColor {
    /// An outline over a scroll view's content.
    Border,
    /// The fill of a popup list's row under the mouse.
    HoverFill,
}

/// A rule of the engine's controls a host may turn the other way ([`GuiExtension::rule`]; all off by default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuiRule {
    /// `GuiPopUpMenuCtrl::setSelected` of an entry that exists runs the control's `command` too (TGE runs it only for an id
    /// with no entry).
    PopupSelectRunsCommand,
    /// Shift-Tab in a text edit without `tabComplete` selects to the start, as Shift-Home (TGE's `switch` falls through);
    /// the engine's passes it to the parent.
    ShiftTabSelectsToStart,
    /// GuiProgressCtrl's `setValue` takes a value that is not a number as 1 and a negative zero as 0 (a value is 1 unless it
    /// is at most 1, then 0 unless it is above 0); the engine's clamp keeps both as they are.
    ProgressNanIsFull,
    /// GuiTextEditCtrl takes TGE's keys and clicks: Ctrl+A/B/D/E/F/N/P/U are Home, Left, Delete, End, Right, Down, Up and
    /// Ctrl+Delete, Ctrl+Z swaps the text with one undo copy, Ctrl+Backspace and Ctrl+Delete clear the text, Ctrl+Left and
    /// Ctrl+Right do nothing, a click places the cursor before the character under it with the text taken unscrolled, and a
    /// double click is a click. The engine's are the usual desktop keys and clicks (select all, word moves, undo and redo
    /// steps; see [`crate::input::text_edit`]).
    TgeEditKeys,
    /// The platform layer gives the keys TGE's codes ([`crate::keys::tge_vk_to_key`]): Pause reads as `numlock`, the
    /// numpad's divide as `slash`, and Print Screen, the Windows and menu keys and F16 to F24 have none. The engine gives
    /// every key its own ([`crate::keys::vk_to_key`]).
    TgeKeyCodes,
}

/// A console message the GUI prints, with what it names; [`GuiMessage::engine_text`] is the engine's wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuiMessage<'a> {
    /// `add` of a control set named an object that does not exist.
    SetAddMissing { object: &'a str, set: &'a str },
    /// `remove` of a control set named an object that is not in it.
    SetRemoveMissing { object: &'a str, set: &'a str },
    /// A profile's `bitmap` does not load.
    ProfileBitmapMissing { profile: &'a str, bitmap: &'a str },
    /// `setCursor` named something that is not a GuiCursor.
    InvalidCursor { name: &'a str },
}

impl GuiMessage<'_> {
    /// The engine's wording (Torque3D's).
    pub fn engine_text(&self) -> String {
        match *self {
            GuiMessage::SetAddMissing { object, set } => {
                format!("Set::add: Object \"{object}\" doesn't exist to add to {set}")
            }
            GuiMessage::SetRemoveMissing { object, set } => {
                format!("Set::remove: Object \"{object}\" does not exist in set {set}")
            }
            GuiMessage::ProfileBitmapMissing { profile, bitmap } => {
                format!("({profile}) - Failed to load profile bitmap ({bitmap})")
            }
            GuiMessage::InvalidCursor { name } => {
                format!("setCursor: no GuiCursor named \"{name}\"")
            }
        }
    }
}

/// What a host adds to the GUI. Every method has the engine's answer as its default; the GUI calls them with no borrow of its
/// state held, and none of them may run script but [`focused`](Self::focused).
pub trait GuiExtension: Any {
    fn into_any(self: Rc<Self>) -> Rc<dyn Any>;

    /// A new font source for a layout or a draw cache (None: [`default_fonts`]).
    fn fonts(&self) -> Option<Box<dyn FontSource>> {
        None
    }

    /// Before a draw cache draws a frame: what the extension keeps for its fonts is given to them.
    fn begin_draw(&self, _gui: &Gui, _fonts: &mut dyn FontSource) {}

    /// Whether a new control of one of the extension's classes starts active (the engine's own classes are decided before).
    fn starts_active(&self, _vm: &Vm, _id: ObjId) -> bool {
        false
    }

    /// The control `id` was removed: anything the extension kept for it goes.
    fn removed(&self, _id: ObjId) {}

    /// Draws the control `id` of a class the engine does not draw (`class`, its own class name); true when it did.
    fn draw_control(
        &self,
        _d: &mut DrawCtx<'_, '_>,
        _class: &str,
        _id: ObjId,
        _offset: (i32, i32),
        _update: [i32; 4],
        _depth: usize,
    ) -> bool {
        false
    }

    /// Before any control draws: the extension's pass under it (offset: its place on the canvas; update: its clip).
    fn before_control(
        &self,
        _d: &mut DrawCtx<'_, '_>,
        _id: ObjId,
        _offset: (i32, i32),
        _update: [i32; 4],
    ) {
    }

    /// Whether the extension's pass replaces the fill of controls with `profile` (they draw no `fillColor`, and a later
    /// sibling with it that covers a control whole hides that control).
    fn replaces_fill(&self, _vm: &Vm, _profile: ObjId) -> bool {
        false
    }

    /// The opacity the control `id` and its children draw at (times their parent's), None: unchanged.
    fn opacity(&self, _vm: &Vm, _id: ObjId) -> Option<f32> {
        None
    }

    /// An extra list colour of `profile`.
    fn list_color(&self, _vm: &Vm, _profile: ObjId, _which: ListColor) -> Option<[u8; 4]> {
        None
    }

    /// A popup list's text inset `(x, y)` for the profile named `profile` (None: the engine's `(4, 0)`, no taller cells).
    fn list_pad(&self, _vm: &Vm, _profile: &[u8]) -> Option<(i32, i32)> {
        None
    }

    /// The canvas size when no `$pref::Video::resolution` says otherwise (the engine's: [`crate::DEFAULT_CANVAS_SIZE`], 16:9).
    fn default_canvas_size(&self) -> (i32, i32) {
        crate::DEFAULT_CANVAS_SIZE
    }

    /// What runs when `createCanvas` makes the canvas, before it exists (the canvas size is set).
    fn before_canvas(&self, _vm: &mut Vm) -> Result<(), VmError> {
        Ok(())
    }

    /// The colour a profile's text is outlined in (drawn nine times around each text first); None: no outline.
    fn text_outline(&self, _vm: &Vm, _profile: ObjId) -> Option<[u8; 4]> {
        None
    }

    /// How a bitmap control fits its bitmap keeping the bitmap's aspect: `(cover, align_left, align_top)` (cover: fill the
    /// control, else fit inside it; unaligned sides centred); None: stretched over the control.
    fn bitmap_fit(&self, _vm: &Vm, _id: ObjId) -> Option<(bool, bool, bool)> {
        None
    }

    /// The tallest an ML text control draws its bitmaps (-1: no limit); None: no limit.
    fn ml_max_bitmap_height(&self, _vm: &Vm, _id: ObjId) -> Option<i32> {
        None
    }

    /// Whether an ML text control is resized to its text's height after a reflow (then its `onResize` runs).
    fn ml_auto_resize(&self, _vm: &Vm, _id: ObjId) -> bool {
        false
    }

    /// The row height and column width a scroll control scrolls by a line (None: the content's line sizes, 30 by 30).
    fn scroll_line_sizes(&self, _vm: &Vm, _id: ObjId) -> Option<(u32, u32)> {
        None
    }

    /// The scroll bars' arrow and track rectangles of `s` for a control of extent `ext` and border `border`, by the extension's
    /// own arithmetic; false: the engine's.
    fn scroll_rects(
        &self,
        _s: &mut crate::scroll::ScrollState,
        _ext: (i32, i32),
        _border: i32,
    ) -> bool {
        false
    }

    /// The scroll thumbs of `s` (`constantThumbHeight`: `constant`) by the extension's own arithmetic; false: the engine's.
    fn scroll_thumbs(&self, _s: &mut crate::scroll::ScrollState, _constant: bool) -> bool {
        false
    }

    /// The displacement the children of `id` are drawn at, `now` (screen pixels).
    fn shift(&self, _gui: &Gui, _id: ObjId, _now: u32) -> (i32, i32) {
        (0, 0)
    }

    /// The content of the scroll control `id` moved by `moved` (screen pixels).
    fn scrolled(&self, _vm: &Vm, _gui: &Gui, _id: ObjId, _moved: (i32, i32)) {}

    /// A thumb of the scroll control `id` is dragged.
    fn thumb_dragged(&self, _gui: &Gui, _id: ObjId) {}

    /// A mouse button went down on the canvas.
    fn mouse_pressed(&self, _gui: &Gui) {}

    /// The bitmap a `<bitmap:>` of ML text draws when the one it names does not load (None: nothing is drawn).
    fn missing_bitmap(&self) -> Option<&'static str> {
        None
    }

    /// The text of a console message (None: the engine's, [`GuiMessage::engine_text`]).
    fn message(&self, _message: &GuiMessage<'_>) -> Option<String> {
        None
    }

    /// Whether a GuiButtonCtrl, a GuiPopUpMenuCtrl with a border or open, its open list and GuiSliderCtrl's thumb in
    /// `profile` draw TGE's beveled boxes and square lists (`renderRaisedBox`, `renderSlightlyRaisedBox`; a depressed button
    /// no box at all, an open popup a `fillColorHL` fill outlined in `borderColor`) instead of the engine's rounded boxes
    /// (see [`crate::draw`]'s rounded boxes). The engine's answer: no.
    fn classic_boxes(&self, _vm: &Vm, _profile: ObjId) -> bool {
        false
    }

    /// Whether `rule` holds (the engine's answer: no).
    fn rule(&self, _rule: GuiRule) -> bool {
        false
    }

    /// The key tables `bind`, the accelerators and the key names a GuiInputCtrl reports read
    /// ([`crate::keys::KeyTables::engine`]).
    fn key_tables(&self) -> crate::keys::KeyTables {
        crate::keys::KeyTables::engine()
    }

    /// Whether the control `id` clips its drawing to its parent's (else to the whole canvas).
    fn clip_to_parent(&self, _vm: &Vm, _id: ObjId) -> bool {
        true
    }

    /// Whether the button `id`, with the key focus, is pressed by Return and Space.
    fn key_press(&self, _vm: &Vm, _id: ObjId) -> bool {
        false
    }

    /// The control `id` took the key focus.
    fn focused(&self, _vm: &mut Vm, _gui: &Gui, _id: ObjId) -> Result<(), VmError> {
        Ok(())
    }
}

/// The engine's GUI, nothing added.
#[derive(Debug, Default)]
pub struct NoExtension;

impl GuiExtension for NoExtension {
    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }
}
