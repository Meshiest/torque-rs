//! A loaded code block: a DSO file with identifiers resolved and string tables decoded.
//!
//! Follows Torque3D's MIT-licensed `console/torquescript/codeBlock.cpp` (Copyright (c) 2012 GarageGames, LLC); the code
//! here is this crate's own.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::namehash::NameMap;
use std::rc::{Rc, Weak};
use std::sync::LazyLock;

use torque_ts_bytecode::classic::ClassicDialect;
use torque_ts_bytecode::{Dialect, DsoError, DsoFile, StringTable};

use crate::ns::{Entry, EntryRef};
use crate::text::fold;

/// A call site's binding: the entry, weakly (see [`CodeBlock`]'s `sites`).
type WeakEntry = Weak<RefCell<Entry>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    /// Not a file of the dialect's version.
    WrongVersion { found: u32, expected: u32 },
    /// An identifier entry names a string outside the global string table.
    BadIdentifier,
    /// An identifier position lies outside the code.
    BadPosition,
    /// The identifier entries name more string bytes than a real file can: the strings of a real table
    /// are disjoint, so the distinct ones add up to at most the table. Hostile entries that point at many
    /// offsets of one long string (n * length bytes of names to copy and hash) are refused.
    IdentifiersTooLarge,
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::WrongVersion { found, expected } => {
                write!(f, "unsupported DSO version {found} (expected {expected})")
            }
            LoadError::BadIdentifier => {
                write!(
                    f,
                    "identifier table names a string outside the string table"
                )
            }
            LoadError::BadPosition => write!(f, "identifier position lies outside the code"),
            LoadError::IdentifiersTooLarge => {
                write!(
                    f,
                    "identifier table names more string bytes than the string table holds"
                )
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// A dialect's compiled-file format, as far as the runtime reads it: the version word `exec` accepts, the
/// file's layout and how its string tables are stored. The executor names its dialect's
/// ([`Executor::format`](crate::exec::Executor::format)); a [`Dialect`] of torque-ts-bytecode is one, with its
/// string tables stored as they read.
pub trait DsoFormat {
    /// The version word of the dialect's files.
    fn version(&self) -> u32;

    /// Parses a file in the dialect's layout (the version is recorded, not checked).
    fn parse(&self, bytes: &[u8]) -> Result<DsoFile, DsoError>;

    /// The text of the global string table of `dso`.
    fn global_strings(&self, dso: &DsoFile) -> StringTable;

    /// The text of the function string table of `dso`.
    fn function_strings(&self, dso: &DsoFile) -> StringTable;
}

impl DsoFormat for Dialect {
    fn version(&self) -> u32 {
        self.dso_version
    }

    fn parse(&self, bytes: &[u8]) -> Result<DsoFile, DsoError> {
        DsoFile::parse(bytes, self)
    }

    fn global_strings(&self, dso: &DsoFile) -> StringTable {
        dso.global_string_table()
    }

    fn function_strings(&self, dso: &DsoFile) -> StringTable {
        dso.function_string_table()
    }
}

impl DsoFormat for ClassicDialect {
    fn version(&self) -> u32 {
        self.dso_version
    }

    fn parse(&self, bytes: &[u8]) -> Result<DsoFile, DsoError> {
        ClassicDialect::parse(self, bytes)
    }

    fn global_strings(&self, dso: &DsoFile) -> StringTable {
        ClassicDialect::global_strings(self, dso)
    }

    fn function_strings(&self, dso: &DsoFile) -> StringTable {
        ClassicDialect::function_strings(self, dso)
    }
}

/// Torque3D's file format ([`Dialect::torque3d`]), the format of an executor that names none.
pub fn torque3d_format() -> &'static Dialect {
    static TORQUE3D: LazyLock<Dialect> = LazyLock::new(Dialect::torque3d);
    &TORQUE3D
}

/// How many times the size of the global string table the distinct identifier names may add up to.
const IDENTIFIER_BYTES_FACTOR: usize = 4;

/// A tagged literal in a block: (is it in the function string table, its offset there).
type TagSite = (bool, u32);

