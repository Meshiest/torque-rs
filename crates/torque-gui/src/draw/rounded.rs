//! The engine's rounded boxes: what GuiButtonCtrl, a GuiPopUpMenuCtrl with a border and its open list, and GuiSliderCtrl's
//! thumb draw unless the host keeps TGE's bevels for their profile ([`crate::ext::GuiExtension::classic_boxes`]).
//!
//! - **The shape**: a rectangle with corners of [`BOX_RADIUS`] pixels (less on a box under twice that), a solid fill and a
//!   one-pixel border, both with anti-aliased corners.
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

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::{Walk, color_i};
use crate::profile::Rect;
use crate::textures::GuiTexture;

/// The corner radius of a rounded box (pixels).
pub const BOX_RADIUS: i32 = 5;
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

/// The corner sprites of every radius from 1 to [`BOX_RADIUS`]: row `r` (at [`Sprites::rows`]) holds three squares of side
/// `2r + 1`, white with the coverage as alpha: the whole shape, the shape half a pixel in (under a border) and the border.
struct Sprites {
    texture: Arc<GuiTexture>,
    rows: [i32; BOX_RADIUS as usize + 1],
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

fn build_sprites() -> Sprites {
    let max_side = 2 * BOX_RADIUS as u32 + 1;
    let width = 3 * max_side;
    let height: u32 = (1..=BOX_RADIUS as u32).map(|r| 2 * r + 1).sum();
    let size = (crate::textures::pow2(width), crate::textures::pow2(height));
    let mut rgba = vec![0u8; (size.0 * size.1 * 4) as usize];
    let mut rows = [0i32; BOX_RADIUS as usize + 1];
    let mut y0 = 0u32;
    for r in 1..=BOX_RADIUS as u32 {
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
            name: "engine:rounded-box".to_string(),
            bitmap_size: (width, height),
            size,
            rgba,
            nearest: true,
            clamp: true,
        }),
        rows,
    }
}

static SPRITES: LazyLock<Sprites> = LazyLock::new(build_sprites);

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
    /// A rounded box over `r` (x, y, width, height): `fill`, then a one-pixel `border` round it (none when its alpha is 0).
    pub(super) fn rounded_box(&mut self, r: Rect, fill: [u8; 4], border: [u8; 4]) {
        if r[2] <= 0 || r[3] <= 0 {
            return;
        }
        let radius = BOX_RADIUS.min(r[2] / 2).min(r[3] / 2);
        let bordered = border[3] != 0;
        if radius == 0 {
            self.fill(r, fill);
            if bordered {
                let (rt, b) = (r[0].wrapping_add(r[2]) - 1, r[1].wrapping_add(r[3]) - 1);
                self.outline((r[0], r[1]), (rt, b), border);
            }
            return;
        }
        let texture = box_texture();
        let fill_sprite = if bordered {
            Sprite::Inner
        } else {
            Sprite::Fill
        };
        self.nine_slice(&texture, r, radius, fill_sprite, fill);
        if bordered {
            self.nine_slice(&texture, r, radius, Sprite::Ring, border);
        }
    }

    /// One sprite of radius `radius` over `r`: the corners one texel a pixel, the edges and the middle stretched from the
    /// sprite's middle row and column (the border's middle is empty and not drawn).
    fn nine_slice(
        &mut self,
        t: &Arc<GuiTexture>,
        r: Rect,
        radius: i32,
        sprite: Sprite,
        color: [u8; 4],
    ) {
        let side = 2 * radius + 1;
        let (sx, sy) = (sprite as i32 * side, SPRITES.rows[radius as usize]);
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
                self.stretch(t, [x, y, w, h], [u, v, uw, vh], color, false);
            }
        }
    }
}

#[cfg(test)]
#[path = "rounded_tests.rs"]
mod tests;
