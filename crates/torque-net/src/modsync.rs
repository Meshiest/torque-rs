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
use crate::manifest::is_name;

/// The section format this build writes and reads.
pub const SECTION_FORMAT: u8 = 1;

/// The largest encoded section.
pub const MAX_SECTION_BYTES: usize = 32;

/// The most mods a server requires.
pub const MAX_REQUIRED: usize = 32;

/// The longest version text, in bytes.
pub const MAX_VERSION_BYTES: usize = 24;

/// A mod a server requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredMod {
    /// The mod's id.
    pub id: String,
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
