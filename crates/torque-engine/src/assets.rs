//! Assets from plugins: loaders by file extension, a typed cache for each script engine, and hot reload while developing.
//!
//! A plugin adds a **loader** while the engine is built ([`EngineBuilder::assets`]): the file extensions it reads, a
//! [`Spec`] (the most bytes it takes, and whether it takes files from untrusted sources: what a server sends, a mod), and a
//! function from a file's bytes to a typed asset (`T: 'static`). Each server and client script engine then has its own
//! [`Assets`]: a cache by normalized path and type, reading through the engine's files ([`FileHost`]: the resource
//! layer's mounts, refusals and caps). The registry itself never panics: a missing file, a refused one, one over the
//! loader's cap, one from an untrusted source its loader does not take, and one its loader cannot read are each an
//! [`AssetError`]. A loader that panics is caught and is an error too, natively only: on the web a panic aborts the
//! module (`catch_unwind` catches nothing there), so a loader that takes untrusted files must not panic on any bytes.
//!
//! **Trust.** A path is trusted only when some classifier says it is trusted and none says it is untrusted. Each layer that
//! knows where some files come from adds a classifier with [`Assets::add_origins`]: it answers [`Some`] for the paths it
//! knows (a mod mount says [`Origin::Untrusted`] for its files, a host says [`Origin::Trusted`] for its own folders with
//! [`Assets::trust_folder`]) and [`None`] for the rest. Classifiers add up (a later one never replaces an earlier one), and a
//! path nobody speaks for is untrusted, so a [`Spec::trusted`] loader (one whose reader is not hardened against hostile bytes)
//! reads only what a host has declared its own, and never a file a layer says it did not make. An asset cached before a
//! classifier was added is checked again when it is.
//!
//! **Memory.** The cache keeps every asset it loaded until [`Assets::forget`] or [`Assets::clear`]: it is bounded only by
//! the files there are, each at most its loader's [`Spec::max_bytes`] (plus what the loader makes of it). A host that
//! loads files from a source that keeps growing (a server's downloads, a mod folder) forgets the assets of a map or a
//! server it leaves, or clears the cache between them.
//!
//! A class's [`Filename`](crate::Filename) field can name an asset type (`#[field(asset = T)]`, see
//! [`EngineClass`](crate::EngineClass)): each datablock's preload then loads the file it names into the cache, and a load
//! error is the datablock's preload failure.
//!
//! **Hot reload.** In a debug build, or with `$Pref::Dev` true ([`DEV_PREF`](crate::script::DEV_PREF)), the client looks
//! at the cached files' change times once per drawn frame (at most every [`HOT_RELOAD_INTERVAL_MS`]); a file that
//! changed is loaded again, [`Assets::changed`] lists it until the next frame, and the renderer's caches are reset. A file
//! whose new contents do not load keeps its old asset. A release build does not look unless the pref says so.
//!
//! # Maintaining
//!
//! **Trust is declared, never assumed.** [`Assets::origin`] is trusted only when a classifier ([`Assets::add_origins`],
//! [`Assets::trust_folder`]) says [`Origin::Trusted`] and none says [`Origin::Untrusted`]; a [`Spec::trusted`] loader is never
//! given an untrusted path. A new layer that mounts files it did not make adds a classifier that answers untrusted for them and
//! `None` for everything else, where it mounts (and calls [`Assets::debug_assert_untrusted`] on what it mounted); it must not
//! speak for paths that are not its own, and there is no call that replaces another's. Tests:
//! `assets::tests::a_path_is_trusted_only_when_one_says_so_and_none_says_otherwise`,
//! `a_trusted_folder_is_the_folder_not_every_path`, `a_cached_asset_is_checked_again_when_a_classifier_is_added`,
//! `with_no_classifier_every_path_is_untrusted_and_a_trusted_only_loader_reads_nothing`,
//! `a_loader_of_trusted_files_is_never_given_an_untrusted_one`, `an_unclassified_mount_trips_the_debug_assertion`. A test that
//! loads through a trusted-only loader declares its folders first (`trust_folder`).
//!
//! ```
//! use torque_engine::assets::{Assets, Spec};
//! use torque_engine::{EngineBuilder, Plugin, PluginId};
//! use torque_ts_vm::host::MemFiles;
//! use torque_ts_vm::vm::Vm;
//!
//! /// A word list: one word a line.
//! #[derive(Debug)]
//! struct Words(Vec<String>);
//!
//! #[derive(PluginId)]
//! #[plugin(id = "example.words")]
//! struct WordsPlugin;
//!
//! impl Plugin for WordsPlugin {
//!     fn build(&self, engine: &mut EngineBuilder) {
//!         engine.assets().loader(&["words"], Spec::untrusted(4096), |bytes, _cx| {
//!             let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8 text".to_string())?;
//!             Ok(Words(text.lines().map(str::to_owned).collect()))
//!         });
//!     }
//! }
//!
//! let engine = EngineBuilder::new().add_plugins(WordsPlugin).build()?;
//! let files = MemFiles::new();
//! files.insert("data/greetings.words", b"hello\nhi\n");
//! let mut vm = Vm::new();
//! vm.set_file_host(files);
//!
//! let assets = engine.assets(&mut vm);
//! let words = assets.load::<Words>(vm.file_host(), "data/./Greetings.words")?;
//! assert_eq!(words.0, ["hello", "hi"]);
//! // The same file again is the cached asset.
//! let again = assets.load::<Words>(vm.file_host(), "data/greetings.words")?;
//! assert!(std::rc::Rc::ptr_eq(&words, &again));
//! // A missing file is an error, not a panic.
//! let error = assets.load::<Words>(vm.file_host(), "data/none.words").unwrap_err();
//! assert_eq!(error.to_string(), "data/none.words is missing");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::any::{Any, TypeId, type_name};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use torque_ts_vm::host::{FileHost, FileRead, FileStat, Refusal};
use torque_ts_vm::vm::Vm;

