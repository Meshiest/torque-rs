//! The GUI object model (no GPU, no window): the Rust state beside the `Vm`'s GUI objects, the canvas, the wake/sleep
//! tree walks, TGE's layout, and the methods a game's scripts call on its GUI classes.
//!
//! **Where the state lives.** A control's authored fields (`position`, `extent`, `minExtent`, `horizSizing`, `vertSizing`, `profile`,
//! `visible`, `enabled`, `text` of a button, `bitmap`, `command`,...) are the `Vm`'s static fields: the class table gives every GUI
//! class its fields ([`specs`] for this crate's classes), and the bounds are read from the fields `position`, `extent` and
//! `minExtent` of `GuiControl`. What has no field (awake, the dialog layer, a text control's `mText` beside its `text`
//! field `mInitialText`, a button's state, a list's rows, a popup's entries) is a [`Control`] in [`GuiState`], made by the classes'
//! `EngineClass::on_add` and dropped by `on_remove`. One [`Gui`] handle is shared by the natives.
//!
//! **Behaviour** (TGE 1.4's design):
//! - `GuiControl::onAdd`: a named control links its name's namespace under its class's (`Vm::link_name_namespace`), joins
//!   GuiGroup, and gets the script `onAdd`. `GuiControlProfile` and `GuiCursor` join GuiDataGroup.
//! - `awaken`: children first, in order, then the control's own `onWake` (a missing profile becomes
//!   `<class before "Ctrl">Profile`, else `GuiDefaultProfile`; the layer from the parent; the script `onWake`). `sleep`: children first,
//!   then `onSleep`. A control added to an awake parent wakes; one taken out of an awake parent sleeps first (`GuiControl::addObject`,
//!   `removeObject`).
//! - `resize`: the extent clamped to `minExtent`, every child's `parentResized(old, new)` (TGE's horiz/vertSizing rules:
//!   `center`, `width`, `left`, `relative`; `right` and `bottom` keep their place), then the bounds. Unconditional.
//! - The canvas (`createCanvas` makes `Canvas`, awake, 1280x720 by default): `setContent` (TGE 1.4's loop, which
//!   removes every child it meets to GuiGroup, the content's place kept first), `pushDialog`/`popDialog`/`popLayer` with layers,
//!   `setCursor` (`<name> is not a valid cursor.`), cursor on/off and show/hide, a stored cursor position. Every frame
//!   ([`Gui::frame`], `GuiCanvas::renderFrame`'s first step) resizes each child of the canvas to the canvas: that is how the 640x480
//!   authoring reaches the canvas's size.
//! - The render pass's scripts: a drawn GuiFadeinBitmapCtrl (awake, visible with its parents) calls `onWait` once after its fade-in
//!   and `onDone` once after its fade-out, timed from its wake or `reset` ([`Fade`]), each frame ([`Gui::frame`]).
//! - `GuiControlProfile`'s constructor: a new profile starts from `GuiDefaultProfile`'s fields (all but
//!   `mouseOverSelected`), its colours otherwise those of fresh memory, before its declaration's `: parent` and fields.
//!
//! **Script callbacks run with no borrow held.** Every walk takes what it needs out of [`GuiState`] under a short borrow, drops it, and
//! calls the script; it re-reads the tree after every call (a callback may change it). Recursion is bounded by the tree's depth; every
//! visit is charged to the step budget (`Vm::charge_work`).
//!
//! - GuiTextCtrl's automatic size at wake and `setText` from the profile's font ([`GuiState::fonts`]; a
//!   face and size with no font is counted in [`Gui::autosize_skipped`]). Drawing is [`draw`].
//!
//! - Input ([`input`]): the canvas's dispatch of the platform's input events, mouse capture, hover, buttons, text edits, first responders,
//!   tabbing, accelerators; the key names and the Win32 layer ([`keys`]).
//!
//! **Not modelled yet** (later steps): the subclasses' own `resize` (`GuiScrollCtrl`, `GuiWindowCtrl`, `GuiMLTextCtrl`'s reflow),
//! the mouse on scroll bars, sliders, popups and lists.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/core/guiControl.cpp` (Copyright (c)
//! 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.), `gui/core/guiCanvas.cpp` (Copyright (c) 2012 GarageGames, LLC;
//! Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.
//!
//! # Maintaining
//!
//! Rules a change must keep, each guarded by a test:
//! - Every font lookup names whose it is ([`torque_fonts::FontUse`]): ML text's `<font:>` styles and a server's client-script
//!   layer are markup, so they never take a profile's font; the GUI starts a font frame every frame (`Gui::frame`,
//!   `draw::draw_canvas`; the server's layer is [`torque_fonts::FontUse::Server`]), and a frame's drawing loads at most
//!   [`draw::MAX_FRAME_MARKUP_LOADS`] new markup fonts (`client_script_the_server_layer_fonts_are_markup_and_never_the_games`,
//!   `a_reflow_of_a_thousand_font_tags_stays_within_the_markup_quota_and_spares_the_profiles`).
//! - A reflow charges every font it loads before it rasterizes it, and names at most [`mltext::MAX_REFLOW_FACES`] faces
//!   (`a_reflow_that_cannot_pay_for_its_fonts_stops`).
//! - A reflow resolves at most [`mltext::MAX_REFLOW_BITMAP_NAMES`] bitmap names, each candidate and byte charged, each file
//!   sized from its first [`mltext::BITMAP_HEADER_BYTES`] (`a_bitmap_is_sized_from_its_first_bytes_and_every_lookup_is_charged`,
//!   `a_reflow_of_five_thousand_bitmap_names_resolves_at_most_its_cap_and_reads_only_headers`); a file not sized by its prefix is
//!   read again at most once, up to [`mltext::BITMAP_SIZE_BYTES`] paid for first, and a file not sized is remembered so
//!   (`a_bitmap_with_no_header_in_reach_is_read_at_most_once_and_never_past_its_cap`).
//! - Every pass of a reflow is constant work: tags are pushed and their ids kept in a set, and every number is read from its tag
//!   alone (`a_reflow_of_sixty_five_thousand_tags_is_linear`, `a_duplicate_tag_id_is_still_counted_once`).
//! - Scrolling ([`glide`]): the content is always at its final place, so scripts, hit tests and `position` never see a glide; only
//!   [`scroll::shown`] (the draw) adds it. Only the wheel, the wheel's parts and [`scroll::reveal`] push a glide (through
//!   [`scroll::report_moved`]); [`scroll::scroll_to`] and the keyboard never do, and a press ends every glide
//!   (`the_content_is_at_its_final_place_at_once_and_only_the_picture_glides`,
//!   `a_press_ends_every_glide_and_leaves_the_content_where_it_is`,
//!   `keyboard_and_script_scrolls_move_at_once_with_the_glide_left_as_it_was`). A glide runs on [`Vm::sim_time`], never on frames
//!   (`a_glide_takes_the_same_clock_time_however_the_frames_come`). A part of a wheel notch the content could not move by is not
//!   owed back (`a_part_the_content_could_not_move_by_is_not_owed_back`). The reveal of a focused control is made in
//!   `input::set_first_responder`, where every way of taking the focus passes, and only with `$Pref::Gui::RevealFocus` on
//!   (`scroll::tests`, `a_control_that_takes_the_focus_scrolls_nothing_until_the_pref_is_on`).
//! - The profile fields TGE has not are read as stock until a game sets them: `keyActivate` (`a_button_whose_profile_does_not_ask_never_answers_the_keys`),
//!   `cornerRadius` (0 draws the square fills and borders, the buttons' own radius of 5) and a control's `opacity` (1;
//!   `opacity_starts_at_one_on_every_class`). The corner sprites of a radius above [`draw::BOX_RADIUS`] are a texture made on first
//!   use; the radius is cut to [`draw::MAX_PROFILE_RADIUS`] and to half the box's shorter side.
//! - `registerFont` checks the face name and the path before it reads anything, refuses a name the game's fonts have (the default
//!   faces, the layout's found faces, the cache files' faces), pays for a file's bytes before it reads them, reads through the file
//!   host, and registers once a face in at most [`torque_fonts::faces::MAX_REGISTERED`]; every draw cache takes the faces up before its frame
//!   (`faces::sync`), so a host's own [`FontSource`] must implement [`FontSource::register_face`] to take them
//!   (`every_draw_cache_draws_text_in_the_registered_face`, `a_face_name_the_games_fonts_have_is_refused_in_any_case`,
//!   `the_bytes_are_paid_for_before_they_are_read`). A face is registered for the session: nothing scopes one to a server.
//! - Extensions ([`fanout`]): a GUI has one primary [`GuiExtension`] and any number added after it, and [`Gui::ext`] is the primary
//!   itself while none is added, so the hot paths pay for the list only when a game has one. Every method of the trait has a rule in
//!   `fanout::FanOut`, and a method added to the trait gets one there in the same change: the game-wide ones (fonts, canvas
//!   size, key tables, rules, messages) are the primary's alone, so an added extension cannot change what the whole GUI is
//!   (`fanout_game_wide_methods_are_the_primarys_alone`); `clip_to_parent` answers true unless a member says no, because the
//!   trait's default is true (`fanout_a_control_is_clipped_unless_a_member_says_no`); a layer stamp is unsaid when any member
//!   cannot say, so a member with hooks the stamp does not cover never gets a stale layer
//!   (`fanout_the_layer_stamp_follows_every_answer_in_order`). Plugins add extensions as factories ([`GuiExtensions`]), made once
//!   for each client's GUI by the GUI plugin; two primaries fail the build (`fanout_a_second_primary_fails_the_build_naming_both_plugins`).
//!   Check: `cargo test -p torque-gui --lib fanout_`.
//! - The pictures of these looks are made by `TORQUE_PICTURE_DIR=<folder> cargo test -p torque-gui --lib pictures_tests`.
//!
//! Costs a frame must not grow back (bench `gui draw 300 controls`, timed under the bench lock): a control's class is matched
//! by its borrowed name (no copy a control, and none for a plugin's control), a run of glyphs finds its font sheet once a sheet
//! (no key a glyph, no copy of the face's name for a face drawn last), a font is looked up once a frame for each face and size, a
//! position or a flag is read as the number it is stored as (no text made and read back), a control with no children does not
//! read its parent's clip, and an outline is laid out once and pushed at each of its offsets when its passes are the same. The
//! client-script layer is kept from frame to frame while nothing it draws from changes (`client_script`, **Kept draws**), so an
//! idle realm costs the canvas next to nothing; the justfile's `client-script-bench` holds the whole frame to its budgets.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use torque_ts_vm::idhash::IdMap;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Native, Vm, VmError};

