//! A DSO file's parts and their reading and writing, in the layout Torque3D's `CodeBlock::read` reads and
//! `CodeBlock::compile` writes (see the crate documentation for the files followed and their copyright lines).

use crate::dialect::{Dialect, FileLayout};
use crate::error::DsoError;
use crate::reader::Reader;

/// One entry of the identifier table: the code positions that should receive `string_index`
/// (a byte offset into the global string table) when the file is loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentEntry {
    pub string_index: u32,
    pub positions: Vec<u32>,
}

/// The local variables of the functions named `function` (`Namespace::name`), by register: `names[r]` is the
/// variable in register `r`. Several functions of the same name share one entry, one after the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalVariables {
    pub function: Vec<u8>,
    pub names: Vec<Vec<u8>>,
}

/// The longest name the local-variable table stores: its length is one byte.
pub const MAX_LOCAL_NAME_BYTES: usize = 255;

/// A DSO file exactly as stored. Code words at identifier positions hold their on-disk value. [`to_bytes`](Self::to_bytes) writes the file back in
/// canonical form (single byte for code words < 0xFF, 0xFF escape + u32 for 0xFF and above); parse accepts
/// non-canonical escapes (0xFF followed by a value < 0xFF) and normalizes them to single-byte form on write.
#[derive(Debug, Clone, PartialEq)]
pub struct DsoFile {
    pub version: u32,
    pub global_strings: Vec<u8>,
    pub global_floats: Vec<f64>,
    pub function_strings: Vec<u8>,
    pub function_floats: Vec<f64>,
    /// Empty in a layout without the table.
    pub local_variables: Vec<LocalVariables>,
    pub code: Vec<u32>,
    /// Pairs of words (line, code position); the length is always even.
    pub line_breaks: Vec<u32>,
    pub identifiers: Vec<IdentEntry>,
}

/// A string table: NUL-separated strings addressed by byte offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringTable {
    bytes: Vec<u8>,
}

impl StringTable {
    /// The table whose bytes are `bytes`.
    pub fn new(bytes: impl Into<Vec<u8>>) -> StringTable {
        StringTable {
            bytes: bytes.into(),
        }
    }

    /// The whole buffer, NUL separators included.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// True when `offset` is inside the table and either the first byte or right after a NUL.
    pub fn is_string_start(&self, offset: u32) -> bool {
        let o = offset as usize;
        o < self.bytes.len() && (o == 0 || self.bytes[o - 1] == 0)
    }

    /// The string starting at `offset`, without its terminator. `None` when out of range.
    pub fn get(&self, offset: u32) -> Option<&[u8]> {
        let o = offset as usize;
        if o >= self.bytes.len() {
            return None;
        }
        let rest = &self.bytes[o..];
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        Some(&rest[..end])
    }
}

fn read_blob(r: &mut Reader<'_>, what: &'static str) -> Result<Vec<u8>, DsoError> {
    let len = r.u32(what)?;
    r.check_count(u64::from(len), 1, what)?;
    Ok(r.bytes(len as usize, what)?.to_vec())
}

fn read_floats(r: &mut Reader<'_>, what: &'static str) -> Result<Vec<f64>, DsoError> {
    let count = r.u32(what)?;
    r.check_count(u64::from(count), 8, what)?;
    let mut floats = Vec::with_capacity(count as usize);
    for _ in 0..count {
        floats.push(r.f64(what)?);
    }
    Ok(floats)
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_len(out: &mut Vec<u8>, len: usize) {
    put_u32(
        out,
        u32::try_from(len).expect("DSO table too large to write (more than u32::MAX entries)"),
    );
}

fn put_blob(out: &mut Vec<u8>, blob: &[u8]) {
    put_len(out, blob.len());
    out.extend_from_slice(blob);
}

fn put_floats(out: &mut Vec<u8>, floats: &[f64]) {
    put_len(out, floats.len());
    for f in floats {
        out.extend_from_slice(&f.to_le_bytes());
    }
}

fn read_local_variables(r: &mut Reader<'_>) -> Result<Vec<LocalVariables>, DsoError> {
    let count = r.u32("local variable table")?;
    // Each entry is at least a length byte and a count word.
    r.check_count(u64::from(count), 5, "local variable table")?;
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let function = r.short_string("local variable function name")?;
        let names_count = r.u32("local variable count")?;
        r.check_count(u64::from(names_count), 1, "local variable names")?;
        let mut names = Vec::with_capacity(names_count as usize);
        for _ in 0..names_count {
            names.push(r.short_string("local variable name")?);
        }
        out.push(LocalVariables { function, names });
    }
    Ok(out)
}

/// A one-byte length and at most 255 bytes: a longer name is cut, as the format can hold no more.
fn put_short_string(out: &mut Vec<u8>, text: &[u8]) {
    let text = &text[..text.len().min(MAX_LOCAL_NAME_BYTES)];
    out.push(text.len() as u8);
    out.extend_from_slice(text);
}

