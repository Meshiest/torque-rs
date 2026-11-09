//! The leak scan: no file of a bundle may hold a folder of the computer that built it (the workspace, the build folder, the
//! cargo and rustup homes, the user's home). A user or host name is looked for too, but only between path separators and only as
//! a warning ([`Hit::Leak::soft`]): on a build server the name is a common word (`runner`).
//!
//! **Spellings.** Text is compared with every backslash turned into a slash and ASCII case folded, so each folder is looked for
//! as one spelling with slashes, doubled slashes (a path printed in Rust's debug form), percent-encoded
//! (`file:///C:/Home/My%20Name`), as an MSYS (`/c/home/...`) and a WSL (`/mnt/c/home/...`) path, and, on a drive path, in the
//! 8.3 short form Windows gives a long name (`MYNAME~1`, `MY-LON~1`, with the numbers 1 to 4); each in UTF-8, UTF-16 little
//! endian and UTF-16 big endian. One Aho-Corasick automaton holds every spelling of every needle.
//!
//! **Inside files.** [`Scanner::scan_file`] also looks inside what a game folder holds compressed: every zip in a file (found
//! by its end record, so data in front of one hides nothing; a record counts only when it points at a central directory), zips in
//! zips, gzip and raw zlib streams, and the compressed text chunks of a PNG, up to a work budget ([`Scanner::max_inflated`]).
//! What cannot be read is a hit of its own ([`Hit::Unscannable`]) and fails the bundle: a zip that does not open, one with more
//! members than the scan reads, a container nested too deep, a PNG cut short or with a text chunk that does not parse, and
//! the formats the scan does not decode (xz, zstd, bzip2, 7z, WOFF).

use std::io::{Cursor, Read};
use std::path::Path;

use aho_corasick::{AhoCorasick, MatchKind};
use memchr::memmem;

/// What a needle is looked for as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A folder: found anywhere.
    Folder,
    /// A bare name: found only between path separators, and only a warning.
    Name,
}

/// How a spelling is encoded in the bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}

impl Encoding {
    /// The bytes of a `/` in this encoding.
    fn separator(self) -> &'static [u8] {
        match self {
            Encoding::Utf8 => b"/",
            Encoding::Utf16Le => b"/\0",
            Encoding::Utf16Be => b"\0/",
        }
    }

    fn encode(self, lower: &str) -> Vec<u8> {
        match self {
            Encoding::Utf8 => lower.as_bytes().to_vec(),
            Encoding::Utf16Le => lower.encode_utf16().flat_map(u16::to_le_bytes).collect(),
            Encoding::Utf16Be => lower.encode_utf16().flat_map(u16::to_be_bytes).collect(),
        }
    }
}

const ENCODINGS: [Encoding; 3] = [Encoding::Utf8, Encoding::Utf16Le, Encoding::Utf16Be];

/// A folder or name to look for, by what it is (never printed: the label is).
#[derive(Clone, Debug)]
pub struct Needle {
    pub label: String,
    kind: Kind,
    /// The lower-case spellings, with `/` for every separator.
    spellings: Vec<String>,
}

/// Folders and names shorter than this are not looked for: a short one would match by chance.
const MIN_LEN: usize = 6;

/// The most bytes inflated for one file of the bundle, over all the members and streams inside it, unless the caller says
/// otherwise ([`Scanner::new`]).
pub const MAX_INFLATED_BYTES: usize = 512 << 20;
/// The deepest nesting of containers looked into (a zip in a zip in a zip).
const MAX_DEPTH: usize = 4;
/// The most members of one zip looked at.
const MAX_MEMBERS: usize = 100_000;
/// The short-name numbers tried for a long name (`NAME~1` to `NAME~4`).
const SHORT_NUMBERS: [u8; 4] = [1, 2, 3, 4];
/// The most components of a path that get more than one short-name number.
const MAX_SHORT_COMPONENTS: usize = 4;

