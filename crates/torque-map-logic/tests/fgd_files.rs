//! FGD files on disk: one open, the length of the open file, no links.

use std::fs;
use std::path::PathBuf;

use torque_map_logic::fgd::{DirSource, FgdSource, MAX_FILE_BYTES, read_plain};

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("fgd-files-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_file_over_the_cap_at_read_time_is_refused_and_one_at_the_cap_is_read() {
    let dir = folder("cap");
    let source = DirSource { dir: dir.clone() };
    fs::write(dir.join("a.fgd"), vec![b' '; MAX_FILE_BYTES]).unwrap();
    assert_eq!(source.read("a.fgd").map(|b| b.len()), Some(MAX_FILE_BYTES));
    // Replaced by a larger file after a first look: the length comes from the open file, so it is refused.
    fs::write(dir.join("a.fgd"), vec![b' '; MAX_FILE_BYTES + 1]).unwrap();
    assert_eq!(source.read("a.fgd"), None);
    // The reader stops past its own limit whatever the file says.
    assert_eq!(read_plain(&dir.join("a.fgd"), 10), None);
    assert_eq!(
        read_plain(&dir.join("a.fgd"), (MAX_FILE_BYTES + 1) as u64).map(|b| b.len()),
        Some(MAX_FILE_BYTES + 1)
    );
}

#[test]
fn folders_and_missing_names_are_not_files() {
    let dir = folder("kinds");
    fs::create_dir(dir.join("d.fgd")).unwrap();
    let source = DirSource { dir };
    assert_eq!(source.read("d.fgd"), None);
    assert_eq!(source.read("missing.fgd"), None);
    assert_eq!(source.read("../x.fgd"), None);
}

#[cfg(unix)]
#[test]
fn a_link_is_refused() {
    let dir = folder("link");
    let outside = folder("link-outside");
    fs::write(outside.join("x.fgd"), b"@PointClass = a : \"a\" []").unwrap();
    std::os::unix::fs::symlink(outside.join("x.fgd"), dir.join("x.fgd")).unwrap();
    assert_eq!(DirSource { dir }.read("x.fgd"), None);
}
