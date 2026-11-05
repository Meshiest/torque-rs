//! The fallback (feature `system-fonts`): a face and size with no cache file available, rasterized by `fontdue` from the system's
//! font file of that name (Windows: `%WINDIR%\Fonts`). An approximation of what TGE's GDI path (`WinFont`) would make: the glyphs
//! of the ANSI charset (the Windows-1252 characters of bytes 32 to 255, remapped from their code points), packed into 256-wide
//! sheets. The [`crate::FontCache`] counts every font it makes.
//!
//! A face's file is `{name without spaces}.ttf` in that folder, unless the host named another file for it
//! ([`SystemFonts::with_file`]).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::ttf::cp1252;
use crate::{Bitmap, Fallback, Font, Glyph, Remap, Tab, Units};

/// The sheet width `GFont::create` packs into, and the tallest sheet.
const SHEET: u32 = 256;
/// The largest size rasterized (a hostile profile's font size).
pub(crate) const MAX_SIZE: i32 = 256;

/// The system's font files.
pub struct SystemFonts {
    dir: PathBuf,
    /// Face names (lower case) and their files, where the file is not the face name without spaces.
    files: HashMap<String, String>,
}

impl SystemFonts {
    /// The fonts folder of this system, when it has one.
    pub fn new() -> Option<SystemFonts> {
        let windir = std::env::var_os("WINDIR").map(PathBuf::from)?;
        let dir = windir.join("Fonts");
        dir.is_dir().then(|| SystemFonts {
            dir,
            files: HashMap::new(),
        })
    }

    /// Names the file of a face (any case), for a face whose file is not its name without spaces.
    pub fn with_file(mut self, face: &str, file: &str) -> SystemFonts {
        self.files
            .insert(face.to_ascii_lowercase(), file.to_string());
        self
    }

    fn file_of(&self, face: &str) -> String {
        let face = face.to_ascii_lowercase();
        match self.files.get(&face) {
            Some(file) => file.clone(),
            None => format!("{}.ttf", face.replace(' ', "")),
        }
    }
}

impl Fallback for SystemFonts {
    fn make(&self, face: &str, size: i32) -> Option<Font> {
        if !(1..=MAX_SIZE).contains(&size) {
            return None;
        }
        let bytes = std::fs::read(self.dir.join(self.file_of(face))).ok()?;
        let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()?;
        let px = size as f32;
        let lines = font.horizontal_line_metrics(px)?;
        let ascent = lines.ascent.ceil() as i32;
        let height = (lines.ascent - lines.descent).ceil().max(1.0) as u32;
        Some(pack(ascent, height, |c| {
            let (m, coverage) = font.rasterize(c, px);
            GlyphImage {
                width: m.width,
                height: m.height,
                coverage,
                xmin: m.xmin,
                ymin: m.ymin,
                advance: m.advance_width,
            }
        }))
    }

    fn name(&self) -> &'static str {
        "system fonts (fontdue)"
    }
}

/// A glyph as a rasterizer gives it: its coverage (`width` x `height`, a row after another), its left edge and its bottom edge
/// (up from the baseline), and its advance.
pub(crate) struct GlyphImage {
    pub width: usize,
    pub height: usize,
    pub coverage: Vec<u8>,
    pub xmin: i32,
    pub ymin: i32,
    pub advance: f32,
}

/// The font of the ANSI charset's glyphs (bytes 32 to 255 as Windows-1252 characters) that `glyph` rasterizes, packed into
/// [`SHEET`]-wide sheets, with `ascent` and line `height`.
pub(crate) fn pack(ascent: i32, height: u32, mut glyph: impl FnMut(char) -> GlyphImage) -> Font {
    let mut glyphs = Vec::new();
    let mut remap = Vec::new();
    let mut sheets: Vec<Bitmap> = Vec::new();
    let (mut x, mut y, mut row_h) = (0u32, 0u32, 0u32);
    let mut sheet = Bitmap {
        width: SHEET,
        height: SHEET,
        format: torque_images::Format::Alpha,
        bits: vec![0; (SHEET * SHEET) as usize],
    };
    for b in 32u8..=255 {
        let m = glyph(cp1252(b));
        let (w, h) = (m.width.min(255) as u32, m.height.min(255) as u32);
        if x + w > SHEET {
            x = 0;
            y += row_h;
            row_h = 0;
        }
        if y + h > SHEET {
            sheets.push(std::mem::replace(
                &mut sheet,
                Bitmap {
                    width: SHEET,
                    height: SHEET,
                    format: torque_images::Format::Alpha,
                    bits: vec![0; (SHEET * SHEET) as usize],
                },
            ));
            x = 0;
            y = 0;
            row_h = 0;
        }
        for r in 0..h {
            for c in 0..w {
                sheet.bits[((y + r) * SHEET + x + c) as usize] =
                    m.coverage[(r as usize) * m.width + c as usize];
            }
        }
        remap.push((u32::from(cp1252(b)), glyphs.len() as i32));
        glyphs.push(Glyph {
            bitmap_index: sheets.len() as i16,
            x_offset: x as u8,
            y_offset: y as u8,
            width: w as u8,
            height: h as u8,
            x_origin: m.xmin.clamp(-128, 127) as i8,
            y_origin: (m.ymin + m.height as i32).clamp(-128, 127) as i8,
            x_increment: m.advance.round().clamp(-128.0, 127.0) as i8,
        });
        x += w;
        row_h = row_h.max(h);
    }
    sheets.push(sheet);
    let baseline = ascent.max(0) as u32;
    Font {
        height,
        baseline,
        ascent: baseline,
        descent: height.saturating_sub(baseline),
        glyphs,
        sheets,
        remap: Remap::from_pairs(&remap),
        units: Units::Utf16,
        tab: Tab::Spaces,
    }
}
