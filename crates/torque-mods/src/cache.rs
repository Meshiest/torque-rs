//! The cache of content downloaded from servers: one entry per content hash, in a per-user folder outside the game's.
//!
//! An entry is the content message a server sent ([`crate::content::encode_content`]) under the file name
//! `<hash>.<server>.mod`, the hash and the server's key in lower case hexadecimal. The cache is partitioned by the server the
//! entry came from ([`CacheOwner`]): an entry counts for a connection only when the same server sent it, so a server that offers
//! another server's content learns nothing of what the player fetched there (it would see which blocks the client skips). A
//! player may turn that off ([`CacheScope::share`]) and let every server match every entry. An entry named `<hash>.mod`, from
//! before the partition, has no server and counts as another server's. Nothing in the cache is trusted when it is read back: [`ContentCache::load`] reads the entry as it
//! read the download (the same caps, no script paths) and hashes its files, and an entry whose files do not hash to the name it
//! has is deleted. A folder listing is read the same way ([`index_of`]): names that are not entries are ignored, so a cache
//! folder full of junk or hand-made names does no harm.
//!
//! The cache is bounded: [`ContentCache::store`] evicts the entries used least recently until the folder is within its cap, and an
//! entry above the cap is not kept at all. A server holds at most half the cap: past that, what it stores evicts its own
//! oldest entries first, so a server that sends new content on every connect churns its own share and leaves the other
//! servers' entries alone. On the web, and when no per-user folder is known, the cache lives in memory for the
//! session.
//!
//! Only content is stored, so nothing in the cache can run: the folder is never a script path, and the files in it are read back
//! only through the content rules above.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

use sha2::{Digest, Sha256};

use crate::content::{ContentCaps, Hash, decode_content, from_hex, hex, summarize_files};

/// The files of a cache entry: `(path, bytes)`.
type CachedFiles = Vec<(String, Vec<u8>)>;

/// The suffix of an entry's file name.
pub const ENTRY_SUFFIX: &str = ".mod";

/// What the cache's cap is divided by for the most one server may hold before its own entries are evicted first (a half).
pub const SERVER_SHARE: u64 = 2;

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

/// The server an entry was downloaded from: a short key of its address, which an entry is named by.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CacheOwner([u8; 8]);

impl CacheOwner {
    /// The key of the server dialed at `addr` (an IPv4 address in IPv6 form is the IPv4 address).
    pub fn of_server(addr: SocketAddr) -> CacheOwner {
        let ip = match addr.ip() {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
            ip => ip,
        };
        let mut hasher = Sha256::new();
        hasher.update(b"mod cache server\0");
        hasher.update(format!("{ip}:{}", addr.port()).as_bytes());
        let digest = hasher.finalize();
        let mut key = [0u8; 8];
        key.copy_from_slice(&digest[..8]);
        CacheOwner(key)
    }

    /// The key as 16 lower-case hexadecimal digits: what an entry's name and a remembered choice are keyed by.
    pub fn to_hex(self) -> String {
        self.hex()
    }

    fn hex(self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The key from its 16 lower-case hexadecimal digits ([`to_hex`](Self::to_hex)).
    pub fn parse_hex(text: &str) -> Option<CacheOwner> {
        CacheOwner::from_hex(text)
    }

    fn from_hex(text: &str) -> Option<CacheOwner> {
        if text.len() != 16 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        let mut key = [0u8; 8];
        for (slot, pair) in key.iter_mut().zip(text.as_bytes().chunks(2)) {
            *slot = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        }
        Some(CacheOwner(key))
    }
}

/// Whose entries a connection may use, and under whose name it stores.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CacheScope {
    /// The server the connection is to. `None` (a cache used with no server named) keeps and matches only entries with no
    /// server.
    pub owner: Option<CacheOwner>,
    /// Whether entries from any server count (a player's choice: a server can then tell what the player has from other servers).
    pub share: bool,
}

/// An entry of the index: a hash in the cache.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IndexEntry {
    /// The content hash.
    pub hash: Hash,
    /// The server it came from (`None`: an entry from before entries were kept by server).
    pub owner: Option<CacheOwner>,
    /// The entry's size.
    pub size: u64,
    /// When it was last used.
    pub stamp: u64,
}

