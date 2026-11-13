//! The voxels' sources: the static triangles (the cells' opaque batches and the opaque planes), each source's kept on the CPU and
//! in one GPU buffer (`gi_voxel.wgsl`'s `Tri`, [`TRI_BYTES`] each), binned by where they lie so a tile finds the triangles near it
//! in a few hash lookups, and compared bin by bin when a source changes so only the boxes of the bins that changed are made
//! again (a cell remeshed for one block dirties about that block, not the whole cell).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use torque_render_core::wgpu;
use torque_render_scene::hd::cells::{Cell, SlotTexture};
use torque_render_scene::hd::{HdDraw, HdVertex, TextureTable, material, oct_decode};
use torque_render_scene::{Blend, Texture};

use crate::arena::RangeAlloc;

/// A triangle's bytes on the GPU: three corners (vec4 each, their w the albedo, the emission and a spare word).
pub(crate) const TRI_BYTES: u64 = 48;
/// A bin's side (units): triangles are binned by their middle.
const BIN: f32 = 2.0;
/// The hash grid's cell side (units) the bins are found through.
const HASH: f32 = 4.0;
/// A bin whose box is wider than this on any axis is kept in a list every query reads (a ground's long triangles would fill
/// thousands of hash cells).
const BIG: f32 = 32.0;
/// The GPU buffer's first size (triangles).
const FIRST_CAPACITY: u64 = 1 << 16;

/// A box: its low and high corners.
pub(crate) type Aabb = [[f32; 3]; 2];

/// A triangle as the GPU reads it: corners a, b, c (the winding agreeing with the vertices' normal), the albedo (rgba8 of linear
/// light), the emission (rgb9e5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GiTri {
    pub(crate) corners: [[f32; 3]; 3],
    pub(crate) albedo: u32,
    pub(crate) emission: u32,
}

impl GiTri {
    fn words(&self) -> [u32; 12] {
        let c = self.corners;
        [
            c[0][0].to_bits(),
            c[0][1].to_bits(),
            c[0][2].to_bits(),
            self.albedo,
            c[1][0].to_bits(),
            c[1][1].to_bits(),
            c[1][2].to_bits(),
            self.emission,
            c[2][0].to_bits(),
            c[2][1].to_bits(),
            c[2][2].to_bits(),
            0,
        ]
    }

    fn bounds(&self) -> Aabb {
        let c = self.corners;
        [
            std::array::from_fn(|k| c[0][k].min(c[1][k]).min(c[2][k])),
            std::array::from_fn(|k| c[0][k].max(c[1][k]).max(c[2][k])),
        ]
    }

    fn bin(&self) -> [i32; 3] {
        let c = self.corners;
        std::array::from_fn(|k| ((c[0][k] + c[1][k] + c[2][k]) / (3.0 * BIN)).floor() as i32)
    }
}

/// Which source a triangle came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum SourceKey {
    Cell([i32; 3]),
    Planes,
}

/// One bin of a source: its key, its triangles (`first .. first + count` of the source's), their box and a hash of their words.
#[derive(Debug, Clone, Copy)]
struct Bin {
    key: [i32; 3],
    first: u32,
    count: u32,
    bounds: Aabb,
    hash: u64,
}

/// A source's triangles (sorted by bin), its bins, where they sit in the GPU buffer, and what it was made from.
struct Source {
    stamp: u64,
    tris: Vec<GiTri>,
    bins: Vec<Bin>,
    base: u64,
}

/// A bin of a source, found through the hash grid.
type BinRef = (SourceKey, u32);

/// The sources, their GPU buffer and the boxes changed since the last [`Sources::take_dirty`].
pub(crate) struct Sources {
    sources: BTreeMap<SourceKey, Source>,
    alloc: RangeAlloc,
    buffer: wgpu::Buffer,
    /// Bumped each time the buffer is made anew (the groups that bind it are made again).
    pub(crate) buffer_generation: u64,
    grid: HashMap<[i32; 3], Vec<BinRef>>,
    big: BTreeSet<BinRef>,
    dirty: Vec<Aabb>,
    /// Each texture's mean color in linear light, by its index in the frame's table.
    means: HashMap<usize, [f32; 3]>,
    /// The most triangles the buffer may hold: the device's largest storage binding (and buffer) over a triangle's bytes.
    cap: u64,
    /// The triangles of the sources left out because the buffer could not hold them, and whether that was said.
    pub(crate) refused: u64,
    warned: bool,
}

