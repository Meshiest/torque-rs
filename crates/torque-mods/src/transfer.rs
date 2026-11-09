//! Mod content over the wire: the limits, the client's half (what reads the server's offer, picks what it lacks, checks and caches
//! and mounts it) and the server's adapter (the host's seam into the mods).
//!
//! **The flow.** A client of a game that shares mods says in its connect block only that it can take content and the most it will
//! (bytes and files); it lists none of the mods it has installed or cached (what it asks for and what it skips still shows a
//! server what it has installed, and with the cache shared across servers, what it has downloaded). The server accepts it into a
//! *held* state: its `onConnect` waits, so no datablock or ghost is sent. The server sends the **offer** on the mods channel: the mods it requires,
//! each with its package hash and, for a mod it serves, every file's path, size and hash and every block's hash ([`crate::offer`]).
//!
//! The client matches the offer against what it has: a mod it has installed, or cached from this same server (or from any, when
//! the player shares the cache; see [`crate::cache`]), with the same package hash needs nothing;
//! else a served mod is assembled from files and blocks of an older copy of it (installed or cached) that match by file hash or by
//! block hash at the same path, and the rest is requested, a few blocks at a time (`TorqueModsGet`, a remote command), from the
//! server's content. The client verifies every block, every file and the package hash, mounts the mod and then keeps it in the
//! cache, and tells the server it is ready (`TorqueModsReady`); the server then runs `onConnect`. If downloads are off, or the
//! content is over the client's caps, or the server does not serve a mod the client lacks, the client leaves with the reason,
//! naming the mod, its version and size.
//!
//! Content only: the files mount as a server's files, which are never read as script, and a script path in an offer is refused.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};

use torque_net::chunk::Kind;
use torque_net::chunk::registry::ChunkChannel;
use torque_net::handshake::ConnId;
use torque_net::modsync::{ModsSection, size_text};
use torque_server::net::channels::ChannelHub;
use torque_server::net::content::{ContentLimits, ContentServer};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use crate::cache::{CacheScope, ContentCache, default_cache_dir};
use crate::content::{ContentCaps, Hash, encode_content, summarize_files};
use crate::files::{MountOrigin, Table, Way};
use crate::mods::{Mods, mount_content};
use crate::offer::{
    BLOCK_BYTES, MAX_REQUEST_BLOCKS, ModOffer, OfferCaps, Request, block_hash, decode_data,
    decode_offer, file_hash, is_data, is_offer, run_bytes,
};
use crate::package::ModLimits;

/// The name of the bulk channel that carries mod content.
pub const CHANNEL: &str = "torque.mods";

/// The bytes of a packet the mods channel's chunks may take: nearly the whole of a burst's.
pub(crate) const BURST_BUDGET: usize = 1_100;

/// The remote commands a held client may run: the two the mods take.
pub(crate) const COMMANDS: [&str; 2] = ["serverCmdTorqueModsGet", "serverCmdTorqueModsReady"];

/// The RPC a client asks for a run of blocks with in the engine's protocol, in place of the `TorqueModsGet` command.
pub const GET_RPC: &str = "torque.mods.get";

/// A client's ask for a run of blocks (`TorqueModsGet`'s arguments) as the message of [`GET_RPC`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, torque_engine::NetMessage)]
pub struct ModsGet {
    /// The mod's place in the offer.
    pub mod_index: u32,
    /// The file's place in the mod.
    pub file: u32,
    /// The first block.
    pub start: u32,
    /// How many blocks.
    pub count: u32,
}

/// Registers [`GET_RPC`]: a notification to the server, at most 64 a second (a burst of 128), served as the command is.
pub(crate) fn register_get_rpc(engine: &mut torque_engine::EngineBuilder) {
    engine
        .net()
        .rpc::<ModsGet, ()>(GET_RPC)
        .to_server()
        .rate(64, 128)
        .max_bytes(32)
        .server(|cx, get: ModsGet| {
            let obj = cx.caller().and_then(|conn| {
                cx.vm
                    .extension::<torque_server::net::rpc::ServerRpc>()?
                    .object_of(conn)
            });
            if let Some(obj) = obj {
                let request = crate::offer::Request {
                    mod_index: get.mod_index,
                    file: get.file,
                    start: get.start,
                    count: get.count,
                };
                crate::natives::serve_get(cx.vm, obj, request);
            }
            Ok(())
        });
}

