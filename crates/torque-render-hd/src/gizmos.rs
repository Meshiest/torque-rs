//! Debug lines drawn over a finished picture: [`GizmoLines`] draws a batch of world-space segments, each a screen-space quad of a
//! given width in pixels, in one draw call, into the rectangle of a target where the world was just drawn.
//!
//! A segment is depth-tested or not. The target is an `Rgba8Unorm` picture with no depth buffer of its own, so a tested segment
//! reads the frame's working depth ([`crate::HdWorld::depth_view`], reversed: nearer is larger) in its fragment shader and is
//! dropped where the scene is nearer; without a depth (the world was drawn by another renderer) every segment is drawn on top.
//! Colors are the picture's own (already display values): nothing is tonemapped or converted.
//!
//! The pass is made on the first draw and costs nothing before it. Nothing is drawn, and no pass begun, for an empty batch.

use torque_render_core::{Gpu, wgpu};
use torque_render_scene::hd::HdCamera;

use crate::{PassKind, count_pass_as, push_f32};

/// The most segments one draw takes (the batch's tail past it is left out).
pub const MAX_SEGMENTS: usize = 1 << 20;
/// Bytes of a [`GizmoSegment`] as the shader reads it.
const SEGMENT_BYTES: usize = 32;
/// Bytes of the pass's uniform: the matrix, the rectangle and four parameters.
const GLOBALS_BYTES: usize = 64 + 16 + 16;

/// One line segment in world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GizmoSegment {
    pub a: [f32; 3],
    pub b: [f32; 3],
    /// Red, green, blue and alpha of the display color.
    pub color: [u8; 4],
    /// Hidden where the scene is nearer than the segment.
    pub depth_test: bool,
}

impl GizmoSegment {
    fn write(&self, out: &mut Vec<u8>) {
        push_f32(out, &self.a);
        out.extend_from_slice(&u32::from(self.depth_test).to_le_bytes());
        push_f32(out, &self.b);
        out.extend_from_slice(&self.color);
    }
}

/// The WGSL of the pass.
pub const SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    // The viewport in the target's pixels: left, top, width, height.
    rect: vec4<f32>,
    // The line's width in pixels, whether the scene's depth is bound, the near plane, unused.
    params: vec4<f32>,
}

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var scene_depth: texture_depth_2d;

struct Vertex {
    @builtin(vertex_index) corner: u32,
    @location(0) a: vec3<f32>,
    @location(1) flags: u32,
    @location(2) b: vec3<f32>,
    @location(3) color: vec4<f32>,
}

struct Varying {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) @interpolate(flat) test: u32,
    @location(2) across: f32,
}

@vertex
fn vs_line(v: Vertex) -> Varying {
    var out: Varying;
    out.color = v.color;
    out.test = v.flags & 1u;
    out.across = 0.0;
    // Everything outside the clip volume: nothing is drawn.
    out.pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    var c0 = g.view_proj * vec4<f32>(v.a, 1.0);
    var c1 = g.view_proj * vec4<f32>(v.b, 1.0);
    // Cut at the near plane (w is the distance in front of the eye).
    let near = max(g.params.z, 1.0e-4);
    if c0.w < near && c1.w < near {
        return out;
    }
    if c0.w < near {
        c0 = mix(c0, c1, (near - c0.w) / (c1.w - c0.w));
    } else if c1.w < near {
        c1 = mix(c1, c0, (near - c1.w) / (c0.w - c1.w));
    }
    let n0 = c0.xyz / c0.w;
    let n1 = c1.xyz / c1.w;
    let half_size = 0.5 * g.rect.zw;
    // Pixels from the viewport's center, y down.
    let s0 = vec2<f32>(n0.x, -n0.y) * half_size;
    let s1 = vec2<f32>(n1.x, -n1.y) * half_size;
    var d = s1 - s0;
    let len = length(d);
    if len < 1.0e-4 {
        d = vec2<f32>(1.0, 0.0);
    } else {
        d = d / len;
    }
    let side = select(-1.0, 1.0, (v.corner & 1u) == 1u);
    let end = f32(v.corner >> 1u);
    // One pixel more than the line's half width: the edge is drawn antialiased across it.
    let half_w = 0.5 * g.params.x + 1.0;
    let s = mix(s0, s1, end) + d * (end * 2.0 - 1.0) * (half_w - 1.0) + vec2<f32>(-d.y, d.x) * side * half_w;
    out.pos = vec4<f32>(s.x / half_size.x, -s.y / half_size.y, mix(n0.z, n1.z, end), 1.0);
    out.across = side * half_w;
    return out;
}

@fragment
fn fs_line(in: Varying) -> @location(0) vec4<f32> {
    let cover = clamp(0.5 * g.params.x + 0.5 - abs(in.across), 0.0, 1.0);
    if cover <= 0.0 {
        discard;
    }
    if in.test == 1u && g.params.y > 0.5 {
        let local = in.pos.xy - g.rect.xy;
        let dims = vec2<f32>(textureDimensions(scene_depth));
        let at = vec2<i32>(clamp(local / g.rect.zw * dims, vec2<f32>(0.0), dims - vec2<f32>(1.0)));
        let scene = textureLoad(scene_depth, at, 0);
        // Reversed depth: nearer is larger. A line lying on a surface is within 2% of its distance.
        if scene > in.pos.z * 1.02 + 1.0e-6 {
            discard;
        }
    }
    return vec4<f32>(in.color.rgb, in.color.a * cover);
}
"#;

