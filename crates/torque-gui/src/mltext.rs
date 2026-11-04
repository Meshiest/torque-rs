//! GuiMLTextCtrl as TGE 1.4 has it: the text buffer, the reflow
//! into lines of atoms and bitmaps, the links and the selection. Drawing is [`crate::draw`]'s (`onRender`, `drawAtomText`).
//!
//! - **The buffer** (`setText`): bytes, cut to `maxChars - 1` when `maxChars` is positive; the cursor 0, the selection
//!   cleared, dirty, `scrollToTop` (a scroll parent shows the top). `addText`: appended (cut the same way); only
//!   with `reformat` the cursor, the selection and dirty as `setText`. `onAdd`: `setText(text)` when the field is not empty.
//! - **The reflow**, run before a frame draws when the control is dirty (`onPreRender`; `onWake` and a
//!   parent's resize make it dirty): the profile's font, colour (`fontColor`), link colours (`fontColors[4]`, `[5]`) start the
//!   style; nothing is laid out without that font. Tags (case-insensitive): `<br>`, `<font:face:size>` (size 1-128), `<tag:id>`,
//!   `<color:RRGGBB[AA]>`, `<shadowcolor:...>`, `<linkcolor:...>`, `<linkcolorhl:...>`, `<shadow:x:y>`, `<bitmap:name>` /
//!   `<bitmapk:name>`, `<spush>`, `<spop>`, `<sbreak>`, `<just:left|right|center>`, `<a:url>`... `</a>` (a `gamelink` URL is not
//!   underlined), `<lmargin%:p>`, `<lmargin:x>` (clamped to `[0, width - 10]`), `<rmargin%:p>`, `<rmargin:x>` (to `[0, width]`),
//!   `<clip:w>`, `</clip>`, `<div:n>`, `<tab:a,b,...>`; a `<` followed by another `<` before its `>` is text; a tag that does not
//!   parse is text. Words wrap at `getBreakPos` with word wrap (at least one byte a line); a line's height is its atoms' largest
//!   baseline plus largest descent (the `lineSpacing` field adds nothing), else the current font's height. With the extension's
//!   auto resize ([`crate::GuiExtension::ml_auto_resize`]) the control is resized to `(width, the text's height)` and the
//!   script's `onResize(width, height)` runs; then the cursor is
//!   kept in a scroll parent's view.
//!
//! Every loop is capped ([`MAX_ATOMS`], [`MAX_LINES`], [`MAX_REFLOW_STEPS`]) and charged to the step budget.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/controls/guiMLTextCtrl.cpp` (Copyright
//! (c) 2012 GarageGames, LLC), `gui/controls/guiMLTextEditCtrl.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is
//! this crate's own.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use torque_ts_vm::host::FileRead;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use crate::control::{bounds, is_a, parent};
use crate::profile::{int, profile_of, text};
use crate::{Gui, atoi};

/// The most atoms, lines, bitmaps and tags one reflow makes (a hostile text of a million tags stops there).
pub const MAX_ATOMS: usize = 1 << 17;
pub const MAX_LINES: usize = 1 << 16;
pub const MAX_BITMAPS: usize = 4096;
/// The most passes the emitting loops take in one reflow.
pub const MAX_REFLOW_STEPS: u64 = 1 << 22;

/// The justifications (`mCurJustify`).
pub const LEFT: u32 = 0;
pub const RIGHT: u32 = 1;
pub const CENTER: u32 = 2;

/// A style (`Style`).
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub color: [u8; 4],
    pub shadow_color: [u8; 4],
    pub link_color: [u8; 4],
    pub link_color_hl: [u8; 4],
    pub shadow_offset: (i32, i32),
    /// The font: face and size, as the tag (or the profile) named them.
    pub font: (String, i32),
    pub used: bool,
    pub next: Option<usize>,
}

/// A link (`URL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Url {
    pub mouse_down: bool,
    pub text_start: u32,
    pub len: u32,
    pub no_underline: bool,
}

/// A run of text on a line (`Atom`).
#[derive(Debug, Clone, PartialEq)]
pub struct Atom {
    pub text_start: u32,
    pub len: u32,
    pub x_start: u32,
    pub y_start: u32,
    pub width: u32,
    pub baseline: u32,
    pub descent: u32,
    pub style: usize,
    pub is_clipped: bool,
    pub url: Option<usize>,
}

/// A line (`Line`).
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub y: u32,
    pub height: u32,
    pub div: u32,
    pub text_start: u32,
    pub len: u32,
    pub atoms: Vec<Atom>,
}

/// A placed bitmap (`BitmapRef`): the rectangle in the control and the bitmap's name and size.
#[derive(Debug, Clone, PartialEq)]
pub struct BitmapRef {
    pub rect: [u32; 4],
    pub name: String,
    pub size: (u32, u32),
}

/// What a reflow made.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MlLayout {
    pub lines: Vec<Line>,
    /// In list order (the last placed first).
    pub bitmaps: Vec<BitmapRef>,
    pub styles: Vec<Style>,
    pub urls: Vec<Url>,
    pub tags: Vec<(u32, u32)>,
    pub max_y: u32,
}

/// A GuiMLTextCtrl's state beside its buffer ([`crate::Control::text`]).
#[derive(Debug, Clone, PartialEq)]
pub struct MlState {
    /// `mDirty`.
    pub dirty: bool,
    /// `mCursorPosition`, the selection (active, start, end), its anchor.
    pub cursor: u32,
    pub sel_active: bool,
    pub sel_start: u32,
    pub sel_end: u32,
    pub anchor: i32,
    pub anchor_point: (i32, i32),
    /// `mAlpha`.
    pub alpha: f32,
    /// The link the mouse went down on.
    pub hit_url: Option<usize>,
    pub layout: Rc<MlLayout>,
}

impl Default for MlState {
    fn default() -> MlState {
        MlState {
            dirty: true,
            cursor: 0,
            sel_active: false,
            sel_start: 0,
            sel_end: 0,
            anchor: 0,
            anchor_point: (0, 0),
            alpha: 1.0,
            hit_url: None,
            layout: Rc::new(MlLayout::default()),
        }
    }
}

