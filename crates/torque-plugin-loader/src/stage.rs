//! Getting a library ready to load without trusting the plugins folder twice: the file is read once, and those bytes are
//! hashed (what the player approves), searched for the exported ABI version (without loading or running anything), and
//! written to a private per-user cache as `<sha256>.<ext>`, made read-only and hashed again. The engine loads that copy
//! by its absolute path, so a file swapped in the plugins folder after it was read is never the one that runs.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};

use object::read::elf::{Dyn as _, ElfFile, FileHeader, ProgramHeader as _};
use object::read::macho::{MachHeader, MachOFile};
use object::{Endianness, Object, ObjectSection, ReadRef, SectionKind};
use sha2::{Digest, Sha256};

/// The largest library read (bytes); a larger one is refused.
pub const MAX_LIBRARY_BYTES: u64 = 512 * 1024 * 1024;

/// The cache's folder name inside the per-user folder.
pub(crate) const CACHE_FOLDER: &str = "cache";

/// A library's bytes, read once.
pub(crate) struct Read1 {
    pub(crate) bytes: Vec<u8>,
    pub(crate) sha256: String,
}

/// The SHA-256 of `bytes`, lower-case hex: how approvals name a file.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Reads the library at `path` once (at most [`MAX_LIBRARY_BYTES`]) and hashes what was read.
pub(crate) fn read_library(path: &Path) -> Result<Read1, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_LIBRARY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_LIBRARY_BYTES {
        return Err(format!(
            "{} is over {MAX_LIBRARY_BYTES} bytes",
            path.display()
        ));
    }
    let sha256 = sha256_hex(&bytes);
    Ok(Read1 { bytes, sha256 })
}

/// The ABI version a library exports as `torque_plugin_abi_version`, read from its file (PE, ELF or Mach-O) without
/// loading it: Err with the reason when the file is no library or exports no such constant.
pub fn exported_abi_version(bytes: &[u8]) -> Result<u32, String> {
    abi_version_at(bytes).map(|(_, version)| version)
}

/// Where in `bytes` the exported ABI version is (its offset), and its value: [`exported_abi_version`] with the place,
/// for tests that write another version into a copy of a library.
#[doc(hidden)]
pub fn abi_version_at(bytes: &[u8]) -> Result<(usize, u32), String> {
    let file = object::File::parse(bytes)
        .map_err(|_| "it is not a library this engine can read".to_string())?;
    let exports = file
        .exports()
        .map_err(|_| "its export table cannot be read".to_string())?;
    let export = exports
        .iter()
        .find(|e| {
            matches!(
                e.name(),
                b"torque_plugin_abi_version" | b"_torque_plugin_abi_version"
            )
        })
        .ok_or("it exports no torque_plugin_abi_version: it is not a native extension")?;
    let address = export.address();
    // Only a data section holding all four bytes: never code, or bytes the file does not hold. A linker that merges string
    // constants into `.rodata` (rust-lld) flags the whole section as strings, so that kind is read-only data too.
    let data = file
        .sections()
        .filter(|s| {
            matches!(
                s.kind(),
                SectionKind::Data
                    | SectionKind::ReadOnlyData
                    | SectionKind::ReadOnlyDataWithRel
                    | SectionKind::ReadOnlyString
            )
        })
        .filter(|s| {
            address >= s.address()
                && address
                    .checked_add(4)
                    .is_some_and(|end| end <= s.address().saturating_add(s.size()))
        })
        .find_map(|s| s.data_range(address, 4).ok().flatten())
        .filter(|d| d.len() == 4)
        .ok_or("its torque_plugin_abi_version is not a uint32_t in a data section of the file")?;
    let offset = (data.as_ptr() as usize).wrapping_sub(bytes.as_ptr() as usize);
    let word = [data[0], data[1], data[2], data[3]];
    let version = if file.is_little_endian() {
        u32::from_le_bytes(word)
    } else {
        u32::from_be_bytes(word)
    };
    Ok((offset, version))
}

