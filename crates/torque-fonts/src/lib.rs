//! GUI fonts: the `.uft` font caches TGE 1.4's `GFont` reads ([`uft`]), and the text metrics and glyph layout of
//! `GFont::getStrNWidth` and `dglDrawTextN`. No script VM and no GPU: a GUI layer takes [`GlyphQuad`]s from here and the renderer
//! draws them with the sheets' texels.
//!
//! **A font** is a glyph table (`PlatformFont::CharInfo`), the grey sheets the glyphs sit on, and a remap from a code unit to its
//! glyph ([`Remap`]). Text is UTF-8, laid out as the UTF-16 code units `convertUTF8toUTF16` makes of it ([`utf16_units`]); a font
//! may instead take each byte as its own code unit ([`Units::Bytes`]). The sheets are grey PNGs, so
//! `GFormatA8` bitmaps uploaded as `GL_ALPHA` with `GL_NEAREST` both ways, never mip-mapped, and padded to powers of two like every
//! texture.
//!
//! **Where the files are:** `"%s/%s %d (%s).uft"` of the cache folder (`$GUI::fontCacheDirectory`, which the host sets:
//! [`FontCache::new`] takes it), the face name, the size and the charset's name ([`uft::file_name`]). TGE makes a missing file
//! from the platform's font and writes it; this crate never writes: a missing face and size goes to the [`Fallback`], counted.
//! The face name goes into the path as given, so the host's `read` decides what a name may reach. A host with another cache file
//! format gives the cache a [`CacheFormat`] (TGE 1.3's single-byte `.gft` caches: [`gft`]); TrueType faces a host registers at
//! run time are looked up before the cache files ([`faces`]).
//!
//! **Hostile input.** A cache file from a content package's folder or a modified cache is hostile: [`Font::parse`] returns a font
//! or a [`FontError`], never panics, and checks every count against what the file can hold and the named caps before it allocates.
//! Where TGE would read out of bounds (a remap entry past the glyph table, a glyph whose sheet does not exist, a tab with no space
//! glyph) this crate draws nothing for that code unit, and the layout reports how many such units it met ([`Layout::invalid`]).
//!
//! **Credits.** The cache format and metrics follow the Torque Game Engine's `GFont` (`dgl/gNewFont.cc`) and
//! `convertUTF8toUTF16` (`core/unicode.cc`), carried into the MIT-licensed Torque3D as `gfx/gFont.cpp`, `gfx/gFont.h` and
//! `core/strings/unicode.cpp` (Copyright (c) 2012 GarageGames, LLC). The layout follows TGE's `dglDrawTextN` (`dgl/dgl.cc`,
//! TGE 1.4, Copyright (C) GarageGames.com, Inc.; the Torque engine is MIT-licensed as Torque3D, Copyright (c) 2012
//! GarageGames, LLC). The code here is this crate's own.
//!
//! # Maintaining
//!
//! **TrueType.** `fontdue` rasterizes with unchecked writes, so a sweep that only sees panics cannot see a bad write. The sweep
//! (`ttf_fuzz::sweep`, `ttf_tests::a_short_hostile_sweep_has_no_panics`) reports its slowest case. `just miri-ttf` runs it under
//! Miri where the nightly `miri` component is installed (`rustup +nightly component add miri`; `TORQUE_FUZZ_TTF` sets the case
//! count, and under Miri the real-face seeds are left out). It is a local run, not a CI job (CI minutes are tight): run it after
//! a change to the TrueType path or a `fontdue` update.
//!
//! Rules a change must keep, each guarded by a test:
//! - A font a profile names is never dropped for one markup names: the markup fonts ([`FontUse::Markup`]) are an LRU of their
//!   own within [`MAX_MARKUP_FONTS`] and [`MAX_MARKUP_SHEET_PIXELS`], and a profile's lookup keeps a markup font for good
//!   (`markup_fonts_are_dropped_first_and_never_take_a_profile_font`, `a_profile_lookup_keeps_a_markup_font_for_the_session`).
//! - A font whose [`estimated_sheet_pixels`] do not fit is refused before it is read or made
//!   (`a_spent_budget_refuses_a_font_before_making_it`).
//! - A markup font something outside the cache holds, or one looked up this frame, is never dropped
//!   (`a_markup_font_looked_up_this_frame_or_held_is_not_dropped`).
//! - A server's GUI ([`FontUse::Server`]) keeps at most [`MAX_SERVER_FONTS`] and drops only its own, so the game's ML text
//!   always has the rest of the markup quota (`a_servers_gui_takes_only_its_share_and_the_games_ml_text_still_loads`).
//! - A refusal asked again in its frame with nothing changed costs no pass over the fonts
//!   (`a_refusal_asked_again_in_its_frame_is_answered_at_once_until_something_changes`).

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

pub use torque_images::Bitmap;

pub mod faces;
pub mod gft;
#[cfg(feature = "system-fonts")]
pub mod system;
pub mod ttf;
pub mod ttf_fuzz;
#[cfg(test)]
mod ttf_tests;
pub mod uft;
#[cfg(all(feature = "system-fonts", target_arch = "wasm32"))]
pub mod web;

#[cfg(test)]
mod fuzz;
#[cfg(test)]
mod tests;

