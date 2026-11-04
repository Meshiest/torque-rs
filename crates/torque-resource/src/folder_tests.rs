use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};

use torque_ts_vm::host::{FileRead, FileStat, ResourceRules};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::*;

/// A game folder the test makes under the temp directory, removed on drop.
struct Folder(PathBuf);

impl Folder {
    fn new() -> Folder {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("torque-resource-folder-{}-{n}", std::process::id()));
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
        zip_folder_spelled: "Mods",
        rules: ResourceRules {
            scanned: &["mods", "prefs"],
            ..ResourceRules::default()
        },
        ..MountPolicy::default()
    }
}

fn open(folder: &Folder, policy: MountPolicy) -> GameFolder {
    GameFolder::open(&folder.0, policy, Limits::default(), Vec::new(), Vec::new())
}

fn listed(folder: &GameFolder) -> Vec<String> {
    folder
        .list()
        .iter()
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect()
}

fn found(read: Option<FileRead>) -> Option<Vec<u8>> {
    match read {
        Some(FileRead::Found(bytes)) => Some(bytes),
        _ => None,
    }
}

fn refusals(folder: &GameFolder) -> Vec<(String, String)> {
    folder.report().refused.clone()
}

#[test]
fn the_index_answers_for_the_scan_and_the_shipping_rule_misses_what_came_later() {
    let disk = Folder::new();
    disk.put("mods/Sub/Old.cs", b"old");
    disk.put("other/x.cs", b"x");
    let tge = open(&disk, policy(None));
    let shipping = open(
        &disk,
        MountPolicy {
            dictionary_only: true,
            ..policy(None)
        },
    );
    disk.put("mods/Sub/new.cs", b"new");
    disk.put("other/y.cs", b"y");
    for folder in [&tge, &shipping] {
        assert!(folder.has_file(b"MODS/sub/old.cs"));
        assert!(folder.has_folder(b"mods/SUB"));
        assert_eq!(folder.disk_read(b"mods/sub/OLD.cs"), Some(b"old".to_vec()));
        assert_eq!(folder.disk_stat(b"mods/sub/old.cs"), Some(3));
        // Outside the mod paths the disk is asked every time.
        assert_eq!(folder.disk_read(b"other/y.cs"), Some(b"y".to_vec()));
        assert!(!folder.has_file(b"other/y.cs"));
    }
    // A file made below a mod path after the scan: TGE's other builds look for it on the disk, the shipping build does not.
    assert_eq!(tge.disk_read(b"mods/sub/new.cs"), Some(b"new".to_vec()));
    assert!(tge.disk_modified(b"mods/sub/new.cs").is_some());
    assert_eq!(shipping.disk_read(b"mods/sub/new.cs"), None);
    assert_eq!(shipping.disk_stat(b"mods/sub/new.cs"), None);
    assert_eq!(shipping.disk_modified(b"mods/sub/new.cs"), None);
    assert!(!shipping.listed(b"mods/sub/new.cs"));
}

#[test]
fn a_resource_keeps_the_folder_spelled_as_stored() {
    let disk = Folder::new();
    disk.put("mods/Sub/a.cs", b"a");
    disk.put(
        "mods/Pack.zip",
        &zip_of(&[("Inner/b.cs", b"b"), ("top.cs", b"t")]),
    );
    let folder = open(&disk, policy(None));
    assert_eq!(
        folder.stored_dir(b"MODS/SUB/A.CS"),
        Some(b"mods/Sub".to_vec())
    );
    assert_eq!(
        folder.stored_dir(b"mods/pack/inner/B.cs"),
        Some(b"mods/Pack/Inner".to_vec())
    );
    assert_eq!(
        folder.stored_dir(b"mods/pack/top.cs"),
        Some(b"mods/Pack".to_vec())
    );
    assert_eq!(folder.stored_dir(b"mods/none.cs"), None);
    assert_eq!(
        folder.stored_dir(b"mods/sub/./a.cs"),
        Some(b"mods/Sub".to_vec())
    );
}