/// The entries among `listed`: files named `<64 hex digits>.mod` or `<64 hex digits>.<16 hex digits>.mod` and nothing else, each
/// hash and server once, the first `MAX_ENTRIES`.
pub fn index_of(listed: &[Listed]) -> Vec<IndexEntry> {
    let mut seen: HashMap<(Hash, Option<CacheOwner>), usize> = HashMap::new();
    let mut entries: Vec<IndexEntry> = Vec::new();
    for item in listed.iter().take(MAX_ENTRIES.saturating_mul(2)) {
        let Some(stem) = item.name.strip_suffix(ENTRY_SUFFIX) else {
            continue;
        };
        let (stem, owner) = match stem.split_once('.') {
            Some((stem, owner)) => match CacheOwner::from_hex(owner) {
                Some(owner) => (stem, Some(owner)),
                None => continue,
            },
            None => (stem, None),
        };
        let Some(hash) = from_hex(stem) else {
            continue;
        };
        let entry = IndexEntry {
            hash,
            owner,
            size: item.size,
            stamp: item.stamp,
        };
        match seen.get(&(hash, owner)) {
            Some(&at) => {
                if let Some(slot) = entries.get_mut(at) {
                    *slot = entry;
                }
            }
            None if entries.len() < MAX_ENTRIES => {
                seen.insert((hash, owner), entries.len());
                entries.push(entry);
            }
            None => {}
        }
    }
    entries
}

fn entry_name(hash: &Hash, owner: Option<CacheOwner>) -> String {
    match owner {
        Some(owner) => format!("{}.{}{ENTRY_SUFFIX}", hex(hash), owner.hex()),
        None => format!("{}{ENTRY_SUFFIX}", hex(hash)),
    }
}

/// A cache of downloaded content.
pub struct ContentCache {
    store: Box<dyn CacheStore>,
    max_bytes: u64,
    clock: u64,
    scope: CacheScope,
}

impl ContentCache {
    /// A cache over `store` that keeps at most `max_bytes`.
    pub fn new(store: Box<dyn CacheStore>, max_bytes: u64) -> ContentCache {
        let clock = store.list().iter().map(|l| l.stamp).max().unwrap_or(0);
        ContentCache {
            store,
            max_bytes,
            clock,
            scope: CacheScope::default(),
        }
    }

    /// Sets whose entries the next lookups use and whose name the next stores go under.
    pub fn set_scope(&mut self, scope: CacheScope) {
        self.scope = scope;
    }

    /// Whose entries the lookups use and whose name the stores go under.
    pub fn scope(&self) -> CacheScope {
        self.scope
    }

    /// The entries the current scope may use: the server's own, or all of them when sharing is on.
    fn visible(&self) -> Vec<IndexEntry> {
        let mut entries = self.index();
        if !self.scope.share {
            entries.retain(|e| e.owner == self.scope.owner);
        }
        entries
    }