pub fn state(gui: &Gui, id: ObjId) -> Option<MlState> {
    gui.with(id, |c| c.ml.as_deref().cloned()).flatten()
}

fn with_state<R>(gui: &Gui, id: ObjId, f: impl FnOnce(&mut MlState) -> R) -> Option<R> {
    gui.with_mut(id, |c| c.ml.as_deref_mut().map(f)).flatten()
}

/// The buffer.
pub fn buffer(gui: &Gui, id: ObjId) -> Vec<u8> {
    gui.with(id, |c| c.text.clone().unwrap_or_default())
        .unwrap_or_default()
}

/// `maxChars`'s cut: positive `maxChars` keeps at most `maxChars - 1` bytes.
fn max_chars(vm: &Vm, id: ObjId) -> Option<usize> {
    let m = int(vm, id, b"maxChars");
    (m > 0).then(|| (m - 1) as usize)
}

/// `scrollToTop`: a scroll parent shows the top left.
fn scroll_to_top(vm: &mut Vm, gui: &Gui, id: ObjId) {
    if let Some(p) = parent(vm, id).filter(|p| is_a(vm, *p, "GuiScrollCtrl")) {
        crate::scroll::scroll_rect_visible(vm, gui, p, [0, 0, 0, 0]);
    }
}

/// `setText`.
pub fn set_text(vm: &mut Vm, gui: &Gui, id: ObjId, t: &[u8]) {
    let mut keep = t.len().min(crate::MAX_TEXT_BYTES);
    if let Some(m) = max_chars(vm, id) {
        keep = keep.min(m);
    }
    if !vm.charge_work(keep as u64) {
        return;
    }
    let text = t[..keep].to_vec();
    gui.with_mut(id, |c| {
        c.text = Some(text);
        if let Some(s) = c.ml.as_deref_mut() {
            s.cursor = 0;
            s.sel_active = false;
            s.sel_start = 0;
            s.sel_end = 0;
            s.dirty = true;
        }
    });
    scroll_to_top(vm, gui, id);
}

/// `addText`.
pub fn add_text(vm: &mut Vm, gui: &Gui, id: ObjId, t: &[u8], reformat: bool) {
    let had = gui.with(id, |c| c.text.as_ref().map(Vec::len)).flatten();
    let Some(had) = had.filter(|_| true) else {
        set_text(vm, gui, id, t);
        return;
    };
    let mut add = t.len();
    if let Some(m) = max_chars(vm, id)
        && m < had + add
    {
        add = m.saturating_sub(had);
    }
    add = add.min(crate::MAX_TEXT_BYTES.saturating_sub(had));
    if !vm.charge_work((had + add) as u64) {
        return;
    }
    gui.with_mut(id, |c| {
        if let Some(b) = c.text.as_mut() {
            b.extend_from_slice(&t[..add]);
        }
        if reformat && let Some(s) = c.ml.as_deref_mut() {
            s.cursor = 0;
            s.sel_active = false;
            s.sel_start = 0;
            s.sel_end = 0;
            s.dirty = true;
        }
    });
    if reformat {
        scroll_to_top(vm, gui, id);
    }
}

/// Marks the control for a reflow (`mDirty`).
pub fn set_dirty(gui: &Gui, id: ObjId) {
    with_state(gui, id, |s| s.dirty = true);
}

/// A font of the layout's styles, by face and size, through the GUI's font cache.
fn font_of(vm: &Vm, gui: &Gui, face: &str, size: i32) -> Option<Arc<torque_fonts::Font>> {
    let fonts = gui.0.borrow().fonts.0.clone();
    let read = |p: &str| match vm.file_host().read(p.as_bytes()) {
        FileRead::Found(b) => Some(b),
        _ => None,
    };
    fonts.borrow_mut().font(face, size, &read)
}

/// The sizes of the bitmaps ML text names: each name (lower case) resolved once to the file the texture search found (None:
/// not found), each file's size read once from its header ([`torque_images::size_of`]); both least-recently-used caches of
/// [`crate::textures::MAX_TEXTURES`] entries. Before, a `<bitmap:>` was decoded in full to size it, and once 4,096 names were kept
/// every reflow decoded each new one again.
pub struct BitmapSizes {
    names: torque_images::lru::Lru<String, Option<String>>,
    files: torque_images::lru::Lru<String, Option<(u32, u32)>>,
    /// How many files' headers were read.
    pub reads: u64,
}

impl Default for BitmapSizes {
    fn default() -> BitmapSizes {
        BitmapSizes {
            names: torque_images::lru::Lru::new(crate::textures::MAX_TEXTURES, usize::MAX),
            files: torque_images::lru::Lru::new(crate::textures::MAX_TEXTURES, usize::MAX),
            reads: 0,
        }
    }
}

impl std::fmt::Debug for BitmapSizes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BitmapSizes")
            .field("names", &self.names.len())
            .field("files", &self.files.len())
            .field("reads", &self.reads)
            .finish()
    }
}

impl BitmapSizes {
    /// The size of the bitmap `name` names, found as the texture manager finds it with `read`: the first candidate that reads,
    /// sized by its header (a header the decoder would refuse, or a file that cannot be sized, is None).
    pub fn size(
        &mut self,
        name: &str,
        read: &dyn Fn(&str) -> Option<Vec<u8>>,
    ) -> Option<(u32, u32)> {
        let key = name.to_ascii_lowercase();
        let (file, bytes) = match self.names.get(&key) {
            Some(None) => return None,
            Some(Some(file)) => (file.clone(), None),
            None => {
                let found = crate::textures::candidates(name)
                    .into_iter()
                    .filter(|p| torque_images::Kind::of_name(p).is_some())
                    .find_map(|p| read(&p).map(|b| (p, b)));
                self.names
                    .insert(key, found.as_ref().map(|(p, _)| p.clone()), 1);
                let (file, bytes) = found?;
                (file, Some(bytes))
            }
        };
        let fkey = file.to_ascii_lowercase();
        if let Some(found) = self.files.get(&fkey) {
            return *found;
        }
        let bytes = match bytes {
            Some(b) => b,
            None => read(&file)?,
        };
        self.reads += 1;
        let size = torque_images::size_of(torque_images::Kind::of_name(&file)?, &bytes);
        self.files.insert(fkey, size, 1);
        size
    }
}

