//! A TrueType face a host can serve through a [`crate::Fallback`]. [`TtfFace::parse`] checks the file, then `fontdue` reads it;
//! [`TtfFace::make`] makes a [`Font`] at a size with the metrics TGE gets from GDI, as `WinFont` and `GFont::create` make one
//! (`GFont::create` is carried into the MIT-licensed Torque3D's `gfx/gFont.cpp`, Copyright (c) 2012 GarageGames, LLC;
//! `WinFont` follows TGE's `platformWin32/winFont.cc`, TGE 1.4, Copyright (C) GarageGames.com, Inc.; the Torque engine is
//! MIT-licensed as Torque3D, Copyright (c) 2012 GarageGames, LLC):
//!
//! - **The cell.** `CreateFontA` with a positive height asks for that CELL height: GDI scales the face so `usWinAscent +
//!   usWinDescent` (the OS/2 table; hhea's ascender and descender without one) is `size` pixels. The font's height is `size`, its
//!   baseline the scaled ascent, rounded.
//! - **The glyphs** of the ANSI charset (Windows-1252, the characters of bytes 32-255), each anti-aliased coverage at that scale,
//!   remapped from their Unicode code points; a character the face has no glyph for has none (draws nothing, advances nothing).
//!   Advances are rounded to whole pixels (`xIncrement` is a byte).
//! - **The sheets** (`GFont::create`'s packing): the glyphs sorted by height, tallest first, laid in rows with a 1-pixel gap on
//!   256-wide sheets, a new sheet when one is full; drawn `GL_NEAREST` one texel a pixel, as every font sheet.
//!
//! **Hostile input.** The file comes from outside the program: [`check`] reads the table directory and the tables `fontdue` and
//! `ttf-parser` walk eagerly before either sees the bytes, and refuses what could make them work without bound (a composite glyph
//! tree whose expansion is exponential, a cmap that enumerates billions of code points or scans its groups per code point, a kern
//! table that loops) or outlines this module does not take (CFF, collections). Every size and count is capped; a glyph larger than
//! a byte's 255 pixels is left out. No cache file is ever written.

use std::sync::Arc;

use crate::{Bitmap, Font, Glyph, Remap, Tab, Units};

/// The largest file read (4 MiB).
pub const MAX_TTF_BYTES: usize = 4 << 20;
/// The sizes a face is made at (`fontSize`), both included.
pub const MIN_SIZE: i32 = 6;
pub const MAX_SIZE: i32 = 96;
/// The most tables a file's directory may list.
pub const MAX_TABLES: u16 = 64;
/// The most glyphs a face may have (`maxp.numGlyphs`; Inter has about 2,900).
pub const MAX_FACE_GLYPHS: u16 = 32_768;
/// The most code points the cmap subtables may enumerate together (fontdue walks every one).
pub const MAX_CMAP_CODEPOINTS: u64 = 1 << 18;
/// The most cmap subtables.
pub const MAX_CMAP_SUBTABLES: u16 = 64;
/// The most outline points (composite components expanded, as `ttf-parser` walks them) of every glyph fontdue loads, together.
pub const MAX_OUTLINE_POINTS: u64 = 1 << 21;
/// The most components one composite glyph lists.
pub const MAX_COMPONENTS: usize = 4_096;
/// `ttf-parser`'s composite depth limit (`glyf::MAX_COMPONENTS`): at this depth a glyph has no outline.
const COMPOSITE_DEPTH: u8 = 32;
/// The most line segments fontdue may make of the loaded glyphs' curves together (64 bytes each: 128 MiB at most). Its curve
/// subdivision depends on each curve's size against `6 * unitsPerEm / FONTDUE_SCALE`, so [`check`] runs the same subdivision,
/// counting only, and stops at this many.
pub const MAX_LINES: u64 = 1 << 21;
/// fontdue's `FontSettings::scale` (its default), which sets how finely it subdivides curves.
pub const FONTDUE_SCALE: f32 = 40.0;
/// The largest glyph bitmap kept, against the size: twice the cell (a glyph past it draws nothing but keeps its advance).
pub const MAX_GLYPH_CELLS: u32 = 2;
/// The most sheets one made font has (a font at 96 needs about 20); later glyphs draw nothing.
pub const MAX_FONT_SHEETS: usize = 32;
/// The most kern subtables (fontdue's loop count).
pub const MAX_KERN_SUBTABLES: u32 = 64;
/// The sheet width (and the tallest sheet) `GFont::create` packs into.
pub const SHEET: u32 = 256;

