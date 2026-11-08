//! The cache of downloaded content on a disk, and its index read from hostile input: names that are not entries, entries that are
//! not messages, messages that do not hash to their names.

use std::path::PathBuf;

use torque_mods::content::{ContentCaps, Hash, encode_content, from_hex, hex, summarize_files};
use torque_mods::{CacheStore, ContentCache, DiskStore, MemoryStore, index_of};

mod common;
use common::Rng;

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("mod-cache-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn caps() -> ContentCaps {
    ContentCaps {
        files: 100,
        file_bytes: 1 << 20,
        total_bytes: 8 << 20,
    }
}

fn entry(id: &str, size: usize, seed: u8) -> (Hash, Vec<u8>) {
    let files = vec![("art/a.bin".to_string(), vec![seed; size])];
    let hash = summarize_files(&files).hash;
    (hash, encode_content(id, "1.0", &hash, &files))
}

#[test]
fn a_disk_cache_keeps_entries_across_a_restart_and_evicts_the_oldest() {
    let dir = folder("restart");
    let (h1, m1) = entry("one", 400, 1);
    let (h2, m2) = entry("two", 400, 2);
    let (h3, m3) = entry("three", 400, 3);
    {
        let mut cache = ContentCache::new(Box::new(DiskStore::new(&dir)), 1 << 20);
        assert!(cache.store(&h1, &m1));
        assert!(cache.store(&h2, &m2));
        // The first is used again, so the second is the oldest.
        assert!(cache.load(&h1, &caps()).is_some());
    }
    // A new process opens the same folder: the entries and their order are there.
    let cap = (m1.len() + m2.len() + m3.len() - 10) as u64;
    let mut cache = ContentCache::new(Box::new(DiskStore::new(&dir)), cap);
    assert!(cache.has(&h1) && cache.has(&h2));
    assert!(cache.store(&h3, &m3));
    assert!(cache.has(&h1) && cache.has(&h3));
    assert!(!cache.has(&h2), "the entry used least recently went");
    assert!(cache.load(&h3, &caps()).is_some());
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().all(|n| n.ends_with(".mod")),
        "no half-written file is left: {names:?}"
    );
    cache.clear();
    assert!(!cache.has(&h1));
}

#[test]
fn a_folder_of_junk_is_ignored_and_never_read_as_content() {
    let dir = folder("junk");
    std::fs::create_dir_all(&dir).unwrap();
    let (hash, message) = entry("props", 300, 4);
    std::fs::write(dir.join("readme.txt"), b"hello").unwrap();
    std::fs::write(dir.join("main.cs"), b"echo(1);").unwrap();
    std::fs::create_dir_all(dir.join(format!("{}.mod", hex(&[7; 32])))).unwrap();
    std::fs::write(dir.join(format!("{}.mod", hex(&[8; 32]))), b"not a message").unwrap();
    let mut cache = ContentCache::new(Box::new(DiskStore::new(&dir)), 1 << 20);
    // The folder named like an entry is not a file; the junk file is an entry that does not read.
    assert_eq!(cache.index().len(), 1);
    assert!(cache.load(&[8; 32], &caps()).is_none());
    assert!(cache.index().is_empty(), "the junk entry was removed");
    assert!(cache.heads(8).is_empty());
    assert!(cache.store(&hash, &message));
    assert_eq!(cache.heads(8).len(), 1);
    assert!(
        dir.join("readme.txt").exists(),
        "files that are not entries are left alone"
    );
    assert!(dir.join("main.cs").exists());
}

#[test]
fn a_disk_store_refuses_names_that_leave_its_folder() {
    let dir = folder("names");
    let mut store = DiskStore::new(&dir);
    assert!(!store.write("../escape.mod", b"x"));
    assert!(!store.write("sub/x.mod", b"x"));
    assert!(!store.write("a\\b.mod", b"x"));
    assert!(store.read("../escape.mod", 10).is_none());
    assert!(store.write("fine.mod", b"x"));
    assert_eq!(store.read("fine.mod", 10), Some(b"x".to_vec()));
    assert_eq!(
        store.read("fine.mod", 0),
        None,
        "a file over the limit is not read"
    );
    assert!(!dir.parent().unwrap().join("escape.mod").exists());
}

