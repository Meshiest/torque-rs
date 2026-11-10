//! The license texts of everything a bundle links, gathered into `licenses/`.
//!
//! For each crate a build links ([`Build::linked`](crate::cargo::Build::linked)), its `cargo metadata` entry gives its folder,
//! SPDX expression, authors and repository; the folder's license, copying, notice and copyright files (and its
//! `license-file`) are copied to `licenses/<crate>-<version>/`. A local crate with no texts of its own takes those of its
//! nearest parent folder inside its workspace (a workspace's LICENSE and NOTICE), copied once under the repository's name. A
//! crate that ships no text gets the standard texts of its SPDX licenses with its authors (or its repository) as the
//! copyright holders. Rust's standard library is linked into every binary, so the toolchain's texts for it go in
//! `licenses/rust-std/`. `licenses/index.txt` lists every crate, its license, its repository and where its texts are; a
//! crate with no text at all is listed as such and counted in [`Gathered::missing`].
//!
//! Folders are read without following links, and every folder name under `licenses/` is a plain name.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::cargo::{Metadata, Package, Toolchain};

/// The bundle folder's licenses folder.
pub const DIR: &str = "licenses";

/// Name stems of license texts (compared without case).
const STEMS: &[&str] = &[
    "license",
    "licence",
    "copying",
    "notice",
    "copyright",
    "unlicense",
];

/// A license text: a file copied as it is, or one this tool wrote (a standard text with its holders filled in).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Text {
    File(PathBuf),
    Made(String),
}

/// What [`gather`] found.
#[derive(Clone, Debug, Default)]
pub struct Gathered {
    /// The texts: the path in the bundle and the text, sorted by path.
    pub files: Vec<(String, Text)>,
    /// `licenses/index.txt`.
    pub index: String,
    /// The crates listed.
    pub crates: usize,
    /// `name version` of each crate with no license text found.
    pub missing: Vec<String>,
    /// `name version` of each crate that ships no text, given the standard texts of its SPDX licenses.
    pub generic: Vec<String>,
}

type Group = BTreeMap<String, Text>;

/// The license texts of the `linked` crates of each workspace (its metadata and its linked list), and Rust's own.
pub fn gather(builds: &[(&Metadata, &[(String, String)])], toolchain: &Toolchain) -> Gathered {
    let roots: Vec<&Path> = builds
        .iter()
        .map(|(m, _)| m.workspace_root.as_path())
        .collect();
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut lines: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut gathered = Gathered::default();

    for (metadata, linked) in builds {
        for (name, version) in *linked {
            let key = (name.clone(), version.clone());
            if lines.contains_key(&key) {
                continue;
            }
            let package = metadata.package_version(name, version);
            let license = package
                .and_then(|p| p.license.clone())
                .unwrap_or_else(|| "(no SPDX expression)".to_string());
            let repository = package
                .and_then(|p| p.repository.clone())
                .filter(|r| !r.chars().any(char::is_control))
                .unwrap_or_else(|| "-".to_string());
            let found = package.and_then(|p| texts_of(p, &roots));
            let place = match (found, package) {
                (Some((group, files)), _) => {
                    let place = format!("{DIR}/{group}/");
                    groups.entry(group).or_default().extend(files);
                    place
                }
                (None, Some(package))
                    if !generic_texts(&license, &toolchain.sysroot).is_empty() =>
                {
                    let group = format!("{name}-{version}");
                    let texts = filled_texts(package, &toolchain.sysroot);
                    groups.entry(group.clone()).or_default().extend(texts);
                    gathered.generic.push(format!("{name} {version}"));
                    format!("{DIR}/{group}/ (standard texts: the crate ships none)")
                }
                _ => {
                    gathered.missing.push(format!("{name} {version}"));
                    "no license text in the crate".to_string()
                }
            };
            lines.insert(
                key,
                format!("{name} {version}\t{license}\t{repository}\t{place}"),
            );
        }
    }

    let doc = toolchain.sysroot.join("share").join("doc").join("rust");
    let mut std_files = Group::new();
    let copyright = doc.join("COPYRIGHT-library.html");
    if copyright.is_file() {
        std_files.insert("COPYRIGHT-library.html".to_string(), Text::File(copyright));
    }
    for (name, id) in [
        ("LICENSE-MIT.txt", "MIT"),
        ("LICENSE-APACHE.txt", "Apache-2.0"),
    ] {
        if let Ok(template) =
            std::fs::read_to_string(doc.join("licenses").join(format!("{id}.txt")))
        {
            std_files.insert(
                name.to_string(),
                Text::Made(fill(&template, "The Rust Project Developers")),
            );
        }
    }
    let std_version = toolchain
        .version
        .split_whitespace()
        .nth(1)
        .unwrap_or("?")
        .to_string();
    let std_place = if std_files.is_empty() {
        gathered.missing.push(format!("rust-std {std_version}"));
        "no license text in the toolchain".to_string()
    } else {
        groups.insert("rust-std".to_string(), std_files);
        format!("{DIR}/rust-std/")
    };
    lines.insert(
        ("rust-std".to_string(), std_version.clone()),
        format!(
            "rust-std {std_version}\tMIT OR Apache-2.0\thttps://github.com/rust-lang/rust\t{std_place}"
        ),
    );

    gathered.crates = lines.len();
    gathered.index = format!(
        "# Every crate the binaries and extensions of this bundle link, its license (SPDX), its repository and where its \
         texts are.\n# name version\tlicense\trepository\ttexts\n{}\n",
        lines.into_values().collect::<Vec<_>>().join("\n")
    );
    for (group, files) in groups {
        for (name, text) in files {
            gathered.files.push((format!("{DIR}/{group}/{name}"), text));
        }
    }
    gathered.files.sort();
    gathered
}

