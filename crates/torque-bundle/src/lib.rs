//! Packages a game made on torque-rs into a folder that runs on its own.
//!
//! The input is a game crate's folder with a [`bundle.toml`](spec) beside its `Cargo.toml`: the game folder (with its
//! `game.toml`, scripts, assets and mods), the binaries to ship, and the native extensions to build and ship beside them. The
//! output is `<out>/<name>-<os>-<arch>/`:
//!
//! | Path | What |
//! |---|---|
//! | `<bin>` (`.exe` on Windows) | the game's binaries, built in release with the plugins its `game.toml` keeps ([`features`]) |
//! | `game/` | the game folder, less the dev files ([`select`]), every file opened by its loader ([`validate`]); with `--precompile` its scripts compiled ([`precompile`]) |
//! | `plugins/` | each extension's library and the manifest the engine loads it with; the libraries are built first, and their hashes are given to the game's build ([`PLUGINS_ENV`]) |
//! | `licenses/` | the license texts of every crate linked, and an index ([`licenses`]) |
//! | `manifest.toml` | every file's size and SHA-256 ([`manifest`]) |
//!
//! The builds remap the computer's folders to neutral names and link reproducibly ([`cargo::rustflags`]); every file is then
//! scanned for those folders ([`scan`]) and one hit fails the bundle. Last, the `[check]` binary runs headless from the bundle
//! folder with the build's environment removed, and the bundle must still match its manifest after it.
//!
//! Before anything is built, every file of the game folder that an engine loader owns is opened by that loader under its caps
//! ([`validate`]), and one that is refused fails the bundle with its path and the reason (`--no-validate` skips this);
//! `--validate <game crate>` does only this. `--precompile` ships the `.cs` and `.gui` scripts as `<name>.dso` in place of
//! their source, which the engine loads without compiling ([`precompile`]).
//!
//! `--archive` also writes the bundle as one file ([`archive`]); `--server` packages the game's dedicated server ([`server`]).
//! `--twice` is for a game maker to verify that a game builds reproducibly: it bundles two times from clean build folders and
//! fails unless the two archives have the same SHA-256, naming the files that differ.
//!
//! # Maintaining
//!
//! **Module map.** `spec` (bundle.toml), `select` (which game files ship), `validate` (the game folder read within
//! [`validate::Limits`] and opened by the engine's loaders; `Kind` is the extension table), `precompile` (a script to its `.dso`),
//! `cargo` and `features` (the builds), `licenses`, `scan` (the leak scan), `manifest`, `archive`, `server`; `bundle` in this file
//! runs them.
//!
//! **Flow of the game folder.** `bundle` reads and opens the game folder first (`read_game`), before any build, so a refused
//! file costs seconds: `select` lists the files, `validate::process` reads each one with the no-follow, capped reader
//! (`torque_resource::plain::read_plain`), opens it with the loader of its kind, compiles it when `--precompile`, and hands back
//! the bytes to ship. Those bytes are what the manifest hashes, the leak scan reads and the folder and archive hold: a file is
//! read once, so what was opened is what ships. Every other file of the bundle is read through the same reader, capped by
//! `Limits::total_bytes`, and moved (not copied) into the list that is hashed and written. With neither validation nor
//! precompile the game files are read at that point instead.
//!
//! **The game folder is untrusted input.** The walk (`select`) stops past `Limits::depth` folders and `Limits::files` entries, as
//! it goes. A file over `Limits::file_bytes` is refused before it is read; the folder as a whole is held to `total_bytes`; each
//! loader keeps its own byte caps (`Kind::max_bytes`; a `mod.toml` is read at the mods plugin's 64 KiB) and, where it charges
//! work, `Limits::work`; a zip is mounted with `ZipMount` (its entry names are the engine's: an entry that would leave the
//! folder, or that the engine does not serve, is never opened) and its entries inflate at most `Limits::inflated_bytes` in all.
//! A name the engine does not serve (`torque_resource::zips::entry_name_ok`) is refused, as are two paths that differ only in
//! case, so no path in the bundle holds `..`, a drive or a device name, and a compiled script's path is its source's plus
//! `.dso`. Every refusal and note is printable text (`precompile::printable`): a file's own bytes cannot reach the terminal as
//! escapes. The loaders are the engine's own and are fuzzed, so a hostile file is an error value, not a panic. The errors of
//! this crate stay `String`s, as the rest of it, rather than an enum: nothing matches on them.
//!
//! **What is not opened.** `.lua` and anything a plugin owns are unowned. So are `.obj`, `.bbmodel` and `.map`: the engine reads
//! them only when a game turns their format plugin on (off by default, trusted files only), and their loaders live in
//! `torque-content`, which links the engine. A `.dif` is parsed as the engine loads it; its stricter cross-reference check is
//! not run, since the engine loads files that fail it.
//!
//! **Precompile.** The scripts are compiled in the format the engine's executor reads (`ScriptFormat`: Torque3D's, or TGE 1.4's
//! with `--classic-scripts`) as `exec` compiles a source with no syntax error, then parsed and loaded again by the engine's
//! loader before they ship; one that does not compile stays source and is reported (`Report::not_precompiled`). A compiled file
//! is shipped in place of its source, never beside it, since the engine prefers a `.dso` only when it is at least as new as the
//! source. Mission files (`.mis`) have no compiled form, a script of only comments has no code, a `.dso` already in the game
//! folder is never overwritten, a script whose `.dso` path would pass the 511 bytes the engine builds it in stays source, and a
//! mod's scripts stay source: below `game.toml`'s `[mods] folder` (default `mods`) or a folder with a `mod.toml` (any case),
//! since the engine never runs a compiled script that came with a mod. A game with a `ModLayout` of its own that finds mods
//! otherwise is not seen. Scripts inside zips are not rewritten, and the Lua extension's scripts are not touched.
//!
//! **A compiled file is trusted.** The engine runs a `.dso`'s top level as trusted and a source's as not, and the flag reaches
//! `DeclarePolicy::may_declare`. The engine's own policy ignores it, so nothing differs for a game that installs none; a game
//! whose policy refuses names to its source scripts would see them allowed once compiled, so it must not use `--precompile`.
//!
//! **Invariants and their tests.**
//! - The mods demo's game folder opens with nothing refused, and its three entry scripts compile while the two mod scripts stay
//!   source (`tests/content.rs::the_mods_demo_game_folder_opens_with_nothing_refused`; `just bundle-content-check`).
//! - A planted bad asset (loose, inside a zip, a broken zip, a script with a syntax error) is named with its path and reason,
//!   on the command line too (`a_planted_bad_asset_is_reported_with_its_path_and_the_reason`), and a bundle fails before it
//!   builds anything; `--no-validate` ships it as it is
//!   (`a_bundle_with_a_refused_asset_fails_before_anything_is_built_unless_validation_is_off`).
//! - Every kind has a good file its loader takes, and a cut, empty or garbled one is refused
//!   (`validate::tests::a_good_file_of_each_kind_opens`, `a_cut_empty_or_garbled_file_of_each_kind_is_refused`); a loader's work
//!   is bounded (`a_loader_that_charges_work_stops_when_the_budget_is_spent`); the kinds are the engine's
//!   (`kinds_follow_the_extensions_the_engine_reads`,
//!   `an_interior_the_engine_loads_is_not_refused_for_failing_the_cross_reference_check`).
//! - The limits hold (`the_limits_hold_a_hostile_game_folder`,
//!   `the_walk_of_a_deep_or_wide_folder_stops_where_it_passes_its_caps`),
//!   names that would escape are never opened (`a_zip_entry_named_to_escape_the_folder_is_never_opened`), and two names of one
//!   file are refused (`paths_that_differ_only_in_case_are_refused_and_so_is_a_dso_name_that_collides`).
//! - No control character reaches a message
//!   (`validate::tests::control_characters_in_a_path_or_a_reason_never_reach_the_message`,
//!   `tests/content.rs::a_refusal_made_of_a_files_text_holds_no_control_character`).
//! - A compiled script runs as its source does: in a script engine, in both formats
//!   (`precompile::tests::a_compiled_script_runs_in_place_of_its_source_and_does_the_same`,
//!   `a_game_on_the_classic_executor_gets_its_own_format`) and as the trace world's server script put through the bundle's own
//!   game-folder pass, whose hash is the committed one (`torque-trace-hash/tests/precompiled.rs`); a compile gives the same
//!   bytes every time (`a_compile_gives_the_same_bytes_every_time`, which the reproducible archive relies on).
//! - A mod's scripts stay source (`tests/content.rs::a_mods_scripts_stay_source`,
//!   `a_mods_folder_game_toml_names_keeps_its_scripts_as_source`, `a_manifest_in_any_case_makes_a_folder_a_mod`), which the mods demo's bundle check (a round with the soccer mod downloaded)
//!   depends on; a script with a path too long for its `.dso` stays source
//!   (`a_script_whose_compiled_path_is_too_long_stays_source_with_a_note`).
//! - The constants copied from the mods plugin are equal to its own (`validate::tests::the_mod_constants_are_the_mods_plugins`;
//!   the tool does not link it, only its tests do).
//! - A bundle with `--precompile` holds `boot.cs.dso` and not `boot.cs`, and still matches its manifest
//!   (`a_precompiled_bundle_ships_the_compiled_script_in_place_of_its_source`); an existing `.dso` is left alone
//!   (`a_compiled_file_already_beside_its_source_is_not_overwritten`).
//!
//! **Extending.** A new asset format: add its extension to `Kind::of`, its label and cap, and its loader call in
//! `validate::check`, with a good file and a bad one in `validate_tests.rs`. A format a plugin owns (the Lua scripts) is not
//! checked here. A new script format: a variant of `ScriptFormat` with its compile, its bytes and its loader. **Check commands:**
//! `just bundle --validate <game crate>` (no build), `just bundle-content-check` (the exit checks) and
//! `cargo test -p torque-bundle`.
//!
//! **The leak scan** (`scan`, one `Scanner` over every needle: an Aho-Corasick automaton) looks for each folder of the build
//! computer in every file of the bundle, with backslashes and case folded, in every spelling a path takes (doubled slashes,
//! percent-encoding with `%2f` and `%5c` separators, MSYS and WSL paths, 8.3 short names that keep `-` and `_`, at most 256 for a folder; UTF-8, UTF-16 LE and BE), and inside what a game
//! folder holds compressed: every zip of a file (found by its end record, which counts only when it points at a central
//! directory, possibly with a zip64 record or signature between them, since a program's code holds the signature bytes), zips in zips, gzip, raw zlib and compressed PNG text chunks, up
//! to `Options::max_inflated_bytes` for each file. A hit is `Hit::Leak` or `Hit::Unscannable`, and both fail the bundle: a zip that
//! does not open, more members than the scan reads, a container nested too deep, a PNG cut short or with a text chunk that does not
//! parse, and xz, zstd, bzip2, 7z and WOFF files (not decoded). Every plausible end record opens a zip of its own and none is
//! skipped (a stored zip inside a zip is found twice and its repeated hits are dropped); a skip on any ground could hide a zip. The user and host
//! name (variables, the home folder, the host name files) are only warnings, and only as a path component (`runner` is a word).
//! A new place a path can hide gets a spelling or a container and a test.
//!
//! **Invariants and their tests.**
//! - Spellings, short names and names (`scan::tests::every_spelling_of_a_folder_is_found`,
//!   `percent_encoded_and_short_name_spellings_are_found`, `short_names_follow_the_8_3_rule_and_keep_dashes_and_underscores`,
//!   `user_and_host_names_are_soft_and_only_between_separators`,
//!   `the_names_of_this_computer_come_from_the_variables_the_home_folder_and_the_host_files`, `short_names_in_edge_cases`; end to end
//!   `tests/bundle.rs::a_binary_that_holds_the_build_servers_user_name_bundles_with_a_warning`).
//! - Containers, with the member named and a leaking member name not printed
//!   (`what_is_compressed_inside_a_file_is_scanned_and_the_member_named`, `every_zip_of_a_file_is_scanned_not_only_the_last`,
//!   `a_zip_behind_other_bytes_is_found_by_its_end_record`, `compressed_formats_the_scan_does_not_decode_are_unscannable`,
//!   `the_inflate_limit_is_the_callers`, `a_crafted_directory_offset_cannot_hide_another_zip`,
//!   `a_zip_hidden_in_an_entry_the_scan_does_not_read_is_still_found`; end to end
//!   `tests/bundle.rs::a_folder_inside_a_zip_and_its_other_spellings_in_plain_files_are_refused_by_name`,
//!   `the_inflate_limit_of_the_leak_scan_is_a_flag`).
//! - What cannot be read is reported (`nesting_is_bounded_and_reported`, `something_that_looks_like_a_zip_and_does_not_open_is_reported`,
//!   `a_zip_with_more_members_than_the_scan_reads_is_reported`, `a_truncated_or_malformed_png_is_reported`).
//! - Only a folder this tool made (it holds the marker) is emptied or removed, `--twice`'s build folders included
//!   (`tests/main.rs::a_twice_run_leaves_a_build_folder_it_did_not_make_alone`).
//! - `--verify` follows no link, in a folder on the way or as the file, and reads through `torque_resource::plain`
//!   (`tests/main.rs::a_file_below_a_linked_folder_is_not_read`, `verify_does_not_follow_a_link_out_of_the_bundle`, Unix only).
//! - Every detector runs on every file (`every_detector_runs_on_a_file_that_is_more_than_one_container`); a gap before an end record
//!   (`a_gap_between_the_central_directory_and_the_end_record_is_still_a_zip`), the short-name cap
//!   (`short_name_spellings_are_capped_with_the_lowest_numbers_first`) and encoded separators (`encoded_separators_are_found`).

