//! Where a server stages the uploads that do not fit in memory: a folder of its own under the user's data folder, with a cap
//! on what it may hold, made for a server of an engine that registered an upload kind ([`torque_engine::upload`]).
//!
//! Each run of a server gets its own folder (`<data>/torque-rs/uploads/.staging/<process>-<time>`) and holds a lock on a file in it
//! (`run.lock`) for as long as the server lives. So two servers on one machine never empty each other's files, and a starting
//! server removes the folders of runs whose lock nobody holds any more (the run ended, however it ended), and never one that is
//! alive, whatever its age. A game that sets its own staging (`NetRegistry::upload_staging`) is left to it. On the web there is no
//! folder and only uploads that fit in memory are taken.

use std::fs::{self, File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use torque_engine::upload::StagingConfig;

/// The most bytes a server stages on disk for uploads (512 MiB): the folder's cap.
pub const STAGING_DISK_CAP: u64 = 512 << 20;

/// The file in a run's folder whose lock says the run is alive.
pub const LOCK_FILE: &str = "run.lock";

/// The user's data folder: `%LOCALAPPDATA%`, `~/Library/Application Support`, `$XDG_DATA_HOME` or `~/.local/share`. `None` when the
/// system gives none (and on the web).
pub fn data_dir() -> Option<PathBuf> {
    if cfg!(target_family = "wasm") {
        return None;
    }
    let absolute = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    if cfg!(windows) {
        absolute("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        absolute("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        absolute("XDG_DATA_HOME")
            .or_else(|| absolute("HOME").map(|h| h.join(".local").join("share")))
    }
}

/// The folder all runs' staging folders are in, under `data`.
pub fn staging_parent(data: &Path) -> PathBuf {
    data.join("torque-rs").join("uploads").join(".staging")
}

/// The staging for a server run whose folder is `folder` (`None`: memory only), with the disk cap.
pub fn staging_config(folder: Option<PathBuf>) -> StagingConfig {
    StagingConfig {
        disk_total: STAGING_DISK_CAP,
        folder,
        ..StagingConfig::default()
    }
}

/// A name for this run: the process and the time.
pub fn run_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{}-{nanos:x}", std::process::id())
}

/// A run's staging folder, claimed: the lock on its `run.lock` is held for as long as this is.
pub struct RunFolder {
    path: PathBuf,
    _lock: File,
}

impl RunFolder {
    /// The folder.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Makes the folder `parent/run` and locks its `run.lock`.
///
/// # Errors
/// The file system's, or the lock's (another process holds it).
pub fn claim_run_folder(parent: &Path, run: &str) -> io::Result<RunFolder> {
    let path = parent.join(run);
    fs::create_dir_all(&path)?;
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.join(LOCK_FILE))?;
    lock.try_lock().map_err(|e| match e {
        TryLockError::Error(e) => e,
        TryLockError::WouldBlock => {
            io::Error::new(io::ErrorKind::WouldBlock, "the folder is in use")
        }
    })?;
    Ok(RunFolder { path, _lock: lock })
}

/// Removes the run folders in `parent` whose lock nobody holds: the runs that ended. A folder without a lock file is not a run's
/// and is left alone. Best effort.
pub fn remove_dead_runs(parent: &Path) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(lock) = File::options().write(true).open(path.join(LOCK_FILE)) else {
            continue;
        };
        if lock.try_lock().is_ok() {
            // Nobody holds it. The file is closed before the folder goes, which some systems insist on.
            drop(lock);
            let _ = fs::remove_dir_all(&path);
        }
    }
}

#[cfg(test)]
#[path = "uploads_tests.rs"]
mod tests;
