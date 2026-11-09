//! The GUI pass: a [`torque_gui::GuiDraws`] list drawn with `gui.wgsl` in the GL state TGE's GUI is drawn with: an
//! orthographic projection in pixels, `GL_MODULATE` of the texture by the vertex colour, blending `SRC_ALPHA, ONE_MINUS_SRC_ALPHA` (or
//! `DST_COLOR, ONE_MINUS_SRC_ALPHA`, which multiplies the destination), each batch clipped to its rectangle (a scissor),
//! no depth. Each texture keeps its filter (`GL_LINEAR` for bitmaps, `GL_NEAREST` for the font sheets; no mip levels) and its wrap
//! (`GL_CLAMP_TO_EDGE` or `GL_REPEAT`) and is uploaded once by its id. The 3D renderers are untouched: this pass has its own
//! shader, pipelines and buffers, and draws only into the target it is given.
//!
//! A frame's backdrops (the extras of the draw list, [`torque_gui::draw::ExtraDraw`], that the [`Backdrops`] the pass was made with
//! claims) are drawn by that hook, each between the batches before and after it; a pass made without one draws the batches alone.

use std::collections::HashMap;

use torque_gui::GuiDraws;
use torque_gui::draw::{ExtraDraw, GuiBlend};

use crate::{COLOR_FORMAT, Gpu};

const SHADER: &str = include_str!("gui.wgsl");
/// Bytes per vertex: position (2 floats), texture coordinates (2 floats), RGBA8.
const VERTEX_BYTES: u64 = 20;
/// A texture not drawn for this many frames is dropped from the GPU.
pub const KEEP_FRAMES: u64 = 600;

/// What draws a frame's backdrops: the GUI pass ends before a backdrop, [`Backdrops::encode`] records it (it may read what the
/// target holds so far), then the pass resumes.
pub trait Backdrops: Send {
    /// Whether `extra` is one of the backdrops this hook draws.
    fn claims(&self, extra: &ExtraDraw) -> bool;

    /// A frame starts (before its first [`encode`](Self::encode); every frame, with backdrops or not).
    fn begin_frame(&mut self);

    /// The backdrops of one segment of the pass, recorded into `encoder` after the batches before them, onto `target`. `timer`:
    /// a timestamp to write at the end of the last pass recorded. Returns how many were drawn.
    fn encode(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        backdrops: &[&ExtraDraw],
        timer: Option<(&wgpu::QuerySet, u32)>,
    ) -> usize;

    /// The frame's segments are all recorded (before the frame is submitted): what they need written to the queue.
    fn end_frame(&mut self, queue: &wgpu::Queue);

    /// The offscreen target of `size` a window draws a frame with backdrops into (a window's surface cannot be read from).
    fn target(&mut self, gpu: &Gpu, size: (u32, u32)) -> wgpu::TextureView;

    /// Copies the offscreen target onto `surface`, texel for texel.
    fn present(&self, gpu: &Gpu, surface: &wgpu::TextureView);
}

/// The vertices of `draws` into `bytes` (cleared first; its room kept): four a quad (its corners counter-clockwise from the top
/// left), batch after batch. [`QUAD_INDICES`] makes each quad's two triangles of them.
pub fn vertex_bytes(draws: &GuiDraws, bytes: &mut Vec<u8>) {
    const QUAD: usize = 4 * VERTEX_BYTES as usize;
    bytes.clear();
    bytes.resize(draws.quads() * QUAD, 0);
    let quads = draws.batches.iter().flat_map(|b| &b.quads);
    for (q, out) in quads.zip(bytes.as_chunks_mut::<QUAD>().0) {
        let [x0, y0, x1, y1] = q.rect;
        let [u0, v0, u1, v1] = q.uv;
        let corners = [
            (x0, y0, u0, v0),
            (x1, y0, u1, v0),
            (x1, y1, u1, v1),
            (x0, y1, u0, v1),
        ];
        for ((x, y, u, v), vertex) in corners
            .into_iter()
            .zip(out.as_chunks_mut::<{ VERTEX_BYTES as usize }>().0)
        {
            vertex[0..4].copy_from_slice(&x.to_le_bytes());
            vertex[4..8].copy_from_slice(&y.to_le_bytes());
            vertex[8..12].copy_from_slice(&u.to_le_bytes());
            vertex[12..16].copy_from_slice(&v.to_le_bytes());
            vertex[16..20].copy_from_slice(&q.color);
        }
    }
}

/// A quad's two triangles of its four corners ([`vertex_bytes`]): the first three, then the first, the third and the fourth.
pub const QUAD_INDICES: [u32; 6] = [0, 1, 2, 0, 2, 3];