pub mod backdrop;
pub mod canvas;
mod classes;
pub mod client_script;
pub mod console;
mod control;
pub mod custom;
pub mod draw;
pub mod ext;
pub mod faces;
pub mod fanout;
pub mod glide;
pub mod hud;
pub mod input;
pub mod keys;
pub mod mltext;
pub mod msgvector;
pub mod popup;
pub mod profile;
pub mod scroll;
pub mod slider;
pub mod specs;
#[cfg(test)]
mod test_rig;
pub mod textlist;
pub mod textures;
pub mod window;

pub use canvas::{
    DEFAULT_CANVAS_SIZE, control_rect, create_canvas, find_button, focus, mouse_move,
};
pub use classes::{color_f_text, color_f_to_i, color_i_text, reg, scan_ints4};
pub use control::{
    awaken, bounds, children, flag, is_a, parent, parent_resized, point, resize, screen_position,
    sleep,
};
pub use draw::{DrawCache, GuiDraws, TextLook, draw_canvas, run_callbacks};
pub use ext::{FontSource, GuiExtension, NoExtension};
pub use fanout::{Extension, GuiExtensionEntry, GuiExtensions, GuiPrimary};
pub use glide::GuiPrefs;
pub use input::{CursorShape, hover_cursor};

/// The most children one walk visits under one control: a hostile script's GUI with more is walked to here (a named cap).
pub const MAX_CHILDREN_WALKED: usize = 65_536;

