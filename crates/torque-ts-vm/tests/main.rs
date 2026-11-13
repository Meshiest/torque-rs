//! The integration tests, linked as one test program: one module per file.

#[path = "../../torque-ts-compiler/tests/common/mod.rs"]
mod common;
mod sweep;

mod classic;
mod defaults;
mod fuzz;
mod holding_warnings;
mod languages;
mod torque3d;
mod torque3d_asm;
