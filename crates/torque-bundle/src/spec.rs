//! `bundle.toml`: what a game crate ships. It sits beside the crate's `Cargo.toml`, is a build input and is never shipped.
//!
//! ```toml
//! [bundle]
//! name = "my-game"                  # the bundle folder's name (`my-game-windows-x86_64`)
//! game = "game"                     # the game folder (with game.toml), shipped as `game/`
//! package = "my-game"               # the crate whose binaries are built
//! bins = ["my-game"]                # the binaries shipped at the bundle's root
//! features = ["bundled"]            # cargo features always on (with the ones game.toml gives)
//! exclude = ["dev/**", "debug.cs"]  # files of the game folder left out, beside the default exclusions
//!
//! [[extension]]                     # a native extension built from its own workspace, shipped in `plugins/`
//! manifest = "../ext/Cargo.toml"
//! config = "../ext/my_ext.toml"     # the manifest the engine loads it with, shipped beside the library
//! rustflags = ["-C", "target-feature=+crt-static"]
//!
//! [check]                           # run from the bundle folder after packaging
//! bin = "my-game"
//! args = ["--ticks", "64", "--work", "{work}"]   # `{work}`: a writable folder outside the bundle
//! expect = ["mission loaded"]       # lines of standard output that must appear
//! timeout_secs = 300
//!
//! [server]                          # the dedicated-server variant (`--server`): what differs from `[bundle]`
//! package = "my-game-server"        # the crate whose binaries are built (required)
//! bins = ["my-game-server"]         # the binaries shipped (required)
//! name = "my-game-server"           # default: the bundle's name and `-server`
//! game = "game"                     # default: `bundle.game`
//! features = []                     # cargo features always on; game.toml's plugins derive none
//! exclude = []                      # files of the game folder left out (not `bundle.exclude`)
//! extensions = false                # ship the `[[extension]]`s too
//!
//! [server.check]                    # the variant's own check, with the keys of `[check]`
//! bin = "my-game-server"
//! args = ["--ticks", "64"]
//! expect = ["Stopped"]
//! ```
//!
//! A bundle of the server variant must link no renderer, window or audio crate ([`crate::server`]).
//!
//! Paths are relative to the file. Unknown keys are errors, so a typo never ships the wrong thing.

use std::path::Path;

use serde::Deserialize;

/// The file's name.
pub const FILE: &str = "bundle.toml";

/// A `bundle.toml` (module docs).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub bundle: Bundle,
    #[serde(default, rename = "extension")]
    pub extensions: Vec<Extension>,
    pub check: Option<Check>,
    pub server: Option<Server>,
}

/// `[server]`: the dedicated-server variant of the bundle.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub name: Option<String>,
    pub package: String,
    pub game: Option<String>,
    pub bins: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub extensions: bool,
    pub check: Option<Check>,
}

/// `[bundle]`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    pub name: String,
    #[serde(default = "default_game")]
    pub game: String,
    pub package: String,
    pub bins: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

fn default_game() -> String {
    "game".to_string()
}

/// `[[extension]]`: a cdylib built from its own workspace.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Extension {
    pub manifest: String,
    pub config: String,
    #[serde(default)]
    pub rustflags: Vec<String>,
}

/// `[check]`: a headless run of a shipped binary.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub bin: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub expect: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    300
}

impl Spec {
    /// The `bundle.toml` in `dir`.
    pub fn load(dir: &Path) -> Result<Spec, String> {
        let path = dir.join(FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("{FILE}: cannot read it in {}: {e}", dir.display()))?;
        Spec::parse(&text)
    }

    /// The spec in `text`, checked: names are plain file names, the check runs a shipped binary.
    pub fn parse(text: &str) -> Result<Spec, String> {
        let spec: Spec = toml::from_str(text).map_err(|e| format!("{FILE}: {}", e.message()))?;
        check_name("bundle.name", &spec.bundle.name)?;
        check_name("bundle.package", &spec.bundle.package)?;
        if spec.bundle.bins.is_empty() {
            return Err(format!("{FILE}: `bundle.bins` names no binary"));
        }
        for bin in &spec.bundle.bins {
            check_name("bundle.bins", bin)?;
        }
        for feature in &spec.bundle.features {
            check_name("bundle.features", feature)?;
        }
        check_relative("bundle.game", &spec.bundle.game)?;
        for extension in &spec.extensions {
            check_relative("extension.manifest", &extension.manifest)?;
            check_relative("extension.config", &extension.config)?;
        }
        if let Some(check) = &spec.check
            && !spec.bundle.bins.contains(&check.bin)
        {
            return Err(format!(
                "{FILE}: `check.bin` is `{}`, which is not in `bundle.bins`",
                check.bin
            ));
        }
        if let Some(server) = &spec.server {
            if let Some(name) = &server.name {
                check_name("server.name", name)?;
            }
            check_name("server.package", &server.package)?;
            if server.bins.is_empty() {
                return Err(format!("{FILE}: `server.bins` names no binary"));
            }
            for bin in &server.bins {
                check_name("server.bins", bin)?;
            }
            for feature in &server.features {
                check_name("server.features", feature)?;
            }
            if let Some(game) = &server.game {
                check_relative("server.game", game)?;
            }
            if let Some(check) = &server.check
                && !server.bins.contains(&check.bin)
            {
                return Err(format!(
                    "{FILE}: `server.check.bin` is `{}`, which is not in `server.bins`",
                    check.bin
                ));
            }
        }
        Ok(spec)
    }

