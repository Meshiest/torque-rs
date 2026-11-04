//! The client's `console.log`, as TGE's `Con::setLogMode(2)` writes it (`-log 2`): every console line, as it is printed, into a
//! file truncated at the start, opened once, written and flushed line by line. The engine writes it into the user's profile folder
//! ([`log_path`]), never into the install, which stays read-only.
//!
//! **What is written:** a first line `//-------------------------- 10/11/2026 -- 19:53:33 -----` (the local date and time of the
//! start), then each line as the console shows it, ended with CR LF.
//!
//! **Engine rules.** The file is capped ([`MAX_LOG_BYTES`]): past it one closing line says so and nothing more is written (TGE has
//! no cap). Secrets never reach it ([`redact`]): a script's `echo` could print one, so the writer replaces each value that looks
//! like a secret: a run of [`MIN_SECRET_HEX`] or more hexadecimal digits (a sign-in ticket is hundreds of them; the 40 of a SHA-1
//! stay), and the value after a `ticket`, `token`, `joinToken`, `key`, `password` or `code` name in a query string or a form
//! (`name=value`), plus any value the host registered as secret ([`ConsoleLog::add_secret`]: a join token, a ticket).
//! A write that fails stops the log (the console goes on). The last start's log is kept as `console.log.1` ([`previous_path`]): a
//! start moves the file there before it writes its own (TGE truncates its one file), so the session a report is about is still on
//! disk after the next start.
//!
//! Credit: the first line and the log modes follow TGE's console as Torque3D carries it (MIT: `console/console.cpp`, Copyright
//! (c) 2012 GarageGames, LLC).

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ts_vm::vm::Level;

/// The log file's name in the profile folder (TGE's own name).
pub const LOG_NAME: &str = "console.log";
/// The most bytes the log file takes (engine cap; the closing line is written past it).
pub const MAX_LOG_BYTES: u64 = 32 << 20;
/// The shortest run of hexadecimal digits redacted as a secret: longer than a SHA-1's 40, shorter than a sign-in ticket.
pub const MIN_SECRET_HEX: usize = 64;
/// The most registered secrets kept (the newest replace the oldest), and the shortest one matched (a short value would hide words).
pub const MAX_SECRETS: usize = 16;
pub const MIN_SECRET_LEN: usize = 6;
/// The names whose `name=value` values are redacted (ASCII case ignored).
pub const SECRET_NAMES: [&str; 6] = ["ticket", "token", "jointoken", "key", "password", "code"];
/// What a redacted value is written as.
pub const REDACTED: &str = "<redacted>";
/// The closing line once the cap is reached.
pub const CAP_LINE: &str = "console.log: the log reached its size cap; later lines are not written";

/// The profile folder's log path.
pub fn log_path(profile_dir: &Path) -> PathBuf {
    profile_dir.join(LOG_NAME)
}

/// Where a start keeps the last start's log (`console.log.1` beside it).
pub fn previous_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".1");
    path.with_file_name(name)
}

/// TGE's first line: `//-------------------------- MM/DD/YYYY -- HH:MM:SS -----` in local time.
pub fn header_line(now: chrono::DateTime<chrono::Local>) -> String {
    format!(
        "//-------------------------- {} -- {} -----",
        now.format("%m/%d/%Y"),
        now.format("%H:%M:%S")
    )
}

/// The open log: the file, the bytes written and the values to hide.
pub struct ConsoleLog {
    file: Option<File>,
    path: PathBuf,
    written: u64,
    secrets: Vec<Vec<u8>>,
}

/// The log shared by the console sink and the host (which registers secrets).
pub type SharedLog = Rc<RefCell<ConsoleLog>>;

