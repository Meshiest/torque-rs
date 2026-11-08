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
//!   the per-user folder's `cache`), checked again, and that copy is loaded by its absolute path, finding its own
//!   dependencies only among the system's libraries;
//! - refuses a library built for another ABI version before it runs any of its code (the version is read from the
//!   file's export table, not by loading it);
//! - catches a panic at every entry into the extension and disables the extension instead of crashing the game.
//!
//! **What an approval covers**: the one library file with that hash, under that id and version. Nothing else it reads or
//! loads is checked, so an extension must be linked statically: a library it needs that is not one of the system's
//! (Windows' System32, the system library paths elsewhere) is not found from the private copy, and it does not load.
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

#![warn(missing_docs)]

mod approve;
pub mod call_policy;
mod extension;
mod ffi;
mod language;
mod manifest;
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
        std::fs::create_dir_all(&user).map_err(|e| format!("{}: {e}", user.display()))?;
        let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        if canonical(&user).starts_with(canonical(&self.folder)) {
            return Err(format!(
                "the per-user folder {} is inside the plugins folder, where a download could write approvals",
                user.display()
            ));
        }
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
        let report = load_folder(&self.folder, &cache, &*approver);
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

/// Reads every manifest in `folder` and loads what `approver` approves, from private copies in `cache`.
fn load_folder(
    folder: &Path,
    cache: &Path,
    approver: &dyn Approve,
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
    for (i, path) in manifests.iter().enumerate() {
        let mut entry = ReportEntry::new(path);
        if i >= MAX_EXTENSIONS {
            entry.outcome = Outcome::Refused(format!(
                "more than {MAX_EXTENSIONS} manifests in the folder"
            ));
            out.push((entry, None));
            continue;
        }
        let plugin = load_one(folder, cache, path, approver, &mut entry);
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
    let loaded = ExtensionPlugin::load_library(&held.path, &library, &manifest, &read.sha256);
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