/// The size of a bitmap a `<bitmap:>` names, found as the texture manager finds it (cached in the GUI's state, [`BitmapSizes`]).
fn bitmap_size(vm: &Vm, gui: &Gui, name: &str) -> Option<(u32, u32)> {
    let read = |p: &str| match vm.file_host().read(p.as_bytes()) {
        FileRead::Found(b) => Some(b),
        _ => None,
    };
    gui.0.borrow_mut().bitmap_sizes.size(name, &read)
}

/// The hexadecimal value of a byte as the colour tags read it (`0-9`, `A-Z`, `a-z`; anything else -1), wrapped to a byte.
fn hex(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'A'..=b'Z' => c - b'A' + 10,
        b'a'..=b'z' => c - b'a' + 10,
        _ => 0xff,
    }
}

/// A colour tag's value at `at` (6 or 8 hex digits).
fn tag_color(s: &[u8], at: usize, with_alpha: bool) -> [u8; 4] {
    let b = |i: usize| s.get(at + i).copied().unwrap_or(0);
    let v = |i: usize| hex(b(i)).wrapping_mul(16).wrapping_add(hex(b(i + 1)));
    [v(0), v(2), v(4), if with_alpha { v(6) } else { 255 }]
}

/// `scanforchar`: from `idx`, stop at `c`, NUL, `:`, `>` or a newline; true when it stopped at `c` having moved.
fn scan_for(s: &[u8], idx: &mut usize, c: u8) -> bool {
    let start = *idx;
    loop {
        let ch = s.get(*idx).copied().unwrap_or(0);
        if ch == c || ch == 0 || ch == b':' || ch == b'>' || ch == b'\n' {
            break;
        }
        *idx += 1;
    }
    s.get(*idx).copied().unwrap_or(0) == c && start != *idx
}

/// [`scan_for`] that does not stop at `:` (links, and the check for a second `<`).
fn scan_for_any(s: &[u8], idx: &mut usize, c: u8) -> bool {
    let start = *idx;
    loop {
        let ch = s.get(*idx).copied().unwrap_or(0);
        if ch == c || ch == 0 || ch == b'>' || ch == b'\n' {
            break;
        }
        *idx += 1;
    }
    s.get(*idx).copied().unwrap_or(0) == c && start != *idx
}

fn starts_with_ci(s: &[u8], at: usize, p: &[u8]) -> bool {
    s.get(at..at + p.len())
        .is_some_and(|w| w.eq_ignore_ascii_case(p))
}

/// The reflow's working state.
struct Flow<'a> {
    vm: &'a Vm,
    gui: &'a Gui,
    text: &'a [u8],
    width: u32,
    max_bitmap_height: u32,
    out: MlLayout,
    fonts: HashMap<(String, i32), Arc<torque_fonts::Font>>,
    cur_style: usize,
    lmargin: u32,
    rmargin: u32,
    justify: u32,
    div: u32,
    cur_y: u32,
    cur_x: u32,
    clip_x: u32,
    line_atoms: Vec<Atom>,
    emit_atoms: Vec<Atom>,
    line_start: u32,
    /// The blockers (`mBlockList`): placed bitmaps not passed yet, then the sentinel (the right edge, endless).
    blockers: Vec<[u32; 4]>,
    tab_stops: Vec<u32>,
    cur_tab: usize,
    cur_url: Option<usize>,
    scan: usize,
    steps: u64,
    atoms: usize,
    /// Duplicate `<tag:>` ids met (each a `Con::warnf` after the reflow).
    dup_tags: u32,
}

