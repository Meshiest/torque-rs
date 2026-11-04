//! A zip mounted as a folder (TGE's resource manager: a zip `<folder>/<Name>.zip` reads as the folder `<folder>/<Name>/`; its
//! entries are files to `isFile`, `getFileLength`, `FileObject` and `exec`, and the searches list them in reverse zip order).
//!
//! An archive comes from anyone, so the reader trusts nothing in it:
//! - the archive is at most [`MAX_ZIP_BYTES`], held in memory. Before the `zip` crate sees it, its bytes are
//!   scanned once for the end-of-directory signatures, because the crate trusts what it finds there:
//!   - it sizes its directory by the entry count of the zip64 end record it picks, checking it only against
//!     the file's length (a 64 MiB archive may claim 1.4 million entries and make it reserve 230 bytes for
//!     each): a zip64 end record claiming more than [`MAX_ENTRIES`] entries refuses the archive. Only a
//!     record the crate could accept counts (its size is at least 40 and fits in the rest of the file), so
//!     data that merely holds the four signature bytes does not refuse an honest archive;
//!   - when an end record does not lead to a directory it tries the next earlier one, and every try looks
//!     through the file again: a 64 MiB archive of 22-byte look-alike end records (a few million) would keep
//!     it busy for days (measured: 1000 look-alikes over 1 MiB take 3.7 s in a debug build, the cost is
//!     records times length). More than [`MAX_END_RECORDS`] end-record signatures of either kind refuses
//!     the archive (an ordinary archive has exactly one);
//! - at most [`MAX_ENTRIES`] entries are mounted (that is the most a zip32 or an accepted zip64 end record
//!   can hold), and only entries a host may name. Every `\` in an entry's name is turned into `/` and the
//!   other bytes are kept as they are (`bs\slash.txt` is served as `bs/slash.txt`, `hi<0xE9>name.txt` with its raw byte;
//!   [`served_name`]), and then [`entry_name_ok`] applies the path rules
//!   of `torque_ts_vm::host::sanitize_component` to every component: no `..`, `.`, empty component, `:`, NUL or
//!   control byte, Windows device name, trailing dot or space. The name is not required to be UTF-8: an
//!   entry is served from memory under its bytes, nothing joins it onto a folder, and the VM's paths are
//!   byte strings (`torque_ts_vm::host::safe_path`, which the folder host needs, is UTF-8 only and not used here);
//! - a directory record (a name ending in `/` that declares a stored entry of no bytes, as zip tools write a folder) is no
//!   resource: `isFile` 0, not listed ([`Skip::Directory`]);
//! - a folder-named entry that declares data (`e/` of five bytes, stored or deflated) is a file like any other, its name
//!   keeping the slash (`isFile` 1, its length, its CRC checked, read back);
//! - any other folder-named entry (`d/` with a deflate stream or an unknown method and no declared bytes) is a resource with
//!   no bytes, whatever its header declares (`isFile("<folder>/X/d/")` is 1, its length 0, `openForRead` 1 with `isEOF` 1,
//!   and the searches list it among the files): it is mounted as a 0-byte entry whose name keeps the slash
//!   ([`served_folder_name`]; [`ZipEntry::folder`]), and a read of it never touches the archive. Compression methods other
//!   than stored and deflate are left out. That includes every AES entry (method 99), which is dropped like any other method
//!   the reader cannot read (only that entry goes, the zip loads). The `zip` crate would refuse the whole archive for a
//!   method-99 entry without its AE-x
//!   extra field, so such a header's method is rewritten in this reader's own copy of the bytes to one the
//!   crate indexes but cannot read (see `scan`), and an entry the crate reports as AES-encrypted is skipped
//!   like it. The encryption flag is ignored: a flagged entry whose data is
//!   plain reads as that data; one whose data really is encrypted fails its CRC and reads as missing;
//! - an entry declaring more than [`MAX_ENTRY_BYTES`] uncompressed is left out; reading inflates at most
//!   the declared size plus one byte, and a different length or a CRC mismatch makes the entry unreadable
//!   (missing), so a lying header cannot make a read allocate more than the cap, whatever the compression
//!   ratio (a "zip bomb" inflates to at most 64 MiB per read; the VM charges a read that succeeds per byte);
//! - an entry that failed a read is remembered as unreadable and is not inflated again, so a lying entry
//!   costs one inflation, not one per script call (the VM can charge only for bytes it gets, and a failed
//!   read returns none); and all the failed reads of one mount together may inflate at most
//!   [`MAX_WASTED_INFLATE`] bytes, after which the mount reads nothing more (entries that share a deflate
//!   stream could otherwise each cost 64 MiB). [`ZipMount::inflated`] counts every byte inflated, failed
//!   reads included;
//! - two entries with one name (ignoring ASCII case, after the `\` conversion) mount as one: the later
//!   entry's data under the earlier one's spelling and place, listed once (identical names and names that differ in case
//!   alike); the `zip` crate merges identical raw names by itself (its directory is a map).
//!
//! Every entry left out is recorded with its reason ([`ZipMount::skipped`]); none of this prints anything.
//!
//! Memory. The `zip` crate keeps the whole directory for as long as the mount lives: names twice (raw and
//! decoded, up to three bytes a character for a CP437 name), comments twice, every extra field (a 5-byte
//! timestamp field becomes a 32-byte record), and about 230 bytes an entry. Measured with a counting
//! allocator, a hostile directory holds 6 times its own size in names (56 MiB -> 338 MiB) and 9 times in
//! timestamp fields (56 MiB -> 507 MiB), which this reader's own copies of the names add to. So the
//! archive's size bounds nothing about that: the directory is bounded instead, after the crate has read it
//! ([`MAX_DIRECTORY_BYTES`]); what stays resident for an accepted archive is the archive itself, at most
//! about 9 times [`MAX_DIRECTORY_BYTES`] for the crate's directory and 3 times for this reader's copies of
//! the names, and at most `MAX_ENTRIES` times 230 bytes of per-entry records. While the crate reads a
//! hostile directory it holds more than that, because it parses extra fields into records and copies them
//! as it goes: about 11 times the archive when the directory is packed with 5-byte timestamp fields
//! (measured: 30 MiB -> 342 MiB, 60 MiB -> 669 MiB peak), so up to about 710 MiB for an
//! archive near [`MAX_ZIP_BYTES`], before [`MAX_DIRECTORY_BYTES`] can refuse it. That is the crate's own
//! transient, freed when the mount is refused, and nothing in its API lets this reader stop it earlier.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Cursor, Read};

