//! The menus' control classes drawn as TGE 1.4 draws them (the controls' `onRender`). [`super`]'s walk dispatches here.
//!
//! - **`renderBorder`**: style 1 `dglDrawRect` in `borderColor`; styles 2 to 4 the bevels below (lines in white, black,
//!   `borderColor`, `borderColorNA`, `fillColor`); other styles nothing. **`renderRaisedBox`** and
//!   **`renderSlightlyRaisedBox`**: a fill and bevel lines. `dglDrawLine` of an axis-aligned line lights every
//!   pixel from one end to the other, both included.
//! - **GuiButtonCtrl**: the raised box unless depressed or on; the text (`renderJustifiedText`), one pixel down and right
//!   while depressed; the children. Unless the host keeps TGE's bevels for the profile
//!   ([`crate::ext::GuiExtension::classic_boxes`]), the box is instead the engine's rounded box of the button's state
//!   (`super::rounded`), and so are a bordered or open GuiPopUpMenuCtrl's box, its open list (the scroll view's fill and
//!   border, the row under the mouse a box inset in its cell) and GuiSliderCtrl's thumb.
//! - **GuiCheckBoxCtrl / GuiRadioCtrl**: with more than 3 rectangles in the profile's bitmap array, the state's (0 off, 1 on
//!   or depressed, 2 and 3 disabled) at the left, centred vertically; the text right of it over `extent.x - extent.y`.
//! - **GuiTextEditCtrl** (`onRender`, `drawText`): see [`Walk::text_edit`].
//! - **GuiScrollCtrl** (`onRender`, the bars, the corner): the fill, the border, the bars from the bitmap array,
//!   the children clipped to the content area.
//! - **GuiArrayCtrl** with the cells of GuiTextListCtrl, GuiConsole and GuiPopUpTextListCtrl.
//! - **GuiWindowCtrl**, **GuiPopUpMenuCtrl**, **GuiSliderCtrl**, **GuiProgressCtrl**,
//!   **GuiMLTextCtrl** (`onRender`, `drawAtomText`), **GuiConsoleTextCtrl**.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/core/guiDefaultControlRender.cpp`
//! (Copyright (c) 2012 GarageGames, LLC), `gui/buttons/guiButtonCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC),
//! `gui/buttons/guiCheckBoxCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC), `gui/controls/guiTextEditCtrl.cpp` (Copyright (c)
//! 2012 GarageGames, LLC), `gui/containers/guiScrollCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC),
//! `gui/core/guiArrayCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC), `gui/controls/guiTextListCtrl.cpp` (Copyright (c) 2012
//! GarageGames, LLC), `gui/controls/guiConsole.cpp` (Copyright (c) 2012 GarageGames, LLC), `gui/controls/guiPopUpCtrl.cpp`
//! (Copyright (c) 2012 GarageGames, LLC), `gui/containers/guiWindowCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC),
//! `gui/controls/guiSliderCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC), `gui/game/guiProgressCtrl.cpp` (Copyright (c) 2012
//! GarageGames, LLC), `gui/controls/guiMLTextCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC),
//! `gui/controls/guiConsoleTextCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use std::sync::Arc;

use torque_fonts::{FontUse, TextColors};
use torque_ts_vm::host::FileRead;
use torque_ts_vm::sim::ObjId;

use super::rounded::{BOX_RADIUS, BOX_TEXT_PAD, BoxState, box_colors, box_text_color};
use super::{Walk, color_field, int_field, intersect, profile_of, text_field};
use crate::control::{bounds, flag, point};

use crate::profile::{Rect, bitmap_array, font_color};
use crate::textures::GuiTexture;

const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
/// How far the row under the mouse in a rounded popup list keeps from its cell's sides and from the rows above and below
/// (pixels).
const ROW_INSET: (i32, i32) = (3, 1);

/// The profile's font colours (`fontColors[0..10]`): 0 normal, 1 highlighted, 2 not active, 3 selected, 4 and 5 links.
fn font_colors(vm: &torque_ts_vm::vm::Vm, p: ObjId) -> Vec<[u8; 4]> {
    (0..10).map(|i| font_color(vm, p, i)).collect()
}

/// How opaque an empty GuiTextEditCtrl's `placeholderText` is, out of 255, of the profile's `fontColor`.
const PLACEHOLDER_ALPHA: u16 = 110;

/// A font, its face and size.
type FontOf = (Arc<torque_fonts::Font>, String, i32);

