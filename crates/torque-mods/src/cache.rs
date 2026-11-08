//! The cache of content downloaded from servers: one entry per content hash, in a per-user folder outside the game's.
//!
//! An entry is the content message a server sent ([`crate::content::encode_content`]) under the file name `<hash>.mod`, the hash in
//! lower case hexadecimal. Nothing in the cache is trusted when it is read back: [`ContentCache::load`] reads the entry as it
//! read the download (the same caps, no script paths) and hashes its files, and an entry whose files do not hash to the name it
//! has is deleted. A folder listing is read the same way ([`index_of`]): names that are not entries are ignored, so a cache
//! folder full of junk or hand-made names does no harm.
//!
//! The cache is bounded: [`ContentCache::store`] evicts the entries used least recently until the folder is within its cap, and an
//! entry above the cap is not kept at all. On the web, and when no per-user folder is known, the cache lives in memory for the
//! session.
//!
//! Only content is stored, so nothing in the cache can run: the folder is never a script path, and the files in it are read back
//! only through the content rules above.

use std::collections::HashMap;

use crate::content::{ContentCaps, Hash, decode_content, from_hex, hex, summarize_files};

/// The suffix of an entry's file name.
pub const ENTRY_SUFFIX: &str = ".mod";

/// The most entries a listing is read for.
pub const MAX_ENTRIES: usize = 4_096;

/// One file of a cache as a store lists it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Listed {
    /// The file's name.
    pub name: String,
    /// Its size in bytes.
    pub size: u64,
    /// When it was last used, in seconds since an epoch the store picks (larger is later).
    pub stamp: u64,
}

/// Where a cache keeps its entries.
pub trait CacheStore {
    /// The files, at most [`MAX_ENTRIES`] of them (more are not listed).
    fn list(&self) -> Vec<Listed>;
    /// A file's bytes, up to `max` of them (`None` for a file that is missing or longer).
    fn read(&self, name: &str, max: u64) -> Option<Vec<u8>>;
    /// The first `n` bytes of a file (fewer when it is shorter).
    fn read_head(&self, name: &str, n: usize) -> Option<Vec<u8>>;
    /// Writes a file whole (replacing one of the same name).
    fn write(&mut self, name: &str, bytes: &[u8]) -> bool;
    /// Deletes a file.
    fn remove(&mut self, name: &str);
    /// Marks a file as used `stamp`.
    fn touch(&mut self, name: &str, stamp: u64);
}

/// An entry of the index: a hash in the cache.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IndexEntry {
    /// The content hash.
    pub hash: Hash,
    /// The entry's size.
    pub size: u64,
    /// When it was last used.
    pub stamp: u64,
}

/// The entries among `listed`: files named `<64 hex digits>.mod` and nothing else, each hash once, the first `MAX_ENTRIES`.
pub fn index_of(listed: &[Listed]) -> Vec<IndexEntry> {
    let mut seen: HashMap<Hash, usize> = HashMap::new();
    let mut entries: Vec<IndexEntry> = Vec::new();
    for item in listed.iter().take(MAX_ENTRIES.saturating_mul(2)) {
        let Some(stem) = item.name.strip_suffix(ENTRY_SUFFIX) else {
            continue;
        };
        let Some(hash) = from_hex(stem) else {
            continue;
        };
        let entry = IndexEntry {
            hash,
            size: item.size,
            stamp: item.stamp,
        };
        match seen.get(&hash) {
            Some(&at) => {
                if let Some(slot) = entries.get_mut(at) {
                    *slot = entry;
                }
            }
            None if entries.len() < MAX_ENTRIES => {
                seen.insert(hash, entries.len());
                entries.push(entry);
            }
            None => {}
        }
    }
    entries
}

fn entry_name(hash: &Hash) -> String {
    format!("{}{ENTRY_SUFFIX}", hex(hash))
}

/// A cache of downloaded content.
pub struct ContentCache {
    store: Box<dyn CacheStore>,
    max_bytes: u64,
    clock: u64,
}

impl ContentCache {
    /// A cache over `store` that keeps at most `max_bytes`.
    pub fn new(store: Box<dyn CacheStore>, max_bytes: u64) -> ContentCache {
        let clock = store.list().iter().map(|l| l.stamp).max().unwrap_or(0);
        ContentCache {
            store,
            max_bytes,
            clock,
        }
    }

    /// A cache in memory.
    pub fn in_memory(max_bytes: u64) -> ContentCache {
        ContentCache::new(Box::new(MemoryStore::default()), max_bytes)
    }

    /// Changes the cap (the next store evicts to it).
    pub fn set_max_bytes(&mut self, max_bytes: u64) {
        self.max_bytes = max_bytes;
    }

    /// The cap.
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    fn tick(&mut self) -> u64 {
        self.clock = self.clock.saturating_add(1);
        self.clock
    }

    /// The entries now.
    pub fn index(&self) -> Vec<IndexEntry> {
        index_of(&self.store.list())
    }

