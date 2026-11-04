//! GuiScrollCtrl as TGE 1.4 has it: the content area, the bars,
//! scrolling, the mouse on the bars and the wheel. The drawing is [`crate::draw`]'s.
//!
//! - **Constructor**: extent 200 x 200, the bar thickness and arrow length 16, the hit region `None` (10).
//! - **`onWake`**: `GuiControl::onWake` (the script's `onWake` included), then the profile's bitmap array, the base thumb
//!   size (rects 6 and 12's heights: the thumb caps), the bar thickness (rect 15's width: the page) and the arrow length (rect 0's
//!   height), then `computeSizes`.
//! - **`computeSizes`**: the content `borderThickness + childMargin` in, `extent - 2 margin - 2 border` big; the bars
//!   (`hScrollBar`, `vScrollBar`: 0 always on, 1 always off, 2 dynamic) take their thickness off it; the first child's bounds
//!   (`calcChildExtents`, any child) move the children when the content has room; the bars are enabled when the child is
//!   larger than the content; `calcScrollRects` (the arrows and tracks) and `calcThumbs` (unsigned divisions).
//! - **`resize`** and a child's resize (`childResized`) run `computeSizes`; so does `addObject`.
//! - **`scrollTo`**: each axis clamped to `[0, child - content]` (the upper bound first), the children moved;
//!   `scrollRectVisible`; `scrollToTop` / `scrollToBottom` (`scrollTo(0, 0)`, `scrollTo(0, 2^31 - 1)`).
//! - **The mouse** (down: the mouse locked, the key focus when the profile `canKeyFocus`, the hit region, depressed,
//!   `scrollByRegion` (a row or a page: the extension's line sizes, else 30 by 30), the thumb's anchor; dragged: the thumb
//!   follows inside the track widened by a bar's thickness, else back to the anchor; elsewhere depressed while over the region;
//!   up: unlocked, region None). **The wheel**: a row up or down (a page with Ctrl), then every child
//!   gets the event as a mouse move.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/containers/guiScrollCtrl.cpp`
//! (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use crate::control::{bounds, children, flag, is_a, point};
use crate::profile::{Rect, bitmap_array, border_thickness, profile_of, text};
use crate::{Gui, MAX_CHILDREN_WALKED};

/// The hit regions (`curHitRegion`).
pub const UP_ARROW: i32 = 0;
pub const DOWN_ARROW: i32 = 1;
pub const LEFT_ARROW: i32 = 2;
pub const RIGHT_ARROW: i32 = 3;
pub const UP_PAGE: i32 = 4;
pub const DOWN_PAGE: i32 = 5;
pub const LEFT_PAGE: i32 = 6;
pub const RIGHT_PAGE: i32 = 7;
pub const VERT_THUMB: i32 = 8;
pub const HORIZ_THUMB: i32 = 9;
pub const NONE: i32 = 10;

/// The bar modes (`hScrollBar`, `vScrollBar`).
pub const ALWAYS_ON: i32 = 0;
pub const DYNAMIC: i32 = 2;

/// A scroll control's state (GuiScrollCtrl's members).
#[derive(Debug, Clone, PartialEq)]
pub struct ScrollState {
    /// `mScrollBarThickness`, `mScrollBarArrowBtnLength`, `mBaseThumbSize`.
    pub thickness: i32,
    pub arrow_len: i32,
    pub base_thumb: i32,
    /// `mHBarEnabled`, `mVBarEnabled`, `mHasHScrollBar`, `mHasVScrollBar`.
    pub h_enabled: bool,
    pub v_enabled: bool,
    pub has_h: bool,
    pub has_v: bool,
    /// `mContentPos`, `mContentExt`, `mChildPos`, `mChildExt`, `mChildRelPos`.
    pub content_pos: (i32, i32),
    pub content_ext: (i32, i32),
    pub child_pos: (i32, i32),
    pub child_ext: (i32, i32),
    pub child_rel: (i32, i32),
    /// `mChildRelPosAnchor` and `mThumbMouseDelta`.
    pub anchor: (i32, i32),
    pub thumb_delta: i32,
    /// `mHThumbSize`, `mHThumbPos`, `mVThumbSize`, `mVThumbPos`.
    pub h_thumb: (i32, i32),
    pub v_thumb: (i32, i32),
    /// The rectangles (up, down, left, right, the horizontal track, the vertical one).
    pub up: Rect,
    pub down: Rect,
    pub left: Rect,
    pub right: Rect,
    pub h_track: Rect,
    pub v_track: Rect,
    /// `stateDepressed` and `curHitRegion`.
    pub depressed: bool,
    pub region: i32,
}

