//! Staging: where the blocks of an upload wait until all of them are in.
//!
//! An upload up to [`StagingConfig::memory_per_upload`] is staged in memory, a larger one in a file of the server-private folder
//! (`StagingConfig::folder`), under a random name. Nothing a client sends is ever part of a path. Both kinds are bounded by the
//! pool's caps, counted at the declared size when the upload is accepted, so staging never holds more than the caps however a
//! client behaves. A staged upload is gone when its [`Staged`] is dropped (its file is deleted), and the folder's leftovers from
//! an earlier run are deleted when the pool is made: only those named like staging files (`upload-<16 hex digits>.part`), so
//! a folder a game chose keeps everything else it holds.
//!
//! The sink reads the finished content through [`UntrustedBytes`]: a read-only view, in memory or on disk, that is never
//! loaded whole unless the sink asks for it with a cap.

use std::collections::hash_map::RandomState;
use std::fs::{self, File, OpenOptions};
use std::hash::BuildHasher;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use crate::blocks::BLOCK_BYTES;
use crate::spec::Refusal;

/// The start and the end of the name of a staging file.
const FILE_PREFIX: &str = "upload-";
const FILE_SUFFIX: &str = ".part";

/// What staging may use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagingConfig {
    /// The largest upload staged in memory (1 MiB).
    pub memory_per_upload: usize,
    /// All the memory staging may hold together (8 MiB).
    pub memory_total: usize,
    /// The bytes staging may hold on disk together (1 GiB).
    pub disk_total: u64,
    /// The declared bytes of the uploads one player may have staged at once, going or waiting to be resumed, all kinds together
    /// (64 MiB). A new upload that would pass it first evicts that player's oldest waiting uploads, and is refused if that is
    /// not enough.
    pub per_subject: u64,
    /// The server-private folder for larger uploads. `None` stages in memory only, so an upload above
    /// `memory_per_upload` is refused. No script or mount may reach this folder.
    pub folder: Option<PathBuf>,
}

impl Default for StagingConfig {
    fn default() -> StagingConfig {
        StagingConfig {
            memory_per_upload: 1 << 20,
            memory_total: 8 << 20,
            disk_total: 1 << 30,
            per_subject: 64 << 20,
            folder: None,
        }
    }
}

/// What a staged upload holds against the pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Charge {
    Memory(usize),
    Disk(u64),
}

enum Backing {
    Memory(Vec<u8>),
    File { file: File, path: PathBuf },
}

/// One upload's staged bytes. Dropping it deletes its file.
pub(crate) struct Staged {
    backing: Backing,
    size: u64,
    charge: Charge,
}

impl Drop for Staged {
    fn drop(&mut self) {
        if let Backing::File { path, .. } = &self.backing {
            let _ = fs::remove_file(path);
        }
    }
}

impl Staged {
    /// Keeps the bytes of block `index` (all of them, in place).
    pub(crate) fn write_block(&mut self, index: u32, bytes: &[u8]) -> io::Result<()> {
        let at = u64::from(index) * BLOCK_BYTES as u64;
        let end = at
            .checked_add(bytes.len() as u64)
            .filter(|&end| end <= self.size)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "block past the end"))?;
        match &mut self.backing {
            Backing::Memory(buffer) => {
                // The buffer is made when the first block arrives, so an upload that never sends one holds no memory.
                if buffer.is_empty() {
                    buffer.resize(self.size as usize, 0);
                }
                buffer[at as usize..end as usize].copy_from_slice(bytes);
                Ok(())
            }
            Backing::File { file, .. } => {
                file.seek(SeekFrom::Start(at))?;
                file.write_all(bytes)
            }
        }
    }

    /// The view of the staged bytes, to read once every block is in.
    pub(crate) fn view(&self) -> UntrustedBytes<'_> {
        UntrustedBytes { staged: self }
    }
}

/// The finished content of an upload, as the client sent it: read-only, never trusted. Its hash matched what the client
/// declared, which says only that it is the bytes the client meant to send. A sink parses it as untrusted input.
#[derive(Clone, Copy)]
pub struct UntrustedBytes<'a> {
    staged: &'a Staged,
}

impl std::fmt::Debug for UntrustedBytes<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "UntrustedBytes({} bytes)", self.len())
    }
}

impl<'a> UntrustedBytes<'a> {
    /// The content's size.
    pub fn len(&self) -> u64 {
        self.staged.size
    }

    /// Whether there are no bytes (an upload is never empty).
    pub fn is_empty(&self) -> bool {
        self.staged.size == 0
    }

