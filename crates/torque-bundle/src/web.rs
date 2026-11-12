//! The web target: a game's browser build as a folder a static host serves.
//!
//! `[web]` in `bundle.toml` ([`crate::spec::Web`]) names the engine's web scripts and the example they build. The tool runs the Lua
//! module's build (`lua-build.mjs`, when the page loads Lua) and `build.mjs` from the engine's checkout, with the build folders
//! remapped as for the binaries, then takes what `build.mjs` wrote and makes the bundle folder `<name>`: the page, its scripts,
//! the game's module, the Lua module, `files.json` and the pack it names, `licenses/` and `manifest.toml`. No path of the build
//! computer may remain in any of it.
//!
//! Every file the list names is opened before anything is written: a pack (a path ending in `.pack`, any case) is unpacked and each
//! of its files opened by the engine's loader for its kind, and any other listed file is opened the same way as a single file, all
//! under the caps of a game folder ([`crate::validate`]); a refusal fails the bundle (`--no-validate` skips the loaders). Last,
//! headless Chrome loads the folder from a local server, once for each mode of `[web.check]`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use torque_bytes::{from_hex, hex};
use torque_resource::zips::entry_name_ok;

use crate::cargo::{Build, Metadata, Toolchain};
use crate::licenses::{self, Text};
use crate::manifest::{self, Entry, Header};
use crate::precompile::printable;
use crate::spec::{Spec, Web, WebFlag};
use crate::validate::{self, GameInfo};
use crate::{
    Computer, Options, Output, absolute, archive, inside, outside_message, own_folder, publish,
    read_files, run_limited, scan_files, select, trusted_roots, write_archive,
};

/// The target the game's module is built for.
pub const MODULE_TARGET: &str = "wasm32-unknown-unknown";

/// The target the Lua module is built for.
pub const LUA_TARGET: &str = "wasm32-wasip1";

/// The start of a pack (`torque_app_server::files::PACK_MAGIC`; a test holds the two equal).
pub const PACK_MAGIC: &[u8; 4] = b"TQPK";

/// What the web target's checks counted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The files `files.json` lists, each equal in size and SHA-256 to the file of the bundle and to the manifest's entry.
    pub listed: usize,
    /// The files inside the packs the list names, each opened by its loader.
    pub pack_files: usize,
    /// The listed files that are not packs, each opened by its loader (or left as they are when no loader owns them).
    pub other_files: usize,
    /// The `check.mjs` modes that passed.
    pub checks: Vec<String>,
}

/// The list the page fetches its files by.
pub const LIST: &str = "files.json";

/// The caps on the list, the engine's (`torque_app_server::manifest`; `web::tests::the_list_is_the_engines` holds the two equal
/// on a set of lists, constants included). The tool links none of the engine, so it keeps its own parser.
pub const MAX_LIST_BYTES: usize = 512 * 1024;
pub const MAX_LISTED_FILES: usize = 1024;
pub const MAX_LISTED_TOTAL: u64 = 256 * 1024 * 1024;
pub const MAX_LISTED_PATH: usize = 200;

/// A file of `files.json`: its path (ASCII letters, digits, `.`, `_` and `-` between `/`s), size and SHA-256 (lowercase hex).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// The ending of a listed path that makes it a pack of files (the engine's rule, in any case).
fn is_pack(path: &str) -> bool {
    path.len() >= 5 && path.as_bytes()[path.len() - 5..].eq_ignore_ascii_case(b".pack")
}

