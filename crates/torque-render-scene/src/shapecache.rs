//! The moving shapes' built draws kept from one control sync to the next (the enhanced renderer's every frame).
//!
//! A window's control sync poses and builds every moving shape each frame (Players, bots, items, vehicles), most of them as they
//! were: a bot standing still, an item on the ground, a parked vehicle. [`crate::shapes::shape_draws`] is a pure function of its
//! inputs (the shape, its nodes' transforms, its parts, its placement, the point lights and the textures its names find), so a
//! call whose inputs are the last one's (byte for byte) gives the last one's draws. Inside [`with_cache`] each call, keyed by its
//! ghost and label, is looked up: the same inputs, and every texture name finding the same texture again (each name is asked of
//! this frame's set in the same order, so the set is what a build would make), give the kept draws; else the call builds and its
//! draws are kept. Either way the draws stay here: the call returns none and its [`Call`] says where they are
//! ([`with_draws`]); a build that cannot be kept (a texture put off) returns its draws as before.
//!
//! Each kept build has a stamp of its own (one counter for the process): a ghost whose calls all gave kept builds with the stamps
//! of its last sync drew the same draws (a scene can keep what it made of them).

use std::cell::RefCell;
use std::collections::HashMap;

use crate::Draw;

pub use crate::textures::Lookup;

/// One [`crate::shapes::shape_draws`] call inside [`with_cache`].
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// Its kept build's stamp; 0: not kept (its draws went out with the call).
    pub stamp: u64,
    /// The list its draws went in: the translucent shapes' (one entry for the group) or the opaque draws'.
    pub translucent: bool,
    /// The calls whose draws went in together (a body and its tyres) share a group.
    pub group: u64,
    /// Its draws.
    pub count: usize,
    /// The kept draws' texture indices (their build's set) and this frame's set's (none: the same).
    pub textures: Vec<(usize, usize)>,
    /// A kept build whose vertices stay in their meshes' own space: where its meshes are this frame (its draws' vertices name
    /// them by index, [`Kept::nodes`]). None: its vertices are in the world.
    pub places: Option<std::sync::Arc<[crate::shapes::MeshPlace]>>,
}

struct Entry {
    label: String,
    key: Vec<u8>,
    draws: Vec<Draw>,
    translucent: bool,
    lookups: Vec<Lookup>,
    stamp: u64,
    used: u64,
    /// The vertices its draws are still to get (a build of this run: [`with_cache`] fills them all at its end).
    pending: Option<crate::shapes::Fill>,
    /// A build in its meshes' own space: each draw's vertices' meshes (indices into its call's places); empty otherwise.
    nodes: Vec<Vec<u16>>,
}

#[derive(Default)]
struct Cache {
    on: bool,
    epoch: u64,
    /// By the ghost whose draws they are and the call's label (a vehicle's tyres share theirs with every vehicle of the kind).
    entries: HashMap<(u32, u64), Entry>,
    /// Each kept build's entry by its stamp.
    stamps: HashMap<u64, (u32, u64)>,
    /// Each kept build's stamp by its label (its ghost's number left out) and key ([`content_key`]): a ghost whose call has
    /// another ghost's label and key draws that build (one set of vertices for every ghost of a shape posed the same).
    by_content: HashMap<u64, u64>,
    owner: u32,
    /// This run's calls in order.
    calls: Vec<Call>,
    groups: u64,
    /// [`set_enabled`]`(false)`: [`with_cache`] runs its function with the cache off.
    disabled: bool,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

static STAMPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A stamp no build had before (never 0).
pub fn next_stamp() -> u64 {
    STAMPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Runs `f` with the cache on: its [`crate::shapes::shape_draws`] calls look up and keep their builds. The builds no call of this
/// run or the last asked for are let go.
pub fn with_cache<T>(f: impl FnOnce() -> T) -> T {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.on = !c.disabled;
        c.epoch += 1;
        c.calls.clear();
    });
    let out = f();
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        fill_pending(&mut c);
        c.on = false;
        let epoch = c.epoch;
        let Cache {
            entries,
            stamps,
            by_content,
            ..
        } = &mut *c;
        entries.retain(|at, e| {
            let keep = e.used + 1 >= epoch;
            if !keep {
                stamps.remove(&e.stamp);
                forget_content(by_content, at.0, e);
            }
            keep
        });
    });
    out
}