    /// The spec to package: this one, or with `server` the dedicated-server variant, which replaces the bundle's name,
    /// package, binaries, features, exclusions and check (and drops the extensions unless it keeps them).
    pub fn variant(self, server: bool) -> Result<Spec, String> {
        if !server {
            return Ok(Spec {
                server: None,
                ..self
            });
        }
        let Some(variant) = self.server else {
            return Err(format!(
                "{FILE}: has no [server] table, so there is no server variant"
            ));
        };
        Ok(Spec {
            bundle: Bundle {
                name: variant
                    .name
                    .unwrap_or_else(|| format!("{}-server", self.bundle.name)),
                game: variant.game.unwrap_or(self.bundle.game),
                package: variant.package,
                bins: variant.bins,
                features: variant.features,
                exclude: variant.exclude,
            },
            extensions: if variant.extensions {
                self.extensions
            } else {
                Vec::new()
            },
            check: variant.check,
            server: None,
        })
    }
}

/// A name that becomes a file name or a cargo argument: letters, digits, `-`, `_` and `.`, not starting with `.` or `-`.
fn check_name(key: &str, name: &str) -> Result<(), String> {
    let plain = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if name.is_empty() || name.len() > 64 || !plain || name.starts_with(['.', '-']) {
        return Err(format!(
            "{FILE}: `{key}` has `{name}`; use letters, digits, `-`, `_` and `.`"
        ));
    }
    Ok(())
}

/// A path relative to the file: not absolute, no drive.
fn check_relative(key: &str, path: &str) -> Result<(), String> {
    let bad = path.is_empty()
        || path.starts_with(['/', '\\'])
        || path.contains(':')
        || path.chars().any(char::is_control);
    if bad {
        return Err(format!(
            "{FILE}: `{key}` must be a path relative to {FILE} (got `{path}`)"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
[bundle]
name = "demo"
package = "demo-crate"
bins = ["demo"]
features = ["bundled"]

[[extension]]
manifest = "../ext/Cargo.toml"
config = "../ext/ext.toml"

[check]
bin = "demo"
args = ["--work", "{work}"]
expect = ["up"]
"#;

    #[test]
    fn a_full_spec_reads_with_its_defaults() {
        let spec = Spec::parse(GOOD).unwrap();
        assert_eq!(spec.bundle.game, "game");
        assert_eq!(spec.extensions.len(), 1);
        assert_eq!(spec.check.unwrap().timeout_secs, 300);
    }

    #[test]
    fn the_server_variant_replaces_what_it_names() {
        let text = format!(
            "{GOOD}\n[server]\npackage = \"demo-server\"\nbins = [\"demo-server\"]\n\
             [server.check]\nbin = \"demo-server\"\n"
        );
        let spec = Spec::parse(&text).unwrap();
        let plain = spec.clone().variant(false).unwrap();
        assert_eq!(plain.bundle.name, "demo");
        assert_eq!(plain.extensions.len(), 1);
        let server = spec.variant(true).unwrap();
        assert_eq!(server.bundle.name, "demo-server");
        assert_eq!(server.bundle.package, "demo-server");
        assert_eq!(server.bundle.game, "game");
        assert!(server.bundle.features.is_empty() && server.extensions.is_empty());
        assert_eq!(server.check.unwrap().bin, "demo-server");
        assert!(Spec::parse(GOOD).unwrap().variant(true).is_err());
        let wrong = text.replace("bin = \"demo-server\"\n", "bin = \"demo\"\n");
        assert!(Spec::parse(&wrong).is_err());
    }

    #[test]
    fn unknown_keys_bad_names_and_absolute_paths_are_refused() {
        let cases = [
            GOOD.replace("features =", "featurez ="),
            GOOD.replace("name = \"demo\"", "name = \"../demo\""),
            GOOD.replace("bins = [\"demo\"]", "bins = []"),
            GOOD.replace("../ext/Cargo.toml", "C:/ext/Cargo.toml"),
            GOOD.replace("../ext/ext.toml", "/ext/ext.toml"),
            GOOD.replace("bin = \"demo\"", "bin = \"other\""),
            GOOD.replace("\"bundled\"", "\"--all-features\""),
        ];
        for case in cases {
            assert!(Spec::parse(&case).is_err(), "accepted:\n{case}");
        }
    }
}
