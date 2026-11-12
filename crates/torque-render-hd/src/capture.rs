//! The environment's cube maps: the **sky cube** (the sky's draws around the eye, drawn again when the
//! scene's draws change) and the camera-local **light capture** (the opaque scene around the eye, [`CAPTURE_SIZE`]² a face, one
//! face a frame in turn, all six when it starts again), each with its mip chain: the GGX lobe of each level's roughness over the
//! whole cube ([`PrefilterPipe`], `prefilter.wgsl`), filtered after every face drawn that frame, the drawn faces' levels from the
//! cube's level above. A box-filtered chain ([`DownPipe`]) is the clouds' cube's and the color pyramid's. Their
//! faces are drawn by the forward pass's secondary-view module (`Permutation::View`) through
//! `torque_render_scene::hd::reflect::cube_face_camera`: a direction samples what was drawn along it. A capture face's alpha holds
//! 2 + the distance of what it drew (the sky leaves 1 or less) and [`LightCapture::eyes`] the eye it was drawn from: the main view's
//! lookups are moved by how far the eye went since (`reflect.wgsl`'s `capture_dir`), so the faces drawn at different frames agree.

use torque_render_core::wgpu;
use torque_render_scene::hd::reflect::{CAPTURE_MIPS, CAPTURE_SIZE};

use crate::shaders::Permutation;

/// The cube maps' and the colour pyramid's format.
pub const CUBE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The mip filter: a level from the one above it (`down.wgsl`; A compute filter, a chain's levels in one pass).
pub struct DownPipe {
    pub layout: wgpu::BindGroupLayout,
    pub pipeline: wgpu::ComputePipeline,
    /// The sampler's own group, shared by every level (a sampler in each level's group made the backend keep an index
    /// buffer a group: 6 MiB over the cubes' and the colour pyramid's levels).
    sampler_group: wgpu::BindGroup,
}

impl DownPipe {
    pub fn new(device: &wgpu::Device, cache: Option<&wgpu::PipelineCache>) -> DownPipe {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd down"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                crate::storage_target(2, CUBE_FORMAT),
            ],
        });
        let sampler_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd down sampler"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            }],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&Permutation::Down.name()),
            source: wgpu::ShaderSource::Wgsl(Permutation::Down.source().into()),
        });
        let pipeline = crate::compute_filter_with(
            device,
            cache,
            "hd down",
            &[&layout, &sampler_layout],
            &module,
            "cs_down",
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hd down"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..wgpu::SamplerDescriptor::default()
        });
        let sampler_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd down sampler"),
            layout: &sampler_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(&sampler),
            }],
        });
        DownPipe {
            layout,
            pipeline,
            sampler_group,
        }
    }

    /// The group that reads `src` (one mip level's view) and writes `dst` (the level below it).
    pub fn group(
        &self,
        device: &wgpu::Device,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd down"),
            layout: &self.layout,
            entries: &[crate::tv(0, src), crate::tv(2, dst)],
        })
    }

    /// A mip chain: each group's level written from the one above it, in order, in one compute pass (`sizes[k]`: the level
    /// `groups[k]` writes).
    pub fn encode_chain(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        groups: &[wgpu::BindGroup],
        sizes: &[(u32, u32)],
    ) {
        let chain: Vec<crate::Dispatch<'_>> = groups
            .iter()
            .zip(sizes)
            .map(|(group, &size)| crate::Dispatch {
                pipeline: &self.pipeline,
                group,
                size,
            })
            .collect();
        crate::encode_chain_with(encoder, "hd down", &chain, &[&self.sampler_group], None);
    }
}

/// The environment cubes' prefilter (`prefilter.wgsl`): a level of one face from the whole cube's level above it, one pipeline
/// a face (its `FACE` constant).
pub struct PrefilterPipe {
    pub layout: wgpu::BindGroupLayout,
    pipelines: [wgpu::ComputePipeline; 6],
    sampler_group: wgpu::BindGroup,
}

impl PrefilterPipe {
    pub fn new(device: &wgpu::Device, cache: Option<&wgpu::PipelineCache>) -> PrefilterPipe {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd prefilter"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube,
                        multisampled: false,
                    },
                    count: None,
                },
                crate::storage_target(2, CUBE_FORMAT),
            ],
        });
        let sampler_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd prefilter sampler"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            }],
        });
        let module = crate::module(device, Permutation::Prefilter);
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd prefilter"),
            bind_group_layouts: &[Some(&layout), Some(&sampler_layout)],
            immediate_size: 0,
        });
        let pipelines = std::array::from_fn(|face| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("hd prefilter"),
                layout: Some(&pl),
                module: &module,
                entry_point: Some("cs_prefilter"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &[("FACE", face as f64)],
                    ..wgpu::PipelineCompilationOptions::default()
                },
                cache,
            })
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hd prefilter"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..wgpu::SamplerDescriptor::default()
        });
        let sampler_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd prefilter sampler"),
            layout: &sampler_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(&sampler),
            }],
        });
        PrefilterPipe {
            layout,
            pipelines,
            sampler_group,
        }
    }
}

