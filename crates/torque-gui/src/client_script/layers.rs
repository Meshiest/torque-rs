//! The host layers: where the server layer sits in the draw order, which input it may get, what the engine layer and the chip
//! add, and what the realm holds (the cursor, a focused text field) while it is on screen.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

use torque_client_script_format::ui::Prop;
use torque_client_script_format::{MAX_EVENTS, Tier};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;
use web_time::Instant;

use super::controls::{NodeState, origin};
use super::events::{UiEvent, encode_events};
use crate::control::{self, children};
use crate::draw::DecodeAllowance;
use crate::input::MAX_EVENT_STEPS;
use crate::{Gui, MAX_FRAME_WORK, MAX_TREE_DEPTH, custom, name_of};

/// The most events kept for the realm between two frames; past it the oldest go.
pub const MAX_QUEUED_EVENTS: usize = 1024;
/// The most controls in the engine layer.
pub const MAX_ENGINE_LAYER: usize = 8;
/// The font faces of the roles `sans`, `mono` and `title`.
pub const DEFAULT_FACES: [&str; 3] = ["Arial", "Lucida Console", "Arial"];

/// One layer of a drawn frame, in the order the canvas draws them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerMark {
    /// The canvas's content.
    Content,
    /// The server layer.
    Server,
    /// One of the canvas's dialogs.
    Dialog,
    /// The engine layer (prompts).
    Engine,
    /// The chip.
    Chip,
}

/// A tier in the chip's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TierToggle {
    /// The tier.
    pub tier: Tier,
    /// Whether it is still granted. A tier switched off stays off for the connection.
    pub on: bool,
}

/// What the chip says about the realm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RealmView {
    /// The server's address.
    pub address: String,
    /// The server's name, plain text.
    pub name: String,
    /// The tiers the realm was granted, besides the base.
    pub tiers: Vec<TierToggle>,
}

/// A hold the realm can have besides the cursor and a focus: shown as an icon on the chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    /// The realm moves the camera.
    Camera,
    /// The realm has a game key bound.
    Key,
}

/// What the realm holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Holds {
    /// The cursor is on for the realm's controls.
    pub cursor: bool,
    /// The camera is the realm's.
    pub camera: bool,
    /// A game key is the realm's.
    pub key: bool,
}

/// What the player did at the chip that the host has to carry out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerAction {
    /// Take the tier away.
    Revoke(Tier),
    /// Hide the realm's controls for now (`true`) or show them again.
    Hide(bool),
    /// Stop server scripts for this connection and remember nothing (Shift+Escape).
    Stop,
    /// Stop server scripts for this connection and never run this server's again (the chip's "Turn off for this server").
    TurnOff,
}

/// Counts the host layers keep, for tests and the host's own checks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// The layout checks run (one per laid-out container a frame it was due), and the work they were charged.
    pub layout_updates: u64,
    /// See [`Stats::layout_updates`]: the `Vm::charge_work` units of the frames' layout checks.
    pub layout_work: u64,
    /// Input events made for the realm.
    pub input_events: u64,
    /// Escape key events a control of the realm was given (always 0).
    pub escape_keys: u64,
    /// Events dropped because the queue was full.
    pub dropped_events: u64,
    /// The `Vm::charge_work` units the applier's flushes were charged (the bytes of their records, and the resizes their ops
    /// made).
    pub apply_work: u64,
    /// Resizes of the layout's containers the applier left to the frame's layout check instead of laying out at once.
    pub deferred_layouts: u64,
    /// Images not shown because the realm's decoded cap had no room for them.
    pub refused_images: u64,
    /// The `ui` records the applier applied.
    pub applied_records: u64,
}

/// How the cursor was when the realm turned it on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SavedCursor {
    on: bool,
    /// [`crate::GuiState::cursor_calls`] then: a call of the game's since means the cursor is the game's again.
    calls: u64,
}

/// A set of key codes.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct KeySet([u64; 4]);

impl KeySet {
    pub(crate) fn insert(&mut self, key: u8) {
        self.0[usize::from(key >> 6)] |= 1 << (key & 63);
    }

    /// Takes `key` out: whether it was in.
    pub(crate) fn take(&mut self, key: u8) -> bool {
        let was = self.contains(key);
        self.0[usize::from(key >> 6)] &= !(1 << (key & 63));
        was
    }