/// Whether this client asks for blocks with [`GET_RPC`] (its engine speaks the engine's protocol), and whether its link takes
/// calls yet: `Some(false)` while the connection's RPCs are not ready.
pub(crate) fn get_rpc_ready(vm: &torque_ts_vm::vm::Vm) -> Option<bool> {
    let rpc = vm.extension::<torque_server::net::rpc::ClientRpc>()?;
    rpc.table().id(GET_RPC)?;
    Some(rpc.is_ready())
}

/// How many runs of blocks a client keeps asked for at once. The server queues each answer behind the one before it and holds a
/// client to a backlog on the mods channel of the window and one more run (`mods::MAX_OUTSTANDING`), so the window leaves a run
/// of room for an acknowledgement that comes late: a run is answered once the ones before it were, whatever the link's round trip.
pub(crate) const DOWNLOAD_WINDOW: usize = 3;

/// The most bytes of one message of the channel: an offer, or the data of a request.
pub const MAX_MESSAGE: usize = 2 << 20;

/// The caps on what a server serves and a client takes. A client also holds itself to its player's prefs
/// (`$Pref::Mods::DownloadMegabytes`), whichever is smaller.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DownloadLimits {
    /// The most bytes a client takes from one server, all its mods together (32 MiB). It counts the bytes sent, not the bytes of
    /// content that matched what the client had.
    pub total_bytes: u64,
    /// The largest file of downloaded content (8 MiB).
    pub file_bytes: u64,
    /// The most files of the mods a client takes from one server (2,000).
    pub files: usize,
    /// The most bytes of one mod's content (16 MiB). A mod above it is required but not served.
    pub mod_bytes: usize,
    /// How long a client has to finish, from the accept (300 s; 0 reads as 300).
    pub seconds: u32,
    /// The most bytes the cache of downloaded content keeps (512 MiB).
    pub cache_bytes: u64,
    /// The most clients a server holds in the downloading state at once; the next is refused with `Server busy, try again.`
    /// (6).
    pub max_clients: usize,
    /// Of those, the most from one source network (an IPv4 /24, an IPv6 /64), so one party cannot hold every slot (2).
    pub max_clients_per_network: std::num::NonZeroUsize,
    /// The least rate, in bytes a second, a held client must take what it asked for at to count as making progress (1,024).
    pub min_bytes_per_s: u32,
    /// How long a client may go without asking for anything, in seconds (20; 0 reads as 20).
    pub idle_seconds: u32,
}

impl Default for DownloadLimits {
    fn default() -> DownloadLimits {
        DownloadLimits {
            total_bytes: 32 << 20,
            file_bytes: 8 << 20,
            files: 2_000,
            mod_bytes: 16 << 20,
            seconds: 300,
            cache_bytes: 512 << 20,
            max_clients: 6,
            max_clients_per_network: std::num::NonZeroUsize::new(2).expect("two"),
            min_bytes_per_s: 1_024,
            idle_seconds: 20,
        }
    }
}

impl DownloadLimits {
    /// The time a download may take, in seconds: 0 reads as the default 300.
    pub fn time_seconds(&self) -> u32 {
        if self.seconds == 0 { 300 } else { self.seconds }
    }

    /// What the host holds a downloading client to.
    pub fn content(&self) -> ContentLimits {
        ContentLimits {
            max_clients: self.max_clients.max(1),
            total_ms: self.time_seconds().saturating_mul(1_000),
            idle_ms: self.idle_seconds.saturating_mul(1_000),
            per_network_clients: self.max_clients_per_network,
            min_bytes_per_s: self.min_bytes_per_s,
        }
    }
}

/// Where a client keeps what it downloaded.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum CacheChoice {
    /// A per-user cache folder (`%LOCALAPPDATA%`, `~/Library/Caches`, `$XDG_CACHE_HOME` or `~/.cache`) when the system gives one,
    /// else memory for the session. Never the game's folder.
    #[default]
    PerUser,
    /// This folder on the disk (not on the web).
    Folder(std::path::PathBuf),
    /// Memory, for the session.
    Memory,
}

impl CacheChoice {
    pub(crate) fn open(&self, max_bytes: u64) -> ContentCache {
        #[cfg(not(target_family = "wasm"))]
        {
            let dir = match self {
                CacheChoice::PerUser => default_cache_dir(),
                CacheChoice::Folder(dir) => Some(dir.clone()),
                CacheChoice::Memory => None,
            };
            if let Some(dir) = dir {
                return ContentCache::new(Box::new(crate::cache::DiskStore::new(dir)), max_bytes);
            }
        }
        #[cfg(target_family = "wasm")]
        let _ = default_cache_dir;
        ContentCache::in_memory(max_bytes)
    }
}

