//! Asset validation: every file of the game folder whose extension an engine loader owns is opened by that loader, under the
//! caps the engine reads it with, before the bundle is written. A file that is refused, or does not parse, fails the bundle
//! with its path and the reason ([`Limits`]; `--no-validate` skips it).
//!
//! The game folder is untrusted input to this tool: a file is read through the resource layer's no-follow reader, within
//! [`Limits::file_bytes`] (a bigger file is refused before it is read), the game folder as a whole within
//! [`Limits::total_bytes`], [`Limits::files`] and [`Limits::depth`], and each loader is given the work budget [`Limits::work`]
//! and its own byte caps. A zip is mounted as the engine mounts it ([`ZipMount`]) and every entry is opened the same way; the
//! entries inflate at most [`Limits::inflated_bytes`] in all. Every message that names a file or quotes one is made printable
//! ([`precompile::printable`]), so a hostile file cannot write escapes to the terminal.
//!
//! | Extensions | Opened with |
//! |---|---|
//! | `dts`, `glb`, `gltf` | the glTF importer where it applies, then `torque_shapes::dts::parse` |
//! | `dsq` | `torque_shapes::dsq::parse` |
//! | `dif`, `ter`, `dml` | `torque_maps` (a `.dif` is parsed as the engine reads it; its cross-reference check is not run, since the engine loads files that fail it) |
//! | `png`, `jpg` | `torque_images` (`torque_images::Kind::of_name` says which) |
//! | `wav`, `ogg` | `torque_audio::buffer::decode` |
//! | `uft`, `gft`, `ttf` | `torque_fonts` |
//! | `cs`, `gui`, `mis` | the TorqueScript compiler of the game's [`ScriptFormat`] (a syntax error is a refusal) |
//! | `dso` | the code loader of that format |
//! | `zip` | `torque_resource::zips::ZipMount`, then each entry by its extension |
//! | `toml` | the TOML parser (a `mod.toml` is read at most [`MAX_MANIFEST_BYTES`]) |
//!
//! Other files (`.lua`, text, and formats a plugin owns) are counted as unowned and shipped as they are. So are `.obj`,
//! `.bbmodel` and `.map`: the engine reads them only when a game turns their format plugin on (they are off in the default set,
//! and read as trusted files only), so a bundle of a game that does not is not made refusable by them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use torque_bytes::budget_charge;
use torque_resource::files::{
    MAX_MOUNTED_BYTES, MAX_MOUNTED_ENTRIES, MAX_MOUNTED_ZIPS, MAX_READ_BYTES,
};
use torque_resource::plain::read_plain;
use torque_resource::zips::{MAX_ZIP_BYTES, ZipMount, entry_name_ok};

use crate::precompile::{self, ScriptFormat, printable};
use crate::select::{Caps, Selected};

/// The caps on what a game folder may hold and on the work of opening it. The defaults are the engine's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The most files and folders below the game folder, counted as the folder is walked.
    pub files: usize,
    /// The deepest folder walked (the game folder is 0).
    pub depth: usize,
    /// The largest file read (the resource layer's [`MAX_READ_BYTES`]); a loader's own cap may be lower.
    pub file_bytes: u64,
    /// The most bytes the game folder's files hold together, all held in memory while the bundle is made.
    pub total_bytes: u64,
    /// The most zips opened.
    pub zips: usize,
    /// The most zip entries opened in all.
    pub entries: usize,
    /// The most bytes the zip entries inflate to in all.
    pub inflated_bytes: u64,
    /// The work units a loader that charges its work may spend on one file.
    pub work: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        let walk = Caps::default();
        Limits {
            files: walk.entries,
            depth: walk.depth,
            file_bytes: MAX_READ_BYTES,
            total_bytes: MAX_MOUNTED_BYTES as u64,
            zips: MAX_MOUNTED_ZIPS,
            entries: MAX_MOUNTED_ENTRIES,
            inflated_bytes: MAX_MOUNTED_BYTES as u64,
            work: 1 << 28,
        }
    }
}

impl Limits {
    /// The caps of the walk of the game folder.
    pub fn walk(&self) -> Caps {
        Caps {
            depth: self.depth,
            entries: self.files,
        }
    }
}

