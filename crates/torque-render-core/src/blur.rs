//! The GUI pass's backdrop blur: a [`BackdropDraw`] (the backdrop a GUI control asks for) blurs what the frame holds under the
//! control before the control draws.
//!
//! **Technique** (dual-filter Kawase, Bjorge 2015; `gui_blur.wgsl`): the GUI pass ends; the control's rectangle (cut by its
//! clip and the target) plus a margin of `2^levels` pixels is copied out of the target (the grab); `levels` 5-tap downsamples
//! (`levels = clamp(floor(log2(radius / 2)), 1, 5)`, into `Rgba16Float` textures, the first read through an sRGB view so the blur
//! is in linear light), `levels - 1` 8-tap upsamples back to half size, and the composite: the last upsample at each target pixel,
//! encoded to sRGB, the tint mixed over it by its alpha, drawn inside the rounded rectangle (corner radius, 1-pixel anti-aliased
//! edge) with alpha blending at the control's opacity; then the GUI pass resumes. Each backdrop blurs what is drawn before it, so
//! nested dialogs stack. Every tap lands on a quarter texel, so the bilinear weights are exact and [`reference()`] (the CPU's) agrees.
//! Without a blur (`radius` 0, [`Blur::enabled`] off, a target that cannot be copied from) the tint alone is drawn in the rounded
//! rectangle.
//!
//! **Hostile input.** Every rectangle is cut to the target before anything is sized from it; the radius and corner are capped here
//! ([`MAX_BLUR_RADIUS`], [`MAX_CORNER_RADIUS`]) whatever the draw list holds; a frame's backdrops are at most
//! [`MAX_FRAME_BACKDROPS`] (more are left out).

use std::collections::HashMap;

use torque_gui::backdrop::{BackdropDraw, MAX_BLUR_RADIUS, MAX_CORNER_RADIUS};
use torque_gui::draw::ExtraDraw;

use crate::gui_pass::{Backdrops, KEEP_FRAMES};
use crate::{COLOR_FORMAT, Gpu};

static BLUR_DEFAULT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Whether blurs made from now on blur their backdrops ([`Blur::enabled`]; a host turns it off to draw the tints only).
pub fn set_blur_default(on: bool) {
    BLUR_DEFAULT.store(on, std::sync::atomic::Ordering::SeqCst);
}

const SHADER: &str = include_str!("gui_blur.wgsl");
/// The most downsample levels.
pub const MAX_LEVELS: u32 = 5;
/// The most backdrops one frame draws.
pub const MAX_FRAME_BACKDROPS: usize = 16;
/// The uniform block's size and the stride between passes (the device's offset alignment, at least 256).
const PARAMS_BYTES: u64 = 80;
/// The most passes one frame's backdrops take: down, up and composite for each.
const MAX_PASSES: usize = MAX_FRAME_BACKDROPS * (2 * MAX_LEVELS as usize + 1);
/// The level textures' format.
const LEVEL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Chains (one per grabbed size) kept between frames.
const MAX_CHAINS: usize = 8;

/// The downsample levels of a blur radius (0: none).
pub fn levels(radius: u32) -> u32 {
    if radius == 0 {
        return 0;
    }
    let half = (radius / 2).max(1);
    (31 - half.leading_zeros()).clamp(1, MAX_LEVELS)
}

/// Where a backdrop draws and what it grabs, in target pixels: the composite area (left, top, right, bottom: the control's
/// rectangle cut by its clip and the target) and the grabbed region (the area plus `2^levels` on each side, cut by the target).
/// None when the area is empty.
pub fn placement(b: &BackdropDraw, size: (u32, u32)) -> Option<([i32; 4], [i32; 4])> {
    let (tw, th) = (
        size.0.min(i32::MAX as u32) as i32,
        size.1.min(i32::MAX as u32) as i32,
    );
    let r = |x: [i32; 4]| {
        [
            x[0],
            x[1],
            x[0].saturating_add(x[2].max(0)),
            x[1].saturating_add(x[3].max(0)),
        ]
    };
    let (a, c) = (r(b.rect), r(b.clip));
    let area = [
        a[0].max(c[0]).max(0),
        a[1].max(c[1]).max(0),
        a[2].min(c[2]).min(tw),
        a[3].min(c[3]).min(th),
    ];
    if area[2] <= area[0] || area[3] <= area[1] {
        return None;
    }
    let m = 1i32 << levels(b.radius.min(MAX_BLUR_RADIUS));
    let grab = [
        (area[0] - m).max(0),
        (area[1] - m).max(0),
        (area[2] + m).min(tw),
        (area[3] + m).min(th),
    ];
    Some((area, grab))
}