impl Flow<'_> {
    fn font(&mut self, key: &(String, i32)) -> Option<Arc<torque_fonts::Font>> {
        if let Some(f) = self.fonts.get(key) {
            return Some(Arc::clone(f));
        }
        let f = font_of(self.vm, self.gui, &key.0, key.1)?;
        self.fonts.insert(key.clone(), Arc::clone(&f));
        Some(f)
    }

    fn style_font(&mut self, style: usize) -> Option<Arc<torque_fonts::Font>> {
        let key = self.out.styles[style].font.clone();
        self.font(&key)
    }

    /// One pass of an emitting loop, charged to the step budget; false when the reflow must stop.
    fn step(&mut self) -> bool {
        self.steps += 1;
        self.steps < MAX_REFLOW_STEPS && self.vm.charge_work(8)
    }

    /// The current style, copied first when an atom used it (`allocStyle`).
    fn style_for_change(&mut self) -> usize {
        if self.out.styles[self.cur_style].used {
            let mut s = self.out.styles[self.cur_style].clone();
            s.used = false;
            self.out.styles.push(s);
            self.cur_style = self.out.styles.len() - 1;
        }
        self.cur_style
    }

    /// `emitTextToken`.
    fn emit_text(&mut self, start: usize, len: usize) {
        if self.lmargin >= self.rmargin || self.atoms >= MAX_ATOMS {
            return;
        }
        let Some(f) = self.style_font(self.cur_style) else {
            return;
        };
        self.out.styles[self.cur_style].used = true;
        self.atoms += 1;
        self.emit_atoms.push(Atom {
            text_start: start as u32,
            len: len as u32,
            x_start: 0,
            y_start: 0,
            width: 0,
            baseline: f.baseline,
            descent: f.height.wrapping_sub(f.baseline),
            style: self.cur_style,
            is_clipped: false,
            url: self.cur_url,
        });
    }

    /// `emitNewLine`.
    fn new_line(&mut self, text_start: u32) {
        self.clip_x = 0;
        let height = self.style_font(self.cur_style).map_or(0, |f| f.height);
        let mut line = Line {
            y: self.cur_y,
            height,
            div: self.div,
            text_start: self.line_start,
            len: text_start.wrapping_sub(self.line_start),
            atoms: std::mem::take(&mut self.line_atoms),
        };
        self.line_start = text_start;
        self.cur_x = self.lmargin;
        self.cur_tab = 0;
        if !line.atoms.is_empty() {
            let (mut base, mut desc) = (0u32, 0u32);
            for a in &line.atoms {
                base = base.max(a.baseline);
                desc = desc.max(a.descent);
            }
            if let Some(last) = line.atoms.last() {
                line.len = last
                    .text_start
                    .wrapping_add(last.len)
                    .wrapping_sub(line.text_start);
                self.line_start = last.text_start.wrapping_add(last.len);
            }
            line.height = base.wrapping_add(desc);
            for a in &mut line.atoms {
                a.y_start = base.wrapping_sub(a.baseline).wrapping_add(self.cur_y);
            }
        }
        self.cur_y = self.cur_y.wrapping_add(line.height);
        if self.out.lines.len() < MAX_LINES {
            self.out.lines.push(line);
        }
        // The blockers passed (all but the sentinel at the end) leave the list.
        let y = self.cur_y;
        let last = self.blockers.len() - 1;
        let mut i = 0;
        while i < last.min(self.blockers.len() - 1) {
            let b = self.blockers[i];
            if b[1].wrapping_add(b[3]) <= y {
                self.blockers.remove(i);
            } else {
                i += 1;
            }
        }
        if self.out.max_y < y {
            self.out.max_y = y;
        }
    }

    /// `splitAtomListEmit`: the atoms that fit in `width` from `cur_x` go on the line; the rest is returned.
    fn split(&mut self, mut list: Vec<Atom>, width: u32) -> Vec<Atom> {
        let mut total: u32 = 0;
        let mut emitted: Vec<Atom> = Vec::new();
        let mut adjust_clip = false;
        let mut clip_atom: Option<usize> = None;
        let mut rest: Vec<Atom> = Vec::new();
        let mut i = 0;
        while i < list.len() {
            let mut a = list[i].clone();
            let Some(f) = self.style_font(a.style) else {
                i += 1;
                continue;
            };
            let s = a.text_start as usize;
            let seg = &self.text[s.min(self.text.len())..(s + a.len as usize).min(self.text.len())];
            let left = width.wrapping_sub(total);
            let bp;
            if self.clip_x == 0 {
                let mut b = f.break_pos(seg, left, true);
                if b == 0 {
                    b = 1;
                }
                if b < a.len as usize && seg.get(b - 1) != Some(&b' ') && !emitted.is_empty() {
                    rest = list.split_off(i);
                    break;
                }
                a.width = f.str_width_precise(&seg[..b.min(seg.len())]) as u32;
                bp = b;
            } else {
                let b = f.break_pos(seg, left, false);
                if b == 0 {
                    bp = 1;
                } else if b == a.len as usize {
                    a.width = f.str_width_precise(&seg[..b.min(seg.len())]) as u32;
                    clip_atom = Some(emitted.len());
                    bp = b;
                } else {
                    let etc = f.str_width_precise(b"...") as u32;
                    let b2 = f.break_pos(seg, left.wrapping_sub(etc), false);
                    if b2 == 0 {
                        adjust_clip = true;
                        a.len = 0;
                        a.width = 0;
                        bp = 0;
                    } else {
                        clip_atom = None;
                        a.is_clipped = true;
                        a.len = b2 as u32;
                        a.width = left;
                        bp = b2;
                    }
                }
            }
            total = total.wrapping_add(a.width);
            let whole = a.is_clipped || bp == a.len as usize;
            if whole {
                emitted.push(a);
                i += 1;
                if width < total || i >= list.len() {
                    rest = list.split_off(i);
                    break;
                }
            } else {
                let mut tail = a.clone();
                tail.text_start = a.text_start.wrapping_add(bp as u32);
                tail.len = a.len.wrapping_sub(bp as u32);
                tail.is_clipped = false;
                a.len = bp as u32;
                emitted.push(a);
                let mut r = vec![tail];
                r.extend(list.split_off(i + 1));
                rest = r;
                break;
            }
        }
        if adjust_clip && let Some(ci) = clip_atom {
            let c = emitted[ci].clone();
            if let Some(f) = self.style_font(c.style) {
                let s = c.text_start as usize;
                let seg =
                    &self.text[s.min(self.text.len())..(s + c.len as usize).min(self.text.len())];
                let etc = f.str_width_precise(b"...") as u32;
                let b = f.break_pos(seg, c.width.wrapping_sub(etc), false);
                if b != 0 {
                    emitted[ci].is_clipped = true;
                    emitted[ci].len = b as u32;
                }
            }
        }
        match self.justify {
            CENTER if total < width => self.cur_x = self.cur_x.wrapping_add((width - total) >> 1),
            RIGHT if total < width => self.cur_x = self.cur_x.wrapping_add(width - total),
            _ => {}
        }
        for mut a in emitted {
            a.x_start = self.cur_x;
            self.cur_x = self.cur_x.wrapping_add(a.width);
            self.line_atoms.push(a);
        }
        rest
    }

    /// `processEmitAtoms` (inlined at each call in the reflow).
    fn process(&mut self) {
        let mut list = std::mem::take(&mut self.emit_atoms);
        while !list.is_empty() {
            if !self.step() {
                return;
            }
            let mut bi = 0;
            let mut need_line = true;
            while bi < self.blockers.len() {
                if list.is_empty() {
                    need_line = false;
                    break;
                }
                let b = self.blockers[bi];
                let cx = self.cur_x;
                if cx < b[2].wrapping_add(b[0]) {
                    if b[0] <= cx {
                        self.cur_x = b[2].wrapping_add(b[0]);
                        bi += 1;
                        continue;
                    }
                    let mut right = b[0];
                    if self.rmargin < right {
                        right = self.rmargin;
                    }
                    if self.clip_x != 0 && self.clip_x < right {
                        right = self.clip_x;
                    }
                    if right <= cx {
                        break;
                    }
                    list = self.split(list, right - cx);
                    if !list.is_empty() {
                        self.cur_x = b[0].wrapping_add(b[2]);
                        bi += 1;
                    }
                    continue;
                }
                bi += 1;
            }
            if list.is_empty() || !need_line {
                break;
            }
            let at = self.scan as u32;
            self.new_line(at);
        }
    }

    /// `emitBitmapToken`.
    fn emit_bitmap(&mut self, name: String, size: (u32, u32), text_start: u32, brk: bool) {
        if self.lmargin >= self.rmargin || size.0 > self.rmargin - self.lmargin {
            return;
        }
        if self.out.bitmaps.len() >= MAX_BITMAPS {
            return;
        }
        let (w, h) = if self.max_bitmap_height == u32::MAX {
            size
        } else {
            let h = self.max_bitmap_height.min(size.1);
            let ratio = size.0 as f32 / size.1 as f32;
            // Truncated toward zero.
            ((h as f32 * ratio) as i64 as u32, h)
        };
        if brk {
            while self.blockers.len() > 1 && self.step() {
                self.new_line(text_start);
            }
        }
        loop {
            if !self.step() {
                return;
            }
            let mut minx = self.cur_x;
            for bi in 0..self.blockers.len() {
                let b = self.blockers[bi];
                if minx < b[0] {
                    let right = b[0].min(self.rmargin);
                    let room = right.wrapping_sub(minx);
                    if minx < right && w <= room {
                        let mut x = minx;
                        match self.justify {
                            CENTER => x = x.wrapping_add((room - w) >> 1),
                            RIGHT => x = x.wrapping_add(room - w),
                            _ => {}
                        }
                        let rect = [x, self.cur_y, w, h];
                        self.blockers.insert(bi, rect);
                        self.out.bitmaps.insert(0, BitmapRef { rect, name, size });
                        let bottom = rect[1].wrapping_add(rect[3]);
                        if self.out.max_y < bottom {
                            self.out.max_y = bottom;
                        }
                        return;
                    }
                }
                let end = b[0].wrapping_add(b[2]);
                if minx < end {
                    minx = end;
                }
            }
            self.new_line(text_start);
        }
    }
}