use torque_ts_vm::host::sanitize_component;
use torque_ts_vm::text::fold;
use zip::{CompressionMethod, HasZipMetadata, ZipArchive, ZipReadOptions};

/// The largest archive mounted (bytes). An ordinary mounted zip is a few megabytes at most.
pub const MAX_ZIP_BYTES: usize = 64 << 20;
/// The most entries mounted from one archive.
pub const MAX_ENTRIES: usize = 65_536;
/// The largest uncompressed entry (bytes): what one read may allocate. The VM's strings stop at 64 MiB too.
pub const MAX_ENTRY_BYTES: u64 = 64 << 20;
/// The most end-of-directory signatures (zip32 `PK\5\6` and plausible zip64 `PK\6\6` records counted apart)
/// an archive may hold. An honest archive has one of each kind at most; a few more allow a zip stored
/// inside another. Every extra one makes the `zip` crate look through the archive again.
pub const MAX_END_RECORDS: usize = 8;
/// The most bytes the central directory (and what follows it: the end records, the archive comment) may
/// span. An ordinary mounted zip's directory is a few tens of kilobytes; 65 536 entries with 100-byte names
/// need 9.6 MB. Names, comments and extra fields are held several times over (see the module docs), so
/// this bounds what a mounted archive keeps resident beyond its own bytes.
pub const MAX_DIRECTORY_BYTES: usize = 16 << 20;
/// The most bytes the failed reads of one mount may inflate in all; after that the mount counts as corrupt
/// and every read of it fails without inflating. A read that fails returns no bytes, so nothing is charged
/// for it by whoever charges per byte read, and a hostile archive of entries that share one deflate stream
/// (each declaring 64 MiB, each with a wrong CRC) could otherwise make a script burn hours of inflation at
/// a few steps a call. An honest archive never fails a read.
pub const MAX_WASTED_INFLATE: u64 = 256 << 20;
/// The method number a method-99 central header is rewritten to before the `zip` crate parses it: one the
/// crate keeps as "unsupported" and does not interpret.
const UNREADABLE_METHOD: u16 = 0xfffe;