    pub(crate) fn contains(&self, key: u8) -> bool {
        self.0[usize::from(key >> 6)] & (1 << (key & 63)) != 0
    }
}

/// How [`release_holds`] ends the holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Release {
    /// The player or the game took them (Escape, a dialog, a prompt, another screen): the realm hears `closed` if it held
    /// anything, and then `ui.interactive(true)` waits for the player's next action on its controls.
    Closed,
    /// The host ended them (a revoke, the teardown): nothing is said.
    Quiet,
}

/// The state of the host layers, in the GUI's state.
#[derive(Debug)]
pub struct LayerState {
    /// The names of the content screens that show server controls, lower case.
    pub(crate) screens: Vec<String>,
    /// The server layer's container; mod roots are its children.
    pub(crate) server: Option<ObjId>,
    /// The engine layer's controls, bottom first.
    pub(crate) engine: Vec<ObjId>,
    /// The chip.
    pub(crate) chip: Option<ObjId>,
    pub(crate) view: Option<RealmView>,
    pub(crate) hidden: bool,
    pub(crate) off: bool,
    pub(crate) holds: Holds,
    /// `ui.interactive(true)` is in force.
    pub(crate) interactive: bool,
    /// The player or the game took the holds away (Escape, a dialog): `ui.interactive(true)` is refused until the player
    /// acts on a control of the realm again.
    pub(crate) interactive_blocked: bool,
    /// How the cursor was before the realm turned it on.
    pub(crate) cursor_saved: Option<SavedCursor>,
    /// How the cursor was before the consent prompt turned it on (a prompt is modal and takes the pointer only while the cursor is
    /// on, which a world played with the mouse has off).
    pub(crate) prompt_cursor: Option<SavedCursor>,
    pub(crate) focus: Option<ObjId>,
    pub(crate) hover: Option<ObjId>,
    pub(crate) captured: Option<ObjId>,
    /// The keys whose press the focused field took: only their releases are kept from the game.
    pub(crate) field_keys: KeySet,
    /// The keys whose press an engine prompt took, likewise.
    pub(crate) prompt_keys: KeySet,
    /// The chip's column (x, y, width, height): the server layer is neither drawn nor hit there, so nothing the realm draws
    /// can pass for the chip or its menu. Set each frame.
    pub(crate) column: Option<[i32; 4]>,
    /// Whether a control of the realm is visible ([`anything_visible`]), until the tree changes.
    pub(crate) visible: Option<bool>,
    pub(crate) events: VecDeque<UiEvent>,
    pub(crate) actions: Vec<LayerAction>,
    /// The host's monotonic clock, microseconds since the realm started.
    pub(crate) now_us: u64,
    pub(crate) activation_us: Option<u64>,
    /// An Escape press was taken, so its release and repeats are too.
    pub(crate) escape_swallow: bool,
    /// The server area's size as the realm was last told.
    pub(crate) screen: (i32, i32),
    pub(crate) faces: [String; 3],
    pub(crate) stats: Stats,
    /// The containers of the layout plugin's classes the realm made, oldest first. They are not canvas children, so the GUI's
    /// frame does not reach them: [`frame`] runs their `update` (a layout checks whether its inputs changed there).
    pub(crate) laid: BTreeSet<ObjId>,
    /// The containers to be checked, with how many more frames each is: set for the containers a change is inside (the applier
    /// is the only writer of a container's fields and of the controls in it) and for all of them by a resize, counting down
    /// each frame the container is checked. Two frames: one lays out, the next sees that it has settled. A container not here
    /// costs a frame nothing, and with none here a frame does no layout work at all.
    pub(crate) layout_due: BTreeMap<ObjId, u8>,
    /// The applier is applying a flush: a resize of a container of the layout's only marks it to be laid out by the frame's
    /// check ([`defer_layout`]), so no op of the flush lays a layout out at once.
    pub(crate) applying: bool,
    /// What the realm's images may still decode.
    pub(crate) decode: DecodeAllowance,
    /// The time the host spent on the realm's behalf outside its frame (its layouts, its image decodes) and has not charged
    /// to it yet ([`Gui::take_client_script_owed`]).
    pub(crate) owed: Duration,
}

/// The frames a container is checked for after something it reads changed.
const LAYOUT_FRAMES: u8 = 2;

