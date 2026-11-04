//! The TorqueScript compiler: source text to DSO bytecode. `lexer` scans the source, `parser` is an LALR(1)
//! parser built from the grammar (`lalr` makes its tables), `ast` is the tree it builds, and `codegen` writes
//! the code for that tree (`tables` holds the string, float and identifier tables).
//!
//! The language is Torque3D's TorqueScript (its scanner and grammar, see [`lexer`] and [`parser`]). Every compile
//! takes a [`Dialect`]: its version, layout and opcode numbering make the DSO; [`Dialect::torque3d`] is the
//! built-in one. The [`ast`] is public, so another front end can feed the code generator, and a code generator of
//! another instruction set can walk the tree [`parser::parse`] builds.
//!
//! **Credits.** The language and its compilation follow Torque3D (MIT), `Engine/source/console/torquescript/`; the code
//! here is this crate's own, and no generated parser table is copied:
//! - `CMDscan.l` (the scanner, [`lexer`]) and `CMDgram.y` (the grammar, [`parser`]) are part of the Torque3D tree,
//!   Copyright (c) 2012 GarageGames, LLC (the files carry no notice of their own);
//! - `ast.h`, `astNodes.cpp`, `compiler.h`, `compiler.cpp`, `codeBlock.cpp` (the tree, the code generation and the
//!   tables: [`ast`], [`codegen`], [`tables`]): Copyright (c) 2012 GarageGames, LLC;
//! - `astAlloc.cpp` (the node constructors): Copyright (c) 2013 GarageGames, LLC;
//! - `optimizer.cpp` (the folding of constant operands): Copyright (c) 2013 GarageGames, LLC; Copyright (c) 2021 TGEMIT
//!   Authors & Contributors;
//! - `compiledEval.cpp` (`consoleStringToNumber`, the local-variable rules): Copyright (c) 2013 GarageGames, LLC;
//!   Copyright (c) 2015 Faust Logic, Inc.; Copyright (c) 2021 TGEMIT Authors & Contributors.
//!
//! Where a rule comes from the Torque Game Engine 1.4 or 1.5 (the `do`-`while` keyword, the parser's default
//! reductions), it is followed as the engine code Torque3D is the MIT release of.
//!
//! **The classic dialect.** [`classic`] compiles TGE 1.4's TorqueScript ([`parser::Syntax::TGE14`], scanned with
//! [`lexer::Language::Tge14`]) to TGE 1.4's instruction set and file format (`torque_ts_bytecode::classic`), with
//! TGE 1.4's two-pass code generator. It follows TGE 1.4's `engine/console/` files `CMDscan.l` and `CMDgram.y` (no
//! notice of their own), `ast.h`, `astNodes.cc`, `astAlloc.cc`, `compiler.h`, `compiler.cc` and `codeBlock.cc`
//! (each Copyright (C) GarageGames.com, Inc.), the engine Torque3D is the MIT release of.

pub mod ast;
pub mod classic;
pub mod codegen;
pub mod lalr;
pub mod lexer;
pub mod parser;
pub mod tables;

use torque_ts_bytecode::{Dialect, DsoFile};

/// The level of a message (`Con::printf`, `warnf`, `errorf`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Normal,
    Warning,
    Error,
}

/// One message of the compiler, as the console prints it (a text may hold several lines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub level: Level,
    pub text: Vec<u8>,
}

/// How a compile is used: there are two entry points with slightly different behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// `CodeBlock::compileExec` (`exec` of a source, `eval`): the statements before a syntax error are still
    /// compiled.
    Run,
    /// `CodeBlock::compile` (`compile()`, writing a DSO): nothing is generated after a syntax error.
    Check,
}

/// What a compile gives.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub diagnostics: Vec<Diagnostic>,
    /// A syntax error was reported.
    pub syntax_error: bool,
    /// The code (None when there was no statement, or for [`Purpose::Check`] after a syntax error).
    pub dso: Option<DsoFile>,
    /// Work done, in work units (one per source byte scanned, per parser step and per node visit of the code
    /// generator): a VM can charge it to a step budget.
    pub work: u64,
}

/// The stack of the thread [`compile`] runs on: the code generator recurses once per level of the tree (at
/// most [`parser::MAX_TREE_DEPTH`]), several KiB per level in a debug build.
pub const WORKER_STACK: usize = 64 << 20;
/// The longest source compiled.
pub const MAX_SOURCE_BYTES: usize = 16 << 20;

/// Compiles `source` (a C string: it stops at the first NUL) in the given dialect. `file` is the file name
/// messages name (None for `eval`'d text). Runs on a thread of its own with [`WORKER_STACK`] bytes of stack, so
/// the caller's stack depth does not matter (a script can `eval` at the bottom of a deep recursion).
pub fn compile(source: &[u8], file: Option<&[u8]>, purpose: Purpose, dialect: &Dialect) -> Output {
    let joined = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("ts-compiler".into())
            .stack_size(WORKER_STACK)
            .spawn_scoped(scope, || compile_here(source, file, purpose, dialect))
            .map(|handle| handle.join())
    });
    match joined {
        Ok(Ok(output)) => output,
        // A thread could not be started (or, impossibly, the compiler panicked).
        _ => Output {
            diagnostics: vec![Diagnostic {
                level: Level::Error,
                text: b"the compiler could not run".to_vec(),
            }],
            syntax_error: true,
            dso: None,
            work: source.len() as u64,
        },
    }
}

/// [`compile`] on the current thread: it needs about [`WORKER_STACK`] bytes of stack for the deepest tree the
/// parser accepts. The measure is the tree's DEPTH, not the source's size: a short source (a 500-level `+`
/// chain is a few KiB) can need many MiB of a debug build's stack, so a caller that trusts a "small source"
/// bound on a small thread can overflow it. Everything but tools and tests that give the thread its stack
/// should call [`compile`].
pub fn compile_here(
    source: &[u8],
    file: Option<&[u8]>,
    purpose: Purpose,
    dialect: &Dialect,
) -> Output {
    let source = torque_ts_text::until_nul(source);
    if source.len() > MAX_SOURCE_BYTES {
        return Output {
            diagnostics: vec![Diagnostic {
                level: Level::Error,
                text: format!(
                    "{}: the script is longer than {MAX_SOURCE_BYTES} bytes",
                    file.map_or_else(
                        || "<input>".to_string(),
                        |f| String::from_utf8_lossy(f).into_owned()
                    )
                )
                .into_bytes(),
            }],
            syntax_error: true,
            dso: None,
            work: source.len() as u64,
        };
    }
    let parsed = parser::parse(source, file);
    let mut diagnostics = parsed.diagnostics;
    let mut work = parsed.work;
    if (parsed.syntax_error && purpose == Purpose::Check) || parsed.script.stmts.is_empty() {
        return Output {
            diagnostics,
            syntax_error: parsed.syntax_error,
            dso: None,
            work,
        };
    }
    let generator = codegen::Gen::new(file, source, dialect);
    let (dso, generated, visits) = generator.generate(&parsed.script, dialect);
    diagnostics.extend(generated);
    work = work.saturating_add(visits);
    let failed = dso.is_err();
    Output {
        diagnostics,
        syntax_error: parsed.syntax_error || failed,
        dso: dso.ok(),
        work,
    }
}