/// Why an archive is not mounted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountError {
    /// Larger than [`MAX_ZIP_BYTES`].
    TooLarge(usize),
    /// A zip64 end record claims more than [`MAX_ENTRIES`] entries.
    TooManyEntries(u64),
    /// More than [`MAX_END_RECORDS`] end-of-directory signatures (the larger of the two counts).
    TooManyEndRecords(usize),
    /// The central directory spans more than [`MAX_DIRECTORY_BYTES`] (the span, in bytes).
    DirectoryTooLarge(usize),
    /// Not a zip archive the reader understands.
    NotAZip(String),
}

/// Why an entry is left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// A compression method other than stored and deflate, AES (method 99) included.
    Unsupported,
    /// A name [`served_name`] refuses.
    BadName,
    /// Declares more than [`MAX_ENTRY_BYTES`] uncompressed.
    TooLarge,
    /// Another entry of the same served name (ignoring ASCII case) came later and took its place: this
    /// entry's data is not reachable. Only names that differ as written (in case, or in `\` against `/`)
    /// are reported; the `zip` crate merges identical raw names before this reader sees them.
    Duplicate,
    /// The entry's local header is not where the directory says (the entry cannot be read at all).
    Unreadable,
    /// A directory record: a name ending in `/` (or `\`) that declares a stored entry of no bytes, as every zip
    /// tool writes a folder. It makes no resource (`isFile` 0, not listed).
    Directory,
}

/// One mounted entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipEntry {
    /// The name the entry is served under: as written in the archive, with `\` turned into `/` (see
    /// [`served_name`]). Bytes, not necessarily UTF-8.
    pub name: Vec<u8>,
    /// Its uncompressed size as the archive declares it.
    pub size: u64,
    /// A zip's folder entry (its name ends in `/`) that declares no bytes: a resource with none.
    pub folder: bool,
    index: usize,
}

/// An archive mounted as a folder.
pub struct ZipMount {
    /// Folded entry name -> position in `entries`.
    by_name: HashMap<Vec<u8>, usize>,
    /// In the archive's (central directory) order.
    entries: Vec<ZipEntry>,
    skipped: Vec<(Vec<u8>, Skip)>,
    /// Per entry: a read failed (it will not be inflated again).
    unreadable: Vec<Cell<bool>>,
    /// Bytes inflated by reads so far, failed ones included.
    inflated: Cell<u64>,
    /// Of those, the bytes of reads that failed.
    wasted: Cell<u64>,
    /// What the central directory spans (see [`MAX_DIRECTORY_BYTES`]).
    directory_bytes: usize,
    archive: RefCell<ZipArchive<Cursor<Vec<u8>>>>,
}

/// Whether a name (`/`-separated, the way an entry is served) is one a host may serve: every component
/// passes `torque_ts_vm::host::sanitize_component`. That refuses an empty name, a leading, a trailing and a
/// doubled `/` (they make an empty component), a `\` and a `:` anywhere, `.` and `..`, NUL and control
/// bytes, Windows device names, a trailing dot or space. The bytes need not be UTF-8.
pub fn entry_name_ok(name: &[u8]) -> bool {
    name.split(|&b| b == b'/').all(sanitize_component)
}

/// The name an archive's entry is served under, or None when a host may not serve it: every `\` in the name
/// turned into `/` and the other bytes kept; the result must pass [`entry_name_ok`], so `..\x`, `C:\x` and
/// `con\x` stay refused.
pub fn served_name(raw: &[u8]) -> Option<Vec<u8>> {
    let name = with_slashes(raw);
    entry_name_ok(&name).then_some(name)
}