/// The most glyphs one font may have (one for every UTF-16 code unit).
pub const MAX_GLYPHS: u32 = 65_536;
/// The most sheets one font may have (typical caches have a few sheets of 256 x 256).
pub const MAX_SHEETS: u32 = 256;
/// The largest cache file read (real caches are tens of KB).
pub const MAX_FILE_BYTES: usize = 16 << 20;
/// The most pixels the sheets of one font may hold together.
pub const MAX_SHEET_PIXELS: u64 = 16 << 20;
/// How many spaces a tab is (`GFont::TabWidthInSpaces`: `xIncrement` of the space glyph times 3).
pub const TAB_SPACES: i32 = 3;
/// The longest text one layout takes (`dglDrawTextN` itself has no cap; a GUI text control keeps 255 bytes, an ML control more).
pub const MAX_LAYOUT_BYTES: usize = 1 << 20;
/// The most fonts one [`FontCache`] keeps (a hostile profile set could ask for every size of every name).
pub const MAX_CACHED_FONTS: usize = 1024;
/// The most sheet pixels the fonts of one cache hold whatever made them: the cache files and the system fallback were not
/// counted, and 1,024 fallback fonts at large sizes were several GiB. A font past it is not kept (counted in
/// [`FontCache::over_budget`]).
pub const MAX_FONT_SHEET_PIXELS: u64 = 128 << 20;
/// The most faces and sizes [`FontUse::Markup`] keeps (misses included), within [`MAX_CACHED_FONTS`]'s own.
pub const MAX_MARKUP_FONTS: usize = 256;
/// The most sheet pixels the [`FontUse::Markup`] fonts hold together, within [`MAX_FONT_SHEET_PIXELS`].
pub const MAX_MARKUP_SHEET_PIXELS: u64 = 32 << 20;
/// The most faces and sizes and sheet pixels a server's GUI's fonts ([`FontUse::Server`]) hold, within the markup quota: the
/// rest of it is the game's ML text's.
pub const MAX_SERVER_FONTS: usize = 192;
pub const MAX_SERVER_SHEET_PIXELS: u64 = 24 << 20;

/// What a font of `size` is taken to hold in sheets before it is made: the 224 glyphs of the ANSI charset in cells of a quarter
/// of `size` squared each, padding left out. The fallbacks make more (Arial at 128 is about 1.8 Mpx); a cache file of fewer glyphs
/// can hold less, and is then refused a little early once the budget is nearly spent.
pub fn estimated_sheet_pixels(size: i32) -> u64 {
    let s = u64::try_from(size.clamp(0, 1 << 12)).unwrap_or(0);
    224 * s * s / 4
}

/// One glyph's metrics (`PlatformFont::CharInfo`, as the cache files store it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Glyph {
    pub bitmap_index: i16,
    pub x_offset: u8,
    pub y_offset: u8,
    pub width: u8,
    pub height: u8,
    pub x_origin: i8,
    pub y_origin: i8,
    pub x_increment: i8,
}

/// The glyph of each code unit (`mRemapTable`): the entries of the units `first..first + len`, -1 (no glyph) everywhere else.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Remap {
    first: u32,
    entries: Vec<i32>,
}

impl Remap {
    /// The entries of the code units from `first` on.
    pub fn new(first: u32, entries: Vec<i32>) -> Remap {
        Remap { first, entries }
    }

    /// The entries of the code units the pairs name (a later pair for the same unit wins); every other unit has none.
    pub fn from_pairs(pairs: &[(u32, i32)]) -> Remap {
        let (Some(lo), Some(hi)) = (
            pairs.iter().map(|p| p.0).min(),
            pairs.iter().map(|p| p.0).max(),
        ) else {
            return Remap::default();
        };
        let mut entries = vec![-1; (hi - lo) as usize + 1];
        for &(unit, glyph) in pairs {
            entries[(unit - lo) as usize] = glyph;
        }
        Remap::new(lo, entries)
    }

    /// The entry of `unit`, -1 when it has none.
    pub fn get(&self, unit: u32) -> i32 {
        unit.checked_sub(self.first)
            .and_then(|i| self.entries.get(i as usize))
            .copied()
            .unwrap_or(-1)
    }

    /// The first and last code unit with an entry (the range a cache file stores), None when there is none.
    pub fn range(&self) -> Option<(u32, u32)> {
        let len = u32::try_from(self.entries.len()).ok()?;
        (len > 0).then(|| (self.first, self.first + len - 1))
    }
}

/// How the bytes of a text become code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Units {
    /// UTF-8 converted to UTF-16 as `convertUTF8toUTF16` does ([`utf16_units`]); the text ends at its first NUL (TGE 1.4).
    #[default]
    Utf16,
    /// Each byte is its own code unit, NUL included.
    Bytes,
}

/// How [`Font::layout`] draws a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    /// Three spaces, whatever the font has for a tab (`dglDrawTextN`).
    #[default]
    Spaces,
    /// As [`Font::str_width`] measures it (`getStrNWidth`): the tab's glyph when the remap names one, else three spaces.
    GlyphOrSpaces,
}

/// The UTF-16 code units of `text` as `convertUTF8toUTF16` makes them: up to the first NUL byte; a lead byte gives the length of
/// its sequence (`0xc0..=0xdf` two, `0xe0..=0xef` three, `0xf0..=0xf7` four, `0xf8..=0xfb` five, `0xfc..=0xfd` six) and each
/// continuation byte (`0x80..=0xbf`, `0xfe`, `0xff`) adds its low 6 bits (an overlong form is decoded as written). A byte that
/// cannot start a sequence, or a sequence cut by a byte that is not a continuation (or by the end), gives U+FFFD and moves one
/// byte on. A code point inside the surrogates (0xd801..=0xdffe, as TGE tests them) or above the Basic Multilingual Plane gives
/// U+FFFD.
pub fn utf16_units(text: &[u8]) -> Vec<u16> {
    const REPLACEMENT: u16 = 0xfffd;
    const LEAD_MASK: [u8; 7] = [0x3f, 0x7f, 0x1f, 0x0f, 0x07, 0x03, 0x01];
    fn sequence_len(b: u8) -> usize {
        match b {
            0x00..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            0xf8..=0xfb => 5,
            0xfc..=0xfd => 6,
            _ => 0,
        }
    }
    let mut out = Vec::with_capacity(text.len());
    let mut at = 0;
    while let Some(&lead) = text.get(at) {
        if lead == 0 {
            break;
        }
        let n = sequence_len(lead);
        let (mut cp, mut walked) = (u32::from(REPLACEMENT), 1);
        if n > 0 {
            cp = u32::from(LEAD_MASK[n] & lead);
            walked = n;
            for i in 1..n {
                let c = text.get(at + i).copied().unwrap_or(0);
                if sequence_len(c) == 0 {
                    cp = (cp << 6) | u32::from(c & 0x3f);
                } else {
                    cp = u32::from(REPLACEMENT);
                    walked = 1;
                    break;
                }
            }
        }
        if (0xd801..=0xdffe).contains(&cp) || cp > 0xffff {
            cp = u32::from(REPLACEMENT);
        }
        out.push(cp as u16);
        at += walked;
    }
    out
}

