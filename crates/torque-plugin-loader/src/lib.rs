//! Native extensions: libraries (`.dll`, `.so`, `.dylib`) the engine loads at run time from a local `plugins/` folder, over
//! its C ABI ([`torque_plugin_abi`]), that add console functions, methods, globals and render passes as a compiled plugin
//! does. An extension is written in Rust with `torque-plugin-sdk`, or in C (or anything that exports C functions) with
//! the header `torque_plugin.h`.
//!
//! **The danger, plainly.** A native extension is an ordinary compiled program running inside the game, with all the game's
//! rights: it can read, change and delete the player's files, use the network, and do anything else the game can do. No
//! sandbox stands between it and the computer. So the engine:
//!
//! - loads nothing unless the game turns extensions on ([`NativeExtensions`] adds the plugin off; game.toml's
//!   `[plugins] enable = ["torque.native-extensions"]` or the [`FLAG`] turns it on);
//! - loads only from a local folder the player put the files in (`plugins/`), and never a library a server sends;
//! - asks before it loads a file the first time, showing its id, version and SHA-256 hash, and asks again whenever the
//!   file changes ([`Approve`], [`Allowlist`]); the approvals are kept per user ([`user_folder`]), never in the plugins
//!   folder, so a download cannot approve itself (an `approved.txt` found there is ignored, with a warning);
//! - loads exactly the bytes that were hashed: the file is read once into a private read-only copy (`<sha256>.<ext>` in
//!   the per-user folder's `cache`), checked again through a handle held open until it has loaded, and that copy is
//!   loaded (see **Each platform** below);
//! - refuses a library that names folders of its own to find its dependencies in (an ELF rpath or runpath, a Mach-O
//!   `LC_RPATH`);
//! - on Unix, loads nothing unless the per-user folder, its cache and its allowlist belong to the process's effective user
//!   and nobody else can write them (the engine makes its folders `0700`), and every folder above it belongs to that user
//!   or root and nobody else can write it, unless it is sticky;
//! - refuses a library built for another ABI version before it runs any of its code (the version is read from the
//!   file's export table, not by loading it);
//! - catches a panic at every entry into the extension and disables the extension instead of crashing the game.
//!
//! **What an approval covers**: the one library file with that hash, under that id and version. Nothing else it reads or
//! loads is checked, so an extension must be linked statically: a library it needs that is not the system's may not be
//! found from the private copy, and then it does not load.
//!
//! **Each platform.**
//! - Windows: the copy is opened by its absolute path, shared only with readers until it has loaded (nothing can replace
//!   it), and its dependencies are looked for beside it (the private cache) and in System32, never in the working
//!   directory or on `PATH`.
//! - Linux: the copy is opened through the held handle (`/proc/self/fd/<n>`), so the file that loads is the one hashed
//!   whatever its path names by then. Its dependencies are found as the system finds any library's: the folders in the
//!   game's own `LD_LIBRARY_PATH`, the loader's cache and the system folders.
//! - macOS and other Unix systems: the copy is opened by its path, which is checked to name the hashed file right before
//!   and right after it is opened, and a copy replaced in between is refused before its `torque_plugin_init` runs. That
//!   check comes too late for the library's own load-time initializers (C++ static constructors, `__attribute__
//!   ((constructor))` functions, Rust's `.init_array` entries): the system runs them while it opens the file, so a file
//!   swapped in at that moment runs its initializers before the engine can stop it. What keeps anyone from swapping it is
//!   the folder rule above: only this user can write the private cache. Its dependencies are found by their install names
//!   (`@loader_path` is the private cache) and the game's own `DYLD_` variables.
//!
//! A server may require an extension by id and version, as any networked plugin (an extension with a network version
//! is in the handshake manifest), but it can never send one.
//!
//! **The folder.** Each extension is a manifest `plugins/<name>.toml` beside its library:
//!
//! ```toml
//! id = "example.native-sample"   # the id the library reports (its plugin id)
//! version = "0.1.0"              # the version it reports
//! library = "native_sample"      # the library's name: native_sample.dll, libnative_sample.so or libnative_sample.dylib
//! ```
//!
//! `library` is the manifest's own name when left out. The engine reads and hashes the library, asks the [`Approve`]r
//! (the approved hashes are kept in the per-user folder's [`ALLOWLIST_FILE`] by default), reads its exported ABI version
//! from the file, loads its private copy, calls its `torque_plugin_init`, checks that the id and version it reports are the manifest's, and adds an [`ExtensionPlugin`]
//! with that id. What the engine did with each file is in the [`LoadReport`] resource.
//!
//! ```
//! use torque_engine::EngineBuilder;
//! use torque_plugin_loader::{NativeExtensions, NativeExtensionsPlugin, LoadReport};
//!
//! let folder = std::path::Path::new("no-such-plugins-folder");
//! let engine = EngineBuilder::new()
//!     // Off unless game.toml or the flag turns it on; here the code does.
//!     .add_plugins(NativeExtensions(NativeExtensionsPlugin::new(folder).deny_unknown()))
//!     .enable_id("torque.native-extensions")
//!     .build()?;
//! let report = engine.resource::<LoadReport>().expect("the plugin ran");
//! assert!(report.entries.is_empty(), "no folder, nothing loaded");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Maintaining
//!
//! Rules a change must keep, each guarded by a test:
//! - Every native a full engine registers is in the reviewed list `tests/app/tests/loader/natives/effects.txt` (torque-test-app's
//!   `loader_native_effects`, as it needs a whole client) with the effects it declares, and every one with a files, code, engine
//!   or permissions effect is refused to a script language (`every_native_that_writes_or_evaluates_is_denied_to_languages`). A new native fails it until it is decided.
//! - A name with a NUL byte on the policy path (a call's namespace and name, a global's name, the names a `schedule`, a
//!   bind or a variable-writing native takes) is refused whatever the policy, as the engine keeps a name only up to its NUL
//!   (`lua_cannot_write_engine_globals` in torque-plugin-lua).
//! - The C ABI's functions that reach a call's script engine take only the innermost call's handle, as the Wasmtime host
//!   serves only its innermost call (`an_outer_call_handle_cannot_reach_the_script_engine_in_a_nested_call`).
//! - A manifest whose id an engine plugin or an earlier manifest has is refused before any of its code runs, and the engine
//!   builds (`an_extension_whose_id_an_engine_plugin_has_is_refused_and_the_engine_builds`,
//!   `of_two_manifests_with_one_id_only_the_first_loads`).
//! - A library with an rpath or runpath is refused (`a_library_that_names_its_own_dependency_folders_is_refused`); on Unix a
//!   per-user folder (or any folder above it) others own or can write loads nothing, judged by the effective uid
//!   (`a_folder_another_user_owns_or_can_write_is_not_private`, `a_user_folder_others_can_write_loads_nothing`), and a
//!   copy replaced after its check never runs its init
//!   (`a_copy_replaced_after_its_check_is_never_the_one_opened`; the Unix tests run in CI).
//! - The allowlist is replaced whole and never over a file it cannot read, and a manifest's version holds no `#`
//!   (`the_allowlist_is_replaced_whole_and_never_over_a_file_it_cannot_read`, `a_version_with_a_comment_mark_is_refused`).
//! - A script engine dropped inside a call into an extension is told to its languages when the outermost call returns
//!   (`a_script_engine_dropped_inside_a_call_is_heard_when_the_call_returns`).
//!
//! **Speed** (`just bench-script` runs `benches/native_call.rs`: loaded against compiled-in natives, back-calls and
//! field calls, in nanoseconds per call):
//! - A call into an extension takes no lock to see it is not disabled (`ExtensionState::is_off`), finds its owner
//!   thread with an atomic, and its frame borrows the call's arguments instead of copying them.
//! - A call back into the engine hands its argument list on to `Vm::invoke_owned` and reads names and field text in
//!   place.
//! - `DenyCodeAndFiles` allocates only to refuse: the message is built in the refusal's branch, a field's name on the
//!   stack (`every_native_that_writes_or_evaluates_is_denied_to_languages` holds the verdicts).
//! - The table of queued language calls is swept of dead ones once per `SWEEP_COOLDOWN` recordings when it stays
//!   full, not on every `schedule` (`a_full_table_is_swept_of_dead_calls_and_then_not_for_every_recording`).