/// The name a folder entry (`d/`, `d\`) is served under: [`served_name`]'s conversion, and the one trailing `/`
/// kept, because it is a resource with that name (`isFile("<folder>/X/d/")` is 1, `.../d` is 0). What is before the
/// slash must pass [`entry_name_ok`] like any name.
pub fn served_folder_name(raw: &[u8]) -> Option<Vec<u8>> {
    let name = with_slashes(raw);
    let body = name.strip_suffix(b"/")?;
    entry_name_ok(body).then_some(name)
}

fn with_slashes(raw: &[u8]) -> Vec<u8> {
    raw.iter()
        .map(|&b| if b == b'\\' { b'/' } else { b })
        .collect()
}

/// What the end-of-directory signatures in an archive's bytes claim.
#[derive(Debug, Default, PartialEq, Eq)]
struct EndRecords {
    /// `PK\5\6` signatures, wherever they are.
    zip32: usize,
    /// `PK\6\6` signatures that could be a zip64 end record the `zip` crate accepts.
    zip64: usize,
    /// The largest entry count any of those zip64 records claims.
    claim: u64,
}

/// The u16 at `offset` of the record at `at`.
fn half(bytes: &[u8], at: usize, offset: usize) -> Option<usize> {
    let start = at.checked_add(offset)?;
    let pair = bytes.get(start..start.checked_add(2)?)?;
    Some(usize::from(u16::from_le_bytes(pair.try_into().ok()?)))
}

/// A central-directory header at `at` that declares method 99 (AES) and is followed, where its name, extra
/// field and comment end, by another header or by an end record: the way a header in a directory is, and
/// the way bytes of stored data that merely look like one are not. Its method becomes
/// [`UNREADABLE_METHOD`]: the `zip` crate fails the whole directory for a method-99 header without the AE-x
/// extra field, where only that entry is to be dropped (as for every method the reader cannot read).
fn unreadable_method_99(bytes: &mut [u8], at: usize) {
    if half(bytes, at, 10) != Some(99) {
        return;
    }
    let lengths = (
        half(bytes, at, 28),
        half(bytes, at, 30),
        half(bytes, at, 32),
    );
    let (Some(name), Some(extra), Some(comment)) = lengths else {
        return;
    };
    let next = at
        .saturating_add(46)
        .saturating_add(name)
        .saturating_add(extra)
        .saturating_add(comment);
    let chained = matches!(
        bytes.get(next..next.saturating_add(4)),
        Some(b"PK\x01\x02" | b"PK\x05\x06" | b"PK\x06\x06" | b"PK\x05\x05")
    );
    if chained && let Some(method) = bytes.get_mut(at.saturating_add(10)..at.saturating_add(12)) {
        method.copy_from_slice(&UNREADABLE_METHOD.to_le_bytes());
    }
}

/// One pass over `bytes` before the `zip` crate sees them.
///
/// It counts the end-of-directory signatures, wherever they are, because the crate looks through the whole
/// file for them. A `PK\6\6` counts only if its record size is one the crate accepts (at least 40 and,
/// with its 12 bytes of header, within the rest of the file): data that merely holds those four bytes must
/// not refuse an honest archive.
///
/// And it repairs central-directory headers that declare method 99 ([`unreadable_method_99`]); that is
/// the only thing it changes.
fn scan(bytes: &mut [u8]) -> EndRecords {
    let mut found = EndRecords::default();
    let mut from = 0;
    while let Some(skip) = bytes
        .get(from..)
        .and_then(|rest| rest.iter().position(|&b| b == b'P'))
    {
        let at = from.saturating_add(skip);
        from = at.saturating_add(1);
        let mut central_header = false;
        match bytes.get(at..at.saturating_add(4)) {
            Some(b"PK\x01\x02") => central_header = true,
            Some(b"PK\x05\x06") => found.zip32 = found.zip32.saturating_add(1),
            Some(b"PK\x06\x06") => {
                // The u64 at `offset` of the record.
                let word = |offset: usize| -> Option<u64> {
                    let start = at.checked_add(offset)?;
                    let field = bytes.get(start..start.checked_add(8)?)?;
                    Some(u64::from_le_bytes(field.try_into().ok()?))
                };
                let rest = bytes.len().saturating_sub(at) as u64;
                // Record size at +4; entries in the whole directory at +32 (APPNOTE 4.3.14).
                if let (Some(size), Some(claim)) = (word(4), word(32))
                    && size >= 40
                    && size.saturating_add(12) <= rest
                {
                    found.zip64 = found.zip64.saturating_add(1);
                    found.claim = found.claim.max(claim);
                }
            }
            _ => {}
        }
        if central_header {
            unreadable_method_99(bytes, at);
        }
    }
    found
}