/// Why a file was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TtfError {
    /// A size or count above a named cap.
    Cap {
        what: &'static str,
        value: u64,
        cap: u64,
    },
    /// The file is not what this module reads (the reason).
    Unsupported(&'static str),
    /// A table is too short or points outside the file.
    Malformed(&'static str),
    /// fontdue did not read it.
    Parse(String),
}

impl std::fmt::Display for TtfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TtfError::Cap { what, value, cap } => {
                write!(f, "{what}: {value} is above the limit of {cap}")
            }
            TtfError::Unsupported(w) => write!(f, "not supported: {w}"),
            TtfError::Malformed(w) => write!(f, "malformed: {w}"),
            TtfError::Parse(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TtfError {}

fn cap(what: &'static str, value: u64, limit: u64) -> Result<(), TtfError> {
    if value > limit {
        Err(TtfError::Cap {
            what,
            value,
            cap: limit,
        })
    } else {
        Ok(())
    }
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_be_bytes([s[0], s[1]]))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// What [`check`] read: the metrics the cell needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checked {
    pub units_per_em: u16,
    /// The cell's ascent and descent in font units (both positive).
    pub ascent: u32,
    pub descent: u32,
    pub glyphs: u16,
    /// Outline points of the loaded glyphs (expanded), and the line segments fontdue makes of them.
    pub points: u64,
    pub lines: u64,
    /// Where the kern table starts (fontdue is given a copy with it turned off: this crate does no kerning).
    pub kern_at: Option<usize>,
}

/// The table directory, sorted by tag (checked).
struct Tables<'a> {
    file: &'a [u8],
    records: Vec<([u8; 4], usize, usize)>,
}

impl<'a> Tables<'a> {
    fn get(&self, tag: &[u8; 4]) -> Option<&'a [u8]> {
        self.records
            .iter()
            .find(|(t, _, _)| t == tag)
            .and_then(|&(_, off, len)| self.file.get(off..off + len))
    }
}

