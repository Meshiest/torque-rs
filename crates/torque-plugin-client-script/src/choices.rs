//! What the player decided for each server, kept in a file the engine owns.
//!
//! The file lives in a per-user folder ([`default_path`]) and is read and written by [`Choices`] only. It is not a `$Pref::`, not
//! a script global and not under any folder a script's file functions reach, and no native, console function or Lua table gives
//! its contents or path, so neither TorqueScript nor a server's Lua can read, change or learn of it. A server is named in it by
//! the cache's key of its address ([`CacheOwner`]), upgraded to its public key once servers have keys.
//!
//! **Format.** One line a server: `server <16 hex digits> never|allow [<tier>=0|1 ...] address <text>`. Text after `#` is a comment.
//! The file is read strictly: at most [`MAX_FILE_BYTES`], at most [`MAX_SERVERS`] servers (the oldest go first), a line that does
//! not follow the format is skipped, and a file that does not read leaves the choices empty. It is written whole beside itself
//! and renamed over, so a crash leaves the old file or the new one.

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use torque_client_script_format::Tier;
use torque_mods::CacheOwner;

use crate::abilities::ABILITIES;
use crate::policy::Remembered;

/// The most bytes of the file read; a longer file counts as empty.
pub const MAX_FILE_BYTES: u64 = 256 * 1024;
/// The most servers kept.
pub const MAX_SERVERS: usize = 512;
/// The most characters of a server's address kept for the options list.
pub const MAX_LABEL: usize = 64;

const HEADER: &str = "# torque-rs server script choices. The game keeps this file; scripts cannot read or change it.\n";

/// The folder the file is in by default: the platform's per-user cache folder, then `torque-rs/client-scripts`. `None` when
/// the platform gives no such folder, and in a browser.
pub fn default_path() -> Option<PathBuf> {
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    let absolute = |v: std::ffi::OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    let home = || std::env::var_os("HOME").and_then(absolute);
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").and_then(absolute)?
    } else if cfg!(target_os = "macos") {
        home()?.join("Library").join("Caches")
    } else {
        match std::env::var_os("XDG_CACHE_HOME").and_then(absolute) {
            Some(p) => p,
            None => home()?.join(".cache"),
        }
    };
    Some(
        base.join("torque-rs")
            .join("client-scripts")
            .join("choices.txt"),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    server: CacheOwner,
    label: String,
    choice: Remembered,
}

/// The remembered choices, in memory and in their file.
#[derive(Debug, Default)]
pub struct Choices {
    entries: Vec<Entry>,
    path: Option<PathBuf>,
}

/// `text` as one plain line of at most [`MAX_LABEL`] characters.
fn plain(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() && !torque_client_script_format::is_hidden(*c))
        .take(MAX_LABEL)
        .collect::<String>()
        .trim()
        .to_string()
}

impl Choices {
    /// Choices kept in memory only, for a game with no per-user folder and for tests.
    pub fn in_memory() -> Choices {
        Choices::default()
    }

    /// The choices in the file at `path` (none when it is missing, unreadable, too long or not text), kept there from now on.
    pub fn open(path: impl Into<PathBuf>) -> Choices {
        let path = path.into();
        let entries = read_text(&path).map(|t| parse(&t)).unwrap_or_default();
        Choices {
            entries,
            path: Some(path),
        }
    }

    /// Where the choices are kept.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// What was remembered for `server`.
    pub fn get(&self, server: CacheOwner) -> Option<&Remembered> {
        self.entries
            .iter()
            .find(|e| e.server == server)
            .map(|e| &e.choice)
    }

    /// Remembers `choice` for `server`, shown in the options as `address`; the file is rewritten. Past [`MAX_SERVERS`] the
    /// server decided longest ago is forgotten.
    ///
    /// # Errors
    ///
    /// When the file cannot be written; the choice then holds for this run only.
    pub fn set(
        &mut self,
        server: CacheOwner,
        address: &str,
        choice: Remembered,
    ) -> std::io::Result<()> {
        self.entries.retain(|e| e.server != server);
        if self.entries.len() >= MAX_SERVERS {
            self.entries.remove(0);
        }
        self.entries.push(Entry {
            server,
            label: plain(address),
            choice: normalized(choice),
        });
        self.save()
    }

    /// Merges earlier decisions with new ones for `server`: a tier decided now replaces its earlier decision, the others stay.
    ///
    /// # Errors
    ///
    /// As [`set`](Self::set).
    pub fn merge(
        &mut self,
        server: CacheOwner,
        address: &str,
        decided: &[(Tier, bool)],
    ) -> std::io::Result<()> {
        let mut merged = match self.get(server) {
            Some(Remembered::Tiers(list)) => list.clone(),
            _ => Vec::new(),
        };
        for (tier, on) in decided {
            merged.retain(|(t, _)| t != tier);
            merged.push((*tier, *on));
        }
        self.set(server, address, Remembered::Tiers(merged))
    }

    /// Forgets `server`; the file is rewritten. Whether there was anything to forget.
    ///
    /// # Errors
    ///
    /// As [`set`](Self::set).
    pub fn forget(&mut self, server: CacheOwner) -> std::io::Result<bool> {
        let before = self.entries.len();
        self.entries.retain(|e| e.server != server);
        if self.entries.len() == before {
            return Ok(false);
        }
        self.save().map(|()| true)
    }

