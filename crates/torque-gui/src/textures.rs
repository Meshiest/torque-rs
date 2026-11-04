//! The GUI's textures as TGE 1.4's `TextureManager` makes them: the bitmap found as `loadBitmapInstance` finds it (the name as
//! given, then with `.jpg` and `.png` appended, then the same in each parent folder, never the root), decoded by `images`, padded to
//! powers of two by `createPaddedBitmap` (each row's last pixel repeated to the right, the last row repeated below), and
//! uploaded as `glTexImage2D` reads it (`GL_UNPACK_ALIGNMENT` 4). A GUI texture is a `BitmapTexture`: level 0 only, `GL_LINEAR` both
//! ways (`createGLName`), clamped to the edge when the handle asks for it (GuiBitmapCtrl and GuiBitmapButtonCtrl do,
//! the cursor does not). The font sheets are padded the same way and drawn `GL_NEAREST`.
//!
//! Texels are RGBA8 as the GPU should hold them for `GL_MODULATE`: an RGB bitmap's alpha is 255; a one-channel bitmap (`GL_ALPHA`)
//! keeps the fragment's colour (`Cv = Cf`), which white texels give under a plain modulate.
//!
//! Credit: the padding follows Torque3D's MIT-licensed `GBitmap::createPaddedBitmap` in `gfx/bitmap/gBitmap.cpp` (Copyright
//! (c) 2012 GarageGames, LLC); the code here is this crate's own.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use torque_images::lru::Lru;
use torque_images::{Bitmap, Format, Kind};

/// The extensions `loadBitmapInstance` appends in turn (the name as given first).
const EXTENSIONS: [&str; 3] = ["", ".jpg", ".png"];
/// The most parent folders tried.
const MAX_PARENT_STEPS: usize = 16;
/// GL's default unpack alignment (the texture manager never changes it).
const UNPACK_ALIGNMENT: usize = 4;
/// The most textures one cache keeps, and the most names it remembers (a hostile script could name a new bitmap every frame): the
/// least recently used are dropped first (past it nothing new loaded for the rest of the session).
pub const MAX_TEXTURES: usize = 4096;
/// The largest padded texture side (GL 1.x drivers: 2,048; the PNG reader refuses taller anyway).
pub const MAX_TEXTURE_SIDE: u32 = 4096;
/// The longest texture name looked up.
pub const MAX_NAME_BYTES: usize = 1024;
/// The most texel bytes one cache holds together: past it the least recently drawn are dropped (past it every new bitmap was
/// refused for the rest of the session); what the frame being drawn has used is kept while it draws.
pub const MAX_CACHE_BYTES: usize = 512 << 20;
/// The most notes kept (a missing bitmap named every frame noted once a name).
pub const MAX_NOTES: usize = 256;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A new texture id, one no other texture has.
pub(crate) fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// A texture on its way to the GPU: plain data (Send + Sync), identified by `id` so a renderer uploads it once.
#[derive(Debug, PartialEq)]
pub struct GuiTexture {
    pub id: u64,
    /// The file it came from (or `font:<face>_<size>#<sheet>`).
    pub name: String,
    /// `bitmapWidth`, `bitmapHeight`: the image's own size.
    pub bitmap_size: (u32, u32),
    /// `texWidth`, `texHeight`: the padded size the texture coordinates divide by.
    pub size: (u32, u32),
    /// `size.0 * size.1` RGBA8 texels, rows top to bottom.
    pub rgba: Vec<u8>,
    /// `GL_NEAREST` (font sheets) or `GL_LINEAR`.
    pub nearest: bool,
    /// `GL_CLAMP_TO_EDGE` or `GL_REPEAT`.
    pub clamp: bool,
}

/// The next power of two of a side (0 counts as 1), as `TextureObject`'s sizes are computed.
pub fn pow2(n: u32) -> u32 {
    torque_fonts::texture_side(n)
}