/// The largest settings file (`game.toml`), as the engine's config reader takes it.
pub const MAX_CONFIG_BYTES: u64 = 4 << 20;

/// The largest `mod.toml`, as the mods plugin reads it.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

/// A file refused: its path in the game folder (`<zip>/<entry>` for a zip's entry) and why, both printable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub path: String,
    pub reason: String,
}

impl Refusal {
    /// A refusal of `path` for `reason`, any control character in either replaced.
    pub fn new(path: &str, reason: impl AsRef<str>) -> Refusal {
        Refusal {
            path: printable(path),
            reason: printable(reason.as_ref()),
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.reason)
    }
}

/// What the engine reads a file as, by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Shape,
    Sequence,
    Interior,
    Terrain,
    MaterialList,
    Png,
    Jpeg,
    Sound,
    BitmapFont,
    TrueType,
    Script,
    Compiled,
    Archive,
    Config,
}

impl Kind {
    /// The kind of the file `path` (its extension, case ignored), or None for a file no loader here owns.
    pub fn of(path: &str) -> Option<Kind> {
        if let Some(image) = torque_images::Kind::of_name(path) {
            return Some(match image {
                torque_images::Kind::Png => Kind::Png,
                torque_images::Kind::Jpeg => Kind::Jpeg,
            });
        }
        let (_, extension) = path.rsplit_once('.')?;
        Some(match extension.to_ascii_lowercase().as_str() {
            "dts" | "glb" | "gltf" => Kind::Shape,
            "dsq" => Kind::Sequence,
            "dif" => Kind::Interior,
            "ter" => Kind::Terrain,
            "dml" => Kind::MaterialList,
            "wav" | "ogg" => Kind::Sound,
            "uft" | "gft" => Kind::BitmapFont,
            "ttf" => Kind::TrueType,
            "cs" | "gui" | "mis" => Kind::Script,
            "dso" => Kind::Compiled,
            "zip" => Kind::Archive,
            "toml" => Kind::Config,
            _ => return None,
        })
    }

    /// The word the report counts the kind under.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Shape => "shape",
            Kind::Sequence => "sequence",
            Kind::Interior => "interior",
            Kind::Terrain => "terrain",
            Kind::MaterialList => "material list",
            Kind::Png => "png",
            Kind::Jpeg => "jpeg",
            Kind::Sound => "sound",
            Kind::BitmapFont => "bitmap font",
            Kind::TrueType => "truetype font",
            Kind::Script => "script",
            Kind::Compiled => "compiled script",
            Kind::Archive => "zip",
            Kind::Config => "settings",
        }
    }

    /// The most bytes the loader of `path` reads of a file of this kind.
    fn max_bytes(self, path: &str, limits: &Limits) -> u64 {
        let cap = match self {
            Kind::Shape if torque_import::native::is_gltf_path(path.as_bytes()) => {
                torque_import::caps::MAX_NATIVE_FILE_BYTES as u64
            }
            Kind::Shape => torque_shapes::MAX_DTS_BYTES as u64,
            Kind::Sequence => torque_shapes::MAX_DSQ_BYTES as u64,
            Kind::Interior => torque_maps::MAX_DIF_BYTES as u64,
            Kind::Terrain => torque_maps::MAX_TER_BYTES as u64,
            Kind::MaterialList => torque_maps::MAX_DML_BYTES as u64,
            Kind::Png => torque_images::png::MAX_PNG_FILE_BYTES as u64,
            Kind::Sound => torque_audio::MAX_SOUND_FILE_BYTES as u64,
            Kind::BitmapFont => torque_fonts::MAX_FILE_BYTES as u64,
            Kind::TrueType => torque_fonts::ttf::MAX_TTF_BYTES as u64,
            Kind::Script => torque_ts_compiler::MAX_SOURCE_BYTES as u64,
            Kind::Compiled => torque_ts_vm::vm::MAX_DSO_BYTES as u64,
            Kind::Archive => MAX_ZIP_BYTES as u64,
            Kind::Config if is_manifest(path) => MAX_MANIFEST_BYTES,
            Kind::Config => MAX_CONFIG_BYTES,
            Kind::Jpeg => limits.file_bytes,
        };
        cap.min(limits.file_bytes)
    }
}