/// Identifier operands are replaced by `index + 1` into the name table (0 means "null"), so the VM
/// never touches the string table for names. Names are case-insensitive; the first spelling is kept.
///
/// A `CodeBlock` stands for one load of a file, as in TGE where every `exec` makes a new one:
/// it remembers which entry each of its plain call sites resolved to (and for which `Vm`), so running
/// the same block again in the same `Vm` keeps those bindings. Load a file again to get fresh sites.
pub struct CodeBlock {
    name: Option<Rc<[u8]>>,
    pub(crate) code: Vec<u32>,
    names: Vec<Vec<u8>>,
    pub(crate) global_strings: Vec<u8>,
    pub(crate) function_strings: Vec<u8>,
    pub(crate) global_floats: Vec<f64>,
    pub(crate) function_floats: Vec<f64>,
    /// Plain function call sites that have been resolved, by the code index of their opcode, each
    /// stamped with the id of the `Vm` that bound it. The binding is weak: a bound entry can be a
    /// function of this very block (or of a block that calls back into it), and a strong one made a
    /// cycle (entry -> block -> site -> entry) that outlived the `Vm` with every native the blocks
    /// had called and what those natives hold (a host's socket stayed bound after its server was
    /// dropped). The `Vm`'s namespace table holds every entry for the `Vm`'s whole life (entries are
    /// never removed from it), so a binding of a live `Vm` always upgrades.
    /// Keyed by `namehash`'s hasher, as cheap as a fixed one and seeded (a hostile block chooses its call positions).
    sites: RefCell<NameMap<usize, (u64, WeakEntry)>>,
    /// Tagged literals this block has converted, by (is it in the function string table, its offset
    /// there), each stamped with the id of the `Vm` that converted it and holding the text that now
    /// stands in the string table (`"<id>"`). TGE rewrites the string table and the opcode in
    /// place; the block's code here is shared and immutable, so this cache stands in for that. It is
    /// per `Vm` like `sites`, and like `sites` it holds one `Vm`'s conversions at a time: when two `Vm`s
    /// take turns on one block each converts again (finding its own table's entry, with one more engine
    /// reference), so only the text, never an id, can pass from one to the other.
    tags: RefCell<NameMap<TagSite, (u64, Vec<u8>)>>,
    /// The line-break table as TGE holds it after loading: `(instLine, ip)` pairs in code order.
    /// A named block's first word is `line << 8 | code[ip]` (`CodeBlock::calcBreakList`); an unnamed
    /// one (evaluated text) keeps the raw line, as that step is skipped without a file name.
    lines: Vec<(u32, u32)>,
}

impl std::fmt::Debug for CodeBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeBlock")
            .field("name", &self.name.as_deref().map(String::from_utf8_lossy))
            .field("code_len", &self.code.len())
            .finish_non_exhaustive()
    }
}

