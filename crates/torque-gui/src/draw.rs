//! The canvas drawn as a draw list: [`GuiDraws`] is plain data (no wgpu) that a renderer's GUI pass draws. The rules are TGE
//! 1.4's `GuiCanvas::renderFrame`, the controls' `onRender` and the `dgl` primitives.
//!
//! - **The frame** (`GuiCanvas::renderFrame`): every child of the canvas (the content, then the dialogs in order), visible or
//!   not, drawn at its position with the clip rectangle the whole canvas (the update region of a settled screen); then the cursor when
//!   the canvas's cursor is on (`cursorOn`) and shown (`showCursor`) and there is one: its bitmap (`bitmapName`, a `BitmapTexture` that
//!   repeats) at the cursor position less its `hotSpot`, one texel a pixel, modulated white.
//! - **`dglSetClipRect`**: `glOrtho` over the rectangle and `glViewport` on it: drawing is clipped to it, pixel for pixel
//!   cut by the parent's update rectangle; a child the extension does not clip to its parent
//!   ([`crate::GuiExtension::clip_to_parent`]) is drawn when that cut is not empty but clipped to the whole viewport instead, and
//!   such a parent cuts its children by the viewport.
//!   cut by the parent's update rectangle; a control the extension does not clip to its parent ([`crate::GuiExtension::clip_to_parent`]) is drawn when that cut is not empty
//!   but clipped to the whole viewport instead, and a parent with `clipToParent` 0 cuts its children by the viewport.
//! - **`GuiControl::onRender`**: the script's `onRender` once (the "has rendered" flag), a fill of the profile's
//!   `fillColor` when `opaque`, the border when `border` is not 0, then the children.
//! - **`dglDrawBitmapStretchSR`**: `GL_MODULATE` with the bitmap modulation colour, blending `SRC_ALPHA,
//!   ONE_MINUS_SRC_ALPHA` (`DST_COLOR, ONE_MINUS_SRC_ALPHA` with the multiply flag), one quad from the destination rectangle's corners,
//!   texture coordinates the source rectangle over the padded texture's size (`texWidth`, `texHeight`).
//! - **`dglDrawRectFill`**: an untextured `glRecti` in the colour, blended. **`dglDrawRect`**: a line loop
//!   through the corner pixels' centres (drawn here as the one-pixel outline it rasterizes to).
//! - **GuiBitmapCtrl**, **GuiBitmapButtonCtrl** (`renderButton`), **GuiTextCtrl** (`onRender` and
//!   `renderJustifiedText`), **GuiFadeinBitmapCtrl**, **GuiChunkedBitmapCtrl**
//!   (`renderRegion`): see each function below. Other classes draw as `GuiControl` (counted in
//!   [`DrawReport::unmodelled`]: the later menus' classes are for a later step).
//!
//! **No script runs while the list is made.** The callback that runs from inside a draw (a control's first `onRender`) is returned
//! (a fade's `onWait` and `onDone` are [`crate::Gui::frame`]'s) ([`Frame::callbacks`]) and run afterwards by [`run_callbacks`], with no borrow held. The walk is bounded by
//! [`crate::MAX_TREE_DEPTH`], [`crate::MAX_CHILDREN_WALKED`], [`MAX_CONTROLS`] and [`MAX_QUADS`]; the textures by the cache's caps
//! ([`crate::textures::MAX_TEXTURES`], [`crate::textures::MAX_CACHE_BYTES`]); GuiChunkedBitmapCtrl's bitmaps by [`ChunkCache`] (one
//! decode per resolved file, at most [`MAX_CHUNKED_BYTES`], least recently used dropped first) and the sheet textures by
//! [`MAX_SHEET_TEXTURES`] and [`MAX_SHEET_BYTES`].
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/core/guiCanvas.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.), `gui/core/guiControl.cpp` (Copyright (c) 2012 GarageGames, LLC;
//! Copyright (C) 2015 Faust Logic, Inc.), `gfx/gfxDrawUtil.cpp` (Copyright (c) 2012 GarageGames, LLC),
//! `gui/controls/guiBitmapCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC), `gui/buttons/guiBitmapButtonCtrl.cpp` (Copyright (c)
//! 2012 GarageGames, LLC), `gui/controls/guiTextCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC),
//! `gui/game/guiFadeinBitmapCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC), `gui/game/guiChunkedBitmapCtrl.cpp` (Copyright (c)
//! 2012 GarageGames, LLC), `gui/3d/guiTSControl.cpp` (Copyright (c) 2012 GarageGames, LLC), `T3D/gameTSCtrl.cpp` (Copyright (c)
//! 2012 GarageGames, LLC); the code here is this crate's own.

use std::collections::BTreeMap;
use std::sync::Arc;

use torque_fonts::TextColors;
use torque_ts_vm::host::FileRead;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use crate::control::{bounds, children, flag, is_a, point};
use crate::textures::{GuiTexture, GuiTextures, texture_of};
use crate::{Gui, MAX_TREE_DEPTH, atoi};

mod hud;
mod menus;
mod rounded;
pub use hud::{color_byte, project};
use menus::CellKind;
pub use rounded::{BOX_RADIUS, BoxState, DEFAULT_FILL, PRESSED_SHADE, box_colors, box_texture};

/// The most quads one frame draws (a hostile GUI with a million controls draws to here).
pub const MAX_QUADS: usize = 1 << 20;
/// The most controls one frame visits.
pub const MAX_CONTROLS: u32 = 1 << 18;
/// The most GameTSCtrls one frame keeps the projection of.
pub const MAX_TS_RECTS: usize = 64;

/// The most columns a GuiGraphCtrl plots (a hostile extent draws to here).
pub const MAX_GRAPH_COLUMNS: i32 = 4096;

/// The most font-sheet and chunk textures one cache keeps.
pub const MAX_SHEET_TEXTURES: usize = 4096;
/// The most texel bytes the font-sheet and chunk textures of one cache hold (4096 chunks of 256 x 256 were 1 GiB).
pub const MAX_SHEET_BYTES: usize = 256 << 20;
/// The most decoded bytes GuiChunkedBitmapCtrl's bitmaps hold at once: past it the least recently used are dropped (and
/// decoded again when drawn again); one bitmap above it is not kept.
pub const MAX_CHUNKED_BYTES: usize = 128 << 20;
/// The most later siblings a control is checked against for one covering it whose fill the extension replaces.
pub const MAX_COVER_SIBLINGS: usize = 64;

/// How a quad is blended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GuiBlend {
    /// `SRC_ALPHA, ONE_MINUS_SRC_ALPHA`.
    Alpha,
    /// `DST_COLOR, ONE_MINUS_SRC_ALPHA` (a bitmap's `mMultiply`).
    Multiply,
}

/// One quad: the screen rectangle (left, top, right, bottom, pixels), the texture coordinates (left, top, right, bottom) and the
/// modulation colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuiQuad {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub color: [u8; 4],
}

/// Quads drawn with one state: the clip rectangle (x, y, width, height), the texture (none: untextured) and the blend.
#[derive(Debug, Clone, PartialEq)]
pub struct GuiBatch {
    pub clip: [i32; 4],
    pub texture: Option<Arc<GuiTexture>>,
    pub blend: GuiBlend,
    pub quads: Vec<GuiQuad>,
}

/// What a frame drew that is not modelled or could not be found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DrawReport {
    pub controls: u32,
    /// Classes drawn as `GuiControl` because their own `onRender` is not modelled yet, by name.
    pub unmodelled: BTreeMap<String, u32>,
    /// Text drawn with no font (no cache file and no fallback), bitmaps not found.
    pub no_font: u32,
    pub no_texture: u32,
    /// The faces and sizes no font was found for (`Face_Size`).
    pub missing_fonts: std::collections::BTreeSet<String>,
    /// Bytes with no glyph in the font (a lookup past its table).
    pub invalid_glyphs: u32,
    /// Profile borders of a style other than 1, drawn as style 1.
    pub border_styles: u32,
    /// The walk stopped at [`MAX_QUADS`].
    pub truncated: bool,
    /// Controls not drawn because a later sibling whose fill the extension replaces covers them whole.
    pub covered: u32,
    /// The extension's own counts, by name.
    pub extension: BTreeMap<&'static str, u32>,
}