/// The variable the game's build is given: the SHA-256 (lower-case hex) of each extension library the bundle ships in
/// `plugins/`, separated by spaces. A game reads it at compile time (`option_env!`) to approve those libraries without
/// asking, and nothing put beside them later.
pub const PLUGINS_ENV: &str = "TORQUE_BUNDLE_PLUGINS";

pub mod archive;
pub mod cargo;
pub mod features;
pub mod licenses;
pub mod manifest;
pub mod precompile;
pub mod scan;
pub mod select;
pub mod server;
pub mod spec;
pub mod validate;

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cargo::{Build, Metadata, Toolchain};
use features::Toggles;
use licenses::Text;
use manifest::{Entry, Header};
use scan::{Needle, Scanner};
use spec::Spec;
use torque_resource::plain::read_plain;

/// What to package and where.
#[derive(Clone, Debug)]
pub struct Options {
    /// The game crate's folder (with `bundle.toml`).
    pub crate_dir: PathBuf,
    /// Where the bundle folder goes.
    pub out: PathBuf,
    /// The build folder; the bundle's builds go in `bundle/` under it.
    pub target_dir: PathBuf,
    /// Run the `[check]` after packaging.
    pub check: bool,
    /// Package the dedicated-server variant (`[server]` in `bundle.toml`).
    pub server: bool,
    /// Also write the bundle as one archive beside its folder: a zip on Windows, a gzipped tar elsewhere ([`archive`]).
    pub archive: bool,
    /// The most bytes the leak scan inflates for one file of the bundle, over everything compressed inside it
    /// ([`scan::MAX_INFLATED_BYTES`] by default on the command line); a file that needs more fails the bundle as unscannable.
    pub max_inflated_bytes: usize,
    /// What is done with the game folder's files besides shipping them: opened by their loaders and refused when they fail
    /// ([`validate`]; off, they ship unread) and its `.cs` and `.gui` scripts compiled in place of their source ([`precompile`]).
    pub mode: validate::Mode,
    /// The caps on the game folder and on the work of opening it.
    pub limits: validate::Limits,
}

