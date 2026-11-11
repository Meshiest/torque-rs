//! The enhanced renderer on the GPU: its WGSL and the shader registry ([`shaders`]), and its passes over a
//! [`torque_render_core::Gpu`]'s device: the bloom chain ([`bloom`]), the automatic exposure ([`exposure`]), the volumetric light
//! ([`volumetric`]), the moving shapes' vertex runs ([`slots`]), the environment's cube maps and the light capture ([`capture`]),
//! the screen-space reflections ([`ssr`]), the volumetric clouds ([`clouds`]), the anti-aliasing and AO ([`post`]), the sky
//! ([`sky`]), the water's planar target and its frame terms ([`water`]), and the static geometry's arena ([`arena`]) culled into
//! draw lists ([`cull`]); what a world's static geometry brings beside the engine's frame ([`ext`]); with the helpers they share (bind group entries, compute filter chains, full-frame pipelines, the pass
//! and draw counts by kind).
//!
//! # Maintaining
//!
//! Rules a change must keep, each guarded by a test:
//! - Nothing past the device's largest buffer is asked of it: an upload whose loose draws or particles do not fit leaves them out
//!   for the frame and counts it ([`HdWorld::oversize_uploads`]), the meshes' places too, so no draw reads another frame's places
//!   (`loose_draws_past_the_largest_buffer_are_left_out_and_counted_not_a_panic`,
//!   `a_table_of_mesh_places_past_the_largest_buffer_leaves_the_loose_draws_out`,
//!   `particles_past_the_largest_buffer_are_left_out_and_counted`).
//! - A world view's scene is an extension's `Scene` that is also an [`ext::ViewScene`] (mode, time, the cells static batches
//!   go in); [`ext::PlainScene`]'s is the engine's own. The shader modules are composed with one extension's chunks a process
//!   ([`shaders::register`]; [`HdWorld::try_from_device`] panics on a second), so a test comparing two extensions' pictures
//!   draws one of them in another process (`torque-app`'s `tests/render_ext.rs`).
//!
//! Costs a frame must not grow back (`benches/upload.rs` and the `frame-cpu` example, timed under the bench lock):
//! - No bind group is made a frame while nothing it binds changed: the forward passes' frame groups and the tonemap group are
//!   kept by what they bind (`GroupCache` in `world.rs`), the bloom's prefilter group by the source it reads, and every
//!   per-frame value goes through buffers already bound.
//! - The particles' vertex and index buffers are kept and written in place (made anew only to grow), their bytes in kept lists;
//!   the shadow casters' bytes go up to the last caster used, the mesh places' table from a kept list (`frame-cpu`).
//! - A stamped object whose stamp its run holds is not encoded or written again (bench `upload_loose 64 figures kept`).
//! - The translucent draws are sorted as one flat list (no list an object); the opaque frame's copy is taken only when a draw
//!   after it or an extension's pass may read it.

use glam::Vec3;
use torque_render_core::wgpu;

use crate::shaders::Permutation;

/// Whether the sphere at `c` of radius `r` is not wholly outside any of `planes` (`a x + b y + c z + d >= 0` inside, the normal
/// not unit).
pub(crate) fn sphere_seen(planes: &[[f32; 4]], c: [f32; 3], r: f32) -> bool {
    planes.iter().all(|p| {
        let n = Vec3::new(p[0], p[1], p[2]);
        n.dot(c.into()) + p[3] >= -r * n.length()
    })
}

pub mod arena;
pub mod bloom;
pub mod capture;
pub mod clouds;
pub mod cull;
pub mod exposure;
pub mod ext;
pub mod ghosting;
#[cfg(feature = "gizmos")]
pub mod gizmos;
pub mod limits;
pub mod passes;
pub mod post;
pub mod profile;
pub mod shaders;
pub mod shadows;
pub mod sky;
pub mod slots;
pub mod ssr;
pub mod volumetric;
pub mod water;
mod world;

pub use world::*;

