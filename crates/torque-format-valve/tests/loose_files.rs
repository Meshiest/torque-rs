//! Loose files and archives on disk: read once by the length of the open file, never past the listing, no links followed.

use std::fs;
use std::path::PathBuf;

use torque_format_valve::Error;
use torque_format_valve::files::SourceFiles;
use torque_format_valve::read::read_plain_file;
use torque_format_valve::vpk::{VpkArchive, write_dir};

/// A fresh folder under the build's own scratch folder.
fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("loose-files-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_listed_file_that_grew_is_refused_not_read() {
    let dir = folder("grew");
    fs::write(dir.join("a.txt"), b"abc").unwrap();
    let mut files = SourceFiles::new();
    files.mount_folder(&dir).unwrap();
    assert_eq!(files.read_path("a.txt").unwrap().unwrap(), b"abc");
    // The file is replaced by a larger one after the listing.
    fs::write(dir.join("a.txt"), vec![7u8; 4096]).unwrap();
    assert!(matches!(
        files.read_path("a.txt").unwrap(),
        Err(Error::TooLarge { cap: 3, .. })
    ));
    // A smaller one reads as it is.
    fs::write(dir.join("a.txt"), b"a").unwrap();
    assert_eq!(files.read_path("a.txt").unwrap().unwrap(), b"a");
}

#[test]
fn a_read_stops_at_the_cap_whatever_the_file_says() {
    let dir = folder("cap");
    let path = dir.join("big.bin");
    fs::write(&path, vec![0u8; 1000]).unwrap();
    assert_eq!(read_plain_file(&path, 1000, None).unwrap().len(), 1000);
    assert!(matches!(
        read_plain_file(&path, 999, None),
        Err(Error::TooLarge {
            asked: 1000,
            cap: 999,
            ..
        })
    ));
    assert!(matches!(
        read_plain_file(&path, 1000, Some(10)),
        Err(Error::TooLarge { cap: 10, .. })
    ));
}

#[test]
fn a_folder_is_not_a_file_and_vpk_pieces_are_not_loose_files() {
    let dir = folder("vpk");
    fs::write(dir.join("pak01_000.vpk"), b"data").unwrap();
    fs::write(dir.join("readme.txt"), b"hi").unwrap();
    fs::create_dir(dir.join("pak02_dir.vpk")).unwrap();
    let mut files = SourceFiles::new();
    files.mount_folder(&dir).unwrap();
    assert_eq!(
        files.len_of("pak01_000.vpk"),
        None,
        "an archive's piece is not a loose file"
    );
    assert_eq!(files.len_of("readme.txt"), Some(2));
    // A directory file that is a folder is refused.
    assert!(VpkArchive::open(&dir.join("pak02_dir.vpk")).is_err());
    assert!(read_plain_file(&dir.join("pak02_dir.vpk"), 1 << 20, None).is_err());
}

#[test]
fn a_directory_file_reads_through_the_capped_reader() {
    let dir = folder("dir");
    fs::write(
        dir.join("pak03_dir.vpk"),
        write_dir(&[("a/b.txt", b"hello")], 2, 5),
    )
    .unwrap();
    let archive = VpkArchive::open(&dir.join("pak03.vpk")).unwrap();
    assert_eq!(archive.read("a/b.txt").unwrap().unwrap(), b"hello");
}

/// A link out of the game: a `_dir.vpk` or a loose file that is a symlink is not followed.
#[cfg(unix)]
#[test]
fn links_are_not_followed() {
    use std::os::unix::fs::symlink;
    let dir = folder("links");
    let outside = folder("links-outside");
    fs::write(outside.join("secret.txt"), b"secret").unwrap();
    fs::write(
        outside.join("real_dir.vpk"),
        write_dir(&[("a/b.txt", b"hello")], 2, 5),
    )
    .unwrap();
    symlink(outside.join("secret.txt"), dir.join("secret.txt")).unwrap();
    symlink(outside.join("real_dir.vpk"), dir.join("pak_dir.vpk")).unwrap();
    assert!(VpkArchive::open(&dir.join("pak_dir.vpk")).is_err());
    assert!(read_plain_file(&dir.join("secret.txt"), 1 << 20, None).is_err());
    let mut files = SourceFiles::new();
    files.mount_folder(&dir).unwrap();
    assert_eq!(files.len_of("secret.txt"), None, "the listing skips links");
}
