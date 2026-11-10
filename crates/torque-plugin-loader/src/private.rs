//! Whether the per-user folder is this user's alone. On Unix another local user who can write it, or any folder above it
//! (an `XDG_CACHE_HOME` pointing at a shared folder, or a `torque-rs/native-extensions` someone else made first), could
//! write approvals or swap the loaded copies. So nothing is loaded unless:
//!
//! - the folder, its cache and its allowlist belong to this process's effective user and nobody else can write them, and
//!   none of them is a link;
//! - every folder above it, up to the root (its real path, links resolved), belongs to this user or to root, and nobody
//!   else can write it, unless it is sticky (`/tmp`), where nobody can rename or remove another user's entries.
//!
//! On Windows the folder is under `%LOCALAPPDATA%`, which other users cannot read.

use std::path::Path;

/// The sticky bit: in a folder with it, only an entry's owner (or root) may rename or remove the entry.
const STICKY: u32 = 0o1000;

/// What a file or folder must be to hold approvals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    all(not(unix), not(test)),
    expect(dead_code, reason = "the rule is Unix's; its tests run everywhere")
)]
pub(crate) enum Role {
    /// The per-user folder, its cache or its allowlist: this user's, and nobody else may write it.
    Own,
    /// A folder above it: this user's or root's, and nobody else may write it unless it is sticky.
    Above,
}

/// Why a file or folder `what` with this `owner` and `mode` (Unix's) cannot hold approvals for the user `me` in its
/// `role`. `None` when it can.
#[cfg_attr(
    all(not(unix), not(test)),
    expect(dead_code, reason = "the rule is Unix's; its tests run everywhere")
)]
pub(crate) fn not_private(
    what: &Path,
    owner: u32,
    mode: u32,
    me: u32,
    role: Role,
) -> Option<String> {
    let owner_ok = owner == me || (role == Role::Above && owner == 0);
    if !owner_ok {
        return Some(format!(
            "{} is owned by another user (uid {owner}, not {me})",
            what.display()
        ));
    }
    let sticky = role == Role::Above && mode & STICKY != 0;
    if mode & 0o022 != 0 && !sticky {
        return Some(format!(
            "{} can be written by other users (mode {:o}): make it this user's alone (chmod go-w)",
            what.display(),
            mode & 0o7777
        ));
    }
    None
}

/// Err with the reason when the per-user folder `user` is not this user's alone ([`not_private`]).
#[cfg(unix)]
pub(crate) fn check(user: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let me = rustix::process::geteuid().as_raw();
    let cache = user.join(crate::stage::CACHE_FOLDER);
    let allowlist = user.join(crate::ALLOWLIST_FILE);
    for path in [user, cache.as_path(), allowlist.as_path()] {
        let meta = match std::fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && path != user => continue,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        if meta.file_type().is_symlink() {
            return Err(format!("{} is a link", path.display()));
        }
        if let Some(reason) = not_private(path, meta.uid(), meta.mode(), me, Role::Own) {
            return Err(reason);
        }
    }
    // The folders above it, by its real path: a link among them is resolved, and each folder it leads through is checked.
    let real = std::fs::canonicalize(user).map_err(|e| format!("{}: {e}", user.display()))?;
    for above in real.ancestors().skip(1) {
        let meta =
            std::fs::symlink_metadata(above).map_err(|e| format!("{}: {e}", above.display()))?;
        if let Some(reason) = not_private(above, meta.uid(), meta.mode(), me, Role::Above) {
            return Err(reason);
        }
    }
    Ok(())
}

/// On Windows (and in a browser build) the per-user folder needs no check of its own.
#[cfg(not(unix))]
pub(crate) fn check(_user: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_another_user_owns_or_can_write_is_not_private() {
        let at = Path::new("/home/me/.cache/torque-rs");
        let own = |owner, mode| not_private(at, owner, mode, 1000, Role::Own);
        let above = |owner, mode| not_private(at, owner, mode, 1000, Role::Above);
        assert_eq!(own(1000, 0o40700), None);
        assert_eq!(own(1000, 0o100600), None);
        assert_eq!(own(1000, 0o40755), None);
        assert!(own(1001, 0o40700).is_some_and(|r| r.contains("another user")));
        // Root may own a folder above it, never the folder itself.
        assert!(own(0, 0o40700).is_some());
        assert_eq!(above(0, 0o40755), None);
        assert!(above(1001, 0o40755).is_some_and(|r| r.contains("another user")));
        for mode in [0o40777, 0o40770, 0o40702, 0o100620] {
            assert!(
                own(1000, mode).is_some_and(|r| r.contains("written by other")),
                "{mode:o}"
            );
            assert!(above(0, mode).is_some(), "{mode:o} above");
        }
        // A sticky folder others can write (`/tmp`) may be above it, never be it.
        assert_eq!(above(0, 0o41777), None);
        assert!(own(1000, 0o41777).is_some());
    }
}
