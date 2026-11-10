//! The consent prompt the engine ships: a modal box in the engine layer that asks whether a server may run its own scripts, one
//! row for each ability it asks for, and three buttons. A game may draw its own instead (the engine's consent API only needs
//! the answer); whoever draws it, it sits in the engine layer, above every dialog and the server layer, and the server layer
//! gets no input while it is up.
//!
//! **Input.** The prompt ignores the pointer for [`IGNORE_MS`] after it is shown, so a click the player was making when it
//! appeared does not answer it. A button, row or the remember box acts when the press and the release are on the same one. No
//! key answers it: Enter does not mean Allow (a server could time a game dialog to put a click there), and Escape is the host's.
//! A press anywhere else on the prompt's layer is swallowed.

use std::collections::HashSet;

use torque_client_script_format::is_hidden;
use torque_engine::EngineClass;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use super::chip::fitted;
use crate::Gui;
use crate::control::{self, make_realm_control};
use crate::custom::{ControlCx, GuiControlClass, Place, PointerEvent};
use crate::draw::DrawCtx;

/// The prompt's class name.
pub(crate) const CLASS: &str = "ClientScriptPrompt";

/// How long after it is shown the prompt ignores the pointer, in milliseconds.
pub const IGNORE_MS: u32 = 500;
/// The most rows a prompt shows.
pub const MAX_ROWS: usize = 16;
/// The most characters of any one line of text the prompt shows.
pub const MAX_LINE: usize = 160;

const FACE_SIZE: i32 = 13;
const PAD: i32 = 12;
const LINE: i32 = 20;
const ROW: i32 = 24;
const BUTTON_HEIGHT: i32 = 30;
const GAP: i32 = 8;
const BACK: [u8; 4] = [22, 24, 30, 250];
const DIM_BACK: [u8; 4] = [0, 0, 0, 150];
const EDGE: [u8; 4] = [52, 120, 220, 255];
const TEXT: [u8; 4] = [235, 238, 242, 255];
const MUTED: [u8; 4] = [140, 146, 158, 255];
const BUTTON: [u8; 4] = [44, 52, 70, 255];
const ALLOW: [u8; 4] = [38, 96, 62, 255];
const NEVER: [u8; 4] = [104, 44, 44, 255];
const BOX_BACK: [u8; 4] = [12, 14, 18, 255];
/// A check box's side, and the room it takes with the gap to its label.
const BOX: i32 = 16;
pub(super) const BOX_ROOM: i32 = 24;
/// The accent of a ticked box (the blue of the prompt's outline), lighter under the pointer; a locked box is muted.
const BOX_ON: [u8; 4] = [52, 120, 220, 255];
const BOX_ON_HOT: [u8; 4] = [84, 146, 238, 255];
const BOX_HOT: [u8; 4] = [96, 156, 240, 255];
const BOX_LOCKED: [u8; 4] = [78, 84, 96, 255];
/// The check mark's corners from the box's top left corner: down to the bottom of the tick, then up to the far end.
const TICK: [(i32, i32); 3] = [(4, 8), (7, 11), (12, 5)];

/// A line from `a` to `b` drawn with squares `width` pixels on a side.
fn stroke(d: &mut DrawCtx<'_, '_>, a: (i32, i32), b: (i32, i32), width: i32, color: [u8; 4]) {
    let steps = (b.0 - a.0).abs().max((b.1 - a.1).abs()).max(1);
    for i in 0..=steps {
        let x = a.0 + (b.0 - a.0) * i / steps;
        let y = a.1 + (b.1 - a.1) * i / steps;
        d.fill([x - width / 2, y - width / 2, width, width], color);
    }
}

/// A check box at `at`: a rounded square with a 1 pixel border, empty when `on` is false and filled with the accent and a thick
/// white check mark when it is true. `hot` (the pointer is over its row) lightens it. A locked box (a row a policy or an earlier
/// answer decided) is muted and takes no hover.
pub(super) fn check_box(
    d: &mut DrawCtx<'_, '_>,
    at: (i32, i32),
    on: bool,
    locked: bool,
    hot: bool,
) {
    let rect = [at.0, at.1, BOX, BOX];
    let hot = hot && !locked;
    let (fill, border) = match (on, locked) {
        (true, true) => (BOX_LOCKED, BOX_LOCKED),
        (true, false) => (if hot { BOX_ON_HOT } else { BOX_ON }, BOX_HOT),
        (false, true) => (BOX_BACK, BOX_LOCKED),
        (false, false) => (BOX_BACK, if hot { BOX_HOT } else { EDGE }),
    };
    d.rounded_box(rect, fill, border);
    if on {
        let mark = if locked { MUTED } else { [255, 255, 255, 255] };
        for pair in TICK.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            stroke(
                d,
                (at.0 + a.0, at.1 + a.1),
                (at.0 + b.0, at.1 + b.1),
                2,
                mark,
            );
        }
    }
}