impl Default for ScrollState {
    fn default() -> ScrollState {
        ScrollState {
            thickness: 16,
            arrow_len: 16,
            base_thumb: 0,
            h_enabled: false,
            v_enabled: false,
            has_h: false,
            has_v: false,
            content_pos: (0, 0),
            content_ext: (0, 0),
            child_pos: (0, 0),
            child_ext: (0, 0),
            child_rel: (0, 0),
            anchor: (0, 0),
            thumb_delta: 0,
            h_thumb: (0, 0),
            v_thumb: (0, 0),
            up: [0; 4],
            down: [0; 4],
            left: [0; 4],
            right: [0; 4],
            h_track: [0; 4],
            v_track: [0; 4],
            depressed: false,
            region: NONE,
        }
    }
}

/// A copy of the control's scroll state (the default for one the model does not know).
pub fn state(gui: &Gui, id: ObjId) -> ScrollState {
    gui.with(id, |c| c.scroll.as_deref().cloned())
        .flatten()
        .unwrap_or_default()
}

fn store(gui: &Gui, id: ObjId, s: ScrollState) {
    gui.with_mut(id, |c| c.scroll = Some(Box::new(s)));
}

/// A bar mode field (`alwaysOn`, `alwaysOff`, `dynamic`, or the number).
fn mode(vm: &Vm, id: ObjId, field: &[u8]) -> i32 {
    let t = text(vm, id, field).to_ascii_lowercase();
    match t.as_slice() {
        b"alwayson" => 0,
        b"alwaysoff" => 1,
        b"dynamic" => 2,
        _ => crate::atoi(&t),
    }
}

/// `GuiScrollCtrl::onWake`'s own part (after `GuiControl::onWake`): the sizes from the profile's bitmap array.
pub fn after_wake(vm: &mut Vm, gui: &Gui, id: ObjId) {
    let mut s = state(gui, id);
    if let Some(p) = profile_of(vm, id) {
        let rects = bitmap_array(vm, gui, p);
        let r = |i: usize| rects.get(i).copied().unwrap_or([0; 4]);
        s.base_thumb = r(6)[3].wrapping_add(r(12)[3]);
        s.thickness = r(15)[2];
        s.arrow_len = r(0)[3];
        let name = crate::profile::text(vm, p, b"bitmap").to_ascii_lowercase();
        if !name.is_empty() {
            let mut g = gui.0.borrow_mut();
            if g.nearest_textures.len() < crate::textures::MAX_TEXTURES {
                g.nearest_textures
                    .insert(String::from_utf8_lossy(&name).into_owned());
            }
        }
    }
    store(gui, id, s);
    compute_sizes(vm, gui, id);
}

