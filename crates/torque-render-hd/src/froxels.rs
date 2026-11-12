//! The enhanced renderer's froxel volumetrics on the GPU (`shaders/froxels.wgsl`): where the quality has them
//! ([`torque_render_scene::hd::sky::VolumetricQuality::froxels`]), the point lights' light the fog scatters toward the eye. A grid
//! over the view frustum ([`FROXEL_TILE`] internal pixels a side, [`FROXEL_SLICES`] slices to the medium's reach) is lit once a
//! frame: one compute pass gathers each froxel's medium (the god rays' thin medium and the distance and height fog's extinction)
//! and the lights' light in it (through the clusters' masks, their cones and their shadow cubes), then sums the grid along each
//! column; after the opaque pass the sum at each pixel's depth is added over the frame. The sun's god rays stay the volumetric
//! march's ([`crate::volumetric`]), drawn beside the grid: a pixel's own ray keeps the shafts' edges, which a froxel's width blurs.
//!
//! The grid is taken from the scene alone every frame (no noise, no history), so a light switched or moved shows in the fog at
//! once. Both grids are `Rgba16Float` 3D textures written as write-only storage, which WebGPU allows.

use torque_render_core::wgpu;

use crate::HDR_FORMAT;
use crate::shaders::Permutation;

pub(crate) const FROXEL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Internal pixels a froxel spans a side (120 x 68 at 1080p: the lights' glow in the fog is soft).
pub const FROXEL_TILE: u32 = 16;
/// The grid's slices from the eye to the medium's reach.
pub const FROXEL_SLICES: u32 = 64;
/// The share of the medium's extinction that scatters the point lights' light (1: all of it, a fog that absorbs nothing).
pub(crate) const FOG_LIGHT_SHARE: f32 = 1.0;
/// The Henyey-Greenstein asymmetry of the fog's scattering of the point lights (a glow about a light seen toward it, a beam
/// still seen from the side).
pub(crate) const FOG_LIGHT_ANISOTROPY: f32 = 0.3;
const UNIFORM_BYTES: u64 = 48;
/// The frames without a gather after which the grid is let go (a pause in the fog's lights, a switch to another quality).
const GRID_IDLE_FRAMES: u32 = 8;
/// The compute shaders' workgroup side (`@workgroup_size(8, 8, 1)`).
const GROUP: u32 = 8;

/// The froxel grid's size for an internal frame of `iw` x `ih`.
pub fn grid_dims(iw: u32, ih: u32) -> [u32; 3] {
    [
        iw.div_ceil(FROXEL_TILE).max(1),
        ih.div_ceil(FROXEL_TILE).max(1),
        FROXEL_SLICES,
    ]
}

/// The view depth of slice coordinate `s` (0 at the eye, [`FROXEL_SLICES`] at `far`), as the shaders place the slices.
pub fn slice_depth(s: f32, far: f32) -> f32 {
    let u = s / FROXEL_SLICES as f32;
    far * u * u
}

struct Grid {
    dims: [u32; 3],
    /// The lit grid and the summed grid.
    lit: wgpu::Texture,
    summed: wgpu::Texture,
    gather_group: wgpu::BindGroup,
    sum_group: wgpu::BindGroup,
    /// The composite's group over the depth it was made with.
    composite: Option<(wgpu::TextureView, wgpu::BindGroup)>,
}

/// The froxel volumetrics' pipelines and grid.
pub struct Froxels {
    layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    gather: wgpu::ComputePipeline,
    sum: wgpu::ComputePipeline,
    composite: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    linear: wgpu::Sampler,
    grid: Option<Grid>,
    /// Whether the last frame drew the grid.
    written: bool,
    /// Whether this frame gathered the grid.
    pub(crate) drawn: bool,
    /// The frames since the grid was last gathered.
    idle: u32,
}

/// What [`Froxels::encode`] draws the grid from: the frame's groups, the depth the composite will read (the prepass's), the
/// internal frame's size and the grid's far end (view depth).
pub(crate) struct GatherArgs<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub frame_group: &'a wgpu::BindGroup,
    pub sky_group: &'a wgpu::BindGroup,
    pub depth: &'a wgpu::TextureView,
    pub internal: (u32, u32),
    pub far: f32,
}