/// A cache apart from this thread's (a test keeps one for each scene it compares, each swapped in for its syncs).
#[derive(Default)]
pub struct Store(Cache);

/// Puts `store` in place of this thread's cache, and this thread's in `store`.
pub fn swap(store: &mut Store) {
    CACHE.with(|c| std::mem::swap(&mut *c.borrow_mut(), &mut store.0));
}

/// The time spent filling the kept builds' vertices so far, on every thread (ns).
static FILL_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The time [`with_cache`] has spent filling its builds' vertices so far, on every thread (ns).
pub fn fill_nanos() -> u64 {
    FILL_NS.load(std::sync::atomic::Ordering::Relaxed)
}

/// The vertices of this run's builds, made over a few threads ([`crate::par`]: each build's draws on one).
fn fill_pending(c: &mut Cache) {
    let t = web_time::Instant::now();
    fill_pending_now(c);
    FILL_NS.fetch_add(
        t.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64,
        std::sync::atomic::Ordering::Relaxed,
    );
}

fn fill_pending_now(c: &mut Cache) {
    type Pending<'a> = (
        crate::shapes::Fill,
        &'a mut Vec<Draw>,
        &'a mut Vec<Vec<u16>>,
    );
    let mut pending: Vec<Pending<'_>> = c
        .entries
        .values_mut()
        .filter_map(|e| e.pending.take().map(|f| (f, &mut e.draws, &mut e.nodes)))
        .collect();
    type Job<'a> = (
        crate::shapes::FillView<'a>,
        &'a mut Vec<Draw>,
        Option<&'a mut Vec<Vec<u16>>>,
    );
    let jobs: Vec<Job<'_>> = pending
        .iter_mut()
        .map(|(f, d, n)| {
            let node_space = f.node_space;
            (f.view(), &mut **d, node_space.then_some(&mut **n))
        })
        .collect();
    crate::par::map_owned(jobs, |(v, _, _)| v.work(), |(v, d, n)| v.run(d, n));
}

/// Whether [`with_cache`] keeps builds on this thread (a test compares its syncs with ones that build every shape).
pub fn set_enabled(on: bool) {
    CACHE.with(|c| c.borrow_mut().disabled = !on);
}

/// `f` of the kept draws of `stamp` (none when it is not kept).
pub fn with_draws<T>(stamp: u64, f: impl FnOnce(Option<&[Draw]>) -> T) -> T {
    CACHE.with(|c| {
        let c = c.borrow();
        let draws = c
            .stamps
            .get(&stamp)
            .and_then(|k| c.entries.get(k))
            .map(|e| e.draws.as_slice());
        f(draws)
    })
}

/// The kept builds' draws by stamp while [`with_kept`] holds the cache.
pub struct Kept<'a> {
    cache: &'a Cache,
}

impl<'a> Kept<'a> {
    /// The kept draws of `stamp` (none when it is not kept).
    pub fn draws(&self, stamp: u64) -> Option<&'a [Draw]> {
        let c = self.cache;
        c.stamps
            .get(&stamp)
            .and_then(|k| c.entries.get(k))
            .map(|e| e.draws.as_slice())
    }

    /// The kept build `stamp`'s draws' vertices' meshes when they stay in their meshes' own space ([`Call::places`]: one list a
    /// draw, one entry a vertex); none otherwise.
    pub fn nodes(&self, stamp: u64) -> Option<&'a [Vec<u16>]> {
        let c = self.cache;
        c.stamps
            .get(&stamp)
            .and_then(|k| c.entries.get(k))
            .map(|e| e.nodes.as_slice())
            .filter(|n| !n.is_empty())
    }
}