fn fnv(h: u64, w: u32) -> u64 {
    (h ^ u64::from(w)).wrapping_mul(0x0100_0000_01b3)
}

fn srgb_to_linear(c: f32) -> f32 {
    torque_render_scene::hd::srgb_to_linear(c)
}

/// A texture's mean color in linear light (its smallest level's texels).
fn texture_mean(t: &Texture) -> [f32; 3] {
    let Some(level) = t.levels.last() else {
        return [1.0; 3];
    };
    let mut sum = [0.0f64; 3];
    let mut n = 0u32;
    for px in level.as_chunks::<4>().0 {
        for k in 0..3 {
            sum[k] += f64::from(srgb_to_linear(f32::from(px[k]) / 255.0));
        }
        n += 1;
    }
    if n == 0 {
        return [1.0; 3];
    }
    sum.map(|s| (s / f64::from(n)) as f32)
}

/// rgb9e5 of a linear color (`gi_common.wgsl`'s `gi_pack_rgb9e5`).
pub(crate) fn pack_rgb9e5(c: [f32; 3]) -> u32 {
    let c = c.map(|v| {
        if v.is_finite() {
            v.clamp(0.0, 65408.0)
        } else {
            0.0
        }
    });
    let m = c[0].max(c[1]).max(c[2]);
    if m < 1.0 / 16_777_216.0 {
        return 0;
    }
    let mut e = (m.log2().floor() as i32).max(-16) + 16;
    let mut denom = 2f32.powi(e - 24);
    if (m / denom + 0.5).floor() >= 512.0 {
        e += 1;
        denom *= 2.0;
    }
    let q = c.map(|v| ((v / denom + 0.5).floor().min(511.0)) as u32);
    q[0] | (q[1] << 9) | (q[2] << 18) | ((e as u32) << 27)
}

/// The color an rgb9e5 word holds.
#[cfg(test)]
pub(crate) fn unpack_rgb9e5(v: u32) -> [f32; 3] {
    let scale = 2f32.powi((v >> 27) as i32 - 24);
    [v & 511, (v >> 9) & 511, (v >> 18) & 511].map(|m| m as f32 * scale)
}

/// What a vertex's material and color give a triangle: its albedo and emission in linear light, or None when it is left out
/// of the voxels (the sky, particles, billboards).
fn surface(v: &HdVertex, texture: Option<[f32; 3]>) -> Option<([f32; 3], [f32; 3])> {
    let word = v.material;
    let kind = (word >> material::KIND_SHIFT) & 0x3;
    if kind == material::KIND_SKY
        || word & (material::PARTICLE | material::BILLBOARD | material::EMITTER) != 0
        || material::layer(word)
        || material::shadow_only(word)
    {
        return None;
    }
    let paint = [0, 1, 2].map(|k| srgb_to_linear(f32::from(v.color[k]) / 255.0));
    let env = (word >> material::TEX_ENV_SHIFT) & material::TEX_ENV_MASK;
    let base = match (env, texture) {
        (1, Some(t)) => std::array::from_fn(|k| 0.5 * (paint[k] + t[k])),
        (2, Some(t)) => std::array::from_fn(|k| paint[k] * t[k]),
        (3, Some(t)) => t,
        _ => paint,
    };
    if word & material::GLOW != 0 {
        return Some(([0.0; 3], base));
    }
    if word & material::UNLIT != 0 {
        return Some(([0.0; 3], [0.0; 3]));
    }
    Some((base, [0.0; 3]))
}