/// The reflow: the layout made, then the automatic size and `onResize`, then the cursor kept in view.
pub fn reflow(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    let Some(_) = state(gui, id) else {
        return Ok(());
    };
    with_state(gui, id, |s| s.dirty = false);
    let text = buffer(gui, id);
    if !vm.charge_work(text.len() as u64) {
        return Ok(());
    }
    let profile = profile_of(vm, id);
    let face = profile
        .map(|p| String::from_utf8_lossy(&text_field(vm, p, b"fontType")).into_owned())
        .unwrap_or_default();
    let size = profile.map_or(0, |p| int(vm, p, b"fontSize"));
    let (_, ext) = bounds(vm, id);
    let width = ext.0 as u32;
    let mbh = gui.ext().ml_max_bitmap_height(vm, id).unwrap_or(-1);
    let layout = {
        let vmr: &Vm = vm;
        if font_of(vmr, gui, &face, size).is_none() {
            None
        } else {
            let color = profile.map_or([0, 0, 0, 255], |p| crate::profile::font_color(vmr, p, 0));
            let first = Style {
                color,
                shadow_color: color,
                link_color: profile.map_or([0; 4], |p| crate::profile::font_color(vmr, p, 4)),
                link_color_hl: profile.map_or([0; 4], |p| crate::profile::font_color(vmr, p, 5)),
                shadow_offset: (0, 0),
                font: (face, size),
                used: false,
                next: None,
            };
            let mut f = Flow {
                vm: vmr,
                gui,
                text: &text,
                width,
                max_bitmap_height: if mbh == -1 { u32::MAX } else { mbh as u32 },
                out: MlLayout {
                    styles: vec![first],
                    ..MlLayout::default()
                },
                fonts: HashMap::new(),
                cur_style: 0,
                lmargin: 0,
                rmargin: width,
                justify: LEFT,
                div: 0,
                cur_y: 0,
                cur_x: 0,
                clip_x: 0,
                line_atoms: Vec::new(),
                emit_atoms: Vec::new(),
                line_start: 0,
                blockers: vec![[width, 0, 0, 0x7f_ffff]],
                tab_stops: Vec::new(),
                cur_tab: 0,
                cur_url: None,
                scan: 0,
                steps: 0,
                atoms: 0,
                dup_tags: 0,
            };
            run_flow(&mut f);
            Some((f.out, f.dup_tags))
        }
    };
    // No profile font: the old lines go and nothing is laid out.
    let Some((layout, dups)) = layout else {
        with_state(gui, id, |s| s.layout = Rc::new(MlLayout::default()));
        return Ok(());
    };
    for _ in 0..dups.min(64) {
        vm.warn("GuiMLTextCtrl - can't add duplicate line tags!".to_string());
    }
    let max_y = layout.max_y;
    with_state(gui, id, |s| s.layout = Rc::new(layout));
    if gui.ext().ml_auto_resize(vm, id) {
        let (pos, ext) = bounds(vm, id);
        crate::control::resize(vm, gui, id, pos, (ext.0, max_y as i32));
        vm.call_method(
            id,
            b"onResize",
            &[
                ext.0.to_string().into_bytes(),
                max_y.to_string().into_bytes(),
            ],
        )?;
    }
    if vm.objects().contains(id) {
        ensure_cursor_visible(vm, gui, id);
    }
    Ok(())
}

fn text_field(vm: &Vm, id: ObjId, name: &[u8]) -> Vec<u8> {
    text(vm, id, name)
}

/// The tag-and-text scan of the reflow.
fn run_flow(f: &mut Flow) {
    let s = f.text;
    loop {
        if !f.step() {
            break;
        }
        let c = s.get(f.scan).copied().unwrap_or(0);
        if c == 0 {
            break;
        }
        if c == b'\n' {
            let start = f.scan as u32;
            f.scan += 1;
            f.process();
            f.new_line(start);
            f.div = 0;
            continue;
        }
        if c == b'\t' {
            f.scan += 1;
            f.process();
            if !f.tab_stops.is_empty() {
                if f.cur_tab < f.tab_stops.len() && f.cur_x < f.tab_stops[f.cur_tab] {
                    f.cur_x = f.tab_stops[f.cur_tab];
                }
                f.cur_tab += 1;
            }
            continue;
        }
        if c == b'<' && tag(f) {
            continue;
        }
        // The text up to the next tab, `<`, newline or end.
        let start = f.scan;
        let mut i = 1;
        while let Some(&ch) = s.get(start + i) {
            if ch == b'\t' || ch == b'<' || ch == b'\n' || ch == 0 {
                break;
            }
            i += 1;
        }
        f.scan += i;
        f.emit_text(start, i);
    }
    f.process();
    let at = f.scan as u32;
    f.new_line(at);
}