/// `f` with every kept build's draws at hand ([`Kept::draws`]), as [`with_draws`] for many stamps at once.
pub fn with_kept<T>(f: impl FnOnce(&Kept<'_>) -> T) -> T {
    CACHE.with(|c| {
        let c = c.borrow();
        f(&Kept { cache: &c })
    })
}

/// How many calls this run made so far (a ghost's calls are those between two marks), the calls from now on `ghost`'s.
pub fn start_ghost(ghost: u32) -> usize {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.owner = ghost;
        c.calls.len()
    })
}

/// Where the next call goes (for [`pushed`]).
pub fn mark() -> usize {
    CACHE.with(|c| c.borrow().calls.len())
}

/// The calls since `from` went in one list together, the translucent shapes' or the opaque draws'.
pub fn pushed(from: usize, translucent: bool) {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if !c.on {
            return;
        }
        c.groups += 1;
        let group = c.groups;
        for call in c.calls.iter_mut().skip(from) {
            call.translucent = translucent;
            call.group = group;
        }
    });
}

/// A build of `count` draws not kept (they go in the frame's lists): its call, when the cache is on.
pub fn unkept(count: usize, translucent: bool) {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if !c.on {
            return;
        }
        c.calls.push(Call {
            stamp: 0,
            translucent,
            group: 0,
            count,
            textures: Vec::new(),
            places: None,
        });
    });
}

/// The calls from `from` on.
pub fn calls_from(from: usize) -> Vec<Call> {
    CACHE.with(|c| {
        c.borrow()
            .calls
            .get(from..)
            .map(<[Call]>::to_vec)
            .unwrap_or_default()
    })
}

pub fn on() -> bool {
    CACHE.with(|c| c.borrow().on)
}

/// Whether `label`'s kept build has the key `key` and every texture name it looked up finds the same texture in `textures`
/// again (asked in the same order): its translucency then, the call recorded.
pub fn lookup(
    label: &str,
    key: &[u8],
    textures: &mut crate::textures::TextureSet<'_>,
) -> Option<bool> {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let epoch = c.epoch;
        let own = (c.owner, label_key(label));
        let at = if c
            .entries
            .get(&own)
            .is_some_and(|e| e.label == label && e.key == key)
        {
            own
        } else {
            // Another ghost's build of the same label and key.
            let stamp = *c.by_content.get(&content_key(c.owner, label, key))?;
            let at = *c.stamps.get(&stamp)?;
            let e = c.entries.get(&at)?;
            if e.key != key || shared_label(at.0, &e.label) != shared_label(c.owner, label) {
                return None;
            }
            at
        };
        let e = c.entries.get_mut(&at)?;
        // Each name asked again, in order; the build's local indices to this set's.
        let mut map: Vec<(usize, usize)> = Vec::new();
        for l in &e.lookups {
            let now = if l.modulated {
                textures.get_modulated(&l.name)
            } else {
                textures.get(&l.name)
            };
            match (&l.found, now) {
                (None, None) => {}
                (Some((was, t)), Some(i)) if textures.is_texture(i, t) => map.push((*was, i)),
                // (The build asks the same names in the same order: the set stays as a build makes it.)
                _ => return None,
            }
        }
        // Every draw's texture is one a name found.
        let mapped = e.draws.iter().all(|d| {
            d.texture
                .is_none_or(|(i, _)| map.iter().any(|(was, _)| *was == i))
        });
        if !mapped {
            return None;
        }
        e.used = epoch;
        let call = Call {
            stamp: e.stamp,
            translucent: e.translucent,
            group: 0,
            count: e.draws.len(),
            textures: map,
            places: None,
        };
        let translucent = e.translucent;
        c.calls.push(call);
        Some(translucent)
    })
}

