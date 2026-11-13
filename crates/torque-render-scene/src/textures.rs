//! Textures as TGE 1.4's `TextureManager` loads them (`dgl/gTexManager.cc`): the name as given, then with each of
//! `.jpg .png .gif .bmp` appended, then the same in the parent folder, folder by folder up to (not into) the root; the image decoded
//! by `images` (PNG by libpng's rules, JPEG by our libjpeg 6b engine) and its mip chain made on the CPU by `extrudeMipLevels`. A
//! shape that names `data/art/props/crate.png` while the files hold `data/art/crate.png` finds it by the parent-folder step (the
//! asset reader matches names ignoring case).
//!
//! Credit: follows TGE 1.4's `dgl/gTexManager.cc` (Copyright (C) GarageGames.com, Inc.); a texture's alpha flag by default scans
//! every texel as Torque3D's MIT-licensed `GBitmap::checkForTransparency` does (`gfx/bitmap/gBitmap.cpp`, Copyright (c) 2012
//! GarageGames, LLC). The code here is this crate's own.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use images::mip::{self, RgbExtrude};
use images::{Format, Kind};

use crate::{Assets, Texture};

/// The extensions `loadBitmapInstance` appends in turn (`extArray`: the name as given first). TGE also tries `.gif` and `.bmp`;
/// this crate has no decoder for them.
const EXTENSIONS: [&str; 3] = ["", ".jpg", ".png"];
/// The most parent folders tried (a path deeper than this is not a game path).
const MAX_PARENT_STEPS: usize = 16;
/// The decoded textures kept between builds hold at most this many texel bytes (the least recently used dropped first; the
/// cache was cleared).
pub const DECODED_CACHE_BYTES: usize = 256 << 20;
/// The most decoded textures kept between builds.
pub const DECODED_CACHE_ENTRIES: usize = 4096;
/// The most texture names whose file a session remembers (`RESOLVED`), and the most it remembers found nothing (`MISSING`).
pub const MAX_RESOLVED_NAMES: usize = 16_384;
/// What an entry counts toward [`DECODED_CACHE_BYTES`] beyond its texels and path.
const ENTRY_COST: usize = 256;
/// The most texel bytes the distinct textures of one frame (one [`TextureSet`]) hold: a texture past it is not loaded (a
/// note says so). Names that resolve to one file share one texture, so the parent-folder search cannot copy a file per name.
pub const MAX_SET_TEXTURE_BYTES: usize = 512 << 20;
/// The most distinct textures, and names looked up, one frame keeps (a flood of tiny files or of names reached no byte cap).
pub const MAX_SET_TEXTURES: usize = 4096;
pub const MAX_SET_NAMES: usize = 16_384;

/// A decoded texture's key: its path, whether its alpha-only texels were whitened, and its file's bytes (length and hash), so the
/// same file read again decodes once while a changed one (a server's blob of the same name) is decoded afresh.
type DecodedKey = (String, bool, usize, u64);

/// The file a texture name resolved to: the candidate as the search found it (the path read), its kind, and its key's file
/// part (lower-case path, length, hash).
#[derive(Clone)]
struct Resolved {
    path: String,
    kind: Kind,
    file: (String, usize, u64),
}

/// The texture loader's work since the session started: the files read (each candidate of a name not yet resolved, or a resolved
/// one whose decoded texture is not kept), the reads that found nothing, and the textures decoded on this thread (the window's
/// frame breakdown).
static READS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static MISSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DECODES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The texture loader's counts so far: (files read, reads that found nothing, textures decoded on this thread).
pub fn loader_counts() -> (u64, u64, u64) {
    use std::sync::atomic::Ordering::Relaxed;
    (
        READS.load(Relaxed),
        MISSES.load(Relaxed),
        DECODES.load(Relaxed),
    )
}

fn counted_read(assets: &dyn Assets, path: &str) -> Option<Vec<u8>> {
    use std::sync::atomic::Ordering::Relaxed;
    READS.fetch_add(1, Relaxed);
    let bytes = assets.read(path);
    if bytes.is_none() {
        MISSES.fetch_add(1, Relaxed);
    }
    bytes
}

