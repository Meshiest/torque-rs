//! The fallback (feature `system-fonts`): a face and size with no cache file available, rasterized by `fontdue` from the system's
//! font file of that name. An approximation of what TGE's GDI path (`WinFont`) would make: the glyphs of the ANSI charset (the
//! Windows-1252 characters of bytes 32 to 255, remapped from their code points), packed into 256-wide sheets. The
//! [`crate::FontCache`] counts every font it makes.
//!
//! **Windows:** a face's file is `{name without spaces}.ttf` in `%WINDIR%\Fonts`, unless the host named another file for it
//! ([`SystemFonts::with_file`]).
//!
//! **Linux and macOS:** the system's font folders (the XDG font folders, or macOS's three) are scanned once per process, within
//! [`MAX_SCAN_DEPTH`], [`MAX_SCAN_ENTRIES`] and [`MAX_INDEXED_FILES`], for `.ttf` files no larger than [`ttf::MAX_TTF_BYTES`]. A
//! face is the file whose name, in lower case without spaces, separators or a trailing `Regular`, is the face's name
//! ([`face_key`]). TGE's common faces fall back on metric-compatible free faces ([`candidates`]: Arial on Liberation Sans, then
//! DejaVu Sans), and when none of those is there, on any sans face found, so text always shows. Every file goes through
//! [`ttf::check`] before fontdue reads it; a file it refuses is passed over.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::ttf::{self, cp1252};
use crate::{Bitmap, Fallback, Font, Glyph, Remap, Tab, Units};

/// The sheet width `GFont::create` packs into, and the tallest sheet.
const SHEET: u32 = 256;
/// The largest size rasterized (a hostile profile's font size).
pub(crate) const MAX_SIZE: i32 = 256;
/// The deepest folder a scan enters below a root (a root's own files are at depth 0).
pub const MAX_SCAN_DEPTH: usize = 6;
/// The most folder entries a scan reads, over every root.
pub const MAX_SCAN_ENTRIES: usize = 50_000;
/// The most font files a scan keeps.
pub const MAX_INDEXED_FILES: usize = 8_192;
/// The most files tried for one face (its own, its stand-ins, then the last resort).
pub const MAX_TRIES: usize = 16;
/// The most faces whose file is remembered.
const MAX_RESOLVED: usize = 1_024;

/// TGE's common faces and the metric-compatible free faces that stand in for them, best first (keys as [`face_key`] makes them).
const SUBSTITUTES: &[(&str, &[&str])] = &[
    ("arial", &["liberationsans", "dejavusans"]),
    ("timesnewroman", &["liberationserif", "dejavuserif"]),
    ("couriernew", &["liberationmono", "dejavusansmono"]),
    ("lucidaconsole", &["liberationmono", "dejavusansmono"]),
];

/// The sans faces tried first when nothing named or standing in is there.
const LAST_RESORT: &[&str] = &[
    "liberationsans",
    "dejavusans",
    "notosans",
    "freesans",
    "arial",
    "helvetica",
    "verdana",
];

/// Style suffixes of a face key, longest first.
const STYLES: &[&str] = &["bolditalic", "boldoblique", "bold", "italic", "oblique"];

/// The system's font files.
pub struct SystemFonts {
    source: Source,
    /// Face names (lower case) and their files, where the file is not the face name without spaces.
    files: HashMap<String, String>,
    /// The file each face (lower case) was made from, or None when none was (scanned folders only).
    resolved: Mutex<HashMap<String, Option<PathBuf>>>,
}

enum Source {
    /// A folder whose faces are found by file name (Windows' fonts folder).
    Folder(PathBuf),
    /// The font files a scan found.
    Scanned(Arc<FontIndex>),
}

impl SystemFonts {
    /// The fonts of this system, when it has any: Windows' fonts folder, or the font folders of Linux or macOS (scanned once per
    /// process, [`native_roots`]). None in a browser, and where no folder holds a font file.
    pub fn new() -> Option<SystemFonts> {
        #[cfg(windows)]
        {
            let windir = std::env::var_os("WINDIR").map(PathBuf::from)?;
            SystemFonts::folder(windir.join("Fonts"))
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            static NATIVE: std::sync::OnceLock<Arc<FontIndex>> = std::sync::OnceLock::new();
            let index = NATIVE.get_or_init(|| Arc::new(FontIndex::scan(&native_roots())));
            (index.files > 0).then(|| SystemFonts::with_source(Source::Scanned(Arc::clone(index))))
        }
        #[cfg(not(any(windows, all(unix, not(target_arch = "wasm32")))))]
        {
            None
        }
    }

