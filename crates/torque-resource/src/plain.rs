//! Plain files, opened without following a link and read within a cap: the one reader the format crates, the folder host and
//! the importers share for a file a user, a mod or a download may have put there.
//!
//! [`open_plain`] opens `path` with the platform's no-follow flag (`O_NOFOLLOW` on Unix, `FILE_FLAG_OPEN_REPARSE_POINT` on
//! Windows, so a link that is the last component is not followed; `O_NONBLOCK` too, so a FIFO is refused rather than waited
//! on) and then looks at the handle that came back: it must be a regular file and no link of any kind. A file that passes has
//! `O_NONBLOCK` cleared again, so reads on the handle are ordinary blocking reads. A link in a folder on the way is the
//! caller's to refuse (the folder host looks at each component). [`read_plain`] reads the whole file within a cap, and within
//! the length a listing gave, taking the length from the open handle and stopping one byte past the limit, so a file that grew
//! after it was listed (or lies about its length) never allocates more than the limit.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::path::Path;

/// Why a file was not read.
#[derive(Debug)]
pub enum PlainError {
    /// The file could not be opened or read.
    Io(io::Error),
    /// It is a link, a folder or some other kind of file.
    NotPlain,
    /// It is longer than the limit.
    TooLarge { len: u64, cap: u64 },
}

impl fmt::Display for PlainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlainError::Io(e) => e.fmt(f),
            PlainError::NotPlain => f.write_str("not a plain file (a link or a folder)"),
            PlainError::TooLarge { len, cap } => {
                write!(f, "{len} bytes is over the limit of {cap}")
            }
        }
    }
}

impl std::error::Error for PlainError {}

impl From<PlainError> for io::Error {
    fn from(e: PlainError) -> io::Error {
        match e {
            PlainError::Io(e) => e,
            PlainError::NotPlain => io::Error::new(io::ErrorKind::InvalidInput, e),
            PlainError::TooLarge { .. } => io::Error::new(io::ErrorKind::InvalidData, e),
        }
    }
}

impl From<io::Error> for PlainError {
    fn from(e: io::Error) -> PlainError {
        PlainError::Io(e)
    }
}

/// `e` with `path` in its message, keeping its kind: for a caller that reads many files and must say which one failed.
pub fn at_path(path: &Path, e: io::Error) -> io::Error {
    io::Error::new(e.kind(), format!("{}: {e}", path.display()))
}

/// Whether `meta` is a link of any kind: a symbolic link, or on Windows any reparse point (of an open handle, or of a path read
/// with `symlink_metadata`).
pub fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

/// Opens `path` for reading without following a link in its last component, and returns the file with its length. A link, a
/// folder and a device are [`PlainError::NotPlain`].
pub fn open_plain(path: &Path) -> Result<(File, u64), PlainError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // `O_NONBLOCK` so that opening a FIFO does not wait for a writer before the check below refuses it; it is cleared once
        // the handle is known to be a regular file.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        // `O_NOFOLLOW` on a link is ELOOP (or EMLINK on some systems): a link, not a missing file.
        #[cfg(unix)]
        Err(e) if matches!(e.raw_os_error(), Some(libc::ELOOP | libc::EMLINK)) => {
            return Err(PlainError::NotPlain);
        }
        Err(e) => return Err(e.into()),
    };
    let meta = file.metadata()?;
    if is_link(&meta) || !meta.is_file() {
        return Err(PlainError::NotPlain);
    }
    #[cfg(unix)]
    clear_nonblocking(&file)?;
    Ok((file, meta.len()))
}

/// Takes `O_NONBLOCK` off `file`, which [`open_plain`] set so that a FIFO would not block the open.
#[cfg(unix)]
fn clear_nonblocking(file: &File) -> io::Result<()> {
    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
    fcntl_setfl(file, fcntl_getfl(file)? - OFlags::NONBLOCK)?;
    Ok(())
}

/// The rest of the open `file` of length `len`, at most `limit` bytes: a file longer than that, by its handle or by what the
/// read finds, is [`PlainError::TooLarge`].
pub fn read_within(file: File, len: u64, limit: u64) -> Result<Vec<u8>, PlainError> {
    let too_large = |len: u64| PlainError::TooLarge { len, cap: limit };
    if len > limit {
        return Err(too_large(len));
    }
    let mut out = Vec::new();
    out.try_reserve_exact(usize::try_from(len).map_err(|_| too_large(len))?)
        .map_err(|_| too_large(len))?;
    file.take(limit.saturating_add(1)).read_to_end(&mut out)?;
    let read = out.len() as u64;
    if read > limit {
        return Err(too_large(read));
    }
    Ok(out)
}

/// The whole plain file at `path` ([`open_plain`]), at most `max` bytes, and at most `listed` bytes when a listing gave its
/// length ([`read_within`]).
pub fn read_plain(path: &Path, max: u64, listed: Option<u64>) -> Result<Vec<u8>, PlainError> {
    let (file, len) = open_plain(path)?;
    read_within(file, len, listed.map_or(max, |l| l.min(max)))
}
