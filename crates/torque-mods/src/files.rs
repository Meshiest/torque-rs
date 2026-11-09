//! The mounts over a game's files: what the mods' files and the files a server sent look like to scripts and loaders.
//!
//! A mount is a [`ModPackage`] laid over the files the script engine had: a lookup asks the mounts last-mounted first (a
//! later mount overrides an earlier one, and any mount overrides the game's own file), then the files underneath. A mod's
//! files are there twice: at their own paths (`art/rock.png`, replacing the game's file of that name), and under
//! `mods/<id>/` (where its entry scripts run from, so two mods can both have a `server.cs`). Its entry scripts and its
//! manifest are only under `mods/<id>/`; they never replace a game file.
//!
//! The files a server sent are a mount of a kind of their own: content only. Anything that could be script (`.cs`, `.dso`,
//! `.gui`, `.mis`) is left out of it when it is mounted, so it cannot be read, listed or run, whatever path a script names.
//! Another language's files (`.lua`) may be mounted, as content: the exec policy refuses to run or load any file a server
//! sent, whatever its language.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use torque_ts_vm::host::{FileHost, FileRead, FileStat};
use torque_ts_vm::text::fold;

use crate::cache::CacheOwner;
use crate::content::Hash;

use crate::package::ModPackage;

/// The folder under which every mod's files are also visible, by the mod's id.
pub const ALIAS_FOLDER: &str = "mods";

/// The extensions of files a server may not send: TorqueScript, which the engine runs by itself, and code a loader takes
/// (WebAssembly, native libraries). They are dropped when the files arrive. Compared without case.
///
/// Lua (`.lua`) is not in the list: a server may send it. It is kept and verified like any content and mounted as untrusted
/// ([`Mods::is_untrusted`](crate::Mods::is_untrusted)); no path the engine runs files by (`exec`, a loader of another language)
/// takes a file that came from a server, so it is data until a sandboxed realm made for it opts in.
pub const SCRIPT_EXTENSIONS: [&str; 10] = [
    "cs", "dso", "gui", "mis", "ed", "wasm", "wat", "dll", "so", "dylib",
];

/// The extensions of the other script languages a game may run (a Lua extension's). Compared without case. A server's
/// files with them are content: they never run, as the exec policy refuses every file a server sent.
pub const OTHER_SCRIPT_EXTENSIONS: [&str; 2] = ["lua", "luau"];

/// Whether `path` could be run as TorqueScript: a script extension, or one under another (`.cs.dso`, `.ed.cs`).
pub fn is_script_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower
        .rsplit('/')
        .next()
        .unwrap_or("")
        .split('.')
        .skip(1)
        .any(|ext| SCRIPT_EXTENSIONS.contains(&ext))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MountKind {
    /// A mod's files, with its scripts.
    Mod,
    /// Files a server sent: content only.
    Server,
}

pub(crate) struct Mount {
    pub(crate) id: String,
    pub(crate) kind: MountKind,
    pub(crate) package: ModPackage,
    pub(crate) enabled: Cell<bool>,
    /// Folded paths not laid over the game's files (entry scripts, the manifest): only under `mods/<id>/`.
    hidden: HashSet<Vec<u8>>,
    /// Which server's content a download mount is, and how it got here; `None` for every other mount.
    origin: Option<MountOrigin>,
}

/// The most files of one mod a client-script realm is given.
pub const MAX_CLIENT_FILES: usize = 1_024;

/// Where the content of a download mount came from: the one record the reader that may run server-sent scripts (a
/// client-script realm) trusts. A mount without one is never read for scripts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct MountOrigin {
    /// The server the content was offered by on this connection.
    pub(crate) server: CacheOwner,
    /// The content hash the mount was verified against.
    pub(crate) hash: Hash,
    /// How the files reached the client.
    pub(crate) way: Way,
}

/// How a download mount's files reached the client.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Way {
    /// Sent by the server this connection is to, and verified against its offer.
    Download,
    /// Found in the cache entry that server's own earlier connection stored.
    OwnCache,
    /// Found in an entry another server stored (the player turned cache sharing on).
    SharedCache,
}