#![warn(missing_docs)]

mod approve;
pub mod call_policy;
mod extension;
mod ffi;
mod language;
mod manifest;
mod private;
#[cfg(feature = "render")]
pub mod render;
mod stage;
pub mod web_lua;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use torque_engine::{EngineBuilder, Plugin, PluginGroup, PluginGroupBuilder, PluginId};

pub use approve::{ALLOWLIST_FILE, Allowlist, ApprovalRequest, Approve, Approved, TerminalPrompt};
pub use extension::{Extension, ExtensionPlugin};
pub use manifest::{MAX_MANIFEST_BYTES, Manifest};
pub use stage::{MAX_LIBRARY_BYTES, abi_version_at, exported_abi_version, sha256_hex};
pub use torque_plugin_abi as abi;

/// The command-line flag that turns native extensions on ([`enable_by_flag`]).
pub const FLAG: &str = "--native-extensions";

/// The folder extensions are loaded from when none is given, relative to the working directory.
pub const DEFAULT_FOLDER: &str = "plugins";

/// The most manifests a folder is read for (the rest are skipped with a warning).
pub const MAX_EXTENSIONS: usize = 256;

/// The per-user folder that keeps the approvals ([`ALLOWLIST_FILE`]) and the loaded copies (`cache`): the platform's
/// per-user cache folder (`%LOCALAPPDATA%` on Windows, `~/Library/Caches` on macOS, `$XDG_CACHE_HOME` or `~/.cache`
/// elsewhere), then `torque-rs/native-extensions`, the base the engine's other per-user files use. None when the
/// platform's folder is unknown, and in a browser.
pub fn user_folder() -> Option<PathBuf> {
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    let absolute = |v: std::ffi::OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    let home = || std::env::var_os("HOME").and_then(absolute);
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").and_then(absolute)?
    } else if cfg!(target_os = "macos") {
        home()?.join("Library").join("Caches")
    } else {
        match std::env::var_os("XDG_CACHE_HOME").and_then(absolute) {
            Some(p) => p,
            None => home()?.join(".cache"),
        }
    };
    Some(base.join("torque-rs").join("native-extensions"))
}