/// Something the layouts of these containers read changed: check them for the next frames. Controls that are not the layout's
/// containers are ignored.
pub(crate) fn mark_layouts(gui: &Gui, containers: impl IntoIterator<Item = ObjId>) {
    let l = &mut gui.0.borrow_mut().client_script;
    for id in containers {
        if l.laid.contains(&id) {
            l.layout_due.insert(id, LAYOUT_FRAMES);
        }
    }
}

/// A resize of `id` while the applier applies a flush, when `id` is a container of the layout's: its bounds are set and it and
/// the containers around it are checked by the frame's layout pass, which lays each out once however many ops resized it.
/// False (nothing done) for any other control or time.
pub(crate) fn defer_layout(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    position: (i32, i32),
    extent: (i32, i32),
) -> bool {
    {
        let l = &gui.0.borrow().client_script;
        if !l.applying || !l.laid.contains(&id) {
            return false;
        }
    }
    control::set_bounds(vm, id, position, extent);
    let mut around = vec![id];
    let mut at = id;
    while around.len() <= MAX_TREE_DEPTH
        && let Some(p) = control::parent(vm, at)
    {
        around.push(p);
        at = p;
    }
    mark_layouts(gui, around);
    gui.0.borrow_mut().client_script.stats.deferred_layouts += 1;
    gui.touch_custom();
    true
}

/// Something every layout reads changed (the layer's size): check all the containers.
fn mark_all_layouts(gui: &Gui) {
    let LayerState {
        laid, layout_due, ..
    } = &mut gui.0.borrow_mut().client_script;
    layout_due.extend(laid.iter().map(|id| (*id, LAYOUT_FRAMES)));
}

impl Default for LayerState {
    fn default() -> LayerState {
        LayerState {
            screens: Vec::new(),
            server: None,
            engine: Vec::new(),
            chip: None,
            view: None,
            hidden: false,
            off: false,
            holds: Holds::default(),
            interactive: false,
            interactive_blocked: false,
            cursor_saved: None,
            prompt_cursor: None,
            focus: None,
            hover: None,
            captured: None,
            field_keys: KeySet::default(),
            prompt_keys: KeySet::default(),
            column: None,
            visible: None,
            events: VecDeque::new(),
            actions: Vec::new(),
            now_us: 0,
            activation_us: None,
            escape_swallow: false,
            screen: (0, 0),
            faces: DEFAULT_FACES.map(str::to_string),
            stats: Stats::default(),
            laid: BTreeSet::new(),
            layout_due: BTreeMap::new(),
            applying: false,
            decode: DecodeAllowance::default(),
            owed: Duration::ZERO,
        }
    }
}

impl LayerState {
    /// Whether a realm is on this GUI and not turned off.
    pub(crate) fn active(&self) -> bool {
        self.server.is_some() && self.view.is_some() && !self.off
    }

    pub(crate) fn holds_any(&self) -> bool {
        self.holds.cursor || self.holds.camera || self.holds.key || self.focus.is_some()
    }

    /// Queues an event for the realm.
    pub(crate) fn emit(&mut self, event: UiEvent) {
        if event.is_input() {
            self.stats.input_events += 1;
        }
        if self.events.len() >= MAX_QUEUED_EVENTS {
            self.events.pop_front();
            self.stats.dropped_events += 1;
        }
        self.events.push_back(event);
    }

    /// The player acted on a control of the realm (or pressed the game's key for its menu): `ui.focus` is let through for a while,
    /// and so is `ui.interactive(true)` after an Escape took the cursor ([`LayerState::activated`]).
    pub(crate) fn activate(&mut self) {
        self.activation_us = Some(self.now_us);
    }

    /// Drops the focus. The keys the field took stay taken: their releases are not the game's.
    pub(crate) fn blur(&mut self) {
        self.focus = None;
    }

    /// Whether the point is in the chip's column.
    pub(crate) fn in_column(&self, pt: (i32, i32)) -> bool {
        self.column.is_some_and(|[x, y, w, h]| {
            (x..x.saturating_add(w)).contains(&pt.0) && (y..y.saturating_add(h)).contains(&pt.1)
        })
    }

    /// Whether the rectangle (x, y, width, height) meets the chip's column.
    pub(crate) fn meets_column(&self, r: [i32; 4]) -> bool {
        self.column.is_some_and(|c| {
            let i = crate::draw::intersect_rect(c, r);
            i[2] > 0 && i[3] > 0
        })
    }

