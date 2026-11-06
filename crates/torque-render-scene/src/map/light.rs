//! A map's sun lighting for the fixed-function renderer: the engine's own relight in
//! place of TGE's scene lighting and its `.ml` caches.
//!
//! **What is lit.** Each terrain block gets a lightmap of [`TERRAIN_SIDE`] texels a side over its 256 squares. A repeated
//! block gets one for each of the [`MAX_TILES`] tiles that the most interiors stand in or near (their shadows fall on that
//! tile only; `ranked_tiles`) and one for every other tile, shadowed by the terrain alone. Each interior instance gets its own copy of its `.dif`'s lightmap rectangles (a surface's `map_offset` and
//! `map_size` in its lightmap), packed with a one-texel gutter into atlas pages of [`PAGE`] texels a side. A texel's value is
//! `baked + ambient + diffuse * max(N . L, 0) * visible`, clamped: `baked` the `.dif`'s own lightmap (map2dif's point lights;
//! black for the terrain), `ambient` and `diffuse` the sun's, `visible` 0 when a ray from the texel towards the sun meets an
//! interior triangle of an opaque material or a terrain block. An open texel is what GL lighting gave the surface before.
//!
//! **Where a texel is.** An interior texel is the point of the surface's plane its lightmap planes map to the texel's centre,
//! moved onto the nearest point of the rectangle's surfaces (a rectangle is larger than its polygon; a point outside it would
//! sit inside the wall beside it and come out dark), then a little off the surface. A terrain texel is on the drawn triangles.
//!
//! **Bounded work.** Map files come from servers: the sizes are capped ([`MAX_RECT_SIDE`], [`MAX_SOURCE_SIDE`], [`MAX_PAGES`],
//! [`MAX_TERRAINS`], [`MAX_CASTERS`], [`MAX_BAKED_BYTES`]) and the rays share [`MAX_WORK`] test units split evenly over fixed
//! jobs (a job past its share leaves its other texels unshadowed), so the same inputs give the same lightmaps byte for byte on
//! any thread count.
//!
//! **Cached by content.** The result is kept under a key over the sun, every file's bytes and every placement
//! (`Inputs::key`): in memory (the last [`MEMORY_KEPT`]) and, when [`set_cache_dir`] names a folder, on disk (one file a key,
//! written whole under a temporary name; a file whose size or header does not match what the key's plan needs is ignored).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use web_time::Instant;

use mapcore::budget::HEIGHTFIELD_SIDE as SIZE;
use mapcore::collision::Heightfield as Terrain;
use rayon::prelude::*;

use super::Fnv;
use super::interior::Local;
use glam::Vec3;

use crate::math::{Mat, unit_or_up};
use crate::{Sun, Texture};

/// Texels a side of a terrain block's lightmap (TGE's 512: two a square).
pub const TERRAIN_SIDE: usize = 512;
/// Texels a side of an interior atlas page.
pub const PAGE: u32 = 1024;
/// The most atlas pages a map's interiors fill (64 MiB of RGBA); rectangles past them stay GL-lit.
pub const MAX_PAGES: usize = 16;
/// The most terrain blocks lit (the rest stay GL-lit).
pub const MAX_TERRAINS: usize = 4;
/// The most tiles of a repeated block that get a lightmap of their own (those with the most interiors, `terrain_maps`; past
/// it a tile takes the terrain-only one).
pub const MAX_TILES: usize = 8;
/// The most tiles one interior's reach is spread over when the tiles are ranked; an interior wider than that (a map's ground
/// plane kilometres across) adds under 1/4096 to any tile and is left out of the ranking (it still casts on every lit tile).
pub const MAX_SPREAD: i64 = 4096;
/// How far from an interior's bounds its tile's lightmap reaches (its shadow falls this far at most, in metres).
pub const SHADOW_REACH: f32 = 128.0;
/// The largest side of a lightmap rectangle a surface may name (larger: that surface stays GL-lit), and of a model's baked page the
/// rectangles may lie in (the layer's caps: the importer leaves the others out).
pub use mapcore::budget::{MAX_RECT_SIDE, MAX_SOURCE_SIDE};
/// The most decoded `.dif` lightmap bytes (RGB) a relight holds; past it the rest count as black.
pub const MAX_BAKED_BYTES: usize = 256 << 20;
/// The most interior triangles that cast shadows (past it the rest cast none).
pub const MAX_CASTERS: usize = 2_000_000;
/// The most triangles of its rectangle's surfaces one texel is moved onto (a rectangle shared by many surfaces looks at the
/// first ones only).
pub const MAX_CLAMP_TRIS: usize = 512;
/// The test units (BVH nodes, triangles, terrain steps) all the rays of a relight may take, split evenly over its jobs.
pub const MAX_WORK: u64 = 6_000_000_000;
/// The most terrain steps one ray takes.
pub const MAX_TERRAIN_STEPS: u32 = 4_096;
/// Texels a job (an interior job is whole rectangles up to about this many texels; a terrain job is rows).
pub const JOB_TEXELS: usize = 8_192;
/// Results kept in memory.
pub const MEMORY_KEPT: usize = 4;
/// Files kept in the disk cache (the oldest are removed past it).
pub const DISK_KEPT: usize = 16;
/// The most bytes a disk cache file may hold.
pub const MAX_FILE_BYTES: u64 = 512 << 20;
/// The cache file's first bytes.
pub const MAGIC: &[u8; 8] = b"TRSMAPLT";
/// The light's version: a change of how texels are made changes every key.
pub const VERSION: u32 = 3;
/// How far a ray starts off its surface (metres).
const LIFT: f32 = 0.05;
/// How far a texel is pulled into its polygon from an edge (lightmap units).
const INSET: f32 = 0.02;

/// A terrain block as the light reads it.
#[derive(Clone)]
pub(crate) struct TerrainIn {
    pub ter: Arc<Terrain>,
    pub origin: [f32; 3],
    pub square: f32,
    pub repeat: bool,
    pub empty: Arc<Vec<bool>>,
    /// The `.ter` file's bytes' hash.
    pub hash: u64,
}

/// An interior instance as the light reads it.
#[derive(Clone)]
pub(crate) struct InstanceIn {
    pub local: Arc<Local>,
    pub matrix: Mat,
    pub scale: [f32; 3],
    /// For each of `local`'s materials (in `by_material`'s order): whether it is opaque (casts shadows, takes a lightmap).
    pub opaque: Vec<bool>,
}

/// What a relight reads.
#[derive(Clone)]
pub(crate) struct Inputs {
    pub sun: Sun,
    pub terrains: Vec<TerrainIn>,
    pub instances: Vec<InstanceIn>,
}

