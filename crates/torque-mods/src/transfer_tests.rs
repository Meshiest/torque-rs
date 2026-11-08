use super::*;
use crate::cache::CacheOwner;
use crate::content::encode_content;
use crate::offer::{FileOffer, encode_data, encode_offer};

type Files = Vec<(String, Vec<u8>)>;

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| ((i * 31 + usize::from(seed) * 17) % 251) as u8)
        .collect()
}

/// A mod of three files: one of three blocks, one small, one that never changes.
fn files(seed: u8) -> Files {
    vec![
        (
            "art/big.bin".to_string(),
            pattern(BLOCK_BYTES * 2 + 500, seed),
        ),
        ("art/small.bin".to_string(), pattern(1_000, 9)),
        ("sounds/boom.ogg".to_string(), pattern(3_000, 4)),
    ]
}

/// What the server's offer of `files` says, with the files in the order the offer lists them.
fn offered(id: &str, version: &str, files: &Files, download: bool) -> (ModOffer, Files) {
    let mut sorted = files.clone();
    sorted.sort_by_key(|(p, _)| p.to_ascii_lowercase());
    let offer = ModOffer {
        id: id.to_string(),
        version: version.to_string(),
        hash: summarize_files(&sorted).hash,
        download,
        files: sorted
            .iter()
            .map(|(p, d)| FileOffer::of(p, d).unwrap())
            .collect(),
    };
    (offer, sorted)
}

fn caps(downloads: bool) -> Caps {
    Caps {
        downloads,
        limits: DownloadLimits::default(),
    }
}

struct Installed(Vec<(String, Files)>);

impl LocalSource for Installed {
    fn hash_of(&self, id: &str) -> Option<Hash> {
        self.0
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, f)| summarize_files(f).hash)
    }

    fn files_of(&self, id: &str) -> Option<Files> {
        self.0.iter().find(|(i, _)| i == id).map(|(_, f)| f.clone())
    }
}

struct Rig {
    d: Downloads,
    table: Rc<Table>,
}

fn rig(caps: Caps) -> Rig {
    let table = Rc::new(Table::default());
    let d = Downloads::new(
        Rc::clone(&table),
        ModLimits::default(),
        caps,
        ContentCache::in_memory(1 << 24),
    );
    d.reset();
    Rig { d, table }
}

impl Rig {
    /// Hands the client the offer `mods` and serves what it asks for from `content` (per mod, in offer order); the bytes the
    /// server sent as data.
    fn join(&self, mods: &[ModOffer], content: &[Files]) -> u64 {
        self.d.reset();
        self.d.receive(&encode_offer(mods));
        let mut sent = 0;
        for _ in 0..10_000 {
            match self.d.next_command() {
                Some(Command::Get(r)) => {
                    let file = &content[r.mod_index as usize][r.file as usize].1;
                    let (from, to) =
                        run_bytes(file.len() as u64, u64::from(r.start), u64::from(r.count));
                    let data = &file[from as usize..to as usize];
                    sent += data.len() as u64;
                    self.d.receive(&encode_data(&r, data));
                }
                Some(Command::Ready) | None => break,
            }
        }
        sent
    }

    fn failed(&self) -> Option<String> {
        self.d.take_events().into_iter().find_map(|e| match e {
            Event::Failed(why) => Some(why),
            _ => None,
        })
    }
}

#[test]
fn a_fresh_download_fetches_every_block_verifies_caches_and_mounts() {
    let r = rig(caps(true));
    let (offer, content) = offered("props", "1.0", &files(1), true);
    let total = offer.bytes();
    let sent = r.join(std::slice::from_ref(&offer), &[content]);
    assert_eq!(r.d.phase(), Phase::Done);
    assert_eq!(sent, total, "all of it was sent");
    assert_eq!(r.d.progress(), (total, total, 1, 1));
    assert!(r.table.is_server_file(b"art/big.bin"));
    assert!(r.table.is_server_file(b"sounds/boom.ogg"));
    assert!(r.d.inner.borrow().cache.has(&offer.hash));
    // The ready word follows once, then nothing.
    assert_eq!(r.d.next_command(), None);
    r.d.closed();
    assert!(
        !r.table.provides("art/big.bin"),
        "the mounts go with the connection"
    );
    assert!(r.d.inner.borrow().cache.has(&offer.hash));
}

