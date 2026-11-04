//! The classic compiler: TGE 1.4's TorqueScript to a classic DSO ([`compile`]), with the engine's two compile entry
//! points ([`Purpose`]).
//!
//! A [`Profile`] says what is compiled and how: the file format ([`ClassicDialect`]: version, numbering, string
//! coding), the grammar ([`Syntax`], TGE 1.4's by default) and its scanner, and the code generator's
//! [`Generation`]. [`Profile::tge14`] is TGE 1.4's; another build of the same compiler is a profile with other
//! values: [`codegen`] says what each one changes.

pub mod codegen;

use torque_ts_bytecode::DsoFile;
use torque_ts_bytecode::classic::ClassicDialect;

use crate::lexer::{Language, Lexer};
use crate::parser::{self, Scanner, Syntax};
use crate::{Diagnostic, Level, MAX_SOURCE_BYTES, Purpose, WORKER_STACK};

/// What a build of the classic code generator does its own way (see [`codegen`]).
#[derive(Clone, Copy, Debug)]
pub struct Generation {
    /// The first pass visits the right operand of a node with two first (TGE 1.4's source reads left to right).
    pub right_operand_first: bool,
    /// A function's parameter names enter the global string table in the first pass, before the function's name
    /// (TGE 1.4: when the second pass writes them).
    pub params_in_first_pass: bool,
    /// `dAtof`, for a string or name constant used as a number.
    pub atof: fn(&[u8]) -> f64,
    /// What [`Purpose::Check`] reports a `precompile size mismatch` with (an error; [`Purpose::Run`] warns
    /// `precompile size mismatch`).
    pub check_mismatch: &'static [u8],
}

impl Generation {
    /// TGE 1.4's.
    pub const TGE14: Generation = Generation {
        right_operand_first: false,
        params_in_first_pass: false,
        atof: torque_ts_text::atof,
        check_mismatch: b"CodeBlock::compile - precompile size mismatch, a precompile/compile function pair is probably \
                          mismatched.",
    };
}

/// A scanner of a classic dialect: makes the token source for a text.
pub type ScannerFn = for<'a> fn(&'a [u8]) -> Box<dyn Scanner + 'a>;

/// TGE 1.4's scanner ([`Language::Tge14`]).
pub fn tge14_scanner(source: &[u8]) -> Box<dyn Scanner + '_> {
    Box::new(Lexer::with_language(source, Language::Tge14))
}

/// A classic compiler profile (see the module documentation).
#[derive(Clone, Debug)]
pub struct Profile {
    pub dialect: ClassicDialect,
    pub syntax: Syntax,
    pub scanner: ScannerFn,
    pub generation: Generation,
}

impl Profile {
    /// TGE 1.4's compiler.
    pub fn tge14() -> Profile {
        Profile {
            dialect: ClassicDialect::tge14(),
            syntax: Syntax::TGE14,
            scanner: tge14_scanner,
            generation: Generation::TGE14,
        }
    }
}

impl Default for Profile {
    fn default() -> Profile {
        Profile::tge14()
    }
}

/// What a compile gives.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub diagnostics: Vec<Diagnostic>,
    /// A syntax error was reported.
    pub syntax_error: bool,
    /// The code (None when there was no statement, or for [`Purpose::Check`] after a syntax error).
    pub dso: Option<DsoFile>,
    /// Where the code the second pass wrote ends: just after the file's final `OP_RETURN`. After a `precompile size
    /// mismatch` the words from there to the end of `dso.code` were never written (TGE leaves them uninitialised;
    /// the code generator fills them with [`codegen::gap_word`]); otherwise it is the code's length. None without
    /// a `dso`.
    pub code_end: Option<usize>,
    /// Work done, in the VM's work units (one per source byte scanned, per parser step and per node visit of
    /// the code generator): the VM charges it to the step budget.
    pub work: u64,
}

/// Compiles `source` (a C string: the compiler stops at the first NUL) with `profile`. `file` is the file name
/// messages name (None for `eval`'d text). Runs on a thread of its own with [`WORKER_STACK`] bytes of stack, so the
/// caller's stack depth does not matter (a script can `eval` at the bottom of a deep recursion).
///
/// The two purposes differ as TGE's entry points do: [`Purpose::Run`] (`CodeBlock::compileExec`) still compiles
/// the statements before a syntax error and warns about a size mismatch; [`Purpose::Check`] (`CodeBlock::compile`,
/// and `exec`'s first pass over a source) generates nothing after a syntax error and reports a size mismatch as an
/// error ([`Generation::check_mismatch`]).
pub fn compile(source: &[u8], file: Option<&[u8]>, purpose: Purpose, profile: &Profile) -> Output {
    let joined = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("ts-compiler".into())
            .stack_size(WORKER_STACK)
            .spawn_scoped(scope, || compile_here(source, file, purpose, profile))
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
            code_end: None,
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
    profile: &Profile,
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
            code_end: None,
            work: source.len() as u64,
        };
    }
    let parsed = parser::parse_scanned(&mut *(profile.scanner)(source), file, &profile.syntax);
    let mut diagnostics = parsed.diagnostics;
    let mut work = parsed.work;
    if (parsed.syntax_error && purpose == Purpose::Check) || parsed.script.stmts.is_empty() {
        return Output {
            diagnostics,
            syntax_error: parsed.syntax_error,
            dso: None,
            code_end: None,
            work,
        };
    }
    let generator = codegen::Gen::new(file, &profile.dialect, profile.generation);
    let (dso, mut generated, visits, code_end) = generator.generate(&parsed.script);
    if purpose == Purpose::Check {
        for d in &mut generated {
            if d.text == codegen::RUN_MISMATCH {
                d.level = Level::Error;
                d.text = profile.generation.check_mismatch.to_vec();
            }
        }
    }
    diagnostics.extend(generated);
    work = work.saturating_add(visits);
    let failed = dso.is_err();
    Output {
        diagnostics,
        syntax_error: parsed.syntax_error || failed,
        code_end: dso.is_ok().then_some(code_end),
        dso: dso.ok(),
        work,
    }
}
