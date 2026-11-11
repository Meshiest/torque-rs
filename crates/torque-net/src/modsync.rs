//! Required mods in the handshake: what a client says about content transfer, and the list a server requires.
//!
//! **Nothing about a client's mods goes to the server.** A client of a game that shares mods adds a [`ModsSection`] to its connect
//! block that says only that it can take content from the server and the most it will take (bytes and files). It does not list the
//! mods it has installed or cached. After the accept the server sends the mods it requires over the mods channel, and the client
//! decides what to fetch (see the mods crate).
//!
//! The gate's only part is the protocol's: a server that requires mods refuses a client with no section, which cannot take them
//! ([`RequiredMods`] holds the server's list for that check).
//!
//! **Bounded.** The section is at most [`MAX_SECTION_BYTES`] encoded; its decoder reads hostile bytes into an `Err`, never a panic.

use std::sync::{Arc, Mutex};

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::manifest::{is_invisible, is_name};

/// The section format this build writes and reads.
pub const SECTION_FORMAT: u8 = 1;

/// The largest encoded section.
pub const MAX_SECTION_BYTES: usize = 32;

/// The most mods a server requires.
pub const MAX_REQUIRED: usize = 32;

/// The longest version text, in bytes.
pub const MAX_VERSION_BYTES: usize = 24;

/// The longest display name, in bytes.
pub const MAX_DISPLAY_NAME_BYTES: usize = 64;

/// The most combining marks in a row in a display name.
pub const MAX_COMBINING_RUN: usize = 2;

/// The most combining marks in all in a display name.
pub const MAX_COMBINING_TOTAL: usize = 8;

/// A mod a server requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredMod {
    /// The mod's id.
    pub id: String,
    /// The name a client shows for the mod (see [`is_display_name`]; the id when the mod names itself nothing else).
    pub name: String,
    /// Its version, as the mod writes it.
    pub version: String,
    /// The SHA-256 of its content (see the mods crate for what is hashed, in which order).
    pub hash: [u8; 32],
    /// The bytes of its content.
    pub bytes: u64,
    /// How many files its content has.
    pub files: u32,
    /// Whether the server sends the content to a client that lacks it.
    pub download: bool,
}

/// What a client tells a server about content transfer, in its connect block: that it takes content, and the most it will.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModsSection {
    /// The most bytes the client takes from one server.
    pub max_bytes: u64,
    /// The most files the client takes from one server.
    pub max_files: u32,
}

impl ModsSection {
    /// The section on the wire.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.write_u8(SECTION_FORMAT);
        w.write_u32(self.max_bytes as u32);
        w.write_u32((self.max_bytes >> 32) as u32);
        w.write_u32(self.max_files);
        w.into_bytes()
    }

    /// Reads a section off `bytes`, all of them. Hostile input is an `Err`.
    pub fn decode(bytes: &[u8]) -> Result<ModsSection, NetError> {
        if bytes.len() > MAX_SECTION_BYTES {
            return Err(NetError::Cap {
                what: "mods section",
                count: bytes.len() as u64,
                cap: MAX_SECTION_BYTES as u64,
            });
        }
        let mut r = BitReader::new(bytes);
        if r.read_u8()? != SECTION_FORMAT {
            return Err(NetError::Invalid("mods section format"));
        }
        let low = u64::from(r.read_u32()?);
        let high = u64::from(r.read_u32()?);
        let max_files = r.read_u32()?;
        if r.bits_left() >= 8 {
            return Err(NetError::Invalid("trailing bytes"));
        }
        Ok(ModsSection {
            max_bytes: low | (high << 32),
            max_files,
        })
    }
}

/// Whether `text` is a version a server may announce: [`MAX_VERSION_BYTES`] at most, a name's characters (no control, bidirectional
/// or zero-width ones), or empty.
pub fn is_version(text: &str) -> bool {
    text.len() <= MAX_VERSION_BYTES && (text.is_empty() || is_name(text))
}

/// Whether `text` is a name a client may show for a mod a server sent: 1 to [`MAX_DISPLAY_NAME_BYTES`] bytes of printable
/// characters (no control, bidirectional or zero-width ones), no `<` or `>` (a name is never markup), no space at either end, and
/// not a name that looks empty or overflows its label: no blank-looking filler characters (Hangul fillers, the blank braille
/// pattern and the like), it starts with a character a mark can sit on, and it has at most [`MAX_COMBINING_RUN`] combining marks
/// in a row and [`MAX_COMBINING_TOTAL`] in all.
pub fn is_display_name(text: &str) -> bool {
    if text.is_empty() || text.len() > MAX_DISPLAY_NAME_BYTES || text.trim() != text {
        return false;
    }
    let (mut run, mut total) = (0usize, 0usize);
    for (at, c) in text.chars().enumerate() {
        if is_invisible(c) || is_filler(c) || c == '<' || c == '>' {
            return false;
        }
        if is_combining(c) {
            run += 1;
            total += 1;
            if at == 0 || run > MAX_COMBINING_RUN || total > MAX_COMBINING_TOTAL {
                return false;
            }
        } else {
            run = 0;
        }
    }
    true
}

/// A character that shows as blank though it is not white space, so a name of it could look empty.
fn is_filler(c: char) -> bool {
    matches!(
        u32::from(c),
        0x034f | 0x115f | 0x1160 | 0x17b4 | 0x17b5 | 0x2800 | 0x3164 | 0xffa0
    )
}

/// A mark that draws on the character before it (the main combining blocks, and the variation selectors).
fn is_combining(c: char) -> bool {
    matches!(
        u32::from(c),
        0x0300..=0x036f
            | 0x0483..=0x0489
            | 0x0591..=0x05bd
            | 0x05bf
            | 0x05c1..=0x05c2
            | 0x05c4..=0x05c5
            | 0x05c7
            | 0x0610..=0x061a
            | 0x064b..=0x065f
            | 0x0670
            | 0x06d6..=0x06dc
            | 0x06df..=0x06e4
            | 0x06e7..=0x06e8
            | 0x06ea..=0x06ed
            | 0x180b..=0x180d
            | 0x1ab0..=0x1aff
            | 0x1dc0..=0x1dff
            | 0x20d0..=0x20ff
            | 0x3099..=0x309a
            | 0xfe00..=0xfe0f
            | 0xfe20..=0xfe2f
            | 0xe0100..=0xe01ef
    )
}

/// The mods a server requires, shared between the gate that judges clients with them and whatever sets them once the server's mods
/// are loaded (the gate exists before the mods do). A clone shares the list.
#[derive(Debug, Clone, Default)]
pub struct RequiredMods(Arc<Mutex<Vec<RequiredMod>>>);

impl RequiredMods {
    /// Replaces the list (at most [`MAX_REQUIRED`] mods, sorted by id; more are dropped).
    pub fn set(&self, mut mods: Vec<RequiredMod>) {
        mods.sort_by(|a, b| a.id.cmp(&b.id));
        mods.dedup_by(|a, b| a.id == b.id);
        mods.truncate(MAX_REQUIRED);
        if let Ok(mut held) = self.0.lock() {
            *held = mods;
        }
    }

    /// The list now.
    pub fn get(&self) -> Vec<RequiredMod> {
        self.0.lock().map(|l| l.clone()).unwrap_or_default()
    }

    /// Whether no mod is required.
    pub fn is_empty(&self) -> bool {
        self.0.lock().is_ok_and(|l| l.is_empty())
    }
}

/// A size as a person reads it (`3.5 MiB`).
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["bytes", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
#[path = "modsync_tests.rs"]
mod tests;