/// Packages the web target (module docs).
pub fn bundle(options: &Options) -> Result<crate::Report, String> {
    if options.server {
        return Err("--web and --server cannot be combined".into());
    }
    if options.mode.precompile {
        return Err("--precompile does not apply to the web target: its files are packed by the example's own build".into());
    }
    let crate_dir = absolute(&options.crate_dir)?;
    let target_dir = absolute(&options.target_dir)?;
    let spec = Spec::load(&crate_dir)?;
    let bundle_name = spec.bundle.name.clone();
    let package_name = spec
        .web
        .as_ref()
        .and_then(|web| web.package.clone())
        .unwrap_or_else(|| spec.bundle.package.clone());
    let web = spec
        .web
        .ok_or("bundle.toml: has no [web] table, so there is no web target")?;
    let name = web
        .name
        .clone()
        .unwrap_or_else(|| format!("{bundle_name}-web"));
    let toolchain = Toolchain::find()?;
    let manifest_path = crate_dir.join("Cargo.toml");
    let members = Metadata::read(&manifest_path, MODULE_TARGET, false)?;
    let package = members.package(&package_name).ok_or_else(|| {
        format!("bundle.toml: the package {package_name} is not in this workspace")
    })?;
    let scripts = web_scripts(
        options.scripts.as_deref(),
        &crate_dir,
        &web,
        &members.workspace_root,
    )?;
    let lua_manifest = web_lua(
        options.lua.as_deref(),
        &crate_dir,
        &web,
        &members.workspace_root,
        &options.allow,
    )?;

    // The computer's folders: remapped in the builds and looked for in every file.
    let mut workspaces = vec![(
        "the workspace",
        members.workspace_root.clone(),
        "/workspace",
    )];
    if let Some(manifest) = &lua_manifest {
        let lua = Metadata::read(manifest, LUA_TARGET, false)?;
        workspaces.push(("an extension's workspace", lua.workspace_root, "/extension"));
    }
    if let Some(sdk) = crate::env_dir("WASI_SDK_PATH") {
        workspaces.push(("the WASI SDK", sdk, "/wasi-sdk"));
    }
    let Computer { remaps, scanner } = Computer::find(
        &workspaces,
        &toolchain,
        &target_dir,
        &crate_dir,
        options.max_inflated_bytes,
    )?;

    let game_build = Build {
        manifest: manifest_path.clone(),
        package: Some(package_name),
        bins: Vec::new(),
        features: None,
        target: MODULE_TARGET.to_string(),
        target_dir: target_dir.join("bundle-web"),
        rustflags: crate::cargo::rustflags(&remaps, MODULE_TARGET, &[]),
        cflags: crate::cargo::cflags(&remaps, LUA_TARGET),
        env: Vec::new(),
    };
    let build_dir = game_build.build_dir();
    run_scripts(&web, &scripts, &crate_dir, &game_build, &build_dir)?;

    // What `build.mjs` wrote, read once: what is hashed, scanned and written is what was opened.
    let output = build_dir.join("web").join(web.output_name());
    let (selected, skipped) = select::select(&output, &[], options.limits.walk())?;
    let mut files: BTreeMap<String, Text> = BTreeMap::new();
    for file in selected {
        if !entry_name_ok(file.path.as_bytes()) {
            return Err(printable(&format!(
                "{}: a name the engine does not serve",
                file.path
            )));
        }
        files.insert(file.path, Text::File(file.source));
    }
    let mut written = read_files(files, options.limits.file_bytes, options.limits.total_bytes)?;
    let list = parse_list(&written)?;
    let opened = open_listed(&list, &written, options)?;

    // The licenses of what the module and the Lua module link.
    let game_linked = game_build.linked()?;
    let game_meta = Metadata::read(&manifest_path, MODULE_TARGET, true)?;
    let mut builds_meta = vec![(game_meta, game_linked)];
    if let Some(manifest) = &lua_manifest {
        let build = Build {
            manifest: manifest.clone(),
            package: None,
            target: LUA_TARGET.to_string(),
            ..game_build.clone()
        };
        builds_meta.push((Metadata::read(manifest, LUA_TARGET, true)?, build.linked()?));
    }
    let builds: Vec<(&Metadata, &[(String, String)])> = builds_meta
        .iter()
        .map(|(meta, linked)| (meta, linked.as_slice()))
        .collect();
    let gathered = licenses::gather(&builds, &toolchain);
    let texts: BTreeMap<String, Text> = gathered.files.iter().cloned().collect();
    written.extend(read_files(
        texts,
        options.limits.file_bytes,
        options.limits.total_bytes,
    )?);
    written.push((
        format!("{}/index.txt", licenses::DIR),
        gathered.index.clone().into_bytes(),
    ));
    check_unique(&written)?;
    let entries: Vec<Entry> = written.iter().map(|(p, b)| Entry::of(p, b)).collect();
    let listed = list_matches(&list, &entries)?;
    let header = Header {
        name: name.clone(),
        version: package.version.clone(),
        target: MODULE_TARGET.to_string(),
        mode: "web".to_string(),
        features: Vec::new(),
        rustc: toolchain.version.clone(),
    };
    written.push((
        manifest::FILE.to_string(),
        manifest::write(&header, &entries).into_bytes(),
    ));
    let warnings = scan_files(&scanner, &written)?;

    let out = absolute(&options.out)?;
    let folder = publish(&out, &name, &written, &[])?;

    let mut report = Report {
        listed,
        pack_files: opened.pack_files,
        other_files: opened.other_files,
        checks: Vec::new(),
    };
    let check = match (&web.check, options.check) {
        (Some(check), true) => {
            let work = out.join(format!("{name}.work"));
            let output = run_checks(&web, check, &scripts, &folder, &work, &mut report)?;
            let problems = manifest::verify(&folder)?;
            if !problems.is_empty() {
                return Err(format!(
                    "the check changed the bundle:\n  {}",
                    problems.join("\n  ")
                ));
            }
            Some(output)
        }
        _ => None,
    };
    let archive = if options.archive {
        Some(write_archive(
            &out,
            &name,
            archive::Format::Zip,
            &written,
            &[],
        )?)
    } else {
        None
    };
    Ok(crate::Report {
        folder,
        files: written.len(),
        bytes: written.iter().map(|(_, b)| b.len() as u64).sum(),
        features: Vec::new(),
        skipped,
        crates: gathered.crates,
        missing_licenses: gathered.missing,
        generic_licenses: gathered.generic,
        check,
        server: None,
        archive,
        content: validate::Report::default(),
        warnings,
        web: Some(report),
    })
}