use crate::{Engine, EngineBuilder, PluginId};

/// The least time between two looks at the cached files' change times ([`Assets::poll`]), in milliseconds.
pub const HOT_RELOAD_INTERVAL_MS: u32 = 250;

/// The longest file extension a loader may name, in bytes.
pub const MAX_EXTENSION_BYTES: usize = 16;

/// What a loader takes: the most bytes, and whether files from untrusted sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    /// The largest file the loader is given; a longer one is [`AssetError::OverCap`] and is not read.
    pub max_bytes: u64,
    /// Whether the loader takes a file from an untrusted source ([`Origin::Untrusted`]: what a server sends, a mod). A
    /// loader that does not is never given one ([`AssetError::Untrusted`]).
    pub untrusted_ok: bool,
}

impl Spec {
    /// A loader of files up to `max_bytes` from trusted sources only.
    pub const fn trusted(max_bytes: u64) -> Spec {
        Spec {
            max_bytes,
            untrusted_ok: false,
        }
    }

    /// A loader of files up to `max_bytes` from any source: one hardened against hostile bytes.
    pub const fn untrusted(max_bytes: u64) -> Spec {
        Spec {
            max_bytes,
            untrusted_ok: true,
        }
    }
}

/// Where a file came from, as the host tells it ([`Assets::add_origins`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Origin {
    /// The game's own files: a classifier said so ([`Assets::trust_folder`]) and none said otherwise.
    Trusted,
    /// Files someone else made (what a server sent, a mod), or files nobody has spoken for: the default.
    #[default]
    Untrusted,
}

/// What a loader is told besides the bytes.
#[derive(Clone, Copy, Debug)]
pub struct LoadCx<'a> {
    path: &'a str,
    origin: Origin,
}

impl<'a> LoadCx<'a> {
    /// The file's normalized path ([`normalize`]).
    pub fn path(&self) -> &'a str {
        self.path
    }

    /// Where the file came from.
    pub fn origin(&self) -> Origin {
        self.origin
    }
}

