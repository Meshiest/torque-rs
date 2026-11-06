//! The fallback (feature `system-fonts`): a face and size with no cache file available, rasterized by `fontdue` from the system's
//! font file of that name. An approximation of what TGE's GDI path (`WinFont`) would make: the glyphs of the ANSI charset (the
//! Windows-1252 characters of bytes 32 to 255, remapped from their code points), packed into 256-wide sheets. The
//! [`crate::FontCache`] counts every font it makes.
//!
//! **Windows:** a face's file is `{name without spaces}.ttf` (any case) among the files of `%WINDIR%\Fonts`, unless the host
//! named another file for it ([`SystemFonts::with_file`]). The name is looked up in the folder's listing, never joined into a
//! path, and a name with a separator, `..`, a colon or NUL has no file.
//!
//! **Linux and macOS:** the system's font folders (the XDG font folders, or macOS's three) are scanned once per process, within
//! [`MAX_SCAN_DEPTH`], [`MAX_SCAN_ENTRIES`], [`MAX_INDEXED_FILES`] and [`MAX_SCAN_TIME`], each folder once, for `.ttf` files no larger than [`ttf::MAX_TTF_BYTES`]. A
//! face is the file whose name, in lower case without spaces, separators or a trailing `Regular`, is the face's name
//! ([`face_key`]). TGE's common faces fall back on metric-compatible free faces ([`candidates`]: Arial on Liberation Sans, then
//! DejaVu Sans), and when none of those is there, on any sans face found, so text always shows.
//!
//! On every system, a file goes through [`ttf::check`] (which caps its size) before fontdue reads it; a file it refuses is passed
//! over. A face read is kept for its other sizes.

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
/// The most faces kept read (each a few hundred KiB to a few MiB of outlines).
const MAX_FACES_KEPT: usize = 16;
/// How long a scan may take before it stops with what it found.
pub const MAX_SCAN_TIME: std::time::Duration = std::time::Duration::from_secs(2);

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
    index: Arc<FontIndex>,
    /// Whether a face whose own file is missing falls back on its stand-ins and the last resort (scanned folders; a folder's face
    /// is its own file only, as `WinFont` finds it).
    stand_ins: bool,
    /// Face names (lower case) and their files, where the file is not the face name without spaces.
    files: HashMap<String, String>,
    /// The file each face (lower case) was made from, or None when none was.
    resolved: Mutex<HashMap<String, Option<PathBuf>>>,
    /// The faces read, by file, so the sizes of a face share one read.
    faces: Mutex<HashMap<PathBuf, Arc<fontdue::Font>>>,
}

impl SystemFonts {
    /// The fonts of this system, when it has any: Windows' fonts folder, or the font folders of Linux or macOS (scanned once per
    /// process, [`native_roots`]). None in a browser, and where no folder holds a font file.
    pub fn new() -> Option<SystemFonts> {
        #[cfg(windows)]
        {
            let windir = std::env::var_os("WINDIR").map(PathBuf::from)?;
            SystemFonts::folder(&windir.join("Fonts"))
        }
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            static NATIVE: std::sync::OnceLock<Arc<FontIndex>> = std::sync::OnceLock::new();
            let index =
                NATIVE.get_or_init(|| Arc::new(FontIndex::scan(&native_roots(), MAX_SCAN_DEPTH)));
            (index.files > 0).then(|| SystemFonts::with_index(Arc::clone(index), true))
        }
        #[cfg(not(any(windows, all(unix, not(target_arch = "wasm32")))))]
        {
            None
        }
    }

    /// The faces of one folder (its own files, no subfolder), each its file `{name without spaces}.ttf` in any case, when the
    /// folder exists.
    #[cfg(any(windows, test))]
    pub(crate) fn folder(dir: &Path) -> Option<SystemFonts> {
        dir.is_dir().then(|| {
            SystemFonts::with_index(Arc::new(FontIndex::scan(&[dir.to_path_buf()], 0)), false)
        })
    }

    /// The faces of the font files a scan of `roots` finds, in that order (see the module's notes); the scan is not cached.
    pub fn scan(roots: &[PathBuf]) -> SystemFonts {
        SystemFonts::with_index(Arc::new(FontIndex::scan(roots, MAX_SCAN_DEPTH)), true)
    }

    fn with_index(index: Arc<FontIndex>, stand_ins: bool) -> SystemFonts {
        SystemFonts {
            index,
            stand_ins,
            files: HashMap::new(),
            resolved: Mutex::new(HashMap::new()),
            faces: Mutex::new(HashMap::new()),
        }
    }

    /// Names the file of a face (any case), for a face whose file is not its name without spaces.
    pub fn with_file(mut self, face: &str, file: &str) -> SystemFonts {
        self.files
            .insert(face.to_ascii_lowercase(), file.to_string());
        self
    }

    /// The font files found.
    pub fn indexed(&self) -> usize {
        self.index.files
    }

    /// The file a face is made from.
    pub fn resolve(&self, face: &str) -> Option<PathBuf> {
        self.load(face).map(|(path, _)| path)
    }

    /// The files tried for a face (lower case), in order: its own file (`{name without spaces}.ttf`, or the one
    /// [`with_file`](SystemFonts::with_file) named), then, in scanned folders, [`FontIndex::tries`]; each once.
    fn tries(&self, name: &str) -> Vec<&PathBuf> {
        let file = match self.files.get(name) {
            Some(file) => file.to_ascii_lowercase(),
            None => format!("{}.ttf", name.replace(' ', "")),
        };
        let own = self.index.by_name.get(&file);
        let mut out: Vec<&PathBuf> = own.into_iter().collect();
        if self.stand_ins {
            out.extend(self.index.tries(name).filter(|p| Some(*p) != own));
        }
        out
    }

    /// The first of a face's files (at most [`MAX_TRIES`]) that passes the checks, read; the file is remembered per face, the
    /// read per file. A face name that could name a path ([`plain_name`]) has none.
    fn load(&self, face: &str) -> Option<(PathBuf, Arc<fontdue::Font>)> {
        if !plain_name(face) {
            return None;
        }
        let name = face.to_ascii_lowercase();
        let known = self
            .resolved
            .lock()
            .ok()
            .and_then(|r| r.get(&name).cloned());
        if let Some(known) = known {
            let path = known?;
            let font = self.read(&path)?;
            return Some((path, font));
        }
        let found = self
            .tries(&name)
            .into_iter()
            .take(MAX_TRIES)
            .find_map(|path| Some((path.clone(), self.read(path)?)));
        if let Ok(mut r) = self.resolved.lock()
            && r.len() < MAX_RESOLVED
        {
            r.insert(name, found.as_ref().map(|(path, _)| path.clone()));
        }
        found
    }

    /// A file's face ([`read_checked`]), read once while fewer than [`MAX_FACES_KEPT`] are kept.
    fn read(&self, path: &Path) -> Option<Arc<fontdue::Font>> {
        if let Some(font) = self.faces.lock().ok().and_then(|f| f.get(path).cloned()) {
            return Some(font);
        }
        let font = Arc::new(read_checked(path)?);
        if let Ok(mut f) = self.faces.lock()
            && f.len() < MAX_FACES_KEPT
        {
            f.insert(path.to_path_buf(), Arc::clone(&font));
        }
        Some(font)
    }
}

