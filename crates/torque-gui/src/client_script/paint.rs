//! How the realm's controls are drawn, by kind.

use torque_client_script_format::ui::{Kind, Prop};

use super::controls::NodeState;
use super::rich::{self, Align};
use crate::custom::Place;
use crate::draw::DrawCtx;

const ACCENT: [u8; 4] = [52, 120, 220, 255];
const DANGER: [u8; 4] = [200, 60, 60, 255];
const SURFACE: [u8; 4] = [60, 64, 72, 255];
const FIELD: [u8; 4] = [20, 22, 26, 255];
const EDGE: [u8; 4] = [120, 126, 138, 255];
const TEXT: [u8; 4] = [235, 238, 242, 255];
/// The caret's blink period, in milliseconds.
const CARET_MS: u32 = 500;
/// The font size of a tooltip.
const TOOLTIP_SIZE: i32 = 12;

/// A font role resolved to a face and a size.
struct Face {
    name: String,
    size: i32,
}

impl Face {
    fn of(st: &NodeState, d: &DrawCtx<'_, '_>) -> Face {
        let role = usize::from(st.props.index(Prop::Font)).min(2);
        Face {
            name: d.gui().0.borrow().client_script.faces[role].clone(),
            size: st.props.num(Prop::FontSize).round() as i32,
        }
    }

    fn width(&self, d: &mut DrawCtx<'_, '_>, text: &str) -> i32 {
        self.width_at(d, text, self.size)
    }

    fn width_at(&self, d: &mut DrawCtx<'_, '_>, text: &str, size: i32) -> i32 {
        d.text_size(&self.name, size, text.as_bytes())
            .map_or(0, |(w, _)| w)
    }

    fn height(&self, d: &mut DrawCtx<'_, '_>, size: i32) -> i32 {
        d.text_size(&self.name, size, b"A").map_or(size, |(_, h)| h)
    }
}

/// A text laid out in lines for one face, size and width, kept in the control's state until one of them changes or the realm
/// sets the control's props ([`NodeState::set_props`]).
#[derive(Debug)]
pub(super) struct Laid {
    face: String,
    size: i32,
    width: Option<i32>,
    lines: LaidLines,
}

#[derive(Debug)]
enum LaidLines {
    /// Each line, its width, and the height of every line.
    Plain(Vec<(String, i32)>, i32),
    /// Each line, its width and its height.
    Rich(Vec<(rich::Line, i32, i32)>),
}

/// The control's text laid out by `make` for `face` and `wrap`, from its state when nothing changed since.
fn laid<'s>(
    st: &'s mut NodeState,
    face: &Face,
    wrap: Option<i32>,
    d: &mut DrawCtx<'_, '_>,
    make: impl FnOnce(&NodeState, &mut DrawCtx<'_, '_>) -> LaidLines,
) -> Option<&'s LaidLines> {
    let fresh = st
        .laid
        .as_ref()
        .is_some_and(|l| l.face == face.name && l.size == face.size && l.width == wrap);
    if !fresh {
        let lines = make(st, d);
        st.laid = Some(Laid {
            face: face.name.clone(),
            size: face.size,
            width: wrap,
            lines,
        });
    }
    st.laid.as_ref().map(|l| &l.lines)
}

/// Where a line of width `w` starts in a box at `x` of width `box_w`.
fn aligned(x: i32, box_w: i32, w: i32, align: Align) -> i32 {
    match align {
        Align::Left => x,
        Align::Center => x + (box_w - w).max(0) / 2,
        Align::Right => x + (box_w - w).max(0),
    }
}

/// `text` cut into lines at newlines and, with a `limit`, between words, each with its width. `measure` gives a word's width and
/// is called once a word.
pub(super) fn wrap_plain(
    text: &str,
    limit: Option<i32>,
    mut measure: impl FnMut(&str) -> i32,
) -> Vec<(String, i32)> {
    let mut lines = Vec::new();
    for logical in text.split('\n') {
        let mut current = String::new();
        let mut width = 0i32;
        for word in logical.split_inclusive(' ') {
            let w = measure(word);
            if let Some(limit) = limit
                && !current.is_empty()
                && width.saturating_add(w) > limit
            {
                lines.push((std::mem::take(&mut current), width));
                width = 0;
            }
            current.push_str(word);
            width = width.saturating_add(w);
        }
        lines.push((current, width));
    }
    lines
}