impl Walk<'_> {
    /// The profile's font (`fontType`, `fontSize`).
    pub(super) fn profile_font(&mut self, p: ObjId) -> Option<FontOf> {
        let face = String::from_utf8_lossy(&text_field(self.vm, p, b"fontType")).into_owned();
        let size = int_field(self.vm, p, b"fontSize");
        self.font_named(&face, size, self.font_use)
    }

    fn font_named(&mut self, face: &str, size: i32, user: FontUse) -> Option<FontOf> {
        match self.font_for(face, size, user) {
            Some(f) => Some((f, face.to_string(), size)),
            None => {
                self.out.draws.report.no_font += 1;
                if self.out.draws.report.missing_fonts.len() < super::MAX_MISSING_FONTS {
                    self.out
                        .draws
                        .report
                        .missing_fonts
                        .insert(format!("{face}_{size}"));
                }
                None
            }
        }
    }

    /// `dglDrawTextN(font, at, text, colorTable)` with the modulation `color`.
    fn text(
        &mut self,
        f: &FontOf,
        at: (i32, i32),
        text: &[u8],
        color: [u8; 4],
        table: Option<&[[u8; 4]]>,
    ) {
        let mut colors = TextColors::new(color, self.cache.saved_color);
        self.glyphs(&f.0, &f.1, f.2, at, text, &mut colors, table);
    }

    /// `dglDrawTextN` from the modulation a previous draw left (`colors`: a colour code changes it for what follows).
    fn text_on(
        &mut self,
        f: &FontOf,
        at: (i32, i32),
        text: &[u8],
        colors: &mut TextColors,
        table: Option<&[[u8; 4]]>,
    ) {
        colors.saved = self.cache.saved_color;
        self.glyphs(&f.0, &f.1, f.2, at, text, colors, table);
    }

    /// The profile's bitmap as a texture (`mTextureHandle`): `GL_NEAREST` once a scroll control set it so.
    fn profile_texture(&mut self, p: ObjId) -> Option<Arc<GuiTexture>> {
        let name = text_field(self.vm, p, b"bitmap");
        if name.is_empty() {
            return None;
        }
        let key = String::from_utf8_lossy(&name).to_ascii_lowercase();
        let nearest = self.gui.0.borrow().nearest_textures.contains(&key);
        let vm = self.vm;
        let read = |path: &str| match vm.file_host().read(path.as_bytes()) {
            FileRead::Found(b) => Some(b),
            _ => None,
        };
        let t = self.cache.textures.get_filtered(
            &String::from_utf8_lossy(&name),
            false,
            nearest,
            &read,
        );
        if t.is_none() {
            self.out.draws.report.no_texture += 1;
        }
        t
    }

    /// `dglDrawBitmapSR`: the source rectangle at its own size.
    fn bitmap_sr(&mut self, t: &Arc<GuiTexture>, at: (i32, i32), src: Rect) {
        self.stretch(t, [at.0, at.1, src[2], src[3]], src, WHITE, false);
    }

    /// `dglDrawLine` of an axis-aligned line: every pixel from `a` to `b`, both included.
    pub(super) fn line(&mut self, a: (i32, i32), b: (i32, i32), color: [u8; 4]) {
        if a.1 == b.1 {
            let (l, r) = (a.0.min(b.0), a.0.max(b.0));
            self.fill([l, a.1, r.wrapping_sub(l).wrapping_add(1), 1], color);
        } else if a.0 == b.0 {
            let (t, bt) = (a.1.min(b.1), a.1.max(b.1));
            self.fill([a.0, t, 1, bt.wrapping_sub(t).wrapping_add(1)], color);
        }
    }

    /// A `GL_LINES` pair from `glVertex2i` (GuiSliderCtrl's rule and ticks): the pixels from `a` up to `b`, the last left out; a
    /// horizontal line lights row y, a vertical one column x - 1 (as OpenGL rasterizes the pair).
    fn gl_line(&mut self, a: (i32, i32), b: (i32, i32), color: [u8; 4]) {
        if a.1 == b.1 {
            let (l, r) = (a.0.min(b.0), a.0.max(b.0));
            self.fill([l, a.1, r.wrapping_sub(l), 1], color);
        } else if a.0 == b.0 {
            // A vertical line on the boundary x lights the column left of it (the slider's ticks at x - 1).
            let (t, bt) = (a.1.min(b.1), a.1.max(b.1));
            self.fill([a.0 - 1, t, 1, bt.wrapping_sub(t)], color);
        }
    }

    /// `renderBorder`.
    pub(super) fn render_border(&mut self, p: ObjId, r: Rect) {
        let vm = self.vm;
        let style = int_field(vm, p, b"border");
        let (l, t) = (r[0], r[1]);
        let (rt, b) = (r[0] + r[2] - 1, r[1] + r[3] - 1);
        let border = color_field(vm, p, b"borderColor");
        let na = color_field(vm, p, b"borderColorNA");
        let fill = color_field(vm, p, b"fillColor");
        match style {
            1 => self.border_corners(r, border, self.profile_radius(p)),
            2 => {
                self.line((l + 1, t + 1), (l + 1, b - 2), WHITE);
                self.line((l + 2, t + 1), (rt - 2, t + 1), WHITE);
                self.line((rt, t), (rt, b), WHITE);
                self.line((l, b), (rt - 1, b), WHITE);
                self.line((l, t), (rt - 1, t), na);
                self.line((l, t + 1), (l, b - 1), na);
                self.line((l + 1, b - 1), (rt - 1, b - 1), na);
                self.line((rt - 1, t + 1), (rt - 1, b - 2), na);
            }
            3 => {
                self.line((l, b), (rt, b), WHITE);
                self.line((rt, t), (rt, b - 1), WHITE);
                self.line((l + 1, b - 1), (rt - 1, b - 1), fill);
                self.line((rt - 1, t + 1), (rt - 1, b - 2), fill);
                self.line((l, t), (l, b - 1), na);
                self.line((l + 1, t), (rt - 1, t), na);
                self.line((l + 1, t + 1), (l + 1, b - 2), BLACK);
                self.line((l + 2, t + 1), (rt - 2, t + 1), BLACK);
            }
            4 => {
                self.line((l, t), (l, b - 1), WHITE);
                self.line((l + 1, t), (rt, t), WHITE);
                self.line((l, b), (rt, b), BLACK);
                self.line((rt, t + 1), (rt, b - 1), BLACK);
                self.line((l + 1, b - 1), (rt - 1, b - 1), border);
                self.line((rt - 1, t + 1), (rt - 1, b - 2), border);
            }
            0 => {}
            _ => self.out.draws.report.border_styles += 1,
        }
    }

    /// `renderRaisedBox`.
    fn raised_box(&mut self, r: Rect, p: ObjId) {
        let (l, t) = (r[0], r[1]);
        let (rt, b) = (r[0] + r[2] - 1, r[1] + r[3] - 1);
        let fill = color_field(self.vm, p, b"fillColor");
        let border = color_field(self.vm, p, b"borderColor");
        self.fill(r, fill);
        self.line((l, t), (l, b - 1), WHITE);
        self.line((l, t), (rt - 1, t), WHITE);
        self.line((l, b), (rt, b), BLACK);
        self.line((rt, b - 1), (rt, t), BLACK);
        self.line((l + 1, b - 1), (rt - 1, b - 1), border);
        self.line((rt - 1, b - 2), (rt - 1, t + 1), border);
    }

    /// `renderSlightlyRaisedBox`.
    fn slightly_raised_box(&mut self, r: Rect, p: ObjId) {
        let (l, t) = (r[0], r[1]);
        let (rt, b) = (r[0] + r[2] - 1, r[1] + r[3] - 1);
        let fill = color_field(self.vm, p, b"fillColor");
        let border = color_field(self.vm, p, b"borderColor");
        self.fill(r, fill);
        self.line((l, t), (l, b), WHITE);
        self.line((l, t), (rt, t), WHITE);
        self.line((l + 1, b), (rt, b), border);
        self.line((rt, t + 1), (rt, b - 1), border);
    }

    /// Whether the boxes of controls in `profile` are TGE's bevels ([`crate::ext::GuiExtension::classic_boxes`]) rather
    /// than the rounded ones.
    fn classic_boxes(&self, profile: ObjId) -> bool {
        self.ext.classic_boxes(self.vm, profile)
    }

    /// A rounded box's state for the control `id`: disabled when not active or not enabled, else pressed, else hover.
    fn box_state(&self, id: ObjId, pressed: bool, hover: bool) -> BoxState {
        if !(crate::input::active(self.gui, id) && flag(self.vm, id, b"enabled")) {
            BoxState::Disabled
        } else if pressed {
            BoxState::Pressed
        } else if hover {
            BoxState::Hover
        } else {
            BoxState::Normal
        }
    }

    /// Whether the mouse is over the control `id` (the canvas's mouse control).
    fn mouse_on(&self, id: ObjId) -> bool {
        self.gui.0.borrow().input.mouse_control == Some(id)
    }

    /// GuiButtonCtrl::onRender: classic, the raised box unless depressed or on; else the rounded box of the button's state.
    pub(super) fn button(&mut self, id: ObjId, offset: (i32, i32), update: Rect, depth: usize) {
        let (_, ext) = bounds(self.vm, id);
        let (over, depressed, on) = self
            .gui
            .with(id, |c| (c.mouse_over, c.depressed, c.state_on))
            .unwrap_or((false, false, false));
        let rect = [offset.0, offset.1, ext.0, ext.1];
        let mut text_color = None;
        if let Some(p) = profile_of(self.vm, id) {
            if self.classic_boxes(p) {
                if !depressed && !on {
                    self.raised_box(rect, p);
                }
            } else {
                let state = self.box_state(id, depressed || on, over);
                let (fill, border) = box_colors(self.vm, p, state);
                let radius = match self.profile_radius(p) {
                    0 => BOX_RADIUS,
                    set => set,
                };
                self.rounded_box_with(rect, Some(fill), border, radius);
                text_color = box_text_color(self.vm, p, state);
            }
        }
        let text = text_field(self.vm, id, b"text");
        let at = if depressed {
            (offset.0 + 1, offset.1 + 1)
        } else {
            offset
        };
        self.justified_text_in(id, &text, at, ext, text_color);
        self.children(id, update, offset, depth);
    }

    /// GuiCheckBoxCtrl::onRender (GuiRadioCtrl's too).
    pub(super) fn check_box(&mut self, id: ObjId, offset: (i32, i32), update: Rect, depth: usize) {
        let vm = self.vm;
        let (_, ext) = bounds(vm, id);
        let mut text_x = 0;
        if let Some(p) = profile_of(vm, id) {
            let rects = bitmap_array(vm, self.gui, p);
            if rects.len() > 3 {
                let (depressed, on) = self
                    .gui
                    .with(id, |c| (c.depressed, c.state_on))
                    .unwrap_or((false, false));
                let index = if flag(vm, id, b"enabled") {
                    usize::from(on || depressed)
                } else {
                    usize::from(on) + 2
                };
                let r0 = rects[0];
                text_x = r0[2] + 2;
                let src = rects[index];
                if let Some(t) = self.profile_texture(p) {
                    self.bitmap_sr(&t, (offset.0, offset.1 + (ext.1 - r0[3]) / 2), src);
                }
            }
        }
        let text = text_field(vm, id, b"text");
        if !text.is_empty() {
            self.justified_text(
                id,
                &text,
                (offset.0 + text_x, offset.1),
                (ext.0 - ext.1, ext.1),
            );
        }
        self.children(id, update, offset, depth);
    }

    /// GuiTextEditCtrl::onRender: the fill when `opaque`, `drawText`, the border. No children.
    pub(super) fn text_edit(&mut self, id: ObjId, offset: (i32, i32)) {
        let vm = self.vm;
        let Some(p) = profile_of(vm, id) else {
            return;
        };
        let (_, ext) = bounds(vm, id);
        let rect = [offset.0, offset.1, ext.0, ext.1];
        if flag(vm, p, b"opaque") {
            let fill = color_field(vm, p, b"fillColor");
            self.fill_profile(p, rect, fill);
        }
        let focused = crate::input::canvas_first_responder(self.gui) == Some(id);
        self.draw_edit_text(id, p, rect, focused);
        if int_field(vm, p, b"border") != 0 {
            self.render_border(p, rect);
        }
    }

    /// `GuiTextEditCtrl::drawText`: the text (stars for a `password`), scrolled so the cursor shows (`mTextOffset.x`,
    /// placed afresh at the left plus 3 when reset or when the text is narrower than the control), in three runs: before the
    /// selection (`fontColor`, `fontColorNA` when not active or not enabled), the selection over a `fillColorHL` box
    /// (`fontColorHL`), after it; with an outline colour each run first nine times in it (and the x moved one pixel
    /// inwards per run); the caret, a line in `cursorColor`, while focused, on and enabled. An empty control shows its
    /// `placeholderText` (Torque3D's field, a dynamic field here) in a faded `fontColor`, focused or not.
    fn draw_edit_text(&mut self, id: ObjId, p: ObjId, rect: Rect, focused: bool) {
        let vm = self.vm;
        let Some(f) = self.profile_font(p) else {
            return;
        };
        let mut text = self
            .gui
            .with(id, |c| c.text.clone().unwrap_or_default())
            .unwrap_or_default();
        text.truncate(crate::input::text_edit::MAX_EDIT_TEXT);
        if flag(vm, id, b"password") {
            text.iter_mut().for_each(|b| *b = b'*');
        }
        let colors = font_colors(vm, p);
        let active = crate::input::active(self.gui, id);
        let enabled = flag(vm, id, b"enabled");
        let font_h = f.0.height as i32;
        let text_w = f.0.str_width(&text);
        let mut color = if active { colors[0] } else { colors[2] };
        let mut st = self
            .gui
            .with(id, |c| c.edit.as_deref().cloned())
            .flatten()
            .unwrap_or_default();
        if !enabled {
            st.block_start = 0;
            st.block_end = 0;
            color = colors[2];
        }
        let text_y = ((rect[3].wrapping_sub(font_h)) as u32 >> 1) as i32
            + point(vm, p, b"textOffset").1
            + rect[1];
        let mut off_x = match st.text_offset {
            Some(x) if text_w >= rect[2] => x,
            _ => rect[0] + 3,
        };
        let mut caret = (0, 0, 0);
        if !focused {
            if off_x < rect[0] {
                off_x = rect[0] + 3;
            }
            st.block_start = 0;
            st.block_end = 0;
        } else {
            let pos = st.cursor.clamp(0, text.len() as i32) as usize;
            let cursor_w = f.0.str_width(&text[..pos]);
            let mut cx = if cursor_w < rect[2] {
                cursor_w + off_x
            } else {
                rect[0] + 4 + rect[2]
            };
            let (top, bottom) = if font_h < rect[3] {
                let top = rect[1] + (rect[3] - font_h) / 2 - 2;
                (top, top + font_h)
            } else {
                (rect[1], rect[1] - 1 + rect[3])
            };
            if cx < rect[0] {
                cx = rect[0] + 3;
                off_x = cx - cursor_w;
            } else if rect[0] + rect[2] <= cx {
                cx = point(vm, id, b"extent").0 - 4 + rect[0];
                off_x = rect[2] - 4 + (rect[0] - cursor_w);
            }
            caret = (cx, top, bottom);
        }
        st.text_offset = Some(off_x);
        let len = text.len() as i32;
        let (mut bs, mut be) = (st.block_start, st.block_end);
        if len < bs || len < be || be < bs {
            bs = 0;
            be = 0;
            st.block_start = 0;
            st.block_end = 0;
        }
        let on = st.caret_on;
        self.gui.with_mut(id, |c| {
            if let Some(e) = c.edit.as_mut() {
                e.text_offset = Some(off_x);
                e.block_start = st.block_start;
                e.block_end = st.block_end;
            }
        });
        let outline_color = self.ext.text_outline(vm, p);
        let outline = outline_color.is_some();
        let outline_color = outline_color.unwrap_or_default();
        let justify = crate::profile::justify(vm, Some(p));
        let shift = |x: i32| -> i32 {
            if !outline {
                x
            } else if justify == 0 {
                x + 1
            } else if justify == 1 {
                x - 1
            } else {
                x
            }
        };
        let mut x = off_x;
        let run = |w: &mut Self, x: &mut i32, part: &[u8], c: [u8; 4]| {
            *x = shift(*x);
            if outline {
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        w.text(&f, (*x + dx, text_y + dy), part, outline_color, None);
                    }
                }
            }
            w.text(&f, (*x, text_y), part, c, Some(&colors));
            *x += f.0.str_width(part);
        };
        if bs > 0 {
            run(self, &mut x, &text[..bs as usize], color);
        }
        if be > 0 {
            let sel = &text[bs as usize..be as usize];
            let sw = f.0.str_width(sel);
            let hl = color_field(vm, p, b"fillColorHL");
            // glRecti(x, top, x + width, top + height - 1).
            self.fill([x, rect[1], sw, rect[3] - 1], hl);
            run(self, &mut x, sel, colors[1]);
        }
        run(self, &mut x, &text[be.max(0) as usize..], color);
        if text.is_empty() {
            let hint = text_field(vm, id, b"placeholderText");
            if !hint.is_empty() {
                let mut dim = colors[0];
                dim[3] = (u16::from(dim[3]) * PLACEHOLDER_ALPHA / 255) as u8;
                self.text(&f, (rect[0] + 3, text_y), &hint, dim, Some(&colors));
            }
        }
        if focused && on && enabled {
            let cc = color_field(vm, p, b"cursorColor");
            self.line((caret.0, caret.1), (caret.0, caret.2), cc);
        }
    }

    /// Whether the scroll control `id` in `profile` holds an open popup's list drawn in a rounded box (the profile's boxes
    /// are not TGE's bevels, [`crate::ext::GuiExtension::classic_boxes`]).
    fn rounded_popup_list(&self, id: ObjId, profile: ObjId) -> bool {
        crate::control::children(self.vm, id).into_iter().any(|c| {
            matches!(
                crate::popup::part_of(self.gui, c),
                Some((_, crate::popup::Part::List))
            )
        }) && !self.classic_boxes(profile)
    }

    /// GuiScrollCtrl::onRender. An open popup's list in a rounded box ([`Walk::rounded_popup_list`]) draws its fill (when
    /// opaque) and border (the extension's list border, else `borderColor` when the profile has a border) as one instead of
    /// the square ones.
    pub(super) fn scroll(&mut self, id: ObjId, offset: (i32, i32), update: Rect, depth: usize) {
        let vm = self.vm;
        let (_, ext) = bounds(vm, id);
        let rect = [offset.0, offset.1, ext.0, ext.1];
        // The bars and the children where the extension's displacement draws them (their places without one).
        let (s, glide) = crate::scroll::shown(vm, self.gui, id, self.now);
        let rounded = profile_of(vm, id).is_some_and(|p| self.rounded_popup_list(id, p));
        if let Some(p) = profile_of(vm, id) {
            if rounded {
                let fill = if flag(vm, p, b"opaque") {
                    color_field(vm, p, b"fillColor")
                } else {
                    [0; 4]
                };
                let border = self
                    .ext
                    .list_color(vm, p, crate::ext::ListColor::Border)
                    .or_else(|| {
                        (int_field(vm, p, b"border") != 0)
                            .then(|| color_field(vm, p, b"borderColor"))
                    })
                    .unwrap_or([0; 4]);
                self.rounded_box(rect, fill, border);
            } else {
                if flag(vm, p, b"opaque") {
                    let fill = color_field(vm, p, b"fillColor");
                    self.fill_profile(p, rect, fill);
                }
                if int_field(vm, p, b"border") != 0 {
                    self.render_border(p, rect);
                }
            }
            let rects = bitmap_array(vm, self.gui, p);
            if rects.len() > 36
                && let Some(t) = self.profile_texture(p)
            {
                if s.has_v {
                    self.v_bar(&t, &rects, &s, offset);
                }
                if s.has_h {
                    self.h_bar(&t, &rects, &s, offset);
                }
                if s.has_v && s.has_h {
                    // The corner right of the right arrow.
                    self.bitmap_sr(
                        &t,
                        (offset.0 + s.right[0] + s.right[2], offset.1 + s.right[1]),
                        rects[36],
                    );
                }
            }
        }
        let content = [
            offset.0 + s.content_pos.0,
            offset.1 + s.content_pos.1,
            s.content_ext.0,
            s.content_ext.1,
        ];
        let clip = intersect(content, update);
        if clip[2] > 0 && clip[3] > 0 {
            let at = (
                offset.0.wrapping_add(glide.0),
                offset.1.wrapping_add(glide.1),
            );
            self.children(id, clip, at, depth);
        }
        // The extension's list border ([`crate::ext::ListColor::Border`]) outlines the view over its content (a popup's open
        // list takes the popup's profile).
        if let Some(p) = profile_of(vm, id)
            && !rounded
            && let Some(c) = self.ext.list_color(vm, p, crate::ext::ListColor::Border)
        {
            let saved_clip = self.clip;
            self.set_clip(update);
            self.outline(
                (rect[0], rect[1]),
                (rect[0] + rect[2] - 1, rect[1] + rect[3] - 1),
                c,
            );
            self.set_clip(saved_clip);
        }
    }

    /// `drawVScrollBar`.
    fn v_bar(
        &mut self,
        t: &Arc<GuiTexture>,
        r: &[Rect],
        s: &crate::scroll::ScrollState,
        offset: (i32, i32),
    ) {
        use crate::scroll::{DOWN_ARROW, DOWN_PAGE, UP_ARROW, VERT_THUMB};
        let lit = |region: i32| s.region == region && s.depressed;
        let x = offset.0 + s.up[0];
        let y = offset.1 + s.up[1];
        let up = if !s.v_enabled {
            2
        } else if lit(UP_ARROW) {
            1
        } else {
            0
        };
        self.stretch(t, [x, y, r[up][2], r[up][3]], r[up], WHITE, false);
        let page_top = y + s.arrow_len;
        let (end, page) = if !s.v_enabled {
            (s.down[1] + offset.1, 17)
        } else {
            (s.v_thumb.1 + offset.1, if lit(DOWN_PAGE) { 16 } else { 15 })
        };
        if page_top < end {
            self.stretch(
                t,
                [x, page_top, r[page][2], end - page_top],
                r[page],
                WHITE,
                false,
            );
        }
        let (at, down) = if s.v_enabled {
            let sel = usize::from(lit(VERT_THUMB));
            let (top, mid, bot) = (6 + sel, 9 + sel, 12 + sel);
            self.stretch(t, [x, end, r[top][2], r[top][3]], r[top], WHITE, false);
            let mid_top = r[top][3] + end;
            let mid_end = offset.1 + (s.v_thumb.1 - r[bot][3]) + s.v_thumb.0;
            if mid_top < mid_end {
                self.stretch(
                    t,
                    [x, mid_top, r[mid][2], mid_end - mid_top],
                    r[mid],
                    WHITE,
                    false,
                );
            }
            self.stretch(t, [x, mid_end, r[bot][2], r[bot][3]], r[bot], WHITE, false);
            let page2 = r[bot][3] + mid_end;
            let end2 = offset.1 - 1 + s.v_track[3] + s.v_track[1];
            let idx = if lit(DOWN_PAGE) { 16 } else { 15 };
            if page2 < end2 {
                self.stretch(t, [x, page2, r[idx][2], end2 - page2], r[idx], WHITE, false);
            }
            (end2, if lit(DOWN_ARROW) { 4 } else { 3 })
        } else {
            (end, 5)
        };
        self.stretch(t, [x, at, r[down][2], r[down][3]], r[down], WHITE, false);
    }

    /// `drawHScrollBar`.
    fn h_bar(
        &mut self,
        t: &Arc<GuiTexture>,
        r: &[Rect],
        s: &crate::scroll::ScrollState,
        offset: (i32, i32),
    ) {
        use crate::scroll::{HORIZ_THUMB, LEFT_ARROW, LEFT_PAGE, RIGHT_ARROW, RIGHT_PAGE};
        let lit = |region: i32| s.region == region && s.depressed;
        let y = offset.1 + s.left[1];
        let x = offset.0 + s.left[0];
        let left = if !s.h_enabled {
            20
        } else if lit(LEFT_ARROW) {
            19
        } else {
            18
        };
        self.stretch(t, [x, y, r[left][2], r[left][3]], r[left], WHITE, false);
        let page_left = s.left[2] + x;
        let (end, page) = if !s.h_enabled {
            (s.right[0] + offset.0, 35)
        } else {
            (s.h_thumb.1 + offset.0, if lit(LEFT_PAGE) { 34 } else { 33 })
        };
        if page_left < end {
            self.stretch(
                t,
                [page_left, y, end - page_left, r[page][3]],
                r[page],
                WHITE,
                false,
            );
        }
        let (at, right) = if s.h_enabled {
            let sel = usize::from(lit(HORIZ_THUMB));
            let (lc, mid, rc) = (24 + sel, 27 + sel, 30 + sel);
            self.stretch(t, [end, y, r[lc][2], r[lc][3]], r[lc], WHITE, false);
            let mid_left = r[lc][2] + end;
            let mid_end = offset.0 + (s.h_thumb.1 - r[rc][2]) + s.h_thumb.0;
            if mid_left < mid_end {
                self.stretch(
                    t,
                    [mid_left, y, mid_end - mid_left, r[mid][3]],
                    r[mid],
                    WHITE,
                    false,
                );
            }
            self.stretch(t, [mid_end, y, r[rc][2], r[rc][3]], r[rc], WHITE, false);
            let page2 = r[rc][2] + mid_end;
            let end2 = offset.0 - 1 + s.h_track[2] + s.h_track[0];
            let idx = if lit(RIGHT_PAGE) { 34 } else { 33 };
            if page2 < end2 {
                self.stretch(t, [page2, y, end2 - page2, r[idx][3]], r[idx], WHITE, false);
            }
            (end2, if lit(RIGHT_ARROW) { 22 } else { 21 })
        } else {
            (end, 23)
        };
        self.stretch(t, [at, y, r[right][2], r[right][3]], r[right], WHITE, false);
    }

    /// GuiArrayCtrl::onRender: every cell the update rectangle meets, clipped to the cell cut by it, by its class's
    /// `onRenderCell`. No fill, no border, no children.
    pub(super) fn array(&mut self, id: ObjId, offset: (i32, i32), update: Rect, kind: CellKind) {
        let vm = self.vm;
        let Some(p) = profile_of(vm, id) else {
            return;
        };
        let s = crate::textlist::state(self.gui, id);
        if s.cell.0 <= 0 || s.cell.1 <= 0 {
            return;
        }
        let Some(f) = self.profile_font(p) else {
            return;
        };
        let rows: Vec<(Vec<u8>, bool)> = match kind {
            CellKind::Console => vm
                .console_history()
                .map(|h| h.iter().map(|l| (l.text.clone(), true)).collect())
                .unwrap_or_default(),
            _ => self
                .gui
                .with(id, |c| c.rows.iter().map(|r| (r.1.clone(), r.2)).collect())
                .unwrap_or_default(),
        };
        let levels: Vec<i32> = match kind {
            CellKind::Console => vm
                .console_history()
                .map(|h| h.iter().map(|l| l.level as i32).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let selected_row = self.gui.with(id, |c| c.selected).unwrap_or(-1);
        let colors = font_colors(vm, p);
        let rounded = matches!(kind, CellKind::PopUp) && !self.classic_boxes(p);
        let cols = crate::textlist::columns(vm, id);
        let clip_columns = flag(vm, id, b"clipColumnText");
        let saved_clip = self.clip;
        for j in 0..s.size.1 {
            if self.out.draws.full() {
                break;
            }
            if (j + 1) * s.cell.1 + offset.1 < update[1] {
                continue;
            }
            if j * s.cell.1 + offset.1 >= update[1] + update[3] {
                break;
            }
            for i in 0..s.size.0 {
                if (i + 1) * s.cell.0 + offset.0 < update[0] {
                    continue;
                }
                if i * s.cell.0 + offset.0 >= update[0] + update[2] {
                    break;
                }
                let cx = offset.0 + s.header.0 + i * s.cell.0;
                let cy = offset.1 + s.header.1 + j * s.cell.1;
                let cell = intersect([cx, cy, s.cell.0, s.cell.1], update);
                if cell[2] <= 0 || cell[3] <= 0 {
                    continue;
                }
                self.set_clip(cell);
                let selected = i == s.selected_x && j == selected_row;
                let over = i == s.mouse_over.0 && j == s.mouse_over.1;
                let Some((text, active)) = rows.get(j as usize) else {
                    continue;
                };
                match kind {
                    CellKind::TextList => self.text_list_cell(
                        p,
                        &f,
                        &colors,
                        (cx, cy),
                        s.cell,
                        text,
                        *active,
                        selected,
                        over,
                        &cols,
                        clip_columns,
                    ),
                    CellKind::Console => {
                        // The level's colour (0 normal, 1 highlighted, 2 not active).
                        let c = match levels.get(j as usize).copied().unwrap_or(0) {
                            1 => colors[1],
                            2 => colors[2],
                            _ => colors[0],
                        };
                        self.text(&f, (cx + 3, cy), text, c, Some(&colors));
                    }
                    CellKind::PopUp => {
                        // Selected `fontColorSEL`, under the mouse `fontColorHL`, else `fontColor`; no fill, no table. The
                        // extension's hover fill ([`crate::ext::ListColor::HoverFill`]) fills the row under the mouse (its text then
                        // `fontColorHL`, selected or not); its list pad insets the text (x from the left, y from the top; the cell is
                        // that much taller).
                        let fill = if over {
                            self.ext.list_color(vm, p, crate::ext::ListColor::HoverFill)
                        } else {
                            None
                        };
                        // In a rounded list the row under the mouse is a rounded box inset in its cell (the extension's
                        // hover fill, else `fillColorHL`), so it stays inside the list's corners.
                        let inset = if rounded && over {
                            fill.or_else(|| {
                                let hl = color_field(vm, p, b"fillColorHL");
                                (hl[3] != 0).then_some(hl)
                            })
                        } else {
                            None
                        };
                        let c = if fill.is_some() {
                            colors[1]
                        } else if selected {
                            colors[3]
                        } else if over {
                            colors[1]
                        } else {
                            colors[0]
                        };
                        // A rounded list's highlighted row keeps the normal text color when the profile sets no
                        // highlight one (an unset color is fully transparent).
                        let c = if rounded && c[3] == 0 { colors[0] } else { c };
                        if let Some(fc) = inset {
                            let r = [
                                cx + ROW_INSET.0,
                                cy + ROW_INSET.1,
                                s.cell.0 - 2 * ROW_INSET.0,
                                s.cell.1 - 2 * ROW_INSET.1,
                            ];
                            self.rounded_box(r, fc, [0; 4]);
                        } else if let Some(fc) = fill {
                            self.fill([cx, cy, s.cell.0, s.cell.1], fc);
                        }
                        let pad = self.ext.list_pad(vm, &text_field(vm, id, b"profile"));
                        let (px, py) = pad.unwrap_or((4, 0));
                        self.text(&f, (cx + px, cy + py), text, c, None);
                    }
                }
            }
        }
        self.set_clip(saved_clip);
    }

    /// GuiTextListCtrl::onRenderCell.
    #[allow(clippy::too_many_arguments)]
    fn text_list_cell(
        &mut self,
        p: ObjId,
        f: &FontOf,
        colors: &[[u8; 4]],
        at: (i32, i32),
        cell: (i32, i32),
        text: &[u8],
        active: bool,
        selected: bool,
        over: bool,
        cols: &[i32],
        clip_columns: bool,
    ) {
        let color = if !active {
            colors[2]
        } else {
            let mos = flag(self.vm, p, b"mouseOverSelected");
            if selected || (mos && over) {
                let hl = color_field(self.vm, p, b"fillColorHL");
                self.fill([at.0, at.1, cell.0, cell.1], hl);
            }
            if over && (mos || !selected) {
                colors[1]
            } else {
                colors[0]
            }
        };
        // The modulation is set once; a colour code in a column carries into the next ones.
        let mut tc = TextColors::new(color, self.cache.saved_color);
        let mut rest = text;
        for (index, &col) in cols.iter().enumerate() {
            let (field, next) = match rest.iter().position(|b| *b == b'\t') {
                Some(t) => (&rest[..t], Some(&rest[t + 1..])),
                None => (rest, None),
            };
            if col >= 0 {
                let pos = (at.0 + 4 + col, at.1);
                let saved = self.clip;
                let mut clipped = false;
                if clip_columns && index != cols.len() - 1 {
                    let c = intersect([pos.0, pos.1, cols[index + 1] - col - 4, cell.1], saved);
                    if c[2] > 0 && c[3] > 0 {
                        clipped = true;
                        self.set_clip(c);
                    }
                }
                self.text_on(f, pos, field, &mut tc, Some(colors));
                if clipped {
                    self.set_clip(saved);
                }
            }
            match next {
                Some(n) => rest = n,
                None => break,
            }
        }
    }

    /// GuiWindowCtrl::onRender.
    pub(super) fn window(&mut self, id: ObjId, offset: (i32, i32), update: Rect, depth: usize) {
        let vm = self.vm;
        let Some(p) = profile_of(vm, id) else {
            return;
        };
        let rects = bitmap_array(vm, self.gui, p);
        let s = crate::window::state(self.gui, id);
        let (_, ext) = bounds(vm, id);
        if rects.len() > 22 {
            let r = |i: usize| rects[i];
            let fr = crate::input::canvas_first_responder(self.gui);
            let key = fr.is_none_or(|f| f == id || crate::control::is_ancestor(vm, id, f));
            let (tl, tr, top) = if key { (12, 13, 14) } else { (15, 16, 17) };
            let win = [
                offset.0 + r(18)[2],
                offset.1 + r(top)[3],
                ext.0 - r(19)[2] - r(18)[2],
                ext.1 - (r(21)[3] + r(top)[3]),
            ];
            if !self.blurred(p) {
                let fill = color_field(vm, p, b"fillColor");
                self.fill(win, fill);
            }
            if let Some(t) = self.profile_texture(p) {
                let inset_x = |q: Rect| [q[0] + 1, q[1], q[2] - 2, q[3]];
                let inset_y = |q: Rect| [q[0], q[1] + 1, q[2], q[3] - 2];
                self.bitmap_sr(&t, offset, r(tl));
                self.bitmap_sr(&t, (offset.0 + ext.0 - r(tr)[2], offset.1), r(tr));
                self.stretch(
                    &t,
                    [
                        offset.0 + r(tl)[2],
                        offset.1,
                        ext.0 - r(tr)[2] - r(tl)[2],
                        r(top)[3],
                    ],
                    inset_x(r(top)),
                    WHITE,
                    false,
                );
                self.stretch(
                    &t,
                    [
                        offset.0,
                        offset.1 + r(tl)[3],
                        r(18)[2],
                        ext.1 - r(20)[3] - r(tl)[3],
                    ],
                    inset_y(r(18)),
                    WHITE,
                    false,
                );
                self.stretch(
                    &t,
                    [
                        offset.0 + ext.0 - r(19)[2],
                        offset.1 + r(tr)[3],
                        r(19)[2],
                        ext.1 - r(22)[3] - r(tr)[3],
                    ],
                    inset_y(r(19)),
                    WHITE,
                    false,
                );
                self.bitmap_sr(&t, (offset.0, offset.1 + ext.1 - r(20)[3]), r(20));
                self.bitmap_sr(
                    &t,
                    (offset.0 + ext.0 - r(22)[2], offset.1 + ext.1 - r(22)[3]),
                    r(22),
                );
                self.stretch(
                    &t,
                    [
                        offset.0 + r(20)[2],
                        offset.1 + ext.1 - r(21)[3],
                        ext.0 - r(22)[2] - r(20)[2],
                        r(21)[3],
                    ],
                    inset_x(r(21)),
                    WHITE,
                    false,
                );
            }
            // The title: `fontColor`, justified within the window rectangle, at the profile's text offset; no colour table.
            let title = self
                .gui
                .with(id, |c| c.text.clone())
                .flatten()
                .unwrap_or_else(|| text_field(vm, id, b"text"));
            if let Some(f) = self.profile_font(p) {
                let w = f.0.str_width(&title);
                let justify = crate::profile::justify(vm, Some(p));
                let mut xo = match justify {
                    1 => win[2] - w,
                    2 => (win[2] - w) / 2,
                    _ => 0,
                };
                if win[2] < w {
                    xo = 0;
                }
                let to = point(vm, p, b"textOffset");
                let c = font_color(vm, p, 0);
                self.text(&f, (to.0 + xo + offset.0, to.1 + offset.1), &title, c, None);
            }
            if let Some(t) = self.profile_texture(p) {
                let cursor = self.gui.0.borrow().cursor_pos;
                let at = crate::control::screen_position(vm, id);
                let local = (cursor.0 - at.0, cursor.1 - at.1);
                let inside = |q: Rect| {
                    local.0 >= q[0]
                        && local.0 < q[0] + q[2]
                        && local.1 >= q[1]
                        && local.1 < q[1] + q[3]
                };
                if flag(vm, id, b"canClose") {
                    let i = usize::from(inside(s.close) && s.press_close);
                    self.bitmap_sr(&t, (s.close[0] + offset.0, s.close[1] + offset.1), r(i));
                }
                if flag(vm, id, b"canMaximize") {
                    let i = if s.maximized { 6 } else { 3 }
                        + usize::from(inside(s.maximize) && s.press_maximize);
                    self.bitmap_sr(
                        &t,
                        (s.maximize[0] + offset.0, s.maximize[1] + offset.1),
                        r(i),
                    );
                }
                if flag(vm, id, b"canMinimize") {
                    let i = if s.minimized { 6 } else { 9 }
                        + usize::from(inside(s.minimize) && s.press_minimize);
                    self.bitmap_sr(
                        &t,
                        (s.minimize[0] + offset.0, s.minimize[1] + offset.1),
                        r(i),
                    );
                }
            }
        }
        if !s.minimized {
            self.children(id, update, offset, depth);
        }
    }

    /// GuiPopUpMenuCtrl::onRender: closed, the slightly raised box when it has a border; open (`mInAction`), a
    /// `fillColorHL` fill outlined in `borderColor`; the text (`fontColor`, `fontColorNA` when not active or not enabled; no colour
    /// table) justified, centered vertically. No children. In a rounded box of a popup with a border, text justified left or right
    /// keeps [`BOX_TEXT_PAD`] from the side.
    pub(super) fn popup(&mut self, id: ObjId, offset: (i32, i32)) {
        let vm = self.vm;
        let Some(p) = profile_of(vm, id) else {
            return;
        };
        let (_, ext) = bounds(vm, id);
        let rect = [offset.0, offset.1, ext.0, ext.1];
        let open = self.gui.with(id, |c| c.in_action).unwrap_or(false);
        let mut text_color = None;
        let mut pad = 0;
        if !self.classic_boxes(p) {
            let bordered = int_field(vm, p, b"border") != 0;
            if open || bordered {
                // A popup with no border of its own keeps its text where it is closed when it opens.
                if bordered {
                    pad = BOX_TEXT_PAD;
                }
                let state = self.box_state(id, false, open || self.mouse_on(id));
                let (fill, border) = box_colors(vm, p, state);
                self.rounded_box(rect, fill, border);
                text_color = box_text_color(vm, p, state);
            }
        } else if open {
            let hl = color_field(vm, p, b"fillColorHL");
            self.fill(rect, hl);
            let border = color_field(vm, p, b"borderColor");
            self.outline(
                (rect[0], rect[1]),
                (rect[0] + rect[2] - 1, rect[1] + rect[3] - 1),
                border,
            );
        } else if int_field(vm, p, b"border") != 0 {
            self.slightly_raised_box(rect, p);
        }
        let Some(f) = self.profile_font(p) else {
            return;
        };
        let text = self
            .gui
            .with(id, |c| c.text.clone())
            .flatten()
            .unwrap_or_default();
        let w = f.0.str_width(&text);
        let y = (ext.1 - f.0.height as i32) >> 1;
        let x = match crate::profile::justify(vm, Some(p)) {
            1 => ext.0 - w - pad,
            2 => (ext.0 - w) / 2,
            _ => pad,
        };
        let colors = font_colors(vm, p);
        let c = if crate::input::active(self.gui, id) && flag(vm, id, b"enabled") {
            text_color.unwrap_or(colors[0])
        } else {
            colors[2]
        };
        self.text(&f, (offset.0 + x, offset.1 + y), &text, c, None);
    }

    /// GuiSliderCtrl::onRender: the rule and the ticks in black, the thumb a raised box, the value (`%0.3f`) under it
    /// when it fits (`mDisplayValue`), then the children.
    pub(super) fn slider(&mut self, id: ObjId, offset: (i32, i32), update: Rect, depth: usize) {
        let vm = self.vm;
        let Some(p) = profile_of(vm, id) else {
            self.children(id, update, offset, depth);
            return;
        };
        let st = crate::slider::state(vm, self.gui, id);
        let (_, ext) = bounds(vm, id);
        let pos_x = offset.0 + st.shift_point;
        let ext_x = ext.0 - st.shift_extent;
        let ticks = st.ticks.max(0) as u32;
        if ext.0 >= ext.1 {
            let mid = if st.display_value {
                st.thumb_size.1 / 2
            } else {
                ext.1 / 2
            };
            let y = offset.1 + mid;
            self.gl_line((pos_x, y), (pos_x + ext_x, y), BLACK);
            for t in 0..=ticks.saturating_add(1).min(1024) {
                let x = ((ext_x - 1) as f32 / (ticks + 1) as f32 * t as f32) as i32;
                self.gl_line(
                    (pos_x + x, y - st.shift_point),
                    (pos_x + x, y + st.shift_point),
                    BLACK,
                );
            }
        } else {
            let mid = ext_x / 2;
            let x = pos_x + mid;
            self.gl_line((x, offset.1), (x, offset.1 + ext.1), BLACK);
            for t in 0..=ticks.saturating_add(1).min(1024) {
                let y = ((ext.1 - 1) as f32 / (ticks + 1) as f32 * t as f32) as i32;
                self.gl_line(
                    (mid - st.shift_point + pos_x, offset.1 + y),
                    (pos_x + mid + st.shift_point, offset.1 + y),
                    BLACK,
                );
            }
        }
        let thumb = [
            st.thumb[0] + pos_x,
            st.thumb[1] + offset.1,
            st.thumb[2],
            st.thumb[3],
        ];
        if self.classic_boxes(p) {
            self.raised_box(thumb, p);
        } else {
            let state = self.box_state(id, false, self.mouse_on(id));
            let (fill, border) = box_colors(vm, p, state);
            self.rounded_box(thumb, fill, border);
        }
        if st.display_value && ext.0 >= ext.1 {
            let buf = torque_ts_vm::text::format_fixed(f64::from(st.value), 3).into_bytes();
            if let Some(f) = self.profile_font(p) {
                let w = f.0.str_width(&buf);
                let mut tx = thumb[0] + thumb[2] / 2 - w / 2;
                let ty = thumb[1] + thumb[3] - 2;
                if tx < offset.0 {
                    tx = offset.0;
                } else if tx + w > ext.0 + offset.0 {
                    tx = ext.0 - w + offset.0;
                }
                let colors = font_colors(vm, p);
                self.text(&f, (tx, ty), &buf, colors[0], Some(&colors));
            }
        }
        self.children(id, update, offset, depth);
    }

    /// GuiProgressCtrl::onRender: the done part in `fillColor`; with a border, `borderColor` round it and three quarters
    /// of it one pixel in; the children.
    pub(super) fn progress(&mut self, id: ObjId, offset: (i32, i32), update: Rect, depth: usize) {
        let vm = self.vm;
        let (_, ext) = bounds(vm, id);
        if let Some(p) = profile_of(vm, id) {
            let value = self.gui.with(id, |c| c.progress).unwrap_or(0.0);
            let w = (value * ext.0 as f32) as i32;
            if w > 0 {
                let fill = color_field(vm, p, b"fillColor");
                self.fill([offset.0, offset.1, w, ext.1], fill);
            }
            if int_field(vm, p, b"border") != 0 {
                let b = color_field(vm, p, b"borderColor");
                self.outline(offset, (offset.0 + ext.0 - 1, offset.1 + ext.1 - 1), b);
                let inner = [
                    (b[0] as f32 * 0.75) as u8,
                    (b[1] as f32 * 0.75) as u8,
                    (b[2] as f32 * 0.75) as u8,
                    255,
                ];
                self.outline(
                    (offset.0 + 1, offset.1 + 1),
                    (offset.0 + ext.0 - 2, offset.1 + ext.1 - 2),
                    inner,
                );
            }
        }
        self.children(id, update, offset, depth);
    }

    /// GuiMLTextCtrl::onRender: `GuiControl::onRender` (the children under the text), the bitmaps, then each line the
    /// update rectangle meets: a divider's `fillColorHL` band, the atoms (`drawAtomText`).
    pub(super) fn ml_text(&mut self, id: ObjId, offset: (i32, i32), update: Rect, depth: usize) {
        self.gui_control(id, offset, update, depth);
        let vm = self.vm;
        let Some(st) = crate::mltext::state(self.gui, id) else {
            return;
        };
        let Some(p) = profile_of(vm, id) else {
            return;
        };
        let layout = st.layout.clone();
        let buf = crate::mltext::buffer(self.gui, id);
        let max_h = self.ext.ml_max_bitmap_height(vm, id).unwrap_or(-1);
        for b in &layout.bitmaps {
            let r = [
                b.rect[0] as i32 + offset.0,
                b.rect[1] as i32 + offset.1,
                b.rect[2] as i32,
                b.rect[3] as i32,
            ];
            let hit = intersect(update, r);
            if hit[2] <= 0 || hit[3] <= 0 {
                continue;
            }
            let Some(t) = self.texture(b.name.as_bytes(), false) else {
                continue;
            };
            let (bw, bh) = (t.bitmap_size.0 as i32, t.bitmap_size.1 as i32);
            let (w, h) = if max_h == -1 {
                (bw, bh)
            } else {
                let h = (bh as u32).min(max_h as u32) as i32;
                let w = (h as f32 * (bw as f32 / bh as f32)) as i32;
                (w, h)
            };
            self.stretch(&t, [r[0], r[1], w, h], [0, 0, bw, bh], WHITE, false);
        }
        let ext_x = point(vm, id, b"extent").0;
        let hl = color_field(vm, p, b"fillColorHL");
        let allow = flag(vm, id, b"allowColorChars");
        let colors = font_colors(vm, p);
        for line in &layout.lines {
            if self.out.draws.full() {
                return;
            }
            let lr = [
                offset.0,
                offset.1 + line.y as i32,
                ext_x,
                line.height as i32,
            ];
            let hit = intersect(update, lr);
            if hit[2] <= 0 || hit[3] <= 0 {
                continue;
            }
            if line.div != 0 {
                self.fill(
                    [
                        offset.0,
                        offset.1 + line.y as i32,
                        ext_x,
                        line.height as i32,
                    ],
                    hl,
                );
            }
            for atom in &line.atoms {
                let (s0, e0) = (atom.text_start, atom.text_start + atom.len);
                if !st.sel_active || st.sel_end < s0 || e0 <= st.sel_start {
                    self.atom_text(
                        p, &buf, &layout, &st, atom, line, false, s0, e0, offset, allow, &colors,
                    );
                    continue;
                }
                let a = st.sel_start.max(s0);
                let b = (st.sel_end + 1).min(e0);
                if s0 < a {
                    self.atom_text(
                        p, &buf, &layout, &st, atom, line, false, s0, a, offset, allow, &colors,
                    );
                }
                self.atom_text(
                    p, &buf, &layout, &st, atom, line, true, a, b, offset, allow, &colors,
                );
                if b < e0 {
                    self.atom_text(
                        p, &buf, &layout, &st, atom, line, false, b, e0, offset, allow, &colors,
                    );
                }
            }
        }
    }

    /// `drawAtomText`.
    #[allow(clippy::too_many_arguments)]
    fn atom_text(
        &mut self,
        p: ObjId,
        buf: &[u8],
        layout: &crate::mltext::MlLayout,
        st: &crate::mltext::MlState,
        atom: &crate::mltext::Atom,
        line: &crate::mltext::Line,
        selected: bool,
        start: u32,
        end: u32,
        offset: (i32, i32),
        allow: bool,
        colors: &[[u8; 4]],
    ) {
        let vm = self.vm;
        let Some(style) = layout.styles.get(atom.style) else {
            return;
        };
        // An ML text's fonts are its markup's, whatever control shows it.
        let Some(f) = self.font_named(&style.font.0, style.font.1, FontUse::Markup) else {
            return;
        };
        let slice = |a: u32, b: u32| -> &[u8] {
            let a = (a as usize).min(buf.len());
            let b = (b as usize).clamp(a, buf.len());
            &buf[a..b]
        };
        let x_off = if start != atom.text_start {
            f.0.str_width(slice(atom.text_start, start))
        } else {
            0
        };
        let text = slice(start, end);
        let mut x = x_off + atom.x_start as i32 + offset.0;
        let y = atom.y_start as i32 + offset.1;
        let url = atom.url.and_then(|u| layout.urls.get(u));
        let mut color = match url {
            None => style.color,
            Some(u) if !u.mouse_down => style.link_color,
            Some(_) => style.link_color_hl,
        };
        let table = if allow { Some(colors) } else { None };
        let alpha = st.alpha;
        let outline_color = self.ext.text_outline(vm, p);
        let outline = outline_color.is_some();
        let outline_color = {
            let mut c = outline_color.unwrap_or_default();
            c[3] = (c[3] as f32 * alpha) as i32 as u8;
            c
        };
        let dots: &[u8] = b"...";
        if !selected {
            if style.shadow_offset != (0, 0) {
                let mut sc = style.shadow_color;
                sc[3] = (f64::from(sc[3]) * f64::from(alpha)) as i32 as u8;
                self.text(
                    &f,
                    (x + style.shadow_offset.0, y + style.shadow_offset.1),
                    text,
                    sc,
                    table,
                );
            }
            color[3] = (alpha * color[3] as f32) as i32 as u8;
            if outline {
                match crate::profile::justify(vm, Some(p)) {
                    0 => x += 1,
                    1 => x -= 1,
                    _ => {}
                }
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        self.text(&f, (x + dx, y + dy), text, outline_color, None);
                    }
                }
            }
            self.text(&f, (x, y), text, color, table);
            if atom.is_clipped {
                let dx = x + f.0.str_width_precise(text);
                if outline {
                    for ox in -1..=1 {
                        for oy in -1..=1 {
                            self.text(&f, (dx + ox, y + oy), dots, outline_color, None);
                        }
                    }
                }
                self.text(&f, (dx, y), dots, color, table);
            }
        } else {
            let w = f.0.str_width(text);
            let hl = color_field(vm, p, b"fillColorHL");
            self.fill(
                [x, line.y as i32 + offset.1, w + 1, line.height as i32 + 1],
                hl,
            );
            color = colors[1];
            self.text(&f, (x, y), text, color, table);
            if atom.is_clipped {
                let dx = x + f.0.str_width_precise(text);
                self.text(&f, (dx, y), dots, color, table);
            }
        }
        if let Some(u) = url
            && !u.no_underline
        {
            let uy = atom.baseline as i32 + 2 + y;
            let x2 = x + f.0.str_width_precise(text);
            if outline {
                self.line((x - 1, uy - 1), (x2 + 1, uy - 1), outline_color);
                self.line((x - 1, uy), (x2 + 1, uy), outline_color);
                self.line((x - 1, uy + 1), (x2 + 1, uy + 1), outline_color);
            }
            self.line((x, uy), (x2, uy), color);
        }
    }

    /// GuiConsoleTextCtrl::onRender: the evaluated text (`fontColor`, with the colour table) at 2 from the left (right:
    /// `extent.x - width - 2`; centre: half the room), then the children.
    pub(super) fn console_text(
        &mut self,
        id: ObjId,
        offset: (i32, i32),
        update: Rect,
        depth: usize,
    ) {
        let vm = self.vm;
        if let Some(p) = profile_of(vm, id)
            && let Some(f) = self.profile_font(p)
        {
            let text = self
                .gui
                .with(id, |c| c.text.clone())
                .flatten()
                .unwrap_or_default();
            let (_, ext) = bounds(vm, id);
            let w = f.0.str_width(&text);
            let x = match crate::profile::justify(vm, Some(p)) {
                1 => ext.0 - w - 2,
                2 => (ext.0 - w) / 2,
                _ => 2,
            };
            let colors = font_colors(vm, p);
            self.text(
                &f,
                (offset.0 + x, offset.1),
                &text,
                colors[0],
                Some(&colors),
            );
        }
        self.children(id, update, offset, depth);
    }
}

/// Which `onRenderCell` an array draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CellKind {
    TextList,
    Console,
    PopUp,
}