/// Why an asset did not load. Each names the path as it was normalized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetError {
    /// The path is empty, ends in a folder, or climbs out of the game folder with `..`.
    BadPath {
        /// The path as it was given.
        path: String,
    },
    /// No loader of the type asked for reads the path's extension.
    NoLoader {
        /// The path.
        path: String,
        /// The type asked for.
        ty: &'static str,
    },
    /// There is no such file.
    Missing {
        /// The path.
        path: String,
    },
    /// The files' rules refuse the path (outside the folders they allow, or it cannot be opened).
    Refused {
        /// The path.
        path: String,
    },
    /// The file is longer than its loader takes ([`Spec::max_bytes`]); it was not read.
    OverCap {
        /// The path.
        path: String,
        /// The file's length.
        len: u64,
        /// The loader's cap.
        max: u64,
    },
    /// The file came from an untrusted source and its loader does not take one ([`Spec::untrusted_ok`]); it was not read.
    Untrusted {
        /// The path.
        path: String,
    },
    /// The loader could not read the file (or it panicked).
    Parse {
        /// The path.
        path: String,
        /// The loader's message.
        message: String,
    },
}

impl AssetError {
    /// The path the error is about.
    pub fn path(&self) -> &str {
        match self {
            AssetError::BadPath { path }
            | AssetError::NoLoader { path, .. }
            | AssetError::Missing { path }
            | AssetError::Refused { path }
            | AssetError::OverCap { path, .. }
            | AssetError::Untrusted { path }
            | AssetError::Parse { path, .. } => path,
        }
    }
}

/// Text an error shows (a path, a loader's message) with its control characters escaped (`\n`, `\u{1b}`), so it
/// cannot start a line of its own or send terminal codes when the error is printed to the console or a log.
struct Shown<'a>(&'a str);

impl fmt::Display for Shown<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for c in self.0.chars() {
            if c.is_control() {
                write!(f, "{}", c.escape_default())?;
            } else {
                write!(f, "{c}")?;
            }
        }
        Ok(())
    }
}

impl fmt::Display for AssetError {
    /// The error in one line: the path and a loader's message are shown with their control characters escaped.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path = Shown(self.path());
        match self {
            AssetError::BadPath { .. } => write!(f, "\"{path}\" is not a file's path"),
            AssetError::NoLoader { ty, .. } => {
                write!(f, "{path} has no loader for {}", short_type(ty))
            }
            AssetError::Missing { .. } => write!(f, "{path} is missing"),
            AssetError::Refused { .. } => write!(f, "{path} is refused"),
            AssetError::OverCap { len, max, .. } => {
                write!(f, "{path} is {len} bytes, over its loader's {max}")
            }
            AssetError::Untrusted { .. } => {
                write!(
                    f,
                    "{path} is from an untrusted source, which its loader does not take"
                )
            }
            AssetError::Parse { message, .. } => {
                write!(f, "{path} did not load: {}", Shown(message))
            }
        }
    }
}

impl std::error::Error for AssetError {}

/// The last part of a type's path (`Words` for `my_game::assets::Words`), keeping generic arguments.
fn short_type(name: &str) -> &str {
    let end = name.find('<').unwrap_or(name.len());
    name[..end].rfind("::").map_or(name, |at| &name[at + 2..])
}

/// `path` as the cache knows it: `\` read as `/`, a leading `/` and every `.` or empty part dropped, `..` taking off the
/// part before it. The spelling's case is kept (the cache's key ignores it, as the files do). An empty path, one that ends
/// in `/`, and one whose `..` climbs out of the game folder are [`AssetError::BadPath`].
pub fn normalize(path: &str) -> Result<String, AssetError> {
    let bad = || AssetError::BadPath {
        path: path.to_owned(),
    };
    let unified = path.replace('\\', "/");
    if unified.ends_with('/') || unified.contains('\0') {
        return Err(bad());
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in unified.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop().ok_or_else(bad)?;
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(bad());
    }
    Ok(parts.join("/"))
}