/// Who decides, as the plugin was told.
enum Decider {
    /// The per-user allowlist, asking on the terminal about the rest (or asking nobody).
    Allowlist { ask: bool },
    /// The host's own.
    Custom(Rc<dyn Approve>),
}

/// Finds, approves and loads the native extensions in a folder when the engine is built, adding each as an
/// [`ExtensionPlugin`] of its own id. Its id is `torque.native-extensions`; add it with [`NativeExtensions`] (off until
/// turned on).
///
/// A file is loaded only when the [`Approve`]r approves its id, version and SHA-256 hash. The default asks on the
/// terminal ([`TerminalPrompt`], which refuses when there is none) and keeps the approved hashes in the per-user
/// folder's [`ALLOWLIST_FILE`] ([`user_folder`]); [`deny_unknown`](Self::deny_unknown) loads only what that file already
/// approves, and [`approver`](Self::approver) puts in another (a callback in a headless test). The per-user folder must
/// not be inside the plugins folder: then nothing loads.
#[derive(torque_engine::PluginId)]
#[plugin(id = "torque.native-extensions")]
pub struct NativeExtensionsPlugin {
    folder: PathBuf,
    user: Option<PathBuf>,
    decider: Decider,
}

impl NativeExtensionsPlugin {
    /// Loads from `folder`, asking on the terminal and keeping the approvals and copies in [`user_folder`].
    pub fn new(folder: impl Into<PathBuf>) -> NativeExtensionsPlugin {
        NativeExtensionsPlugin {
            folder: folder.into(),
            user: user_folder(),
            decider: Decider::Allowlist { ask: true },
        }
    }

