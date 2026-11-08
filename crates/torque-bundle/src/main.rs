//! `torque-bundle <game crate folder> [--out <dir>] [--target-dir <dir>] [--no-check] [--server] [--archive] [--twice]`:
//! packages the game for this computer (the library's docs say what goes in). `--server` packages its dedicated-server variant;
//! `--archive` also writes the bundle as a zip (Windows) or a gzipped tar, the same bytes from the same inputs; `--twice` builds
//! it two times from clean build folders (`<out>/one`, `<out>/two`) and fails unless the two archives have the same SHA-256.
//! `torque-bundle --verify <bundle folder>` checks a bundle against its manifest.
//!
//! The build folder is `--target-dir`, else `CARGO_TARGET_DIR`, else the workspace's `target`; the bundle goes in `--out`,
//! else `<build folder>/bundles`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use torque_bundle::{ArchiveReport, Options, Report, bundle, manifest};

const USAGE: &str = "usage: torque-bundle <game crate folder> [--out <dir>] [--target-dir <dir>] [--no-check] [--server] [--archive] [--twice]\n       torque-bundle --verify <bundle folder>";

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut crate_dir = None;
    let mut out = None;
    let mut target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let mut check = true;
    let mut server = false;
    let mut archive = false;
    let mut twice = false;
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
            "--server" => server = true,
            "--archive" => archive = true,
            "--twice" => twice = true,
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
        server,
        archive: archive || twice,
    };
    if twice {
        return build_twice(&options);
    }
    print_report(&bundle(&options)?);
    Ok(())
}

/// Two bundles of the same inputs, each from a build folder of its own that is removed after it (so nothing carries over, and a
/// path of the build folder in a binary shows as a difference). Fails unless the archives' SHA-256 are equal, naming the files
/// whose hashes differ.
fn build_twice(options: &Options) -> Result<(), String> {
    let mut runs: Vec<(Report, ArchiveReport)> = Vec::new();
    for run in ["one", "two"] {
        let build = Options {
            out: options.out.join(run),
            target_dir: options.target_dir.join(format!("twice-{run}")),
            ..options.clone()
        };
        let report = bundle(&build)?;
        print_report(&report);
        let _ = std::fs::remove_dir_all(&build.target_dir);
        let archive = report.archive.clone().ok_or("no archive was written")?;
        runs.push((report, archive));
    }
    let [(first, a), (second, b)] = &runs[..] else {
        return Err("two builds were expected".into());
    };
    println!("archive sha256 of build one: {}", a.sha256);
    println!("archive sha256 of build two: {}", b.sha256);
    if a.sha256 == b.sha256 {
        println!("the two archives are byte-identical ({} bytes)", a.bytes);
        return Ok(());
    }
    let differing = differing_files(&first.folder, &second.folder);
    Err(format!(
        "the two archives differ ({} and {} bytes); files that differ:\n  {}",
        a.bytes,
        b.bytes,
        differing.join("\n  ")
    ))
}

/// The paths whose size or hash differs between two bundle folders' manifests, or that only one has.
fn differing_files(one: &Path, two: &Path) -> Vec<String> {
    let read = |dir: &Path| {
        std::fs::read_to_string(dir.join(manifest::FILE))
            .ok()
            .and_then(|text| manifest::parse(&text).ok())
            .unwrap_or_default()
    };
    let (a, b) = (read(one), read(two));
    let mut paths: Vec<&str> = a.iter().chain(&b).map(|e| e.path.as_str()).collect();
    paths.sort_unstable();
    paths.dedup();
    let find = |list: &[manifest::Entry], path: &str| list.iter().find(|e| e.path == path).cloned();
    paths
        .into_iter()
        .filter(|path| find(&a, path) != find(&b, path))
        .map(String::from)
        .collect()
}

fn print_report(report: &Report) {
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
    if let Some(server) = &report.server {
        println!(
            "  server variant: {} crates linked, {} renderer, window or audio crates, {} GPU, window or audio system libraries imported",
            server.linked, server.drawing, server.system_libraries
        );
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
    if let Some(archive) = &report.archive {
        println!(
            "archive: {}  {} ({} bytes)",
            archive.sha256,
            archive.path.display(),
            archive.bytes
        );
    }
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