thread_local! {
    /// The decoded textures by their key: a live window builds its scene again every half second, and decoding the sky's six
    /// images each time took tens of milliseconds of it. What a decode gives is a function of the key, so it is kept: the texture, or the note.
    static DECODED: RefCell<images::lru::Lru<DecodedKey, Result<Texture, String>>> =
        RefCell::new(images::lru::Lru::new(DECODED_CACHE_ENTRIES, DECODED_CACHE_BYTES));
    /// Each name's file, by the name in lower case, found once a session. A frame's set (the particles make one every frame)
    /// read every candidate of every name and hashed the file before asking [`DECODED`]: 512 spellings of one PNG were 36 ms a frame.
    /// [`clear_resolved`] forgets them.
    static RESOLVED: RefCell<images::lru::Lru<String, Resolved>> =
        RefCell::new(images::lru::Lru::new(MAX_RESOLVED_NAMES, usize::MAX));
    /// The names whose search found nothing, by the name in lower case, with the [`FILES_GENERATION`] of that search. A server's
    /// custom shapes may name textures it never sent: without this, every frame searched each of them again in every parent
    /// folder (thousands of failed reads a second). A name is searched again once the files may have changed ([`files_changed`]).
    static MISSING: RefCell<images::lru::Lru<String, u64>> =
        RefCell::new(images::lru::Lru::new(MAX_RESOLVED_NAMES, usize::MAX));
}