/// One tag at `f.scan` (a `<`): true when it was taken as a tag.
fn tag(f: &mut Flow) -> bool {
    let full = f.text;
    let s = &full[f.scan..];
    let mut at = 1;
    if scan_for_any(s, &mut at, b'<') {
        return false;
    }
    let ci = |p: &[u8]| starts_with_ci(s, 1, p);
    if ci(b"br>") {
        let start = f.scan as u32 + 8;
        f.scan += 4;
        f.process();
        f.new_line(start);
        f.div = 0;
        return true;
    }
    if ci(b"font:") {
        let mut idx = 6;
        if !scan_for(s, &mut idx, b':') {
            return false;
        }
        let mut sz = idx + 1;
        if !scan_for(s, &mut sz, b'>') {
            return false;
        }
        let size = atoi(&s[idx + 1..]) as u32;
        if size == 0 || size > 128 {
            return false;
        }
        let face = String::from_utf8_lossy(&s[6..idx]).into_owned();
        f.scan += sz + 1;
        if f.font(&(face.clone(), size as i32)).is_some() {
            let st = f.style_for_change();
            f.out.styles[st].font = (face, size as i32);
        }
        return true;
    }
    if ci(b"tag:") {
        let mut idx = 5;
        if !scan_for(s, &mut idx, b'>') {
            return false;
        }
        let id = atoi(&s[5..]) as u32;
        if f.out.tags.iter().any(|t| t.0 == id) {
            // `Con::warnf`: printed after the reflow.
            f.dup_tags = f.dup_tags.saturating_add(1);
            f.scan += idx + 1;
            return true;
        }
        if f.out.tags.len() < MAX_LINES {
            f.out.tags.insert(0, (id, f.cur_y));
        }
        f.scan += idx + 1;
        return true;
    }
    for (name, field) in [
        (&b"color:"[..], 0),
        (b"shadowcolor:", 1),
        (b"linkcolor:", 2),
        (b"linkcolorhl:", 3),
    ] {
        if ci(name) {
            let at = 1 + name.len();
            let mut idx = at;
            if !scan_for(s, &mut idx, b'>') {
                return false;
            }
            if idx != at + 6 && idx != at + 8 {
                return false;
            }
            let color = tag_color(s, at, idx == at + 8);
            f.scan += idx + 1;
            let st = f.style_for_change();
            let style = &mut f.out.styles[st];
            match field {
                0 => style.color = color,
                1 => style.shadow_color = color,
                2 => style.link_color = color,
                _ => style.link_color_hl = color,
            }
            return true;
        }
    }
    if ci(b"shadow:") {
        let mut idx = 8;
        if !scan_for(s, &mut idx, b':') {
            return false;
        }
        let mut y = idx + 1;
        if !scan_for(s, &mut y, b'>') {
            return false;
        }
        f.scan += y + 1;
        let off = (atoi(&s[8..]), atoi(&s[idx + 1..]));
        let st = f.style_for_change();
        f.out.styles[st].shadow_offset = off;
        return true;
    }
    if ci(b"bitmap") {
        let (start, brk) = if s.get(7) == Some(&b'k') && s.get(8) == Some(&b':') {
            (9, true)
        } else if s.get(7) == Some(&b':') {
            (8, false)
        } else {
            return false;
        };
        let mut idx = start;
        if !scan_for(s, &mut idx, b'>') {
            return false;
        }
        let text_start = (f.scan + start) as u32;
        let len = (idx - start).min(256);
        let name = String::from_utf8_lossy(&s[start..start + len]).into_owned();
        f.scan += idx + 1;
        f.process();
        let found = bitmap_size(f.vm, f.gui, &name)
            .map(|sz| (name.clone(), sz))
            .or_else(|| {
                let missing = f.gui.ext().missing_bitmap()?;
                bitmap_size(f.vm, f.gui, missing).map(|sz| (missing.to_string(), sz))
            });
        if let Some((n, sz)) = found {
            f.emit_bitmap(n, sz, text_start, brk);
        }
        return true;
    }
    if ci(b"spush>") {
        f.scan += 7;
        let mut st = f.out.styles[f.cur_style].clone();
        st.used = false;
        st.next = Some(f.cur_style);
        f.out.styles.push(st);
        f.cur_style = f.out.styles.len() - 1;
        return true;
    }
    if ci(b"spop>") {
        f.scan += 6;
        if let Some(n) = f.out.styles[f.cur_style].next {
            f.cur_style = n;
        }
        return true;
    }
    if ci(b"sbreak>") {
        f.scan += 8;
        f.process();
        while f.blockers.len() > 1 && f.step() {
            let at = f.scan as u32;
            f.new_line(at);
        }
        return true;
    }
    for (name, j) in [
        (&b"just:left>"[..], LEFT),
        (b"just:right>", RIGHT),
        (b"just:center>", CENTER),
    ] {
        if ci(name) {
            f.process();
            f.justify = j;
            f.scan += 1 + name.len();
            return true;
        }
    }
    if ci(b"a:") {
        let mut idx = 3;
        if !scan_for_any(s, &mut idx, b'>') {
            return false;
        }
        let no_underline = starts_with_ci(s, 3, b"gamelink");
        f.out.urls.push(Url {
            mouse_down: false,
            text_start: (f.scan + 3) as u32,
            len: (idx - 3) as u32,
            no_underline,
        });
        f.cur_url = Some(f.out.urls.len() - 1);
        f.scan += idx + 1;
        return true;
    }
    if ci(b"/a>") {
        f.cur_url = None;
        f.scan += 4;
        return true;
    }
    let width = f.width as i32;
    let lmargin = if ci(b"lmargin%:") {
        let mut idx = 10;
        if !scan_for(s, &mut idx, b'>') {
            return false;
        }
        let pct = atoi(&s[10..]);
        f.scan += idx + 1;
        Some(pct.wrapping_mul(width) / 100)
    } else if ci(b"lmargin:") {
        let mut idx = 9;
        if !scan_for(s, &mut idx, b'>') {
            return false;
        }
        f.scan += idx + 1;
        Some(atoi(&s[9..]))
    } else {
        None
    };
    if let Some(m) = lmargin {
        let m = m.min(width.wrapping_sub(10)).max(0) as u32;
        f.process();
        let old = f.lmargin;
        let mut m = m;
        if f.width <= m {
            m = f.width.wrapping_sub(1);
        }
        f.lmargin = m;
        if f.cur_x == old || f.cur_x < m {
            f.cur_x = m;
        }
        return true;
    }
    let rmargin = if ci(b"rmargin%:") {
        let mut idx = 10;
        if !scan_for(s, &mut idx, b'>') {
            return false;
        }
        let pct = atoi(&s[10..]);
        f.scan += idx + 1;
        Some((pct.wrapping_mul(width) / 100).min(width))
    } else if ci(b"rmargin:") {
        let mut idx = 9;
        if !scan_for(s, &mut idx, b'>') {
            return false;
        }
        f.scan += idx + 1;
        Some(atoi(&s[9..]).min(width))
    } else {
        None
    };
    if let Some(m) = rmargin {
        let m = m.max(0) as u32;
        f.process();
        f.rmargin = m;
        if f.width <= f.lmargin {
            f.lmargin = f.width;
        }
        if m < f.clip_x {
            f.clip_x = m;
        }
        return true;
    }
    if ci(b"clip:") {
        let mut idx = 6;
        if !scan_for(s, &mut idx, b'>') {
            return false;
        }
        let w = atoi(&s[6..]) as u32;
        f.scan += idx + 1;
        f.process();
        f.clip_x = if w == 0 { 0 } else { f.cur_x.wrapping_add(w) };
        if f.rmargin < f.clip_x {
            f.clip_x = f.rmargin;
        }
        return true;
    }
    if ci(b"/clip>") {
        f.process();
        f.scan += 7;
        f.clip_x = 0;
        return true;
    }
    if ci(b"div:") {
        let mut idx = 5;
        if !scan_for(s, &mut idx, b'>') {
            return false;
        }
        f.scan += idx + 1;
        f.div = atoi(&s[5..]) as u32;
        return true;
    }
    if ci(b"tab:") {
        let mut idx = 5;
        if !scan_for(s, &mut idx, b'>') {
            return false;
        }
        let mut stops = Vec::new();
        let mut idx = 5;
        stops.push(atoi(&s[5..]) as u32);
        while stops.len() < 1024 && scan_for(s, &mut idx, b',') {
            idx += 1;
            stops.push(atoi(&s[idx..]) as u32);
        }
        // The stops are counted in a first pass the same way, then parsed; the scan position ends past the last one.
        f.tab_stops = stops;
        f.scan += idx + 1;
        return true;
    }
    false
}