/// A frame's draw list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GuiDraws {
    pub size: (u32, u32),
    pub batches: Vec<GuiBatch>,
    pub report: DrawReport,
    /// The GameTSCtrls' worlds, each to be drawn before its batch (the client renders the connection's world there).
    pub worlds: Vec<WorldDraw>,
    /// What the extension asked for between batches, each before its batch, in order.
    pub extras: Vec<ExtraDraw>,
    /// Quads pushed so far (the cap's running count; [`GuiDraws::quads`] counts the batches).
    pub pushed: usize,
}

impl GuiDraws {
    pub fn quads(&self) -> usize {
        self.batches.iter().map(|b| b.quads.len()).sum()
    }

    /// Whether the quad budget is spent ([`MAX_QUADS`]): every loop of the walk stops at it.
    pub fn full(&self) -> bool {
        self.report.truncated || self.pushed >= MAX_QUADS
    }

    fn push(
        &mut self,
        clip: [i32; 4],
        texture: Option<&Arc<GuiTexture>>,
        blend: GuiBlend,
        quad: GuiQuad,
    ) {
        if self.pushed >= MAX_QUADS {
            self.report.truncated = true;
            return;
        }
        self.pushed += 1;
        // A view's or a world's frame goes between two batches: the next quad starts a batch of its own.
        let after_view = self
            .extras
            .last()
            .is_some_and(|v| v.at_batch == self.batches.len())
            || self
                .worlds
                .last()
                .is_some_and(|w| w.at_batch == self.batches.len());
        if !after_view
            && let Some(last) = self.batches.last_mut()
            && last.clip == clip
            && last.blend == blend
            && last.texture.as_ref().map(|t| t.id) == texture.map(|t| t.id)
        {
            last.quads.push(quad);
            return;
        }
        self.batches.push(GuiBatch {
            clip,
            texture: texture.cloned(),
            blend,
            quads: vec![quad],
        });
    }
}

/// Where a GameTSCtrl draws the world (GuiTSCtrl::onRender: the viewport set to the control, `renderWorld`, then the
/// children over it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldDraw {
    /// The batch the frame goes before.
    pub at_batch: usize,
    /// The screen rectangle drawn (the control cut by its update rectangle) and the control's whole one.
    pub rect: [i32; 4],
    pub control: [i32; 4],
}

/// Any value an extension's draw keeps.
pub trait ExtraItem: std::any::Any + std::fmt::Debug {
    fn as_any(&self) -> &dyn std::any::Any;
}

impl<T: std::any::Any + std::fmt::Debug> ExtraItem for T {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Something an extension draws between two batches (the host's renderer knows its type): before batch `at_batch`.
#[derive(Debug, Clone)]
pub struct ExtraDraw {
    pub at_batch: usize,
    pub item: std::rc::Rc<dyn ExtraItem>,
}

impl ExtraDraw {
    /// The item as a `T`, when it is one.
    pub fn get<T: std::any::Any>(&self) -> Option<&T> {
        (*self.item).as_any().downcast_ref::<T>()
    }
}

impl PartialEq for ExtraDraw {
    fn eq(&self, other: &ExtraDraw) -> bool {
        self.at_batch == other.at_batch && std::rc::Rc::ptr_eq(&self.item, &other.item)
    }
}

impl GuiDraws {
    /// The extras that are a `T`, with the batch each goes before.
    pub fn extras_of<T: std::any::Any>(&self) -> impl Iterator<Item = (usize, &T)> {
        self.extras
            .iter()
            .filter_map(|e| e.get::<T>().map(|v| (e.at_batch, v)))
    }
}

/// A script callback a draw asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Callback {
    /// The control's first `onRender` (`GuiControl::onRender`).
    OnRender(ObjId),
}

/// A frame: the draw list and the callbacks to run after it.
#[derive(Debug, Clone, Default)]
pub struct Frame {
    pub draws: GuiDraws,
    pub callbacks: Vec<Callback>,
}

/// What a frame reuses: the textures, the fonts and their sheets, and `dglDrawTextN`'s saved colour (a global in TGE).
pub struct DrawCache {
    pub textures: GuiTextures,
    pub fonts: Box<dyn crate::FontSource>,
    sheets: BTreeMap<(String, i32, usize), Arc<GuiTexture>>,
    /// The sheets' texel bytes.
    sheet_bytes: usize,
    /// GuiChunkedBitmapCtrl's bitmaps.
    pub chunked: ChunkCache,
    saved_color: [u8; 4],
}

/// GuiChunkedBitmapCtrl's bitmaps: each name resolved once to the file the texture search found (names that differ only
/// by the parent-folder search, `X/a1/t` and `X/a2/t` for `X/t.png`, share it), each file decoded once, the decoded bytes at most
/// [`MAX_CHUNKED_BYTES`] (least recently drawn dropped first).
pub struct ChunkCache {
    /// Name (lower case) to the resolved file's key (lower case), None when no candidate was found.
    names: BTreeMap<String, Option<String>>,
    /// Resolved file to its bitmap (None: it did not decode), when it was last used, and the frame of that use.
    files: BTreeMap<String, (Option<Arc<torque_images::Bitmap>>, u64, u64)>,
    /// Each file's decoded size once known: a file that cannot fit is then skipped without decoding it again.
    sizes: BTreeMap<String, usize>,
    bytes: usize,
    clock: u64,
    /// The frame being drawn ([`draw_canvas`] starts each): what it drew is never dropped while it draws.
    frame: u64,
    /// How many times a file was decoded.
    pub decodes: u64,
    /// The decoded bytes held at most ([`MAX_CHUNKED_BYTES`]; a test lowers it).
    pub cap: usize,
}

impl Default for ChunkCache {
    fn default() -> ChunkCache {
        ChunkCache {
            names: BTreeMap::new(),
            files: BTreeMap::new(),
            sizes: BTreeMap::new(),
            bytes: 0,
            clock: 0,
            frame: 0,
            decodes: 0,
            cap: MAX_CHUNKED_BYTES,
        }
    }
}

impl ChunkCache {
    /// The decoded bytes held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// How many files are held.
    pub fn files(&self) -> usize {
        self.files.len()
    }

    fn begin_frame(&mut self) {
        self.frame += 1;
    }

    fn touch(&mut self, file: &str) -> Option<Option<Arc<torque_images::Bitmap>>> {
        self.clock += 1;
        let (now, frame) = (self.clock, self.frame);
        self.files.get_mut(file).map(|e| {
            e.1 = now;
            e.2 = frame;
            e.0.clone()
        })
    }

    /// Whether `size` bytes fit once every bitmap this frame has not drawn is dropped.
    fn fits(&self, size: usize) -> bool {
        let pinned: usize = self
            .files
            .values()
            .filter(|e| e.2 == self.frame)
            .filter_map(|e| e.0.as_ref())
            .map(|b| b.bits.len())
            .sum();
        size <= self.cap && pinned + size <= self.cap
    }

    /// Whether `file` is worth decoding: its size is unknown, or it fits.
    fn may_decode(&self, file: &str) -> bool {
        self.sizes.get(file).is_none_or(|&n| self.fits(n))
    }