/// `createPaddedBitmap` then `glTexImage2D`'s unpacking, as RGBA8 texels for `GL_MODULATE` (see the module docs).
pub fn padded_texels(bitmap: &Bitmap) -> Option<(u32, u32, Vec<u8>)> {
    let (w, h) = (bitmap.width.max(1), bitmap.height.max(1));
    let (pw, ph) = (pow2(w), pow2(h));
    if pw > MAX_TEXTURE_SIDE || ph > MAX_TEXTURE_SIDE {
        return None;
    }
    let bpp = bitmap.format.bytes_per_pixel();
    if bitmap.bits.len() < w as usize * h as usize * bpp {
        return None;
    }
    // The padded bitmap's bytes, rows `pw * bpp` long.
    let row = pw as usize * bpp;
    let mut padded = vec![0u8; row * ph as usize];
    for y in 0..ph as usize {
        let sy = y.min(h as usize - 1);
        let src = &bitmap.bits[sy * w as usize * bpp..(sy + 1) * w as usize * bpp];
        let dst = &mut padded[y * row..(y + 1) * row];
        dst[..src.len()].copy_from_slice(src);
        let last = &src[src.len() - bpp..];
        for x in w as usize..pw as usize {
            dst[x * bpp..(x + 1) * bpp].copy_from_slice(last);
        }
    }
    // GL reads row r at r * stride (stride rounded up to the alignment), zeros past the end.
    let stride = row.div_ceil(UNPACK_ALIGNMENT) * UNPACK_ALIGNMENT;
    let mut out = Vec::with_capacity(pw as usize * ph as usize * 4);
    for y in 0..ph as usize {
        for x in 0..pw as usize {
            let at = y * stride + x * bpp;
            let b = |k: usize| padded.get(at + k).copied().unwrap_or(0);
            match bitmap.format {
                Format::Alpha => out.extend_from_slice(&[255, 255, 255, b(0)]),
                Format::Rgb => out.extend_from_slice(&[b(0), b(1), b(2), 255]),
                Format::Rgba => out.extend_from_slice(&[b(0), b(1), b(2), b(3)]),
            }
        }
    }
    Some((pw, ph, out))
}