/// Checks a TrueType file before any parser walks it (see the module's notes).
pub fn check(bytes: &[u8]) -> Result<Checked, TtfError> {
    cap("file bytes", bytes.len() as u64, MAX_TTF_BYTES as u64)?;
    let version = be32(bytes, 0).ok_or(TtfError::Malformed("header"))?;
    if version != 0x0001_0000 && version != u32::from_be_bytes(*b"true") {
        return Err(TtfError::Unsupported(
            "not a single TrueType face (OpenType CFF, a collection, WOFF)",
        ));
    }
    let count = be16(bytes, 4).ok_or(TtfError::Malformed("header"))?;
    cap("tables", u64::from(count), u64::from(MAX_TABLES))?;
    let mut records = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let at = 12 + 16 * i;
        let tag: [u8; 4] = bytes
            .get(at..at + 4)
            .and_then(|s| s.try_into().ok())
            .ok_or(TtfError::Malformed("table directory"))?;
        let off = be32(bytes, at + 8).ok_or(TtfError::Malformed("table directory"))? as usize;
        let len = be32(bytes, at + 12).ok_or(TtfError::Malformed("table directory"))? as usize;
        if off.checked_add(len).is_none_or(|end| end > bytes.len()) {
            return Err(TtfError::Malformed("a table outside the file"));
        }
        // ttf-parser finds a table by binary search: only a sorted, duplicate-free directory finds what this check read.
        if records
            .last()
            .is_some_and(|(prev, _, _): &([u8; 4], usize, usize)| *prev >= tag)
        {
            return Err(TtfError::Malformed("the table directory is not sorted"));
        }
        records.push((tag, off, len));
    }
    let t = Tables {
        file: bytes,
        records,
    };
    if t.get(b"CFF ").is_some() || t.get(b"CFF2").is_some() {
        return Err(TtfError::Unsupported("CFF outlines"));
    }
    let head = t.get(b"head").ok_or(TtfError::Malformed("no head"))?;
    let hhea = t.get(b"hhea").ok_or(TtfError::Malformed("no hhea"))?;
    let maxp = t.get(b"maxp").ok_or(TtfError::Malformed("no maxp"))?;
    let loca = t.get(b"loca").ok_or(TtfError::Malformed("no loca"))?;
    let glyf = t.get(b"glyf").ok_or(TtfError::Malformed("no glyf"))?;
    let cmap = t.get(b"cmap").ok_or(TtfError::Malformed("no cmap"))?;
    t.get(b"hmtx").ok_or(TtfError::Malformed("no hmtx"))?;
    if head.len() < 54 {
        return Err(TtfError::Malformed("head"));
    }
    let upem = be16(head, 18).ok_or(TtfError::Malformed("head"))?;
    if !(16..=16384).contains(&upem) {
        return Err(TtfError::Malformed("unitsPerEm"));
    }
    let long_loca = match be16(head, 50) {
        Some(0) => false,
        Some(1) => true,
        _ => return Err(TtfError::Malformed("indexToLocFormat")),
    };
    let glyphs = be16(maxp, 4).ok_or(TtfError::Malformed("maxp"))?;
    if glyphs == 0 {
        return Err(TtfError::Malformed("no glyphs"));
    }
    cap("glyphs", u64::from(glyphs), u64::from(MAX_FACE_GLYPHS))?;
    let (ascent, descent) = cell_metrics(t.get(b"OS/2"), hhea)?;
    if ascent + descent == 0 || ascent + descent > 8 * u32::from(upem) {
        return Err(TtfError::Malformed("the cell height"));
    }
    let kern_at = t
        .records
        .iter()
        .find(|(tag, _, _)| tag == b"kern")
        .map(|&(_, off, _)| off);
    if let Some(kern) = t.get(b"kern") {
        check_kern(kern)?;
    }
    let mapped = check_cmap(cmap, glyphs)?;
    let outlines = Outlines {
        loca,
        glyf,
        long: long_loca,
        glyphs,
        memo: vec![u64::MAX; glyphs as usize * (COMPOSITE_DEPTH as usize + 1)],
    };
    let mut outlines = outlines;
    let mut points = 0u64;
    for &g in &mapped {
        points = points.saturating_add(outlines.cost(g, 0)?);
        cap("outline points", points, MAX_OUTLINE_POINTS)?;
    }
    // fontdue's curve subdivision, counted (ttf-parser walks at most `points` points now).
    let face = ttf_parser::Face::parse(bytes, 0).map_err(|e| TtfError::Parse(e.to_string()))?;
    let mut lines = LineCount {
        max_area: 3.0 * 2.0 * (f32::from(upem) / FONTDUE_SCALE),
        prev: (0.0, 0.0),
        start: (0.0, 0.0),
        lines: 0,
    };
    for &g in &mapped {
        face.outline_glyph(ttf_parser::GlyphId(g), &mut lines);
        cap("curve segments", lines.lines, MAX_LINES)?;
    }
    Ok(Checked {
        units_per_em: upem,
        ascent,
        descent,
        glyphs,
        points,
        lines: lines.lines,
        kern_at,
    })
}

/// fontdue's `Geometry` (0.9.5 `math.rs`) as an outline builder that only counts the lines it would push, and stops subdividing once
/// past [`MAX_LINES`] (so the count itself is bounded).
struct LineCount {
    max_area: f32,
    prev: (f32, f32),
    start: (f32, f32),
    lines: u64,
}

impl LineCount {
    fn push(&mut self, a: (f32, f32), b: (f32, f32)) {
        if a.1.to_bits() != b.1.to_bits() {
            self.lines += 1;
        }
    }

    /// The subdivision of a curve given by `point(t)`, from `a` to `c`.
    fn subdivide(&mut self, a: (f32, f32), c: (f32, f32), point: impl Fn(f32) -> (f32, f32)) {
        let mut stack = vec![(a, 0.0f32, c, 1.0f32)];
        while let Some((a, at, c, ct)) = stack.pop() {
            if self.lines > MAX_LINES {
                return;
            }
            let bt = (at + ct) * 0.5;
            let b = point(bt);
            let area = (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1);
            if area.abs() > self.max_area {
                stack.push((a, at, b, bt));
                stack.push((b, bt, c, ct));
            } else {
                self.push(a, c);
            }
        }
    }
}

impl ttf_parser::OutlineBuilder for LineCount {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = (x, y);
        self.prev = (x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.push(self.prev, (x, y));
        self.prev = (x, y);
    }

