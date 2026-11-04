use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};

use torque_ts_vm::host::{FileHost, FileRead, FileStat, ResourceRules};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::*;
use crate::zips::MAX_ZIP_BYTES;

/// A game folder the test makes under the temp directory, removed on drop.
struct Folder(PathBuf);

impl Folder {
    fn new() -> Folder {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("torque-resource-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Folder(path)
    }

    fn put(&self, path: &str, bytes: &[u8]) {
        let file = self.0.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, contents) in entries {
        w.start_file(*name, options).unwrap();
        w.write_all(contents).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn policy(zip_folder: Option<&'static [u8]>) -> MountPolicy {
    MountPolicy {
        zip_folder,
        rules: ResourceRules {
            scanned: &["mods", "prefs"],
            ..ResourceRules::default()
        },
        ..MountPolicy::default()
    }
}

fn listed(files: &ResourceFiles) -> Vec<String> {
    files
        .files()
        .iter()
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect()
}

fn found(read: FileRead) -> Option<Vec<u8>> {
    match read {
        FileRead::Found(bytes) => Some(bytes),
        _ => None,
    }
}

#[test]
fn a_zip_reads_as_a_folder_and_its_entry_beats_a_loose_file() {
    let folder = Folder::new();
    folder.put(
        "mods/pack.zip",
        &zip_of(&[("a.cs", b"zipped"), ("sub/b.cs", b"bee")]),
    );
    folder.put("mods/pack/a.cs", b"loose");
    folder.put("mods/plain.cs", b"plain");
    let files = ResourceFiles::open(&folder.0, policy(None));
    assert_eq!(
        found(files.read(b"mods/pack/a.cs")),
        Some(b"zipped".to_vec())
    );
    assert_eq!(
        found(files.read(b"MODS/PACK/SUB/B.CS")),
        Some(b"bee".to_vec())
    );
    assert_eq!(
        files.stat(b"mods/pack/sub/b.cs"),
        FileStat::Found { len: 3 }
    );
    assert!(files.is_archive_entry(b"mods/pack/a.cs"));
    assert!(!files.is_archive_entry(b"mods/plain.cs"));
    assert_eq!(found(files.read(b"mods/plain.cs")), Some(b"plain".to_vec()));
    assert_eq!(files.read(b"mods/../mods/plain.cs"), FileRead::Missing);
    // The scan reversed; a zip's entries (reversed) right before the zip; the loose twin of an entry listed once.
    let list = listed(&files);
    let at = |name: &str| list.iter().position(|p| p == name).unwrap();
    assert_eq!(
        list.iter()
            .filter(|p| p.eq_ignore_ascii_case("mods/pack/a.cs"))
            .count(),
        1
    );
    assert!(at("mods/pack/sub/b.cs") < at("mods/pack/a.cs"));
    assert_eq!(at("mods/pack/a.cs") + 1, at("mods/pack.zip"));
}

#[test]
fn every_zip_mounts_unless_the_policy_names_one_folder() {
    let folder = Folder::new();
    folder.put("mods/one.zip", &zip_of(&[("x.cs", b"1")]));
    folder.put("prefs/two.zip", &zip_of(&[("y.cs", b"2")]));
    folder.put("mods/deep/three.zip", &zip_of(&[("z.cs", b"3")]));
    let all = ResourceFiles::open(&folder.0, policy(None));
    assert_eq!(all.mounted().len(), 3);
    assert!(found(all.read(b"prefs/two/y.cs")).is_some());
    let only = ResourceFiles::open(&folder.0, policy(Some(b"MODS")));
    assert_eq!(only.mounted(), vec![b"mods/one.zip".to_vec()]);
    assert!(found(only.read(b"mods/one/x.cs")).is_some());
    assert_eq!(only.read(b"prefs/two/y.cs"), FileRead::Missing);
    assert_eq!(only.read(b"mods/deep/three/z.cs"), FileRead::Missing);
}

#[test]
fn writes_stay_in_memory_go_to_the_head_and_deletes_take_only_written_files() {
    let folder = Folder::new();
    folder.put("prefs/old.cs", b"old");
    let files = ResourceFiles::open(&folder.0, policy(None));
    assert!(files.write(b"prefs/new.cs", b"one", false));
    assert!(files.write(b"prefs/new.cs", b"two", true));
    assert_eq!(found(files.read(b"prefs/new.cs")), Some(b"onetwo".to_vec()));
    assert_eq!(listed(&files)[0], "prefs/new.cs");
    // A write over a file the list has keeps its place; the folder is not written.
    assert!(files.write(b"prefs/old.cs", b"x", true));
    assert_eq!(found(files.read(b"prefs/old.cs")), Some(b"oldx".to_vec()));
    assert_eq!(
        std::fs::read(folder.0.join("prefs/old.cs")).unwrap(),
        b"old"
    );
    assert_eq!(
        listed(&files)
            .iter()
            .filter(|p| *p == "prefs/old.cs")
            .count(),
        1
    );
    assert!(files.remove(b"prefs/new.cs"));
    assert!(!listed(&files).contains(&"prefs/new.cs".to_string()));
    assert!(!files.remove(b"prefs/new.cs"));
    // The folder's own file is not deleted, and stays listed.
    assert!(files.remove(b"prefs/old.cs"));
    assert!(!files.remove(b"prefs/old.cs"));
    assert_eq!(found(files.read(b"prefs/old.cs")), Some(b"old".to_vec()));
    // A copy reads any resource and writes memory.
    assert!(files.copy(b"prefs/old.cs", b"prefs/copy.cs"));
    assert_eq!(files.written("prefs/copy.cs"), Some(b"old".to_vec()));
}

fn limited(folder: &Folder, limits: Limits) -> ResourceFiles {
    ResourceFiles::open_with_limits(&folder.0, policy(None), limits)
}

#[test]
fn a_zip_longer_than_the_cap_is_refused_before_it_is_read() {
    let folder = Folder::new();
    folder.put("mods/ok.zip", &zip_of(&[("a.cs", b"a")]));
    folder.put("mods/huge.zip", b"");
    let huge = std::fs::OpenOptions::new()
        .write(true)
        .open(folder.0.join("mods/huge.zip"))
        .unwrap();
    huge.set_len(MAX_ZIP_BYTES as u64 + 1).unwrap();
    drop(huge);
    let files = ResourceFiles::open(&folder.0, policy(None));
    assert_eq!(files.mounted(), vec![b"mods/ok.zip".to_vec()]);
    // A zip past the bytes left is refused the same way.
    let small = zip_of(&[("b.cs", b"b")]);
    let files = limited(
        &folder,
        Limits {
            mounted_bytes: small.len() - 1,
            ..Limits::default()
        },
    );
    assert!(files.mounted().is_empty());
}

#[test]
fn the_mount_budgets_hold_zips_bytes_with_the_directory_and_entries() {
    let folder = Folder::new();
    let one = zip_of(&[("x.cs", b"1")]);
    folder.put("mods/one.zip", &one);
    folder.put("mods/two.zip", &zip_of(&[("y.cs", b"2")]));
    folder.put(
        "prefs/three.zip",
        &zip_of(&[("a.cs", b"a"), ("b.cs", b"b"), ("c.cs", b"c")]),
    );
    let all = ResourceFiles::open(&folder.0, policy(None));
    assert_eq!(all.mounted().len(), 3);
    let zips = limited(
        &folder,
        Limits {
            mounted_zips: 1,
            ..Limits::default()
        },
    );
    assert_eq!(zips.mounted().len(), 1);
    // The archive's bytes fit, its directory's charge does not.
    let bytes = limited(
        &folder,
        Limits {
            mounted_bytes: one.len() + 1,
            ..Limits::default()
        },
    );
    assert!(bytes.mounted().is_empty());
    let entries = limited(
        &folder,
        Limits {
            mounted_entries: 2,
            ..Limits::default()
        },
    );
    assert_eq!(entries.mounted().len(), 2);
    assert!(!entries.mounted().contains(&b"prefs/three.zip".to_vec()));
    assert_eq!(entries.read(b"prefs/three/a.cs"), FileRead::Missing);
}

#[test]
fn a_write_past_the_written_bytes_fails_and_changes_nothing() {
    let folder = Folder::new();
    let files = limited(
        &folder,
        Limits {
            overlay_bytes: 10,
            ..Limits::default()
        },
    );
    assert!(!files.write(b"prefs/big.cs", &[b'x'; 11], false));
    assert_eq!(files.written("prefs/big.cs"), None);
    assert!(files.write(b"prefs/a.cs", b"12345", false));
    assert!(!files.write(b"prefs/a.cs", b"123456", true));
    assert!(!files.write(b"prefs/b.cs", b"123456", false));
    assert_eq!(files.written("prefs/a.cs"), Some(b"12345".to_vec()));
    // Replacing a file frees its old bytes; deleting it frees them all.
    assert!(files.write(b"prefs/a.cs", b"1234567890", false));
    assert!(!files.write(b"prefs/b.cs", b"1", false));
    assert!(files.remove(b"prefs/a.cs"));
    assert!(files.write(b"prefs/b.cs", b"1234567890", false));
}

#[test]
fn a_new_file_past_the_written_files_cap_fails() {
    let folder = Folder::new();
    let files = limited(
        &folder,
        Limits {
            overlay_files: 2,
            ..Limits::default()
        },
    );
    assert!(files.write(b"prefs/a.cs", b"", false));
    assert!(files.write(b"prefs/b.cs", b"", false));
    assert!(!files.write(b"prefs/c.cs", b"", false));
    assert!(files.write(b"PREFS/A.CS", b"again", true));
    assert!(files.remove(b"prefs/b.cs"));
    assert!(files.write(b"prefs/c.cs", b"", false));
    assert_eq!(listed(&files), vec!["prefs/c.cs", "prefs/a.cs"]);
}

#[test]
fn a_written_path_is_at_most_the_name_buffer() {
    let folder = Folder::new();
    let files = ResourceFiles::open(&folder.0, policy(None));
    let path = |len: usize| {
        let mut p = b"prefs/".to_vec();
        p.resize(len - 3, b'a');
        p.extend_from_slice(b".cs");
        p
    };
    assert!(files.write(&path(MAX_WRITTEN_PATH), b"1", false));
    assert!(!files.write(&path(MAX_WRITTEN_PATH + 1), b"1", false));
    assert_eq!(listed(&files).len(), 1);
}

#[test]
fn an_append_copy_is_counted_as_work_and_refused_when_it_cannot_fit() {
    let folder = Folder::new();
    folder.put("prefs/log.txt", &[b'l'; 100]);
    let tight = limited(
        &folder,
        Limits {
            overlay_bytes: 50,
            ..Limits::default()
        },
    );
    assert!(!tight.write(b"prefs/log.txt", b"more", true));
    assert_eq!(tight.work_done(), 0);
    assert_eq!(tight.written("prefs/log.txt"), None);
    let files = ResourceFiles::open(&folder.0, policy(None));
    assert!(files.write(b"prefs/log.txt", b"more", true));
    assert_eq!(files.work_done(), 100);
    assert!(files.write(b"prefs/log.txt", b"more", true));
    assert_eq!(files.work_done(), 100);
    assert_eq!(files.written("prefs/log.txt").map(|f| f.len()), Some(108));
}

#[test]
fn the_files_serve_a_hosts_parts_and_the_stored_folder() {
    let folder = Folder::new();
    folder.put("mods/Sub/a.cs", b"a");
    let files = ResourceFiles::open_with_parts(
        &folder.0,
        MountPolicy {
            zip_folder_spelled: "mods",
            ..policy(None)
        },
        Limits::default(),
        vec![("Given".to_string(), zip_of(&[("g.cs", b"given")]))],
        vec![("prefs/start.cs".to_string(), b"start".to_vec())],
    );
    assert_eq!(files.report().mounted, 1);
    assert_eq!(files.report().startup_files, 1);
    assert_eq!(
        found(files.read(b"mods/given/g.cs")),
        Some(b"given".to_vec())
    );
    assert!(files.is_archive_entry(b"mods/given/g.cs"));
    assert_eq!(
        found(files.read(b"prefs/start.cs")),
        Some(b"start".to_vec())
    );
    assert_eq!(
        files.real_path(b"MODS/SUB/A.CS"),
        Some(b"mods/Sub".to_vec())
    );
    assert_eq!(
        files.real_path(b"mods/given/g.cs"),
        Some(b"mods/Given".to_vec())
    );
    assert!(listed(&files).contains(&"mods/Given.zip".to_string()));
    // A written file has no stored folder; a name the host refuses to serve has none either.
    assert!(files.write(b"mods/Sub/a.cs", b"over", false));
    assert_eq!(files.real_path(b"mods/sub/a.cs"), None);
    assert_eq!(files.real_path(b"mods/sub/../sub/a.cs"), None);
}
