//! Cargo and rustc, run as commands: the toolchain's facts, a crate's metadata, release builds with remapped paths, and the
//! crates a build links.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

/// The cargo to run: `CARGO` (set when this runs under cargo), else `cargo` on the path.
fn cargo() -> Command {
    let program = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let mut command = Command::new(program);
    // A flag set for the outer build must not reach the bundle's: it gets its own.
    for name in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
    ] {
        command.env_remove(name);
    }
    command
}

fn rustc() -> Command {
    Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc")))
}

/// Runs `command`, its standard error shown, and returns its standard output.
fn output(mut command: Command, what: &str) -> Result<String, String> {
    let out = command
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("{what}: cannot run it: {e}"))?;
    if !out.status.success() {
        return Err(format!("{what} failed ({})", out.status));
    }
    String::from_utf8(out.stdout).map_err(|_| format!("{what}: the output is not UTF-8"))
}

/// The toolchain: the host's target triple, rustc's version line and its sysroot.
#[derive(Clone, Debug)]
pub struct Toolchain {
    pub host: String,
    pub version: String,
    pub sysroot: PathBuf,
}

impl Toolchain {
    pub fn find() -> Result<Toolchain, String> {
        let mut verbose = rustc();
        verbose.arg("-vV");
        let verbose = output(verbose, "rustc -vV")?;
        let host = verbose
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .ok_or("rustc -vV printed no host")?
            .trim()
            .to_string();
        let version = verbose
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        let mut sysroot = rustc();
        sysroot.args(["--print", "sysroot"]);
        let sysroot = PathBuf::from(output(sysroot, "rustc --print sysroot")?.trim());
        Ok(Toolchain {
            host,
            version,
            sysroot,
        })
    }
}

/// A package of `cargo metadata`, the fields packaging reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub license: Option<String>,
    pub license_file: Option<String>,
    pub repository: Option<String>,
    pub authors: Vec<String>,
    pub manifest_path: PathBuf,
    /// A registry or git package (not a local path one).
    pub remote: bool,
    pub features: BTreeSet<String>,
}

impl Package {
    /// The package's folder.
    pub fn dir(&self) -> &Path {
        self.manifest_path.parent().unwrap_or(Path::new("."))
    }
}

/// `cargo metadata` of a workspace: its root and packages.
#[derive(Clone, Debug)]
pub struct Metadata {
    pub workspace_root: PathBuf,
    pub packages: Vec<Package>,
}

impl Metadata {
    /// The metadata of the workspace of `manifest`, with every dependency for `target` (`deps`), or only its members.
    pub fn read(manifest: &Path, target: &str, deps: bool) -> Result<Metadata, String> {
        let mut command = cargo();
        command
            .args([
                "metadata",
                "--format-version",
                "1",
                "--locked",
                "--manifest-path",
            ])
            .arg(manifest);
        if deps {
            command.args(["--filter-platform", target]);
        } else {
            command.arg("--no-deps");
        }
        let text = output(command, "cargo metadata")?;
        Metadata::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Metadata, String> {
        let json: Value = serde_json::from_str(text).map_err(|e| format!("cargo metadata: {e}"))?;
        let string =
            |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(String::from);
        let workspace_root = string(&json, "workspace_root")
            .map(PathBuf::from)
            .ok_or("cargo metadata: no workspace_root")?;
        let mut packages = Vec::new();
        for p in json
            .get("packages")
            .and_then(Value::as_array)
            .ok_or("cargo metadata: no packages")?
        {
            let features = p
                .get("features")
                .and_then(Value::as_object)
                .map(|f| f.keys().cloned().collect())
                .unwrap_or_default();
            packages.push(Package {
                name: string(p, "name").ok_or("cargo metadata: a package has no name")?,
                version: string(p, "version").unwrap_or_default(),
                license: string(p, "license"),
                license_file: string(p, "license_file"),
                repository: string(p, "repository"),
                authors: p
                    .get("authors")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default(),
                manifest_path: string(p, "manifest_path")
                    .map(PathBuf::from)
                    .ok_or("cargo metadata: a package has no manifest_path")?,
                remote: p.get("source").is_some_and(|s| !s.is_null()),
                features,
            });
        }
        Ok(Metadata {
            workspace_root,
            packages,
        })
    }

    pub fn package(&self, name: &str) -> Option<&Package> {
        self.packages.iter().find(|p| p.name == name)
    }

    pub fn package_version(&self, name: &str, version: &str) -> Option<&Package> {
        self.packages
            .iter()
            .find(|p| p.name == name && p.version == version)
    }
}

/// A release build: which manifest and package, what to build, for which target, with which features and flags.
#[derive(Clone, Debug)]
pub struct Build {
    pub manifest: PathBuf,
    /// The package (`-p`); none for a workspace with one package.
    pub package: Option<String>,
    /// `--bin` names; none builds the library (`--lib`).
    pub bins: Vec<String>,
    /// `--features`; the defaults are off when this is `Some`.
    pub features: Option<Vec<String>>,
    pub target: String,
    pub target_dir: PathBuf,
    pub rustflags: Vec<String>,
    /// `CFLAGS` and `CXXFLAGS`, for the C and C++ that build scripts compile (the `cc` crate reads them).
    pub cflags: Vec<String>,
}

/// What a build made: each binary's path by name, and the libraries' files.
#[derive(Clone, Debug, Default)]
pub struct Built {
    pub bins: Vec<(String, PathBuf)>,
    pub libraries: Vec<PathBuf>,
}

impl Build {
    fn selection(&self, command: &mut Command) {
        command
            .arg("--manifest-path")
            .arg(&self.manifest)
            .arg("--locked");
        if let Some(package) = &self.package {
            command.args(["-p", package]);
        }
        if let Some(features) = &self.features {
            command.arg("--no-default-features");
            if !features.is_empty() {
                command.args(["--features", &features.join(",")]);
            }
        }
        command.args(["--target", &self.target]);
    }