/// The size of level `k` of a grabbed region of `size` (level 0 is the region).
pub fn level_size(size: (u32, u32), k: u32) -> (u32, u32) {
    let mut s = size;
    for _ in 0..k {
        s = (s.0.div_ceil(2).max(1), s.1.div_ceil(2).max(1));
    }
    s
}

// ------------------------------------------------------------------------------------------------------------ the GPU side

struct Chain {
    grab: wgpu::Texture,
    /// The bind groups reading the grab (sRGB view) and each level 1..=MAX_LEVELS; the levels' views to draw into.
    read_grab: wgpu::BindGroup,
    read_level: Vec<wgpu::BindGroup>,
    levels: Vec<wgpu::TextureView>,
    last_used: u64,
}

/// The backdrop pipelines, the level textures by grabbed size, the uniform block.
pub struct Blur {
    layout: wgpu::BindGroupLayout,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    stride: u64,
    chains: HashMap<(u32, u32), Chain>,
    /// A bind group over a 1x1 texture (the tint-only composite reads nothing).
    blank: wgpu::BindGroup,
    _blank_texture: wgpu::Texture,
    frame: u64,
    /// Whether backdrops blur (off: their tints only).
    pub enabled: bool,
    /// The uniform blocks of this frame's passes, written when the frame's segments are all recorded.
    params_bytes: Vec<u8>,
    /// The offscreen GUI target a window draws into when a frame has backdrops ([`Blur::target`]).
    target: Option<(
        (u32, u32),
        wgpu::Texture,
        wgpu::TextureView,
        wgpu::BindGroup,
    )>,
}

fn pipeline(
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    entry: &str,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("gui blur"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_full"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache,
    })
}

/// One pass's uniform block.
#[derive(Debug, Clone, Copy, Default)]
struct Params {
    src_size: [f32; 2],
    dst_size: [f32; 2],
    origin: [f32; 2],
    blur: f32,
    corner: f32,
    rect: [f32; 4],
    tint: [f32; 4],
    opacity: f32,
}

