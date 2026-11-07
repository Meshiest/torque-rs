//! `game.toml`: the small file in a game's folder that says how the prebuilt engine runs it, read from the game's files so it
//! works from a folder, a zip or the web pack alike.
//!
//! ```toml
//! [game]
//! name = "Minimal"          # shown by hosts that show a name
//! server = "server.cs"      # the script a server runs first (default `server.cs`)
//! client = "main.cs"        # the script a client runs first (default `main.cs`)
//!
//! [plugins]
//! disable = ["torque.audio"]    # plugin ids to turn off
//! enable = []                   # plugin ids to turn on (ones a group added disabled)
//!
//! [runner]
//! id = "torque.dedicated-runner"   # which runner plugin runs the game
//! ```
//!
//! Every key is optional, and a game with no `game.toml` runs as it always did. [`GameConfig::load`] reads the file from the
//! [`GameFiles`], and [`EngineConfig::config`] applies it to an [`EngineBuilder`]: the plugin toggles (by id, as
//! [`EngineBuilder::disable_id`] does), the runner, and the entry scripts, which [`GameServer::start_engine`] and the client
//! builder then read from the engine ([`server_entry`], [`client_entry`]).
//!
//! **Untrusted input.** The file can come from a downloaded game folder, so it is read strictly and never panics: it is at most
//! [`MAX_BYTES`] long, a key it does not know is a warning (kept in [`GameConfig::warnings`] and shown with the engine's own),
//! a key of the wrong type or a malformed file is a [`ConfigError`] with the line and column, and its lists and names have
//! limits ([`MAX_IDS`], [`MAX_NAME_LEN`]).
//!
//! ```
//! use torque_app_server::config::{EngineConfig, GameConfig};
//! use torque_app_server::plugins::{MinimalPlugins, WaterPlugin};
//! use torque_engine::EngineBuilder;
//!
//! let config = GameConfig::parse("[plugins]\ndisable = [\"torque.water\"]\n")?;
//! let engine = EngineBuilder::new()
//!     .add_plugins(MinimalPlugins)
//!     .config(&config)
//!     .build()?;
//! assert!(!engine.is_plugin_added::<WaterPlugin>());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! [`GameServer::start_engine`]: crate::server::GameServer::start_engine

use std::fmt;

use serde::Deserialize;
use torque_engine::{Engine, EngineBuilder};

use crate::files::GameFiles;

/// The config's path in a game's files.
pub const PATH: &str = "game.toml";

/// The longest config read (bytes); a longer file is an error.
pub const MAX_BYTES: usize = 64 * 1024;

/// The most ids in each of `disable` and `enable`.
pub const MAX_IDS: usize = 256;

/// The longest id, name or script path (bytes).
pub const MAX_NAME_LEN: usize = 256;

/// The script a server runs first when the config does not name one.
pub const DEFAULT_SERVER_ENTRY: &str = "server.cs";

/// The script a client runs first when the config does not name one.
pub const DEFAULT_CLIENT_ENTRY: &str = "main.cs";

/// The ids of the runner plugins the engine ships. Choosing one in `[runner]` turns the others off, as the runner slot holds one.
pub const RUNNER_IDS: [&str; 4] = [
    "torque.window-runner",
    "torque.web-page-runner",
    "torque.offscreen-runner",
    "torque.dedicated-runner",
];

/// A game's config (module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameConfig {
    /// `[game] name`: what to call the game.
    pub name: Option<String>,
    /// `[game] server`: the script a server runs first, a path in the game's files.
    pub server: String,
    /// `[game] client`: the script a client runs first.
    pub client: String,
    /// `[plugins] disable`: ids of plugins to turn off.
    pub disable: Vec<String>,
    /// `[plugins] enable`: ids of plugins to turn on.
    pub enable: Vec<String>,
    /// `[runner] id`: the runner plugin that runs the game.
    pub runner: Option<String>,
    /// What the parse skipped: a key it does not know, an id in both lists. Shown with the engine's warnings.
    pub warnings: Vec<String>,
}

impl Default for GameConfig {
    fn default() -> GameConfig {
        GameConfig {
            name: None,
            server: DEFAULT_SERVER_ENTRY.to_string(),
            client: DEFAULT_CLIENT_ENTRY.to_string(),
            disable: Vec::new(),
            enable: Vec::new(),
            runner: None,
            warnings: Vec::new(),
        }
    }
}

