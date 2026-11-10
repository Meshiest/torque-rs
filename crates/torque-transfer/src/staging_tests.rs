use super::*;

/// A folder next to the test binary (inside the build directory), removed when the test ends.
struct Folder(PathBuf);

impl Folder {
    fn new(name: &str) -> Folder {
        let exe = std::env::current_exe().expect("the test binary's path");
        let dir = exe
            .parent()
            .expect("a folder")
            .join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Folder(dir)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn config(folder: Option<PathBuf>) -> StagingConfig {
    StagingConfig {
        memory_per_upload: 1000,
        memory_total: 1500,
        disk_total: 10_000_000,
        per_subject: 1 << 30,
        folder,
    }
}

#[test]
fn small_uploads_stage_in_memory_up_to_the_total() {
    let mut pool = Pool::new(config(None));
    let a = pool.open(1000).unwrap();
    assert_eq!(pool.memory_used(), 1000);
    // Fits the per-upload cap but not what is left of the total.
    assert_eq!(pool.open(600).err(), Some(Refusal::ServerBusy));
    let b = pool.open(500).unwrap();
    assert_eq!(pool.memory_used(), 1500);
    pool.release(a);
    pool.release(b);
    assert_eq!(pool.memory_used(), 0);
}

#[test]
fn with_no_folder_a_large_upload_is_refused() {
    let mut pool = Pool::new(config(None));
    assert_eq!(pool.open(1001).err(), Some(Refusal::ServerBusy));
}

#[test]
fn blocks_are_written_in_place_and_read_back() {
    let mut pool = Pool::new(config(None));
    let size = 100;
    let mut staged = pool.open(size).unwrap();
    staged.write_block(0, &[9u8; 100]).unwrap();
    let view = staged.view();
    assert_eq!(view.len(), size);
    assert_eq!(view.as_slice(), Some(&[9u8; 100][..]));
    let mut buf = [0u8; 10];
    assert_eq!(view.read_at(95, &mut buf).unwrap(), 5);
    assert_eq!(view.read_at(500, &mut buf).unwrap(), 0);
    assert_eq!(
        view.to_vec(99).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(view.to_vec(100).unwrap(), vec![9u8; 100]);
}

#[test]
fn a_block_past_the_end_is_an_error() {
    let mut pool = Pool::new(config(None));
    let mut staged = pool.open(10).unwrap();
    assert!(staged.write_block(0, &[0u8; 11]).is_err());
    assert!(staged.write_block(1, &[0u8; 1]).is_err());
    assert!(staged.write_block(u32::MAX, &[0u8; 1]).is_err());
}

#[test]
fn large_uploads_stage_in_a_file_that_is_deleted_with_them() {
    let folder = Folder::new("staging-file");
    let mut pool = Pool::new(config(Some(folder.0.clone())));
    let size = BLOCK_BYTES as u64 + 10;
    let mut staged = pool.open(size).unwrap();
    assert_eq!(pool.disk_used(), size);
    assert_eq!(fs::read_dir(&folder.0).unwrap().count(), 1);
    staged.write_block(1, &[2u8; 10]).unwrap();
    staged.write_block(0, &vec![1u8; BLOCK_BYTES]).unwrap();
    let view = staged.view();
    assert!(view.as_slice().is_none());
    let all = view.to_vec(usize::MAX).unwrap();
    assert_eq!(all.len() as u64, size);
    assert!(all[..BLOCK_BYTES].iter().all(|&b| b == 1));
    assert!(all[BLOCK_BYTES..].iter().all(|&b| b == 2));
    pool.release(staged);
    assert_eq!(pool.disk_used(), 0);
    assert_eq!(fs::read_dir(&folder.0).unwrap().count(), 0);
}

#[test]
fn the_disk_total_is_a_cap() {
    let folder = Folder::new("staging-cap");
    let mut cfg = config(Some(folder.0.clone()));
    cfg.disk_total = 5_000;
    let mut pool = Pool::new(cfg);
    let a = pool.open(3_000).unwrap();
    assert_eq!(pool.open(3_000).err(), Some(Refusal::ServerBusy));
    pool.release(a);
    assert!(pool.open(3_000).is_ok());
}

#[test]
fn files_left_by_an_earlier_run_are_deleted() {
    let folder = Folder::new("staging-leftovers");
    fs::create_dir_all(&folder.0).unwrap();
    fs::write(folder.0.join("upload-00000000000000ff.part"), b"left over").unwrap();
    fs::write(folder.0.join("upload-0123456789abcdef.part"), b"left over").unwrap();
    let _pool = Pool::new(config(Some(folder.0.clone())));
    assert_eq!(fs::read_dir(&folder.0).unwrap().count(), 0);
}

#[test]
fn only_files_named_like_staging_files_are_deleted() {
    let folder = Folder::new("staging-others");
    fs::create_dir_all(folder.0.join("sub")).unwrap();
    let keep = [
        "notes.txt",
        "run.lock",
        "upload-.part",
        "upload-xyz.part",
        "upload-0123456789abcdef.part.bak",
        "upload-0123456789abcdeg.part",
        "xupload-0123456789abcdef.part",
    ];
    for name in keep {
        fs::write(folder.0.join(name), b"mine").unwrap();
    }
    fs::write(folder.0.join("sub/upload-0123456789abcdef.part"), b"mine").unwrap();
    fs::write(folder.0.join("upload-0123456789abcdef.part"), b"theirs").unwrap();
    let mut pool = Pool::new(config(Some(folder.0.clone())));
    for name in keep {
        assert!(folder.0.join(name).exists(), "{name} was deleted");
    }
    assert!(folder.0.join("sub/upload-0123456789abcdef.part").exists());
    assert!(!folder.0.join("upload-0123456789abcdef.part").exists());
    // The files it makes carry the marker.
    let staged = pool.open(2_000).unwrap();
    let made: Vec<_> = fs::read_dir(&folder.0)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|n| !keep.contains(&n.to_str().unwrap()) && n != "sub")
        .collect();
    assert_eq!(made.len(), 1);
    assert!(is_staging_name(&made[0]), "{made:?}");
    drop(staged);
}

#[test]
fn an_unusable_folder_falls_back_to_memory_with_a_warning() {
    let folder = Folder::new("staging-unusable");
    fs::create_dir_all(folder.0.parent().unwrap()).unwrap();
    // A file where the folder should be.
    fs::write(&folder.0, b"x").unwrap();
    let pool = Pool::new(config(Some(folder.0.clone())));
    assert!(pool.warning.is_some());
    assert!(pool.config().folder.is_none());
    let _ = fs::remove_file(&folder.0);
}

#[test]
fn file_names_are_random_and_not_repeated() {
    let folder = Folder::new("staging-names");
    let mut pool = Pool::new(config(Some(folder.0.clone())));
    let a = pool.open(2_000).unwrap();
    let b = pool.open(2_000).unwrap();
    let names: std::collections::HashSet<_> = fs::read_dir(&folder.0)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 2);
    drop((a, b));
}