impl Inputs {
    /// The content key: the version, the sun, each terrain's file and placement, each instance's file, placement and opaque
    /// materials.
    pub fn key(&self) -> u64 {
        let mut h = Fnv::new("map light");
        h.u32(VERSION);
        let s = &self.sun;
        for v in s.towards.iter().chain(&s.diffuse).chain(&s.ambient) {
            h.u32(v.to_bits());
        }
        h.u64(self.terrains.len() as u64);
        for t in &self.terrains {
            h.u64(t.hash);
            t.origin.iter().for_each(|v| h.u32(v.to_bits()));
            h.u32(t.square.to_bits());
            h.u32(u32::from(t.repeat));
            let holes = t.empty.iter().filter(|&&e| e).count();
            h.u64(holes as u64);
            for (i, _) in t.empty.iter().enumerate().filter(|(_, e)| **e) {
                h.u32(i as u32);
            }
        }
        h.u64(self.instances.len() as u64);
        for i in &self.instances {
            h.u64(i.local.content);
            i.matrix.m.iter().for_each(|v| h.u32(v.to_bits()));
            i.scale.iter().for_each(|v| h.u32(v.to_bits()));
            h.u64(i.opaque.len() as u64);
            i.opaque.iter().for_each(|&o| h.u32(u32::from(o)));
        }
        h.0
    }
}

/// Where an instance's rectangle lies: its page and the corner of its gutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Place {
    pub page: u16,
    pub x: u32,
    pub y: u32,
}

/// A terrain lightmap: its block, the tile it covers and whether the interiors cast on it (false: the terrain-only lightmap of
/// a repeated block's other tiles).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerrainMap {
    pub block: usize,
    pub tile: [i32; 2],
    pub interiors: bool,
}

/// The atlas: each page's size and each instance's rectangles' places (None: GL-lit); the terrain lightmaps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    pub pages: Vec<[u32; 2]>,
    pub places: Vec<Vec<Option<Place>>>,
    pub terrains: Vec<TerrainMap>,
}

/// An instance's world bounds (None when not finite).
pub(crate) fn world_bounds(i: &InstanceIn) -> Option<([f32; 3], [f32; 3])> {
    let (lo, hi) = i.local.bounds;
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for k in 0..8 {
        let p = [
            if k & 1 == 0 { lo[0] } else { hi[0] },
            if k & 2 == 0 { lo[1] } else { hi[1] },
            if k & 4 == 0 { lo[2] } else { hi[2] },
        ];
        let s = i.scale;
        let w = i.matrix.mul_p([p[0] * s[0], p[1] * s[1], p[2] * s[2]]);
        for c in 0..3 {
            min[c] = min[c].min(w[c]);
            max[c] = max[c].max(w[c]);
        }
    }
    min.iter()
        .chain(&max)
        .all(|v| v.is_finite())
        .then_some((min, max))
}

/// The tile of block `t` that world (x, y) lies in.
pub(crate) fn tile_of(t: &TerrainIn, x: f32, y: f32) -> [i32; 2] {
    let span = SIZE as f32 * t.square;
    let f = |v: f32, o: f32| ((v - o) / span).floor().clamp(-1.0e6, 1.0e6) as i32;
    [f(x, t.origin[0]), f(y, t.origin[1])]
}

/// The terrain lightmaps: a block that does not repeat has one; a repeated block has one for each of the [`MAX_TILES`] tiles
/// with the most interiors near them, then the terrain-only one. Each interior's bounds, widened by [`SHADOW_REACH`], give one
/// unit spread over the tiles they cover by the area they cover of each, so a tile with buildings outranks one that a
/// kilometre-wide plane only crosses (ties in tile order; the chosen tiles in tile order).
fn terrain_maps(inputs: &Inputs) -> Vec<TerrainMap> {
    let mut out = Vec::new();
    let bounds: Vec<([f32; 3], [f32; 3])> =
        inputs.instances.iter().filter_map(world_bounds).collect();
    for (block, t) in inputs.terrains.iter().take(MAX_TERRAINS).enumerate() {
        if !t.repeat {
            out.push(TerrainMap {
                block,
                tile: [0, 0],
                interiors: true,
            });
            continue;
        }
        for tile in ranked_tiles(t, &bounds) {
            out.push(TerrainMap {
                block,
                tile,
                interiors: true,
            });
        }
        out.push(TerrainMap {
            block,
            tile: [0, 0],
            interiors: false,
        });
    }
    out
}

/// The tiles of block `t` that get a lightmap of their own (`terrain_maps`), in tile order.
pub(crate) fn ranked_tiles(t: &TerrainIn, bounds: &[([f32; 3], [f32; 3])]) -> Vec<[i32; 2]> {
    let span = f64::from(SIZE as f32 * t.square);
    let mut weight: std::collections::BTreeMap<[i32; 2], f64> = std::collections::BTreeMap::new();
    for (min, max) in bounds {
        let lo = [min[0] - SHADOW_REACH, min[1] - SHADOW_REACH];
        let hi = [max[0] + SHADOW_REACH, max[1] + SHADOW_REACH];
        let a = tile_of(t, lo[0], lo[1]);
        let b = tile_of(t, hi[0], hi[1]);
        let n = (i64::from(b[0]) - i64::from(a[0]) + 1) * (i64::from(b[1]) - i64::from(a[1]) + 1);
        let area = f64::from(hi[0] - lo[0]) * f64::from(hi[1] - lo[1]);
        if n > MAX_SPREAD || area.is_nan() || area <= 0.0 {
            continue;
        }
        let cover = |v: i32, axis: usize| {
            let start = f64::from(t.origin[axis]) + f64::from(v) * span;
            (f64::from(hi[axis]).min(start + span) - f64::from(lo[axis]).max(start)).max(0.0)
        };
        for ty in a[1]..=b[1] {
            for tx in a[0]..=b[0] {
                *weight.entry([tx, ty]).or_default() += cover(tx, 0) * cover(ty, 1) / area;
            }
        }
    }
    let mut ranked: Vec<([i32; 2], f64)> = weight.into_iter().filter(|(_, w)| *w > 0.0).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.truncate(MAX_TILES);
    let mut tiles: Vec<[i32; 2]> = ranked.into_iter().map(|(tile, _)| tile).collect();
    tiles.sort_unstable();
    tiles
}

impl Plan {
    /// The lightmap of block `block`'s tile `tile` (its own, else the block's terrain-only one).
    pub fn terrain_map(&self, block: usize, tile: [i32; 2]) -> Option<usize> {
        let own = self
            .terrains
            .iter()
            .position(|m| m.block == block && m.interiors && m.tile == tile);
        own.or_else(|| {
            self.terrains
                .iter()
                .position(|m| m.block == block && !m.interiors)
        })
    }

    /// The texels the rectangles placed hold (without their gutters).
    pub fn texels(&self, inputs: &Inputs) -> u64 {
        let mut n = 0u64;
        for (inst, places) in inputs.instances.iter().zip(&self.places) {
            for (r, p) in inst.local.rects.iter().zip(places) {
                if p.is_some() {
                    n += u64::from(r.size[0]) * u64::from(r.size[1]);
                }
            }
        }
        n
    }
}

/// Which rectangles of an instance are drawn with a lightmap: those an opaque material's triangle names.
fn wanted(inst: &InstanceIn) -> Vec<bool> {
    let mut out = vec![false; inst.local.rects.len()];
    for (k, tris) in inst.local.lightmap_tris.values().enumerate() {
        if !inst.opaque.get(k).copied().unwrap_or(false) {
            continue;
        }
        for t in tris {
            if let Some(w) = out.get_mut(t.rect as usize) {
                *w = true;
            }
        }
    }
    out
}