    /// The faces of a folder, each found by file name (`{name without spaces}.ttf`), when the folder exists.
    pub fn folder(dir: PathBuf) -> Option<SystemFonts> {
        dir.is_dir()
            .then(|| SystemFonts::with_source(Source::Folder(dir)))
    }

    /// The faces of the font files a scan of `roots` finds, in that order (see the module's notes); the scan is not cached.
    pub fn scan(roots: &[PathBuf]) -> SystemFonts {
        SystemFonts::with_source(Source::Scanned(Arc::new(FontIndex::scan(roots))))
    }

    fn with_source(source: Source) -> SystemFonts {
        SystemFonts {
            source,
            files: HashMap::new(),
            resolved: Mutex::new(HashMap::new()),
        }
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

    /// The font files a scan found (0 for a folder).
    pub fn indexed(&self) -> usize {
        match &self.source {
            Source::Folder(_) => 0,
            Source::Scanned(index) => index.files,
        }
    }

    /// The file a face is made from, of the scanned folders (None for a folder).
    pub fn resolve(&self, face: &str) -> Option<PathBuf> {
        self.load(face).map(|(path, _)| path)
    }

    /// The first file of a face's ([`FontIndex::tries`], at most [`MAX_TRIES`]) that passes the checks, read; remembered per
    /// face.
    fn load(&self, face: &str) -> Option<(PathBuf, fontdue::Font)> {
        let Source::Scanned(index) = &self.source else {
            return None;
        };
        let name = face.to_ascii_lowercase();
        let known = self
            .resolved
            .lock()
            .ok()
            .and_then(|r| r.get(&name).cloned());
        if let Some(known) = known {
            let path = known?;
            let font = read_checked(&path)?;
            return Some((path, font));
        }
        let named = self
            .files
            .get(&name)
            .and_then(|file| index.by_name.get(&file.to_ascii_lowercase()));
        let found = named
            .into_iter()
            .chain(index.tries(face))
            .take(MAX_TRIES)
            .find_map(|path| Some((path.clone(), read_checked(path)?)));
        if let Ok(mut r) = self.resolved.lock()
            && r.len() < MAX_RESOLVED
        {
            r.insert(name, found.as_ref().map(|(path, _)| path.clone()));
        }
        found
    }
}

impl Fallback for SystemFonts {
    fn make(&self, face: &str, size: i32) -> Option<Font> {
        if !(1..=MAX_SIZE).contains(&size) {
            return None;
        }
        let font = match &self.source {
            Source::Folder(dir) => {
                let bytes = std::fs::read(dir.join(self.file_of(face))).ok()?;
                fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()?
            }
            Source::Scanned(_) => self.load(face)?.1,
        };
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

/// A file read whole when it is no larger than [`ttf::MAX_TTF_BYTES`], checked ([`ttf::check`]) and read by fontdue.
fn read_checked(path: &Path) -> Option<fontdue::Font> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(ttf::MAX_TTF_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > ttf::MAX_TTF_BYTES {
        return None;
    }
    ttf::load(&bytes).ok().map(|(font, _)| font)
}

/// The font folders of this system, most personal first: on macOS `~/Library/Fonts`, `/Library/Fonts` and
/// `/System/Library/Fonts`; elsewhere `$XDG_DATA_HOME/fonts`, `~/.local/share/fonts`, `~/.fonts`, `/usr/local/share/fonts` and
/// `/usr/share/fonts`.
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub fn native_roots() -> Vec<PathBuf> {
    // A relative path in these variables is ignored, as the XDG base directory specification says.
    let absolute = |var: &str| {
        std::env::var_os(var)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let home = absolute("HOME");
    let mut roots = Vec::new();
    if cfg!(target_os = "macos") {
        roots.extend(home.iter().map(|h| h.join("Library/Fonts")));
        roots.push(PathBuf::from("/Library/Fonts"));
        roots.push(PathBuf::from("/System/Library/Fonts"));
    } else {
        roots.extend(absolute("XDG_DATA_HOME").map(|d| d.join("fonts")));
        if let Some(h) = &home {
            roots.push(h.join(".local/share/fonts"));
            roots.push(h.join(".fonts"));
        }
        roots.push(PathBuf::from("/usr/local/share/fonts"));
        roots.push(PathBuf::from("/usr/share/fonts"));
    }
    let mut seen = HashSet::new();
    roots.retain(|r| seen.insert(r.clone()));
    roots
}

/// A face's key: its ASCII letters and digits in lower case, without a trailing `regular` (`Liberation Sans`,
/// `LiberationSans-Regular` and `liberation_sans` are all `liberationsans`).
pub fn face_key(name: &str) -> String {
    let key: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    match key.strip_suffix("regular") {
        Some(rest) if !rest.is_empty() => rest.to_string(),
        _ => key,
    }
}

/// The keys tried for a face, best first: its own; for a face of TGE's common ones (Arial, Times New Roman, Courier New, Lucida
/// Console), each metric-compatible stand-in in the same style (an italic also as an oblique); then, for a styled face, the same
/// faces in their regular style.
pub fn candidates(face: &str) -> Vec<String> {
    let key = face_key(face);
    let (family, style) = STYLES
        .iter()
        .find_map(|s| {
            key.strip_suffix(s)
                .filter(|f| !f.is_empty())
                .map(|f| (f.to_string(), *s))
        })
        .unwrap_or_else(|| (key.clone(), ""));
    let subs: &[&str] = SUBSTITUTES
        .iter()
        .find(|(f, _)| *f == family)
        .map_or(&[], |(_, s)| s);
    let mut out = vec![key];
    for sub in subs {
        out.push(format!("{sub}{style}"));
        if style.contains("italic") {
            out.push(format!("{sub}{}", style.replace("italic", "oblique")));
        }
    }
    if !style.is_empty() {
        out.push(family);
        out.extend(subs.iter().map(|&s| s.to_string()));
    }
    let mut seen = HashSet::new();
    out.retain(|k| seen.insert(k.clone()));
    out
}

/// The font files a scan found.
struct FontIndex {
    /// Files by the [`face_key`] of their name, the first found kept.
    by_key: HashMap<String, PathBuf>,
    /// Files by name (lower case), for [`SystemFonts::with_file`].
    by_name: HashMap<String, PathBuf>,
    /// The keys in the order found.
    keys: Vec<String>,
    files: usize,
}

impl FontIndex {
    /// Scans `roots` in order, each folder's entries sorted by name, within the module's limits.
    fn scan(roots: &[PathBuf]) -> FontIndex {
        let mut index = FontIndex {
            by_key: HashMap::new(),
            by_name: HashMap::new(),
            keys: Vec::new(),
            files: 0,
        };
        let mut entries = 0usize;
        for root in roots {
            index.walk(root, 0, &mut entries);
        }
        index
    }

    fn walk(&mut self, dir: &Path, depth: usize, entries: &mut usize) {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths = Vec::new();
        for entry in read {
            if *entries >= MAX_SCAN_ENTRIES {
                break;
            }
            *entries += 1;
            if let Ok(entry) = entry {
                paths.push(entry.path());
            }
        }
        paths.sort();
        let mut dirs = Vec::new();
        for path in paths {
            if self.files >= MAX_INDEXED_FILES {
                return;
            }
            // Links are followed (a linked folder's loop ends at the depth and entry limits).
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                dirs.push(path);
            } else if meta.is_file() && meta.len() <= ttf::MAX_TTF_BYTES as u64 {
                self.add(path);
            }
        }
        if depth < MAX_SCAN_DEPTH {
            for d in dirs {
                self.walk(&d, depth + 1, entries);
            }
        }
    }

    fn add(&mut self, path: PathBuf) {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return;
        };
        let name = name.to_ascii_lowercase();
        let Some(stem) = name.strip_suffix(".ttf") else {
            return;
        };
        let key = face_key(stem);
        if key.is_empty() {
            return;
        }
        self.files += 1;
        if !self.by_key.contains_key(&key) {
            self.keys.push(key.clone());
            self.by_key.insert(key, path.clone());
        }
        self.by_name.entry(name).or_insert(path);
    }

    /// The files tried for a face, in order: its [`candidates`] that were found, then the last resort (the faces of
    /// [`LAST_RESORT`], any regular sans face, any sans face, any face), each file once.
    fn tries(&self, face: &str) -> impl Iterator<Item = &PathBuf> {
        let regular_sans = |k: &&String| {
            k.contains("sans")
                && ![
                    "mono",
                    "serif",
                    "bold",
                    "italic",
                    "oblique",
                    "narrow",
                    "condensed",
                ]
                .iter()
                .any(|s| k.contains(s))
        };
        let named = candidates(face);
        let keys = named
            .iter()
            .map(String::as_str)
            .chain(LAST_RESORT.iter().copied())
            .chain(self.keys.iter().filter(regular_sans).map(String::as_str))
            .chain(
                self.keys
                    .iter()
                    .filter(|k| k.contains("sans"))
                    .map(String::as_str),
            )
            .chain(self.keys.iter().map(String::as_str));
        let mut seen = HashSet::new();
        let paths: Vec<&PathBuf> = keys
            .filter_map(|k| self.by_key.get(k))
            .filter(|p| seen.insert(*p))
            .collect();
        paths.into_iter()
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