/// The HDR and depth formats.
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// The fog takes the sky behind a surface without its detail (the clouds' shapes, the box's painted clouds): the sky copy's mip
/// of this level (the mean over 64 x 64 pixels of the internal frame), the clouds' image's level of the same span.
pub const FOG_LOW_LEVEL: u32 = 6;

/// The render and compute passes begun so far (the frame's CPU cost grows with its passes: each is a command list of its own).
pub static PASSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// What a frame's passes and draw calls are counted as (the breakdown's `passes by kind`): the GPU culls and the Hi-Z, the static
/// sun cascades (and their transmittance), the point lights' static cube faces, the moving shapes' overlays on both, the secondary
/// views (the water's mirror, a light capture's face), the main view's prepass, opaque and translucent passes, and the rest after them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassKind {
    Cull = 0,
    Cascade = 1,
    Face = 2,
    Overlay = 3,
    View = 4,
    Main = 5,
    Post = 6,
}

/// The kinds' names, in their order.
pub const PASS_KIND_NAMES: [&str; 7] =
    ["cull", "cascade", "face", "overlay", "view", "main", "post"];
/// The kinds' names as the CPU marks carry their counts (the passes' breakdown).
pub const PASS_KIND_MARKS: [&str; 7] = [
    "passes: cull",
    "passes: cascade",
    "passes: face",
    "passes: overlay",
    "passes: view",
    "passes: main",
    "passes: post",
];
pub const DRAW_KIND_MARKS: [&str; 7] = [
    "draws: cull",
    "draws: cascade",
    "draws: face",
    "draws: overlay",
    "draws: view",
    "draws: main",
    "draws: post",
];

/// The kind the passes begun now are counted as ([`set_pass_kind`]), and the passes and draw calls by kind so far.
static PASS_KIND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(6);
static PASSES_BY_KIND: [std::sync::atomic::AtomicU64; 7] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 7];
static DRAWS_BY_KIND: [std::sync::atomic::AtomicU64; 7] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 7];

/// The passes and draws from now on are `kind`'s (a cull's are always [`PassKind::Cull`]: [`count_pass_as`]).
pub fn set_pass_kind(kind: PassKind) -> usize {
    PASS_KIND.swap(kind as usize, std::sync::atomic::Ordering::Relaxed)
}

/// The kind [`set_pass_kind`] gave back made the passes' kind again.
pub fn restore_pass_kind(kind: usize) {
    PASS_KIND.store(kind.min(6), std::sync::atomic::Ordering::Relaxed);
}

/// The passes and draw calls by kind so far.
pub fn passes_by_kind() -> ([u64; 7], [u64; 7]) {
    let load = |a: &[std::sync::atomic::AtomicU64; 7]| {
        std::array::from_fn(|i| a[i].load(std::sync::atomic::Ordering::Relaxed))
    };
    (load(&PASSES_BY_KIND), load(&DRAWS_BY_KIND))
}