/// What the scan found.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Hit {
    /// A needle was found.
    Leak {
        /// `!` and the member's name for each container the bytes were inside (`!mods/a.zip!notes.txt`), or empty for the file's
        /// own bytes.
        member: String,
        label: String,
        /// Only a warning (a bare user or host name): it does not fail a bundle.
        soft: bool,
    },
    /// Something was not looked into, so the scan cannot say it is clean.
    Unscannable { member: String, why: String },
}

impl Hit {
    /// `path` with the member, and what was found: `mods/a.zip!notes.txt: holds the workspace`.
    pub fn describe(&self, path: &str) -> String {
        match self {
            Hit::Leak { member, label, .. } => format!("{path}{member}: holds {label}"),
            Hit::Unscannable { member, why } => {
                format!("{path}{member}: could not be scanned ({why})")
            }
        }
    }

    /// Whether the hit fails a bundle (a warning does not).
    pub fn is_hard(&self) -> bool {
        !matches!(self, Hit::Leak { soft: true, .. })
    }
}

impl Needle {
    /// `folder`, under `label`, or none when it is too short to look for.
    pub fn new(label: &str, folder: &Path) -> Option<Needle> {
        let text = folder.to_string_lossy().replace('\\', "/");
        let text = text.trim_end_matches('/');
        let text = text.strip_prefix("//?/").unwrap_or(text);
        if text.len() < MIN_LEN {
            return None;
        }
        let mut spellings = vec![
            text.to_string(),
            text.replace('/', "//"),
            percent_encode(text, false, None),
            percent_encode(text, true, None),
        ];
        for slash in ["%2f", "%5c"] {
            spellings.push(percent_encode(text, false, Some(slash)));
            spellings.push(percent_encode(text, true, Some(slash)));
        }
        if let Some((drive, rest)) = drive_path(text) {
            spellings.push(format!("/{drive}{rest}"));
            spellings.push(format!("/mnt/{drive}{rest}"));
            spellings.extend(short_forms(text));
        }
        Some(Needle {
            label: label.to_string(),
            kind: Kind::Folder,
            spellings,
        })
    }

    /// A bare `name` (a user or a host), under `label`, or none when it is too short to look for.
    pub fn name(label: &str, name: &str) -> Option<Needle> {
        let name = name.trim();
        if name.len() < MIN_LEN || name.contains(['/', '\\']) {
            return None;
        }
        Some(Needle {
            label: label.to_string(),
            kind: Kind::Name,
            spellings: vec![name.to_string(), percent_encode(name, false, None)],
        })
    }
}

/// The user and host names of this computer as needles ([`Needle::name`]; each the sources give, once): the user from
/// `USERNAME`, `USER` and `LOGNAME` and the last part of the home folder; the host from `COMPUTERNAME` and `HOSTNAME`
/// (a shell often does not export the latter) and the host name files of Linux. `var` reads an environment variable and `read`
/// a text file; both give `None` for what is missing or empty.
pub fn name_needles(
    var: impl Fn(&str) -> Option<String>,
    read: impl Fn(&str) -> Option<String>,
    home: Option<&Path>,
) -> Vec<Needle> {
    let mut users: Vec<String> = ["USERNAME", "USER", "LOGNAME"]
        .into_iter()
        .filter_map(&var)
        .collect();
    users.extend(
        home.and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned()),
    );
    let mut hosts: Vec<String> = ["COMPUTERNAME", "HOSTNAME"]
        .into_iter()
        .filter_map(&var)
        .collect();
    hosts.extend(
        ["/proc/sys/kernel/hostname", "/etc/hostname"]
            .into_iter()
            .filter_map(&read),
    );
    let mut seen: Vec<String> = Vec::new();
    let mut needles = Vec::new();
    for (label, names) in [("the user name", users), ("the host name", hosts)] {
        for name in names {
            let name = name.trim().to_string();
            if !name.is_empty() && !seen.contains(&name.to_ascii_lowercase()) {
                seen.push(name.to_ascii_lowercase());
                needles.extend(Needle::name(label, &name));
            }
        }
    }
    needles
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// `text` with every byte outside the URL-unreserved set percent-encoded: `/` kept, or spelled `slash` (`%2f` or `%5c`, the
/// separator of a path that was itself encoded as one component or one that had backslashes), and `:` kept unless `colon`.
fn percent_encode(text: &str, colon: bool, slash: Option<&str>) -> String {
    let mut out = String::with_capacity(text.len());
    for &b in text.as_bytes() {
        if b == b'/'
            && let Some(slash) = slash
        {
            out.push_str(slash);
        } else if b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/')
            || (b == b':' && !colon)
        {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 15)]));
        }
    }
    out
}

