//! An extension's manifest, `plugins/<name>.toml`: its id and version (shown before any of its code runs) and its
//! library's name.

use std::io::Read;
use std::path::{Path, PathBuf};

use torque_engine::PluginId;
use torque_plugin_abi::TORQUE_MAX_NAME;

/// The longest manifest read (bytes).
pub const MAX_MANIFEST_BYTES: u64 = 16 * 1024;

/// An extension's manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// The id the library must report: a valid plugin id.
    pub id: String,
    /// The version the library must report.
    pub version: String,
    /// The library's name without the platform's prefix and suffix (`native_sample` for `native_sample.dll` or
    /// `libnative_sample.so`): the manifest's own name when it gives none.
    pub library: String,
}

impl Manifest {
    /// Reads the manifest at `path`.
    pub fn read(path: &Path) -> Result<Manifest, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut text = String::new();
        file.take(MAX_MANIFEST_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if text.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(format!(
                "{}: over {MAX_MANIFEST_BYTES} bytes",
                path.display()
            ));
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Manifest::parse(&text, &stem).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The manifest in `text`, named `stem`.
    pub fn parse(text: &str, stem: &str) -> Result<Manifest, String> {
        let table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| e.message().to_string())?;
        let field = |key: &str| -> Result<Option<String>, String> {
            match table.get(key) {
                None => Ok(None),
                Some(toml::Value::String(s)) => Ok(Some(s.clone())),
                Some(_) => Err(format!("`{key}` is not text")),
            }
        };
        if let Some(key) = table
            .keys()
            .find(|k| !matches!(k.as_str(), "id" | "version" | "library"))
        {
            return Err(format!(
                "unknown key `{key}` (a manifest has id, version and library)"
            ));
        }
        let id = field("id")?.ok_or("no `id`")?;
        if id.len() > TORQUE_MAX_NAME || !PluginId::is_valid(&id) {
            return Err(format!(
                "the id {id:?} is not a plugin id (ASCII letters, digits, '_', '-', '.' and ':')"
            ));
        }
        let version = field("version")?.ok_or("no `version`")?;
        if version.is_empty()
            || version.len() > TORQUE_MAX_NAME
            || !version.bytes().all(|b| b.is_ascii_graphic() && b != b'#')
        {
            // `#` starts a comment in the allowlist, which would record a shorter version than the one approved.
            return Err(format!(
                "the version {version:?} is not printable ASCII without spaces or '#'"
            ));
        }
        let library = field("library")?.unwrap_or_else(|| stem.to_string());
        let plain = !library.is_empty()
            && library.len() <= TORQUE_MAX_NAME
            && library
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
            && !library.starts_with('.');
        if !plain {
            return Err(format!(
                "the library {library:?} is not a plain name (ASCII letters, digits, '_', '-' and '.', no folders)"
            ));
        }
        Ok(Manifest {
            id,
            version,
            library,
        })
    }

    /// The library's path in `folder` (`native_sample.dll`, `libnative_sample.so`, `libnative_sample.dylib`): Err when
    /// it is not a file there.
    pub fn library_path(&self, folder: &Path) -> Result<PathBuf, String> {
        let path = folder.join(library_file_name(&self.library));
        let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !meta.is_file() {
            return Err(format!("{} is not a file", path.display()));
        }
        Ok(path)
    }
}

/// The platform's file name of the library `name`.
pub(crate) fn library_file_name(name: &str) -> String {
    let (prefix, suffix) = if cfg!(target_os = "windows") {
        ("", ".dll")
    } else if cfg!(target_os = "macos") {
        ("lib", ".dylib")
    } else {
        ("lib", ".so")
    };
    format!("{prefix}{name}{suffix}")
}