    /// Whether `ui.focus` is let through now.
    pub(crate) fn activated(&self) -> bool {
        self.activation_us.is_some_and(|at| {
            self.now_us.saturating_sub(at) <= torque_client_script_format::ui::ACTIVATION_US
        })
    }
}

/// The content screen's name, lower case ("" for none).
fn content_name(vm: &Vm, gui: &Gui) -> String {
    let (content, _) = gui.stack(vm);
    content
        .map(|c| name_of(vm, c).to_ascii_lowercase())
        .unwrap_or_default()
}

/// Whether the canvas's content is one of the screens that host server controls.
fn content_is_marked(vm: &Vm, gui: &Gui) -> bool {
    let name = content_name(vm, gui);
    !name.is_empty() && gui.0.borrow().client_script.screens.contains(&name)
}

/// Whether a game dialog is up.
pub(crate) fn dialogs_up(vm: &Vm, gui: &Gui) -> bool {
    !gui.stack(vm).1.is_empty()
}

/// Whether an engine prompt is up.
pub(crate) fn prompt_up(gui: &Gui) -> bool {
    !gui.0.borrow().client_script.engine.is_empty()
}

/// Whether the server layer is drawn: a realm, not hidden, over a marked content screen.
pub(crate) fn server_shown(vm: &Vm, gui: &Gui) -> bool {
    let (active, hidden) = {
        let s = gui.0.borrow();
        (s.client_script.active(), s.client_script.hidden)
    };
    active && !hidden && content_is_marked(vm, gui)
}

/// Whether the server layer may be given input: shown, and no dialog or prompt over it.
pub(crate) fn server_takes_input(vm: &Vm, gui: &Gui) -> bool {
    server_shown(vm, gui) && !dialogs_up(vm, gui) && !prompt_up(gui)
}

/// Whether a control other than a mod root is visible under the server layer (it and every ancestor to the container). Kept
/// until the realm's tree changes ([`invalidate`]).
pub(crate) fn anything_visible(vm: &Vm, gui: &Gui) -> bool {
    let (root, known) = {
        let s = gui.0.borrow();
        (s.client_script.server, s.client_script.visible)
    };
    let Some(root) = root else {
        return false;
    };
    if let Some(known) = known {
        return known;
    }
    let found = find_visible(vm, gui, root);
    gui.0.borrow_mut().client_script.visible = Some(found);
    found
}

/// The realm's tree changed: what was worked out from it is worked out again.
pub(crate) fn invalidate(gui: &Gui) {
    gui.0.borrow_mut().client_script.visible = None;
}

fn find_visible(vm: &Vm, gui: &Gui, root: ObjId) -> bool {
    let mut stack: Vec<(ObjId, usize)> = vec![(root, 0)];
    let mut budget = torque_client_script_format::ui::MAX_CONTROLS * 2;
    while let Some((id, depth)) = stack.pop() {
        budget = budget.saturating_sub(1);
        if budget == 0 {
            return false;
        }
        if id != root && !control::flag(vm, id, b"visible") {
            continue;
        }
        if gui
            .with_custom_state::<NodeState, _>(id, |s| s.kind.is_some())
            .unwrap_or(false)
        {
            return true;
        }
        if depth < MAX_TREE_DEPTH {
            stack.extend(children(vm, id).into_iter().map(|c| (c, depth + 1)));
        }
    }
    false
}

/// Whether the chip is drawn: whenever the realm has something on screen, holds something, or is hidden.
pub(crate) fn chip_wanted(vm: &Vm, gui: &Gui) -> bool {
    let (active, hidden, holds) = {
        let s = gui.0.borrow();
        let l = &s.client_script;
        (l.active() && l.chip.is_some(), l.hidden, l.holds_any())
    };
    active && (hidden || holds || (server_shown(vm, gui) && anything_visible(vm, gui)))
}

/// What the canvas draws of the host layers in one frame.
#[derive(Clone, Debug, Default)]
pub struct DrawPlan {
    /// The server layer's container, between the content and the dialogs.
    pub(crate) server: Option<ObjId>,
    /// The engine layer's controls.
    pub(crate) engine: Vec<ObjId>,
    /// The chip, drawn last.
    pub(crate) chip: Option<ObjId>,
}