impl ConsoleLog {
    /// Creates the file at `path`, its folder made if missing, and writes TGE's first line; a log already there is kept as
    /// [`previous_path`] (the one kept before is replaced).
    pub fn create(path: &Path) -> io::Result<ConsoleLog> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if path.is_file() {
            let previous = previous_path(path);
            let _ = std::fs::remove_file(&previous);
            // A log that cannot be moved (open elsewhere) is truncated, as before.
            let _ = std::fs::rename(path, &previous);
        }
        let file = File::create(path)?;
        let mut log = ConsoleLog {
            file: Some(file),
            path: path.to_path_buf(),
            written: 0,
            secrets: Vec::new(),
        };
        log.write_raw(header_line(chrono::Local::now()).as_bytes());
        Ok(log)
    }

    /// Where the log is.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether lines still go to the file (not capped, no failed write).
    pub fn is_open(&self) -> bool {
        self.file.is_some()
    }

    /// The bytes written so far.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// A value never written as it is from now on (the join token, a ticket the host holds).
    pub fn add_secret(&mut self, value: &[u8]) {
        if value.len() < MIN_SECRET_LEN || self.secrets.iter().any(|s| s == value) {
            return;
        }
        if self.secrets.len() >= MAX_SECRETS {
            self.secrets.remove(0);
        }
        self.secrets.push(value.to_vec());
    }

    /// One printed line (it may hold newlines: each part is one line of the file).
    pub fn line(&mut self, _level: Level, text: &[u8]) {
        if self.file.is_none() {
            return;
        }
        for part in text.split(|b| *b == b'\n') {
            let part = part.strip_suffix(b"\r").unwrap_or(part);
            let clean = redact(part, &self.secrets);
            if self.written + clean.len() as u64 + 2 > MAX_LOG_BYTES {
                self.write_raw(CAP_LINE.as_bytes());
                self.file = None;
                return;
            }
            self.write_raw(&clean);
        }
    }

    fn write_raw(&mut self, line: &[u8]) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let mut out = Vec::with_capacity(line.len() + 2);
        out.extend_from_slice(line);
        out.extend_from_slice(b"\r\n");
        // One write a line, unbuffered: what was printed is on disk if the client stops (TGE flushes each line too).
        match file.write_all(&out) {
            Ok(()) => self.written += out.len() as u64,
            Err(_) => self.file = None,
        }
    }
}

/// Opens the log at `path` and makes it the console's sink in `vm`: every line printed from now on goes to the file.
pub fn attach(vm: &mut ts_vm::vm::Vm, path: &Path) -> io::Result<SharedLog> {
    let log = Rc::new(RefCell::new(ConsoleLog::create(path)?));
    let sink = Rc::clone(&log);
    vm.set_console_sink(Some(Box::new(move |level, text| {
        sink.borrow_mut().line(level, text);
    })));
    Ok(log)
}

fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

/// `line` with its secrets replaced by [`REDACTED`] (see the module docs).
pub fn redact(line: &[u8], secrets: &[Vec<u8>]) -> Vec<u8> {
    let mut out = line.to_vec();
    for s in secrets {
        out = replace_all(&out, s);
    }
    out = redact_named(&out);
    redact_hex(&out)
}

fn replace_all(text: &[u8], needle: &[u8]) -> Vec<u8> {
    if needle.is_empty() || text.len() < needle.len() {
        return text.to_vec();
    }
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with(needle) {
            out.extend_from_slice(REDACTED.as_bytes());
            i += needle.len();
        } else {
            out.push(text[i]);
            i += 1;
        }
    }
    out
}

/// `name=value` with a secret name: the value (up to `&`, a space or a quote) replaced.
fn redact_named(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i] == b'=' {
            let start = text[..i]
                .iter()
                .rposition(|b| !b.is_ascii_alphanumeric() && *b != b'_')
                .map_or(0, |p| p + 1);
            let name = &text[start..i];
            let secret = SECRET_NAMES
                .iter()
                .any(|n| name.eq_ignore_ascii_case(n.as_bytes()));
            out.push(b'=');
            i += 1;
            if secret {
                let end = text[i..]
                    .iter()
                    .position(|b| matches!(b, b'&' | b' ' | b'"' | b'\'' | b';' | b','))
                    .map_or(text.len(), |p| i + p);
                if end > i {
                    out.extend_from_slice(REDACTED.as_bytes());
                }
                i = end;
            }
            continue;
        }
        out.push(text[i]);
        i += 1;
    }
    out
}

/// Each run of [`MIN_SECRET_HEX`] or more hexadecimal digits replaced.
fn redact_hex(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if is_hex(text[i]) {
            let end = text[i..]
                .iter()
                .position(|b| !is_hex(*b))
                .map_or(text.len(), |p| i + p);
            if end - i >= MIN_SECRET_HEX {
                out.extend_from_slice(REDACTED.as_bytes());
            } else {
                out.extend_from_slice(&text[i..end]);
            }
            i = end;
        } else {
            out.push(text[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
#[path = "console_log_tests.rs"]
mod tests;