/// What the server variant's checks counted. A bundle is refused when a count that must be 0 is not, so a report holds zeros.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerReport {
    /// The crates the binaries link.
    pub linked: usize,
    /// Of them, renderer, window, audio, GUI or client crates.
    pub drawing: usize,
    /// GPU, window or audio system libraries the binaries import.
    pub system_libraries: usize,
}

/// A written archive.
#[derive(Clone, Debug)]
pub struct ArchiveReport {
    pub path: PathBuf,
    pub bytes: u64,
    /// The SHA-256 of the archive's bytes, in lowercase hex.
    pub sha256: String,
}

/// What a bundle came to.
#[derive(Clone, Debug)]
pub struct Report {
    pub folder: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub features: Vec<String>,
    pub skipped: Vec<String>,
    pub crates: usize,
    pub missing_licenses: Vec<String>,
    /// Crates that ship no license text, given the standard texts of their licenses.
    pub generic_licenses: Vec<String>,
    /// The check's standard output, when it ran.
    pub check: Option<String>,
    /// What the server variant's check found, when it was packaged.
    pub server: Option<ServerReport>,
    pub archive: Option<ArchiveReport>,
    /// What opening the game folder's files came to ([`validate`]); empty when neither validation nor precompile ran.
    pub content: validate::Report,
    /// What the leak scan noticed that does not fail a bundle: the user or host name of the build computer as a path component.
    pub warnings: Vec<String>,
}