impl ZipMount {
    /// Reads the archive's directory (no entry is inflated).
    pub fn open(mut bytes: Vec<u8>) -> Result<ZipMount, MountError> {
        if bytes.len() > MAX_ZIP_BYTES {
            return Err(MountError::TooLarge(bytes.len()));
        }
        let ends = scan(&mut bytes);
        if ends.zip32.max(ends.zip64) > MAX_END_RECORDS {
            return Err(MountError::TooManyEndRecords(ends.zip32.max(ends.zip64)));
        }
        if ends.claim > MAX_ENTRIES as u64 {
            return Err(MountError::TooManyEntries(ends.claim));
        }
        let len = bytes.len();
        let mut archive =
            ZipArchive::new(Cursor::new(bytes)).map_err(|e| MountError::NotAZip(e.to_string()))?;
        let directory_start =
            usize::try_from(archive.central_directory_start()).unwrap_or(usize::MAX);
        let span = len.saturating_sub(directory_start);
        if span > MAX_DIRECTORY_BYTES {
            return Err(MountError::DirectoryTooLarge(span));
        }
        let mut entries: Vec<ZipEntry> = Vec::new();
        let mut by_name: HashMap<Vec<u8>, usize> = HashMap::new();
        // Per entry: where the header that supplies its data stands in the central directory, and the name
        // it is served under. Of entries that share a name (ignoring case) the later header wins.
        let mut winners: Vec<(u64, Vec<u8>)> = Vec::new();
        let mut skipped = Vec::new();
        for index in 0..archive.len() {
            let Ok(file) = archive.by_index_raw(index) else {
                let name = archive
                    .name_for_index(index)
                    .map_or_else(Vec::new, |n| n.as_bytes().to_vec());
                skipped.push((name, Skip::Unreadable));
                continue;
            };
            let written = file.name_raw().to_vec();
            // The crate's `is_dir` is a name ending in `/` or `\`. A folder-named entry that declares no bytes is a resource
            // with none whatever its method (a stored one is no resource at all), but one that declares data is a file like any
            // other with its slash in the name.
            let folder_name = file.is_dir();
            let (name, size, folder) = if folder_name && file.size() == 0 {
                let Some(name) = served_folder_name(&written) else {
                    skipped.push((written, Skip::BadName));
                    continue;
                };
                if file.compression() == CompressionMethod::Stored
                    && file.compressed_size() == 0
                    && file.size() == 0
                {
                    skipped.push((written, Skip::Directory));
                    continue;
                }
                (name, 0, true)
            } else {
                // An entry the crate reports as AES (its method is then the one inside the AE-x field, not 99)
                // is as unreadable as any other method.
                if file.get_metadata().aes_mode.is_some()
                    || !matches!(
                        file.compression(),
                        CompressionMethod::Stored | CompressionMethod::Deflated
                    )
                {
                    skipped.push((written, Skip::Unsupported));
                    continue;
                }
                let Some(name) = (if folder_name {
                    served_folder_name(&written)
                } else {
                    served_name(&written)
                }) else {
                    skipped.push((written, Skip::BadName));
                    continue;
                };
                if file.size() > MAX_ENTRY_BYTES {
                    skipped.push((written, Skip::TooLarge));
                    continue;
                }
                (name, file.size(), false)
            };
            let header = file.central_header_start();
            let key = fold(&name);
            if let Some(&slot) = by_name.get(&key)
                && let Some(earlier) = entries.get_mut(slot)
                && let Some(winner) = winners.get_mut(slot)
            {
                // One resource, at the first one's place and under its spelling; the data of the last of
                // them in the central directory (the `zip` crate has merged identical raw names at their
                // first place already, so the header's position says which came last). Both are
                // folders or both are files: a folder's served name ends in `/` and a file's never does.
                if header > winner.0 {
                    skipped.push((std::mem::replace(&mut winner.1, name), Skip::Duplicate));
                    winner.0 = header;
                    earlier.size = size;
                    earlier.index = index;
                } else {
                    skipped.push((name, Skip::Duplicate));
                }
                continue;
            }
            by_name.insert(key, entries.len());
            winners.push((header, name.clone()));
            entries.push(ZipEntry {
                name,
                size,
                folder,
                index,
            });
        }
        Ok(ZipMount {
            unreadable: vec![Cell::new(false); entries.len()],
            by_name,
            entries,
            skipped,
            inflated: Cell::new(0),
            wasted: Cell::new(0),
            directory_bytes: span,
            archive: RefCell::new(archive),
        })
    }