/// Draws the control of `st` at `at`.
pub(super) fn draw_node(st: &mut NodeState, d: &mut DrawCtx<'_, '_>, at: &Place) {
    let Some(kind) = st.kind else {
        return;
    };
    d.scale_alpha(st.props.num(Prop::Opacity) as f32);
    if !st.props.flag(Prop::Enabled) {
        d.scale_alpha(0.5);
    }
    d.report().realm_controls += 1;
    let rect = at.rect();
    match kind {
        Kind::Panel => panel(st, d, rect),
        Kind::Text => text(st, d, rect),
        Kind::RichText => rich_text(st, d, rect),
        Kind::Image => {
            if let Some(image) = &st.image {
                d.realm_image(
                    image,
                    rect,
                    st.props.index(Prop::Fit),
                    st.props.color(Prop::Tint),
                );
            }
        }
        Kind::Button => button(st, d, rect),
        Kind::Toggle => toggle(st, d, rect),
        Kind::TextField => text_field(st, d, at, rect),
        Kind::List => list(st, d, rect),
        Kind::Scroll => scroll_bar(st, d, rect),
        Kind::Slider => slider(st, d, rect),
        Kind::Progress => progress(st, d, rect),
    }
}

fn outline(d: &mut DrawCtx<'_, '_>, rect: [i32; 4], color: [u8; 4]) {
    if color[3] > 0 {
        d.outline(
            (rect[0], rect[1]),
            (rect[0] + rect[2] - 1, rect[1] + rect[3] - 1),
            color,
        );
    }
}

fn panel(st: &NodeState, d: &mut DrawCtx<'_, '_>, rect: [i32; 4]) {
    let (fill, border) = (st.props.color(Prop::Fill), st.props.color(Prop::Border));
    if st.props.num(Prop::Radius) >= 1.0 {
        d.rounded_box(rect, fill, border);
    } else {
        if fill[3] > 0 {
            d.fill(rect, fill);
        }
        outline(d, rect, border);
    }
}

fn text(st: &mut NodeState, d: &mut DrawCtx<'_, '_>, rect: [i32; 4]) {
    let face = Face::of(st, d);
    let wrap = st.props.flag(Prop::Wrap).then_some(rect[2]);
    let align = match st.props.index(Prop::Align) {
        1 => Align::Center,
        2 => Align::Right,
        _ => Align::Left,
    };
    let color = st.props.color(Prop::Color);
    let Some(LaidLines::Plain(lines, lh)) = laid(st, &face, wrap, d, |st, d| {
        let lines = wrap_plain(st.props.text(Prop::Text), wrap, |s| face.width(d, s));
        LaidLines::Plain(lines, face.height(d, face.size))
    }) else {
        return;
    };
    for (i, (line, w)) in lines.iter().enumerate() {
        let at = (
            aligned(rect[0], rect[2], *w, align),
            rect[1] + i as i32 * lh,
        );
        d.text(&face.name, face.size, at, line.as_bytes(), color);
    }
}

fn rich_text(st: &mut NodeState, d: &mut DrawCtx<'_, '_>, rect: [i32; 4]) {
    let face = Face::of(st, d);
    let wrap = st.props.flag(Prop::Wrap).then_some(rect[2]);
    let color = st.props.color(Prop::Color);
    let Some(LaidLines::Rich(lines)) = laid(st, &face, wrap, d, |st, d| {
        let lines = rich::layout(st.props.text(Prop::RichText), wrap, face.size, |s, size| {
            face.width_at(d, s, size)
        });
        LaidLines::Rich(
            lines
                .into_iter()
                .map(|line| {
                    let (width, height) = (line.width(), face.height(d, line.size(face.size)));
                    (line, width, height)
                })
                .collect(),
        )
    }) else {
        return;
    };
    let mut y = rect[1];
    for (line, line_width, height) in lines {
        let mut x = aligned(rect[0], rect[2], *line_width, line.align);
        for (run, width) in &line.runs {
            let at = (x, y);
            let size = run.size.unwrap_or(face.size);
            let c = run.color.unwrap_or(color);
            d.text(&face.name, size, at, run.text.as_bytes(), c);
            if run.bold {
                d.text(&face.name, size, (x + 1, y), run.text.as_bytes(), c);
            }
            x += width;
        }
        y += height;
    }
}

