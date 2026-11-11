//! The engine's rounded boxes: what GuiButtonCtrl, a GuiPopUpMenuCtrl with a border and its open list, and GuiSliderCtrl's
//! thumb draw unless the host keeps TGE's bevels for their profile ([`crate::ext::GuiExtension::classic_boxes`]).
//!
//! - **The shape**: a rectangle with corners of [`BOX_RADIUS`] pixels (less on a box under twice that), a solid fill and a
//!   one-pixel border, both with anti-aliased corners. A profile's `cornerRadius` (at most [`MAX_PROFILE_RADIUS`]) gives a
//!   control's fill and style 1 border the same shape with that radius instead of the square ones, and the boxes above that
//!   radius.
//! - **The colors**, by state ([`BoxState`]), from the profile: normal `fillColor`; hover `fillColorHL`; pressed (a depressed
//!   or toggled-on button, an open popup's list) `fillColorHL` darkened to [`PRESSED_SHADE`]; disabled (not active or not
//!   enabled) `fillColorNA`. The border is `borderColor` (`borderColorHL` over and pressed, `borderColorNA` disabled, when the
//!   profile sets them). A color the profile leaves unset (or fully transparent) is made from the others (no fill:
//!   [`DEFAULT_FILL`]).
//! - **The text** over a hover or pressed box takes the profile's highlight font color (`fontColors[1]`, else `fontColorHL`)
//!   when it sets one.
//! - **The draw**: nine quads of one small nearest-filtered texture (corner sprites drawn one texel a pixel, their middle
//!   row and column stretched), the fill then the border, so a box is a single batch.

use std::sync::{Arc, LazyLock};

use torque_ts_vm::fields::Value;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::{Walk, color_i, int_field};
use crate::profile::Rect;
use crate::textures::GuiTexture;

/// The corner radius of a rounded box (pixels).
pub const BOX_RADIUS: i32 = 5;
/// The largest corner radius a profile's `cornerRadius` gives (pixels; a larger value is cut to it).
pub const MAX_PROFILE_RADIUS: i32 = 24;
/// How far a popup's left or right justified text keeps from the side of its rounded box (pixels).
pub const BOX_TEXT_PAD: i32 = BOX_RADIUS + 3;
/// A box's fill when its profile sets no `fillColor`.
pub const DEFAULT_FILL: [u8; 4] = [224, 227, 232, 255];
/// How much of the highlight fill's color a pressed box keeps.
pub const PRESSED_SHADE: f32 = 0.8;
/// How far an unset highlight fill goes from the fill toward white.
const HOVER_LIFT: f32 = 0.18;
/// Subsamples a side of a sprite texel's coverage.
const SUBSAMPLES: u32 = 8;

/// A rounded box's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxState {
    Normal,
    Hover,
    Pressed,
    Disabled,
}

/// The corner sprites of every radius from 1 to the set's largest: row `r` (at [`Sprites::rows`]) holds three squares of side
/// `2r + 1`, white with the coverage as alpha: the whole shape, the shape half a pixel in (under a border) and the border.
struct Sprites {
    texture: Arc<GuiTexture>,
    /// The first texel row of each radius's squares, by radius.
    rows: Vec<i32>,
}

/// Which sprite of a row.
#[derive(Clone, Copy)]
enum Sprite {
    Fill = 0,
    Inner = 1,
    Ring = 2,
}