    /// Keeps `bitmap` for `file`, dropping the least recently used bitmaps this frame has not drawn until it fits; None when it
    /// cannot fit (it is then not drawn this frame, and not decoded again while it cannot).
    fn insert(
        &mut self,
        file: String,
        bitmap: Option<Arc<torque_images::Bitmap>>,
    ) -> Option<Arc<torque_images::Bitmap>> {
        let size = bitmap.as_ref().map_or(0, |b| b.bits.len());
        if self.sizes.len() < MAX_SHEET_TEXTURES {
            self.sizes.insert(file.clone(), size);
        }
        if bitmap.is_some() && !self.fits(size) {
            return None;
        }
        while self.bytes + size > self.cap {
            let Some(oldest) = self
                .files
                .iter()
                .filter(|(_, e)| e.0.is_some() && e.2 != self.frame)
                .min_by_key(|(_, e)| e.1)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((Some(b), _, _)) = self.files.remove(&oldest) {
                self.bytes -= b.bits.len();
            }
        }
        if self.files.len() >= MAX_SHEET_TEXTURES || self.bytes + size > self.cap {
            return None;
        }
        self.clock += 1;
        self.bytes += size;
        self.files
            .insert(file, (bitmap.clone(), self.clock, self.frame));
        bitmap
    }
}

impl DrawCache {
    pub fn new(fonts: Box<dyn crate::FontSource>) -> DrawCache {
        DrawCache {
            textures: GuiTextures::default(),
            fonts,
            sheets: BTreeMap::new(),
            sheet_bytes: 0,
            chunked: ChunkCache::default(),
            saved_color: [0; 4],
        }
    }
}

impl Default for DrawCache {
    fn default() -> DrawCache {
        DrawCache::new(crate::ext::default_fonts())
    }
}

impl DrawCache {
    /// A cache whose fonts come from `gui`'s extension's source (else the default).
    pub fn for_gui(gui: &Gui) -> DrawCache {
        DrawCache::new(gui.ext().fonts().unwrap_or_else(crate::ext::default_fonts))
    }
}

/// A `ColorI` field: up to four integers (`%d %d %d %d`), the alpha 255 when absent, each kept to its low byte.
pub fn color_i(text: &[u8]) -> [u8; 4] {
    let mut out = [0, 0, 0, 255];
    for (i, w) in text
        .split(|b| *b == b' ' || *b == b'\t')
        .filter(|w| !w.is_empty())
        .take(4)
        .enumerate()
    {
        out[i] = atoi(w) as u8;
    }
    out
}

fn text_field(vm: &Vm, id: ObjId, name: &[u8]) -> Vec<u8> {
    vm.objects().field(id, name, b"").unwrap_or_default()
}

fn color_field(vm: &Vm, id: ObjId, name: &[u8]) -> [u8; 4] {
    color_i(&text_field(vm, id, name))
}

fn int_field(vm: &Vm, id: ObjId, name: &[u8]) -> i32 {
    atoi(&text_field(vm, id, name))
}

/// The intersection `renderChildControls` computes (x, y, width, height; empty when width or height is not positive).
fn intersect(u: [i32; 4], c: [i32; 4]) -> [i32; 4] {
    let x = u[0].max(c[0]);
    let y = u[1].max(c[1]);
    let right = if u[0].wrapping_add(u[2]).wrapping_sub(1) < c[0].wrapping_add(c[2]).wrapping_sub(1)
    {
        u[0].wrapping_add(u[2])
    } else {
        c[0].wrapping_add(c[2])
    };
    let bottom =
        if u[1].wrapping_add(u[3]).wrapping_sub(1) < c[1].wrapping_add(c[3]).wrapping_sub(1) {
            u[1].wrapping_add(u[3])
        } else {
            c[1].wrapping_add(c[3])
        };
    [x, y, right.wrapping_sub(x), bottom.wrapping_sub(y)]
}

/// The profile a control draws with (its `profile` field's object), if it exists.
fn profile_of(vm: &Vm, id: ObjId) -> Option<ObjId> {
    let name = text_field(vm, id, b"profile");
    if name.is_empty() {
        return None;
    }
    vm.objects()
        .find(&name)
        .filter(|p| is_a(vm, *p, "GuiControlProfile"))
}

/// What a draw walk carries.
struct Walk<'a> {
    vm: &'a Vm,
    gui: &'a Gui,
    cache: &'a mut DrawCache,
    out: Frame,
    /// The viewport (`dglSetViewport`: the whole canvas).
    viewport: [i32; 4],
    /// The current clip rectangle (`dglSetClipRect`).
    clip: [i32; 4],
    /// The tick count (`Vm::sim_time`, as [`crate::Fade`]).
    now: u32,
    /// Each GameTSCtrl drawn this frame: its id, its whole rectangle and its update rectangle (`GuiTSCtrl::onRender`'s saved
    /// projection, which GuiShapeNameHud's `project` reads).
    ts_rects: Vec<(ObjId, [i32; 4], [i32; 4])>,
    /// The host's extension and the opacity the subtree draws at ([`crate::GuiExtension::opacity`]).
    ext: std::rc::Rc<dyn crate::GuiExtension>,
    alpha: f32,
    /// Each profile's [`crate::GuiExtension::replaces_fill`], read once a frame (no script runs during the walk, so it holds
    /// for the frame).
    blurred: std::cell::RefCell<std::collections::HashMap<ObjId, bool>>,
}

