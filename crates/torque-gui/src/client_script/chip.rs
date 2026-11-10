//! The chip: what the realm holds, then "Server scripts on" and the server's address, in a fixed corner over every layer,
//! with a menu of per-tier toggles, "Hide for now" and "Turn off for this server". The server layer is neither drawn nor hit in
//! the chip's column ([`super::LayerState`]), which holds the chip at its widest and its menu open.

use torque_client_script_format::{Tier, is_hidden};
use torque_engine::EngineClass;
use torque_ts_vm::vm::{Vm, VmError};

use super::layers::{LayerAction, turn_off};
use crate::Gui;
use crate::control;
use crate::custom::{ControlCx, GuiControlClass, Place, PointerEvent};
use crate::draw::DrawCtx;

/// The chip's class name.
pub(crate) const CLASS: &str = "ClientScriptChip";

/// An ability in the words the chip's menu shows it by.
fn ability_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Ui => "Menus",
        Tier::Net => "Talking to the server",
        Tier::State => "Seeing what your game shows",
        Tier::Draw => "Markers and lines",
        Tier::Effects => "Sounds and effects",
        Tier::Input => "Extra keys",
        Tier::Camera => "Camera",
        Tier::Local => "Objects only you see",
        Tier::Predict => "Movement smoothing",
        Tier::Realm => "",
    }
}

const HEADER: i32 = 22;
const ROW: i32 = 20;
const MARGIN: i32 = 8;
const FACE_SIZE: i32 = 12;
const MAX_WIDTH: i32 = 480;
const MIN_WIDTH: i32 = 140;
const CHAR_WIDTH: i32 = 7;
/// The room between the holds, the address and the name.
const GAP: i32 = 8;
const MAX_ADDRESS: usize = 48;
const BACK: [u8; 4] = [20, 22, 26, 235];
const EDGE: [u8; 4] = [52, 120, 220, 255];
const TEXT: [u8; 4] = [235, 238, 242, 255];
const DIM: [u8; 4] = [140, 146, 158, 255];
/// The color of the holds.
pub(crate) const HOLD: [u8; 4] = [255, 196, 64, 255];
/// The side of the dot that says the realm holds something.
const DOT: i32 = 8;

/// A line of the open menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    /// A tier, and whether it is still granted.
    Tier(Tier, bool),
    /// Hide the realm's controls (`false`), or show them again (`true`).
    Hide(bool),
    /// Turn server scripts off for the connection.
    TurnOff,
}

/// What the chip keeps.
#[derive(Debug, Default)]
pub(crate) struct ChipState {
    /// What the realm holds, by name.
    pub(super) holds: Vec<&'static str>,
    /// "Server scripts on".
    pub(super) label: String,
    /// The server's name, plain text.
    pub(super) name: String,
    open: bool,
    rows: Vec<Row>,
}