/// `computeSizes`.
pub fn compute_sizes(vm: &mut Vm, gui: &Gui, id: ObjId) {
    if !gui.is_control(id) {
        return;
    }
    let mut s = state(gui, id);
    let border = border_thickness(vm, profile_of(vm, id));
    let margin = point(vm, id, b"childMargin");
    let (_, ext) = bounds(vm, id);
    let (fh, fv) = (mode(vm, id, b"hScrollBar"), mode(vm, id, b"vScrollBar"));
    s.content_pos = (margin.0.wrapping_add(border), margin.1.wrapping_add(border));
    s.content_ext = (
        ext.0
            .wrapping_sub(margin.0.wrapping_mul(2))
            .wrapping_sub(border.wrapping_mul(2)),
        ext.1
            .wrapping_sub(margin.1.wrapping_mul(2))
            .wrapping_sub(border.wrapping_mul(2)),
    );
    s.h_enabled = false;
    s.v_enabled = false;
    s.has_v = fv == ALWAYS_ON;
    s.has_h = fh == ALWAYS_ON;
    let kids = children(vm, id);
    if let Some(&first) = kids.first() {
        // `calcChildExtents`: the first child.
        let (cp, ce) = bounds(vm, first);
        s.child_pos = cp;
        s.child_ext = ce;
        let t = s.thickness;
        if s.has_v {
            s.content_ext.0 = s.content_ext.0.wrapping_sub(t);
        }
        if s.has_h {
            s.content_ext.1 = s.content_ext.1.wrapping_sub(t);
        }
        if s.content_ext.0 < s.child_ext.0 && fh == DYNAMIC {
            s.has_h = true;
            s.content_ext.1 = s.content_ext.1.wrapping_sub(t);
        }
        if s.content_ext.1 < s.child_ext.1 && fv == DYNAMIC {
            s.has_v = true;
            s.content_ext.0 = s.content_ext.0.wrapping_sub(t);
            if s.content_ext.0 < s.child_ext.0 && !s.has_h && fh == DYNAMIC {
                s.has_h = true;
                s.content_ext.1 = s.content_ext.1.wrapping_sub(t);
            }
        }
        let delta_of = |cpos: i32, cext: i32, chpos: i32, chext: i32| -> i32 {
            if cpos < chpos {
                cpos.wrapping_sub(chpos)
            } else if chpos.wrapping_add(chext) < cpos.wrapping_add(cext) {
                let diff = cpos
                    .wrapping_add(cext)
                    .wrapping_sub(chpos.wrapping_add(chext));
                let room = cpos.wrapping_sub(chpos);
                diff.min(room)
            } else {
                0
            }
        };
        let dx = delta_of(
            s.content_pos.0,
            s.content_ext.0,
            s.child_pos.0,
            s.child_ext.0,
        );
        let dy = delta_of(
            s.content_pos.1,
            s.content_ext.1,
            s.child_pos.1,
            s.child_ext.1,
        );
        if dx != 0 || dy != 0 {
            move_children(vm, &kids, (dx, dy));
            s.child_pos = (
                s.child_pos.0.wrapping_add(dx),
                s.child_pos.1.wrapping_add(dy),
            );
        }
        if s.content_ext.0 < s.child_ext.0 {
            s.h_enabled = true;
        }
        if s.content_ext.1 < s.child_ext.1 {
            s.v_enabled = true;
        }
        s.child_rel = (
            s.content_pos.0.wrapping_sub(s.child_pos.0),
            s.content_pos.1.wrapping_sub(s.child_pos.1),
        );
    }
    let ext_hook = gui.ext();
    if !ext_hook.scroll_rects(&mut s, ext, border) {
        calc_scroll_rects(&mut s, ext, border);
    }
    thumbs(gui, &mut s, flag(vm, id, b"constantThumbHeight"));
    store(gui, id, s);
}

/// Every child's position moved by `d` (`mBounds.point` written directly: no resize).
fn move_children(vm: &mut Vm, kids: &[ObjId], d: (i32, i32)) {
    for &k in kids.iter().take(MAX_CHILDREN_WALKED) {
        if !vm.objects().contains(k) || !is_a(vm, k, "GuiControl") {
            continue;
        }
        let p = point(vm, k, b"position");
        let np = (p.0.wrapping_add(d.0), p.1.wrapping_add(d.1));
        vm.set_object_field(k, b"position", b"", format!("{} {}", np.0, np.1).as_bytes());
    }
}