/// Packs every wanted rectangle (with a one-texel gutter) into pages, tallest first, in shelves (next fit).
pub(crate) fn plan(inputs: &Inputs) -> Plan {
    let mut places: Vec<Vec<Option<Place>>> = inputs
        .instances
        .iter()
        .map(|i| vec![None; i.local.rects.len()])
        .collect();
    let mut items: Vec<(u32, u32, usize, usize)> = Vec::new();
    for (ii, inst) in inputs.instances.iter().enumerate() {
        for (ri, (r, w)) in inst.local.rects.iter().zip(wanted(inst)).enumerate() {
            if w {
                items.push((r.size[0] + 2, r.size[1] + 2, ii, ri));
            }
        }
    }
    items.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(b.0.cmp(&a.0))
            .then((a.2, a.3).cmp(&(b.2, b.3)))
    });
    let mut pages: Vec<[u32; 2]> = Vec::new();
    // Next fit, the items tallest first: the open shelf of the last page (y, height, x used), else a new shelf under it, else
    // a new page (constant work an item, whatever a map holds).
    let mut shelf: Option<(u32, u32, u32)> = None;
    for (w, h, ii, ri) in items {
        let mut placed = None;
        if let Some(s) = shelf.as_mut() {
            if h <= s.1 && s.2 + w <= PAGE {
                placed = Some((s.2, s.0));
                s.2 += w;
            } else if s.0 + s.1 + h <= PAGE {
                *s = (s.0 + s.1, h, w);
                placed = Some((0, s.0));
            }
        }
        if placed.is_none() && pages.len() < MAX_PAGES {
            pages.push([PAGE, PAGE]);
            shelf = Some((0, h, w));
            placed = Some((0, 0));
        }
        if let Some((x, y)) = placed {
            places[ii][ri] = Some(Place {
                page: (pages.len() - 1) as u16,
                x,
                y,
            });
        }
    }
    Plan {
        pages,
        places,
        terrains: terrain_maps(inputs),
    }
}

/// Where a lit map's lightmaps came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Computed,
    Memory,
    Disk,
}

/// A relight's result.
#[derive(Debug)]
pub(crate) struct Lit {
    pub key: u64,
    pub plan: Plan,
    /// The interior atlas pages (RGBA8, one level).
    pub pages: Vec<Arc<Texture>>,
    /// Each lit terrain block's lightmap (RGBA8, one level).
    pub terrains: Vec<Arc<Texture>>,
}

/// What the last relight a thread asked for did (the checks print it).
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub key: u64,
    pub source: Source,
    pub seconds: f64,
    pub texels: u64,
    pub rays: u64,
    pub casters: usize,
    pub pages: usize,
    pub terrains: usize,
    /// Jobs that ran out of their share of [`MAX_WORK`].
    pub over_budget: usize,
    /// A hash of every lightmap texel (the cache file's bytes).
    pub digest: u64,
}

struct Cache {
    done: Vec<Arc<Lit>>,
    running: Option<u64>,
    dir: Option<PathBuf>,
    last: Option<Report>,
}

static CACHE: Mutex<Cache> = Mutex::new(Cache {
    done: Vec::new(),
    running: None,
    dir: None,
    last: None,
});

/// Off: maps are drawn GL-lit, without the relight.
pub static ENABLED: AtomicBool = AtomicBool::new(true);

fn cache() -> std::sync::MutexGuard<'static, Cache> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The disk cache's folder (None: memory only, as headless runs keep it).
pub fn set_cache_dir(dir: Option<PathBuf>) {
    cache().dir = dir;
}

/// The cache file of `key` in `dir` (the window client's folder is its profile's `cache\map-light`).
pub fn cache_file(dir: &std::path::Path, key: u64) -> PathBuf {
    dir.join(format!("{key:016x}.tml"))
}

/// Lets go of the results kept in memory (the checks' second load reads the disk).
pub fn forget_memory() {
    cache().done.clear();
}

/// What the last relight did.
pub fn last_report() -> Option<Report> {
    cache().last.clone()
}

/// The lightmaps for `inputs`: kept, read from the disk cache or made. `blocking` false (a live window): made on a thread of
/// its own and None until it is done (the map is drawn GL-lit meanwhile).
pub(crate) fn get(inputs: &Inputs, key: u64, blocking: bool) -> Option<Arc<Lit>> {
    // A browser has no threads: the maps are lit on the caller's.
    let blocking = blocking || cfg!(target_arch = "wasm32");
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    let dir = {
        let mut c = cache();
        if let Some(i) = c.done.iter().position(|l| l.key == key) {
            let lit = c.done.remove(i);
            c.done.push(Arc::clone(&lit));
            return Some(lit);
        }
        if !blocking {
            if c.running.is_some() {
                return None;
            }
            c.running = Some(key);
        }
        c.dir.clone()
    };
    if blocking {
        let (lit, report) = load_or_make(inputs, key, dir);
        let lit = Arc::new(lit);
        keep(Arc::clone(&lit), report);
        return Some(lit);
    }
    let inputs = inputs.clone();
    let spawned = std::thread::Builder::new()
        .name("map light".into())
        .spawn(move || {
            let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                load_or_make(&inputs, key, dir)
            }));
            match made {
                Ok((lit, report)) => keep(Arc::new(lit), report),
                // Never lit again this session rather than asked again every frame.
                Err(_) => cache().running = Some(u64::MAX),
            }
        });
    if spawned.is_err() {
        cache().running = None;
    }
    None
}

/// Whether the lightmaps for `key` are kept (a live window's build waits for them).
pub(crate) fn ready(key: u64) -> bool {
    cache().done.iter().any(|l| l.key == key)
}

/// The kept lightmaps for `key`.
pub(crate) fn kept(key: u64) -> Option<Arc<Lit>> {
    cache().done.iter().find(|l| l.key == key).cloned()
}

fn digest(lit: &Lit) -> u64 {
    let mut h = Fnv::new("map light texels");
    h.bytes(&encode(lit));
    h.0
}

fn keep(lit: Arc<Lit>, report: Report) {
    let mut c = cache();
    if c.running == Some(lit.key) {
        c.running = None;
    }
    c.done.retain(|l| l.key != lit.key);
    if c.done.len() >= MEMORY_KEPT {
        c.done.remove(0);
    }
    c.done.push(lit);
    c.last = Some(report);
}

fn load_or_make(inputs: &Inputs, key: u64, dir: Option<PathBuf>) -> (Lit, Report) {
    let start = Instant::now();
    let plan = plan(inputs);
    if let Some(d) = &dir
        && let Some(lit) = read_file(&cache_file(d, key), key, &plan)
    {
        let seconds = start.elapsed().as_secs_f64();
        let report = Report {
            key,
            source: Source::Disk,
            seconds,
            texels: lit.plan.texels(inputs),
            rays: 0,
            casters: 0,
            pages: lit.pages.len(),
            terrains: lit.terrains.len(),
            over_budget: 0,
            digest: digest(&lit),
        };
        return (lit, report);
    }
    let (lit, mut report) = make(inputs, key, plan);
    report.seconds = start.elapsed().as_secs_f64();
    report.digest = digest(&lit);
    if let Some(d) = &dir {
        write_file(d, &lit);
    }
    (lit, report)
}

// ---- the geometry the rays meet