/// `getCursorPositionAndColor`'s top and bottom (TGE 1.4): the line of the cursor (the first line it is inside, else the last).
pub(crate) fn cursor_rect(vm: &Vm, gui: &Gui, id: ObjId) -> (i32, i32, i32) {
    let Some(s) = state(gui, id) else {
        return (0, 0, 0);
    };
    let mut y = 0i32;
    let mut height = profile_of(vm, id)
        .and_then(|p| {
            let face = String::from_utf8_lossy(&text(vm, p, b"fontType")).into_owned();
            font_of(vm, gui, &face, int(vm, p, b"fontSize"))
        })
        .map_or(0, |f| f.height as i32);
    let mut x = 0i32;
    let n = s.layout.lines.len();
    for (i, line) in s.layout.lines.iter().enumerate() {
        if s.cursor <= line.text_start.wrapping_add(line.len) || i + 1 == n {
            y = line.y as i32;
            height = line.height as i32;
            for a in &line.atoms {
                if s.cursor < a.text_start {
                    x = a.x_start as i32;
                    break;
                }
                if s.cursor > a.text_start.wrapping_add(a.len) {
                    x = a.x_start.wrapping_add(a.width) as i32;
                    continue;
                }
                x = a.x_start as i32;
                break;
            }
            break;
        }
    }
    (x, y, y.wrapping_add(height))
}

/// `ensureCursorOnScreen`.
fn ensure_cursor_visible(vm: &mut Vm, gui: &Gui, id: ObjId) {
    let Some(p) = parent(vm, id).filter(|p| is_a(vm, *p, "GuiScrollCtrl")) else {
        return;
    };
    let (x, top, bottom) = cursor_rect(vm, gui, id);
    crate::scroll::scroll_rect_visible(vm, gui, p, [x, top, 1, bottom.wrapping_sub(top)]);
}

/// `GuiControl::preRender` (TGE 1.4) of `root`: every awake control below it, children first, then its own `onPreRender`: a dirty
/// GuiMLTextCtrl reflows, a GuiConsole follows the log, a GuiConsoleTextCtrl evaluates its expression,
/// the first-responder GuiTextEditCtrl's caret blinks.
/// The order is made first (bounded); a control a callback put to sleep or deleted is skipped.
pub fn pre_render(vm: &mut Vm, gui: &Gui, root: ObjId) -> Result<(), VmError> {
    let mut order = Vec::new();
    let mut stack = vec![(root, 0usize, false)];
    while let Some((id, depth, done)) = stack.pop() {
        if !vm.charge_work(16) || order.len() >= crate::MAX_CHILDREN_WALKED {
            break;
        }
        if done {
            order.push(id);
            continue;
        }
        if !gui.is_awake(id) {
            continue;
        }
        stack.push((id, depth, true));
        if depth < crate::MAX_TREE_DEPTH {
            for c in crate::control::children(vm, id).into_iter().rev() {
                stack.push((c, depth + 1, false));
            }
        }
    }
    for id in order {
        if !vm.objects().contains(id) || !gui.is_awake(id) {
            continue;
        }
        if gui.with(id, |c| c.ml.as_ref().is_some_and(|s| s.dirty)) == Some(true) {
            reflow(vm, gui, id)?;
        } else if is_a(vm, id, "GuiConsole") {
            crate::console::pre_render_console(vm, gui, id);
        } else if is_a(vm, id, "GuiConsoleTextCtrl") {
            crate::console::pre_render_text(vm, gui, id)?;
        } else if is_a(vm, id, "GuiTextEditCtrl")
            && crate::input::canvas_first_responder(gui) == Some(id)
        {
            // The first responder's caret blinks.
            crate::input::caret_frame(vm, gui, id);
        }
    }
    Ok(())
}