impl Walk<'_> {
    /// A colour drawn at the subtree's opacity (unchanged at 1).
    fn faded(&self, mut c: [u8; 4]) -> [u8; 4] {
        if self.alpha < 1.0 {
            c[3] = (f32::from(c[3]) * self.alpha).round() as u8;
        }
        c
    }

    /// Whether the extension replaces a profile's fill ([`crate::GuiExtension::replaces_fill`]).
    fn blurred(&self, profile: ObjId) -> bool {
        *self
            .blurred
            .borrow_mut()
            .entry(profile)
            .or_insert_with(|| self.ext.replaces_fill(self.vm, profile))
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        match self.vm.file_host().read(path.as_bytes()) {
            FileRead::Found(b) => Some(b),
            _ => None,
        }
    }

    fn texture(&mut self, name: &[u8], clamp: bool) -> Option<Arc<GuiTexture>> {
        let name = String::from_utf8_lossy(name).into_owned();
        if name.is_empty() {
            return None;
        }
        let vm = self.vm;
        let read = |p: &str| match vm.file_host().read(p.as_bytes()) {
            FileRead::Found(b) => Some(b),
            _ => None,
        };
        let t = self.cache.textures.get(&name, clamp, &read);
        if t.is_none() {
            self.out.draws.report.no_texture += 1;
        }
        t
    }

    /// `dglDrawBitmapStretchSR`: `dst` (x, y, w, h) from `src` (x, y, w, h) of the texture, modulated by `color`.
    fn stretch(
        &mut self,
        t: &Arc<GuiTexture>,
        dst: [i32; 4],
        src: [i32; 4],
        color: [u8; 4],
        multiply: bool,
    ) {
        if dst[2] <= 0 || dst[3] <= 0 {
            return;
        }
        let (tw, th) = (t.size.0 as f32, t.size.1 as f32);
        let quad = GuiQuad {
            rect: [
                dst[0] as f32,
                dst[1] as f32,
                dst[0].wrapping_add(dst[2]) as f32,
                dst[1].wrapping_add(dst[3]) as f32,
            ],
            uv: [
                src[0] as f32 / tw,
                src[1] as f32 / th,
                src[0].wrapping_add(src[2]) as f32 / tw,
                src[1].wrapping_add(src[3]) as f32 / th,
            ],
            color: self.faded(color),
        };
        let blend = if multiply {
            GuiBlend::Multiply
        } else {
            GuiBlend::Alpha
        };
        self.out.draws.push(self.clip, Some(t), blend, quad);
    }

    /// `dglDrawRectFill`.
    fn fill(&mut self, r: [i32; 4], color: [u8; 4]) {
        if r[2] <= 0 || r[3] <= 0 {
            return;
        }
        let quad = GuiQuad {
            rect: [
                r[0] as f32,
                r[1] as f32,
                r[0].wrapping_add(r[2]) as f32,
                r[1].wrapping_add(r[3]) as f32,
            ],
            uv: [0.0; 4],
            color: self.faded(color),
        };
        self.out.draws.push(self.clip, None, GuiBlend::Alpha, quad);
    }

    /// `dglDrawRect(upperLeft, lowerRight, color)`: the outline of the pixels from one corner to the other, both included.
    fn outline(&mut self, a: (i32, i32), b: (i32, i32), color: [u8; 4]) {
        let (w, h) = (
            b.0.wrapping_sub(a.0).wrapping_add(1),
            b.1.wrapping_sub(a.1).wrapping_add(1),
        );
        if w <= 0 || h <= 0 {
            return;
        }
        self.fill([a.0, a.1, w, 1], color);
        if h > 1 {
            self.fill([a.0, b.1, w, 1], color);
        }
        if h > 2 {
            self.fill([a.0, a.1 + 1, 1, h - 2], color);
            if w > 1 {
                self.fill([b.0, a.1 + 1, 1, h - 2], color);
            }
        }
    }

    fn set_clip(&mut self, r: [i32; 4]) {
        self.clip = r;
    }

    /// The rectangle (x0, y0, x1, y1) of a shown control whose profile has a blurred backdrop; `None` for any other.
    fn frost_rect(&self, id: ObjId) -> Option<[i64; 4]> {
        if !profile_of(self.vm, id).is_some_and(|pr| self.blurred(pr)) {
            return None;
        }
        let (p, e) = bounds(self.vm, id);
        let (x0, y0) = (i64::from(p.0), i64::from(p.1));
        Some([x0, y0, x0 + i64::from(e.0), y0 + i64::from(e.1)])
    }

    /// Whether child `at` is covered whole by one of its later siblings whose fill the extension replaces (`frost`: their indexes
    /// and rectangles, in order), of the [`MAX_COVER_SIBLINGS`] after it: such a sibling would show the covered one through
    /// where an opaque fill hid it.
    fn covered_by_frost(&self, child: ObjId, at: usize, frost: &[(usize, [i64; 4])]) -> bool {
        let first = frost.partition_point(|(j, _)| *j <= at);
        let near = frost[first..]
            .iter()
            .take_while(|(j, _)| *j - at <= MAX_COVER_SIBLINGS);
        let mut near = near.peekable();
        if near.peek().is_none() {
            return false;
        }
        let (p, e) = bounds(self.vm, child);
        if e.0 <= 0 || e.1 <= 0 {
            return false;
        }
        let (cx0, cy0) = (i64::from(p.0), i64::from(p.1));
        let (cx1, cy1) = (cx0 + i64::from(e.0), cy0 + i64::from(e.1));
        near.any(|(_, r)| r[0] <= cx0 && r[1] <= cy0 && cx1 <= r[2] && cy1 <= r[3])
    }

    /// `renderChildControls`.
    fn children(&mut self, id: ObjId, update: [i32; 4], offset: (i32, i32), depth: usize) {
        if depth >= MAX_TREE_DEPTH {
            return;
        }
        let update = if self.ext.clip_to_parent(self.vm, id) {
            update
        } else {
            self.viewport
        };
        // The extension's displacement of the children (a GuiScrollCtrl's own is in the offset it passes).
        let d = self.ext.shift(self.gui, id, self.now);
        let offset = if d == (0, 0) || is_a(self.vm, id, "GuiScrollCtrl") {
            offset
        } else {
            (offset.0.wrapping_add(d.0), offset.1.wrapping_add(d.1))
        };
        let kids = children(self.vm, id);
        // Which children are shown, read once: no script runs during the walk, so it holds for the whole loop. The shown ones
        // whose fill the extension replaces are then the only siblings a child is checked against (one pass, not a pass a child);
        // the first shown child has no shown sibling before it to cover.
        let shown: Vec<bool> = kids
            .iter()
            .map(|&c| {
                self.vm.objects().contains(c)
                    && is_a(self.vm, c, "GuiControl")
                    && flag(self.vm, c, b"visible")
            })
            .collect();
        let frost: Vec<(usize, [i64; 4])> = kids
            .iter()
            .enumerate()
            .filter(|(i, _)| shown[*i])
            .skip(1)
            .filter_map(|(i, &c)| self.frost_rect(c).map(|r| (i, r)))
            .collect();
        for (i, &child) in kids.iter().enumerate() {
            if self.out.draws.report.truncated {
                return;
            }
            if !shown[i] {
                continue;
            }
            if !frost.is_empty() && self.covered_by_frost(child, i, &frost) {
                self.out.draws.report.covered += 1;
                continue;
            }
            let (pos, ext) = bounds(self.vm, child);
            let at = (offset.0.wrapping_add(pos.0), offset.1.wrapping_add(pos.1));
            let mut clip = intersect(update, [at.0, at.1, ext.0, ext.1]);
            if clip[2] > 0 && clip[3] > 0 {
                if !self.ext.clip_to_parent(self.vm, child) {
                    clip = self.viewport;
                }
                self.set_clip(clip);
                let alpha = self.alpha;
                if let Some(o) = self.ext.opacity(self.vm, child) {
                    self.alpha = alpha * o;
                }
                self.control(child, at, clip, depth + 1);
                self.alpha = alpha;
            }
        }
    }

    /// One control's `onRender`, by its class.
    fn control(&mut self, id: ObjId, offset: (i32, i32), update: [i32; 4], depth: usize) {
        if self.out.draws.report.controls >= MAX_CONTROLS {
            self.out.draws.report.truncated = true;
            return;
        }
        self.out.draws.report.controls += 1;
        let ext = std::rc::Rc::clone(&self.ext);
        ext.before_control(&mut DrawCtx(self), id, offset, update);
        let class = self
            .vm
            .objects()
            .get(id)
            .map(|o| {
                String::from_utf8_lossy(self.vm.objects().classes.name(o.class())).into_owned()
            })
            .unwrap_or_default();
        match class.as_str() {
            _ if ext.draw_control(&mut DrawCtx(self), &class, id, offset, update, depth) => {}
            // GameTSCtrl::onRender: nothing at all (not its children, not the script's `onRender`) while `processCameraQuery`
            // fails, as it does with no connection to a server.
            "GameTSCtrl" => self.game_ts(id, offset, update, depth),
            "GuiControl" => self.gui_control(id, offset, update, depth),
            "GuiChunkedBitmapCtrl" => self.chunked(id, offset, update, depth),
            "GuiBitmapCtrl" => self.bitmap(id, offset, update, depth),
            "GuiFadeinBitmapCtrl" => self.fadein(id, offset),
            "GuiBitmapButtonCtrl" => self.bitmap_button(id, offset, update, depth),
            "GuiTextCtrl" => self.text_ctrl(id, offset, update, depth),
            "GuiButtonCtrl" => self.button(id, offset, update, depth),
            "GuiCheckBoxCtrl" | "GuiRadioCtrl" => self.check_box(id, offset, update, depth),
            "GuiTextEditCtrl" | "GuiConsoleEditCtrl" => self.text_edit(id, offset),
            "GuiScrollCtrl" => self.scroll(id, offset, update, depth),
            "GuiTextListCtrl" => {
                let kind = match crate::popup::part_of(self.gui, id) {
                    Some((_, crate::popup::Part::List)) => CellKind::PopUp,
                    _ => CellKind::TextList,
                };
                self.array(id, offset, update, kind)
            }
            "GuiConsole" => self.array(id, offset, update, CellKind::Console),
            "GuiPopUpTextListCtrl" => self.array(id, offset, update, CellKind::PopUp),
            "GuiWindowCtrl" => self.window(id, offset, update, depth),
            "GuiPopUpMenuCtrl" => self.popup(id, offset),
            "GuiSliderCtrl" => self.slider(id, offset, update, depth),
            "GuiProgressCtrl" => self.progress(id, offset, update, depth),
            "GuiMLTextCtrl" => self.ml_text(id, offset, update, depth),
            "GuiConsoleTextCtrl" => self.console_text(id, offset, update, depth),
            "GuiMLTextEditCtrl" => self.ml_text_edit(id, offset, update, depth),
            "GuiHealthBarHud" => self.health_bar(id, update),
            "GuiCrossHairHud" => self.cross_hair(id, offset, update, depth),
            "GuiShapeNameHud" => self.shape_names(id, update),
            "GuiGraphCtrl" => self.graph(id, offset, update),
            // No `onRender` of their own (GuiControl's).
            "GuiInputCtrl" | "GuiPopUpBackgroundCtrl" | "GuiMouseEventCtrl" => {
                self.gui_control(id, offset, update, depth)
            }
            _ => {
                *self.out.draws.report.unmodelled.entry(class).or_default() += 1;
                self.gui_control(id, offset, update, depth);
            }
        }
    }

    /// `GuiControl::onRender`.
    /// GameTSCtrl's `onRender`: with a camera (the client host says whether the connection's
    /// `processCameraQuery` has one: a control object), the world in the control's rectangle, then the children; without one,
    /// nothing at all.
    fn game_ts(&mut self, id: ObjId, offset: (i32, i32), update: [i32; 4], depth: usize) {
        if !self.gui.0.borrow().world_camera {
            return;
        }
        // GuiTSCtrl's `onRender` runs the script's `onRender` once a wake (the "has rendered" flag, as GuiControl's does),
        // after the camera query and before the world: a script that lays its controls out from `getRes()` there sees each new
        // size, since a change of screen mode resets the canvas and clears the flag.
        let first = self
            .gui
            .with_mut(id, |c| !std::mem::replace(&mut c.has_rendered, true))
            .unwrap_or(false);
        if first {
            self.out.callbacks.push(Callback::OnRender(id));
        }
        let ext = point(self.vm, id, b"extent");
        let control = [offset.0, offset.1, ext.0, ext.1];
        let rect = intersect(update, control);
        if self.ts_rects.len() < MAX_TS_RECTS {
            self.ts_rects.push((id, control, update));
        }
        if rect[2] > 0 && rect[3] > 0 {
            let at_batch = self.out.draws.batches.len();
            self.out.draws.worlds.push(WorldDraw {
                at_batch,
                rect,
                control,
            });
        }
        self.children(id, update, offset, depth);
    }

    fn gui_control(&mut self, id: ObjId, offset: (i32, i32), update: [i32; 4], depth: usize) {
        let first = self
            .gui
            .with_mut(id, |c| !std::mem::replace(&mut c.has_rendered, true))
            .unwrap_or(false);
        if first {
            self.out.callbacks.push(Callback::OnRender(id));
        }
        let ext = point(self.vm, id, b"extent");
        let rect = [offset.0, offset.1, ext.0, ext.1];
        if let Some(p) = profile_of(self.vm, id) {
            if flag(self.vm, p, b"opaque") && !self.blurred(p) {
                let color = color_field(self.vm, p, b"fillColor");
                self.fill(rect, color);
            }
            self.render_border(p, rect);
        }
        self.children(id, update, offset, depth);
    }

    /// The rectangle a bitmap control that keeps its bitmap's aspect ([`crate::GuiExtension::bitmap_fit`]) draws a `bw` x
    /// `bh` bitmap in, over a control at `offset` of extent `ext`: the bitmap's aspect against the control's picks a side to
    /// fill; `cover` swaps the choice (cover instead of fit); the other side is centred unless aligned left or top. `ftol`
    /// truncates.
    fn aspect_rect(
        offset: (i32, i32),
        ext: (i32, i32),
        bw: u32,
        bh: u32,
        overflow: bool,
        align_left: bool,
        align_top: bool,
    ) -> [i32; 4] {
        let (w, h) = (ext.0, ext.1);
        let ctrl = w as f32 / h as f32;
        let a = bw as f32 / bh as f32;
        // Fill the height (width from the aspect) when the bitmap is narrower than the control, else fill the width; overflow swaps.
        // A NaN aspect (not ordered) fills the height either way.
        let fill_height = if overflow {
            ctrl.partial_cmp(&a) != Some(std::cmp::Ordering::Greater)
        } else {
            !matches!(
                a.partial_cmp(&ctrl),
                Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
            )
        };
        if fill_height {
            let width = (f64::from(h) * f64::from(a)) as i32;
            let x = if align_left {
                0
            } else {
                (f64::from(w.wrapping_sub(width) as f32) * 0.5) as i32
            };
            [offset.0.wrapping_add(x), offset.1, width, h]
        } else {
            let height = (f64::from(w) / f64::from(a)) as i32;
            let y = if align_top {
                0
            } else {
                (f64::from(h.wrapping_sub(height) as f32) * 0.5) as i32
            };
            [offset.0, offset.1.wrapping_add(y), w, height]
        }
    }

    /// GuiBitmapCtrl::onRender.
    fn bitmap(&mut self, id: ObjId, offset: (i32, i32), update: [i32; 4], depth: usize) {
        let name = text_field(self.vm, id, b"bitmap");
        self.bitmap_named(id, offset, update, depth, Some(&name));
    }

    /// GuiBitmapCtrl::onRender with the texture of `name` (None: no texture).
    fn bitmap_named(
        &mut self,
        id: ObjId,
        offset: (i32, i32),
        update: [i32; 4],
        depth: usize,
        name: Option<&[u8]>,
    ) {
        let vm = self.vm;
        let color = color_field(vm, id, b"mColor");
        let multiply = flag(vm, id, b"mMultiply");
        // Nothing at all (not the children) when the colour hides it.
        let hidden = if multiply {
            color == [0; 4]
        } else {
            color[3] == 0
        };
        if hidden {
            return;
        }
        let (_, ext) = bounds(vm, id);
        let tex = name.and_then(|n| self.texture(n, true));
        if let Some(t) = &tex {
            let (bw, bh) = t.bitmap_size;
            let src = [0, 0, bw as i32, bh as i32];
            if flag(vm, id, b"wrap") {
                // Tiles of the bitmap's size from the start offset (setValue, kept as 0 by the model), cut by the clip.
                let xs = (ext.0 as f32 / bw.max(1) as f32 + 1.0) as i32;
                let ys = (ext.1 as f32 / bh.max(1) as f32 + 1.0) as i32;
                'tiles: for ty in 0..ys.clamp(0, 4096) {
                    for tx in 0..xs.clamp(0, 4096) {
                        if self.out.draws.full() {
                            self.out.draws.report.truncated = true;
                            break 'tiles;
                        }
                        let dst = [
                            offset.0 + bw as i32 * tx,
                            offset.1 + bh as i32 * ty,
                            bw as i32,
                            bh as i32,
                        ];
                        self.stretch(t, dst, src, color, multiply);
                    }
                }
            } else {
                let dst = if let Some((cover, left, top)) = self.ext.bitmap_fit(vm, id) {
                    Self::aspect_rect(offset, ext, bw, bh, cover, left, top)
                } else {
                    [offset.0, offset.1, ext.0, ext.1]
                };
                self.stretch(t, dst, src, color, multiply);
            }
        }
        let profile = profile_of(vm, id);
        let bordered = profile.is_some_and(|p| int_field(vm, p, b"border") != 0);
        if bordered || tex.is_none() {
            let c = profile.map_or([0, 0, 0, 255], |p| color_field(vm, p, b"borderColor"));
            self.outline(offset, (offset.0 + ext.0 - 1, offset.1 + ext.1 - 1), c);
        }
        self.children(id, update, offset, depth);
    }

    /// GuiFadeinBitmapCtrl::onRender: no children, no border. The phases from the wake (or `reset`, [`crate::Fade`]): the
    /// bitmap's alpha rises over `fadeinTime`, holds over `waitTime`, falls over `fadeoutTime`, then is 0 (255 when `fadeoutTime` is
    /// 0). The alpha steps are truncated toward zero. Its `onWait` and `onDone` are run by
    /// [`crate::Gui::frame`] (the render pass's scripts), not here.
    fn fadein(&mut self, id: ObjId, offset: (i32, i32)) {
        let vm = self.vm;
        let wake = self.gui.with(id, |c| c.fade.wake).unwrap_or(0);
        let elapsed = self.now.wrapping_sub(wake);
        let fin = int_field(vm, id, b"fadeinTime") as u32;
        let wait = int_field(vm, id, b"waitTime") as u32;
        let fout = int_field(vm, id, b"fadeoutTime") as u32;
        let cover: u8 = if elapsed < fin {
            (255.0f32 - (elapsed as f32 / fin as f32) * 255.0) as i64 as u8
        } else if elapsed < wait.wrapping_add(fin) {
            0
        } else if elapsed < fout.wrapping_add(wait).wrapping_add(fin) {
            let t = elapsed.wrapping_sub(wait).wrapping_sub(fin);
            ((t as f32 * 255.0) / fout as f32) as i64 as u8
        } else if fout != 0 {
            0xff
        } else {
            0
        };
        let Some(t) = self.texture(&text_field(vm, id, b"bitmap"), true) else {
            return;
        };
        let color = [255, 255, 255, !cover];
        let (_, ext) = bounds(vm, id);
        let (bw, bh) = t.bitmap_size;
        let dst = if let Some((cover, left, top)) = self.ext.bitmap_fit(vm, id) {
            Self::aspect_rect(offset, ext, bw, bh, cover, left, top)
        } else {
            [offset.0, offset.1, ext.0, ext.1]
        };
        self.stretch(&t, dst, [0, 0, bw as i32, bh as i32], color, false);
    }

    /// GuiChunkedBitmapCtrl::onRender: without a bitmap it is a GuiControl; with one, the bitmap in 256 x 256 chunks each
    /// stretched to its share of the control (`renderRegion`: the last chunk of a row or column ends at the control's edge),
    /// or tiled at the bitmap's size (`tile`).
    fn chunked(&mut self, id: ObjId, offset: (i32, i32), update: [i32; 4], depth: usize) {
        let name = text_field(self.vm, id, b"bitmap");
        let found = (!name.is_empty())
            .then(|| self.chunk_bitmap(&name))
            .flatten();
        // The chunks' textures are keyed by the resolved file, so the names of one file share them too.
        let Some((name, bitmap)) = found else {
            self.gui_control(id, offset, update, depth);
            return;
        };
        let (_, ext) = bounds(self.vm, id);
        if flag(self.vm, id, b"tile") {
            let (bw, bh) = (bitmap.width.max(1) as i32, bitmap.height.max(1) as i32);
            let mut y = 0;
            'tiles: while y < ext.1 && y < 1 << 16 {
                let mut x = 0;
                while x < ext.0 && x < 1 << 16 {
                    if self.out.draws.full() {
                        self.out.draws.report.truncated = true;
                        break 'tiles;
                    }
                    self.chunk_region(&name, &bitmap, (offset.0 + x, offset.1 + y), (bw, bh));
                    x += bw;
                }
                y += bh;
            }
        } else {
            self.chunk_region(&name, &bitmap, offset, ext);
        }
        self.children(id, update, offset, depth);
    }

    /// The bitmap a GuiChunkedBitmapCtrl names and the key of the file it was decoded from ([`ChunkCache`]).
    fn chunk_bitmap(&mut self, name: &[u8]) -> Option<(Vec<u8>, Arc<torque_images::Bitmap>)> {
        let name = String::from_utf8_lossy(name).into_owned();
        let key = name.to_ascii_lowercase();
        let file = match self.cache.chunked.names.get(&key) {
            Some(file) => file.clone(),
            None => {
                if self.cache.chunked.names.len() >= MAX_SHEET_TEXTURES {
                    return None;
                }
                let mut file = None;
                for path in crate::textures::candidates(&name) {
                    if torque_images::Kind::of_name(&path).is_some() && self.read(&path).is_some() {
                        file = Some(path.to_ascii_lowercase());
                        break;
                    }
                }
                self.cache.chunked.names.insert(key, file.clone());
                file
            }
        };
        let Some(file) = file else {
            self.out.draws.report.no_texture += 1;
            return None;
        };
        let bitmap = match self.cache.chunked.touch(&file) {
            Some(b) => b,
            None if !self.cache.chunked.may_decode(&file) => None,
            None => {
                self.cache.chunked.decodes += 1;
                let decoded = torque_images::Kind::of_name(&file)
                    .and_then(|kind| torque_images::decode(kind, &self.read(&file)?).ok())
                    .map(Arc::new);
                self.cache.chunked.insert(file.clone(), decoded)
            }
        };
        match bitmap {
            Some(b) => Some((file.into_bytes(), b)),
            None => {
                self.out.draws.report.no_texture += 1;
                None
            }
        }
    }

    fn chunk_region(
        &mut self,
        name: &[u8],
        bitmap: &torque_images::Bitmap,
        at: (i32, i32),
        ext: (i32, i32),
    ) {
        const CHUNK: u32 = 256;
        let (cols, rows) = (bitmap.width.div_ceil(CHUNK), bitmap.height.div_ceil(CHUNK));
        if cols == 0 || rows == 0 || cols * rows > 1024 {
            return;
        }
        let sx = ext.0 as f32 / bitmap.width as f32;
        let sy = ext.1 as f32 / bitmap.height as f32;
        let bpp = bitmap.format.bytes_per_pixel();
        for i in 0..cols {
            for j in 0..rows {
                if self.out.draws.full() {
                    self.out.draws.report.truncated = true;
                    return;
                }
                let (cw, ch) = (
                    (bitmap.width - i * CHUNK).min(CHUNK),
                    (bitmap.height - j * CHUNK).min(CHUNK),
                );
                let key = (
                    format!(
                        "{}#chunk",
                        String::from_utf8_lossy(name).to_ascii_lowercase()
                    ),
                    (j * cols + i) as i32,
                    0usize,
                );
                let tex = match self.cache.sheets.get(&key) {
                    Some(t) => t.clone(),
                    None => {
                        if self.cache.sheets.len() >= MAX_SHEET_TEXTURES
                            || self.cache.sheet_bytes >= MAX_SHEET_BYTES
                        {
                            self.out.draws.report.truncated = true;
                            continue;
                        }
                        let mut bits = Vec::with_capacity((cw * ch) as usize * bpp);
                        for r in 0..ch {
                            let row = ((j * CHUNK + r) * bitmap.width + i * CHUNK) as usize * bpp;
                            bits.extend_from_slice(&bitmap.bits[row..row + cw as usize * bpp]);
                        }
                        let b = torque_images::Bitmap {
                            width: cw,
                            height: ch,
                            format: bitmap.format,
                            bits,
                        };
                        let Some(t) = texture_of("chunk", &b, false, true) else {
                            continue;
                        };
                        self.cache.sheet_bytes += t.rgba.len();
                        self.cache.sheets.insert(key, t.clone());
                        t
                    }
                };
                let x0 = at.0 + (((i * CHUNK) as f32) * sx) as i32;
                let y0 = at.1 + (((j * CHUNK) as f32) * sy) as i32;
                let x1 = if i == cols - 1 {
                    at.0 + ext.0
                } else {
                    at.0 + (((i * CHUNK + cw) as f32) * sx) as i32
                };
                let y1 = if j == rows - 1 {
                    at.1 + ext.1
                } else {
                    at.1 + (((j * CHUNK + ch) as f32) * sy) as i32
                };
                self.stretch(
                    &tex,
                    [x0, y0, x1 - x0, y1 - y0],
                    [0, 0, cw as i32, ch as i32],
                    [255; 4],
                    false,
                );
            }
        }
    }

    /// GuiBitmapButtonCtrl::onRender: nothing at all without the normal bitmap (`<bitmap>_n.png`); else the state's
    /// bitmap: inactive -> `_i` (else `_n`); depressed or on -> `_d` (else `_h`, else `_n`); mouse over -> `_h` (else `_n`); `_n`.
    fn bitmap_button(&mut self, id: ObjId, offset: (i32, i32), update: [i32; 4], depth: usize) {
        let vm = self.vm;
        let base = text_field(vm, id, b"bitmap");
        if base.is_empty() {
            return;
        }
        let named = |suffix: &str| {
            let mut n = base.clone();
            n.extend_from_slice(suffix.as_bytes());
            n
        };
        let Some(normal) = self.texture(&named("_n.png"), true) else {
            return;
        };
        // `mActive`, what `setActive` sets.
        let active = crate::input::active(self.gui, id);
        let (over, depressed, on) = self
            .gui
            .with(id, |c| (c.mouse_over, c.depressed, c.state_on))
            .unwrap_or((false, false, false));
        let tex = if !active {
            self.texture(&named("_i.png"), true).unwrap_or(normal)
        } else if depressed || on {
            match self.texture(&named("_d.png"), true) {
                Some(t) => t,
                None => self.texture(&named("_h.png"), true).unwrap_or(normal),
            }
        } else if over {
            self.texture(&named("_h.png"), true).unwrap_or(normal)
        } else {
            normal
        };
        // renderButton.
        let color = color_field(vm, id, b"mColor");
        let (_, ext) = bounds(vm, id);
        let (bw, bh) = tex.bitmap_size;
        let dst = if let Some((cover, left, top)) = self.ext.bitmap_fit(vm, id) {
            Self::aspect_rect(offset, ext, bw, bh, cover, left, top)
        } else {
            [offset.0, offset.1, ext.0, ext.1]
        };
        self.stretch(&tex, dst, [0, 0, bw as i32, bh as i32], color, false);
        let text = text_field(vm, id, b"text");
        let at = if depressed {
            (offset.0 + 1, offset.1 + 1)
        } else {
            offset
        };
        self.justified_text(id, &text, at, ext);
        self.children(id, update, offset, depth);
    }

    /// GuiTextCtrl::onRender: the text (`mText`), then the children.
    fn text_ctrl(&mut self, id: ObjId, offset: (i32, i32), update: [i32; 4], depth: usize) {
        let text = self
            .gui
            .with(id, |c| c.text.clone())
            .flatten()
            .unwrap_or_else(|| crate::classes::initial_text(self.vm, id));
        let (_, ext) = bounds(self.vm, id);
        // What the draw returned is what `getPixelWidth` answers.
        if let Some(width) = self.justified_text(id, &text, offset, ext) {
            self.gui.with_mut(id, |c| c.pixel_width = width);
        }
        self.children(id, update, offset, depth);
    }

    /// `GuiControl::renderJustifiedText`: the profile's font; the colour `fontColor` (`fontColorNA` when the control is
    /// not active or not enabled: the model keeps both in `enabled`); the x by `justify` (left; right: `w - width`; centre:
    /// `(w - width) / 2`; 0 when the text is wider), the y `(h - font height) >> 1` as an unsigned shift; with an outline
    /// colour ([`crate::GuiExtension::text_outline`]) the text first drawn nine times in it at every offset of -1..1 (the x
    /// moved 1 inwards for left and right), then once with
    /// the profile's ten colours as the colour table.
    /// Returns the pen's advance of the text's draw (`dglDrawTextN`'s result), or `None` when nothing was drawn (no profile, no font).
    fn justified_text(
        &mut self,
        id: ObjId,
        text: &[u8],
        at: (i32, i32),
        ext: (i32, i32),
    ) -> Option<i32> {
        self.justified_text_in(id, text, at, ext, None)
    }

    /// [`Walk::justified_text`] in `color` instead of the profile's when given (a rounded box's highlight text).
    fn justified_text_in(
        &mut self,
        id: ObjId,
        text: &[u8],
        at: (i32, i32),
        ext: (i32, i32),
        color: Option<[u8; 4]>,
    ) -> Option<i32> {
        let vm = self.vm;
        let profile = profile_of(vm, id)?;
        let face = String::from_utf8_lossy(&text_field(vm, profile, b"fontType")).into_owned();
        let size = int_field(vm, profile, b"fontSize");
        let read = |p: &str| match vm.file_host().read(p.as_bytes()) {
            FileRead::Found(b) => Some(b),
            _ => None,
        };
        let Some(font) = self.cache.fonts.font(&face, size, &read) else {
            self.out.draws.report.no_font += 1;
            if self.out.draws.report.missing_fonts.len() < 64 {
                self.out
                    .draws
                    .report
                    .missing_fonts
                    .insert(format!("{face}_{size}"));
            }
            return None;
        };
        let width = font.str_width(text);
        let table: Vec<[u8; 4]> = (0..10)
            .map(|i| {
                color_i(
                    &vm.objects()
                        .field(profile, b"fontColors", i.to_string().as_bytes())
                        .unwrap_or_default(),
                )
            })
            .collect();
        // `mActive` picks `fontColor` or `fontColorNA`, and a control that is not `enabled` is NA either way.
        let active = crate::input::active(self.gui, id) && flag(vm, id, b"enabled");
        let color = color.unwrap_or(if active { table[0] } else { table[2] });
        let justify = text_field(vm, profile, b"justify").to_ascii_lowercase();
        let justify = match justify.as_slice() {
            b"right" | b"1" => 1,
            b"center" | b"2" => 2,
            _ => 0,
        };
        let mut xoff = match justify {
            1 => ext.0.wrapping_sub(width),
            2 => ext.0.wrapping_sub(width) / 2,
            _ => 0,
        };
        if ext.0 < width {
            xoff = 0;
        }
        let yoff = ((ext.1.wrapping_sub(font.height as i32)) as u32 >> 1) as i32;
        let mut x = at.0;
        if let Some(outline) = self.ext.text_outline(vm, profile) {
            match justify {
                0 => x += 1,
                1 => x -= 1,
                _ => {}
            }
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let mut colors = TextColors::new(outline, self.cache.saved_color);
                    self.glyphs(
                        &font,
                        &face,
                        size,
                        (
                            x.wrapping_add(dx).wrapping_add(xoff),
                            at.1.wrapping_add(yoff).wrapping_add(dy),
                        ),
                        text,
                        &mut colors,
                        None,
                    );
                }
            }
        }
        let mut colors = TextColors::new(color, self.cache.saved_color);
        Some(self.glyphs(
            &font,
            &face,
            size,
            // A text taller than its control: the unsigned shift gives a huge offset, and its sum wraps (32-bit).
            (x.wrapping_add(xoff), at.1.wrapping_add(yoff)),
            text,
            &mut colors,
            Some(&table),
        ))
    }

    /// `dglDrawTextN`: the glyph quads, each sheet a nearest-filtered texture.
    #[allow(clippy::too_many_arguments)]
    fn glyphs(
        &mut self,
        font: &Arc<torque_fonts::Font>,
        face: &str,
        size: i32,
        at: (i32, i32),
        text: &[u8],
        colors: &mut TextColors,
        table: Option<&[[u8; 4]]>,
    ) -> i32 {
        // At most one quad a byte: the text is cut to what the budget has left.
        let left = MAX_QUADS.saturating_sub(self.out.draws.pushed);
        if left == 0 {
            self.out.draws.report.truncated = true;
            return 0;
        }
        let text = &text[..text.len().min(left)];
        let layout = font.layout(at, text, colors, table, 9);
        let advance = layout.advance;
        self.cache.saved_color = colors.saved;
        self.out.draws.report.invalid_glyphs += layout.invalid;
        for q in layout.quads {
            if self.out.draws.full() {
                self.out.draws.report.truncated = true;
                return advance;
            }
            let key = (face.to_ascii_lowercase(), size, q.sheet);
            let tex = match self.cache.sheets.get(&key) {
                Some(t) => t.clone(),
                None => {
                    if self.cache.sheets.len() >= MAX_SHEET_TEXTURES
                        || self.cache.sheet_bytes >= MAX_SHEET_BYTES
                    {
                        self.out.draws.report.truncated = true;
                        continue;
                    }
                    let Some(t) = font.sheets.get(q.sheet).and_then(|s| {
                        texture_of(&format!("font:{face}_{size}#{}", q.sheet), s, true, false)
                    }) else {
                        continue;
                    };
                    self.cache.sheet_bytes += t.rgba.len();
                    self.cache.sheets.insert(key, t.clone());
                    t
                }
            };
            let quad = GuiQuad {
                rect: q.rect.map(|v| v as f32),
                uv: q.uv,
                color: self.faded(q.color),
            };
            self.out
                .draws
                .push(self.clip, Some(&tex), GuiBlend::Alpha, quad);
        }
        advance
    }

    /// The cursor (the end of `renderFrame`).
    fn cursor(&mut self) {
        let (on, shown, cursor, pos) = {
            let s = self.gui.0.borrow();
            (s.cursor_on, s.show_cursor, s.cursor, s.cursor_pos)
        };
        let Some(cursor) = cursor.filter(|c| on && shown && self.vm.objects().contains(*c)) else {
            return;
        };
        let hot = point(self.vm, cursor, b"hotSpot");
        let Some(t) = self.texture(&text_field(self.vm, cursor, b"bitmapName"), false) else {
            return;
        };
        let (bw, bh) = (t.bitmap_size.0 as i32, t.bitmap_size.1 as i32);
        self.stretch(
            &t,
            [pos.0 - hot.0, pos.1 - hot.1, bw, bh],
            [0, 0, bw, bh],
            [255; 4],
            false,
        );
    }
}