/// What the plugin was told about sharing mods over the network.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct NetOptions {
    pub(crate) limits: DownloadLimits,
    pub(crate) allow_downloads: bool,
    pub(crate) cache: CacheChoice,
}

// ---- what a client does with what arrives

/// What happened that scripts hear of.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Event {
    /// The offer was read: this many mods are fetched, this many bytes in all.
    Started { mods: usize, bytes: u64 },
    /// The content of a mod is mounted after a fetch.
    Mounted { id: String },
    /// Everything the server requires is mounted.
    Done,
    /// The download ended with this reason, and the connection should too.
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Phase {
    /// No connection that needs content (or none yet).
    Idle,
    /// Accepted; the offer has not come.
    Waiting,
    /// Fetching what the offer said is missing.
    Downloading,
    /// Everything is mounted.
    Done,
    /// Ended with a reason.
    Failed,
}

/// The caps in force for one connection: the game's, held to the player's.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Caps {
    pub(crate) downloads: bool,
    pub(crate) limits: DownloadLimits,
}

/// What a client has installed, which an offer may match.
pub(crate) trait LocalSource {
    /// The content hash of the installed mod `id`.
    fn hash_of(&self, id: &str) -> Option<Hash>;
    /// The bytes of the installed mod `id`'s content, known before its files are read.
    fn bytes_of(&self, id: &str) -> Option<u64>;
    /// The content files of the installed mod `id`.
    fn files_of(&self, id: &str) -> Option<Vec<(String, Vec<u8>)>>;
}

/// The most bytes of older copies one offer may have the client read to build from, whatever it would download: twice what
/// it downloads is the budget, and this its ceiling.
pub const MAX_BASE_BYTES: u64 = 64 << 20;

/// What the client sends the server next.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Command {
    /// A run of blocks.
    Get(Request),
    /// Everything is mounted: the server may start the join.
    Ready,
}

/// A mod's files as `(path, bytes)`.
type Files = Vec<(String, Vec<u8>)>;

/// A mod being assembled.
struct Job {
    offer: ModOffer,
    /// Each file's bytes so far.
    bufs: Vec<Vec<u8>>,
    /// Each file's blocks still to come.
    missing: Vec<Vec<bool>>,
    done: bool,
}

struct Inner {
    caps: Caps,
    cache: ContentCache,
    local: Option<Weak<dyn LocalSource>>,
    phase: Phase,
    offer: Vec<ModOffer>,
    /// A job for each mod of the offer that needs fetching.
    jobs: Vec<Option<Job>>,
    todo: VecDeque<Request>,
    /// The runs asked for and not answered yet, oldest first (at most [`DOWNLOAD_WINDOW`]): the server answers them in order.
    pending: VecDeque<Request>,
    fetched_bytes: u64,
    total_bytes: u64,
    fetching: usize,
    mounted_mods: usize,
    mounted: Vec<String>,
    events: VecDeque<Event>,
    ready_sent: bool,
    /// The simulation time the wait was first seen (the client's per-tick work sets it).
    started: Option<u32>,
    /// The bytes received when scripts were last told of the progress.
    reported: Option<u64>,
}

/// A client's downloads: shared by the channel that reads the messages and the code that owns the script engine.
pub(crate) struct Downloads {
    table: Rc<Table>,
    mod_limits: ModLimits,
    inner: RefCell<Inner>,
}

pub(crate) fn mount_name(id: &str) -> String {
    format!("download:{id}")
}

fn named(offer: &ModOffer) -> String {
    format!(
        "{} {} ({})",
        offer.id,
        offer.version,
        size_text(offer.bytes())
    )
}

impl Downloads {
    pub(crate) fn new(
        table: Rc<Table>,
        mod_limits: ModLimits,
        caps: Caps,
        cache: ContentCache,
    ) -> Downloads {
        Downloads {
            table,
            mod_limits,
            inner: RefCell::new(Inner {
                caps,
                cache,
                local: None,
                phase: Phase::Idle,
                offer: Vec::new(),
                jobs: Vec::new(),
                todo: VecDeque::new(),
                pending: VecDeque::new(),
                fetched_bytes: 0,
                total_bytes: 0,
                fetching: 0,
                mounted_mods: 0,
                mounted: Vec::new(),
                events: VecDeque::new(),
                ready_sent: false,
                started: None,
                reported: None,
            }),
        }
    }