/// The triangles of `indices` over `vertices`, each with the surface of its first vertex.
fn triangles_of(
    vertices: &[HdVertex],
    indices: &[u32],
    texture: Option<[f32; 3]>,
    out: &mut Vec<GiTri>,
) {
    for t in indices.as_chunks::<3>().0 {
        let [Some(v0), Some(v1), Some(v2)] = t.map(|i| vertices.get(i as usize)) else {
            continue;
        };
        let v = [v0, v1, v2];
        let Some((albedo, emission)) = surface(v[0], texture) else {
            continue;
        };
        let mut corners = [v[0].position, v[1].position, v[2].position];
        if corners.iter().flatten().any(|c| !c.is_finite()) {
            continue;
        }
        let e1 = glam::Vec3::from(corners[1]) - glam::Vec3::from(corners[0]);
        let e2 = glam::Vec3::from(corners[2]) - glam::Vec3::from(corners[0]);
        let n = e1.cross(e2);
        if n.length_squared() < 1e-14 {
            continue;
        }
        // The winding turned to agree with the vertices' normal (a voxel's normal is its triangle's).
        let shading = glam::Vec3::from(oct_decode(v[0].normal));
        if v[0].material & material::ZERO_NORMAL == 0 && n.dot(shading) < 0.0 {
            corners.swap(1, 2);
        }
        let a = albedo.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u32);
        out.push(GiTri {
            corners,
            albedo: a[0] | (a[1] << 8) | (a[2] << 16) | (255 << 24),
            emission: pack_rgb9e5(emission),
        });
    }
}

/// Whether two boxes overlap.
fn meets(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|k| a[0][k] <= b[1][k] && b[0][k] <= a[1][k])
}

/// The hash cells a box covers.
fn hash_cells(b: &Aabb) -> impl Iterator<Item = [i32; 3]> {
    let lo = b[0].map(|c| (c / HASH).floor() as i32);
    let hi = b[1].map(|c| (c / HASH).floor() as i32);
    (lo[2]..=hi[2]).flat_map(move |z| {
        (lo[1]..=hi[1]).flat_map(move |y| (lo[0]..=hi[0]).map(move |x| [x, y, z]))
    })
}

fn is_big(b: &Aabb) -> bool {
    (0..3).any(|k| b[1][k] - b[0][k] > BIG)
}