    /// The build folder: [`Build::target_dir`] and a key of the C flags under it. Cargo tracks the Rust flags itself, but a
    /// build script that does not declare `CFLAGS` would keep C compiled with other flags.
    pub fn build_dir(&self) -> PathBuf {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(self.cflags.join(" ").as_bytes());
        let key: String = hash.iter().take(6).map(|b| format!("{b:02x}")).collect();
        self.target_dir.join(format!("c-{key}"))
    }

    /// Builds, returning the binaries and the cdylib files.
    pub fn run(&self) -> Result<Built, String> {
        let mut command = cargo();
        command.args([
            "build",
            "--release",
            "--message-format=json-render-diagnostics",
        ]);
        self.selection(&mut command);
        if self.bins.is_empty() {
            command.arg("--lib");
        }
        for bin in &self.bins {
            command.args(["--bin", bin]);
        }
        command
            .env("CARGO_TARGET_DIR", self.build_dir())
            .env("CARGO_ENCODED_RUSTFLAGS", self.rustflags.join("\u{1f}"));
        if !self.cflags.is_empty() {
            let flags = self.cflags.join(" ");
            command.env("CFLAGS", &flags).env("CXXFLAGS", &flags);
        }
        let text = output(command, "cargo build")?;
        let mut built = Built::default();
        for line in text.lines() {
            let Ok(message) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if message.get("reason").and_then(Value::as_str) != Some("compiler-artifact") {
                continue;
            }
            let target = &message["target"];
            let kinds: Vec<&str> = target["kind"]
                .as_array()
                .map(|k| k.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let name = target["name"].as_str().unwrap_or_default();
            if kinds.contains(&"bin") && self.bins.iter().any(|b| b == name) {
                let exe = message["executable"]
                    .as_str()
                    .ok_or_else(|| format!("cargo build: the binary {name} has no file"))?;
                built.bins.push((name.to_string(), PathBuf::from(exe)));
            }
            if kinds.contains(&"cdylib") {
                let files = message["filenames"].as_array().cloned().unwrap_or_default();
                built.libraries.extend(
                    files
                        .iter()
                        .filter_map(Value::as_str)
                        .map(PathBuf::from)
                        .filter(|f| is_library(f)),
                );
            }
        }
        for bin in &self.bins {
            if !built.bins.iter().any(|(name, _)| name == bin) {
                return Err(format!("cargo build made no binary {bin}"));
            }
        }
        Ok(built)
    }

    /// The (name, version) of every crate the build links: normal and build dependencies (a `*-src` crate's C and C++ is a
    /// build dependency's), never proc-macros or dev-dependencies, for the build's own features and target.
    pub fn linked(&self) -> Result<Vec<(String, String)>, String> {
        let mut command = cargo();
        command.args([
            "tree",
            "-e",
            "normal,build,no-proc-macro",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ]);
        self.selection(&mut command);
        let text = output(command, "cargo tree")?;
        Ok(parse_tree(&text))
    }
}

/// The (name, version) of each `cargo tree --prefix none --format {p}` line, once each, sorted.
pub fn parse_tree(text: &str) -> Vec<(String, String)> {
    let set: BTreeSet<(String, String)> = text
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let name = words.next()?;
            let version = words.next()?.strip_prefix('v')?;
            Some((name.to_string(), version.to_string()))
        })
        .collect();
    set.into_iter().collect()
}