/// Whether the pointer is inside `rect` (the GUI's cursor).
pub(super) fn pointer_in(d: &DrawCtx<'_, '_>, rect: [i32; 4]) -> bool {
    let p = d.gui().0.borrow().cursor_pos;
    (rect[0]..rect[0] + rect[2]).contains(&p.0) && (rect[1]..rect[1] + rect[3]).contains(&p.1)
}

/// Whether a row can be changed in the prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowLock {
    /// The player ticks or clears it.
    Free,
    /// A policy decided it: it is shown as it is and cannot be changed here.
    Locked,
}

/// One ability the prompt asks about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptRow {
    /// The ability's stable key (`client_script.ability.ui`): what an answer names it by.
    pub key: String,
    /// The line shown, already in the player's language.
    pub label: String,
    /// Whether it starts ticked.
    pub on: bool,
    /// Whether the player may change it.
    pub lock: RowLock,
}

/// What a prompt says. All text is plain: hidden and control characters are replaced and a line is cut at [`MAX_LINE`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptSpec {
    /// The first line: who is asking.
    pub heading: String,
    /// The second line: what they ask.
    pub detail: String,
    /// The abilities, at most [`MAX_ROWS`] (the rest are dropped).
    pub rows: Vec<PromptRow>,
    /// The label of the button that allows.
    pub allow: String,
    /// The label of the button that declines for now.
    pub not_now: String,
    /// The label of the button that declines for good.
    pub never: String,
    /// The label of the "remember" box.
    pub remember: String,
    /// Whether the remember box starts ticked.
    pub remember_on: bool,
}

/// What the player chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptAnswer {
    /// Allow, with each row's key and whether it is ticked, and whether to remember the choice.
    Allow {
        /// The rows as they were left.
        rows: Vec<(String, bool)>,
        /// Whether the remember box was ticked.
        remember: bool,
    },
    /// Not now: nothing is remembered, and the server asks again next time.
    NotNow,
    /// Never for this server, remembered.
    Never,
}

/// A place in the prompt's box that takes a press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptSpot {
    /// A row's box, by its place in the spec.
    Row(usize),
    /// The "remember" box.
    Remember,
    /// The button that allows.
    Allow,
    /// The button that declines for now.
    NotNow,
    /// The button that declines for good.
    Never,
}

pub(super) use PromptSpot as Spot;

/// Where each place of a prompt with `rows` rows is on a canvas of `size`: `(spot, [x, y, width, height])`. For a game that
/// tests its UI by clicking the prompt.
pub fn prompt_spots(size: (i32, i32), rows: usize) -> Vec<(PromptSpot, [i32; 4])> {
    layout(size, rows.min(MAX_ROWS)).spots
}

/// What the prompt keeps.
#[derive(Default)]
pub(crate) struct PromptState {
    spec: Option<PromptSpec>,
    on: Vec<bool>,
    remember: bool,
    /// The simulation time it was shown at.
    born: u32,
    down: Option<Spot>,
    answer: Option<PromptAnswer>,
}