/// The deepest control tree a walk follows (menus are rarely more than 12 deep); deeper controls are left as they are.
pub const MAX_TREE_DEPTH: usize = 64;

/// The work one host frame ([`Gui::frame`]) may do outside a run (`Vm::charge_work` units, about 250,000 steps; a main menu
/// costs a few thousand): a canvas a script filled with controls is laid out to here and the rest waits for the next frame.
pub const MAX_FRAME_WORK: u64 = 16_000_000;

/// The most rows a GuiTextListCtrl or entries a GuiPopUpMenuCtrl keeps (a safety cap: TGE's vectors have none).
pub const MAX_LIST_ROWS: usize = 100_000;

/// The most overlays shown at once ([`Gui::set_overlay`]).
pub const MAX_OVERLAYS: usize = 8;

/// The longest text a control keeps (TGE's `GuiTextCtrl::MAX_STRING_LENGTH` is 255; an ML text control keeps more).
pub const MAX_TEXT_BYTES: usize = 1 << 20;

/// What a control has that is not a field.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Control {
    pub awake: bool,
    /// `mLayer`: a dialog's layer, inherited by its children at wake.
    pub layer: i32,
    /// `mText` of the text controls (GuiTextCtrl and its subclasses, GuiMLTextCtrl): the `text` field is `mInitialText`.
    pub text: Option<Vec<u8>>,
    /// A button's `mStateOn` (check boxes, radios).
    pub state_on: bool,
    /// A GuiProgressCtrl's value, and a GuiTextEditSliderCtrl's (`mValue`).
    pub progress: f32,
    /// A GuiTextListCtrl's rows `(id, text, active)` and selected row; a GuiPopUpMenuCtrl's entries `(id, text)` and selected id.
    pub rows: Vec<(i32, Vec<u8>, bool)>,
    pub selected: i32,
    /// `onRender` has run once (`setHasRendered`).
    pub has_rendered: bool,
    /// A GuiTextCtrl's text width as its last render drew it (`onRender` keeps `renderJustifiedText`'s result, the pen's
    /// advance; the constructor and the script `setText` clear it): what `getPixelWidth` answers.
    pub pixel_width: i32,
    /// The script file whose code made the control (`$Con::File` at its `onAdd`; empty for one made from the host).
    pub file: Vec<u8>,
    /// A button's `mMouseOver` (0x95) and `mDepressed` (0x94): what its drawing shows (set by the canvas's mouse handling).
    pub mouse_over: bool,
    pub depressed: bool,
    /// A GuiFadeinBitmapCtrl's clock (see [`Fade`]).
    pub fade: Fade,
    /// `mActive`: false for a GuiControl, true for the classes whose constructor sets it (buttons, text edits, lists,...);
    /// `setActive` changes it.
    pub active: bool,
    /// `mFirstResponder`: the control below it keys went to last ([`input`]).
    pub first_responder: Option<ObjId>,
    /// A GuiTextEditCtrl's cursor, selection, history ([`input::EditState`]).
    pub edit: Option<Box<input::EditState>>,
    /// A GuiScrollCtrl's sizes, bars and thumbs ([`scroll::ScrollState`]).
    pub scroll: Option<Box<scroll::ScrollState>>,
    /// A GuiArrayCtrl's (GuiTextListCtrl's) cells ([`textlist::ListState`]).
    pub list: Option<textlist::ListState>,
    /// A GuiMLTextCtrl's layout and selection ([`mltext::MlState`]); its buffer is [`Control::text`].
    pub ml: Option<Box<mltext::MlState>>,
    /// A GuiWindowCtrl's title bar and buttons ([`window::WindowState`]).
    pub window: Option<window::WindowState>,
    /// A GuiSliderCtrl's thumb and `mDisplayValue` ([`slider::SliderMembers`]).
    pub slider: Option<slider::SliderMembers>,
    /// A GuiPopUpMenuCtrl's `mInAction`: its list is open, and the controls it made ([`popup`]).
    pub in_action: bool,
    pub popup: Option<popup::Parts>,
    /// The popup an open list or its background belongs to.
    pub popup_of: Option<(ObjId, popup::Part)>,
    /// A GuiMessageVectorCtrl's MessageVector (`attach`, [`msgvector`]).
    pub message_vector: Option<ObjId>,
    /// A GuiGraphCtrl's plots ([`hud::GraphState`]).
    pub graph: Option<Box<hud::GraphState>>,
    /// The host made the control for a client-script realm ([`client_script`]): no script callback runs for it.
    pub realm: bool,
}

