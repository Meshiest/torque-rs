//! The client's known servers: the identity it saw for each address it joined with no identity given, trusted on first use
//! and held to afterwards (as SSH's `known_hosts`). One line per server, `host:port sha256:<hex>`, in [`KNOWN_SERVERS_FILE`]
//! under the client's prefs folder; at most [`MAX_KNOWN_SERVERS`] servers, and never one forgotten on its own: at the cap a
//! new server is refused until the user removes lines ([`KnownError::Full`]). A line that is not an entry is kept as it is.

use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use torque_net::engine::ServerIdentity;

/// The known-servers file's name in the client's prefs folder.
pub const KNOWN_SERVERS_FILE: &str = "known_servers";

/// The most servers the file remembers.
pub const MAX_KNOWN_SERVERS: usize = 1_024;

/// The most bytes read from the file (a line is under 120 bytes).
pub const MAX_KNOWN_SERVERS_BYTES: u64 = 256 << 10;

/// Why the known servers could not be read or written.
#[derive(Debug)]
pub enum KnownError {
    Io(PathBuf, io::Error),
    TooLarge(PathBuf),
    /// The file holds [`MAX_KNOWN_SERVERS`] servers; none is forgotten without the user.
    Full(PathBuf),
}

impl fmt::Display for KnownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KnownError::Io(path, e) => write!(f, "the known servers file {}: {e}", path.display()),
            KnownError::TooLarge(path) => write!(
                f,
                "the known servers file {} is larger than {MAX_KNOWN_SERVERS_BYTES} bytes: not written by this client; move it aside or remove the lines you do not need",
                path.display()
            ),
            KnownError::Full(path) => write!(
                f,
                "the known servers file {} already holds {MAX_KNOWN_SERVERS} servers; remove the lines of servers you no longer join before trusting a new one",
                path.display()
            ),
        }
    }
}

impl std::error::Error for KnownError {}

/// One line of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    Entry(SocketAddr, ServerIdentity),
    /// Not an entry: kept as it is (a comment, a hand edit), never read as one.
    Other(String),
}

/// The servers a client remembers, oldest first.
#[derive(Debug, Clone)]
pub struct KnownServers {
    path: PathBuf,
    lines: Vec<Line>,
}

impl KnownServers {
    /// The file at `path` (none there: no servers known yet).
    ///
    /// # Errors
    /// The file exists but cannot be read, or is too large.
    pub fn load(path: &Path) -> Result<KnownServers, KnownError> {
        let mut known = KnownServers {
            path: path.to_path_buf(),
            lines: Vec::new(),
        };
        let text = match fs::metadata(path) {
            Ok(meta) if meta.len() > MAX_KNOWN_SERVERS_BYTES => {
                return Err(KnownError::TooLarge(path.to_path_buf()));
            }
            Ok(_) => fs::read_to_string(path).map_err(|e| KnownError::Io(path.to_path_buf(), e))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(known),
            Err(e) => return Err(KnownError::Io(path.to_path_buf(), e)),
        };
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            match parse_line(line.trim()) {
                Some((addr, id)) if known.get(addr).is_none() => {
                    known.lines.push(Line::Entry(addr, id));
                }
                _ => known.lines.push(Line::Other(line.to_owned())),
            }
        }
        Ok(known)
    }

    /// The identity remembered for `addr`.
    #[must_use]
    pub fn get(&self, addr: SocketAddr) -> Option<ServerIdentity> {
        self.lines.iter().find_map(|l| match l {
            Line::Entry(a, id) if *a == addr => Some(*id),
            _ => None,
        })
    }

    /// Whether one more server can be remembered.
    #[must_use]
    pub fn has_room(&self) -> bool {
        self.len() < MAX_KNOWN_SERVERS
    }

    /// Remembers `id` for `addr` (an address already known keeps what it had) and writes the file.
    ///
    /// # Errors
    /// The file is full ([`KnownError::Full`]; nothing is forgotten for it), or cannot be written.
    pub fn remember(&mut self, addr: SocketAddr, id: ServerIdentity) -> Result<(), KnownError> {
        if self.get(addr).is_some() {
            return Ok(());
        }
        if !self.has_room() {
            return Err(KnownError::Full(self.path.clone()));
        }
        self.lines.push(Line::Entry(addr, id));
        self.save()
    }

    /// Forgets `addr` (a server whose key really changed) and writes the file.
    ///
    /// # Errors
    /// The file cannot be written.
    pub fn forget(&mut self, addr: SocketAddr) -> Result<(), KnownError> {
        self.lines
            .retain(|l| !matches!(l, Line::Entry(a, _) if *a == addr));
        self.save()
    }

    /// How many servers are known.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| matches!(l, Line::Entry(..)))
            .count()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Lines of the file that are not entries (kept as they are).
    #[must_use]
    pub fn skipped(&self) -> usize {
        self.lines.len() - self.len()
    }

    fn save(&self) -> Result<(), KnownError> {
        let io_error = |e| KnownError::Io(self.path.clone(), e);
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        let mut text = String::new();
        for line in &self.lines {
            match line {
                Line::Entry(addr, id) => writeln!(text, "{addr} {id}"),
                Line::Other(raw) => writeln!(text, "{raw}"),
            }
            .expect("writing to a String cannot fail");
        }
        // Written beside under a name of this process's own, synced, then moved over: a crash leaves the old file whole,
        // and two clients saving at once do not write into one temporary file.
        let temp = temp_path(&self.path);
        let written = fs::File::create(&temp).and_then(|mut file| {
            file.write_all(text.as_bytes())?;
            file.sync_all()
        });
        if let Err(e) = written {
            let _ = fs::remove_file(&temp);
            return Err(io_error(e));
        }
        fs::rename(&temp, &self.path).map_err(|e| {
            let _ = fs::remove_file(&temp);
            io_error(e)
        })
    }
}