#[test]
fn zips_mount_in_the_scans_order_so_a_cap_leaves_out_the_last() {
    let disk = Folder::new();
    disk.put("mods/a.zip", &zip_of(&[("x.cs", b"1")]));
    disk.put("mods/b.zip", &zip_of(&[("y.cs", b"2")]));
    let folder = GameFolder::open(
        &disk.0,
        policy(None),
        Limits {
            mounted_zips: 1,
            ..Limits::default()
        },
        Vec::new(),
        Vec::new(),
    );
    assert_eq!(folder.mounted(), vec![b"mods/a.zip".to_vec()]);
    assert_eq!(
        refusals(&folder),
        vec![("b.zip".to_string(), "too many zips mounted (1)".to_string())]
    );
    assert_eq!(folder.report().mounted, 1);
    assert_eq!(folder.report().entries, 1);
}

#[test]
fn a_hosts_archive_mounts_where_its_file_would_be() {
    let disk = Folder::new();
    disk.put("mods/a.cs", b"a");
    disk.put("mods/z.cs", b"z");
    let zip = zip_of(&[("one.cs", b"1"), ("two.cs", b"2")]);
    let folder = GameFolder::open(
        &disk.0,
        policy(Some(b"mods")),
        Limits::default(),
        vec![("Middle".to_string(), zip.clone())],
        Vec::new(),
    );
    assert_eq!(folder.report().mounted, 1);
    let list = listed(&folder);
    let at = |name: &str| list.iter().position(|p| p == name).unwrap();
    // The list is the scan reversed: names descending, so the archive comes between `z.cs` and `a.cs`, its entries first.
    assert!(at("mods/z.cs") < at("mods/Middle/two.cs"));
    assert!(at("mods/Middle/two.cs") < at("mods/Middle/one.cs"));
    assert!(at("mods/Middle/one.cs") < at("mods/Middle.zip"));
    assert!(at("mods/Middle.zip") < at("mods/a.cs"));
    assert_eq!(
        folder.archive(b"MODS/middle.ZIP").map(|b| b.len()),
        Some(zip.len())
    );
    assert_eq!(
        folder.stat(b"mods/middle.zip"),
        FileStat::Found {
            len: zip.len() as u64
        }
    );
    assert_eq!(
        folder.read(b"mods/middle/two.cs"),
        FileRead::Found(b"2".to_vec())
    );
    assert_eq!(folder.modified(b"mods/middle/two.cs"), None);
    assert_eq!(folder.modified(b"mods/middle.zip"), None);
    assert!(folder.listed(b"mods/middle.zip") && folder.listed(b"mods/middle/one.cs"));
    // Charged twice (the bytes a read of its file returns are kept).
    assert!(folder.report().bytes >= 2 * zip.len());
}

#[test]
fn hostile_archives_from_a_host_are_refused_with_a_reason() {
    let disk = Folder::new();
    disk.put("mods/taken.zip", &zip_of(&[("x.cs", b"x")]));
    let good = zip_of(&[("a.cs", b"a")]);
    let archives = vec![
        ("taken".to_string(), good.clone()),
        ("con".to_string(), good.clone()),
        ("a/b".to_string(), good.clone()),
        ("dot.".to_string(), good.clone()),
        ("..".to_string(), good.clone()),
        (String::new(), good.clone()),
        ("junk".to_string(), b"PK\x05\x06 not a zip at all".to_vec()),
        ("twin".to_string(), good.clone()),
        ("TWIN".to_string(), good.clone()),
    ];
    let folder = GameFolder::open(
        &disk.0,
        policy(Some(b"mods")),
        Limits::default(),
        archives,
        Vec::new(),
    );
    let why: HashMap<String, String> = refusals(&folder).into_iter().collect();
    assert_eq!(why["taken.zip"], "a zip of that name is already mounted");
    for name in ["con.zip", "a/b.zip", "dot..zip", "...zip", ".zip"] {
        assert_eq!(why[name], "not a name a host may serve", "{name}");
    }
    assert!(why.contains_key("junk.zip"));
    assert_eq!(why["TWIN.zip"], "a zip of that name is already mounted");
    assert_eq!(folder.mounted().len(), 2);
    // A stem the folder has a file of, mounted or not.
    disk.put("mods/plain.zip", b"not a zip");
    let folder = GameFolder::open(
        &disk.0,
        policy(Some(b"mods")),
        Limits::default(),
        vec![("plain".to_string(), good.clone())],
        Vec::new(),
    );
    let why: HashMap<String, String> = refusals(&folder).into_iter().collect();
    assert_eq!(why["plain.zip"], "the folder has a file of that name");
    // No folder for them: no zip folder and no spelling.
    let folder = GameFolder::open(
        &disk.0,
        MountPolicy {
            zip_folder_spelled: "",
            ..policy(None)
        },
        Limits::default(),
        vec![("lost".to_string(), good.clone())],
        Vec::new(),
    );
    assert_eq!(
        refusals(&folder).last(),
        Some(&(
            "lost.zip".to_string(),
            "no folder for a host's archives".to_string()
        ))
    );
}

