//! The connection manifest: what a peer's build of a game puts on the wire, compared when a client connects so that two peers whose
//! network classes could disagree never get as far as a ghost or a datablock.
//!
//! A manifest lists the plugins that have a network version (id and version, sorted by id), the handshake's protocol versions, the
//! sorted names of the ghost classes, the datablock classes and the bulk channels (each table as a count and a 64-bit digest, and the
//! names themselves when they fit), and a short list of mods by name and content hash ([`Manifest::mods`]; the full list a server
//! requires, with versions, sizes and downloads, is [`crate::modsync`]). The server compares the client's
//! manifest with its own ([`Manifest::mismatch`]) and refuses a client that differs with one line of reason, for the client to show
//! ([`Mismatch`]'s `Display`): `server needs plugin torque.rope 2, you have 1`.
//!
//! **Bounded.** A manifest is at most [`MAX_MANIFEST_BYTES`] encoded, so the connect request stays well inside a datagram
//! ([`crate::cap::NET_MAX_DATAGRAM`]). Names are listed only while that fits; a table too large for it travels as its count and
//! digest, and a mismatch in it says that the tables differ and how many entries each side has. Every list on the wire has its own
//! cap ([`MAX_PLUGINS`], [`MAX_NAMES_LISTED`], [`MAX_MODS`]) and is checked against the bytes left before anything is reserved; a
//! decoder never panics ([`Manifest::decode`]).
//!
//! **Canonical.** Plugins, names and mods are sorted and unique on the wire and the decoder insists on it, as it does on a digest that
//! matches the names beside it, so one manifest has one encoding.

use std::collections::BTreeSet;
use std::fmt;

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::cap;
use crate::handshake::siphash24;

/// The manifest format this build writes and reads.
pub const MANIFEST_FORMAT: u8 = 1;

/// The format a manifest with an RPC table is written in (format 1, then the RPC table after the mods). A manifest with no RPCs
/// stays in format 1, so peers that register none keep the bytes they always had.
pub const MANIFEST_FORMAT_RPC: u8 = 2;

/// The largest encoded manifest a peer sends or a server reads.
pub const MAX_MANIFEST_BYTES: usize = 768;

/// The most plugins a manifest lists.
pub const MAX_PLUGINS: usize = 64;

/// The longest plugin id or name, in bytes.
pub const MAX_NAME_BYTES: usize = 64;

/// The most names one table lists (a table with more travels as its count and digest).
pub const MAX_NAMES_LISTED: usize = 256;

/// The most entries a table may claim: a count, not a list.
pub const MAX_TABLE_ENTRIES: u32 = 1 << 20;

/// The most mods a manifest lists.
pub const MAX_MODS: usize = 8;

/// The key of the digest of a name table: fixed, since the digest is a checksum, not a secret.
const DIGEST_KEY: [u8; 16] = *b"torque-manifest!";

/// Why a manifest could not be built, written or read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// A field is outside what a manifest allows (`what` says which).
    Invalid(&'static str),
    /// The manifest is written by a format this build does not read.
    Format(u8),
    /// More than [`MAX_MANIFEST_BYTES`] encoded, even with no names listed.
    TooLarge(usize),
    /// A count or a length above its cap.
    Cap(NetError),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Invalid(what) => write!(f, "invalid manifest ({what})"),
            ManifestError::Format(v) => write!(
                f,
                "manifest format {v} is not supported (this side speaks {MANIFEST_FORMAT} and {MANIFEST_FORMAT_RPC})"
            ),
            ManifestError::TooLarge(n) => {
                write!(
                    f,
                    "manifest of {n} bytes is over the limit of {MAX_MANIFEST_BYTES}"
                )
            }
            ManifestError::Cap(e) => write!(f, "invalid manifest ({e})"),
        }
    }
}

impl std::error::Error for ManifestError {}

impl From<NetError> for ManifestError {
    fn from(error: NetError) -> ManifestError {
        match error {
            NetError::Invalid(what) => ManifestError::Invalid(what),
            other => ManifestError::Cap(other),
        }
    }
}

/// A plugin with a network version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginEntry {
    /// The plugin's stable id.
    pub id: String,
    /// Its network version (a plugin raises it when its wire form changes).
    pub version: u32,
}

/// The manifest entry of an RPC: its name, its direction (`s` for a call a client makes, `c` for one the server makes) and a digest
/// of the layouts of its request and reply, so a peer whose RPC differs in any of the three has another entry.
pub fn rpc_entry(name: &str, to_server: bool, digest: u64) -> String {
    format!("{name}:{}:{digest:016x}", if to_server { 's' } else { 'c' })
}

