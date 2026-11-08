//! The default layout: a folder or zip with a `mod.toml` at its root, in the folders `mods` of the game's files.
//!
//! ```toml
//! id = "bigger-bombs"              # required: letters, digits, _ - .
//! version = "1.2.0"                # required
//! name = "Bigger Bombs"            # shown in lists (default: the id)
//! description = "Bombs with more bang."
//! author = "Ann"
//! requires = ["core >= 1.0", "sounds"]    # mods needed, loaded first
//! load_after = ["balance"]                # loaded after these, when they are there
//! load_before = ["cosmetics"]
//! requires_plugins = ["torque.audio"]     # plugin ids the engine must have
//! client_side = false                     # true: only clients load it
//! share = "download"                      # what a server asks of clients: none, required or download
//!
//! [scripts]                        # entry scripts per stage; a name or a list
//! server = "server.cs"
//! client = ["client.cs", "gui/hud.cs"]
//!
//! [extra]                          # free-form fields, kept for a game's own rules
//! priority = 10
//! ```
//!
//! Without `[scripts]`, each declared stage runs the script named after it (`server.cs`, `client.cs`) when the mod has one.
//! The file is read strictly: at most [`MAX_MANIFEST_BYTES`], UTF-8, and a key it does not know is a warning.

use std::collections::BTreeMap;

use serde::Deserialize;
use torque_engine::Engine;

use crate::info::{ModInfo, Requirement, Version, clean};
use crate::layout::{Candidate, ModLayout, ModRoot, ModStages};
use crate::share::Share;

/// The manifest's file name.
pub const MANIFEST: &str = "mod.toml";

/// The longest manifest read (bytes).
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;

/// The folder of the game's files the default layout searches.
pub const DEFAULT_FOLDER: &str = "mods";

/// The default [`ModLayout`]: `mod.toml` in folders and zips (see the module docs), searching the folder of the game's files
/// that `[mods] folder` in game.toml names (`mods` by default) and any folders added with [`disk`](Self::disk).
#[derive(Clone, Debug, Default)]
pub struct ModTomlLayout {
    /// The folders of the game's files to search, instead of game.toml's (empty: game.toml's, or `mods`).
    pub folders: Vec<String>,
    /// Folders on the disk to search as well.
    pub disk: Vec<std::path::PathBuf>,
}

impl ModLayout for ModTomlLayout {
    fn roots(&self, engine: &Engine) -> Vec<ModRoot> {
        let mut roots = Vec::new();
        if self.folders.is_empty() {
            let folder = engine
                .resource::<crate::ModsConfig>()
                .map_or_else(|| DEFAULT_FOLDER.to_string(), |c| c.folder.clone());
            roots.push(ModRoot::files(folder));
        } else {
            roots.extend(self.folders.iter().cloned().map(ModRoot::files));
        }
        roots.extend(self.disk.iter().cloned().map(ModRoot::disk));
        roots
    }

    fn recognize(
        &self,
        candidate: &Candidate<'_>,
        stages: &ModStages,
    ) -> Result<Option<ModInfo>, String> {
        let bytes = candidate
            .package
            .read(MANIFEST)
            .ok_or_else(|| format!("no {MANIFEST}"))?;
        parse(&bytes, candidate, stages).map(Some)
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Raw {
    id: Option<String>,
    version: Option<String>,
    name: Option<String>,
    description: Option<String>,
    author: Option<String>,
    requires: Vec<String>,
    load_after: Vec<String>,
    load_before: Vec<String>,
    requires_plugins: Vec<String>,
    client_side: bool,
    share: Option<String>,
    scripts: Option<BTreeMap<String, OneOrMany>>,
    extra: BTreeMap<String, toml::Value>,
    #[serde(flatten)]
    unknown: toml::Table,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

/// The manifest in `bytes`, for `candidate`.
fn parse(bytes: &[u8], candidate: &Candidate<'_>, stages: &ModStages) -> Result<ModInfo, String> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(format!(
            "{MANIFEST} is {} bytes; at most {MAX_MANIFEST_BYTES} are read",
            bytes.len()
        ));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| format!("{MANIFEST} is not UTF-8 text"))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let raw: Raw = toml::from_str(text).map_err(|e| {
        let at = e.span().map_or(String::new(), |span| {
            let line = text[..span.start.min(text.len())].matches('\n').count() + 1;
            format!(":{line}")
        });
        format!("{MANIFEST}{at}: {}", clean(e.message().trim_end()))
    })?;
    let id = match raw.id {
        Some(id) => id,
        None => return Err(format!("{MANIFEST} has no `id`")),
    };
    let Some(version) = raw.version else {
        return Err(format!("{MANIFEST} has no `version`"));
    };
    let mut info = ModInfo::new(id);
    info.hidden.push(MANIFEST.to_string());
    info.version = version.parse::<Version>()?;
    info.name = raw.name.unwrap_or_else(|| info.id.clone());
    info.description = raw.description.unwrap_or_default();
    info.author = raw.author.unwrap_or_default();
    info.category = candidate.name.category.clone();
    info.client_side = raw.client_side;
    if let Some(text) = &raw.share {
        match Share::parse(text) {
            Some(share) => info.share = Some(share),
            None => info.warnings.push(format!(
                "`share` is `{}`; it must be none, required or download; the default applies",
                clean(text)
            )),
        }
    }
    for text in &raw.requires {
        info.requires.push(text.parse::<Requirement>()?);
    }
    info.load_after = raw.load_after;
    info.load_before = raw.load_before;
    info.requires_plugins = raw.requires_plugins;
    for key in raw.unknown.keys() {
        info.warnings.push(format!(
            "the key `{}` of {MANIFEST} is not known; skipped",
            clean(key)
        ));
    }
    for (key, value) in raw.extra {
        let text = match value {
            toml::Value::String(s) => s,
            toml::Value::Integer(n) => n.to_string(),
            toml::Value::Float(n) => n.to_string(),
            toml::Value::Boolean(b) => b.to_string(),
            _ => {
                info.warnings.push(format!(
                    "the extra field `{}` is not text, a number or true/false; skipped",
                    clean(&key)
                ));
                continue;
            }
        };
        info.extra.insert(key, text);
    }
    match raw.scripts {
        Some(table) => {
            for (stage, paths) in table {
                let paths = match paths {
                    OneOrMany::One(path) => vec![path],
                    OneOrMany::Many(paths) => paths,
                };
                info.scripts.push((stage, paths));
            }
        }
        None => {
            for stage in stages.names() {
                let path = format!("{}.cs", stage.to_ascii_lowercase());
                if candidate.package.contains(&path) {
                    info.scripts.push((stage.clone(), vec![path]));
                }
            }
        }
    }
    Ok(info)
}