pub fn count_pass() {
    PASSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let kind = PASS_KIND.load(std::sync::atomic::Ordering::Relaxed);
    PASSES_BY_KIND[kind].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// A pass of `kind`, whatever kind the passes around it are.
pub fn count_pass_as(kind: PassKind) {
    PASSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    PASSES_BY_KIND[kind as usize].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// `n` draw calls of the passes' kind now (a multi-draw is one).
pub fn count_draws(n: usize) {
    let kind = PASS_KIND.load(std::sync::atomic::Ordering::Relaxed);
    DRAWS_BY_KIND[kind].fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
}

/// A 4 x 4 column-major inverse (f64), the identity when singular.
pub fn invert(m: &[f32; 16]) -> [f32; 16] {
    let a: Vec<f64> = m.iter().map(|v| f64::from(*v)).collect();
    let mut inv = [0.0f64; 16];
    inv[0] = a[5] * a[10] * a[15] - a[5] * a[11] * a[14] - a[9] * a[6] * a[15]
        + a[9] * a[7] * a[14]
        + a[13] * a[6] * a[11]
        - a[13] * a[7] * a[10];
    inv[4] = -a[4] * a[10] * a[15] + a[4] * a[11] * a[14] + a[8] * a[6] * a[15]
        - a[8] * a[7] * a[14]
        - a[12] * a[6] * a[11]
        + a[12] * a[7] * a[10];
    inv[8] = a[4] * a[9] * a[15] - a[4] * a[11] * a[13] - a[8] * a[5] * a[15]
        + a[8] * a[7] * a[13]
        + a[12] * a[5] * a[11]
        - a[12] * a[7] * a[9];
    inv[12] = -a[4] * a[9] * a[14] + a[4] * a[10] * a[13] + a[8] * a[5] * a[14]
        - a[8] * a[6] * a[13]
        - a[12] * a[5] * a[10]
        + a[12] * a[6] * a[9];
    inv[1] = -a[1] * a[10] * a[15] + a[1] * a[11] * a[14] + a[9] * a[2] * a[15]
        - a[9] * a[3] * a[14]
        - a[13] * a[2] * a[11]
        + a[13] * a[3] * a[10];
    inv[5] = a[0] * a[10] * a[15] - a[0] * a[11] * a[14] - a[8] * a[2] * a[15]
        + a[8] * a[3] * a[14]
        + a[12] * a[2] * a[11]
        - a[12] * a[3] * a[10];
    inv[9] = -a[0] * a[9] * a[15] + a[0] * a[11] * a[13] + a[8] * a[1] * a[15]
        - a[8] * a[3] * a[13]
        - a[12] * a[1] * a[11]
        + a[12] * a[3] * a[9];
    inv[13] = a[0] * a[9] * a[14] - a[0] * a[10] * a[13] - a[8] * a[1] * a[14]
        + a[8] * a[2] * a[13]
        + a[12] * a[1] * a[10]
        - a[12] * a[2] * a[9];
    inv[2] = a[1] * a[6] * a[15] - a[1] * a[7] * a[14] - a[5] * a[2] * a[15]
        + a[5] * a[3] * a[14]
        + a[13] * a[2] * a[7]
        - a[13] * a[3] * a[6];
    inv[6] = -a[0] * a[6] * a[15] + a[0] * a[7] * a[14] + a[4] * a[2] * a[15]
        - a[4] * a[3] * a[14]
        - a[12] * a[2] * a[7]
        + a[12] * a[3] * a[6];
    inv[10] = a[0] * a[5] * a[15] - a[0] * a[7] * a[13] - a[4] * a[1] * a[15]
        + a[4] * a[3] * a[13]
        + a[12] * a[1] * a[7]
        - a[12] * a[3] * a[5];
    inv[14] = -a[0] * a[5] * a[14] + a[0] * a[6] * a[13] + a[4] * a[1] * a[14]
        - a[4] * a[2] * a[13]
        - a[12] * a[1] * a[6]
        + a[12] * a[2] * a[5];
    inv[3] = -a[1] * a[6] * a[11] + a[1] * a[7] * a[10] + a[5] * a[2] * a[11]
        - a[5] * a[3] * a[10]
        - a[9] * a[2] * a[7]
        + a[9] * a[3] * a[6];
    inv[7] = a[0] * a[6] * a[11] - a[0] * a[7] * a[10] - a[4] * a[2] * a[11]
        + a[4] * a[3] * a[10]
        + a[8] * a[2] * a[7]
        - a[8] * a[3] * a[6];
    inv[11] = -a[0] * a[5] * a[11] + a[0] * a[7] * a[9] + a[4] * a[1] * a[11]
        - a[4] * a[3] * a[9]
        - a[8] * a[1] * a[7]
        + a[8] * a[3] * a[5];
    inv[15] = a[0] * a[5] * a[10] - a[0] * a[6] * a[9] - a[4] * a[1] * a[10]
        + a[4] * a[2] * a[9]
        + a[8] * a[1] * a[6]
        - a[8] * a[2] * a[5];
    let det = a[0] * inv[0] + a[1] * inv[4] + a[2] * inv[8] + a[3] * inv[12];
    if det == 0.0 || !det.is_finite() {
        let mut id = [0.0f32; 16];
        for i in 0..4 {
            id[i * 5] = 1.0;
        }
        return id;
    }
    inv.map(|v| (v / det) as f32)
}

pub fn push_f32(out: &mut Vec<u8>, v: &[f32]) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

pub fn push_u32(out: &mut Vec<u8>, v: &[u32]) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

pub fn module(device: &wgpu::Device, p: Permutation) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&p.name()),
        source: wgpu::ShaderSource::Wgsl(p.source().into()),
    })
}