/// The RPC name of a manifest entry ([`rpc_entry`]).
fn rpc_entry_name(entry: &str) -> &str {
    entry.split(':').next().unwrap_or(entry)
}

/// A required mod: its name and the hash of its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModEntry {
    /// The mod's name.
    pub name: String,
    /// The hash of its files.
    pub hash: [u8; 32],
}

/// A sorted set of names (a ghost class table, a datablock class table, a channel table), kept as a count and a digest, with the
/// names themselves when they fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameTable {
    count: u32,
    digest: u64,
    names: Option<Vec<String>>,
}

impl Default for NameTable {
    fn default() -> NameTable {
        NameTable::new(std::iter::empty::<&str>())
    }
}

fn digest_of<'a>(names: impl Iterator<Item = &'a str>) -> u64 {
    let mut bytes = Vec::new();
    for name in names {
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
    }
    siphash24(&DIGEST_KEY, &bytes)
}

/// Whether `c` could hide or reorder text on a screen: a control character, a line or paragraph separator, or a format character
/// (Unicode category Cf: the bidirectional overrides and isolates, zero-width characters, the invisible operators, tags).
pub(crate) fn is_invisible(c: char) -> bool {
    c.is_control()
        || matches!(
            u32::from(c),
            0x00ad
                | 0x0600..=0x0605
                | 0x061c
                | 0x06dd
                | 0x070f
                | 0x0890..=0x0891
                | 0x08e2
                | 0x180e
                | 0x200b..=0x200f
                | 0x2028..=0x202e
                | 0x2060..=0x2064
                | 0x2066..=0x206f
                | 0xfeff
                | 0xfff9..=0xfffb
                | 0x110bd
                | 0x110cd
                | 0x13430..=0x1343f
                | 0x1bca0..=0x1bca3
                | 0x1d173..=0x1d17a
                | 0xe0001
                | 0xe0020..=0xe007f
        )
}

/// Whether `text` is something a manifest may carry as an id or a name: 1 to [`MAX_NAME_BYTES`] bytes, no invisible
/// characters (control, bidirectional and zero-width ones), so a name shown in a refusal reason cannot be reordered or hidden.
pub fn is_name(text: &str) -> bool {
    !text.is_empty() && text.len() <= MAX_NAME_BYTES && !text.chars().any(is_invisible)
}

impl NameTable {
    /// The table of `names`: sorted by byte order, each once. The names are kept for listing unless there are more than
    /// [`MAX_NAMES_LISTED`] or one is not a [valid name](MAX_NAME_BYTES); the digest covers all of them.
    pub fn new<I>(names: I) -> NameTable
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let set: BTreeSet<String> = names.into_iter().map(|n| n.as_ref().to_string()).collect();
        let count = u32::try_from(set.len()).unwrap_or(u32::MAX);
        let digest = digest_of(set.iter().map(String::as_str));
        let listable = set.len() <= MAX_NAMES_LISTED && set.iter().all(|n| is_name(n));
        NameTable {
            count,
            digest,
            names: listable.then(|| set.into_iter().collect()),
        }
    }

    /// How many names.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// The digest of the sorted names.
    pub fn digest(&self) -> u64 {
        self.digest
    }

    /// The sorted names, when this table has them (one read off the wire may not).
    pub fn names(&self) -> Option<&[String]> {
        self.names.as_deref()
    }

    /// The same count and digest: the same names.
    pub fn same_as(&self, other: &NameTable) -> bool {
        self.count == other.count && self.digest == other.digest
    }

    /// The same table without its names.
    fn without_names(&self) -> NameTable {
        NameTable {
            names: None,
            ..self.clone()
        }
    }
}

/// What a peer's build says about its network surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// The oldest handshake protocol this peer accepts.
    pub protocol_min: u32,
    /// The newest handshake protocol this peer speaks.
    pub protocol_current: u32,
    /// The plugins that have a network version, sorted by id.
    pub plugins: Vec<PluginEntry>,
    /// The ghost classes.
    pub ghosts: NameTable,
    /// The datablock classes.
    pub datablocks: NameTable,
    /// The bulk channels.
    pub channels: NameTable,
    /// The RPCs plugins registered, each as an [`rpc_entry`]; a manifest that lists any is written in [`MANIFEST_FORMAT_RPC`].
    pub rpcs: NameTable,
    /// Mods by name and content hash, sorted by name, checked by [`Manifest::mismatch`] (a server lists the ones it needs, a client
    /// the ones it has). The engine leaves it empty and uses [`crate::modsync`] instead, which also carries versions, sizes and
    /// downloads.
    pub mods: Vec<ModEntry>,
}

