//! Packages a game made on torque-rs into a folder that runs on its own.
//!
//! The input is a game crate's folder with a [`bundle.toml`](spec) beside its `Cargo.toml`: the game folder (with its
//! `game.toml`, scripts, assets and mods), the binaries to ship, and the native extensions to build and ship beside them. The
//! output is `<out>/<name>-<os>-<arch>/`:
//!
//! | Path | What |
//! |---|---|
//! | `<bin>` (`.exe` on Windows) | the game's binaries, built in release with the plugins its `game.toml` keeps ([`features`]) |
//! | `game/` | the game folder, less the dev files ([`select`]) |
//! | `plugins/` | each extension's library and the manifest the engine loads it with |
//! | `licenses/` | the license texts of every crate linked, and an index ([`licenses`]) |
//! | `manifest.toml` | every file's size and SHA-256 ([`manifest`]) |
//!
//! The builds remap the computer's folders to neutral names and link reproducibly ([`cargo::rustflags`]); every file is then
//! scanned for those folders ([`scan`]) and one hit fails the bundle. Last, the `[check]` binary runs headless from the bundle
//! folder with the build's environment removed, and the bundle must still match its manifest after it.

pub mod cargo;
pub mod features;
pub mod licenses;
pub mod manifest;
pub mod scan;
pub mod select;
pub mod spec;

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cargo::{Build, Metadata, Toolchain};
use features::Toggles;
use licenses::Text;
use manifest::{Entry, Header};
use scan::Needle;
use spec::Spec;

/// What to package and where.
#[derive(Clone, Debug)]
pub struct Options {
    /// The game crate's folder (with `bundle.toml`).
    pub crate_dir: PathBuf,
    /// Where the bundle folder goes.
    pub out: PathBuf,
    /// The build folder; the bundle's builds go in `bundle/` under it.
    pub target_dir: PathBuf,
    /// Run the `[check]` after packaging.
    pub check: bool,
}

/// What a bundle came to.
#[derive(Clone, Debug)]
pub struct Report {
    pub folder: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub features: Vec<String>,
    pub skipped: Vec<String>,
    pub crates: usize,
    pub missing_licenses: Vec<String>,
    /// Crates that ship no license text, given the standard texts of their licenses.
    pub generic_licenses: Vec<String>,
    /// The check's standard output, when it ran.
    pub check: Option<String>,
}