/// A font: read from a cache file ([`Font::parse`]), made by a [`Fallback`], or built by a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Font {
    pub height: u32,
    pub baseline: u32,
    pub ascent: u32,
    pub descent: u32,
    pub glyphs: Vec<Glyph>,
    /// The glyph sheets as `readPNG` leaves them (grey: [`torque_images::Format::Alpha`]).
    pub sheets: Vec<Bitmap>,
    pub remap: Remap,
    pub units: Units,
    pub tab: Tab,
}

/// Why a cache file was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontError {
    /// The file ended before the field (TGE: the stream's status is not `Ok`).
    Short(&'static str),
    Version(u32),
    /// A count or size above a named cap.
    Cap {
        what: &'static str,
        value: u64,
        cap: u64,
    },
    /// A sheet's PNG did not decode (in TGE a libpng error is a fatal assert).
    Sheet(usize, String),
    /// The compressed remap did not give exactly its range's entries (a fatal assert in TGE).
    Remap(String),
}

impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FontError::Short(what) => write!(f, "the file ends inside the {what}"),
            FontError::Version(v) => write!(f, "version {v} is not read"),
            FontError::Cap { what, value, cap } => {
                write!(f, "{what}: {value} is above the limit of {cap}")
            }
            FontError::Sheet(i, e) => write!(f, "sheet {i}: {e}"),
            FontError::Remap(e) => write!(f, "the remap: {e}"),
        }
    }
}

impl std::error::Error for FontError {}

/// A little-endian reader that fails like a TGE stream: a short read is an error. The cache formats share it.
pub struct Reader<'a> {
    bytes: &'a [u8],
    /// Where the next read starts.
    pub at: usize,
}

impl<'a> Reader<'a> {
    /// A reader at the start of `bytes`, refusing a file past [`MAX_FILE_BYTES`].
    pub fn new(bytes: &'a [u8]) -> Result<Reader<'a>, FontError> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err(FontError::Cap {
                what: "file bytes",
                value: bytes.len() as u64,
                cap: MAX_FILE_BYTES as u64,
            });
        }
        Ok(Reader { bytes, at: 0 })
    }

    pub fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], FontError> {
        let end = self.at.checked_add(n).ok_or(FontError::Short(what))?;
        let s = self.bytes.get(self.at..end).ok_or(FontError::Short(what))?;
        self.at = end;
        Ok(s)
    }

    pub fn u8(&mut self, what: &'static str) -> Result<u8, FontError> {
        Ok(self.take(1, what)?[0])
    }

    pub fn u32(&mut self, what: &'static str) -> Result<u32, FontError> {
        let s = self.take(4, what)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    pub fn i32(&mut self, what: &'static str) -> Result<i32, FontError> {
        Ok(self.u32(what)? as i32)
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.at)
    }

    /// A glyph count and that many 9-byte `CharInfo`s (`i16 bitmapIndex, u8 xOffset, u8 yOffset, u8 width, u8 height,
    /// i8 xOrigin, i8 yOrigin, i8 xIncrement`), refusing a count past `max` or past what the file holds.
    pub fn glyphs(&mut self, max: u32) -> Result<Vec<Glyph>, FontError> {
        let count = self.u32("glyph count")?;
        if count > max {
            return Err(FontError::Cap {
                what: "glyphs",
                value: u64::from(count),
                cap: u64::from(max),
            });
        }
        if self.remaining() < count as usize * 9 {
            return Err(FontError::Short("glyph table"));
        }
        let mut glyphs = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let g = self.take(9, "glyph table")?;
            glyphs.push(Glyph {
                bitmap_index: i16::from_le_bytes([g[0], g[1]]),
                x_offset: g[2],
                y_offset: g[3],
                width: g[4],
                height: g[5],
                x_origin: g[6] as i8,
                y_origin: g[7] as i8,
                x_increment: g[8] as i8,
            });
        }
        Ok(glyphs)
    }

    /// A sheet count and that many PNGs, each read by `GBitmap::readPNG` (libpng reads to `IEND` and no further, so the next sheet
    /// starts right after it).
    pub fn sheets(&mut self) -> Result<Vec<Bitmap>, FontError> {
        let count = self.u32("sheet count")?;
        if count > MAX_SHEETS {
            return Err(FontError::Cap {
                what: "sheets",
                value: u64::from(count),
                cap: u64::from(MAX_SHEETS),
            });
        }
        let mut sheets = Vec::new();
        let mut pixels = 0u64;
        for i in 0..count as usize {
            let rest = &self.bytes[self.at.min(self.bytes.len())..];
            if rest.len() < 8 {
                return Err(FontError::Short("sheet"));
            }
            // readPNG checks the signature first, then libpng reads to IEND.
            let n = png_extent(rest).ok_or(FontError::Short("sheet"))?;
            let sheet = torque_images::png::decode(&rest[..n])
                .map_err(|e| FontError::Sheet(i, e.to_string()))?;
            pixels += u64::from(sheet.width) * u64::from(sheet.height);
            if pixels > MAX_SHEET_PIXELS {
                return Err(FontError::Cap {
                    what: "sheet pixels",
                    value: pixels,
                    cap: MAX_SHEET_PIXELS,
                });
            }
            sheets.push(sheet);
            self.at += n;
        }
        Ok(sheets)
    }
}