impl DrawPlan {
    /// The plan for the GUI as it is now.
    pub fn of(vm: &Vm, gui: &Gui) -> DrawPlan {
        let (server, engine, chip) = {
            let s = gui.0.borrow();
            let l = &s.client_script;
            if l.server.is_none() && l.engine.is_empty() {
                return DrawPlan::default();
            }
            (l.server, l.engine.clone(), l.chip)
        };
        DrawPlan {
            server: server.filter(|_| server_shown(vm, gui)),
            engine: engine
                .into_iter()
                .filter(|id| vm.objects().contains(*id) && gui.is_awake(*id))
                .collect(),
            chip: chip.filter(|_| chip_wanted(vm, gui)),
        }
    }
}

/// Gives back what the realm held: the cursor (as it was), the focus and the cursor request; `how` says whether the realm is
/// told and must wait for the player before it asks for the cursor again.
pub(crate) fn release_holds(gui: &Gui, how: Release) {
    let mut s = gui.0.borrow_mut();
    let s = &mut *s;
    let l = &mut s.client_script;
    let had = l.holds_any() || l.interactive;
    l.interactive = false;
    l.blur();
    l.holds = Holds::default();
    l.captured = None;
    restore_cursor(&mut s.cursor_on, s.cursor_calls, l.cursor_saved.take());
    if had && how == Release::Closed {
        l.interactive_blocked = true;
        l.emit(UiEvent::Closed);
    }
}

/// Puts the cursor back as `saved` has it, unless the game turned it on or off since the realm took it.
fn restore_cursor(cursor_on: &mut bool, calls: u64, saved: Option<SavedCursor>) {
    if let Some(saved) = saved
        && saved.calls == calls
    {
        *cursor_on = saved.on;
    }
}

/// Turns the cursor on for the realm, remembering how it was.
fn take_cursor(gui: &Gui) {
    let mut s = gui.0.borrow_mut();
    let s = &mut *s;
    let l = &mut s.client_script;
    if l.holds.cursor {
        return;
    }
    l.cursor_saved = Some(SavedCursor {
        on: s.cursor_on,
        calls: s.cursor_calls,
    });
    s.cursor_on = true;
    l.holds.cursor = true;
}

/// Turns the cursor on for the consent prompt, remembering how it was.
pub(crate) fn take_prompt_cursor(gui: &Gui) {
    let mut s = gui.0.borrow_mut();
    let s = &mut *s;
    if s.client_script.prompt_cursor.is_some() {
        return;
    }
    s.client_script.prompt_cursor = Some(SavedCursor {
        on: s.cursor_on,
        calls: s.cursor_calls,
    });
    s.cursor_on = true;
}

/// Keeps the cursor on while the prompt is up. A game that turns the cursor off meanwhile (a world played with the mouse does, as
/// it starts) is heard: what it asked for is what comes back when the prompt goes.
pub(crate) fn hold_prompt_cursor(gui: &Gui) {
    let mut s = gui.0.borrow_mut();
    let s = &mut *s;
    if let Some(saved) = s.client_script.prompt_cursor.as_mut() {
        if s.cursor_calls != saved.calls {
            saved.on = s.cursor_on;
            saved.calls = s.cursor_calls;
        }
        s.cursor_on = true;
    }
}

/// Puts the cursor back as it was before the prompt, unless the game turned it on or off since.
pub(crate) fn give_back_prompt_cursor(gui: &Gui) {
    let mut s = gui.0.borrow_mut();
    let s = &mut *s;
    restore_cursor(
        &mut s.cursor_on,
        s.cursor_calls,
        s.client_script.prompt_cursor.take(),
    );
}

/// Gives back the cursor the realm took.
fn give_back_cursor(gui: &Gui) {
    let mut s = gui.0.borrow_mut();
    let s = &mut *s;
    let l = &mut s.client_script;
    if !l.holds.cursor {
        return;
    }
    l.holds.cursor = false;
    restore_cursor(&mut s.cursor_on, s.cursor_calls, l.cursor_saved.take());
}

/// Stops server scripts for the connection: the layer goes at once, the realm is told to stop by `action`
/// ([`LayerAction::Stop`] or [`LayerAction::TurnOff`]).
pub(crate) fn turn_off(gui: &Gui, action: LayerAction) {
    release_holds(gui, Release::Closed);
    let mut s = gui.0.borrow_mut();
    s.client_script.off = true;
    s.client_script.actions.push(action);
}