pub fn storage(
    binding: u32,
    read_only: bool,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// A write-only storage texture of `format` for a compute filter's output (a mip chain's level).
pub fn storage_target(binding: u32, format: wgpu::TextureFormat) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

/// A compute filter's pipeline (`entry` of `module` over the one group `layout`).
pub fn compute_filter(
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    module: &wgpu::ShaderModule,
    entry: &str,
) -> wgpu::ComputePipeline {
    compute_filter_with(device, cache, label, &[layout], module, entry)
}

/// [`compute_filter`] over several groups (A filter's sampler in a group of its own, shared by its levels).
pub fn compute_filter_with(
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    label: &str,
    layouts: &[&wgpu::BindGroupLayout],
    module: &wgpu::ShaderModule,
    entry: &str,
) -> wgpu::ComputePipeline {
    let groups: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(|l| Some(*l)).collect();
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &groups,
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&pl),
        module,
        entry_point: Some(entry),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache,
    })
}

/// The side of a compute filter's square workgroup (the shaders' `@workgroup_size(8, 8)`).
pub const FILTER_GROUP: u32 = 8;

/// One level of a filter chain: its pipeline, its group (the level it reads and the one it writes) and the written level's size.
pub struct Dispatch<'a> {
    pub pipeline: &'a wgpu::ComputePipeline,
    pub group: &'a wgpu::BindGroup,
    pub size: (u32, u32),
}

/// A chain of full-frame filters (a mip pyramid, the bloom's levels) as the dispatches of ONE compute pass, each level after the
/// one it reads (the passes' barriers are the pass's own): a render pass a level made a command list a level, most of a big frame's
/// CPU cost. The same arithmetic at the same texel centres as the fragment filters they replace.
pub fn encode_chain(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    chain: &[Dispatch<'_>],
    stamps: Option<wgpu::ComputePassTimestampWrites<'_>>,
) {
    encode_chain_with(encoder, label, chain, &[], stamps);
}

/// [`encode_chain`] with groups every level shares (bound once, from group 1).
pub fn encode_chain_with(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    chain: &[Dispatch<'_>],
    shared: &[&wgpu::BindGroup],
    stamps: Option<wgpu::ComputePassTimestampWrites<'_>>,
) {
    if chain.is_empty() {
        return;
    }
    count_pass();
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some(label),
        timestamp_writes: stamps,
    });
    for (k, g) in shared.iter().enumerate() {
        pass.set_bind_group(1 + k as u32, *g, &[]);
    }
    for d in chain {
        pass.set_pipeline(d.pipeline);
        pass.set_bind_group(0, d.group, &[]);
        pass.dispatch_workgroups(
            d.size.0.div_ceil(FILTER_GROUP).max(1),
            d.size.1.div_ceil(FILTER_GROUP).max(1),
            1,
        );
    }
}

pub fn uniform(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// A bind group entry of a texture view.
pub fn tv(binding: u32, v: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(v),
    }
}

/// A colour attachment cleared to transparent black.
pub fn colour(view: &wgpu::TextureView) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            store: wgpu::StoreOp::Store,
        },
    }
}

/// A full-frame triangle's pipeline of `layout`, `entry` of `module` into `format`.
pub fn full_frame(
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    module: &wgpu::ShaderModule,
    entry: &str,
    formats: &[wgpu::TextureFormat],
) -> wgpu::RenderPipeline {
    let targets: Vec<Option<wgpu::ColorTargetState>> = formats
        .iter()
        .map(|f| {
            Some(wgpu::ColorTargetState {
                format: *f,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })
        })
        .collect();
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pl),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_full"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &targets,
        }),
        multiview_mask: None,
        cache,
    })
}
