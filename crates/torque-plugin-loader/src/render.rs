//! The extensions' render passes, drawn by the engine's renderer: [`TrianglesPass`] draws flat colored triangles at a
//! stage of the frame (into the HDR frame, depth tested, at the world's stages; over the picture at the overlay), and an
//! extension's pass is one whose triangles come from the extension each frame.
//!
//! A compiled plugin can use [`TrianglesPass`] too, with a closure: the same triangles draw the same pixels whether they
//! come from Rust in the game or from a loaded library.

use std::sync::Arc;

use torque_app::render::{EngineRender, HdPass, PassCx, Stage};
use torque_engine::EngineBuilder;
use torque_plugin_abi::{
    TORQUE_ERR_PANIC, TORQUE_OK, TORQUE_SPACE_CLIP, TORQUE_SPACE_WORLD, TORQUE_STAGE_OPAQUE,
    TORQUE_STAGE_OVERLAY, TORQUE_STAGE_TRANSLUCENT, TorquePassFrame,
};
use torque_render_core::wgpu;

use crate::extension::ExtensionState;
use crate::ffi;

pub use torque_plugin_abi::TorqueVertex as Vertex;

/// What space a batch's vertices are in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    /// World units, seen through the frame's camera.
    World,
    /// Clip space: x and y from -1 to 1 across the viewport, y up (z is the reversed depth at the world's stages).
    Clip,
}

/// The triangles of one frame of a [`TrianglesPass`], in the order they draw.
#[derive(Default)]
pub struct TriangleList {
    batches: Vec<(Space, Vec<Vertex>)>,
}

impl TriangleList {
    /// Adds `vertices` (three a triangle; a partial last triangle is left out) in `space`.
    pub fn push(&mut self, space: Space, vertices: &[Vertex]) {
        let whole = vertices.len() - vertices.len() % 3;
        if whole > 0 {
            self.batches.push((space, vertices[..whole].to_vec()));
        }
    }

    /// Whether it holds no triangle.
    pub fn is_empty(&self) -> bool {
        self.batches.is_empty()
    }
}

/// The frame a [`TrianglesPass`]'s source is told about, as the ABI's [`TorquePassFrame`].
pub fn pass_frame(cx: &PassCx<'_>) -> TorquePassFrame {
    TorquePassFrame {
        stage: stage_code(cx.stage).unwrap_or(u32::MAX),
        size: cx.size,
        viewport: cx.viewport,
        view_proj: cx.camera.view_proj,
        eye: cx.camera.eye,
        time: cx.scene.time,
    }
}

/// The ABI's code of a stage a pass may draw at.
fn stage_code(stage: Stage) -> Option<u32> {
    match stage {
        Stage::Opaque => Some(TORQUE_STAGE_OPAQUE),
        Stage::Translucent => Some(TORQUE_STAGE_TRANSLUCENT),
        Stage::Overlay => Some(TORQUE_STAGE_OVERLAY),
        _ => None,
    }
}

/// The stage of an ABI code.
fn stage_of(code: u32) -> Option<Stage> {
    match code {
        TORQUE_STAGE_OPAQUE => Some(Stage::Opaque),
        TORQUE_STAGE_TRANSLUCENT => Some(Stage::Translucent),
        TORQUE_STAGE_OVERLAY => Some(Stage::Overlay),
        _ => None,
    }
}

const SHADER: &str = r#"
struct Out { @builtin(position) pos: vec4<f32>, @location(0) color: vec4<f32> };
@vertex fn vs(@location(0) p: vec4<f32>, @location(1) c: vec4<f32>) -> Out {
    var o: Out;
    o.pos = p;
    o.color = c;
    return o;
}
@fragment fn fs(i: Out) -> @location(0) vec4<f32> { return i.color; }
"#;

/// The source of a [`TrianglesPass`]'s triangles, each frame.
type Source = Box<dyn FnMut(&TorquePassFrame, &mut TriangleList) + Send>;

/// A pass that draws flat colored triangles, alpha blended: at [`Stage::Opaque`] and [`Stage::Translucent`] into the HDR
/// frame (linear light), depth tested and written; at [`Stage::Overlay`] over the finished picture, with no depth. At
/// any other stage it draws nothing.
pub struct TrianglesPass {
    source: Source,
    /// The pipeline and the format and depth it was made for.
    made: Option<(wgpu::TextureFormat, bool, wgpu::RenderPipeline)>,
}

impl TrianglesPass {
    /// A pass whose triangles `source` gives each frame.
    pub fn new(
        source: impl FnMut(&TorquePassFrame, &mut TriangleList) + Send + 'static,
    ) -> TrianglesPass {
        TrianglesPass {
            source: Box::new(source),
            made: None,
        }
    }