/// One frame of the host layers: the layers sized to the canvas, the realm told the screen size, the chip laid out, and the
/// holds checked against what is on screen.
pub(crate) fn frame(vm: &mut Vm, gui: &Gui, size: (i32, i32)) {
    hold_prompt_cursor(gui);
    let (server, engine) = {
        let s = gui.0.borrow();
        (s.client_script.server, s.client_script.engine.clone())
    };
    for id in server.into_iter().chain(engine) {
        if !vm.objects().contains(id) {
            continue;
        }
        let (pos, ext) = control::bounds(vm, id);
        if pos != (0, 0) || ext != size {
            control::resize(vm, gui, id, (0, 0), size);
            mark_all_layouts(gui);
        }
    }
    if server.is_none() {
        return;
    }
    invalidate(gui);
    {
        let mut s = gui.0.borrow_mut();
        let l = &mut s.client_script;
        if l.screen != size {
            l.screen = size;
            l.emit(UiEvent::Screen(f64::from(size.0), f64::from(size.1), 1.0));
        }
    }
    enforce_holds(vm, gui);
    super::chip::layout(vm, gui, size);
    update_layouts(vm, gui);
}

/// The layout plugin's per-frame check for the containers the realm made that are on screen. A container inside another does
/// nothing in its own `update`; the outermost compares what its layout read with what is there now and lays out on a difference.
fn update_layouts(vm: &mut Vm, gui: &Gui) {
    // An idle realm does nothing here: no fingerprint is read unless something changed (`layout_due`). A layer that is not
    // shown keeps what is owed until it is.
    let due: Vec<ObjId> = gui
        .0
        .borrow()
        .client_script
        .layout_due
        .keys()
        .copied()
        .collect();
    if due.is_empty() || !server_shown(vm, gui) {
        return;
    }
    let started = Instant::now();
    let saved = vm.begin_host_budget_with_runs(MAX_FRAME_WORK, MAX_EVENT_STEPS);
    let mut runs = 0;
    for &id in &due {
        // No borrow is held across `update`, and a layout may be removed meanwhile.
        if vm.objects().contains(id) && gui.is_awake(id) {
            runs += 1;
            if let Err(error) = custom::update(vm, gui, id) {
                vm.error(format!("update of control {id}: {error}"));
            }
        }
    }
    let work = vm.end_host_budget(saved);
    let l = &mut gui.0.borrow_mut().client_script;
    for id in due {
        if let Some(frames) = l.layout_due.get_mut(&id) {
            *frames -= 1;
            if *frames == 0 {
                l.layout_due.remove(&id);
            }
        }
    }
    l.stats.layout_updates += runs;
    l.stats.layout_work += work;
    l.owed += started.elapsed();
}

/// Drops what the realm may not hold now: everything when its layer is not shown or a dialog or prompt is over it, the focus
/// when its control is gone or hidden or the cursor is off; and gives it the cursor it asked for when it may have it.
fn enforce_holds(vm: &Vm, gui: &Gui) {
    if !server_shown(vm, gui) || dialogs_up(vm, gui) || prompt_up(gui) {
        release_holds(gui, Release::Closed);
        gui.0.borrow_mut().client_script.hover = None;
        return;
    }
    let (focus, cursor_on) = {
        let s = gui.0.borrow();
        (s.client_script.focus, s.cursor_on)
    };
    if let Some(f) = focus
        && (!cursor_on || !effectively_visible(vm, gui, f, None))
    {
        gui.0.borrow_mut().client_script.blur();
    }
    if gui.0.borrow().client_script.interactive && anything_visible(vm, gui) {
        take_cursor(gui);
    } else {
        give_back_cursor(gui);
    }
}

/// The least opacity, own and ancestors' together, at which a text field counts as seen.
pub(crate) const MIN_OPACITY: f64 = 0.5;
/// The least width and height, in pixels, at which a text field counts as seen.
pub(crate) const MIN_SIDE: i32 = 8;