impl Mount {
    pub(crate) fn new(
        id: String,
        kind: MountKind,
        package: ModPackage,
        hidden: impl IntoIterator<Item = String>,
    ) -> Mount {
        Mount {
            id,
            kind,
            package,
            enabled: Cell::new(true),
            hidden: hidden.into_iter().map(|p| fold(p.as_bytes())).collect(),
            origin: None,
        }
    }

    pub(crate) fn with_origin(mut self, origin: Option<MountOrigin>) -> Mount {
        self.origin = origin;
        self
    }
}

/// The mounts and the answer to "which one has this path". Shared by the file layer, the asset origins and the exec policy.
#[derive(Default)]
pub(crate) struct Table {
    mounts: RefCell<Vec<Rc<Mount>>>,
    generation: Cell<u64>,
}

/// Where a path is found.
pub(crate) struct Hit {
    pub(crate) mount: Rc<Mount>,
    /// The path inside the mount's package.
    pub(crate) inner: Vec<u8>,
}

impl Table {
    pub(crate) fn push(&self, mount: Rc<Mount>) {
        self.mounts.borrow_mut().push(mount);
        self.bump();
    }

    pub(crate) fn remove_server(&self, id: &str) -> bool {
        let mut mounts = self.mounts.borrow_mut();
        let before = mounts.len();
        mounts.retain(|m| !(m.kind == MountKind::Server && m.id == id));
        let removed = mounts.len() != before;
        drop(mounts);
        if removed {
            self.bump();
        }
        removed
    }