/// A `C:/dir` path as its lower-case drive letter and the rest from the first slash (`c`, `/dir`).
fn drive_path(slashed: &str) -> Option<(String, &str)> {
    let b = slashed.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/' {
        Some((
            char::from(b[0].to_ascii_lowercase()).to_string(),
            &slashed[2..],
        ))
    } else {
        None
    }
}

/// The punctuation a short name keeps besides letters and digits.
const SHORT_PUNCTUATION: &str = "-_!#$%&'()@^`{}~";

fn is_short_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || SHORT_PUNCTUATION.contains(c)
}

/// Windows' 8.3 short name of a long one: the first six characters of its name that a short name keeps (letters, digits and
/// the punctuation of [`SHORT_PUNCTUATION`]; spaces and dots are dropped), upper case, and `~N`, with the first three of its
/// extension. A name that already fits 8.3 has none.
fn short_name(component: &str, number: u8) -> Option<String> {
    // Leading dots are dropped (`.config` is `CONFIG~1`), and a name that had any does not fit 8.3.
    let trimmed = component.trim_start_matches('.');
    let (stem, ext) = trimmed.rsplit_once('.').unwrap_or((trimmed, ""));
    let fits = trimmed.len() == component.len()
        && !stem.is_empty()
        && stem.chars().count() <= 8
        && ext.chars().count() <= 3
        && !stem.contains('.')
        && stem.chars().chain(ext.chars()).all(is_short_char);
    if fits {
        return None;
    }
    let head: String = stem
        .chars()
        .filter(|&c| is_short_char(c))
        .take(6)
        .collect::<String>()
        .to_ascii_uppercase();
    if head.is_empty() {
        return None;
    }
    let ext: String = ext
        .chars()
        .filter(|&c| is_short_char(c))
        .take(3)
        .collect::<String>()
        .to_ascii_uppercase();
    Some(if ext.is_empty() {
        format!("{head}~{number}")
    } else {
        format!("{head}~{number}.{ext}")
    })
}

/// The most short-name spellings of one folder: four long components with four numbers each would be 624.
const MAX_SHORT_SPELLINGS: usize = 256;

/// A `C:/a/b` path with each long component in turn (and, for the first few, each number) as its short name, the long ones
/// that are not shortened kept: `C:/HOME/MYNAME~1/...`, `C:/Home/MYNAME~2/...`. At most [`MAX_SHORT_SPELLINGS`], the ones
/// that use the lowest numbers first (a short name is `~1` unless another file took it).
fn short_forms(slashed: &str) -> Vec<String> {
    let parts: Vec<&str> = slashed.split('/').collect();
    let shortened: Vec<usize> = (1..parts.len())
        .filter(|&i| short_name(parts[i], 1).is_some())
        .collect();
    if shortened.is_empty() {
        return Vec::new();
    }
    let numbers: &[u8] = if shortened.len() <= MAX_SHORT_COMPONENTS {
        &SHORT_NUMBERS
    } else {
        &SHORT_NUMBERS[..1]
    };
    // Each component either long (0) or short with one of the numbers (1..): (numbers + 1) ^ shortened choices, less the
    // all-long spelling, the ones with the lowest numbers first.
    let mut choices: Vec<Vec<usize>> = Vec::new();
    let mut choice = vec![0usize; shortened.len()];
    'count: loop {
        let mut k = 0;
        loop {
            if k == choice.len() {
                break 'count;
            }
            choice[k] += 1;
            if choice[k] <= numbers.len() {
                break;
            }
            choice[k] = 0;
            k += 1;
        }
        choices.push(choice.clone());
    }
    choices.sort_by_key(|c| (c.iter().copied().max(), c.iter().sum::<usize>()));
    choices.truncate(MAX_SHORT_SPELLINGS);
    choices
        .into_iter()
        .map(|choice| {
            let mut next: Vec<String> = parts.iter().map(|s| (*s).to_string()).collect();
            for (k, &i) in shortened.iter().enumerate() {
                if choice[k] > 0
                    && let Some(short) = short_name(parts[i], numbers[choice[k] - 1])
                {
                    next[i] = short;
                }
            }
            next.join("/")
        })
        .collect()
}