impl Params {
    fn bytes(&self) -> Vec<u8> {
        let mut f = Vec::with_capacity(20);
        f.extend_from_slice(&self.src_size);
        f.extend_from_slice(&self.dst_size);
        f.extend_from_slice(&self.origin);
        f.push(self.blur);
        f.push(self.corner);
        f.extend_from_slice(&self.rect);
        f.extend_from_slice(&self.tint);
        f.extend_from_slice(&[self.opacity, 0.0, 0.0, 0.0]);
        f.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
}

/// What one backdrop's passes need (made before the encoder runs).
struct Planned {
    area: [i32; 4],
    grab: [i32; 4],
    levels: u32,
    /// The first uniform slot of its passes.
    slot: usize,
}

impl Blur {
    pub fn new(gpu: &Gpu) -> Blur {
        let device = gpu.device();
        let cache = gpu.pipeline_cache();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gui blur"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gui blur"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(PARAMS_BYTES),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gui blur"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let alpha = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let blend = wgpu::BlendState {
            color: alpha,
            alpha,
        };
        let stride = u64::from(device.limits().min_uniform_buffer_offset_alignment)
            .max(256)
            .max(PARAMS_BYTES);
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gui blur params"),
            size: stride * MAX_PASSES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gui blur"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            lod_max_clamp: 0.0,
            ..wgpu::SamplerDescriptor::default()
        });
        let blank_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gui blur blank"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: LEVEL_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let blank_view = blank_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let blank = bind(device, &layout, &params, &blank_view, &sampler);
        Blur {
            down: pipeline(device, cache, &pl, &shader, "fs_down", LEVEL_FORMAT, None),
            up: pipeline(device, cache, &pl, &shader, "fs_up", LEVEL_FORMAT, None),
            composite: pipeline(
                device,
                cache,
                &pl,
                &shader,
                "fs_composite",
                COLOR_FORMAT,
                Some(blend),
            ),
            blit: pipeline(device, cache, &pl, &shader, "fs_blit", COLOR_FORMAT, None),
            layout,
            sampler,
            params,
            stride,
            chains: HashMap::new(),
            blank,
            _blank_texture: blank_texture,
            frame: 0,
            enabled: BLUR_DEFAULT.load(std::sync::atomic::Ordering::SeqCst),
            params_bytes: Vec::new(),
            target: None,
        }
    }

    fn chain(&mut self, device: &wgpu::Device, size: (u32, u32)) -> &mut Chain {
        if !self.chains.contains_key(&size) && self.chains.len() >= MAX_CHAINS {
            // The chain used longest ago goes.
            if let Some(old) = self
                .chains
                .iter()
                .min_by_key(|(_, c)| c.last_used)
                .map(|(k, _)| *k)
            {
                self.chains.remove(&old);
            }
        }
        let (layout, params, sampler) = (&self.layout, &self.params, &self.sampler);
        let frame = self.frame;
        let c = self.chains.entry(size).or_insert_with(|| {
            let grab = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("gui blur grab"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: COLOR_FORMAT,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
            });
            let srgb = grab.create_view(&wgpu::TextureViewDescriptor {
                format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
                ..wgpu::TextureViewDescriptor::default()
            });
            let read_grab = bind(device, layout, params, &srgb, sampler);
            let mut levels = Vec::new();
            let mut read_level = Vec::new();
            for k in 1..=MAX_LEVELS {
                let (w, h) = level_size(size, k);
                let t = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("gui blur level"),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: LEVEL_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let v = t.create_view(&wgpu::TextureViewDescriptor::default());
                read_level.push(bind(device, layout, params, &v, sampler));
                levels.push(v);
            }
            Chain {
                grab,
                read_grab,
                read_level,
                levels,
                last_used: frame,
            }
        });
        c.last_used = frame;
        c
    }

    /// The backdrops of one GUI pass segment, encoded after the batches before them: grabs, the chain, the composite. `blur`
    /// false draws the tints only. Returns how many were drawn.
    #[allow(clippy::too_many_arguments)]
    fn encode_segment(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        backdrops: &[&BackdropDraw],
        blur: bool,
        params: &mut Vec<u8>,
        timer: Option<(&wgpu::QuerySet, u32)>,
    ) -> usize {
        let device = gpu.device();
        let texture = target.texture();
        let tsize = (texture.width(), texture.height());
        let can_copy = texture.usage().contains(wgpu::TextureUsages::COPY_SRC)
            && texture.format() == COLOR_FORMAT;
        let mut planned = Vec::new();
        for b in backdrops {
            if planned.len() >= MAX_FRAME_BACKDROPS {
                break;
            }
            let Some((area, grab)) = placement(b, tsize) else {
                continue;
            };
            let levels = if blur && can_copy {
                levels(b.radius.min(MAX_BLUR_RADIUS))
            } else {
                0
            };
            let slot = params.len() / self.stride as usize;
            if slot + 2 * levels as usize + 1 > MAX_PASSES {
                break;
            }
            let g = ((grab[2] - grab[0]) as u32, (grab[3] - grab[1]) as u32);
            let rect = [
                b.rect[0] as f32,
                b.rect[1] as f32,
                b.rect[0].saturating_add(b.rect[2]) as f32,
                b.rect[1].saturating_add(b.rect[3]) as f32,
            ];
            let mut push = |p: Params| {
                let mut bytes = p.bytes();
                bytes.resize(self.stride as usize, 0);
                params.extend_from_slice(&bytes);
            };
            for k in 1..=levels {
                push(Params {
                    src_size: size_f(level_size(g, k - 1)),
                    dst_size: size_f(level_size(g, k)),
                    ..Params::default()
                });
            }
            for k in (2..=levels).rev() {
                push(Params {
                    src_size: size_f(level_size(g, k)),
                    dst_size: size_f(level_size(g, k - 1)),
                    ..Params::default()
                });
            }
            let tint = b.tint.map(|c| f32::from(c) / 255.0);
            push(Params {
                src_size: size_f(level_size(g, 1)),
                dst_size: size_f(g),
                origin: [grab[0] as f32, grab[1] as f32],
                blur: if levels > 0 { 1.0 } else { 0.0 },
                corner: b.corner.min(MAX_CORNER_RADIUS) as f32,
                rect,
                tint,
                opacity: if b.opacity.is_finite() {
                    b.opacity.clamp(0.0, 1.0)
                } else {
                    1.0
                },
            });
            planned.push(Planned {
                area,
                grab,
                levels,
                slot,
            });
        }
        let n = planned.len();
        for (i, p) in planned.into_iter().enumerate() {
            let g = (
                (p.grab[2] - p.grab[0]) as u32,
                (p.grab[3] - p.grab[1]) as u32,
            );
            let stride = self.stride;
            let offset = |s: usize| (s as u64 * stride) as u32;
            let mut slot = p.slot;
            if p.levels > 0 {
                let chain = self.chain(device, g);
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: p.grab[0] as u32,
                            y: p.grab[1] as u32,
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &chain.grab,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: g.0,
                        height: g.1,
                        depth_or_array_layers: 1,
                    },
                );
            }
            let chain = if p.levels > 0 {
                self.chains.get(&g)
            } else {
                None
            };
            if let Some(chain) = chain {
                for k in 1..=p.levels {
                    let src = if k == 1 {
                        &chain.read_grab
                    } else {
                        &chain.read_level[(k - 2) as usize]
                    };
                    level_pass(
                        encoder,
                        &chain.levels[(k - 1) as usize],
                        &self.down,
                        src,
                        offset(slot),
                    );
                    slot += 1;
                }
                for k in (2..=p.levels).rev() {
                    level_pass(
                        encoder,
                        &chain.levels[(k - 2) as usize],
                        &self.up,
                        &chain.read_level[(k - 1) as usize],
                        offset(slot),
                    );
                    slot += 1;
                }
            }
            let last = i + 1 == n;
            let timestamps =
                timer
                    .filter(|_| last)
                    .map(|(q, at)| wgpu::RenderPassTimestampWrites {
                        query_set: q,
                        beginning_of_pass_write_index: None,
                        end_of_pass_write_index: Some(at),
                    });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gui blur composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: timestamps,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.composite);
            let group = chain.map_or(&self.blank, |c| &c.read_level[0]);
            pass.set_bind_group(0, group, &[offset(slot)]);
            pass.set_scissor_rect(
                p.area[0] as u32,
                p.area[1] as u32,
                (p.area[2] - p.area[0]) as u32,
                (p.area[3] - p.area[1]) as u32,
            );
            pass.draw(0..3, 0..1);
        }
        n
    }

    /// Starts a frame (the chains' clock; ones unused for [`KEEP_FRAMES`] frames go).
    fn start_frame(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        self.chains
            .retain(|_, c| frame.saturating_sub(c.last_used) < KEEP_FRAMES);
    }

    /// The offscreen GUI target of `size` (made again when the size changes): a window draws a frame with backdrops into it (its
    /// surface cannot be copied from), then [`present`](Self::present)s it.
    pub fn target(&mut self, gpu: &Gpu, size: (u32, u32)) -> wgpu::TextureView {
        let size = (size.0.max(1), size.1.max(1));
        if self.target.as_ref().is_none_or(|(s, ..)| *s != size) {
            let device = gpu.device();
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("gui target"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: COLOR_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let v = t.create_view(&wgpu::TextureViewDescriptor::default());
            let g = bind(device, &self.layout, &self.params, &v, &self.sampler);
            self.target = Some((size, t, v, g));
        }
        self.target
            .as_ref()
            .map(|(_, _, v, _)| v.clone())
            .expect("made above")
    }

    /// Copies the offscreen GUI target onto `surface` (a view of the window's surface texture), texel for texel.
    pub fn present(&self, gpu: &Gpu, surface: &wgpu::TextureView) {
        let Some((_, _, _, group)) = self.target.as_ref() else {
            return;
        };
        let mut encoder = gpu
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gui present"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gui present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: surface,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.blit);
            pass.set_bind_group(0, group, &[0]);
            pass.draw(0..3, 0..1);
        }
        gpu.queue().submit([encoder.finish()]);
    }
}

