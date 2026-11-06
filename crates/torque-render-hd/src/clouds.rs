//! The volumetric clouds on the GPU (the data, the layers, the ladder and the noise are `torque_render_scene::hd::clouds`;
//! `clouds.wgsl`): the noise volumes and the cirrus plane uploaded once with their mips, then each frame the cloud shadow map (256²
//! around the eye on the lowest base's plane), the march of the quality's layers at its reduced size (their light, the distance and
//! their motion; it reads the sky's group for the atmosphere's tables), the resolve into one of two histories (each frame reads the
//! other: the temporal reprojection, held within the march's neighbourhood), and the history copied into the image the sky pass
//! composites (`sky.wgsl`'s `fs_clouds`, in front of the server's flat cloud layers). The image and the shadow map are in the sky
//! group (bindings 6 and 7): the forward pass reads the shadow at Ultra (the clouds' shadows on the ground) and the volumetric light
//! always (no god rays under a cloud).

use glam::Vec3;
use torque_render_core::wgpu;
use torque_render_scene::hd::clouds::{
    self as cl, BLUE_SIZE, CIRRUS_SIZE, CLOUD_SUN, CloudLook, CloudQuality, DETAIL_SIZE, LayerForm,
    LayerProfile, MAX_LAYERS, SHADOW_STEPS, SHADOW_TEXELS, SHAPE_SIZE, SLAB, SUN_RAYS,
    SUN_SEGMENTS, UNITS_PER_KM, WEATHER_SIZE,
};
use torque_render_scene::hd::{HdCamera, HdScene, srgb_to_linear};

use crate::push_f32;
use crate::shaders::Permutation;

/// The clouds' image, histories and shadow map format.
pub const CLOUD_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The march's distance target (r the distance over the shader's DIST_SCALE, gb the layers' motion since the last frame).
const DIST_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The clouds' uniform (`clouds.wgsl`'s `Clouds`: [`MAX_LAYERS`] layers of 8 vec4, then 16 vec4).
const LAYER_VEC4S: usize = 8;
const UNIFORM_BYTES: u64 = ((MAX_LAYERS * LAYER_VEC4S + 16) * 16) as u64;
/// The clouds' shadows on the ground at the qualities that cast them (the share of the sun a full cloud takes away).
pub const GROUND_SHADOW: f32 = 0.4;

/// The noise volumes on the GPU and the march's group (the uniform, the volumes, their sampler).
struct Noise {
    group: wgpu::BindGroup,
}

/// The per-size targets: the march's light and distance, the two histories, the image the sky pass reads, the resolve's groups
/// (one per history written).
struct Targets {
    key: (u32, u32),
    low: u32,
    current_view: wgpu::TextureView,
    dist_view: wgpu::TextureView,
    history: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
    /// The image with its box-filtered mips (the fog takes the clouds without their detail from a low one).
    out: crate::capture::Chained,
    out_view: wgpu::TextureView,
    groups: [wgpu::BindGroup; 2],
}

/// What a frame's clouds give the sky uniform: the shadow map's place (centre, 1 / span, the base), the uses (the ground's
/// shadow, the volumetric light's, the image's uv scale) and the eye's view of the sun through them (its strength, 0 off; the
/// sines of the sun's elevation over which it hands over to the map; the glow's reach about the sun).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SkyClouds {
    pub map: [f32; 4],
    pub uses: [f32; 4],
    pub sun: [f32; 4],
    /// The image's mip level the fog reads (its clouds without their detail; 0 without clouds).
    pub low: f32,
    /// The volume layers drawn, lowest first: the first's base and top, the second's (0 0 none): the forward pass lays the clouds
    /// over a surface the ray reaches through one.
    pub slabs: [f32; 4],
}