/// Whether `path` names a mod's manifest (`mod.toml`, any case).
fn is_manifest(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case(precompile::MOD_MANIFEST))
}

/// What opening the game folder came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Files opened without a refusal, counted by [`Kind::label`] (a zip's entries among them).
    pub opened: BTreeMap<&'static str, usize>,
    /// Files no loader here owns, shipped as they are.
    pub unowned: usize,
    /// Files refused, in the order met.
    pub refused: Vec<Refusal>,
    /// Scripts shipped as `.dso` in place of their source (`--precompile`).
    pub precompiled: usize,
    /// Scripts `--precompile` left as source, with the reason (a mod's, a script that does not compile, a path too long).
    pub not_precompiled: Vec<Refusal>,
}

impl Report {
    /// The files opened, all kinds.
    pub fn opened_total(&self) -> usize {
        self.opened.values().sum()
    }

    /// The report as one line per fact, for the command line.
    pub fn summary(&self) -> Vec<String> {
        let kinds: Vec<String> = self
            .opened
            .iter()
            .map(|(label, n)| format!("{n} {label}"))
            .collect();
        let mut lines = vec![format!(
            "{} files opened by their loaders ({}), {} unowned, {} refused",
            self.opened_total(),
            if kinds.is_empty() {
                "none".to_string()
            } else {
                kinds.join(", ")
            },
            self.unowned,
            self.refused.len()
        )];
        if self.precompiled > 0 || !self.not_precompiled.is_empty() {
            lines.push(format!(
                "{} scripts compiled, {} left as source",
                self.precompiled,
                self.not_precompiled.len()
            ));
        }
        lines
    }

    fn refuse(&mut self, path: &str, reason: impl AsRef<str>) {
        self.refused.push(Refusal::new(path, reason));
    }

    fn keep_source(&mut self, path: &str, reason: impl AsRef<str>) {
        self.not_precompiled.push(Refusal::new(path, reason));
    }
}

/// A file of the game folder, read: its path in the bundle's `game/` and the bytes to ship.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// What [`process`] does besides reading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mode {
    /// Open every file with its loader and refuse the ones that fail.
    pub validate: bool,
    /// Ship the scripts compiled ([`precompile`]).
    pub precompile: bool,
    /// The format the game's scripts are read in.
    pub format: ScriptFormat,
}

/// What the game folder says about its mods, for [`process`].
#[derive(Clone, Debug)]
pub struct GameInfo {
    /// `game.toml`'s `[mods] folder` ([`precompile::mods_folder`]).
    pub mods_folder: String,
}

impl GameInfo {
    /// What `game_dir` says: its `game.toml`, read within [`MAX_CONFIG_BYTES`] (a missing or unreadable one says nothing).
    pub fn read(game_dir: &std::path::Path) -> GameInfo {
        let toml = read_plain(&game_dir.join("game.toml"), MAX_CONFIG_BYTES, None).ok();
        GameInfo {
            mods_folder: precompile::mods_folder(toml.as_deref()),
        }
    }
}

impl Default for GameInfo {
    fn default() -> GameInfo {
        GameInfo {
            mods_folder: precompile::MODS_FOLDER.to_string(),
        }
    }
}