    /// Tells the downloads where the installed mods are.
    pub(crate) fn set_local(&self, local: Weak<dyn LocalSource>) {
        self.inner.borrow_mut().local = Some(local);
    }

    /// The caps for the connection that is starting, and the section its connect block carries: that the client takes content, and
    /// the most it takes. Nothing about the mods it has.
    #[cfg(test)]
    pub(crate) fn begin_connect(&self, caps: Caps) -> ModsSection {
        self.begin_connect_to(caps, CacheScope::default())
    }

    /// [`begin_connect`](Self::begin_connect) for a connection whose cache entries are `scope`'s: the server it dials, and whether
    /// entries from other servers count.
    pub(crate) fn begin_connect_to(&self, caps: Caps, scope: CacheScope) -> ModsSection {
        let mut inner = self.inner.borrow_mut();
        inner.caps = caps;
        inner.cache.set_scope(scope);
        inner.cache.set_max_bytes(caps.limits.cache_bytes);
        ModsSection {
            max_bytes: if caps.downloads {
                caps.limits.total_bytes
            } else {
                0
            },
            max_files: if caps.downloads {
                u32::try_from(caps.limits.files).unwrap_or(u32::MAX)
            } else {
                0
            },
        }
    }

    /// A new connection is starting: what the last one mounted goes, and the next message is an offer.
    pub(crate) fn reset(&self) {
        self.unmount_all();
        let mut inner = self.inner.borrow_mut();
        inner.phase = Phase::Waiting;
        inner.offer.clear();
        inner.jobs.clear();
        inner.todo.clear();
        inner.pending.clear();
        inner.fetched_bytes = 0;
        inner.total_bytes = 0;
        inner.fetching = 0;
        inner.mounted_mods = 0;
        inner.events.clear();
        inner.ready_sent = false;
        inner.started = None;
        inner.reported = None;
    }

    /// The connection ended: what it mounted goes.
    pub(crate) fn closed(&self) {
        self.unmount_all();
        let mut inner = self.inner.borrow_mut();
        if inner.phase != Phase::Failed {
            inner.phase = Phase::Idle;
        }
        inner.todo.clear();
        inner.pending.clear();
        inner.jobs.clear();
    }

    fn unmount_all(&self) {
        let names = std::mem::take(&mut self.inner.borrow_mut().mounted);
        for name in names {
            self.table.remove_server(&name);
        }
    }

    /// The mods the server announced.
    pub(crate) fn offer(&self) -> Vec<ModOffer> {
        self.inner.borrow().offer.clone()
    }

    /// Records that scripts were told `received` bytes; false when they were told that already.
    pub(crate) fn note_reported(&self, received: u64) -> bool {
        let mut inner = self.inner.borrow_mut();
        let changed = inner.reported != Some(received);
        inner.reported = Some(received);
        changed
    }

    /// Deletes the cache's entries; the bytes it held.
    pub(crate) fn clear_cache(&self) -> u64 {
        let mut inner = self.inner.borrow_mut();
        let bytes = inner.cache.bytes();
        inner.cache.clear();
        bytes
    }

    pub(crate) fn phase(&self) -> Phase {
        self.inner.borrow().phase
    }

    pub(crate) fn take_events(&self) -> Vec<Event> {
        self.inner.borrow_mut().events.drain(..).collect()
    }

    /// `(bytes received so far, bytes to receive in all, mods mounted after a fetch, mods fetched)`.
    pub(crate) fn progress(&self) -> (u64, u64, usize, usize) {
        let inner = self.inner.borrow();
        (
            inner.fetched_bytes,
            inner.total_bytes,
            inner.mounted_mods,
            inner.fetching,
        )
    }

    /// The id of the mod being received.
    pub(crate) fn current(&self) -> Option<String> {
        let inner = self.inner.borrow();
        let at = inner
            .pending
            .front()
            .map(|r| r.mod_index)
            .or_else(|| inner.todo.front().map(|r| r.mod_index))?;
        inner.offer.get(at as usize).map(|o| o.id.clone())
    }

    pub(crate) fn started(&self) -> Option<u32> {
        self.inner.borrow().started
    }

    pub(crate) fn set_started(&self, now: u32) {
        self.inner.borrow_mut().started = Some(now);
    }

    pub(crate) fn seconds(&self) -> u32 {
        self.inner.borrow().caps.limits.time_seconds()
    }

