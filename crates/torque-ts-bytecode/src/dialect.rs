//! A dialect profile: what about a compiled TorqueScript file can vary between engine builds of the same
//! format. The compiler, the DSO reader and writer, the decoder and the text form all take one. torque-rs ships
//! one profile, [`Dialect::torque3d`]; another build of the same format (an older version word, TGE 1.4's
//! files without a local-variable table, another opcode numbering) is a [`Dialect`] value passed instead.
//!
//! The built-in profile follows Torque3D's TorqueScript as of the Torque3D repository's commit
//! `4c44642aab32cf79be4f66966d49fd74ab18e221` (2026-09-25), `Engine/source/console/torquescript/`.
//! Torque3D is MIT-licensed; the files this crate follows and their copyright lines are listed in the crate
//! documentation. This module implements the file format and copies no code.

use crate::opcodes::OpcodeTable;

/// Where the sections of a DSO file go. The file always starts with the version word, then the global string
/// table, the function string table, the global float table and the function float table, and ends with the
/// code, the line-break table and the identifier table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileLayout {
    /// A table of each function's local variable names (by register) follows the constant tables (Torque3D;
    /// TGE 1.4's files, whose VM has no registers, have none).
    pub local_variable_table: bool,
}

impl FileLayout {
    pub const TORQUE3D: FileLayout = FileLayout {
        local_variable_table: true,
    };
}

/// A dialect profile (see the module documentation).
#[derive(Clone, Debug)]
pub struct Dialect {
    /// The first word of the file. Readers record it but do not enforce it.
    pub dso_version: u32,
    pub layout: FileLayout,
    /// The code word of each operation.
    pub opcodes: OpcodeTable,
}

impl Dialect {
    /// Torque3D's `Con::DSOVersion`.
    pub const TORQUE3D_DSO_VERSION: u32 = 50;

    /// Torque3D's dialect: DSO version 50, a local-variable table, Torque3D's opcode numbering.
    pub fn torque3d() -> Dialect {
        Dialect {
            dso_version: Dialect::TORQUE3D_DSO_VERSION,
            layout: FileLayout::TORQUE3D,
            opcodes: OpcodeTable::TORQUE3D,
        }
    }
}

impl Default for Dialect {
    fn default() -> Dialect {
        Dialect::torque3d()
    }
}