fn is_library(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("dll" | "so" | "dylib")
    )
}

/// The flags every bundle build gets: the local folders remapped to neutral names (most specific last, as rustc applies the
/// last match), and on MSVC the C runtime linked in and a reproducible link (`/Brepro`: no time stamp) with no PDB (`/DEBUG:NONE`: a PDB is never shipped,
/// and its id in the binary differs from build to build).
pub fn rustflags(remaps: &[(PathBuf, &str)], target: &str, extra: &[String]) -> Vec<String> {
    let mut flags: Vec<String> = remaps
        .iter()
        .map(|(from, to)| format!("--remap-path-prefix={}={to}", from.display()))
        .collect();
    if target.ends_with("-msvc") {
        for arg in ["/Brepro", "/DEBUG:NONE"] {
            flags.extend(["-C".to_string(), format!("link-arg={arg}")]);
        }
        // The C runtime linked in: a bundle needs no Visual C++ redistributable installed.
        flags.extend(["-C".to_string(), "target-feature=+crt-static".to_string()]);
    }
    flags.extend(extra.iter().cloned());
    flags
}

/// The C and C++ flags that remap the same folders in `__FILE__` (asserts in vendored C and C++ print it): `/pathmap` for
/// MSVC (in its deterministic mode), `-ffile-prefix-map` for GCC and Clang. `cc` splits the variables at spaces, so a folder with a space is left out (the
/// leak scan still catches it if it ends up in a file).
pub fn cflags(remaps: &[(PathBuf, &str)], target: &str) -> Vec<String> {
    // MSVC reads `/pathmap` only in its deterministic mode (which also refuses `__DATE__` and `__TIME__`).
    let msvc = target
        .ends_with("-msvc")
        .then(|| "/experimental:deterministic".to_string());
    msvc.into_iter()
        .chain(
            remaps
                .iter()
                .map(|(from, to)| (from.display().to_string(), to))
                .filter(|(from, _)| !from.contains(' '))
                .map(|(from, to)| {
                    if target.ends_with("-msvc") {
                        format!("/pathmap:{from}={to}")
                    } else {
                        format!("-ffile-prefix-map={from}={to}")
                    }
                }),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_lines_give_names_and_versions_once() {
        let text = "demo v0.1.0 (C:\\w\\demo)\nserde v1.0.229\ntoml v1.2.0+spec-1.1.0\nserde v1.0.229 (*)\n\n";
        assert_eq!(
            parse_tree(text),
            [
                ("demo".to_string(), "0.1.0".to_string()),
                ("serde".to_string(), "1.0.229".to_string()),
                ("toml".to_string(), "1.2.0+spec-1.1.0".to_string()),
            ]
        );
    }

    #[test]
    fn msvc_links_are_reproducible_and_paths_remapped() {
        let flags = rustflags(
            &[(PathBuf::from("/w"), "/workspace")],
            "x86_64-pc-windows-msvc",
            &[],
        );
        assert_eq!(flags[0], "--remap-path-prefix=/w=/workspace");
        assert!(flags.contains(&"link-arg=/Brepro".to_string()));
        let linux = rustflags(&[], "x86_64-unknown-linux-gnu", &["-Cx".to_string()]);
        assert_eq!(linux, ["-Cx"]);
        let remaps = [
            (PathBuf::from("/w"), "/workspace"),
            (PathBuf::from("/a b"), "/x"),
        ];
        assert_eq!(
            cflags(&remaps, "x86_64-pc-windows-msvc"),
            ["/experimental:deterministic", "/pathmap:/w=/workspace"]
        );
        assert_eq!(
            cflags(&remaps, "aarch64-apple-darwin"),
            ["-ffile-prefix-map=/w=/workspace"]
        );
    }

    #[test]
    fn metadata_reads_packages_and_sources() {
        let text = r#"{"workspace_root":"/w","packages":[
            {"name":"a","version":"0.1.0","license":"MIT","license_file":null,"repository":null,
             "manifest_path":"/w/a/Cargo.toml","source":null,"features":{"bundled":[],"gizmos":["x/gizmos"]}},
            {"name":"b","version":"2.0.0","license":"MIT OR Apache-2.0","manifest_path":"/r/b/Cargo.toml",
             "source":"registry+https://github.com/rust-lang/crates.io-index","features":{}}]}"#;
        let meta = Metadata::parse(text).unwrap();
        let a = meta.package("a").unwrap();
        assert!(!a.remote && a.features.contains("gizmos"));
        assert!(meta.package_version("b", "2.0.0").unwrap().remote);
        assert_eq!(a.dir(), Path::new("/w/a"));
    }
}