/// The node program: `NODE`, else `node` on the path.
fn node() -> Command {
    Command::new(std::env::var_os("NODE").unwrap_or_else(|| "node".into()))
}

/// The longest one of the engine's build scripts may run (a cold build of the module and the Lua module takes minutes).
const BUILD_TIMEOUT_SECS: u64 = 3 * 3600;

/// Runs the engine's scripts that build the web files, with the bundle's build folder and the paths remapped: the Lua module's
/// build first when the page loads Lua, then `build.mjs`. Their output is relayed a line at a time, made printable; they get no
/// input, and a script past [`BUILD_TIMEOUT_SECS`] is killed with everything it started.
fn run_scripts(
    web: &Web,
    scripts: &Path,
    crate_dir: &Path,
    build: &Build,
    build_dir: &Path,
) -> Result<(), String> {
    let mut steps: Vec<Vec<String>> = Vec::new();
    if web.flags.contains(&WebFlag::Lua) {
        steps.push(vec![script(scripts, "lua-build.mjs")?]);
    }
    let mut args = vec![script(scripts, "build.mjs")?, web.example.clone()];
    args.extend(web.flags.iter().map(|flag| flag.arg().to_string()));
    steps.push(args);
    for args in steps {
        let mut command = node();
        command.args(&args).current_dir(crate_dir);
        for name in [
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_BUILD_RUSTFLAGS",
        ] {
            command.env_remove(name);
        }
        command
            .env("CARGO_TARGET_DIR", build_dir)
            .env("CARGO_ENCODED_RUSTFLAGS", build.rustflags.join("\u{1f}"))
            .env("SOURCE_DATE_EPOCH", crate::archive::EPOCH.to_string());
        run_limited(
            command,
            &format!("node {}", printable(&args.join(" "))),
            BUILD_TIMEOUT_SECS,
            &[],
            Output::Relay,
        )?;
    }
    Ok(())
}

/// The folder of the engine's web scripts: `given` (`--scripts`, taken as it is), else the folder `[web].scripts` names, which
/// must be the `web` folder of the crate's workspace or a folder in it: a `bundle.toml` is a build input but it can come from
/// anywhere, and what it names here is run.
fn web_scripts(
    given: Option<&Path>,
    crate_dir: &Path,
    web: &Web,
    workspace_root: &Path,
) -> Result<PathBuf, String> {
    if let Some(given) = given {
        return absolute(given);
    }
    let named = crate_dir.join(&web.scripts);
    let allowed = workspace_root.join("web");
    if !inside(&named, std::slice::from_ref(&allowed)) {
        return Err(outside_message(
            "web.scripts",
            &web.scripts,
            &allowed,
            "--scripts <folder>",
        ));
    }
    Ok(named)
}

