use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use super::*;
use crate::cache::{CacheOwner, CacheScope, ContentCache};
use crate::content::{ContentCaps, Hash, encode_content, hex, summarize_files};

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

fn server(port: u16) -> CacheOwner {
    CacheOwner::of_server(std::net::SocketAddr::from(([203, 0, 113, 7], port)))
}

/// A database that keeps what it is told, as the browser's two object stores do: the bytes by name and the stamps by name.
#[derive(Default)]
struct Db {
    content: BTreeMap<String, Vec<u8>>,
    stamps: BTreeMap<String, u64>,
    /// A refusing database (quota, a closed connection) keeps no write.
    refusing: bool,
}

#[derive(Clone, Default)]
struct Fake(Rc<RefCell<Db>>);

impl Backing for Fake {
    fn put(&self, name: &str, bytes: &[u8]) {
        let mut db = self.0.borrow_mut();
        if !db.refusing {
            db.content.insert(name.to_string(), bytes.to_vec());
        }
    }

    fn remove(&self, name: &str) {
        let mut db = self.0.borrow_mut();
        db.content.remove(name);
        db.stamps.remove(name);
    }

    fn stamp(&self, name: &str, stamp: u64) {
        let mut db = self.0.borrow_mut();
        if !db.refusing {
            db.stamps.insert(name.to_string(), stamp);
        }
    }
}

impl Fake {
    /// What a page that starts now reads back.
    fn records(&self) -> Vec<Record> {
        let db = self.0.borrow();
        db.content
            .iter()
            .map(|(name, bytes)| Record {
                name: name.clone(),
                bytes: bytes.clone(),
                stamp: db.stamps.get(name).copied().unwrap_or(0),
            })
            .collect()
    }

    /// A cache for a page that starts now.
    fn reload(&self, cap: u64) -> ContentCache {
        let store = MirroredStore::load(self.clone(), self.records(), cap);
        ContentCache::new(Box::new(store), cap)
    }

    fn names(&self) -> Vec<String> {
        self.0.borrow().content.keys().cloned().collect()
    }
}

fn fresh(db: &Fake, cap: u64) -> ContentCache {
    ContentCache::new(Box::new(MirroredStore::new(db.clone(), cap)), cap)
}

#[test]
fn what_one_page_stored_the_next_page_finds_verified() {
    let db = Fake::default();
    let (hash, message) = entry("props", 500, 3);
    let mut first = fresh(&db, 1 << 20);
    assert!(first.store(&hash, &message));
    drop(first);
    let mut second = db.reload(1 << 20);
    assert!(second.has(&hash));
    assert_eq!(
        second.heads(4),
        vec![("props".to_string(), "1.0".to_string(), hash)]
    );
    let files = second.load(&hash, &caps()).expect("a hit after a reload");
    assert_eq!(files[0].1, vec![3; 500]);
}

#[test]
fn an_entry_is_kept_for_the_server_it_came_from_after_a_reload() {
    let db = Fake::default();
    let (hash, message) = entry("props", 100, 1);
    let scope = |owner, share| CacheScope {
        owner: Some(owner),
        share,
    };
    let mut first = fresh(&db, 1 << 20);
    first.set_scope(scope(server(1), false));
    first.store(&hash, &message);
    let mut second = db.reload(1 << 20);
    second.set_scope(scope(server(2), false));
    assert!(!second.has(&hash), "another server has no sight of it");
    second.set_scope(scope(server(1), false));
    assert!(second.has(&hash));
    second.set_scope(scope(server(2), true));
    assert!(second.has(&hash), "sharing lets any server match it");
    let name = db.names().remove(0);
    assert!(name.starts_with(&hex(&hash)));
    assert!(name.contains(&format!(".{}.", server(1).to_hex())));
}

#[test]
fn a_changed_entry_is_dropped_from_memory_and_the_database_and_downloaded_again() {
    let db = Fake::default();
    let (hash, message) = entry("props", 500, 3);
    fresh(&db, 1 << 20).store(&hash, &message);
    // Somebody changes a stored byte inside a file.
    let name = db.names().remove(0);
    let at = message.len() - 10;
    db.0.borrow_mut().content.get_mut(&name).expect("stored")[at] ^= 0xff;
    let mut cache = db.reload(1 << 20);
    assert!(cache.has(&hash), "the name alone is not proof");
    assert!(cache.load(&hash, &caps()).is_none());
    assert!(!cache.has(&hash));
    assert!(db.names().is_empty(), "the database lost it too");
    // The download stores it whole, and the next page reads it.
    assert!(cache.store(&hash, &message));
    assert!(db.reload(1 << 20).load(&hash, &caps()).is_some());
}

#[test]
fn an_entry_whose_bytes_are_another_mods_is_not_mounted() {
    let db = Fake::default();
    let (hash, message) = entry("props", 500, 3);
    let (_, other) = entry("props", 500, 4);
    fresh(&db, 1 << 20).store(&hash, &message);
    let name = db.names().remove(0);
    db.0.borrow_mut().content.insert(name, other);
    let mut cache = db.reload(1 << 20);
    assert!(cache.load(&hash, &caps()).is_none());
    assert!(db.names().is_empty());
}

