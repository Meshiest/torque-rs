//! The physically based materials' path ([`torque_render_scene::hd::pbr`]) on the GPU: each material's group 1 (forward.wgsl's
//! base texture and sampler, then the normal, ORM and height maps, the material's uniform and the emissive map, `pbr.wgsl`), the
//! loose draws' tangent stream, and the [`crate::shaders::Permutation::ForwardPbr`] modules and pipelines. Nothing here is made
//! until a scene holds a material: a scene without one makes no module, pipeline, group or buffer of this path, so its passes,
//! draws and pictures are the same as before.

use std::collections::HashMap;

use torque_render_core::wgpu;
use torque_render_scene::hd::pbr::{
    MATERIAL_UNIFORM_BYTES, MaterialId, MaterialTable, TANGENT_BYTES,
};

use crate::shaders::Permutation;

/// A material's GPU state: its uniform and group.
struct GpuMaterial {
    _uniform: wgpu::Buffer,
    group: wgpu::BindGroup,
}

pub(crate) struct PbrGpu {
    /// Group 1 of a material's draw.
    layout: wgpu::BindGroupLayout,
    /// The forward layout with group 1 the material's.
    pipeline_layout: wgpu::PipelineLayout,
    /// The modules by (clustered, front layer, parallax).
    modules: HashMap<(bool, bool, bool), wgpu::ShaderModule>,
    materials: Vec<GpuMaterial>,
    /// The maps' sampler: repeating, trilinear and anisotropic.
    sampler: wgpu::Sampler,
    /// The loose draws' tangents, vertex for vertex beside the loose vertex buffer (only a material's draws' are written).
    pub(crate) tangents: Option<wgpu::Buffer>,
    /// Whether the frame marches the height maps (the highest quality's).
    pub(crate) parallax: bool,
}

impl PbrGpu {
    pub(crate) fn new(
        device: &wgpu::Device,
        frame: &wgpu::BindGroupLayout,
        sky: &wgpu::BindGroupLayout,
        refl: &wgpu::BindGroupLayout,
    ) -> PbrGpu {
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd pbr material"),
            entries: &[
                texture(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                texture(2),
                texture(3),
                texture(4),
                crate::uniform(5, wgpu::ShaderStages::FRAGMENT),
                texture(6),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd pbr forward"),
            bind_group_layouts: &[Some(frame), Some(&layout), Some(sky), Some(refl)],
            immediate_size: 0,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hd pbr maps"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..wgpu::SamplerDescriptor::default()
        });
        PbrGpu {
            layout,
            pipeline_layout,
            modules: HashMap::new(),
            materials: Vec::new(),
            sampler,
            tangents: None,
            parallax: false,
        }
    }

    /// The module of `clustered` and `front_layer` at the frame's parallax (made the first time it is asked for), and the
    /// pipeline layout.
    pub(crate) fn module(
        &mut self,
        device: &wgpu::Device,
        clustered: bool,
        front_layer: bool,
    ) -> (&wgpu::ShaderModule, &wgpu::PipelineLayout) {
        let parallax = self.parallax;
        let key = (clustered, front_layer, parallax);
        self.modules.entry(key).or_insert_with(|| {
            crate::module(
                device,
                Permutation::ForwardPbr {
                    clustered,
                    front_layer,
                    parallax,
                },
            )
        });
        (&self.modules[&key], &self.pipeline_layout)
    }

    /// Makes the groups of the scene's materials not yet on the GPU. `textures` are the scene's texture table's views (sRGB),
    /// `white` the view a missing map binds.
    pub(crate) fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        table: &MaterialTable,
        textures: &[wgpu::TextureView],
        white: &wgpu::TextureView,
    ) {
        while self.materials.len() < table.materials.len() {
            let m = &table.materials[self.materials.len()];
            let srgb = |i: Option<usize>| i.and_then(|i| textures.get(i)).unwrap_or(white).clone();
            let linear = |i: Option<usize>| {
                i.and_then(|i| textures.get(i)).map_or_else(
                    || white.clone(),
                    |v| {
                        v.texture().create_view(&wgpu::TextureViewDescriptor {
                            format: Some(wgpu::TextureFormat::Rgba8Unorm),
                            ..wgpu::TextureViewDescriptor::default()
                        })
                    },
                )
            };
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hd pbr material"),
                size: MATERIAL_UNIFORM_BYTES as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&uniform, 0, &m.uniform_bytes());
            let views = [
                srgb(m.base),
                linear(m.normal),
                linear(m.orm),
                linear(m.height),
                srgb(m.emissive),
            ];
            let view = |k: usize| wgpu::BindingResource::TextureView(&views[k]);
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hd pbr material"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: view(0),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: view(1),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: view(2),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: view(3),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: view(4),
                    },
                ],
            });
            self.materials.push(GpuMaterial {
                _uniform: uniform,
                group,
            });
        }
    }

    /// The group of material `id`, once it is on the GPU.
    pub(crate) fn group(&self, id: MaterialId) -> Option<&wgpu::BindGroup> {
        self.materials.get(id.0 as usize).map(|m| &m.group)
    }

    /// Writes each `(first vertex, tangents)` into the tangent stream of `vertices` vertices in all (made again when smaller).
    /// With nothing to write the stream is let go.
    pub(crate) fn write_tangents(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: usize,
        writes: &[(usize, &[[i16; 4]])],
    ) {
        if writes.is_empty() {
            self.tangents = None;
            return;
        }
        let size = (vertices * TANGENT_BYTES).max(TANGENT_BYTES) as u64;
        if self.tangents.as_ref().is_none_or(|b| b.size() < size) {
            self.tangents = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hd pbr tangents"),
                size: size + size / 4,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let Some(buffer) = &self.tangents else {
            return;
        };
        for &(first, tangents) in writes {
            if (first + tangents.len()) * TANGENT_BYTES > size as usize {
                continue;
            }
            let bytes: Vec<u8> = tangents
                .iter()
                .flat_map(|t| t.iter().flat_map(|c| c.to_le_bytes()))
                .collect();
            queue.write_buffer(buffer, (first * TANGENT_BYTES) as u64, &bytes);
        }
    }
}
