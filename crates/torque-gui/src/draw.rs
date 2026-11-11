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
//!   Torque-rs adds two things TGE has not: a control's `opacity` (0 to 1, default 1) dims the control and everything in it (each
//!   quad's alpha times it, so the children's own opacities multiply), and a profile's `cornerRadius` (pixels, default 0) draws
//!   its fill and style 1 border with that radius, anti-aliased, instead of the square ones (see `rounded`).
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
//! **A client-script layer is drawn once while it stays the same.** The server layer's batches are kept in the cache
//! ([`DrawCache`]) with the key they were drawn under (the stamp of the plugin controls' state, the extension's
//! [`crate::GuiExtension::layer_stamp`], the canvas, the chip's column, hover, focus, the caret's blink, the pointer under a
//! tooltip, the fonts' revision), and a frame with the same key pushes them again, with the counts they added to the report; see
//! [`crate::client_script`], **Kept draws**.
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

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(not(target_family = "wasm"))]
use std::sync::mpsc;
use std::time::Duration;

use torque_client_script_format::ui::{MAX_IMAGE_BYTES, MAX_IMAGE_SIDE};
use torque_fonts::{FontUse, TextColors};
use torque_ts_vm::host::FileRead;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};
use web_time::Instant;

use crate::control::{bounds, children, flag, is_a, point};
use crate::textures::{GuiTexture, GuiTextures, texture_of};
use crate::{Gui, MAX_TREE_DEPTH, atoi};

mod hud;
mod menus;
#[cfg(test)]
mod pictures_tests;
mod rounded;
pub use hud::{color_byte, name_opacity, project};
use menus::CellKind;
pub use rounded::{
    BOX_RADIUS, BoxState, DEFAULT_FILL, MAX_PROFILE_RADIUS, PRESSED_SHADE, box_colors, box_texture,
};

/// The most quads one frame draws (a hostile GUI with a million controls draws to here).
pub const MAX_QUADS: usize = 1 << 20;
/// The most faces and sizes a report names as having no font.
pub const MAX_MISSING_FONTS: usize = 64;
/// The most quads of the server layer a cache keeps to draw again.
pub const MAX_KEPT_QUADS: usize = 1 << 16;
/// The most controls one frame visits.
pub const MAX_CONTROLS: u32 = 1 << 18;
/// The most GameTSCtrls one frame keeps the projection of.
pub const MAX_TS_RECTS: usize = 64;

/// The most texel bytes of client-script realms' images one frame decodes (a 1024 x 1024 image's), but for one larger image,
/// which decodes alone in its frame: the rest wait for the next frames.
pub const MAX_REALM_DECODE_FRAME: usize = 4 << 20;
/// The texel bytes a realm's images may decode at once: enough to show every image it may hold (the format's decoded cap).
pub const REALM_DECODE_BURST: usize = MAX_IMAGE_BYTES;
/// The texel bytes a second a realm's images decode once that burst is spent: a realm that keeps changing its images to new
/// ones waits for them.
pub const REALM_DECODE_PER_SECOND: usize = 4 << 20;
/// What a realm image costs to hold beside its texels and its file: its texture's own data, and a renderer's.
pub const REALM_TEXTURE_OVERHEAD: usize = 4 << 10;
/// The most realm images one cache decodes on threads at once; the rest wait for them.
pub const MAX_REALM_DECODES_IN_FLIGHT: usize = 2;
/// The texel bytes of realm images a frame decodes on its own thread (small ones: a 128 x 128 image's, a fraction of a
/// millisecond), so that an icon shows in the frame it first appears; larger images decode on threads.
pub const REALM_DECODE_HERE_FRAME: usize = 64 << 10;

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
    /// Controls of a client-script realm drawn this frame ([`crate::client_script`]).
    pub realm_controls: u32,
    /// Texts measured by plugin controls' draws ([`DrawCtx::text_size`]).
    pub text_measures: u32,
    /// Images of a client-script realm whose decode began this frame.
    pub realm_decodes: u32,
    /// Of those, the ones decoded on the frame's own thread (the web's, or a thread that could not be made): their time is
    /// the frame's.
    pub realm_decoded_here: u32,
    /// Images of a client-script realm not drawn this frame because the decode allowances could not take them yet.
    pub realm_waiting: u32,
    /// Whether the client-script chip was drawn.
    pub chip: bool,
    /// The layers in the order they were drawn: the content, the server layer, each dialog, the engine layer, the chip.
    pub layers: Vec<crate::client_script::LayerMark>,
}

/// Where the next quad of a frame goes: the number of batches, and the quads of the last.
#[derive(Clone, Copy, Debug)]
struct QuadMark {
    batches: usize,
    quads: usize,
}