/// The Lua extension's manifest: `given` (`--lua`, taken as it is), else the one `[web].lua` names, which must lie in the crate's
/// workspace or a folder of `allow`.
fn web_lua(
    given: Option<&Path>,
    crate_dir: &Path,
    web: &Web,
    workspace_root: &Path,
    allow: &[PathBuf],
) -> Result<Option<PathBuf>, String> {
    if let Some(given) = given {
        return absolute(given).map(Some);
    }
    let Some(lua) = &web.lua else {
        return Ok(None);
    };
    let named = crate_dir.join(lua);
    if !inside(&named, &trusted_roots(workspace_root, allow)) {
        return Err(outside_message(
            "web.lua",
            lua,
            workspace_root,
            "--lua <Cargo.toml>",
        ));
    }
    Ok(Some(named))
}

/// The path of the script `name` in `scripts`, as text for a command line.
fn script(scripts: &Path, name: &str) -> Result<String, String> {
    let path = scripts.join(name);
    if !path.is_file() {
        return Err(format!("{}: no such script", path.display()));
    }
    Ok(path.to_string_lossy().into_owned())
}

/// `files.json` of `written`, read as the engine's page reads it ([`MAX_LIST_BYTES`] and the rest of the caps above, a hostile
/// list being an error): one JSON object whose `files` array names each file by `path`, `size` and `sha256`; other keys are
/// ignored.
pub fn parse_list(written: &[(String, Vec<u8>)]) -> Result<Vec<Listed>, String> {
    let (_, bytes) = written
        .iter()
        .find(|(path, _)| path == LIST)
        .ok_or_else(|| format!("the web build has no {LIST}"))?;
    parse_list_text(bytes)
}

/// The list in `bytes` (see [`parse_list`]).
pub fn parse_list_text(bytes: &[u8]) -> Result<Vec<Listed>, String> {
    if bytes.len() > MAX_LIST_BYTES {
        return Err(format!("the list of files is over {MAX_LIST_BYTES} bytes"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "the list of files is not UTF-8")?;
    let value: Value = serde_json::from_str(text)
        .map_err(|e| printable(&format!("the list of files is not JSON: {e}")))?;
    let files = value
        .get("files")
        .and_then(Value::as_array)
        .ok_or("the list of files has no \"files\" array")?;
    if files.len() > MAX_LISTED_FILES {
        return Err(format!("the list names over {MAX_LISTED_FILES} files"));
    }
    let list: Vec<Listed> = files.iter().map(listed).collect::<Result<_, _>>()?;
    let mut seen = HashSet::new();
    let mut total = 0u64;
    for file in &list {
        if !seen.insert(file.path.to_ascii_lowercase()) {
            return Err(printable(&format!("the list names {} twice", file.path)));
        }
        total = total.saturating_add(file.size);
        if total > MAX_LISTED_TOTAL {
            return Err(format!(
                "the files of the list are over {MAX_LISTED_TOTAL} bytes"
            ));
        }
    }
    Ok(list)
}

/// One file of the list's `files` array.
fn listed(value: &Value) -> Result<Listed, String> {
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .ok_or("a file of the list has no \"path\" string")?;
    check_path(path)?;
    let size = value
        .get("size")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{path}: no \"size\" (a whole number of bytes)"))?;
    if size > MAX_LISTED_TOTAL {
        return Err(format!("{path}: over {MAX_LISTED_TOTAL} bytes"));
    }
    let hash = value
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{path}: no \"sha256\" string"))?;
    let sha256 = from_hex::<32>(hash)
        .ok_or_else(|| format!("{path}: its \"sha256\" is not 64 lowercase hex digits"))?;
    Ok(Listed {
        path: path.to_string(),
        size,
        sha256: hex(&sha256),
    })
}

/// A path the list may name: ASCII letters, digits, `.`, `_` and `-`, with `/` between parts that are not empty, `.` or `..`.
fn check_path(path: &str) -> Result<(), String> {
    let shown: String = path.chars().take(MAX_LISTED_PATH).collect();
    if path.is_empty() || path.len() > MAX_LISTED_PATH {
        return Err(printable(&format!(
            "the list names a path of {} bytes: {shown:?}",
            path.len()
        )));
    }
    let fine = path.split('/').all(|part| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    });
    if fine {
        Ok(())
    } else {
        Err(printable(&format!(
            "the list names the path {shown:?}, which a page may not fetch"
        )))
    }
}