    /// Keeps the approvals and the loaded copies in `folder` instead of [`user_folder`]: a folder only this user can
    /// write, never one a download fills (it may not be inside the plugins folder).
    pub fn user_folder(mut self, folder: impl Into<PathBuf>) -> NativeExtensionsPlugin {
        self.user = Some(folder.into());
        self
    }

    /// Loads only the files the per-user [`ALLOWLIST_FILE`] already approves, asking nobody: for a host with no one to
    /// ask (a dedicated server, a test).
    pub fn deny_unknown(mut self) -> NativeExtensionsPlugin {
        self.decider = Decider::Allowlist { ask: false };
        self
    }

    /// Decides with `approver` instead.
    pub fn approver(mut self, approver: impl Approve + 'static) -> NativeExtensionsPlugin {
        self.decider = Decider::Custom(Rc::new(approver));
        self
    }

    /// The folder it loads from.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// The per-user folder it keeps the approvals and copies in.
    pub fn user(&self) -> Option<&Path> {
        self.user.as_deref()
    }

    /// The per-user folder, made, when it is usable: Err with the reason when it is unknown or inside the plugins
    /// folder.
    fn checked_user(&self) -> Result<PathBuf, String> {
        let user = self
            .user
            .clone()
            .ok_or("there is no per-user folder to keep approvals and loaded copies in")?;
        stage::make_private_dir(&user).map_err(|e| format!("{}: {e}", user.display()))?;
        let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        if canonical(&user).starts_with(canonical(&self.folder)) {
            return Err(format!(
                "the per-user folder {} is inside the plugins folder, where a download could write approvals",
                user.display()
            ));
        }
        private::check(&user)?;
        Ok(user)
    }
}

impl Default for NativeExtensionsPlugin {
    /// Loads from [`DEFAULT_FOLDER`].
    fn default() -> NativeExtensionsPlugin {
        NativeExtensionsPlugin::new(DEFAULT_FOLDER)
    }
}

impl Plugin for NativeExtensionsPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        if cfg!(target_arch = "wasm32") {
            engine.warn("native extensions do not load in a browser build: none was looked for");
            engine.insert_resource(LoadReport::default());
            return;
        }
        if !self.folder.is_dir() {
            engine.insert_resource(LoadReport::default());
            return;
        }
        let shipped = self.folder.join(ALLOWLIST_FILE);
        if shipped.exists() {
            engine.warn(format!(
                "native extensions: {} is ignored: approvals are kept per user, never in the plugins folder",
                shipped.display()
            ));
        }
        let user = match self.checked_user() {
            Ok(user) => user,
            Err(reason) => {
                engine.warn(format!("native extensions: none is loaded: {reason}"));
                engine.insert_resource(LoadReport::default());
                return;
            }
        };
        let approver: Rc<dyn Approve> = match &self.decider {
            Decider::Allowlist { ask: true } => {
                Rc::new(Allowlist::new(user.join(ALLOWLIST_FILE)).asking(TerminalPrompt))
            }
            Decider::Allowlist { ask: false } => Rc::new(Allowlist::new(user.join(ALLOWLIST_FILE))),
            Decider::Custom(approver) => Rc::clone(approver),
        };
        let cache = user.join(stage::CACHE_FOLDER);
        let report = load_folder(&self.folder, &cache, &*approver, &|id| {
            engine.is_plugin_added_id(id)
        });
        let used: HashSet<String> = report
            .iter()
            .filter(|(_, plugin)| plugin.is_some())
            .filter_map(|(entry, _)| entry.sha256.clone())
            .collect();
        stage::clean_cache(&cache, &used);
        let mut entries = Vec::with_capacity(report.len());
        for (entry, plugin) in report {
            match &entry.outcome {
                Outcome::Loaded => {}
                Outcome::NotApproved => {
                    engine.warn(format!(
                        "native extension {}: not approved, not loaded",
                        entry.label()
                    ));
                }
                Outcome::Refused(reason) => {
                    engine.warn(format!(
                        "native extension {}: refused: {reason}",
                        entry.label()
                    ));
                }
            }
            if let Some(plugin) = plugin {
                engine.add_plugins(plugin);
            }
            entries.push(entry);
        }
        engine.insert_resource(LoadReport { entries });
    }
}