/// `calcScrollRects`: the arrows at the ends of each bar, the tracks between them; a bar beside the other one's corner ends
/// one pixel into it.
fn calc_scroll_rects(s: &mut ScrollState, ext: (i32, i32), border: i32) {
    let t = s.thickness;
    let a = s.arrow_len;
    if s.has_h {
        s.left = [border, ext.1 - border - t, a, t];
        let vbar = if s.has_v { t - 1 } else { 0 };
        s.right = [ext.0 - border - vbar - a, ext.1 - border - t, a, t];
        let start = s.left[0] + s.left[2];
        s.h_track = [start, s.left[1], s.right[0] - start, t];
    }
    if s.has_v {
        s.up = [ext.0 - border - t, border, t, a];
        let hbar = if s.has_h { t - 1 } else { 0 };
        s.down = [ext.0 - border - t, ext.1 - border - hbar - a, t, a];
        let start = s.up[1] + s.up[3];
        s.v_track = [s.up[0], start, t, s.down[1] - start];
    }
}

/// The thumbs, by the extension's arithmetic when it has its own ([`crate::GuiExtension::scroll_thumbs`]).
fn thumbs(gui: &Gui, s: &mut ScrollState, constant: bool) {
    if !gui.ext().scroll_thumbs(s, constant) {
        calc_thumbs(s, constant);
    }
}

/// `calcThumbs`, for a bar that is enabled over a child with an extent: the thumb's size its track's share of the content
/// (rounded up, at least the base size; the base size with `constantThumbHeight`) and its place along the track, in unsigned
/// arithmetic as the track's length is unsigned.
fn calc_thumbs(s: &mut ScrollState, constant: bool) {
    let thumb = |base: i32, content: i32, child: i32, track: i32, rel: i32| -> (i32, i32) {
        let track = track as u32;
        let size = if constant {
            base
        } else {
            base.max(((content as u32).wrapping_mul(track) as f32 / child as f32).ceil() as i32)
        };
        let span = (rel as u32).wrapping_mul(track.wrapping_sub(size as u32));
        let pos = span
            .checked_div(child.wrapping_sub(content) as u32)
            .unwrap_or(0);
        (size, pos as i32)
    };
    if s.h_enabled && s.child_ext.0 > 0 {
        let (size, pos) = thumb(
            s.base_thumb,
            s.content_ext.0,
            s.child_ext.0,
            s.h_track[2],
            s.child_rel.0,
        );
        s.h_thumb = (size, s.h_track[0].wrapping_add(pos));
    }
    if s.v_enabled && s.child_ext.1 > 0 {
        let (size, pos) = thumb(
            s.base_thumb,
            s.content_ext.1,
            s.child_ext.1,
            s.v_track[3],
            s.child_rel.1,
        );
        s.v_thumb = (size, s.v_track[1].wrapping_add(pos));
    }
}

/// `scrollTo`: nothing without children.
pub fn scroll_to(vm: &mut Vm, gui: &Gui, id: ObjId, x: i32, y: i32) {
    let kids = children(vm, id);
    if kids.is_empty() {
        return;
    }
    let mut s = state(gui, id);
    let x = x.min(s.child_ext.0.wrapping_sub(s.content_ext.0)).max(0);
    let y = y.min(s.child_ext.1.wrapping_sub(s.content_ext.1)).max(0);
    let dx = x.wrapping_sub(s.child_rel.0);
    let dy = y.wrapping_sub(s.child_rel.1);
    s.child_rel = (x, y);
    s.child_pos = (
        s.child_pos.0.wrapping_sub(dx),
        s.child_pos.1.wrapping_sub(dy),
    );
    move_children(vm, &kids, (dx.wrapping_neg(), dy.wrapping_neg()));
    thumbs(gui, &mut s, flag(vm, id, b"constantThumbHeight"));
    store(gui, id, s);
}