/// How many bytes of `bytes` libpng reads for one PNG: the signature, then every chunk (length, type, data, CRC) up to and including
/// `IEND`. None when the chunks run past the end or a length is above `2^31 - 1` (`png_get_uint_31`).
pub fn png_extent(bytes: &[u8]) -> Option<usize> {
    let mut at = 8usize;
    if bytes.len() < at {
        return None;
    }
    loop {
        let len = bytes.get(at..at + 4)?;
        let len = u32::from_be_bytes([len[0], len[1], len[2], len[3]]);
        if len > 0x7fff_ffff {
            return None;
        }
        let name = bytes.get(at + 4..at + 8)?;
        let end = at.checked_add(12)?.checked_add(len as usize)?;
        if end > bytes.len() {
            return None;
        }
        at = end;
        if name == b"IEND" {
            return Some(at);
        }
    }
}

/// A text as code units (see [`Units`]), at most [`MAX_LAYOUT_BYTES`] of its bytes.
enum Text<'a> {
    Bytes(&'a [u8]),
    Wide(Vec<u16>),
}

impl Text<'_> {
    fn len(&self) -> usize {
        match self {
            Text::Bytes(b) => b.len(),
            Text::Wide(w) => w.len(),
        }
    }

    fn at(&self, i: usize) -> u32 {
        match self {
            Text::Bytes(b) => u32::from(b[i]),
            Text::Wide(w) => u32::from(w[i]),
        }
    }

    fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.len()).map(|i| self.at(i))
    }
}

impl Font {
    /// Reads a `.uft` as TGE 1.4's `GFont::read` does ([`uft`]).
    pub fn parse(bytes: &[u8]) -> Result<Font, FontError> {
        uft::UftFile::parse(bytes).map(|f| f.font)
    }