/// The extension of a normalized path, in lower case: what follows the last part's last `.`.
fn extension(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?;
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| ext.to_ascii_lowercase())
}

type LoadFn = Rc<dyn Fn(&[u8], &LoadCx<'_>) -> Result<Rc<dyn Any>, String>>;

/// A loader a plugin added.
#[derive(Clone)]
pub(crate) struct LoaderEntry {
    plugin: PluginId,
    extensions: Vec<String>,
    spec: Spec,
    ty: TypeId,
    ty_name: &'static str,
    load: LoadFn,
}

/// What a loader declared ([`Engine::asset_loaders`], [`Assets::loaders`]).
#[derive(Clone, Copy, Debug)]
pub struct LoaderInfo<'a> {
    /// The plugin that added it.
    pub plugin: PluginId,
    /// The extensions it reads, in lower case and without the dot.
    pub extensions: &'a [String],
    /// What it takes.
    pub spec: Spec,
    /// The type of asset it makes.
    pub ty: &'static str,
}

impl LoaderEntry {
    fn info(&self) -> LoaderInfo<'_> {
        LoaderInfo {
            plugin: self.plugin,
            extensions: &self.extensions,
            spec: self.spec,
            ty: self.ty_name,
        }
    }
}

impl EngineBuilder {
    /// The asset registry: loaders by file extension ([module docs](crate::assets)).
    pub fn assets(&mut self) -> AssetRegistry<'_> {
        AssetRegistry { engine: self }
    }
}

/// Adds asset loaders to an engine being built ([module docs](crate::assets)).
pub struct AssetRegistry<'a> {
    engine: &'a mut EngineBuilder,
}

impl AssetRegistry<'_> {
    /// Adds a loader of `T` assets from files with one of `extensions` (`"glb"` or `".glb"`, any case): `load` is given a
    /// file's bytes (never more than `spec.max_bytes`, and from an untrusted source only when `spec.untrusted_ok`) and
    /// returns the asset or a message saying why it cannot. An extension that is empty or not letters, digits, `_` and
    /// `-`, and a second loader of `T` for an extension, fail the build ([`BuildError::Rejected`](crate::BuildError::Rejected)).
    /// Loaders of different types may share an extension: a load asks for its type.
    pub fn loader<T: Any>(
        &mut self,
        extensions: &[&str],
        spec: Spec,
        load: impl Fn(&[u8], &LoadCx<'_>) -> Result<T, String> + 'static,
    ) -> &mut Self {
        let ty = TypeId::of::<T>();
        let ty_name = type_name::<T>();
        let mut kept: Vec<String> = Vec::new();
        for raw in extensions {
            let ext = raw.strip_prefix('.').unwrap_or(raw).to_ascii_lowercase();
            let fits = !ext.is_empty()
                && ext.len() <= MAX_EXTENSION_BYTES
                && ext
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
            if !fits {
                self.engine.reject(format!(
                    "the asset extension \"{raw}\" is not 1 to {MAX_EXTENSION_BYTES} letters, digits, '_' or '-'"
                ));
                return self;
            }
            let taken = self
                .engine
                .list::<LoaderEntry>()
                .iter()
                .find(|e| e.ty == ty && e.extensions.contains(&ext))
                .map(|e| e.plugin);
            if let Some(first) = taken {
                self.engine.reject(format!(
                    "`{first}` already added a loader of {} assets for .{ext}",
                    short_type(ty_name)
                ));
                return self;
            }
            if !kept.contains(&ext) {
                kept.push(ext);
            }
        }
        if kept.is_empty() {
            self.engine.reject(format!(
                "a loader of {} assets names no extension",
                short_type(ty_name)
            ));
            return self;
        }
        let plugin = self.engine.current_plugin();
        self.engine.push(LoaderEntry {
            plugin,
            extensions: kept,
            spec,
            ty,
            ty_name,
            load: Rc::new(move |bytes, cx| {
                load(bytes, cx).map(|asset| Rc::new(asset) as Rc<dyn Any>)
            }),
        });
        self
    }
}