/// Bumped whenever the files a name may resolve to change (a download arrived, a new manifest, a connection gone): the names that
/// found nothing before are searched again.
static FILES_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Forgets which file each texture name resolved to (a new connection's files may differ under the same names), and which names
/// found nothing.
pub fn clear_resolved() {
    RESOLVED.with(|r| r.borrow_mut().clear());
    files_changed();
}

/// The count [`files_changed`] bumps (the shape loader's names that found nothing keep it too).
pub fn files_generation() -> u64 {
    FILES_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

/// The files a texture name may resolve to changed (a file the server sent arrived): the names that found nothing are searched
/// again (on any thread), the names found keep their files.
pub fn files_changed() {
    FILES_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// A file's part of a [`DecodedKey`].
fn file_key(path: &str, bytes: &[u8]) -> (String, usize, u64) {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    // Lower case: the asset reader matches names ignoring case, so `.../T` and `.../t` are one file.
    (path.to_ascii_lowercase(), bytes.len(), h.finish())
}

/// One texture lookup of a [`TextureSet`] while it records: the name, whether it was the modulated form, and what it found
/// (the texture's index in the set and the texture).
#[derive(Debug, Clone)]
pub struct Lookup {
    pub name: String,
    pub modulated: bool,
    pub found: Option<(usize, Texture)>,
}

/// How a texture's alpha flag ([`TextureSet::has_alpha`]) is found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlphaScan {
    /// Any texel with an alpha under 255 (`GBitmap::checkForTransparency`).
    #[default]
    EveryTexel,
    /// Any texel at every `step`th column and row, both below the next power of two of the width (and inside the bitmap),
    /// with an alpha under 255 (a caller's own, cheaper rule).
    Sampled { step: u32 },
}

/// The textures of one frame, each loaded once.
pub struct TextureSet<'a> {
    assets: &'a dyn Assets,
    textures: Vec<Texture>,
    by_name: HashMap<String, Option<usize>>,
    /// Each decoded file's texture (by its [`DecodedKey`]): names that resolve to one file share it.
    by_file: HashMap<DecodedKey, usize>,
    /// The distinct textures' texel bytes, at most [`TextureSet::cap`].
    bytes: usize,
    /// [`MAX_SET_TEXTURE_BYTES`] (a test lowers it).
    pub cap: usize,
    /// Textures that did not load, one line each.
    pub notes: Vec<String>,
    /// Files this set has no decoded texture of are decoded on the decoder thread (`decode_aside`) instead of now, and
    /// the texture is put off ([`TextureSet::deferred`]) until a later set finds it decoded, so a live window's build that meets many
    /// new textures at once does not decode them all in one frame.
    pub aside: bool,
    /// The textures put off (asked again by a later build).
    pub deferred: usize,
    /// Each lookup's name and what it found, while it is `Some` (a caller that keeps a build replays its lookups).
    pub recording: Option<Vec<Lookup>>,
    /// How [`TextureSet::has_alpha`] finds a texture's alpha flag.
    pub alpha_scan: AlphaScan,
}

/// The most decodes waiting for or on the decoder thread (a server's blobs cannot queue more; past it a texture waits for a
/// later build to ask again), and the most file bytes they hold.
pub const MAX_ASIDE_JOBS: usize = 32;
pub const MAX_ASIDE_BYTES: usize = 64 << 20;

/// The decodes the decoder thread finished.
type AsideDone = std::sync::Arc<std::sync::Mutex<Vec<(DecodedKey, Result<Texture, String>)>>>;

/// A decode for the decoder thread: its key, the file's path, kind and bytes, whether alpha-only texels are whitened.
type AsideJob = (DecodedKey, String, Kind, Vec<u8>, bool);

/// The decoder thread's queue and what it finished (the finished results go into this thread's [`DECODED`] when a set asks).
struct Aside {
    jobs: std::sync::mpsc::Sender<AsideJob>,
    done: AsideDone,
    /// The keys queued or decoding, and their file bytes.
    waiting: HashMap<DecodedKey, usize>,
}

impl Aside {
    fn start() -> Option<Aside> {
        let (tx, rx) = std::sync::mpsc::channel::<AsideJob>();
        let done: AsideDone = std::sync::Arc::default();
        let out = std::sync::Arc::clone(&done);
        std::thread::Builder::new()
            .name("texture decoder".into())
            .spawn(move || {
                while let Ok((key, path, kind, bytes, alpha_white)) = rx.recv() {
                    let d = decode_texture(kind, &path, &bytes, alpha_white);
                    out.lock().unwrap_or_else(|e| e.into_inner()).push((key, d));
                }
            })
            .ok()?;
        Some(Aside {
            jobs: tx,
            done,
            waiting: HashMap::new(),
        })
    }

    /// The finished decodes into the decoded cache.
    fn collect(&mut self) {
        let finished = std::mem::take(&mut *self.done.lock().unwrap_or_else(|e| e.into_inner()));
        for (key, d) in finished {
            self.waiting.remove(&key);
            let size = ENTRY_COST
                + key.0.len()
                + d.as_ref()
                    .map_or(0, |t| t.levels.iter().map(Vec::len).sum::<usize>());
            DECODED.with(|c| c.borrow_mut().insert(key, d, size));
        }
    }
}

thread_local! {
    /// This thread's decoder thread (started on first use).
    static ASIDE: RefCell<Option<Aside>> = const { RefCell::new(None) };
}

/// Queues a decode on the decoder thread (once per key while it waits, within [`MAX_ASIDE_JOBS`] and [`MAX_ASIDE_BYTES`]).
fn decode_aside(key: &DecodedKey, path: &str, kind: Kind, bytes: &[u8], alpha_white: bool) {
    ASIDE.with(|a| {
        let mut a = a.borrow_mut();
        if a.is_none() {
            *a = Aside::start();
        }
        let Some(a) = a.as_mut() else { return };
        let held: usize = a.waiting.values().sum();
        if a.waiting.contains_key(key)
            || a.waiting.len() >= MAX_ASIDE_JOBS
            || held + bytes.len() > MAX_ASIDE_BYTES
        {
            return;
        }
        let job = (
            key.clone(),
            path.to_string(),
            kind,
            bytes.to_vec(),
            alpha_white,
        );
        if a.jobs.send(job).is_ok() {
            a.waiting.insert(key.clone(), bytes.len());
        }
    });
}

/// The decodes the decoder thread finished, into the decoded cache.
fn collect_aside() {
    ASIDE.with(|a| {
        if let Some(a) = a.borrow_mut().as_mut() {
            a.collect();
        }
    });
}

/// The RGBA8 texels GL makes of one level's bits (as [`images::Bitmap::to_rgba8`]).
fn rgba8(format: Format, bits: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bits.len() / format.bytes_per_pixel() * 4);
    match format {
        Format::Alpha => bits
            .iter()
            .for_each(|&a| out.extend_from_slice(&[0, 0, 0, a])),
        Format::Rgb => bits
            .as_chunks::<3>()
            .0
            .iter()
            .for_each(|p| out.extend_from_slice(&[p[0], p[1], p[2], 255])),
        Format::Rgba => out.extend_from_slice(bits),
    }
    out
}

/// GL's `GL_UNPACK_ALIGNMENT`: TGE's Windows path never sets it, so GL's default of 4 holds.
const UNPACK_ALIGNMENT: usize = 4;