/// `scrollDelta`: the relative position moved.
pub fn scroll_delta(vm: &mut Vm, gui: &Gui, id: ObjId, dx: i32, dy: i32) {
    let s = state(gui, id);
    scroll_to(
        vm,
        gui,
        id,
        s.child_rel.0.wrapping_add(dx),
        s.child_rel.1.wrapping_add(dy),
    );
}

/// `scrollRectVisible`: the content scrolled so the child's rectangle `r` is inside it.
pub fn scroll_rect_visible(vm: &mut Vm, gui: &Gui, id: ObjId, r: Rect) {
    let s = state(gui, id);
    let (w, h) = (r[2].min(s.content_ext.0), r[3].min(s.content_ext.1));
    let right = s.child_rel.0.wrapping_add(s.content_ext.0);
    let bottom = s.child_rel.1.wrapping_add(s.content_ext.1);
    let dx = if r[0] < s.child_rel.0 {
        r[0] - s.child_rel.0
    } else if right < w.wrapping_add(r[0]) {
        w.wrapping_add(r[0]) - right
    } else {
        0
    };
    let dy = if r[1] < s.child_rel.1 {
        r[1] - s.child_rel.1
    } else if bottom < h.wrapping_add(r[1]) {
        h.wrapping_add(r[1]) - bottom
    } else {
        0
    };
    if dx != 0 || dy != 0 {
        scroll_to(
            vm,
            gui,
            id,
            s.child_rel.0.wrapping_add(dx),
            s.child_rel.1.wrapping_add(dy),
        );
    }
}

/// `scrollByRegion`.
pub fn scroll_by_region(vm: &mut Vm, gui: &Gui, id: ObjId, region: i32) {
    if children(vm, id).is_empty() {
        return;
    }
    let s = state(gui, id);
    let (row, col) = gui.ext().scroll_line_sizes(vm, id).unwrap_or((30, 30));
    let page_v = if (s.content_ext.1 as u32) <= row {
        1
    } else {
        (s.content_ext.1 as u32).wrapping_sub(row)
    } as i32;
    let page_h = if (s.content_ext.0 as u32) <= col {
        1
    } else {
        (s.content_ext.0 as u32).wrapping_sub(col)
    } as i32;
    if s.v_enabled {
        let d = match region {
            UP_ARROW => Some((row as i32).wrapping_neg()),
            DOWN_ARROW => Some(row as i32),
            UP_PAGE => Some(page_v.wrapping_neg()),
            DOWN_PAGE => Some(page_v),
            _ => None,
        };
        if let Some(d) = d {
            scroll_delta(vm, gui, id, 0, d);
        }
    }
    if s.h_enabled {
        let d = match region {
            LEFT_ARROW => Some((col as i32).wrapping_neg()),
            RIGHT_ARROW => Some(col as i32),
            LEFT_PAGE => Some(page_h.wrapping_neg()),
            RIGHT_PAGE => Some(page_h),
            _ => None,
        };
        if let Some(d) = d {
            scroll_delta(vm, gui, id, d, 0);
        }
    }
}

fn in_rect(r: Rect, p: (i32, i32)) -> bool {
    r[0] <= p.0 && p.0 < r[0].wrapping_add(r[2]) && r[1] <= p.1 && p.1 < r[1].wrapping_add(r[3])
}

/// `findHitRegion` of a point in the control's coordinates.
pub fn hit_region(s: &ScrollState, p: (i32, i32)) -> i32 {
    if s.v_enabled && s.has_v {
        if in_rect(s.up, p) {
            return UP_ARROW;
        }
        if in_rect(s.down, p) {
            return DOWN_ARROW;
        }
        if in_rect(s.v_track, p) {
            if p.1 < s.v_thumb.1 {
                return UP_PAGE;
            }
            return if p.1 < s.v_thumb.0.wrapping_add(s.v_thumb.1) {
                VERT_THUMB
            } else {
                DOWN_PAGE
            };
        }
    }
    if s.h_enabled && s.has_h {
        if in_rect(s.left, p) {
            return LEFT_ARROW;
        }
        if in_rect(s.right, p) {
            return RIGHT_ARROW;
        }
        if in_rect(s.h_track, p) {
            if p.0 < s.h_thumb.1 {
                return LEFT_PAGE;
            }
            return if p.0 < s.h_thumb.0.wrapping_add(s.h_thumb.1) {
                HORIZ_THUMB
            } else {
                RIGHT_PAGE
            };
        }
    }
    NONE
}