impl Backdrops for Blur {
    fn begin_frame(&mut self) {
        self.start_frame();
        self.params_bytes.clear();
    }

    fn claims(&self, extra: &ExtraDraw) -> bool {
        extra.get::<BackdropDraw>().is_some()
    }

    fn encode(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        backdrops: &[&ExtraDraw],
        timer: Option<(&wgpu::QuerySet, u32)>,
    ) -> usize {
        let backdrops: Vec<&BackdropDraw> = backdrops
            .iter()
            .filter_map(|x| x.get::<BackdropDraw>())
            .collect();
        let mut params = std::mem::take(&mut self.params_bytes);
        let blur = self.enabled;
        let n = self.encode_segment(gpu, encoder, target, &backdrops, blur, &mut params, timer);
        self.params_bytes = params;
        n
    }

    fn end_frame(&mut self, queue: &wgpu::Queue) {
        if !self.params_bytes.is_empty() {
            queue.write_buffer(&self.params, 0, &self.params_bytes);
        }
    }

    fn target(&mut self, gpu: &Gpu, size: (u32, u32)) -> wgpu::TextureView {
        Blur::target(self, gpu, size)
    }

    fn present(&self, gpu: &Gpu, surface: &wgpu::TextureView) {
        Blur::present(self, gpu, surface);
    }
}

