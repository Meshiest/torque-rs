//! `just script-reference`: writes the guide's script reference pages (see the library's docs).
//!
//! `script-reference [--out DIR]`: DIR defaults to `book/src/reference/script` in this repository.

use std::path::PathBuf;
use std::process::ExitCode;

use torque_script_reference::engines::reference;
use torque_script_reference::render::{FOLDER, pages};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("script-reference: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../book/src")
        .join(FOLDER);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = PathBuf::from(args.next().ok_or("--out needs a folder")?),
            other => return Err(format!("unknown argument `{other}` (usage: [--out DIR])")),
        }
    }
    let pages = pages(&reference()?);
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    // The folder holds nothing but these pages: a page that is no longer written goes.
    for entry in std::fs::read_dir(&out).map_err(|e| format!("{}: {e}", out.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let known = pages
            .iter()
            .any(|(name, _)| path.file_name().is_some_and(|f| f == *name));
        if path.extension().is_some_and(|e| e == "md") && !known {
            std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    for (name, text) in &pages {
        let path = out.join(name);
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        println!(
            "{name}: {} lines, {} bytes",
            text.lines().count(),
            text.len()
        );
    }
    Ok(())
}