static TEMP_COUNTER: AtomicU32 = AtomicU32::new(0);

/// A temporary name beside `path` that no other process, and no other save of this one, uses.
fn temp_path(path: &Path) -> PathBuf {
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().map_or_else(
        || "known_servers".to_owned(),
        |f| f.to_string_lossy().into_owned(),
    );
    path.with_file_name(format!("{name}.{}.{n}.tmp", std::process::id()))
}

fn parse_line(line: &str) -> Option<(SocketAddr, ServerIdentity)> {
    let (addr, id) = line.split_once(char::is_whitespace)?;
    Some((addr.parse().ok()?, id.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "torque-net-quic-known-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ))
    }

    #[test]
    fn servers_are_remembered_once_read_back_and_bad_lines_are_skipped() {
        let dir = scratch("round-trip");
        let path = dir.join(KNOWN_SERVERS_FILE);
        let a: SocketAddr = "10.0.0.1:28000".parse().expect("an address");
        let b: SocketAddr = "10.0.0.2:28000".parse().expect("an address");
        let mut known = KnownServers::load(&path).expect("no file is fine");
        assert!(known.is_empty());
        known.remember(a, ServerIdentity([1; 32])).expect("written");
        known.remember(a, ServerIdentity([2; 32])).expect("written");
        known.remember(b, ServerIdentity([3; 32])).expect("written");
        assert_eq!(known.get(a), Some(ServerIdentity([1; 32])));
        let mut text = fs::read_to_string(&path).expect("the file");
        text.push_str("# a note\n10.0.0.3:1 sha256:zz\n");
        fs::write(&path, text).expect("written");
        let again = KnownServers::load(&path).expect("read");
        assert_eq!((again.len(), again.skipped()), (2, 2));
        assert_eq!(again.get(b), Some(ServerIdentity([3; 32])));
        let mut again = again;
        again.forget(a).expect("written");
        let text = fs::read_to_string(&path).expect("the file");
        assert_eq!(KnownServers::load(&path).expect("read").get(a), None);
        // The lines that are not entries are written back as they were.
        assert!(text.contains("# a note\n10.0.0.3:1 sha256:zz\n"), "{text}");
        fs::remove_dir_all(&dir).expect("removed");
    }

    #[test]
    fn a_save_leaves_no_temporary_file_and_two_savers_do_not_clobber_each_other() {
        let dir = scratch("atomic");
        let path = dir.join(KNOWN_SERVERS_FILE);
        let a: SocketAddr = "10.0.0.1:28000".parse().expect("an address");
        let b: SocketAddr = "10.0.0.2:28000".parse().expect("an address");
        let mut first = KnownServers::load(&path).expect("no file is fine");
        let mut second = first.clone();
        assert_ne!(temp_path(&path), temp_path(&path), "a name per save");
        assert!(
            temp_path(&path)
                .to_string_lossy()
                .contains(&std::process::id().to_string())
        );
        first.remember(a, ServerIdentity([1; 32])).expect("written");
        second
            .remember(b, ServerIdentity([2; 32]))
            .expect("written");
        let left: Vec<_> = fs::read_dir(&dir)
            .expect("the folder")
            .map(|e| {
                e.expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(left, [KNOWN_SERVERS_FILE], "no temporary file is left");
        // The last save won whole; nothing was interleaved.
        let again = KnownServers::load(&path).expect("read");
        assert_eq!(
            (again.len(), again.get(b)),
            (1, Some(ServerIdentity([2; 32])))
        );
        fs::write(&path, vec![b'x'; MAX_KNOWN_SERVERS_BYTES as usize + 1]).expect("written");
        let error = KnownServers::load(&path).expect_err("too large");
        assert!(error.to_string().contains("move it aside"), "{error}");
        fs::remove_dir_all(&dir).expect("removed");
    }

    #[test]
    fn at_the_cap_a_new_server_is_refused_and_no_pin_is_forgotten() {
        let dir = scratch("full");
        let path = dir.join(KNOWN_SERVERS_FILE);
        let mut known = KnownServers::load(&path).expect("no file is fine");
        for i in 0..MAX_KNOWN_SERVERS {
            let addr = SocketAddr::from(([10, (i >> 8) as u8, i as u8, 1], 28_000));
            known
                .lines
                .push(Line::Entry(addr, ServerIdentity([i as u8; 32])));
        }
        assert!(!known.has_room());
        let more = SocketAddr::from(([10, 99, 99, 1], 28_000));
        let error = known
            .remember(more, ServerIdentity([7; 32]))
            .expect_err("full");
        assert!(
            matches!(error, KnownError::Full(ref p) if *p == path),
            "{error}"
        );
        assert!(error.to_string().contains(KNOWN_SERVERS_FILE), "{error}");
        assert_eq!(known.len(), MAX_KNOWN_SERVERS);
        assert_eq!(
            known.get(SocketAddr::from(([10, 0, 0, 1], 28_000))),
            Some(ServerIdentity([0; 32])),
            "the oldest pin stays"
        );
        assert_eq!(known.get(more), None);
        known.save().expect("written");
        assert!(fs::metadata(&path).expect("the file").len() < MAX_KNOWN_SERVERS_BYTES);
        fs::remove_dir_all(&dir).expect("removed");
    }
}