fn size_f(s: (u32, u32)) -> [f32; 2] {
    [s.0 as f32, s.1 as f32]
}

fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    params: &wgpu::Buffer,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("gui blur"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: params,
                    offset: 0,
                    size: wgpu::BufferSize::new(PARAMS_BYTES),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn level_pass(
    encoder: &mut wgpu::CommandEncoder,
    dst: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    src: &wgpu::BindGroup,
    offset: u32,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("gui blur level"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: dst,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, src, &[offset]);
    pass.draw(0..3, 0..1);
}

// ------------------------------------------------------------------------------------------------------------ the CPU reference

/// An RGBA image of `f32` channels (a level), rows top to bottom.
#[derive(Debug, Clone)]
struct Plane {
    w: u32,
    h: u32,
    px: Vec<[f32; 4]>,
}

impl Plane {
    fn at(&self, x: i64, y: i64) -> [f32; 4] {
        let x = x.clamp(0, i64::from(self.w) - 1) as usize;
        let y = y.clamp(0, i64::from(self.h) - 1) as usize;
        self.px[y * self.w as usize + x]
    }

    /// Bilinear at texel coordinates `(x, y)` (texel centres at `i + 0.5`), clamped to the edge.
    fn sample(&self, x: f32, y: f32) -> [f32; 4] {
        let (tx, ty) = (x - 0.5, y - 0.5);
        let (x0, y0) = (tx.floor(), ty.floor());
        let (fx, fy) = (tx - x0, ty - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let a = self.at(x0, y0);
        let b = self.at(x0 + 1, y0);
        let c = self.at(x0, y0 + 1);
        let d = self.at(x0 + 1, y0 + 1);
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bot = c[i] + (d[i] - c[i]) * fx;
            out[i] = top + (bot - top) * fy;
        }
        out
    }
}

/// `f32` rounded to the nearest `f16` (what an `Rgba16Float` level stores), for the values a blur holds (0..=1 and a little over).
pub fn to_f16(v: f32) -> f32 {
    if !v.is_finite() || v == 0.0 {
        return v;
    }
    let a = v.abs();
    if a < 6.103_515_6e-5 {
        // Subnormal: steps of 2^-24.
        let step = 5.960_464_5e-8_f32;
        return (v / step).round_ties_even() * step;
    }
    let e = a.log2().floor();
    let step = (e - 10.0).exp2();
    (v / step).round_ties_even() * step
}

/// sRGB to linear (the exact curve).
pub fn decode(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear to sRGB (the exact curve), 0..1.
pub fn encode(l: f32) -> f32 {
    let c = l.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn down(src: &Plane) -> Plane {
    let (w, h) = (src.w.div_ceil(2).max(1), src.h.div_ceil(2).max(1));
    let mut px = Vec::with_capacity((w * h) as usize);
    for j in 0..h {
        for i in 0..w {
            let (cx, cy) = (2.0 * i as f32 + 1.0, 2.0 * j as f32 + 1.0);
            let mut s = src.sample(cx, cy).map(|v| v * 4.0);
            for (dx, dy) in [(-1.0, -1.0), (1.0, 1.0), (1.0, -1.0), (-1.0, 1.0)] {
                let t = src.sample(cx + dx, cy + dy);
                for k in 0..4 {
                    s[k] += t[k];
                }
            }
            px.push(s.map(|v| to_f16(v / 8.0)));
        }
    }
    Plane { w, h, px }
}

fn up_at(src: &Plane, cx: f32, cy: f32) -> [f32; 4] {
    let mut s = [0.0f32; 4];
    for (dx, dy, wgt) in [
        (-1.0, 0.0, 1.0),
        (-0.5, 0.5, 2.0),
        (0.0, 1.0, 1.0),
        (0.5, 0.5, 2.0),
        (1.0, 0.0, 1.0),
        (0.5, -0.5, 2.0),
        (0.0, -1.0, 1.0),
        (-0.5, -0.5, 2.0),
    ] {
        let t = src.sample(cx + dx, cy + dy);
        for k in 0..4 {
            s[k] += t[k] * wgt;
        }
    }
    s.map(|v| v / 12.0)
}

fn up(src: &Plane, w: u32, h: u32) -> Plane {
    let mut px = Vec::with_capacity((w * h) as usize);
    for j in 0..h {
        for i in 0..w {
            let c = up_at(src, (i as f32 + 0.5) * 0.5, (j as f32 + 0.5) * 0.5);
            px.push(c.map(to_f16));
        }
    }
    Plane { w, h, px }
}

/// The rounded rectangle's coverage at a pixel centre (the shader's `coverage`).
fn coverage(rect: [f32; 4], corner: f32, px: f32, py: f32) -> f32 {
    let (cx, cy) = ((rect[0] + rect[2]) * 0.5, (rect[1] + rect[3]) * 0.5);
    let (hx, hy) = ((rect[2] - rect[0]) * 0.5, (rect[3] - rect[1]) * 0.5);
    let r = corner.min(hx.min(hy));
    let qx = (px - cx).abs() - hx + r;
    let qy = (py - cy).abs() - hy + r;
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    let d = outside + qx.max(qy).min(0.0) - r;
    (0.5 - d).clamp(0.0, 1.0)
}

/// The CPU reference of one backdrop drawn over `image` (`size.0 * size.1` RGBA8 texels, rows top to bottom), as the GPU path draws
/// it with `blur` on: the same placement, levels, taps and composite, the levels rounded to `f16` as `Rgba16Float` stores them.
pub fn reference(image: &mut [u8], size: (u32, u32), b: &BackdropDraw, blur: bool) {
    let Some((area, grab)) = placement(b, size) else {
        return;
    };
    let levels = if blur {
        levels(b.radius.min(MAX_BLUR_RADIUS))
    } else {
        0
    };
    let w = size.0 as usize;
    let (gw, gh) = ((grab[2] - grab[0]) as u32, (grab[3] - grab[1]) as u32);
    let level1 = if levels > 0 {
        let mut px = Vec::with_capacity((gw * gh) as usize);
        for y in grab[1]..grab[3] {
            for x in grab[0]..grab[2] {
                let i = (y as usize * w + x as usize) * 4;
                px.push([
                    decode(image[i]),
                    decode(image[i + 1]),
                    decode(image[i + 2]),
                    f32::from(image[i + 3]) / 255.0,
                ]);
            }
        }
        let mut chain = vec![Plane { w: gw, h: gh, px }];
        for _ in 1..=levels {
            let next = down(chain.last().expect("level 0"));
            chain.push(next);
        }
        let mut cur = chain.pop().expect("a level");
        while chain.len() > 1 {
            let below = chain.pop().expect("a level");
            cur = up(&cur, below.w, below.h);
        }
        Some(cur)
    } else {
        None
    };
    let rect = [
        b.rect[0] as f32,
        b.rect[1] as f32,
        b.rect[0].saturating_add(b.rect[2]) as f32,
        b.rect[1].saturating_add(b.rect[3]) as f32,
    ];
    let tint = b.tint.map(|c| f32::from(c) / 255.0);
    let opacity = if b.opacity.is_finite() {
        b.opacity.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let corner = b.corner.min(MAX_CORNER_RADIUS) as f32;
    for y in area[1]..area[3] {
        for x in area[0]..area[2] {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let a = coverage(rect, corner, px, py) * opacity;
            let (rgb, alpha) = match &level1 {
                Some(l1) => {
                    let l = up_at(l1, (px - grab[0] as f32) * 0.5, (py - grab[1] as f32) * 0.5);
                    let g = [encode(l[0]), encode(l[1]), encode(l[2])];
                    (
                        [
                            g[0] + (tint[0] - g[0]) * tint[3],
                            g[1] + (tint[1] - g[1]) * tint[3],
                            g[2] + (tint[2] - g[2]) * tint[3],
                        ],
                        a,
                    )
                }
                None => ([tint[0], tint[1], tint[2]], tint[3] * a),
            };
            let i = (y as usize * w + x as usize) * 4;
            for k in 0..3 {
                let dst = f32::from(image[i + k]) / 255.0;
                let v = rgb[k] * alpha + dst * (1.0 - alpha);
                image[i + k] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            let dst = f32::from(image[i + 3]) / 255.0;
            let v = alpha * alpha + dst * (1.0 - alpha);
            image[i + 3] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
}