/// The bundle folder's name for `name` on `target`: `name-windows-x86_64`.
pub fn folder_name(name: &str, target: &str) -> String {
    let arch = target.split('-').next().unwrap_or(target);
    let os = if target.contains("windows") {
        "windows"
    } else if target.contains("apple-darwin") {
        "macos"
    } else if target.contains("linux") {
        "linux"
    } else {
        target.split('-').nth(2).unwrap_or("unknown")
    };
    format!("{name}-{os}-{arch}")
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Packages the game (module docs).
pub fn bundle(options: &Options) -> Result<Report, String> {
    let crate_dir = absolute(&options.crate_dir)?;
    let target_dir = absolute(&options.target_dir)?;
    let spec = Spec::load(&crate_dir)?.variant(options.server)?;
    let toolchain = Toolchain::find()?;
    let target = toolchain.host.clone();
    let game_dir = crate_dir.join(&spec.bundle.game);
    let toggles = match std::fs::read_to_string(game_dir.join("game.toml")) {
        Ok(text) => Toggles::parse(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Toggles::default(),
        Err(e) => return Err(format!("game.toml: {e}")),
    };

    // The game folder, read and opened before anything is built: a file its loader refuses fails the bundle in seconds.
    let game = read_game(&game_dir, &spec.bundle.exclude, options)?;

    let manifest_path = crate_dir.join("Cargo.toml");
    let members = Metadata::read(&manifest_path, &target, false)?;
    let package = members.package(&spec.bundle.package).ok_or_else(|| {
        format!(
            "bundle.toml: the package {} is not in this workspace",
            spec.bundle.package
        )
    })?;
    // The server variant has no window or sound device to switch plugins for: only the features it names.
    let features = if options.server {
        spec.bundle.features.clone()
    } else {
        features::derive(&toggles, &package.features, &spec.bundle.features)
    };

    // The computer's folders: remapped in the builds (most specific last) and looked for in every file.
    let home = env_dir("USERPROFILE").or_else(|| env_dir("HOME"));
    let cargo_home = env_dir("CARGO_HOME").or_else(|| home.as_ref().map(|h| h.join(".cargo")));
    let rustup_home = env_dir("RUSTUP_HOME").or_else(|| home.as_ref().map(|h| h.join(".rustup")));
    let mut folders: Vec<(&str, PathBuf, &str)> = Vec::new();
    if let Some(home) = &home {
        folders.push(("the home folder", home.clone(), "/home"));
    }
    folders.push((
        "the workspace",
        members.workspace_root.clone(),
        "/workspace",
    ));
    let mut extension_roots = Vec::new();
    for extension in &spec.extensions {
        let ext = Metadata::read(&crate_dir.join(&extension.manifest), &target, false)?;
        folders.push((
            "an extension's workspace",
            ext.workspace_root.clone(),
            "/extension",
        ));
        extension_roots.push(ext.workspace_root);
    }
    if let Some(dir) = &cargo_home {
        folders.push(("the cargo home", dir.clone(), "/cargo"));
    }
    if let Some(dir) = &rustup_home {
        folders.push(("the rustup home", dir.clone(), "/rustup"));
    }
    folders.push((
        "the toolchain",
        toolchain.sysroot.clone(),
        "/rustup/toolchain",
    ));
    folders.push(("the build folder", target_dir.clone(), "/target"));
    let remaps: Vec<(PathBuf, &str)> = folders
        .iter()
        .map(|(_, path, to)| (path.clone(), *to))
        .collect();
    let needles: Vec<Needle> = folders
        .iter()
        .filter_map(|(label, path, _)| Needle::new(label, path))
        .chain(Needle::new("the crate folder", &crate_dir))
        .chain(scan::name_needles(
            |name| std::env::var(name).ok().filter(|v| !v.is_empty()),
            |path| std::fs::read_to_string(path).ok(),
            home.as_deref(),
        ))
        .collect();
    let scanner = Scanner::new(needles, options.max_inflated_bytes)?;

    // The game's binaries.
    let mut game_build = Build {
        manifest: manifest_path.clone(),
        package: Some(spec.bundle.package.clone()),
        bins: spec.bundle.bins.clone(),
        features: Some(features.clone()),
        target: target.clone(),
        target_dir: target_dir.join("bundle"),
        rustflags: cargo::rustflags(&remaps, &target, &[]),
        cflags: cargo::cflags(&remaps, &target),
        env: Vec::new(),
    };
    // The server variant is checked before it is built: a crate that draws is a refusal in seconds, not after the compile.
    let game_linked = game_build.linked()?;
    let mut server_linked = game_linked.len();
    if options.server {
        refuse_drawing(&game_linked)?;
    }
    // The extensions, each from its own workspace and build folder: built before the game, whose build is told their
    // libraries' hashes ([`PLUGINS_ENV`]), so a game can approve the libraries it shipped without reading anything beside
    // them.
    let mut extension_builds = Vec::new();
    let mut shipped = Vec::new();
    for (extension, root) in spec.extensions.iter().zip(&extension_roots) {
        let name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("extension")
            .to_string();
        let build = Build {
            manifest: crate_dir.join(&extension.manifest),
            package: None,
            bins: Vec::new(),
            features: None,
            target: target.clone(),
            target_dir: target_dir.join("bundle-extensions").join(&name),
            rustflags: cargo::rustflags(&remaps, &target, &extension.rustflags),
            cflags: cargo::cflags(&remaps, &target),
            env: Vec::new(),
        };
        let made = build.run()?;
        let [library] = made.libraries.as_slice() else {
            return Err(format!(
                "the extension {name} built {} libraries; one is shipped",
                made.libraries.len()
            ));
        };
        let bytes = std::fs::read(library).map_err(|e| format!("{}: {e}", library.display()))?;
        shipped.push((
            library.clone(),
            crate_dir.join(&extension.config),
            manifest::Entry::of("", &bytes).sha256,
        ));
        extension_builds.push(build);
    }
    let hashes: Vec<&str> = shipped.iter().map(|(_, _, sha)| sha.as_str()).collect();
    game_build
        .env
        .push((PLUGINS_ENV.to_string(), hashes.join(" ")));
    let built = game_build.run()?;
    if options.server {
        refuse_system_libraries(&built.bins)?;
    }
    let mut files: BTreeMap<String, Text> = BTreeMap::new();
    let add = |files: &mut BTreeMap<String, Text>, path: String, source: Text| {
        if files.insert(path.clone(), source).is_some() {
            return Err(format!("two files would be {path} in the bundle"));
        }
        Ok(())
    };
    for (_, exe) in &built.bins {
        add(&mut files, file_name(exe)?, Text::File(exe.clone()))?;
    }
    for (library, config, _) in &shipped {
        add(
            &mut files,
            format!("plugins/{}", file_name(library)?),
            Text::File(library.clone()),
        )?;
        add(
            &mut files,
            format!("plugins/{}", file_name(config)?),
            Text::File(config.clone()),
        )?;
    }

    for (path, text) in game.files {
        add(&mut files, format!("game/{path}"), text)?;
    }

    // The licenses of what the builds link.
    let game_meta = Metadata::read(&manifest_path, &target, true)?;
    let mut extension_meta = Vec::new();
    for build in &extension_builds {
        let linked = build.linked()?;
        if options.server {
            refuse_drawing(&linked)?;
            server_linked += linked.len();
        }
        extension_meta.push((Metadata::read(&build.manifest, &target, true)?, linked));
    }
    let mut builds: Vec<(&Metadata, &[(String, String)])> = vec![(&game_meta, &game_linked)];
    builds.extend(extension_meta.iter().map(|(m, l)| (m, l.as_slice())));
    let gathered = licenses::gather(&builds, &toolchain);
    for (path, text) in &gathered.files {
        add(&mut files, path.clone(), text.clone())?;
    }

    // Everything in memory, scanned before anything is written: a bundle that holds a folder of this computer never reaches
    // the disk.
    let mut written: Vec<(String, Vec<u8>)> = Vec::new();
    for (path, text) in files {
        let bytes = match text {
            Text::File(source) => read_plain(&source, options.limits.total_bytes, None)
                .map_err(|e| format!("{path}: {e}"))?,
            Text::Made(text) => text.into_bytes(),
            Text::Bytes(bytes) => bytes,
        };
        written.push((path, bytes));
    }
    written.push((
        format!("{}/index.txt", licenses::DIR),
        gathered.index.clone().into_bytes(),
    ));
    let entries: Vec<Entry> = written.iter().map(|(p, b)| Entry::of(p, b)).collect();
    let header = Header {
        name: spec.bundle.name.clone(),
        version: package.version.clone(),
        target: target.clone(),
        mode: "compiled".to_string(),
        features: features.clone(),
        rustc: toolchain.version.clone(),
    };
    let manifest_text = manifest::write(&header, &entries);
    written.push((manifest::FILE.to_string(), manifest_text.into_bytes()));
    let mut leaks = Vec::new();
    let mut warnings = Vec::new();
    for (path, bytes) in &written {
        for hit in scanner.scan_file(bytes) {
            let line = hit.describe(path);
            if hit.is_hard() {
                leaks.push(line);
            } else {
                warnings.push(line);
            }
        }
    }
    if !leaks.is_empty() {
        return Err(format!(
            "the bundle holds folders of this computer, or something the scan could not look into (nothing was written):\n  {}",
            leaks.join("\n  ")
        ));
    }

    // Written beside the folder, then moved over it: a bundle folder is whole or not there. An old one is replaced only
    // when it is a bundle.
    let out = absolute(&options.out)?;
    let name = folder_name(&spec.bundle.name, &target);
    let folder = out.join(&name);
    if folder.exists() {
        let is_bundle = folder.join(manifest::FILE).is_file();
        let empty = std::fs::read_dir(&folder).is_ok_and(|mut d| d.next().is_none());
        if !is_bundle && !empty {
            return Err(format!(
                "{} is not a bundle folder (no {}); it is left as it is",
                folder.display(),
                manifest::FILE
            ));
        }
    }
    let executable: Vec<String> = built
        .bins
        .iter()
        .filter_map(|(_, exe)| file_name(exe).ok())
        .collect();
    let staging = out.join(format!("{name}.partial"));
    own_folder(&staging)?;
    for (path, bytes) in &written {
        let dest = staging.join(path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        write_file(&dest, bytes, is_executable(path, &executable))
            .map_err(|e| format!("{path}: {e}"))?;
    }
    std::fs::remove_file(staging.join(MARKER)).map_err(|e| format!("{MARKER}: {e}"))?;
    if folder.exists() {
        std::fs::remove_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    }
    std::fs::rename(&staging, &folder).map_err(|e| format!("{}: {e}", folder.display()))?;

    let check = match (&spec.check, options.check) {
        (Some(check), true) => {
            let exe = built
                .bins
                .iter()
                .find(|(name, _)| *name == check.bin)
                .map(|(_, exe)| folder.join(file_name(exe).unwrap_or_default()))
                .ok_or("the check's binary was not built")?;
            let work = out.join(format!("{name}.work"));
            let out = run_check(&exe, &folder, &work, check)?;
            let problems = manifest::verify(&folder)?;
            if !problems.is_empty() {
                return Err(format!(
                    "the check changed the bundle:\n  {}",
                    problems.join("\n  ")
                ));
            }
            Some(out)
        }
        _ => None,
    };

    let archive = if options.archive {
        Some(write_archive(&out, &name, &target, &written, &executable)?)
    } else {
        None
    };

    Ok(Report {
        folder,
        files: written.len(),
        bytes: written.iter().map(|(_, b)| b.len() as u64).sum(),
        features,
        skipped: game.skipped,
        crates: gathered.crates,
        missing_licenses: gathered.missing,
        generic_licenses: gathered.generic,
        check,
        server: options.server.then_some(ServerReport {
            linked: server_linked,
            drawing: 0,
            system_libraries: 0,
        }),
        archive,
        content: game.content,
        warnings,
    })
}

/// Opens every file of the game crate `crate_dir`'s game folder with its loader under `limits` ([`validate`]), and compiles its
/// scripts too when `mode` says (to see which would not), without building or writing anything. The report holds the refusals.
pub fn validate_game(
    crate_dir: &Path,
    limits: &validate::Limits,
    mode: validate::Mode,
) -> Result<validate::Report, String> {
    let crate_dir = absolute(crate_dir)?;
    let spec = Spec::load(&crate_dir)?;
    let game_dir = crate_dir.join(&spec.bundle.game);
    let (selected, _) = select::select(&game_dir, &spec.bundle.exclude, limits.walk())?;
    let game = validate::GameInfo::read(&game_dir);
    let mode = validate::Mode {
        validate: true,
        ..mode
    };
    Ok(validate::process(&selected, limits, mode, &game).1)
}

/// The game folder, read: the files to ship (their path in it and contents), what opening them came to, and the paths left out.
struct GameFolder {
    files: Vec<(String, Text)>,
    content: validate::Report,
    skipped: Vec<String>,
}

/// Reads the game folder ([`GameFolder`]) as `options` say.
fn read_game(game_dir: &Path, exclude: &[String], options: &Options) -> Result<GameFolder, String> {
    let (selected, skipped) = select::select(game_dir, exclude, options.limits.walk())?;
    let mode = options.mode;
    if !mode.validate && !mode.precompile {
        let files = selected
            .into_iter()
            .map(|file| (file.path, Text::File(file.source)))
            .collect();
        return Ok(GameFolder {
            files,
            content: validate::Report::default(),
            skipped,
        });
    }
    let game = validate::GameInfo::read(game_dir);
    let (read, content) = validate::process(&selected, &options.limits, mode, &game);
    if !content.refused.is_empty() {
        let lines: Vec<String> = content.refused.iter().map(ToString::to_string).collect();
        return Err(format!(
            "{} files of the game folder were refused by their loaders (nothing was built or written; --no-validate skips the check):\n  {}",
            lines.len(),
            lines.join("\n  ")
        ));
    }
    let files = read
        .into_iter()
        .map(|file| (file.path, Text::Bytes(file.bytes)))
        .collect();
    Ok(GameFolder {
        files,
        content,
        skipped,
    })
}

/// An error naming the renderer, window or audio crates in `linked`, if there are any (the server variant links none).
fn refuse_drawing(linked: &[(String, String)]) -> Result<(), String> {
    let drawing = server::drawing_crates(linked);
    if drawing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the server variant links {} renderer, window or audio crates:\n  {}",
        drawing.len(),
        drawing.join("\n  ")
    ))
}

/// An error naming the GPU, window and audio system libraries the server variant's binaries `bins` (name, file) import.
fn refuse_system_libraries(bins: &[(String, PathBuf)]) -> Result<(), String> {
    let mut found = Vec::new();
    for (name, exe) in bins {
        let bytes = std::fs::read(exe).map_err(|e| format!("{}: {e}", exe.display()))?;
        for library in server::system_libraries(&bytes) {
            found.push(format!("{name}: {library}"));
        }
    }
    if found.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the server variant's binaries import {} GPU, window or audio system libraries:\n  {}",
        found.len(),
        found.join("\n  ")
    ))
}