    fn quad_to(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let (a, b, c) = (self.prev, (x0, y0), (x1, y1));
        self.subdivide(a, c, |t| {
            let tm = 1.0 - t;
            let (ka, kb, kc) = (tm * tm, 2.0 * tm * t, t * t);
            (
                ka * a.0 + kb * b.0 + kc * c.0,
                ka * a.1 + kb * b.1 + kc * c.1,
            )
        });
        self.prev = c;
    }

    fn curve_to(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, x2: f32, y2: f32) {
        let (a, b, c, d) = (self.prev, (x0, y0), (x1, y1), (x2, y2));
        self.subdivide(a, d, |t| {
            let tm = 1.0 - t;
            let (ka, kb, kc, kd) = (tm * tm * tm, 3.0 * tm * tm * t, 3.0 * tm * t * t, t * t * t);
            (
                ka * a.0 + kb * b.0 + kc * c.0 + kd * d.0,
                ka * a.1 + kb * b.1 + kc * c.1 + kd * d.1,
            )
        });
        self.prev = d;
    }

    fn close(&mut self) {
        if self.start != self.prev {
            self.push(self.prev, self.start);
        }
        self.prev = self.start;
    }
}

/// GDI's cell: OS/2 `usWinAscent` and `usWinDescent` (a table of 78 bytes or more), else hhea's ascender and minus its descender.
fn cell_metrics(os2: Option<&[u8]>, hhea: &[u8]) -> Result<(u32, u32), TtfError> {
    if let Some(os2) = os2
        && os2.len() >= 78
    {
        let a = be16(os2, 74).ok_or(TtfError::Malformed("OS/2"))?;
        let d = be16(os2, 76).ok_or(TtfError::Malformed("OS/2"))?;
        return Ok((u32::from(a), u32::from(d)));
    }
    let a = be16(hhea, 4).ok_or(TtfError::Malformed("hhea"))? as i16;
    let d = be16(hhea, 6).ok_or(TtfError::Malformed("hhea"))? as i16;
    Ok((i32::from(a).max(0) as u32, (-i32::from(d)).max(0) as u32))
}

/// fontdue's kern reader loops over the subtable count the header gives (a u32 for Apple's version), re-reading a subtable whose
/// length is 0: the count is capped.
fn check_kern(kern: &[u8]) -> Result<(), TtfError> {
    let n = match be16(kern, 0) {
        Some(0) => u32::from(be16(kern, 2).unwrap_or(0)),
        Some(1) => be32(kern, 4).unwrap_or(0),
        _ => 0,
    };
    cap(
        "kern subtables",
        u64::from(n),
        u64::from(MAX_KERN_SUBTABLES),
    )
}

/// The glyphs fontdue loads (glyph 0 and every glyph a cmap subtable maps a code point to), after capping what fontdue's cmap walk
/// costs: the code points every subtable enumerates (formats 0, 4, 6, 10, 12; 8 and 14 enumerate none), and refusing formats 2 and
/// 13 (13's lookup scans every group for each code point).
fn check_cmap(cmap: &[u8], glyphs: u16) -> Result<Vec<u16>, TtfError> {
    let n = be16(cmap, 2).ok_or(TtfError::Malformed("cmap"))?;
    cap(
        "cmap subtables",
        u64::from(n),
        u64::from(MAX_CMAP_SUBTABLES),
    )?;
    let mut total = 0u64;
    let mut subtables = Vec::new();
    for i in 0..n as usize {
        let Some(off) = be32(cmap, 4 + 8 * i + 4) else {
            break;
        };
        let Some(sub) = cmap.get(off as usize..) else {
            continue;
        };
        let Some(format) = be16(sub, 0) else {
            continue;
        };
        let count = match format {
            0 => 256,
            4 => format4_codepoints(sub).unwrap_or(0),
            6 => u64::from(be16(sub, 8).unwrap_or(0)),
            10 => u64::from(be32(sub, 16).unwrap_or(0)),
            12 => format12_codepoints(sub).unwrap_or(0),
            8 | 14 => 0,
            2 | 13 => return Err(TtfError::Unsupported("cmap formats 2 and 13")),
            _ => 0,
        };
        total = total.saturating_add(count);
        cap("cmap code points", total, MAX_CMAP_CODEPOINTS)?;
        subtables.push(sub);
    }
    // The glyphs mapped: ttf-parser's own lookup, now that the walk is bounded.
    let mut used = vec![false; glyphs as usize];
    used[0] = true;
    let mut outside = false;
    if let Ok(table) = ttf_parser::cmap::Table::parse(cmap).ok_or(()) {
        for sub in table.subtables {
            sub.codepoints(|c| {
                if let Some(g) = sub.glyph_index(c) {
                    match used.get_mut(g.0 as usize) {
                        Some(u) => *u = true,
                        // fontdue refuses the file then, after loading glyphs: refused here first.
                        None => outside |= g.0 != 0 && char::from_u32(c).is_some(),
                    }
                }
            });
        }
    }
    if outside {
        return Err(TtfError::Malformed(
            "the cmap maps a code point past the glyphs",
        ));
    }
    Ok(used
        .iter()
        .enumerate()
        .filter(|(_, u)| **u)
        .map(|(g, _)| g as u16)
        .collect())
}