    /// The bytes the entries take.
    pub fn bytes(&self) -> u64 {
        self.index().iter().map(|e| e.size).sum()
    }

    /// What the entries are: the id, version and hash each one's start says, most recently used first, at most `max` of them. A
    /// start that does not read as a content message is skipped (and the entry left for a load to delete).
    pub fn heads(&self, max: usize) -> Vec<(String, String, Hash)> {
        let mut entries = self.index();
        entries.sort_by_key(|e| std::cmp::Reverse(e.stamp));
        entries
            .iter()
            .filter_map(|e| {
                let head = self
                    .store
                    .read_head(&entry_name(&e.hash), crate::content::HEAD_BYTES)?;
                let (id, version, hash) = crate::content::peek_head(&head)?;
                (hash == e.hash).then_some((id, version, hash))
            })
            .take(max)
            .collect()
    }

    /// Whether the cache has an entry for `hash` (not verified).
    pub fn has(&self, hash: &Hash) -> bool {
        self.index().iter().any(|e| &e.hash == hash)
    }

    /// The files of the entry for `hash`, read under `caps` and checked: they must hash to `hash`. An entry that does not read or
    /// does not hash right is deleted. A hit counts as a use.
    pub fn load(&mut self, hash: &Hash, caps: &ContentCaps) -> Option<Vec<(String, Vec<u8>)>> {
        let name = entry_name(hash);
        let limit = caps
            .total_bytes
            .saturating_add(caps.files as u64 * 300 + 4_096);
        let bytes = self.store.read(&name, limit)?;
        let files = match decode_content(&bytes, caps) {
            Ok(content) if &content.hash == hash => content.files,
            _ => {
                self.store.remove(&name);
                return None;
            }
        };
        if &summarize_files(&files).hash != hash {
            self.store.remove(&name);
            return None;
        }
        let stamp = self.tick();
        self.store.touch(&name, stamp);
        Some(files)
    }

    /// Keeps the content message `message` (already checked against `hash` by the receiver) and evicts the entries used least
    /// recently while the cache is over its cap. A message over the cap is not kept; the result says whether it was.
    pub fn store(&mut self, hash: &Hash, message: &[u8]) -> bool {
        if message.len() as u64 > self.max_bytes {
            return false;
        }
        let name = entry_name(hash);
        if !self.store.write(&name, message) {
            return false;
        }
        let stamp = self.tick();
        self.store.touch(&name, stamp);
        self.evict(Some(hash));
        true
    }

    /// Deletes the least recently used entries until the cache is within its cap; `keep` is never deleted.
    pub fn evict(&mut self, keep: Option<&Hash>) {
        let mut entries = self.index();
        let mut total: u64 = entries.iter().map(|e| e.size).sum();
        entries.sort_by_key(|e| e.stamp);
        for entry in entries {
            if total <= self.max_bytes {
                break;
            }
            if keep == Some(&entry.hash) {
                continue;
            }
            self.store.remove(&entry_name(&entry.hash));
            total = total.saturating_sub(entry.size);
        }
    }

    /// Deletes every entry.
    pub fn clear(&mut self) {
        for entry in self.index() {
            self.store.remove(&entry_name(&entry.hash));
        }
    }
}

// ---- stores

/// A store in memory.
#[derive(Default)]
pub struct MemoryStore {
    files: HashMap<String, (Vec<u8>, u64)>,
}

impl CacheStore for MemoryStore {
    fn list(&self) -> Vec<Listed> {
        self.files
            .iter()
            .take(MAX_ENTRIES.saturating_mul(2))
            .map(|(name, (bytes, stamp))| Listed {
                name: name.clone(),
                size: bytes.len() as u64,
                stamp: *stamp,
            })
            .collect()
    }

    fn read(&self, name: &str, max: u64) -> Option<Vec<u8>> {
        self.files
            .get(name)
            .filter(|(bytes, _)| bytes.len() as u64 <= max)
            .map(|(bytes, _)| bytes.clone())
    }

    fn read_head(&self, name: &str, n: usize) -> Option<Vec<u8>> {
        self.files
            .get(name)
            .map(|(bytes, _)| bytes.iter().take(n).copied().collect())
    }

    fn write(&mut self, name: &str, bytes: &[u8]) -> bool {
        self.files.insert(name.to_string(), (bytes.to_vec(), 0));
        true
    }

    fn remove(&mut self, name: &str) {
        self.files.remove(name);
    }

    fn touch(&mut self, name: &str, stamp: u64) {
        if let Some(file) = self.files.get_mut(name) {
            file.1 = stamp;
        }
    }
}

/// A store in a folder on the disk. The folder is made when something is first written. Only plain files directly inside it are
/// listed or read: links, folders and anything deeper are left alone.
#[cfg(not(target_family = "wasm"))]
pub struct DiskStore {
    dir: std::path::PathBuf,
}