#[test]
fn a_hosts_archive_over_the_budget_is_refused_before_it_is_mounted() {
    let disk = Folder::new();
    let zip = zip_of(&[("a.cs", b"a")]);
    // Charged twice: room for one copy is not enough.
    let folder = GameFolder::open(
        &disk.0,
        policy(Some(b"mods")),
        Limits {
            mounted_bytes: zip.len() * 2 - 1,
            ..Limits::default()
        },
        vec![("big".to_string(), zip.clone())],
        Vec::new(),
    );
    assert!(folder.mounted().is_empty());
    assert!(
        refusals(&folder)[0]
            .1
            .starts_with("over the mount budget before reading")
    );
    let huge = vec![0u8; MAX_ZIP_BYTES + 1];
    let folder = GameFolder::open(
        &disk.0,
        policy(Some(b"mods")),
        Limits::default(),
        vec![("huge".to_string(), huge)],
        Vec::new(),
    );
    assert!(refusals(&folder)[0].1.starts_with("too large"));
}

#[test]
fn start_up_files_are_presented_where_the_scan_would_find_them() {
    let disk = Folder::new();
    disk.put("mods/b.cs", b"b");
    disk.put("prefs/p.cs", b"p");
    let folder = GameFolder::open(
        &disk.0,
        policy(None),
        Limits::default(),
        Vec::new(),
        vec![
            ("mods/c.cs".to_string(), b"c".to_vec()),
            ("mods/a.cs".to_string(), b"a".to_vec()),
            ("prefs/new/q.cs".to_string(), b"q".to_vec()),
        ],
    );
    assert_eq!(folder.report().startup_files, 3);
    let list = listed(&folder);
    let at = |name: &str| list.iter().position(|p| p == name).unwrap();
    // The scan goes through `mods` then `prefs`, names ascending; the list is it reversed.
    assert!(at("prefs/p.cs") < at("prefs/new/q.cs"));
    assert!(at("prefs/p.cs") < at("mods/c.cs"));
    assert!(at("mods/c.cs") < at("mods/b.cs"));
    assert!(at("mods/b.cs") < at("mods/a.cs"));
    assert_eq!(folder.disk_read(b"MODS/A.cs"), Some(b"a".to_vec()));
    assert_eq!(folder.disk_stat(b"mods/c.cs"), Some(1));
    assert_eq!(folder.disk_modified(b"mods/a.cs"), Some(0));
    assert!(folder.has_folder(b"prefs/new"));
    assert_eq!(
        folder.stored_dir(b"prefs/NEW/q.cs"),
        Some(b"prefs/new".to_vec())
    );
    // The folder itself is never written.
    assert!(!disk.0.join("mods/a.cs").exists());
}