impl Manifest {
    /// A manifest of the given protocol versions with nothing else in it.
    pub fn new(protocol_min: u32, protocol_current: u32) -> Manifest {
        Manifest {
            protocol_min,
            protocol_current,
            plugins: Vec::new(),
            ghosts: NameTable::default(),
            datablocks: NameTable::default(),
            channels: NameTable::default(),
            rpcs: NameTable::default(),
            mods: Vec::new(),
        }
    }

    /// Adds a plugin (a second entry for an id replaces the first).
    #[must_use]
    pub fn with_plugin(mut self, id: impl Into<String>, version: u32) -> Manifest {
        let id = id.into();
        self.plugins.retain(|p| p.id != id);
        self.plugins.push(PluginEntry { id, version });
        self.plugins.sort_by(|a, b| a.id.cmp(&b.id));
        self
    }

    /// Sets the ghost class table.
    #[must_use]
    pub fn with_ghosts<I>(mut self, names: I) -> Manifest
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        self.ghosts = NameTable::new(names);
        self
    }

    /// Sets the datablock class table.
    #[must_use]
    pub fn with_datablocks<I>(mut self, names: I) -> Manifest
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        self.datablocks = NameTable::new(names);
        self
    }

    /// Sets the channel table.
    #[must_use]
    pub fn with_channels<I>(mut self, names: I) -> Manifest
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        self.channels = NameTable::new(names);
        self
    }

    /// Sets the RPC table: one [`rpc_entry`] for each RPC.
    #[must_use]
    pub fn with_rpcs<I>(mut self, entries: I) -> Manifest
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        self.rpcs = NameTable::new(entries);
        self
    }

    /// Adds a required mod (a second entry for a name replaces the first).
    #[must_use]
    pub fn with_mod(mut self, name: impl Into<String>, hash: [u8; 32]) -> Manifest {
        let name = name.into();
        self.mods.retain(|m| m.name != name);
        self.mods.push(ModEntry { name, hash });
        self.mods.sort_by(|a, b| a.name.cmp(&b.name));
        self
    }

    /// Whether the manifest says nothing beyond its protocol versions: no plugin, no name, no mod.
    pub fn is_bare(&self) -> bool {
        self.plugins.is_empty()
            && self.mods.is_empty()
            && self.ghosts.count == 0
            && self.datablocks.count == 0
            && self.channels.count == 0
            && self.rpcs.count == 0
    }

    /// What a peer that sent no manifest says: this one's protocol versions and nothing else.
    pub fn bare_like(&self) -> Manifest {
        Manifest::new(self.protocol_min, self.protocol_current)
    }

    /// The manifest on the wire: its names are listed table by table (ghosts, datablocks, channels) while the whole stays within
    /// [`MAX_MANIFEST_BYTES`]; a table that does not fit goes as its count and digest.
    pub fn encode(&self) -> Result<Vec<u8>, ManifestError> {
        self.check()?;
        let mut tables = [
            self.ghosts.without_names(),
            self.datablocks.without_names(),
            self.channels.without_names(),
            self.rpcs.without_names(),
        ];
        let mut best = self.write(&tables);
        if best.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::TooLarge(best.len()));
        }
        for (i, full) in [&self.ghosts, &self.datablocks, &self.channels, &self.rpcs]
            .into_iter()
            .enumerate()
        {
            if full.names.is_none() {
                continue;
            }
            let kept = std::mem::replace(&mut tables[i], full.clone());
            let attempt = self.write(&tables);
            if attempt.len() <= MAX_MANIFEST_BYTES {
                best = attempt;
            } else {
                tables[i] = kept;
            }
        }
        Ok(best)
    }

    /// The limits of what may be written: the lists' sizes and the names' form.
    fn check(&self) -> Result<(), ManifestError> {
        if self.plugins.len() > MAX_PLUGINS {
            return Err(ManifestError::Invalid("too many plugins"));
        }
        if self.mods.len() > MAX_MODS {
            return Err(ManifestError::Invalid("too many mods"));
        }
        if self.plugins.iter().any(|p| !is_name(&p.id)) {
            return Err(ManifestError::Invalid("plugin id"));
        }
        if self.mods.iter().any(|m| !is_name(&m.name)) {
            return Err(ManifestError::Invalid("mod name"));
        }
        let sorted = |ids: Vec<&str>| ids.windows(2).all(|w| w[0] < w[1]);
        if !sorted(self.plugins.iter().map(|p| p.id.as_str()).collect())
            || !sorted(self.mods.iter().map(|m| m.name.as_str()).collect())
        {
            return Err(ManifestError::Invalid("order"));
        }
        Ok(())
    }

    fn write(&self, tables: &[NameTable; 4]) -> Vec<u8> {
        let mut w = BitWriter::new();
        let with_rpcs = self.rpcs.count > 0;
        w.write_u8(if with_rpcs {
            MANIFEST_FORMAT_RPC
        } else {
            MANIFEST_FORMAT
        });
        w.write_u32(self.protocol_min);
        w.write_u32(self.protocol_current);
        w.write_u8(self.plugins.len() as u8);
        for plugin in &self.plugins {
            write_text(&mut w, &plugin.id);
            w.write_u32(plugin.version);
        }
        for table in &tables[..3] {
            write_table(&mut w, table);
        }
        w.write_u8(self.mods.len() as u8);
        for entry in &self.mods {
            write_text(&mut w, &entry.name);
            w.write_bytes(&entry.hash);
        }
        if with_rpcs {
            write_table(&mut w, &tables[3]);
        }
        w.into_bytes()
    }

    /// Reads a manifest off `bytes`, all of them. Hostile input is an `Err`, never a panic and never a large allocation: more than
    /// [`MAX_MANIFEST_BYTES`] is refused unread, each list is held to its cap and to what the bytes left could hold, the plugins,
    /// names and mods must be sorted and unique, and a listed table's digest must be the names' own.
    pub fn decode(bytes: &[u8]) -> Result<Manifest, ManifestError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::TooLarge(bytes.len()));
        }
        let mut r = BitReader::new(bytes);
        let format = r.read_u8()?;
        if format != MANIFEST_FORMAT && format != MANIFEST_FORMAT_RPC {
            return Err(ManifestError::Format(format));
        }
        let protocol_min = r.read_u32()?;
        let protocol_current = r.read_u32()?;
        let plugin_count = cap::count(
            "manifest plugins",
            u64::from(r.read_u8()?),
            MAX_PLUGINS,
            r.bits_left(),
            // a length byte, one character, a version
            8 + 8 + 32,
        )?;
        let mut plugins = Vec::with_capacity(plugin_count);
        for _ in 0..plugin_count {
            let id = read_text(&mut r)?;
            let version = r.read_u32()?;
            plugins.push(PluginEntry { id, version });
        }
        if !plugins.windows(2).all(|w| w[0].id < w[1].id) {
            return Err(ManifestError::Invalid("plugin order"));
        }
        let ghosts = read_table(&mut r)?;
        let datablocks = read_table(&mut r)?;
        let channels = read_table(&mut r)?;
        let mod_count = cap::count(
            "manifest mods",
            u64::from(r.read_u8()?),
            MAX_MODS,
            r.bits_left(),
            16 + 256,
        )?;
        let mut mods = Vec::with_capacity(mod_count);
        for _ in 0..mod_count {
            let name = read_text(&mut r)?;
            let mut hash = [0u8; 32];
            for byte in &mut hash {
                *byte = r.read_u8()?;
            }
            mods.push(ModEntry { name, hash });
        }
        if !mods.windows(2).all(|w| w[0].name < w[1].name) {
            return Err(ManifestError::Invalid("mod order"));
        }
        let rpcs = if format == MANIFEST_FORMAT_RPC {
            let table = read_table(&mut r)?;
            // Format 2 exists to carry an RPC table: one without any is not the one encoding of its manifest.
            if table.count == 0 {
                return Err(ManifestError::Invalid("empty rpc table"));
            }
            table
        } else {
            NameTable::default()
        };
        if r.bits_left() >= 8 {
            return Err(ManifestError::Invalid("trailing bytes"));
        }
        Ok(Manifest {
            protocol_min,
            protocol_current,
            plugins,
            ghosts,
            datablocks,
            channels,
            rpcs,
            mods,
        })
    }

    /// How this client's manifest differs from the server's (`self`), or `None` when it is what the server needs. The first
    /// difference found, in this order: the protocol versions, the plugins, the ghost classes, the datablock classes, the channels,
    /// the mods.
    pub fn mismatch(&self, client: &Manifest) -> Option<Mismatch> {
        if client.protocol_current < self.protocol_min
            || client.protocol_min > self.protocol_current
        {
            return Some(Mismatch::Protocol {
                server: (self.protocol_min, self.protocol_current),
                client: (client.protocol_min, client.protocol_current),
            });
        }
        for plugin in &self.plugins {
            match client.plugins.iter().find(|p| p.id == plugin.id) {
                None => {
                    return Some(Mismatch::PluginMissing {
                        id: plugin.id.clone(),
                        version: plugin.version,
                    });
                }
                Some(have) if have.version != plugin.version => {
                    return Some(Mismatch::PluginVersion {
                        id: plugin.id.clone(),
                        server: plugin.version,
                        client: have.version,
                    });
                }
                Some(_) => {}
            }
        }
        if let Some(extra) = client
            .plugins
            .iter()
            .find(|p| !self.plugins.iter().any(|s| s.id == p.id))
        {
            return Some(Mismatch::PluginExtra {
                id: extra.id.clone(),
                version: extra.version,
            });
        }
        for (kind, server, theirs) in [
            (TableKind::Ghost, &self.ghosts, &client.ghosts),
            (TableKind::Datablock, &self.datablocks, &client.datablocks),
            (TableKind::Channel, &self.channels, &client.channels),
            (TableKind::Rpc, &self.rpcs, &client.rpcs),
        ] {
            if server.same_as(theirs) {
                continue;
            }
            return Some(table_difference(kind, server, theirs));
        }
        for needed in &self.mods {
            match client.mods.iter().find(|m| m.name == needed.name) {
                None => {
                    return Some(Mismatch::ModMissing {
                        name: needed.name.clone(),
                    });
                }
                Some(have) if have.hash != needed.hash => {
                    return Some(Mismatch::ModDiffers {
                        name: needed.name.clone(),
                    });
                }
                Some(_) => {}
            }
        }
        None
    }
}

