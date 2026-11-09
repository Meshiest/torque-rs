use crate::manifest::{Manifest, library_file_name};

#[test]
fn a_manifest_names_its_id_version_and_library() {
    let m = Manifest::parse("id = \"example.sample\"\nversion = \"1.2.0\"\n", "sample").unwrap();
    assert_eq!(
        (m.id.as_str(), m.version.as_str(), m.library.as_str()),
        ("example.sample", "1.2.0", "sample")
    );
    let m = Manifest::parse(
        "id = \"a\"\nversion = \"1\"\nlibrary = \"other_lib\"\n",
        "x",
    )
    .unwrap();
    assert_eq!(m.library, "other_lib");
    for bad in [
        "version = \"1\"",
        "id = \"a b\"\nversion = \"1\"",
        "id = \"a\"",
        "id = \"a\"\nversion = \"1 2\"",
        "id = \"a\"\nversion = \"1\"\nlibrary = \"../escape\"",
        "id = \"a\"\nversion = \"1\"\nlibrary = \"dir/lib\"",
        "id = \"a\"\nversion = \"1\"\nextra = 1",
        "id = 3\nversion = \"1\"",
        "not toml",
    ] {
        assert!(Manifest::parse(bad, "x").is_err(), "{bad:?} is refused");
    }
    assert!(library_file_name("sample").contains("sample"));
}

/// The `[lints]` tables, by line, below the headers that start with `prefix`.
fn lint_lines(manifest: &str, prefix: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut table = None;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            table = line
                .strip_prefix(prefix)
                .map(|t| t.trim_end_matches(']').to_string());
        } else if let Some(table) = &table
            && !line.is_empty()
            && !line.starts_with('#')
        {
            out.push((table.clone(), line.to_string()));
        }
    }
    out
}

#[test]
fn the_lints_are_the_workspaces() {
    let workspace = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"));
    let ours = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    let mut expected = lint_lines(workspace, "[workspace.lints.");
    assert!(expected.len() > 10, "the workspace's lints were not found");
    for (_, line) in &mut expected {
        if line == "unsafe_code = \"forbid\"" {
            *line = "unsafe_code = \"deny\"".into();
        }
    }
    assert_eq!(lint_lines(ours, "[lints."), expected);
}

/// Every `.rs` file under `dir`, in its folders too.
fn sources(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(sources(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
    files
}

/// `unsafe` is allowed in one module, `ffi`, and every `unsafe` block there has a SAFETY comment.
#[test]
fn unsafe_code_is_only_in_ffi() {
    let allow = concat!("allow(", "unsafe_code)");
    let files = sources(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src"
    )));
    assert!(
        files.iter().any(|f| f.ends_with("web_lua/page.rs")),
        "the folders are read"
    );
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let allows = text.matches(allow).count();
        assert_eq!(allows, usize::from(name == "ffi.rs"), "{name}");
        if name == "ffi.rs" {
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if line.contains(concat!("unsafe", " {")) && !line.trim_start().starts_with("//") {
                    let before = lines[..i]
                        .iter()
                        .rev()
                        .take(4)
                        .any(|l| l.contains("SAFETY:"));
                    assert!(
                        before,
                        "ffi.rs:{}: an unsafe block without a SAFETY comment",
                        i + 1
                    );
                }
            }
        }
    }
}