/// Writes a file of the bundle folder: on Unix an executable one ([`is_executable`]) gets the archive's executable mode, so
/// the folder's binaries run as they are, the check's among them.
fn write_file(dest: &Path, bytes: &[u8], executable: bool) -> std::io::Result<()> {
    std::fs::write(dest, bytes)?;
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(
            dest,
            std::fs::Permissions::from_mode(archive::EXECUTABLE_MODE),
        )?;
    }
    #[cfg(not(unix))]
    let _ = executable;
    Ok(())
}

/// Whether `path` of the bundle gets the executable mode: a binary of the bundle, or a shared library in `plugins/`.
fn is_executable(path: &str, bins: &[String]) -> bool {
    let library = path.starts_with("plugins/")
        && [".dll", ".so", ".dylib"]
            .iter()
            .any(|ending| path.ends_with(ending));
    library || bins.iter().any(|bin| bin == path)
}

/// Writes the bundle `written` as `<out>/<name>.<zip or tar.gz>` (through a partial file, so one is whole or not there). The
/// files `executable` get the executable mode, and so does every shared library (`.dll`, `.so`, `.dylib`) in `plugins/`.
fn write_archive(
    out: &Path,
    name: &str,
    target: &str,
    written: &[(String, Vec<u8>)],
    executable: &[String],
) -> Result<ArchiveReport, String> {
    use sha2::{Digest, Sha256};

    let items: Vec<archive::Item> = written
        .iter()
        .map(|(path, bytes)| archive::Item {
            path,
            bytes,
            executable: is_executable(path, executable),
        })
        .collect();
    let format = archive::Format::for_target(target);
    let bytes = archive::build(name, &items, format)?;
    let path = out.join(format!("{name}.{}", format.extension()));
    let partial = out.join(format!("{name}.{}.partial", format.extension()));
    std::fs::write(&partial, &bytes).map_err(|e| format!("{}: {e}", partial.display()))?;
    std::fs::rename(&partial, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    let sha256 = torque_bytes::hex(&Sha256::digest(&bytes));
    Ok(ArchiveReport {
        path,
        bytes: bytes.len() as u64,
        sha256,
    })
}

/// The file this tool puts in a folder it made (a bundle being written, a check's work folder), so that it only ever empties
/// folders of its own.
pub const MARKER: &str = ".torque-bundle-folder";

/// Makes `dir` an empty folder of this tool's, with its [`MARKER`]: one that is missing is made, one with the marker is
/// emptied, and any other folder with something in it is an error and left as it is.
pub fn own_folder(dir: &Path) -> Result<(), String> {
    if dir.exists() {
        let ours = dir.join(MARKER).is_file();
        let empty = std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none());
        if !ours && !empty {
            return Err(format!(
                "{} is not a folder this tool made (no {MARKER}); it is left as it is",
                dir.display()
            ));
        }
        std::fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(dir.join(MARKER), "made by torque-bundle\n")
        .map_err(|e| format!("{}: {e}", dir.display()))
}

/// Removes `dir` when it carries the [`MARKER`] (a folder [`own_folder`] made), and leaves any other folder as it is.
pub fn remove_owned_folder(dir: &Path) -> Result<(), String> {
    if !dir.join(MARKER).is_file() {
        return Err(format!(
            "{} is not a folder this tool made (no {MARKER}); it is left as it is",
            dir.display()
        ));
    }
    std::fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))
}

