//! Temporary folders in the build directory, removed when the test is done.

use std::path::{Path, PathBuf};

/// A fresh, empty folder that is removed (with what the test put in it) when this value drops.
#[derive(Debug)]
pub struct TempFolder {
    path: PathBuf,
}

impl TempFolder {
    /// A fresh folder named after `name` and the process under `target_tmp`, the build directory's scratch space that
    /// integration tests and benches get from `CARGO_TARGET_TMPDIR`. Unit tests have none and pass `manifest_dir`, the
    /// crate's own, to fall back to the workspace's `target/tmp`. Prefer the [`temp_folder!`](crate::temp_folder) macro,
    /// which reads both at the call site.
    ///
    /// # Panics
    ///
    /// When the folder cannot be made.
    #[must_use]
    pub fn new(target_tmp: Option<&str>, manifest_dir: &str, name: &str) -> TempFolder {
        let base =
            target_tmp.map_or_else(|| workspace_target(manifest_dir).join("tmp"), PathBuf::from);
        let path = base.join(format!("{name}-{}", std::process::id()));
        // A leftover of a run that was killed.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("make the temporary folder");
        TempFolder { path }
    }

    /// The folder.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A path inside the folder.
    #[must_use]
    pub fn join(&self, child: impl AsRef<Path>) -> PathBuf {
        self.path.join(child)
    }
}

impl AsRef<Path> for TempFolder {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// The `target` folder of the workspace that holds `manifest_dir`: the first ancestor with a `Cargo.toml` that has a
/// `[workspace]` table, else the crate's own folder.
fn workspace_target(manifest_dir: &str) -> PathBuf {
    let start = Path::new(manifest_dir);
    let root = start
        .ancestors()
        .find(|dir| {
            std::fs::read_to_string(dir.join("Cargo.toml"))
                .is_ok_and(|text| text.lines().any(|line| line.trim() == "[workspace]"))
        })
        .unwrap_or(start);
    root.join("target")
}

/// A [`TempFolder`] named `$name`, in the build directory's scratch space when the calling test has one.
#[macro_export]
macro_rules! temp_folder {
    ($name:expr) => {
        $crate::TempFolder::new(
            ::core::option_env!("CARGO_TARGET_TMPDIR"),
            ::core::env!("CARGO_MANIFEST_DIR"),
            $name,
        )
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_folder_is_fresh_and_goes_away_on_drop() {
        let folder = crate::temp_folder!("folder-test");
        let path = folder.path().to_path_buf();
        assert_eq!(std::fs::read_dir(&path).expect("read").count(), 0);
        std::fs::write(folder.join("a.txt"), b"a").expect("write");
        assert!(path.join("a.txt").exists());
        drop(folder);
        assert!(!path.exists());
    }
}