/// A bounding volume hierarchy over the shadow casters (world-space triangles), split at the median of the widest axis.
pub struct Bvh {
    tris: Vec<[[f32; 3]; 3]>,
    nodes: Vec<Node>,
}

#[derive(Clone, Copy)]
struct Node {
    min: [f32; 3],
    max: [f32; 3],
    /// A leaf's first triangle, or an inner node's right child (its left is the node after it).
    index: u32,
    /// A leaf's triangles (0: an inner node).
    count: u32,
}

const LEAF: usize = 4;

impl Bvh {
    pub fn new(mut tris: Vec<[[f32; 3]; 3]>) -> Bvh {
        tris.retain(|t| t.iter().flatten().all(|v| v.is_finite()));
        let mut nodes = Vec::new();
        if !tris.is_empty() {
            let n = tris.len();
            let mut centres: Vec<[f32; 3]> = tris
                .iter()
                .map(|t| std::array::from_fn(|c| (t[0][c] + t[1][c] + t[2][c]) / 3.0))
                .collect();
            build_node(&mut tris, &mut centres, 0, n, &mut nodes);
        }
        Bvh { tris, nodes }
    }

    pub fn len(&self) -> usize {
        self.tris.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// Whether the segment from `o` along `d` (unit) for `t` in (0, `far`) meets a triangle; `work` is charged a unit a node
    /// and a triangle, and the ray answers "no" once it is spent.
    pub fn hits(&self, o: [f32; 3], d: [f32; 3], far: f32, work: &mut u64) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        let inv = d.map(|c| if c == 0.0 { f32::INFINITY } else { 1.0 / c });
        let mut stack = [0u32; 64];
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let ni = stack[top] as usize;
            let node = self.nodes[ni];
            if *work == 0 {
                return false;
            }
            *work -= 1;
            if !slab(&node, o, inv, far) {
                continue;
            }
            if node.count > 0 {
                let first = node.index as usize;
                for t in &self.tris[first..first + node.count as usize] {
                    if *work == 0 {
                        return false;
                    }
                    *work -= 1;
                    if tri_hit(t, o, d, far) {
                        return true;
                    }
                }
            } else if top + 2 <= stack.len() {
                stack[top] = node.index;
                stack[top + 1] = (ni + 1) as u32;
                top += 2;
            }
        }
        false
    }

    /// The nearest triangle the segment from `o` along `d` (unit) for `t` in (0, `far`) meets: its `t` and the triangle.
    pub fn nearest(&self, o: [f32; 3], d: [f32; 3], far: f32) -> Option<(f32, [[f32; 3]; 3])> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = d.map(|c| if c == 0.0 { f32::INFINITY } else { 1.0 / c });
        let mut best: Option<(f32, [[f32; 3]; 3])> = None;
        let mut stack = [0u32; 64];
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let ni = stack[top] as usize;
            let node = self.nodes[ni];
            let reach = best.map_or(far, |b| b.0);
            if !slab(&node, o, inv, reach) {
                continue;
            }
            if node.count > 0 {
                let first = node.index as usize;
                for t in &self.tris[first..first + node.count as usize] {
                    if let Some(tt) = tri_t(t, o, d, best.map_or(far, |b| b.0)) {
                        best = Some((tt, *t));
                    }
                }
            } else if top + 2 <= stack.len() {
                stack[top] = node.index;
                stack[top + 1] = (ni + 1) as u32;
                top += 2;
            }
        }
        best
    }
}

fn build_node(
    tris: &mut [[[f32; 3]; 3]],
    centres: &mut [[f32; 3]],
    start: usize,
    end: usize,
    nodes: &mut Vec<Node>,
) -> usize {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for t in &tris[start..end] {
        for v in t {
            for c in 0..3 {
                min[c] = min[c].min(v[c]);
                max[c] = max[c].max(v[c]);
            }
        }
    }
    let me = nodes.len();
    nodes.push(Node {
        min,
        max,
        index: start as u32,
        count: (end - start) as u32,
    });
    if end - start <= LEAF {
        return me;
    }
    let mut cmin = [f32::INFINITY; 3];
    let mut cmax = [f32::NEG_INFINITY; 3];
    for c in &centres[start..end] {
        for k in 0..3 {
            cmin[k] = cmin[k].min(c[k]);
            cmax[k] = cmax[k].max(c[k]);
        }
    }
    let axis = (0..3)
        .max_by(|&a, &b| (cmax[a] - cmin[a]).total_cmp(&(cmax[b] - cmin[b])))
        .unwrap_or(0);
    if cmax[axis] - cmin[axis] <= 0.0 {
        return me;
    }
    let mid = (start + end) / 2;
    // Median split along the widest axis of the centres (both slices sorted together).
    let mut order: Vec<usize> = (start..end).collect();
    order.select_nth_unstable_by(mid - start, |&a, &b| {
        centres[a][axis]
            .total_cmp(&centres[b][axis])
            .then(a.cmp(&b))
    });
    let t2: Vec<[[f32; 3]; 3]> = order.iter().map(|&i| tris[i]).collect();
    let c2: Vec<[f32; 3]> = order.iter().map(|&i| centres[i]).collect();
    tris[start..end].copy_from_slice(&t2);
    centres[start..end].copy_from_slice(&c2);
    build_node(tris, centres, start, mid, nodes);
    let right = build_node(tris, centres, mid, end, nodes);
    nodes[me].index = right as u32;
    nodes[me].count = 0;
    me
}

fn slab(n: &Node, o: [f32; 3], inv: [f32; 3], far: f32) -> bool {
    let mut t0 = 0.0f32;
    let mut t1 = far;
    for c in 0..3 {
        let a = (n.min[c] - o[c]) * inv[c];
        let b = (n.max[c] - o[c]) * inv[c];
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        // NaN (0 * inf on a slab's face) keeps the bound.
        if lo > t0 {
            t0 = lo;
        }
        if hi < t1 {
            t1 = hi;
        }
        if t0 > t1 {
            return false;
        }
    }
    true
}

/// Möller and Trumbore, both sides, `t` in (0, `far`).
fn tri_hit(t: &[[f32; 3]; 3], o: [f32; 3], d: [f32; 3], far: f32) -> bool {
    tri_t(t, o, d, far).is_some()
}

/// Where the ray meets the triangle (its `t` in (0, `far`)), both faces.
fn tri_t(t: &[[f32; 3]; 3], o: [f32; 3], d: [f32; 3], far: f32) -> Option<f32> {
    let [a, b, c] = t.map(Vec3::from);
    let d = Vec3::from(d);
    let (e1, e2) = (b - a, c - a);
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = Vec3::from(o) - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let tt = e2.dot(q) * inv;
    (tt > 0.0 && tt < far).then_some(tt)
}

/// A terrain block's surface as the rays meet it.
pub(crate) struct Ground {
    t: TerrainIn,
    top: f32,
}

impl Ground {
    pub fn new(t: TerrainIn) -> Ground {
        let mut top = f32::NEG_INFINITY;
        for y in 0..SIZE {
            for x in 0..SIZE {
                top = top.max(t.ter.height(x, y));
            }
        }
        Ground {
            top: top + t.origin[2],
            t,
        }
    }

