//! VPK archives: a directory file (`<name>_dir.vpk`) that lists every entry by extension, folder and name, and numbered data
//! files beside it (`<name>_000.vpk`, `<name>_001.vpk`, ...) that hold the entries' bytes.
//!
//! **Layout** (versions 1 and 2, little-endian): the signature `0x55aa1234`, the version, the tree's size, and for version 2 four
//! more sizes (the data held in the directory file, the archive and other checksums, the signature). The tree is three nested
//! lists of NUL-terminated strings, each ended by an empty one: extensions, then folders of each extension, then names in each
//! folder; after each name an entry: a CRC-32, the count of preload bytes (stored right after the entry), the data file's index
//! (`0x7fff`: the directory file itself, after the tree), the offset and length there, and the terminator `0xffff`. A single
//! space stands for an empty extension or folder.
//!
//! [`VpkDir::parse`] reads the directory's bytes (hostile bytes give an error); [`VpkArchive`] opens a directory file on disk
//! and reads entries from its data files.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::read::{
    Cursor, Error, Result, cap, invalid, normalize_path, open_plain, read_plain_file,
};

/// The directory file's signature.
pub const SIGNATURE: u32 = 0x55aa_1234;
/// The data-file index of an entry stored in the directory file itself.
pub const IN_DIRECTORY: u16 = 0x7fff;
/// The most entries one directory may list.
pub const MAX_ENTRIES: usize = 1 << 20;
/// The largest directory file read.
pub const MAX_DIR_BYTES: u64 = 256 << 20;
/// The largest single entry read.
pub const MAX_ENTRY_BYTES: u64 = 512 << 20;
/// The most bytes of paths one directory makes (each entry's folder, name and extension joined).
pub const MAX_PATH_BYTES: usize = 64 << 20;
/// The longest extension, folder or name.
const MAX_NAME: usize = 1024;

/// One entry of the directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VpkEntry {
    pub crc: u32,
    /// The data file's index, or [`IN_DIRECTORY`].
    pub archive: u16,
    pub offset: u32,
    pub len: u32,
    /// Where its preload bytes are in the directory's bytes.
    preload_at: usize,
    preload_len: u16,
}

impl VpkEntry {
    /// The entry's whole length (its preload bytes and its data).
    pub fn total_len(&self) -> u64 {
        u64::from(self.preload_len) + u64::from(self.len)
    }
}

/// A parsed directory: its entries by normalized path ([`normalize_path`]).
#[derive(Debug, Clone)]
pub struct VpkDir {
    pub version: u32,
    bytes: Vec<u8>,
    /// Where the data stored in the directory file begins (after the header and the tree).
    data_start: usize,
    entries: HashMap<String, VpkEntry>,
}

impl VpkDir {
    /// Reads a directory file's bytes.
    pub fn parse(bytes: Vec<u8>) -> Result<VpkDir> {
        Self::parse_capped(bytes, MAX_PATH_BYTES)
    }

    /// [`VpkDir::parse`] with `path_cap` bytes of joined paths at most.
    fn parse_capped(bytes: Vec<u8>, path_cap: usize) -> Result<VpkDir> {
        cap(
            "the VPK directory's size",
            bytes.len() as u64,
            MAX_DIR_BYTES,
        )?;
        let mut c = Cursor::new(&bytes);
        if c.u32("the VPK signature")? != SIGNATURE {
            return invalid("not a VPK directory (bad signature)");
        }
        let version = c.u32("the VPK version")?;
        let tree_size = c.u32("the VPK tree size")? as usize;
        match version {
            1 => {}
            2 => {
                c.bytes(16, "the VPK 2 header")?;
            }
            v => return invalid(format!("VPK version {v} is not read")),
        }
        let header = c.pos;
        let data_start = header
            .checked_add(tree_size)
            .filter(|&e| e <= bytes.len())
            .ok_or(Error::Truncated {
                what: "the VPK tree",
            })?;
        let mut c = Cursor::new(&bytes[..data_start]);
        c.pos = header;
        let mut entries = HashMap::new();
        // A long folder is stored once but joined to every name in it, so the joined paths are paid for.
        let mut path_bytes = 0usize;
        loop {
            let ext = c.cstr(MAX_NAME, "a VPK extension")?;
            if ext.is_empty() {
                break;
            }
            loop {
                let folder = c.cstr(MAX_NAME, "a VPK folder")?;
                if folder.is_empty() {
                    break;
                }
                loop {
                    let name = c.cstr(MAX_NAME, "a VPK name")?;
                    if name.is_empty() {
                        break;
                    }
                    let crc = c.u32("a VPK entry")?;
                    let preload_len = c.u16("a VPK entry")?;
                    let archive = c.u16("a VPK entry")?;
                    let offset = c.u32("a VPK entry")?;
                    let len = c.u32("a VPK entry")?;
                    if c.u16("a VPK entry")? != 0xffff {
                        return invalid("a VPK entry's terminator is not 0xffff");
                    }
                    let preload_at = c.pos;
                    c.bytes(usize::from(preload_len), "a VPK entry's preload bytes")?;
                    if entries.len() >= MAX_ENTRIES {
                        return Err(Error::TooLarge {
                            what: "the VPK's entries",
                            asked: entries.len() as u64 + 1,
                            cap: MAX_ENTRIES as u64,
                        });
                    }
                    path_bytes += folder.len() + name.len() + ext.len() + 2;
                    cap("the VPK's paths", path_bytes as u64, path_cap as u64)?;
                    let mut path = String::new();
                    if folder != b" " {
                        path.push_str(&String::from_utf8_lossy(folder));
                        path.push('/');
                    }
                    path.push_str(&String::from_utf8_lossy(name));
                    if ext != b" " {
                        path.push('.');
                        path.push_str(&String::from_utf8_lossy(ext));
                    }
                    if let Some(p) = normalize_path(&path) {
                        entries.insert(
                            p,
                            VpkEntry {
                                crc,
                                archive,
                                offset,
                                len,
                                preload_at,
                                preload_len,
                            },
                        );
                    }
                }
            }
        }
        Ok(VpkDir {
            version,
            bytes,
            data_start,
            entries,
        })
    }