fn shade(color: [u8; 4], by: i32) -> [u8; 4] {
    let s = |c: u8| (i32::from(c) + by).clamp(0, 255) as u8;
    [s(color[0]), s(color[1]), s(color[2]), color[3]]
}

fn button(st: &NodeState, d: &mut DrawCtx<'_, '_>, rect: [i32; 4]) {
    let base = match st.props.index(Prop::Style) {
        1 => ACCENT,
        2 => DANGER,
        3 => [0, 0, 0, 0],
        _ => SURFACE,
    };
    let fill = match (st.pressed, st.hover) {
        (true, _) => shade(base, -30),
        (false, true) => shade(base, 24),
        _ => base,
    };
    d.rounded_box(rect, fill, EDGE);
    let face = Face::of(st, d);
    let mut inner = rect;
    if let Some(image) = &st.image {
        let side = rect[3] - 8;
        d.realm_image(image, [rect[0] + 4, rect[1] + 4, side, side], 1, [255; 4]);
        inner[0] += side + 8;
        inner[2] -= side + 8;
    }
    let label = st.props.text(Prop::Text);
    let w = face.width(d, label);
    let lh = face.height(d, face.size);
    let at = (
        aligned(inner[0], inner[2], w, Align::Center),
        inner[1] + (inner[3] - lh) / 2,
    );
    d.text(&face.name, face.size, at, label.as_bytes(), TEXT);
}

fn toggle(st: &NodeState, d: &mut DrawCtx<'_, '_>, rect: [i32; 4]) {
    let side = 14.min(rect[3]);
    let boxed = [rect[0] + 2, rect[1] + (rect[3] - side) / 2, side, side];
    d.fill(boxed, FIELD);
    outline(d, boxed, EDGE);
    if st.props.flag(Prop::On) {
        d.fill([boxed[0] + 3, boxed[1] + 3, side - 6, side - 6], ACCENT);
    }
    let face = Face::of(st, d);
    let lh = face.height(d, face.size);
    let at = (boxed[0] + side + 6, rect[1] + (rect[3] - lh) / 2);
    d.text(
        &face.name,
        face.size,
        at,
        st.props.text(Prop::Text).as_bytes(),
        st.props.color(Prop::Color),
    );
}

fn text_field(st: &NodeState, d: &mut DrawCtx<'_, '_>, at: &Place, rect: [i32; 4]) {
    let focused = d.gui().0.borrow().client_script.focus == Some(at.id);
    // Drawn over the rest of the layer while focused: nothing of it spills past its own rectangle.
    d.clip_to(rect);
    d.fill(rect, FIELD);
    outline(d, rect, if focused { ACCENT } else { EDGE });
    let face = Face::of(st, d);
    let lh = face.height(d, face.size);
    let y = rect[1] + (rect[3] - lh) / 2;
    let value = st.props.text(Prop::Text);
    if value.is_empty() {
        let hint = st.props.text(Prop::Placeholder);
        d.text(
            &face.name,
            face.size,
            (rect[0] + 4, y),
            hint.as_bytes(),
            EDGE,
        );
    }
    let before: String = value.chars().take(st.caret).collect();
    let caret_x = face.width(d, &before);
    let slide = (caret_x - (rect[2] - 10)).max(0);
    let x = rect[0] + 4 - slide;
    // The value is drawn in the host's own color on the host's own fill, whatever the realm set: what the player types is
    // always seen.
    d.text(&face.name, face.size, (x, y), value.as_bytes(), TEXT);
    if focused && (d.now() / CARET_MS).is_multiple_of(2) {
        d.fill([x + caret_x, y, 1, lh], TEXT);
    }
}

/// A list's row height.
pub(super) fn row_height(size: i32) -> i32 {
    size + 4
}