fn file_name(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(String::from)
        .ok_or_else(|| format!("{} has no file name", path.display()))
}

/// Runs the check's binary `exe` in the bundle folder `dir` with `{work}` as `work` (made or emptied by [`own_folder`]), no cargo or build
/// variables in its environment, until it exits or its time is up. Its standard output, when it exits 0 and prints every
/// expected line.
pub fn run_check(
    exe: &Path,
    dir: &Path,
    work: &Path,
    check: &spec::Check,
) -> Result<String, String> {
    own_folder(work)?;
    let work_text = work.to_string_lossy();
    let args: Vec<String> = check
        .args
        .iter()
        .map(|a| a.replace("{work}", &work_text))
        .collect();
    let mut command = Command::new(exe);
    command
        .args(&args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if text.starts_with("CARGO") || text.starts_with("RUST") || text.starts_with("TORQUE_") {
            command.env_remove(&name);
        }
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("the check {}: {e}", exe.display()))?;
    let reader = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut text = Vec::new();
            let _ = pipe.read_to_end(&mut text);
            String::from_utf8_lossy(&text).into_owned()
        })
    };
    let stdout = reader(Box::new(child.stdout.take().ok_or("no stdout")?));
    let stderr = reader(Box::new(child.stderr.take().ok_or("no stderr")?));
    let deadline = Instant::now() + Duration::from_secs(check.timeout_secs);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "the check ran past {} s and was stopped",
                check.timeout_secs
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let out = stdout.join().unwrap_or_default();
    let err = stderr.join().unwrap_or_default();
    if !status.success() {
        return Err(format!("the check failed ({status}):\n{out}{err}"));
    }
    let absent: Vec<&String> = check
        .expect
        .iter()
        .filter(|line| !out.contains(line.as_str()))
        .collect();
    if !absent.is_empty() {
        return Err(format!("the check did not print {absent:?}:\n{out}{err}"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_binaries_and_shared_libraries_are_executable() {
        let bins = ["demo.exe".to_string()];
        assert!(is_executable("demo.exe", &bins));
        assert!(is_executable("plugins/torque_lua.dll", &bins));
        assert!(is_executable("plugins/libtorque_lua.so", &bins));
        assert!(is_executable("plugins/libtorque_lua.dylib", &bins));
        assert!(!is_executable("plugins/torque_lua.toml", &bins));
        assert!(!is_executable("plugins/notes.txt", &bins));
        assert!(!is_executable("licenses/index.txt", &bins));
        assert!(!is_executable("game/plugins/x.dll", &bins));
    }

    #[test]
    fn bundle_folders_are_named_by_os_and_arch() {
        assert_eq!(
            folder_name("demo", "x86_64-pc-windows-msvc"),
            "demo-windows-x86_64"
        );
        assert_eq!(
            folder_name("demo", "aarch64-apple-darwin"),
            "demo-macos-aarch64"
        );
        assert_eq!(
            folder_name("demo", "x86_64-unknown-linux-gnu"),
            "demo-linux-x86_64"
        );
    }
}