    fn text<'a>(&self, text: &'a [u8]) -> Text<'a> {
        let text = &text[..text.len().min(MAX_LAYOUT_BYTES)];
        match self.units {
            Units::Bytes => Text::Bytes(text),
            Units::Utf16 => Text::Wide(utf16_units(text)),
        }
    }

    /// The glyph of code unit `c`, when the remap names one inside the table (`isValidChar` and `getCharInfo`).
    pub fn glyph(&self, c: impl Into<u32>) -> Option<&Glyph> {
        let r = self.remap.get(c.into());
        if r < 0 {
            return None;
        }
        self.glyphs.get(r as usize)
    }

    /// Whether the remap names a glyph for `c` (`mRemapTable[c] != -1`), whatever the table holds.
    pub fn is_valid_char(&self, c: impl Into<u32>) -> bool {
        self.remap.get(c.into()) != -1
    }

    /// The space glyph's advance times [`TAB_SPACES`]: what a tab adds (0 when the font has no space glyph).
    fn tab_advance(&self) -> i32 {
        self.glyph(b' ')
            .map_or(0, |g| i32::from(g.x_increment) * TAB_SPACES)
    }

    fn units_width(&self, text: &Text<'_>) -> i32 {
        let mut w = 0i32;
        for c in text.iter() {
            if self.is_valid_char(c) {
                w = w.wrapping_add(self.glyph(c).map_or(0, |g| i32::from(g.x_increment)));
            } else if c == u32::from(b'\t') {
                w = w.wrapping_add(self.tab_advance());
            }
        }
        w
    }

    /// `GFont::getStrNWidth`: every code unit with a glyph adds its `xIncrement`; a unit without one adds nothing, except a tab
    /// (three spaces). Colour codes count when the remap names a glyph for them (typical caches name none below 32).
    pub fn str_width(&self, text: &[u8]) -> i32 {
        self.units_width(&self.text(text))
    }

    /// `GFont::getStrNWidthPrecise`: [`str_width`](Self::str_width)'s sum, plus, when the last code unit has a glyph wider than its
    /// advance, the difference (the last glyph's ink is all inside the width). With [`Units::Utf16`] the last unit is the one at
    /// the text's byte length less one, as TGE indexes it: when the text has fewer units than bytes (a multi-byte sequence, a
    /// NUL) that is the end of the units, so nothing is added.
    pub fn str_width_precise(&self, text: &[u8]) -> i32 {
        let text = &text[..text.len().min(MAX_LAYOUT_BYTES)];
        let units = self.text(text);
        let mut w = self.units_width(&units);
        let last = match &units {
            Text::Bytes(b) => b.last().map(|&b| u32::from(b)),
            Text::Wide(w) if w.is_empty() => None,
            Text::Wide(w) => Some(if w.len() >= text.len() {
                u32::from(w[text.len() - 1])
            } else {
                0
            }),
        };
        if let Some(last) = last
            && let Some(g) = self.glyph(last)
            && i32::from(g.x_increment) < i32::from(g.width)
        {
            w = w.wrapping_add(i32::from(g.width) - i32::from(g.x_increment));
        }
        w
    }

    /// `GFont::getBreakPos`: how many code units of `text` fit in `width` pixels (with [`Units::Utf16`] a count of UTF-16 units,
    /// as TGE returns it; the byte offset only for ASCII text). A tab counts as a space; a unit without a glyph fits and takes
    /// nothing; a glyph fits while neither its bitmap width nor its advance (as unsigned) exceeds what is left. At the first glyph
    /// that does not fit: with `word_wrap` and a space seen (the failing unit included), the position after the last space, else
    /// the failing position.
    pub fn break_pos(&self, text: &[u8], width: u32, word_wrap: bool) -> usize {
        let units = self.text(text);
        let mut left = width;
        let mut last_ws = 0usize;
        for i in 0..units.len() {
            let raw = units.at(i);
            let c = if raw == u32::from(b'\t') {
                u32::from(b' ')
            } else {
                raw
            };
            let Some(g) = self.glyph(c) else {
                continue;
            };
            let ws = if c == u32::from(b' ') { i + 1 } else { last_ws };
            let adv = i32::from(g.x_increment) as u32;
            if left < u32::from(g.width) || left < adv {
                if ws == 0 || !word_wrap {
                    return i;
                }
                return ws;
            }
            left -= adv;
            last_ws = ws;
        }
        units.len()
    }

    /// The texture size a sheet is uploaded at: the bitmap padded to powers of two (`TextureObject::texWidth`, `texHeight`).
    pub fn sheet_texture_size(&self, sheet: usize) -> Option<(u32, u32)> {
        let s = self.sheets.get(sheet)?;
        Some((texture_side(s.width), texture_side(s.height)))
    }

    /// `dglDrawTextN`: the glyph quads of `text` drawn at `origin` (the top left of the line), the colours from `colors`
    /// (the modulation colour on entry, changed by the colour codes as it goes).
    ///
    /// Code units 1-7, 11, 12 and 14 pick `table[map]` (map 0-9: `\c0`-`\c9`) when a table is given and the index is at most
    /// `max_index`; 15 goes back to the anchor colour; 16 saves the colour and 17 restores it (the saved colour is a global in TGE,
    /// kept in `colors` across calls). A tab adds three spaces (with [`Tab::GlyphOrSpaces`], only when the remap names no glyph
    /// for it). Every other unit: no glyph adds nothing; a glyph without a bitmap adds its
    /// `xIncrement`; a glyph is drawn at `x + xOrigin`, `y + baseline - yOrigin`, `width` by `height`, and the pen moves to
    /// `x + xIncrement`.
    pub fn layout(
        &self,
        origin: (i32, i32),
        text: &[u8],
        colors: &mut TextColors,
        table: Option<&[[u8; 4]]>,
        max_index: u32,
    ) -> Layout {
        let units = self.text(text);
        let tab_first = self.tab == Tab::Spaces;
        let mut out = Layout::default();
        let (mut x, y) = origin;
        for c in units.iter() {
            match c {
                1..=7 | 11 | 12 | 14 => {
                    let index = COLOR_CODE_INDEX[c as usize];
                    if let Some(t) = table
                        && u32::from(index) <= max_index
                        && let Some(color) = t.get(index as usize)
                    {
                        colors.current = *color;
                    }
                }
                15 => colors.current = colors.anchor,
                16 => colors.saved = colors.current,
                17 => colors.current = colors.saved,
                _ => {
                    let tab = c == u32::from(b'\t');
                    if tab && tab_first {
                        x = x.wrapping_add(self.tab_advance());
                        continue;
                    }
                    if !self.is_valid_char(c) {
                        if tab {
                            x = x.wrapping_add(self.tab_advance());
                        }
                        continue;
                    }
                    let Some(g) = self.glyph(c) else {
                        out.invalid += 1;
                        continue;
                    };
                    if g.width == 0 || g.height == 0 {
                        x = x.wrapping_add(i32::from(g.x_increment));
                        continue;
                    }
                    let left = x.wrapping_add(i32::from(g.x_origin));
                    let top = y
                        .wrapping_add(self.baseline as i32)
                        .wrapping_sub(i32::from(g.y_origin));
                    x = left.wrapping_add(i32::from(g.x_increment) - i32::from(g.x_origin));
                    let Ok(sheet) = usize::try_from(g.bitmap_index) else {
                        out.invalid += 1;
                        continue;
                    };
                    let Some((tw, th)) = self.sheet_texture_size(sheet) else {
                        out.invalid += 1;
                        continue;
                    };
                    let (tw, th) = (tw as f32, th as f32);
                    out.quads.push(GlyphQuad {
                        sheet,
                        rect: [
                            left,
                            top,
                            left.wrapping_add(i32::from(g.width)),
                            top.wrapping_add(i32::from(g.height)),
                        ],
                        uv: [
                            f32::from(g.x_offset) / tw,
                            f32::from(g.y_offset) / th,
                            (f32::from(g.x_offset) + f32::from(g.width)) / tw,
                            (f32::from(g.y_offset) + f32::from(g.height)) / th,
                        ],
                        color: colors.current,
                    });
                }
            }
        }
        out.advance = x.wrapping_sub(origin.0);
        out
    }
}

/// `dglDrawTextN`'s colour-code map: the table index of code units 1-7, 11, 12 and 14.
pub const COLOR_CODE_INDEX: [u8; 16] = [0, 0, 1, 2, 3, 4, 5, 6, 0, 0, 0, 7, 8, 0, 9, 0];

/// The texture side TGE makes for a bitmap side: the next power of two (0 counts as 1).
pub fn texture_side(n: u32) -> u32 {
    n.max(1).checked_next_power_of_two().unwrap_or(1 << 31)
}

/// The colours `dglDrawTextN` carries: the modulation colour (`sg_bitmapModulation`), the anchor (`sg_textAnchorColor`, what code 15
/// returns to: the colour the caller set) and the saved colour (codes 16 and 17; a global in TGE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextColors {
    pub current: [u8; 4],
    pub anchor: [u8; 4],
    pub saved: [u8; 4],
}

impl TextColors {
    /// The caller set `color` as both the modulation and the anchor.
    pub fn new(color: [u8; 4], saved: [u8; 4]) -> TextColors {
        TextColors {
            current: color,
            anchor: color,
            saved,
        }
    }
}

/// One glyph drawn: the screen rectangle (left, top, right, bottom), the texture coordinates over the sheet's padded texture, the
/// modulation colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphQuad {
    pub sheet: usize,
    pub rect: [i32; 4],
    pub uv: [f32; 4],
    pub color: [u8; 4],
}