#[test]
fn a_mod_that_was_cached_needs_nothing_on_the_next_join() {
    let r = rig(caps(true));
    let (offer, content) = offered("props", "1.0", &files(1), true);
    r.join(std::slice::from_ref(&offer), std::slice::from_ref(&content));
    r.d.closed();
    let sent = r.join(&[offer], &[content]);
    assert_eq!(sent, 0);
    assert_eq!(r.d.phase(), Phase::Done);
    assert_eq!(r.d.progress(), (0, 0, 0, 0));
    assert!(
        r.table.is_server_file(b"art/big.bin"),
        "mounted from the cache"
    );
}

#[test]
fn a_mod_updated_in_one_block_transfers_only_that_block() {
    let r = rig(caps(true));
    let (v1, c1) = offered("props", "1.0", &files(1), true);
    r.join(&[v1], &[c1]);
    r.d.closed();
    // The big file changes in its middle block only; every other file stays.
    let mut updated = files(1);
    for b in &mut updated[0].1[BLOCK_BYTES + 5..BLOCK_BYTES + 9] {
        *b ^= 0xff;
    }
    let (v2, c2) = offered("props", "1.1", &updated, true);
    let sent = r.join(&[v2], &[c2]);
    assert_eq!(r.d.phase(), Phase::Done, "{:?}", r.failed());
    assert_eq!(
        sent, BLOCK_BYTES as u64,
        "one block of the one changed file"
    );
    assert_eq!(
        r.d.progress(),
        (BLOCK_BYTES as u64, BLOCK_BYTES as u64, 1, 1)
    );
    assert!(r.table.is_server_file(b"art/big.bin"));
}

#[test]
fn a_mod_with_a_new_file_and_a_renamed_one_transfers_only_the_new_one() {
    let r = rig(caps(true));
    let (v1, c1) = offered("props", "1.0", &files(1), true);
    r.join(&[v1], &[c1]);
    r.d.closed();
    let mut next = files(1);
    // A renamed file is matched by its hash, whatever its path; a new one is sent.
    next[2].0 = "sounds/blast.ogg".to_string();
    next.push(("art/new.bin".to_string(), pattern(2_000, 77)));
    let (v2, c2) = offered("props", "1.1", &next, true);
    let sent = r.join(&[v2], &[c2]);
    assert_eq!(sent, 2_000);
    assert_eq!(r.d.phase(), Phase::Done);
}

#[test]
fn an_installed_older_copy_is_a_base_too() {
    let r = rig(caps(true));
    let old = files(1);
    let installed: Rc<dyn LocalSource> =
        Rc::new(Installed(vec![("props".to_string(), old.clone())]));
    r.d.set_local(Rc::downgrade(&installed));
    let mut updated = old;
    updated[1].1[0] ^= 1;
    let (v2, c2) = offered("props", "1.1", &updated, true);
    let sent = r.join(&[v2], &[c2]);
    assert_eq!(sent, 1_000, "only the small file that changed");
    assert_eq!(r.d.phase(), Phase::Done);
    // The installed mod that already is the offered content needs nothing, even with downloads off.
    let r = rig(caps(false));
    let current = files(1);
    let installed: Rc<dyn LocalSource> =
        Rc::new(Installed(vec![("props".to_string(), current.clone())]));
    r.d.set_local(Rc::downgrade(&installed));
    let (offer, content) = offered("props", "1.0", &current, true);
    assert_eq!(r.join(&[offer], &[content]), 0);
    assert_eq!(r.d.phase(), Phase::Done);
    assert!(
        !r.table.provides("art/big.bin"),
        "the installed mod is its own mount"
    );
}

#[test]
fn downloads_off_or_not_served_leaves_with_the_mod_named() {
    let (offer, content) = offered("props", "1.2", &files(1), true);
    let r = rig(caps(false));
    r.join(std::slice::from_ref(&offer), std::slice::from_ref(&content));
    assert_eq!(r.d.phase(), Phase::Failed);
    let why = r.failed().unwrap();
    assert!(
        why.contains("props 1.2") && why.contains("KiB") && why.contains("turned off"),
        "{why}"
    );
    assert!(!r.table.provides("art/big.bin"));
    let (unserved, _) = offered("props", "1.2", &files(1), false);
    let r = rig(caps(true));
    r.join(&[unserved], &[Vec::new()]);
    let why = r.failed().unwrap();
    assert!(
        why.contains("props 1.2") && why.contains("do not have it"),
        "{why}"
    );
}