/// The parts of `q` outside `hole` (x, y, width, height), each with its texture coordinates, pushed to `out`.
fn cut_quad(q: GuiQuad, hole: [i32; 4], out: &mut Vec<GuiQuad>) {
    let [hx0, hy0] = [hole[0] as f32, hole[1] as f32];
    let [hx1, hy1] = [hx0 + hole[2] as f32, hy0 + hole[3] as f32];
    let [x0, y0, x1, y1] = q.rect;
    if x0.max(x1) <= hx0 || x0.min(x1) >= hx1 || y0.max(y1) <= hy0 || y0.min(y1) >= hy1 {
        out.push(q);
        return;
    }
    // A quad drawn mirrored or empty that meets the hole is dropped whole.
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let piece = |r: [f32; 4]| {
        let u = |x: f32| q.uv[0] + (q.uv[2] - q.uv[0]) * (x - x0) / (x1 - x0);
        let v = |y: f32| q.uv[1] + (q.uv[3] - q.uv[1]) * (y - y0) / (y1 - y0);
        GuiQuad {
            rect: r,
            uv: [u(r[0]), v(r[1]), u(r[2]), v(r[3])],
            color: q.color,
        }
    };
    if y0 < hy0 {
        out.push(piece([x0, y0, x1, hy0]));
    }
    if y1 > hy1 {
        out.push(piece([x0, hy1, x1, y1]));
    }
    let (my0, my1) = (y0.max(hy0), y1.min(hy1));
    if x0 < hx0 {
        out.push(piece([x0, my0, hx0, my1]));
    }
    if x1 > hx1 {
        out.push(piece([hx1, my0, x1, my1]));
    }
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
    /// Pushes a batch that was drawn before, as its quads would have been pushed one by one: into the last batch when it has the
    /// same state, else as a batch of its own. `counted` is what the budget counts for it ([`GuiDraws::pushed`]).
    fn push_again(&mut self, batch: &GuiBatch, counted: usize) {
        self.pushed += counted;
        if !self.after_view()
            && let Some(last) = self.batches.last_mut()
            && last.clip == batch.clip
            && last.blend == batch.blend
            && last.texture.as_ref().map(|t| t.id) == batch.texture.as_ref().map(|t| t.id)
        {
            last.quads.extend_from_slice(&batch.quads);
            return;
        }
        self.batches.push(batch.clone());
    }

    /// Whether a view's or a world's frame goes between the last batch and the next quad: that quad starts a batch of its own.
    fn after_view(&self) -> bool {
        self.extras
            .last()
            .is_some_and(|v| v.at_batch == self.batches.len())
            || self
                .worlds
                .last()
                .is_some_and(|w| w.at_batch == self.batches.len())
    }

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
        if !self.after_view()
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

/// An image a client-script realm shows: the bytes its server's store gave for one of its paths, under an id no other image
/// has. It is drawn from those bytes alone, its texture kept apart from the game's: it is never looked up by name, so a
/// realm's path cannot reach the game's art. Its header is read when it is made (a side past [`MAX_IMAGE_SIDE`] is refused);
/// it is decoded once, when first drawn (natively on a thread of its own, so no frame waits for it), and its texture is shared
/// by every copy: the realm keeps it while it fits in the realm's decoded cap ([`MAX_IMAGE_BYTES`]).
#[derive(Clone)]
pub(crate) struct RealmImage {
    id: u64,
    kind: torque_images::Kind,
    bytes: Arc<[u8]>,
    size: (u32, u32),
    /// Where its texture is.
    texture: Rc<RefCell<Decode>>,
}

/// Where a realm image's texture is.
#[derive(Default)]
enum Decode {
    /// Not asked for yet.
    #[default]
    Idle,
    /// Decoding on a thread of its own.
    #[cfg(not(target_family = "wasm"))]
    Pending(mpsc::Receiver<Option<Arc<GuiTexture>>>),
    /// Decoded (none: it did not decode).
    Done(Option<Arc<GuiTexture>>),
}

/// The texture of an image of `kind` and `size` from `bytes`, named by `id`: none when the bytes do not decode to that size.
fn decode_realm_image(
    kind: torque_images::Kind,
    bytes: &[u8],
    size: (u32, u32),
    id: u64,
) -> Option<Arc<GuiTexture>> {
    torque_images::decode(kind, bytes)
        .ok()
        .filter(|b| (b.width, b.height) == size)
        .and_then(|b| texture_of(&format!("realm:{id}"), &b, false, true))
}

impl RealmImage {
    /// The kind and size of `bytes`, given for `path`: None when the path's extension names no image kind, the header does
    /// not read, or a side is 0 or past [`MAX_IMAGE_SIDE`].
    pub(crate) fn header(path: &str, bytes: &[u8]) -> Option<(torque_images::Kind, (u32, u32))> {
        let kind = match path.rsplit_once('.')?.1 {
            "png" => torque_images::Kind::Png,
            "jpg" | "jpeg" => torque_images::Kind::Jpeg,
            _ => return None,
        };
        let (w, h) = torque_images::size_of(kind, bytes)?;
        let fits = |side: u32| (1..=MAX_IMAGE_SIDE).contains(&side);
        (fits(w) && fits(h)).then_some((kind, (w, h)))
    }

    /// The bytes an image of `size` takes decoded.
    pub(crate) fn decoded_bytes_of(size: (u32, u32)) -> usize {
        4 * size.0 as usize * size.1 as usize
    }

    /// The image of `bytes`, given for `path` ([`RealmImage::header`]'s rules).
    pub(crate) fn new(path: &str, bytes: Arc<[u8]>) -> Option<RealmImage> {
        let (kind, size) = RealmImage::header(path, &bytes)?;
        Some(RealmImage {
            id: crate::textures::next_id(),
            kind,
            bytes,
            size,
            texture: Rc::default(),
        })
    }

    /// The bytes it takes decoded.
    pub(crate) fn decoded_bytes(&self) -> usize {
        RealmImage::decoded_bytes_of(self.size)
    }

    /// The bytes holding it costs: decoded, its file's, and a texture's own ([`REALM_TEXTURE_OVERHEAD`]).
    pub(crate) fn held_bytes(&self) -> usize {
        self.decoded_bytes() + self.bytes.len() + REALM_TEXTURE_OVERHEAD
    }
}

impl fmt::Debug for RealmImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RealmImage")
            .field("id", &self.id)
            .field("bytes", &self.bytes.len())
            .field("size", &self.size)
            .finish()
    }
}

/// What a realm's images may still decode: a burst, refilled with time ([`REALM_DECODE_BURST`], [`REALM_DECODE_PER_SECOND`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct DecodeAllowance {
    bytes: usize,
    /// The tick count (milliseconds) of the last refill; none before the first image.
    at_ms: Option<u32>,
}

impl Default for DecodeAllowance {
    fn default() -> DecodeAllowance {
        DecodeAllowance {
            bytes: REALM_DECODE_BURST,
            at_ms: None,
        }
    }
}

impl DecodeAllowance {
    /// Takes `cost` bytes at tick `now_ms`: false (and nothing taken) when there are not that many yet.
    pub(crate) fn take(&mut self, now_ms: u32, cost: usize) -> bool {
        if let Some(at) = self.at_ms {
            let gained =
                (now_ms.wrapping_sub(at) as usize).saturating_mul(REALM_DECODE_PER_SECOND / 1000);
            self.bytes = self.bytes.saturating_add(gained).min(REALM_DECODE_BURST);
        }
        self.at_ms = Some(now_ms);
        let Some(left) = self.bytes.checked_sub(cost) else {
            return false;
        };
        self.bytes = left;
        true
    }
}

/// What a frame reuses: the textures, the fonts and their sheets, and `dglDrawTextN`'s saved colour (a global in TGE).
pub struct DrawCache {
    pub textures: GuiTextures,
    /// The texel bytes of realms' images this frame may still decode ([`MAX_REALM_DECODE_FRAME`]).
    realm_decode_left: usize,
    /// The time this frame's decodes of realms' images took on its thread, charged to the realm when the frame ends.
    realm_decode_time: Duration,
    /// Realm images decoding on threads now.
    realm_in_flight: Arc<AtomicUsize>,
    /// The texel bytes of realm images this frame may still decode on its own thread ([`REALM_DECODE_HERE_FRAME`]).
    realm_here_left: usize,
    pub fonts: Box<dyn crate::FontSource>,
    sheets: BTreeMap<(String, i32, usize), Arc<GuiTexture>>,
    /// The sheets' texel bytes.
    sheet_bytes: usize,
    /// GuiChunkedBitmapCtrl's bitmaps.
    pub chunked: ChunkCache,
    saved_color: [u8; 4],
    /// What the server layer drew last, kept while nothing it draws from changes ([`LayerMemo`]).
    layer: Option<LayerMemo>,
    /// How many frames drew the server layer from what it drew before.
    pub layer_replays: u64,
    /// How many canvas frames [`draw_canvas`] walked with this cache.
    pub canvas_draws: u64,
    /// The sheet texture the last glyph of the last run was drawn from.
    sheet_hint: Option<SheetHint>,
    /// How many of the GUI's registered TrueType faces ([`crate::faces`]) the fonts have taken.
    faces_taken: usize,
}

/// A font sheet's texture and what names it ([`DrawCache::sheets`] has the same under its key).
struct SheetHint {
    /// Lower case.
    face: String,
    size: i32,
    sheet: usize,
    texture: Arc<GuiTexture>,
}

/// What the server layer drew in a frame, drawn again from here in a frame whose [`LayerKey`] is the same: the batches it pushed
/// (the first only with the quads it added), what the quad budget counted for them, the counts it added to the report, and the
/// glyph color it left saved.
struct LayerMemo {
    key: LayerKey,
    batches: Vec<GuiBatch>,
    pushed: usize,
    counts: ReportCounts,
    /// The faces and sizes no font was found for that the layer's draw added to the report.
    missing_fonts: Vec<String>,
    saved_color: [u8; 4],
}