/// The gather's and the sum's group (group 1): the grid's uniform, the grid read, the grid written.
pub(crate) fn layout_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![
        crate::uniform(2, wgpu::ShaderStages::COMPUTE),
        texture_3d(3, wgpu::ShaderStages::COMPUTE),
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: FROXEL_FORMAT,
                view_dimension: wgpu::TextureViewDimension::D3,
            },
            count: None,
        },
    ]
}

/// The composite's group (group 1): the depth, the grid's uniform, the summed grid, a linear sampler.
pub(crate) fn composite_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    let f = wgpu::ShaderStages::FRAGMENT;
    vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: f,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        crate::uniform(2, f),
        texture_3d(3, f),
        wgpu::BindGroupLayoutEntry {
            binding: 5,
            visibility: f,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
    ]
}

fn texture_3d(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D3,
            multisampled: false,
        },
        count: None,
    }
}

fn view(t: &wgpu::Texture) -> wgpu::TextureView {
    t.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The grids of `dims`: the lit and the summed, and the groups over them.
fn make_grid(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    dims: [u32; 3],
) -> Grid {
    let texture = |label: &str| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: dims[0],
                height: dims[1],
                depth_or_array_layers: dims[2],
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: FROXEL_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    };
    let lit = texture("hd froxels lit");
    let summed = texture("hd froxels summed");
    let group = |read: &wgpu::TextureView, write: &wgpu::TextureView| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd froxels"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
                crate::tv(3, read),
                crate::tv(4, write),
            ],
        })
    };
    // (The gather reads nothing of the grid: the summed grid stands in for its read binding.)
    let (lit_view, summed_view) = (view(&lit), view(&summed));
    Grid {
        dims,
        gather_group: group(&summed_view, &lit_view),
        sum_group: group(&lit_view, &summed_view),
        lit,
        summed,
        composite: None,
    }
}

/// The grid in `slot` if it is of `dims`, else one `make` makes (the size changed).
fn ensure_grid(slot: &mut Option<Grid>, dims: [u32; 3], make: impl FnOnce() -> Grid) -> &mut Grid {
    let grid = match slot.take() {
        Some(g) if g.dims == dims => g,
        _ => make(),
    };
    slot.insert(grid)
}