/// Reads the `selected` files of the game folder within `limits`, opens them ([`Mode::validate`]) and compiles the scripts
/// ([`Mode::precompile`]). The bytes returned are the ones to ship: the files as read, but for a compiled script, which
/// stands in place of its source at `<path>.dso`. Nothing is returned for a file that could not be read; whether the report
/// holds refusals is the caller's to act on.
pub fn process(
    selected: &[Selected],
    limits: &Limits,
    mode: Mode,
    game: &GameInfo,
) -> (Vec<GameFile>, Report) {
    let mut report = Report::default();
    let mut files = Vec::new();
    if selected.len() > limits.files {
        report.refuse(
            "",
            format!(
                "{} files in the game folder; the limit is {}",
                selected.len(),
                limits.files
            ),
        );
        return (files, report);
    }
    // Two paths that differ only in case are one file to the engine (and to Windows and macOS): neither is read.
    let mut taken: HashMap<String, &str> = HashMap::with_capacity(selected.len());
    let mut doubled: HashSet<&str> = HashSet::new();
    for file in selected {
        if let Some(first) = taken.insert(file.path.to_ascii_lowercase(), file.path.as_str()) {
            report.refuse(
                &file.path,
                format!("the same name as {first} when case is ignored"),
            );
            doubled.insert(file.path.as_str());
        }
    }
    let mod_dirs =
        precompile::mod_dirs(&game.mods_folder, selected.iter().map(|f| f.path.as_str()));
    let context = Context {
        taken: &taken,
        mod_dirs: &mod_dirs,
        format: mode.format,
    };
    let mut total = 0u64;
    let mut zips = 0usize;
    let mut zipped = ZipBudget {
        entries: 0,
        inflated: 0,
    };
    for file in selected {
        let path = file.path.as_str();
        if doubled.contains(path) {
            continue;
        }
        if !entry_name_ok(path.as_bytes()) {
            report.refuse(path, "a name the engine does not serve");
            continue;
        }
        let kind = Kind::of(path);
        let bytes = match read_plain(&file.source, limits.file_bytes, None) {
            Ok(bytes) => bytes,
            Err(e) => {
                report.refuse(path, e.to_string());
                continue;
            }
        };
        total = total.saturating_add(bytes.len() as u64);
        if total > limits.total_bytes {
            report.refuse(
                path,
                format!(
                    "the game folder holds more than {} bytes in all",
                    limits.total_bytes
                ),
            );
            break;
        }
        if mode.validate {
            match kind {
                None => report.unowned += 1,
                Some(Kind::Archive) => {
                    zips += 1;
                    if zips > limits.zips {
                        report.refuse(path, format!("more than {} zips", limits.zips));
                    } else {
                        open_zip(path, &bytes, limits, mode.format, &mut zipped, &mut report);
                    }
                }
                Some(kind) => match check(kind, path, &bytes, limits, mode.format) {
                    Ok(()) => *report.opened.entry(kind.label()).or_default() += 1,
                    Err(reason) => report.refuse(path, reason),
                },
            }
        }
        let (shipped, bytes) = if mode.precompile && precompile::is_source(path) {
            ship_compiled(path, bytes, &context, &mut report)
        } else {
            (path.to_string(), bytes)
        };
        files.push(GameFile {
            path: shipped,
            bytes,
        });
    }
    (files, report)
}

/// What the files of the game folder say about one of them.
struct Context<'a> {
    /// Every path, lower case, with its spelling.
    taken: &'a HashMap<String, &'a str>,
    /// The folders of the mods, lower case, each ending in `/` ([`precompile::mod_dirs`]).
    mod_dirs: &'a [String],
    format: ScriptFormat,
}

/// The path and bytes to ship for the script `path` read as `source`: its compiled file at `<path>.dso` when it compiles,
/// nothing sits there already and it is no mod's, else the source (the reason goes to the report).
fn ship_compiled(
    path: &str,
    source: Vec<u8>,
    context: &Context,
    report: &mut Report,
) -> (String, Vec<u8>) {
    let dso_path = format!("{path}{}", precompile::EXTENSION);
    if precompile::in_mod(path, context.mod_dirs) {
        report.keep_source(path, "a mod's scripts ship as source");
        return (path.to_string(), source);
    }
    if context.taken.contains_key(&dso_path.to_ascii_lowercase()) {
        report.keep_source(path, format!("{dso_path} is already in the game folder"));
        return (path.to_string(), source);
    }
    match context.format.compile(path, &source) {
        Ok(Some(dso)) => {
            report.precompiled += 1;
            (dso_path, dso)
        }
        Ok(None) => (path.to_string(), source),
        Err(reason) => {
            report.keep_source(path, reason);
            (path.to_string(), source)
        }
    }
}

/// What the zips opened so far have used.
struct ZipBudget {
    entries: usize,
    inflated: u64,
}