struct GpuTexture {
    group: wgpu::BindGroup,
    last_used: u64,
    _texture: wgpu::Texture,
}

/// The GUI pass's pipelines, textures and vertex buffer.
pub struct GuiPass {
    alpha: wgpu::RenderPipeline,
    multiply: wgpu::RenderPipeline,
    screen: wgpu::Buffer,
    screen_group: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    white: wgpu::BindGroup,
    _white_texture: wgpu::Texture,
    textures: HashMap<u64, GpuTexture>,
    vertices: Option<wgpu::Buffer>,
    /// The last frame's vertex bytes (their room kept).
    bytes: Vec<u8>,
    /// Every quad's [`QUAD_INDICES`] for as many quads as it holds (written when it grows).
    indices: Option<(wgpu::Buffer, usize)>,
    frame: u64,
    /// What draws the backdrops (None: they are left out).
    backdrops: Option<Box<dyn Backdrops>>,
    timer: Option<Timer>,
}

fn pipeline(
    device: &wgpu::Device,
    cache: Option<&wgpu::PipelineCache>,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    blend: GuiBlend,
) -> wgpu::RenderPipeline {
    let component = |src| wgpu::BlendComponent {
        src_factor: src,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    };
    let state = match blend {
        GuiBlend::Alpha => wgpu::BlendState {
            color: component(wgpu::BlendFactor::SrcAlpha),
            alpha: component(wgpu::BlendFactor::SrcAlpha),
        },
        GuiBlend::Multiply => wgpu::BlendState {
            color: component(wgpu::BlendFactor::Dst),
            alpha: component(wgpu::BlendFactor::Dst),
        },
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("gui"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: VERTEX_BYTES,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Unorm8x4],
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend: Some(state),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache,
    })
}