    /// What to send the server next, if anything: the next request (up to [`DOWNLOAD_WINDOW`] are outstanding at a time), or the
    /// ready word once everything is mounted.
    pub(crate) fn next_command(&self) -> Option<Command> {
        let mut inner = self.inner.borrow_mut();
        match inner.phase {
            Phase::Downloading if inner.pending.len() < DOWNLOAD_WINDOW => {
                let request = inner.todo.pop_front()?;
                inner.pending.push_back(request);
                Some(Command::Get(request))
            }
            Phase::Done if !inner.ready_sent => {
                inner.ready_sent = true;
                Some(Command::Ready)
            }
            _ => None,
        }
    }

    /// Ends the download with `reason`.
    pub(crate) fn fail(&self, reason: String) {
        self.unmount_all();
        let mut inner = self.inner.borrow_mut();
        inner.phase = Phase::Failed;
        inner.todo.clear();
        inner.pending.clear();
        inner.jobs.clear();
        inner.events.push_back(Event::Failed(reason));
    }

    /// A message arrived whole.
    pub(crate) fn receive(&self, data: &[u8]) {
        let result = match self.phase() {
            Phase::Waiting if is_offer(data) => self.on_offer(data),
            Phase::Downloading if is_data(data) => self.on_data(data),
            Phase::Waiting | Phase::Downloading => {
                Err("The server sent a message out of turn.".to_string())
            }
            Phase::Done | Phase::Failed | Phase::Idle => return,
        };
        if let Err(reason) = result {
            self.fail(reason);
        }
    }

    fn content_caps(&self, caps: &Caps) -> ContentCaps {
        ContentCaps::of(
            &self.mod_limits,
            caps.limits.files,
            caps.limits.file_bytes,
            caps.limits.total_bytes,
        )
    }

    /// Mounts the verified `files` of mod `id`, whose content hash is `hash`, noting how they reached the client.
    fn mount(
        &self,
        id: &str,
        hash: Hash,
        way: Way,
        files: Vec<(String, Vec<u8>)>,
    ) -> Result<(), String> {
        let name = mount_name(id);
        let origin = self
            .inner
            .borrow()
            .cache
            .scope()
            .owner
            .map(|server| MountOrigin { server, hash, way });
        mount_content(&self.table, &self.mod_limits, &name, files, origin)
            .map_err(|e| format!("The mod {id} could not be mounted: {e}."))?;
        let mut inner = self.inner.borrow_mut();
        if !inner.mounted.contains(&name) {
            inner.mounted.push(name);
        }
        Ok(())
    }

