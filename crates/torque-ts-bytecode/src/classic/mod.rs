//! The classic dialect: compiled TorqueScript as the Torque Game Engine 1.4 writes it. Its VM has no registers
//! and keeps typed values on an integer, a float and a string stack, so its instruction set ([`opcodes`]) has the
//! type conversions and string-stack operations Torque3D's dropped, and its files have no local-variable table.
//!
//! A [`ClassicDialect`] says what can vary between builds of the format: the version word, the opcode numbering
//! and how the string tables are stored ([`StringCoding`]); [`ClassicDialect::tge14`] is TGE 1.4's own. The
//! [`decode`] and [`dsotext`] modules are the classic counterparts of the crate's Torque3D ones.
//!
//! Credit: the format follows TGE 1.4's `engine/console/` files `codeBlock.cc` (the file), `compiler.h` (the
//! instruction set), `compiledEval.cc` (the operand layouts) and `console.h` (the version word), each Copyright (C)
//! GarageGames.com, Inc.; their MIT-licensed Torque3D successors and copyright lines are in the crate documentation.
//! The code is this crate's own.

pub mod decode;
pub mod dsotext;
pub mod opcodes;

use crate::dialect::FileLayout;
use crate::dso::{DsoFile, StringTable};
use crate::error::DsoError;

pub use opcodes::{Op, OpcodeTable, OperandShape};

/// How a dialect stores its string tables: `decode` turns the stored bytes into the text, `encode` the text into
/// what is stored. TGE 1.4 stores the text as it is ([`StringCoding::PLAIN`]); a build that keeps its tables
/// unreadable on disk gives its own pair.
#[derive(Clone, Copy, Debug)]
pub struct StringCoding {
    pub decode: fn(&[u8]) -> Vec<u8>,
    pub encode: fn(&[u8]) -> Vec<u8>,
}

impl StringCoding {
    /// The text as it is.
    pub const PLAIN: StringCoding = StringCoding {
        decode: <[u8]>::to_vec,
        encode: <[u8]>::to_vec,
    };

    /// The text of a table stored as `stored`.
    pub fn table(&self, stored: &[u8]) -> StringTable {
        StringTable::new((self.decode)(stored))
    }
}

/// A classic dialect profile (see the module documentation).
#[derive(Clone, Debug)]
pub struct ClassicDialect {
    /// The first word of the file. Readers record it but do not enforce it.
    pub dso_version: u32,
    /// Where the sections go ([`ClassicDialect::parse`] and [`ClassicDialect::to_bytes`]); never a local-variable
    /// table.
    pub layout: FileLayout,
    /// The code word of each operation.
    pub opcodes: OpcodeTable,
    pub strings: StringCoding,
}

impl ClassicDialect {
    /// TGE 1.4's `Con::DSOVersion`.
    pub const TGE14_DSO_VERSION: u32 = 36;

    /// The layout of TGE 1.4's files: Torque3D's without the local-variable table.
    pub const TGE14_LAYOUT: FileLayout = FileLayout {
        local_variable_table: false,
    };

    /// TGE 1.4's dialect: DSO version 36, its layout and numbering, plain string tables.
    pub const TGE14: ClassicDialect = ClassicDialect {
        dso_version: ClassicDialect::TGE14_DSO_VERSION,
        layout: ClassicDialect::TGE14_LAYOUT,
        opcodes: OpcodeTable::TGE14,
        strings: StringCoding::PLAIN,
    };

    /// [`ClassicDialect::TGE14`].
    pub fn tge14() -> ClassicDialect {
        ClassicDialect::TGE14
    }

    /// The text of the global string table.
    pub fn global_strings(&self, dso: &DsoFile) -> StringTable {
        self.strings.table(&dso.global_strings)
    }

    /// The text of the function string table.
    pub fn function_strings(&self, dso: &DsoFile) -> StringTable {
        self.strings.table(&dso.function_strings)
    }

    /// Parses a file of this dialect (its string tables stay as stored). The version is recorded but not enforced.
    pub fn parse(&self, bytes: &[u8]) -> Result<DsoFile, DsoError> {
        DsoFile::parse_layout(bytes, self.layout)
    }

    /// Writes `dso` in this dialect's layout (see [`DsoFile::to_bytes`]).
    ///
    /// # Panics
    ///
    /// As [`DsoFile::to_bytes`].
    pub fn to_bytes(&self, dso: &DsoFile) -> Vec<u8> {
        dso.to_bytes_layout(self.layout)
    }
}

impl Default for ClassicDialect {
    fn default() -> ClassicDialect {
        ClassicDialect::tge14()
    }
}
