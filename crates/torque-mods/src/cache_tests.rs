use super::*;
use crate::content::{ContentCaps, encode_content, summarize_files};

fn caps() -> ContentCaps {
    ContentCaps {
        files: 100,
        file_bytes: 1 << 20,
        total_bytes: 8 << 20,
    }
}

/// A content message for a mod of `size` bytes: its hash and the message.
fn entry(id: &str, size: usize, seed: u8) -> (Hash, Vec<u8>) {
    let files = vec![("art/a.bin".to_string(), vec![seed; size])];
    let hash = summarize_files(&files).hash;
    (hash, encode_content(id, "1.0", &hash, &files))
}

fn listed(name: &str) -> Listed {
    Listed {
        name: name.to_string(),
        size: 10,
        stamp: 1,
    }
}

#[test]
fn only_files_named_for_a_hash_are_entries() {
    let (hash, _) = entry("a", 10, 1);
    let good = format!("{}{ENTRY_SUFFIX}", hex(&hash));
    let upper = good.to_uppercase();
    let short = format!("{}{ENTRY_SUFFIX}", &hex(&hash)[1..]);
    let listing = [
        listed(&good),
        listed(&upper),
        listed(&short),
        listed("notes.txt"),
        listed(".mod"),
        listed(&format!("{}.mod.part", hex(&hash))),
        listed(&format!("../{good}")),
        listed(&format!("sub/{good}")),
        listed(&good),
    ];
    let index = index_of(&listing);
    assert_eq!(index.len(), 1);
    assert_eq!(index[0].hash, hash);
}

#[test]
fn a_listing_of_junk_is_bounded() {
    let listing: Vec<Listed> = (0..50_000)
        .map(|i| {
            let mut hash = [0u8; 32];
            hash[..4].copy_from_slice(&(i as u32).to_le_bytes());
            listed(&format!("{}{ENTRY_SUFFIX}", hex(&hash)))
        })
        .collect();
    assert_eq!(index_of(&listing).len(), MAX_ENTRIES);
}

#[test]
fn an_entry_stored_loads_back_and_counts_as_used() {
    let mut cache = ContentCache::in_memory(1 << 20);
    let (hash, message) = entry("props", 500, 3);
    assert!(cache.load(&hash, &caps()).is_none());
    assert!(cache.store(&hash, &message));
    assert!(cache.has(&hash));
    let files = cache.load(&hash, &caps()).expect("a hit");
    assert_eq!(files[0].1.len(), 500);
    assert_eq!(
        cache.heads(8),
        vec![("props".to_string(), "1.0".to_string(), hash)]
    );
    cache.clear();
    assert!(!cache.has(&hash));
}

#[test]
fn an_entry_whose_files_do_not_hash_to_its_name_is_deleted_not_used() {
    let mut cache = ContentCache::in_memory(1 << 20);
    let (hash, _) = entry("props", 500, 3);
    // A message that claims the hash but carries other bytes.
    let other = vec![("art/a.bin".to_string(), vec![9; 500])];
    let forged = encode_content("props", "1.0", &hash, &other);
    cache.store(&hash, &forged);
    assert!(cache.has(&hash));
    assert!(cache.load(&hash, &caps()).is_none());
    assert!(!cache.has(&hash), "the forged entry was removed");
    // Garbage under a good name goes the same way, and so does an entry with a script in it.
    cache.store(&hash, b"not a message");
    assert!(cache.load(&hash, &caps()).is_none());
    assert!(!cache.has(&hash));
    let with_script = vec![
        ("art/a.bin".to_string(), vec![3; 500]),
        ("main.cs".to_string(), b"echo(1);".to_vec()),
    ];
    let hash2 = summarize_files(&with_script).hash;
    cache.store(
        &hash2,
        &encode_content("props", "1.0", &hash2, &with_script),
    );
    assert!(cache.load(&hash2, &caps()).is_none());
    assert!(!cache.has(&hash2));
}