    fn on_offer(&self, data: &[u8]) -> Result<(), String> {
        let offer =
            decode_offer(data, &OfferCaps::of(&self.mod_limits)).map_err(|e| e.to_string())?;
        let mut inner = self.inner.borrow_mut();
        let caps = inner.caps;
        let content_caps = self.content_caps(&caps);
        let local = inner.local.as_ref().and_then(Weak::upgrade);
        let mut jobs: Vec<Option<Job>> = Vec::with_capacity(offer.len());
        let mut cached: Vec<(String, Hash, Way, Files)> = Vec::new();
        let (mut transfer, mut assembled) = (0u64, 0u64);
        // The cache's heads are read once an offer, and the older copies loaded to build from are bounded in all: twice what
        // the offer would download, so an offer cannot make the client read and hash its whole cache.
        let heads = inner.cache.heads(64);
        let download_total: u64 = offer
            .iter()
            .filter(|i| i.download)
            .map(ModOffer::bytes)
            .fold(0, u64::saturating_add);
        let mut base_budget = download_total.saturating_mul(2).min(MAX_BASE_BYTES);
        for item in &offer {
            // By package hash: an installed or cached copy of this very content needs nothing.
            if local
                .as_ref()
                .and_then(|l| l.hash_of(&item.id))
                .is_some_and(|h| h == item.hash)
            {
                jobs.push(None);
                continue;
            }
            if let Some((files, own)) = inner.cache.load_entry(&item.hash, &content_caps) {
                let way = if own { Way::OwnCache } else { Way::SharedCache };
                cached.push((item.id.clone(), item.hash, way, files));
                jobs.push(None);
                continue;
            }
            if !item.download {
                return Err(format!(
                    "The server needs the mod {} {} and you do not have it.",
                    item.id, item.version
                ));
            }
            if !caps.downloads {
                return Err(format!(
                    "The server needs the mod {}, and downloads are turned off in your settings.",
                    named(item)
                ));
            }
            if item.bytes() > caps.limits.mod_bytes as u64
                || item.files.len() > caps.limits.files
                || item
                    .files
                    .iter()
                    .any(|f| u64::from(f.size) > caps.limits.file_bytes)
            {
                return Err(format!(
                    "The mod {} is over your download limits.",
                    named(item)
                ));
            }
            assembled = assembled.saturating_add(item.bytes());
            if assembled > caps.limits.total_bytes.saturating_mul(4) {
                return Err(format!(
                    "The mod {} is over your download limit ({}).",
                    named(item),
                    size_text(caps.limits.total_bytes)
                ));
            }
            // The older copies this one may be built from: the installed mod of that id, and the cache's entries of it.
            let mut bases: Vec<Vec<(String, Vec<u8>)>> = Vec::new();
            let mut take_base = |files: Files, budget: &mut u64| {
                let size = files
                    .iter()
                    .map(|(_, d)| u64::try_from(d.len()).unwrap_or(u64::MAX))
                    .fold(0, u64::saturating_add);
                if size <= *budget {
                    *budget -= size;
                    bases.push(files);
                }
            };
            // An installed copy is read only when its size, known first, fits what is left of the budget.
            if let Some(local) = local.as_ref()
                && local.bytes_of(&item.id).is_some_and(|b| b <= base_budget)
                && let Some(files) = local.files_of(&item.id)
            {
                take_base(files, &mut base_budget);
            }
            let older = heads
                .iter()
                .filter(|(id, _, _)| id == &item.id)
                .map(|(_, _, hash)| *hash)
                .take(2);
            for hash in older {
                // Its size is known before it is read: an entry past what is left of the budget is not read at all.
                if inner.cache.size_of(&hash).is_none_or(|s| s > base_budget) {
                    continue;
                }
                if let Some(files) = inner.cache.load(&hash, &content_caps) {
                    take_base(files, &mut base_budget);
                }
            }
            let job = assemble(item, &bases);
            let missing: u64 = job
                .missing
                .iter()
                .zip(&item.files)
                .map(|(m, f)| {
                    m.iter()
                        .enumerate()
                        .filter(|(_, need)| **need)
                        .map(|(j, _)| {
                            let (from, to) = run_bytes(u64::from(f.size), j as u64, 1);
                            to - from
                        })
                        .sum::<u64>()
                })
                .sum();
            transfer = transfer.saturating_add(missing);
            if transfer > caps.limits.total_bytes {
                return Err(format!(
                    "The mod {} needs {} of your {} download limit.",
                    named(item),
                    size_text(transfer),
                    size_text(caps.limits.total_bytes)
                ));
            }
            jobs.push(Some(job));
        }
        let mut todo = VecDeque::new();
        for (at, job) in jobs.iter().enumerate() {
            let Some(job) = job else { continue };
            for (file, missing) in job.missing.iter().enumerate() {
                let mut j = 0;
                while j < missing.len() {
                    if !missing[j] {
                        j += 1;
                        continue;
                    }
                    let mut count = 1;
                    while j + count < missing.len()
                        && missing[j + count]
                        && (count as u32) < MAX_REQUEST_BLOCKS
                    {
                        count += 1;
                    }
                    todo.push_back(Request {
                        mod_index: at as u32,
                        file: file as u32,
                        start: j as u32,
                        count: count as u32,
                    });
                    j += count;
                }
            }
        }
        inner.fetching = jobs.iter().flatten().count();
        inner.total_bytes = transfer;
        inner.todo = todo;
        inner.offer = offer;
        inner.jobs = jobs;
        let (mods, bytes) = (inner.fetching, transfer);
        inner.events.push_back(Event::Started { mods, bytes });
        drop(inner);
        for (id, hash, way, files) in cached {
            self.mount(&id, hash, way, files)?;
        }
        // A mod that matched in pieces and needs nothing more is finished at once.
        let ready: Vec<usize> = {
            let inner = self.inner.borrow();
            inner
                .jobs
                .iter()
                .enumerate()
                .filter(|(at, job)| {
                    job.as_ref()
                        .is_some_and(|j| j.missing.iter().all(|m| m.iter().all(|n| !n)))
                        && !inner.todo.iter().any(|r| r.mod_index as usize == *at)
                })
                .map(|(at, _)| at)
                .collect()
        };
        for at in ready {
            self.finish_job(at)?;
        }
        self.settle();
        Ok(())
    }