    /// The surface height and smooth normal at (x, y) (None off a block that does not repeat, or over a hole).
    pub fn surface(&self, x: f32, y: f32) -> Option<(f32, [f32; 3])> {
        let t = &self.t;
        let gx = (x - t.origin[0]) / t.square;
        let gy = (y - t.origin[1]) / t.square;
        if !gx.is_finite() || !gy.is_finite() {
            return None;
        }
        let span = SIZE as f32;
        let inside = (0.0..span).contains(&gx) && (0.0..span).contains(&gy);
        if !t.repeat && !inside {
            return None;
        }
        let (ix, iy) = (gx.floor(), gy.floor());
        let (fx, fy) = (gx - ix, gy - iy);
        let (x0, y0) = (ix as i64, iy as i64);
        let w = |v: i64| v.rem_euclid(SIZE as i64) as usize;
        if t.empty.get(w(x0) + w(y0) * SIZE).copied().unwrap_or(true) {
            return None;
        }
        let h = |dx: i64, dy: i64| t.ter.height(w(x0 + dx), w(y0 + dy));
        let z = if t.ter.main_diagonal(w(x0), w(y0)) {
            if fx > fy {
                h(0, 0) + fx * (h(1, 0) - h(0, 0)) + fy * (h(1, 1) - h(1, 0))
            } else {
                h(0, 0) + fy * (h(0, 1) - h(0, 0)) + fx * (h(1, 1) - h(0, 1))
            }
        } else if fx + fy < 1.0 {
            h(0, 0) + fx * (h(1, 0) - h(0, 0)) + fy * (h(0, 1) - h(0, 0))
        } else {
            h(1, 1) + (1.0 - fx) * (h(0, 1) - h(1, 1)) + (1.0 - fy) * (h(1, 0) - h(1, 1))
        };
        let sq = t.square;
        let corner = |cx: i64, cy: i64| -> [f32; 3] {
            let hh = |dx: i64, dy: i64| t.ter.height(w(cx + dx), w(cy + dy));
            [
                (hh(-1, 0) - hh(1, 0)) / (2.0 * sq),
                (hh(0, -1) - hh(0, 1)) / (2.0 * sq),
                1.0,
            ]
        };
        let (n00, n10, n01, n11) = (
            corner(x0, y0),
            corner(x0 + 1, y0),
            corner(x0, y0 + 1),
            corner(x0 + 1, y0 + 1),
        );
        let n: [f32; 3] = std::array::from_fn(|c| {
            let a = n00[c] + (n10[c] - n00[c]) * fx;
            let b = n01[c] + (n11[c] - n01[c]) * fx;
            a + (b - a) * fy
        });
        Some((z + t.origin[2], unit_or_up(n.into()).to_array()))
    }

    /// Whether the ray from `o` along `d` passes under this block's surface within `far`, walked in half squares.
    pub fn hits(&self, o: [f32; 3], d: [f32; 3], far: f32, work: &mut u64) -> bool {
        let step = self.t.square * 0.5;
        // Rays heading up leave the block once above its highest sample.
        let mut s = step;
        let mut steps = 0;
        while s < far && steps < MAX_TERRAIN_STEPS {
            if *work == 0 {
                return false;
            }
            *work -= 1;
            steps += 1;
            let p = [o[0] + d[0] * s, o[1] + d[1] * s, o[2] + d[2] * s];
            if p[2] > self.top && d[2] >= 0.0 {
                return false;
            }
            match self.surface(p[0], p[1]) {
                Some((z, _)) if p[2] < z - 0.01 => return true,
                None if !self.t.repeat => {
                    // Off a block that does not repeat: it may come back only if the ray heads into it again; stop when
                    // leaving.
                    let span = SIZE as f32 * self.t.square;
                    let gx = (p[0] - self.t.origin[0]) / span;
                    let gy = (p[1] - self.t.origin[1]) / span;
                    let leaving = (gx < 0.0 && d[0] <= 0.0)
                        || (gx > 1.0 && d[0] >= 0.0)
                        || (gy < 0.0 && d[1] <= 0.0)
                        || (gy > 1.0 && d[1] >= 0.0);
                    if leaving {
                        return false;
                    }
                }
                _ => {}
            }
            s += step;
        }
        false
    }
}

/// Everything a shadow ray may meet.
pub(crate) struct World {
    pub bvh: Bvh,
    pub grounds: Vec<Ground>,
}

/// How far a shadow ray looks.
const FAR: f32 = 4_000.0;

impl World {
    pub fn occluded(&self, o: [f32; 3], d: [f32; 3], work: &mut u64) -> bool {
        self.bvh.hits(o, d, FAR, work) || self.ground_occluded(o, d, work)
    }

    fn ground_occluded(&self, o: [f32; 3], d: [f32; 3], work: &mut u64) -> bool {
        self.grounds.iter().any(|g| g.hits(o, d, FAR, work))
    }
}

// ---- making the texels

/// The world-space triangles of every instance's opaque materials (at most [`MAX_CASTERS`]).
pub(crate) fn casters(inputs: &Inputs) -> Vec<[[f32; 3]; 3]> {
    let mut out = Vec::new();
    'all: for inst in &inputs.instances {
        let s = inst.scale;
        for (k, tris) in inst.local.by_material.values().enumerate() {
            if !inst.opaque.get(k).copied().unwrap_or(false) {
                continue;
            }
            for t in tris.chunks_exact(3) {
                if out.len() >= MAX_CASTERS {
                    break 'all;
                }
                out.push([0, 1, 2].map(|i| {
                    let p = t[i].0;
                    inst.matrix.mul_p([p[0] * s[0], p[1] * s[1], p[2] * s[2]])
                }));
            }
        }
    }
    out
}

/// One job's texels: a terrain block's rows, or some rectangles of one instance.
enum Job {
    /// A terrain lightmap's rows (the lightmap's index in the plan).
    Terrain {
        map: usize,
        rows: (usize, usize),
    },
    Rects {
        instance: usize,
        rects: Vec<usize>,
    },
}

fn jobs(inputs: &Inputs, plan: &Plan) -> Vec<Job> {
    let mut out = Vec::new();
    let rows = (JOB_TEXELS / TERRAIN_SIDE).max(1);
    for map in 0..plan.terrains.len() {
        let mut y = 0;
        while y < TERRAIN_SIDE {
            out.push(Job::Terrain {
                map,
                rows: (y, (y + rows).min(TERRAIN_SIDE)),
            });
            y += rows;
        }
    }
    for (instance, places) in plan.places.iter().enumerate() {
        let rects = &inputs.instances[instance].local.rects;
        let mut batch = Vec::new();
        let mut texels = 0usize;
        for (ri, p) in places.iter().enumerate() {
            if p.is_none() {
                continue;
            }
            batch.push(ri);
            texels += (rects[ri].size[0] * rects[ri].size[1]) as usize;
            if texels >= JOB_TEXELS {
                out.push(Job::Rects {
                    instance,
                    rects: std::mem::take(&mut batch),
                });
                texels = 0;
            }
        }
        if !batch.is_empty() {
            out.push(Job::Rects {
                instance,
                rects: batch,
            });
        }
    }
    out
}

/// A job's texels (RGB, in its rows' or rectangles' order), the rays it cast, and whether it ran out of work.
struct Done {
    texels: Vec<[u8; 3]>,
    rays: u64,
    over: bool,
}

