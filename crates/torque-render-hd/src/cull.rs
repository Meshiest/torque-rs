//! The static geometry's clusters culled on the GPU (`cull.wgsl`) into a view's draw lists ([`crate::arena::Lists`]):
//!
//! - **Views** ([`ViewPlanes`]): up to eight planes made from a view's world-to-clip matrix (a frustum; a sun cascade's square and far
//!   side, with a patch's rectangle; a point light's cube face with its sphere). A clip-space plane is a plane in world space, so a
//!   box whose eight corners all lie behind one is not seen by the view at all: what is culled could draw no fragment there. The
//!   test keeps a margin for the planes' rounding, so the lists are a little larger than exact, never smaller. The CPU's lists
//!   ([`crate::arena::Lists::write_cpu`], the reference and the fallback) use the same test ([`ViewPlanes::seen`]).
//! - **Cells** ([`CellTable`]): each cell's box by its table place; `cs_cells` marks the cells a view sees and those within its
//!   near distance (the near-only clusters are drawn there), `cs_clusters` a slot's pass flag, then the compaction.
//! - **The report**: the main view's cells seen and draws listed, summed on the GPU and read back without a wait, a frame or
//!   more late (the reports only).
//! - **The grid** ([`CellGrid`]): the CPU's questions about every cell (the shadows' range, a reflective cell in sight, the cells
//!   near the eye) answered from blocks of cells, looking inside only the blocks that can change the answer.

use std::collections::{BTreeMap, BTreeSet};

use glam::Vec3;

use torque_render_core::wgpu;

use crate::arena::{Lists, MAX_SEGMENTS, Table};

/// The most planes of a view.
pub const MAX_PLANES: usize = 8;
/// What a view draws: the opaque clusters, the translucent ones, the near-only clusters (within its near distance).
pub const VIEW_OPAQUE: u32 = 1;
pub const VIEW_TRANSLUCENT: u32 = 2;
pub const VIEW_NEAR: u32 = 4;
/// The view's lists as one list an arena page, each with one count (a depth-only shadow map's, whose depth does not depend on its
/// draws' order: a draw a page in place of one a segment; [`Cull::page_lists`]).
pub const VIEW_WHOLE: u32 = 8;
/// The most arena pages a whole list covers (an arena of more draws its maps' lists a segment at a time).
pub const MAX_PAGES: usize = 64;
/// The views a frame can cull (each a uniform slot).
pub const MAX_VIEWS: u32 = 256;
const VIEW_STRIDE: u64 = 512;
const VIEW_BYTES: u64 = 272;
/// The farthest-depth pyramid's levels (its first level the frame padded to a multiple of the coarsest texel, 128 pixels).
pub const HIZ_LEVELS: u32 = 8;

/// A view the cull makes lists for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewPlanes {
    pub planes: [[f32; 4]; MAX_PLANES],
    pub count: u32,
    /// A sphere a box must meet (radius above 0), else none.
    pub sphere: [f32; 4],
    /// The eye, and its near distance (the near-only clusters are drawn within it; below 0: nowhere).
    pub eye: [f32; 4],
    pub kinds: u32,
    /// The main view's phase (0 a plain view), its world-to-clip matrix and frame size (the occlusion test).
    pub phase: u32,
    pub view_proj: [f32; 16],
    pub frame: [u32; 2],
    /// The levels of detail ([`lod_selected`]): the distances past which L1 and L2 are drawn and whether they are (1), seen from
    /// `eye`; off (0) for every view but those of a frame that draws them.
    pub lod: [f32; 4],
}

fn row(m: &[f32; 16], r: usize) -> [f32; 4] {
    [m[r], m[4 + r], m[8 + r], m[12 + r]]
}

fn plus(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}

fn minus(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]]
}

fn times(a: [f32; 4], k: f32) -> [f32; 4] {
    a.map(|v| v * k)
}

impl ViewPlanes {
    fn of(planes: &[[f32; 4]], sphere: [f32; 4], eye: [f32; 4], kinds: u32) -> ViewPlanes {
        let mut p = [[0.0; 4]; MAX_PLANES];
        let n = planes.len().min(MAX_PLANES);
        p[..n].copy_from_slice(&planes[..n]);
        ViewPlanes {
            planes: p,
            count: n as u32,
            sphere,
            eye,
            kinds,
            phase: 0,
            view_proj: [0.0; 16],
            frame: [0; 2],
            lod: [0.0; 4],
        }
    }

    /// The view drawing the levels of detail past `d1` (L1) and `d2` (L2) from its eye.
    pub fn with_lod(&self, d1: f32, d2: f32) -> ViewPlanes {
        ViewPlanes {
            lod: [d1, d2, 1.0, 0.0],
            ..*self
        }
    }

    /// The main view in one of its two phases (1: the clusters seen last frame; 2: the rest, tested against the frame's
    /// farthest-depth pyramid of `frame` pixels).
    pub fn phased(&self, phase: u32, view_proj: &[f32; 16], frame: [u32; 2]) -> ViewPlanes {
        ViewPlanes {
            phase,
            view_proj: *view_proj,
            frame,
            ..*self
        }
    }

    /// A camera's frustum (clip x and y within w, depth 0 to w), its eye and near distance.
    pub fn frustum(
        view_proj: &[f32; 16],
        eye: [f32; 3],
        near: Option<f32>,
        kinds: u32,
    ) -> ViewPlanes {
        let [r0, r1, r2, r3] = [0, 1, 2, 3].map(|r| row(view_proj, r));
        ViewPlanes::of(
            &[
                plus(r3, r0),
                minus(r3, r0),
                plus(r3, r1),
                minus(r3, r1),
                r2,
                minus(r3, r2),
            ],
            [0.0; 4],
            [eye[0], eye[1], eye[2], near.unwrap_or(-1.0)],
            kinds,
        )
    }