/// A GuiFadeinBitmapCtrl's state (`onWake`, `reset`, `onRender`): the tick count it woke or was reset at, whether `onWait`
/// and `onDone` have run since. The tick count is the simulation clock (`Vm::sim_time`), which moves with the platform's
/// millisecond clock.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Fade {
    pub wake: u32,
    pub waited: bool,
    pub done: bool,
}

/// The GUI's fonts, shared (their files are read through the `Vm`'s file host when first used).
#[derive(Clone)]
pub struct SharedFonts(pub Rc<RefCell<Box<dyn FontSource>>>);

impl SharedFonts {
    pub fn new(source: Box<dyn FontSource>) -> SharedFonts {
        SharedFonts(Rc::new(RefCell::new(source)))
    }
}

impl Default for SharedFonts {
    fn default() -> SharedFonts {
        SharedFonts::new(ext::default_fonts())
    }
}

impl std::fmt::Debug for SharedFonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SharedFonts")
    }
}

/// The canvas and every control's state.
#[derive(Debug)]
pub struct GuiState {
    /// The `Canvas` object, once `createCanvas` made it.
    pub canvas: Option<ObjId>,
    /// The window's client size (`$pref::Video::resolution` at `createCanvas`, headless: a stored size).
    pub canvas_size: (i32, i32),
    /// `GuiCanvas::setCursor`: the GuiCursor, or none.
    pub cursor: Option<ObjId>,
    /// `cursorOn`/`cursorOff` (TGE's constructor: on).
    pub cursor_on: bool,
    /// How many times the game called `cursorOn` or `cursorOff`: the client-script layers give back a cursor they turned on
    /// only while the game has not set it since.
    pub cursor_calls: u64,
    /// `showCursor`/`hideCursor` (on).
    pub show_cursor: bool,
    /// `getCursorPos` (headless: what `setCursorPos` stored).
    pub cursor_pos: (i32, i32),
    /// Whether the last drawn frame drew the canvas's cursor (a GuiCursor is set and its bitmap loaded). A window shows the
    /// system's cursor while the canvas's is on and none was drawn, so a game without a GuiCursor still has a pointer.
    pub cursor_drawn: bool,
    pub render_front: bool,
    pub controls: IdMap<ObjId, Control>,
    /// Frames drawn ([`Gui::frame`]).
    pub frames: u64,
    /// Text controls whose profile asked for an automatic size that could not be computed (no font for the profile's face and
    /// size).
    pub autosize_skipped: u64,
    /// The fonts the layout measures text with (the automatic sizes; a [`FontSource`]).
    pub fonts: SharedFonts,
    /// The canvas size `createCanvas` uses instead of `$pref::Video::resolution` (headless runs: the default, 1280x720).
    pub forced_size: Option<(i32, i32)>,
    /// The canvas's input state (first responder, capture, the control under the mouse, clicks, accelerators: [`input`]).
    pub input: input::CanvasInput,
    /// What plays a button profile's sounds (the audio; none: silent).
    pub sound_hook: input::SoundHookSlot,
    /// The profiles' bitmap arrays (`constructBitmapArray`, [`profile::bitmap_array`]), made once per profile.
    pub bitmap_arrays: HashMap<ObjId, Rc<Vec<profile::Rect>>>,
    /// The sizes of the bitmaps ML text names (`<bitmap:>`, [`mltext::BitmapSizes`]).
    pub bitmap_sizes: mltext::BitmapSizes,
    /// The texture names a control set to `GL_NEAREST` (GuiScrollCtrl's `onWake`, `setFilterNearest`): one texture object a
    /// name, so every control drawing that bitmap draws it nearest from then on (lower case).
    pub nearest_textures: std::collections::HashSet<String>,
    /// Whether a GameTSCtrl's camera query has a camera (the client host sets it: a connection with a control object), so the
    /// controls draw the world.
    pub world_camera: bool,
    /// What the HUD classes read of the connection (the client host sets it every tick: [`hud::HudFeed`]).
    pub hud: hud::HudFeed,
    /// The host's extensions ([`GuiExtension`], [`fanout`]).
    pub ext: Extension,
    /// The control classes plugins added and their controls ([`custom`]).
    pub(crate) custom: custom::Custom,
    /// The host's overlays ([`Gui::set_overlay`]): controls drawn over the canvas's children, never members of the canvas, so a
    /// script's `Canvas.getCount()` and `Canvas.getObject(%i)` count only what the scripts put there.
    pub overlays: Vec<ObjId>,
    /// The host layers of client scripts: the server layer, the engine layer and the chip ([`client_script`]).
    pub client_script: client_script::LayerState,
    /// The scroll controls' glides, with smooth scrolling on ([`glide`]).
    pub glides: glide::Glides,
    /// The TrueType faces scripts registered (`registerFont`), which every draw cache takes up ([`faces`]).
    pub faces: faces::Registered,
}