#[test]
fn the_caps_stop_a_download_with_the_reason() {
    let (offer, content) = offered("props", "1.0", &files(1), true);
    let limits = DownloadLimits {
        total_bytes: 1_000,
        ..DownloadLimits::default()
    };
    let r = rig(Caps {
        downloads: true,
        limits,
    });
    r.join(std::slice::from_ref(&offer), std::slice::from_ref(&content));
    let why = r.failed().unwrap();
    assert!(
        why.contains("props") && why.contains("download limit"),
        "{why}"
    );
    assert!(!r.table.provides("art/big.bin"));
    let limits = DownloadLimits {
        file_bytes: 1_000,
        ..DownloadLimits::default()
    };
    let r = rig(Caps {
        downloads: true,
        limits,
    });
    r.join(std::slice::from_ref(&offer), std::slice::from_ref(&content));
    assert!(r.failed().unwrap().contains("over your download limits"));
    let limits = DownloadLimits {
        files: 2,
        ..DownloadLimits::default()
    };
    let r = rig(Caps {
        downloads: true,
        limits,
    });
    r.join(&[offer], &[content]);
    assert_eq!(r.d.phase(), Phase::Failed);
}

#[test]
fn a_block_that_does_not_match_its_hash_is_refused_and_nothing_is_kept() {
    let r = rig(caps(true));
    let (offer, content) = offered("props", "1.0", &files(1), true);
    r.d.receive(&encode_offer(std::slice::from_ref(&offer)));
    let Some(Command::Get(request)) = r.d.next_command() else {
        panic!("a request");
    };
    let file = &content[request.file as usize].1;
    let (from, to) = run_bytes(
        file.len() as u64,
        u64::from(request.start),
        u64::from(request.count),
    );
    let mut data = file[from as usize..to as usize].to_vec();
    data[0] ^= 1;
    r.d.receive(&encode_data(&request, &data));
    assert_eq!(r.d.phase(), Phase::Failed);
    assert!(r.failed().unwrap().contains("did not match"));
    assert!(!r.table.provides("art/big.bin"));
    assert_eq!(r.d.inner.borrow().cache.bytes(), 0, "nothing was cached");
}

#[test]
fn data_nobody_asked_for_or_of_the_wrong_size_ends_the_download() {
    let (offer, content) = offered("props", "1.0", &files(1), true);
    // Data with no request outstanding, and data for another run than the one asked.
    let r = rig(caps(true));
    r.d.receive(&encode_offer(std::slice::from_ref(&offer)));
    let stray = Request {
        mod_index: 0,
        file: 1,
        start: 0,
        count: 1,
    };
    r.d.receive(&encode_data(&stray, &content[1].1));
    assert_eq!(r.d.phase(), Phase::Failed);
    let r = rig(caps(true));
    r.d.receive(&encode_offer(std::slice::from_ref(&offer)));
    let Some(Command::Get(request)) = r.d.next_command() else {
        panic!("a request");
    };
    let file = &content[request.file as usize].1;
    let (from, _) = run_bytes(file.len() as u64, u64::from(request.start), 1);
    r.d.receive(&encode_data(
        &request,
        &file[from as usize..from as usize + 10],
    ));
    assert_eq!(r.d.phase(), Phase::Failed);
    assert!(r.failed().unwrap().contains("wrong size"));
    // A second offer, or an offer after the start, is out of turn.
    let r = rig(caps(true));
    r.d.receive(&encode_offer(std::slice::from_ref(&offer)));
    r.d.receive(&encode_offer(&[offer]));
    assert_eq!(r.d.phase(), Phase::Failed);
}