/// A line laid out.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layout {
    pub quads: Vec<GlyphQuad>,
    /// How far the pen moved (`dglDrawTextN`'s result, the end x less the start x).
    pub advance: i32,
    /// Code units whose glyph TGE would read out of bounds (a remap entry past the table, a sheet that does not exist).
    pub invalid: u32,
}

/// The text metrics a GUI layout needs (the automatic sizes of `GuiTextCtrl`, the justification): implemented by [`Font`].
pub trait FontMetrics {
    /// `getStrNWidth` of the text.
    fn str_width(&self, text: &[u8]) -> i32;
    /// `getHeight`.
    fn height(&self) -> i32;
    /// `getBaseline`.
    fn baseline(&self) -> i32;
}

impl FontMetrics for Font {
    fn str_width(&self, text: &[u8]) -> i32 {
        Font::str_width(self, text)
    }

    fn height(&self) -> i32 {
        self.height as i32
    }

    fn baseline(&self) -> i32 {
        self.baseline as i32
    }
}

/// The default cache file's name for a face and size: [`uft::file_name`] with the ANSI charset (`GFont::create`'s default),
/// without the folder.
pub fn cache_file_name(face: &str, size: i32) -> String {
    uft::file_name(face, size, uft::ANSI_CHARSET).unwrap_or_default()
}

/// A font cache file format: where a face and size's file is, and how it reads.
pub trait CacheFormat: Send + Sync {
    /// The file's name inside the cache folder.
    fn file_name(&self, face: &str, size: i32) -> String;
    /// The font the file holds.
    fn parse(&self, bytes: &[u8]) -> Result<Font, FontError>;
}

/// A source of fonts for a face and size no cache file exists for.
pub trait Fallback: Send + Sync {
    /// The font, or None when this fallback cannot make it.
    fn make(&self, face: &str, size: i32) -> Option<Font>;
    /// A name for the reports.
    fn name(&self) -> &'static str;
    /// The cache dropped `font`, which this fallback made for `face` (lower case): what was counted for it may be given back.
    fn dropped(&self, _face: &str, _font: &Font) {}
}

/// No fallback: a face and size without a cache file draws no text (counted).
pub struct NoFallback;

impl Fallback for NoFallback {
    fn make(&self, _face: &str, _size: i32) -> Option<Font> {
        None
    }

    fn name(&self) -> &'static str {
        "none"
    }
}

/// How a font was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A cache file.
    Cache,
    /// The fallback rasterizer made it.
    Fallback,
}

/// A font found and where it came from.
pub type Found = (Arc<Font>, Source);

/// Who names a font, which decides the quota it is kept under ([`FontCache::get_for`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontUse {
    /// A font the game's own profiles and controls name: kept for the session, at most [`MAX_CACHED_FONTS`] of them.
    Profile,
    /// A font ML text's `<font:>` tags name (the game's text or a server's): kept under [`MAX_MARKUP_FONTS`] and
    /// [`MAX_MARKUP_SHEET_PIXELS`], the least recently used one that nothing holds dropped first, so a server that names every
    /// face and size never takes a font from the game's profiles.
    Markup,
    /// A font a server's own GUI names (its client-script layer): markup within [`MAX_SERVER_FONTS`] and
    /// [`MAX_SERVER_SHEET_PIXELS`], so the rest of the markup quota stays for the game's ML text.
    Server,
}

impl FontUse {
    /// Whether the font is under the markup quota (and may be dropped).
    fn is_markup(self) -> bool {
        self != FontUse::Profile
    }
}

/// One face and size the cache looked up.
struct Entry {
    found: Option<Found>,
    /// The sheet pixels `found` holds (0 for a miss).
    pixels: u64,
    tier: FontUse,
    /// The cache's clock at the last lookup (least recently used first).
    used: u64,
    /// The frame of the last lookup: a markup font looked up this frame is not dropped while it draws.
    frame: u64,
}

/// The kept fonts' sheet pixels and keys, by quota.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Usage {
    all: u64,
    /// Every markup font's, a server's included.
    markup: u64,
    markup_keys: usize,
    server: u64,
    server_keys: usize,
}

impl Usage {
    fn add(&mut self, tier: FontUse, pixels: u64) {
        self.all += pixels;
        if tier.is_markup() {
            self.markup += pixels;
            self.markup_keys += 1;
        }
        if tier == FontUse::Server {
            self.server += pixels;
            self.server_keys += 1;
        }
    }

    fn remove(&mut self, tier: FontUse, pixels: u64) {
        self.all -= pixels;
        if tier.is_markup() {
            self.markup -= pixels;
            self.markup_keys -= 1;
        }
        if tier == FontUse::Server {
            self.server -= pixels;
            self.server_keys -= 1;
        }
    }
}

/// A refusal of [`FontCache::make_room`]: while the frame, the usage and the quota are the same, a font as large is refused
/// again at once (a text asks for its missing font once a glyph run).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Refusal {
    frame: u64,
    usage: Usage,
    tier: FontUse,
    pixels: u64,
}