impl Default for GuiState {
    fn default() -> GuiState {
        GuiState {
            canvas: None,
            canvas_size: DEFAULT_CANVAS_SIZE,
            cursor: None,
            cursor_on: true,
            cursor_calls: 0,
            show_cursor: true,
            cursor_pos: (0, 0),
            cursor_drawn: false,
            render_front: false,
            controls: IdMap::default(),
            frames: 0,
            autosize_skipped: 0,
            fonts: SharedFonts::default(),
            forced_size: None,
            input: input::CanvasInput::default(),
            sound_hook: input::SoundHookSlot::default(),
            bitmap_arrays: HashMap::new(),
            bitmap_sizes: Default::default(),
            nearest_textures: Default::default(),
            world_camera: false,
            hud: hud::HudFeed::default(),
            ext: Extension::default(),
            custom: custom::Custom::default(),
            overlays: Vec::new(),
            client_script: client_script::LayerState::default(),
            glides: glide::Glides::default(),
            faces: faces::Registered::default(),
        }
    }
}

/// The handle the natives share. Cloning it shares the state.
#[derive(Clone, Default)]
pub struct Gui(pub Rc<RefCell<GuiState>>);

impl Gui {
    /// Whether `id` is a GUI control the model knows.
    pub fn is_control(&self, id: ObjId) -> bool {
        self.0.borrow().controls.contains_key(&id)
    }