impl Engine {
    /// Every asset loader the plugins added, in the order they were added.
    pub fn asset_loaders(&self) -> impl Iterator<Item = LoaderInfo<'_>> + '_ {
        self.list::<LoaderEntry>().iter().map(LoaderEntry::info)
    }

    /// The asset cache of the script engine `vm` ([`Assets::of`]); one with this engine's loaders is attached when it has
    /// none.
    pub fn assets(&self, vm: &mut Vm) -> Rc<Assets> {
        if let Some(assets) = Assets::of(vm) {
            return assets;
        }
        let assets = Rc::new(Assets::new(self));
        vm.set_extension(Rc::clone(&assets));
        assets
    }
}

/// One cached asset.
struct Entry {
    /// The path as it was first normalized (its case kept).
    path: String,
    /// Its loader, in [`Assets::loaders`].
    loader: usize,
    value: Rc<dyn Any>,
    /// The file's change time when it was read.
    modified: Option<u64>,
}

/// A cached asset whose file changed: its key, its path, its loader and the file's change time now.
type Stale = ((String, TypeId), String, usize, Option<u64>);

/// What an origin classifier is: the path's origin when the classifier knows it, else `None`.
type Origins = Rc<dyn Fn(&str) -> Option<Origin>>;

/// The asset cache of one script engine (a server's, a client's): what its loaders made, by normalized path and type.
/// [`Engine::assets`] attaches one to a script engine; [`Assets::of`] finds it again. Interior mutability throughout,
/// as it is shared behind an `Rc`; no method holds a borrow while a loader runs.
pub struct Assets {
    loaders: Rc<[LoaderEntry]>,
    cache: RefCell<HashMap<(String, TypeId), Entry>>,
    origins: RefCell<Vec<Origins>>,
    hot_reload: Cell<Option<bool>>,
    last_poll: Cell<Option<u32>>,
    changed: RefCell<Vec<String>>,
    reads: Cell<u64>,
}

impl Assets {
    /// An empty cache with `engine`'s loaders.
    pub fn new(engine: &Engine) -> Assets {
        Assets {
            loaders: engine.list::<LoaderEntry>().iter().cloned().collect(),
            cache: RefCell::default(),
            origins: RefCell::default(),
            hot_reload: Cell::new(None),
            last_poll: Cell::new(None),
            changed: RefCell::default(),
            reads: Cell::new(0),
        }
    }

    /// The cache attached to `vm` ([`Engine::assets`]), if there is one.
    pub fn of(vm: &Vm) -> Option<Rc<Assets>> {
        vm.extension::<Assets>()
    }

    /// The loaders this cache uses, in the order they were added.
    pub fn loaders(&self) -> impl Iterator<Item = LoaderInfo<'_>> + '_ {
        self.loaders.iter().map(LoaderEntry::info)
    }

    /// Adds a classifier of where files come from (a mod's folder, the files a server sent). It answers `Some(origin)` for a
    /// path it knows and `None` for any other, and is given the normalized path. Classifiers add up: a path is
    /// [`Origin::Trusted`] only when at least one says so and none says [`Origin::Untrusted`], so a layer cannot make
    /// another layer's files trusted, and a path nobody speaks for is untrusted. The classifier must not call back into this
    /// cache. A loader whose [`Spec`] does not take untrusted files is never given an untrusted path, and a cached asset of
    /// such a loader whose path is untrusted now is dropped from the cache.
    pub fn add_origins(&self, origins: impl Fn(&str) -> Option<Origin> + 'static) {
        self.origins.borrow_mut().push(Rc::new(origins));
        let loaders = &self.loaders;
        self.cache.borrow_mut().retain(|_, entry| {
            loaders[entry.loader].spec.untrusted_ok || self.origin(&entry.path) == Origin::Trusted
        });
    }

