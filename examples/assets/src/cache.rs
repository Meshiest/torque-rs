//! The kit kept under the build folder, so a process reads it instead of making it: `<target>/example-kit/<key>/kit.bin`,
//! one file of every asset's name and bytes behind a checksum. The key is the digest the build script takes of the code the
//! kit is made by and the profile (`TORQUE_KIT_KEY`), so changed code reads nothing stale: it makes its kit and keeps it
//! under its own key, and kits of other keys a day old or more are removed. A writer writes a file of its own and renames it
//! into place, so a reader sees a whole kit or none; a file that does not decode or check is made again. Only a build with
//! debug assertions knows its build folder (a shipped binary carries no path of the machine that built it); [`ENV`] names
//! another folder for any build, or turns the cache off.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::Asset;

/// The environment variable that names another folder for the kit, or turns the cache off (`0` or empty).
pub const ENV: &str = "TORQUE_KIT_CACHE";

/// The file's head: its format, bumped when the layout changes.
const MAGIC: &[u8; 8] = b"TQKIT\0\0\x02";

/// The file each kit is kept as, under its key's folder.
const FILE: &str = "kit.bin";

/// How old a kit of another key must be before it is removed: a build still running on the old code keeps its kit.
const KEEP_OTHERS: Duration = Duration::from_secs(24 * 60 * 60);

/// Where the kit is kept.
enum Folder {
    /// The build folder's `example-kit/`: ours to tidy.
    Build(PathBuf),
    /// A folder [`ENV`] named: nothing in it but our own file is touched.
    Named(PathBuf),
}

impl Folder {
    fn of_env() -> Option<Folder> {
        if cfg!(target_arch = "wasm32") {
            return None;
        }
        match std::env::var_os(ENV) {
            Some(value) if value.is_empty() || value == "0" => None,
            Some(value) => Some(Folder::Named(PathBuf::from(value))),
            None => {
                option_env!("TORQUE_KIT_CACHE_DIR").map(|dir| Folder::Build(PathBuf::from(dir)))
            }
        }
    }

    fn path(&self) -> &Path {
        match self {
            Folder::Build(path) | Folder::Named(path) => path,
        }
    }
}

/// The file the kit of this code is kept in, or none with the cache off (a release build without [`ENV`], the web).
pub fn file() -> Option<PathBuf> {
    Some(
        Folder::of_env()?
            .path()
            .join(env!("TORQUE_KIT_KEY"))
            .join(FILE),
    )
}

/// The kit from its file, else made and kept there (a kit that cannot be kept, say in a read-only build folder, is made
/// again next time).
pub(crate) fn read_or_make() -> io::Result<Vec<Asset>> {
    let Some(folder) = Folder::of_env() else {
        return crate::make();
    };
    let file = folder.path().join(env!("TORQUE_KIT_KEY")).join(FILE);
    if let Some(kit) = fs::read(&file).ok().and_then(|bytes| decode(&bytes)) {
        return Ok(kit);
    }
    let kit = crate::make()?;
    if store(&file, &encode(&kit)).is_ok()
        && let Folder::Build(path) = &folder
    {
        prune(path, &file);
    }
    Ok(kit)
}

/// Keeps `bytes` as `file`: written beside it under a name of this process's and this write's own, then renamed into place.
/// Another writer getting there first is no error: its kit is this one.
fn store(file: &Path, bytes: &[u8]) -> io::Result<()> {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let dir = file
        .parent()
        .ok_or_else(|| io::Error::other("a kit file with no folder"))?;
    fs::create_dir_all(dir)?;
    let write = WRITES.fetch_add(1, Ordering::Relaxed);
    let own = dir.join(format!("kit.{}.{write}.tmp", std::process::id()));
    fs::write(&own, bytes)?;
    if let Err(error) = fs::rename(&own, file) {
        let _ = fs::remove_file(&own);
        if !file.is_file() {
            return Err(error);
        }
    }
    Ok(())
}