/// The names `loadBitmapInstance` tries for `name`, in order.
pub fn candidates(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = name.to_string();
    for _ in 0..MAX_PARENT_STEPS {
        for ext in EXTENSIONS {
            out.push(format!("{current}{ext}"));
        }
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

/// Makes a texture of a decoded bitmap.
pub fn texture_of(
    name: &str,
    bitmap: &Bitmap,
    nearest: bool,
    clamp: bool,
) -> Option<Arc<GuiTexture>> {
    let (pw, ph, rgba) = padded_texels(bitmap)?;
    Some(Arc::new(GuiTexture {
        id: next_id(),
        name: name.to_string(),
        bitmap_size: (bitmap.width, bitmap.height),
        size: (pw, ph),
        rgba,
        nearest,
        clamp,
    }))
}

/// A texture's key in the cache: the resolved file (lower case), its wrap and its filter.
type FileKey = (String, bool, bool);

/// The textures loaded so far: each name (lower case, with its wrap and filter) resolved once to the file the search found, and
/// each file's texture made once and shared by its names (one `TextureObject` per name; names resolving to one file give
/// equal ones). Both are least-recently-used caches ([`MAX_TEXTURES`], [`MAX_CACHE_BYTES`]); a file whose texture cannot fit with
/// the frame's is not drawn, and its size is remembered so it is not decoded again while it cannot.
pub struct GuiTextures {
    /// Name (lower case), wrap and filter to the file the search found (None: nothing found).
    names: Lru<FileKey, Option<String>>,
    files: Lru<FileKey, Option<Arc<GuiTexture>>>,
    /// Each file's texel bytes once decoded.
    sizes: Lru<FileKey, usize>,
    /// How many times a file was decoded.
    pub decodes: u64,
    /// Names that did not load (`not found`, or the decoder's error), for the reports (at most [`MAX_NOTES`]).
    pub notes: Vec<String>,
}

impl Default for GuiTextures {
    fn default() -> GuiTextures {
        GuiTextures {
            names: Lru::new(MAX_TEXTURES, usize::MAX),
            files: Lru::new(MAX_TEXTURES, MAX_CACHE_BYTES),
            sizes: Lru::new(MAX_TEXTURES, usize::MAX),
            decodes: 0,
            notes: Vec::new(),
        }
    }
}

impl GuiTextures {
    /// The texture `name` (a path relative to the root folder), loaded with `read` on first use; None when no candidate
    /// decodes.
    pub fn get(
        &mut self,
        name: &str,
        clamp: bool,
        read: &dyn Fn(&str) -> Option<Vec<u8>>,
    ) -> Option<Arc<GuiTexture>> {
        self.get_filtered(name, clamp, false, read)
    }

    /// Starts a frame: the textures it uses stay while it draws ([`crate::draw::draw_canvas`] calls it).
    pub fn begin_frame(&mut self) {
        self.files.begin_frame();
    }

    fn note(&mut self, text: String) {
        if self.notes.len() < MAX_NOTES {
            self.notes.push(text);
        }
    }

    /// [`get`](Self::get) with the filter chosen: `nearest` for a texture whose object a control set to `GL_NEAREST`
    /// (`setFilterNearest`: GuiScrollCtrl's profile bitmap).
    pub fn get_filtered(
        &mut self,
        name: &str,
        clamp: bool,
        nearest: bool,
        read: &dyn Fn(&str) -> Option<Vec<u8>>,
    ) -> Option<Arc<GuiTexture>> {
        if name.is_empty() || name.len() > MAX_NAME_BYTES {
            return None;
        }
        let key = (name.to_ascii_lowercase(), clamp, nearest);
        // The file the name resolved to (as the search found it), and its bytes when the search just read them.
        let (path, mut bytes) = match self.names.get(&key) {
            Some(None) => return None,
            Some(Some(path)) => (path.clone(), None),
            None => {
                let Some((path, bytes)) = candidates(name)
                    .into_iter()
                    .filter(|p| Kind::of_name(p).is_some())
                    .find_map(|p| read(&p).map(|b| (p, b)))
                else {
                    self.names.insert(key, None, 1);
                    self.note(format!("texture {name}: not found"));
                    return None;
                };
                self.names.insert(key, Some(path.clone()), 1);
                (path, Some(bytes))
            }
        };
        let fkey = (path.to_ascii_lowercase(), clamp, nearest);
        if let Some(found) = self.files.get(&fkey) {
            return found.clone();
        }
        // A file that cannot fit beside what the frame drew is not decoded again while it cannot.
        if let Some(&size) = self.sizes.peek(&fkey)
            && !self.files.fits(size)
        {
            return None;
        }
        let kind = Kind::of_name(&path)?;
        let bytes = match bytes.take() {
            Some(b) => b,
            None => read(&path)?,
        };
        self.decodes += 1;
        let made = match torque_images::decode(kind, &bytes) {
            Ok(b) => texture_of(&path, &b, nearest, clamp),
            Err(e) => {
                self.note(format!("texture {path}: {e}"));
                None
            }
        };
        let size = made.as_ref().map_or(0, |t| t.rgba.len());
        self.sizes.insert(fkey.clone(), size, 1);
        if !self.files.insert(fkey, made.clone(), size) {
            self.note(format!("texture {path}: too large"));
            return None;
        }
        made
    }

    /// How many names are remembered (loaded or not).
    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The textures held, and their texel bytes.
    pub fn held(&self) -> (usize, usize) {
        (self.files.len(), self.files.bytes())
    }

    /// Lowers the caps (a test's).
    #[cfg(test)]
    pub(crate) fn set_caps(&mut self, entries: usize, bytes: usize) {
        self.files.set_caps(entries, bytes);
        self.names.set_caps(entries, usize::MAX);
    }
}

#[cfg(test)]
#[path = "textures_tests.rs"]
mod tests;