impl CodeBlock {
    /// Loads `dso`, a file of `format`, as the code block of `name` (None for evaluated text).
    pub fn from_dso(
        dso: &DsoFile,
        name: Option<&[u8]>,
        format: &dyn DsoFormat,
    ) -> Result<CodeBlock, LoadError> {
        if dso.version != format.version() {
            return Err(LoadError::WrongVersion {
                found: dso.version,
                expected: format.version(),
            });
        }
        let global = format.global_strings(dso);
        let mut code = dso.code.clone();
        let mut names: Vec<Vec<u8>> = Vec::new();
        let mut seen: HashMap<Vec<u8>, usize> = HashMap::new();
        // Entries naming one offset share one name: each distinct offset is scanned, folded and hashed once,
        // and the bytes of the distinct names are limited to the table's own size times
        // `IDENTIFIER_BYTES_FACTOR` (a real table holds each name once, so its names add up to less than
        // the table). The work of this loop is then linear in the file, which the caller has charged.
        let table = global.as_bytes();
        let mut left = table.len().saturating_mul(IDENTIFIER_BYTES_FACTOR);
        let mut by_offset: HashMap<u32, usize> = HashMap::new();
        for entry in &dso.identifiers {
            let index = match by_offset.get(&entry.string_index) {
                Some(index) => *index,
                None => {
                    // As `StringTable::get`: an offset at or past the end names nothing.
                    let rest = table
                        .get(entry.string_index as usize..)
                        .filter(|rest| !rest.is_empty())
                        .ok_or(LoadError::BadIdentifier)?;
                    // A name longer than what is left of the allowance is found without scanning past it.
                    let window = rest.get(..left.saturating_add(1)).unwrap_or(rest);
                    let len = match window.iter().position(|&b| b == 0) {
                        Some(len) => len,
                        None if window.len() == rest.len() => rest.len(),
                        None => return Err(LoadError::IdentifiersTooLarge),
                    };
                    left = left
                        .checked_sub(len)
                        .ok_or(LoadError::IdentifiersTooLarge)?;
                    let text = &rest[..len];
                    let index = *seen.entry(fold(text)).or_insert_with(|| {
                        names.push(text.to_vec());
                        names.len() - 1
                    });
                    by_offset.insert(entry.string_index, index);
                    index
                }
            };
            // The name index plus one must fit an operand word (it always does in practice).
            let Some(word) = u32::try_from(index).ok().and_then(|i| i.checked_add(1)) else {
                return Err(LoadError::BadIdentifier);
            };
            for &position in &entry.positions {
                let Some(slot) = code.get_mut(position as usize) else {
                    return Err(LoadError::BadPosition);
                };
                *slot = word;
            }
        }
        let lines = dso
            .line_breaks
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                let (line, ip) = (pair[0], pair[1]);
                let inst = match name {
                    // A named block's entry carries the code word at `ip` in its low byte, as Torque3D's
                    // `CodeBlock::calcBreakList` makes it (identifier operands hold their raw words there).
                    Some(_) => {
                        line.wrapping_shl(8) | dso.code.get(ip as usize).copied().unwrap_or(0)
                    }
                    None => line,
                };
                (inst, ip)
            })
            .collect();
        Ok(CodeBlock {
            name: name.map(Rc::from),
            code,
            names,
            global_strings: global.as_bytes().to_vec(),
            function_strings: format.function_strings(dso).as_bytes().to_vec(),
            global_floats: dso.global_floats.clone(),
            function_floats: dso.function_floats.clone(),
            sites: RefCell::new(NameMap::default()),
            tags: RefCell::new(NameMap::default()),
            lines,
        })
    }

    /// The entry the plain function call at `ip` is bound to by the `Vm` with this id, if it has
    /// already been resolved there. A binding made by another `Vm` does not count.
    pub(crate) fn cached_site(&self, ip: usize, vm_id: u64) -> Option<EntryRef> {
        match self.sites.borrow().get(&ip) {
            Some((id, entry)) if *id == vm_id => entry.upgrade(),
            _ => None,
        }
    }

    /// Binds the plain function call at `ip` to `entry` for the `Vm` with this id, replacing a
    /// binding made by another one.
    pub(crate) fn cache_site(&self, ip: usize, vm_id: u64, entry: &EntryRef) {
        self.sites
            .borrow_mut()
            .insert(ip, (vm_id, Rc::downgrade(entry)));
    }

    /// The text a tagged literal at `offset` of the function (`true`) or global string table was turned
    /// into by the `Vm` with this id, if it has run there. The cache is only borrowed to clone.
    pub(crate) fn converted_tag(&self, function: bool, offset: u32, vm_id: u64) -> Option<Vec<u8>> {
        match self.tags.borrow().get(&(function, offset)) {
            Some((id, text)) if *id == vm_id => Some(text.clone()),
            _ => None,
        }
    }

    /// Remembers what the `Vm` with this id turned the tagged literal at `offset` into, replacing a
    /// conversion made by another one.
    pub(crate) fn convert_tag(&self, function: bool, offset: u32, vm_id: u64, text: Vec<u8>) {
        self.tags
            .borrow_mut()
            .insert((function, offset), (vm_id, text));
    }

    /// The source line of the instruction at `ip`, as Torque3D's `CodeBlock::findBreakLine` finds it:
    /// the line of the closest line-break entry at or before `ip`, by binary search; 0 when the table is
    /// empty or `ip` lies before its first entry or AFTER ITS LAST one. The compiler records entries only
    /// for statements inside function bodies, so top-level code reports the last function statement
    /// before it, or 0.
    pub fn line_at(&self, ip: usize) -> u32 {
        let pairs = &self.lines;
        let Some(last) = pairs.len().checked_sub(1) else {
            return 0;
        };
        let ip = u32::try_from(ip).unwrap_or(u32::MAX);
        if pairs[0].1 > ip || pairs[last].1 < ip {
            return 0;
        }
        let found = if pairs[0].1 == ip {
            0
        } else if pairs[last].1 == ip {
            last
        } else {
            let (mut min, mut max) = (0, last);
            loop {
                if min + 1 >= max {
                    break min;
                }
                let mid = (min + max) >> 1;
                if pairs[mid].1 == ip {
                    break mid;
                }
                if pairs[mid].1 > ip {
                    max = mid;
                } else {
                    min = mid;
                }
            }
        };
        pairs[found].0 >> 8
    }

    /// `CodeBlock::getFileLine`: `name (line)`, `<input> (line)` for a block without a name.
    pub fn file_line(&self, ip: usize) -> String {
        let name = self.name.as_deref().map_or_else(
            || "<input>".to_string(),
            |n| String::from_utf8_lossy(n).into_owned(),
        );
        format!("{name} ({})", self.line_at(ip))
    }

    pub fn name(&self) -> Option<&[u8]> {
        self.name.as_deref()
    }

    /// [`name`](Self::name), shared.
    pub fn name_shared(&self) -> Option<Rc<[u8]>> {
        self.name.clone()
    }

    /// The code words, with every identifier operand replaced by its name index (see [`ident`](Self::ident)).
    pub fn code(&self) -> &[u32] {
        &self.code
    }

    /// Respells every identifier as `respell` gives it (the `Vm`'s string table: loading a block puts each identifier
    /// through `StringTable::insert`, so a name is spelled as the string table first saw it, in any block or string field).
    pub(crate) fn respell_names(&mut self, mut respell: impl FnMut(&[u8]) -> std::rc::Rc<[u8]>) {
        for name in &mut self.names {
            let spelled = respell(name);
            if spelled[..] != name[..] {
                *name = spelled.to_vec();
            }
        }
    }

    /// The identifier an operand word stands for; `None` for the null identifier.
    pub fn ident(&self, word: u32) -> Option<&[u8]> {
        if word == 0 {
            return None;
        }
        self.names.get(word as usize - 1).map(Vec::as_slice)
    }
}
