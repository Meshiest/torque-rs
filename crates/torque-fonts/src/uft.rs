//! The `.uft` font cache: the file TGE 1.4's `GFont` reads and writes (version 3), all little-endian:
//!
//! `u32 version` (3, else refused); the face name (`u8` length, then that many bytes); `u32 size`, `u32 charset`, `u32 height`,
//! `u32 baseline`, `u32 ascent`, `u32 descent`; `u32` glyph count, then per glyph `i16 bitmapIndex, u8 xOffset, u8 yOffset,
//! u8 width, u8 height, i8 xOrigin, i8 yOrigin, i8 xIncrement` (9 bytes); `u32` sheet count, each sheet a PNG read by
//! `GBitmap::readPNG`; the packing pen (`i32 curX, curY, curSheet`); then the remap: `u32 minGlyph, u32 maxGlyph`, and when
//! `maxGlyph >= minGlyph` a `u32` length and that many bytes of zlib data holding the entries of `minGlyph..=maxGlyph` as
//! big-endian `i32`s (every other code unit has none). Any short read fails the whole file; bytes after the remap are never read.
//!
//! The file's name is `"%s %d (%s).uft"` of the face, the size and the charset's name ([`file_name`]).
//!
//! `GFont::read` and `GFont::write` are the Torque Game Engine's, carried into the MIT-licensed Torque3D's `gfx/gFont.cpp`
//! (Copyright (c) 2012 GarageGames, LLC); see the crate documentation.

use crate::{CacheFormat, Font, FontError, MAX_GLYPHS, Reader, Remap, Tab, Units};

/// The only version `GFont::read` accepts.
pub const VERSION: u32 = 3;
/// The highest code unit a remap may name (TGE's table has 65,536 entries).
pub const MAX_CODE_UNIT: u32 = 0xffff;
/// `TGE_ANSI_CHARSET`, the charset `GFont::create` uses unless told otherwise.
pub const ANSI_CHARSET: u32 = 0;
/// The charsets' names in the file names (`getCharSetName`), by number.
pub const CHARSET_NAMES: [&str; 19] = [
    "ansi",
    "symbol",
    "shiftjis",
    "hangeul",
    "hangul",
    "gb2312",
    "chinesebig5",
    "oem",
    "johab",
    "hebrew",
    "arabic",
    "greek",
    "turkish",
    "vietnamese",
    "thai",
    "easteurope",
    "russian",
    "mac",
    "baltic",
];

/// A charset's name, None for a number TGE has none for.
pub fn charset_name(charset: u32) -> Option<&'static str> {
    CHARSET_NAMES.get(charset as usize).copied()
}

/// The cache file's name for a face, size and charset (`"%s %d (%s).uft"` without the folder).
pub fn file_name(face: &str, size: i32, charset: u32) -> Option<String> {
    Some(format!("{face} {size} ({}).uft", charset_name(charset)?))
}

/// A `.uft` read: the font and what else the file records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UftFile {
    /// The face name as stored (up to 255 bytes).
    pub face: Vec<u8>,
    pub size: u32,
    pub charset: u32,
    /// Where the next glyph would be packed: `curX`, `curY`, `curSheet`.
    pub pen: [i32; 3],
    pub font: Font,
}

impl UftFile {
    /// Reads a `.uft` as `GFont::read` does.
    pub fn parse(bytes: &[u8]) -> Result<UftFile, FontError> {
        let mut r = Reader::new(bytes)?;
        let version = r.u32("version")?;
        if version != VERSION {
            return Err(FontError::Version(version));
        }
        let name_len = r.u8("face name")?;
        let face = r.take(name_len as usize, "face name")?.to_vec();
        let size = r.u32("size")?;
        let charset = r.u32("charset")?;
        let height = r.u32("height")?;
        let baseline = r.u32("baseline")?;
        let ascent = r.u32("ascent")?;
        let descent = r.u32("descent")?;
        let glyphs = r.glyphs(MAX_GLYPHS)?;
        let sheets = r.sheets()?;
        let pen = [r.i32("pen")?, r.i32("pen")?, r.i32("pen")?];
        let min = r.u32("remap range")?;
        let max = r.u32("remap range")?;
        let remap = if max >= min {
            if max > MAX_CODE_UNIT {
                return Err(FontError::Cap {
                    what: "remap code unit",
                    value: u64::from(max),
                    cap: u64::from(MAX_CODE_UNIT),
                });
            }
            let len = r.u32("remap length")?;
            let packed = r.take(len as usize, "remap")?;
            let want = (max - min + 1) as usize * 4;
            let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(packed, want)
                .map_err(|e| FontError::Remap(format!("{:?}", e.status)))?;
            if raw.len() != want {
                return Err(FontError::Remap(format!(
                    "{} bytes for {} entries",
                    raw.len(),
                    want / 4
                )));
            }
            let entries = raw
                .chunks_exact(4)
                .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
                .collect();
            Remap::new(min, entries)
        } else {
            Remap::default()
        };
        Ok(UftFile {
            face,
            size,
            charset,
            pen,
            font: Font {
                height,
                baseline,
                ascent,
                descent,
                glyphs,
                sheets,
                remap,
                units: Units::Utf16,
                tab: Tab::Spaces,
            },
        })
    }
}

/// The `.uft` files of one charset, as [`crate::FontCache::new`] reads them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Uft {
    charset: u32,
}

impl Default for Uft {
    fn default() -> Uft {
        Uft {
            charset: ANSI_CHARSET,
        }
    }
}

impl Uft {
    /// The files of `charset`, None for a number TGE has no name for.
    pub fn new(charset: u32) -> Option<Uft> {
        charset_name(charset).map(|_| Uft { charset })
    }
}

impl CacheFormat for Uft {
    fn file_name(&self, face: &str, size: i32) -> String {
        file_name(face, size, self.charset).unwrap_or_default()
    }

    fn parse(&self, bytes: &[u8]) -> Result<Font, FontError> {
        Font::parse(bytes)
    }
}