/// A config that could not be read, with where in the file when that is known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
    /// What is wrong.
    pub message: String,
    /// The line (from 1) and the column (from 1, in characters) of the fault.
    pub at: Option<(usize, usize)>,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.at {
            Some((line, column)) => write!(f, "{PATH}:{line}:{column}: {}", self.message),
            None => write!(f, "{PATH}: {}", self.message),
        }
    }
}

impl std::error::Error for ConfigError {}

impl ConfigError {
    fn plain(message: impl Into<String>) -> ConfigError {
        ConfigError {
            message: message.into(),
            at: None,
        }
    }
}

/// The file as serde reads it: every key optional, and whatever else is in a section is kept to be reported.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Raw {
    game: RawGame,
    plugins: RawPlugins,
    runner: RawRunner,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawGame {
    name: Option<String>,
    server: Option<String>,
    client: Option<String>,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawPlugins {
    disable: Vec<String>,
    enable: Vec<String>,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawRunner {
    id: Option<String>,
    #[serde(flatten)]
    extra: toml::Table,
}

impl GameConfig {
    /// The config in the game's files ([`PATH`]); a game with none gets the default config.
    pub fn load(files: &GameFiles) -> Result<GameConfig, ConfigError> {
        match files.file(PATH) {
            Some(bytes) => GameConfig::from_bytes(bytes),
            None => Ok(GameConfig::default()),
        }
    }

    /// The config in `bytes`: at most [`MAX_BYTES`] of UTF-8 text.
    pub fn from_bytes(bytes: &[u8]) -> Result<GameConfig, ConfigError> {
        if bytes.len() > MAX_BYTES {
            return Err(ConfigError::plain(format!(
                "the file is {} bytes; at most {MAX_BYTES} are read",
                bytes.len()
            )));
        }
        let text = std::str::from_utf8(bytes).map_err(|e| ConfigError {
            message: "the file is not UTF-8 text".to_string(),
            at: Some(line_column(bytes, e.valid_up_to())),
        })?;
        GameConfig::parse(text.strip_prefix('\u{feff}').unwrap_or(text))
    }

    /// The config in `text`.
    pub fn parse(text: &str) -> Result<GameConfig, ConfigError> {
        if text.len() > MAX_BYTES {
            return Err(ConfigError::plain(format!(
                "the file is {} bytes; at most {MAX_BYTES} are read",
                text.len()
            )));
        }
        let raw: Raw = toml::from_str(text).map_err(|e| {
            let at = e
                .span()
                .map(|span| line_column(text.as_bytes(), span.start.min(text.len())));
            ConfigError {
                message: e.message().trim_end().to_string(),
                at,
            }
        })?;

        let mut config = GameConfig::default();
        for (table, extra) in [
            ("", &raw.extra),
            ("game.", &raw.game.extra),
            ("plugins.", &raw.plugins.extra),
            ("runner.", &raw.runner.extra),
        ] {
            for key in extra.keys() {
                config.warnings.push(format!(
                    "the key `{table}{}` is not known; skipped",
                    clean(key)
                ));
            }
        }
        config.name = raw
            .game
            .name
            .map(|name| check_text("game.name", name))
            .transpose()?;
        if let Some(server) = raw.game.server {
            config.server = check_script("game.server", &server)?;
        }
        if let Some(client) = raw.game.client {
            config.client = check_script("game.client", &client)?;
        }
        config.disable = check_ids("plugins.disable", raw.plugins.disable)?;
        config.enable = check_ids("plugins.enable", raw.plugins.enable)?;
        for id in &config.disable {
            if config.enable.contains(id) {
                config.warnings.push(format!(
                    "the plugin `{id}` is in both plugins.disable and plugins.enable; it is enabled"
                ));
            }
        }
        config.runner = raw
            .runner
            .id
            .map(|id| check_id("runner.id", id))
            .transpose()?;
        Ok(config)
    }
}

/// The line and column (both from 1; the column counts characters) of byte `offset` of `bytes`.
fn line_column(bytes: &[u8], offset: usize) -> (usize, usize) {
    let before = &bytes[..offset.min(bytes.len())];
    let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
    let start = before
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |at| at + 1);
    // Continuation bytes of a multi-byte character are not columns.
    let column = before[start..]
        .iter()
        .filter(|&&b| b & 0xC0 != 0x80)
        .count()
        + 1;
    (line, column)
}

/// `key` as it is shown in a warning: cut short, with no control characters.
fn clean(key: &str) -> String {
    key.chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_LEN)
        .collect()
}