/// What opening the listed files counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Opened {
    pack_files: usize,
    other_files: usize,
}

/// Opens every file the list names, so nothing the page will fetch ships unread. A path that ends in `.pack` (any case, as the
/// engine reads it) must unpack, its files each then opened by their loader ([`crate::validate`]); any other listed file is opened by
/// the loader for its kind, as a game folder's file is. A listed file the folder lacks is an error. With validation off the packs
/// are still unpacked (a pack that does not is an error) but no loader runs. A refusal fails the bundle with the path and
/// the reason.
fn open_listed(
    list: &[Listed],
    written: &[(String, Vec<u8>)],
    options: &Options,
) -> Result<Opened, String> {
    let by_path: HashMap<&str, &[u8]> = written
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    let mode = validate::Mode {
        validate: true,
        precompile: false,
        ..options.mode
    };
    let cap = options.limits.file_bytes;
    let mut opened = Opened::default();
    for file in list {
        let bytes = *by_path
            .get(file.path.as_str())
            .ok_or_else(|| printable(&format!("{}: listed, but not in the bundle", file.path)))?;
        let (names, contents, game): (Vec<&str>, Vec<&[u8]>, GameInfo) = if is_pack(&file.path) {
            let pack = unpack(bytes, &options.limits)
                .map_err(|e| printable(&format!("{}: {e}", file.path)))?;
            opened.pack_files += pack.len();
            let game = GameInfo::from_toml(
                pack.iter()
                    .find(|(path, _)| path.eq_ignore_ascii_case("game.toml"))
                    .map(|(_, bytes)| *bytes),
            );
            (
                pack.iter().map(|(path, _)| *path).collect(),
                pack.iter().map(|(_, bytes)| *bytes).collect(),
                game,
            )
        } else {
            opened.other_files += 1;
            (vec![file.path.as_str()], vec![bytes], GameInfo::default())
        };
        if !options.mode.validate {
            continue;
        }
        let (_, report) = validate::process_with(
            &names,
            |index| {
                let bytes = contents[index];
                if bytes.len() as u64 > cap {
                    return Err(format!("{} bytes is over the limit of {cap}", bytes.len()));
                }
                Ok(bytes.to_vec())
            },
            &options.limits,
            mode,
            &game,
        );
        if !report.refused.is_empty() {
            let lines: Vec<String> = report.refused.iter().map(ToString::to_string).collect();
            return Err(format!(
                "{} files of {} were refused by their loaders (nothing was written; --no-validate skips the check):\n  {}",
                lines.len(),
                printable(&file.path),
                lines.join("\n  ")
            ));
        }
    }
    Ok(opened)
}

/// The files of a pack (`torque_app_server::files`: `TQPK`, the count, then each file's path and bytes with little-endian `u32`
/// lengths). Err when it is cut short, holds more files than `limits` allow, a path the engine does not serve or bytes past
/// its last file.
pub fn unpack<'a>(
    bytes: &'a [u8],
    limits: &validate::Limits,
) -> Result<Vec<(&'a str, &'a [u8])>, String> {
    fn take<'a>(rest: &mut &'a [u8], n: usize) -> Result<&'a [u8], String> {
        if n > rest.len() {
            return Err("a pack of files cut short".into());
        }
        let (got, after) = rest.split_at(n);
        *rest = after;
        Ok(got)
    }
    fn take_len(rest: &mut &[u8]) -> Result<usize, String> {
        let b = take(rest, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }
    let mut rest = bytes
        .strip_prefix(PACK_MAGIC.as_slice())
        .ok_or("not a pack of files")?;
    let count = take_len(&mut rest)?;
    if count > limits.files {
        return Err(format!(
            "a pack of {count} files; the limit is {}",
            limits.files
        ));
    }
    let mut files = Vec::with_capacity(count.min(rest.len() / 8));
    for _ in 0..count {
        let n = take_len(&mut rest)?;
        let path = take(&mut rest, n)?;
        if !entry_name_ok(path) {
            return Err("a pack holds a name the engine does not serve".into());
        }
        let path = std::str::from_utf8(path).map_err(|_| "a pack's path is not UTF-8")?;
        let n = take_len(&mut rest)?;
        files.push((path, take(&mut rest, n)?));
    }
    if !rest.is_empty() {
        return Err(format!("{} bytes after the pack's last file", rest.len()));
    }
    Ok(files)
}