#[test]
fn hostile_start_up_files_are_left_out_and_the_report_stays_bounded() {
    let disk = Folder::new();
    disk.put("mods/b.cs", b"b");
    let long = format!("mods/{}", "x".repeat(MAX_WRITTEN_PATH));
    let bad = [
        ("a.cs", "not a plain path below a folder"),
        ("mods/../x.cs", "not a plain path below a folder"),
        ("mods/a\\b.cs", "not a plain path below a folder"),
        ("mods/nul.cs", "not a plain path below a folder"),
        ("mods/x:y.cs", "not a plain path below a folder"),
        (long.as_str(), "not a plain path below a folder"),
        (
            "other/x.cs",
            "not below a folder the start-up scan goes through",
        ),
        ("mods/B.CS", "the folder has a file of that name"),
        ("mods/b.cs/under.cs", "below a file"),
        ("mods", "not a plain path below a folder"),
    ];
    let mut startup: Vec<(String, Vec<u8>)> = bad
        .iter()
        .map(|(p, _)| (p.to_string(), b"x".to_vec()))
        .collect();
    startup.push(("mods/new/file.cs".to_string(), b"ok".to_vec()));
    startup.push(("mods/NEW/FILE.cs".to_string(), b"twin".to_vec()));
    startup.push(("mods/new".to_string(), b"folder".to_vec()));
    startup.push(("mods/big.cs".to_string(), vec![0; MAX_STARTUP_BYTES]));
    let folder = GameFolder::open(
        &disk.0,
        policy(None),
        Limits::default(),
        Vec::new(),
        startup,
    );
    let why = refusals(&folder);
    for ((path, reason), (got_path, got)) in bad.iter().zip(&why) {
        assert_eq!(got, reason, "{path}");
        assert_eq!(*got_path, path.chars().take(80).collect::<String>());
    }
    let rest: Vec<&str> = why[bad.len()..].iter().map(|(_, w)| w.as_str()).collect();
    assert_eq!(
        rest,
        [
            "the folder has a file of that name",
            "a folder has that name",
            "over the start-up files' bytes"
        ]
    );
    assert_eq!(folder.report().startup_files, 1);
    assert_eq!(folder.disk_read(b"mods/new/file.cs"), Some(b"ok".to_vec()));
    // Too many: the cap holds, and the report keeps at most its own cap of refusals.
    let many: Vec<(String, Vec<u8>)> = (0..MAX_STARTUP_FILES + MAX_REPORTED_REFUSALS + 10)
        .map(|i| (format!("mods/m{i}.cs"), Vec::new()))
        .collect();
    let folder = GameFolder::open(&disk.0, policy(None), Limits::default(), Vec::new(), many);
    assert_eq!(folder.report().startup_files, MAX_STARTUP_FILES);
    assert_eq!(folder.report().refused.len(), MAX_REPORTED_REFUSALS);
    assert_eq!(folder.report().refused_more, 10);
    assert_eq!(folder.report().refused[0].1, "too many start-up files");
}

#[test]
fn a_failed_inflation_is_counted_and_leaves_the_loose_file() {
    let disk = Folder::new();
    let mut zip = {
        let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        w.start_file("bad.cs", stored).unwrap();
        w.write_all(b"hello world").unwrap();
        w.finish().unwrap().into_inner()
    };
    let at = zip.windows(11).position(|w| w == b"hello world").unwrap();
    zip[at] = b'H';
    disk.put("mods/pack.zip", &zip);
    disk.put("mods/pack/bad.cs", b"loose");
    let folder = open(&disk, policy(None));
    assert_eq!(
        folder.stat(b"mods/pack/bad.cs"),
        FileStat::Found { len: 11 }
    );
    assert_eq!(found(folder.read_entry(b"mods/pack/bad.cs")), None);
    assert!(folder.wasted() > 0);
    assert!(folder.inflated() >= folder.wasted());
    // This crate's rule: the loose file at the entry's path is read instead.
    assert_eq!(
        folder.read(b"mods/pack/bad.cs"),
        FileRead::Found(b"loose".to_vec())
    );
    // The loose file is the entry's twin: listed once.
    assert_eq!(
        listed(&folder)
            .iter()
            .filter(|p| p.eq_ignore_ascii_case("mods/pack/bad.cs"))
            .count(),
        1
    );
}

#[test]
fn every_zip_mounts_without_a_zip_folder_and_only_the_folders_with_one() {
    let disk = Folder::new();
    disk.put("mods/deep/a.zip", &zip_of(&[("x.cs", b"x")]));
    disk.put("prefs/b.zip", &zip_of(&[("y.cs", b"y")]));
    let all = open(&disk, policy(None));
    assert_eq!(
        found(all.read_entry(b"mods/deep/a/x.cs")),
        Some(b"x".to_vec())
    );
    assert_eq!(all.mounted().len(), 2);
    let only = open(&disk, policy(Some(b"PREFS")));
    assert_eq!(only.mounted(), vec![b"prefs/b.zip".to_vec()]);
    assert!(only.entry(b"mods/deep/a/x.cs").is_none());
}