/// `GuiCanvas::renderFrame` as a draw list (see the module docs). Runs no script; the callbacks it asks for are in the frame.
pub fn draw_canvas(vm: &Vm, gui: &Gui, cache: &mut DrawCache) -> Frame {
    cache.chunked.begin_frame();
    cache.textures.begin_frame();
    let (canvas, size) = {
        let s = gui.0.borrow();
        (s.canvas, s.canvas_size)
    };
    let now = vm.sim_time();
    let (w, h) = (size.0.max(0), size.1.max(0));
    let viewport = [0, 0, w, h];
    let ext = gui.ext();
    ext.begin_draw(gui, cache.fonts.as_mut());
    let mut walk = Walk {
        vm,
        gui,
        cache,
        out: Frame::default(),
        viewport,
        clip: viewport,
        now,
        ts_rects: Vec::new(),
        ext,
        alpha: 1.0,
        blurred: Default::default(),
    };
    walk.out.draws.size = (w as u32, h as u32);
    if let Some(canvas) = canvas.filter(|c| vm.objects().contains(*c)) {
        for child in children(vm, canvas) {
            if !vm.objects().contains(child) || !is_a(vm, child, "GuiControl") {
                continue;
            }
            walk.set_clip(viewport);
            let pos = point(vm, child, b"position");
            walk.control(child, pos, viewport, 1);
        }
        // The host's overlays, over the canvas's children and under the cursor.
        for o in gui.overlays(vm) {
            if !is_a(vm, o, "GuiControl") || !gui.is_awake(o) {
                continue;
            }
            walk.set_clip(viewport);
            let pos = point(vm, o, b"position");
            walk.control(o, pos, viewport, 1);
        }
        walk.set_clip(viewport);
        walk.cursor();
    }
    walk.out
}