/// The bytes GL reads for level `i` of a chain: `glTexImage2D` takes each row at a multiple of `UNPACK_ALIGNMENT` (4) from the level's
/// start in the one buffer `GBitmap` keeps every level in, so a level whose rows are not a multiple of four bytes (an RGB level 2
/// pixels wide, a one-channel level under 4 wide) has its later rows read shifted, into the next level's bytes and past the end
/// (taken as 0 here). A 2 x 2 RGB level's texels show this way (water and grounds at a grazing angle).
pub fn gl_unpack(chain: &images::mip::MipChain, i: usize) -> Option<Vec<u8>> {
    let l = chain.levels.get(i)?;
    let bpp = chain.format.bytes_per_pixel();
    let row = l.width as usize * bpp;
    let stride = row.div_ceil(UNPACK_ALIGNMENT) * UNPACK_ALIGNMENT;
    if stride == row || l.height <= 1 {
        return chain.level(i).map(<[u8]>::to_vec);
    }
    let mut out = Vec::with_capacity(row * l.height as usize);
    for r in 0..l.height as usize {
        let at = l.offset + r * stride;
        out.extend((at..at + row).map(|k| chain.bits.get(k).copied().unwrap_or(0)));
    }
    Some(out)
}

/// The names `loadTexture` tries for `name`, in order.
pub fn candidates(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = name.to_string();
    for _ in 0..MAX_PARENT_STEPS {
        for ext in EXTENSIONS {
            out.push(format!("{current}{ext}"));
        }
        // The parent folder: drop the last folder before the file name; never into the root.
        let Some((dir, file)) = current.rsplit_once('/') else {
            break;
        };
        let Some((parent, _)) = dir.rsplit_once('/') else {
            break;
        };
        current = format!("{parent}/{file}");
    }
    out
}

impl<'a> TextureSet<'a> {
    pub fn new(assets: &'a dyn Assets) -> TextureSet<'a> {
        TextureSet {
            assets,
            textures: Vec::new(),
            by_name: HashMap::new(),
            by_file: HashMap::new(),
            bytes: 0,
            cap: MAX_SET_TEXTURE_BYTES,
            notes: Vec::new(),
            aside: false,
            deferred: 0,
            recording: None,
            alpha_scan: AlphaScan::EveryTexel,
        }
    }

    /// The distinct textures' texel bytes.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// How many distinct textures are loaded.
    pub fn len(&self) -> usize {
        self.textures.len()
    }

    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }

    /// The texture `name` (a game path), loaded on first use; `None` (and a note) when no candidate decodes.
    pub fn get(&mut self, name: &str) -> Option<usize> {
        let found = self.get_unrecorded(name);
        self.record(name, false, found);
        found
    }

    fn get_unrecorded(&mut self, name: &str) -> Option<usize> {
        if let Some(found) = self.by_name.get(name) {
            return *found;
        }
        if self.by_name.len() >= MAX_SET_NAMES {
            return None;
        }
        let loaded = self.load(name, false);
        self.by_name.insert(name.to_string(), loaded);
        loaded
    }

    fn record(&mut self, name: &str, modulated: bool, found: Option<usize>) {
        if self.recording.is_some() {
            let found = found.and_then(|i| self.textures.get(i).map(|t| (i, t.clone())));
            if let Some(r) = &mut self.recording {
                r.push(Lookup {
                    name: name.to_string(),
                    modulated,
                    found,
                });
            }
        }
    }

    /// A texture the engine made (a map's lightmap), named by its path: admitted once a set, within the set's caps.
    pub fn generated(&mut self, t: &std::sync::Arc<Texture>) -> Option<usize> {
        let name = format!("{}#generated", t.path);
        if let Some(found) = self.by_name.get(&name) {
            return *found;
        }
        if self.by_name.len() >= MAX_SET_NAMES {
            return None;
        }
        let bytes = t.levels.iter().map(Vec::len).sum();
        let key = (name.clone(), false, bytes, 0);
        let found = self.admit(key, Ok(Texture::clone(t)), &t.path);
        self.by_name.insert(name, found);
        found
    }

    /// Whether texture `i` of the set is `t` (its file's decoded levels, the very same).
    pub fn is_texture(&self, i: usize, t: &Texture) -> bool {
        self.textures.get(i).is_some_and(|u| {
            std::sync::Arc::ptr_eq(&u.levels, &t.levels)
                && u.path == t.path
                && u.width == t.width
                && u.height == t.height
        })
    }