    /// Whether the control `id` is awake.
    pub fn is_awake(&self, id: ObjId) -> bool {
        self.0.borrow().controls.get(&id).is_some_and(|c| c.awake)
    }

    /// The control's value as its class's `getValue` gives it (a text field's text, a list's selected row's text), read from its state
    /// (never through a script: a package cannot change the answer).
    pub fn value_of(&self, vm: &Vm, id: ObjId) -> Vec<u8> {
        classes::get_value(vm, self, id)
    }

    /// The id of a text list's selected row (`getSelectedId`; -1 when none), read from its state.
    pub fn selected_id(&self, id: ObjId) -> i32 {
        self.with(id, |c| {
            usize::try_from(c.selected)
                .ok()
                .and_then(|r| c.rows.get(r))
                .map_or(-1, |r| r.0)
        })
        .unwrap_or(-1)
    }

    /// A whole copy of the control's state (its text and every row of a list): for hosts and tests. The natives never call it on a
    /// script's behalf; they read what they need under a short borrow ([`with`](Self::with)).
    pub fn control(&self, id: ObjId) -> Option<Control> {
        self.0.borrow().controls.get(&id).cloned()
    }

    /// `f` of the control's state under a short borrow (no script may run inside `f`). None for an unknown control.
    pub fn with<R>(&self, id: ObjId, f: impl FnOnce(&Control) -> R) -> Option<R> {
        self.0.borrow().controls.get(&id).map(f)
    }

    /// `f` of the control's state, mutable, under a short borrow (no script may run inside `f`).
    pub fn with_mut<R>(&self, id: ObjId, f: impl FnOnce(&mut Control) -> R) -> Option<R> {
        self.0.borrow_mut().controls.get_mut(&id).map(f)
    }

    /// Whether the host made the control for a client-script realm ([`Control::realm`]).
    pub fn is_realm(&self, id: ObjId) -> bool {
        self.0.borrow().controls.get(&id).is_some_and(|c| c.realm)
    }

    /// The control's dialog layer (0 for one the model does not know).
    pub fn layer(&self, id: ObjId) -> i32 {
        self.with(id, |c| c.layer).unwrap_or(0)
    }

    /// The canvas object.
    pub fn canvas(&self) -> Option<ObjId> {
        self.0.borrow().canvas
    }

    /// Sets the canvas's size (the window's client size); the next [`frame`](Self::frame) lays the content out to it.
    pub fn set_canvas_size(&self, size: (i32, i32)) {
        self.0.borrow_mut().canvas_size = size;
    }

    pub fn autosize_skipped(&self) -> u64 {
        self.0.borrow().autosize_skipped
    }

    /// Shows (`on`) or takes down the host's overlay `id` ([`GuiState::overlays`]): shown, it is awake, laid out to the canvas every
    /// frame, pre-rendered after the canvas's children and drawn over them; it takes no input (the canvas's routing never sees it).
    /// At most [`MAX_OVERLAYS`]; a control that is not one, or is on the canvas, is not shown.
    pub fn set_overlay(&self, vm: &mut Vm, id: ObjId, on: bool) -> Result<(), VmError> {
        let shown = self.0.borrow().overlays.contains(&id);
        if on {
            if !shown {
                let in_control = vm
                    .objects()
                    .get(id)
                    .and_then(|o| o.group())
                    .is_some_and(|g| self.is_control(g) || Some(g) == self.canvas());
                if in_control
                    || !self.is_control(id)
                    || self.0.borrow().overlays.len() >= MAX_OVERLAYS
                {
                    return Ok(());
                }
                self.0.borrow_mut().overlays.push(id);
            }
            control::awaken(vm, self, id)
        } else {
            if !shown {
                return Ok(());
            }
            self.0.borrow_mut().overlays.retain(|o| *o != id);
            control::sleep(vm, self, id)
        }
    }

    /// The overlays shown that still exist outside every control, in drawing order (one deleted, or added to the canvas or another
    /// control since, is forgotten).
    pub fn overlays(&self, vm: &Vm) -> Vec<ObjId> {
        let shown = self.0.borrow().overlays.clone();
        let keep: Vec<ObjId> = shown
            .into_iter()
            .filter(|o| {
                vm.objects().get(*o).is_some_and(|obj| {
                    !obj.group()
                        .is_some_and(|g| self.is_control(g) || Some(g) == self.canvas())
                })
            })
            .collect();
        self.0.borrow_mut().overlays = keep.clone();
        keep
    }

    /// Whether `id` is a shown overlay.
    pub fn is_overlay(&self, id: ObjId) -> bool {
        self.0.borrow().overlays.contains(&id)
    }