fn texture_group(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    size: (u32, u32),
    rgba: &[u8],
    nearest: bool,
    clamp: bool,
) -> (wgpu::Texture, wgpu::BindGroup) {
    let extent = wgpu::Extent3d {
        width: size.0.max(1),
        height: size.1.max(1),
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gui"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * extent.width),
            rows_per_image: Some(extent.height),
        },
        extent,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let address = if clamp {
        wgpu::AddressMode::ClampToEdge
    } else {
        wgpu::AddressMode::Repeat
    };
    let filter = if nearest {
        wgpu::FilterMode::Nearest
    } else {
        wgpu::FilterMode::Linear
    };
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("gui"),
        address_mode_u: address,
        address_mode_v: address,
        address_mode_w: address,
        mag_filter: filter,
        min_filter: filter,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        lod_max_clamp: 0.0,
        ..wgpu::SamplerDescriptor::default()
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("gui texture"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    (texture, group)
}

impl GuiPass {
    /// The pass on `gpu`'s device, its backdrops drawn by `backdrops`.
    pub fn new(gpu: &Gpu, backdrops: Option<Box<dyn Backdrops>>) -> GuiPass {
        let device = gpu.device();
        let cache = gpu.pipeline_cache();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gui"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let screen_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gui screen"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gui texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gui"),
            bind_group_layouts: &[Some(&screen_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let screen = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gui screen"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let screen_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gui screen"),
            layout: &screen_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: screen.as_entire_binding(),
            }],
        });
        let (white_texture, white) = texture_group(
            device,
            gpu.queue(),
            &texture_layout,
            (1, 1),
            &[255; 4],
            true,
            true,
        );
        GuiPass {
            alpha: pipeline(device, cache, &layout, &shader, GuiBlend::Alpha),
            multiply: pipeline(device, cache, &layout, &shader, GuiBlend::Multiply),
            screen,
            screen_group,
            texture_layout,
            white,
            _white_texture: white_texture,
            textures: HashMap::new(),
            vertices: None,
            bytes: Vec::new(),
            indices: None,
            frame: 0,
            backdrops,
            timer: None,
        }
    }

    /// The index buffer holds `quads` quads' indices (made anew, at a power of two of them, when it does not).
    fn ensure_indices(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, quads: usize) {
        let quads = quads.max(1);
        if self.indices.as_ref().is_some_and(|(_, n)| *n >= quads) {
            return;
        }
        let n = quads.next_power_of_two();
        let mut bytes = Vec::with_capacity(n * 6 * 4);
        for q in 0..n as u32 {
            for i in QUAD_INDICES {
                bytes.extend_from_slice(&(q * 4 + i).to_le_bytes());
            }
        }
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gui indices"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, &bytes);
        self.indices = Some((buffer, n));
    }

    /// Textures on the GPU now.
    pub fn textures(&self) -> usize {
        self.textures.len()
    }

    /// Draws `draws` into `view` (a target of `draws.size`), first cleared to `clear` when given, and submits the work. With
    /// backdrops the batches are drawn in segments, each backdrop between the batches before and after it
    /// ([`Backdrops`]); without (or without a [`Backdrops`]), in one pass.
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        draws: &GuiDraws,
        view: &wgpu::TextureView,
        clear: Option<[f64; 4]>,
    ) {
        self.frame += 1;
        let mut backdrops = self.backdrops.take();
        if let Some(b) = backdrops.as_mut() {
            b.begin_frame();
        }
        let device = gpu.device();
        let queue = gpu.queue();
        let (w, h) = (draws.size.0.max(1), draws.size.1.max(1));
        let floats = [w as f32, h as f32, 0.0, 0.0];
        // The scissors are cut by the target as well as the canvas: for a frame after a mode change the canvas may be
        // larger than the surface it is drawn to.
        let target = view.texture();
        let clip = (w.min(target.width()), h.min(target.height()));
        queue.write_buffer(
            &self.screen,
            0,
            &floats
                .iter()
                .flat_map(|f| f.to_le_bytes())
                .collect::<Vec<u8>>(),
        );
        let mut bytes = std::mem::take(&mut self.bytes);
        vertex_bytes(draws, &mut bytes);
        self.ensure_indices(device, queue, draws.quads());
        let needed = (bytes.len() as u64).max(VERTEX_BYTES * 4);
        if self.vertices.as_ref().is_none_or(|v| v.size() < needed) {
            self.vertices = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gui vertices"),
                size: needed.next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let Some(vertices) = self.vertices.as_ref() else {
            self.backdrops = backdrops;
            return;
        };
        if !bytes.is_empty() {
            queue.write_buffer(vertices, 0, &bytes);
        }
        self.bytes = bytes;
        // Textures first seen.
        for b in &draws.batches {
            if let Some(t) = &b.texture {
                let frame = self.frame;
                let entry = self.textures.entry(t.id).or_insert_with(|| {
                    let (texture, group) = texture_group(
                        device,
                        queue,
                        &self.texture_layout,
                        t.size,
                        &t.rgba,
                        t.nearest,
                        t.clamp,
                    );
                    GpuTexture {
                        group,
                        last_used: frame,
                        _texture: texture,
                    }
                });
                entry.last_used = frame;
            }
        }
        let frame = self.frame;
        self.textures
            .retain(|_, t| frame.saturating_sub(t.last_used) < KEEP_FRAMES);
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("gui") });
        let n = draws.batches.len();
        let claimed: Vec<&ExtraDraw> = backdrops.as_ref().map_or_else(Vec::new, |hook| {
            draws.extras.iter().filter(|x| hook.claims(x)).collect()
        });
        if let Some(hook) = backdrops.as_mut().filter(|_| !claimed.is_empty()) {
            // The backdrops in batch order (a stable sort keeps the walk's order at one batch).
            let mut at = claimed;
            at.sort_by_key(|b| b.at_batch);
            let timer = self.timer.take();
            let queries = timer.as_ref().map(|t| &t.queries);
            let mut clear = clear;
            let mut start = 0usize;
            let mut i = 0usize;
            let mut timed_end = false;
            while i < at.len() {
                let end = at[i].at_batch.clamp(start, n);
                let mut j = i;
                while j < at.len() && at[j].at_batch.clamp(start, n) == end {
                    j += 1;
                }
                let first = i == 0;
                self.batches_pass(
                    &mut encoder,
                    draws,
                    view,
                    start..end,
                    clear.take(),
                    clip,
                    queries.filter(|_| first).map(|q| (q, 0)),
                );
                let last = j == at.len();
                let drawn = hook.encode(
                    gpu,
                    &mut encoder,
                    view,
                    &at[i..j],
                    queries.filter(|_| last).map(|q| (q, 1)),
                );
                timed_end |= last && drawn > 0;
                start = end;
                i = j;
            }
            hook.end_frame(queue);
            // The end timestamp falls on the last batches when no composite wrote it (every backdrop off the target).
            let end_query = queries.filter(|_| !timed_end).map(|q| (q, 1));
            self.batches_pass(&mut encoder, draws, view, start..n, None, clip, end_query);
            if let Some(t) = timer.as_ref() {
                encoder.resolve_query_set(&t.queries, 0..2, &t.resolve, 0);
                encoder.copy_buffer_to_buffer(&t.resolve, 0, &t.read, 0, 16);
            }
            self.timer = timer;
        } else {
            self.batches_pass(&mut encoder, draws, view, 0..n, clear, clip, None);
        }
        self.backdrops = backdrops;
        queue.submit([encoder.finish()]);
    }

    /// One render pass of the batches `range` (cleared first when `clear` is given; a pass is begun to clear, or to time, with no
    /// batch).
    #[allow(clippy::too_many_arguments)]
    fn batches_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        draws: &GuiDraws,
        view: &wgpu::TextureView,
        range: std::ops::Range<usize>,
        clear: Option<[f64; 4]>,
        (w, h): (u32, u32),
        timer: Option<(&wgpu::QuerySet, u32)>,
    ) {
        if range.is_empty() && clear.is_none() && timer.is_none() {
            return;
        }
        let Some(vertices) = self.vertices.as_ref() else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: match clear {
                        Some([r, g, b, a]) => wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                        None => wgpu::LoadOp::Load,
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: timer.map(|(q, at)| wgpu::RenderPassTimestampWrites {
                query_set: q,
                beginning_of_pass_write_index: None,
                end_of_pass_write_index: Some(at),
            }),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let Some((indices, _)) = self.indices.as_ref() else {
            return;
        };
        pass.set_bind_group(0, &self.screen_group, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        let mut first: u32 = draws.batches[..range.start.min(draws.batches.len())]
            .iter()
            .map(|b| (b.quads.len() * 6) as u32)
            .sum();
        for b in &draws.batches[range] {
            let count = (b.quads.len() * 6) as u32;
            let start = first;
            first += count;
            // The clip rectangle inside the target (a scissor must be).
            let x0 = b.clip[0].clamp(0, w as i32);
            let y0 = b.clip[1].clamp(0, h as i32);
            let x1 = b.clip[0].saturating_add(b.clip[2]).clamp(0, w as i32);
            let y1 = b.clip[1].saturating_add(b.clip[3]).clamp(0, h as i32);
            if x1 <= x0 || y1 <= y0 || count == 0 {
                continue;
            }
            pass.set_scissor_rect(x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32);
            pass.set_pipeline(match b.blend {
                GuiBlend::Alpha => &self.alpha,
                GuiBlend::Multiply => &self.multiply,
            });
            let group = b
                .texture
                .as_ref()
                .and_then(|t| self.textures.get(&t.id))
                .map_or(&self.white, |t| &t.group);
            pass.set_bind_group(1, group, &[]);
            pass.draw_indexed(start..start + count, 0, 0..1);
        }
    }

    /// The offscreen GUI target a window draws a frame with backdrops into ([`Backdrops::target`]); None when the pass has no
    /// [`Backdrops`] (the frame is drawn onto the surface).
    pub fn target(&mut self, gpu: &Gpu, size: (u32, u32)) -> Option<wgpu::TextureView> {
        self.backdrops.as_mut().map(|b| b.target(gpu, size))
    }

    /// The offscreen GUI target copied onto the window surface ([`Backdrops::present`]).
    pub fn present(&self, gpu: &Gpu, surface: &wgpu::TextureView) {
        if let Some(b) = self.backdrops.as_ref() {
            b.present(gpu, surface);
        }
    }

    /// Times the backdrops from now on with GPU timestamps (when the device has them; false otherwise): the end of the batches
    /// before the first backdrop to the end of the last composite, read by [`backdrop_ms`](Self::backdrop_ms).
    pub fn time_backdrops(&mut self, gpu: &Gpu) -> bool {
        let device = gpu.device();
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return false;
        }
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("gui backdrops"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gui backdrops resolve"),
            size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gui backdrops read"),
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.timer = Some(Timer {
            queries,
            resolve,
            read,
            period_ns: f64::from(gpu.queue().get_timestamp_period()),
        });
        true
    }

    /// The last frame's backdrop time in milliseconds (waits for the GPU).
    pub fn backdrop_ms(&self, gpu: &Gpu) -> Option<f64> {
        let t = self.timer.as_ref()?;
        let slice = t.read.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
        rx.recv().ok()?.ok()?;
        let ticks = {
            let data = slice.get_mapped_range().ok()?;
            let a = u64::from_le_bytes(data[0..8].try_into().ok()?);
            let b = u64::from_le_bytes(data[8..16].try_into().ok()?);
            b.saturating_sub(a)
        };
        t.read.unmap();
        Some(ticks as f64 * t.period_ns / 1e6)
    }
}

/// The backdrops' GPU timer.
struct Timer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    period_ns: f64,
}