/// A 64-bit little-endian ELF shared object with one `PT_DYNAMIC` segment holding `tags` (each with a value of 0) and
/// a closing `DT_NULL`, and no section headers.
fn elf_with_dynamic(tags: &[u64]) -> Vec<u8> {
    const HEADER: usize = 64;
    const PHDR: usize = 56;
    let dynamic_at = HEADER + PHDR;
    let mut entries: Vec<u8> = Vec::new();
    for &tag in tags.iter().chain(&[0]) {
        entries.extend_from_slice(&tag.to_le_bytes());
        entries.extend_from_slice(&0u64.to_le_bytes());
    }
    let mut out = vec![0u8; HEADER];
    out[..4].copy_from_slice(b"\x7fELF");
    out[4] = 2; // 64-bit
    out[5] = 1; // little-endian
    out[6] = 1; // version
    out[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
    out[18..20].copy_from_slice(&62u16.to_le_bytes()); // x86-64
    out[20..24].copy_from_slice(&1u32.to_le_bytes());
    out[32..40].copy_from_slice(&(HEADER as u64).to_le_bytes()); // e_phoff
    out[52..54].copy_from_slice(&(HEADER as u16).to_le_bytes()); // e_ehsize
    out[54..56].copy_from_slice(&(PHDR as u16).to_le_bytes()); // e_phentsize
    out[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
    out[58..60].copy_from_slice(&64u16.to_le_bytes()); // e_shentsize
    let mut phdr = vec![0u8; PHDR];
    phdr[..4].copy_from_slice(&2u32.to_le_bytes()); // PT_DYNAMIC
    phdr[8..16].copy_from_slice(&(dynamic_at as u64).to_le_bytes()); // p_offset
    phdr[32..40].copy_from_slice(&(entries.len() as u64).to_le_bytes()); // p_filesz
    phdr[40..48].copy_from_slice(&(entries.len() as u64).to_le_bytes()); // p_memsz
    out.extend_from_slice(&phdr);
    out.extend_from_slice(&entries);
    out
}

/// A 64-bit Mach-O dynamic library whose one load command is `command` (an `LC_RPATH` names "x").
fn macho_with_command(command: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for word in [0xfeed_facf_u32, 0x0100_0007, 3, 6, 1, 16, 0, 0] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    for word in [command, 16, 12] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.extend_from_slice(b"x\0\0\0");
    out
}

#[test]
fn a_library_that_names_its_own_dependency_folders_is_refused() {
    use crate::stage::check_search_paths;
    const DT_NEEDED: u64 = 1;
    const DT_RPATH: u64 = 15;
    const DT_RUNPATH: u64 = 29;
    assert_eq!(check_search_paths(&elf_with_dynamic(&[DT_NEEDED])), Ok(()));
    for tag in [DT_RPATH, DT_RUNPATH] {
        let refused = check_search_paths(&elf_with_dynamic(&[DT_NEEDED, tag]));
        assert!(refused.is_err_and(|r| r.contains("rpath")), "{tag}");
    }
    // LC_RPATH, and LC_ID_DYLIB (0xd), which names no folder.
    assert!(check_search_paths(&macho_with_command(0x8000_001c)).is_err());
    assert_eq!(check_search_paths(&macho_with_command(0xd)), Ok(()));
}

/// The scratch folder `name` of these tests, below the build folder.
fn scratch(name: &str) -> std::path::PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let dir = target
        .join("native-ext-tests")
        .join(format!("unit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A private copy replaced after it was checked: its path no longer names the hashed file, and on Linux the library is
/// opened through the held handle, which still reads it.
#[cfg(unix)]
#[test]
fn a_copy_replaced_after_its_check_is_never_the_one_opened() {
    let cache = scratch("swap");
    let bytes = b"the approved bytes".to_vec();
    let read = crate::stage::Read1 {
        sha256: crate::stage::sha256_hex(&bytes),
        bytes,
    };
    let held = crate::stage::cache_copy(&read, &cache).unwrap();
    assert!(held.still_same());
    let other = cache.join("other");
    std::fs::write(&other, b"something else").unwrap();
    std::fs::rename(&other, &held.path).unwrap();
    assert!(
        !held.still_same(),
        "the swap is seen, so the init never runs"
    );
    if cfg!(target_os = "linux") {
        assert_eq!(std::fs::read(held.open_path()).unwrap(), read.bytes);
    }
}

#[cfg(unix)]
#[test]
fn a_user_folder_others_can_write_loads_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let base = scratch("shared");
    let plugins = base.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let user = base.join("user");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::set_permissions(&user, std::fs::Permissions::from_mode(0o777)).unwrap();
    let plugin = crate::NativeExtensionsPlugin::new(&plugins).user_folder(&user);
    let refused = plugin.checked_user().unwrap_err();
    assert!(refused.contains("written by other users"), "{refused}");
    std::fs::set_permissions(&user, std::fs::Permissions::from_mode(0o700)).unwrap();
    let allowlist = user.join(crate::ALLOWLIST_FILE);
    std::fs::write(&allowlist, b"").unwrap();
    std::fs::set_permissions(&allowlist, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(
        plugin.checked_user().is_err(),
        "an allowlist others can write"
    );
    std::fs::set_permissions(&allowlist, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(plugin.checked_user().is_ok());
}

#[test]
fn a_version_with_a_comment_mark_is_refused() {
    let refused = Manifest::parse("id = \"a\"\nversion = \"1.0#x\"\n", "x");
    assert!(refused.is_err_and(|r| r.contains("'#'")));
}

#[test]
fn the_allowlist_is_replaced_whole_and_never_over_a_file_it_cannot_read() {
    use crate::{Allowlist, ApprovalRequest};
    let dir = scratch("allowlist");
    let path = dir.join(crate::ALLOWLIST_FILE);
    let list = Allowlist::new(&path);
    let request = |id: &str, sha: char| ApprovalRequest {
        id: id.to_string(),
        version: "1".to_string(),
        library: dir.join("x"),
        sha256: sha.to_string().repeat(64),
        approved_before: None,
    };
    list.record(&request("a", 'a')).unwrap();
    list.record(&request("b", 'b')).unwrap();
    assert_eq!(list.approved().len(), 2);
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        [crate::ALLOWLIST_FILE],
        "no half-written file is left"
    );
    // A file that cannot be read (here: not UTF-8) is never written over, so the approvals in it are not lost.
    let unreadable = [&std::fs::read(&path).unwrap()[..], b"\xff\n"].concat();
    std::fs::write(&path, &unreadable).unwrap();
    assert!(list.record(&request("c", 'c')).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), unreadable);
}