/// `findHitControl`'s override: a point inside the content area (`borderThickness` from the top left, less the bars from
/// the bottom right) looks into the children; anywhere else the scroll control itself.
pub fn hits_content(vm: &Vm, gui: &Gui, id: ObjId, p: (i32, i32)) -> bool {
    let s = state(gui, id);
    let border = border_thickness(vm, profile_of(vm, id));
    let (_, ext) = bounds(vm, id);
    let vbar = if s.has_v { s.thickness } else { 0 };
    let hbar = if s.has_h { s.thickness } else { 0 };
    border <= p.0 && border <= p.1 && p.0 < ext.0 - border - vbar && p.1 < ext.1 - border - hbar
}

/// The point `pt` (canvas coordinates) in the control's (`globalToLocalCoord`).
pub fn local(vm: &Vm, id: ObjId, pt: (i32, i32)) -> (i32, i32) {
    let at = crate::control::screen_position(vm, id);
    (pt.0.wrapping_sub(at.0), pt.1.wrapping_sub(at.1))
}

/// `onMouseDown`.
pub fn mouse_down(vm: &mut Vm, gui: &Gui, id: ObjId, pt: (i32, i32)) -> Result<(), VmError> {
    crate::input::mouse_lock(vm, gui, id)?;
    if gui.is_awake(id)
        && flag(vm, id, b"visible")
        && crate::profile::pflag(vm, profile_of(vm, id), b"canKeyFocus")
    {
        crate::input::set_first_responder(vm, gui, id)?;
    }
    let p = local(vm, id, pt);
    let mut s = state(gui, id);
    let region = hit_region(&s, p);
    s.region = region;
    s.depressed = true;
    store(gui, id, s);
    scroll_by_region(vm, gui, id, region);
    let mut s = state(gui, id);
    if region == VERT_THUMB {
        s.anchor = s.child_rel;
        s.thumb_delta = p.1.wrapping_sub(s.v_thumb.1);
    } else if region == HORIZ_THUMB {
        s.anchor = s.child_rel;
        s.thumb_delta = p.0.wrapping_sub(s.h_thumb.1);
    }
    store(gui, id, s);
    Ok(())
}

/// `onMouseUp`.
pub fn mouse_up(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    crate::input::mouse_unlock(vm, gui, id)?;
    gui.with_mut(id, |c| {
        if let Some(s) = c.scroll.as_mut() {
            s.region = NONE;
            s.depressed = false;
        }
    });
    Ok(())
}