/// The group (folder name under `licenses/`) and the texts of `package`, or none when it has none. A local crate's search
/// for its workspace's texts stays inside one of the builds' workspace `roots`.
fn texts_of(package: &Package, roots: &[&Path]) -> Option<(String, Group)> {
    let mut files = texts_in(package.dir());
    if let Some(file) = &package.license_file
        && let Some(path) = license_file_in(package.dir(), Path::new(file))
        && let Some(name) = path.file_name().and_then(|n| n.to_str())
    {
        files.insert(name.to_string(), Text::File(path.clone()));
    }
    if !files.is_empty() {
        return Some((format!("{}-{}", package.name, package.version), files));
    }
    if package.remote {
        return None;
    }
    // A local crate: its workspace's texts, once for every crate of it.
    let mut dir = package.dir().parent();
    while let Some(here) = dir {
        if !roots.iter().any(|root| here.starts_with(root)) {
            return None;
        }
        let files = texts_in(here);
        if !files.is_empty() {
            return Some((repository_name(package), files));
        }
        dir = here.parent();
    }
    None
}

/// The standard texts of the licenses an SPDX `expression` names (`MIT OR Apache-2.0`), from the copies the Rust toolchain
/// ships (`share/doc/rust/licenses/<id>.txt`), by the name they are copied under (`LICENSE-<id>.txt`).
pub fn generic_texts(expression: &str, sysroot: &Path) -> BTreeMap<String, PathBuf> {
    let dir = sysroot
        .join("share")
        .join("doc")
        .join("rust")
        .join("licenses");
    expression
        .split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == '/')
        .filter(|id| !id.is_empty() && !matches!(*id, "OR" | "AND" | "WITH"))
        .filter(|id| {
            id.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '+'))
        })
        .filter_map(|id| {
            let path = dir.join(format!("{id}.txt"));
            path.is_file().then(|| (format!("LICENSE-{id}.txt"), path))
        })
        .collect()
}

/// The standard texts of `package`'s licenses with its copyright holders filled in, and a line saying where they came from.
fn filled_texts(package: &Package, sysroot: &Path) -> Group {
    let license = package.license.clone().unwrap_or_default();
    let holders = holders(package);
    generic_texts(&license, sysroot)
        .into_iter()
        .filter_map(|(name, path)| {
            let template = std::fs::read_to_string(path).ok()?;
            let note = format!(
                "The {} {} crate ships no license file; this is the standard text of a license its manifest names \
                 ({license}), with its authors as the copyright holders.\n\n",
                package.name, package.version
            );
            Some((name, Text::Made(note + &fill(&template, &holders))))
        })
        .collect()
}

/// `package`'s copyright holders: its authors without their e-mail addresses, else its contributors at its repository.
pub fn holders(package: &Package) -> String {
    let names: Vec<String> = package
        .authors
        .iter()
        .map(|a| a.split('<').next().unwrap_or(a).trim().to_string())
        .filter(|a| !a.is_empty() && !a.chars().any(char::is_control))
        .collect();
    if !names.is_empty() {
        return names.join(", ");
    }
    match &package.repository {
        Some(repository) => format!("The {} contributors ({repository})", package.name),
        None => format!("The {} contributors", package.name),
    }
}

/// A standard text with its `<year> <copyright holders>` placeholder (or `<copyright holders>` alone) replaced.
pub fn fill(template: &str, holders: &str) -> String {
    template
        .replace("<year> <copyright holders>", holders)
        .replace("<copyright holders>", holders)
}

