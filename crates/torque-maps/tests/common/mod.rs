//! The folder of sample map files that `TORQUE_MAPS_DIR` names. None when it is unset or not a folder.

use std::path::{Path, PathBuf};

pub fn samples() -> Option<PathBuf> {
    let dir = std::env::var("TORQUE_MAPS_DIR").ok()?;
    Some(PathBuf::from(dir)).filter(|p| p.is_dir())
}

pub fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

pub fn ext(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}