/// Whether a face name is a name only: not empty, with no path separator, `..`, drive letter (a colon) or NUL in it.
fn plain_name(face: &str) -> bool {
    !face.is_empty() && !face.contains(['/', '\\', ':', '\0']) && !face.contains("..")
}

impl Fallback for SystemFonts {
    fn make(&self, face: &str, size: i32) -> Option<Font> {
        if !(1..=MAX_SIZE).contains(&size) {
            return None;
        }
        let (_, font) = self.load(face)?;
        rasterize(&font, size)
    }

    fn name(&self) -> &'static str {
        "system fonts (fontdue)"
    }
}

/// The font of a face at `size` pixels: the ANSI charset's glyphs, packed ([`pack`]).
fn rasterize(font: &fontdue::Font, size: i32) -> Option<Font> {
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

/// A scan's running limits.
struct Walk {
    entries: usize,
    max_depth: usize,
    /// The folders read (their canonical paths).
    visited: HashSet<PathBuf>,
    deadline: Option<std::time::Instant>,
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
    /// Scans `roots` in order, each folder's entries sorted by name, `depth` folders deep at most, within the module's limits.
    fn scan(roots: &[PathBuf], depth: usize) -> FontIndex {
        let mut index = FontIndex {
            by_key: HashMap::new(),
            by_name: HashMap::new(),
            keys: Vec::new(),
            files: 0,
        };
        let mut walk = Walk {
            entries: 0,
            max_depth: depth,
            visited: HashSet::new(),
            // A browser has no clock to read here (and no folders to scan).
            deadline: (!cfg!(target_arch = "wasm32"))
                .then(|| std::time::Instant::now() + MAX_SCAN_TIME),
        };
        for root in roots {
            index.walk(root, 0, &mut walk);
        }
        index
    }

    fn walk(&mut self, dir: &Path, depth: usize, walk: &mut Walk) {
        if walk
            .deadline
            .is_some_and(|d| std::time::Instant::now() >= d)
        {
            return;
        }
        // Each folder once, however it is reached (a linked folder, a root inside another, a link loop).
        let Ok(real) = std::fs::canonicalize(dir) else {
            return;
        };
        if !walk.visited.insert(real) {
            return;
        }
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths = Vec::new();
        for entry in read {
            if walk.entries >= MAX_SCAN_ENTRIES {
                break;
            }
            walk.entries += 1;
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
            // Links are followed.
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                dirs.push(path);
            } else if meta.is_file() && meta.len() <= ttf::MAX_TTF_BYTES as u64 {
                self.add(path);
            }
        }
        if depth < walk.max_depth {
            for d in dirs {
                self.walk(&d, depth + 1, walk);
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

#[cfg(test)]
#[path = "system_tests.rs"]
mod tests;