/// Format 4's code points as ttf-parser enumerates them: each segment's `start..=end` until the `0xFFFF` end marker.
fn format4_codepoints(sub: &[u8]) -> Option<u64> {
    let seg = be16(sub, 6)? / 2;
    let mut total = 0u64;
    for i in 0..seg as usize {
        let end = be16(sub, 14 + 2 * i)?;
        let start = be16(sub, 16 + 2 * seg as usize + 2 * i)?;
        if start == end && start == 0xffff {
            break;
        }
        if start <= end {
            total += u64::from(end - start) + 1;
        }
    }
    Some(total)
}

/// Format 12's code points: every group's `start..=end`.
fn format12_codepoints(sub: &[u8]) -> Option<u64> {
    let groups = be32(sub, 12)?;
    let mut total = 0u64;
    for i in 0..groups as usize {
        let at = 16usize.checked_add(i.checked_mul(12)?)?;
        let (Some(start), Some(end)) = (be32(sub, at), be32(sub, at + 4)) else {
            break;
        };
        if start <= end {
            total = total.saturating_add(u64::from(end - start) + 1);
        }
        if total > MAX_CMAP_CODEPOINTS {
            break;
        }
    }
    Some(total)
}

/// The glyf table's outline cost, as ttf-parser walks it: a simple glyph its points, a composite 1 plus its components' costs one
/// level deeper, nothing at [`COMPOSITE_DEPTH`]. Memoised by glyph and depth, so the check itself is bounded by the table's size.
struct Outlines<'a> {
    loca: &'a [u8],
    glyf: &'a [u8],
    long: bool,
    glyphs: u16,
    memo: Vec<u64>,
}

impl Outlines<'_> {
    fn range(&self, g: u16) -> Option<(usize, usize)> {
        // ttf-parser's loca holds numGlyphs + 1 offsets (numGlyphs when that is 0xFFFF).
        let total = if self.glyphs == u16::MAX {
            u32::from(self.glyphs)
        } else {
            u32::from(self.glyphs) + 1
        };
        if u32::from(g) + 1 >= total {
            return None;
        }
        let (a, b) = if self.long {
            if self.loca.len() < total as usize * 4 {
                return None;
            }
            (
                be32(self.loca, g as usize * 4)? as usize,
                be32(self.loca, g as usize * 4 + 4)? as usize,
            )
        } else {
            if self.loca.len() < total as usize * 2 {
                return None;
            }
            (
                be16(self.loca, g as usize * 2)? as usize * 2,
                be16(self.loca, g as usize * 2 + 2)? as usize * 2,
            )
        };
        if a >= b || b > self.glyf.len() {
            return None;
        }
        Some((a, b))
    }

    fn cost(&mut self, g: u16, depth: u8) -> Result<u64, TtfError> {
        if depth >= COMPOSITE_DEPTH || g >= self.glyphs {
            return Ok(0);
        }
        let slot = depth as usize * self.glyphs as usize + g as usize;
        if self.memo[slot] != u64::MAX {
            return Ok(self.memo[slot]);
        }
        let Some((a, b)) = self.range(g) else {
            self.memo[slot] = 0;
            return Ok(0);
        };
        let data = &self.glyf[a..b];
        let contours = be16(data, 0).map_or(0, |c| c as i16);
        let mut cost = 0u64;
        if contours > 0 {
            let last = 10 + 2 * (contours as usize - 1);
            cost = be16(data, last).map_or(0, |p| u64::from(p) + 1);
        } else if contours < 0 {
            cost = 1;
            let mut at = 10usize;
            let mut n = 0usize;
            while let (Some(flags), Some(child)) = (be16(data, at), be16(data, at + 2)) {
                at += 4;
                n += 1;
                if n > MAX_COMPONENTS {
                    return Err(TtfError::Cap {
                        what: "components of one glyph",
                        value: n as u64,
                        cap: MAX_COMPONENTS as u64,
                    });
                }
                // ttf-parser reads the arguments only when they are x and y values (flag 2), words with flag 1.
                if flags & 0x0002 != 0 {
                    at += if flags & 0x0001 != 0 { 4 } else { 2 };
                }
                at += if flags & 0x0080 != 0 {
                    8
                } else if flags & 0x0040 != 0 {
                    4
                } else if flags & 0x0008 != 0 {
                    2
                } else {
                    0
                };
                if at > data.len() {
                    break;
                }
                // Every component visited costs one (an empty, missing or too-deep child included: ttf-parser still looks it up).
                cost = cost.saturating_add(1 + self.cost(child, depth + 1)?);
                if cost > MAX_OUTLINE_POINTS {
                    return Err(TtfError::Cap {
                        what: "outline points",
                        value: cost,
                        cap: MAX_OUTLINE_POINTS,
                    });
                }
                if flags & 0x0020 == 0 {
                    break;
                }
            }
        }
        self.memo[slot] = cost;
        Ok(cost)
    }
}