/// Fonts by face and size, loaded once: the cache file through `read` (this crate never writes one), else the
/// [`Fallback`]. Counts what came from where.
///
/// The fonts profiles name ([`FontUse::Profile`]) are never dropped. The fonts markup names ([`FontUse::Markup`],
/// [`FontUse::Server`]) are an LRU of their own: what is dropped first is the least recently used one that nothing outside the
/// cache holds and that was not looked up this frame ([`next_frame`](Self::next_frame)); a server's GUI drops only its own. Before
/// a font is loaded, an estimate of its sheets ([`estimated_sheet_pixels`]) must fit, so a spent budget refuses a font without
/// rasterizing it.
pub struct FontCache {
    /// `$GUI::fontCacheDirectory`, the folder the cache files are looked up in.
    pub dir: String,
    format: Box<dyn CacheFormat>,
    fonts: HashMap<(String, i32), Entry>,
    fallback: Box<dyn Fallback>,
    usage: Usage,
    refused: Option<Refusal>,
    clock: u64,
    frame: u64,
    /// The cap on every kept font's sheet pixels (a test lowers it).
    pub pixel_cap: u64,
    /// The caps on the markup fonts' sheet pixels and on a server's GUI's share of them (a test lowers them).
    pub markup_pixel_cap: u64,
    pub server_pixel_cap: u64,
    /// Fonts not kept because they would pass a pixel cap or the markup keys, before or after loading.
    pub over_budget: u64,
    /// Markup fonts dropped to make room.
    pub evicted: u64,
    /// Fonts loaded: every cache file parsed and every font the fallback was asked for.
    pub loads: u64,
    /// Faces and sizes the fallback made, the ones nothing made, and cache files that did not parse.
    pub fallbacks: u32,
    pub missing: u32,
    pub bad_files: Vec<(String, String)>,
}

impl FontCache {
    /// A cache over the `.uft` files (ANSI charset) in `dir` (`$GUI::fontCacheDirectory`; Torque has no built-in default, the host
    /// sets it).
    pub fn new(dir: &str, fallback: Box<dyn Fallback>) -> FontCache {
        FontCache::with_format(dir, fallback, Box::new(uft::Uft::default()))
    }

    /// A cache over the files of another format in `dir`.
    pub fn with_format(
        dir: &str,
        fallback: Box<dyn Fallback>,
        format: Box<dyn CacheFormat>,
    ) -> FontCache {
        FontCache {
            dir: dir.trim_end_matches('/').to_string(),
            format,
            fonts: HashMap::new(),
            fallback,
            usage: Usage::default(),
            refused: None,
            clock: 0,
            frame: 0,
            pixel_cap: MAX_FONT_SHEET_PIXELS,
            markup_pixel_cap: MAX_MARKUP_SHEET_PIXELS,
            server_pixel_cap: MAX_SERVER_SHEET_PIXELS,
            over_budget: 0,
            evicted: 0,
            loads: 0,
            fallbacks: 0,
            missing: 0,
            bad_files: Vec::new(),
        }
    }

    /// [`get_for`](Self::get_for) of a font a profile names.
    pub fn get(
        &mut self,
        face: &str,
        size: i32,
        read: &dyn Fn(&str) -> Option<Vec<u8>>,
    ) -> Option<Found> {
        self.get_for(face, size, FontUse::Profile, read)
    }

    /// The font for `face` at `size` loading it on first use with `read` (a path relative to the root the files are served
    /// from), kept under the quota of `user`. A profile's lookup of a markup font keeps it for the session from then on, and ML
    /// text's lookup of a server GUI's font takes it out of the server's share.
    pub fn get_for(
        &mut self,
        face: &str,
        size: i32,
        user: FontUse,
        read: &dyn Fn(&str) -> Option<Vec<u8>>,
    ) -> Option<Found> {
        let key = (face.to_ascii_lowercase(), size);
        self.clock += 1;
        if let Some(e) = self.fonts.get_mut(&key) {
            e.used = self.clock;
            e.frame = self.frame;
            let tier = match (e.tier, user) {
                (FontUse::Profile, _) | (_, FontUse::Profile) => FontUse::Profile,
                (FontUse::Markup, _) | (_, FontUse::Markup) => FontUse::Markup,
                (FontUse::Server, FontUse::Server) => FontUse::Server,
            };
            if tier != e.tier {
                self.usage.remove(e.tier, e.pixels);
                self.usage.add(tier, e.pixels);
                e.tier = tier;
            }
            return e.found.clone();
        }
        if user == FontUse::Profile && self.fonts.len() - self.usage.markup_keys >= MAX_CACHED_FONTS
        {
            return None;
        }
        // Refused before any work when even the estimate does not fit (a markup refusal is not remembered: asking again costs
        // nothing, and the font may fit once others are dropped).
        if !self.make_room(user, estimated_sheet_pixels(size)) {
            self.over_budget += 1;
            if user == FontUse::Profile {
                self.insert(key, None, user);
            }
            return None;
        }
        let mut found = self.load(face, size, read);
        if let Some((f, _)) = &found
            && !self.make_room(user, sheet_pixels(f))
        {
            // Remembered as missing, so it is not made again on every draw.
            self.over_budget += 1;
            if let Some(dropped) = found.take() {
                self.dropped(&key.0, &dropped);
            }
        }
        self.insert(key, found.clone(), user);
        found
    }

    fn insert(&mut self, key: (String, i32), found: Option<Found>, tier: FontUse) {
        let pixels = found.as_ref().map_or(0, |(f, _)| sheet_pixels(f));
        self.usage.add(tier, pixels);
        let entry = Entry {
            found,
            pixels,
            tier,
            used: self.clock,
            frame: self.frame,
        };
        self.fonts.insert(key, entry);
    }

    /// Whether one more font of `pixels` fits `tier`'s caps at `usage`.
    fn fits(&self, usage: &Usage, tier: FontUse, pixels: u64) -> bool {
        let all = usage.all.saturating_add(pixels) <= self.pixel_cap;
        let markup = usage.markup.saturating_add(pixels) <= self.markup_pixel_cap
            && usage.markup_keys < MAX_MARKUP_FONTS;
        let server = usage.server.saturating_add(pixels) <= self.server_pixel_cap
            && usage.server_keys < MAX_SERVER_FONTS;
        match tier {
            FontUse::Profile => all,
            FontUse::Markup => all && markup,
            FontUse::Server => all && markup && server,
        }
    }