/// Err when two files of the bundle are one path, or two differ only in case, or one is the manifest's.
fn check_unique(written: &[(String, Vec<u8>)]) -> Result<(), String> {
    // `manifest.toml` is the tool's own, written last.
    let mut seen = HashSet::from([manifest::FILE.to_ascii_lowercase()]);
    for (path, _) in written {
        if !seen.insert(path.to_ascii_lowercase()) {
            return Err(printable(&format!(
                "two files would be {path} in the bundle"
            )));
        }
    }
    Ok(())
}

/// Checks that each file `files.json` lists is in the bundle with the size and the SHA-256 the list gives and that the
/// manifest's `entries` hold the same hash. The count of files listed.
fn list_matches(list: &[Listed], entries: &[Entry]) -> Result<usize, String> {
    let by_path: HashMap<&str, &Entry> = entries.iter().map(|e| (e.path.as_str(), e)).collect();
    let mut problems = Vec::new();
    for file in list {
        match by_path.get(file.path.as_str()) {
            None => problems.push(format!("{}: listed, but not in the bundle", file.path)),
            Some(entry) if entry.size != file.size => problems.push(format!(
                "{}: the list says {} bytes, the file has {}",
                file.path, file.size, entry.size
            )),
            Some(entry) if entry.sha256 != file.sha256 => {
                problems.push(format!(
                    "{}: the list's SHA-256 is not the file's",
                    file.path
                ));
            }
            Some(_) => {}
        }
    }
    if problems.is_empty() {
        return Ok(list.len());
    }
    Err(printable(&format!(
        "{LIST} does not match the files:\n  {}",
        problems.join("\n  ")
    )))
}

/// Runs `check.mjs` against the bundle folder for each mode of `check`, headless and served from a local port by the script
/// itself; the screenshots go in `work`. The tools (`TORQUE_WEB_TOOLS`: the folder with puppeteer-core) are the caller's.
fn run_checks(
    web: &Web,
    check: &crate::spec::WebCheck,
    scripts: &Path,
    folder: &Path,
    work: &Path,
    report: &mut Report,
) -> Result<String, String> {
    if std::env::var_os("TORQUE_WEB_TOOLS").is_none_or(|v| v.is_empty()) {
        return Err("TORQUE_WEB_TOOLS names no folder: install puppeteer-core in one (`npm install --prefix <folder> puppeteer-core`) and set it, or run with --no-check".into());
    }
    let checker = script(scripts, "check.mjs")?;
    own_folder(work)?;
    let mut said = String::new();
    for mode in &check.modes {
        let shot = work.join(format!("{}-{}.png", web.example, mode.arg()));
        let mut command = node();
        command
            .arg(&checker)
            .arg(folder)
            .arg(&shot)
            .arg(check.seconds.to_string())
            .arg(mode.arg());
        for (name, _) in std::env::vars_os() {
            let text = name.to_string_lossy();
            // As the native check: no build variables; the web check keeps the tools' folder (and the sandbox switch the
            // guard reads).
            let kept = matches!(
                text.as_ref(),
                "TORQUE_WEB_TOOLS" | "TORQUE_CHROME_NO_SANDBOX"
            );
            if text.starts_with("CARGO")
                || text.starts_with("RUST")
                || (text.starts_with("TORQUE_") && !kept)
            {
                command.env_remove(&name);
            }
        }
        let output = run_limited(
            command,
            &format!("the web check ({})", mode.arg()),
            check.timeout_secs,
            &["ok: no console errors".to_string()],
            Output::Keep,
        )?;
        said.push_str(&format!("[{}]\n{output}", mode.arg()));
        report.checks.push(mode.arg().to_string());
    }
    Ok(said)
}

#[cfg(test)]
#[path = "web_tests.rs"]
mod tests;