/// What a batch of segments is drawn over: the target and the rectangle of it the world was drawn into (left, top, width, height),
/// the camera that drew it, the frame's working depth the tested segments are checked against (None: none is hidden) and the
/// lines' width in pixels.
#[derive(Clone, Copy)]
pub struct GizmoFrame<'a> {
    pub target: &'a wgpu::TextureView,
    pub rect: [u32; 4],
    pub camera: &'a HdCamera,
    pub depth: Option<&'a wgpu::TextureView>,
    pub width: f32,
}

/// The lines pass: pipeline, uniform and instance buffer, made on first use.
pub struct GizmoLines {
    layout: wgpu::BindGroupLayout,
    module: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    pipeline: Option<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    globals: wgpu::Buffer,
    instances: Option<(wgpu::Buffer, usize)>,
    /// Bound where the frame has no depth to read.
    stand_in: wgpu::TextureView,
}

impl GizmoLines {
    pub fn new(gpu: &Gpu) -> GizmoLines {
        let device = gpu.device();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gizmo lines"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gizmo lines"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gizmo lines"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gizmo globals"),
            size: GLOBALS_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let stand_in = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("gizmo depth stand-in"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: crate::DEPTH_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        GizmoLines {
            layout,
            module,
            pipeline_layout,
            pipeline: None,
            globals,
            instances: None,
            stand_in,
        }
    }

    fn pipeline(&mut self, gpu: &Gpu, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        if self.pipeline.as_ref().is_none_or(|(f, _)| *f != format) {
            let pipeline = gpu
                .device()
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("gizmo lines"),
                    layout: Some(&self.pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &self.module,
                        entry_point: Some("vs_line"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: SEGMENT_BYTES as u64,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: &[
                                wgpu::VertexAttribute {
                                    format: wgpu::VertexFormat::Float32x3,
                                    offset: 0,
                                    shader_location: 0,
                                },
                                wgpu::VertexAttribute {
                                    format: wgpu::VertexFormat::Uint32,
                                    offset: 12,
                                    shader_location: 1,
                                },
                                wgpu::VertexAttribute {
                                    format: wgpu::VertexFormat::Float32x3,
                                    offset: 16,
                                    shader_location: 2,
                                },
                                wgpu::VertexAttribute {
                                    format: wgpu::VertexFormat::Unorm8x4,
                                    offset: 28,
                                    shader_location: 3,
                                },
                            ],
                        })],
                    },
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleStrip,
                        ..wgpu::PrimitiveState::default()
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &self.module,
                        entry_point: Some("fs_line"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: gpu.pipeline_cache(),
                });
            self.pipeline = Some((format, pipeline));
        }
        &self
            .pipeline
            .as_ref()
            .expect("the pipeline was just made")
            .1
    }

    /// Draws `segments` over what `frame` holds. Submits the work; nothing is drawn for an empty batch or an empty rectangle.
    /// Returns the segments drawn.
    pub fn draw(&mut self, gpu: &Gpu, frame: &GizmoFrame<'_>, segments: &[GizmoSegment]) -> usize {
        let GizmoFrame {
            target,
            rect,
            camera,
            depth,
            width,
        } = *frame;
        let [x, y, w, h] = rect;
        let count = segments.len().min(MAX_SEGMENTS);
        if count == 0 || w == 0 || h == 0 {
            return 0;
        }
        let device = gpu.device();
        let queue = gpu.queue();
        let mut bytes = Vec::with_capacity(count * SEGMENT_BYTES);
        for s in &segments[..count] {
            s.write(&mut bytes);
        }
        if self.instances.as_ref().is_none_or(|(_, cap)| *cap < count) {
            let cap = count.next_power_of_two().max(256);
            self.instances = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("gizmo segments"),
                    size: (cap * SEGMENT_BYTES) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                cap,
            ));
        }
        let mut globals = Vec::with_capacity(GLOBALS_BYTES);
        push_f32(&mut globals, &camera.view_proj);
        push_f32(&mut globals, &[x as f32, y as f32, w as f32, h as f32]);
        push_f32(
            &mut globals,
            &[
                width.max(1.0),
                if depth.is_some() { 1.0 } else { 0.0 },
                camera.near,
                0.0,
            ],
        );
        queue.write_buffer(&self.globals, 0, &globals);
        let format = target.texture().format();
        self.pipeline(gpu, format);
        let (instances, _) = self.instances.as_ref().expect("the buffer was just made");
        queue.write_buffer(instances, 0, &bytes);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gizmo lines"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.globals.as_entire_binding(),
                },
                crate::tv(1, depth.unwrap_or(&self.stand_in)),
            ],
        });
        let (_, pipeline) = self.pipeline.as_ref().expect("the pipeline was just made");
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gizmo lines"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gizmo lines"),
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
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
            pass.set_scissor_rect(x, y, w, h);
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.set_vertex_buffer(0, instances.slice(..(count * SEGMENT_BYTES) as u64));
            pass.draw(0..4, 0..count as u32);
        }
        count_pass_as(PassKind::Post);
        queue.submit([encoder.finish()]);
        count
    }
}

#[cfg(test)]
#[path = "gizmos_tests.rs"]
mod tests;