    /// A sun cascade (an orthographic square): within its square across and not wholly beyond its far side (nearer the sun is
    /// pancaked); with `patch` (clip x0, y0, x1, y1, inside the square) only what may meet that rectangle.
    pub fn cascade(view_proj: &[f32; 16], patch: Option<[f32; 4]>, kinds: u32) -> ViewPlanes {
        let [r0, r1, r2, r3] = [0, 1, 2, 3].map(|r| row(view_proj, r));
        let [x0, y0, x1, y1] = patch.unwrap_or([-1.0, -1.0, 1.0, 1.0]);
        ViewPlanes::of(
            &[
                minus(r0, times(r3, x0)),
                minus(times(r3, x1), r0),
                minus(r1, times(r3, y0)),
                minus(times(r3, y1), r1),
                minus(r3, r2),
            ],
            [0.0; 4],
            [0.0, 0.0, 0.0, -1.0],
            kinds,
        )
    }

    /// A point light's cube face `m` (in front of the light, within its pyramid) and the light's sphere.
    pub fn cube_face(m: &[f32; 16], light: [f32; 3], radius: f32, kinds: u32) -> ViewPlanes {
        let [r0, r1, _, r3] = [0, 1, 2, 3].map(|r| row(m, r));
        ViewPlanes::of(
            &[r3, minus(r3, r0), plus(r3, r0), minus(r3, r1), plus(r3, r1)],
            [light[0], light[1], light[2], radius],
            [0.0, 0.0, 0.0, -1.0],
            kinds,
        )
    }

    /// Whether a box may be seen: `cull.wgsl`'s `box_seen`, the same arithmetic.
    pub fn seen(&self, lo: [f32; 3], hi: [f32; 3]) -> bool {
        let (lo, hi) = (Vec3::from(lo), Vec3::from(hi));
        let c = (lo + hi) * 0.5;
        let e = (hi - lo) * 0.5;
        for p in &self.planes[..(self.count as usize).min(MAX_PLANES)] {
            let n = Vec3::new(p[0], p[1], p[2]);
            let dc = n.dot(c);
            let s = dc + p[3];
            let r = n.abs().dot(e);
            let margin = 1e-5 * (dc.abs() + p[3].abs() + r);
            if s + r < -margin {
                return false;
            }
        }
        let radius = self.sphere[3];
        if radius > 0.0 {
            let d2: f32 = (0..3)
                .map(|k| {
                    let d = (lo[k] - self.sphere[k])
                        .max(self.sphere[k] - hi[k])
                        .max(0.0);
                    d * d
                })
                .sum();
            if d2 > radius * radius * (1.0 + 1e-5) {
                return false;
            }
        }
        true
    }

    /// Whether a box lies within the near distance (`cs_cells`).
    pub fn near(&self, lo: [f32; 3], hi: [f32; 3]) -> bool {
        let far = self.eye[3];
        let d2: f32 = (0..3)
            .map(|k| {
                let d = (lo[k] - self.eye[k]).max(self.eye[k] - hi[k]).max(0.0);
                d * d
            })
            .sum();
        far >= 0.0 && d2 <= far * far
    }

