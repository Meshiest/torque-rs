//! TrueType faces a host registers at run time ([`FaceCache::register`]), looked up before the cache files: a [`FaceCache`] is a
//! [`FontCache`] whose [`Fallback`] makes a registered face's fonts itself and sends every other face to the host's fallback,
//! so the cache's counts, caps and pixel budget apply to them as to every other font. A registered face never reads a cache file.
//!
//! Every font the fallbacks make passes the cache's [`Convert`] hook (a host that lays text out a byte at a time makes them byte
//! fonts there, [`crate::gft::byte_font`]); a cache file's font is the format's.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::ttf::SharedFace;
use crate::{CacheFormat, Fallback, Font, FontCache, FontUse, sheet_pixels};

/// The most faces one cache registers.
pub const MAX_REGISTERED: usize = 8;
/// The most sheet pixels the fonts one cache makes from registered faces may hold together (64 MiB).
pub const MAX_REGISTERED_SHEET_PIXELS: u64 = 64 << 20;

/// What every font the fallbacks make passes through before the cache keeps it.
pub type Convert = fn(Font) -> Font;

/// How a font was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A cache file.
    Cache,
    /// The fallback rasterizer made it.
    Fallback,
    /// A face the host registered.
    Registered,
}

/// A font found and where it came from.
pub type Found = (Arc<Font>, Source);

/// The registered faces and what was made from them, shared with the cache's fallback.
#[derive(Default)]
struct Registry {
    faces: HashMap<String, SharedFace>,
    made: u32,
    made_pixels: u64,
}

/// The cache's fallback: a registered face makes its own fonts; any other face goes to the host's fallback.
struct RegistryFallback {
    registry: Arc<Mutex<Registry>>,
    next: Box<dyn Fallback>,
    convert: Convert,
}

impl Fallback for RegistryFallback {
    fn make(&self, face: &str, size: i32) -> Option<Font> {
        let mut reg = self.registry.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(ttf) = reg.faces.get(&face.to_ascii_lowercase()).cloned() else {
            drop(reg);
            return self.next.make(face, size).map(self.convert);
        };
        let font = ttf.make(size).map(self.convert).filter(|f| {
            let px = sheet_pixels(f);
            let ok = reg.made_pixels + px <= MAX_REGISTERED_SHEET_PIXELS;
            if ok {
                reg.made_pixels += px;
            }
            ok
        })?;
        reg.made += 1;
        Some(font)
    }

    fn name(&self) -> &'static str {
        self.next.name()
    }

    fn dropped(&self, face: &str, font: &Font) {
        let mut reg = self.registry.lock().unwrap_or_else(PoisonError::into_inner);
        if reg.faces.contains_key(face) {
            reg.made_pixels = reg.made_pixels.saturating_sub(sheet_pixels(font));
        } else {
            drop(reg);
            self.next.dropped(face, font);
        }
    }
}

/// Fonts by face and size, loaded once: a registered face's, else the cache file through `read`, else the host's [`Fallback`].
/// Everything [`FontCache`] has (its counts and caps) is reachable through `Deref`.
pub struct FaceCache {
    inner: FontCache,
    registry: Arc<Mutex<Registry>>,
}

impl Deref for FaceCache {
    type Target = FontCache;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl FaceCache {
    /// A cache over the files of `format` in `dir`, every font the fallbacks make passed through `convert`.
    pub fn new(
        dir: &str,
        fallback: Box<dyn Fallback>,
        format: Box<dyn CacheFormat>,
        convert: Convert,
    ) -> FaceCache {
        let registry = Arc::new(Mutex::new(Registry::default()));
        let fallback = RegistryFallback {
            registry: Arc::clone(&registry),
            next: fallback,
            convert,
        };
        FaceCache {
            inner: FontCache::with_format(dir, Box::new(fallback), format),
            registry,
        }
    }

    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(PoisonError::into_inner)
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

    /// The font for `face` at `size`, loading it on first use with `read` (a path relative to the root the files are served from),
    /// kept under the quota of `user` ([`FontCache::get_for`]).
    pub fn get_for(
        &mut self,
        face: &str,
        size: i32,
        user: FontUse,
        read: &dyn Fn(&str) -> Option<Vec<u8>>,
    ) -> Option<Found> {
        let registered = self.is_registered(face);
        let found = if registered {
            // A registered face never reads a cache file.
            self.inner.get_for(face, size, user, &|_| None)
        } else {
            self.inner.get_for(face, size, user, read)
        };
        found.map(|(font, source)| (font, source_of(source, registered)))
    }

    /// [`FontCache::next_frame`].
    pub fn next_frame(&mut self) {
        self.inner.next_frame();
    }

    /// Registers `face` as a TrueType face, looked up before the cache files from now on (by its lower-case name): what this cache
    /// had for the face (none, or a cache file's) is dropped. False when the name is empty or [`MAX_REGISTERED`] faces are
    /// registered already.
    pub fn register(&mut self, face: &str, ttf: SharedFace) -> bool {
        if face.is_empty() {
            return false;
        }
        let key = face.to_ascii_lowercase();
        {
            let reg = self.registry();
            if !reg.faces.contains_key(&key) && reg.faces.len() >= MAX_REGISTERED {
                return false;
            }
        }
        // What is dropped gives its pixels back to the budgets (a face registered before, through `RegistryFallback::dropped`).
        self.inner.remove_face(face);
        self.registry().faces.insert(key, ttf);
        true
    }

    /// Whether `face` is registered.
    pub fn is_registered(&self, face: &str) -> bool {
        self.registry()
            .faces
            .contains_key(&face.to_ascii_lowercase())
    }

    /// How many faces are registered.
    pub fn registered(&self) -> usize {
        self.registry().faces.len()
    }

    /// How many fonts were made from registered faces.
    pub fn made(&self) -> u32 {
        self.registry().made
    }

    /// The faces and sizes found, with where each came from (lower-case face, size), sorted.
    pub fn found(&self) -> Vec<(String, i32, Source)> {
        let reg = self.registry();
        self.inner
            .found()
            .into_iter()
            .map(|(face, size, source)| {
                let registered = reg.faces.contains_key(&face);
                (face, size, source_of(source, registered))
            })
            .collect()
    }
}

fn source_of(source: crate::Source, registered: bool) -> Source {
    match source {
        crate::Source::Cache => Source::Cache,
        crate::Source::Fallback if registered => Source::Registered,
        crate::Source::Fallback => Source::Fallback,
    }
}

#[cfg(test)]
#[path = "faces_tests.rs"]
mod tests;
