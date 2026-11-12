//! `torque-bundle <game crate folder> [--out <dir>] [--target-dir <dir>] [--no-check] [--server] [--web] [--archive] [--twice] [--max-inflated <MiB>]
//! [--no-validate] [--precompile] [--classic-scripts]`:
//! packages the game for this computer (the library's docs say what goes in). `--server` packages its dedicated-server variant;
//! `--archive` also writes the bundle as a zip (Windows) or a gzipped tar, the same bytes from the same inputs; `--twice` builds
//! it two times from clean build folders (`<out>/one`, `<out>/two`) and fails unless the two archives have the same SHA-256.
//! Every file of the game folder is opened by the engine's loader for it first, and a file that is refused fails the bundle
//! (`--no-validate` skips that); `--precompile` ships the `.cs` and `.gui` scripts compiled in place of their source.
//! `--web` packages the game's browser build instead (the library's docs say what goes in); `--scripts <dir>` names the engine's
//! web scripts when `bundle.toml` does not name the `web` folder of the crate's workspace, and `--lua <Cargo.toml>` the Lua
//! extension; `--allow <dir>` (repeatable) lets `[[extension]]` and `[web].lua` name files in another folder.
//! `--classic-scripts` says the game's executor reads TGE 1.4's script format, not Torque3D's.
//! `torque-bundle --validate <game crate folder> [--precompile]` only opens the game folder's files (nothing is built), and
//! `torque-bundle --verify <bundle folder>` checks a bundle against its manifest.
//!
//! The build folder is `--target-dir`, else `CARGO_TARGET_DIR`, else the workspace's `target`; the bundle goes in `--out`,
//! else `<build folder>/bundles`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use torque_bundle::{
    ArchiveReport, Options, Report, bundle, manifest, own_folder, precompile,
    precompile::ScriptFormat, remove_owned_folder, scan, validate, validate_game,
};

/// Prints `text` to standard output or standard error a line at a time with every control character replaced
/// ([`precompile::printable`]). Everything this program prints goes through here, so no file name or message made of a file's
/// bytes (a game folder's, a bundle's or a check's output) can write an escape sequence to the terminal.
fn emit(text: &str, to_error: bool) {
    for line in text.lines() {
        let line = precompile::printable(line);
        if to_error {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
    }
}

macro_rules! say {
    ($($arg:tt)*) => { emit(&format!($($arg)*), false) };
}

macro_rules! say_err {
    ($($arg:tt)*) => { emit(&format!($($arg)*), true) };
}

const USAGE: &str = "usage: torque-bundle <game crate folder> [--out <dir>] [--target-dir <dir>] [--no-check] [--server] [--web] [--scripts <dir>] [--lua <Cargo.toml>] [--allow <dir>] [--archive] [--twice] [--max-inflated <MiB>] [--no-validate] [--precompile] [--classic-scripts]\n       torque-bundle --validate <game crate folder> [--precompile] [--classic-scripts]\n       torque-bundle --verify <bundle folder>";

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut crate_dir = None;
    let mut out = None;
    let mut target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let mut check = true;
    let mut server = false;
    let mut web = false;
    let mut scripts = None;
    let mut lua = None;
    let mut allow = Vec::new();
    let mut archive = false;
    let mut twice = false;
    let mut max_inflated = scan::MAX_INFLATED_BYTES;
    let mut mode = validate::Mode {
        validate: true,
        ..validate::Mode::default()
    };
    let mut only_validate = false;
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
                    say!("{}: every file matches the manifest", dir.display());
                    return Ok(());
                }
                return Err(problems.join("\n"));
            }
            "--validate" => only_validate = true,
            "--no-validate" => mode.validate = false,
            "--precompile" => mode.precompile = true,
            "--classic-scripts" => mode.format = ScriptFormat::Classic,
            "--out" => out = Some(value("--out")?),
            "--target-dir" => target_dir = Some(value("--target-dir")?),
            "--no-check" => check = false,
            "--server" => server = true,
            "--web" => web = true,
            "--scripts" => scripts = Some(value("--scripts")?),
            "--lua" => lua = Some(value("--lua")?),
            "--allow" => allow.push(value("--allow")?),
            "--archive" => archive = true,
            "--twice" => twice = true,
            "--max-inflated" => {
                let mib: usize = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--max-inflated needs a number of MiB")?;
                max_inflated = mib.saturating_mul(1 << 20);
            }
            "-h" | "--help" => {
                say!("{USAGE}");
                return Ok(());
            }
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}\n{USAGE}")),
            _ if crate_dir.is_none() => crate_dir = Some(PathBuf::from(arg)),
            _ => return Err(format!("one game crate at a time\n{USAGE}")),
        }
    }
    let crate_dir = crate_dir.ok_or(USAGE)?;
    let limits = validate::Limits::default();
    if only_validate {
        return validate_only(&crate_dir, &limits, mode);
    }
    let target_dir = target_dir.unwrap_or_else(|| PathBuf::from("target"));
    let options = Options {
        out: out.unwrap_or_else(|| target_dir.join("bundles")),
        crate_dir,
        target_dir,
        check,
        server,
        web,
        scripts,
        lua,
        allow,
        archive: archive || twice,
        max_inflated_bytes: max_inflated,
        mode,
        limits,
    };
    if twice {
        return build_twice(&options);
    }
    print_report(&bundle(&options)?);
    Ok(())
}