/// Runs a frame's callbacks (no borrow held): `onRender` on each control that still exists.
pub fn run_callbacks(vm: &mut Vm, callbacks: &[Callback]) -> Result<(), VmError> {
    for c in callbacks {
        let (id, name): (ObjId, &[u8]) = match *c {
            Callback::OnRender(id) => (id, b"onRender"),
        };
        if vm.objects().contains(id) {
            vm.call_method(id, name, &[])?;
        }
    }
    Ok(())
}

/// What an extension's draw ([`crate::GuiExtension::draw_control`], [`crate::GuiExtension::before_control`]) reaches of the
/// walk: the engine's primitives, at the walk's clip and opacity.
pub struct DrawCtx<'w, 'a>(&'w mut Walk<'a>);

impl<'a> DrawCtx<'_, 'a> {
    pub fn vm(&self) -> &'a Vm {
        self.0.vm
    }

    pub fn gui(&self) -> &'a Gui {
        self.0.gui
    }

    /// The clock the frame is drawn at (`Vm::sim_time`).
    pub fn now(&self) -> u32 {
        self.0.now
    }

    /// The opacity the current subtree draws at.
    pub fn alpha(&self) -> f32 {
        self.0.alpha
    }

    /// The batches drawn so far (an extra pushed now goes before the next one).
    pub fn batches(&self) -> usize {
        self.0.out.draws.batches.len()
    }

    /// The extras asked for so far this frame.
    pub fn extras(&self) -> &[ExtraDraw] {
        &self.0.out.draws.extras
    }

    /// Asks for `item` before the next batch.
    pub fn push_extra(&mut self, item: std::rc::Rc<dyn ExtraItem>) {
        let at_batch = self.batches();
        self.0.out.draws.extras.push(ExtraDraw { at_batch, item });
    }

    pub fn report(&mut self) -> &mut DrawReport {
        &mut self.0.out.draws.report
    }

    /// The control's profile.
    pub fn profile(&self, id: ObjId) -> Option<ObjId> {
        profile_of(self.0.vm, id)
    }

    /// `dglDrawRectFill` in the walk's clip.
    pub fn fill(&mut self, r: [i32; 4], color: [u8; 4]) {
        self.0.fill(r, color);
    }

    /// `dglDrawRect` through the corner pixels `a` and `b`.
    pub fn outline(&mut self, a: (i32, i32), b: (i32, i32), color: [u8; 4]) {
        self.0.outline(a, b, color);
    }

    /// A rounded box ([`BOX_RADIUS`]): `fill` over `r` (x, y, width, height), then a one-pixel `border` round it (none when
    /// its alpha is 0).
    pub fn rounded_box(&mut self, r: [i32; 4], fill: [u8; 4], border: [u8; 4]) {
        self.0.rounded_box(r, fill, border);
    }

    /// GuiBitmapCtrl's `onRender` with the bitmap `name` (none: nothing but the children).
    pub fn bitmap_named(
        &mut self,
        id: ObjId,
        offset: (i32, i32),
        update: [i32; 4],
        depth: usize,
        name: Option<&[u8]>,
    ) {
        self.0.bitmap_named(id, offset, update, depth, name);
    }

    /// GuiBitmapCtrl's `onRender` with the control's own `bitmap`.
    pub fn bitmap(&mut self, id: ObjId, offset: (i32, i32), update: [i32; 4], depth: usize) {
        self.0.bitmap(id, offset, update, depth);
    }

    /// The control's children (`renderChildControls`).
    pub fn children(&mut self, id: ObjId, update: [i32; 4], offset: (i32, i32), depth: usize) {
        self.0.children(id, update, offset, depth);
    }

    /// The control's script `onRender`, asked for after the frame the first time it draws.
    pub fn first_render(&mut self, id: ObjId) {
        let first = self
            .0
            .gui
            .with_mut(id, |c| !std::mem::replace(&mut c.has_rendered, true))
            .unwrap_or(false);
        if first {
            self.0.out.callbacks.push(Callback::OnRender(id));
        }
    }
}

/// A rectangle (x, y, width, height) cut by another.
pub fn intersect_rect(u: [i32; 4], c: [i32; 4]) -> [i32; 4] {
    intersect(u, c)
}
