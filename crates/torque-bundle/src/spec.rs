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
//! ```toml
//! [web]                             # the web target (`--web`): the engine's web build, as a folder a static host serves
//! scripts = "../../web"             # the engine's web scripts (build.mjs, lua-build.mjs, check.mjs)
//! example = "my-game"               # the example `build.mjs` builds
//! flags = ["--lua"]                 # `--lua` (the page loads the Lua module) and `--solo` (the page plays alone)
//! lua = "../ext/Cargo.toml"         # the Lua extension, built to WebAssembly by `lua-build.mjs` (required with `--lua`)
//! package = "my-game"               # the crate the module is built from (default: `bundle.package`), for the licenses
//! name = "my-game-web"              # default: the bundle's name and `-web`
//!
//! [web.check]                       # headless Chrome loads the folder from a local server: one `check.mjs` run for each mode
//! modes = ["menu", "refused"]       # `menu` (the page draws; a page that joins a server needs one) and `refused` (a changed file is refused); the default
//! seconds = 90                      # how long a page may take to be ready
//!                                   # (`scripts` must be the `web` folder of the crate's workspace unless `--scripts` is given)
//! timeout_secs = 600                # for each run
//! ```
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
    pub web: Option<Web>,
}

/// `[web]`: the web target of the bundle.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Web {
    pub name: Option<String>,
    pub scripts: String,
    pub example: String,
    pub package: Option<String>,
    #[serde(default)]
    pub flags: Vec<WebFlag>,
    pub lua: Option<String>,
    pub check: Option<WebCheck>,
}

/// A flag of `build.mjs` the web target passes.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum WebFlag {
    /// The page loads the Lua module before the game starts.
    #[serde(rename = "--lua")]
    Lua,
    /// The page is marked to be played alone.
    #[serde(rename = "--solo")]
    Solo,
}

impl WebFlag {
    /// The flag as `build.mjs` takes it.
    pub fn arg(self) -> &'static str {
        match self {
            WebFlag::Lua => "--lua",
            WebFlag::Solo => "--solo",
        }
    }
}

/// A run of `check.mjs` that needs no server and no click.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WebMode {
    /// The page's menu is drawn.
    Menu,
    /// A fresh browser is given every file with a byte changed, and the page must refuse them.
    Refused,
}

impl WebMode {
    /// The mode as `check.mjs` takes it.
    pub fn arg(self) -> &'static str {
        match self {
            WebMode::Menu => "menu",
            WebMode::Refused => "refused",
        }
    }
}

/// `[web.check]`: headless runs of `check.mjs` against the bundle folder.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WebCheck {
    #[serde(default = "default_modes")]
    pub modes: Vec<WebMode>,
    #[serde(default = "default_seconds")]
    pub seconds: u64,
    #[serde(default = "default_web_timeout")]
    pub timeout_secs: u64,
}

fn default_modes() -> Vec<WebMode> {
    vec![WebMode::Menu, WebMode::Refused]
}

fn default_seconds() -> u64 {
    90
}

fn default_web_timeout() -> u64 {
    600
}

impl Web {
    /// The folder `build.mjs` writes below `<target>/web/`: the example's name with `-solo` when the page plays alone.
    pub fn output_name(&self) -> String {
        let solo = if self.flags.contains(&WebFlag::Solo) {
            "-solo"
        } else {
            ""
        };
        format!("{}{solo}", self.example)
    }
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
        if let Some(web) = &spec.web {
            check_name("web.example", &web.example)?;
            if let Some(name) = &web.name {
                check_name("web.name", name)?;
            }
            if let Some(package) = &web.package {
                check_name("web.package", package)?;
            }
            check_relative("web.scripts", &web.scripts)?;
            if let Some(lua) = &web.lua {
                check_relative("web.lua", lua)?;
            }
            if let Some(check) = &web.check
                && check.modes.is_empty()
            {
                return Err(format!(
                    "{FILE}: `web.check.modes` names no mode, so there is nothing to run; leave out `[web.check]` for no check"
                ));
            }
            if web.flags.contains(&WebFlag::Lua) != web.lua.is_some() {
                return Err(format!(
                    "{FILE}: `web.lua` and the `--lua` flag go together (the module is built from `web.lua`, and only `--lua` ships it)"
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
            web: None,
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

    const WEB: &str = "\n[web]\nscripts = \"../../web\"\nexample = \"demo\"\nflags = [\"--lua\", \"--solo\"]\nlua = \"../ext/Cargo.toml\"\n";

    #[test]
    fn the_web_target_reads_with_its_defaults() {
        let spec = Spec::parse(&format!("{GOOD}{WEB}")).unwrap();
        let web = spec.web.unwrap();
        assert_eq!(web.flags, [WebFlag::Lua, WebFlag::Solo]);
        assert_eq!(web.output_name(), "demo-solo");
        assert!(web.check.is_none());
        let with_check = format!("{GOOD}{WEB}\n[web.check]\n");
        let check = Spec::parse(&with_check)
            .unwrap()
            .web
            .unwrap()
            .check
            .unwrap();
        assert_eq!(check.modes, [WebMode::Menu, WebMode::Refused]);
        assert_eq!((check.seconds, check.timeout_secs), (90, 600));
        // The server variant has no web target.
        let server = format!("{GOOD}{WEB}\n[server]\npackage = \"s\"\nbins = [\"s\"]\n");
        let variant = Spec::parse(&server).unwrap().variant(true).unwrap();
        assert!(variant.web.is_none());
    }

    #[test]
    fn a_web_target_with_a_bad_flag_name_path_or_no_mode_is_refused() {
        let cases = [
            WEB.replace("--solo", "--embed"),
            WEB.replace(
                "flags = [\"--lua\", \"--solo\"]\n",
                "flags = [\"--solo\"]\n",
            ),
            WEB.replace("lua = \"../ext/Cargo.toml\"\n", ""),
            WEB.replace("example = \"demo\"", "example = \"../demo\""),
            WEB.replace("../../web", "/web"),
            WEB.replace("../ext/Cargo.toml", "C:/ext/Cargo.toml"),
            format!("{WEB}scriptz = 1\n"),
            format!("{WEB}\n[web.check]\nmodes = []\n"),
        ];
        for case in cases {
            let text = format!("{GOOD}{case}");
            assert!(Spec::parse(&text).is_err(), "accepted:\n{case}");
        }
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