/// Refuses a library that names folders of its own to find its dependencies in: an ELF `DT_RPATH` or `DT_RUNPATH` (read
/// from its dynamic segment, which the loader reads, and its dynamic section), or a Mach-O `LC_RPATH`. Such a folder
/// (`$ORIGIN` is the private cache, an absolute one may be a downloads folder) would let files nobody approved load with
/// it. A PE file has none: Windows' search is set when the library is loaded.
pub(crate) fn check_search_paths(bytes: &[u8]) -> Result<(), String> {
    let file = object::File::parse(bytes)
        .map_err(|_| "it is not a library this engine can read".to_string())?;
    let found = match &file {
        object::File::Elf32(elf) => elf_search_paths(elf),
        object::File::Elf64(elf) => elf_search_paths(elf),
        object::File::MachO32(macho) => macho_search_paths(macho),
        object::File::MachO64(macho) => macho_search_paths(macho),
        _ => Ok(false),
    };
    match found {
        Ok(false) => Ok(()),
        Ok(true) => Err(
            "it names folders of its own to load its dependencies from (an rpath or runpath): link it statically"
                .to_string(),
        ),
        Err(()) => Err("its dynamic linking table cannot be read".to_string()),
    }
}

/// Whether an ELF file has a `DT_RPATH` or `DT_RUNPATH`, in a dynamic segment or a dynamic section.
fn elf_search_paths<'data, Elf, R>(elf: &ElfFile<'data, Elf, R>) -> Result<bool, ()>
where
    Elf: FileHeader<Endian = Endianness>,
    R: ReadRef<'data>,
{
    let endian = elf.endian();
    let names_paths = |entries: &[Elf::Dyn]| {
        entries.iter().any(|entry| {
            matches!(
                entry.tag32(endian),
                Some(object::elf::DT_RPATH | object::elf::DT_RUNPATH)
            )
        })
    };
    for header in elf.elf_program_headers() {
        if let Some(entries) = header.dynamic(endian, elf.data()).map_err(|_| ())?
            && names_paths(entries)
        {
            return Ok(true);
        }
    }
    let sections = elf.elf_section_table();
    Ok(sections
        .dynamic(endian, elf.data())
        .map_err(|_| ())?
        .is_some_and(|(entries, _)| names_paths(entries)))
}