/// Everything the server layer's draw reads that is not in the layer's own controls, and a stamp of those
/// ([`Gui::custom_stamp`]): a frame with the same key draws the same list.
#[derive(Clone, Debug, PartialEq)]
struct LayerKey {
    root: ObjId,
    stamp: u64,
    /// The extension's stamp ([`crate::GuiExtension::layer_stamp`]).
    ext: u64,
    viewport: [i32; 4],
    column: Option<[i32; 4]>,
    hover: Option<ObjId>,
    focus: Option<ObjId>,
    /// The caret's blink phase, while a field has the focus.
    caret: Option<bool>,
    /// Where the pointer is, while a tooltip shows.
    pointer: Option<(i32, i32)>,
    /// The fonts' revision ([`torque_fonts::FontCache::revision`]).
    fonts: u64,
    /// The glyph color saved when the layer starts.
    saved_color: [u8; 4],
}

/// The counts of a [`DrawReport`] a draw adds and a kept one adds again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ReportCounts {
    controls: u32,
    realm_controls: u32,
    text_measures: u32,
    no_font: u32,
    invalid_glyphs: u32,
    border_styles: u32,
    covered: u32,
}

impl ReportCounts {
    /// The counts of `r`. Every field of a report is named, so a counter added to it does not compile until it is counted here or
    /// said to be kept some other way.
    fn of(r: &DrawReport) -> ReportCounts {
        let DrawReport {
            controls,
            realm_controls,
            text_measures,
            no_font,
            invalid_glyphs,
            border_styles,
            covered,
            // Not counts a kept layer adds again: a draw that changes these is not kept ([`DrawMarks`]), or they are the frame's
            // own (the layers and the chip are marked after the server layer), or they are added by name ([`LayerMemo`]).
            unmodelled: _,
            no_texture: _,
            realm_waiting: _,
            realm_decodes: _,
            realm_decoded_here: _,
            missing_fonts: _,
            truncated: _,
            extension: _,
            chip: _,
            layers: _,
        } = r;
        ReportCounts {
            controls: *controls,
            realm_controls: *realm_controls,
            text_measures: *text_measures,
            no_font: *no_font,
            invalid_glyphs: *invalid_glyphs,
            border_styles: *border_styles,
            covered: *covered,
        }
    }

    /// What was added from `before` to `self`.
    fn since(self, before: ReportCounts) -> ReportCounts {
        let ReportCounts {
            controls,
            realm_controls,
            text_measures,
            no_font,
            invalid_glyphs,
            border_styles,
            covered,
        } = self;
        ReportCounts {
            controls: controls - before.controls,
            realm_controls: realm_controls - before.realm_controls,
            text_measures: text_measures - before.text_measures,
            no_font: no_font - before.no_font,
            invalid_glyphs: invalid_glyphs - before.invalid_glyphs,
            border_styles: border_styles - before.border_styles,
            covered: covered - before.covered,
        }
    }

    fn add_to(self, r: &mut DrawReport) {
        let ReportCounts {
            controls,
            realm_controls,
            text_measures,
            no_font,
            invalid_glyphs,
            border_styles,
            covered,
        } = self;
        r.controls += controls;
        r.realm_controls += realm_controls;
        r.text_measures += text_measures;
        r.no_font += no_font;
        r.invalid_glyphs += invalid_glyphs;
        r.border_styles += border_styles;
        r.covered += covered;
    }
}

/// What a draw must leave as it was for it to be kept: nothing it could not be drawn again from (a missing texture, a class not
/// modelled, a view or a world, a script callback, the quad cap).
#[derive(Clone, Debug, PartialEq, Eq)]
struct DrawMarks {
    no_texture: u32,
    /// Realm images waiting for their decode: the layer draws them on a later frame.
    realm_waiting: u32,
    unmodelled: u32,
    extension: u32,
    truncated: bool,
    extras: usize,
    worlds: usize,
    callbacks: usize,
    ts_rects: usize,
}

/// Where a layer's draw started.
struct LayerStart {
    key: Option<LayerKey>,
    mark: QuadMark,
    pushed: usize,
    counts: ReportCounts,
    missing_fonts: BTreeSet<String>,
    marks: DrawMarks,
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
    /// Whether the server layer's draw is kept to be drawn again.
    pub fn keeps_layer(&self) -> bool {
        self.layer.is_some()
    }

    pub fn new(fonts: Box<dyn crate::FontSource>) -> DrawCache {
        DrawCache {
            textures: GuiTextures::default(),
            realm_decode_left: MAX_REALM_DECODE_FRAME,
            realm_decode_time: Duration::ZERO,
            realm_in_flight: Arc::default(),
            realm_here_left: REALM_DECODE_HERE_FRAME,
            fonts,
            sheets: BTreeMap::new(),
            sheet_bytes: 0,
            chunked: ChunkCache::default(),
            saved_color: [0; 4],
            layer: None,
            layer_replays: 0,
            canvas_draws: 0,
            sheet_hint: None,
            faces_taken: 0,
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

/// How [`DrawCache::push_text`] draws a text: the font's face and size, the color, and the color of the outline one pixel around it
/// (none: no outline).
#[derive(Debug, Clone, Copy)]
pub struct TextLook<'a> {
    pub face: &'a str,
    pub size: i32,
    pub color: [u8; 4],
    pub outline: Option<[u8; 4]>,
}

/// What one run of glyphs is drawn with: the clip rectangle, the opacity, and the font (with its face and size, the key of its
/// sheets' textures).
#[derive(Clone, Copy)]
struct GlyphRun<'a> {
    clip: [i32; 4],
    alpha: f32,
    font: &'a Arc<torque_fonts::Font>,
    face: &'a str,
    size: i32,
}

impl DrawCache {
    /// `dglDrawTextN`'s quads for `text` at `at`, pushed to `draws`: the pen's advance. A run cut by the quad budget or the
    /// sheets' caps is marked in `draws.report.truncated`.
    fn glyph_quads(
        &mut self,
        draws: &mut GuiDraws,
        run: GlyphRun<'_>,
        at: (i32, i32),
        text: &[u8],
        colors: &mut TextColors,
        table: Option<&[[u8; 4]]>,
    ) -> i32 {
        let font = run.font;
        // At most one quad a byte: the text is cut to what the budget has left.
        let left = MAX_QUADS.saturating_sub(draws.pushed);
        if left == 0 {
            draws.report.truncated = true;
            return 0;
        }
        let text = &text[..text.len().min(left)];
        let layout = font.layout(at, text, colors, table, 9);
        let advance = layout.advance;
        self.saved_color = colors.saved;
        draws.report.invalid_glyphs += layout.invalid;
        self.push_glyphs(draws, run, &layout.quads, (0, 0));
        advance
    }

    /// [`DrawCache::glyph_quads`] of `text` in `colors` at each of `offsets` from `at` in turn (an outline's passes): laid out once
    /// and pushed at each offset when every pass is the same (the quad budget holds them all, and the text leaves the saved colour
    /// as it was), else laid out again for each.
    fn glyph_quads_at(
        &mut self,
        draws: &mut GuiDraws,
        run: GlyphRun<'_>,
        at: (i32, i32),
        offsets: &[(i32, i32)],
        text: &[u8],
        color: [u8; 4],
    ) {
        let saved = self.saved_color;
        let left = MAX_QUADS.saturating_sub(draws.pushed);
        if left >= text.len().saturating_mul(offsets.len()) {
            let mut colors = TextColors::new(color, saved);
            let layout = run.font.layout(at, text, &mut colors, None, 9);
            if colors.saved == saved {
                for &offset in offsets {
                    draws.report.invalid_glyphs += layout.invalid;
                    if !self.push_glyphs(draws, run, &layout.quads, offset) {
                        return;
                    }
                }
                return;
            }
        }
        for &(dx, dy) in offsets {
            let mut colors = TextColors::new(color, self.saved_color);
            let at = (at.0.wrapping_add(dx), at.1.wrapping_add(dy));
            self.glyph_quads(draws, run, at, text, &mut colors, None);
        }
    }

