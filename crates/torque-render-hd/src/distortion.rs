//! The screen-space distortion pass (`shaders/distortion.wgsl`): the frame's [`HdDistorter`]s bend the anti-aliased picture behind
//! them (the look's rules are `torque_render_scene::hd::distortion`'s).
//!
//! **Where in the frame.** After the TAA resolve, before the late sprites are added: the distorters in sight are drawn with the
//! unjittered camera into two targets of the internal size over the frame's depth (`offset`: the read's move in UV, the split's
//! share, the nearest distorter's depth; `tint`: a multiply), then one full-frame pass reads the resolved picture through them into
//! the shown copy the tonemap reads. The TAA's history never holds a bent pixel, so a moving bend leaves no trail.
//!
//! **What it costs.** Nothing without a distorter in sight: [`seen`] culls their spheres on the CPU, and with none the frame
//! makes no target, records no pass and copies nothing (it is the frame without the pass, byte for byte). The pass and its
//! targets are made the first frame one is seen. FXAA frames (Low, or the TAA off) skip it: it writes the shown copy only the
//! TAA's frame has.
//!
//! **The particles' seam.** A look of the GPU particle pool that bends draws its quads with `fs_distort` (a vertex entry of its
//! own, the look's slot as the instance) into the same targets: [`Distortion::encode`]'s `extra` callback is handed the open
//! pass with group 0 bound, after the distorters' draws; [`Distortion::look_slot`] reserves a look's slot for it.

use torque_render_core::wgpu;
use torque_render_scene::hd::HD_VERTEX_BYTES;
use torque_render_scene::hd::HdCamera;
use torque_render_scene::hd::distortion::{
    DistortionBudget, DistortionLook, HdDistorter, MAX_SCREEN_OFFSET, nearest_first,
};

use torque_render_scene::hd::HdVertex;

use crate::{DEPTH_FORMAT, push_f32, push_u32, storage, uniform};

/// The offset target's format (rg: the move in UV, b: the split's share, a: the nearest distorter's reversed depth).
pub const OFFSET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The tint target's format (a multiply, cleared to white).
pub const TINT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The bytes of one look in the looks' table (three `vec4<f32>`).
pub const LOOK_BYTES: usize = 48;
/// The uniform's bytes: the view, the projection, three `vec4<f32>`.
const FRAME_BYTES: usize = 16 * 4 * 2 + 16 * 3;

const SOURCE: &str = include_str!("../shaders/distortion.wgsl");

/// A sampled texture of the fragment stage at `binding`.
fn texture_entry_fragment(
    binding: u32,
    sample_type: wgpu::TextureSampleType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// The indices of `distorters` drawn this frame: those with a look that changes the picture whose sphere is not wholly outside
/// `planes` (the frustum of the camera they are drawn with), the nearest to `eye` first within a [`DistortionBudget`] (a frame's
/// distorters come from a server's ghosts and decals: the caps keep its buffers bounded whatever it sends).
pub fn seen(distorters: &[HdDistorter], planes: &[[f32; 4]], eye: [f32; 3]) -> Vec<usize> {
    let shown: Vec<usize> = distorters
        .iter()
        .enumerate()
        .filter(|(_, d)| d.is_drawn() && crate::sphere_seen(planes, d.centre, d.radius))
        .map(|(i, _)| i)
        .collect();
    let order = nearest_first(
        eye,
        shown
            .iter()
            .map(|&i| (distorters[i].centre, distorters[i].radius)),
    );
    let mut budget = DistortionBudget::default();
    let mut out = Vec::with_capacity(order.len());
    for k in order {
        if budget.is_spent() {
            break;
        }
        let d = &distorters[shown[k]].draw;
        if budget.take(1, d.vertices.len(), d.indices.len()) {
            out.push(shown[k]);
        }
    }
    out
}

/// A look's bytes in the looks' table (`Look` in the shader); `two_sided` for a draw without a back-face cull.
pub fn look_bytes(look: &DistortionLook, two_sided: bool) -> [u8; LOOK_BYTES] {
    let b = |v: bool| if v { 1.0 } else { 0.0 };
    let mut out = Vec::with_capacity(LOOK_BYTES);
    push_f32(
        &mut out,
        &[
            look.strength,
            look.edge,
            look.map_scale,
            look.chroma,
            look.scroll[0],
            look.scroll[1],
            b(look.map.is_some()),
            b(two_sided),
        ],
    );
    push_f32(&mut out, &look.tint);
    let mut a = [0u8; LOOK_BYTES];
    a.copy_from_slice(&out);
    a
}

/// What a frame's distortion is drawn with.
pub struct DistortFrame<'a> {
    /// The unjittered camera of the internal frame.
    pub camera: &'a HdCamera,
    /// The effects' time (seconds: the maps' scroll).
    pub time: f32,
    /// The frame's working depth (internal size).
    pub depth: &'a wgpu::TextureView,
    /// The anti-aliased picture (output size) read through the offsets, and the copy of it written.
    pub source: &'a wgpu::TextureView,
    pub target: &'a wgpu::TextureView,
    pub output: [u32; 2],
}