#[test]
fn a_hostile_servers_offer_that_lies_about_a_hash_is_caught_before_anything_is_mounted() {
    let r = rig(caps(true));
    let (mut offer, content) = offered("props", "1.0", &files(1), true);
    // The file hash does not belong to the blocks the offer lists.
    offer.files[1].hash = [9; 32];
    r.join(&[offer], &[content]);
    assert_eq!(r.d.phase(), Phase::Failed);
    assert!(r.failed().unwrap().contains("does not match its hash"));
    assert!(!r.table.provides("art/big.bin"));
    assert_eq!(r.d.inner.borrow().cache.bytes(), 0);
    // A package hash that does not belong to the files.
    let r = rig(caps(true));
    let (mut offer, content) = offered("props", "1.0", &files(1), true);
    offer.hash = [7; 32];
    r.join(&[offer], &[content]);
    assert_eq!(r.d.phase(), Phase::Failed);
    assert!(r.failed().unwrap().contains("announced"));
    assert!(!r.table.provides("art/big.bin"));
}

#[test]
fn a_script_in_an_offer_is_refused() {
    let r = rig(caps(true));
    let mut with = files(1);
    with.push(("lib/x.cs".to_string(), b"echo(1);".to_vec()));
    let (offer, _) = offered("props", "1.0", &with, true);
    r.d.receive(&encode_offer(&[offer]));
    assert_eq!(r.d.phase(), Phase::Failed);
    assert!(r.failed().unwrap().contains("script"));
}