    /// Declares the files under `folder` (a normalized path, any case) the game's own: a classifier that says trusted for them
    /// and nothing for any other path. A mount of someone else's files over that folder still makes its paths untrusted
    /// (a mod's `shapes/rock.obj` is not trusted because `shapes` is).
    pub fn trust_folder(&self, folder: &str) {
        let folder = folder.replace('\\', "/").to_ascii_lowercase();
        let folder = folder.trim_matches('/').to_string();
        self.add_origins(move |path| {
            let lower = path.to_ascii_lowercase();
            let inside = lower
                .strip_prefix(folder.as_str())
                .is_some_and(|rest| rest.starts_with('/'));
            inside.then_some(Origin::Trusted)
        });
    }

    /// Where the file at the normalized `path` comes from: trusted when a classifier says so and none says untrusted.
    pub fn origin(&self, path: &str) -> Origin {
        // The classifiers are cloned out so that none runs under the borrow.
        let classifiers = self.origins.borrow().clone();
        let mut trusted = false;
        for answer in classifiers.iter().filter_map(|f| f(path)) {
            match answer {
                Origin::Untrusted => return Origin::Untrusted,
                Origin::Trusted => trusted = true,
            }
        }
        if trusted {
            Origin::Trusted
        } else {
            Origin::Untrusted
        }
    }

    /// In a debug build, panics when a path of files someone else made (a download folder, a mount) is not untrusted to this
    /// cache: the layer that mounted them has not added its classifier, or another layer's trusted folder covers them and its
    /// own classifier is missing. Every layer that mounts such files calls it with what it mounted. In a release build it
    /// does nothing.
    pub fn debug_assert_untrusted<S: AsRef<str>>(
        &self,
        what: &str,
        paths: impl IntoIterator<Item = S>,
    ) {
        if !cfg!(debug_assertions) {
            return;
        }
        for path in paths {
            let Ok(path) = normalize(path.as_ref()) else {
                continue;
            };
            assert!(
                self.origin(&path) == Origin::Untrusted,
                "{what}: {path} holds files someone else made but no classifier marks it untrusted (Assets::add_origins)"
            );
        }
    }

    /// The `T` asset of the file at `path`: the cached one, else the file read through `files` and given to the loader of
    /// `T` for its extension (then cached). The registry never panics (a loader's panic is caught natively only; see
    /// the [module docs](crate::assets)): see [`AssetError`] for what can go wrong. A failed load is not cached, so the
    /// next one tries again.
    pub fn load<T: Any>(&self, files: &dyn FileHost, path: &str) -> Result<Rc<T>, AssetError> {
        let path = normalize(path)?;
        let key = (path.to_ascii_lowercase(), TypeId::of::<T>());
        let cached = self
            .cache
            .borrow()
            .get(&key)
            .map(|entry| Rc::clone(&entry.value));
        if let Some(value) = cached.and_then(|value| value.downcast::<T>().ok()) {
            return Ok(value);
        }
        let loader = self.loader_for::<T>(&path)?;
        let (value, modified) = self.read_and_load(files, &path, loader)?;
        let asset = Rc::clone(&value)
            .downcast::<T>()
            .map_err(|_| AssetError::Parse {
                path: path.clone(),
                message: "the loader made another type".into(),
            })?;
        self.cache.borrow_mut().insert(
            key,
            Entry {
                path,
                loader,
                value,
                modified,
            },
        );
        Ok(asset)
    }

    /// Loads the `T` asset at `path` ([`load`](Self::load)) for what it says, keeping it cached: what a preload does.
    pub fn check<T: Any>(&self, files: &dyn FileHost, path: &str) -> Result<(), AssetError> {
        self.load::<T>(files, path).map(drop)
    }

    /// The cached `T` asset of `path`, without reading anything.
    pub fn get<T: Any>(&self, path: &str) -> Option<Rc<T>> {
        let path = normalize(path).ok()?;
        let key = (path.to_ascii_lowercase(), TypeId::of::<T>());
        let value = Rc::clone(&self.cache.borrow().get(&key)?.value);
        value.downcast::<T>().ok()
    }