/// Removes the kits of other code under `folder` (ours, the build folder's): the folders named by a key that hold a kit file
/// [`KEEP_OTHERS`] old or more, other than `keep`'s. Anything else there is left alone.
fn prune(folder: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_key = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.len() == 16 && name.bytes().all(|b| b.is_ascii_hexdigit()));
        let kit = path.join(FILE);
        if !is_key || kit == keep {
            continue;
        }
        let old = fs::metadata(&kit)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| now.duration_since(at).ok())
            .is_some_and(|age| age >= KEEP_OTHERS);
        if old {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

/// The kit as one file: [`MAGIC`], the payload's checksum, then the payload: the count, then each asset's name and bytes,
/// every length a little-endian `u64`.
fn encode(kit: &[Asset]) -> Vec<u8> {
    let mut payload = Vec::new();
    put_len(&mut payload, kit.len());
    for asset in kit {
        put_len(&mut payload, asset.name.len());
        payload.extend_from_slice(asset.name.as_bytes());
        put_len(&mut payload, asset.bytes.len());
        payload.extend_from_slice(&asset.bytes);
    }
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&checksum(&payload).to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

/// The kit of [`encode`]'s bytes; none for anything else (another format, a checksum that does not match, a cut file, bytes
/// left over).
fn decode(bytes: &[u8]) -> Option<Vec<Asset>> {
    let mut rest = bytes.strip_prefix(MAGIC)?;
    let (sum, payload) = rest.split_first_chunk::<8>()?;
    if u64::from_le_bytes(*sum) != checksum(payload) {
        return None;
    }
    rest = payload;
    let count = take_len(&mut rest)?;
    let mut kit = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let name = String::from_utf8(take(&mut rest)?.to_vec()).ok()?;
        let bytes = take(&mut rest)?.to_vec();
        kit.push(Asset { name, bytes });
    }
    rest.is_empty().then_some(kit)
}

/// 64-bit FNV-1a of `bytes`.
fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn put_len(out: &mut Vec<u8>, len: usize) {
    out.extend_from_slice(&(len as u64).to_le_bytes());
}

fn take_len(rest: &mut &[u8]) -> Option<usize> {
    let (head, tail) = rest.split_first_chunk::<8>()?;
    *rest = tail;
    usize::try_from(u64::from_le_bytes(*head)).ok()
}

fn take<'a>(rest: &mut &'a [u8]) -> Option<&'a [u8]> {
    let len = take_len(rest)?;
    let (head, tail) = rest.split_at_checked(len)?;
    *rest = tail;
    Some(head)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kit() -> Vec<Asset> {
        vec![
            Asset {
                name: "a.dts".into(),
                bytes: vec![1, 2, 3],
            },
            Asset {
                name: "b.png".into(),
                bytes: Vec::new(),
            },
        ]
    }

    /// A folder of this test's own under the build folder's cache.
    fn folder(test: &str) -> PathBuf {
        // A test build has debug assertions, so the build script named the folder.
        let dir = PathBuf::from(env!("TORQUE_KIT_CACHE_DIR"))
            .join("tests")
            .join(format!("{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_kit_reads_back_and_anything_else_is_no_kit() {
        let bytes = encode(&kit());
        assert_eq!(decode(&bytes), Some(kit()));
        assert_eq!(decode(&bytes[..bytes.len() - 1]), None, "cut short");
        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(decode(&longer), None, "a byte left over");
        let mut other = bytes.clone();
        other[7] += 1;
        assert_eq!(decode(&other), None, "another format");
        let mut flipped = bytes.clone();
        *flipped.last_mut().unwrap() ^= 1;
        assert_eq!(
            decode(&flipped),
            None,
            "a byte of the payload changed: the checksum says so"
        );
        assert_eq!(decode(b""), None);
        assert_eq!(decode(&encode(&[])), Some(Vec::new()));
    }

    #[test]
    fn a_kit_is_kept_whole_over_an_old_one_and_only_old_kits_of_other_code_go() {
        let folder = folder("kept");
        let file = folder.join("0123456789abcdef").join(FILE);
        let (old, fresh, no_kit) = (
            folder.join("fedcba9876543210"),
            folder.join("0000000000000001"),
            folder.join("0000000000000002"),
        );
        for dir in [&old, &fresh, &no_kit] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(old.join(FILE), b"old").unwrap();
        fs::write(fresh.join(FILE), b"fresh").unwrap();
        fs::write(no_kit.join("other.txt"), b"not a kit").unwrap();
        let two_days_ago = std::time::SystemTime::now() - 2 * KEEP_OTHERS;
        fs::OpenOptions::new()
            .write(true)
            .open(old.join(FILE))
            .unwrap()
            .set_modified(two_days_ago)
            .unwrap();
        store(&file, &encode(&[])).unwrap();
        store(&file, &encode(&kit())).unwrap();
        prune(&folder, &file);
        assert_eq!(decode(&fs::read(&file).unwrap()), Some(kit()));
        assert!(!old.exists(), "the other key's old kit is gone");
        assert!(fresh.join(FILE).is_file(), "a kit of today stays");
        assert!(
            no_kit.join("other.txt").is_file(),
            "a folder with no kit is not touched"
        );
        assert_eq!(
            fs::read_dir(file.parent().unwrap()).unwrap().count(),
            1,
            "no file of the writer's own is left"
        );
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn writers_at_once_leave_one_whole_kit() {
        let folder = folder("race");
        let file = folder.join("0123456789abcdef").join(FILE);
        let bytes = encode(&kit());
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..20 {
                        store(&file, &bytes).unwrap();
                        assert_eq!(decode(&fs::read(&file).unwrap()), Some(kit()));
                    }
                });
            }
        });
        let _ = fs::remove_dir_all(&folder);
    }
}