#[test]
fn the_connect_section_says_nothing_of_what_the_client_has() {
    let r = rig(caps(true));
    let section = r.d.begin_connect(caps(true));
    assert_eq!(section.max_bytes, DownloadLimits::default().total_bytes);
    let off = r.d.begin_connect(caps(false));
    assert_eq!((off.max_bytes, off.max_files), (0, 0));
    // Whatever is cached or installed, the bytes are the same few.
    let (offer, content) = offered("props", "1.0", &files(1), true);
    r.join(&[offer], &[content]);
    assert_eq!(r.d.begin_connect(caps(true)).encode().len(), 13);
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

#[test]
fn the_receiver_survives_hostile_input() {
    let mut rng = Rng(0x7a11_c0de_0bad_f00d);
    let (offer, content) = offered("props", "1.0", &files(1), true);
    let good_offer = encode_offer(&[offer]);
    let good_data = encode_data(
        &Request {
            mod_index: 0,
            file: 1,
            start: 0,
            count: 1,
        },
        &content[1].1,
    );
    let cached = encode_content("props", "1.0", &[1; 32], &files(1));
    for case in 0..20_000 {
        let r = rig(caps(true));
        // A run of messages, some mutated, some cut, in any order; the receiver also hands out commands as it goes.
        for _ in 0..1 + rng.next() % 5 {
            let mut bytes = match rng.next() % 4 {
                0 => good_offer.clone(),
                1 => good_data.clone(),
                2 => cached.clone(),
                _ => (0..(rng.next() % 300) as usize)
                    .map(|_| rng.next() as u8)
                    .collect(),
            };
            match rng.next() % 4 {
                0 if !bytes.is_empty() => {
                    let at = (rng.next() as usize) % bytes.len();
                    bytes[at] = rng.next() as u8;
                }
                1 => bytes.truncate((rng.next() as usize) % (bytes.len() + 1)),
                _ => {}
            }
            r.d.receive(&bytes);
            let _ = r.d.next_command();
        }
        let _ = r.d.take_events();
        assert!(!r.table.provides("server.cs"), "{case}");
        r.d.closed();
        assert!(!r.table.provides("art/big.bin"), "{case}");
    }
}

#[test]
fn the_cap_admits_up_to_the_maximum_and_a_held_client_may_say_only_two_things() {
    let limits = DownloadLimits {
        max_clients: 3,
        ..DownloadLimits::default()
    };
    let server = ModsContentServer {
        limits: limits.content(),
    };
    let vm = Vm::new();
    assert!(server.admit(&vm, 0));
    assert!(server.admit(&vm, 2), "one below the maximum");
    assert!(!server.admit(&vm, 3), "at the maximum");
    assert!(!server.admit(&vm, 9));
    assert!(
        !server.holds(&vm),
        "no mods on this script engine: nothing is held"
    );
    assert!(server.allows_command(b"serverCmdTorqueModsGet"));
    assert!(server.allows_command(b"servercmdtorquemodsready"));
    for name in [
        &b"serverCmdHostile"[..],
        b"serverCmdTorqueModsGetX",
        b"onConnect",
        b"",
    ] {
        assert!(!server.allows_command(name), "{name:?}");
    }
    let zero = DownloadLimits {
        seconds: 0,
        idle_seconds: 0,
        max_clients: 0,
        ..DownloadLimits::default()
    }
    .content();
    assert_eq!(
        (zero.total(), zero.idle(), zero.max_clients),
        (300_000, 20_000, 1)
    );
}

fn server_at(port: u16) -> std::net::SocketAddr {
    std::net::SocketAddr::from(([203, 0, 113, 7], port))
}

impl Rig {
    /// Starts a connection to the server at `port`, with other servers' cache entries counting or not.
    fn connect_to(&self, port: u16, share: bool) {
        self.d.begin_connect_to(
            caps(true),
            CacheScope {
                owner: Some(CacheOwner::of_server(server_at(port))),
                share,
            },
        );
    }
}

#[test]
fn another_servers_offer_of_a_cached_mod_is_a_full_download() {
    let r = rig(caps(true));
    let (offer, content) = offered("props", "1.0", &files(1), true);
    r.connect_to(1, false);
    r.join(std::slice::from_ref(&offer), std::slice::from_ref(&content));
    r.d.closed();
    // The same content offered by a second server: nothing is skipped, so the second server learns nothing of the first.
    r.connect_to(2, false);
    let sent = r.join(std::slice::from_ref(&offer), std::slice::from_ref(&content));
    assert_eq!(sent, offer.bytes(), "no block was skipped");
    assert_eq!(r.d.phase(), Phase::Done);
    r.d.closed();
    // An updated version is not built from the first server's copy either.
    let mut updated = files(1);
    updated[1].1[0] ^= 1;
    let (v2, c2) = offered("props", "1.1", &updated, true);
    r.connect_to(3, false);
    let sent = r.join(std::slice::from_ref(&v2), &[c2]);
    assert_eq!(
        sent,
        v2.bytes(),
        "no older copy of another server was a base"
    );
}

#[test]
fn a_server_reuses_what_it_sent_before() {
    let r = rig(caps(true));
    let (offer, content) = offered("props", "1.0", &files(1), true);
    r.connect_to(1, false);
    r.join(std::slice::from_ref(&offer), std::slice::from_ref(&content));
    r.d.closed();
    r.connect_to(1, false);
    assert_eq!(
        r.join(std::slice::from_ref(&offer), std::slice::from_ref(&content)),
        0
    );
    assert_eq!(r.d.phase(), Phase::Done);
    r.d.closed();
    let mut updated = files(1);
    for b in &mut updated[0].1[BLOCK_BYTES + 5..BLOCK_BYTES + 9] {
        *b ^= 0xff;
    }
    let (v2, c2) = offered("props", "1.1", &updated, true);
    r.connect_to(1, false);
    assert_eq!(
        r.join(&[v2], &[c2]),
        BLOCK_BYTES as u64,
        "its own copy is a base"
    );
}

#[test]
fn sharing_the_cache_lets_every_server_match_every_entry() {
    let r = rig(caps(true));
    let (offer, content) = offered("props", "1.0", &files(1), true);
    r.connect_to(1, false);
    r.join(std::slice::from_ref(&offer), std::slice::from_ref(&content));
    r.d.closed();
    r.connect_to(2, true);
    assert_eq!(
        r.join(std::slice::from_ref(&offer), std::slice::from_ref(&content)),
        0
    );
    assert_eq!(r.d.phase(), Phase::Done);
    r.d.closed();
    let mut updated = files(1);
    updated[1].1[0] ^= 1;
    let (v2, c2) = offered("props", "1.1", &updated, true);
    r.connect_to(3, true);
    assert_eq!(r.join(&[v2], &[c2]), 1_000, "a base from another server");
}

#[test]
fn entries_with_no_server_are_another_servers() {
    let r = rig(caps(true));
    let (offer, content) = offered("props", "1.0", &files(1), true);
    // A cache with no server named stores entries with none, as an older version did.
    r.join(std::slice::from_ref(&offer), std::slice::from_ref(&content));
    r.d.closed();
    r.connect_to(1, false);
    let sent = r.join(std::slice::from_ref(&offer), std::slice::from_ref(&content));
    assert_eq!(sent, offer.bytes());
    r.d.closed();
    r.connect_to(2, true);
    let sent = r.join(&[offer], &[content]);
    assert_eq!(sent, 0, "sharing still reads them");
}