/// The decoded `.dif` lightmaps of each distinct file (by its hash): RGB, width, height.
type Baked = HashMap<u64, Arc<Vec<Option<(Vec<u8>, u32, u32)>>>>;

fn decode_baked(inputs: &Inputs, plan: &Plan) -> Baked {
    let mut out: Baked = HashMap::new();
    let mut bytes = 0usize;
    for (inst, places) in inputs.instances.iter().zip(&plan.places) {
        let l = &inst.local;
        if out.contains_key(&l.content) || places.iter().all(Option::is_none) {
            continue;
        }
        let mut maps = Vec::new();
        for png in &l.lightmaps {
            let fits = png.width <= MAX_SOURCE_SIDE && png.height <= MAX_SOURCE_SIDE;
            let size = png.width as usize * png.height as usize * 3;
            let decoded = if fits && bytes + size <= MAX_BAKED_BYTES {
                images::decode(images::Kind::Png, &png.bytes)
                    .ok()
                    .filter(|b| b.width == png.width && b.height == png.height)
                    .map(|b| {
                        let bpp = b.format.bytes_per_pixel();
                        let mut rgb = Vec::with_capacity(size);
                        for px in b.bits.chunks_exact(bpp) {
                            match b.format {
                                images::Format::Alpha => rgb.extend_from_slice(&[0, 0, 0]),
                                _ => rgb.extend_from_slice(&px[..3]),
                            }
                        }
                        (rgb, b.width, b.height)
                    })
            } else {
                None
            };
            if let Some((rgb, _, _)) = &decoded {
                bytes += rgb.len();
            }
            maps.push(decoded);
        }
        out.insert(l.content, Arc::new(maps));
    }
    out
}

/// A texel's value from what reaches it.
fn shade(baked: [f32; 3], sun: &Sun, ndotl: f32, visible: bool) -> [u8; 3] {
    let lit = if visible { ndotl.max(0.0) } else { 0.0 };
    std::array::from_fn(|c| {
        let v = baked[c] + sun.ambient[c] + sun.diffuse[c] * lit;
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    })
}

fn terrain_job(
    world: &World,
    map: TerrainMap,
    rows: (usize, usize),
    sun: &Sun,
    mut work: u64,
) -> Done {
    let g = &world.grounds[map.block];
    let t = &g.t;
    let span = SIZE as f32 * t.square;
    let tile = [map.tile[0] as f32 * span, map.tile[1] as f32 * span];
    let d = unit_or_up(sun.towards.into());
    let mut texels = Vec::with_capacity((rows.1 - rows.0) * TERRAIN_SIDE);
    let mut rays = 0;
    let per = SIZE as f32 / TERRAIN_SIDE as f32;
    for j in rows.0..rows.1 {
        for i in 0..TERRAIN_SIDE {
            let x = t.origin[0] + tile[0] + (i as f32 + 0.5) * per * t.square;
            let y = t.origin[1] + tile[1] + (j as f32 + 0.5) * per * t.square;
            let Some((z, n)) = g.surface(x, y) else {
                texels.push(shade([0.0; 3], sun, 1.0, true));
                continue;
            };
            let ndotl = Vec3::from(n).dot(d);
            let mut visible = ndotl > 0.0;
            if visible {
                rays += 1;
                let o = [x + n[0] * LIFT, y + n[1] * LIFT, z + n[2] * LIFT + 0.02];
                visible = if map.interiors {
                    !world.occluded(o, d.to_array(), &mut work)
                } else {
                    !world.ground_occluded(o, d.to_array(), &mut work)
                };
            }
            texels.push(shade([0.0; 3], sun, ndotl, visible));
        }
    }
    Done {
        texels,
        rays,
        over: work == 0,
    }
}

/// The point of rectangle `r`'s plane at lightmap texel (tx, ty), pulled onto its polygons; None when the plane is edge-on to
/// the lightmap's axes. Each clamp triangle tried costs one unit of `work` (a map's polygons set how many a texel tries, so the
/// job's share bounds them with its rays); with none left the point is the plane's, not pulled onto the polygons.
pub(crate) fn texel_point(
    r: &super::interior::Rect,
    tx: f32,
    ty: f32,
    work: &mut u64,
) -> Option<[f32; 3]> {
    let [a0, a1] = r.axes;
    let a2 = 3 - a0 - a1;
    if r.normal[a2].abs() < 1e-4 || r.scale[0] == 0.0 || r.scale[1] == 0.0 {
        return None;
    }
    let mut q = [(tx - r.add[0]) / r.scale[0], (ty - r.add[1]) / r.scale[1]];
    // Onto the nearest point of the rectangle's surfaces (in the lightmap's two axes).
    let mut inside = false;
    let mut best = (f32::INFINITY, q);
    let budget = MAX_CLAMP_TRIS.min(usize::try_from(*work).unwrap_or(usize::MAX));
    let mut left = budget;
    for poly in &r.polys {
        if left == 0 || inside {
            break;
        }
        let pts: Vec<[f32; 2]> = poly.iter().take(left + 2).map(|p| [p[a0], p[a1]]).collect();
        for k in 0..pts.len().saturating_sub(2) {
            if left == 0 {
                break;
            }
            left -= 1;
            let tri = [pts[k], pts[k + 1], pts[k + 2]];
            if in_tri(tri, q) {
                inside = true;
            }
            for e in 0..3 {
                let c = nearest_on_segment(tri[e], tri[(e + 1) % 3], q);
                let dd = (c[0] - q[0]).powi(2) + (c[1] - q[1]).powi(2);
                if dd < best.0 {
                    let centre = [
                        (tri[0][0] + tri[1][0] + tri[2][0]) / 3.0,
                        (tri[0][1] + tri[1][1] + tri[2][1]) / 3.0,
                    ];
                    // A little into the triangle, so the point is not on the edge the next wall stands on.
                    let to = [centre[0] - c[0], centre[1] - c[1]];
                    let l = (to[0] * to[0] + to[1] * to[1]).sqrt();
                    let inset = if l > 0.0 {
                        let k = (INSET / r.scale[0].abs().max(1e-6)).min(l * 0.5) / l;
                        [c[0] + to[0] * k, c[1] + to[1] * k]
                    } else {
                        c
                    };
                    best = (dd, inset);
                }
            }
        }
    }
    *work = work.saturating_sub((budget - left) as u64);
    if !inside && best.0.is_finite() {
        q = best.1;
    }
    let mut p = [0.0f32; 3];
    p[a0] = q[0];
    p[a1] = q[1];
    p[a2] = -(r.d + r.normal[a0] * q[0] + r.normal[a1] * q[1]) / r.normal[a2];
    p.iter().all(|v| v.is_finite()).then_some(p)
}

