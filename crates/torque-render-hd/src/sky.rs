//! The enhanced renderer's sky on the GPU: the sky uniform (the settings' sky, fog, emitters
//! and volumetric medium: `skycommon.wgsl`'s `Sky`), the physical sky's three tables (`atmosphere.wgsl`: the transmittance and the
//! multiple scattering once, the sky view each frame that draws the physical sky) and the sky passes drawn among the sky's draws
//! (`sky.wgsl`: the physical sky's shading of the server's sky box after the box; the sun disk before the clouds, always; the
//! volumetric clouds). The group of bind group 2 ([`SkyGpu::group`]: the uniform, the sky view table,
//! the transmittance table, a linear sampler, the sky copy; bindings 1 to 5: group 2's binding 0 is the prepass's motion matrix in the
//! forward module) is shared by the forward pass (the fog and the lit cloud layers, the emitters), the sky passes
//! and the volumetrics. Bindings 6, 7 and 8 hold the volumetric clouds' image (the sky pass composites it in front of the flat
//! cloud layers), their shadow map (the forward pass and the volumetric light read it) and the sun's transmittance through them as
//! the eye sees it (one texel: a low sun's light, its glint and its glow); the uniform's last three vec4 say where the map lies,
//! what uses it and how the eye's view of the sun is taken.

use torque_render_core::wgpu;
use torque_render_scene::hd::atmosphere::{
    self, EARTH, MULTISCATTER_SIZE, SKY_VIEW_SIZE, TRANSMITTANCE_SIZE,
};
use torque_render_scene::hd::sky::{
    CLOUD_HORIZON_FADE, CLOUD_LOBE_CAP, CLOUD_LOBE_SHARE, DEFAULT_HEIGHT_FOG, DEFAULT_SKY_SHADING,
    NIGHT_SKY, STAR_BRIGHTNESS, STAR_DENSITY, STAR_SKY_LIMIT, default_sky_scale,
};
use torque_render_scene::hd::{HdScene, ImageSettings, Quality, srgb_to_linear};

use crate::clouds::SkyClouds;
use crate::shaders::Permutation;
use crate::{DEPTH_FORMAT, HDR_FORMAT, push_f32};

/// The tables' format.
pub const LUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The headings about the eye the extension's fog ends cover (the uniform's `ext_fog_ends`).
pub const EXT_FOG_SECTORS: usize = 16;
/// The sky uniform's bytes (26 vec4).
const SKY_BYTES: u64 = 416;
pub use crate::FOG_LOW_LEVEL;
/// Where a surface the fog wholly hides takes the sky's detail behind it again (from, to: shares of the visible distance; the
/// surface ends there, so it meets the sky beside it without a seam; skyfog.wgsl's FOG_DETAIL_KEEP: only where the fog leaves less
/// than that share of its own colour).
pub const FOG_DETAIL_EDGE: (f32, f32) = (0.9, 0.98);

/// A full-frame pass among the sky's draws: the physical sky's shading of the server's box, the sun disk, the volumetric clouds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkyPass {
    Shade,
    Disk,
    Clouds,
}

fn lut(device: &wgpu::Device, label: &str, (w, h): (u32, u32)) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: LUT_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn view(t: &wgpu::Texture) -> wgpu::TextureView {
    t.create_view(&wgpu::TextureViewDescriptor::default())
}

fn sampled(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
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

fn sampler_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

/// A full-frame pipeline drawn inside the opaque pass (its depth attachment: no test, no write) into the HDR target.
fn sky_pass(
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    label: &str,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    entry: &str,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_full"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache,
    })
}

/// The sky's GPU state.
pub struct SkyGpu {
    pub uniform: wgpu::Buffer,
    /// Bind group 2's layout and group.
    pub layout: wgpu::BindGroupLayout,
    pub group: wgpu::BindGroup,
    pub transmittance: wgpu::Texture,
    pub multiscatter: wgpu::Texture,
    pub sky_view: wgpu::Texture,
    /// The tables' compute pipelines (transmittance, multiple scattering, sky view) and groups.
    lut_pipelines: [wgpu::ComputePipeline; 3],
    lut_groups: [wgpu::BindGroup; 3],
    luts_built: bool,
    /// The physical sky's pass, the sun disk's, the volumetric clouds'.
    pub shade_pipeline: wgpu::RenderPipeline,
    pub disk_pipeline: wgpu::RenderPipeline,
    pub cloud_pipeline: wgpu::RenderPipeline,
    /// Whether the uniform holds the physical sky this frame.
    pub physical: bool,
    sv_view: wgpu::TextureView,
    t_view: wgpu::TextureView,
    linear: wgpu::Sampler,
    /// The group's frame-sized views: the sky copy, the clouds' image and shadow map.
    sky_copy: wgpu::TextureView,
    cloud_image: wgpu::TextureView,
    cloud_shadow: wgpu::TextureView,
    cloud_sun: wgpu::TextureView,
    /// The sky copy's mip the fog reads, and whether it reads it (`HdFeatures::fog_sky_mean`).
    sky_low: f32,
    pub fog_mean: bool,
}

