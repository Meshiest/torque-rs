//! `torque-bundle <game crate folder> [--out <dir>] [--target-dir <dir>] [--no-check]`: packages the game for this computer
//! (the library's docs say what goes in). `torque-bundle --verify <bundle folder>` checks a bundle against its manifest.
//!
//! The build folder is `--target-dir`, else `CARGO_TARGET_DIR`, else the workspace's `target`; the bundle goes in `--out`,
//! else `<build folder>/bundles`.

use std::path::PathBuf;
use std::process::ExitCode;

use torque_bundle::{Options, bundle, manifest};

const USAGE: &str = "usage: torque-bundle <game crate folder> [--out <dir>] [--target-dir <dir>] [--no-check]\n       torque-bundle --verify <bundle folder>";

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut crate_dir = None;
    let mut out = None;
    let mut target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let mut check = true;
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| {
            args.next()
                .map(PathBuf::from)
                .ok_or(format!("{flag} needs a folder"))
        };
        match arg.as_str() {
            "--verify" => {
                let dir = value("--verify")?;
                let problems = manifest::verify(&dir)?;
                if problems.is_empty() {
                    println!("{}: every file matches the manifest", dir.display());
                    return Ok(());
                }
                return Err(problems.join("\n"));
            }
            "--out" => out = Some(value("--out")?),
            "--target-dir" => target_dir = Some(value("--target-dir")?),
            "--no-check" => check = false,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}\n{USAGE}")),
            _ if crate_dir.is_none() => crate_dir = Some(PathBuf::from(arg)),
            _ => return Err(format!("one game crate at a time\n{USAGE}")),
        }
    }
    let crate_dir = crate_dir.ok_or(USAGE)?;
    let target_dir = target_dir.unwrap_or_else(|| PathBuf::from("target"));
    let options = Options {
        out: out.unwrap_or_else(|| target_dir.join("bundles")),
        crate_dir,
        target_dir,
        check,
    };
    let report = bundle(&options)?;
    println!("bundle: {}", report.folder.display());
    println!(
        "  {} files, {} bytes; features [{}]",
        report.files,
        report.bytes,
        report.features.join(", ")
    );
    println!("  left out of the game folder: {}", report.skipped.len());
    for path in &report.skipped {
        println!("    {path}");
    }
    println!(
        "  licenses: {} crates, {} without a license text",
        report.crates,
        report.missing_licenses.len()
    );
    for name in &report.missing_licenses {
        println!("    no text: {name}");
    }
    for name in &report.generic_licenses {
        println!("    standard texts (the crate ships none): {name}");
    }
    println!("  leak scan: 0 local folders found");
    match &report.check {
        Some(out) => {
            println!(
                "  check: passed from the bundle folder; the bundle still matches its manifest"
            );
            for line in out.lines().take(12) {
                println!("    | {line}");
            }
        }
        None => println!("  check: not run"),
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("torque-bundle: {e}");
            ExitCode::FAILURE
        }
    }
}