/// Whether `(x, y)` is in the square `[inset, side - inset]` with corners of radius `radius`.
fn inside(x: f32, y: f32, side: f32, inset: f32, radius: f32) -> bool {
    let (lo, hi) = (inset, side - inset);
    if x < lo || x > hi || y < lo || y > hi {
        return false;
    }
    let radius = radius.max(0.0);
    let cx = x.clamp(lo + radius, hi - radius);
    let cy = y.clamp(lo + radius, hi - radius);
    (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
}

/// The share of texel `(tx, ty)` that `covered` holds (as an alpha).
fn coverage(tx: u32, ty: u32, covered: impl Fn(f32, f32) -> bool) -> u8 {
    let n = SUBSAMPLES;
    let mut hits = 0u32;
    for sy in 0..n {
        for sx in 0..n {
            let x = tx as f32 + (sx as f32 + 0.5) / n as f32;
            let y = ty as f32 + (sy as f32 + 0.5) / n as f32;
            hits += u32::from(covered(x, y));
        }
    }
    ((hits * 255 + n * n / 2) / (n * n)) as u8
}

/// The sprites of the radii 1 to `max`, in a texture named `name`.
fn build_sprites(max: i32, name: &str) -> Sprites {
    let max = max as u32;
    let width = 3 * (2 * max + 1);
    let height: u32 = (1..=max).map(|r| 2 * r + 1).sum();
    let size = (crate::textures::pow2(width), crate::textures::pow2(height));
    let mut rgba = vec![0u8; (size.0 * size.1 * 4) as usize];
    let mut rows = vec![0i32; max as usize + 1];
    let mut y0 = 0u32;
    for r in 1..=max {
        rows[r as usize] = y0 as i32;
        let side = 2 * r + 1;
        let (s, rf) = (side as f32, r as f32);
        for ty in 0..side {
            for tx in 0..side {
                let fill = coverage(tx, ty, |x, y| inside(x, y, s, 0.0, rf));
                let inner = coverage(tx, ty, |x, y| inside(x, y, s, 0.5, rf - 0.5));
                let ring = coverage(tx, ty, |x, y| {
                    inside(x, y, s, 0.0, rf) && !inside(x, y, s, 1.0, rf - 1.0)
                });
                for (k, a) in [fill, inner, ring].into_iter().enumerate() {
                    let i = (((y0 + ty) * size.0 + k as u32 * side + tx) * 4) as usize;
                    rgba[i..i + 4].copy_from_slice(&[255, 255, 255, a]);
                }
            }
        }
        y0 += side;
    }
    Sprites {
        texture: Arc::new(GuiTexture {
            id: crate::textures::next_id(),
            name: name.to_string(),
            bitmap_size: (width, height),
            size,
            rgba,
            nearest: true,
            clamp: true,
        }),
        rows,
    }
}

/// The sprites of the boxes' own radii; the wider ones of a profile's `cornerRadius` are made when a control first asks.
static SPRITES: LazyLock<Sprites> =
    LazyLock::new(|| build_sprites(BOX_RADIUS, "engine:rounded-box"));
static WIDE_SPRITES: LazyLock<Sprites> =
    LazyLock::new(|| build_sprites(MAX_PROFILE_RADIUS, "engine:rounded-corners"));

/// The sprites that hold radius `radius` (1 or more).
fn sprites_for(radius: i32) -> &'static Sprites {
    if radius <= BOX_RADIUS {
        &SPRITES
    } else {
        &WIDE_SPRITES
    }
}

/// The rounded boxes' texture (one for the process, made on first use).
pub fn box_texture() -> Arc<GuiTexture> {
    Arc::clone(&SPRITES.texture)
}

/// A profile color, None when the profile leaves it unset (an unset color reads fully transparent, and a fully
/// transparent one counts as unset).
fn set_color(vm: &Vm, p: ObjId, name: &[u8], index: &[u8]) -> Option<[u8; 4]> {
    let c = color_i(&vm.objects().field(p, name, index).unwrap_or_default());
    (c[3] != 0).then_some(c)
}

/// `a` moved toward `b` by `t` (0 to 1), the alpha `a`'s.
fn mix(a: [u8; 4], b: [u8; 4], t: f32) -> [u8; 4] {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2]), a[3]]
}

/// `c` with its color times `f`.
fn shade(c: [u8; 4], f: f32) -> [u8; 4] {
    let s = |x: u8| (f32::from(x) * f).round() as u8;
    [s(c[0]), s(c[1]), s(c[2]), c[3]]
}

/// The fill and the border of a box of profile `p` in `state`.
pub fn box_colors(vm: &Vm, p: ObjId, state: BoxState) -> ([u8; 4], [u8; 4]) {
    let get = |name: &[u8]| set_color(vm, p, name, b"");
    let fill = get(b"fillColor").unwrap_or(DEFAULT_FILL);
    let hover = get(b"fillColorHL").unwrap_or_else(|| mix(fill, [255; 4], HOVER_LIFT));
    let border = get(b"borderColor").unwrap_or_else(|| shade(fill, 0.6));
    match state {
        BoxState::Normal => (fill, border),
        BoxState::Hover => (hover, get(b"borderColorHL").unwrap_or(border)),
        BoxState::Pressed => (
            shade(hover, PRESSED_SHADE),
            get(b"borderColorHL").unwrap_or(border),
        ),
        BoxState::Disabled => (
            get(b"fillColorNA").unwrap_or_else(|| mix(fill, [128, 128, 128, 255], 0.5)),
            get(b"borderColorNA").unwrap_or(border),
        ),
    }
}

/// The font color of text over a box in `state` when it is not the profile's usual one: the highlight color over a hover
/// or pressed box, when the profile sets one.
pub fn box_text_color(vm: &Vm, p: ObjId, state: BoxState) -> Option<[u8; 4]> {
    match state {
        BoxState::Hover | BoxState::Pressed => {
            set_color(vm, p, b"fontColors", b"1").or_else(|| set_color(vm, p, b"fontColorHL", b""))
        }
        BoxState::Normal | BoxState::Disabled => None,
    }
}