fn in_tri(t: [[f32; 2]; 3], p: [f32; 2]) -> bool {
    let s =
        |a: [f32; 2], b: [f32; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    let (d0, d1, d2) = (s(t[0], t[1]), s(t[1], t[2]), s(t[2], t[0]));
    let neg = d0 < 0.0 || d1 < 0.0 || d2 < 0.0;
    let pos = d0 > 0.0 || d1 > 0.0 || d2 > 0.0;
    !(neg && pos)
}

fn nearest_on_segment(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> [f32; 2] {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let l = ab[0] * ab[0] + ab[1] * ab[1];
    if l <= 0.0 {
        return a;
    }
    let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / l).clamp(0.0, 1.0);
    [a[0] + ab[0] * t, a[1] + ab[1] * t]
}

fn rects_job(
    world: &World,
    inputs: &Inputs,
    baked: &Baked,
    instance: usize,
    rects: &[usize],
    mut work: u64,
) -> Done {
    let inst = &inputs.instances[instance];
    let sun = &inputs.sun;
    let d = unit_or_up(sun.towards.into());
    let s = Vec3::from(inst.scale);
    let maps = baked.get(&inst.local.content);
    let mut texels = Vec::new();
    let mut rays = 0;
    for &ri in rects {
        let r = &inst.local.rects[ri];
        let n = unit_or_up(
            inst.matrix
                .mul_v((Vec3::from(r.normal) / s).to_array())
                .into(),
        );
        let ndotl = n.dot(d);
        let source = maps
            .and_then(|m| m.get(r.lightmap as usize))
            .and_then(Option::as_ref);
        for j in 0..r.size[1] {
            for i in 0..r.size[0] {
                let (sx, sy) = (r.offset[0] + i, r.offset[1] + j);
                let b = source.filter(|(_, w, h)| sx < *w && sy < *h).map_or(
                    [0.0; 3],
                    |(rgb, w, _)| {
                        let k = ((sy * w + sx) * 3) as usize;
                        [0, 1, 2].map(|c| f32::from(rgb[k + c]) / 255.0)
                    },
                );
                let mut visible = ndotl > 0.0;
                if visible {
                    match texel_point(r, sx as f32 + 0.5, sy as f32 + 0.5, &mut work) {
                        Some(p) => {
                            let w = inst.matrix.mul_p([p[0] * s[0], p[1] * s[1], p[2] * s[2]]);
                            let o = [w[0] + n[0] * LIFT, w[1] + n[1] * LIFT, w[2] + n[2] * LIFT];
                            rays += 1;
                            visible = !world.occluded(o, d.to_array(), &mut work);
                        }
                        None => visible = false,
                    }
                }
                texels.push(shade(b, sun, ndotl, visible));
            }
        }
    }
    Done {
        texels,
        rays,
        over: work == 0,
    }
}

/// Makes the lightmaps of `inputs` under `plan`.
pub(crate) fn make(inputs: &Inputs, key: u64, plan: Plan) -> (Lit, Report) {
    let baked = decode_baked(inputs, &plan);
    let bvh = Bvh::new(casters(inputs));
    let grounds: Vec<Ground> = inputs
        .terrains
        .iter()
        .take(MAX_TERRAINS)
        .cloned()
        .map(Ground::new)
        .collect();
    let world = World { bvh, grounds };
    let jobs = jobs(inputs, &plan);
    let share = MAX_WORK / jobs.len().max(1) as u64;
    let done: Vec<Done> = jobs
        .par_iter()
        .map(|j| match j {
            Job::Terrain { map, rows } => {
                terrain_job(&world, plan.terrains[*map], *rows, &inputs.sun, share)
            }
            Job::Rects { instance, rects } => {
                rects_job(&world, inputs, &baked, *instance, rects, share)
            }
        })
        .collect();
    let mut terrains: Vec<Vec<u8>> = (0..plan.terrains.len())
        .map(|_| vec![255; TERRAIN_SIDE * TERRAIN_SIDE * 4])
        .collect();
    let mut pages: Vec<Vec<u8>> = plan
        .pages
        .iter()
        .map(|p| vec![255; (p[0] * p[1] * 4) as usize])
        .collect();
    let mut rays = 0;
    let mut over = 0;
    for (job, d) in jobs.iter().zip(&done) {
        rays += d.rays;
        over += usize::from(d.over);
        let mut it = d.texels.iter();
        match job {
            Job::Terrain { map, rows } => {
                let out = &mut terrains[*map];
                for j in rows.0..rows.1 {
                    for i in 0..TERRAIN_SIDE {
                        if let Some(t) = it.next() {
                            let k = (j * TERRAIN_SIDE + i) * 4;
                            out[k..k + 3].copy_from_slice(t);
                        }
                    }
                }
            }
            Job::Rects { instance, rects } => {
                let inst = &inputs.instances[*instance];
                for &ri in rects {
                    let r = &inst.local.rects[ri];
                    let Some(p) = plan.places[*instance][ri] else {
                        continue;
                    };
                    let w = plan.pages[p.page as usize][0];
                    let page = &mut pages[p.page as usize];
                    let mut rect = Vec::with_capacity((r.size[0] * r.size[1]) as usize);
                    for _ in 0..r.size[0] * r.size[1] {
                        rect.push(it.next().copied().unwrap_or([255; 3]));
                    }
                    // The rectangle and its gutter (each gutter texel its nearest edge texel's value).
                    for j in 0..r.size[1] + 2 {
                        for i in 0..r.size[0] + 2 {
                            let si = i.saturating_sub(1).min(r.size[0] - 1);
                            let sj = j.saturating_sub(1).min(r.size[1] - 1);
                            let v = rect[(sj * r.size[0] + si) as usize];
                            let k = (((p.y + j) * w + p.x + i) * 4) as usize;
                            page[k..k + 3].copy_from_slice(&v);
                        }
                    }
                }
            }
        }
    }
    let texels = plan.texels(inputs);
    let lit = Lit {
        key,
        pages: plan
            .pages
            .iter()
            .zip(pages)
            .enumerate()
            .map(|(n, (s, texels))| texture(format!("map lightmap {key:016x} {n}"), *s, texels))
            .collect(),
        terrains: terrains
            .into_iter()
            .enumerate()
            .map(|(n, texels)| {
                texture(
                    format!("map terrain lightmap {key:016x} {n}"),
                    [TERRAIN_SIDE as u32; 2],
                    texels,
                )
            })
            .collect(),
        plan,
    };
    let report = Report {
        key,
        source: Source::Computed,
        seconds: 0.0,
        texels,
        rays,
        casters: world.bvh.len(),
        pages: lit.pages.len(),
        terrains: lit.terrains.len(),
        over_budget: over,
        digest: 0,
    };
    (lit, report)
}

fn texture(path: String, size: [u32; 2], texels: Vec<u8>) -> Arc<Texture> {
    Arc::new(Texture {
        path,
        width: size[0],
        height: size[1],
        levels: Arc::new(vec![texels]),
    })
}

// ---- the disk cache

/// The bytes a cache file for `plan` holds.
fn file_size(plan: &Plan) -> u64 {
    let mut n = 8 + 4 + 8 + 4 + 4;
    for p in &plan.pages {
        n += 8 + u64::from(p[0]) * u64::from(p[1]) * 3;
    }
    n += plan.terrains.len() as u64 * (8 + (TERRAIN_SIDE * TERRAIN_SIDE * 3) as u64);
    n
}

pub(crate) fn encode(lit: &Lit) -> Vec<u8> {
    let mut out = Vec::with_capacity(file_size(&lit.plan) as usize);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&lit.key.to_le_bytes());
    out.extend_from_slice(&(lit.pages.len() as u32).to_le_bytes());
    out.extend_from_slice(&(lit.terrains.len() as u32).to_le_bytes());
    for t in lit.pages.iter().chain(&lit.terrains) {
        out.extend_from_slice(&t.width.to_le_bytes());
        out.extend_from_slice(&t.height.to_le_bytes());
        if let Some(l) = t.levels.first() {
            for px in l.chunks_exact(4) {
                out.extend_from_slice(&px[..3]);
            }
        }
    }
    out
}

