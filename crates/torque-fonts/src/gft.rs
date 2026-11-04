//! TGE's single-byte font caches: the `.gft` files of `GOldFont` (TGE 1.3's `GFont`, file version 1). TGE 1.4's `GFont` reads
//! `.uft` files instead ([`crate::uft`], [`crate::FontCache::new`]'s default); a host whose caches are `.gft` files gives its
//! cache [`Gft`] ([`crate::FontCache::with_format`]). A `.gft` font lays each byte out as its own code unit ([`Units::Bytes`])
//! and draws a tab as three spaces ([`Tab::Spaces`]); a host that draws it as [`Font::str_width`] measures it sets [`Font::tab`].
//!
//! **The file** (all little-endian): `u32 version` (1, else refused), `u32 height`, `u32 baseline`, `u32 glyph count`, then per
//! glyph `i16 bitmapIndex, u8 xOffset, u8 yOffset, u8 width, u8 height, i8 xOrigin, i8 yOrigin, i8 xIncrement` (9 bytes); `u32 sheet
//! count`, each sheet a PNG read by `GBitmap::readPNG` (libpng reads to `IEND` and no further, so the next sheet starts right after
//! it); then the remap, 256 `i16` (the glyph of each byte, -1 for none). Any short read fails the whole file (the stream's status is
//! checked at the end); bytes after the remap are never read.
//!
//! **Where the files are:** `"%s/%s_%d.gft"` of the cache folder, the face name and the size ([`gft_file_name`]).
//!
//! **Credits.** The format follows TGE's `GOldFont::read` and `GOldFont::create` (`dgl/gFont.cc`, TGE 1.4, Copyright (C)
//! GarageGames.com, Inc.; the Torque engine is MIT-licensed as Torque3D, Copyright (c) 2012 GarageGames, LLC). The code here is
//! this crate's own.

use crate::ttf::cp1252;
use crate::{CacheFormat, Font, FontError, Reader, Remap, Tab, Units};

/// The only version `GOldFont::read` accepts.
pub const GFT_VERSION: u32 = 1;
/// The most glyphs one `.gft` may have (a byte font uses at most 256).
pub const MAX_GFT_GLYPHS: u32 = 4096;

/// Reads a `.gft` as `GOldFont::read` does: a font of [`Units::Bytes`] (ascent the baseline, descent the rest of the height).
pub fn parse_gft(bytes: &[u8]) -> Result<Font, FontError> {
    let mut r = Reader::new(bytes)?;
    let version = r.u32("version")?;
    if version != GFT_VERSION {
        return Err(FontError::Version(version));
    }
    let height = r.u32("height")?;
    let baseline = r.u32("baseline")?;
    let glyphs = r.glyphs(MAX_GFT_GLYPHS)?;
    let sheets = r.sheets()?;
    let table = r.take(512, "remap")?;
    let remap = table
        .chunks_exact(2)
        .map(|b| i32::from(i16::from_le_bytes([b[0], b[1]])))
        .collect();
    Ok(Font {
        height,
        baseline,
        ascent: baseline,
        descent: height.saturating_sub(baseline),
        glyphs,
        sheets,
        remap: Remap::new(0, remap),
        units: Units::Bytes,
        tab: Tab::Spaces,
    })
}

/// The cache file's name for a face and size (`"%s/%s_%d.gft"` without the folder).
pub fn gft_file_name(face: &str, size: i32) -> String {
    format!("{face}_{size}.gft")
}

/// The `.gft` cache format.
pub struct Gft;

impl CacheFormat for Gft {
    fn file_name(&self, face: &str, size: i32) -> String {
        gft_file_name(face, size)
    }

    fn parse(&self, bytes: &[u8]) -> Result<Font, FontError> {
        parse_gft(bytes)
    }
}

/// A Unicode font (a TrueType face's, a [`crate::Fallback`]'s) as a single-byte font like a `.gft`'s: byte `b` takes the glyph of
/// its Windows-1252 character, and the text is laid out a byte at a time. The tab rule stays the font's.
pub fn byte_font(font: Font) -> Font {
    let remap = (0..=255u8)
        .map(|b| font.remap.get(u32::from(cp1252(b))))
        .collect();
    Font {
        remap: Remap::new(0, remap),
        units: Units::Bytes,
        ..font
    }
}

#[cfg(test)]
#[path = "gft_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "gft_fuzz.rs"]
mod fuzz;