fn list(st: &NodeState, d: &mut DrawCtx<'_, '_>, rect: [i32; 4]) {
    let face = Face::of(st, d);
    let rh = row_height(face.size);
    let selected = st.props.int(Prop::Selected) as usize;
    let first = (st.scroll.1 / rh).max(0) as usize;
    let color = st.props.color(Prop::Color);
    for (i, row) in st.props.rows().iter().enumerate().skip(first) {
        let y = rect[1] + (i as i32) * rh - st.scroll.1;
        if y >= rect[1] + rect[3] {
            break;
        }
        if i + 1 == selected {
            d.fill([rect[0], y, rect[2], rh], ACCENT);
        }
        d.text(
            &face.name,
            face.size,
            (rect[0] + 4, y + 2),
            row.as_bytes(),
            color,
        );
    }
}

fn scroll_bar(st: &NodeState, d: &mut DrawCtx<'_, '_>, rect: [i32; 4]) {
    let content = st.props.vec2(Prop::ContentSize).1 as i32;
    if content <= rect[3] || rect[3] <= 0 {
        return;
    }
    let thumb = (rect[3] * rect[3] / content).max(8);
    let travel = rect[3] - thumb;
    let y = rect[1] + travel * st.scroll.1 / (content - rect[3]);
    d.fill([rect[0] + rect[2] - 4, y, 3, thumb], EDGE);
}

/// A slider's position in its range, 0 to 1.
pub(super) fn slider_fraction(st: &NodeState) -> f64 {
    let (lo, hi) = st.props.vec2(Prop::Range);
    if hi <= lo {
        return 0.0;
    }
    ((st.props.num(Prop::SliderValue) - lo) / (hi - lo)).clamp(0.0, 1.0)
}

fn slider(st: &NodeState, d: &mut DrawCtx<'_, '_>, rect: [i32; 4]) {
    let mid = rect[1] + rect[3] / 2;
    d.fill([rect[0], mid - 2, rect[2], 4], SURFACE);
    let t = (slider_fraction(st) * f64::from(rect[2] - 10)) as i32;
    d.fill([rect[0] + t, rect[1] + 2, 10, (rect[3] - 4).max(1)], ACCENT);
}

fn progress(st: &NodeState, d: &mut DrawCtx<'_, '_>, rect: [i32; 4]) {
    let track = st.props.color(Prop::Fill);
    if track[3] > 0 {
        d.fill(rect, track);
    }
    let w = (st.props.num(Prop::Progress) * f64::from(rect[2])) as i32;
    d.fill([rect[0], rect[1], w, rect[3]], st.props.color(Prop::Color));
}

/// The tooltip of the control the pointer is over, if it has one and the layer is drawn.
pub(super) fn tooltip_text(gui: &crate::Gui) -> Option<String> {
    let hover = gui.0.borrow().client_script.hover?;
    gui.with_custom_state::<NodeState, _>(hover, |s| {
        let text = s.props.text(Prop::Tooltip);
        (!text.is_empty()).then(|| text.to_string())
    })?
}

/// Draws the tooltip of the control under the pointer, near the pointer and inside the canvas.
pub(crate) fn draw_tooltip(d: &mut DrawCtx<'_, '_>) {
    let gui = d.gui();
    let Some(text) = tooltip_text(gui) else {
        return;
    };
    let (face, pointer, canvas) = {
        let s = gui.0.borrow();
        (
            s.client_script.faces[0].clone(),
            s.cursor_pos,
            s.canvas_size,
        )
    };
    let Some((w, h)) = d.text_size(&face, TOOLTIP_SIZE, text.as_bytes()) else {
        return;
    };
    let (bw, bh) = (w + 8, h + 6);
    let x = (pointer.0 + 12).min(canvas.0 - bw).max(0);
    let y = (pointer.1 + 18).min(canvas.1 - bh).max(0);
    d.fill([x, y, bw, bh], [20, 22, 26, 235]);
    outline(d, [x, y, bw, bh], EDGE);
    d.text(&face, TOOLTIP_SIZE, (x + 4, y + 3), text.as_bytes(), TEXT);
}