/// The needles of a bundle, ready to look for in many files: one Aho-Corasick automaton over every spelling.
pub struct Scanner {
    needles: Vec<Needle>,
    automaton: AhoCorasick,
    /// For each pattern of the automaton: its needle and its encoding.
    patterns: Vec<(usize, Encoding)>,
    /// The most bytes inflated for one file, over everything inside it.
    pub max_inflated: usize,
}

impl Scanner {
    /// A scanner for `needles`, inflating at most `max_inflated` bytes for each file ([`MAX_INFLATED_BYTES`] by default).
    ///
    /// # Errors
    /// When the automaton for the spellings cannot be built.
    pub fn new(needles: Vec<Needle>, max_inflated: usize) -> Result<Scanner, String> {
        let mut patterns = Vec::new();
        let mut bytes: Vec<Vec<u8>> = Vec::new();
        for (index, needle) in needles.iter().enumerate() {
            for spelling in &needle.spellings {
                let lower = spelling.to_ascii_lowercase();
                for encoding in ENCODINGS {
                    let form = encoding.encode(&lower);
                    if !patterns.iter().zip(&bytes).any(
                        |(&(i, e), b): (&(usize, Encoding), &Vec<u8>)| {
                            i == index && e == encoding && *b == form
                        },
                    ) {
                        patterns.push((index, encoding));
                        bytes.push(form);
                    }
                }
            }
        }
        // Standard matching with overlapping results: a short spelling inside a longer one must not hide the longer.
        let automaton = AhoCorasick::builder()
            .match_kind(MatchKind::Standard)
            .build(&bytes)
            .map_err(|e| format!("the leak scan's patterns: {e}"))?;
        Ok(Scanner {
            needles,
            automaton,
            patterns,
            max_inflated,
        })
    }

    /// `bytes` as the automaton reads them: ASCII upper case folded, every backslash a slash (so `C:\Work` and `c:/work` are
    /// one spelling, in UTF-8 and in either UTF-16 byte order).
    fn normalized(bytes: &[u8]) -> Vec<u8> {
        bytes
            .iter()
            .map(|&b| match b {
                b'\\' => b'/',
                b'A'..=b'Z' => b + 32,
                _ => b,
            })
            .collect()
    }

    /// The needles found in `bytes` (the bytes themselves, not what is compressed inside them), each once, as the label and
    /// whether the hit is only a warning.
    pub fn scan(&self, bytes: &[u8]) -> Vec<(String, bool)> {
        if self.needles.is_empty() {
            return Vec::new();
        }
        let text = Scanner::normalized(bytes);
        let mut found = vec![false; self.needles.len()];
        for m in self.automaton.find_overlapping_iter(&text) {
            let (index, encoding) = self.patterns[m.pattern().as_usize()];
            if found[index] {
                continue;
            }
            if self.needles[index].kind == Kind::Name {
                let sep = encoding.separator();
                let before = text[..m.start()].ends_with(sep);
                let after = text[m.end()..].starts_with(sep);
                if !(before && after) {
                    continue;
                }
            }
            found[index] = true;
        }
        self.needles
            .iter()
            .zip(found)
            .filter(|&(_, hit)| hit)
            .map(|(n, _)| (n.label.clone(), n.kind == Kind::Name))
            .collect()
    }