/// Whether the control `id` is on screen to be seen: visible with every ancestor, at least [`MIN_SIDE`] square, its rectangle on
/// the canvas, inside every ancestor's (drawing clips to them) and clear of the chip's column (the layer is not drawn there),
/// and [`MIN_OPACITY`] or more of opacity in all. `own_opacity` is the control's own when its state is in use by the caller. A
/// realm text field takes focus only while this holds, so it cannot collect typing it cannot be seen collecting.
pub(crate) fn effectively_visible(vm: &Vm, gui: &Gui, id: ObjId, own_opacity: Option<f64>) -> bool {
    if !vm.objects().contains(id) || !visible_chain(vm, id) {
        return false;
    }
    let (_, ext) = control::bounds(vm, id);
    if ext.0 < MIN_SIDE || ext.1 < MIN_SIDE {
        return false;
    }
    let at = origin(vm, gui, id);
    let inside = |outer: ((i32, i32), (i32, i32))| {
        let ((ox, oy), (ow, oh)) = outer;
        at.0 >= ox && at.1 >= oy && at.0 + ext.0 <= ox + ow && at.1 + ext.1 <= oy + oh
    };
    {
        let s = gui.0.borrow();
        if !inside(((0, 0), s.canvas_size))
            || s.client_script.meets_column([at.0, at.1, ext.0, ext.1])
        {
            return false;
        }
    }
    let opacity_of = |c: ObjId| {
        gui.with_custom_state::<NodeState, _>(c, |s| s.props.num(Prop::Opacity))
            .unwrap_or(1.0)
    };
    let mut opacity = own_opacity.unwrap_or_else(|| opacity_of(id));
    let mut current = control::parent(vm, id);
    for _ in 0..=MAX_TREE_DEPTH {
        let Some(c) = current else {
            break;
        };
        opacity *= opacity_of(c);
        if !inside((origin(vm, gui, c), control::bounds(vm, c).1)) {
            return false;
        }
        current = control::parent(vm, c);
    }
    opacity >= MIN_OPACITY
}

/// The focused text field and where it is on the canvas, when it is to be drawn and hit above every other control of the
/// server layer: while a field holds focus nothing the realm draws can cover it.
pub(crate) fn focus_on_top(vm: &Vm, gui: &Gui) -> Option<(ObjId, (i32, i32))> {
    let id = gui.0.borrow().client_script.focus?;
    effectively_visible(vm, gui, id, None).then(|| (id, origin(vm, gui, id)))
}

/// Whether `id` and every ancestor up to the server layer are visible.
pub(crate) fn visible_chain(vm: &Vm, id: ObjId) -> bool {
    let mut at = Some(id);
    for _ in 0..=MAX_TREE_DEPTH {
        let Some(c) = at else {
            return true;
        };
        if !control::flag(vm, c, b"visible") {
            return false;
        }
        at = control::parent(vm, c);
    }
    false
}

/// A control left the model: the layers forget it.
pub(crate) fn forget(gui: &Gui, id: ObjId) {
    let mut state = gui.0.borrow_mut();
    let s = &mut *state;
    let l = &mut s.client_script;
    for slot in [&mut l.focus, &mut l.hover, &mut l.captured] {
        if *slot == Some(id) {
            *slot = None;
        }
    }
    let was_prompt = l.engine.contains(&id);
    l.engine.retain(|e| *e != id);
    // A prompt that goes any way but `close_client_script_prompt` takes its hold on the cursor with it.
    if was_prompt && l.engine.is_empty() {
        let saved = l.prompt_cursor.take();
        restore_cursor(&mut s.cursor_on, s.cursor_calls, saved);
    }
    let l = &mut s.client_script;
    l.laid.remove(&id);
    l.layout_due.remove(&id);
    l.visible = None;
    if l.chip == Some(id) {
        l.chip = None;
    }
    if l.server == Some(id) {
        l.server = None;
    }
}

impl Gui {
    /// Names the content screens that show server controls (`PlayGui`, say): on any other screen the realm's controls are
    /// hidden and get no input.
    pub fn set_client_script_screens(&self, names: &[&str]) {
        self.0.borrow_mut().client_script.screens =
            names.iter().map(|n| n.to_ascii_lowercase()).collect();
    }

    /// Sets the font faces of the roles `sans`, `mono` and `title`.
    pub fn set_client_script_faces(&self, faces: [&str; 3]) {
        self.0.borrow_mut().client_script.faces = faces.map(str::to_string);
        self.touch_custom();
    }

    /// Sets the realm's clock, in microseconds since it started (the same value its entry calls carry).
    pub fn set_client_script_clock(&self, now_us: u64) {
        let l = &mut self.0.borrow_mut().client_script;
        l.now_us = l.now_us.max(now_us);
    }

