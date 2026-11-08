// ANCHOR: pass
use torque_app::render::{HdPass, PassCx};
use torque_render_core::wgpu;
use torque_render_hd::passes::{DEPTH_FORMAT, HDR_FORMAT};

use crate::sim::Ropes;

/// Flat colored lines: each vertex is a world point and a color, the matrix the camera's.
const SHADER: &str = r#"
@group(0) @binding(0) var<uniform> view_proj: mat4x4<f32>;
struct Out { @builtin(position) clip: vec4<f32>, @location(0) color: vec4<f32> };
@vertex fn vs(@location(0) p: vec3<f32>, @location(1) c: vec4<f32>) -> Out {
    var o: Out;
    o.clip = view_proj * vec4<f32>(p, 1.0);
    o.color = c;
    return o;
}
@fragment fn fs(o: Out) -> @location(0) vec4<f32> { return o.color; }
"#;

/// Draws every rope as a strip of lines into the HDR frame, hidden behind what is nearer.
pub struct LinesPass {
    ropes: Ropes,
    pipeline: Option<wgpu::RenderPipeline>,
}

impl LinesPass {
    pub fn new(ropes: Ropes) -> LinesPass {
        LinesPass {
            ropes,
            pipeline: None,
        }
    }

    fn pipeline(&mut self, device: &wgpu::Device) -> &wgpu::RenderPipeline {
        self.pipeline.get_or_insert_with(|| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("rope lines"),
                source: wgpu::ShaderSource::Wgsl(SHADER.into()),
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("rope lines"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: 28,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::LineList,
                    ..Default::default()
                },
                // The depth is reversed (1 near, 0 far): what is nearer has the greater value.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(HDR_FORMAT.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }
}

impl HdPass for LinesPass {
    fn draw(&mut self, cx: &mut PassCx<'_>) {
        use wgpu::util::DeviceExt;
        // The opaque stage has the HDR frame and the depth to draw over.
        let (Some(hdr), Some(depth)) = (cx.hdr, cx.depth) else {
            return;
        };
        let mut vertices: Vec<u8> = Vec::new();
        for rope in self.ropes.lock().iter() {
            let color = rope.color.map(|c| f32::from(c) / 255.0);
            for pair in rope.points.windows(2) {
                for p in pair {
                    for v in p.iter().chain(&color) {
                        vertices.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }
        }
        if vertices.is_empty() {
            return;
        }
        let device = cx.device;
        let matrix: Vec<u8> = cx
            .camera
            .view_proj
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rope matrix"),
            contents: &matrix,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rope vertices"),
            contents: &vertices,
            usage: wgpu::BufferUsages::VERTEX,
        });
        let pipeline = self.pipeline(device).clone();
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rope matrix"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let mut pass = cx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rope lines"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: hdr,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
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
        let [x, y, w, h] = cx.viewport;
        pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        pass.draw(0..(vertices.len() / 28) as u32, 0..1);
    }
}
// ANCHOR_END: pass