/// The size of a texture's mip `m` (at least 1 x 1).
pub fn mip_size(w: u32, h: u32, m: u32) -> (u32, u32) {
    ((w >> m).max(1), (h >> m).max(1))
}

/// A 2D texture of [`CUBE_FORMAT`] with a box-filtered mip chain down to level `levels - 1` (fewer when it is too small): the
/// texture, the groups that fill each level from the one above, and the levels it has.
pub struct Chained {
    pub texture: wgpu::Texture,
    down: Vec<wgpu::BindGroup>,
    pub levels: u32,
}

impl Chained {
    pub fn new(
        device: &wgpu::Device,
        down: &DownPipe,
        label: &str,
        (w, h): (u32, u32),
        levels: u32,
        usage: wgpu::TextureUsages,
    ) -> Chained {
        let (w, h) = (w.max(1), h.max(1));
        let levels = levels.clamp(1, 32 - w.max(h).leading_zeros());
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CUBE_FORMAT,
            usage: usage
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let views: Vec<wgpu::TextureView> = (0..levels)
            .map(|m| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: m,
                    mip_level_count: Some(1),
                    ..wgpu::TextureViewDescriptor::default()
                })
            })
            .collect();
        let down = views
            .windows(2)
            .map(|v| down.group(device, &v[0], &v[1]))
            .collect();
        Chained {
            texture,
            down,
            levels,
        }
    }

    /// Every level below the first, from level 0 (after it was written).
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, down: &DownPipe) {
        if self.down.is_empty() {
            return;
        }
        let (w, h) = (self.texture.width(), self.texture.height());
        let sizes: Vec<(u32, u32)> = (1..self.levels).map(|m| mip_size(w, h, m)).collect();
        down.encode_chain(encoder, &self.down, &sizes);
    }
}

/// The checks write a cube's texture and read it back (`capture_tests.rs`); a build's cubes need neither.
const CHECK_USAGES: wgpu::TextureUsages = if cfg!(test) {
    wgpu::TextureUsages::COPY_SRC.union(wgpu::TextureUsages::COPY_DST)
} else {
    wgpu::TextureUsages::empty()
};

/// How a cube's levels below the first are made.
enum Mips {
    /// `[f][m]`: reads face `f`'s level `m` to fill its level `m + 1` (a 2 x 2 box, [`DownPipe`]).
    Box(Vec<Vec<wgpu::BindGroup>>),
    /// `[m][f]`: reads the whole cube's level `m` to fill face `f`'s level `m + 1` (the GGX lobe, [`PrefilterPipe`]).
    Ggx(Vec<Vec<wgpu::BindGroup>>),
}

/// A cube map with its mip chain: each face's level views (render targets) and the groups that filter each level from the one above.
pub struct Cube {
    pub view: wgpu::TextureView,
    /// `faces[f][m]`: face `f`'s level `m`.
    pub faces: Vec<Vec<wgpu::TextureView>>,
    mips: Mips,
    texture: wgpu::Texture,
}

impl Cube {
    /// A cube whose levels are box-filtered from each face's level above ([`Cube::encode_mips`]).
    pub fn new(device: &wgpu::Device, label: &str, size: u32, mips: u32, down: &DownPipe) -> Cube {
        let (texture, view, faces) = Cube::views(device, label, size, mips);
        let groups = faces
            .iter()
            .map(|levels| {
                levels
                    .windows(2)
                    .map(|v| down.group(device, &v[0], &v[1]))
                    .collect()
            })
            .collect();
        Cube {
            view,
            faces,
            mips: Mips::Box(groups),
            texture,
        }
    }