    /// One frame of the canvas, as far as it changes state: `GuiCanvas::renderFrame`'s first step, every child of the canvas (the
    /// content and the dialogs) resized to the canvas when its bounds differ. No drawing. Runs no script.
    pub fn frame(&self, vm: &mut Vm) {
        let (canvas, size) = {
            let state = self.0.borrow();
            (state.canvas, state.canvas_size)
        };
        let Some(canvas) = canvas.filter(|c| vm.objects().contains(*c)) else {
            return;
        };
        if size.0 <= 0 || size.1 <= 0 {
            return;
        }
        // Outside a run (the host's frame) the walk has a budget of its own; inside one (`Canvas.repaint()`) the run pays.
        let saved = vm.begin_host_budget(MAX_FRAME_WORK);
        control::set_bounds(vm, canvas, (0, 0), size);
        let children = control::children(vm, canvas);
        for child in children {
            if !vm.charge_work(64) {
                break;
            }
            if !vm.objects().contains(child) {
                continue;
            }
            let (pos, ext) = control::bounds(vm, child);
            if pos != (0, 0) || ext != size {
                control::resize(vm, self, child, (0, 0), size);
            }
        }
        let overlays = self.overlays(vm);
        for &o in &overlays {
            let (pos, ext) = control::bounds(vm, o);
            if pos != (0, 0) || ext != size {
                control::resize(vm, self, o, (0, 0), size);
            }
        }
        // The host layers of client scripts: sized to the canvas, the holds checked against what is on screen.
        client_script::frame(vm, self, size);
        // The render pass, as far as it runs scripts: the GuiFadeinBitmapCtrl clocks of the controls drawn (awake, and visible
        // with every parent: `renderChildControls` draws the visible children only), in drawing order.
        let mut calls: Vec<(ObjId, &'static [u8])> = Vec::new();
        // The plugins' controls shown, parents first: their `update` runs after the walk.
        let mut updates: Vec<ObjId> = Vec::new();
        let now = vm.sim_time();
        let mut stack: Vec<(ObjId, usize)> = control::children(vm, canvas)
            .into_iter()
            .rev()
            .map(|c| (c, 0))
            .collect();
        while let Some((id, depth)) = stack.pop() {
            if !vm.charge_work(64) {
                break;
            }
            if !self.is_awake(id) || !control::flag(vm, id, b"visible") {
                continue;
            }
            if control::is_a(vm, id, "GuiFadeinBitmapCtrl")
                && let Some(call) = classes::fade_callback(vm, self, id, now)
            {
                calls.push((id, call));
            }
            if self.is_custom(id) {
                updates.push(id);
            }
            if depth < MAX_TREE_DEPTH {
                stack.extend(
                    control::children(vm, id)
                        .into_iter()
                        .rev()
                        .map(|c| (c, depth + 1)),
                );
            }
        }
        vm.end_host_budget(saved);
        // The plugins' controls' `update`, under a budget of their own (one may run script); an error is printed and the
        // other controls still update.
        if !updates.is_empty() {
            let saved = vm.begin_host_budget_with_runs(MAX_FRAME_WORK, input::MAX_EVENT_STEPS);
            for id in updates {
                if vm.objects().contains(id)
                    && let Err(error) = custom::update(vm, self, id)
                {
                    vm.error(format!("update of control {id}: {error}"));
                }
            }
            vm.end_host_budget(saved);
        }
        self.0.borrow_mut().frames += 1;
        // What the draw caches withheld of the registered faces (`faces::sync`) is said once.
        let notes = self.0.borrow_mut().faces.take_notes();
        for note in notes {
            vm.print(note.as_bytes());
        }
        let fonts = self.0.borrow().fonts.0.clone();
        fonts.borrow_mut().next_frame();
        // `preRender` (`renderFrame` runs it over the canvas's children after the resize, before drawing): the dirty ML text
        // controls reflow (their `onResize` is a script call), the consoles follow the log, the console texts evaluate, under a
        // budget of their own.
        let saved = vm.begin_host_budget_with_runs(MAX_FRAME_WORK, input::MAX_EVENT_STEPS);
        let mut pre_error = None;
        for child in control::children(vm, canvas).into_iter().chain(overlays) {
            if vm.objects().contains(child)
                && let Err(e) = mltext::pre_render(vm, self, child)
            {
                pre_error = Some(e);
                break;
            }
        }
        vm.end_host_budget(saved);
        if pre_error.is_some() {
            return;
        }
        // The callbacks run with no borrow held, each as a script call of its own (`Con::executef`); one that ends its run (the
        // step budget) ends the frame's.
        for (id, method) in calls {
            if vm.objects().contains(id) && vm.call_method(id, method, &[]).is_err() {
                break;
            }
        }
    }

    /// The controls the script file `file` made (any case) and how many of them are top-level GUIs (no parent among them), as
    /// `(guis, controls)`.
    pub fn file_controls(&self, vm: &Vm, file: &[u8]) -> (usize, usize) {
        let state = self.0.borrow();
        let made: Vec<ObjId> = state
            .controls
            .iter()
            .filter(|(_, c)| c.file.eq_ignore_ascii_case(file))
            .map(|(id, _)| *id)
            .collect();
        let guis = made
            .iter()
            .filter(|id| {
                let parent = vm.objects().get(**id).and_then(|o| o.group());
                !parent.is_some_and(|p| {
                    state
                        .controls
                        .get(&p)
                        .is_some_and(|c| c.file.eq_ignore_ascii_case(file))
                })
            })
            .count();
        (guis, made.len())
    }

    /// The `GuiControlProfile` objects that exist.
    pub fn profile_count(&self, vm: &Vm) -> usize {
        let objects = vm.objects();
        let Some(class) = objects.classes.find(b"GuiControlProfile") else {
            return 0;
        };
        objects
            .iter()
            .filter(|(_, o)| objects.classes.is_a(o.class(), class))
            .count()
    }

    /// What plays the button profiles' sounds (`soundButtonDown`, `soundButtonOver`, a text edit's `deniedSound`): called with the
    /// AudioProfile when `$Pref::Audio::MenuSounds` is on.
    pub fn set_sound_hook(&self, hook: Option<input::SoundHook>) {
        self.0.borrow_mut().sound_hook = input::SoundHookSlot(hook);
    }

    /// One engine input event from the host ([`input::host_event`]): whether the canvas consumed it, and a callback's error.
    pub fn input_event(&self, vm: &mut Vm, ev: &input::InputEvent) -> (bool, Option<VmError>) {
        input::host_event(vm, self, ev)
    }

    /// The window's mouse at `pt` ([`input::host_mouse_move`]).
    pub fn mouse_moved(&self, vm: &mut Vm, pt: (i32, i32), modifier: u8) -> Option<VmError> {
        input::host_mouse_move(vm, self, pt, modifier)
    }

    /// The canvas's first responder (where keys go).
    pub fn first_responder(&self) -> Option<ObjId> {
        self.0.borrow().input.first_responder
    }

    /// Makes `createCanvas` use `size` instead of `$pref::Video::resolution` (headless runs: the default, 1280x720); None: the preference.
    pub fn force_canvas_size(&self, size: Option<(i32, i32)>) {
        self.0.borrow_mut().forced_size = size;
    }

    /// The canvas's content (its first child) and its dialogs (the other children, in order), with their layers.
    pub fn stack(&self, vm: &Vm) -> (Option<ObjId>, Vec<(ObjId, i32)>) {
        let Some(canvas) = self.canvas() else {
            return (None, Vec::new());
        };
        let children = control::children(vm, canvas);
        let state = self.0.borrow();
        let layer = |id: &ObjId| state.controls.get(id).map_or(0, |c| c.layer);
        let mut iter = children.into_iter();
        let content = iter.next();
        (content, iter.map(|id| (id, layer(&id))).collect())
    }
}

/// Gives `vm` this crate's GUI classes ([`specs`], each one `vm` has not yet), their behaviours and methods (replacing the stubs a
/// host's class table made), and returns the shared state.
pub fn register(vm: &mut Vm) -> Gui {
    specs::install(vm);
    let gui = Gui::default();
    control::register(vm, &gui);
    glide::register(vm, &gui);
    faces::register(vm, &gui);
    canvas::register(vm, &gui);
    classes::register(vm, &gui);
    gui
}

/// The text of `message`: the extension's, else the engine's.
pub(crate) fn message_text(gui: &Gui, message: &ext::GuiMessage<'_>) -> String {
    gui.ext()
        .message(message)
        .unwrap_or_else(|| message.engine_text())
}

/// An object's name as messages print it ("" for none).
pub(crate) fn name_of(vm: &Vm, id: ObjId) -> String {
    vm.objects()
        .get(id)
        .and_then(|o| o.name())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default()
}

pub(crate) use torque_ts_vm::fields::atob;
pub(crate) use torque_ts_vm::text::{atof, atoi};

/// A native's result for a script error that ends the run.
pub(crate) fn done(result: Result<(), VmError>) -> Native {
    match result {
        Ok(()) => Native::Void,
        Err(e) => Native::Fail(e),
    }
}
