//! Reading and writing compiled TorqueScript (DSO) files in Torque3D's format. The details that vary between
//! builds of that format (version word, local-variable table, opcode numbering) come from a [`Dialect`];
//! [`Dialect::torque3d`] is the built-in one. TGE 1.4's older format, with its own instruction set, is the
//! [`classic`] dialect.
//!
//! The format follows Torque3D (MIT), `Engine/source/console/`: the file layout of `torquescript/codeBlock.cpp`
//! (`CodeBlock::read`, and the writing in `CodeBlock::compile`) and `codeBlock.h`, the instruction set of
//! `torquescript/compiler.h` (`Compiler::CompiledInstructions`) and the version word of `console.h` (`DSOVersion`):
//! Copyright (c) 2012 GarageGames, LLC. The operand layouts follow `torquescript/compiledEval.cpp`: Copyright (c) 2013
//! GarageGames, LLC; Copyright (c) 2015 Faust Logic, Inc.; Copyright (c) 2021 TGEMIT Authors & Contributors. The code
//! here is this crate's own.

pub mod classic;
pub mod decode;
pub mod dialect;
pub mod dso;
pub mod dsotext;
pub mod error;
pub mod opcodes;
mod reader;

pub use dialect::{Dialect, FileLayout};
pub use dso::{DsoFile, IdentEntry, LocalVariables, StringTable};
pub use error::DsoError;