    /// Moves to Done when nothing is left to fetch or finish.
    fn settle(&self) {
        let mut inner = self.inner.borrow_mut();
        if inner.phase == Phase::Failed {
            return;
        }
        let waiting = inner.jobs.iter().flatten().any(|j| !j.done);
        if waiting || !inner.todo.is_empty() || !inner.pending.is_empty() {
            inner.phase = Phase::Downloading;
        } else {
            inner.phase = Phase::Done;
            inner.events.push_back(Event::Done);
        }
    }

    /// Verifies a job's files and package hash, mounts it, and only then keeps it in the cache.
    fn finish_job(&self, at: usize) -> Result<(), String> {
        let (offer, files) = {
            let mut inner = self.inner.borrow_mut();
            let Some(Some(job)) = inner.jobs.get_mut(at) else {
                return Ok(());
            };
            if job.done {
                return Ok(());
            }
            let files: Vec<(String, Vec<u8>)> = job
                .offer
                .files
                .iter()
                .zip(&job.bufs)
                .map(|(f, b)| (f.path.clone(), b.clone()))
                .collect();
            (job.offer.clone(), files)
        };
        for (f, (_, bytes)) in offer.files.iter().zip(&files) {
            if file_hash(bytes) != f.hash {
                return Err(format!(
                    "The file {} of the mod {} does not match its hash; it was not kept.",
                    f.path.chars().take(60).collect::<String>(),
                    offer.id
                ));
            }
        }
        if summarize_files(&files).hash != offer.hash {
            return Err(format!(
                "The mod {} does not match the hash the server announced; it was not kept.",
                named(&offer)
            ));
        }
        let message = encode_content(&offer.id, &offer.version, &offer.hash, &files);
        self.mount(&offer.id, offer.hash, Way::Download, files)?;
        let mut inner = self.inner.borrow_mut();
        inner.cache.store(&offer.hash, &message);
        if let Some(Some(job)) = inner.jobs.get_mut(at) {
            job.done = true;
            job.bufs.clear();
        }
        inner.mounted_mods += 1;
        inner.events.push_back(Event::Mounted { id: offer.id });
        Ok(())
    }

    fn on_data(&self, data: &[u8]) -> Result<(), String> {
        let (request, bytes) = decode_data(data).map_err(|e| e.to_string())?;
        let at;
        {
            let mut inner = self.inner.borrow_mut();
            if inner.pending.front() != Some(&request) {
                return Err("The server sent data nobody asked for.".to_string());
            }
            at = request.mod_index as usize;
            let Some(Some(job)) = inner.jobs.get_mut(at) else {
                return Err("The server sent data for a mod that needs none.".to_string());
            };
            let file = request.file as usize;
            let Some(offer) = job.offer.files.get(file) else {
                return Err("The server sent data for a file that is not there.".to_string());
            };
            let (from, to) = run_bytes(
                u64::from(offer.size),
                u64::from(request.start),
                u64::from(request.count),
            );
            if bytes.len() as u64 != to - from {
                return Err("The server sent a block of the wrong size.".to_string());
            }
            for k in 0..request.count as usize {
                let lo = k * BLOCK_BYTES;
                let hi = (lo + BLOCK_BYTES).min(bytes.len());
                let block = bytes.get(lo..hi).unwrap_or_default();
                let index = request.start as usize + k;
                if offer
                    .blocks
                    .get(index)
                    .is_none_or(|h| *h != block_hash(block))
                {
                    return Err(format!(
                        "A block of the mod {} did not match its hash.",
                        job.offer.id
                    ));
                }
            }
            let (from, to) = (from as usize, to as usize);
            if let Some(buf) = job.bufs.get_mut(file).and_then(|b| b.get_mut(from..to)) {
                buf.copy_from_slice(bytes);
            }
            if let Some(m) = job.missing.get_mut(file) {
                for k in 0..request.count as usize {
                    if let Some(flag) = m.get_mut(request.start as usize + k) {
                        *flag = false;
                    }
                }
            }
            let done = job.missing.iter().all(|m| m.iter().all(|n| !n));
            inner.fetched_bytes = inner.fetched_bytes.saturating_add(bytes.len() as u64);
            inner.pending.pop_front();
            if !done {
                return Ok(());
            }
        }
        // The last block of the mod: check it all and mount it.
        self.finish_job(at)?;
        self.settle();
        Ok(())
    }
}