/// Adds a [`NativeExtensionsPlugin`] turned off: game.toml (`[plugins] enable = ["torque.native-extensions"]`),
/// [`enable_by_flag`] or `EngineBuilder::enable_id` turns it on.
#[derive(Default)]
pub struct NativeExtensions(pub NativeExtensionsPlugin);

impl PluginGroup for NativeExtensions {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::new()
            .add(self.0)
            .disable::<NativeExtensionsPlugin>()
    }
}

/// Turns the [`NativeExtensionsPlugin`] on when `args` hold [`FLAG`]: true when they do.
pub fn enable_by_flag<S: AsRef<str>>(
    engine: &mut EngineBuilder,
    args: impl IntoIterator<Item = S>,
) -> bool {
    let on = args.into_iter().any(|a| a.as_ref() == FLAG);
    if on {
        engine.enable_id(<NativeExtensionsPlugin as torque_engine::PluginType>::ID.as_str());
    }
    on
}

/// What the [`NativeExtensionsPlugin`] did with each manifest in its folder, in name order: an engine resource.
#[derive(Clone, Debug, Default)]
pub struct LoadReport {
    /// One entry per manifest.
    pub entries: Vec<ReportEntry>,
}

impl LoadReport {
    /// The entry of the extension with the id `id`.
    pub fn get(&self, id: &str) -> Option<&ReportEntry> {
        self.entries.iter().find(|e| e.id.as_deref() == Some(id))
    }
}

/// What happened to one manifest.
#[derive(Clone, Debug)]
pub struct ReportEntry {
    /// The manifest's path.
    pub manifest: PathBuf,
    /// The library's path, once the manifest named one.
    pub library: Option<PathBuf>,
    /// The id the manifest gives.
    pub id: Option<String>,
    /// The version the manifest gives.
    pub version: Option<String>,
    /// The library's SHA-256 hash (lower-case hex), once it was read.
    pub sha256: Option<String>,
    /// The outcome.
    pub outcome: Outcome,
}

impl ReportEntry {
    fn new(manifest: &Path) -> ReportEntry {
        ReportEntry {
            manifest: manifest.to_path_buf(),
            library: None,
            id: None,
            version: None,
            sha256: None,
            outcome: Outcome::Loaded,
        }
    }

    /// How warnings name it: the id and version when known, else the manifest's file name.
    pub fn label(&self) -> String {
        match (&self.id, &self.version) {
            (Some(id), Some(version)) => format!("{id} {version}"),
            _ => self
                .manifest
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        }
    }
}

/// What happened to an extension's files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Loaded, and its plugin added.
    Loaded,
    /// The approver did not approve it: not loaded (none of its code ran).
    NotApproved,
    /// Refused, with the reason: a bad manifest, a missing file, another ABI version, an init that failed, a
    /// declaration the engine refused.
    Refused(String),
}