/// The sky group's bindings (the tables, the fog's frame and the clouds' images).
pub(crate) fn group_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    let all = wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE;
    vec![
        uniform_entry(1, all),
        sampled(2, all),
        sampled(3, all),
        sampler_entry(4, all),
        // The frame after the sky's draws (the fog's colour).
        sampled(5, all),
        // The volumetric clouds' image, their shadow map and the sun through them.
        sampled(6, all),
        sampled(7, all),
        sampled(8, all),
    ]
}

impl SkyGpu {
    #[allow(clippy::too_many_arguments)]
    fn make_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniform: &wgpu::Buffer,
        sv: &wgpu::TextureView,
        t: &wgpu::TextureView,
        linear: &wgpu::Sampler,
        sky_copy: &wgpu::TextureView,
        clouds: (&wgpu::TextureView, &wgpu::TextureView, &wgpu::TextureView),
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd sky group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
                crate::tv(2, sv),
                crate::tv(3, t),
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(linear),
                },
                crate::tv(5, sky_copy),
                crate::tv(6, clouds.0),
                crate::tv(7, clouds.1),
                crate::tv(8, clouds.2),
            ],
        })
    }

    fn rebind(&mut self, device: &wgpu::Device) {
        self.group = Self::make_group(
            device,
            &self.layout,
            &self.uniform,
            &self.sv_view,
            &self.t_view,
            &self.linear,
            &self.sky_copy,
            (&self.cloud_image, &self.cloud_shadow, &self.cloud_sun),
        );
    }

    /// Binds the frame targets' sky copy (made with the targets; `low` the mip the fog reads without the sky's detail).
    pub fn bind_sky_copy(&mut self, device: &wgpu::Device, sky_copy: &wgpu::TextureView, low: u32) {
        self.sky_copy = sky_copy.clone();
        self.sky_low = low as f32;
        self.rebind(device);
    }

    /// Binds the volumetric clouds' image, shadow map and the sun's transmittance through them.
    pub fn bind_clouds(
        &mut self,
        device: &wgpu::Device,
        image: &wgpu::TextureView,
        shadow: &wgpu::TextureView,
        sun: &wgpu::TextureView,
    ) {
        self.cloud_image = image.clone();
        self.cloud_shadow = shadow.clone();
        self.cloud_sun = sun.clone();
        self.rebind(device);
    }

    /// `texture_layout`: the forward pass's group 1 (the sky passes do not read it; with it their layout's groups are the forward
    /// pass's, so the frame's and the sky's groups stay valid as the sky's draws and these passes alternate in one pass: the
    /// shading pass follows the box, and with group 1 left out its reads of the sky group came back zero on D3D12).
    pub fn new(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        frame_layout: &wgpu::BindGroupLayout,
        texture_layout: &wgpu::BindGroupLayout,
        clouds: (&wgpu::TextureView, &wgpu::TextureView, &wgpu::TextureView),
    ) -> SkyGpu {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd sky"),
            size: SKY_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd sky group"),
            entries: &group_entries(),
        });
        let transmittance = lut(device, "hd sky transmittance", TRANSMITTANCE_SIZE);
        let multiscatter = lut(
            device,
            "hd sky multiscatter",
            (MULTISCATTER_SIZE, MULTISCATTER_SIZE),
        );
        let sky_view = lut(device, "hd sky view", SKY_VIEW_SIZE);
        let dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hd sky dummy"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: LUT_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let dummy_view = view(&dummy);
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hd sky linear"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..wgpu::SamplerDescriptor::default()
        });
        let (t_view, ms_view, sv_view) =
            (view(&transmittance), view(&multiscatter), view(&sky_view));
        let group = Self::make_group(
            device,
            &layout,
            &uniform,
            &sv_view,
            &t_view,
            &linear,
            &dummy_view,
            clouds,
        );
        // The tables' compute group: the uniform, the transmittance and multiple scattering tables read, the sampler, the table
        // written, the sky view table (declared, unread).
        let lut_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd sky tables"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::COMPUTE),
                sampled(1, wgpu::ShaderStages::COMPUTE),
                sampled(2, wgpu::ShaderStages::COMPUTE),
                sampler_entry(3, wgpu::ShaderStages::COMPUTE),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: LUT_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                sampled(5, wgpu::ShaderStages::COMPUTE),
            ],
        });
        let lut_group = |reads: [&wgpu::TextureView; 2], out: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hd sky table"),
                layout: &lut_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    crate::tv(1, reads[0]),
                    crate::tv(2, reads[1]),
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&linear),
                    },
                    crate::tv(4, out),
                    crate::tv(5, &dummy_view),
                ],
            })
        };
        let lut_groups = [
            lut_group([&dummy_view, &dummy_view], &t_view),
            lut_group([&t_view, &dummy_view], &ms_view),
            lut_group([&t_view, &ms_view], &sv_view),
        ];
        let lut_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd sky tables"),
            bind_group_layouts: &[Some(&lut_layout)],
            immediate_size: 0,
        });
        let atm = crate::module(device, Permutation::Atmosphere);
        let compute = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&lut_pipeline_layout),
                module: &atm,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache,
            })
        };
        let lut_pipelines = [
            compute("cs_transmittance"),
            compute("cs_multiscatter"),
            compute("cs_sky_view"),
        ];
        let pass_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd sky pass"),
            bind_group_layouts: &[Some(frame_layout), Some(texture_layout), Some(&layout)],
            immediate_size: 0,
        });
        let sky_module = crate::module(device, Permutation::Sky);
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let keep = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        // The shading over the server's box: the stars plus the box times the shading (the alpha; a float target, so it is
        // not clamped to 1).
        let shade = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::SrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let shade_pipeline = sky_pass(
            device,
            cache,
            "hd physical sky",
            &pass_layout,
            &sky_module,
            "fs_atmosphere_sky",
            Some(wgpu::BlendState {
                color: shade,
                alpha: keep,
            }),
        );
        let disk_pipeline = sky_pass(
            device,
            cache,
            "hd sun disk",
            &pass_layout,
            &sky_module,
            "fs_sun_disk",
            Some(wgpu::BlendState {
                color: add,
                alpha: keep,
            }),
        );
        let cloud_pipeline = sky_pass(
            device,
            cache,
            "hd volumetric clouds",
            &pass_layout,
            &sky_module,
            "fs_clouds",
            Some(wgpu::BlendState {
                color: shade,
                alpha: keep,
            }),
        );
        SkyGpu {
            sky_copy: dummy_view.clone(),
            sky_low: 0.0,
            fog_mean: true,
            cloud_image: clouds.0.clone(),
            cloud_shadow: clouds.1.clone(),
            cloud_sun: clouds.2.clone(),
            cloud_pipeline,
            sv_view,
            t_view,
            linear,
            uniform,
            layout,
            group,
            transmittance,
            multiscatter,
            sky_view,
            lut_pipelines,
            lut_groups,
            luts_built: false,
            shade_pipeline,
            disk_pipeline,
            physical: false,
        }
    }

    /// The sky uniform for `scene` drawn with `image` at `quality` at the effect time `time`; `volumetrics`: whether they run this
    /// frame; `clouds`: the shadow map and its uses (all 0 without clouds); `ext_fog_ends`: where the extension's geometry is wholly
    /// fogged in each heading about the eye (0: with the rest of the fog; its fog hook reads them).
    #[allow(clippy::too_many_arguments)]
    pub fn write<G>(
        &mut self,
        queue: &wgpu::Queue,
        scene: &HdScene<G>,
        image: &ImageSettings,
        quality: Quality,
        time: f32,
        volumetrics: bool,
        clouds: SkyClouds,
        ext_fog_ends: &[f32; EXT_FOG_SECTORS],
    ) {
        self.physical = image.physical_sky;
        let s = &scene.sun;
        let gain = default_sky_scale();
        let disk = image.sun_disk;
        let sun_light = [0, 1, 2].map(|k| {
            (srgb_to_linear((s.ambient[k] + s.diffuse[k]).max(0.0))
                - srgb_to_linear(s.ambient[k].max(0.0)))
            .max(0.0)
        });
        // The disk in the sun's colour (the server's sun).
        let disk_colour = {
            let c = s.diffuse.map(|v| srgb_to_linear(v.clamp(0.0, 1.0)));
            let m = c[0].max(c[1]).max(c[2]);
            if m > 1e-4 {
                c.map(|v| v / m * disk.radiance)
            } else {
                [0.0; 3]
            }
        };
        // The server's sky's brightness (its horizon colour's luminance, linear): the stars show where it is dark.
        let f = scene.fog.color.map(|v| srgb_to_linear(v.clamp(0.0, 1.0)));
        let sky_luminance = 0.2126 * f[0] + 0.7152 * f[1] + 0.0722 * f[2];
        let shading = DEFAULT_SKY_SHADING;
        let e = image.emissive;
        let vol = image.volumetric;
        let vq = quality.volumetrics();
        let t = scene.atmosphere_towards();
        let mut b = Vec::with_capacity(SKY_BYTES as usize);
        let flag = |on: bool| if on { 1.0 } else { 0.0 };
        // w: 1 with the physical sky, 2 when it is drawn in place of the server's box (`HdScene::atmosphere`).
        let physical = if image.physical_sky && scene.atmosphere {
            2.0
        } else {
            flag(image.physical_sky)
        };
        push_f32(&mut b, &[flag(image.linear_light), gain, time, physical]);
        push_f32(&mut b, &[t[0], t[1], t[2], disk.radius]);
        push_f32(
            &mut b,
            &[
                disk_colour[0],
                disk_colour[1],
                disk_colour[2],
                sky_luminance,
            ],
        );
        push_f32(
            &mut b,
            &[disk.limb[0], disk.limb[1], disk.limb[2], shading.strength],
        );
        push_f32(&mut b, &atmosphere::uniform_floats(&EARTH));
        let night = NIGHT_SKY.map(|v| v * gain);
        push_f32(&mut b, &[night[0], night[1], night[2], STAR_DENSITY]);
        push_f32(
            &mut b,
            &[
                STAR_BRIGHTNESS,
                STAR_SKY_LIMIT,
                CLOUD_HORIZON_FADE,
                shading.floor,
            ],
        );
        let hf = DEFAULT_HEIGHT_FOG;
        push_f32(
            &mut b,
            &[hf.density, hf.falloff, hf.base, flag(image.height_fog)],
        );
        push_f32(&mut b, &[e.glow, e.shape, e.emitter, shading.cap]);
        push_f32(
            &mut b,
            &[vol.scattering, vol.anisotropy, vol.strength, vol.reach],
        );
        push_f32(
            &mut b,
            &[
                if volumetrics { 1.0 } else { 0.0 },
                vq.map_or(0.0, |v| v.steps as f32),
                vq.map_or(1.0, |v| v.scale as f32),
                CLOUD_LOBE_CAP,
            ],
        );
        push_f32(
            &mut b,
            &[sun_light[0], sun_light[1], sun_light[2], CLOUD_LOBE_SHARE],
        );
        push_f32(&mut b, &clouds.map);
        push_f32(&mut b, &clouds.uses);
        push_f32(&mut b, &clouds.sun);
        // The sun as the sky shows it (the box's painted sun, else the Sun's direction): the disk, the glow about it.
        let d = scene.sky_sun();
        push_f32(&mut b, &[d[0], d[1], d[2], 0.0]);
        // The fog's sky without its detail: the sky copy's mip, the clouds' image's, and where a wholly fogged surface takes the
        // detail behind it again (as distance shares; the keep under which it does is the shader's FOG_DETAIL_KEEP).
        let fog_low = if self.fog_mean {
            [
                self.sky_low,
                clouds.low,
                FOG_DETAIL_EDGE.0,
                FOG_DETAIL_EDGE.1,
            ]
        } else {
            // (The sky behind with its detail everywhere.)
            [0.0, 0.0, -1.0, -1.0]
        };
        push_f32(&mut b, &fog_low);
        push_f32(&mut b, &clouds.slabs);
        // Where the extension's geometry is wholly fogged in each heading (the sky's `ext_fog_ends`).
        for e in ext_fog_ends {
            push_f32(&mut b, &[e.max(0.0)]);
        }
        debug_assert_eq!(b.len() as u64, SKY_BYTES);
        queue.write_buffer(&self.uniform, 0, &b);
    }

    /// The tables into `pass` (the frame's pass with the clusters): the transmittance and the multiple scattering the first time,
    /// the sky view with the physical sky.
    pub fn encode_tables(&mut self, pass: &mut wgpu::ComputePass<'_>) {
        let mut jobs: Vec<(usize, (u32, u32))> = Vec::new();
        if !self.luts_built {
            jobs.push((0, TRANSMITTANCE_SIZE));
            jobs.push((1, (MULTISCATTER_SIZE, MULTISCATTER_SIZE)));
            self.luts_built = true;
        }
        if self.physical {
            jobs.push((2, SKY_VIEW_SIZE));
        }
        for (k, (w, h)) in jobs {
            pass.set_pipeline(&self.lut_pipelines[k]);
            pass.set_bind_group(0, &self.lut_groups[k], &[]);
            pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
        }
    }
}