#[test]
fn the_cap_evicts_the_least_recently_used_in_the_database_as_well() {
    let db = Fake::default();
    let (h1, m1) = entry("one", 400, 1);
    let (h2, m2) = entry("two", 400, 2);
    let (h3, m3) = entry("three", 400, 3);
    let cap = (m1.len() + m2.len() + m3.len() - 10) as u64;
    let mut cache = fresh(&db, cap);
    cache.store(&h1, &m1);
    cache.store(&h2, &m2);
    assert!(cache.load(&h1, &caps()).is_some(), "uses the first");
    cache.store(&h3, &m3);
    assert!(cache.has(&h1) && cache.has(&h3) && !cache.has(&h2));
    assert_eq!(db.names().len(), 2);
    // The order of use survives the reload: the first was used before the third was stored, so it is the older.
    let mut again = db.reload(cap);
    let (h4, m4) = entry("four", 400, 4);
    again.store(&h4, &m4);
    assert!(again.has(&h4) && again.has(&h3), "{:?}", again.index());
    assert!(!again.has(&h1), "the oldest use went");
}

#[test]
fn a_store_that_holds_less_than_the_cap_asks_lowers_it() {
    let db = Fake::default();
    let (hash, message) = entry("big", 3_000, 1);
    let mut cache = ContentCache::new(Box::new(MirroredStore::new(db.clone(), 1_000)), 512 << 20);
    assert_eq!(cache.max_bytes(), 1_000);
    cache.set_max_bytes(u64::MAX);
    assert_eq!(cache.max_bytes(), 1_000);
    assert!(!cache.store(&hash, &message));
    assert!(db.names().is_empty());
}

#[test]
fn what_is_not_an_entry_or_does_not_fit_is_removed_from_the_database_on_load() {
    let db = Fake::default();
    let (h1, m1) = entry("one", 400, 1);
    let (h2, m2) = entry("two", 400, 2);
    let (h3, m3) = entry("three", 400, 3);
    let cap = (m1.len() + m2.len()) as u64;
    {
        let mut d = db.0.borrow_mut();
        for (name, bytes, stamp) in [
            (format!("{}.mod", hex(&h1)), &m1, 5),
            (format!("{}.mod", hex(&h2)), &m2, 9),
            (format!("{}.mod", hex(&h3)), &m3, 1),
        ] {
            d.content.insert(name.clone(), bytes.clone());
            d.stamps.insert(name, stamp);
        }
        d.content.insert("notes.txt".into(), vec![1; 10]);
        d.content
            .insert(format!("{}.mod", hex(&h1)).to_uppercase(), vec![1; 10]);
    }
    let cache = db.reload(cap);
    assert!(
        cache.has(&h1) && cache.has(&h2),
        "the most recently used fit"
    );
    assert!(!cache.has(&h3), "the least recently used is over the cap");
    assert_eq!(
        db.names().len(),
        2,
        "the rest left the database: {:?}",
        db.names()
    );
}

#[test]
fn a_database_that_refuses_writes_leaves_a_cache_that_works_for_the_session() {
    let db = Fake::default();
    db.0.borrow_mut().refusing = true;
    let (hash, message) = entry("props", 500, 3);
    let mut cache = fresh(&db, 1 << 20);
    assert!(cache.store(&hash, &message));
    assert!(cache.load(&hash, &caps()).is_some());
    assert!(db.names().is_empty());
    assert!(
        db.reload(1 << 20).load(&hash, &caps()).is_none(),
        "the next page downloads it again"
    );
}

#[test]
fn a_database_with_junk_names_loads_what_is_valid() {
    let db = Fake::default();
    let (hash, message) = entry("props", 100, 1);
    fresh(&db, 1 << 20).store(&hash, &message);
    db.0.borrow_mut()
        .content
        .insert("..%2f.mod".into(), vec![7; 3]);
    let cache = db.reload(1 << 20);
    assert!(cache.has(&hash));
    assert_eq!(cache.index().len(), 1);
}

#[test]
fn two_servers_behind_one_address_cannot_see_each_others_entries_even_after_a_reload() {
    // A browser's link dials one placeholder address for every server; the client names the server instead.
    let placeholder = std::net::SocketAddr::from(([127, 0, 0, 1], 28_000));
    let scope = |name: &str| CacheScope {
        owner: CacheOwner::of_connection(Some(name), Some(placeholder)),
        share: false,
    };
    let a = crate::cache::server_name_of_url("ws://a.example:28000/ws").unwrap();
    let b = crate::cache::server_name_of_url("ws://b.example:28000/ws").unwrap();
    let room = crate::cache::server_name_of_room("wss://rooms.example", "abcd").unwrap();
    let db = Fake::default();
    let (hash, message) = entry("props", 100, 1);
    let mut first = fresh(&db, 1 << 20);
    first.set_scope(scope(&a));
    first.store(&hash, &message);
    for page in [first, db.reload(1 << 20)] {
        let mut cache = page;
        cache.set_scope(scope(&b));
        assert!(!cache.has(&hash) && cache.heads(8).is_empty());
        assert!(cache.load(&hash, &caps()).is_none());
        cache.set_scope(scope(&room));
        assert!(!cache.has(&hash));
        cache.set_scope(scope(&a));
        assert!(cache.has(&hash) && cache.heads(8).len() == 1);
    }
}