/// Builds the job of `item` from what the older copies in `bases` give: a file whose hash is a base file's (at any path), else
/// each block that equals the block of the same position of the base file of the same path.
fn assemble(item: &ModOffer, bases: &[Vec<(String, Vec<u8>)>]) -> Job {
    let mut by_hash: HashMap<Hash, &[u8]> = HashMap::new();
    let mut by_path: HashMap<String, Vec<&[u8]>> = HashMap::new();
    for base in bases {
        for (path, data) in base {
            by_hash.entry(file_hash(data)).or_insert(data);
            by_path
                .entry(path.to_ascii_lowercase())
                .or_default()
                .push(data);
        }
    }
    let mut bufs = Vec::with_capacity(item.files.len());
    let mut missing = Vec::with_capacity(item.files.len());
    for f in &item.files {
        let size = f.size as usize;
        if let Some(data) = by_hash.get(&f.hash).filter(|d| d.len() == size) {
            bufs.push(data.to_vec());
            missing.push(vec![false; f.blocks.len()]);
            continue;
        }
        let mut buf = vec![0u8; size];
        let mut need = vec![true; f.blocks.len()];
        if let Some(candidates) = by_path.get(&f.path.to_ascii_lowercase()) {
            for (j, want) in f.blocks.iter().enumerate() {
                let (from, to) = run_bytes(u64::from(f.size), j as u64, 1);
                let (from, to) = (from as usize, to as usize);
                for data in candidates {
                    let Some(block) = data.get(from..to) else {
                        continue;
                    };
                    if block_hash(block) == *want {
                        buf[from..to].copy_from_slice(block);
                        need[j] = false;
                        break;
                    }
                }
            }
        }
        bufs.push(buf);
        missing.push(need);
    }
    Job {
        offer: item.clone(),
        bufs,
        missing,
        done: false,
    }
}

/// The logic of the mods channel: the client's copy reads what arrives into its downloads (checks it, caches it, mounts it); the
/// server's has nothing to say (the server queues through its content server).
#[derive(Default)]
pub struct ModsChannel {
    client: Option<Rc<Downloads>>,
}

impl ModsChannel {
    pub(crate) fn bind(&mut self, downloads: Rc<Downloads>) {
        self.client = Some(downloads);
    }
}

impl ChunkChannel for ModsChannel {
    fn receive(&mut self, _kind: Kind, data: &[u8]) {
        if let Some(downloads) = &self.client {
            downloads.receive(data);
        }
    }

    fn reset(&mut self) {
        if let Some(downloads) = &self.client {
            downloads.reset();
        }
    }

    fn closed(&mut self) {
        if let Some(downloads) = &self.client {
            downloads.closed();
        }
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

// ---- the server's half

/// The server's [`ContentServer`]: the host's seam into the mods' serving state ([`Mods`]).
pub(crate) struct ModsContentServer {
    pub(crate) limits: ContentLimits,
}

impl ContentServer for ModsContentServer {
    fn limits(&self, _vm: &Vm) -> ContentLimits {
        self.limits
    }

    fn holds(&self, vm: &Vm) -> bool {
        Mods::of(vm).is_some_and(|mods| !mods.required().is_empty())
    }

    fn admit(&self, _vm: &Vm, held: usize) -> bool {
        held < self.limits.max_clients
    }

    fn allows_command(&self, name: &[u8]) -> bool {
        COMMANDS
            .iter()
            .any(|c| name.eq_ignore_ascii_case(c.as_bytes()))
    }

    fn begin(
        &self,
        vm: &mut Vm,
        hub: &ChannelHub,
        obj: ObjId,
        conn: ConnId,
        section: &ModsSection,
    ) -> Result<bool, String> {
        let Some(mods) = Mods::of(vm) else {
            return Err("This server cannot send its mods.".to_string());
        };
        mods.begin_serving(hub, obj, conn, section)
    }

    fn is_ready(&self, vm: &Vm, obj: ObjId) -> bool {
        Mods::of(vm).is_none_or(|mods| mods.serving_ready(obj))
    }

    fn progress(&self, vm: &Vm, obj: ObjId) -> u64 {
        let hub = vm.extension::<ChannelHub>();
        Mods::of(vm).map_or(0, |mods| mods.serving_progress(hub.as_deref(), obj))
    }

    fn end(&self, vm: &Vm, obj: ObjId) {
        if let Some(mods) = Mods::of(vm) {
            mods.end_serving(obj);
        }
    }
}

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;