/// Reads every manifest in `folder` and loads what `approver` approves, from private copies in `cache`. A manifest whose
/// id `taken` says an engine plugin already has, or an earlier manifest of the folder took, is refused before its
/// library is read: adding it would fail the engine's build.
fn load_folder(
    folder: &Path,
    cache: &Path,
    approver: &dyn Approve,
    taken: &dyn Fn(&str) -> bool,
) -> Vec<(ReportEntry, Option<ExtensionPlugin>)> {
    let Ok(read) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut manifests: Vec<PathBuf> = read
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml") && p.is_file())
        .collect();
    manifests.sort();
    let mut out = Vec::new();
    let mut ids: HashSet<String> = HashSet::new();
    for (i, path) in manifests.iter().enumerate() {
        let mut entry = ReportEntry::new(path);
        if i >= MAX_EXTENSIONS {
            entry.outcome = Outcome::Refused(format!(
                "more than {MAX_EXTENSIONS} manifests in the folder"
            ));
            out.push((entry, None));
            continue;
        }
        let held = |id: &str| taken(id) || ids.contains(id);
        let plugin = load_one(folder, cache, path, approver, &held, &mut entry);
        if plugin.is_some()
            && let Some(id) = &entry.id
        {
            ids.insert(id.clone());
        }
        out.push((entry, plugin));
    }
    out
}

/// Loads the extension of the manifest at `path`, filling in `entry`.
fn load_one(
    folder: &Path,
    cache: &Path,
    path: &Path,
    approver: &dyn Approve,
    taken: &dyn Fn(&str) -> bool,
    entry: &mut ReportEntry,
) -> Option<ExtensionPlugin> {
    let refuse = |entry: &mut ReportEntry, reason: String| {
        entry.outcome = Outcome::Refused(reason);
        None
    };
    let manifest = match Manifest::read(path) {
        Ok(manifest) => manifest,
        Err(reason) => return refuse(entry, reason),
    };
    entry.id = Some(manifest.id.clone());
    entry.version = Some(manifest.version.clone());
    // The library reports the manifest's id or is refused, so the id is known before any of its code runs.
    if taken(&manifest.id) {
        return refuse(
            entry,
            format!(
                "the engine already has a plugin with the id {}",
                manifest.id
            ),
        );
    }
    let library = match manifest.library_path(folder) {
        Ok(library) => library,
        Err(reason) => return refuse(entry, reason),
    };
    entry.library = Some(library.clone());
    // Read once: these bytes are what is hashed, approved, checked and loaded.
    let read = match stage::read_library(&library) {
        Ok(read) => read,
        Err(reason) => return refuse(entry, reason),
    };
    entry.sha256 = Some(read.sha256.clone());
    let request = ApprovalRequest {
        id: manifest.id.clone(),
        version: manifest.version.clone(),
        library: library.clone(),
        sha256: read.sha256.clone(),
        approved_before: None,
    };
    if !approver.approve(&request) {
        entry.outcome = Outcome::NotApproved;
        return None;
    }
    // The file is untrusted until it is approved and checked: a panic reading it refuses it.
    let parsed = std::panic::catch_unwind(|| stage::exported_abi_version(&read.bytes));
    let version = match parsed {
        Ok(Ok(version)) => version,
        Ok(Err(reason)) => return refuse(entry, reason),
        Err(_) => return refuse(entry, "its export table could not be read".to_string()),
    };
    if let Err(reason) = ffi::check_abi(version) {
        return refuse(entry, reason);
    }
    if let Err(reason) = stage::check_search_paths(&read.bytes) {
        return refuse(entry, reason);
    }
    // Held open until the library has loaded, so the copy cannot be replaced in between.
    let held = match stage::cache_copy(&read, cache) {
        Ok(held) => held,
        Err(reason) => return refuse(entry, reason),
    };
    if !held.still_same() {
        return refuse(
            entry,
            "its private copy was replaced before it was loaded".to_string(),
        );
    }
    let loaded = ExtensionPlugin::load_library(&held, &library, &manifest, &read.sha256);
    drop(held);
    match loaded {
        Ok(plugin) => Some(plugin),
        Err(reason) => refuse(entry, reason),
    }
}

/// The id of the [`NativeExtensionsPlugin`].
pub const PLUGIN_ID: PluginId = <NativeExtensionsPlugin as torque_engine::PluginType>::ID;

#[cfg(test)]
mod tests;