#[test]
fn the_cache_evicts_what_was_used_least_recently_and_keeps_to_its_cap() {
    let (h1, m1) = entry("one", 400, 1);
    let (h2, m2) = entry("two", 400, 2);
    let (h3, m3) = entry("three", 400, 3);
    let cap = (m1.len() + m2.len() + m3.len() - 10) as u64;
    let mut cache = ContentCache::in_memory(cap);
    cache.store(&h1, &m1);
    cache.store(&h2, &m2);
    // Using the first makes the second the oldest.
    assert!(cache.load(&h1, &caps()).is_some());
    cache.store(&h3, &m3);
    assert!(cache.has(&h1) && cache.has(&h3));
    assert!(!cache.has(&h2), "the least recently used entry went");
    assert!(cache.bytes() <= cap);
    // An entry above the cap is not kept, and does not disturb the others.
    let (big, big_message) = entry("big", 5_000, 4);
    assert!(!cache.store(&big, &big_message));
    assert!(cache.has(&h1) && cache.has(&h3));
    // Lowering the cap evicts on the next store.
    cache.set_max_bytes(m1.len() as u64 + 10);
    let (h4, m4) = entry("four", 400, 5);
    cache.store(&h4, &m4);
    assert!(cache.has(&h4));
    assert!(cache.bytes() <= m1.len() as u64 + 10);
    assert!(!cache.has(&h1) && !cache.has(&h3));
}

#[test]
fn one_server_storing_new_content_cannot_flush_another_servers_entries() {
    let server = |port| CacheOwner::of_server(std::net::SocketAddr::from(([203, 0, 113, 7], port)));
    let scope = |owner| CacheScope {
        owner: Some(owner),
        share: false,
    };
    let (a, b) = (server(1), server(2));
    let one = entry("x", 400, 0).1.len() as u64;
    let mut cache = ContentCache::in_memory(one * 8);
    // Server A fills the cache.
    cache.set_scope(scope(a));
    let mut a_hashes = Vec::new();
    for seed in 0..8 {
        let (hash, message) = entry(&format!("a{seed}"), 400, seed);
        assert!(cache.store(&hash, &message));
        a_hashes.push(hash);
    }
    // Server B stores on every connect: A's newest entries survive, B churns its own half.
    cache.set_scope(scope(b));
    for seed in 0..40 {
        let (hash, message) = entry(&format!("b{seed}"), 400, 100 + seed);
        assert!(cache.store(&hash, &message));
        assert!(cache.bytes() <= one * 8);
    }
    cache.set_scope(scope(a));
    let kept = a_hashes.iter().filter(|h| cache.has(h)).count();
    assert!(kept >= 3, "A keeps about its half: {kept} of 8");
    assert!(cache.has(&a_hashes[7]), "A's newest entry survives");
}

#[test]
fn heads_skip_entries_that_do_not_start_like_a_message() {
    let mut cache = ContentCache::in_memory(1 << 20);
    let (hash, message) = entry("good", 100, 1);
    cache.store(&hash, &message);
    let junk = [2u8; 32];
    cache.store(&junk, b"short junk");
    let heads = cache.heads(10);
    assert_eq!(heads.len(), 1);
    assert_eq!(heads[0].0, "good");
}

#[test]
fn entries_are_named_for_their_server_and_listed_apart() {
    let (hash, message) = entry("a", 10, 1);
    let a = CacheOwner::of_server(std::net::SocketAddr::from(([203, 0, 113, 7], 28000)));
    let b = CacheOwner::of_server(std::net::SocketAddr::from(([203, 0, 113, 7], 28001)));
    assert_ne!(a, b);
    let mapped = std::net::SocketAddr::from((
        std::net::Ipv4Addr::new(203, 0, 113, 7).to_ipv6_mapped(),
        28000,
    ));
    assert_eq!(
        CacheOwner::of_server(mapped),
        a,
        "an IPv4 address by any name"
    );
    let mut cache = ContentCache::in_memory(1 << 20);
    cache.set_scope(CacheScope {
        owner: Some(a),
        share: false,
    });
    assert!(cache.store(&hash, &message));
    assert!(cache.has(&hash));
    cache.set_scope(CacheScope {
        owner: Some(b),
        share: false,
    });
    assert!(!cache.has(&hash));
    cache.set_scope(CacheScope {
        owner: Some(b),
        share: true,
    });
    assert!(cache.has(&hash));
    assert_eq!(cache.index().len(), 1);
    assert_eq!(cache.index()[0].owner, Some(a));
}