/// The bundle folder's name for `name` on `target`: `name-windows-x86_64`.
pub fn folder_name(name: &str, target: &str) -> String {
    let arch = target.split('-').next().unwrap_or(target);
    let os = if target.contains("windows") {
        "windows"
    } else if target.contains("apple-darwin") {
        "macos"
    } else if target.contains("linux") {
        "linux"
    } else {
        target.split('-').nth(2).unwrap_or("unknown")
    };
    format!("{name}-{os}-{arch}")
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Packages the game (module docs).
pub fn bundle(options: &Options) -> Result<Report, String> {
    let crate_dir = absolute(&options.crate_dir)?;
    let target_dir = absolute(&options.target_dir)?;
    let spec = Spec::load(&crate_dir)?;
    let toolchain = Toolchain::find()?;
    let target = toolchain.host.clone();
    let game_dir = crate_dir.join(&spec.bundle.game);
    let toggles = match std::fs::read_to_string(game_dir.join("game.toml")) {
        Ok(text) => Toggles::parse(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Toggles::default(),
        Err(e) => return Err(format!("game.toml: {e}")),
    };

    let manifest_path = crate_dir.join("Cargo.toml");
    let members = Metadata::read(&manifest_path, &target, false)?;
    let package = members.package(&spec.bundle.package).ok_or_else(|| {
        format!(
            "bundle.toml: the package {} is not in this workspace",
            spec.bundle.package
        )
    })?;
    let features = features::derive(&toggles, &package.features, &spec.bundle.features);

    // The computer's folders: remapped in the builds (most specific last) and looked for in every file.
    let home = env_dir("USERPROFILE").or_else(|| env_dir("HOME"));
    let cargo_home = env_dir("CARGO_HOME").or_else(|| home.as_ref().map(|h| h.join(".cargo")));
    let rustup_home = env_dir("RUSTUP_HOME").or_else(|| home.as_ref().map(|h| h.join(".rustup")));
    let mut folders: Vec<(&str, PathBuf, &str)> = Vec::new();
    if let Some(home) = &home {
        folders.push(("the home folder", home.clone(), "/home"));
    }
    folders.push((
        "the workspace",
        members.workspace_root.clone(),
        "/workspace",
    ));
    let mut extension_roots = Vec::new();
    for extension in &spec.extensions {
        let ext = Metadata::read(&crate_dir.join(&extension.manifest), &target, false)?;
        folders.push((
            "an extension's workspace",
            ext.workspace_root.clone(),
            "/extension",
        ));
        extension_roots.push(ext.workspace_root);
    }
    if let Some(dir) = &cargo_home {
        folders.push(("the cargo home", dir.clone(), "/cargo"));
    }
    if let Some(dir) = &rustup_home {
        folders.push(("the rustup home", dir.clone(), "/rustup"));
    }
    folders.push((
        "the toolchain",
        toolchain.sysroot.clone(),
        "/rustup/toolchain",
    ));
    folders.push(("the build folder", target_dir.clone(), "/target"));
    let remaps: Vec<(PathBuf, &str)> = folders
        .iter()
        .map(|(_, path, to)| (path.clone(), *to))
        .collect();
    let needles: Vec<Needle> = folders
        .iter()
        .filter_map(|(label, path, _)| Needle::new(label, path))
        .chain(Needle::new("the crate folder", &crate_dir))
        .collect();

    // The game's binaries.
    let game_build = Build {
        manifest: manifest_path.clone(),
        package: Some(spec.bundle.package.clone()),
        bins: spec.bundle.bins.clone(),
        features: Some(features.clone()),
        target: target.clone(),
        target_dir: target_dir.join("bundle"),
        rustflags: cargo::rustflags(&remaps, &target, &[]),
        cflags: cargo::cflags(&remaps, &target),
    };
    let built = game_build.run()?;
    let mut files: BTreeMap<String, Text> = BTreeMap::new();
    let add = |files: &mut BTreeMap<String, Text>, path: String, source: Text| {
        if files.insert(path.clone(), source).is_some() {
            return Err(format!("two files would be {path} in the bundle"));
        }
        Ok(())
    };
    for (_, exe) in &built.bins {
        add(&mut files, file_name(exe)?, Text::File(exe.clone()))?;
    }

    // The extensions, each from its own workspace and build folder.
    let mut extension_builds = Vec::new();
    for (extension, root) in spec.extensions.iter().zip(&extension_roots) {
        let name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("extension")
            .to_string();
        let build = Build {
            manifest: crate_dir.join(&extension.manifest),
            package: None,
            bins: Vec::new(),
            features: None,
            target: target.clone(),
            target_dir: target_dir.join("bundle-extensions").join(&name),
            rustflags: cargo::rustflags(&remaps, &target, &extension.rustflags),
            cflags: cargo::cflags(&remaps, &target),
        };
        let made = build.run()?;
        let [library] = made.libraries.as_slice() else {
            return Err(format!(
                "the extension {name} built {} libraries; one is shipped",
                made.libraries.len()
            ));
        };
        add(
            &mut files,
            format!("plugins/{}", file_name(library)?),
            Text::File(library.clone()),
        )?;
        let config = crate_dir.join(&extension.config);
        add(
            &mut files,
            format!("plugins/{}", file_name(&config)?),
            Text::File(config),
        )?;
        extension_builds.push(build);
    }

    // The game folder.
    let (selected, skipped) = select::select(&game_dir, &spec.bundle.exclude)?;
    for file in selected {
        add(
            &mut files,
            format!("game/{}", file.path),
            Text::File(file.source),
        )?;
    }

    // The licenses of what the builds link.
    let game_meta = Metadata::read(&manifest_path, &target, true)?;
    let game_linked = game_build.linked()?;
    let mut extension_meta = Vec::new();
    for build in &extension_builds {
        extension_meta.push((
            Metadata::read(&build.manifest, &target, true)?,
            build.linked()?,
        ));
    }
    let mut builds: Vec<(&Metadata, &[(String, String)])> = vec![(&game_meta, &game_linked)];
    builds.extend(extension_meta.iter().map(|(m, l)| (m, l.as_slice())));
    let gathered = licenses::gather(&builds, &toolchain);
    for (path, text) in &gathered.files {
        add(&mut files, path.clone(), text.clone())?;
    }

    // Everything in memory, scanned before anything is written: a bundle that holds a folder of this computer never reaches
    // the disk.
    let mut written: Vec<(String, Vec<u8>)> = Vec::new();
    for (path, text) in &files {
        let bytes = match text {
            Text::File(source) => std::fs::read(source).map_err(|e| format!("{path}: {e}"))?,
            Text::Made(text) => text.clone().into_bytes(),
        };
        written.push((path.clone(), bytes));
    }
    written.push((
        format!("{}/index.txt", licenses::DIR),
        gathered.index.clone().into_bytes(),
    ));
    let entries: Vec<Entry> = written.iter().map(|(p, b)| Entry::of(p, b)).collect();
    let header = Header {
        name: spec.bundle.name.clone(),
        version: package.version.clone(),
        target: target.clone(),
        mode: "compiled".to_string(),
        features: features.clone(),
        rustc: toolchain.version.clone(),
    };
    let manifest_text = manifest::write(&header, &entries);
    written.push((manifest::FILE.to_string(), manifest_text.into_bytes()));
    let leaks: Vec<String> = written
        .iter()
        .flat_map(|(path, bytes)| {
            scan::scan(bytes, &needles)
                .into_iter()
                .map(move |label| format!("{path}: holds {label}"))
        })
        .collect();
    if !leaks.is_empty() {
        return Err(format!(
            "the bundle holds folders of this computer (nothing was written):\n  {}",
            leaks.join("\n  ")
        ));
    }

    // Written beside the folder, then moved over it: a bundle folder is whole or not there. An old one is replaced only
    // when it is a bundle.
    let out = absolute(&options.out)?;
    let name = folder_name(&spec.bundle.name, &target);
    let folder = out.join(&name);
    if folder.exists() {
        let is_bundle = folder.join(manifest::FILE).is_file();
        let empty = std::fs::read_dir(&folder).is_ok_and(|mut d| d.next().is_none());
        if !is_bundle && !empty {
            return Err(format!(
                "{} is not a bundle folder (no {}); it is left as it is",
                folder.display(),
                manifest::FILE
            ));
        }
    }
    let staging = out.join(format!("{name}.partial"));
    own_folder(&staging)?;
    for (path, bytes) in &written {
        let dest = staging.join(path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&dest, bytes).map_err(|e| format!("{path}: {e}"))?;
    }
    std::fs::remove_file(staging.join(MARKER)).map_err(|e| format!("{MARKER}: {e}"))?;
    if folder.exists() {
        std::fs::remove_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    }
    std::fs::rename(&staging, &folder).map_err(|e| format!("{}: {e}", folder.display()))?;

    let check = match (&spec.check, options.check) {
        (Some(check), true) => {
            let exe = built
                .bins
                .iter()
                .find(|(name, _)| *name == check.bin)
                .map(|(_, exe)| folder.join(file_name(exe).unwrap_or_default()))
                .ok_or("the check's binary was not built")?;
            let work = out.join(format!("{name}.work"));
            let out = run_check(&exe, &folder, &work, check)?;
            let problems = manifest::verify(&folder)?;
            if !problems.is_empty() {
                return Err(format!(
                    "the check changed the bundle:\n  {}",
                    problems.join("\n  ")
                ));
            }
            Some(out)
        }
        _ => None,
    };

    Ok(Report {
        folder,
        files: written.len(),
        bytes: written.iter().map(|(_, b)| b.len() as u64).sum(),
        features,
        skipped,
        crates: gathered.crates,
        missing_licenses: gathered.missing,
        generic_licenses: gathered.generic,
        check,
    })
}

/// The file this tool puts in a folder it made (a bundle being written, a check's work folder), so that it only ever empties
/// folders of its own.
pub const MARKER: &str = ".torque-bundle-folder";

/// Makes `dir` an empty folder of this tool's, with its [`MARKER`]: one that is missing is made, one with the marker is
/// emptied, and any other folder with something in it is an error and left as it is.
pub fn own_folder(dir: &Path) -> Result<(), String> {
    if dir.exists() {
        let ours = dir.join(MARKER).is_file();
        let empty = std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none());
        if !ours && !empty {
            return Err(format!(
                "{} is not a folder this tool made (no {MARKER}); it is left as it is",
                dir.display()
            ));
        }
        std::fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(dir.join(MARKER), "made by torque-bundle\n")
        .map_err(|e| format!("{}: {e}", dir.display()))
}

fn file_name(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(String::from)
        .ok_or_else(|| format!("{} has no file name", path.display()))
}

/// Runs the check's binary `exe` in the bundle folder `dir` with `{work}` as `work` (made or emptied by [`own_folder`]), no cargo or build
/// variables in its environment, until it exits or its time is up. Its standard output, when it exits 0 and prints every
/// expected line.
pub fn run_check(
    exe: &Path,
    dir: &Path,
    work: &Path,
    check: &spec::Check,
) -> Result<String, String> {
    own_folder(work)?;
    let work_text = work.to_string_lossy();
    let args: Vec<String> = check
        .args
        .iter()
        .map(|a| a.replace("{work}", &work_text))
        .collect();
    let mut command = Command::new(exe);
    command
        .args(&args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if text.starts_with("CARGO") || text.starts_with("RUST") || text.starts_with("TORQUE_") {
            command.env_remove(&name);
        }
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("the check {}: {e}", exe.display()))?;
    let reader = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut text = Vec::new();
            let _ = pipe.read_to_end(&mut text);
            String::from_utf8_lossy(&text).into_owned()
        })
    };
    let stdout = reader(Box::new(child.stdout.take().ok_or("no stdout")?));
    let stderr = reader(Box::new(child.stderr.take().ok_or("no stderr")?));
    let deadline = Instant::now() + Duration::from_secs(check.timeout_secs);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "the check ran past {} s and was stopped",
                check.timeout_secs
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let out = stdout.join().unwrap_or_default();
    let err = stderr.join().unwrap_or_default();
    if !status.success() {
        return Err(format!("the check failed ({status}):\n{out}{err}"));
    }
    let absent: Vec<&String> = check
        .expect
        .iter()
        .filter(|line| !out.contains(line.as_str()))
        .collect();
    if !absent.is_empty() {
        return Err(format!("the check did not print {absent:?}:\n{out}{err}"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_folders_are_named_by_os_and_arch() {
        assert_eq!(
            folder_name("demo", "x86_64-pc-windows-msvc"),
            "demo-windows-x86_64"
        );
        assert_eq!(
            folder_name("demo", "aarch64-apple-darwin"),
            "demo-macos-aarch64"
        );
        assert_eq!(
            folder_name("demo", "x86_64-unknown-linux-gnu"),
            "demo-linux-x86_64"
        );
    }
}