    fn write(&self, cells: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(VIEW_BYTES as usize);
        for p in &self.planes {
            for v in p {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        for v in self.sphere.iter().chain(&self.eye) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for w in [self.count, self.kinds, cells, self.phase] {
            out.extend_from_slice(&w.to_le_bytes());
        }
        for v in &self.view_proj {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for w in [self.frame[0], self.frame[1], HIZ_LEVELS, 0] {
            out.extend_from_slice(&w.to_le_bytes());
        }
        for v in &self.lod {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }
}

/// Whether a cluster passes a plain view's cull (`cs_cells` and `cs_clusters` at phase 0, the same tests): live, of a kind the
/// view draws, its cell seen (and within the near distance for a near-only one), its own box seen.
pub fn cpu_pass(view: &ViewPlanes, cells: &CellTable, r: &crate::arena::Record) -> bool {
    use crate::arena::{CLUSTER_LIVE, CLUSTER_NEAR, CLUSTER_TRANSLUCENT};
    if r.flags & CLUSTER_LIVE == 0 {
        return false;
    }
    let kind = if r.flags & CLUSTER_TRANSLUCENT != 0 {
        VIEW_TRANSLUCENT
    } else {
        VIEW_OPAQUE
    };
    if view.kinds & kind == 0 {
        return false;
    }
    let Some(c) = cells.mirror.get(r.cell as usize).filter(|c| c.live) else {
        return false;
    };
    if !view.seen(c.lo, c.hi) {
        return false;
    }
    // An opaque cluster only at its unit's level where the levels of detail are drawn (a translucent one at every distance).
    if kind == VIEW_OPAQUE && !lod_selected(view, c) {
        return false;
    }
    if r.flags & CLUSTER_NEAR != 0 && (view.kinds & VIEW_NEAR == 0 || !view.near(c.lo, c.hi)) {
        return false;
    }
    view.seen(r.lo, r.hi)
}

/// A unit's box as the cull reads it (`cull.wgsl`'s `CellBox`, 48 bytes): a cell (level 0) or a level of detail's block, its grid
/// key at its level, and whether the coarser levels' blocks holding it have meshes ([`LOD_HAS_L1`], [`LOD_HAS_L2`]).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CellBox {
    pub lo: [f32; 3],
    pub hi: [f32; 3],
    pub live: bool,
    pub key: [i32; 3],
    pub level: u32,
    pub lod: u32,
}

/// [`CellBox::lod`]'s bits: its L1 block has a mesh, its L2 block has one; its L1 block, and its L2 block, was drawn coarse at
/// the last choice (it stays so until the eye comes [`LOD_HYSTERESIS`] nearer than the switch: [`lod_coarse`]).
pub const LOD_HAS_L1: u32 = 1;
pub const LOD_HAS_L2: u32 = 2;
pub const LOD_STAY_L1: u32 = 4;
pub const LOD_STAY_L2: u32 = 8;

/// A block drawn coarse goes back to the finer level only once the eye is this share nearer than the distance it switched at: a
/// camera hovering at the switch (a walk's bob, a slow approach) does not flip the level back and forth.
pub const LOD_HYSTERESIS: f32 = 0.15;

/// Whether a block `d` units from the eye is drawn coarse at a level whose switch is `at` units, `stay` when it was at the last
/// choice. `cull.wgsl`'s `lod_coarse`, the same arithmetic.
pub fn lod_coarse(d: f32, at: f32, stay: bool) -> bool {
    d >= if stay {
        at * (1.0 - LOD_HYSTERESIS)
    } else {
        at
    }
}

/// The distance from `eye` to the grid box of block `key` (a side of `side` units), `cull.wgsl`'s arithmetic.
pub fn block_distance(eye: [f32; 3], key: [f32; 3], side: f32) -> f32 {
    let mut d2 = 0.0f32;
    for k in 0..3 {
        let lo = key[k] * side;
        let d = (lo - eye[k]).max(eye[k] - (lo + side)).max(0.0);
        d2 += d * d;
    }
    d2.sqrt()
}

/// Whether a unit's level is the one drawn where it stands: the level the distance asks for (L2 past `lod.y` from the L2 block,
/// else L1 past `lod.x` from the L1 block, else L0; each switch held back by [`lod_coarse`]'s hysteresis), made finer while that
/// level's block has no mesh; with the levels off, L0.
/// Every unit of a block decides alike (the same grid boxes), so the levels tile. `cull.wgsl`'s `lod_selected`, the same
/// arithmetic.
pub fn lod_selected(view: &ViewPlanes, c: &CellBox) -> bool {
    if view.lod[2] < 0.5 {
        return c.level == 0;
    }
    let eye = [view.eye[0], view.eye[1], view.eye[2]];
    let key = c.key.map(|k| k as f32);
    let up = |n: f32| key.map(|k| (k / n).floor());
    let side = torque_render_scene::hd::cells::CELL_SIZE;
    let (b1, b2) = match c.level {
        0 => (up(2.0), up(4.0)),
        1 => (key, up(2.0)),
        _ => (key, key),
    };
    let d2 = block_distance(eye, b2, 4.0 * side);
    let d1 = if c.level >= 2 {
        d2
    } else {
        block_distance(eye, b1, 2.0 * side)
    };
    let (has1, has2) = match c.level {
        0 => (c.lod & LOD_HAS_L1 != 0, c.lod & LOD_HAS_L2 != 0),
        1 => (true, c.lod & LOD_HAS_L2 != 0),
        _ => (true, true),
    };
    let mut chosen = if lod_coarse(d2, view.lod[1], c.lod & LOD_STAY_L2 != 0) {
        2
    } else if lod_coarse(d1, view.lod[0], c.lod & LOD_STAY_L1 != 0) {
        1
    } else {
        0
    };
    if chosen == 2 && !has2 {
        chosen = 1;
    }
    if chosen == 1 && !has1 {
        chosen = 0;
    }
    chosen == c.level
}

const CELL_BYTES: u64 = 48;
const FIRST_CELLS: u32 = 1024;

/// The cells' boxes by table place.
pub struct CellTable {
    pub buffer: wgpu::Buffer,
    pub mirror: Vec<CellBox>,
    dirty: BTreeSet<u32>,
    regrown: bool,
}

fn cell_buffer(device: &wgpu::Device, n: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hd static cells"),
        size: u64::from(n.max(1)) * CELL_BYTES,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn cell_bytes(c: &CellBox, out: &mut Vec<u8>) {
    for v in c.lo {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&c.lod.to_le_bytes());
    for v in c.hi {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&u32::from(c.live).to_le_bytes());
    for k in c.key {
        out.extend_from_slice(&k.to_le_bytes());
    }
    out.extend_from_slice(&c.level.to_le_bytes());
}

impl CellTable {
    pub fn new(device: &wgpu::Device) -> CellTable {
        CellTable {
            buffer: cell_buffer(device, FIRST_CELLS),
            mirror: vec![CellBox::default(); FIRST_CELLS as usize],
            dirty: BTreeSet::new(),
            regrown: false,
        }
    }

    pub fn capacity(&self) -> u32 {
        self.mirror.len() as u32
    }

    pub fn set(&mut self, id: u32, c: CellBox) {
        let i = id as usize;
        if i >= self.mirror.len() {
            let n = (self.mirror.len() * 2).max(i + 1);
            self.mirror.resize(n, CellBox::default());
            self.regrown = true;
        }
        self.mirror[i] = c;
        self.dirty.insert(id);
    }

    pub fn bytes(&self) -> u64 {
        self.buffer.size()
    }

    /// The changed boxes to the GPU (the buffer made again when the table grew).
    pub fn flush(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if std::mem::take(&mut self.regrown) {
            self.buffer = cell_buffer(device, self.capacity());
            let mut b = Vec::with_capacity(self.mirror.len() * CELL_BYTES as usize);
            for c in &self.mirror {
                cell_bytes(c, &mut b);
            }
            queue.write_buffer(&self.buffer, 0, &b);
            self.dirty.clear();
            return;
        }
        let dirty = std::mem::take(&mut self.dirty);
        let mut run: Option<(u32, u32)> = None;
        let write = |(a, b): (u32, u32)| {
            let mut bytes = Vec::with_capacity((b - a) as usize * CELL_BYTES as usize);
            for c in &self.mirror[a as usize..b as usize] {
                cell_bytes(c, &mut bytes);
            }
            queue.write_buffer(&self.buffer, u64::from(a) * CELL_BYTES, &bytes);
        };
        for s in dirty {
            run = match run {
                Some((a, b)) if b == s => Some((a, s + 1)),
                Some(r) => {
                    write(r);
                    Some((s, s + 1))
                }
                None => Some((s, s + 1)),
            };
        }
        if let Some(r) = run {
            write(r);
        }
    }
}

/// The report's readback slots.
const REPORT_RING: usize = 4;

/// The farthest-depth pyramid of the main view's phase-1 depth: for a frame size and depth target.
struct Hiz {
    key: (u32, u32, u64),
    view: wgpu::TextureView,
    first: wgpu::BindGroup,
    downs: Vec<(wgpu::BindGroup, (u32, u32))>,
    size: (u32, u32),
}

/// What a cull pass runs: the cells' test, the clusters' test, the compaction.
#[derive(Clone, Copy)]
struct Steps {
    cells: bool,
    clusters: bool,
    compact: bool,
}

const ALL_STEPS: Steps = Steps {
    cells: true,
    clusters: true,
    compact: true,
};

/// The buffers a cull pass reads and writes besides the tables: the flags it writes (and compacts), phase 1's flags, the seen
/// bits, phase 2's second flags, the pyramid, the report.
struct Io<'a> {
    flags: &'a wgpu::Buffer,
    prev: &'a wgpu::Buffer,
    vis: &'a wgpu::Buffer,
    flags2: &'a wgpu::Buffer,
    hiz: &'a wgpu::TextureView,
    report: &'a wgpu::Buffer,
}

/// The cull's pipelines and working buffers.
pub struct Cull {
    layout: wgpu::BindGroupLayout,
    cells_pipe: wgpu::ComputePipeline,
    clusters_pipe: wgpu::ComputePipeline,
    blocks_pipe: wgpu::ComputePipeline,
    top_pipe: wgpu::ComputePipeline,
    scatter_pipe: wgpu::ComputePipeline,
    counts_pipe: wgpu::ComputePipeline,
    page_offsets_pipe: wgpu::ComputePipeline,
    /// Each arena page's list in a whole list (its first slot and its slots: its segments' slots), this frame.
    page_lists: Vec<(u32, u32)>,
    views: wgpu::Buffer,
    next_view: std::cell::Cell<u32>,
    scan: wgpu::Buffer,
    flags: wgpu::Buffer,
    prefix: wgpu::Buffer,
    blocks: wgpu::Buffer,
    segments: wgpu::Buffer,
    cellvis: wgpu::Buffer,
    /// The main view's report and a scratch one for the other views.
    report: wgpu::Buffer,
    scratch_report: wgpu::Buffer,
    /// The main view's phase 1, phase 2 and frame flags, its seen bits (kept from frame to frame), and stand-ins for the views
    /// that read none of them.
    flags_first: wgpu::Buffer,
    flags_second: wgpu::Buffer,
    flags_frame: wgpu::Buffer,
    vis: wgpu::Buffer,
    stand_in_ro: wgpu::Buffer,
    stand_in_vis: wgpu::Buffer,
    stand_in_flags2: wgpu::Buffer,
    stand_in_hiz: wgpu::TextureView,
    hiz_first_pipe: wgpu::ComputePipeline,
    hiz_down_pipe: wgpu::ComputePipeline,
    hiz: Option<Hiz>,
    ring: Vec<(wgpu::Buffer, std::sync::Arc<std::sync::atomic::AtomicU8>)>,
    ring_copied: Option<usize>,
    /// The last report read: the cells seen and the draws listed.
    pub seen: Option<(u32, u32)>,
    slots: u32,
    segment_ids: u32,
    cells: u32,
}

fn storage_buffer(
    device: &wgpu::Device,
    label: &str,
    words: u64,
    extra: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: words.max(4) * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | extra,
        mapped_at_creation: false,
    })
}

/// The storage buffers one cull pass binds (`cull.wgsl`'s group).
const STORAGE_BINDINGS: u32 = 10;

impl Cull {
    /// The cull, on a device that can bind its buffers (None: the CPU's lists).
    pub fn new(device: &wgpu::Device) -> Option<Cull> {
        if device.limits().max_storage_buffers_per_shader_stage < STORAGE_BINDINGS {
            return None;
        }
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let uniform = |binding, dynamic| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: wgpu::BufferSize::new(if dynamic { VIEW_BYTES } else { 16 }),
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd cull"),
            entries: &[
                storage(0, true),
                storage(1, false),
                storage(2, false),
                storage(3, false),
                storage(4, true),
                storage(5, false),
                storage(6, false),
                uniform(7, false),
                storage(8, true),
                storage(9, false),
                uniform(10, true),
                storage(11, false),
                storage(12, true),
                storage(13, false),
                storage(14, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 15,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hd cull"),
            source: wgpu::ShaderSource::Wgsl(crate::shaders::Permutation::Cull.source().into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd cull"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipe = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pl),
                module: &module,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        };
        let none = wgpu::BufferUsages::empty();
        let report_usage = wgpu::BufferUsages::COPY_SRC;
        let hiz_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hd hiz far"),
            source: wgpu::ShaderSource::Wgsl(crate::shaders::Permutation::Hiz.source().into()),
        });
        let hiz_pipe = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &hiz_module,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        };
        let stand_in_hiz = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("hd hiz stand-in"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        Some(Cull {
            flags_first: storage_buffer(device, "hd cull first flags", 0, none),
            flags_second: storage_buffer(device, "hd cull second flags", 0, none),
            flags_frame: storage_buffer(device, "hd cull frame flags", 0, none),
            vis: storage_buffer(device, "hd cull seen bits", 0, wgpu::BufferUsages::COPY_SRC),
            stand_in_ro: storage_buffer(device, "hd cull stand-in", 0, none),
            stand_in_vis: storage_buffer(device, "hd cull stand-in seen bits", 0, none),
            stand_in_flags2: storage_buffer(device, "hd cull stand-in flags", 0, none),
            stand_in_hiz,
            hiz_first_pipe: hiz_pipe("cs_first_cull"),
            hiz_down_pipe: hiz_pipe("cs_down_far"),
            hiz: None,
            cells_pipe: pipe("cs_cells"),
            clusters_pipe: pipe("cs_clusters"),
            blocks_pipe: pipe("cs_scan_blocks"),
            top_pipe: pipe("cs_scan_top"),
            scatter_pipe: pipe("cs_scatter"),
            counts_pipe: pipe("cs_counts"),
            page_offsets_pipe: pipe("cs_page_offsets"),
            page_lists: Vec::new(),
            layout,
            views: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hd cull views"),
                size: u64::from(MAX_VIEWS) * VIEW_STRIDE,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            next_view: std::cell::Cell::new(0),
            scan: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hd cull scan"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            flags: storage_buffer(device, "hd cull flags", 0, none),
            prefix: storage_buffer(device, "hd cull prefix", 0, none),
            blocks: storage_buffer(device, "hd cull blocks", 1024, none),
            segments: storage_buffer(
                device,
                "hd cull segments",
                u64::from(MAX_SEGMENTS) * 4,
                none,
            ),
            cellvis: storage_buffer(device, "hd cull cell flags", 0, none),
            report: storage_buffer(device, "hd cull report", 2, report_usage),
            scratch_report: storage_buffer(device, "hd cull scratch report", 2, none),
            ring: (0..REPORT_RING)
                .map(|_| {
                    (
                        device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("hd cull report ring"),
                            size: 8,
                            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                            mapped_at_creation: false,
                        }),
                        std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0)),
                    )
                })
                .collect(),
            ring_copied: None,
            seen: None,
            slots: 0,
            segment_ids: 0,
            cells: 0,
        })
    }

    /// The working buffers' bytes.
    pub fn bytes(&self) -> u64 {
        self.flags.size()
            + self.prefix.size()
            + self.cellvis.size()
            + self.blocks.size()
            + self.segments.size()
            + self.flags_first.size()
            + self.flags_second.size()
            + self.flags_frame.size()
            + self.vis.size()
    }

    /// The frame's start: the buffers sized to the tables, the segments and the scan's counts written, the views' slots free again.
    pub fn begin_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        table: &Table,
        cells: &CellTable,
    ) {
        let slots = table.capacity();
        if self.flags.size() < u64::from(slots) * 4 {
            let none = wgpu::BufferUsages::empty();
            let n = u64::from(slots);
            self.flags = storage_buffer(device, "hd cull flags", n, none);
            self.prefix = storage_buffer(device, "hd cull prefix", n, none);
            self.flags_first = storage_buffer(device, "hd cull first flags", n, none);
            self.flags_second = storage_buffer(device, "hd cull second flags", n, none);
            self.flags_frame = storage_buffer(device, "hd cull frame flags", n, none);
            // The seen bits carried over (a grown table's slots stay where they were: emptied, a growth while the cells stream
            // in drew everything for two frames).
            let vis = storage_buffer(device, "hd cull seen bits", n, wgpu::BufferUsages::COPY_SRC);
            let mut e = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("hd cull seen bits carried"),
            });
            e.copy_buffer_to_buffer(&self.vis, 0, &vis, 0, self.vis.size().min(vis.size()));
            queue.submit([e.finish()]);
            self.vis = vis;
        }
        if self.cellvis.size() < u64::from(cells.capacity()) * 4 {
            self.cellvis = storage_buffer(
                device,
                "hd cull cell flags",
                u64::from(cells.capacity()),
                wgpu::BufferUsages::empty(),
            );
        }
        self.slots = slots;
        self.cells = cells.capacity();
        let n = table.segment_ids();
        self.segment_ids = n;
        let mut seg = Vec::with_capacity(n as usize * 8);
        for id in 0..n {
            let (first, len) = table.segment(id).map_or((0, 0), |s| (s.first, s.len));
            seg.extend_from_slice(&first.to_le_bytes());
            seg.extend_from_slice(&len.to_le_bytes());
        }
        if !seg.is_empty() {
            queue.write_buffer(&self.segments, 0, &seg);
        }
        // Each segment's page and its page's list's first slot (the pages' lists one after another, each its segments' slots).
        let page_of = |id: u32| {
            table
                .segment(id)
                .map_or(0, |s| (s.group.2 as usize).min(MAX_PAGES - 1))
        };
        let mut lens = vec![0u32; MAX_PAGES];
        for id in 0..n {
            if let Some(s) = table.segment(id) {
                lens[page_of(id)] += s.len;
            }
        }
        let last = lens.iter().rposition(|&l| l > 0).map_or(0, |p| p + 1);
        let mut base = 0u32;
        self.page_lists = lens[..last]
            .iter()
            .map(|&l| {
                let at = (base, l);
                base += l;
                at
            })
            .collect();
        let mut pages = Vec::with_capacity(n as usize * 8);
        for id in 0..n {
            let p = page_of(id);
            let first = self.page_lists.get(p).map_or(0, |l| l.0);
            pages.extend_from_slice(&(p as u32).to_le_bytes());
            pages.extend_from_slice(&first.to_le_bytes());
        }
        if !pages.is_empty() {
            queue.write_buffer(&self.segments, u64::from(MAX_SEGMENTS) * 8, &pages);
        }
        let mut scan = Vec::with_capacity(16);
        for w in [slots, n, 0, 0] {
            scan.extend_from_slice(&w.to_le_bytes());
        }
        queue.write_buffer(&self.scan, 0, &scan);
        self.next_view.set(0);
        self.read_report(device);
    }

    /// Each arena page's list in a whole list ([`VIEW_WHOLE`]): its first slot and its slots (empty pages included).
    pub fn page_lists(&self) -> &[(u32, u32)] {
        &self.page_lists
    }

    /// A view's uniform slot for this frame (None when the frame's slots are used up).
    pub fn view(&self, queue: &wgpu::Queue, v: &ViewPlanes) -> Option<u32> {
        let at = self.next_view.get();
        if at >= MAX_VIEWS {
            return None;
        }
        self.next_view.set(at + 1);
        queue.write_buffer(
            &self.views,
            u64::from(at) * VIEW_STRIDE,
            &v.write(self.cells),
        );
        Some(at)
    }

    /// Culls the clusters for the view in `slot` into `lists` (the main view's with `main`: its report kept).
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        slot: u32,
        table: &Table,
        cells: &CellTable,
        lists: &Lists,
        main: bool,
    ) {
        if self.slots == 0 {
            return;
        }
        let report = if main {
            &self.report
        } else {
            &self.scratch_report
        };
        if main {
            encoder.clear_buffer(report, 0, None);
        }
        let io = Io {
            flags: &self.flags,
            prev: &self.stand_in_ro,
            vis: &self.stand_in_vis,
            flags2: &self.stand_in_flags2,
            hiz: &self.stand_in_hiz,
            report,
        };
        self.pass(device, encoder, slot, table, cells, lists, &io, ALL_STEPS);
    }

    /// Culls the clusters for each view (its slot) into its own lists, all in one compute pass (the shadow maps' views: the same
    /// steps in the same order as [`Cull::encode`] for each, one after the other).
    pub fn encode_batch(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        views: &[(u32, &Lists, bool)],
        table: &Table,
        cells: &CellTable,
    ) {
        if self.slots == 0 || views.is_empty() {
            return;
        }
        let io = Io {
            flags: &self.flags,
            prev: &self.stand_in_ro,
            vis: &self.stand_in_vis,
            flags2: &self.stand_in_flags2,
            hiz: &self.stand_in_hiz,
            report: &self.scratch_report,
        };
        let groups: Vec<wgpu::BindGroup> = views
            .iter()
            .map(|(_, lists, _)| self.group(device, table, cells, lists, &io))
            .collect();
        crate::count_pass_as(crate::PassKind::Cull);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("hd cull batch"),
            timestamp_writes: None,
        });
        for ((slot, _, whole), group) in views.iter().zip(&groups) {
            if *whole {
                self.steps_whole(&mut pass, group, *slot);
            } else {
                self.steps(&mut pass, group, *slot, ALL_STEPS);
            }
        }
    }

    /// The main view's phase 1 (`slot`: its view at phase 1): the opaque clusters seen last frame into `first`.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_first(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        slot: u32,
        table: &Table,
        cells: &CellTable,
        first: &Lists,
    ) {
        if self.slots == 0 {
            return;
        }
        let io = Io {
            flags: &self.flags_first,
            prev: &self.stand_in_ro,
            vis: &self.vis,
            flags2: &self.stand_in_flags2,
            hiz: &self.stand_in_hiz,
            report: &self.scratch_report,
        };
        self.pass(device, encoder, slot, table, cells, first, &io, ALL_STEPS);
    }

    /// The farthest-depth pyramid for a frame of `size` drawn into `depth` (`key`: a count that changes whenever the frame's
    /// targets are made again).
    pub fn ensure_hiz(
        &mut self,
        device: &wgpu::Device,
        size: (u32, u32),
        depth: &wgpu::TextureView,
        key: u64,
    ) {
        let k = (size.0, size.1, key);
        if self.hiz.as_ref().is_some_and(|h| h.key == k) {
            return;
        }
        let pad = 1u32 << (HIZ_LEVELS - 1);
        let (w, h) = (
            size.0.max(1).div_ceil(pad) * pad,
            size.1.max(1).div_ceil(pad) * pad,
        );
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hd hiz far"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: HIZ_LEVELS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let level = |m: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: m,
                mip_level_count: Some(1),
                ..wgpu::TextureViewDescriptor::default()
            })
        };
        let levels: Vec<wgpu::TextureView> = (0..HIZ_LEVELS).map(level).collect();
        let first = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd hiz far first"),
            layout: &self.hiz_first_pipe.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&levels[0]),
                },
            ],
        });
        let down_layout = self.hiz_down_pipe.get_bind_group_layout(0);
        let downs = levels
            .windows(2)
            .enumerate()
            .map(|(m, v)| {
                (
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("hd hiz far down"),
                        layout: &down_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(&v[0]),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::TextureView(&v[1]),
                            },
                        ],
                    }),
                    (w >> (m + 1), h >> (m + 1)),
                )
            })
            .collect();
        self.hiz = Some(Hiz {
            key: k,
            view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
            first,
            downs,
            size: (w, h),
        });
    }

    /// The main view's phase 2 (after phase 1's depth is drawn; `slot`: its view at phase 2): the pyramid, the opaque clusters
    /// phase 1 did not draw that are not wholly behind it into `second`, the frame's lists into `frame` (with the report), the
    /// next frame's seen bits.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_second(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        slot: u32,
        table: &Table,
        cells: &CellTable,
        second: &Lists,
        frame: &Lists,
    ) {
        let Some(hiz) = &self.hiz else {
            return;
        };
        if self.slots == 0 {
            return;
        }
        let second_io = Io {
            flags: &self.flags_second,
            prev: &self.flags_first,
            vis: &self.vis,
            flags2: &self.flags_frame,
            hiz: &hiz.view,
            report: &self.scratch_report,
        };
        let frame_io = Io {
            flags: &self.flags_frame,
            prev: &self.stand_in_ro,
            vis: &self.stand_in_vis,
            flags2: &self.stand_in_flags2,
            hiz: &self.stand_in_hiz,
            report: &self.report,
        };
        let second_group = self.group(device, table, cells, second, &second_io);
        let frame_group = self.group(device, table, cells, frame, &frame_io);
        // The frame's report counts from nothing (the frame's lists' cull below is the only one that adds to it).
        encoder.clear_buffer(&self.report, 0, None);
        // The pyramid, phase 2's cull and the frame's lists in one compute pass (one after the other, as three passes did).
        crate::count_pass_as(crate::PassKind::Cull);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("hd hiz far and cull"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.hiz_first_pipe);
        pass.set_bind_group(0, &hiz.first, &[]);
        pass.dispatch_workgroups(hiz.size.0.div_ceil(8), hiz.size.1.div_ceil(8), 1);
        pass.set_pipeline(&self.hiz_down_pipe);
        for (group, size) in &hiz.downs {
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(size.0.div_ceil(8).max(1), size.1.div_ceil(8).max(1), 1);
        }
        self.steps(&mut pass, &second_group, slot, ALL_STEPS);
        // The frame's lists from the second flags (the cells' test again for the report: the cells seen).
        self.steps(
            &mut pass,
            &frame_group,
            slot,
            Steps {
                cells: true,
                clusters: false,
                compact: true,
            },
        );
    }

    /// One cull pass of `steps` for the view in `slot` into `lists`.
    #[allow(clippy::too_many_arguments)]
    fn pass(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        slot: u32,
        table: &Table,
        cells: &CellTable,
        lists: &Lists,
        io: &Io<'_>,
        steps: Steps,
    ) {
        let group = self.group(device, table, cells, lists, io);
        crate::count_pass_as(crate::PassKind::Cull);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("hd cull"),
            timestamp_writes: None,
        });
        self.steps(&mut pass, &group, slot, steps);
    }

    /// A cull's bind group: the tables, `io`'s buffers and `lists`.
    fn group(
        &self,
        device: &wgpu::Device,
        table: &Table,
        cells: &CellTable,
        lists: &Lists,
        io: &Io<'_>,
    ) -> wgpu::BindGroup {
        fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            }
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd cull"),
            layout: &self.layout,
            entries: &[
                entry(0, &table.buffer),
                entry(1, io.flags),
                entry(2, &self.prefix),
                entry(3, &self.blocks),
                entry(4, &self.segments),
                entry(5, &lists.args),
                entry(6, &lists.counts),
                entry(7, &self.scan),
                entry(8, &cells.buffer),
                entry(9, &self.cellvis),
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.views,
                        offset: 0,
                        size: wgpu::BufferSize::new(VIEW_BYTES),
                    }),
                },
                entry(11, io.report),
                entry(12, io.prev),
                entry(13, io.vis),
                entry(14, io.flags2),
                wgpu::BindGroupEntry {
                    binding: 15,
                    resource: wgpu::BindingResource::TextureView(io.hiz),
                },
            ],
        })
    }

    /// A whole list's dispatches ([`VIEW_WHOLE`]): the tests, the scan, the segments' counts, their places in their pages' lists,
    /// the scatter.
    fn steps_whole(&self, pass: &mut wgpu::ComputePass<'_>, group: &wgpu::BindGroup, slot: u32) {
        pass.set_bind_group(0, group, &[(u64::from(slot) * VIEW_STRIDE) as u32]);
        pass.set_pipeline(&self.cells_pipe);
        pass.dispatch_workgroups(self.cells.div_ceil(64).max(1), 1, 1);
        pass.set_pipeline(&self.clusters_pipe);
        pass.dispatch_workgroups(self.slots.div_ceil(256), 1, 1);
        pass.set_pipeline(&self.blocks_pipe);
        pass.dispatch_workgroups(self.slots.div_ceil(1024), 1, 1);
        pass.set_pipeline(&self.top_pipe);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_pipeline(&self.counts_pipe);
        pass.dispatch_workgroups(self.segment_ids.div_ceil(64).max(1), 1, 1);
        pass.set_pipeline(&self.page_offsets_pipe);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_pipeline(&self.scatter_pipe);
        pass.dispatch_workgroups(self.slots.div_ceil(256), 1, 1);
    }

    /// The dispatches of `steps` for the view in `slot` (its group bound).
    fn steps(
        &self,
        pass: &mut wgpu::ComputePass<'_>,
        group: &wgpu::BindGroup,
        slot: u32,
        steps: Steps,
    ) {
        pass.set_bind_group(0, group, &[(u64::from(slot) * VIEW_STRIDE) as u32]);
        if steps.cells {
            pass.set_pipeline(&self.cells_pipe);
            pass.dispatch_workgroups(self.cells.div_ceil(64).max(1), 1, 1);
        }
        if steps.clusters {
            pass.set_pipeline(&self.clusters_pipe);
            pass.dispatch_workgroups(self.slots.div_ceil(256), 1, 1);
        }
        if !steps.compact {
            return;
        }
        pass.set_pipeline(&self.blocks_pipe);
        pass.dispatch_workgroups(self.slots.div_ceil(1024), 1, 1);
        pass.set_pipeline(&self.top_pipe);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_pipeline(&self.scatter_pipe);
        pass.dispatch_workgroups(self.slots.div_ceil(256), 1, 1);
        pass.set_pipeline(&self.counts_pipe);
        pass.dispatch_workgroups(self.segment_ids.div_ceil(64).max(1), 1, 1);
    }

    /// The main view's report copied into a free slot of the ring (after its cull, before the frame's submit).
    pub fn keep_report(&mut self, encoder: &mut wgpu::CommandEncoder) {
        self.ring_copied = None;
        if let Some(i) = self
            .ring
            .iter()
            .position(|r| r.1.load(std::sync::atomic::Ordering::Acquire) == 0)
        {
            encoder.copy_buffer_to_buffer(&self.report, 0, &self.ring[i].0, 0, 8);
            self.ring[i]
                .1
                .store(1, std::sync::atomic::Ordering::Release);
            self.ring_copied = Some(i);
        }
    }

    /// After the frame's submit: the slot copied into is mapped.
    pub fn map_report(&mut self) {
        if let Some(i) = self.ring_copied.take() {
            let (b, state) = &self.ring[i];
            state.store(2, std::sync::atomic::Ordering::Release);
            let state = state.clone();
            b.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                state.store(
                    if r.is_ok() { 3 } else { 4 },
                    std::sync::atomic::Ordering::Release,
                );
            });
        }
    }

    /// The reports that came back (never waits): the last one kept in [`Cull::seen`].
    pub fn read_report(&mut self, device: &wgpu::Device) {
        let _ = device.poll(wgpu::PollType::Poll);
        for (b, state) in &self.ring {
            match state.load(std::sync::atomic::Ordering::Acquire) {
                3 => {
                    if let Ok(data) = b.slice(..).get_mapped_range() {
                        let w = |i: usize| {
                            u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]])
                        };
                        self.seen = Some((w(0), w(4)));
                    }
                    b.unmap();
                    state.store(0, std::sync::atomic::Ordering::Release);
                }
                4 => state.store(0, std::sync::atomic::Ordering::Release),
                _ => {}
            }
        }
    }
}