    /// How many assets are cached (one for each path and type).
    pub fn len(&self) -> usize {
        self.cache.borrow().len()
    }

    /// Whether nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.cache.borrow().is_empty()
    }

    /// How many files the cache has read for its loaders: each load that missed the cache and each reload.
    pub fn reads(&self) -> u64 {
        self.reads.get()
    }

    /// Forgets every cached asset of `path` (of any type); the next load reads it again.
    pub fn forget(&self, path: &str) {
        let Ok(path) = normalize(path) else {
            return;
        };
        let key = path.to_ascii_lowercase();
        self.cache.borrow_mut().retain(|(k, _), _| *k != key);
    }

    /// Forgets every cached asset.
    pub fn clear(&self) {
        self.cache.borrow_mut().clear();
    }

    /// Turns hot reload on or off for [`poll_in`](Self::poll_in), whatever the build; `None` goes back to the default
    /// (on in a debug build or with `$Pref::Dev` true).
    pub fn set_hot_reload(&self, on: Option<bool>) {
        self.hot_reload.set(on);
    }

    /// Whether [`poll_in`](Self::poll_in) reloads changed files for `vm`.
    pub fn hot_reload_on(&self, vm: &Vm) -> bool {
        self.hot_reload
            .get()
            .unwrap_or_else(|| crate::script::dev_allowed_in(vm, cfg!(debug_assertions)))
    }

    /// The paths the latest [`poll`](Self::poll) loaded again (each once, as first spelled), in the order it found them:
    /// what changed this frame.
    pub fn changed(&self) -> Vec<String> {
        self.changed.borrow().clone()
    }

    /// One frame's look at the cached files ([`changed`](Self::changed) is emptied first): once [`HOT_RELOAD_INTERVAL_MS`]
    /// have passed since the last look (`now_ms` is a millisecond counter that may wrap), each cached file whose change
    /// time `files` gives differs from the one it had when it was read is read and loaded again; the new asset replaces the
    /// old and its path is listed in [`changed`](Self::changed). A file that no longer loads keeps its old asset and is
    /// tried again when it changes again; the errors are returned. A host whose files give no change times reloads
    /// nothing.
    pub fn poll(&self, files: &dyn FileHost, now_ms: u32) -> Vec<AssetError> {
        self.changed.borrow_mut().clear();
        if let Some(last) = self.last_poll.get()
            && now_ms.wrapping_sub(last) < HOT_RELOAD_INTERVAL_MS
        {
            return Vec::new();
        }
        self.last_poll.set(Some(now_ms));
        let stale: Vec<Stale> = self
            .cache
            .borrow()
            .iter()
            .filter_map(|(key, entry)| {
                let now = files.modified(entry.path.as_bytes());
                (now != entry.modified)
                    .then(|| (key.clone(), entry.path.clone(), entry.loader, now))
            })
            .collect();
        let mut failed = Vec::new();
        let mut changed: Vec<String> = Vec::new();
        for (key, path, loader, now) in stale {
            let result = self.read_and_load(files, &path, loader);
            let mut cache = self.cache.borrow_mut();
            let Some(entry) = cache.get_mut(&key) else {
                continue;
            };
            match result {
                Ok((value, modified)) => {
                    entry.value = value;
                    entry.modified = modified;
                    if !changed.iter().any(|p| p.eq_ignore_ascii_case(&path)) {
                        changed.push(path);
                    }
                }
                Err(error) => {
                    entry.modified = now;
                    failed.push(error);
                }
            }
        }
        *self.changed.borrow_mut() = changed;
        failed
    }

    /// [`poll`](Self::poll) over `vm`'s files and clock, when hot reload is on for it ([`hot_reload_on`](Self::hot_reload_on));
    /// otherwise only empties [`changed`](Self::changed).
    pub fn poll_in(&self, vm: &Vm) -> Vec<AssetError> {
        if !self.hot_reload_on(vm) {
            self.changed.borrow_mut().clear();
            return Vec::new();
        }
        self.poll(vm.file_host(), vm.clock().real_time_ms())
    }

    /// The loader of `T` for `path`'s extension.
    fn loader_for<T: Any>(&self, path: &str) -> Result<usize, AssetError> {
        let ty = TypeId::of::<T>();
        extension(path)
            .and_then(|ext| {
                self.loaders
                    .iter()
                    .position(|l| l.ty == ty && l.extensions.contains(&ext))
            })
            .ok_or_else(|| AssetError::NoLoader {
                path: path.to_owned(),
                ty: type_name::<T>(),
            })
    }

    /// Reads the file at the normalized `path` within `loader`'s spec and gives it to the loader: the asset and the file's
    /// change time.
    fn read_and_load(
        &self,
        files: &dyn FileHost,
        path: &str,
        loader: usize,
    ) -> Result<(Rc<dyn Any>, Option<u64>), AssetError> {
        let entry = &self.loaders[loader];
        let spec = entry.spec;
        let owned = || path.to_owned();
        let origin = self.origin(path);
        if origin == Origin::Untrusted && !spec.untrusted_ok {
            return Err(AssetError::Untrusted { path: owned() });
        }
        let over = |len: u64| AssetError::OverCap {
            path: owned(),
            len,
            max: spec.max_bytes,
        };
        match files.stat(path.as_bytes()) {
            FileStat::Missing => return Err(AssetError::Missing { path: owned() }),
            FileStat::Refused(Refusal::NotDefaultDirectory | Refusal::CannotOpen) => {
                return Err(AssetError::Refused { path: owned() });
            }
            FileStat::Found { len } if len > spec.max_bytes => return Err(over(len)),
            FileStat::Found { .. } => {}
        }
        let modified = files.modified(path.as_bytes());
        let bytes = match files.read(path.as_bytes()) {
            FileRead::Found(bytes) => bytes,
            FileRead::Missing => return Err(AssetError::Missing { path: owned() }),
            FileRead::Refused(_) => return Err(AssetError::Refused { path: owned() }),
        };
        self.reads.set(self.reads.get().saturating_add(1));
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if len > spec.max_bytes {
            return Err(over(len));
        }
        let load = Rc::clone(&entry.load);
        let cx = LoadCx { path, origin };
        match catch_unwind(AssertUnwindSafe(|| load(&bytes, &cx))) {
            Ok(Ok(value)) => Ok((value, modified)),
            Ok(Err(message)) => Err(AssetError::Parse {
                path: owned(),
                message,
            }),
            Err(_) => Err(AssetError::Parse {
                path: owned(),
                message: "its loader panicked".into(),
            }),
        }
    }
}