    /// The entry at a path (any case, `\` or `/`).
    pub fn entry(&self, path: &str) -> Option<&VpkEntry> {
        self.entries.get(&normalize_path(path)?)
    }

    /// Every entry's path.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// An entry's bytes: its preload bytes, then its data, read by `data(archive, offset, len)` for a data file's part (an entry
    /// in the directory file is read from the directory's own bytes).
    pub fn read_with(
        &self,
        e: &VpkEntry,
        data: &mut dyn FnMut(u16, u64, u32) -> Result<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        cap("a VPK entry's length", e.total_len(), MAX_ENTRY_BYTES)?;
        let pre = self
            .bytes
            .get(e.preload_at..e.preload_at + usize::from(e.preload_len))
            .ok_or(Error::Truncated {
                what: "a VPK entry's preload bytes",
            })?;
        let mut out = Vec::with_capacity(pre.len());
        out.extend_from_slice(pre);
        if e.len == 0 {
            return Ok(out);
        }
        if e.archive == IN_DIRECTORY {
            let at = self
                .data_start
                .checked_add(e.offset as usize)
                .ok_or(Error::Truncated {
                    what: "a VPK entry",
                })?;
            let s = crate::read::slice(
                &self.bytes,
                at,
                e.len as usize,
                "a VPK entry in the directory",
            )?;
            out.extend_from_slice(s);
        } else {
            let more = data(e.archive, u64::from(e.offset), e.len)?;
            if more.len() != e.len as usize {
                return Err(Error::Truncated {
                    what: "a VPK entry's data",
                });
            }
            out.extend_from_slice(&more);
        }
        Ok(out)
    }
}

/// A directory file on disk and its data files.
#[derive(Debug)]
pub struct VpkArchive {
    pub dir: VpkDir,
    /// The data files' path without the `_NNN.vpk` (the directory file's without `_dir.vpk`).
    stem: PathBuf,
}

impl VpkArchive {
    /// Opens `<stem>_dir.vpk` (`path` may name the directory file or the archive as a game's search paths do, `<stem>.vpk`).
    pub fn open(path: &Path) -> Result<VpkArchive> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let stem_name = name
            .strip_suffix("_dir.vpk")
            .or_else(|| name.strip_suffix(".vpk"))
            .ok_or_else(|| Error::Invalid(format!("{} is not a .vpk", path.display())))?
            .to_string();
        let stem = path.with_file_name(&stem_name);
        let dir_path = path.with_file_name(format!("{stem_name}_dir.vpk"));
        let bytes = read_plain_file(&dir_path, MAX_DIR_BYTES, None)?;
        Ok(VpkArchive {
            dir: VpkDir::parse(bytes)?,
            stem,
        })
    }

    /// The bytes of the entry at `path`, or None when the archive has no such entry.
    pub fn read(&self, path: &str) -> Option<Result<Vec<u8>>> {
        let e = *self.dir.entry(path)?;
        Some(self.read_entry(&e))
    }

    /// An entry's bytes.
    pub fn read_entry(&self, e: &VpkEntry) -> Result<Vec<u8>> {
        let stem = &self.stem;
        self.dir.read_with(e, &mut |archive, offset, len| {
            let name = format!(
                "{}_{archive:03}.vpk",
                stem.file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default()
            );
            let p = stem.with_file_name(name);
            let mut f = open_plain(&p)?;
            f.seek(SeekFrom::Start(offset))
                .map_err(|e| Error::Missing(format!("{}: {e}", p.display())))?;
            let mut out = Vec::new();
            f.take(u64::from(len))
                .read_to_end(&mut out)
                .map_err(|e| Error::Missing(format!("{}: {e}", p.display())))?;
            Ok(out)
        })
    }
}

