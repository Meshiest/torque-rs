//! Who decides whether a library may load: [`Approve`], the hashes approved so far ([`Allowlist`]), and the question on
//! the terminal ([`TerminalPrompt`]).

use std::fmt::Write as _;
use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

/// The file in the per-user folder ([`user_folder`](crate::user_folder)) that keeps the approved hashes, one
/// `<sha256> <id> <version>` line each. Never in the plugins folder: one found there is ignored.
pub const ALLOWLIST_FILE: &str = "approved.txt";

/// The largest allowlist read (bytes); a longer one counts as empty.
const MAX_ALLOWLIST_BYTES: u64 = 1024 * 1024;

/// What the player is asked to approve before a library loads: none of its code has run yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalRequest {
    /// The id its manifest gives (the library must report the same).
    pub id: String,
    /// The version its manifest gives (the library must report the same).
    pub version: String,
    /// The library's path.
    pub library: PathBuf,
    /// The library's SHA-256 hash, lower-case hex: what is approved.
    pub sha256: String,
    /// The hash approved before for this id, when it was another: the file changed since.
    pub approved_before: Option<String>,
}

impl ApprovalRequest {
    /// The question, as a terminal or a dialog shows it.
    pub fn question(&self) -> String {
        let mut text = String::from(
            "A native extension wants to load. It is a compiled program that runs with all of the game's rights: it can read \
             and change your files, use the network, and do anything else the game can do. Load it only if you trust \
             where it came from.\n",
        );
        let _ = writeln!(text, "  id:      {}", self.id);
        let _ = writeln!(text, "  version: {}", self.version);
        let _ = writeln!(text, "  file:    {}", self.library.display());
        let _ = writeln!(text, "  sha256:  {}", self.sha256);
        if let Some(before) = &self.approved_before {
            let _ = writeln!(
                text,
                "  The file has changed since you approved it (then sha256 {before})."
            );
        }
        text
    }
}

/// Decides whether a library may load.
pub trait Approve {
    /// True to load it.
    fn approve(&self, request: &ApprovalRequest) -> bool;
}

impl<F: Fn(&ApprovalRequest) -> bool> Approve for F {
    fn approve(&self, request: &ApprovalRequest) -> bool {
        self(request)
    }
}

/// One approved file in an [`Allowlist`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approved {
    /// The library's SHA-256 hash.
    pub sha256: String,
    /// The extension's id.
    pub id: String,
    /// Its version.
    pub version: String,
}

/// The hashes approved so far, kept in a file (`<sha256> <id> <version>` lines; `#` starts a comment): a library whose
/// id, version and hash are there loads without a question. Any other is asked about, when an asker is given
/// ([`asking`](Self::asking)), and recorded when approved, in place of an older line of its id; without one it is not
/// loaded.
pub struct Allowlist {
    path: PathBuf,
    ask: Option<Box<dyn Approve>>,
}

impl Allowlist {
    /// The allowlist kept at `path`, asking nobody.
    pub fn new(path: impl Into<PathBuf>) -> Allowlist {
        Allowlist {
            path: path.into(),
            ask: None,
        }
    }

    /// Asks `ask` about a library it does not approve yet.
    pub fn asking(mut self, ask: impl Approve + 'static) -> Allowlist {
        self.ask = Some(Box::new(ask));
        self
    }

    /// The file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file's text: `None` when there is no file, Err when it cannot be read, is not UTF-8 or is over
    /// [`MAX_ALLOWLIST_BYTES`].
    fn text(&self) -> std::io::Result<Option<String>> {
        let file = match std::fs::File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut text = String::new();
        file.take(MAX_ALLOWLIST_BYTES + 1)
            .read_to_string(&mut text)?;
        if text.len() as u64 > MAX_ALLOWLIST_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("it is over {MAX_ALLOWLIST_BYTES} bytes"),
            ));
        }
        Ok(Some(text))
    }

    /// What it approves now (none when the file is missing or unreadable).
    pub fn approved(&self) -> Vec<Approved> {
        match self.text() {
            Ok(Some(text)) => parse_approved(&text),
            _ => Vec::new(),
        }
    }

    /// Records `request` as approved, in place of any older line of its id. The file is replaced whole (written beside
    /// it, then renamed over it), and never over a file that cannot be read: Err then, and the file is left as it was.
    pub fn record(&self, request: &ApprovalRequest) -> std::io::Result<()> {
        let old = self
            .text()?
            .map(|text| parse_approved(&text))
            .unwrap_or_default();
        let mut kept: Vec<Approved> = old.into_iter().filter(|a| a.id != request.id).collect();
        kept.push(Approved {
            sha256: request.sha256.clone(),
            id: request.id.clone(),
            version: request.version.clone(),
        });
        let mut text = String::from(
            "# Native extensions approved to load: <sha256> <id> <version>. Each runs with the game's full rights.\n",
        );
        for a in &kept {
            let _ = writeln!(text, "{} {} {}", a.sha256, a.id, a.version);
        }
        if let Some(parent) = self.path.parent() {
            crate::stage::make_private_dir(parent)?;
        }
        let mut part = self.path.clone().into_os_string();
        part.push(format!(".part-{}", std::process::id()));
        let part = PathBuf::from(part);
        let written =
            write_private(&part, text.as_bytes()).and_then(|()| std::fs::rename(&part, &self.path));
        if written.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        written
    }
}

/// Writes `bytes` to the file `path`, made on Unix readable and writable by this user only.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

/// The approvals in an allowlist's text: `<sha256> <id> <version>` lines, `#` starting a comment.
fn parse_approved(text: &str) -> Vec<Approved> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let (sha256, id, version) = (words.next()?, words.next()?, words.next()?);
            let hex = sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit());
            (hex && words.next().is_none()).then(|| Approved {
                sha256: sha256.to_ascii_lowercase(),
                id: id.to_string(),
                version: version.to_string(),
            })
        })
        .collect()
}

impl Approve for Allowlist {
    fn approve(&self, request: &ApprovalRequest) -> bool {
        let approved = self.approved();
        if approved.iter().any(|a| {
            a.id == request.id && a.version == request.version && a.sha256 == request.sha256
        }) {
            return true;
        }
        let Some(ask) = &self.ask else {
            return false;
        };
        let mut request = request.clone();
        request.approved_before = approved
            .iter()
            .find(|a| a.id == request.id && a.sha256 != request.sha256)
            .map(|a| a.sha256.clone());
        if !ask.approve(&request) {
            return false;
        }
        // Approved: loaded this time even when the line cannot be written.
        let _ = self.record(&request);
        true
    }
}

/// Asks on the terminal: the [`question`](ApprovalRequest::question) on standard error, and `yes` typed on standard
/// input approves. With no terminal on both (a test, a service, a launcher), it approves nothing and never waits.
#[derive(Clone, Copy, Debug, Default)]
pub struct TerminalPrompt;

impl Approve for TerminalPrompt {
    fn approve(&self, request: &ApprovalRequest) -> bool {
        if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
            return false;
        }
        let mut err = std::io::stderr();
        let _ = write!(err, "{}Type yes to load it: ", request.question());
        let _ = err.flush();
        let mut answer = String::new();
        if std::io::stdin().lock().read_line(&mut answer).is_err() {
            return false;
        }
        answer.trim().eq_ignore_ascii_case("yes")
    }
}