    /// The texture of sheet `sheet` of `font` (the face `face` at `size`), made on first use: None when a cap on the sheets cuts
    /// it (marked in `draws.report.truncated`) or it cannot be made.
    fn sheet_texture(
        &mut self,
        draws: &mut GuiDraws,
        font: &torque_fonts::Font,
        face: &str,
        size: i32,
        sheet: usize,
    ) -> Option<Arc<GuiTexture>> {
        if let Some(h) = &self.sheet_hint
            && h.sheet == sheet
            && h.size == size
            && h.face.eq_ignore_ascii_case(face)
        {
            return Some(Arc::clone(&h.texture));
        }
        let key = (face.to_ascii_lowercase(), size, sheet);
        let texture = match self.sheets.get(&key) {
            Some(t) => Arc::clone(t),
            None => {
                if self.sheets.len() >= MAX_SHEET_TEXTURES || self.sheet_bytes >= MAX_SHEET_BYTES {
                    draws.report.truncated = true;
                    return None;
                }
                let t = texture_of(
                    &format!("font:{face}_{size}#{sheet}"),
                    font.sheets.get(sheet)?,
                    true,
                    false,
                )?;
                self.sheet_bytes += t.rgba.len();
                self.sheets.insert(key.clone(), Arc::clone(&t));
                t
            }
        };
        self.sheet_hint = Some(SheetHint {
            face: key.0,
            size,
            sheet,
            texture: Arc::clone(&texture),
        });
        Some(texture)
    }

    /// Pushes the glyph quads `quads` of `run` moved by `offset` to `draws`, each sheet a nearest-filtered texture: false when the
    /// quad budget ran out (marked in `draws.report.truncated`).
    fn push_glyphs(
        &mut self,
        draws: &mut GuiDraws,
        run: GlyphRun<'_>,
        quads: &[torque_fonts::GlyphQuad],
        (dx, dy): (i32, i32),
    ) -> bool {
        let GlyphRun {
            clip,
            alpha,
            font,
            face,
            size,
        } = run;
        // The sheet of the last glyph: most runs are of one sheet.
        let mut last: Option<(usize, Arc<GuiTexture>)> = None;
        for q in quads {
            if draws.full() {
                draws.report.truncated = true;
                return false;
            }
            let tex = match &last {
                Some((sheet, t)) if *sheet == q.sheet => Arc::clone(t),
                _ => {
                    let Some(t) = self.sheet_texture(draws, font, face, size, q.sheet) else {
                        continue;
                    };
                    last = Some((q.sheet, Arc::clone(&t)));
                    t
                }
            };
            let mut color = q.color;
            if alpha < 1.0 {
                color[3] = (f32::from(color[3]) * alpha).round() as u8;
            }
            let [x0, y0, x1, y1] = q.rect;
            let rect = [
                x0.wrapping_add(dx),
                y0.wrapping_add(dy),
                x1.wrapping_add(dx),
                y1.wrapping_add(dy),
            ];
            let quad = GuiQuad {
                rect: rect.map(|v| v as f32),
                uv: q.uv,
                color,
            };
            draws.push(clip, Some(&tex), GuiBlend::Alpha, quad);
        }
        true
    }

    /// The width and height in pixels of `text` in the font `face` at `size` (None: no font was found for them).
    pub fn text_size(&mut self, face: &str, size: i32, text: &[u8]) -> Option<(i32, i32)> {
        let font = self.fonts.font(face, size, FontUse::Profile, &|_| None)?;
        Some((font.str_width(text), font.height as i32))
    }

    /// Pushes the glyph quads of `text`, its top left corner at `at`, to `draws`, clipped to `clip` (left, top, width and
    /// height), as `look` says. The fonts are found as the GUI's are, with no file of the game to read. Returns the text's width
    /// in pixels, or None when no font was found for the face (counted in [`DrawReport::no_font`]).
    pub fn push_text(
        &mut self,
        draws: &mut GuiDraws,
        clip: [i32; 4],
        at: (i32, i32),
        text: &[u8],
        look: &TextLook<'_>,
    ) -> Option<i32> {
        let TextLook {
            face,
            size,
            color,
            outline,
        } = *look;
        let Some(font) = self.fonts.font(face, size, FontUse::Profile, &|_| None) else {
            draws.report.no_font += 1;
            if draws.report.missing_fonts.len() < MAX_MISSING_FONTS {
                draws.report.missing_fonts.insert(format!("{face}_{size}"));
            }
            return None;
        };
        let run = || GlyphRun {
            clip,
            alpha: 1.0,
            font: &font,
            face,
            size,
        };
        if let Some(outline) = outline {
            const AROUND: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
            self.glyph_quads_at(draws, run(), at, &AROUND, text, outline);
        }
        let mut colors = TextColors::new(color, self.saved_color);
        Some(self.glyph_quads(draws, run(), at, text, &mut colors, None))
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
    if let Some(torque_ts_vm::fields::Value::Object(None)) =
        vm.objects().static_value(id, b"profile")
    {
        return None;
    }
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
    /// Whose fonts the controls being drawn name: [`FontUse::Server`] while a server's layer draws.
    font_use: FontUse,
    /// The markup fonts this frame may still load ([`MAX_FRAME_MARKUP_LOADS`]).
    markup_loads: u32,
    /// The class `GuiControl`, looked up once a frame.
    gui_control: Option<torque_ts_vm::sim::ClassId>,
    /// The extent a parent has just read for the control it draws next, so that the control does not read it again.
    known_extent: Option<(ObjId, (i32, i32))>,
    /// The font the last lookup found, for the next lookup of it.
    font_memo: Option<FontMemo>,
}

/// A font a frame's draw looked up, and the fonts' revision it was found at: a lookup of the same face, size and use at that
/// revision finds it again without asking the cache (and without its recency stamp, which a lookup earlier in the frame set).
struct FontMemo {
    face: String,
    size: i32,
    user: FontUse,
    revision: u64,
    font: Arc<torque_fonts::Font>,
}

/// The most markup fonts ([`FontUse::Markup`], [`FontUse::Server`]) one frame's drawing loads: past it a markup font not loaded
/// yet draws nothing this frame (counted as a missing font) and loads in a later one, so text that names hundreds of fonts
/// rasterizes a few a frame.
pub const MAX_FRAME_MARKUP_LOADS: u32 = 8;

impl Walk<'_> {
    /// The font `face` at `size` for `user`, read through the `Vm`'s files on first use; a markup font not loaded yet waits for a
    /// later frame once this one loaded [`MAX_FRAME_MARKUP_LOADS`].
    fn font_for(
        &mut self,
        face: &str,
        size: i32,
        user: FontUse,
    ) -> Option<Arc<torque_fonts::Font>> {
        let revision = self.cache.fonts.cache().revision();
        if let Some(m) = &self.font_memo
            && m.revision == revision
            && m.size == size
            && m.user == user
            && m.face == face
        {
            return Some(Arc::clone(&m.font));
        }
        if user != FontUse::Profile && !self.cache.fonts.cache().contains(face, size) {
            if self.markup_loads == 0 {
                return None;
            }
            self.markup_loads -= 1;
        }
        let vm = self.vm;
        let read = |p: &str| match vm.file_host().read(p.as_bytes()) {
            FileRead::Found(b) => Some(b),
            _ => None,
        };
        let font = self.cache.fonts.font(face, size, user, &read);
        if let Some(font) = &font {
            self.font_memo = Some(FontMemo {
                face: face.to_string(),
                size,
                user,
                revision: self.cache.fonts.cache().revision(),
                font: Arc::clone(font),
            });
        }
        font
    }