#[cfg(not(target_family = "wasm"))]
impl DiskStore {
    /// A store in `dir`. Half-written files an earlier run left (`*.part`) are removed.
    pub fn new(dir: impl Into<std::path::PathBuf>) -> DiskStore {
        let store = DiskStore { dir: dir.into() };
        store.sweep();
        store
    }

    /// Removes the plain files named `*.part`: what a run that ended in the middle of a write left.
    fn sweep(&self) {
        let Ok(dir) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for item in dir.take(MAX_ENTRIES.saturating_mul(4)).flatten() {
            let stray = item.file_name().to_string_lossy().ends_with(".part");
            let plain = std::fs::symlink_metadata(item.path()).is_ok_and(|m| m.is_file());
            if stray && plain {
                let _ = std::fs::remove_file(item.path());
            }
        }
    }

    fn path(&self, name: &str) -> Option<std::path::PathBuf> {
        // Names the cache makes: no separators, no dots beyond the suffix.
        let ok = name.len() <= 80 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.');
        ok.then(|| self.dir.join(name))
    }
}

#[cfg(not(target_family = "wasm"))]
fn seconds(time: std::io::Result<std::time::SystemTime>) -> u64 {
    time.ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

#[cfg(not(target_family = "wasm"))]
impl CacheStore for DiskStore {
    fn list(&self) -> Vec<Listed> {
        let Ok(dir) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut listed = Vec::new();
        for item in dir.take(MAX_ENTRIES.saturating_mul(4)).flatten() {
            let Ok(meta) = std::fs::symlink_metadata(item.path()) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let Some(name) = item.file_name().to_str().map(str::to_string) else {
                continue;
            };
            listed.push(Listed {
                name,
                size: meta.len(),
                stamp: seconds(meta.modified()),
            });
        }
        listed
    }

    fn read(&self, name: &str, max: u64) -> Option<Vec<u8>> {
        use std::io::Read;
        let path = self.path(name)?;
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if !meta.is_file() || meta.len() > max {
            return None;
        }
        let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).ok()?);
        std::fs::File::open(&path)
            .ok()?
            .take(max.saturating_add(1))
            .read_to_end(&mut bytes)
            .ok()?;
        (bytes.len() as u64 <= max).then_some(bytes)
    }

    fn read_head(&self, name: &str, n: usize) -> Option<Vec<u8>> {
        use std::io::Read;
        let path = self.path(name)?;
        if !std::fs::symlink_metadata(&path).ok()?.is_file() {
            return None;
        }
        let mut bytes = Vec::with_capacity(n);
        std::fs::File::open(&path)
            .ok()?
            .take(n as u64)
            .read_to_end(&mut bytes)
            .ok()?;
        Some(bytes)
    }

    fn write(&mut self, name: &str, bytes: &[u8]) -> bool {
        let Some(path) = self.path(name) else {
            return false;
        };
        if std::fs::create_dir_all(&self.dir).is_err() {
            return false;
        }
        // The temporary file is new (`create_new` follows no link and takes no file that is there), and its name is this process's
        // own, so nothing planted in the folder is written through.
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp = self
            .dir
            .join(format!("{name}.{}-{unique}.part", std::process::id()));
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .and_then(|mut file| {
                use std::io::Write;
                file.write_all(bytes)
            });
        if written.is_err() {
            let _ = std::fs::remove_file(&temp);
            return false;
        }
        if std::fs::rename(&temp, &path).is_err() {
            let _ = std::fs::remove_file(&temp);
            return false;
        }
        true
    }

    fn remove(&mut self, name: &str) {
        if let Some(path) = self.path(name) {
            let _ = std::fs::remove_file(path);
        }
    }

    fn touch(&mut self, name: &str, stamp: u64) {
        // The stamp is the cache's own count of uses, above every stamp it found; it goes in the file's time, so the order
        // survives a restart.
        if let Some(path) = self.path(name)
            && let Ok(file) = std::fs::OpenOptions::new().write(true).open(path)
        {
            let _ =
                file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(stamp));
        }
    }
}

/// The folder downloaded content is cached in by default: a per-user cache folder (`%LOCALAPPDATA%`, `~/Library/Caches`,
/// `$XDG_CACHE_HOME` or `~/.cache`), never the game's own folder. `None` when the system gives none (and on the web).
pub fn default_cache_dir() -> Option<std::path::PathBuf> {
    #[cfg(target_family = "wasm")]
    {
        None
    }
    #[cfg(not(target_family = "wasm"))]
    {
        let absolute = |name: &str| {
            std::env::var_os(name)
                .map(std::path::PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        let base = if cfg!(windows) {
            absolute("LOCALAPPDATA")
        } else if cfg!(target_os = "macos") {
            absolute("HOME").map(|h| h.join("Library").join("Caches"))
        } else {
            absolute("XDG_CACHE_HOME").or_else(|| absolute("HOME").map(|h| h.join(".cache")))
        };
        base.map(|b| b.join("torque-rs").join("mod-content"))
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