    /// What is found in the file `bytes` and in what is compressed inside it (module docs).
    pub fn scan_file(&self, bytes: &[u8]) -> Vec<Hit> {
        let mut walk = Walk {
            scanner: self,
            budget: self.max_inflated,
            hits: Vec::new(),
        };
        walk.run(bytes, "", 0);
        let mut seen = std::collections::HashSet::new();
        walk.hits.retain(|hit| seen.insert(hit.clone()));
        walk.hits
    }
}

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
const ZIP_LOCAL: &[u8] = b"PK\x03\x04";
const ZIP_END: &[u8] = b"PK\x05\x06";
const ZIP_CENTRAL: &[u8] = b"PK\x01\x02";

/// The compressed formats the scan does not decode, by their first bytes.
const UNREAD_FORMATS: [(&[u8], &str); 6] = [
    (b"\xfd7zXZ\0", "xz"),
    (b"\x28\xb5\x2f\xfd", "zstd"),
    (b"BZh", "bzip2"),
    (b"7z\xbc\xaf\x27\x1c", "7z"),
    (b"wOFF", "WOFF"),
    (b"wOF2", "WOFF2"),
];

/// The most bytes between the end of a central directory and its end record that a zip may hold (a zip64 end record with
/// extensible data, its locator, a digital signature).
const MAX_GAP_BEFORE_END: usize = 70_000;

/// The plausible zip end records of the file, last first, as the record's position and where its zip ends (after the record and
/// its comment). A record counts only when it points at a central directory, because a program's code holds the four signature
/// bytes as a constant. Which of them are zips of their own, and which belong to a stored member of another, is decided where
/// the zips are opened ([`Walk::zips`]).
fn zip_ends(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut ends: Vec<(usize, usize)> = memmem::find_iter(bytes, ZIP_END)
        .filter_map(|at| Some((at, end_record_end(bytes, at)?)))
        .collect();
    ends.reverse();
    ends
}

/// Where the zip whose end record is at `at` ends, when the record fits in the file, its comment fits too, and the central
/// directory it names ends where the record starts (or its sizes are the zip64 markers, or the zip is empty).
fn end_record_end(bytes: &[u8], at: usize) -> Option<usize> {
    let record = bytes.get(at..at + 22)?;
    let word =
        |i: usize| u32::from_le_bytes([record[i], record[i + 1], record[i + 2], record[i + 3]]);
    let comment = usize::from(u16::from_le_bytes([record[20], record[21]]));
    let end = at + 22 + comment;
    if end > bytes.len() {
        return None;
    }
    let entries = u16::from_le_bytes([record[10], record[11]]);
    let size = usize::try_from(word(12)).ok()?;
    if entries == 0xffff || word(12) == 0xffff_ffff {
        return Some(end);
    }
    if entries == 0 {
        return (size == 0).then_some(end);
    }
    // The central directory ends at the record, or a little before it: a zip64 end record and its locator, or a digital
    // signature, may sit between them. A reader that cannot follow them will not open the zip, which is reported.
    let directory = at.checked_sub(size)?;
    let window = bytes.get(directory.saturating_sub(MAX_GAP_BEFORE_END)..directory + 4)?;
    memmem::find(window, ZIP_CENTRAL).map(|_| end)
}

/// A file being scanned: the scanner, what is left of the file's inflate budget, and the hits so far.
struct Walk<'a> {
    scanner: &'a Scanner,
    budget: usize,
    hits: Vec<Hit>,
}