/// Makes `link` a link to the folder `target` (a junction on Windows, which needs no privilege; a symbolic link elsewhere).
/// False when the platform does not let the test make one.
fn link_folder(target: &Path, link: &Path) -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link.to_string_lossy().replace('/', "\\"))
            .arg(target.to_string_lossy().replace('/', "\\"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
            && link.exists()
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (target, link);
        false
    }
}

/// Takes the link away (the link itself, never what it names).
fn unlink_folder(link: &Path) {
    #[cfg(windows)]
    let _ = std::fs::remove_dir(link);
    #[cfg(not(windows))]
    let _ = std::fs::remove_file(link);
}

#[test]
fn a_link_in_the_folder_is_not_read_listed_or_mounted_through_the_index() {
    let disk = Folder::new();
    let outside = Folder::new();
    outside.put("secret.cs", b"secret");
    outside.put("pack.zip", &zip_of(&[("x.cs", b"x")]));
    disk.put("mods/sub/secret.cs", b"inside");
    // A link there when the folder is scanned.
    let at_scan = disk.0.join("mods/linked");
    if !link_folder(&outside.0, &at_scan) {
        eprintln!("skipped: no link could be made here");
        return;
    }
    let folder = open(&disk, policy(None));
    assert!(!folder.has_file(b"mods/linked/secret.cs"));
    assert!(!listed(&folder).iter().any(|p| p.starts_with("mods/linked")));
    assert!(folder.mounted().is_empty());
    assert_eq!(folder.disk_read(b"mods/linked/secret.cs"), None);
    assert_eq!(folder.disk_stat(b"mods/linked/secret.cs"), None);
    // A folder the scan indexed that has become a link since: its files are no longer read through the index.
    assert_eq!(
        folder.disk_read(b"mods/sub/secret.cs"),
        Some(b"inside".to_vec())
    );
    let sub = disk.0.join("mods/sub");
    std::fs::remove_dir_all(&sub).unwrap();
    let swapped = link_folder(&outside.0, &sub);
    assert!(swapped);
    assert!(folder.has_file(b"mods/sub/secret.cs"));
    assert_eq!(folder.disk_read(b"mods/sub/secret.cs"), None);
    assert_eq!(folder.disk_stat(b"mods/sub/secret.cs"), None);
    assert_eq!(folder.disk_modified(b"mods/sub/secret.cs"), None);
    assert_eq!(folder.read(b"mods/sub/secret.cs"), FileRead::Missing);
    unlink_folder(&sub);
    unlink_folder(&at_scan);
    assert!(outside.0.join("secret.cs").exists());
}

#[test]
fn a_folder_swapped_for_a_link_around_the_open_is_no_file() {
    let disk = Folder::new();
    let outside = Folder::new();
    disk.put("mods/sub/a.cs", b"inside");
    outside.put("a.cs", b"outside");
    let trial = disk.0.join("mods/trial");
    if !link_folder(&outside.0, &trial) {
        eprintln!("skipped: no link could be made here");
        return;
    }
    unlink_folder(&trial);
    let rel: &[u8] = b"mods/sub/a.cs";
    let (sub, kept) = (disk.0.join("mods/sub"), disk.0.join("mods/kept"));
    let to_link = || {
        std::fs::rename(&sub, &kept).unwrap();
        assert!(link_folder(&outside.0, &sub));
    };
    let back = || {
        unlink_folder(&sub);
        std::fs::rename(&kept, &sub).unwrap();
    };
    let found = || plain_file(&disk.0, rel).map(|(at, _)| at);
    assert!(open_plain(&disk.0, rel).is_some());
    // A link in the folder's place from the check on: the open goes through it, the look after the open meets it.
    let mut looks = 0;
    let opened = open_found(|| {
        looks += 1;
        let at = found();
        if looks == 1 {
            to_link();
        }
        at
    });
    assert!(opened.is_none());
    back();
    // A link only for the open: the look after it finds the folder again, and a file that is not the one opened.
    let mut looks = 0;
    let opened = open_found(|| {
        looks += 1;
        if looks == 2 {
            back();
        }
        let at = found();
        if looks == 1 {
            to_link();
        }
        at
    });
    assert!(opened.is_none());
    assert_eq!(read_file(&disk.0, rel, 64), Some(b"inside".to_vec()));
    assert!(outside.0.join("a.cs").exists());
}

