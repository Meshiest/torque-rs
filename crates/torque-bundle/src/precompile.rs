//! Script precompile: a TorqueScript source of the game folder shipped as the compiled file the engine loads in its place.
//!
//! `exec("x.cs")` looks for `x.cs.dso` beside the source and, when it is there and loads, runs it without compiling
//! (`Vm::exec_file`; a compiled file with no source is run all the same). `--precompile` compiles each `.cs` and `.gui` of the
//! game folder in the format the engine's executor reads ([`ScriptFormat`]: Torque3D's unless the game runs TGE 1.4's) and ships
//! `<path>.dso` instead of `<path>`, so the engine never compiles at start-up and the bundle holds no script text. The
//! source is not kept beside it: the engine prefers a compiled file only when it is at least as new as the source, and
//! the times of files change in archives, copies and downloads.
//!
//! **A compiled file is trusted.** `exec` runs the top level of a `.dso` as trusted and a source's as not: the flag reaches
//! `DeclarePolicy::may_declare`, so a game that installs a policy refusing names to its source scripts would see them
//! declared by the same script shipped compiled. The engine's own policy allows every name, so nothing changes for a game that
//! installs none; a game that does must keep its scripts as source (no `--precompile`).
//!
//! The scripts of a mod stay source ([`in_mod`]): the engine never runs a compiled script that came with a mod, since a mod's
//! files may come from a server. So do scripts whose compiled path would pass the 511 bytes the engine builds it in.
//!
//! A compile is the one `exec` makes of a source that has no syntax error (`Purpose::Check`: the same code as the run's), so a
//! script that does not compile is not compiled here either: it stays source and is reported. Each compiled file is parsed
//! and loaded again by the engine's own loader before it is shipped. Mission files (`.mis`) have no compiled form, and a
//! script of only comments compiles to nothing, so both stay source.

use torque_ts_bytecode::DsoFile;
use torque_ts_compiler::{Diagnostic, Level, Purpose};
use torque_ts_vm::code::{CodeBlock, DsoFormat, torque3d_format};
use torque_ts_vm::exec::classic::TGE14_DIALECT;
use torque_ts_vm::vm::{MAX_DSO_BYTES, MAX_DSO_PATH};

/// What a compiled script's path ends with, after its source's.
pub const EXTENSION: &str = ".dso";

/// The folder the engine looks for mods in when `game.toml` names none, and the manifest that makes a folder a mod. The engine
/// never runs a compiled script that came with a mod (a mod's content may come from a server), so the scripts of a mod ship as
/// source. A game with a mod layout of its own (`ModLayout`) that finds mods otherwise is not seen here: it keeps its mods as
/// source by listing them in `bundle.toml`'s `exclude`, or does not use `--precompile`.
pub const MODS_FOLDER: &str = "mods";
pub const MOD_MANIFEST: &str = "mod.toml";

/// The most lines of a script's diagnostics a refusal quotes, and the most characters of each.
const QUOTED_LINES: usize = 3;
const QUOTED_CHARS: usize = 160;

/// The compiled-file format a game's scripts are read in: the format of its executor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScriptFormat {
    /// Torque3D's, the engine's default executor.
    #[default]
    Torque3d,
    /// TGE 1.4's, for a game made with `ClassicExecutor`.
    Classic,
}