    /// A cube whose levels are GGX-prefiltered from the whole cube's level above ([`Cube::encode_prefilter`]).
    pub fn prefiltered(
        device: &wgpu::Device,
        label: &str,
        size: u32,
        mips: u32,
        prefilter: &PrefilterPipe,
    ) -> Cube {
        let (texture, view, faces) = Cube::views(device, label, size, mips);
        let groups = (1..mips)
            .map(|m| {
                let above = texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(label),
                    dimension: Some(wgpu::TextureViewDimension::Cube),
                    base_mip_level: m - 1,
                    mip_level_count: Some(1),
                    ..wgpu::TextureViewDescriptor::default()
                });
                faces
                    .iter()
                    .map(|levels| {
                        device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("hd prefilter"),
                            layout: &prefilter.layout,
                            entries: &[crate::tv(0, &above), crate::tv(2, &levels[m as usize])],
                        })
                    })
                    .collect()
            })
            .collect();
        Cube {
            view,
            faces,
            mips: Mips::Ggx(groups),
            texture,
        }
    }

    /// The texture, its cube view and its faces' level views.
    fn views(
        device: &wgpu::Device,
        label: &str,
        size: u32,
        mips: u32,
    ) -> (
        wgpu::Texture,
        wgpu::TextureView,
        Vec<Vec<wgpu::TextureView>>,
    ) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 6,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CUBE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | CHECK_USAGES,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some(label),
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..wgpu::TextureViewDescriptor::default()
        });
        let faces: Vec<Vec<wgpu::TextureView>> = (0..6)
            .map(|f| {
                (0..mips)
                    .map(|m| {
                        texture.create_view(&wgpu::TextureViewDescriptor {
                            label: Some(label),
                            dimension: Some(wgpu::TextureViewDimension::D2),
                            base_mip_level: m,
                            mip_level_count: Some(1),
                            base_array_layer: f,
                            array_layer_count: Some(1),
                            ..wgpu::TextureViewDescriptor::default()
                        })
                    })
                    .collect()
            })
            .collect();
        (texture, view, faces)
    }

    /// Face `face`'s mip chain from its level 0 (a box-filtered cube's; a prefiltered cube's are [`Cube::encode_prefilter`]'s).
    pub fn encode_mips(&self, encoder: &mut wgpu::CommandEncoder, face: usize, down: &DownPipe) {
        let Mips::Box(groups) = &self.mips else {
            return;
        };
        let s = self.texture.width();
        let sizes: Vec<(u32, u32)> = (1..=groups[face].len() as u32)
            .map(|m| mip_size(s, s, m))
            .collect();
        down.encode_chain(encoder, &groups[face], &sizes);
    }

    /// The prefiltered levels of `faces` (each drawn this frame) in one compute pass, a level of every one of them before the
    /// next level: a face's level reads its neighbors' level above, so every face drawn must have that level first (the faces
    /// not drawn keep the levels they were filtered with). Nothing for a box-filtered cube.
    pub fn encode_prefilter(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        faces: impl Iterator<Item = usize> + Clone,
        prefilter: &PrefilterPipe,
    ) {
        let Mips::Ggx(groups) = &self.mips else {
            return;
        };
        let s = self.texture.width();
        let chain: Vec<crate::Dispatch<'_>> = groups
            .iter()
            .enumerate()
            .flat_map(|(k, level)| {
                let size = mip_size(s, s, k as u32 + 1);
                faces.clone().map(move |f| crate::Dispatch {
                    pipeline: &prefilter.pipelines[f],
                    group: &level[f],
                    size,
                })
            })
            .collect();
        crate::encode_chain_with(
            encoder,
            "hd prefilter",
            &chain,
            &[&prefilter.sampler_group],
            None,
        );
    }

    pub fn bytes(&self) -> u64 {
        let s = u64::from(self.texture.width());
        // Six faces, 8 bytes a texel, a third more for the mips.
        6 * s * s * 8 * 4 / 3
    }
}

/// The two cube maps, their depth, and which faces are drawn when.
pub struct LightCapture {
    pub scene: Cube,
    pub sky: Cube,
    pub depth: wgpu::TextureView,
    /// Each face's frame uniform (the capture's and the sky cube's faces are drawn from the same eye: one a face).
    pub frames: Vec<wgpu::Buffer>,
    /// Whether every face of the capture was drawn since it started again, and the scene draws the sky cube holds.
    pub valid: bool,
    pub sky_generation: Option<u64>,
    /// The face drawn next.
    pub next: usize,
    /// A face was drawn without every moving shape it may show (the scene's shapes were kept for other faces): the next frame
    /// draws all six again.
    pub refresh: bool,
    /// The eye each face was last drawn from (its lookups from a later eye are moved by the difference: `reflect.wgsl`'s
    /// `capture_dir`).
    pub eyes: [[f32; 3]; 6],
}

impl LightCapture {
    /// `frame_bytes`: the frame uniform's size (the engine's fields and the extension's block).
    pub fn new(device: &wgpu::Device, prefilter: &PrefilterPipe, frame_bytes: u64) -> LightCapture {
        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("hd capture depth"),
                size: wgpu::Extent3d {
                    width: CAPTURE_SIZE,
                    height: CAPTURE_SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        let frames = (0..12)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("hd capture frame"),
                    size: frame_bytes,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        LightCapture {
            scene: Cube::prefiltered(device, "hd capture", CAPTURE_SIZE, CAPTURE_MIPS, prefilter),
            sky: Cube::prefiltered(device, "hd sky cube", CAPTURE_SIZE, CAPTURE_MIPS, prefilter),
            depth,
            frames,
            valid: false,
            sky_generation: None,
            next: 0,
            refresh: false,
            eyes: [[0.0; 3]; 6],
        }
    }

    /// The faces [`LightCapture::faces`] draws next, without drawing them.
    pub fn faces_next(&self) -> [bool; 6] {
        if self.valid && !self.refresh {
            std::array::from_fn(|f| f == self.next)
        } else {
            [true; 6]
        }
    }

    /// The faces to draw this frame: all six when the capture starts again (or [`LightCapture::refresh`]), else the next one in turn.
    pub fn faces(&mut self) -> Vec<usize> {
        if self.valid && !self.refresh {
            let f = self.next;
            self.next = (self.next + 1) % 6;
            vec![f]
        } else {
            self.next = 0;
            self.valid = true;
            self.refresh = false;
            (0..6).collect()
        }
    }

    pub fn bytes(&self) -> u64 {
        self.scene.bytes() + self.sky.bytes()
    }
}

#[cfg(test)]
#[path = "capture_tests.rs"]
mod tests;