/// Draws as much of `text` at `at` as fits in `room` pixels, cut with "..." when it does not all fit: the width drawn.
pub(super) fn fitted(
    d: &mut DrawCtx<'_, '_>,
    face: &str,
    at: (i32, i32),
    room: i32,
    text: &str,
    color: [u8; 4],
) -> i32 {
    if room <= 0 || text.is_empty() {
        return 0;
    }
    let mut width = |s: &str| {
        d.text_size(face, FACE_SIZE, s.as_bytes())
            .map_or(0, |(w, _)| w)
    };
    let shown = if width(text) <= room {
        text.to_string()
    } else {
        let byte = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
        // The longest start of the text that fits with the dots, found in a few measures.
        let (mut lo, mut hi) = (0, text.chars().count());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if width(&format!("{}...", &text[..byte(mid)])) <= room {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let cut = format!("{}...", &text[..byte(lo)]);
        if width(&cut) > room {
            return 0;
        }
        cut
    };
    d.text(face, FACE_SIZE, at, shown.as_bytes(), color)
        .unwrap_or(0)
}

/// The chip's class: drawn and clicked by itself, over every layer.
#[derive(EngineClass)]
#[class(name = "ClientScriptChip", parent = "GuiControl")]
pub(crate) struct ChipCtrl {}

impl GuiControlClass for ChipCtrl {
    type State = ChipState;

    fn draw(&self, st: &mut ChipState, d: &mut DrawCtx<'_, '_>, at: &Place) {
        let rect = at.rect();
        let face = d.gui().0.borrow().client_script.faces[0].clone();
        d.clip_to(rect);
        d.fill(rect, BACK);
        d.outline(
            (rect[0], rect[1]),
            (rect[0] + rect[2] - 1, rect[1] + rect[3] - 1),
            EDGE,
        );
        let pen = |d: &mut DrawCtx<'_, '_>, y: i32, text: &str, color: [u8; 4]| {
            d.text(
                &face,
                FACE_SIZE,
                (rect[0] + 6, rect[1] + y + 4),
                text.as_bytes(),
                color,
            );
        };
        // The holds first, at a fixed place: the server's text is cut to the room they leave, never the other way.
        let (y, right) = (rect[1] + 4, rect[0] + rect[2] - 6);
        let mut x = rect[0] + 6;
        // A dot while the realm holds the cursor, the camera, keys or typing: it says that something is held, and Escape lets go.
        if !st.holds.is_empty() {
            d.fill([x, y + 3, DOT, DOT], HOLD);
            x += DOT + GAP;
        }
        x += fitted(d, &face, (x, y), right - x, &st.label, TEXT) + GAP;
        fitted(d, &face, (x, y), right - x, &st.name, DIM);
        if !st.open {
            return;
        }
        for (i, row) in st.rows.iter().enumerate() {
            let (label, color) = match row {
                Row::Tier(tier, on) => {
                    // A check box and the ability in player words.
                    let top = rect[1] + HEADER + i as i32 * ROW;
                    let hot = super::prompt::pointer_in(d, [rect[0], top, rect[2], ROW]);
                    super::prompt::check_box(d, (rect[0] + 6, top + 2), *on, false, hot);
                    d.text(
                        &face,
                        FACE_SIZE,
                        (rect[0] + 6 + super::prompt::BOX_ROOM, top + 4),
                        ability_name(*tier).as_bytes(),
                        if *on { TEXT } else { DIM },
                    );
                    continue;
                }
                Row::Hide(hidden) => (
                    if *hidden {
                        "Show again"
                    } else {
                        "Hide for now"
                    }
                    .to_string(),
                    TEXT,
                ),
                Row::TurnOff => ("Turn off for this server".to_string(), TEXT),
            };
            pen(d, HEADER + i as i32 * ROW, &label, color);
        }
    }

    fn mouse_down(
        &self,
        st: &mut ChipState,
        cx: &mut ControlCx<'_>,
        ev: &PointerEvent,
    ) -> Result<(), VmError> {
        if ev.local.1 < HEADER {
            st.open = !st.open;
            return Ok(());
        }
        if !st.open {
            return Ok(());
        }
        let Some(row) = usize::try_from((ev.local.1 - HEADER) / ROW)
            .ok()
            .and_then(|i| st.rows.get(i).copied())
        else {
            return Ok(());
        };
        press(cx.gui, row);
        Ok(())
    }
}

/// The player chose `row`.
fn press(gui: &Gui, row: Row) {
    match row {
        Row::Tier(tier, true) => {
            let mut s = gui.0.borrow_mut();
            let l = &mut s.client_script;
            if let Some(view) = l.view.as_mut()
                && let Some(t) = view.tiers.iter_mut().find(|t| t.tier == tier)
            {
                t.on = false;
            }
            l.actions.push(LayerAction::Revoke(tier));
        }
        Row::Tier(_, false) => {}
        Row::Hide(hidden) => {
            let mut s = gui.0.borrow_mut();
            let l = &mut s.client_script;
            l.hidden = !hidden;
            l.actions.push(LayerAction::Hide(!hidden));
        }
        Row::TurnOff => turn_off(gui, LayerAction::TurnOff),
    }
}

/// `text` as plain one-line text of at most `max` characters.
fn plain(text: &str, max: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if is_hidden(c) { ' ' } else { c })
        .collect();
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max.saturating_sub(3)).collect();
    format!("{cut}...")
}

/// Sets the chip's text, menu rows, size and place for this frame, and the column kept clear for it.
pub(super) fn layout(vm: &mut Vm, gui: &Gui, size: (i32, i32)) {
    let (chip, label, name, holds, rows) = {
        let s = gui.0.borrow();
        let l = &s.client_script;
        let (Some(chip), Some(view)) = (l.chip, l.view.as_ref()) else {
            return;
        };
        let label = "Server scripts on".to_string();
        // The address is what the engine knows of the server; the name a server calls itself is left to the prompt.
        let name = plain(&view.address, MAX_ADDRESS);
        let holds: Vec<&'static str> = [
            (l.holds.cursor, "cursor"),
            (l.holds.camera, "camera"),
            (l.holds.key, "keys"),
            (l.focus.is_some(), "typing"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        let mut rows: Vec<Row> = view.tiers.iter().map(|t| Row::Tier(t.tier, t.on)).collect();
        rows.push(Row::Hide(l.hidden));
        rows.push(Row::TurnOff);
        (chip, label, name, holds, rows)
    };
    let chars = |s: &str| s.chars().count() as i32;
    let text = chars(&label) + if name.is_empty() { 0 } else { 2 + chars(&name) };
    let width_with = |dot: bool| {
        let extra = if dot { DOT + GAP } else { 0 };
        (text * CHAR_WIDTH + 12 + extra)
            .clamp(MIN_WIDTH, MAX_WIDTH)
            .min((size.0 - 2 * MARGIN).max(1))
    };
    let width = width_with(!holds.is_empty());
    let widest = width_with(true);
    let menu = HEADER + rows.len() as i32 * ROW;
    let open = gui
        .with_custom_state::<ChipState, _>(chip, |st| {
            st.label = label;
            st.name = name;
            st.holds = holds;
            st.rows = rows;
            st.open
        })
        .unwrap_or(false);
    let height = if open { menu } else { HEADER };
    control::set_bounds(vm, chip, (size.0 - width - MARGIN, MARGIN), (width, height));
    let left = (size.0 - widest - 2 * MARGIN).max(0);
    gui.0.borrow_mut().client_script.column = Some([left, 0, size.0 - left, menu + 2 * MARGIN]);
}