impl Walk<'_> {
    fn unscannable(&mut self, member: &str, why: &str) {
        self.hits.push(Hit::Unscannable {
            member: member.to_string(),
            why: why.to_string(),
        });
    }

    /// Looks at `bytes` (found at `member`, `depth` containers down) and inside what they hold.
    fn run(&mut self, bytes: &[u8], member: &str, depth: usize) {
        for (label, soft) in self.scanner.scan(bytes) {
            self.hits.push(Hit::Leak {
                member: member.to_string(),
                label,
                soft,
            });
        }
        let unread = UNREAD_FORMATS
            .iter()
            .find(|(magic, _)| bytes.starts_with(magic))
            .map(|&(_, name)| name);
        let zips = zip_ends(bytes);
        let zip = bytes.starts_with(ZIP_LOCAL) || !zips.is_empty();
        let gzip = bytes.starts_with(&[0x1f, 0x8b]);
        let png = bytes.starts_with(&PNG_SIGNATURE);
        let zlib = bytes.starts_with(&[0x78, 0x01])
            || bytes.starts_with(&[0x78, 0x9c])
            || bytes.starts_with(&[0x78, 0xda])
            || bytes.starts_with(&[0x78, 0x5e]);
        if let Some(name) = unread {
            self.unscannable(
                member,
                &format!("it is {name}, which the scan does not read"),
            );
        }
        if !(zip || gzip || png || zlib) {
            return;
        }
        if depth >= MAX_DEPTH {
            // A container this deep is not opened: say so rather than pass what it may hold.
            self.unscannable(member, "it is nested too deep");
            return;
        }
        // Every detector runs: a file can be a zip and a PNG at once (a polyglot), and each is looked into.
        if zip {
            self.zips(bytes, &zips, member, depth);
        }
        if gzip {
            let inner = self.inflate(flate2::read::MultiGzDecoder::new(bytes));
            self.inside(member, "gunzip", inner, depth);
        }
        if png {
            self.png(bytes, member, depth);
        }
        if zlib {
            // 78 5E is also the start of ASCII text ("x^"): only a stream that inflates counts there.
            let inner = self.inflate(flate2::read::ZlibDecoder::new(bytes));
            if inner.is_ok() || bytes[1] != 0x5e {
                self.inside(member, "zlib", inner, depth);
            }
        }
    }

    /// Scans what was found inside `member` as `name`, or reports why it could not be read.
    fn inside(
        &mut self,
        member: &str,
        name: &str,
        inner: Result<Vec<u8>, &'static str>,
        depth: usize,
    ) {
        let at = format!("{member}!{name}");
        match inner {
            Ok(inner) => self.run(&inner, &at, depth + 1),
            Err(why) => self.unscannable(&at, why),
        }
    }

    /// Reads `source` to its end, at most what is left of the file's budget (which it pays down).
    fn inflate(&mut self, source: impl Read) -> Result<Vec<u8>, &'static str> {
        let left = self.budget;
        let mut out = Vec::new();
        source
            .take(u64::try_from(left).unwrap_or(u64::MAX).saturating_add(1))
            .read_to_end(&mut out)
            .map_err(|_| "it does not inflate")?;
        if out.len() > left {
            self.budget = 0;
            return Err("it inflates past the scan's limit");
        }
        self.budget = left - out.len();
        Ok(out)
    }

    /// Every zip of the file: each plausible end record (`ends`, last first; none when only a local header says it is a zip) opens
    /// a zip of its own, none skipped. A record inside a stored member of another zip is then found twice (as the member's own
    /// file and as a zip of this one), and the repeated hits are dropped afterwards ([`Scanner::scan_file`]): skipping a record
    /// because a range holds it would let an entry the scan does not read (a directory entry, a short read) hide a zip.
    fn zips(&mut self, bytes: &[u8], ends: &[(usize, usize)], member: &str, depth: usize) {
        if ends.is_empty() {
            self.zip(bytes, "", member, depth);
            return;
        }
        let many = ends.len() > 1;
        for (k, &(_, end)) in ends.iter().enumerate() {
            let prefix = if many {
                format!("zip{}/", ends.len() - k)
            } else {
                String::new()
            };
            self.zip(&bytes[..end], &prefix, member, depth);
        }
    }

    /// The members of one zip, each named `prefix` and its own name (a name is printed only when it holds none of the needles).
    fn zip(&mut self, bytes: &[u8], prefix: &str, member: &str, depth: usize) {
        let mut zip = match zip::ZipArchive::new(Cursor::new(bytes)) {
            Ok(zip) => zip,
            Err(_) => {
                let at = format!("{prefix}zip");
                self.unscannable(
                    &format!("{member}!{at}"),
                    "it looks like a zip and does not open",
                );
                return;
            }
        };
        if zip.len() > MAX_MEMBERS {
            self.unscannable(
                &format!("{member}!{prefix}members past the first {MAX_MEMBERS}"),
                "it has more members than the scan reads",
            );
        }
        for i in 0..zip.len().min(MAX_MEMBERS) {
            let Ok(entry) = zip.by_index(i) else {
                self.unscannable(&format!("{member}!{prefix}member {i}"), "it does not open");
                continue;
            };
            if entry.is_dir() {
                continue;
            }
            let name = if self.scanner.scan(entry.name_raw()).is_empty() {
                String::from_utf8_lossy(entry.name_raw()).into_owned()
            } else {
                format!("member {i}")
            };
            let inner = self.inflate(entry);
            self.inside(member, &format!("{prefix}{name}"), inner, depth);
        }
    }

    /// The inflated text of each compressed text chunk of the PNG `bytes`: `zTXt`, `iTXt` (when its flag says compressed) and
    /// `iCCP`. A chunk cut short, a text chunk whose fields do not parse and a file with no `IEND` are each reported.
    fn png(&mut self, bytes: &[u8], member: &str, depth: usize) {
        let mut at = 8usize;
        loop {
            // length, type, data, CRC
            let Some(head) = bytes.get(at..at.saturating_add(8)) else {
                self.unscannable(&format!("{member}!PNG"), "it ends before its IEND chunk");
                return;
            };
            let len = usize::try_from(u32::from_be_bytes([head[0], head[1], head[2], head[3]]))
                .unwrap_or(usize::MAX);
            let kind = &head[4..8];
            let name = format!("{} chunk", String::from_utf8_lossy(kind));
            let end = at.saturating_add(8).saturating_add(len);
            let Some(data) = bytes
                .get(at + 8..end)
                .filter(|_| end.saturating_add(4) <= bytes.len())
            else {
                self.unscannable(&format!("{member}!{name}"), "it is cut short");
                return;
            };
            let stream: Result<Option<&[u8]>, &'static str> = match kind {
                b"IEND" => return,
                // keyword, NUL, method byte, zlib stream
                b"zTXt" | b"iCCP" => data
                    .iter()
                    .position(|&b| b == 0)
                    .and_then(|nul| data.get(nul + 2..))
                    .map(Some)
                    .ok_or("its fields do not parse"),
                // keyword, NUL, flag, method, language, NUL, translated keyword, NUL, text
                b"iTXt" => data
                    .iter()
                    .position(|&b| b == 0)
                    .and_then(|nul| {
                        let (flag, rest) = (*data.get(nul + 1)?, data.get(nul + 3..)?);
                        let lang = rest.iter().position(|&b| b == 0)?;
                        let rest = rest.get(lang + 1..)?;
                        let translated = rest.iter().position(|&b| b == 0)?;
                        match flag {
                            0 => Some(None),
                            1 => Some(Some(&rest[translated + 1..])),
                            _ => None,
                        }
                    })
                    .ok_or("its fields do not parse"),
                _ => Ok(None),
            };
            match stream {
                Ok(Some(stream)) => {
                    let inner = self.inflate(flate2::read::ZlibDecoder::new(stream));
                    self.inside(member, &name, inner, depth);
                }
                Ok(None) => {}
                Err(why) => self.unscannable(&format!("{member}!{name}"), why),
            }
            at = end + 4;
        }
    }
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