    /// Whether a realm is on this GUI and not turned off.
    pub fn client_script_active(&self) -> bool {
        self.0.borrow().client_script.active()
    }

    /// Whether the server layer is drawn now.
    pub fn client_script_shown(&self, vm: &Vm) -> bool {
        server_shown(vm, self)
    }

    /// What the realm holds now.
    pub fn client_script_holds(&self) -> Holds {
        self.0.borrow().client_script.holds
    }

    /// The time the GUI spent on the realm's behalf since the last call (laying out its containers, decoding its images): the
    /// host charges it to the realm's frame budget.
    pub fn take_client_script_owed(&self) -> Duration {
        std::mem::take(&mut self.0.borrow_mut().client_script.owed)
    }

    /// The counts the layers keep.
    pub fn client_script_stats(&self) -> Stats {
        self.0.borrow().client_script.stats
    }

    /// Sets or drops a hold other tiers take (the camera, a game key); the chip shows it.
    pub fn client_script_hold(&self, hold: Hold, on: bool) {
        let holds = &mut self.0.borrow_mut().client_script.holds;
        match hold {
            Hold::Camera => holds.camera = on,
            Hold::Key => holds.key = on,
        }
    }

    /// The Escape filter of the host layers for `ev`, for a host that takes input before the canvas (an action map's binding of
    /// Escape would otherwise see the key first): true when the key is taken here (Shift+Escape stops server scripts; Escape with
    /// the realm holding the cursor, a focus or the camera drops them and tells it `closed`; an engine prompt takes it). A key this
    /// does not take goes on to the game as usual.
    pub fn client_script_escape(&self, ev: &crate::input::InputEvent) -> bool {
        ev.device == crate::keys::KEYBOARD_DEVICE
            && ev.obj_inst == crate::keys::KEY_ESCAPE
            && super::route::escape(self, ev)
    }

    /// The player pressed a key the game bound to open the realm's menu: the realm may ask for the cursor again (`ui.interactive(true)`),
    /// which Escape had taken from it until the player acted on its controls, and may focus a field for a second. Only the game's own
    /// code calls this, from the key it chose; no script reaches it and a server cannot make it happen.
    pub fn client_script_player_asked(&self) {
        self.0.borrow_mut().client_script.activate();
    }

    /// Takes the events made for the realm since the last call.
    pub fn take_client_script_events(&self) -> Vec<UiEvent> {
        std::mem::take(&mut self.0.borrow_mut().client_script.events).into()
    }

    /// Takes up to [`MAX_EVENTS`] events as the batch the realm's next frame takes; the rest stay queued.
    pub fn take_client_script_batch(&self) -> Vec<u8> {
        let mut s = self.0.borrow_mut();
        let events = &mut s.client_script.events;
        let batch: Vec<UiEvent> = events.drain(..events.len().min(MAX_EVENTS)).collect();
        encode_events(&batch)
    }

    /// Takes what the player asked of the chip since the last call.
    pub fn take_client_script_actions(&self) -> Vec<LayerAction> {
        std::mem::take(&mut self.0.borrow_mut().client_script.actions)
    }

    /// Puts `id` (a control outside the canvas) in the engine layer: drawn over every dialog, awake, sized to the canvas, and
    /// given input before anything else. While one is there the server layer takes no input. False when it is not a control, is
    /// in the canvas or another control, or the layer is full.
    pub fn push_engine_layer(&self, vm: &mut Vm, id: ObjId) -> bool {
        let in_tree = vm
            .objects()
            .get(id)
            .and_then(|o| o.group())
            .is_some_and(|g| self.is_control(g) || Some(g) == self.canvas());
        {
            let s = self.0.borrow();
            let l = &s.client_script;
            if in_tree
                || !self.is_control(id)
                || l.engine.len() >= MAX_ENGINE_LAYER
                || l.engine.contains(&id)
            {
                return false;
            }
        }
        self.0.borrow_mut().client_script.engine.push(id);
        control::awaken(vm, self, id).is_ok()
    }

    /// Takes `id` out of the engine layer.
    pub fn pop_engine_layer(&self, vm: &mut Vm, id: ObjId) {
        let was = {
            let l = &mut self.0.borrow_mut().client_script;
            let before = l.engine.len();
            l.engine.retain(|e| *e != id);
            l.engine.len() != before
        };
        if was {
            let _ = control::sleep(vm, self, id);
        }
    }
}