/// The internal-size targets the distorters are drawn into.
struct Targets {
    size: [u32; 2],
    offset: wgpu::TextureView,
    tint: wgpu::TextureView,
}

/// The pass's pipelines, buffers and targets ([`Distortion::new`] makes the pipelines; the targets come with the first frame).
pub struct Distortion {
    draw_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    draw: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    looks: wgpu::Buffer,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    sampler: wgpu::Sampler,
    targets: Option<Targets>,
    /// The frame's bytes, kept between frames (their room).
    vertex_bytes: Vec<u8>,
    index_bytes: Vec<u8>,
    look_bytes: Vec<u8>,
    /// The frame's draws: first index, index count, base vertex.
    draws: Vec<(u32, u32, i32)>,
    /// Looks reserved after the distorters' ([`Distortion::look_slot`]).
    extra_looks: u32,
}

fn buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16),
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// `buf` written with `bytes`, made again (the next power of two, at most the device's largest buffer) when they do not fit;
/// false, and nothing written, when they are larger than the device's largest buffer.
fn write_grown(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buf: &mut wgpu::Buffer,
    label: &str,
    usage: wgpu::BufferUsages,
    bytes: &[u8],
) -> bool {
    let len = bytes.len() as u64;
    let largest = device.limits().max_buffer_size;
    if len > largest {
        return false;
    }
    if len > buf.size() {
        *buf = buffer(device, label, len.next_power_of_two().min(largest), usage);
    }
    if !bytes.is_empty() {
        queue.write_buffer(buf, 0, bytes);
    }
    true
}

const MESH_ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
    0 => Float32x3, 1 => Snorm16x2, 2 => Float32x2, 3 => Unorm8x4, 4 => Uint32, 5 => Uint32
];

