use super::*;
use crate::cache::CacheOwner;
use crate::content::encode_content;
use crate::offer::{FileOffer, encode_data, encode_offer};
use torque_testkit::Rng;

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
        name: id.to_string(),
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

    fn bytes_of(&self, id: &str) -> Option<u64> {
        self.0
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, f)| f.iter().map(|(_, d)| d.len() as u64).sum())
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
        self.serve(content)
    }

    /// Hands the connected client the offer `mods` again (a late offer) and serves what it asks for; the bytes sent.
    fn late(&self, mods: &[ModOffer], content: &[Files]) -> u64 {
        self.d.receive(&encode_offer(mods));
        self.serve(content)
    }

    /// Serves the runs the client asks for from `content` until it says ready or stops asking; the bytes sent.
    fn serve(&self, content: &[Files]) -> u64 {
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

#[test]
fn the_receiver_survives_hostile_input() {
    let mut rng = Rng::new(0x7a11_c0de_0bad_f00d);
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
        for _ in 0..1 + rng.next_u64() % 5 {
            let mut bytes = match rng.next_u64() % 4 {
                0 => good_offer.clone(),
                1 => good_data.clone(),
                2 => cached.clone(),
                _ => (0..(rng.next_u64() % 300) as usize)
                    .map(|_| rng.next_u64() as u8)
                    .collect(),
            };
            match rng.next_u64() % 4 {
                0 if !bytes.is_empty() => {
                    let at = (rng.next_u64() as usize) % bytes.len();
                    bytes[at] = rng.next_u64() as u8;
                }
                1 => bytes.truncate((rng.next_u64() as usize) % (bytes.len() + 1)),
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

/// A store that counts its reads.
struct Counting {
    inner: crate::cache::MemoryStore,
    reads: Rc<std::cell::Cell<usize>>,
    heads: Rc<std::cell::Cell<usize>>,
}

impl crate::cache::CacheStore for Counting {
    fn list(&self) -> Vec<crate::cache::Listed> {
        self.inner.list()
    }

    fn read(&self, name: &str, max: u64) -> Option<Vec<u8>> {
        self.reads.set(self.reads.get() + 1);
        self.inner.read(name, max)
    }

    fn read_head(&self, name: &str, n: usize) -> Option<Vec<u8>> {
        self.heads.set(self.heads.get() + 1);
        self.inner.read_head(name, n)
    }

    fn write(&mut self, name: &str, bytes: &[u8]) -> bool {
        self.inner.write(name, bytes)
    }

    fn remove(&mut self, name: &str) {
        self.inner.remove(name);
    }

    fn touch(&mut self, name: &str, stamp: u64) {
        self.inner.touch(name, stamp);
    }
}

#[test]
fn an_offer_loads_each_ids_older_copies_once_and_within_a_budget() {
    let reads = Rc::new(std::cell::Cell::new(0));
    let heads = Rc::new(std::cell::Cell::new(0));
    let store = Counting {
        inner: crate::cache::MemoryStore::default(),
        reads: Rc::clone(&reads),
        heads: Rc::clone(&heads),
    };
    let table = Rc::new(Table::default());
    let d = Downloads::new(
        Rc::clone(&table),
        ModLimits::default(),
        caps(true),
        ContentCache::new(Box::new(store), 1 << 24),
    );
    let r = Rig { d, table };
    // Two older copies of `props` in the cache.
    for seed in [1, 2] {
        let (offer, content) = offered("props", "1.0", &files(seed), true);
        r.join(&[offer], &[content]);
        r.d.closed();
    }
    // A 32-item offer: a new `props` and 31 others. The cache's heads are read once, and `props`'s copies at most twice.
    let mut mods = vec![offered("props", "1.1", &files(3), true).0];
    for n in 0..31 {
        mods.push(offered(&format!("other{n}"), "1.0", &files(4 + n), true).0);
    }
    let (reads_before, heads_before) = (reads.get(), heads.get());
    r.d.reset();
    r.d.receive(&encode_offer(&mods));
    assert!(
        reads.get() - reads_before <= 2,
        "{} loads",
        reads.get() - reads_before
    );
    assert!(heads.get() - heads_before <= 2, "the heads were read once");
    // An offer of a tiny new `props` builds from nothing: its copies are past twice what it would download.
    let tiny = vec![("art/one.bin".to_string(), vec![7u8; 10])];
    let reads_before = reads.get();
    r.d.reset();
    r.d.receive(&encode_offer(&[offered("props", "2.0", &tiny, true).0]));
    assert_eq!(reads.get(), reads_before, "no older copy was read");
}

/// An installed mod that says it is huge and counts the reads of its files.
struct Huge(std::cell::Cell<usize>);

impl LocalSource for Huge {
    fn hash_of(&self, _id: &str) -> Option<Hash> {
        Some([9; 32])
    }

    fn bytes_of(&self, _id: &str) -> Option<u64> {
        Some(1 << 40)
    }

    fn files_of(&self, _id: &str) -> Option<Files> {
        self.0.set(self.0.get() + 1);
        Some(files(1))
    }
}

#[test]
fn an_installed_copy_past_the_base_budget_is_never_read() {
    let r = rig(caps(true));
    let huge = Rc::new(Huge(std::cell::Cell::new(0)));
    let local: Rc<dyn LocalSource> = huge.clone();
    r.d.set_local(Rc::downgrade(&local));
    let (offer, content) = offered("props", "2.0", &files(2), true);
    r.join(&[offer], &[content]);
    assert_eq!(r.d.phase(), Phase::Done);
    assert_eq!(huge.0.get(), 0, "its size was checked first");
    const { assert!(MAX_BASE_BYTES <= 1 << 30) };
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

impl Rig {
    /// Like [`Rig::join`], counting the round trips: a round is every request the client has ready to send, then the server's
    /// answers to all of them, so a client that waits for each answer before asking for the next takes one round a request.
    fn join_in_rounds(&self, mods: &[ModOffer], content: &[Files]) -> usize {
        self.d.reset();
        self.d.receive(&encode_offer(mods));
        let mut rounds = 0;
        for _ in 0..10_000 {
            let mut asked = Vec::new();
            while let Some(Command::Get(r)) = self.d.next_command() {
                asked.push(r);
            }
            if asked.is_empty() {
                break;
            }
            rounds += 1;
            for r in asked {
                let file = &content[r.mod_index as usize][r.file as usize].1;
                let (from, to) =
                    run_bytes(file.len() as u64, u64::from(r.start), u64::from(r.count));
                self.d
                    .receive(&encode_data(&r, &file[from as usize..to as usize]));
            }
        }
        rounds
    }
}

#[test]
fn a_download_takes_a_fraction_of_the_round_trips_a_request_at_a_time_would() {
    let big = vec![
        ("art/a.bin".to_string(), pattern(7 << 20, 1)),
        ("art/b.bin".to_string(), pattern(7 << 20, 2)),
    ];
    let (offer, content) = offered("big", "1.0", &big, true);
    let runs = offer
        .bytes()
        .div_ceil((BLOCK_BYTES * MAX_REQUEST_BLOCKS as usize) as u64) as usize;
    let r = rig(caps(true));
    let rounds = r.join_in_rounds(std::slice::from_ref(&offer), &[content]);
    println!("mod download: {runs} runs in {rounds} round trips");
    assert_eq!(r.d.phase(), Phase::Done);
    assert!(
        rounds <= runs.div_ceil(DOWNLOAD_WINDOW),
        "{runs} runs took {rounds} rounds"
    );
}

#[test]
fn a_mod_of_many_small_files_asks_for_them_together() {
    let small: Files = (0..40u8)
        .map(|i| (format!("art/{i:02}.png"), pattern(2_500, i)))
        .collect();
    let (offer, content) = offered("small", "1.0", &small, true);
    let r = rig(caps(true));
    let rounds = r.join_in_rounds(std::slice::from_ref(&offer), &[content]);
    assert_eq!(r.d.phase(), Phase::Done);
    assert_eq!(
        rounds,
        small.len().div_ceil(MAX_PENDING_RUNS),
        "{rounds} rounds"
    );
}

/// A mod with a client script among its files.
fn scripted(seed: u8) -> Files {
    let mut files = files(seed);
    files.push((
        "client/main.lua".to_string(),
        b"realm.log('hello')".to_vec(),
    ));
    files
}

fn client_script_reader(r: &Rig, port: u16, hash: &Hash, mod_id: &str) -> Option<Vec<String>> {
    r.table
        .client_scripts(mod_id, CacheOwner::of_server(server_at(port)), hash)
        .map(|files| files.into_iter().map(|(path, _)| path).collect())
}

#[test]
fn client_script_content_is_read_only_for_the_server_that_sent_it() {
    let r = rig(caps(true));
    let (offer, content) = offered("props", "1.0", &scripted(1), true);
    r.connect_to(7000, false);
    r.join(std::slice::from_ref(&offer), &[content]);
    assert_eq!(r.d.phase(), Phase::Done);
    let paths = client_script_reader(&r, 7000, &offer.hash, "props").expect("the sender's own");
    assert_eq!(
        paths,
        ["mods/props/client/main.lua"],
        "only the scripts are read"
    );
    // Another server's key, another hash, another id: nothing.
    assert!(client_script_reader(&r, 7001, &offer.hash, "props").is_none());
    assert!(client_script_reader(&r, 7000, &[1; 32], "props").is_none());
    assert!(client_script_reader(&r, 7000, &offer.hash, "other").is_none());
    for bad in ["", "..", "a/b", "props\0", "pro ps"] {
        assert!(
            client_script_reader(&r, 7000, &offer.hash, bad).is_none(),
            "{bad:?}"
        );
    }
    let owner = CacheOwner::of_server(server_at(7000));
    let script = |path: &str| r.table.client_script("props", owner, &offer.hash, path);
    assert_eq!(
        &*script("client/main.lua").expect("the entry"),
        b"realm.log('hello')"
    );
    assert!(script("art/big.bin").is_none(), "only Lua files");
    assert!(script("client/missing.lua").is_none());
}

#[test]
fn client_script_content_reads_only_scripts_and_images_one_at_a_time_within_their_caps() {
    let r = rig(caps(true));
    let files: Files = vec![
        ("client/main.lua".to_string(), b"realm.log('hi')".to_vec()),
        ("sounds/theme.ogg".to_string(), vec![0u8; 50 << 20]),
        ("art/logo.png".to_string(), pattern(2_000, 3)),
        (
            "art/huge.png".to_string(),
            vec![0u8; crate::files::MAX_CLIENT_IMAGE_BYTES as usize + 1],
        ),
    ];
    let (offer, content) = offered("big", "1.0", &files, true);
    let limits = ModLimits {
        file_bytes: 64 << 20,
        ..ModLimits::default()
    };
    let owner = CacheOwner::of_server(server_at(7000));
    mount_content(
        &r.table,
        &limits,
        &mount_name("big"),
        content,
        Some(MountOrigin {
            server: owner,
            hash: offer.hash,
            way: Way::Download,
        }),
    )
    .expect("mounts");
    let scripts = r
        .table
        .client_scripts("big", owner, &offer.hash)
        .expect("the sender's own");
    let paths: Vec<&str> = scripts.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(paths, ["mods/big/client/main.lua"], "the sound is not read");
    let image = |path: &str| r.table.client_image("big", owner, &offer.hash, path);
    assert_eq!(image("art/logo.png").expect("an image").len(), 2_000);
    assert!(image("art/huge.png").is_none(), "past the cap");
    assert!(image("sounds/theme.ogg").is_none(), "not an image");
    assert!(image("client/main.lua").is_none(), "not an image");
    assert!(
        r.table
            .client_image(
                "big",
                CacheOwner::of_server(server_at(7001)),
                &offer.hash,
                "art/logo.png"
            )
            .is_none(),
        "another server's"
    );
}

#[test]
fn client_script_ready_looks_without_reading_and_follows_the_same_rules() {
    let r = rig(caps(true));
    let (offer, content) = offered("props", "1.0", &scripted(1), true);
    r.connect_to(7000, false);
    r.join(std::slice::from_ref(&offer), &[content]);
    let owner = CacheOwner::of_server(server_at(7000));
    let ready =
        |server, hash: &Hash, entry: &str| r.table.client_ready("props", server, hash, entry);
    assert!(ready(owner, &offer.hash, "client/main.lua"));
    assert!(
        ready(owner, &offer.hash, "CLIENT/main.lua"),
        "paths fold case as the package does"
    );
    assert!(
        !ready(owner, &offer.hash, "client/missing.lua"),
        "no entry, not ready"
    );
    assert!(!ready(
        CacheOwner::of_server(server_at(7001)),
        &offer.hash,
        "client/main.lua"
    ));
    assert!(!ready(owner, &[3; 32], "client/main.lua"));
}

#[test]
fn client_script_content_from_a_mount_with_no_origin_is_never_read() {
    let r = rig(caps(true));
    let (offer, content) = offered("props", "1.0", &scripted(1), true);
    // Files mounted by the public API, and a mount whose cache scope has no server, have no origin.
    mount_content(
        &r.table,
        &ModLimits::default(),
        &mount_name("props"),
        content.clone(),
        None,
    )
    .expect("mounts");
    assert!(client_script_reader(&r, 7000, &offer.hash, "props").is_none());
    r.d.reset();
    r.d.receive(&encode_offer(std::slice::from_ref(&offer)));
    r.join(std::slice::from_ref(&offer), &[content]);
    assert!(
        client_script_reader(&r, 7000, &offer.hash, "props").is_none(),
        "a connection that names no server has no owner to read for"
    );
}

#[test]
fn client_script_content_found_through_cache_sharing_is_refused() {
    let r = rig(caps(true));
    let (offer, content) = offered("props", "1.0", &scripted(1), true);
    // Server A sent it and the cache keeps it under A's name.
    r.connect_to(7000, false);
    r.join(std::slice::from_ref(&offer), std::slice::from_ref(&content));
    r.d.closed();
    // Server B offers the same content. Without sharing it is downloaded again from B (and is B's own); with sharing the
    // cache's entry of A satisfies it, and B may not run scripts from it.
    r.connect_to(7001, true);
    let sent = r.join(std::slice::from_ref(&offer), std::slice::from_ref(&content));
    assert_eq!(sent, 0, "A's entry was used");
    assert!(
        client_script_reader(&r, 7001, &offer.hash, "props").is_none(),
        "content that came from another server's partition"
    );
    assert!(
        client_script_reader(&r, 7000, &offer.hash, "props").is_none(),
        "and A is not on this connection"
    );
    r.d.closed();
    // Without sharing, B downloads its own copy and the reader gives it.
    r.connect_to(7001, false);
    let sent = r.join(std::slice::from_ref(&offer), std::slice::from_ref(&content));
    assert!(sent > 0, "B's partition has nothing");
    assert!(client_script_reader(&r, 7001, &offer.hash, "props").is_some());
}

#[test]
fn client_script_content_from_the_servers_own_cache_entry_is_read() {
    let r = rig(caps(true));
    let (offer, content) = offered("props", "1.0", &scripted(1), true);
    r.connect_to(7000, false);
    r.join(std::slice::from_ref(&offer), std::slice::from_ref(&content));
    r.d.closed();
    r.connect_to(7000, false);
    let sent = r.join(std::slice::from_ref(&offer), &[content]);
    assert_eq!(sent, 0, "the cache served it");
    assert!(client_script_reader(&r, 7000, &offer.hash, "props").is_some());
}

#[test]
fn client_script_content_is_gone_when_another_server_replaces_the_mount() {
    let r = rig(caps(true));
    let (offer_a, content_a) = offered("props", "1.0", &scripted(1), true);
    let (offer_b, content_b) = offered("props", "1.0", &scripted(2), true);
    r.connect_to(7000, false);
    r.join(std::slice::from_ref(&offer_a), &[content_a]);
    assert!(client_script_reader(&r, 7000, &offer_a.hash, "props").is_some());
    // The same mod id mounted by another server's connection takes the mount's place.
    r.connect_to(7001, false);
    r.join(std::slice::from_ref(&offer_b), &[content_b]);
    assert!(client_script_reader(&r, 7000, &offer_a.hash, "props").is_none());
    assert!(client_script_reader(&r, 7001, &offer_b.hash, "props").is_some());
}

#[test]
fn client_script_content_with_more_files_than_the_realm_takes_is_refused() {
    let r = rig(caps(true));
    let many: Files = (0..=crate::files::MAX_CLIENT_FILES)
        .map(|n| (format!("f/{n}.txt"), vec![1u8]))
        .collect();
    let (offer, content) = offered("big", "1.0", &many, true);
    let limits = ModLimits {
        files: many.len() + 8,
        ..ModLimits::default()
    };
    let owner = CacheOwner::of_server(server_at(7000));
    mount_content(
        &r.table,
        &limits,
        &mount_name("big"),
        content,
        Some(MountOrigin {
            server: owner,
            hash: offer.hash,
            way: Way::Download,
        }),
    )
    .expect("mounts");
    assert!(r.table.client_scripts("big", owner, &offer.hash).is_none());
}

// ---- late offers

#[test]
fn a_late_offer_fetches_only_what_is_new_and_the_ready_word_goes_again() {
    let r = rig(caps(true));
    let (v1, c1) = offered("props", "1.0", &files(1), true);
    r.join(std::slice::from_ref(&v1), std::slice::from_ref(&c1));
    assert_eq!(r.d.next_command(), None, "ready went once");
    r.d.take_events();
    let mut next = files(1);
    next.push(("art/new.bin".to_string(), pattern(2_000, 77)));
    let (v2, c2) = offered("props", "1.1", &next, true);
    let sent = r.late(&[v2], &[c2]);
    assert_eq!(sent, 2_000, "the new file alone");
    assert_eq!(r.d.phase(), Phase::Done, "{:?}", r.failed());
    assert!(r.table.is_server_file(b"art/new.bin"));
    assert!(r.table.is_server_file(b"art/big.bin"));
    assert_eq!(r.d.progress(), (2_000, 2_000, 1, 1));
    let events = r.d.take_events();
    assert!(events.contains(&Event::Started {
        mods: 1,
        bytes: 2_000
    }));
    assert!(events.contains(&Event::Done));
}

#[test]
fn a_repeated_offer_costs_nothing_and_is_not_answered_again() {
    let r = rig(caps(true));
    let (v1, c1) = offered("props", "1.0", &files(1), true);
    r.join(std::slice::from_ref(&v1), &[c1]);
    r.d.take_events();
    let before = r.d.progress();
    let message = encode_offer(std::slice::from_ref(&v1));
    for _ in 0..1_000 {
        r.d.receive(&message);
    }
    assert_eq!(r.d.phase(), Phase::Done);
    assert_eq!(r.d.next_command(), None, "no ready word, no request");
    assert!(r.d.take_events().is_empty(), "scripts hear nothing");
    assert_eq!(r.d.progress(), before);
    assert_eq!(
        r.d.inner.borrow().late_offers,
        0,
        "a repeat is not an offer"
    );
}

#[test]
fn a_late_offer_of_what_the_connection_has_reads_nothing_and_says_ready() {
    let r = rig(caps(true));
    let (v1, c1) = offered("props", "1.0", &files(1), true);
    r.join(std::slice::from_ref(&v1), &[c1]);
    r.d.take_events();
    // The same content under another version: the message differs, the hash does not.
    let (same, _) = offered("props", "1.0.1", &files(1), true);
    let sent = r.late(&[same], &[]);
    assert_eq!(sent, 0);
    assert_eq!(r.d.phase(), Phase::Done);
    assert!(
        r.d.take_events().is_empty(),
        "nothing to fetch: scripts hear nothing"
    );
    assert_eq!(r.d.progress(), (0, 0, 0, 0));
    assert_eq!(r.d.next_command(), None, "the ready word went in `late`");
    assert!(r.table.is_server_file(b"art/big.bin"), "the mount stays");
}

#[test]
fn late_offers_are_held_to_what_the_connections_caps_have_left() {
    let mut caps = caps(true);
    caps.limits.total_bytes = 200_000;
    let r = rig(caps);
    let first: Files = vec![("art/a.bin".to_string(), pattern(150_000, 1))];
    let (v1, c1) = offered("props", "1.0", &first, true);
    assert_eq!(r.join(&[v1], &[c1]), 150_000);
    let mut second = first;
    second.push(("art/b.bin".to_string(), pattern(100_000, 2)));
    let (v2, c2) = offered("props", "1.1", &second, true);
    let sent = r.late(&[v2], &[c2]);
    assert_eq!(sent, 0, "refused before anything is asked for");
    assert_eq!(r.d.phase(), Phase::Failed);
    let why = r.failed().expect("a reason");
    assert!(
        why.contains("needs 97.7 KiB of your 195.3 KiB download limit"),
        "{why}"
    );
    assert!(!r.table.provides("art/b.bin"));
    assert!(
        !r.table.provides("art/a.bin"),
        "a failed connection keeps no mount"
    );
}

#[test]
fn a_connection_takes_a_bounded_number_of_late_offers() {
    let r = rig(caps(true));
    let (v1, c1) = offered("props", "1.0", &files(1), true);
    r.join(&[v1], &[c1]);
    r.d.take_events();
    // Each offer differs (another version), none asks for anything, and scripts hear of none of them.
    for n in 0..MAX_LATE_OFFERS {
        let (same, _) = offered("props", &format!("2.{n}"), &files(1), true);
        r.late(&[same], &[]);
        assert_eq!(r.d.phase(), Phase::Done, "offer {n}: {:?}", r.failed());
    }
    assert!(r.d.take_events().is_empty());
    let (one_more, _) = offered("props", "3.0", &files(1), true);
    r.late(&[one_more], &[]);
    assert_eq!(r.d.phase(), Phase::Failed);
    assert!(
        r.failed()
            .is_some_and(|why| why.contains("changed its mods too many times"))
    );
}

#[test]
fn a_script_in_a_late_offer_is_refused_as_at_the_accept() {
    let r = rig(caps(true));
    let (v1, c1) = offered("props", "1.0", &files(1), true);
    r.join(&[v1], &[c1]);
    let mut next = files(1);
    next.push(("scripts/evil.cs".to_string(), b"$pwned = 1;".to_vec()));
    let (v2, c2) = offered("props", "1.1", &next, true);
    assert_eq!(r.late(&[v2], &[c2]), 0);
    assert_eq!(r.d.phase(), Phase::Failed);
    assert!(
        r.failed()
            .is_some_and(|why| why.contains("a server sends content, never scripts"))
    );
    assert!(!r.table.provides("scripts/evil.cs"));
}

#[test]
fn a_late_offer_matched_by_an_installed_copy_drops_the_stale_download_mount() {
    let r = rig(caps(true));
    let (v1, c1) = offered("props", "1.0", &files(1), true);
    r.join(&[v1], &[c1]);
    assert!(r.table.is_server_file(b"art/big.bin"));
    // The player installs the mod's next version; the server moves to it too.
    let next = files(2);
    let installed: Rc<dyn LocalSource> =
        Rc::new(Installed(vec![("props".to_string(), next.clone())]));
    r.d.set_local(Rc::downgrade(&installed));
    let (v2, _) = offered("props", "1.1", &next, true);
    assert_eq!(r.late(&[v2], &[]), 0, "the installed copy is the content");
    assert_eq!(r.d.phase(), Phase::Done);
    assert!(
        !r.table.provides("art/big.bin"),
        "the download mount of the older copy is gone"
    );
}

#[test]
fn a_run_of_late_offers_under_the_cap_stops_at_the_connections_assembled_cap() {
    let mut caps = caps(true);
    caps.limits.total_bytes = 100_000;
    let r = rig(caps);
    // The player has the mod installed; each offer lists its one file under a new path, so every offer is a new hash built
    // whole from the installed copy: 0 bytes transferred, 60,000 assembled and mounted each time.
    let bytes = pattern(60_000, 5);
    let installed: Rc<dyn LocalSource> = Rc::new(Installed(vec![(
        "props".to_string(),
        vec![("art/a0.bin".to_string(), bytes.clone())],
    )]));
    r.d.set_local(Rc::downgrade(&installed));
    let version = |n: usize| -> (ModOffer, Files) {
        offered(
            "props",
            &format!("1.{n}"),
            &vec![(format!("art/a{n}.bin"), bytes.clone())],
            true,
        )
    };
    let (v0, _) = version(0);
    assert_eq!(r.join(&[v0], &[]), 0, "the installed copy is the content");
    let mut taken = 0;
    for n in 1..20 {
        let (v, _) = version(n);
        assert_eq!(r.late(&[v], &[]), 0, "nothing is ever transferred");
        if r.d.phase() == Phase::Failed {
            break;
        }
        taken += 1;
        assert!(r.table.is_server_file(format!("art/a{n}.bin").as_bytes()));
    }
    // The first offer was the installed copy itself and assembled nothing; 60,000 a late offer against four times the
    // 100,000 cap: six fit, the seventh is refused.
    assert_eq!(taken, 6, "six late offers fit");
    assert_eq!(r.d.phase(), Phase::Failed);
    let why = r.failed().expect("a reason");
    assert!(
        why.contains("is over your download limit (97.7 KiB)"),
        "{why}"
    );
    assert!(
        !r.table.provides("art/a7.bin") && !r.table.provides("art/a6.bin"),
        "nothing past the cap is mounted, and a failed connection keeps no mount"
    );
    assert!(r.d.inner.borrow().base_bytes <= MAX_CONNECTION_BASE_BYTES);
}

#[test]
fn a_mod_a_later_offer_drops_loses_its_mount() {
    let r = rig(caps(true));
    let (props, c1) = offered("props", "1.0", &files(1), true);
    let extra_files: Files = vec![("art/extra.bin".to_string(), pattern(2_000, 8))];
    let (extra, c2) = offered("extra", "1.0", &extra_files, true);
    r.join(&[props.clone(), extra], &[c1, c2]);
    assert!(r.table.is_server_file(b"art/extra.bin"));
    // The server stops requiring `extra`: its content is no longer this server's.
    assert_eq!(r.late(&[props], &[]), 0);
    assert_eq!(r.d.phase(), Phase::Done);
    assert!(
        !r.table.provides("art/extra.bin"),
        "the dropped mod's mount is gone"
    );
    assert!(r.table.is_server_file(b"art/big.bin"), "the kept mod stays");
    assert_eq!(r.d.next_command(), None, "the ready word went in `late`");
}

#[test]
fn the_files_cap_counts_an_offers_mods_together() {
    let mut caps = caps(true);
    caps.limits.files = 10;
    let r = rig(caps);
    let six = |seed: u8| -> Files {
        (0..6)
            .map(|n| (format!("f/{seed}-{n}.bin"), pattern(100, seed + n)))
            .collect()
    };
    let (a, ca) = offered("a", "1.0", &six(1), true);
    let (b, cb) = offered("b", "1.0", &six(2), true);
    // One mod of six files fits; two of them are twelve files from one server, over the cap of ten (a second client, so the
    // first mod is not in a cache that would count its files as nothing).
    assert_eq!(
        r.join(std::slice::from_ref(&a), std::slice::from_ref(&ca)),
        600
    );
    assert_eq!(r.d.phase(), Phase::Done);
    let fresh = rig(caps);
    assert_eq!(
        fresh.join(&[a, b], &[ca, cb]),
        0,
        "refused before a request"
    );
    assert_eq!(fresh.d.phase(), Phase::Failed);
    let why = fresh.failed().expect("a reason");
    assert!(
        why.starts_with("The mod b 1.0") && why.contains("is over your download limits"),
        "{why}"
    );
}