fn write_table(w: &mut BitWriter, table: &NameTable) {
    w.write_u32(table.count);
    w.write_u32(table.digest as u32);
    w.write_u32((table.digest >> 32) as u32);
    match &table.names {
        Some(names) => {
            w.write_flag(true);
            w.write_bits(names.len() as u32, 16);
            for name in names {
                write_text(w, name);
            }
        }
        None => w.write_flag(false),
    }
}

fn write_text(w: &mut BitWriter, text: &str) {
    w.write_u8(text.len() as u8);
    w.write_bytes(text.as_bytes());
}

fn read_text(r: &mut BitReader) -> Result<String, ManifestError> {
    let len = usize::from(r.read_u8()?);
    if len == 0 || len > MAX_NAME_BYTES {
        return Err(ManifestError::Invalid("name length"));
    }
    if r.bits_left() < len * 8 {
        return Err(NetError::Overrun.into());
    }
    let bytes = r.read_bytes(len)?;
    let text = String::from_utf8(bytes).map_err(|_| ManifestError::Invalid("name text"))?;
    if !is_name(&text) {
        return Err(ManifestError::Invalid("name text"));
    }
    Ok(text)
}

fn read_table(r: &mut BitReader) -> Result<NameTable, ManifestError> {
    let count = r.read_u32()?;
    if count > MAX_TABLE_ENTRIES {
        return Err(NetError::Cap {
            what: "manifest table entries",
            count: u64::from(count),
            cap: u64::from(MAX_TABLE_ENTRIES),
        }
        .into());
    }
    let digest = u64::from(r.read_u32()?) | (u64::from(r.read_u32()?) << 32);
    if !r.read_flag()? {
        return Ok(NameTable {
            count,
            digest,
            names: None,
        });
    }
    let listed = cap::count(
        "manifest names",
        u64::from(r.read_bits(16)?),
        MAX_NAMES_LISTED,
        r.bits_left(),
        16,
    )?;
    if listed as u32 != count {
        return Err(ManifestError::Invalid("name count"));
    }
    let mut names: Vec<String> = Vec::with_capacity(listed);
    for _ in 0..listed {
        names.push(read_text(r)?);
    }
    if !names.windows(2).all(|w| w[0] < w[1]) {
        return Err(ManifestError::Invalid("name order"));
    }
    if digest_of(names.iter().map(String::as_str)) != digest {
        return Err(ManifestError::Invalid("name digest"));
    }
    Ok(NameTable {
        count,
        digest,
        names: Some(names),
    })
}