impl Distortion {
    /// The pass's pipelines; `texture_layout` is the forward pass's texture group (a look's map is bound through it).
    pub fn new(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        texture_layout: &wgpu::BindGroupLayout,
    ) -> Distortion {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("distortion.wgsl"),
            source: wgpu::ShaderSource::Wgsl(SOURCE.into()),
        });
        let both = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd distortion"),
            entries: &[
                uniform(0, both),
                storage(1, true, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hd distortion composite"),
            entries: &[
                uniform(0, wgpu::ShaderStages::FRAGMENT),
                texture_entry_fragment(2, float),
                texture_entry_fragment(3, float),
                texture_entry_fragment(4, float),
                texture_entry_fragment(5, wgpu::TextureSampleType::Depth),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let draw_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd distortion"),
            bind_group_layouts: &[Some(&draw_layout), Some(texture_layout)],
            immediate_size: 0,
        });
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let draw = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hd distortion"),
            layout: Some(&draw_pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_distort"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: HD_VERTEX_BYTES as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &MESH_ATTRIBUTES,
                })],
            },
            // Both sides reach the fragment stage: it keeps the back faces of a two-sided look only.
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: wgpu::StencilState::default(),
                // Toward the eye (reversed depth): a shape that is drawn and bends too has its own depth there, drawn with the
                // jittered camera, and the bias holds its surface in front of that within the jitter's half pixel of slope.
                bias: wgpu::DepthBiasState {
                    constant: 4,
                    slope_scale: 2.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_distort"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[
                    // The moves and the split add up; the depth keeps the nearest.
                    Some(wgpu::ColorTargetState {
                        format: OFFSET_FORMAT,
                        blend: Some(wgpu::BlendState {
                            color: add,
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Max,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    // The tints multiply.
                    Some(wgpu::ColorTargetState {
                        format: TINT_FORMAT,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::Dst,
                                dst_factor: wgpu::BlendFactor::Zero,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent::REPLACE,
                        }),
                        write_mask: wgpu::ColorWrites::COLOR,
                    }),
                ],
            }),
            multiview_mask: None,
            cache,
        });
        let composite_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hd distortion composite"),
            bind_group_layouts: &[Some(&composite_layout)],
            immediate_size: 0,
        });
        let full = format!("{}\n{SOURCE}", include_str!("../shaders/fullscreen.wgsl"));
        let full_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("distortion.wgsl (composite)"),
            source: wgpu::ShaderSource::Wgsl(full.into()),
        });
        let composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hd distortion composite"),
            layout: Some(&composite_pl),
            vertex: wgpu::VertexState {
                module: &full_module,
                entry_point: Some("vs_full"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &full_module,
                entry_point: Some("fs_composite"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: crate::post::SHOWN_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hd distortion"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        Distortion {
            draw_layout,
            composite_layout,
            draw,
            composite,
            uniform: buffer(
                device,
                "hd distortion",
                FRAME_BYTES as u64,
                wgpu::BufferUsages::UNIFORM,
            ),
            looks: buffer(
                device,
                "hd distortion looks",
                LOOK_BYTES as u64 * 16,
                wgpu::BufferUsages::STORAGE,
            ),
            vertices: buffer(
                device,
                "hd distortion vertices",
                1 << 16,
                wgpu::BufferUsages::VERTEX,
            ),
            indices: buffer(
                device,
                "hd distortion indices",
                1 << 15,
                wgpu::BufferUsages::INDEX,
            ),
            sampler,
            targets: None,
            vertex_bytes: Vec::new(),
            index_bytes: Vec::new(),
            look_bytes: Vec::new(),
            draws: Vec::new(),
            extra_looks: 0,
        }
    }

    /// Reserves the next look's slot after the frame's distorters for a draw of the `extra` callback (its instance), written with
    /// `look`; call it between [`Distortion::prepare`] and [`Distortion::encode`].
    pub fn look_slot(&mut self, look: &DistortionLook, two_sided: bool) -> u32 {
        let slot = (self.look_bytes.len() / LOOK_BYTES) as u32;
        self.look_bytes
            .extend_from_slice(&look_bytes(look, two_sided));
        self.extra_looks += 1;
        slot
    }

    /// The frame's distorters `seen` (indices into `distorters`) as the pass's bytes, kept for [`Distortion::encode`].
    pub fn prepare(&mut self, distorters: &[HdDistorter], seen: &[usize]) {
        self.vertex_bytes.clear();
        self.index_bytes.clear();
        self.look_bytes.clear();
        self.draws.clear();
        self.extra_looks = 0;
        let (mut first, mut base) = (0u32, 0i32);
        for &i in seen {
            let d = &distorters[i];
            HdVertex::extend_bytes(&d.draw.vertices, &mut self.vertex_bytes);
            push_u32(&mut self.index_bytes, &d.draw.indices);
            let count = d.draw.indices.len() as u32;
            self.draws.push((first, count, base));
            first += count;
            base += d.draw.vertices.len() as i32;
            self.look_bytes.extend_from_slice(&look_bytes(
                &d.look,
                d.draw.cull == torque_render_scene::Cull::None,
            ));
        }
    }

    /// The internal-size targets, made at `size` when they are not.
    fn ensure_targets(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let tex = |label, format| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        self.targets = Some(Targets {
            size,
            offset: tex("hd distortion offset", OFFSET_FORMAT),
            tint: tex("hd distortion tint", TINT_FORMAT),
        });
    }

    /// Records the frame's distortion: the prepared distorters (each bound with its map's group, `group` of its index in
    /// `seen`) and then `extra`'s draws into the targets, and the composite of `f.source` through them into `f.target`.
    pub fn encode<'g>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        f: &DistortFrame<'_>,
        group: impl Fn(usize) -> &'g wgpu::BindGroup,
        extra: impl FnOnce(&mut wgpu::RenderPass<'_>),
    ) {
        let size = [f.camera.width, f.camera.height];
        self.ensure_targets(device, size);
        let mut frame = Vec::with_capacity(FRAME_BYTES);
        push_f32(&mut frame, &f.camera.view);
        push_f32(&mut frame, &f.camera.proj);
        let (w, h) = (size[0] as f32, size[1] as f32);
        let (ow, oh) = (f.output[0] as f32, f.output[1] as f32);
        push_f32(
            &mut frame,
            &[
                f.time,
                w / h,
                MAX_SCREEN_OFFSET,
                0.0,
                w,
                h,
                1.0 / w,
                1.0 / h,
                ow,
                oh,
                1.0 / ow,
                1.0 / oh,
            ],
        );
        queue.write_buffer(&self.uniform, 0, &frame);
        let (vb, ib, lb) = (
            std::mem::take(&mut self.vertex_bytes),
            std::mem::take(&mut self.index_bytes),
            std::mem::take(&mut self.look_bytes),
        );
        // A frame's bytes past the device's largest buffer draw nothing (the caps of `seen` keep them far under it).
        let written = write_grown(
            device,
            queue,
            &mut self.vertices,
            "hd distortion vertices",
            wgpu::BufferUsages::VERTEX,
            &vb,
        ) && write_grown(
            device,
            queue,
            &mut self.indices,
            "hd distortion indices",
            wgpu::BufferUsages::INDEX,
            &ib,
        ) && write_grown(
            device,
            queue,
            &mut self.looks,
            "hd distortion looks",
            wgpu::BufferUsages::STORAGE,
            &lb,
        );
        (self.vertex_bytes, self.index_bytes, self.look_bytes) = (vb, ib, lb);
        if !written {
            return;
        }
        let targets = self.targets.as_ref().expect("the targets were just made");
        let draw_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd distortion"),
            layout: &self.draw_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.looks.as_entire_binding(),
                },
            ],
        });
        {
            crate::count_pass();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hd distortion"),
                color_attachments: &[
                    Some(crate::colour(&targets.offset)),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &targets.tint,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: f.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.draw);
            pass.set_bind_group(0, &draw_group, &[]);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
            for (k, &(first, count, base)) in self.draws.iter().enumerate() {
                pass.set_bind_group(1, group(k), &[]);
                let slot = k as u32;
                pass.draw_indexed(first..first + count, base, slot..slot + 1);
            }
            crate::count_draws(self.draws.len());
            extra(&mut pass);
        }
        let composite_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hd distortion composite"),
            layout: &self.composite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                crate::tv(2, f.source),
                crate::tv(3, &targets.offset),
                crate::tv(4, &targets.tint),
                crate::tv(5, f.depth),
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        crate::count_pass();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hd distortion composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: f.target,
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
        pass.set_bind_group(0, &composite_group, &[]);
        pass.draw(0..3, 0..1);
        crate::count_draws(1);
    }
}

#[cfg(test)]
#[path = "distortion_tests.rs"]
mod tests;