impl Walk<'_> {
    /// A rounded box over `r` (x, y, width, height): `fill`, then a one-pixel `border` around it (none when its alpha is 0).
    pub(super) fn rounded_box(&mut self, r: Rect, fill: [u8; 4], border: [u8; 4]) {
        self.rounded_box_with(r, Some(fill), border, BOX_RADIUS);
    }

    /// [`rounded_box`](Self::rounded_box) with corners of `radius` pixels (cut to half the box's shorter side), the fill left out
    /// when it is `None`; square corners at a radius of 0.
    pub(super) fn rounded_box_with(
        &mut self,
        r: Rect,
        fill: Option<[u8; 4]>,
        border: [u8; 4],
        radius: i32,
    ) {
        if r[2] <= 0 || r[3] <= 0 {
            return;
        }
        let bordered = border[3] != 0;
        if let Some(fill) = fill {
            self.fill_corners(r, fill, radius, bordered);
        }
        if bordered {
            self.border_corners(r, border, radius);
        }
    }

    /// The fill color `fill` over `r` with corners of `radius` pixels (square at 0); `bordered`: a one-pixel border is drawn over its
    /// edge, so it is the shape half a pixel in.
    pub(super) fn fill_corners(&mut self, r: Rect, fill: [u8; 4], radius: i32, bordered: bool) {
        let radius = radius.min(r[2] / 2).min(r[3] / 2);
        if radius <= 0 {
            self.fill(r, fill);
            return;
        }
        let sprite = if bordered {
            Sprite::Inner
        } else {
            Sprite::Fill
        };
        self.nine_slice(r, radius, sprite, fill);
    }

    /// A one-pixel border of color `border` around `r` with corners of `radius` pixels (square at 0).
    pub(super) fn border_corners(&mut self, r: Rect, border: [u8; 4], radius: i32) {
        let radius = radius.min(r[2] / 2).min(r[3] / 2);
        if radius <= 0 {
            let (rt, b) = (r[0].wrapping_add(r[2]) - 1, r[1].wrapping_add(r[3]) - 1);
            self.outline((r[0], r[1]), (rt, b), border);
            return;
        }
        self.nine_slice(r, radius, Sprite::Ring, border);
    }

    /// A profile's fill `color` over `r`, with the corners of its `cornerRadius` (square when it has none).
    pub(super) fn fill_profile(&mut self, profile: ObjId, r: Rect, color: [u8; 4]) {
        let bordered = int_field(self.vm, profile, b"border") == 1;
        self.fill_corners(r, color, self.profile_radius(profile), bordered);
    }

    /// The profile's `cornerRadius` in pixels, at most [`MAX_PROFILE_RADIUS`] (0: none).
    pub(super) fn profile_radius(&self, profile: ObjId) -> i32 {
        match self.vm.objects().static_value(profile, b"cornerRadius") {
            Some(Value::Int(radius)) => (*radius).clamp(0, MAX_PROFILE_RADIUS),
            _ => 0,
        }
    }

    /// One sprite of radius `radius` (at least 1) over `r`: the corners one texel a pixel, the edges and the middle stretched from the
    /// sprite's middle row and column (the border's middle is empty and not drawn).
    fn nine_slice(&mut self, r: Rect, radius: i32, sprite: Sprite, color: [u8; 4]) {
        let sprites = sprites_for(radius);
        let side = 2 * radius + 1;
        let (sx, sy) = (sprite as i32 * side, sprites.rows[radius as usize]);
        let (x0, y0) = (r[0], r[1]);
        let (mw, mh) = (r[2] - 2 * radius, r[3] - 2 * radius);
        let (x1, y1) = (x0.wrapping_add(radius), y0.wrapping_add(radius));
        let (x2, y2) = (x1.wrapping_add(mw), y1.wrapping_add(mh));
        let (u1, v1) = (sx + radius, sy + radius);
        let (u2, v2) = (u1 + 1, v1 + 1);
        let cols = [
            (x0, radius, sx, radius),
            (x1, mw, u1, 1),
            (x2, radius, u2, radius),
        ];
        let rows = [
            (y0, radius, sy, radius),
            (y1, mh, v1, 1),
            (y2, radius, v2, radius),
        ];
        for (j, &(y, h, v, vh)) in rows.iter().enumerate() {
            for (i, &(x, w, u, uw)) in cols.iter().enumerate() {
                if i == 1 && j == 1 && matches!(sprite, Sprite::Ring) {
                    continue;
                }
                self.stretch(&sprites.texture, [x, y, w, h], [u, v, uw, vh], color, false);
            }
        }
    }
}

#[cfg(test)]
#[path = "rounded_tests.rs"]
mod tests;