    /// The texture `name` as a `GL_MODULATE` draw sees it: a one-channel bitmap (`GL_ALPHA`) leaves the fragment's colour as it is
    /// (GL 1.x's texture environment table: `Cv = Cf`, `Av = Af * At`), which white texels give under the renderer's modulate.
    pub fn get_modulated(&mut self, name: &str) -> Option<usize> {
        let found = self.get_modulated_unrecorded(name);
        self.record(name, true, found);
        found
    }

    fn get_modulated_unrecorded(&mut self, name: &str) -> Option<usize> {
        let key = format!("{name}#modulate");
        if let Some(found) = self.by_name.get(&key) {
            return *found;
        }
        if self.by_name.len() >= MAX_SET_NAMES {
            return None;
        }
        let loaded = self.load(name, true);
        self.by_name.insert(key, loaded);
        loaded
    }

    fn load(&mut self, name: &str, alpha_white: bool) -> Option<usize> {
        let lower = name.to_ascii_lowercase();
        // A name this session resolved: its file's texture without reading the file, when the frame or the decoded cache has it.
        if let Some(r) = RESOLVED.with(|c| c.borrow_mut().get(&lower).cloned()) {
            let key = (r.file.0.clone(), alpha_white, r.file.1, r.file.2);
            if let Some(&i) = self.by_file.get(&key) {
                return Some(i);
            }
            if let Some(decoded) = DECODED.with(|c| c.borrow_mut().get(&key).cloned()) {
                return self.admit(key, decoded, &r.path);
            }
            if let Some(bytes) = counted_read(self.assets, &r.path) {
                let file = file_key(&r.path, &bytes);
                if file != r.file {
                    let again = Resolved {
                        file: file.clone(),
                        ..r.clone()
                    };
                    RESOLVED.with(|c| c.borrow_mut().insert(lower.clone(), again, 1));
                }
                return self.decode_into(&r.path, r.kind, &bytes, file, alpha_white);
            }
            // Gone since: searched for again.
            RESOLVED.with(|c| c.borrow_mut().remove(&lower));
        }
        let generation = files_generation();
        if MISSING.with(|m| m.borrow_mut().get(&lower).copied()) == Some(generation) {
            self.notes.push(format!("texture {name}: not found"));
            return None;
        }
        for path in candidates(name) {
            let Some(kind) = Kind::of_name(&path) else {
                continue;
            };
            let Some(bytes) = counted_read(self.assets, &path) else {
                continue;
            };
            let file = file_key(&path, &bytes);
            RESOLVED.with(|c| {
                c.borrow_mut().insert(
                    lower.clone(),
                    Resolved {
                        path: path.clone(),
                        kind,
                        file: file.clone(),
                    },
                    1,
                )
            });
            return self.decode_into(&path, kind, &bytes, file, alpha_white);
        }
        MISSING.with(|m| m.borrow_mut().insert(lower, generation, 1));
        self.notes.push(format!("texture {name}: not found"));
        None
    }

    /// The texture of a file read: the frame's when it has it, else the decoded cache's, else decoded now (and kept).
    fn decode_into(
        &mut self,
        path: &str,
        kind: Kind,
        bytes: &[u8],
        file: (String, usize, u64),
        alpha_white: bool,
    ) -> Option<usize> {
        let key = (file.0, alpha_white, file.1, file.2);
        if let Some(&i) = self.by_file.get(&key) {
            return Some(i);
        }
        let mut decoded = DECODED.with(|c| c.borrow_mut().get(&key).cloned());
        // A set that decodes aside takes what the decoder thread finished, else queues it and puts the texture off.
        if decoded.is_none() && self.aside {
            collect_aside();
            decoded = DECODED.with(|c| c.borrow_mut().get(&key).cloned());
            if decoded.is_none() {
                decode_aside(&key, path, kind, bytes, alpha_white);
                self.deferred += 1;
                return None;
            }
        }
        let decoded = match decoded {
            Some(d) => d,
            None => {
                DECODES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let d = decode_texture(kind, path, bytes, alpha_white);
                // Each entry also costs its key and bookkeeping, so a flood of failed or tiny decodes (a server's blobs) still
                // reaches the cap; at most DECODED_CACHE_ENTRIES entries either way, the least recently used dropped first.
                let size = ENTRY_COST
                    + key.0.len()
                    + d.as_ref()
                        .map_or(0, |t| t.levels.iter().map(Vec::len).sum::<usize>());
                DECODED.with(|c| c.borrow_mut().insert(key.clone(), d.clone(), size));
                d
            }
        };
        self.admit(key, decoded, path)
    }