    /// Makes room for one more font of `pixels` sheet pixels under `tier`'s caps, dropping the least recently used markup fonts
    /// that may go (a server's GUI drops only its own); false (and nothing dropped) when even dropping all of them would not
    /// make it fit. A refusal is one pass over the fonts, and asked again in the same frame with nothing changed, none.
    fn make_room(&mut self, tier: FontUse, pixels: u64) -> bool {
        if self.fits(&self.usage, tier, pixels) {
            return true;
        }
        let refusal = Refusal {
            frame: self.frame,
            usage: self.usage,
            tier,
            pixels,
        };
        if self
            .refused
            .is_some_and(|r| r.pixels <= pixels && Refusal { pixels, ..r } == refusal)
        {
            return false;
        }
        let frame = self.frame;
        let mut victims: Vec<(u64, &(String, i32), FontUse, u64)> = self
            .fonts
            .iter()
            .filter(|(_, e)| {
                e.tier.is_markup()
                    && (tier != FontUse::Server || e.tier == FontUse::Server)
                    && e.frame != frame
                    && e.found
                        .as_ref()
                        .is_none_or(|(f, _)| Arc::strong_count(f) == 1)
            })
            .map(|(k, e)| (e.used, k, e.tier, e.pixels))
            .collect();
        let mut all_gone = self.usage;
        for &(_, _, t, px) in &victims {
            all_gone.remove(t, px);
        }
        if !self.fits(&all_gone, tier, pixels) {
            self.refused = Some(refusal);
            return false;
        }
        victims.sort_unstable_by_key(|v| v.0);
        let mut usage = self.usage;
        let mut take = 0;
        for &(_, _, t, px) in &victims {
            if self.fits(&usage, tier, pixels) {
                break;
            }
            usage.remove(t, px);
            take += 1;
        }
        let gone: Vec<(String, i32)> = victims[..take].iter().map(|v| v.1.clone()).collect();
        for key in gone {
            if let Some(e) = self.fonts.remove(&key) {
                self.forget(&key.0, e);
                self.evicted += 1;
            }
        }
        true
    }

    /// Takes a dropped entry's pixels and key off the counts.
    fn forget(&mut self, face: &str, e: Entry) {
        self.usage.remove(e.tier, e.pixels);
        if let Some(found) = &e.found {
            self.dropped(face, found);
        }
    }

    /// Tells the fallback a font it made is gone.
    fn dropped(&self, face: &str, (font, source): &Found) {
        if *source == Source::Fallback {
            self.fallback.dropped(face, font);
        }
    }

    /// Starts a frame: the markup fonts looked up from now on are not dropped until the next one starts. A host that never calls
    /// it drops no markup font, so its markup fonts stop at their caps.
    pub fn next_frame(&mut self) {
        self.frame += 1;
    }

    /// Whether `face` at `size` was looked up and is still kept (found or not): a lookup of it loads nothing.
    pub fn contains(&self, face: &str, size: i32) -> bool {
        self.fonts.contains_key(&(face.to_ascii_lowercase(), size))
    }

    /// The sheet pixels the kept fonts hold.
    pub fn sheet_pixels(&self) -> u64 {
        self.usage.all
    }

    /// The sheet pixels and the faces and sizes the markup fonts hold (a server GUI's included).
    pub fn markup_usage(&self) -> (u64, usize) {
        (self.usage.markup, self.usage.markup_keys)
    }

    /// The sheet pixels and the faces and sizes a server's GUI's fonts hold.
    pub fn server_usage(&self) -> (u64, usize) {
        (self.usage.server, self.usage.server_keys)
    }

    /// Forgets every font of `face` at any size, including the ones nothing made, so the next [`get`](Self::get) looks again. The
    /// sheet pixels of the dropped fonts go back to the budget; the fonts that were kept are returned.
    pub fn remove_face(&mut self, face: &str) -> Vec<Found> {
        let key = face.to_ascii_lowercase();
        let keys: Vec<(String, i32)> = self.fonts.keys().filter(|k| k.0 == key).cloned().collect();
        let mut dropped = Vec::new();
        for k in keys {
            if let Some(e) = self.fonts.remove(&k) {
                dropped.extend(e.found.clone());
                self.forget(&k.0, e);
            }
        }
        dropped
    }

    fn load(
        &mut self,
        face: &str,
        size: i32,
        read: &dyn Fn(&str) -> Option<Vec<u8>>,
    ) -> Option<Found> {
        self.loads += 1;
        let path = format!("{}/{}", self.dir, self.format.file_name(face, size));
        let mut found = None;
        if let Some(bytes) = read(&path) {
            match self.format.parse(&bytes) {
                Ok(f) => found = Some((Arc::new(f), Source::Cache)),
                Err(e) => self.bad_files.push((path, e.to_string())),
            }
        }
        if found.is_none() {
            match self.fallback.make(face, size) {
                Some(f) => {
                    self.fallbacks += 1;
                    found = Some((Arc::new(f), Source::Fallback));
                }
                None => self.missing += 1,
            }
        }
        found
    }

    /// The faces and sizes looked up that nothing made (lower-case face, size), sorted.
    pub fn misses(&self) -> Vec<(String, i32)> {
        let mut v: Vec<(String, i32)> = self
            .fonts
            .iter()
            .filter(|(_, e)| e.found.is_none())
            .map(|(k, _)| k.clone())
            .collect();
        v.sort();
        v
    }

    /// The faces and sizes found, with where each came from (lower-case face, size), sorted.
    pub fn found(&self) -> Vec<(String, i32, Source)> {
        let mut v: Vec<(String, i32, Source)> = self
            .fonts
            .iter()
            .filter_map(|((f, s), e)| e.found.as_ref().map(|(_, src)| (f.clone(), *s, *src)))
            .collect();
        v.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
        v
    }

    /// The fallback's name.
    pub fn fallback_name(&self) -> &'static str {
        self.fallback.name()
    }
}

/// The pixels of a font's sheets.
pub fn sheet_pixels(f: &Font) -> u64 {
    f.sheets
        .iter()
        .map(|s| u64::from(s.width) * u64::from(s.height))
        .sum()
}