impl ScriptFormat {
    fn dso_format(self) -> &'static dyn DsoFormat {
        match self {
            ScriptFormat::Torque3d => torque3d_format(),
            ScriptFormat::Classic => &TGE14_DIALECT,
        }
    }

    /// Compiles `source` as `exec` does a source with no syntax error: the code (None: no statement), or the reason.
    fn compile_checked(self, path: &str, source: &[u8]) -> Result<Option<DsoFile>, String> {
        let name = Some(path.as_bytes());
        let (diagnostics, syntax_error, same_as_run, dso) = match self {
            ScriptFormat::Torque3d => {
                let out =
                    torque_ts_compiler::compile(source, name, Purpose::Check, torque3d_format());
                (out.diagnostics, out.syntax_error, true, out.dso)
            }
            ScriptFormat::Classic => {
                let profile = torque_ts_compiler::classic::Profile::tge14();
                let out =
                    torque_ts_compiler::classic::compile(source, name, Purpose::Check, &profile);
                (out.diagnostics, out.syntax_error, !out.run_differs, out.dso)
            }
        };
        if syntax_error {
            return Err(format!("does not compile: {}", quoted(&diagnostics)));
        }
        if !same_as_run {
            return Err("compiles to other code than a run of it does".to_string());
        }
        Ok(dso)
    }

    fn to_bytes(self, dso: &DsoFile) -> Vec<u8> {
        match self {
            ScriptFormat::Torque3d => dso.to_bytes(torque3d_format()),
            ScriptFormat::Classic => TGE14_DIALECT.to_bytes(dso),
        }
    }

    /// Compiles `source` for its syntax: the reason when it has an error.
    pub fn check_source(self, path: &str, source: &[u8]) -> Result<(), String> {
        self.compile_checked(path, source).map(drop)
    }

    /// Compiles the script `source`, the file `path`. `Ok(None)` when it holds no statement (nothing to ship compiled); the
    /// reason when it has a syntax error, when its compiled path is longer than the engine reads, or when the compiled file is
    /// not one the engine's loader takes.
    pub fn compile(self, path: &str, source: &[u8]) -> Result<Option<Vec<u8>>, String> {
        let compiled_path = path.len() + EXTENSION.len();
        if compiled_path > MAX_DSO_PATH {
            return Err(format!(
                "its compiled path is {compiled_path} bytes; the engine reads at most {MAX_DSO_PATH}"
            ));
        }
        let Some(dso) = self.compile_checked(path, source)? else {
            return Ok(None);
        };
        let bytes = self.to_bytes(&dso);
        if bytes.len() > MAX_DSO_BYTES {
            return Err(format!(
                "compiles to {} bytes; the engine loads a compiled file of at most {MAX_DSO_BYTES}",
                bytes.len()
            ));
        }
        self.check_compiled(path, &bytes)?;
        Ok(Some(bytes))
    }

    /// Parses `bytes` as a compiled script of this format and loads it as the engine does: the reason when it does not.
    pub fn check_compiled(self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let format = self.dso_format();
        let dso = format.parse(bytes).map_err(|e| e.to_string())?;
        if dso.version != format.version() {
            return Err(format!(
                "version {}, the engine reads {}",
                dso.version,
                format.version()
            ));
        }
        CodeBlock::from_dso(&dso, Some(path.as_bytes()), format)
            .map(drop)
            .map_err(|e| e.to_string())
    }
}

/// Whether the file `path` of the game folder is a script `--precompile` compiles (`.cs` and `.gui`, case ignored).
pub fn is_source(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("cs") || ext.eq_ignore_ascii_case("gui"))
}

/// The folders a game's scripts are mods in: the mods folder (`game.toml`'s `[mods] folder`, else [`MODS_FOLDER`]) and the
/// folder of every `mod.toml` among `paths`, each lower case and ending in `/`.
pub fn mod_dirs<'a>(mods_folder: &str, paths: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let folder = mods_folder
        .replace('\\', "/")
        .trim_matches('/')
        .to_ascii_lowercase();
    let mut dirs = Vec::new();
    if !folder.is_empty() {
        dirs.push(format!("{folder}/"));
    }
    for path in paths {
        let (dir, name) = path.rsplit_once('/').map_or(("", path), |(d, n)| (d, n));
        if name.eq_ignore_ascii_case(MOD_MANIFEST) && !dir.is_empty() {
            dirs.push(format!("{}/", dir.to_ascii_lowercase()));
        }
    }
    dirs
}

/// Whether the script `path` belongs to a mod: it is below one of `mod_dirs` ([`mod_dirs`]).
pub fn in_mod(path: &str, mod_dirs: &[String]) -> bool {
    let lower = path.to_ascii_lowercase();
    mod_dirs.iter().any(|dir| lower.starts_with(dir.as_str()))
}

/// `game.toml`'s `[mods] folder`, the folder the default mod layout searches; [`MODS_FOLDER`] when the file says none or cannot
/// be read.
pub fn mods_folder(game_toml: Option<&[u8]>) -> String {
    game_toml
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|table| {
            table
                .get("mods")?
                .get("folder")?
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_else(|| MODS_FOLDER.to_string())
}

/// `text` with every control character replaced by `?`, so a message made of a file's bytes cannot move the cursor or set the
/// title of the terminal it is printed to.
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// The errors of a compile, the first lines of them, each cut: a hostile script cannot make a message long or hold escapes.
fn quoted(diagnostics: &[Diagnostic]) -> String {
    let lines: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.level == Level::Error)
        .flat_map(|d| {
            String::from_utf8_lossy(&d.text)
                .into_owned()
                .lines()
                .map(String::from)
                .collect::<Vec<_>>()
        })
        .filter(|line| !line.trim().is_empty())
        .take(QUOTED_LINES)
        .map(|line| printable(&line.chars().take(QUOTED_CHARS).collect::<String>()))
        .collect();
    if lines.is_empty() {
        "a syntax error".to_string()
    } else {
        lines.join(" | ")
    }
}

#[cfg(test)]
#[path = "precompile_tests.rs"]
mod tests;