/// Opens the zip `path` as the engine mounts it and every entry by its extension.
fn open_zip(
    path: &str,
    bytes: &[u8],
    limits: &Limits,
    format: ScriptFormat,
    used: &mut ZipBudget,
    report: &mut Report,
) {
    let mount = match ZipMount::open(bytes.to_vec()) {
        Ok(mount) => mount,
        Err(e) => {
            report.refuse(path, format!("not a zip the engine mounts: {e:?}"));
            return;
        }
    };
    *report.opened.entry(Kind::Archive.label()).or_default() += 1;
    for entry in mount.entries() {
        let name = String::from_utf8_lossy(&entry.name).into_owned();
        let label = format!("{path}/{name}");
        used.entries += 1;
        if used.entries > limits.entries {
            report.refuse(&label, format!("more than {} zip entries", limits.entries));
            return;
        }
        if entry.folder {
            continue;
        }
        let Some(kind) = Kind::of(&name) else {
            report.unowned += 1;
            continue;
        };
        // A nested zip is no folder to the engine (only the scan's zips mount), so it is an unowned file here.
        if kind == Kind::Archive {
            report.unowned += 1;
            continue;
        }
        used.inflated = used.inflated.saturating_add(entry.size);
        if used.inflated > limits.inflated_bytes {
            report.refuse(
                &label,
                format!(
                    "the zips inflate to more than {} bytes in all",
                    limits.inflated_bytes
                ),
            );
            return;
        }
        let Some(data) = mount.read(&entry.name) else {
            report.refuse(&label, "the entry cannot be read (damaged or over its cap)");
            continue;
        };
        match check(kind, &name, &data, limits, format) {
            Ok(()) => *report.opened.entry(kind.label()).or_default() += 1,
            Err(reason) => report.refuse(&label, reason),
        }
    }
}

/// Opens `bytes`, the file `path` of `kind`, with its loader: Ok when the loader takes it, else the reason. Never panics on any
/// bytes (the loaders are the engine's own and fuzzed).
pub fn check(
    kind: Kind,
    path: &str,
    bytes: &[u8],
    limits: &Limits,
    format: ScriptFormat,
) -> Result<(), String> {
    let cap = kind.max_bytes(path, limits);
    if bytes.len() as u64 > cap {
        return Err(format!("{} bytes is over the limit of {cap}", bytes.len()));
    }
    let mut charge = budget_charge(limits.work);
    match kind {
        Kind::Shape => {
            let converted;
            let dts = if torque_import::native::is_gltf_path(path.as_bytes()) {
                converted = torque_import::native::dts_bytes(bytes, &mut charge)
                    .map_err(|e| e.to_string())?;
                converted.as_slice()
            } else {
                bytes
            };
            torque_shapes::dts::parse(dts, &mut charge)
                .map(drop)
                .map_err(|e| e.to_string())
        }
        Kind::Sequence => torque_shapes::dsq::parse(bytes, &mut charge)
            .map(drop)
            .map_err(|e| e.to_string()),
        Kind::Interior => torque_maps::dif::parse(bytes)
            .map(drop)
            .map_err(|e| e.to_string()),
        Kind::Terrain => torque_maps::ter::parse(bytes)
            .map(drop)
            .map_err(|e| e.to_string()),
        Kind::MaterialList => torque_maps::dml::parse(bytes)
            .map(drop)
            .map_err(|e| e.to_string()),
        Kind::Png => torque_images::png::decode(bytes)
            .map(drop)
            .map_err(|e| e.to_string()),
        Kind::Jpeg => torque_images::jpeg::decode(bytes)
            .map(drop)
            .map_err(|e| e.to_string()),
        Kind::Sound => {
            let mut messages = Vec::new();
            torque_audio::buffer::decode(
                path.as_bytes(),
                bytes,
                &torque_audio::wav::TgeWav,
                &mut messages,
            )
            .map(drop)
        }
        Kind::BitmapFont => if path.to_ascii_lowercase().ends_with(".gft") {
            torque_fonts::gft::parse_gft(bytes)
        } else {
            torque_fonts::Font::parse(bytes)
        }
        .map(drop)
        .map_err(|e| e.to_string()),
        Kind::TrueType => torque_fonts::ttf::TtfFace::parse(bytes)
            .map(drop)
            .map_err(|e| e.to_string()),
        Kind::Script => format.check_source(path, bytes),
        Kind::Compiled => format.check_compiled(path, bytes),
        Kind::Archive => Ok(()),
        Kind::Config => {
            let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8 text".to_string())?;
            text.parse::<toml::Table>()
                .map(drop)
                .map_err(|e| e.message().to_string())
        }
    }
}

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