pub struct Clouds {
    frame_layout: wgpu::BindGroupLayout,
    march_layout: wgpu::BindGroupLayout,
    resolve_layout: wgpu::BindGroupLayout,
    module: wgpu::ShaderModule,
    /// The march reads the sky's group: made once its layout is known ([`Clouds::set_sky_layout`]).
    march: Option<wgpu::RenderPipeline>,
    resolve: wgpu::RenderPipeline,
    shadow: wgpu::RenderPipeline,
    /// The sun's transmittance through the clouds as the eye sees it: the rays' parts, then their mean (one texel, in the sky
    /// group), and the mean's group (the uniform and the parts).
    sun_rays: wgpu::RenderPipeline,
    sun_mean: wgpu::RenderPipeline,
    sun_rays_map: wgpu::TextureView,
    sun_texture: wgpu::Texture,
    sun_map: wgpu::TextureView,
    sun_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    repeat: wgpu::Sampler,
    clamp: wgpu::Sampler,
    noise: Option<Noise>,
    targets: Option<Targets>,
    shadow_map: wgpu::TextureView,
    /// A 1 x 1 image (no clouds) for the sky group before the first targets.
    blank: wgpu::TextureView,
    /// The history written this frame (the other is last frame's), whether last frame wrote one, the frames since a reset.
    parity: usize,
    written: bool,
    frame: u64,
    /// Last frame's drift of each layer drawn (the reprojection's displacement), and those layers' mask.
    prev_drift: Option<([[f32; 2]; MAX_LAYERS], u32)>,
    /// Whether the sky group holds this state's current image (else it must be bound again).
    bound: bool,
    /// Whether this frame marched.
    pub drawn: bool,
    /// The image's mip chain.
    down: crate::capture::DownPipe,
    /// The march from the eye wherever it is (the renderer's choice); off: from no higher than [`CLAMPED_UNDER`] under the
    /// lowest base, as before.
    pub from_eye: bool,
}

/// How far under the lowest base the march started at most before it started at the eye.
pub const CLAMPED_UNDER: f32 = 60.0;

fn entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty,
        count: None,
    }
}

fn texture_entry(binding: u32, dim: wgpu::TextureViewDimension) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dim,
            multisampled: false,
        },
    )
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
    )
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
    )
}

fn plain_texture(
    device: &wgpu::Device,
    label: &str,
    (w, h): (u32, u32),
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// A noise volume (`depth` 1: a plane) with its mip chain, RGBA8.
fn noise_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    data: &[u8],
    size: usize,
    depth: usize,
) -> wgpu::TextureView {
    let levels = cl::mips(data, size, depth);
    let d3 = depth > 1;
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size as u32,
            height: size as u32,
            depth_or_array_layers: depth as u32,
        },
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: if d3 {
            wgpu::TextureDimension::D3
        } else {
            wgpu::TextureDimension::D2
        },
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (i, (s, d, bytes)) in levels.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: i as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * *s as u32),
                rows_per_image: Some(*s as u32),
            },
            wgpu::Extent3d {
                width: *s as u32,
                height: *s as u32,
                depth_or_array_layers: *d as u32,
            },
        );
    }
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

/// A full-frame pipeline of `module`'s `entry` over `layouts` into `targets`.
fn pipeline(
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    module: &wgpu::ShaderModule,
    entry: &str,
    layouts: &[Option<&wgpu::BindGroupLayout>],
    targets: &[Option<wgpu::ColorTargetState>],
) -> wgpu::RenderPipeline {
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(entry),
        bind_group_layouts: layouts,
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(entry),
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
            targets,
        }),
        multiview_mask: None,
        cache,
    })
}

fn target(format: wgpu::TextureFormat) -> Option<wgpu::ColorTargetState> {
    Some(wgpu::ColorTargetState {
        format,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    })
}