/// The license texts directly in `dir`, and the files of a `LICENSES` folder in it, by the name they are copied under. Links
/// are skipped, never followed.
fn texts_in(dir: &Path) -> Group {
    let mut files = Group::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let Some(name) = entry.file_name().to_str().map(String::from) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        if kind.is_symlink() || !STEMS.iter().any(|stem| lower.starts_with(stem)) {
            continue;
        }
        if kind.is_file() {
            files.insert(name, Text::File(entry.path()));
        } else if kind.is_dir() && lower == "licenses" {
            for inner in std::fs::read_dir(entry.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                let plain = inner.file_type().is_ok_and(|k| k.is_file());
                if let Some(inner_name) = inner.file_name().to_str()
                    && plain
                {
                    files.insert(format!("{name}-{inner_name}"), Text::File(inner.path()));
                }
            }
        }
    }
    files
}

/// A plain folder name: letters, digits, `-`, `_`, `.` and `+`, not starting with `.` (so never `..`).
fn is_plain(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
}

/// A local crate's group: its repository's last path segment (`torque-rs`) when that is a plain name, else its own name.
fn repository_name(package: &Package) -> String {
    package
        .repository
        .as_deref()
        .and_then(|url| url.trim_end_matches('/').rsplit(['/', '\\']).next())
        .map(|name| name.trim_end_matches(".git"))
        .filter(|name| is_plain(name))
        .unwrap_or(&package.name)
        .to_string()
}

/// A crate's declared `license-file`, only when it is a plain file inside the crate's own folder: a relative path of
/// normal parts (no `..`, root or drive), no link anywhere along it, and still inside the folder once resolved, so a
/// manifest can't put a file from elsewhere on the machine into a bundle.
fn license_file_in(dir: &Path, file: &Path) -> Option<PathBuf> {
    use std::path::Component;
    if !file.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let mut at = dir.to_path_buf();
    for part in file.components() {
        at.push(part);
        if std::fs::symlink_metadata(&at)
            .ok()?
            .file_type()
            .is_symlink()
        {
            return None;
        }
    }
    let inside = at
        .canonicalize()
        .ok()?
        .starts_with(dir.canonicalize().ok()?);
    let plain = std::fs::symlink_metadata(&at).is_ok_and(|m| m.is_file());
    (inside && plain).then_some(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package() -> Package {
        Package {
            name: "torque-app".into(),
            version: "0.1.0".into(),
            license: Some("MIT".into()),
            license_file: None,
            repository: Some("https://example.com/someone/torque-rs.git".into()),
            authors: Vec::new(),
            manifest_path: PathBuf::from("/w/crates/torque-app/Cargo.toml"),
            remote: false,
            features: Default::default(),
        }
    }

    #[test]
    fn a_license_file_outside_the_crate_folder_is_refused() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("license-file-test");
        let _ = std::fs::remove_dir_all(&root);
        let krate = root.join("crate");
        std::fs::create_dir_all(krate.join("docs")).unwrap();
        std::fs::write(krate.join("docs").join("LICENSE"), "ok").unwrap();
        std::fs::write(root.join("secret.txt"), "not a license").unwrap();
        assert!(license_file_in(&krate, Path::new("docs/LICENSE")).is_some());
        for bad in ["../secret.txt", "docs/../../secret.txt", "missing", "docs"] {
            assert!(license_file_in(&krate, Path::new(bad)).is_none(), "{bad}");
        }
        let absolute = root.join("secret.txt");
        assert!(license_file_in(&krate, &absolute).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_local_crate_is_grouped_under_its_repository_when_it_is_a_plain_name() {
        assert_eq!(repository_name(&package()), "torque-rs");
        for bad in [
            None,
            Some("https://example.com/.."),
            Some("https://example.com/a\\..\\.."),
            Some("https://example.com/x y"),
            Some(""),
        ] {
            let p = Package {
                repository: bad.map(String::from),
                ..package()
            };
            assert_eq!(repository_name(&p), "torque-app", "{bad:?}");
        }
    }

    #[test]
    fn holders_are_the_authors_else_the_repository() {
        let with_authors = Package {
            authors: vec!["Ann Example <ann@example.com>".into(), "Bo".into()],
            ..package()
        };
        assert_eq!(holders(&with_authors), "Ann Example, Bo");
        assert_eq!(
            holders(&package()),
            "The torque-app contributors (https://example.com/someone/torque-rs.git)"
        );
        let mit = "MIT License\n\nCopyright (c) <year> <copyright holders>\n\nPermission...";
        assert_eq!(
            fill(mit, "Bo"),
            "MIT License\n\nCopyright (c) Bo\n\nPermission..."
        );
    }
}