#[test]
fn zips_that_do_not_mount_are_charged_so_bad_archives_cannot_read_without_end() {
    let disk = Folder::new();
    let junk = [b'x'; 100];
    for i in 0..20 {
        disk.put(&format!("mods/bad{i:02}.zip"), &junk);
    }
    disk.put("mods/good.zip", &zip_of(&[("a.cs", b"a")]));
    let not_a_zip = |folder: &GameFolder| {
        refusals(folder)
            .iter()
            .filter(|(_, why)| why.starts_with("not a zip"))
            .count()
    };
    // By count: each bad zip read takes one of the zips allowed.
    let limits = Limits {
        mounted_zips: 8,
        ..Limits::default()
    };
    let folder = GameFolder::open(&disk.0, policy(None), limits, Vec::new(), Vec::new());
    assert!(folder.mounted().is_empty());
    assert_eq!(not_a_zip(&folder), 8);
    assert!(refusals(&folder).contains(&("good.zip".into(), "too many zips read (8)".into())));
    assert_eq!(folder.report().bytes, 800);
    // By bytes: what the bad zips read is spent.
    let limits = Limits {
        mounted_bytes: 1000,
        ..Limits::default()
    };
    let folder = GameFolder::open(&disk.0, policy(None), limits, Vec::new(), Vec::new());
    assert_eq!(not_a_zip(&folder), 10);
    assert_eq!(
        refusals(&folder)
            .iter()
            .filter(|(_, why)| why.starts_with("over the mount budget before reading"))
            .count(),
        11
    );
    // A host's bad archives are charged the same way, and a long name is cut in the report.
    let empty = Folder::new();
    empty.put("mods/keep.txt", b"");
    let mut archives: Vec<(String, Vec<u8>)> = (0..20)
        .map(|i| (format!("junk{i:02}"), junk.to_vec()))
        .collect();
    archives.push(("y".repeat(200), junk.to_vec()));
    let folder = GameFolder::open(
        &empty.0,
        policy(Some(b"mods")),
        limits,
        archives,
        Vec::new(),
    );
    // Each is let in only with room for two copies, so the tenth is refused before it is looked at.
    assert_eq!(not_a_zip(&folder), 9);
    assert_eq!(folder.report().bytes, 900);
    let last = refusals(&folder).pop().unwrap();
    assert_eq!(last.0, "y".repeat(80));
}

#[test]
fn zips_over_a_budget_still_spend_what_was_read() {
    let disk = Folder::new();
    // Many entries: a directory charged far over the zip's own bytes.
    let names: Vec<String> = (0..40).map(|i| format!("f{i:02}.cs")).collect();
    let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
    let zip = zip_of(&entries);
    for i in 0..20 {
        disk.put(&format!("mods/big{i:02}.zip"), &zip);
    }
    let budget = zip.len() * 7 / 2;
    let count = |folder: &GameFolder, why: &str| {
        refusals(folder)
            .iter()
            .filter(|(_, w)| w.starts_with(why))
            .count()
    };
    // Over the bytes with its directory: each read is spent, so only three are read.
    let limits = Limits {
        mounted_bytes: budget,
        ..Limits::default()
    };
    let folder = GameFolder::open(&disk.0, policy(None), limits, Vec::new(), Vec::new());
    assert!(folder.mounted().is_empty());
    assert_eq!(
        count(&folder, "over the mount budget with its directory"),
        3
    );
    assert_eq!(count(&folder, "over the bytes the mount may read"), 17);
    assert_eq!(folder.report().read, 3 * zip.len());
    assert!(folder.report().read <= budget);
    // Over the entries: the same.
    let limits = Limits {
        mounted_bytes: budget,
        mounted_entries: 10,
        ..Limits::default()
    };
    let folder = GameFolder::open(&disk.0, policy(None), limits, Vec::new(), Vec::new());
    assert!(folder.mounted().is_empty());
    assert!(folder.report().read <= budget);
    assert_eq!(
        count(&folder, "over the budget of entries") + count(&folder, "over the mount budget with"),
        3
    );
}