fn too_long(key: &str) -> ConfigError {
    ConfigError::plain(format!("`{key}` is longer than {MAX_NAME_LEN} bytes"))
}

/// A free-text value: not too long, no control characters.
fn check_text(key: &str, value: String) -> Result<String, ConfigError> {
    if value.len() > MAX_NAME_LEN {
        return Err(too_long(key));
    }
    if value.chars().any(char::is_control) {
        return Err(ConfigError::plain(format!(
            "`{key}` has a control character"
        )));
    }
    Ok(value)
}

/// A plugin id: not empty, not too long, no spaces or control characters.
fn check_id(key: &str, id: String) -> Result<String, ConfigError> {
    if id.is_empty() {
        return Err(ConfigError::plain(format!("`{key}` has an empty id")));
    }
    if id.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(ConfigError::plain(format!(
            "`{key}` has an id with a space or a control character"
        )));
    }
    check_text(key, id)
}

fn check_ids(key: &str, ids: Vec<String>) -> Result<Vec<String>, ConfigError> {
    if ids.len() > MAX_IDS {
        return Err(ConfigError::plain(format!(
            "`{key}` has {} ids; at most {MAX_IDS} are read",
            ids.len()
        )));
    }
    let mut seen = Vec::with_capacity(ids.len());
    for id in ids {
        let id = check_id(key, id)?;
        if !seen.contains(&id) {
            seen.push(id);
        }
    }
    Ok(seen)
}

/// A script path inside the game's files: relative, with `/` between folders (a `\` is read as `/`), and nothing that climbs out.
fn check_script(key: &str, path: &str) -> Result<String, ConfigError> {
    let path = check_text(key, path.replace('\\', "/"))?;
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.contains(':')
        || path.split('/').any(|part| part.is_empty() || part == "..");
    if bad {
        return Err(ConfigError::plain(format!(
            "`{key}` must be a path inside the game's files, such as `server.cs` (got `{}`)",
            clean(&path)
        )));
    }
    Ok(path)
}

/// The engine builder's method for a game's config.
pub trait EngineConfig {
    /// Applies `config`: the runner it chose, then its `disable` and `enable` ids ([`EngineBuilder::disable_id`] and
    /// [`EngineBuilder::enable_id`]: an id no plugin has is a warning when the engine is built), and its own warnings go
    /// among the engine's ([`EngineBuilder::warn`]). The config is kept as a resource, which is how a server and a client read
    /// their entry scripts ([`server_entry`], [`client_entry`]).
    ///
    /// Call it after the plugins are added, so the runner can tell which runners the game has. Choosing a runner the engine does
    /// not have is a warning, and the runner that was set stays.
    fn config(&mut self, config: &GameConfig) -> &mut EngineBuilder;
}

impl EngineConfig for EngineBuilder {
    fn config(&mut self, config: &GameConfig) -> &mut EngineBuilder {
        for warning in &config.warnings {
            self.warn(format!("{PATH}: {warning}"));
        }
        if let Some(id) = &config.runner {
            if self.contains(id) {
                let others: Vec<&str> = RUNNER_IDS
                    .into_iter()
                    .filter(|other| other != id && self.contains(other))
                    .collect();
                for other in others {
                    self.disable_id(other);
                }
                self.enable_id(id);
            } else {
                self.warn(format!(
                    "{PATH}: the runner `{id}` is not in this build; the runner already set is kept"
                ));
            }
        }
        for id in &config.disable {
            self.disable_id(id);
        }
        for id in &config.enable {
            self.enable_id(id);
        }
        self.insert_resource(config.clone())
    }
}

/// The script a server of `engine` runs first: the config's, or [`DEFAULT_SERVER_ENTRY`].
pub fn server_entry(engine: &Engine) -> &str {
    engine
        .resource::<GameConfig>()
        .map_or(DEFAULT_SERVER_ENTRY, |config| config.server.as_str())
}

/// The script a client of `engine` runs first: the config's, or [`DEFAULT_CLIENT_ENTRY`].
pub fn client_entry(engine: &Engine) -> &str {
    engine
        .resource::<GameConfig>()
        .map_or(DEFAULT_CLIENT_ENTRY, |config| config.client.as_str())
}