    /// Whether `id` exists and is a `GuiControl`.
    fn is_gui_control(&self, id: ObjId) -> bool {
        let objects = self.vm.objects();
        match (objects.get(id), self.gui_control) {
            (Some(object), Some(ancestor)) => objects.classes.is_a(object.class(), ancestor),
            _ => false,
        }
    }

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

    /// A realm image's texture: decoded from its own bytes once (never from the game's), when this frame's and the realm's
    /// decode allowances hold it; natively on a thread, the image drawing nothing until it is done. Else it waits for a later
    /// frame.
    fn realm_texture(&mut self, image: &RealmImage) -> Option<Arc<GuiTexture>> {
        let mut slot = image.texture.borrow_mut();
        match &*slot {
            Decode::Done(made) => return made.clone(),
            #[cfg(not(target_family = "wasm"))]
            Decode::Pending(done) => {
                let made = match done.try_recv() {
                    Ok(made) => made,
                    Err(mpsc::TryRecvError::Empty) => {
                        self.out.draws.report.realm_waiting += 1;
                        return None;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => None,
                };
                if made.is_none() {
                    self.out.draws.report.no_texture += 1;
                }
                *slot = Decode::Done(made.clone());
                return made;
            }
            Decode::Idle => {}
        }
        let cost = image.decoded_bytes();
        let first = self.cache.realm_decode_left == MAX_REALM_DECODE_FRAME;
        let here = cost <= self.cache.realm_here_left || cfg!(target_family = "wasm");
        let free = here
            || self.cache.realm_in_flight.load(Ordering::Acquire) < MAX_REALM_DECODES_IN_FLIGHT;
        let allowed = free
            && (first || cost <= self.cache.realm_decode_left)
            && self
                .gui
                .0
                .borrow_mut()
                .client_script
                .decode
                .take(self.now, cost);
        if !allowed {
            self.out.draws.report.realm_waiting += 1;
            return None;
        }
        self.cache.realm_decode_left = self.cache.realm_decode_left.saturating_sub(cost);
        self.out.draws.report.realm_decodes += 1;
        #[cfg(not(target_family = "wasm"))]
        if !here && let Some(done) = self.decode_on_a_thread(image) {
            *slot = Decode::Pending(done);
            self.out.draws.report.realm_waiting += 1;
            return None;
        }
        self.cache.realm_here_left = self.cache.realm_here_left.saturating_sub(cost);
        let started = Instant::now();
        let made = decode_realm_image(image.kind, &image.bytes, image.size, image.id);
        self.cache.realm_decode_time += started.elapsed();
        self.out.draws.report.realm_decoded_here += 1;
        if made.is_none() {
            self.out.draws.report.no_texture += 1;
        }
        *slot = Decode::Done(made.clone());
        made
    }

    /// Starts decoding `image` on a thread of its own: what it gives back, or none when no thread could be made (then it is
    /// decoded here).
    #[cfg(not(target_family = "wasm"))]
    fn decode_on_a_thread(
        &self,
        image: &RealmImage,
    ) -> Option<mpsc::Receiver<Option<Arc<GuiTexture>>>> {
        let (send, done) = mpsc::channel();
        let in_flight = Arc::clone(&self.cache.realm_in_flight);
        let (kind, bytes, size, id) = (image.kind, Arc::clone(&image.bytes), image.size, image.id);
        in_flight.fetch_add(1, Ordering::AcqRel);
        let spawned = std::thread::Builder::new()
            .name("realm image".into())
            .spawn({
                let in_flight = Arc::clone(&in_flight);
                move || {
                    // A realm that is gone takes nothing back: the texture is dropped with the channel.
                    let _ = send.send(decode_realm_image(kind, &bytes, size, id));
                    in_flight.fetch_sub(1, Ordering::AcqRel);
                }
            });
        if spawned.is_err() {
            in_flight.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(done)
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

    /// The font `face` at `size` (a miss is counted in the report).
    fn font(&mut self, face: &str, size: i32) -> Option<Arc<torque_fonts::Font>> {
        let font = self.font_for(face, size, self.font_use);
        if font.is_none() {
            self.out.draws.report.no_font += 1;
            if self.out.draws.report.missing_fonts.len() < MAX_MISSING_FONTS {
                self.out
                    .draws
                    .report
                    .missing_fonts
                    .insert(format!("{face}_{size}"));
            }
        }
        font
    }

    /// `renderChildControls`.
    fn children(&mut self, id: ObjId, update: [i32; 4], offset: (i32, i32), depth: usize) {
        if depth >= MAX_TREE_DEPTH {
            return;
        }
        let kids = children(self.vm, id);
        // A control with no children has nothing to place; the extension is still asked for its displacement.
        if kids.is_empty() {
            self.ext.shift(self.gui, id, self.now);
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
        // Which children are shown, read once: no script runs during the walk, so it holds for the whole loop. The shown ones
        // whose fill the extension replaces are then the only siblings a child is checked against (one pass, not a pass a child);
        // the first shown child has no shown sibling before it to cover.
        let shown: Vec<bool> = kids
            .iter()
            .map(|&c| self.is_gui_control(c) && flag(self.vm, c, b"visible"))
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
                self.known_extent = Some((child, ext));
                self.control_faded(child, at, clip, depth + 1, true);
            }
        }
    }

    /// The opacity the control `id` draws itself and its children at, times their parent's: its `opacity` field (0 to 1) and, for
    /// a child (`child`), the extension's ([`crate::GuiExtension::opacity`]); `None` when neither dims it.
    fn opacity_of(&self, id: ObjId, child: bool) -> Option<f32> {
        let own = match self.vm.objects().static_value(id, b"opacity") {
            Some(torque_ts_vm::fields::Value::Float(f)) if *f < 1.0 => Some(f.max(0.0)),
            _ => None,
        };
        if !child {
            return own;
        }
        match (own, self.ext.opacity(self.vm, id)) {
            (Some(own), Some(ext)) => Some(own * ext),
            (own, ext) => own.or(ext),
        }
    }

    /// [`control`](Self::control) at its opacity ([`opacity_of`](Self::opacity_of)): the controls under it draw at it too. `child`:
    /// a child of a control drawn (the extension may dim those); a layer's content or an overlay takes its own field alone.
    fn control_faded(
        &mut self,
        id: ObjId,
        offset: (i32, i32),
        update: [i32; 4],
        depth: usize,
        child: bool,
    ) {
        let alpha = self.alpha;
        if let Some(o) = self.opacity_of(id, child) {
            self.alpha = alpha * o;
        }
        self.control(id, offset, update, depth);
        self.alpha = alpha;
    }

    /// One control's `onRender`, by its class.
    fn control(&mut self, id: ObjId, offset: (i32, i32), update: [i32; 4], depth: usize) {
        if self.out.draws.report.controls >= MAX_CONTROLS {
            self.out.draws.report.truncated = true;
            return;
        }
        self.out.draws.report.controls += 1;
        let known = self.known_extent.take().filter(|k| k.0 == id).map(|k| k.1);
        let ext = std::rc::Rc::clone(&self.ext);
        ext.before_control(&mut DrawCtx(self), id, offset, update);
        if self.custom_control(id, offset, update, depth, known) {
            return;
        }
        let vm = self.vm;
        let objects = vm.objects();
        let class = objects.get(id).map_or(std::borrow::Cow::Borrowed(""), |o| {
            String::from_utf8_lossy(objects.classes.name(o.class()))
        });
        match class.as_ref() {
            // The deprecated seam, for a class no plugin control handles.
            #[allow(deprecated)]
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
                *self
                    .out
                    .draws
                    .report
                    .unmodelled
                    .entry(class.into_owned())
                    .or_default() += 1;
                self.gui_control(id, offset, update, depth);
            }
        }
    }

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

    /// A control of a class a plugin declared ([`crate::custom`]): the script's first `onRender`, the class's draw, then the
    /// children. False for any other class.
    fn custom_control(
        &mut self,
        id: ObjId,
        offset: (i32, i32),
        update: [i32; 4],
        depth: usize,
        known_extent: Option<(i32, i32)>,
    ) -> bool {
        let Some((behavior, mut state)) = self.gui.take_custom(id) else {
            return false;
        };
        let first = !self.gui.is_realm(id)
            && self
                .gui
                .with_mut(id, |c| !std::mem::replace(&mut c.has_rendered, true))
                .unwrap_or(false);
        if first {
            self.out.callbacks.push(Callback::OnRender(id));
        }
        let place = crate::custom::Place {
            id,
            offset,
            extent: known_extent.unwrap_or_else(|| point(self.vm, id, b"extent")),
            update,
            depth,
        };
        behavior.draw(&mut *state, &mut DrawCtx(self), &place);
        let shift = behavior.child_shift(&*state);
        self.gui.put_custom(id, state);
        let inside = (
            offset.0.wrapping_add(shift.0),
            offset.1.wrapping_add(shift.1),
        );
        self.children(id, update, inside, depth);
        true
    }

    /// `GuiControl::onRender`.
    fn gui_control(&mut self, id: ObjId, offset: (i32, i32), update: [i32; 4], depth: usize) {
        let first = self
            .gui
            .with_mut(id, |c| !std::mem::replace(&mut c.has_rendered, true))
            .unwrap_or(false);
        if first {
            self.out.callbacks.push(Callback::OnRender(id));
        }
        self.control_box(id, offset);
        self.children(id, update, offset, depth);
    }

    /// `GuiControl::onRender` without the children: the profile's fill when `opaque`, then its border.
    fn control_box(&mut self, id: ObjId, offset: (i32, i32)) {
        let ext = point(self.vm, id, b"extent");
        let rect = [offset.0, offset.1, ext.0, ext.1];
        if let Some(p) = profile_of(self.vm, id) {
            if flag(self.vm, p, b"opaque") && !self.blurred(p) {
                let color = color_field(self.vm, p, b"fillColor");
                self.fill_profile(p, rect, color);
            }
            self.render_border(p, rect);
        }
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
        let chunk_key = format!(
            "{}#chunk",
            String::from_utf8_lossy(name).to_ascii_lowercase()
        );
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
                let key = (chunk_key.clone(), (j * cols + i) as i32, 0usize);
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
        let Some(font) = self.font_for(&face, size, self.font_use) else {
            self.out.draws.report.no_font += 1;
            if self.out.draws.report.missing_fonts.len() < MAX_MISSING_FONTS {
                self.out
                    .draws
                    .report
                    .missing_fonts
                    .insert(format!("{face}_{size}"));
            }
            return None;
        };
        let width = font.str_width(text);
        let table: [[u8; 4]; 10] =
            std::array::from_fn(|i| crate::profile::font_color(vm, profile, i));
        // `mActive` picks `fontColor` or `fontColorNA`, and a control that is not `enabled` is NA either way.
        let active = crate::input::active(self.gui, id) && flag(vm, id, b"enabled");
        let color = color.unwrap_or(if active { table[0] } else { table[2] });
        let justify = text_field(vm, profile, b"justify");
        let justify = match justify.as_slice() {
            j if j.eq_ignore_ascii_case(b"right") || j == b"1" => 1,
            j if j.eq_ignore_ascii_case(b"center") || j == b"2" => 2,
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
            // Every offset of -1..1 each way, x then y.
            const NINE: [(i32, i32); 9] = [
                (-1, -1),
                (-1, 0),
                (-1, 1),
                (0, -1),
                (0, 0),
                (0, 1),
                (1, -1),
                (1, 0),
                (1, 1),
            ];
            let run = GlyphRun {
                clip: self.clip,
                alpha: self.alpha,
                font: &font,
                face: &face,
                size,
            };
            let at = (x.wrapping_add(xoff), at.1.wrapping_add(yoff));
            self.cache
                .glyph_quads_at(&mut self.out.draws, run, at, &NINE, text, outline);
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
            Some(table.as_slice()),
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
        self.cache.glyph_quads(
            &mut self.out.draws,
            GlyphRun {
                clip: self.clip,
                alpha: self.alpha,
                font,
                face,
                size,
            },
            at,
            text,
            colors,
            table,
        )
    }

    fn mark(&mut self, mark: crate::client_script::LayerMark) {
        self.out.draws.report.layers.push(mark);
    }

    fn quad_mark(&self) -> QuadMark {
        let batches = &self.out.draws.batches;
        QuadMark {
            batches: batches.len(),
            quads: batches.last().map_or(0, |b| b.quads.len()),
        }
    }

    /// Cuts `hole` (x, y, width, height) out of every quad pushed since `mark` ([`cut_quad`]).
    fn cut_since(&mut self, mark: QuadMark, hole: [i32; 4]) {
        // The batch that was last at the mark may have grown since.
        let first = mark.batches.saturating_sub(1);
        for (i, batch) in self.out.draws.batches.iter_mut().enumerate().skip(first) {
            let from = if i + 1 == mark.batches { mark.quads } else { 0 };
            let tail = batch.quads.split_off(from.min(batch.quads.len()));
            for q in tail {
                cut_quad(q, hole, &mut batch.quads);
            }
        }
    }

    /// Draws one control of a client-script layer at the screen's origin.
    fn layer_control(&mut self, id: ObjId) {
        if !self.is_gui_control(id) {
            return;
        }
        let viewport = self.viewport;
        self.set_clip(viewport);
        let alpha = std::mem::replace(&mut self.alpha, 1.0);
        let pos = point(self.vm, id, b"position");
        self.control(id, pos, viewport, 1);
        self.alpha = alpha;
    }

    /// The server layer, when the plan has it ([`crate::client_script`]): drawn again from what it drew last when nothing it
    /// draws from changed since ([`LayerMemo`]).
    fn server_layer(&mut self, plan: &crate::client_script::DrawPlan) {
        let Some(root) = plan.server else {
            return;
        };
        let start = self.begin_layer(root);
        if let Some(key) = &start.key
            && self.replay_layer(key)
        {
            self.mark(crate::client_script::LayerMark::Server);
            return;
        }
        let user = std::mem::replace(&mut self.font_use, FontUse::Server);
        self.layer_control(root);
        // A focused text field (a leaf) is drawn again over everything else of the layer.
        if let Some((field, at)) = crate::client_script::focus_on_top(self.vm, self.gui) {
            let viewport = self.viewport;
            self.set_clip(viewport);
            self.control(field, at, viewport, 1);
        }
        let viewport = self.viewport;
        self.set_clip(viewport);
        crate::client_script::draw_tooltip(&mut DrawCtx(self));
        self.font_use = user;
        // Nothing of the layer shows in the chip's column.
        let column = self.gui.0.borrow().client_script.column;
        if let Some(column) = column {
            self.cut_since(start.mark, column);
        }
        self.keep_layer(start);
        self.mark(crate::client_script::LayerMark::Server);
    }

    /// What a layer's draw leaves out of the draw list or the report, other than the counts a kept layer adds again.
    fn draw_marks(&self) -> DrawMarks {
        let draws = &self.out.draws;
        let r = &draws.report;
        DrawMarks {
            no_texture: r.no_texture,
            realm_waiting: r.realm_waiting,
            unmodelled: r.unmodelled.values().sum(),
            extension: r.extension.values().sum(),
            truncated: r.truncated,
            extras: draws.extras.len(),
            worlds: draws.worlds.len(),
            callbacks: self.out.callbacks.len(),
            ts_rects: self.ts_rects.len(),
        }
    }

    /// The key a draw of the server layer made now would be kept under (none: it cannot be kept).
    fn layer_key(&self, root: ObjId) -> Option<LayerKey> {
        let ext = self.ext.layer_stamp(self.gui, self.now)?;
        let stamp = self.gui.custom_stamp();
        let tooltip = crate::client_script::tooltip_text(self.gui).is_some();
        let s = self.gui.0.borrow();
        let l = &s.client_script;
        Some(LayerKey {
            root,
            stamp,
            ext,
            viewport: self.viewport,
            column: l.column,
            hover: l.hover,
            focus: l.focus,
            caret: l.focus.map(|_| crate::client_script::caret_on(self.now)),
            pointer: tooltip.then_some(s.cursor_pos),
            fonts: self.cache.fonts.cache().revision(),
            saved_color: self.cache.saved_color,
        })
    }

    fn begin_layer(&self, root: ObjId) -> LayerStart {
        LayerStart {
            key: self.layer_key(root),
            mark: self.quad_mark(),
            pushed: self.out.draws.pushed,
            counts: ReportCounts::of(&self.out.draws.report),
            missing_fonts: self.out.draws.report.missing_fonts.clone(),
            marks: self.draw_marks(),
        }
    }

    /// Pushes what the layer drew under `key`, if that is what is kept and the budgets hold it: whether it did.
    fn replay_layer(&mut self, key: &LayerKey) -> bool {
        let Some(memo) = self.cache.layer.as_ref().filter(|m| m.key == *key) else {
            return false;
        };
        let draws = &mut self.out.draws;
        if draws.report.truncated
            || draws.pushed.saturating_add(memo.pushed) > MAX_QUADS
            || draws.report.controls.saturating_add(memo.counts.controls) > MAX_CONTROLS
        {
            return false;
        }
        let mut left = memo.pushed;
        for batch in &memo.batches {
            let counted = batch.quads.len().min(left);
            left -= counted;
            draws.push_again(batch, counted);
        }
        draws.pushed += left;
        memo.counts.add_to(&mut draws.report);
        draws.report.missing_fonts.extend(
            memo.missing_fonts
                .iter()
                .take(MAX_MISSING_FONTS.saturating_sub(draws.report.missing_fonts.len()))
                .cloned(),
        );
        self.cache.saved_color = memo.saved_color;
        self.cache.layer_replays += 1;
        let viewport = self.viewport;
        self.set_clip(viewport);
        true
    }

    /// Keeps what the layer drew since `start`, under the key it started with, unless it drew what cannot be drawn again from
    /// the key alone.
    fn keep_layer(&mut self, start: LayerStart) {
        let LayerStart {
            key,
            mark,
            pushed,
            counts,
            missing_fonts,
            marks,
        } = start;
        self.cache.layer = None;
        let Some(mut key) = key else {
            return;
        };
        let counted = ReportCounts::of(&self.out.draws.report).since(counts);
        // A font the frame's cap on loads kept from the layer is loaded by a later frame, which then draws the text: only a
        // font nothing can make is the same every frame.
        let waiting = counted.no_font > 0 && self.markup_loads == 0;
        if waiting || self.out.draws.pushed - pushed > MAX_KEPT_QUADS || self.draw_marks() != marks
        {
            return;
        }
        key.fonts = self.cache.fonts.cache().revision();
        let draws = &self.out.draws;
        let from = mark.batches.saturating_sub(1);
        let batches = draws
            .batches
            .iter()
            .enumerate()
            .skip(from)
            .filter_map(|(i, b)| {
                // The batch the layer started in keeps what came before it; a batch the cut left empty is still a batch.
                let first = i + 1 == mark.batches;
                let quads = b.quads.get(if first { mark.quads } else { 0 }..)?;
                if first && quads.is_empty() {
                    return None;
                }
                Some(GuiBatch {
                    clip: b.clip,
                    texture: b.texture.clone(),
                    blend: b.blend,
                    quads: quads.to_vec(),
                })
            })
            .collect();
        self.cache.layer = Some(LayerMemo {
            key,
            batches,
            pushed: draws.pushed - pushed,
            counts: counted,
            missing_fonts: draws
                .report
                .missing_fonts
                .difference(&missing_fonts)
                .cloned()
                .collect(),
            saved_color: self.cache.saved_color,
        });
    }

    /// The engine layer's controls and the chip, over every dialog.
    fn engine_layer(&mut self, plan: &crate::client_script::DrawPlan) {
        for &id in &plan.engine {
            self.layer_control(id);
        }
        self.mark(crate::client_script::LayerMark::Engine);
        if let Some(chip) = plan.chip {
            self.layer_control(chip);
            self.out.draws.report.chip = true;
            self.mark(crate::client_script::LayerMark::Chip);
        }
    }

    /// The cursor (the end of `renderFrame`); notes whether it was drawn ([`crate::GuiState::cursor_drawn`]).
    fn cursor(&mut self) {
        let drawn = self.draw_cursor();
        self.gui.0.borrow_mut().cursor_drawn = drawn;
    }

    /// The canvas's GuiCursor at the cursor's place: whether it was drawn.
    fn draw_cursor(&mut self) -> bool {
        let (on, shown, cursor, pos) = {
            let s = self.gui.0.borrow();
            (s.cursor_on, s.show_cursor, s.cursor, s.cursor_pos)
        };
        let Some(cursor) = cursor.filter(|c| on && shown && self.vm.objects().contains(*c)) else {
            return false;
        };
        let hot = point(self.vm, cursor, b"hotSpot");
        let Some(t) = self.texture(&text_field(self.vm, cursor, b"bitmapName"), false) else {
            return false;
        };
        let (bw, bh) = (t.bitmap_size.0 as i32, t.bitmap_size.1 as i32);
        self.stretch(
            &t,
            [pos.0 - hot.0, pos.1 - hot.1, bw, bh],
            [0, 0, bw, bh],
            [255; 4],
            false,
        );
        true
    }
}

/// `GuiCanvas::renderFrame` as a draw list (see the module docs). Runs no script; the callbacks it asks for are in the frame.
pub fn draw_canvas(vm: &Vm, gui: &Gui, cache: &mut DrawCache) -> Frame {
    cache.canvas_draws += 1;
    cache.chunked.begin_frame();
    cache.textures.begin_frame();
    cache.realm_decode_left = MAX_REALM_DECODE_FRAME;
    cache.realm_here_left = REALM_DECODE_HERE_FRAME;
    let (canvas, size) = {
        let s = gui.0.borrow();
        (s.canvas, s.canvas_size)
    };
    let now = vm.sim_time();
    let (w, h) = (size.0.max(0), size.1.max(0));
    let viewport = [0, 0, w, h];
    let layers = crate::client_script::DrawPlan::of(vm, gui);
    // A layer that is not drawn (the realm is gone or hidden, or another screen is up) keeps none of its textures alive.
    if layers.server.is_none() {
        cache.layer = None;
    }
    let ext = gui.ext();
    cache.fonts.next_frame();
    crate::faces::sync(vm, gui, cache.fonts.as_mut(), &mut cache.faces_taken);
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
        font_use: FontUse::Profile,
        markup_loads: MAX_FRAME_MARKUP_LOADS,
        gui_control: vm.objects().classes.find(b"GuiControl"),
        known_extent: None,
        font_memo: None,
    };
    walk.out.draws.size = (w as u32, h as u32);
    if let Some(canvas) = canvas.filter(|c| vm.objects().contains(*c)) {
        for (i, child) in children(vm, canvas).into_iter().enumerate() {
            if walk.is_gui_control(child) {
                walk.set_clip(viewport);
                let pos = point(vm, child, b"position");
                walk.control_faded(child, pos, viewport, 1, false);
            }
            walk.mark(if i == 0 {
                crate::client_script::LayerMark::Content
            } else {
                crate::client_script::LayerMark::Dialog
            });
            // The server layer: after the content, before every dialog.
            if i == 0 {
                walk.server_layer(&layers);
            }
        }
        // The host's overlays, over the canvas's children and under the cursor.
        for o in gui.overlays(vm) {
            if !walk.is_gui_control(o) || !gui.is_awake(o) {
                continue;
            }
            walk.set_clip(viewport);
            let pos = point(vm, o, b"position");
            walk.control_faded(o, pos, viewport, 1, false);
        }
        walk.engine_layer(&layers);
        walk.set_clip(viewport);
        walk.cursor();
    }
    let decoded = std::mem::take(&mut walk.cache.realm_decode_time);
    if !decoded.is_zero() {
        gui.0.borrow_mut().client_script.owed += decoded;
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

/// What a plugin control's draw ([`crate::custom::GuiControlClass::draw`]) and an extension's ([`crate::GuiExtension::before_control`]) reach of the
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

    /// Multiplies the opacity of the control being drawn and its children by `factor` (0 to 1); the walk restores it after
    /// the control.
    pub fn scale_alpha(&mut self, factor: f32) {
        self.0.alpha *= factor.clamp(0.0, 1.0);
    }

    /// The width and height of `text` in the font `face` at `size` (None: no such font).
    pub fn text_size(&mut self, face: &str, size: i32, text: &[u8]) -> Option<(i32, i32)> {
        self.0.out.draws.report.text_measures += 1;
        let font = self.0.font(face, size)?;
        Some((font.str_width(text), font.height as i32))
    }

    /// Narrows the clip to `r` (x, y, width, height) for the rest of this control's own drawing.
    pub fn clip_to(&mut self, r: [i32; 4]) {
        self.0.clip = intersect(self.0.clip, r);
    }

    /// `text` at `at` (its top left corner) in the font `face` at `size`, in one color: the pen's advance (None: no such
    /// font, nothing drawn).
    pub fn text(
        &mut self,
        face: &str,
        size: i32,
        at: (i32, i32),
        text: &[u8],
        color: [u8; 4],
    ) -> Option<i32> {
        let font = self.0.font(face, size)?;
        let mut colors = TextColors::new(self.0.faded(color), self.0.cache.saved_color);
        Some(
            self.0
                .glyphs(&font, face, size, at, text, &mut colors, None),
        )
    }

    /// The bitmap `name` over `rect` (x, y, width, height): `fit` is 0 stretch, 1 contain, 2 cover (cut to the rectangle),
    /// 3 centered at its own size; `tint` modulates it. False when it does not load.
    pub fn image(&mut self, name: &[u8], rect: [i32; 4], fit: u8, tint: [u8; 4]) -> bool {
        let Some(t) = self.0.texture(name, true) else {
            return false;
        };
        self.fitted(&t, rect, fit, tint);
        true
    }

    /// A client-script realm's image over `rect`, as [`DrawCtx::image`] draws a bitmap: only what the server's store gave,
    /// never a file of the game's. False when it does not decode.
    pub(crate) fn realm_image(
        &mut self,
        image: &RealmImage,
        rect: [i32; 4],
        fit: u8,
        tint: [u8; 4],
    ) -> bool {
        let Some(t) = self.0.realm_texture(image) else {
            return false;
        };
        self.fitted(&t, rect, fit, tint);
        true
    }

    /// The texture `t` over `rect` by `fit` ([`DrawCtx::image`]).
    fn fitted(&mut self, t: &Arc<GuiTexture>, rect: [i32; 4], fit: u8, tint: [u8; 4]) {
        let (bw, bh) = (t.bitmap_size.0 as i32, t.bitmap_size.1 as i32);
        if bw <= 0 || bh <= 0 || rect[2] <= 0 || rect[3] <= 0 {
            return;
        }
        let full = [0, 0, bw, bh];
        let (rw, rh) = (f64::from(rect[2]), f64::from(rect[3]));
        let (fw, fh) = (f64::from(bw), f64::from(bh));
        match fit {
            1 => {
                let scale = (rw / fw).min(rh / fh);
                let (w, h) = ((fw * scale) as i32, (fh * scale) as i32);
                let at = [
                    rect[0] + (rect[2] - w) / 2,
                    rect[1] + (rect[3] - h) / 2,
                    w,
                    h,
                ];
                self.0.stretch(t, at, full, tint, false);
            }
            2 => {
                let scale = (rw / fw).max(rh / fh);
                let sw = ((rw / scale) as i32).clamp(1, bw);
                let sh = ((rh / scale) as i32).clamp(1, bh);
                let src = [(bw - sw) / 2, (bh - sh) / 2, sw, sh];
                self.0.stretch(t, rect, src, tint, false);
            }
            3 => {
                let at = [
                    rect[0] + (rect[2] - bw) / 2,
                    rect[1] + (rect[3] - bh) / 2,
                    bw,
                    bh,
                ];
                self.0.stretch(t, at, full, tint, false);
            }
            _ => self.0.stretch(t, rect, full, tint, false),
        }
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

    /// A rounded box ([`BOX_RADIUS`]): `fill` over `r` (x, y, width, height), then a one-pixel `border` around it (none when
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

    /// `GuiControl::onRender` without its children: the profile's fill when it is `opaque`, then its border, for the control
    /// `id` drawn at `offset`.
    pub fn control_box(&mut self, id: ObjId, offset: (i32, i32)) {
        self.0.control_box(id, offset);
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
