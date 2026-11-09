//! The TorqueScript runtime: the console's variables, namespaces, objects, natives and events, with a pluggable
//! executor for a dialect's opcode loop.
//!
//! Follows Torque3D's MIT-licensed `console/` and `console/torquescript/` (Copyright (c) 2012 GarageGames, LLC, and
//! later holders where a file names them); the code here is this crate's own.
//!
//! # Maintaining
//!
//! Rules a change must keep, each guarded by a test:
//! - A native's and a global's effects ([`effects`]) only grow, and a name's are every namespace's
//!   (`a_names_effects_only_grow_and_cover_every_namespace`, `a_globals_effects_come_from_every_pattern_that_names_it`).
//! - `export` never writes a partial statement: a value too long for a line goes in whole statements of at most a line each,
//!   and reads back as it was (`export_never_writes_an_unterminated_statement`,
//!   `two_long_prefs_never_run_as_script_when_read_back`).
//! - TorqueScript's own extensions are never another language's, and an extension is one language's
//!   (`torque_script_s_extensions_are_never_taken_and_others_only_once`).
//! - A variable's name ends at its first NUL byte, for its key and the spelling it keeps alike, and a global pattern
//!   reads a name the same way (`a_name_is_the_same_variable_up_to_its_nul`,
//!   `a_globals_effects_come_from_every_pattern_that_names_it`).

pub mod classes;
pub mod code;
pub mod convert;
pub mod effects;
pub mod events;
pub mod exec;
pub mod fields;
/// The matrix and quaternion maths of the vector functions (TGE's `QuatF`, `AngAxisF` and `MatrixF`), with their
/// rounding, for a host that keeps transforms: the same functions `MatrixCreate` and the `rotation` field use, so a host's
/// numbers are theirs.
pub mod geometry {
    /// A 4x4 matrix, row by row (as TGE's `MatrixF`: translation in the last column).
    pub use crate::vm::vectors::{Mat4, V3};
    pub use crate::vm::vectors::{
        axis_angle_from_quat, matrix_from_quat, quat_from_axis_angle, quat_from_matrix,
    };
}
pub mod host;
pub mod idhash;
pub mod listing;
pub mod namehash;
pub mod ns;
pub mod policy;
pub mod sim;
pub mod tags;
/// The C-library behavior the runtime depends on (the `torque-ts-text` crate, shared with the compiler).
pub use torque_ts_text as text;
pub mod vars;
pub mod vm;