/// `onMouseDragged`.
pub fn mouse_dragged(vm: &mut Vm, gui: &Gui, id: ObjId, pt: (i32, i32)) {
    let p = local(vm, id, pt);
    let s = state(gui, id);
    let t = s.thickness;
    if s.region == VERT_THUMB || s.region == HORIZ_THUMB {
        gui.ext().thumb_dragged(gui, id);
    }
    match s.region {
        VERT_THUMB => {
            let tr = s.v_track;
            if tr[0] - t <= p.0
                && p.0 <= tr[2] - 1 + t + tr[0]
                && tr[1] - t <= p.1
                && p.1 <= tr[1] - 1 + tr[3] + t
            {
                let y = p.1.wrapping_sub(s.thumb_delta);
                if y == s.v_thumb.1 {
                    return;
                }
                let span = tr[3].wrapping_sub(s.v_thumb.0);
                let ny = if span == 0 {
                    0
                } else {
                    s.child_ext
                        .1
                        .wrapping_sub(s.content_ext.1)
                        .wrapping_mul(y.wrapping_sub(tr[1]))
                        .wrapping_div(span)
                };
                scroll_to(vm, gui, id, s.anchor.0, ny);
            } else {
                scroll_to(vm, gui, id, s.anchor.0, s.anchor.1);
            }
        }
        HORIZ_THUMB => {
            let tr = s.h_track;
            if tr[0] - t <= p.0
                && p.0 <= tr[0] - 1 + tr[2] + t
                && tr[1] - t <= p.1
                && p.1 <= tr[3] - 1 + tr[1] + t
            {
                let x = p.0.wrapping_sub(s.thumb_delta);
                if x == s.h_thumb.1 {
                    return;
                }
                let span = tr[2].wrapping_sub(s.h_thumb.0);
                let nx = if span == 0 {
                    0
                } else {
                    s.child_ext
                        .0
                        .wrapping_sub(s.content_ext.0)
                        .wrapping_mul(x.wrapping_sub(tr[0]))
                        .wrapping_div(span)
                };
                scroll_to(vm, gui, id, nx, s.anchor.1);
            } else {
                scroll_to(vm, gui, id, s.anchor.0, s.anchor.1);
            }
        }
        region => {
            let now = hit_region(&s, p);
            gui.with_mut(id, |c| {
                if let Some(s) = c.scroll.as_mut() {
                    s.depressed = now == region;
                }
            });
        }
    }
}

/// The wheel (`onMouseWheelUp`, `onMouseWheelDown`): awake and visible, a row (a page with Ctrl), then each child's
/// `onMouseMove`. Whether it was taken.
pub fn wheel(vm: &mut Vm, gui: &Gui, id: ObjId, up: bool, modifier: u8) -> bool {
    if !gui.is_awake(id) || !flag(vm, id, b"visible") {
        return false;
    }
    let page = modifier & 0x0c != 0;
    let region = match (up, page) {
        (true, false) => UP_ARROW,
        (true, true) => UP_PAGE,
        (false, false) => DOWN_ARROW,
        (false, true) => DOWN_PAGE,
    };
    let before = state(gui, id).child_rel;
    scroll_by_region(vm, gui, id, region);
    report_moved(vm, gui, id, before);
    true
}

/// The content's move since `before` (its relative position then), told to the extension ([`crate::GuiExtension::scrolled`]).
pub fn report_moved(vm: &Vm, gui: &Gui, id: ObjId, before: (i32, i32)) {
    let after = state(gui, id).child_rel;
    gui.ext().scrolled(
        vm,
        gui,
        id,
        (
            before.0.wrapping_sub(after.0),
            before.1.wrapping_sub(after.1),
        ),
    );
}

/// The control's state as drawn `now`: the relative position the extension's displacement ([`crate::GuiExtension::shift`])
/// shows (kept inside the scroll range) and the thumbs there, with the displacement its children are drawn at. Without one,
/// the state and none.
pub fn shown(vm: &Vm, gui: &Gui, id: ObjId, now: u32) -> (ScrollState, (i32, i32)) {
    let mut s = state(gui, id);
    let d = gui.ext().shift(gui, id, now);
    if d == (0, 0) {
        return (s, d);
    }
    let max = (
        s.child_ext.0.wrapping_sub(s.content_ext.0).max(0),
        s.child_ext.1.wrapping_sub(s.content_ext.1).max(0),
    );
    let rel = (
        s.child_rel.0.wrapping_sub(d.0).clamp(0, max.0),
        s.child_rel.1.wrapping_sub(d.1).clamp(0, max.1),
    );
    let d = (
        s.child_rel.0.wrapping_sub(rel.0),
        s.child_rel.1.wrapping_sub(rel.1),
    );
    s.child_rel = rel;
    thumbs(gui, &mut s, flag(vm, id, b"constantThumbHeight"));
    (s, d)
}