    /// The bytes if they are in memory.
    pub fn as_slice(&self) -> Option<&'a [u8]> {
        match &self.staged.backing {
            Backing::Memory(buffer) => Some(buffer),
            Backing::File { .. } => None,
        }
    }

    /// Reads up to `buf.len()` bytes from `offset`; fewer only at the end of the content.
    ///
    /// # Errors
    /// The disk's, for content staged in a file.
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.staged.size.saturating_sub(offset);
        let want = usize::try_from(left).unwrap_or(usize::MAX).min(buf.len());
        match &self.staged.backing {
            Backing::Memory(bytes) => {
                let from = usize::try_from(offset)
                    .unwrap_or(usize::MAX)
                    .min(bytes.len());
                let part = &bytes[from..bytes.len().min(from + want)];
                buf[..part.len()].copy_from_slice(part);
                Ok(part.len())
            }
            Backing::File { file, .. } => {
                let mut file = file;
                file.seek(SeekFrom::Start(offset))?;
                file.read(&mut buf[..want])
            }
        }
    }

    /// A reader over the content from its start. Wrap it with [`Read::take`] to bound what a parser may read.
    pub fn reader(&self) -> ContentReader<'a> {
        ContentReader {
            bytes: *self,
            at: 0,
        }
    }

    /// The whole content in memory, if it is at most `cap` bytes.
    ///
    /// # Errors
    /// [`io::ErrorKind::InvalidInput`] for content above `cap`; else the disk's.
    pub fn to_vec(&self, cap: usize) -> io::Result<Vec<u8>> {
        if self.staged.size > cap as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the content is larger than the cap",
            ));
        }
        let mut out = Vec::with_capacity(self.staged.size as usize);
        self.reader().read_to_end(&mut out)?;
        Ok(out)
    }
}

/// A [`Read`] over [`UntrustedBytes`].
pub struct ContentReader<'a> {
    bytes: UntrustedBytes<'a>,
    at: u64,
}

impl Read for ContentReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.bytes.read_at(self.at, buf)?;
        self.at += n as u64;
        Ok(n)
    }
}

/// The staging caps and what is held against them.
pub(crate) struct Pool {
    config: StagingConfig,
    memory_used: usize,
    disk_used: u64,
    names: RandomState,
    counter: u64,
    /// Why the folder could not be used, if so (staging then holds memory only).
    pub(crate) warning: Option<String>,
}

impl Pool {
    pub(crate) fn new(mut config: StagingConfig) -> Pool {
        let mut warning = None;
        if let Some(folder) = &config.folder
            && let Err(e) = prepare_folder(folder)
        {
            warning = Some(format!(
                "upload staging folder {} is not usable ({e}); staging in memory only",
                folder.display()
            ));
            config.folder = None;
        }
        Pool {
            config,
            memory_used: 0,
            disk_used: 0,
            names: RandomState::new(),
            counter: 0,
            warning,
        }
    }

    pub(crate) fn memory_used(&self) -> usize {
        self.memory_used
    }

    pub(crate) fn disk_used(&self) -> u64 {
        self.disk_used
    }

    pub(crate) fn config(&self) -> &StagingConfig {
        &self.config
    }

    /// Room for `size` bytes, in memory if it is small enough and fits, else in a file.
    pub(crate) fn open(&mut self, size: u64) -> Result<Staged, Refusal> {
        if size <= self.config.memory_per_upload as u64
            && self.memory_used as u64 + size <= self.config.memory_total as u64
        {
            let size_in_memory = size as usize;
            self.memory_used += size_in_memory;
            return Ok(Staged {
                backing: Backing::Memory(Vec::new()),
                size,
                charge: Charge::Memory(size_in_memory),
            });
        }
        let folder = self.config.folder.as_ref().ok_or(Refusal::ServerBusy)?;
        if self.disk_used.saturating_add(size) > self.config.disk_total {
            return Err(Refusal::ServerBusy);
        }
        self.counter += 1;
        let name = staging_name(self.names.hash_one(self.counter));
        let path = folder.join(name);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| Refusal::ServerBusy)?;
        if file.set_len(size).is_err() {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(Refusal::ServerBusy);
        }
        self.disk_used += size;
        Ok(Staged {
            backing: Backing::File { file, path },
            size,
            charge: Charge::Disk(size),
        })
    }

    /// Gives back what `staged` held.
    pub(crate) fn release(&mut self, staged: Staged) {
        match staged.charge {
            Charge::Memory(n) => self.memory_used = self.memory_used.saturating_sub(n),
            Charge::Disk(n) => self.disk_used = self.disk_used.saturating_sub(n),
        }
    }
}

/// The name of a staging file: the marker is what tells a file of this crate's from anything else in a folder a game chose.
fn staging_name(random: u64) -> String {
    format!("{FILE_PREFIX}{random:016x}{FILE_SUFFIX}")
}

/// Whether `name` is a staging file's.
fn is_staging_name(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|n| {
        n.strip_prefix(FILE_PREFIX)
            .and_then(|n| n.strip_suffix(FILE_SUFFIX))
            .is_some_and(|hex| hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
    })
}

/// Makes the folder and deletes the staging files an earlier run left in it: only files that carry the marker, whatever else the
/// folder holds.
fn prepare_folder(folder: &std::path::Path) -> io::Result<()> {
    fs::create_dir_all(folder)?;
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && is_staging_name(&entry.file_name()) {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "staging_tests.rs"]
mod tests;