/// A link's atom: the link's index, its URL and its rectangle on the canvas ([`link_spans`]).
pub type LinkSpan = (usize, Vec<u8>, [i32; 4]);

/// Every link's atoms (a control's lines reflowed first when dirty): the link's index, its URL (the `<a:...>` text) and the atom's
/// rectangle `[x, y, w, h]` in canvas coordinates cut to what the control's parents show (a scroll's view: empty when the atom is
/// scrolled away); a link wrapped over lines has an atom on each. For hosts and tests.
pub fn link_spans(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<Vec<LinkSpan>, VmError> {
    if state(gui, id).is_none_or(|s| s.dirty) {
        reflow(vm, gui, id)?;
    }
    let Some(s) = state(gui, id) else {
        return Ok(Vec::new());
    };
    let text = buffer(gui, id);
    let at = crate::control::screen_position(vm, id);
    // What the parents show: the intersection of their screen rectangles (at most 64 levels).
    let mut clip = [i32::MIN / 4, i32::MIN / 4, i32::MAX / 4, i32::MAX / 4];
    let mut p = crate::control::parent(vm, id);
    for _ in 0..64 {
        let Some(c) = p else { break };
        let o = crate::control::screen_position(vm, c);
        let e = bounds(vm, c).1;
        clip = [
            clip[0].max(o.0),
            clip[1].max(o.1),
            clip[2].min(o.0.saturating_add(e.0)),
            clip[3].min(o.1.saturating_add(e.1)),
        ];
        p = crate::control::parent(vm, c);
    }
    let mut out = Vec::new();
    for line in &s.layout.lines {
        for a in &line.atoms {
            let Some(u) = a.url else { continue };
            let Some(url) = s.layout.urls.get(u) else {
                continue;
            };
            let start = (url.text_start as usize).min(text.len());
            let end = (start + url.len as usize).min(text.len());
            let (x0, y0) = (at.0 + a.x_start as i32, at.1 + line.y as i32);
            let (x1, y1) = (x0 + a.width as i32, y0 + line.height as i32);
            let (cx0, cy0, cx1, cy1) = (
                x0.max(clip[0]),
                y0.max(clip[1]),
                x1.min(clip[2]),
                y1.min(clip[3]),
            );
            out.push((
                u,
                text[start..end].to_vec(),
                [cx0, cy0, (cx1 - cx0).max(0), (cy1 - cy0).max(0)],
            ));
        }
    }
    Ok(out)
}

/// `findHitAtom`: the atom under a point in the control (none outside the lines).
fn hit_atom(layout: &MlLayout, p: (i32, i32)) -> Option<&Atom> {
    for line in &layout.lines {
        if (p.1 as u32) < line.y {
            return None;
        }
        if p.1 >= line.y as i32 && p.1 < line.y.wrapping_add(line.height) as i32 {
            for a in &line.atoms {
                if p.0 < a.x_start as i32 {
                    return None;
                }
                if p.0 >= a.x_start.wrapping_add(a.width) as i32 {
                    continue;
                }
                return Some(a);
            }
        }
    }
    None
}

/// `onMouseDown` (TGE 1.4): active: the link under the point pressed, the mouse locked, the selection's anchor.
pub fn mouse_down(vm: &mut Vm, gui: &Gui, id: ObjId, pt: (i32, i32)) -> Result<(), VmError> {
    if !crate::input::active(gui, id) {
        return Ok(());
    }
    if state(gui, id).is_some_and(|s| s.dirty) {
        reflow(vm, gui, id)?;
    }
    let p = crate::scroll::local(vm, id, pt);
    let hit = state(gui, id).and_then(|s| hit_atom(&s.layout, p).and_then(|a| a.url));
    if let Some(u) = hit {
        crate::input::mouse_lock(vm, gui, id)?;
        with_state(gui, id, |s| {
            let mut l = (*s.layout).clone();
            if let Some(url) = l.urls.get_mut(u) {
                url.mouse_down = true;
            }
            s.layout = Rc::new(l);
            s.hit_url = Some(u);
        });
    } else {
        with_state(gui, id, |s| s.hit_url = None);
    }
    crate::input::set_first_responder(vm, gui, id)?;
    crate::input::mouse_lock(vm, gui, id)?;
    with_state(gui, id, |s| {
        s.sel_active = false;
        s.anchor_point = pt;
    });
    Ok(())
}

/// `onMouseUp` (TGE 1.4): a press released on the same link calls the script's `onURL(url)`.
pub fn mouse_up(vm: &mut Vm, gui: &Gui, id: ObjId, pt: (i32, i32)) -> Result<(), VmError> {
    if !crate::input::active(gui, id) || gui.0.borrow().input.capture != Some(id) {
        return Ok(());
    }
    crate::input::mouse_unlock(vm, gui, id)?;
    let p = crate::scroll::local(vm, id, pt);
    let Some(s) = state(gui, id) else {
        return Ok(());
    };
    let under = hit_atom(&s.layout, p).and_then(|a| a.url);
    if let Some(u) = s.hit_url
        && under == Some(u)
        && s.layout.urls.get(u).is_some_and(|url| url.mouse_down)
    {
        let url = s.layout.urls[u];
        let text = buffer(gui, id);
        let a = (url.text_start as usize).min(text.len());
        let b = (a + url.len as usize).min(text.len());
        let arg = text[a..b].to_vec();
        with_state(gui, id, |s| {
            let mut l = (*s.layout).clone();
            if let Some(x) = l.urls.get_mut(u) {
                x.mouse_down = false;
            }
            s.layout = Rc::new(l);
            s.hit_url = None;
        });
        vm.call_method(id, b"onURL", &[arg])?;
        return Ok(());
    }
    Ok(())
}

#[cfg(test)]
#[path = "mltext_tests.rs"]
mod tests;