/// Cells a block's side.
pub const BLOCK: i32 = 8;

/// A block's cells (their boxes and whether they reflect) and their union.
#[derive(Debug, Clone, Default)]
struct Block {
    cells: BTreeMap<[i32; 3], ([[f32; 3]; 2], bool)>,
    union: [[f32; 3]; 2],
    reflective: u32,
}

impl Block {
    fn rebox(&mut self) {
        let mut u = [[f32::INFINITY; 3], [f32::NEG_INFINITY; 3]];
        for (b, _) in self.cells.values() {
            for k in 0..3 {
                u[0][k] = u[0][k].min(b[0][k]);
                u[1][k] = u[1][k].max(b[1][k]);
            }
        }
        self.union = u;
    }
}

/// The squared distance from `e` to a box's nearest point, and to its farthest corner (`shadow_range`'s arithmetic).
fn reach2(e: [f32; 3], a: [f32; 3], b: [f32; 3]) -> (f32, f32) {
    let mut near2 = 0.0;
    let mut far2 = 0.0;
    for k in 0..3 {
        let d = (e[k] - e[k].clamp(a[k], b[k])).abs();
        near2 += d * d;
        let f = (e[k] - a[k]).abs().max((e[k] - b[k]).abs());
        far2 += f * f;
    }
    (near2, far2)
}