impl DsoFile {
    /// Parses a DSO file laid out as `dialect` says. The version is recorded but not enforced.
    pub fn parse(bytes: &[u8], dialect: &Dialect) -> Result<DsoFile, DsoError> {
        DsoFile::parse_layout(bytes, dialect.layout)
    }

    /// Parses a DSO file with the given layout (see [`DsoFile::parse`]).
    pub fn parse_layout(bytes: &[u8], layout: FileLayout) -> Result<DsoFile, DsoError> {
        let mut r = Reader::new(bytes);
        let version = r.u32("version")?;
        let global_strings = read_blob(&mut r, "global string table")?;
        let function_strings = read_blob(&mut r, "function string table")?;
        let global_floats = read_floats(&mut r, "global float table")?;
        let function_floats = read_floats(&mut r, "function float table")?;
        let local_variables = if layout.local_variable_table {
            read_local_variables(&mut r)?
        } else {
            Vec::new()
        };

        let code_size = r.u32("code size")?;
        let line_break_count = r.u32("line break count")?;

        r.check_count(u64::from(code_size), 1, "code")?;
        let mut code = Vec::with_capacity(code_size as usize);
        for _ in 0..code_size {
            code.push(r.op("code")?);
        }

        let line_words = u64::from(line_break_count) * 2;
        r.check_count(line_words, 4, "line breaks")?;
        let mut line_breaks = Vec::with_capacity(line_words as usize);
        for _ in 0..line_words {
            line_breaks.push(r.u32("line break")?);
        }

        let ident_count = r.u32("identifier count")?;
        r.check_count(u64::from(ident_count), 8, "identifier table")?;
        let mut identifiers = Vec::with_capacity(ident_count as usize);
        for _ in 0..ident_count {
            let string_index = r.u32("identifier string index")?;
            let count = r.u32("identifier position count")?;
            r.check_count(u64::from(count), 4, "identifier positions")?;
            let mut positions = Vec::with_capacity(count as usize);
            for _ in 0..count {
                positions.push(r.u32("identifier position")?);
            }
            identifiers.push(IdentEntry {
                string_index,
                positions,
            });
        }

        if r.remaining() != 0 {
            return Err(DsoError::TrailingBytes {
                offset: r.position(),
                extra: r.remaining(),
            });
        }

        Ok(DsoFile {
            version,
            global_strings,
            global_floats,
            function_strings,
            function_floats,
            local_variables,
            code,
            line_breaks,
            identifiers,
        })
    }

    pub fn global_string_table(&self) -> StringTable {
        StringTable::new(self.global_strings.as_slice())
    }

    pub fn function_string_table(&self) -> StringTable {
        StringTable::new(self.function_strings.as_slice())
    }

    /// Writes the file in canonical form, laid out as `dialect` says. For files produced by
    /// [`DsoFile::parse`] with the same dialect, the output is byte-identical to the input when the input uses
    /// canonical code-word encoding (values < 0xFF as one byte, 0xFF+ as escape + u32). Names in the
    /// local-variable table are cut to [`MAX_LOCAL_NAME_BYTES`].
    ///
    /// # Panics
    ///
    /// Panics if `line_breaks` does not contain an even number of words, if the layout has no local-variable
    /// table but `local_variables` is not empty, or if any table or identifier count exceeds u32::MAX.
    pub fn to_bytes(&self, dialect: &Dialect) -> Vec<u8> {
        self.to_bytes_layout(dialect.layout)
    }

    /// Writes the file with the given layout (see [`DsoFile::to_bytes`]).
    ///
    /// # Panics
    ///
    /// As [`DsoFile::to_bytes`].
    pub fn to_bytes_layout(&self, layout: FileLayout) -> Vec<u8> {
        assert!(
            self.line_breaks.len().is_multiple_of(2),
            "line_breaks must come in pairs"
        );
        assert!(
            layout.local_variable_table || self.local_variables.is_empty(),
            "the layout has no local-variable table"
        );
        let mut out = Vec::new();
        put_u32(&mut out, self.version);
        put_blob(&mut out, &self.global_strings);
        put_blob(&mut out, &self.function_strings);
        put_floats(&mut out, &self.global_floats);
        put_floats(&mut out, &self.function_floats);
        if layout.local_variable_table {
            put_len(&mut out, self.local_variables.len());
            for entry in &self.local_variables {
                put_short_string(&mut out, &entry.function);
                put_len(&mut out, entry.names.len());
                for name in &entry.names {
                    put_short_string(&mut out, name);
                }
            }
        }
        put_len(&mut out, self.code.len());
        put_len(&mut out, self.line_breaks.len() / 2);
        for &word in &self.code {
            if word < 0xFF {
                out.push(word as u8);
            } else {
                out.push(0xFF);
                put_u32(&mut out, word);
            }
        }
        for &word in &self.line_breaks {
            put_u32(&mut out, word);
        }
        put_len(&mut out, self.identifiers.len());
        for entry in &self.identifiers {
            put_u32(&mut out, entry.string_index);
            put_len(&mut out, entry.positions.len());
            for &p in &entry.positions {
                put_u32(&mut out, p);
            }
        }
        out
    }
}