impl Sources {
    pub(crate) fn new(device: &wgpu::Device) -> Sources {
        let limits = device.limits();
        let cap = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size)
            / TRI_BYTES;
        let first = FIRST_CAPACITY.min(cap).max(1);
        Sources {
            sources: BTreeMap::new(),
            alloc: RangeAlloc::new(first),
            buffer: tri_buffer(device, first),
            buffer_generation: 0,
            grid: HashMap::new(),
            big: BTreeSet::new(),
            dirty: Vec::new(),
            means: HashMap::new(),
            cap,
            refused: 0,
            warned: false,
        }
    }

    /// Forgets every source (the buffer kept for the next).
    pub(crate) fn clear(&mut self) {
        self.sources.clear();
        self.alloc = RangeAlloc::new(self.alloc.capacity());
        self.grid.clear();
        self.big.clear();
        self.dirty.clear();
        self.means.clear();
    }

    pub(crate) fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    /// The triangles kept, over every source.
    pub(crate) fn triangles(&self) -> usize {
        self.sources.values().map(|s| s.tris.len()).sum()
    }

    /// The boxes changed since the last call.
    pub(crate) fn take_dirty(&mut self) -> Vec<Aabb> {
        std::mem::take(&mut self.dirty)
    }

    /// Brings the sources to the scene's cells and planes: a cell gone, a cell of a new generation, the planes of a new
    /// generation (whose triangles' hash changed) are taken out or made again, their changed bins' boxes marked dirty.
    #[expect(
        clippy::too_many_arguments,
        reason = "the scene's parts a sync reads, each borrowed"
    )]
    pub(crate) fn sync<X>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cells: &BTreeMap<[i32; 3], Cell<X>>,
        slots: &BTreeMap<u32, SlotTexture>,
        planes: &[HdDraw],
        planes_generation: u64,
        textures: &TextureTable,
    ) {
        let gone: Vec<SourceKey> = self
            .sources
            .keys()
            .filter(|k| match k {
                SourceKey::Cell(key) => cells
                    .get(key)
                    .is_none_or(|c| c.indices.is_empty() && !c.resident),
                SourceKey::Planes => false,
            })
            .copied()
            .collect();
        for k in gone {
            self.replace(device, queue, k, None);
        }
        for (key, c) in cells {
            if c.indices.is_empty() {
                continue;
            }
            let k = SourceKey::Cell(*key);
            if self
                .sources
                .get(&k)
                .is_some_and(|s| s.stamp == c.generation)
            {
                continue;
            }
            let mut tris = Vec::new();
            for b in c.batches.iter().filter(|b| !b.translucent) {
                let texture = slots
                    .get(&b.slot)
                    .and_then(|s| s.texture)
                    .and_then(|i| self.mean(textures, i));
                let end = (b.first + b.count) as usize;
                if let Some(ix) = c.indices.get(b.first as usize..end) {
                    triangles_of(&c.vertices, ix, texture, &mut tris);
                }
            }
            self.replace(device, queue, k, Some((c.generation, tris)));
        }
        if self
            .sources
            .get(&SourceKey::Planes)
            .is_none_or(|s| s.stamp != planes_generation)
        {
            let mut tris = Vec::new();
            for d in planes.iter().filter(|d| d.blend == Blend::Opaque) {
                let texture = d.texture.and_then(|(i, _)| self.mean(textures, i));
                triangles_of(&d.vertices, &d.indices, texture, &mut tris);
            }
            self.replace(
                device,
                queue,
                SourceKey::Planes,
                Some((planes_generation, tris)),
            );
        }
    }

    fn mean(&mut self, textures: &TextureTable, i: usize) -> Option<[f32; 3]> {
        let t = textures.textures.get(i)?;
        Some(*self.means.entry(i).or_insert_with(|| texture_mean(t)))
    }

    /// Puts `new` (its stamp and triangles) in `key`'s place (None: takes the source out), marking dirty each bin whose
    /// triangles changed (its old and new boxes). A source the buffer cannot hold within the device's limits is left out (the
    /// old one, if any, stays) and counted in [`Sources::refused`]; the first is reported.
    fn replace(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: SourceKey,
        new: Option<(u64, Vec<GiTri>)>,
    ) {
        if let Some((_, tris)) = &new {
            let old = self.sources.get(&key).map_or(0, |s| s.tris.len() as u64);
            if self.alloc.used() - old + tris.len() as u64 > self.cap {
                self.refused += tris.len() as u64;
                if !self.warned {
                    self.warned = true;
                    eprintln!(
                        "global illumination: {} static triangles past the device's {} for the voxels; the rest are left out",
                        self.alloc.used() - old + tris.len() as u64,
                        self.cap
                    );
                }
                return;
            }
        }
        let made = new.map(|(stamp, mut tris)| {
            // Sorted by bin, stable: the same triangles give the same order.
            tris.sort_by_key(GiTri::bin);
            let mut bins: Vec<Bin> = Vec::new();
            for (i, t) in tris.iter().enumerate() {
                let key = t.bin();
                let b = t.bounds();
                match bins.last_mut() {
                    Some(last) if last.key == key => {
                        last.count += 1;
                        last.bounds = [
                            std::array::from_fn(|k| last.bounds[0][k].min(b[0][k])),
                            std::array::from_fn(|k| last.bounds[1][k].max(b[1][k])),
                        ];
                        last.hash = t.words().into_iter().fold(last.hash, fnv);
                    }
                    _ => bins.push(Bin {
                        key,
                        first: i as u32,
                        count: 1,
                        bounds: b,
                        hash: t.words().into_iter().fold(0xcbf2_9ce4_8422_2325, fnv),
                    }),
                }
            }
            (stamp, tris, bins)
        });
        let old = self.sources.remove(&key);
        // What changed: each bin key's old and new hash.
        let mut changed: BTreeMap<[i32; 3], (Option<Bin>, Option<Bin>)> = BTreeMap::new();
        if let Some(o) = &old {
            for b in &o.bins {
                changed.entry(b.key).or_default().0 = Some(*b);
            }
        }
        if let Some((_, _, bins)) = &made {
            for b in bins {
                changed.entry(b.key).or_default().1 = Some(*b);
            }
        }
        // A changed bin marks the triangles it lost and gained (not its whole box: a ground's long triangle in it would mark
        // the ground).
        let old_tris = old.as_ref().map_or(&[][..], |o| &o.tris[..]);
        let new_tris = made.as_ref().map_or(&[][..], |m| &m.1[..]);
        for (a, b) in changed.values() {
            if let (Some(a), Some(b)) = (a, b)
                && a.hash == b.hash
                && a.count == b.count
            {
                continue;
            }
            let of = |bin: &Option<Bin>, tris: &[GiTri]| -> BTreeMap<[u32; 12], (usize, Aabb)> {
                let mut m: BTreeMap<[u32; 12], (usize, Aabb)> = BTreeMap::new();
                if let Some(bin) = bin {
                    let range = bin.first as usize..(bin.first + bin.count) as usize;
                    for t in &tris[range] {
                        m.entry(t.words()).or_insert((0, t.bounds())).0 += 1;
                    }
                }
                m
            };
            let (was, now) = (of(a, old_tris), of(b, new_tris));
            for (words, (n, bounds)) in &was {
                if now.get(words).is_none_or(|m| m.0 != *n) {
                    self.dirty.push(*bounds);
                }
            }
            for (words, (n, bounds)) in &now {
                if was.get(words).is_none_or(|m| m.0 != *n) {
                    self.dirty.push(*bounds);
                }
            }
        }
        if let Some(o) = old {
            for (i, b) in o.bins.iter().enumerate() {
                self.unlist(&b.bounds, (key, i as u32));
            }
            self.alloc.free(o.base, o.tris.len() as u64);
        }
        let Some((stamp, tris, bins)) = made else {
            return;
        };
        let len = tris.len() as u64;
        let base = if len == 0 {
            0
        } else {
            match self.alloc.alloc(len) {
                Some(at) => at,
                None => {
                    // Grown (within the device's limits), the sources packed from its start: the new one fits, as the
                    // count was checked against the cap.
                    let capacity = (self.alloc.capacity() * 2)
                        .max(self.alloc.used() + len)
                        .min(self.cap);
                    self.buffer = tri_buffer(device, capacity);
                    self.buffer_generation += 1;
                    self.alloc = RangeAlloc::new(capacity);
                    for s in self.sources.values_mut() {
                        s.base = self.alloc.alloc(s.tris.len() as u64).unwrap_or_default();
                        write_tris(queue, &self.buffer, s.base, &s.tris);
                    }
                    self.alloc
                        .alloc(len)
                        .expect("a packed buffer within the cap holds the source")
                }
            }
        };
        write_tris(queue, &self.buffer, base, &tris);
        for (i, b) in bins.iter().enumerate() {
            self.list(&b.bounds, (key, i as u32));
        }
        self.sources.insert(
            key,
            Source {
                stamp,
                tris,
                bins,
                base,
            },
        );
    }

    fn list(&mut self, b: &Aabb, r: BinRef) {
        if is_big(b) {
            self.big.insert(r);
            return;
        }
        for c in hash_cells(b) {
            self.grid.entry(c).or_default().push(r);
        }
    }

    fn unlist(&mut self, b: &Aabb, r: BinRef) {
        if is_big(b) {
            self.big.remove(&r);
            return;
        }
        for c in hash_cells(b) {
            if let Some(v) = self.grid.get_mut(&c) {
                v.retain(|x| *x != r);
                if v.is_empty() {
                    self.grid.remove(&c);
                }
            }
        }
    }

    /// The GPU buffer's triangles (their places, ascending) whose boxes meet `b` (found through the bins that meet it).
    pub(crate) fn triangles_in(&self, b: &Aabb) -> Vec<u32> {
        let mut refs: BTreeSet<BinRef> = BTreeSet::new();
        for c in hash_cells(b) {
            if let Some(v) = self.grid.get(&c) {
                refs.extend(v.iter().copied());
            }
        }
        refs.extend(self.big.iter().copied());
        let mut out: Vec<u32> = Vec::new();
        for (key, i) in refs {
            let Some(s) = self.sources.get(&key) else {
                continue;
            };
            let Some(bin) = s.bins.get(i as usize).filter(|bin| meets(&bin.bounds, b)) else {
                continue;
            };
            let first = bin.first as usize;
            for (k, t) in s.tris[first..first + bin.count as usize].iter().enumerate() {
                if meets(&t.bounds(), b) {
                    out.push((s.base as usize + first + k) as u32);
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// Every triangle's corners (the CPU reference's input).
    #[cfg(test)]
    pub(crate) fn corners(&self) -> Vec<[glam::Vec3; 3]> {
        self.sources
            .values()
            .flat_map(|s| s.tris.iter().map(|t| t.corners.map(glam::Vec3::from)))
            .collect()
    }
}

fn tri_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hd gi triangles"),
        size: capacity * TRI_BYTES,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn write_tris(queue: &wgpu::Queue, buffer: &wgpu::Buffer, base: u64, tris: &[GiTri]) {
    if tris.is_empty() {
        return;
    }
    let mut bytes = Vec::with_capacity(tris.len() * TRI_BYTES as usize);
    for t in tris {
        for w in t.words() {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
    }
    queue.write_buffer(buffer, base * TRI_BYTES, &bytes);
}
