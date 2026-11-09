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

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Native, NativeCall, Vm, VmError};

pub mod backdrop;
pub mod canvas;
mod classes;
pub mod client_script;
pub mod console;
mod control;
pub mod custom;
pub mod draw;
pub mod ext;
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

/// The host's extension of the GUI ([`GuiExtension`]; [`NoExtension`] by default).
#[derive(Clone)]
pub struct Extension(pub Rc<dyn GuiExtension>);

impl Default for Extension {
    fn default() -> Extension {
        Extension(Rc::new(NoExtension))
    }
}

impl std::fmt::Debug for Extension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Extension")
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
    pub controls: HashMap<ObjId, Control>,
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
    /// The host's extension ([`GuiExtension`]).
    pub ext: Extension,
    /// The control classes plugins added and their controls ([`custom`]).
    pub(crate) custom: custom::Custom,
    /// The host's overlays ([`Gui::set_overlay`]): controls drawn over the canvas's children, never members of the canvas, so a
    /// script's `Canvas.getCount()` and `Canvas.getObject(%i)` count only what the scripts put there.
    pub overlays: Vec<ObjId>,
    /// The host layers of client scripts: the server layer, the engine layer and the chip ([`client_script`]).
    pub client_script: client_script::LayerState,
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
            controls: HashMap::new(),
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
        }
    }
}

/// The handle the natives share. Cloning it shares the state.
#[derive(Clone, Default)]
pub struct Gui(pub Rc<RefCell<GuiState>>);

impl Gui {
    /// The host's extension (held outside the state's borrow while it runs).
    pub fn ext(&self) -> Rc<dyn GuiExtension> {
        Rc::clone(&self.0.borrow().ext.0)
    }

    /// The extension as its own type, when it is one.
    pub fn extension<T: GuiExtension>(&self) -> Option<Rc<T>> {
        self.ext().into_any().downcast::<T>().ok()
    }

    /// Sets the host's extension and the layout's font source it gives (if any).
    pub fn set_extension(&self, ext: Rc<dyn GuiExtension>) {
        let fonts = ext.fonts();
        let mut s = self.0.borrow_mut();
        if s.canvas.is_none() {
            s.canvas_size = ext.default_canvas_size();
        }
        if let Some(f) = fonts {
            s.fonts = SharedFonts::new(f);
        }
        s.ext = Extension(ext);
    }

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

/// The object of a method call (`%this`), if it exists.
pub(crate) fn this(vm: &Vm, c: &NativeCall) -> Option<ObjId> {
    c.this_object.filter(|id| vm.objects().contains(*id))
}

/// Argument `i` of a call (`argv[i]`), "" when absent.
pub(crate) fn arg(c: &NativeCall, i: usize) -> &[u8] {
    c.argv.get(i).map_or(&[][..], Vec::as_slice)
}

/// C `atoi` as the engine's runtime does it (`torque_ts_vm::text::atoi`: white space, a sign, digits, saturating): one pass.
pub(crate) fn atoi(text: &[u8]) -> i32 {
    torque_ts_vm::text::atoi(text)
}

/// `dAtof`: C's `atof` as the runtime reads it (`torque_ts_vm::text::atof`: white space, a sign, `inf`, `nan`, hex, or
/// digits with an optional `.` and an exponent that counts only with a digit). The text is scanned once and the number parsed once,
/// so its cost is linear in the input (a hostile megabyte of digits included).
pub(crate) fn atof(text: &[u8]) -> f64 {
    torque_ts_vm::text::atof(text)
}

/// `dAtob` (`torque_ts_vm::fields::atob`): "true" in any case, or a number that is not zero as a float.
pub(crate) fn atob(text: &[u8]) -> bool {
    torque_ts_vm::fields::atob(text)
}

/// A native's result for a script error that ends the run.
pub(crate) fn done(result: Result<(), VmError>) -> Native {
    match result {
        Ok(()) => Native::Void,
        Err(e) => Native::Fail(e),
    }
}