    /// The servers remembered, oldest decision first, with the address each was shown as.
    pub fn servers(&self) -> Vec<(CacheOwner, &str, &Remembered)> {
        self.entries
            .iter()
            .map(|e| (e.server, e.label.as_str(), &e.choice))
            .collect()
    }

    fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let mut text = String::from(HEADER);
        for e in &self.entries {
            let _ = write!(text, "server {} ", e.server.to_hex());
            match &e.choice {
                Remembered::Never => text.push_str("never"),
                Remembered::Tiers(list) => {
                    text.push_str("allow");
                    for (tier, on) in list {
                        let _ = write!(text, " {}={}", tier.name(), u8::from(*on));
                    }
                }
            }
            let _ = writeln!(text, " address {}", e.label);
        }
        let mut beside = path.clone().into_os_string();
        beside.push(".new");
        let beside = PathBuf::from(beside);
        std::fs::write(&beside, text)?;
        std::fs::rename(&beside, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&beside);
        })
    }
}

/// A choice with each ability once and only the abilities a player decides on.
fn normalized(choice: Remembered) -> Remembered {
    let Remembered::Tiers(list) = choice else {
        return choice;
    };
    let mut kept: Vec<(Tier, bool)> = Vec::new();
    for (tier, on) in list {
        if ABILITIES.contains(&tier) && !kept.iter().any(|(t, _)| *t == tier) {
            kept.push((tier, on));
        }
    }
    Remembered::Tiers(kept)
}

fn read_text(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut text = String::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() as u64 <= MAX_FILE_BYTES).then_some(text)
}

/// The entries in `text`: lines that follow the format, the last of a server winning, the newest [`MAX_SERVERS`].
fn parse(text: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        let Some(entry) = parse_line(line) else {
            continue;
        };
        entries.retain(|e| e.server != entry.server);
        entries.push(entry);
    }
    let extra = entries.len().saturating_sub(MAX_SERVERS);
    entries.drain(..extra);
    entries
}

fn parse_line(line: &str) -> Option<Entry> {
    let mut words = line.split_whitespace();
    if words.next()? != "server" {
        return None;
    }
    let server = CacheOwner::parse_hex(words.next()?)?;
    let mut choice = None;
    let mut list: Vec<(Tier, bool)> = Vec::new();
    let mut label = String::new();
    match words.next()? {
        "never" => choice = Some(Remembered::Never),
        "allow" => {}
        _ => return None,
    }
    while let Some(word) = words.next() {
        if word == "address" {
            label = plain(&words.collect::<Vec<_>>().join(" "));
            break;
        }
        if choice.is_some() {
            return None;
        }
        let (name, value) = word.split_once('=')?;
        let tier = Tier::from_name(name).filter(|t| ABILITIES.contains(t))?;
        let on = match value {
            "1" => true,
            "0" => false,
            _ => return None,
        };
        if list.iter().any(|(t, _)| *t == tier) {
            return None;
        }
        list.push((tier, on));
    }
    Some(Entry {
        server,
        label,
        choice: choice.unwrap_or(Remembered::Tiers(list)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(n: u8) -> CacheOwner {
        CacheOwner::of_server(std::net::SocketAddr::from(([203, 0, 113, n], 28000)))
    }

    #[test]
    fn a_line_that_does_not_follow_the_format_is_skipped() {
        let a = server(1).to_hex();
        let text = format!(
            "server {a} allow ui=1 net=0 address host one\n\
             server zz never\n\
             server {a} allow ui=2\n\
             server {a} allow ui=1 ui=0\n\
             server {a} allow realm=1\n\
             server {a} never ui=1\n\
             server {a} maybe\n\
             nonsense\n"
        );
        let parsed = parse(&text);
        assert_eq!(parsed.len(), 1, "{parsed:?}");
        assert_eq!(
            parsed[0].choice,
            Remembered::Tiers(vec![(Tier::Ui, true), (Tier::Net, false)])
        );
        assert_eq!(parsed[0].label, "host one");
    }

    #[test]
    fn the_last_line_of_a_server_wins_and_the_newest_servers_stay() {
        let mut text = String::new();
        for n in 0..(MAX_SERVERS + 5) {
            let owner =
                CacheOwner::of_server(std::net::SocketAddr::from(([10, 0, 0, 1], 1000 + n as u16)));
            let _ = writeln!(text, "server {} never address s{n}", owner.to_hex());
        }
        let first = CacheOwner::of_server(std::net::SocketAddr::from(([10, 0, 0, 1], 1000)));
        let _ = writeln!(text, "server {} allow ui=1", first.to_hex());
        let parsed = parse(&text);
        assert_eq!(parsed.len(), MAX_SERVERS);
        assert_eq!(
            parsed.last().map(|e| &e.choice),
            Some(&Remembered::Tiers(vec![(Tier::Ui, true)])),
            "a later line replaces the server's earlier one and is the newest"
        );
    }

    #[test]
    fn text_is_plain_and_bounded() {
        assert_eq!(plain("a\u{202e}b\nc"), "abc");
        assert_eq!(plain(&"x".repeat(500)).chars().count(), MAX_LABEL);
    }
}