#[test]
fn hostile_listings_and_entries_never_panic_and_never_load() {
    let mut rng = Rng(0x0ddb_a11c_ace5_0001);
    let (hash, good) = entry("props", 200, 6);
    for case in 0..20_000 {
        // A listing of invented names, some almost right.
        let listing: Vec<_> = (0..1 + rng.below(8))
            .map(|_| {
                let mut name = match rng.below(4) {
                    0 => format!("{}.mod", hex(&[rng.next() as u8; 32])),
                    1 => hex(&[rng.next() as u8; 32]),
                    2 => (0..rng.below(90))
                        .map(|_| (rng.next() % 94 + 33) as u8 as char)
                        .collect(),
                    _ => format!("{}.mod", hex(&hash)),
                };
                if rng.below(3) == 0 && !name.is_empty() {
                    let at = rng.below(name.len());
                    name.replace_range(at..=at, "Z");
                }
                torque_mods::Listed {
                    name,
                    size: rng.next(),
                    stamp: rng.next(),
                }
            })
            .collect();
        for found in index_of(&listing) {
            assert_eq!(from_hex(&hex(&found.hash)), Some(found.hash));
        }
        // An entry under a good name with hostile bytes: never loads, and is gone afterwards.
        let mut bytes = good.clone();
        match rng.below(4) {
            0 => {
                let at = rng.below(bytes.len());
                bytes[at] ^= 1 << rng.below(8);
            }
            1 => bytes.truncate(rng.below(bytes.len() + 1)),
            2 => bytes = (0..rng.below(400)).map(|_| rng.next() as u8).collect(),
            _ => bytes.extend((0..rng.below(20)).map(|_| rng.next() as u8)),
        }
        let mut store = MemoryStore::default();
        store.write(&format!("{}.mod", hex(&hash)), &bytes);
        let mut cache = ContentCache::new(Box::new(store), 1 << 20);
        let loaded = cache.load(&hash, &caps());
        match loaded {
            // Only the unchanged message (a flipped bit in a file's bytes changes the hash, and one elsewhere breaks the parse).
            Some(files) => assert_eq!(summarize_files(&files).hash, hash, "{case}"),
            None => assert!(!cache.has(&hash), "{case}"),
        }
        let _ = cache.heads(8);
    }
}

#[test]
fn opening_a_folder_sweeps_half_written_files_and_a_write_never_takes_a_planted_name() {
    let dir = folder("sweep");
    std::fs::create_dir_all(&dir).unwrap();
    let (hash, message) = entry("props", 300, 4);
    let name = format!("{}.mod", hex(&hash));
    // What an earlier run left in the middle of a write, and a file of another kind that merely looks alike.
    std::fs::write(dir.join(format!("{name}.part")), b"half").unwrap();
    std::fs::write(dir.join("old.1-7.part"), b"half").unwrap();
    std::fs::write(dir.join("keep.txt"), b"mine").unwrap();
    let mut store = DiskStore::new(&dir);
    assert!(!dir.join(format!("{name}.part")).exists());
    assert!(!dir.join("old.1-7.part").exists());
    assert!(
        dir.join("keep.txt").exists(),
        "only half-written files are swept"
    );
    // A write goes through a new file of its own, so a file planted at the name of an entry (or of a guessable temporary) is
    // replaced, not written through, and no temporary is left.
    std::fs::write(dir.join(&name), b"planted").unwrap();
    assert!(store.write(&name, &message));
    assert_eq!(store.read(&name, 1 << 20), Some(message.clone()));
    let left: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".part"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}