/// Which table of the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableKind {
    /// Ghost classes.
    Ghost,
    /// Datablock classes.
    Datablock,
    /// Bulk channels.
    Channel,
    /// RPCs (each entry is a name, a direction and a layout digest).
    Rpc,
}

impl TableKind {
    fn noun(self) -> &'static str {
        match self {
            TableKind::Ghost => "ghost class",
            TableKind::Datablock => "datablock class",
            TableKind::Channel => "channel",
            TableKind::Rpc => "RPC",
        }
    }
}

fn table_difference(kind: TableKind, server: &NameTable, client: &NameTable) -> Mismatch {
    if let (Some(a), Some(b)) = (server.names(), client.names()) {
        if let Some(name) = a.iter().find(|n| b.binary_search(n).is_err()) {
            return Mismatch::NameMissing {
                kind,
                name: name.clone(),
            };
        }
        if let Some(name) = b.iter().find(|n| a.binary_search(n).is_err()) {
            return Mismatch::NameExtra {
                kind,
                name: name.clone(),
            };
        }
    }
    Mismatch::TablesDiffer {
        kind,
        server: server.count,
        client: client.count,
    }
}

/// How a client's manifest differs from what a server needs: its `Display` is the reason the server sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mismatch {
    /// The protocol version ranges do not meet.
    Protocol {
        /// The server's oldest and newest.
        server: (u32, u32),
        /// The client's oldest and newest.
        client: (u32, u32),
    },
    /// The server has a plugin the client lacks.
    PluginMissing {
        /// The plugin's id.
        id: String,
        /// The server's version of it.
        version: u32,
    },
    /// Both have a plugin, at different versions.
    PluginVersion {
        /// The plugin's id.
        id: String,
        /// The server's version.
        server: u32,
        /// The client's version.
        client: u32,
    },
    /// The client has a networked plugin the server lacks.
    PluginExtra {
        /// The plugin's id.
        id: String,
        /// The client's version of it.
        version: u32,
    },
    /// The server has a name the client lacks.
    NameMissing {
        /// The table.
        kind: TableKind,
        /// The name.
        name: String,
    },
    /// The client has a name the server lacks.
    NameExtra {
        /// The table.
        kind: TableKind,
        /// The name.
        name: String,
    },
    /// The tables differ and a name cannot be pointed at (a side did not list its names).
    TablesDiffer {
        /// The table.
        kind: TableKind,
        /// The server's entry count.
        server: u32,
        /// The client's entry count.
        client: u32,
    },
    /// The server requires a mod the client lacks.
    ModMissing {
        /// The mod.
        name: String,
    },
    /// The client's mod is not the server's files.
    ModDiffers {
        /// The mod.
        name: String,
    },
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mismatch::Protocol { server, client } => write!(
                f,
                "server speaks protocol {} to {}, you speak {} to {}",
                server.0, server.1, client.0, client.1
            ),
            Mismatch::PluginMissing { id, version } => {
                write!(f, "server needs plugin {id} {version}, you do not have it")
            }
            Mismatch::PluginVersion { id, server, client } => {
                write!(f, "server needs plugin {id} {server}, you have {client}")
            }
            Mismatch::PluginExtra { id, version } => {
                write!(
                    f,
                    "you have plugin {id} {version}, which this server does not"
                )
            }
            Mismatch::NameMissing {
                kind: TableKind::Rpc,
                name,
            }
            | Mismatch::NameExtra {
                kind: TableKind::Rpc,
                name,
            } => write!(
                f,
                "server and client disagree on RPC {}",
                rpc_entry_name(name)
            ),
            Mismatch::NameMissing { kind, name } => {
                write!(f, "server has {} {name}, you do not", kind.noun())
            }
            Mismatch::NameExtra { kind, name } => {
                write!(
                    f,
                    "you have {} {name}, which this server does not",
                    kind.noun()
                )
            }
            Mismatch::TablesDiffer {
                kind,
                server,
                client,
            } => write!(
                f,
                "{} tables differ (server {server}, you {client})",
                kind.noun()
            ),
            Mismatch::ModMissing { name } => {
                write!(f, "server needs mod {name}, you do not have it")
            }
            Mismatch::ModDiffers { name } => {
                write!(f, "your mod {name} is not the server's")
            }
        }
    }
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