    /// The entry for `hash` the scope may use: the server's own, else (sharing on) any.
    fn entry_for(&self, hash: &Hash) -> Option<IndexEntry> {
        let mut found: Option<IndexEntry> = None;
        for e in self.visible().into_iter().filter(|e| &e.hash == hash) {
            if e.owner == self.scope.owner {
                return Some(e);
            }
            found.get_or_insert(e);
        }
        found
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

    /// Every entry now, whichever server it came from.
    pub fn index(&self) -> Vec<IndexEntry> {
        index_of(&self.store.list())
    }

    /// The bytes the entries take.
    pub fn bytes(&self) -> u64 {
        self.index().iter().map(|e| e.size).sum()
    }

    /// What the entries the scope may use are: the id, version and hash each one's start says, most recently used first, at most
    /// `max` of them. A start that does not read as a content message is skipped (and the entry left for a load to delete).
    pub fn heads(&self, max: usize) -> Vec<(String, String, Hash)> {
        let mut entries = self.visible();
        entries.sort_by_key(|e| std::cmp::Reverse(e.stamp));
        let mut seen: Vec<Hash> = Vec::new();
        entries
            .iter()
            .filter_map(|e| {
                let head = self
                    .store
                    .read_head(&entry_name(&e.hash, e.owner), crate::content::HEAD_BYTES)?;
                let (id, version, hash) = crate::content::peek_head(&head)?;
                if hash != e.hash || seen.contains(&hash) {
                    return None;
                }
                seen.push(hash);
                Some((id, version, hash))
            })
            .take(max)
            .collect()
    }

    /// The stored size of the entry for `hash` the scope may use (not verified).
    #[must_use]
    pub fn size_of(&self, hash: &Hash) -> Option<u64> {
        self.entry_for(hash).map(|e| e.size)
    }

    /// Whether the scope may use an entry for `hash` (not verified).
    pub fn has(&self, hash: &Hash) -> bool {
        self.entry_for(hash).is_some()
    }

    /// The files of the entry for `hash`, read under `caps` and checked: they must hash to `hash`. An entry that does not read or
    /// does not hash right is deleted. A hit counts as a use.
    pub fn load(&mut self, hash: &Hash, caps: &ContentCaps) -> Option<Vec<(String, Vec<u8>)>> {
        self.load_entry(hash, caps).map(|(files, _)| files)
    }

    /// [`load`](Self::load), and whether the entry is this server's own (named for the scope's server). An entry from another
    /// server (sharing on), or from before entries were kept by server, is not.
    pub fn load_entry(&mut self, hash: &Hash, caps: &ContentCaps) -> Option<(CachedFiles, bool)> {
        let entry = self.entry_for(hash)?;
        let own = entry.owner.is_some() && entry.owner == self.scope.owner;
        let name = entry_name(hash, entry.owner);
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
        Some((files, own))
    }

    /// Keeps the content message `message` (already checked against `hash` by the receiver) and evicts the entries used least
    /// recently while the cache is over its cap. A message over the cap is not kept; the result says whether it was.
    pub fn store(&mut self, hash: &Hash, message: &[u8]) -> bool {
        if message.len() as u64 > self.max_bytes {
            return false;
        }
        let name = entry_name(hash, self.scope.owner);
        if !self.store.write(&name, message) {
            return false;
        }
        let stamp = self.tick();
        self.store.touch(&name, stamp);
        self.evict(Some(hash));
        true
    }

    /// Deletes the least recently used entries until the cache is within its cap; `keep` is never deleted. While the scope's
    /// server holds more than its share (half the cap), its own entries go first.
    pub fn evict(&mut self, keep: Option<&Hash>) {
        let mut entries = self.index();
        let mut total: u64 = entries.iter().map(|e| e.size).sum();
        let owner = self.scope.owner;
        let share = self.max_bytes / SERVER_SHARE;
        let mut own: u64 = entries
            .iter()
            .filter(|e| owner.is_some() && e.owner == owner)
            .map(|e| e.size)
            .sum();
        entries.sort_by_key(|e| e.stamp);
        let mut left = Vec::with_capacity(entries.len());
        for entry in entries {
            let is_own = owner.is_some() && entry.owner == owner;
            if total > self.max_bytes && own > share && is_own && keep != Some(&entry.hash) {
                self.store.remove(&entry_name(&entry.hash, entry.owner));
                total = total.saturating_sub(entry.size);
                own = own.saturating_sub(entry.size);
            } else {
                left.push(entry);
            }
        }
        for entry in left {
            if total <= self.max_bytes {
                break;
            }
            if keep == Some(&entry.hash) {
                continue;
            }
            self.store.remove(&entry_name(&entry.hash, entry.owner));
            total = total.saturating_sub(entry.size);
        }
    }

    /// Deletes every entry.
    pub fn clear(&mut self) {
        for entry in self.index() {
            self.store.remove(&entry_name(&entry.hash, entry.owner));
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
        let ok = name.len() <= 96 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.');
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
        let file = crate::disk::open_plain(&path, std::fs::OpenOptions::new().read(true))?;
        let len = file.metadata().ok()?.len();
        if len > max {
            return None;
        }
        let mut bytes = Vec::with_capacity(usize::try_from(len).ok()?);
        file.take(max.saturating_add(1))
            .read_to_end(&mut bytes)
            .ok()?;
        (bytes.len() as u64 <= max).then_some(bytes)
    }

    fn read_head(&self, name: &str, n: usize) -> Option<Vec<u8>> {
        use std::io::Read;
        let path = self.path(name)?;
        let file = crate::disk::open_plain(&path, std::fs::OpenOptions::new().read(true))?;
        let mut bytes = Vec::with_capacity(n);
        file.take(u64::try_from(n).unwrap_or(u64::MAX))
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
            && let Some(file) =
                crate::disk::open_plain(&path, std::fs::OpenOptions::new().write(true))
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