impl fmt::Debug for Assets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Assets")
            .field("loaders", &self.loaders.len())
            .field("cached", &self.len())
            .finish_non_exhaustive()
    }
}

/// An asset type a class's `Filename` field names (`#[field(asset = T)]`): its preload loads the file into the cache.
#[derive(Clone, Copy)]
pub struct AssetType {
    id: fn() -> TypeId,
    name: fn() -> &'static str,
    check: fn(&Assets, &dyn FileHost, &str) -> Result<(), AssetError>,
}

impl AssetType {
    /// The asset type `T`.
    pub const fn of<T: Any>() -> AssetType {
        AssetType {
            id: TypeId::of::<T>,
            name: type_name::<T>,
            check: Assets::check::<T>,
        }
    }

    /// The type's name.
    pub fn name(&self) -> &'static str {
        (self.name)()
    }

    /// Loads the `path` asset of this type into `assets` for what it says.
    pub fn check(
        &self,
        assets: &Assets,
        files: &dyn FileHost,
        path: &str,
    ) -> Result<(), AssetError> {
        (self.check)(assets, files, path)
    }
}

impl PartialEq for AssetType {
    fn eq(&self, other: &AssetType) -> bool {
        (self.id)() == (other.id)()
    }
}

impl fmt::Debug for AssetType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("AssetType").field(&self.name()).finish()
    }
}

#[cfg(test)]
#[path = "assets_tests.rs"]
mod tests;