/// `label` of a build of ghost `owner` around the ghost's number (the moving shapes' labels name it once, between spaces:
/// `StaticShape 7 path`, `image 0 of Player 7 path`): the text before it and the text after, or the whole label and "".
fn shared_label(owner: u32, label: &str) -> (&str, &str) {
    use std::io::Write as _;
    let mut buf = [0u8; 12];
    let len = {
        let mut at = &mut buf[..];
        let _ = write!(at, " {owner} ");
        12 - at.len()
    };
    let number = std::str::from_utf8(&buf[..len]).unwrap_or(" ");
    match label.find(number) {
        Some(at) => (&label[..at], &label[at + len..]),
        None => (label, ""),
    }
}

/// A build's label (its ghost's number left out) and key as [`Cache::by_content`] keys it, one hash of both (the entry found is
/// checked against them).
fn content_key(owner: u32, label: &str, key: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    shared_label(owner, label).hash(&mut h);
    key.hash(&mut h);
    h.finish()
}

/// Drops the content key of the build `e` of ghost `owner`, when it names `e`'s build.
fn forget_content(by_content: &mut HashMap<u64, u64>, owner: u32, e: &Entry) {
    let k = content_key(owner, &e.label, &e.key);
    if by_content.get(&k) == Some(&e.stamp) {
        by_content.remove(&k);
    }
}

/// How many builds are kept (each its own set of vertices; ghosts posed the same share one).
pub fn kept_builds() -> usize {
    CACHE.with(|c| c.borrow().entries.len())
}

/// A call's label as the entries key it (each entry holds its label too: a key two labels share finds neither's build).
fn label_key(label: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    label.hash(&mut h);
    h.finish()
}

/// Keeps a build of `label` for `key`: its draws stay here (none go back; a copy inside `with_cache_given`), or, when a texture it asked for was put off (a later
/// frame finds it decoded), they go back and nothing is kept.
pub fn keep(
    label: &str,
    key: Vec<u8>,
    draws: Vec<Draw>,
    translucent: bool,
    lookups: Vec<Lookup>,
    deferred: bool,
    fill: crate::shapes::Fill,
) -> Vec<Draw> {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let owner = c.owner;
        let at = (owner, label_key(label));
        if let Some(old) = c.entries.remove(&at) {
            c.stamps.remove(&old.stamp);
            forget_content(&mut c.by_content, owner, &old);
        }
        if deferred {
            // Not kept: in the world (its call has no places).
            let mut draws = draws;
            let mut fill = fill;
            fill.node_space = false;
            fill.view().run(&mut draws, None);
            c.calls.push(Call {
                stamp: 0,
                translucent,
                group: 0,
                count: draws.len(),
                textures: Vec::new(),
                places: None,
            });
            return draws;
        }
        let stamp = next_stamp();
        let epoch = c.epoch;
        c.calls.push(Call {
            stamp,
            translucent,
            group: 0,
            count: draws.len(),
            textures: Vec::new(),
            places: None,
        });
        c.stamps.insert(stamp, at);
        c.by_content.insert(content_key(owner, label, &key), stamp);
        c.entries.insert(
            at,
            Entry {
                label: label.to_string(),
                key,
                draws,
                translucent,
                lookups,
                stamp,
                used: epoch,
                pending: Some(fill),
                nodes: Vec::new(),
            },
        );
        Vec::new()
    })
}

/// The last call's places this frame ([`Call::places`]), when it gave a kept build (a build not kept is in the world).
pub fn set_places(places: Vec<crate::shapes::MeshPlace>) {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(call) = c.calls.last_mut().filter(|call| call.stamp != 0) {
            call.places = Some(places.into());
        }
    });
}

/// A key's bytes.
#[derive(Default)]
pub struct Key(pub Vec<u8>);

impl Key {
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn f32s(&mut self, v: &[f32]) -> &mut Self {
        for x in v {
            self.0.extend_from_slice(&x.to_bits().to_le_bytes());
        }
        self
    }

    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u64(v.len() as u64);
        self.0.extend_from_slice(v);
        self
    }
}
