//! The TorqueScript runtime: the console's variables, namespaces, objects, natives and events, with a pluggable
//! executor for a dialect's opcode loop.
//!
//! Follows Torque3D's MIT-licensed `console/` and `console/torquescript/` (Copyright (c) 2012 GarageGames, LLC, and
//! later holders where a file names them); the code here is this crate's own.

pub mod classes;
pub mod code;
pub mod convert;
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
pub mod namehash;
pub mod ns;
pub mod policy;
pub mod sim;
pub mod tags;
/// The C-library behavior the runtime depends on (the `torque-ts-text` crate, shared with the compiler).
pub use torque_ts_text as text;
pub mod vars;
pub mod vm;