/// The plain one-line form of `text`.
fn plain(text: &str) -> String {
    text.chars()
        .map(|c| {
            if is_hidden(c) || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .take(MAX_LINE)
        .collect()
}

/// Where everything is for a canvas of `size` and `rows` rows: the box, then each spot's rectangle.
struct Layout {
    shape: [i32; 4],
    spots: Vec<(Spot, [i32; 4])>,
}

fn layout(size: (i32, i32), rows: usize) -> Layout {
    let rows_i = i32::try_from(rows).unwrap_or(0);
    let width = (size.0 - 2 * GAP).clamp(1, 520);
    let height = PAD + 2 * LINE + GAP + rows_i * ROW + ROW + GAP + BUTTON_HEIGHT + PAD;
    let x = ((size.0 - width) / 2).max(0);
    let y = ((size.1 - height) / 2).max(0);
    let shape = [x, y, width, height];
    let mut spots = Vec::with_capacity(rows + 4);
    let mut at = y + PAD + 2 * LINE + GAP;
    for row in 0..rows {
        spots.push((Spot::Row(row), [x + PAD, at, width - 2 * PAD, ROW]));
        at += ROW;
    }
    spots.push((Spot::Remember, [x + PAD, at, width - 2 * PAD, ROW]));
    at += ROW + GAP;
    let button = (width - 2 * PAD - 2 * GAP) / 3;
    for (n, spot) in [Spot::Allow, Spot::NotNow, Spot::Never]
        .into_iter()
        .enumerate()
    {
        let n = i32::try_from(n).unwrap_or(0);
        spots.push((
            spot,
            [x + PAD + n * (button + GAP), at, button, BUTTON_HEIGHT],
        ));
    }
    Layout { shape, spots }
}

impl Layout {
    fn spot_at(&self, point: (i32, i32)) -> Option<Spot> {
        self.spots.iter().find_map(|(spot, r)| {
            ((r[0]..r[0] + r[2]).contains(&point.0) && (r[1]..r[1] + r[3]).contains(&point.1))
                .then_some(*spot)
        })
    }
}

/// The prompt's class: drawn and clicked by itself, over every layer.
#[derive(EngineClass)]
#[class(name = "ClientScriptPrompt", parent = "GuiControl")]
pub(crate) struct PromptCtrl {}

impl GuiControlClass for PromptCtrl {
    type State = PromptState;

    fn draw(&self, st: &mut PromptState, d: &mut DrawCtx<'_, '_>, at: &Place) {
        let Some(spec) = st.spec.as_ref() else {
            return;
        };
        let canvas = at.rect();
        let face = d.gui().0.borrow().client_script.faces[0].clone();
        d.clip_to(canvas);
        d.fill(canvas, DIM_BACK);
        let layout = layout((canvas[2], canvas[3]), spec.rows.len());
        let shape = layout.shape;
        let origin = (canvas[0], canvas[1]);
        let at_rect = |r: [i32; 4]| [origin.0 + r[0], origin.1 + r[1], r[2], r[3]];
        let frame = at_rect(shape);
        d.fill(frame, BACK);
        d.outline(
            (frame[0], frame[1]),
            (frame[0] + frame[2] - 1, frame[1] + frame[3] - 1),
            EDGE,
        );
        let room = frame[2] - 2 * PAD;
        fitted(
            d,
            &face,
            (frame[0] + PAD, frame[1] + PAD),
            room,
            &spec.heading,
            TEXT,
        );
        fitted(
            d,
            &face,
            (frame[0] + PAD, frame[1] + PAD + LINE),
            room,
            &spec.detail,
            MUTED,
        );
        for (spot, rect) in &layout.spots {
            let rect = at_rect(*rect);
            // A row and the remember box have a check box before their label: (checked, locked).
            let (label, boxed, color, fill, check) = match spot {
                Spot::Row(i) => {
                    let row = &spec.rows[*i];
                    let tick = st.on.get(*i).copied().unwrap_or(false);
                    let locked = row.lock == RowLock::Locked;
                    let color = if locked { MUTED } else { TEXT };
                    (row.label.clone(), false, color, None, Some((tick, locked)))
                }
                Spot::Remember => (
                    spec.remember.clone(),
                    false,
                    TEXT,
                    None,
                    Some((st.remember, false)),
                ),
                Spot::Allow => (spec.allow.clone(), true, TEXT, Some(ALLOW), None),
                Spot::NotNow => (spec.not_now.clone(), true, TEXT, Some(BUTTON), None),
                Spot::Never => (spec.never.clone(), true, TEXT, Some(NEVER), None),
            };
            if let Some(fill) = fill {
                d.fill(rect, fill);
            }
            let mut inset = if boxed { 8 } else { 0 };
            if let Some((on, locked)) = check {
                let hot = pointer_in(d, rect);
                check_box(d, (rect[0], rect[1] + (rect[3] - BOX) / 2), on, locked, hot);
                inset = BOX_ROOM;
            }
            fitted(
                d,
                &face,
                (rect[0] + inset, rect[1] + (rect[3] - FACE_SIZE) / 2 - 1),
                rect[2] - 2 * inset,
                &label,
                color,
            );
        }
    }

    fn mouse_down(
        &self,
        st: &mut PromptState,
        cx: &mut ControlCx<'_>,
        ev: &PointerEvent,
    ) -> Result<(), VmError> {
        st.down = None;
        let Some(spec) = st.spec.as_ref() else {
            return Ok(());
        };
        if cx.vm.sim_time().wrapping_sub(st.born) < IGNORE_MS {
            return Ok(());
        }
        let (_, extent) = cx.bounds();
        st.down = layout(extent, spec.rows.len()).spot_at(ev.local);
        Ok(())
    }

    fn mouse_up(
        &self,
        st: &mut PromptState,
        cx: &mut ControlCx<'_>,
        ev: &PointerEvent,
    ) -> Result<(), VmError> {
        let Some(pressed) = st.down.take() else {
            return Ok(());
        };
        let Some(spec) = st.spec.as_ref() else {
            return Ok(());
        };
        let (_, extent) = cx.bounds();
        if layout(extent, spec.rows.len()).spot_at(ev.local) != Some(pressed) {
            return Ok(());
        }
        match pressed {
            Spot::Row(i) => {
                if spec.rows.get(i).is_some_and(|r| r.lock == RowLock::Free)
                    && let Some(on) = st.on.get_mut(i)
                {
                    *on = !*on;
                }
            }
            Spot::Remember => st.remember = !st.remember,
            Spot::Allow => {
                st.answer = Some(PromptAnswer::Allow {
                    rows: spec
                        .rows
                        .iter()
                        .zip(&st.on)
                        .map(|(row, on)| (row.key.clone(), *on))
                        .collect(),
                    remember: st.remember,
                });
            }
            Spot::NotNow => st.answer = Some(PromptAnswer::NotNow),
            Spot::Never => st.answer = Some(PromptAnswer::Never),
        }
        Ok(())
    }
}

impl Gui {
    /// Shows the default consent prompt for `spec` in the engine layer: modal, over every dialog, taking the pointer after
    /// [`IGNORE_MS`]. The cursor is on while it is up, whatever the game had, and goes back as it was when the prompt is taken down
    /// (unless the game turned it on or off meanwhile). `None` when the layer is full or the control cannot be made. Take the answer with
    /// [`take_client_script_prompt`](Self::take_client_script_prompt).
    pub fn show_client_script_prompt(&self, vm: &mut Vm, spec: PromptSpec) -> Option<ObjId> {
        let id = make_realm_control(vm, self, CLASS, &[])?;
        let mut rows = spec.rows;
        rows.truncate(MAX_ROWS);
        let mut seen = HashSet::new();
        rows.retain(|row| seen.insert(row.key.clone()));
        let rows: Vec<PromptRow> = rows
            .into_iter()
            .map(|row| PromptRow {
                label: plain(&row.label),
                ..row
            })
            .collect();
        let spec = PromptSpec {
            heading: plain(&spec.heading),
            detail: plain(&spec.detail),
            allow: plain(&spec.allow),
            not_now: plain(&spec.not_now),
            never: plain(&spec.never),
            remember: plain(&spec.remember),
            rows,
            remember_on: spec.remember_on,
        };
        let born = vm.sim_time();
        let on = spec.rows.iter().map(|r| r.on).collect();
        let remember = spec.remember_on;
        self.with_custom_state::<PromptState, _>(id, |st| {
            *st = PromptState {
                spec: Some(spec),
                on,
                remember,
                born,
                down: None,
                answer: None,
            };
        });
        let size = self.0.borrow().canvas_size;
        control::set_bounds(vm, id, (0, 0), size);
        if !self.push_engine_layer(vm, id) {
            let _ = vm.delete_object_quiet(id);
            return None;
        }
        super::layers::take_prompt_cursor(self);
        Some(id)
    }

    /// The answer the prompt `id` was given, if it has one; the prompt is then taken down. `None` while it waits.
    pub fn take_client_script_prompt(&self, vm: &mut Vm, id: ObjId) -> Option<PromptAnswer> {
        let answer = self
            .with_custom_state::<PromptState, _>(id, |st| st.answer.take())
            .flatten()?;
        self.close_client_script_prompt(vm, id);
        Some(answer)
    }

    /// Takes the prompt `id` down without an answer.
    pub fn close_client_script_prompt(&self, vm: &mut Vm, id: ObjId) {
        self.pop_engine_layer(vm, id);
        super::layers::give_back_prompt_cursor(self);
        let _ = vm.delete_object_quiet(id);
    }

    /// Whether the prompt `id` is up.
    pub fn client_script_prompt_up(&self, vm: &Vm, id: ObjId) -> bool {
        vm.objects().contains(id) && self.0.borrow().client_script.engine.contains(&id)
    }
}

#[cfg(test)]
impl PromptState {
    pub(super) fn row_count(&self) -> usize {
        self.spec.as_ref().map_or(0, |s| s.rows.len())
    }

    pub(super) fn heading(&self) -> String {
        self.spec
            .as_ref()
            .map_or_else(String::new, |s| s.heading.clone())
    }
}