    pub(crate) fn bump(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// The download mount of mod `mod_id`, for a client-script realm of `server`: only when the mount was offered by `server`,
    /// verified against `hash` and did not come through cache sharing.
    fn client_mount(&self, mod_id: &str, server: CacheOwner, hash: &Hash) -> Option<Rc<Mount>> {
        crate::info::check_id(mod_id).ok()?;
        let name = crate::transfer::mount_name(mod_id);
        let mounts = self.mounts.borrow();
        mounts
            .iter()
            .rev()
            .find(|m| m.kind == MountKind::Server && m.id == name)
            .filter(|m| {
                m.origin.is_some_and(|o| {
                    o.server == server && o.hash == *hash && o.way != Way::SharedCache
                })
            })
            .map(Rc::clone)
    }

    /// Whether that mount is there with `entry` among its files: nothing is read.
    pub(crate) fn client_ready(
        &self,
        mod_id: &str,
        server: CacheOwner,
        hash: &Hash,
        entry: &str,
    ) -> bool {
        self.client_mount(mod_id, server, hash)
            .is_some_and(|m| m.package.len() <= MAX_CLIENT_FILES && m.package.contains(entry))
    }

    /// The files of that mount, each as `mods/<mod_id>/<path>`.
    pub(crate) fn client_content(
        &self,
        mod_id: &str,
        server: CacheOwner,
        hash: &Hash,
    ) -> Option<Vec<(String, Arc<[u8]>)>> {
        let mount = self.client_mount(mod_id, server, hash)?;
        if mount.package.len() > MAX_CLIENT_FILES {
            return None;
        }
        mount
            .package
            .names()
            .iter()
            .map(|name| {
                let bytes = mount.package.read(name)?;
                Some((format!("mods/{mod_id}/{name}"), Arc::from(bytes)))
            })
            .collect()
    }

    /// One `.lua` or `.luau` file of that mount, by its path within the mod.
    pub(crate) fn client_script(
        &self,
        mod_id: &str,
        server: CacheOwner,
        hash: &Hash,
        path: &str,
    ) -> Option<Arc<[u8]>> {
        let lower = path.to_ascii_lowercase();
        if !(lower.ends_with(".lua") || lower.ends_with(".luau")) {
            return None;
        }
        let mount = self.client_mount(mod_id, server, hash)?;
        mount.package.read(path).map(Arc::from)
    }

    /// The bytes held by every mount but the server mount `name`, and the files of the server mounts but `name`.
    pub(crate) fn held_except(&self, name: &str) -> (u64, usize) {
        let mounts = self.mounts.borrow();
        let others = mounts
            .iter()
            .filter(|m| !(m.kind == MountKind::Server && m.id == name));
        let bytes = others.clone().map(|m| m.package.held_bytes()).sum();
        let files = others
            .filter(|m| m.kind == MountKind::Server)
            .map(|m| m.package.len())
            .sum();
        (bytes, files)
    }

    fn mounts(&self) -> Vec<Rc<Mount>> {
        self.mounts.borrow().clone()
    }

    /// The mount that serves `path` (a game path, `/`-separated, any case): the last enabled one that has it.
    pub(crate) fn find(&self, path: &[u8]) -> Option<Hit> {
        let path = normalize(path);
        let mounts = self.mounts.borrow();
        if let Some(rest) = alias(&path) {
            let (id, inner) = rest;
            return mounts
                .iter()
                .rev()
                .find(|m| {
                    m.kind == MountKind::Mod && m.enabled.get() && fold(m.id.as_bytes()) == fold(id)
                })
                .filter(|m| m.package.position(inner).is_some())
                .map(|m| Hit {
                    mount: Rc::clone(m),
                    inner: inner.to_vec(),
                });
        }
        mounts
            .iter()
            .rev()
            .filter(|m| m.enabled.get())
            .find(|m| m.package.position(&path).is_some() && !m.hidden.contains(&fold(&path)))
            .map(|m| Hit {
                mount: Rc::clone(m),
                inner: path.clone(),
            })
    }

    /// Whether an enabled mount of any kind serves `path`: what the asset cache treats as untrusted.
    pub(crate) fn provides(&self, path: &str) -> bool {
        self.find(path.as_bytes()).is_some()
    }

    /// Whether `path` is served from files a server sent.
    pub(crate) fn is_server_file(&self, path: &[u8]) -> bool {
        self.find(path)
            .is_some_and(|hit| hit.mount.kind == MountKind::Server)
    }

    /// The game paths the mounts serve, last mount first, each once: overlay paths, then `mods/<id>/` paths.
    fn listing(&self) -> Vec<Vec<u8>> {
        let mut seen: HashSet<Vec<u8>> = HashSet::new();
        let mut out = Vec::new();
        for mount in self.mounts().iter().rev().filter(|m| m.enabled.get()) {
            for name in mount.package.names().iter().rev() {
                let key = fold(name.as_bytes());
                if !mount.hidden.contains(&key) && seen.insert(key) {
                    out.push(name.as_bytes().to_vec());
                }
                if mount.kind == MountKind::Mod {
                    let aliased = format!("{ALIAS_FOLDER}/{}/{name}", mount.id);
                    if seen.insert(fold(aliased.as_bytes())) {
                        out.push(aliased.into_bytes());
                    }
                }
            }
        }
        out
    }

    pub(crate) fn wasted(&self) -> u64 {
        self.mounts
            .borrow()
            .iter()
            .map(|m| m.package.wasted())
            .sum()
    }
}

/// `\` as `/`, a leading `./` and a leading `/` dropped.
fn normalize(path: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = path
        .iter()
        .map(|&b| if b == b'\\' { b'/' } else { b })
        .collect();
    while out.starts_with(b"./") {
        out.drain(..2);
    }
    out
}

/// A path under `mods/<id>/` as `(id, the rest)`.
fn alias(path: &[u8]) -> Option<(&[u8], &[u8])> {
    let head = ALIAS_FOLDER.len();
    if path.len() <= head + 1
        || !path
            .get(..head)?
            .eq_ignore_ascii_case(ALIAS_FOLDER.as_bytes())
        || *path.get(head)? != b'/'
    {
        return None;
    }
    let rest = path.get(head + 1..)?;
    let slash = rest.iter().position(|&b| b == b'/')?;
    Some((rest.get(..slash)?, rest.get(slash + 1..)?))
}

/// The game's files with the mounts over them.
pub(crate) struct ModFiles {
    inner: Box<dyn FileHost>,
    table: Rc<Table>,
    listed: RefCell<Option<Listed>>,
}

struct Listed {
    generation: u64,
    under: Rc<Vec<Vec<u8>>>,
    all: Rc<Vec<Vec<u8>>>,
}

impl ModFiles {
    pub(crate) fn new(inner: Box<dyn FileHost>, table: Rc<Table>) -> ModFiles {
        ModFiles {
            inner,
            table,
            listed: RefCell::new(None),
        }
    }
}

impl FileHost for ModFiles {
    fn read(&self, path: &[u8]) -> FileRead {
        match self.table.find(path) {
            Some(hit) => hit
                .mount
                .package
                .read_bytes(&hit.inner)
                .map_or(FileRead::Missing, FileRead::Found),
            None => self.inner.read(path),
        }
    }