/// A parsed face: the checked file read by fontdue.
pub struct TtfFace {
    font: fontdue::Font,
    pub metrics: Checked,
}

impl std::fmt::Debug for TtfFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TtfFace({:?})", self.metrics)
    }
}

/// Windows-1252's characters for 0x80-0x9f (the rest of 0xa0-0xff is Latin-1).
const CP1252: [char; 32] = [
    '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}', '\u{8f}',
    '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}', '\u{178}',
];

/// The character a byte is in Windows-1252.
pub fn cp1252(b: u8) -> char {
    if (0x80..0xa0).contains(&b) {
        CP1252[(b - 0x80) as usize]
    } else {
        char::from(b)
    }
}

impl TtfFace {
    /// Checks the file ([`check`]), then reads it with fontdue (no substitutions).
    pub fn parse(bytes: &[u8]) -> Result<TtfFace, TtfError> {
        let metrics = check(bytes)?;
        let settings = fontdue::FontSettings {
            load_substitutions: false,
            scale: FONTDUE_SCALE,
            ..fontdue::FontSettings::default()
        };
        // fontdue reads a kern table on every load (its format 3 loops over glyphCount squared); this crate does no kerning, so it
        // gets the file with the kern table's version made one it does not read.
        let mut owned = bytes.to_vec();
        if let Some(at) = metrics.kern_at
            && let Some(v) = owned.get_mut(at..at + 2)
        {
            v.copy_from_slice(&[0xff, 0xff]);
        }
        let font = fontdue::Font::from_bytes(owned, settings)
            .map_err(|e| TtfError::Parse(e.to_string()))?;
        Ok(TtfFace { font, metrics })
    }

    /// The pixels per em at a cell height of `size` (GDI's mapping of a positive `CreateFontA` height).
    pub fn ppem(&self, size: i32) -> f32 {
        let cell = (self.metrics.ascent + self.metrics.descent) as f32;
        size as f32 * f32::from(self.metrics.units_per_em) / cell
    }