/// The cells grouped in blocks of [`BLOCK`] cells a side with the blocks' union boxes, so the questions a frame asks of every cell
/// (the nearest and farthest seen, a reflective one in sight, those near the eye) look only at the blocks that can change the
/// answer and the cells inside them. The answers are those a walk of every cell gives.
#[derive(Debug, Clone, Default)]
pub struct CellGrid {
    blocks: BTreeMap<[i32; 3], Block>,
    cells: usize,
}

fn block_of(key: [i32; 3]) -> [i32; 3] {
    key.map(|k| k.div_euclid(BLOCK))
}

impl CellGrid {
    pub fn insert(&mut self, key: [i32; 3], bounds: [[f32; 3]; 2], reflective: bool) {
        self.remove(key);
        let b = self.blocks.entry(block_of(key)).or_default();
        b.cells.insert(key, (bounds, reflective));
        b.reflective += u32::from(reflective);
        b.rebox();
        self.cells += 1;
    }

    pub fn remove(&mut self, key: [i32; 3]) {
        let bk = block_of(key);
        let Some(b) = self.blocks.get_mut(&bk) else {
            return;
        };
        let Some((_, reflective)) = b.cells.remove(&key) else {
            return;
        };
        b.reflective -= u32::from(reflective);
        self.cells -= 1;
        if b.cells.is_empty() {
            self.blocks.remove(&bk);
        } else {
            b.rebox();
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.cells
    }

    /// The nearest point and the farthest corner (units) of the seen cells from `eye` (`seen` a box test that holds for a
    /// box whenever it holds for one inside it), or None when none is seen: the extremes over every cell.
    pub fn seen_reach(
        &self,
        eye: [f32; 3],
        seen: &dyn Fn([f32; 3], [f32; 3]) -> bool,
    ) -> Option<(f32, f32)> {
        let (mut lo2, mut hi2) = (f32::INFINITY, f32::NEG_INFINITY);
        // The blocks nearest first: the nearest seen cell is found early, and fewer blocks remain that could lower it.
        let mut order: Vec<(f32, f32, &Block)> = self
            .blocks
            .values()
            .map(|b| {
                let (n, f) = reach2(eye, b.union[0], b.union[1]);
                (n, f, b)
            })
            .collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (n, f, b) in order {
            // No cell inside can come nearer than the union, nor reach farther.
            if n >= lo2 && f <= hi2 {
                continue;
            }
            if !seen(b.union[0], b.union[1]) {
                continue;
            }
            for (bb, _) in b.cells.values() {
                if seen(bb[0], bb[1]) {
                    let (cn, cf) = reach2(eye, bb[0], bb[1]);
                    lo2 = lo2.min(cn);
                    hi2 = hi2.max(cf);
                }
            }
        }
        lo2.is_finite().then(|| (lo2.sqrt(), hi2.sqrt()))
    }

    /// Whether a reflective cell is seen.
    pub fn reflective_seen(&self, seen: &dyn Fn([f32; 3], [f32; 3]) -> bool) -> bool {
        self.blocks.values().any(|b| {
            b.reflective > 0
                && seen(b.union[0], b.union[1])
                && b.cells.values().any(|(bb, r)| *r && seen(bb[0], bb[1]))
        })
    }

    /// The cells whose box lies within `reach` of `eye` (in key order).
    pub fn near(&self, eye: [f32; 3], reach: f32) -> Vec<[i32; 3]> {
        let r2 = reach * reach;
        let mut out = Vec::new();
        for b in self.blocks.values() {
            if reach2(eye, b.union[0], b.union[1]).0 > r2 {
                continue;
            }
            for (k, (bb, _)) in &b.cells {
                if reach2(eye, bb[0], bb[1]).0 <= r2 {
                    out.push(*k);
                }
            }
        }
        out.sort_unstable();
        out
    }
}

impl CellTable {
    /// The CPU's copies' bytes (the cells' boxes and the dirty slots).
    pub fn cpu_bytes(&self) -> u64 {
        (self.mirror.capacity() * std::mem::size_of::<CellBox>() + self.dirty.len() * 24) as u64
    }
}

impl CellGrid {
    /// The CPU's bytes (the blocks and their cells' boxes).
    pub fn cpu_bytes(&self) -> u64 {
        self.blocks
            .values()
            .map(|b| (b.cells.len() * (12 + 25 + 16) + 96) as u64)
            .sum()
    }
}

#[cfg(test)]
#[path = "cull_tests.rs"]
mod tests;