    fn pipeline(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        depth: bool,
    ) -> wgpu::RenderPipeline {
        if let Some((f, d, pipeline)) = &self.made
            && *f == format
            && *d == depth
        {
            return pipeline.clone();
        }
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("extension triangles"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("extension triangles"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 32,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: depth.then(|| wgpu::DepthStencilState {
                format: torque_render_hd::passes::DEPTH_FORMAT,
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
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.made = Some((format, depth, pipeline.clone()));
        pipeline
    }
}

impl HdPass for TrianglesPass {
    fn draw(&mut self, cx: &mut PassCx<'_>) {
        use wgpu::util::DeviceExt;
        if stage_code(cx.stage).is_none() {
            return;
        }
        let frame = pass_frame(cx);
        let mut list = TriangleList::default();
        (self.source)(&frame, &mut list);
        if list.is_empty() {
            return;
        }
        let m = frame.view_proj;
        let mut data: Vec<f32> = Vec::new();
        for (space, vertices) in &list.batches {
            for v in vertices {
                let p = v.position;
                let clip = match space {
                    // Column major: clip = M * (p, 1).
                    Space::World => [0, 1, 2, 3]
                        .map(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r]),
                    Space::Clip => [p[0], p[1], p[2], 1.0],
                };
                data.extend(clip);
                data.extend(v.color);
            }
        }
        let world = cx.stage != Stage::Overlay;
        let (target, format, depth) = if world {
            let Some(hdr) = cx.hdr else {
                return;
            };
            let depth = cx.depth.filter(|_| cx.depth_attachable);
            (hdr, torque_render_hd::passes::HDR_FORMAT, depth)
        } else {
            let Some((ldr, format)) = cx.ldr else {
                return;
            };
            (ldr, format, None)
        };
        let pipeline = self.pipeline(cx.device, format, depth.is_some());
        let buffer = cx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("extension triangles"),
                contents: &data
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<u8>>(),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let mut pass = cx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("extension triangles"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: depth.map(|view| wgpu::RenderPassDepthStencilAttachment {
                view,
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
        pass.set_vertex_buffer(0, buffer.slice(..));
        pass.draw(0..(data.len() / 8) as u32, 0..1);
    }
}

/// An extension's pass instance for one view: made on its first frame, freed with the view.
struct Instance {
    state: Arc<ExtensionState>,
    index: usize,
    made: Option<ffi::PassInstance>,
    /// It failed to make an instance: it draws nothing.
    failed: bool,
}

impl Instance {
    fn frame(&mut self, frame: &TorquePassFrame, out: &mut TriangleList) {
        if self.failed || self.state.is_off() {
            return;
        }
        let spec = &self.state.passes[self.index];
        let drawn = {
            let Some(_one_at_a_time) = self.state.enter() else {
                return;
            };
            // Disabled by another thread's call while this one waited for the lock: not called again.
            if self.state.is_off() {
                return;
            }
            if self.made.is_none() {
                let created = ffi::create_pass(spec);
                for line in created.log {
                    self.state.note(line);
                }
                match (created.status, created.made) {
                    (TORQUE_OK, Some(made)) => self.made = Some(made),
                    (TORQUE_ERR_PANIC, _) => {
                        drop(_one_at_a_time);
                        self.state
                            .disable(format!("it panicked making its pass {}", self.index));
                        return;
                    }
                    (status, _) => {
                        self.failed = true;
                        drop(_one_at_a_time);
                        self.state.note(format!(
                            "error: its pass {} made no instance (status {status}): it draws nothing",
                            self.index
                        ));
                        return;
                    }
                }
            }
            let Some(made) = &self.made else {
                return;
            };
            ffi::draw_pass(spec, made, frame)
        };
        for line in drawn.log {
            self.state.note(line);
        }
        match drawn.status {
            TORQUE_OK => {}
            TORQUE_ERR_PANIC => {
                self.state
                    .disable(format!("it panicked in its pass {}", self.index));
                return;
            }
            other => {
                self.state.note(format!(
                    "error: its pass {} failed with status {other}",
                    self.index
                ));
                return;
            }
        }
        for (space, vertices) in drawn.batches {
            let space = if space == TORQUE_SPACE_CLIP {
                Space::Clip
            } else {
                debug_assert_eq!(space, TORQUE_SPACE_WORLD);
                Space::World
            };
            out.push(space, &vertices);
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // A disabled extension is not called again, even to free: its instance is left as it is.
        if let Some(made) = self.made.take()
            && !self.state.is_off()
            && let Some(_one_at_a_time) = self.state.enter()
        {
            ffi::drop_pass(&self.state.passes[self.index], made);
        }
    }
}

/// Adds the extension's passes to the engine's render registry.
pub(crate) fn add_passes(engine: &mut EngineBuilder, state: &Arc<ExtensionState>) {
    for (index, spec) in state.passes.iter().enumerate() {
        let Some(stage) = stage_of(spec.stage) else {
            continue;
        };
        let state = Arc::clone(state);
        engine.render().pass(stage, move || {
            let mut instance = Instance {
                state: Arc::clone(&state),
                index,
                made: None,
                failed: false,
            };
            TrianglesPass::new(move |frame, out| instance.frame(frame, out))
        });
    }
}