/// `--validate`: the game folder's files opened by their loaders, nothing built. Fails when one is refused.
fn validate_only(
    crate_dir: &Path,
    limits: &validate::Limits,
    mode: validate::Mode,
) -> Result<(), String> {
    let report = validate_game(crate_dir, limits, mode)?;
    for line in report.summary() {
        say!("{line}");
    }
    for skipped in &report.not_precompiled {
        say!("  left as source: {skipped}");
    }
    if report.refused.is_empty() {
        return Ok(());
    }
    let lines: Vec<String> = report.refused.iter().map(ToString::to_string).collect();
    Err(format!(
        "{} files refused:\n  {}",
        lines.len(),
        lines.join("\n  ")
    ))
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
        // A build folder of the user's own is never emptied or removed: only one this run made (it carries the marker).
        own_folder(&build.target_dir)?;
        let report = bundle(&build);
        let removed = remove_owned_folder(&build.target_dir);
        let report = report?;
        removed?;
        print_report(&report);
        let archive = report.archive.clone().ok_or("no archive was written")?;
        runs.push((report, archive));
    }
    let [(first, a), (second, b)] = &runs[..] else {
        return Err("two builds were expected".into());
    };
    say!("archive sha256 of build one: {}", a.sha256);
    say!("archive sha256 of build two: {}", b.sha256);
    if a.sha256 == b.sha256 {
        say!("the two archives are byte-identical ({} bytes)", a.bytes);
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
    say!("bundle: {}", report.folder.display());
    say!(
        "  {} files, {} bytes; features [{}]",
        report.files,
        report.bytes,
        report.features.join(", ")
    );
    if report.content.opened_total() > 0 || report.content.precompiled > 0 {
        for line in report.content.summary() {
            say!("  game folder: {line}");
        }
    }
    for skipped in &report.content.not_precompiled {
        say!("  left as source: {skipped}");
    }
    say!("  left out of the game folder: {}", report.skipped.len());
    for warning in &report.warnings {
        say!("  warning: {warning}");
    }
    for path in &report.skipped {
        say!("    {path}");
    }
    say!(
        "  licenses: {} crates, {} without a license text",
        report.crates,
        report.missing_licenses.len()
    );
    for name in &report.missing_licenses {
        say!("    no text: {name}");
    }
    for name in &report.generic_licenses {
        say!("    standard texts (the crate ships none): {name}");
    }
    if let Some(web) = &report.web {
        say!(
            "  web target: {} files listed in files.json, each equal to the file and to the manifest's hash; {} files in packs and {} others, each opened by its loader",
            web.listed,
            web.pack_files,
            web.other_files
        );
        if web.checks.is_empty() {
            say!("  web check: nothing to run");
        } else {
            say!("  web checks passed: {}", web.checks.join(", "));
        }
    }
    if let Some(server) = &report.server {
        say!(
            "  server variant: {} crates linked, {} renderer, window or audio crates, {} GPU, window or audio system libraries imported",
            server.linked,
            server.drawing,
            server.system_libraries
        );
    }
    say!("  leak scan: 0 local folders found");
    match &report.check {
        Some(out) => {
            say!("  check: passed from the bundle folder; the bundle still matches its manifest");
            for line in out.lines().take(12) {
                say!("    | {line}");
            }
        }
        None => say!("  check: not run"),
    }
    if let Some(archive) = &report.archive {
        say!(
            "archive: {}  {} ({} bytes)",
            archive.sha256,
            archive.path.display(),
            archive.bytes
        );
    }
}

fn main() -> ExitCode {
    // An interrupt stops what the bundle is running (a build, a headless browser) before this program ends.
    let _ = ctrlc::set_handler(|| {
        torque_bundle::kill_running();
        std::process::exit(130);
    });
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            say_err!("torque-bundle: {e}");
            ExitCode::FAILURE
        }
    }
}