    fn read_prefix(&self, path: &[u8], max: usize) -> FileRead {
        match self.table.find(path) {
            Some(hit) => hit
                .mount
                .package
                .read_prefix(&hit.inner, max)
                .map_or(FileRead::Missing, FileRead::Found),
            None => self.inner.read_prefix(path, max),
        }
    }

    fn stat(&self, path: &[u8]) -> FileStat {
        match self.table.find(path) {
            Some(hit) => hit
                .mount
                .package
                .size_of(&String::from_utf8_lossy(&hit.inner))
                .map_or(FileStat::Missing, |len| FileStat::Found { len }),
            None => self.inner.stat(path),
        }
    }

    fn stat_without_look_up(&self, path: &[u8]) -> FileStat {
        match self.table.find(path) {
            Some(_) => self.stat(path),
            None => self.inner.stat_without_look_up(path),
        }
    }

    fn is_archive_entry(&self, path: &[u8]) -> bool {
        match self.table.find(path) {
            Some(hit) => hit.mount.package.is_zip(),
            None => self.inner.is_archive_entry(path),
        }
    }

    fn real_path(&self, path: &[u8]) -> Option<Vec<u8>> {
        match self.table.find(path) {
            Some(_) => None,
            None => self.inner.real_path(path),
        }
    }

    fn modified(&self, path: &[u8]) -> Option<u64> {
        match self.table.find(path) {
            Some(_) => None,
            None => self.inner.modified(path),
        }
    }

    fn files(&self) -> Rc<Vec<Vec<u8>>> {
        let under = self.inner.files();
        let generation = self.table.generation();
        if let Some(listed) = self.listed.borrow().as_ref()
            && listed.generation == generation
            && Rc::ptr_eq(&listed.under, &under)
        {
            return Rc::clone(&listed.all);
        }
        let mut all = self.table.listing();
        let mut seen: HashSet<Vec<u8>> = all.iter().map(|p| fold(p)).collect();
        for path in under.iter() {
            if seen.insert(fold(path)) {
                all.push(path.clone());
            }
        }
        let all = Rc::new(all);
        *self.listed.borrow_mut() = Some(Listed {
            generation,
            under,
            all: Rc::clone(&all),
        });
        all
    }

    fn remove(&self, path: &[u8]) -> bool {
        self.inner.remove(path)
    }

    fn unlist(&self, path: &[u8]) {
        self.inner.unlist(path);
    }

    fn copy(&self, from: &[u8], to: &[u8]) -> bool {
        match self.table.find(from) {
            // A server's file copied to a script's name would be a script a server sent: refused, whatever the client's
            // scripts ask.
            Some(hit)
                if hit.mount.kind == MountKind::Server
                    && is_script_path(&String::from_utf8_lossy(to)) =>
            {
                false
            }
            Some(hit) => hit
                .mount
                .package
                .read_bytes(&hit.inner)
                .is_some_and(|bytes| self.inner.write(to, &bytes, false)),
            None => self.inner.copy(from, to),
        }
    }

    fn write(&self, path: &[u8], contents: &[u8], append: bool) -> bool {
        self.inner.write(path, contents, append)
    }

    fn work_done(&self) -> u64 {
        self.inner.work_done().saturating_add(self.table.wasted())
    }

    fn add_mod_path(&self, path: &[u8]) -> usize {
        self.inner.add_mod_path(path)
    }
}