    /// The font at `size` (see the module's notes); None outside [`MIN_SIZE`]..=[`MAX_SIZE`].
    pub fn make(&self, size: i32) -> Option<Font> {
        if !(MIN_SIZE..=MAX_SIZE).contains(&size) {
            return None;
        }
        let px = self.ppem(size);
        let cell = (self.metrics.ascent + self.metrics.descent) as f32;
        let baseline = (self.metrics.ascent as f32 * size as f32 / cell).round() as u32;
        // Rasterize every byte's glyph first.
        struct Raster {
            byte: u8,
            w: u32,
            h: u32,
            coverage: Vec<u8>,
            x_origin: i8,
            y_origin: i8,
            advance: i8,
        }
        let mut rasters = Vec::new();
        for b in 32u8..=255 {
            let c = cp1252(b);
            if self.font.lookup_glyph_index(c) == 0 {
                continue;
            }
            let m = self.font.metrics(c, px);
            let advance = m.advance_width.round().clamp(-128.0, 127.0) as i8;
            let (w, h) = (m.width as u64, m.height as u64);
            let big = u64::from(MAX_GLYPH_CELLS) * size as u64;
            let (w, h, coverage) = if w == 0 || h == 0 || w > 255 || h > 255 || w > big || h > big {
                (0, 0, Vec::new())
            } else {
                let (metrics2, cov) = self.font.rasterize(c, px);
                if metrics2.width as u64 != w
                    || metrics2.height as u64 != h
                    || cov.len() as u64 != w * h
                {
                    (0, 0, Vec::new())
                } else {
                    (w as u32, h as u32, cov)
                }
            };
            rasters.push(Raster {
                byte: b,
                w,
                h,
                coverage,
                x_origin: m.xmin.clamp(-128, 127) as i8,
                y_origin: (i64::from(m.ymin) + m.height as i64).clamp(-128, 127) as i8,
                advance,
            });
        }
        // GFont::create's packing: tallest first, rows with a 1-pixel gap, a new sheet when the next row does not fit.
        let mut order: Vec<usize> = (0..rasters.len()).collect();
        order.sort_by(|&a, &b| rasters[b].h.cmp(&rasters[a].h).then(a.cmp(&b)));
        let mut glyphs = vec![Glyph::default(); rasters.len()];
        let mut sheets: Vec<Bitmap> = Vec::new();
        let mut placed: Vec<(usize, u32, u32)> = Vec::new();
        let (mut x, mut y, mut row) = (0u32, 0u32, 0u32);
        let mut used_h = 0u32;
        let mut sheet = 0usize;
        for &i in &order {
            let r = &rasters[i];
            if r.w == 0 {
                continue;
            }
            if x + r.w > SHEET {
                x = 0;
                y += row + 1;
                row = 0;
            }
            if y + r.h > SHEET {
                if sheets.len() + 1 >= MAX_FONT_SHEETS {
                    break;
                }
                sheets.push(blank_sheet(SHEET));
                sheet += 1;
                x = 0;
                y = 0;
                row = 0;
            }
            placed.push((i, x, y));
            glyphs[i].bitmap_index = sheet as i16;
            glyphs[i].x_offset = x as u8;
            glyphs[i].y_offset = y as u8;
            x += r.w + 1;
            row = row.max(r.h);
            used_h = y + row;
        }
        sheets.push(blank_sheet(used_h.max(1)));
        for &(i, px0, py0) in &placed {
            let r = &rasters[i];
            let s = &mut sheets[glyphs[i].bitmap_index as usize];
            for row in 0..r.h {
                let dst = ((py0 + row) * SHEET + px0) as usize;
                let src = (row * r.w) as usize;
                s.bits[dst..dst + r.w as usize]
                    .copy_from_slice(&r.coverage[src..src + r.w as usize]);
            }
        }
        let mut remap = Vec::with_capacity(rasters.len());
        let mut kept = vec![false; rasters.len()];
        for &(i, _, _) in &placed {
            kept[i] = true;
        }
        for (i, r) in rasters.iter().enumerate() {
            remap.push((u32::from(cp1252(r.byte)), i as i32));
            let g = &mut glyphs[i];
            // A glyph past the sheets' cap has no bitmap (it keeps its advance).
            let (w, h) = if kept[i] { (r.w, r.h) } else { (0, 0) };
            g.width = w as u8;
            g.height = h as u8;
            g.x_origin = r.x_origin;
            g.y_origin = r.y_origin;
            g.x_increment = r.advance;
            if w == 0 {
                g.bitmap_index = 0;
                g.x_offset = 0;
                g.y_offset = 0;
            }
        }
        Some(Font {
            height: size as u32,
            baseline,
            ascent: baseline,
            descent: (size as u32).saturating_sub(baseline),
            glyphs,
            sheets,
            remap: Remap::from_pairs(&remap),
            units: Units::Utf16,
            tab: Tab::Spaces,
        })
    }
}

fn blank_sheet(height: u32) -> Bitmap {
    Bitmap {
        width: SHEET,
        height,
        format: torque_images::Format::Alpha,
        bits: vec![0; (SHEET * height) as usize],
    }
}

/// A face shared by the caches that draw and measure with it.
pub type SharedFace = Arc<TtfFace>;
