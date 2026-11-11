//! The kit cache's key and folder (`TORQUE_KIT_KEY`, `TORQUE_KIT_CACHE_DIR`, read by `src/cache.rs`).
//!
//! The key is a digest of everything the kit is made by: the profile, and the manifest, sources and build script of this
//! crate and of every path dependency it reaches (the shape, interior and model-layer builders, the maps, images and
//! resource crates under them, found by walking the manifests' `path = ".."` dependencies), the workspace manifest and the
//! lock file (the versions of the crates beneath, `png` among them). Changed code is a new key, so a kit kept under an old
//! one is never read as this code's. The folder is the build folder's `example-kit/`, found from `OUT_DIR`
//! (`<target>[/<triple>]/<profile>/build/<crate>/out`), and named only for a build with debug assertions: a shipped binary
//! carries no path of the machine that built it.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// 64-bit FNV-1a over tagged, length-prefixed items.
struct Digest(u64);

impl Digest {
    fn new() -> Digest {
        Digest(0xcbf2_9ce4_8422_2325)
    }

    fn put(&mut self, bytes: &[u8]) {
        for b in (bytes.len() as u64)
            .to_le_bytes()
            .iter()
            .chain(bytes.iter())
        {
            self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

fn read(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Folds `path` (a file, or a folder and everything under it, in name order) into `digest` as paths relative to `root` and
/// contents.
fn digest_path(digest: &mut Digest, root: &Path, path: &Path) {
    if path.is_dir() {
        let mut entries: Vec<PathBuf> = fs::read_dir(path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
            .map(|entry| entry.expect("a readable folder entry").path())
            .collect();
        entries.sort();
        for entry in entries {
            digest_path(digest, root, &entry);
        }
        return;
    }
    let relative = path.strip_prefix(root).unwrap_or(path);
    digest.put(relative.to_string_lossy().replace('\\', "/").as_bytes());
    digest.put(&read(path));
}

/// The `path = ".."` dependencies of the manifest at `manifest`, resolved, from every dependency table but the
/// `dev-dependencies` and `build-dependencies` (they make nothing of the kit).
fn path_dependencies(manifest: &Path) -> Vec<PathBuf> {
    let dir = manifest.parent().expect("a manifest has a folder");
    let text = String::from_utf8_lossy(&read(manifest)).into_owned();
    let mut in_dependencies = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_dependencies = line.ends_with("dependencies]")
                && !line.ends_with("dev-dependencies]")
                && !line.ends_with("build-dependencies]");
            continue;
        }
        if !in_dependencies {
            continue;
        }
        let Some(rest) = line.split_once("path = \"").map(|(_, rest)| rest) else {
            continue;
        };
        let Some((path, _)) = rest.split_once('"') else {
            continue;
        };
        out.push(dir.join(path));
    }
    out
}

/// This crate and every path dependency it reaches, as canonical crate folders.
fn crates_of(root: &Path) -> BTreeSet<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let dir = dir.canonicalize().unwrap_or(dir);
        if !seen.insert(dir.clone()) {
            continue;
        }
        pending.extend(path_dependencies(&dir.join("Cargo.toml")));
    }
    seen
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let workspace = manifest.join("../..");
    let mut digest = Digest::new();
    digest.put(env::var("PROFILE").expect("PROFILE").as_bytes());
    let mut inputs: Vec<PathBuf> = vec![workspace.join("Cargo.toml"), workspace.join("Cargo.lock")];
    for dir in crates_of(&manifest) {
        inputs.extend(
            ["Cargo.toml", "build.rs", "src"]
                .iter()
                .map(|name| dir.join(name))
                .filter(|path| path.exists()),
        );
    }
    for path in inputs {
        println!("cargo::rerun-if-changed={}", path.display());
        digest_path(&mut digest, &workspace, &path);
    }
    println!("cargo::rustc-env=TORQUE_KIT_KEY={:016x}", digest.0);

    // Only a build with debug assertions (dev, test) learns where its build folder is.
    if env::var_os("CARGO_CFG_DEBUG_ASSERTIONS").is_none() {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    // `out` -> `<crate>-<hash>` -> `build` -> `<profile>` -> the build folder.
    let target = out.ancestors().nth(4).unwrap_or(&out);
    println!(
        "cargo::rustc-env=TORQUE_KIT_CACHE_DIR={}",
        target.join("example-kit").display()
    );
}