/// Whether a Mach-O file has an `LC_RPATH`.
fn macho_search_paths<'data, Mach, R>(macho: &MachOFile<'data, Mach, R>) -> Result<bool, ()>
where
    Mach: MachHeader<Endian = Endianness>,
    R: ReadRef<'data>,
{
    let mut commands = macho.macho_load_commands().map_err(|_| ())?;
    while let Some(command) = commands.next().map_err(|_| ())? {
        if command.cmd() == object::macho::LC_RPATH {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The platform's file extension of a library.
fn library_extension() -> &'static str {
    if cfg!(target_os = "windows") {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}

/// Clears a file's read-only flag and removes it.
fn remove(path: &Path) -> std::io::Result<()> {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut permissions = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        let _ = std::fs::set_permissions(path, permissions);
    }
    std::fs::remove_file(path)
}

/// The SHA-256 of the file at `path`, read whole.
fn sha256_of_file(path: &Path) -> std::io::Result<String> {
    Ok(sha256_hex(&std::fs::read(path)?))
}

/// Writes `read`'s bytes to `cache` as `<sha256>.<ext>` (kept when a copy with that hash is already there), read-only,
/// and checks the copy's hash again: the copy's absolute path.
pub(crate) fn cache_copy(read: &Read1, cache: &Path) -> Result<Held, String> {
    let fail = |what: &str, e: std::io::Error| format!("its private copy could not be {what}: {e}");
    make_private_dir(cache).map_err(|e| fail("made", e))?;
    let path = std::path::absolute(cache.join(format!("{}.{}", read.sha256, library_extension())))
        .map_err(|e| fail("named", e))?;
    let kept = path.is_file() && sha256_of_file(&path).is_ok_and(|h| h == read.sha256);
    if !kept {
        if path.exists() {
            remove(&path).map_err(|e| fail("replaced", e))?;
        }
        let part = path.with_extension(format!(
            "{}.part-{}",
            library_extension(),
            std::process::id()
        ));
        std::fs::write(&part, &read.bytes).map_err(|e| fail("written", e))?;
        let mut permissions = std::fs::metadata(&part)
            .map_err(|e| fail("written", e))?
            .permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&part, permissions).map_err(|e| fail("made read-only", e))?;
        if let Err(e) = std::fs::rename(&part, &path) {
            let _ = remove(&part);
            // Another process may have put the same copy there first.
            if !(path.is_file() && sha256_of_file(&path).is_ok_and(|h| h == read.sha256)) {
                return Err(fail("put in place", e));
            }
        }
    }
    let mut file = open_held(&path).map_err(|e| fail("opened", e))?;
    let mut bytes = Vec::new();
    // Through the held handle (not the path), at most what the original was.
    (&mut file)
        .take(MAX_LIBRARY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| fail("read back", e))?;
    if sha256_hex(&bytes) != read.sha256 {
        return Err("its private copy does not hash as the file approved".to_string());
    }
    Ok(Held { path, file })
}

/// Makes the folder `path` (and the folders above it that are missing), on Unix readable and writable by this user only.
pub(crate) fn make_private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// A private copy, checked through an open handle that is kept until the library has loaded: on Windows the handle
/// lets others only read the file, so nothing replaces it in between. On Linux the library is loaded through the held
/// handle itself ([`Held::open_path`]), so it is the file that was hashed; elsewhere on Unix [`Held::still_same`] checks,
/// right before and right after the library is opened, that the path still names the file the handle read. That second
/// check stops a swapped file's `torque_plugin_init`, not its load-time initializers, which the system runs while it
/// opens the file: there only the private folder's rule (`private::check`) keeps others from swapping it.
pub(crate) struct Held {
    pub(crate) path: PathBuf,
    file: std::fs::File,
}

impl Held {
    /// The path to open the library by: on Linux the held handle's own (`/proc/self/fd/<n>`, the file that was hashed
    /// whatever the cache's path names now), elsewhere the copy's path.
    pub(crate) fn open_path(&self) -> PathBuf {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            let fds = Path::new("/proc/self/fd");
            if fds.is_dir() {
                return fds.join(self.file.as_raw_fd().to_string());
            }
        }
        self.path.clone()
    }

    /// Whether the path still names the file that was hashed (on Windows the share mode already makes it so).
    pub(crate) fn still_same(&self) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let (Ok(held), Ok(named)) = (self.file.metadata(), std::fs::metadata(&self.path))
            else {
                return false;
            };
            held.dev() == named.dev() && held.ino() == named.ino()
        }
        #[cfg(not(unix))]
        {
            let _ = &self.file;
            true
        }
    }
}

/// Opens the copy for reading; on Windows, sharing it only with readers (no writer, no delete or rename).
fn open_held(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ.
        options.share_mode(0x1);
    }
    options.open(path)
}

/// Removes the copies in `cache` that are not `used` (hashes) and any half-written ones. A copy in use by another
/// process (a loaded library on Windows) stays.
pub(crate) fn clean_cache(cache: &Path, used: &HashSet<String>) {
    let Ok(read) = std::fs::read_dir(cache) else {
        return;
    };
    for entry in read.filter_map(Result::ok) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let hash = name.split('.').next().unwrap_or("");
        let ours = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
        if ours && (!used.contains(hash) || name.contains(".part-")) {
            let _ = remove(&path);
        }
    }
}