    /// The mounted entries, in the archive's order.
    pub fn entries(&self) -> &[ZipEntry] {
        &self.entries
    }

    /// The entries left out, with why.
    pub fn skipped(&self) -> &[(Vec<u8>, Skip)] {
        &self.skipped
    }

    /// The mounted entry called `name` (ignoring ASCII case).
    pub fn entry(&self, name: &[u8]) -> Option<&ZipEntry> {
        self.entries.get(*self.by_name.get(&fold(name))?)
    }

    /// The bytes the central directory spans (at most [`MAX_DIRECTORY_BYTES`]). The `zip` crate keeps the
    /// directory in memory for as long as the mount lives, up to about nine times this (module docs): what
    /// a host that budgets the memory of its mounts should add to the archive's own size.
    pub fn directory_bytes(&self) -> usize {
        self.directory_bytes
    }

    /// Every byte the reads so far inflated, the failed ones included (a failed read returns nothing, so
    /// this is the only way to see what it cost).
    pub fn inflated(&self) -> u64 {
        self.inflated.get()
    }

    /// Of [`inflated`](Self::inflated), the bytes of reads that failed: work that returned nothing, which a
    /// caller that charges for the bytes it gets (the VM) cannot see.
    pub fn wasted(&self) -> u64 {
        self.wasted.get()
    }

    /// The contents of the entry called `name`, inflated; None when there is none, or when it does not
    /// inflate to exactly its declared size with a matching CRC. An entry that failed once is not tried
    /// again.
    pub fn read(&self, name: &[u8]) -> Option<Vec<u8>> {
        let slot = *self.by_name.get(&fold(name))?;
        let entry = self.entries.get(slot)?;
        if entry.folder {
            return Some(Vec::new());
        }
        let unreadable = self.unreadable.get(slot)?;
        if unreadable.get() || self.wasted.get() >= MAX_WASTED_INFLATE {
            return None;
        }
        let mut archive = self.archive.try_borrow_mut().ok()?;
        let mut out = Vec::new();
        // The encryption flag is ignored; the crate refuses a flagged entry without a password unless told to
        // ignore the flag. An AES entry never gets this far: it is not mounted.
        let options = ZipReadOptions::new().ignore_encryption_flag(true);
        let ok = match archive.by_index_with_options(entry.index, options) {
            Ok(file) => {
                // Room for the declared size and the one byte that shows a lying header, so an honest
                // read grows nothing (a doubling vector would hold twice a 64 MiB entry), but no more
                // than the stream can ever produce (deflate expands at most 1032 to 1): a header that
                // lies about its size reserves little. An allocation that fails is a failed read.
                let room = entry.size.saturating_add(1).min(
                    file.compressed_size()
                        .saturating_mul(1032)
                        .saturating_add(1033),
                );
                out.try_reserve_exact(usize::try_from(room).unwrap_or(0))
                    .is_ok()
                    // One byte more than declared shows a lying header; the CRC is checked when the
                    // reader ends.
                    && file
                        .take(entry.size.saturating_add(1))
                        .read_to_end(&mut out)
                        .is_ok()
                    && out.len() as u64 == entry.size
            }
            Err(_) => false,
        };
        self.inflated
            .set(self.inflated.get().saturating_add(out.len() as u64));
        if ok {
            Some(out)
        } else {
            unreadable.set(true);
            self.wasted
                .set(self.wasted.get().saturating_add(out.len() as u64));
            None
        }
    }
}