/// A cache file's lightmaps when it is exactly what `plan` under `key` needs.
pub(crate) fn decode(bytes: &[u8], key: u64, plan: &Plan) -> Option<Lit> {
    if bytes.len() as u64 != file_size(plan) {
        return None;
    }
    let mut at = 0usize;
    let mut take = |n: usize| -> Option<&[u8]> {
        let s = bytes.get(at..at + n)?;
        at += n;
        Some(s)
    };
    let word = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    if take(8)? != MAGIC || word(take(4)?) != VERSION {
        return None;
    }
    let k = take(8)?;
    if u64::from_le_bytes(std::array::from_fn(|i| k[i])) != key {
        return None;
    }
    if word(take(4)?) as usize != plan.pages.len() || word(take(4)?) as usize != plan.terrains.len()
    {
        return None;
    }
    let sizes: Vec<[u32; 2]> = plan
        .pages
        .iter()
        .copied()
        .chain(std::iter::repeat_n(
            [TERRAIN_SIDE as u32; 2],
            plan.terrains.len(),
        ))
        .collect();
    let mut made = Vec::new();
    for (n, s) in sizes.iter().enumerate() {
        if word(take(4)?) != s[0] || word(take(4)?) != s[1] {
            return None;
        }
        let rgb = take((s[0] * s[1] * 3) as usize)?;
        let mut texels = Vec::with_capacity((s[0] * s[1] * 4) as usize);
        for px in rgb.chunks_exact(3) {
            texels.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
        let name = if n < plan.pages.len() {
            format!("map lightmap {key:016x} {n}")
        } else {
            format!("map terrain lightmap {key:016x} {}", n - plan.pages.len())
        };
        made.push(texture(name, *s, texels));
    }
    let terrains = made.split_off(plan.pages.len());
    Some(Lit {
        key,
        plan: plan.clone(),
        pages: made,
        terrains,
    })
}

fn read_file(path: &std::path::Path, key: u64, plan: &Plan) -> Option<Lit> {
    let want = file_size(plan);
    if want > MAX_FILE_BYTES || std::fs::metadata(path).ok()?.len() != want {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    decode(&bytes, key, plan)
}

fn write_file(dir: &std::path::Path, lit: &Lit) {
    if file_size(&lit.plan) > MAX_FILE_BYTES || std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = cache_file(dir, lit.key);
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, encode(lit)).is_err() || std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    // The oldest files past [`DISK_KEPT`] go.
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "tml"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() > DISK_KEPT {
        files.sort();
        for (_, p) in &files[..files.len() - DISK_KEPT] {
            let _ = std::fs::remove_file(p);
        }
    }
}

// ---- the checks' readings

/// How high over the terrain a roof is (the roof check's): the lowest and the highest.
const ROOF_LOWEST: f32 = 2.0;
const ROOF_HIGHEST: f32 = 40.0;
/// How far over the terrain a building's lowest point may be to stand on it (the roof check's).
const STANDING: f32 = 1.0;

/// The plan's roof check: the `count` interiors with the largest footprints that stand on the terrain (their lowest point at
/// most a metre over it at their middle) and whose middle and the four points a terrain texel away from it a ray straight
/// down meets from 2 to 40 metres over the terrain (a solid roof, not a floor on the ground, a beam, a deck on stilts or a
/// structure in the sky), each read with [`terrain_reading`] at that middle.
pub(crate) fn roofs(lit: &Lit, inputs: &Inputs, count: usize) -> Vec<[f32; 3]> {
    let Some(t) = inputs.terrains.first() else {
        return Vec::new();
    };
    let ground = Ground::new(t.clone());
    let bvh = Bvh::new(casters(inputs));
    let mut boxes: Vec<([f32; 3], [f32; 3])> =
        inputs.instances.iter().filter_map(world_bounds).collect();
    let area = |b: &([f32; 3], [f32; 3])| (b.1[0] - b.0[0]) * (b.1[1] - b.0[1]);
    boxes.sort_by(|a, b| area(b).total_cmp(&area(a)));
    let texel = t.square * SIZE as f32 / TERRAIN_SIDE as f32;
    let roofed = |x: f32, y: f32| -> bool {
        let Some((z, _)) = ground.surface(x, y) else {
            return false;
        };
        let top = z + ROOF_HIGHEST;
        let mut work = u64::MAX;
        bvh.hits(
            [x, y, top],
            [0.0, 0.0, -1.0],
            ROOF_HIGHEST - ROOF_LOWEST,
            &mut work,
        )
    };
    let mut out = Vec::new();
    for (min, max) in boxes {
        if out.len() >= count {
            break;
        }
        let (x, y) = ((min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5);
        // A building standing on the terrain (not a deck on stilts the sun shines under).
        let standing = ground
            .surface(x, y)
            .is_some_and(|(z, _)| min[2] <= z + STANDING);
        let around = [
            (0.0, 0.0),
            (texel, 0.0),
            (-texel, 0.0),
            (0.0, texel),
            (0.0, -texel),
        ];
        if !standing || !around.iter().all(|(dx, dy)| roofed(x + dx, y + dy)) {
            continue;
        }
        if let Some(r) = terrain_reading(lit, inputs, x, y) {
            out.push(r);
        }
    }
    out
}

/// The terrain lightmap's texel under (x, y) and what an open texel with the same normal holds (the checks' "under a roof"
/// reading): (lit, open, ambient) as luminances. None off the terrain or without a lit terrain.
pub(crate) fn terrain_reading(lit: &Lit, inputs: &Inputs, x: f32, y: f32) -> Option<[f32; 3]> {
    let t = inputs.terrains.first()?;
    let tex = &lit.terrains[lit.plan.terrain_map(0, tile_of(t, x, y))?];
    let g = Ground::new(t.clone());
    let (_, n) = g.surface(x, y)?;
    let per = t.square * SIZE as f32 / TERRAIN_SIDE as f32;
    let i = ((x - t.origin[0]) / per).floor() as i64;
    let j = ((y - t.origin[1]) / per).floor() as i64;
    let w = |v: i64| v.rem_euclid(TERRAIN_SIDE as i64) as usize;
    let k = (w(j) * TERRAIN_SIDE + w(i)) * 4;
    let px = tex.levels.first()?.get(k..k + 3)?;
    let lum = |c: [f32; 3]| 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
    let here = lum([0, 1, 2].map(|c| f32::from(px[c]) / 255.0));
    let s = &inputs.sun;
    let ndotl = Vec3::from(n).dot(unit_or_up(s.towards.into())).max(0.0);
    let open = lum(std::array::from_fn(|c| {
        (s.ambient[c] + s.diffuse[c] * ndotl).min(1.0)
    }));
    let ambient = lum(std::array::from_fn(|c| s.ambient[c].min(1.0)));
    Some([here, open, ambient])
}