/// A directory file's bytes for the given entries (tests and fuzz seeds): every entry stored in the directory file itself, the
/// first `preload` bytes of each as preload bytes.
pub fn write_dir(files: &[(&str, &[u8])], version: u32, preload: usize) -> Vec<u8> {
    use std::collections::BTreeMap;
    type Names<'a> = Vec<(String, &'a [u8])>;
    let mut tree: BTreeMap<String, BTreeMap<String, Names<'_>>> = BTreeMap::new();
    for (path, data) in files {
        let (folder, file) = path.rsplit_once('/').unwrap_or((" ", path));
        let (name, ext) = file.rsplit_once('.').unwrap_or((file, " "));
        tree.entry(ext.to_string())
            .or_default()
            .entry(folder.to_string())
            .or_default()
            .push((name.to_string(), data));
    }
    let mut t = Vec::new();
    let mut blob = Vec::new();
    for (ext, folders) in &tree {
        t.extend_from_slice(ext.as_bytes());
        t.push(0);
        for (folder, names) in folders {
            t.extend_from_slice(folder.as_bytes());
            t.push(0);
            for (name, data) in names {
                t.extend_from_slice(name.as_bytes());
                t.push(0);
                let pre = preload.min(data.len());
                t.extend_from_slice(&0u32.to_le_bytes());
                t.extend_from_slice(&(pre as u16).to_le_bytes());
                t.extend_from_slice(&IN_DIRECTORY.to_le_bytes());
                t.extend_from_slice(&(blob.len() as u32).to_le_bytes());
                t.extend_from_slice(&((data.len() - pre) as u32).to_le_bytes());
                t.extend_from_slice(&0xffffu16.to_le_bytes());
                t.extend_from_slice(&data[..pre]);
                blob.extend_from_slice(&data[pre..]);
            }
            t.push(0);
        }
        t.push(0);
    }
    t.push(0);
    let mut out = Vec::new();
    out.extend_from_slice(&SIGNATURE.to_le_bytes());
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&(t.len() as u32).to_le_bytes());
    if version == 2 {
        out.extend_from_slice(&(blob.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 12]);
    }
    out.extend_from_slice(&t);
    out.extend_from_slice(&blob);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_data(_: u16, _: u64, _: u32) -> Result<Vec<u8>> {
        Err(Error::Missing("data file".into()))
    }

    #[test]
    fn reads_what_it_wrote() {
        for version in [1, 2] {
            let bytes = write_dir(
                &[
                    ("materials/concrete/wall.vmt", b"LightmappedGeneric {}"),
                    ("readme", b"no extension"),
                    ("models/a.mdl", b"IDST...."),
                ],
                version,
                4,
            );
            let dir = VpkDir::parse(bytes).unwrap();
            assert_eq!(dir.len(), 3);
            let e = *dir.entry("Materials\\Concrete\\WALL.vmt").unwrap();
            assert_eq!(
                dir.read_with(&e, &mut no_data).unwrap(),
                b"LightmappedGeneric {}"
            );
            let e = *dir.entry("readme").unwrap();
            assert_eq!(dir.read_with(&e, &mut no_data).unwrap(), b"no extension");
        }
    }

    #[test]
    fn refuses_bad_files() {
        assert!(VpkDir::parse(vec![]).is_err());
        assert!(VpkDir::parse(vec![0; 12]).is_err());
        let mut b = write_dir(&[("a/b.c", b"x")], 1, 0);
        b[8] = 0xff;
        assert!(VpkDir::parse(b).is_err());
    }

    #[test]
    fn a_long_folder_cannot_make_huge_paths() {
        // One 1,000-byte folder holding 3,000 short names: 70 kB of tree joins into 3 MB of paths, against a 1 MiB cap (the
        // real cap is reached the same way, only later).
        let mut tree = b"c\0".to_vec();
        tree.extend(std::iter::repeat_n(b'f', 1000));
        tree.push(0);
        for k in 0..3_000u32 {
            tree.extend_from_slice(format!("{k:x}").as_bytes());
            tree.push(0);
            tree.extend_from_slice(&[0; 4]);
            tree.extend_from_slice(&0u16.to_le_bytes());
            tree.extend_from_slice(&IN_DIRECTORY.to_le_bytes());
            tree.extend_from_slice(&[0; 8]);
            tree.extend_from_slice(&0xffffu16.to_le_bytes());
        }
        tree.extend_from_slice(&[0, 0, 0]);
        let mut b = SIGNATURE.to_le_bytes().to_vec();
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&(tree.len() as u32).to_le_bytes());
        b.extend_from_slice(&tree);
        assert!(VpkDir::parse_capped(b.clone(), 4 << 20).is_ok());
        let e = VpkDir::parse_capped(b, 1 << 20).unwrap_err();
        assert!(e.to_string().contains("paths"), "{e}");
    }
}