    /// A decoded texture (or its note) into the frame's set, within the set's caps.
    fn admit(
        &mut self,
        key: DecodedKey,
        decoded: Result<Texture, String>,
        path: &str,
    ) -> Option<usize> {
        match decoded {
            Ok(t) => {
                let size: usize =
                    t.levels.iter().map(Vec::len).sum::<usize>() + ENTRY_COST + path.len();
                if self.bytes.saturating_add(size) > self.cap
                    || self.textures.len() >= MAX_SET_TEXTURES
                {
                    self.notes.push(format!(
                        "texture {path}: over the frame's {} texture bytes",
                        self.cap
                    ));
                    return None;
                }
                self.bytes += size;
                self.textures.push(t);
                self.by_file.insert(key, self.textures.len() - 1);
                Some(self.textures.len() - 1)
            }
            Err(note) => {
                self.notes.push(note);
                None
            }
        }
    }

    /// A texture's alpha flag: a texel with an alpha under 255, found as [`TextureSet::alpha_scan`] says. A draw tinted over its
    /// texture picks `GL_DECAL` for a texture with it and `GL_MODULATE` for one without; a map surface with it is translucent.
    pub fn has_alpha(&self, i: usize) -> bool {
        let Some(t) = self.textures.get(i) else {
            return false;
        };
        let Some(level) = t.levels.first() else {
            return false;
        };
        let step = match self.alpha_scan {
            AlphaScan::EveryTexel => {
                let texels = (t.width as usize) * (t.height as usize);
                return level
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .take(texels)
                    .any(|p| p[3] < 255);
            }
            AlphaScan::Sampled { step } => step.max(1) as usize,
        };
        let pw = t.width.max(1).next_power_of_two();
        (0..pw).step_by(step).any(|x| {
            (0..pw).step_by(step).any(|y| {
                x < t.width
                    && y < t.height
                    && level
                        .get(((y * t.width + x) * 4 + 3) as usize)
                        .is_some_and(|&a| a < 255)
            })
        })
    }

    /// The width of a loaded texture.
    pub fn width(&self, i: usize) -> Option<u32> {
        self.textures.get(i).map(|t| t.width)
    }

    /// The game path a loaded texture was read from (a map's kept meshes name their textures by it).
    pub fn path(&self, i: usize) -> Option<&str> {
        self.textures.get(i).map(|t| t.path.as_str())
    }

    /// The height of a loaded texture.
    pub fn height(&self, i: usize) -> Option<u32> {
        self.textures.get(i).map(|t| t.height)
    }

    pub fn into_textures(self) -> Vec<Texture> {
        self.textures
    }
}

/// A texture's file decoded as `loadTexture` makes it: the image, its mip chain extruded, every level down to 1 x 1 as RGBA8 (an
/// alpha-only bitmap's texels whitened for `GL_MODULATE` with `alpha_white`); the note when it does not decode.
fn decode_texture(
    kind: Kind,
    path: &str,
    bytes: &[u8],
    alpha_white: bool,
) -> Result<Texture, String> {
    let bitmap = match images::decode(kind, bytes) {
        Ok(b) => b,
        Err(e) => return Err(format!("texture {path}: {e}")),
    };
    let chain = match mip::extrude(&bitmap, RgbExtrude::default(), false) {
        Ok(c) => c,
        Err(e) => return Err(format!("texture {path}: {e}")),
    };
    // Down to 1 x 1 once (the chain's do-while gives a 1 x 1 bitmap a second level, which GL never samples).
    let mut levels = Vec::new();
    for (i, l) in chain.levels.iter().enumerate() {
        let Some(bits) = gl_unpack(&chain, i) else {
            break;
        };
        let mut texels = rgba8(chain.format, &bits);
        if alpha_white && chain.format == Format::Alpha {
            texels
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .for_each(|t| t[..3].fill(255));
        }
        levels.push(texels);
        if l.width == 1 && l.height == 1 {
            break;
        }
    }
    Ok(Texture {
        format: crate::TexelFormat::Rgba8,
        path: path.to_string(),
        width: bitmap.width,
        height: bitmap.height,
        levels: std::sync::Arc::new(levels),
    })
}

#[cfg(test)]
#[path = "textures_tests.rs"]
pub(crate) mod tests;