impl Froxels {
    pub fn new(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        frame_layout: &wgpu::BindGroupLayout,
        sky_layout: &wgpu::BindGroupLayout,
    ) -> Froxels {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd froxels"),
            entries: &layout_entries(),
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd froxels composite"),
            entries: &composite_entries(),
        });
        let module = crate::module(device, Permutation::Froxels);
        let groups = [frame_layout, &layout, sky_layout];
        let gather = crate::compute_filter_with(
            device,
            cache,
            "hd froxels gather",
            &groups,
            &module,
            "cs_inject",
        );
        let sum = crate::compute_filter_with(
            device,
            cache,
            "hd froxels sum",
            &groups,
            &module,
            "cs_integrate",
        );
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd froxels composite"),
            bind_group_layouts: &[
                Some(frame_layout),
                Some(&composite_layout),
                Some(sky_layout),
            ],
            immediate_size: 0,
        });
        let add = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hd froxels composite"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_full"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_composite"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: Some(add),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd froxels"),
            size: UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hd froxels linear"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..wgpu::SamplerDescriptor::default()
        });
        Froxels {
            layout,
            composite_layout,
            gather,
            sum,
            composite,
            uniform,
            linear,
            grid: None,
            written: false,
            drawn: false,
            idle: 0,
        }
    }

    /// Writes the grid's uniform and gathers and sums the grid (one compute pass).
    pub(crate) fn encode(&mut self, encoder: &mut wgpu::CommandEncoder, args: &GatherArgs) {
        let GatherArgs {
            device,
            queue,
            frame_group,
            sky_group,
            depth,
            internal,
            far,
        } = *args;
        let dims = grid_dims(internal.0, internal.1);
        let mut b = Vec::with_capacity(UNIFORM_BYTES as usize);
        crate::push_u32(&mut b, &[dims[0], dims[1], dims[2], 0]);
        crate::push_f32(&mut b, &[far, 0.0, 0.0, FOG_LIGHT_SHARE]);
        crate::push_f32(&mut b, &[FOG_LIGHT_ANISOTROPY, 0.0, 0.0, 0.0]);
        debug_assert_eq!(b.len() as u64, UNIFORM_BYTES);
        queue.write_buffer(&self.uniform, 0, &b);
        // (A grid made again below is not the one the last frame showed.)
        self.written = false;
        let grid = ensure_grid(&mut self.grid, dims, || {
            make_grid(device, &self.layout, &self.uniform, dims)
        });
        if grid.composite.as_ref().is_none_or(|(d, _)| d != depth) {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hd froxels composite"),
                layout: &self.composite_layout,
                entries: &[
                    crate::tv(0, depth),
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.uniform.as_entire_binding(),
                    },
                    crate::tv(3, &view(&grid.summed)),
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(&self.linear),
                    },
                ],
            });
            grid.composite = Some((depth.clone(), group));
        }
        crate::count_pass();
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("hd froxels"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, frame_group, &[]);
        pass.set_bind_group(2, sky_group, &[]);
        pass.set_pipeline(&self.gather);
        pass.set_bind_group(1, &grid.gather_group, &[]);
        pass.dispatch_workgroups(dims[0].div_ceil(GROUP), dims[1].div_ceil(GROUP), dims[2]);
        pass.set_pipeline(&self.sum);
        pass.set_bind_group(1, &grid.sum_group, &[]);
        pass.dispatch_workgroups(dims[0].div_ceil(GROUP), dims[1].div_ceil(GROUP), 1);
        self.drawn = true;
    }

    /// The summed grid added over `hdr` (after the opaque pass, over the depth `Froxels::encode` was given).
    pub fn encode_composite(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        frame_group: &wgpu::BindGroup,
        sky_group: &wgpu::BindGroup,
        hdr: &wgpu::TextureView,
    ) {
        let Some((_, group)) = self.grid.as_ref().and_then(|g| g.composite.as_ref()) else {
            return;
        };
        crate::count_pass();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hd froxels composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: hdr,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.composite);
        pass.set_bind_group(0, frame_group, &[]);
        pass.set_bind_group(1, group, &[]);
        pass.set_bind_group(2, sky_group, &[]);
        pass.draw(0..3, 0..1);
        crate::count_draws(1);
    }

    /// After a frame: whether it drew the grid (what the readbacks show). A grid left undrawn for [`GRID_IDLE_FRAMES`] frames is
    /// let go.
    pub(crate) fn advance(&mut self) {
        self.written = self.drawn;
        self.idle = if self.drawn {
            0
        } else {
            self.idle.saturating_add(1)
        };
        if self.idle >= GRID_IDLE_FRAMES {
            self.grid = None;
        }
        self.drawn = false;
    }

    /// Lets go of the composite's group over the last depth (its frame's targets are being let go).
    pub(crate) fn forget_depth(&mut self) {
        if let Some(g) = &mut self.grid {
            g.composite = None;
        }
    }

    /// The last frame's lit grid (rgb: the point lights' light the medium scatters toward the eye per unit length, a: the
    /// medium's extinction) and its size, if it drew one.
    pub fn gathered(&self) -> Option<(&wgpu::Texture, [u32; 3])> {
        let g = self.grid.as_ref().filter(|_| self.written)?;
        Some((&g.lit, g.dims))
    }

    /// The last frame's summed grid (rgb: the light up to each froxel's far boundary, a: the transmittance there).
    pub fn summed(&self) -> Option<(&wgpu::Texture, [u32; 3])> {
        let g = self.grid.as_ref().filter(|_| self.written)?;
        Some((&g.summed, g.dims))
    }
}

#[cfg(test)]
#[path = "froxels_tests.rs"]
mod tests;