impl Clouds {
    pub fn new(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        frame_layout: &wgpu::BindGroupLayout,
    ) -> Clouds {
        let march_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd clouds"),
            entries: &[
                uniform_entry(0),
                texture_entry(1, wgpu::TextureViewDimension::D3),
                texture_entry(2, wgpu::TextureViewDimension::D3),
                texture_entry(3, wgpu::TextureViewDimension::D2),
                texture_entry(4, wgpu::TextureViewDimension::D2),
                sampler_entry(5),
                texture_entry(6, wgpu::TextureViewDimension::D2),
            ],
        });
        let resolve_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd clouds resolve"),
            entries: &[
                uniform_entry(10),
                texture_entry(11, wgpu::TextureViewDimension::D2),
                texture_entry(12, wgpu::TextureViewDimension::D2),
                texture_entry(13, wgpu::TextureViewDimension::D2),
                sampler_entry(14),
            ],
        });
        let module = crate::module(device, Permutation::Clouds);
        let shadow = pipeline(
            device,
            cache,
            &module,
            "fs_shadow",
            &[Some(frame_layout), Some(&march_layout)],
            &[target(CLOUD_FORMAT)],
        );
        let resolve = pipeline(
            device,
            cache,
            &module,
            "fs_resolve",
            &[Some(frame_layout), Some(&resolve_layout)],
            &[target(CLOUD_FORMAT)],
        );
        let sun_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd clouds sun"),
            entries: &[
                uniform_entry(10),
                texture_entry(15, wgpu::TextureViewDimension::D2),
            ],
        });
        let sun_rays = pipeline(
            device,
            cache,
            &module,
            "fs_sun_rays",
            &[Some(frame_layout), Some(&march_layout)],
            &[target(CLOUD_FORMAT)],
        );
        let sun_mean = pipeline(
            device,
            cache,
            &module,
            "fs_sun_mean",
            &[Some(frame_layout), Some(&sun_layout)],
            &[target(CLOUD_FORMAT)],
        );
        let sampler = |label: &str, mode: wgpu::AddressMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: mode,
                address_mode_v: mode,
                address_mode_w: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..wgpu::SamplerDescriptor::default()
            })
        };
        let shadow_map = plain_texture(
            device,
            "hd cloud shadow",
            (SHADOW_TEXELS, SHADOW_TEXELS),
            CLOUD_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        let sun_usage =
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let sun_rays_map = plain_texture(
            device,
            "hd cloud sun rays",
            (SUN_RAYS * SUN_SEGMENTS, MAX_LAYERS as u32),
            CLOUD_FORMAT,
            sun_usage,
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        let sun_texture = plain_texture(
            device,
            "hd cloud sun",
            (1, 1),
            CLOUD_FORMAT,
            sun_usage | wgpu::TextureUsages::COPY_SRC,
        );
        let sun_map = sun_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd clouds"),
            size: UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sun_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd clouds sun"),
            layout: &sun_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: uniform.as_entire_binding(),
                },
                crate::tv(15, &sun_rays_map),
            ],
        });
        let blank = plain_texture(
            device,
            "hd clouds blank",
            (1, 1),
            CLOUD_FORMAT,
            wgpu::TextureUsages::TEXTURE_BINDING,
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        Clouds {
            down: crate::capture::DownPipe::new(device, cache),
            from_eye: true,
            frame_layout: frame_layout.clone(),
            march_layout,
            resolve_layout,
            module,
            march: None,
            resolve,
            shadow,
            sun_rays,
            sun_mean,
            sun_rays_map,
            sun_texture,
            sun_map,
            sun_group,
            uniform,
            repeat: sampler("hd clouds repeat", wgpu::AddressMode::Repeat),
            clamp: sampler("hd clouds clamp", wgpu::AddressMode::ClampToEdge),
            noise: None,
            targets: None,
            shadow_map,
            blank,
            parity: 0,
            written: false,
            frame: 0,
            prev_drift: None,
            bound: false,
            drawn: false,
        }
    }

    /// Makes the march's pipeline over the sky's group layout (its third group: the atmosphere's tables).
    pub fn set_sky_layout(
        &mut self,
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        sky_layout: &wgpu::BindGroupLayout,
    ) {
        self.march = Some(pipeline(
            device,
            cache,
            &self.module,
            "fs_march",
            &[
                Some(&self.frame_layout),
                Some(&self.march_layout),
                Some(sky_layout),
            ],
            &[target(CLOUD_FORMAT), target(DIST_FORMAT)],
        ));
    }

    /// The image the sky pass composites, the shadow map and the sun through the clouds (for the sky group).
    pub fn views(&self) -> (&wgpu::TextureView, &wgpu::TextureView, &wgpu::TextureView) {
        (
            self.targets.as_ref().map_or(&self.blank, |t| &t.out_view),
            &self.shadow_map,
            &self.sun_map,
        )
    }

    /// Whether the sky group must be bound again with [`Clouds::views`] (the image changed); clears the flag.
    pub fn take_rebind(&mut self) -> bool {
        !std::mem::replace(&mut self.bound, true)
    }

    /// Starts the history again (a cut).
    pub fn reset(&mut self) {
        self.written = false;
        self.frame = 0;
        self.prev_drift = None;
    }

    fn ensure_noise(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.noise.is_some() {
            return;
        }
        let n = cl::noise();
        let blue = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hd clouds blue noise"),
            size: wgpu::Extent3d {
                width: BLUE_SIZE as u32,
                height: BLUE_SIZE as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &blue,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &n.blue,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(BLUE_SIZE as u32),
                rows_per_image: Some(BLUE_SIZE as u32),
            },
            wgpu::Extent3d {
                width: BLUE_SIZE as u32,
                height: BLUE_SIZE as u32,
                depth_or_array_layers: 1,
            },
        );
        let shape = noise_texture(
            device,
            queue,
            "hd clouds shape",
            &n.shape,
            SHAPE_SIZE,
            SHAPE_SIZE,
        );
        let detail = noise_texture(
            device,
            queue,
            "hd clouds detail",
            &n.detail,
            DETAIL_SIZE,
            DETAIL_SIZE,
        );
        let weather = noise_texture(
            device,
            queue,
            "hd clouds weather",
            &n.weather,
            WEATHER_SIZE,
            1,
        );
        let cirrus = noise_texture(device, queue, "hd clouds cirrus", &n.cirrus, CIRRUS_SIZE, 1);
        let blue = blue.create_view(&wgpu::TextureViewDescriptor::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd clouds"),
            layout: &self.march_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                crate::tv(1, &shape),
                crate::tv(2, &detail),
                crate::tv(3, &weather),
                crate::tv(4, &blue),
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&self.repeat),
                },
                crate::tv(6, &cirrus),
            ],
        });
        self.noise = Some(Noise { group });
    }

    /// The targets at the march's size `key`, the image's mips down to level `low`.
    fn ensure_targets(&mut self, device: &wgpu::Device, key: (u32, u32), low: u32) {
        if self
            .targets
            .as_ref()
            .is_some_and(|t| t.key == key && t.low == low)
        {
            return;
        }
        let attach = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let current = plain_texture(device, "hd clouds march", key, CLOUD_FORMAT, attach);
        let dist = plain_texture(device, "hd clouds distance", key, DIST_FORMAT, attach);
        let usage = attach | wgpu::TextureUsages::COPY_SRC;
        let history = [
            plain_texture(device, "hd clouds history 0", key, CLOUD_FORMAT, usage),
            plain_texture(device, "hd clouds history 1", key, CLOUD_FORMAT, usage),
        ];
        let views = [0, 1].map(|k| history[k].create_view(&wgpu::TextureViewDescriptor::default()));
        let out = crate::capture::Chained::new(
            device,
            &self.down,
            "hd clouds image",
            key,
            low + 1,
            wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
        );
        let out_view = out
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let current_view = current.create_view(&wgpu::TextureViewDescriptor::default());
        let dist_view = dist.create_view(&wgpu::TextureViewDescriptor::default());
        // The resolve writing history k reads the other.
        let group = |prev: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hd clouds resolve"),
                layout: &self.resolve_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: self.uniform.as_entire_binding(),
                    },
                    crate::tv(11, &current_view),
                    crate::tv(12, &dist_view),
                    crate::tv(13, prev),
                    wgpu::BindGroupEntry {
                        binding: 14,
                        resource: wgpu::BindingResource::Sampler(&self.clamp),
                    },
                ],
            })
        };
        let groups = [group(&views[1]), group(&views[0])];
        self.targets = Some(Targets {
            key,
            low,
            current_view,
            dist_view,
            history,
            views,
            out,
            out_view,
            groups,
        });
        self.written = false;
        self.bound = false;
    }

    /// The frame's clouds: `quality` None (or a scene without a server sky, or no layer) draws none. Otherwise the noise and the
    /// targets are made when needed and the uniform written for `camera` (the internal, unjittered camera) at the internal size
    /// `(iw, ih)`, the quality's layers of the fair-weather set lowest first. `sun_through`: a low sun's light through the clouds
    /// as the eye sees it ([`CLOUD_SUN`]); `time`: the effect time (the drift). Returns what the sky uniform takes.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare<G>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &HdScene<G>,
        time: f32,
        camera: &HdCamera,
        (iw, ih): (u32, u32),
        quality: Option<CloudQuality>,
        sun_through: bool,
    ) -> SkyClouds {
        let (Some(q), Some(sky)) = (quality, scene.clouds.as_ref()) else {
            self.drawn = false;
            return SkyClouds::default();
        };
        // The scene's look (the engine's without one): its layers, light and coverage, and what it asks of the quality.
        let look = scene
            .cloud_look
            .map_or(CloudLook::DEFAULT, |l| l.sanitized());
        let q = look.quality(q);
        let coverage = look.coverage.unwrap_or(sky.coverage);
        let mut layers = cl::drawn_layers(&look.layers, q.layers);
        // Over a world that rises into the deck, every layer is raised over it.
        let lift = if sky.lift.is_finite() {
            sky.lift.clamp(0.0, cl::MAX_LIFT)
        } else {
            0.0
        };
        for l in &mut layers {
            l.base += lift;
            l.top += lift;
        }
        if self.march.is_none() || layers.is_empty() {
            self.drawn = false;
            return SkyClouds::default();
        }
        self.ensure_noise(device, queue);
        let scale = q.scale.max(1);
        let key = (iw.div_ceil(scale), ih.div_ceil(scale));
        // The fog's clouds without their detail: the image's mean over cells of FOG_LOW_LEVEL's size in the internal frame.
        let low_level = crate::FOG_LOW_LEVEL.saturating_sub(scale.ilog2());
        self.ensure_targets(device, key, low_level);
        if self.targets.is_none() {
            self.drawn = false;
            return SkyClouds::default();
        }
        let t = time;
        let mut drift = [[0.0f32; 2]; MAX_LAYERS];
        for (d, l) in drift.iter_mut().zip(&layers) {
            *d = cl::drift_at(cl::layer_drift(sky.drift, l), t);
        }
        let prev = self.prev_drift.filter(|(_, mask)| *mask == q.layers);
        let delta = |k: usize| {
            prev.map_or([0.0; 2], |(p, _)| {
                [drift[k][0] - p[k][0], drift[k][1] - p[k][1]]
            })
        };
        let valid = self.written && prev.is_some();
        let s = {
            let v = Vec3::from(scene.sun.towards);
            (v / v.length().max(1e-6)).to_array()
        };
        let sun = &scene.sun;
        // An overcast's light is neutral (cl::OVERCAST): the sun's light on the clouds, their tint and the ambient lose their colour.
        let overcast = cl::overcast_share(coverage);
        let neutral = 1.0 - overcast;
        // The look's own sunlight through the air while the scene's light is the sun (under the moon the scene's).
        let by_sun = scene.atmosphere_sun.is_none_or(|a| {
            let a = Vec3::from(a);
            a.dot(s.into()) / a.length().max(1e-6) > 0.999
        });
        let physical = look.sun.filter(|_| by_sun);
        let sun_light = cl::keep_chroma(
            physical.unwrap_or_else(|| {
                [0, 1, 2].map(|k| {
                    (srgb_to_linear((sun.ambient[k] + sun.diffuse[k]).max(0.0))
                        - srgb_to_linear(sun.ambient[k].max(0.0)))
                    .max(0.0)
                })
            }),
            neutral,
        );
        let lowest = layers[0];
        let (centre, span) =
            cl::shadow_window_at(camera.eye, s, scene.visible_distance, lowest.base);
        let pixel_angle = (camera.frustum[3] - camera.frustum[2]).abs()
            / camera.near.max(1e-6)
            / ih.max(1) as f32;
        let l = look.light;
        let uv = [
            iw as f32 / (scale * key.0) as f32,
            ih as f32 / (scale * key.1) as f32,
        ];
        // The march starts at the eye: in a layer, over it or under it (the layers seen from above, from inside, from below).
        let start = if self.from_eye {
            camera.eye[2]
        } else {
            camera.eye[2].min(lowest.base - CLAMPED_UNDER)
        };
        // The atmosphere's radius at the eye (km) and each layer's over it.
        let eye_radius = torque_render_scene::hd::atmosphere::EARTH.bottom as f32
            + torque_render_scene::hd::atmosphere::CAMERA_HEIGHT as f32;
        let mut b = Vec::with_capacity(UNIFORM_BYTES as usize);
        let mut below = 0.0f32;
        for (k, here) in drift.iter().enumerate() {
            let Some(layer) = layers.get(k) else {
                b.extend(std::iter::repeat_n(0u8, LAYER_VEC4S * 16));
                continue;
            };
            let sheet = layer.form == LayerForm::Sheet;
            let coverage = cl::layer_coverage(coverage, layer);
            let steps = if sheet {
                0.0
            } else {
                ((q.steps as f32 * layer.steps).round()).max(4.0)
            };
            let light_steps = if layer.light_steps == 0 {
                q.light_steps
            } else {
                layer.light_steps.min(q.light_steps)
            };
            let texel = if sheet {
                layer.shape_period / CIRRUS_SIZE as f32
            } else {
                layer.shape_period / SHAPE_SIZE as f32
            } * (-layer.blur).exp2();
            let d = delta(k);
            push_f32(
                &mut b,
                &[layer.base, layer.top, if sheet { 1.0 } else { 0.0 }, steps],
            );
            push_f32(
                &mut b,
                &[
                    coverage,
                    layer.extinction * sky.density,
                    1.0 / layer.shape_period,
                    1.0 / layer.detail_period,
                ],
            );
            push_f32(
                &mut b,
                &[
                    1.0 / layer.weather_period,
                    layer.detail,
                    layer.variation,
                    light_steps as f32,
                ],
            );
            push_f32(
                &mut b,
                &[
                    -here[0],
                    -here[1],
                    (layer.evolve * t).rem_euclid(layer.shape_period * 8.0),
                    texel,
                ],
            );
            // A sheet's fibres lie along its wind (its domain turned to it); a volume's domain is turned off the world's axes.
            let turn = if sheet {
                let w = cl::layer_drift(sky.drift, layer);
                -w[1].atan2(w[0])
            } else {
                layer.turn
            };
            push_f32(&mut b, &[d[0], d[1], turn.cos(), turn.sin()]);
            push_f32(
                &mut b,
                &[
                    layer.edge_base,
                    layer.edge_top,
                    layer.top_density,
                    if layer.profile == LayerProfile::Elements {
                        1.0
                    } else {
                        0.0
                    },
                ],
            );
            push_f32(
                &mut b,
                &[layer.aerial, layer.fade.0, layer.fade.1, layer.reach],
            );
            push_f32(
                &mut b,
                &[
                    layer.sun,
                    layer.ambient,
                    eye_radius + 0.5 * (layer.base + layer.top) / UNITS_PER_KM,
                    below,
                ],
            );
            // The next layer up sees this one's cover under it (the layers' combined share).
            below = 1.0 - (1.0 - below) * (1.0 - coverage);
        }
        push_f32(
            &mut b,
            &[
                layers.len() as f32,
                start,
                SLAB.haze,
                if valid { 1.0 } else { 0.0 },
            ],
        );
        push_f32(&mut b, &[s[0], s[1], s[2], l.powder]);
        push_f32(
            &mut b,
            &[sun_light[0], sun_light[1], sun_light[2], l.octaves as f32],
        );
        // The server box's colours: only some of their colour (a painted box), none under an overcast.
        let box_chroma = cl::OVERCAST.box_chroma * neutral;
        let top = cl::keep_chroma(sky.ambient_top, box_chroma);
        // The undersides with the server's box: some of its horizon's colour (the march adds the light the ground and the decks
        // under a layer send up; with our own sky it takes the horizon from the atmosphere).
        let low = cl::keep_chroma(sky.ambient_horizon, box_chroma).map(|c| c * l.underside);
        let tint = cl::keep_chroma(sky.tint, neutral);
        push_f32(&mut b, &[top[0], top[1], top[2], l.ambient_floor]);
        push_f32(&mut b, &[low[0], low[1], low[2], l.multiple]);
        push_f32(&mut b, &[tint[0], tint[1], tint[2], l.multiple_decay]);
        push_f32(&mut b, &[l.forward_g, l.back_g, l.back_share, l.light_step]);
        push_f32(
            &mut b,
            &[
                (self.frame % 1024) as f32,
                q.history,
                scale as f32,
                if q.detail { 1.0 } else { 0.0 },
            ],
        );
        push_f32(&mut b, &[uv[0], uv[1], key.0 as f32, key.1 as f32]);
        push_f32(&mut b, &[centre[0], centre[1], span, SHADOW_STEPS as f32]);
        push_f32(
            &mut b,
            &[
                pixel_angle,
                SHADOW_TEXELS as f32,
                q.spread,
                if scene.atmosphere { 1.0 } else { 0.0 },
            ],
        );
        push_f32(
            &mut b,
            &[
                l.octave_energy,
                l.octave_extinction,
                l.octave_eccentricity,
                l.sun_tint,
            ],
        );
        push_f32(
            &mut b,
            &[l.bounce, l.underside, l.layer_shadow, l.layer_slant],
        );
        push_f32(
            &mut b,
            &[
                l.far_shadow,
                l.near_steps as f32,
                l.far_view.0,
                l.far_view.1,
            ],
        );
        push_f32(
            &mut b,
            &[l.thin_slant, l.thin_power, l.thin_density, CLOUD_SUN.spread],
        );
        push_f32(
            &mut b,
            &[
                overcast,
                if physical.is_some() { 1.0 } else { 0.0 },
                look.fine_step,
                look.fine_budget,
            ],
        );
        debug_assert_eq!(b.len() as u64, UNIFORM_BYTES);
        queue.write_buffer(&self.uniform, 0, &b);
        self.drawn = true;
        self.prev_drift = Some((drift, q.layers));
        SkyClouds {
            map: [centre[0], centre[1], 1.0 / span, lowest.base],
            uses: [
                if q.ground_shadows {
                    look.ground_shadow.unwrap_or(GROUND_SHADOW)
                } else {
                    0.0
                },
                1.0,
                uv[0],
                uv[1],
            ],
            sun: [
                if sun_through { CLOUD_SUN.strength } else { 0.0 },
                CLOUD_SUN.low.0,
                CLOUD_SUN.low.1,
                CLOUD_SUN.aureole,
            ],
            low: low_level as f32,
            slabs: {
                let mut v = layers
                    .iter()
                    .filter(|l| self.from_eye && l.form == LayerForm::Volume)
                    .flat_map(|l| [l.base, l.top]);
                [0; 4].map(|_| v.next().unwrap_or(0.0))
            },
        }
    }

    /// The shadow map, the march and the resolve (the timestamps at the shadow pass's start and the resolve's end), then the
    /// history copied into the image the sky pass reads. `sky_group`: the sky's group (the march reads the atmosphere's tables).
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        frame_group: &wgpu::BindGroup,
        sky_group: &wgpu::BindGroup,
        stamps: Option<(&wgpu::QuerySet, u32, u32)>,
    ) {
        let (Some(t), Some(n), Some(march)) = (
            self.targets.as_ref().filter(|_| self.drawn),
            self.noise.as_ref(),
            self.march.as_ref(),
        ) else {
            return;
        };
        {
            crate::count_pass();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hd cloud shadow"),
                color_attachments: &[Some(crate::colour(&self.shadow_map))],
                depth_stencil_attachment: None,
                timestamp_writes: stamps.map(|(q, b, _)| wgpu::RenderPassTimestampWrites {
                    query_set: q,
                    beginning_of_pass_write_index: Some(b),
                    end_of_pass_write_index: None,
                }),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.shadow);
            pass.set_bind_group(0, frame_group, &[]);
            pass.set_bind_group(1, &n.group, &[]);
            pass.draw(0..3, 0..1);
            crate::count_draws(1);
        }
        // The sun as the eye sees it through the clouds: the rays' parts, then their mean.
        for (label, target, pipe, group) in [
            (
                "hd cloud sun rays",
                &self.sun_rays_map,
                &self.sun_rays,
                &n.group,
            ),
            (
                "hd cloud sun",
                &self.sun_map,
                &self.sun_mean,
                &self.sun_group,
            ),
        ] {
            crate::count_pass();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(crate::colour(target))],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, frame_group, &[]);
            pass.set_bind_group(1, group, &[]);
            pass.draw(0..3, 0..1);
            crate::count_draws(1);
        }
        {
            crate::count_pass();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hd clouds march"),
                color_attachments: &[
                    Some(crate::colour(&t.current_view)),
                    Some(crate::colour(&t.dist_view)),
                ],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(march);
            pass.set_bind_group(0, frame_group, &[]);
            pass.set_bind_group(1, &n.group, &[]);
            pass.set_bind_group(2, sky_group, &[]);
            pass.draw(0..3, 0..1);
            crate::count_draws(1);
        }
        {
            crate::count_pass();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hd clouds resolve"),
                color_attachments: &[Some(crate::colour(&t.views[self.parity]))],
                depth_stencil_attachment: None,
                timestamp_writes: stamps.map(|(q, _, e)| wgpu::RenderPassTimestampWrites {
                    query_set: q,
                    beginning_of_pass_write_index: None,
                    end_of_pass_write_index: Some(e),
                }),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.resolve);
            pass.set_bind_group(0, frame_group, &[]);
            pass.set_bind_group(1, &t.groups[self.parity], &[]);
            pass.draw(0..3, 0..1);
            crate::count_draws(1);
        }
        encoder.copy_texture_to_texture(
            t.history[self.parity].as_image_copy(),
            t.out.texture.as_image_copy(),
            wgpu::Extent3d {
                width: t.key.0,
                height: t.key.1,
                depth_or_array_layers: 1,
            },
        );
        t.out.encode(encoder, &self.down);
    }

    /// After a frame: the next history, and whether this one was written.
    pub fn advance(&mut self) {
        if self.drawn {
            self.parity ^= 1;
            self.written = true;
            self.frame += 1;
        } else {
            self.written = false;
            self.prev_drift = None;
        }
    }

    /// The sun's transmittance through the clouds as the eye saw it last frame (one texel, r), if they were drawn.
    pub fn sun_image(&self) -> Option<&wgpu::Texture> {
        self.written.then_some(&self.sun_texture)
    }

    /// The current image (the last frame's clouds: rgb their light, a their transmittance), if any.
    pub fn image(&self) -> Option<&wgpu::Texture> {
        self.targets
            .as_ref()
            .filter(|_| self.written)
            .map(|t| &t.out.texture)
    }
}
