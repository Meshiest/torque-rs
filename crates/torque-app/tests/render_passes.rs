//! Rendering from plugins, drawn offscreen over a small scene: a pass at the opaque stage and one at the overlay stage, a static
//! batch of many cubes and an emitter kind of its own, each added by a plugin through `engine.render()` and checked by the
//! pixels it changed from the same scene drawn without it. `TORQUE_RENDER_PASS_PICTURES=<folder>` also writes the pictures there
//! as PNGs.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use torque_app::render::{
    EmitterDraws, EmitterFrame, EmitterKind, EngineRender, HdPass, PassCx, Sprite, Stage,
    StaticBatch, StaticBatchSource,
};
use torque_app::renderers::{RenderFeatures, RendererEntry, RendererPlugin, RendererSlot};
use torque_app::shots::write_png;
use torque_app::view::{Daylight, PLUGIN_BATCHES, WorldView};
use torque_engine::{Engine, EngineBuilder, Plugin};
use torque_render_core::{Frame, Gpu, wgpu};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::math::Mat;
use torque_render_scene::source::Eye;
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Assets, Blend, Cull, Draw, TexEnv, Vertex};

const SIZE: (u32, u32) = (960, 540);
/// Frames each picture settles for (the anti-aliasing's history).
const FRAMES: u32 = 10;

fn gpu() -> Option<Gpu> {
    match torque_app::window::offscreen_gpu() {
        Ok(gpu) => Some(gpu),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn save(frame: &Frame, name: &str) {
    if let Some(dir) = std::env::var_os("TORQUE_RENDER_PASS_PICTURES").map(PathBuf::from) {
        std::fs::create_dir_all(&dir).unwrap();
        write_png(frame, &dir.join(format!("{name}.png"))).unwrap();
    }
}

/// A view of a plain floor under the default day, following a slot over `engine`'s renderers and what its plugins added.
fn view(gpu: &Gpu, engine: Option<&Engine>) -> WorldView {
    let mut view = WorldView::new(gpu);
    view.set_plain_floor(120.0, [128, 150, 110]);
    view.set_daylight(&Daylight::default());
    view.set_effect_frame_seconds(Some(1.0 / 60.0));
    if let Some(engine) = engine {
        let features = engine
            .resource::<RenderFeatures>()
            .as_deref()
            .copied()
            .unwrap_or(RenderFeatures::ALL);
        let (slot, warnings) = RendererSlot::new(engine.list::<RendererEntry>(), features);
        assert!(warnings.is_empty(), "{warnings:?}");
        let adds = torque_app::render::RenderAdds::of_engine(engine).kept(|w| panic!("{w}"));
        view.attach_slot(Rc::new(slot.with_adds(adds)))
            .expect("the engine's renderer is made");
    }
    view
}

fn engine<P: torque_engine::PluginType>(plugin: P) -> Engine {
    EngineBuilder::new()
        .add_plugins((
            RendererPlugin,
            torque_app::renderers::RenderFeaturePlugins,
            plugin,
        ))
        .build()
        .expect("the engine is built")
}

fn eye() -> Eye {
    look_at([0.0, -16.0, 4.5], [0.0, 0.0, 2.0])
}

fn shot(view: &mut WorldView, gpu: &Gpu, eye: Eye) -> Frame {
    view.reset_history();
    let mut frame = None;
    for _ in 0..FRAMES {
        frame = Some(
            view.render_world(gpu, SIZE, eye)
                .expect("the world is drawn"),
        );
    }
    frame.unwrap()
}

/// The pixels of `rect` (left, top, width, height) that differ clearly between two pictures.
fn changed(a: &Frame, b: &Frame, rect: [u32; 4]) -> usize {
    let [x0, y0, w, h] = rect;
    let mut n = 0;
    for y in y0..(y0 + h).min(a.height) {
        for x in x0..(x0 + w).min(a.width) {
            let i = 4 * (y * a.width + x) as usize;
            let d = (0..3)
                .map(|c| (i32::from(a.rgba[i + c]) - i32::from(b.rgba[i + c])).abs())
                .max()
                .unwrap();
            if d > 40 {
                n += 1;
            }
        }
    }
    n
}

/// Where the world point `p` is in the picture seen from `eye`.
fn project(eye: Eye, p: [f32; 3]) -> (f32, f32) {
    let fov = f32::from(eye.fov.unwrap_or(90));
    let camera = torque_render_scene::hd::HdCamera::new(eye, fov, 1000.0, SIZE.0, SIZE.1);
    let m = &camera.view_proj;
    let row = |i: usize| m[i] * p[0] + m[4 + i] * p[1] + m[8 + i] * p[2] + m[12 + i];
    let w = row(3);
    assert!(w > camera.near, "the point is in front of the eye");
    let (nx, ny) = (row(0) / w, row(1) / w);
    (
        (nx * 0.5 + 0.5) * SIZE.0 as f32,
        (0.5 - ny * 0.5) * SIZE.1 as f32,
    )
}

fn pixel(f: &Frame, x: u32, y: u32) -> [u8; 4] {
    let i = 4 * (y * f.width + x) as usize;
    [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2], f.rgba[i + 3]]
}

const WHOLE: [u32; 4] = [0, 0, SIZE.0, SIZE.1];

/// A pass drawing flat quads of one color: at the opaque stage in the world (into the HDR frame, depth tested and written),
/// at the overlay stage in the picture (clip-space corners, within the viewport).
struct Flat {
    /// The quads' corners: world points (opaque) or clip-space points (overlay), four a quad.
    corners: Vec<[f32; 3]>,
    color: [f32; 4],
    /// The pipeline and the format it was made for.
    made: Option<(wgpu::TextureFormat, bool, wgpu::RenderPipeline)>,
    ran: Arc<AtomicU32>,
}

const FLAT: &str = r#"
struct Out { @builtin(position) pos: vec4<f32> };
@group(0) @binding(0) var<uniform> color: vec4<f32>;
@vertex fn vs(@location(0) p: vec4<f32>) -> Out { var o: Out; o.pos = p; return o; }
@fragment fn fs() -> @location(0) vec4<f32> { return color; }
"#;

impl Flat {
    fn new(corners: Vec<[f32; 3]>, color: [f32; 4], ran: &Arc<AtomicU32>) -> Flat {
        Flat {
            corners,
            color,
            made: None,
            ran: Arc::clone(ran),
        }
    }

    fn pipeline(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        depth: bool,
    ) -> &wgpu::RenderPipeline {
        if self
            .made
            .as_ref()
            .is_none_or(|m| m.0 != format || m.1 != depth)
        {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("flat"),
                source: wgpu::ShaderSource::Wgsl(FLAT.into()),
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("flat"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: 16,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x4],
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
                    targets: &[Some(format.into())],
                }),
                multiview_mask: None,
                cache: None,
            });
            self.made = Some((format, depth, pipeline));
        }
        &self.made.as_ref().unwrap().2
    }
}

impl HdPass for Flat {
    fn draw(&mut self, cx: &mut PassCx<'_>) {
        use wgpu::util::DeviceExt;
        self.ran.fetch_add(1, Ordering::Relaxed);
        let world = cx.stage != Stage::Overlay;
        let m = cx.camera.view_proj;
        let clip = |p: [f32; 3]| -> [f32; 4] {
            if !world {
                return [p[0], p[1], p[2], 1.0];
            }
            // Column major: clip = M * (p, 1).
            let mut c = [0.0; 4];
            for (r, out) in c.iter_mut().enumerate() {
                *out = m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r];
            }
            c
        };
        let mut vertices: Vec<f32> = Vec::new();
        for q in self.corners.as_chunks::<4>().0 {
            for i in [0, 1, 2, 0, 2, 3] {
                vertices.extend(clip(q[i]));
            }
        }
        let device = cx.device;
        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("flat vertices"),
            contents: &vertices
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<u8>>(),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ub = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("flat color"),
            contents: &self
                .color
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<u8>>(),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let (target, format, depth) = if world {
            (
                cx.hdr.expect("the stage has the HDR frame"),
                torque_render_hd::HDR_FORMAT,
                cx.depth,
            )
        } else {
            let (view, format) = cx.ldr.expect("the overlay has the picture");
            (view, format, None)
        };
        let pipeline = self.pipeline(device, format, depth.is_some()).clone();
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flat"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: ub.as_entire_binding(),
            }],
        });
        let mut pass = cx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("flat"),
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
        pass.set_bind_group(0, &group, &[]);
        pass.set_vertex_buffer(0, vb.slice(..));
        pass.draw(0..(vertices.len() / 4) as u32, 0..1);
    }
}

/// A plugin adding one pass of corners and color at a stage.
struct Passes {
    stage: Stage,
    corners: Vec<[f32; 3]>,
    color: [f32; 4],
    ran: Arc<AtomicU32>,
}

impl torque_engine::PluginType for Passes {
    const ID: torque_engine::PluginId = torque_engine::PluginId::new("test.render-passes");
}

impl Plugin for Passes {
    fn build(&self, engine: &mut EngineBuilder) {
        let (corners, color, ran) = (self.corners.clone(), self.color, Arc::clone(&self.ran));
        engine
            .render()
            .pass(self.stage, move || Flat::new(corners.clone(), color, &ran));
    }
}

#[test]
fn a_pass_at_the_opaque_stage_draws_into_the_world() {
    let Some(gpu) = gpu() else { return };
    let plain = shot(&mut view(&gpu, None), &gpu, eye());
    // Two upright magenta squares standing on the floor, two units across.
    let square = |y: f32, x: f32| {
        vec![
            [x - 1.0, y, 0.5],
            [x + 1.0, y, 0.5],
            [x + 1.0, y, 2.5],
            [x - 1.0, y, 2.5],
        ]
    };
    let mut corners = square(0.0, 0.0);
    corners.extend(square(-2.0, -5.0));
    let ran = Arc::new(AtomicU32::new(0));
    let engine = engine(Passes {
        stage: Stage::Opaque,
        corners,
        color: [4.0, 0.1, 3.0, 1.0],
        ran: Arc::clone(&ran),
    });
    let mut v = view(&gpu, Some(&engine));
    let drawn = shot(&mut v, &gpu, eye());
    save(&drawn, "opaque_pass");
    assert_eq!(
        ran.load(Ordering::Relaxed),
        FRAMES,
        "the pass ran once a frame"
    );
    let (cx, cy) = project(eye(), [0.0, 0.0, 1.5]);
    let p = pixel(&drawn, cx as u32, cy as u32);
    assert!(
        p[0] > 200 && p[1] < 120 && p[2] > 150,
        "the square's middle is magenta: {p:?}"
    );
    let n = changed(&plain, &drawn, WHOLE);
    assert!(n > 2_000, "{n} pixels changed");
}

#[test]
fn a_pass_at_the_overlay_stage_draws_over_the_finished_picture() {
    let Some(gpu) = gpu() else { return };
    let plain = shot(&mut view(&gpu, None), &gpu, eye());
    let ran = Arc::new(AtomicU32::new(0));
    // The lower left quarter of the picture's lower left quarter, in clip space.
    let corners = vec![
        [-1.0, -1.0, 0.5],
        [-0.5, -1.0, 0.5],
        [-0.5, -0.5, 0.5],
        [-1.0, -0.5, 0.5],
    ];
    let engine = engine(Passes {
        stage: Stage::Overlay,
        corners,
        color: [0.0, 1.0, 0.0, 1.0],
        ran: Arc::clone(&ran),
    });
    let mut v = view(&gpu, Some(&engine));
    let drawn = shot(&mut v, &gpu, eye());
    save(&drawn, "overlay_pass");
    assert_eq!(ran.load(Ordering::Relaxed), FRAMES);
    // Exactly the pass's green in its rectangle (nothing tonemaps it), the rest of the picture untouched.
    assert_eq!(pixel(&drawn, 60, SIZE.1 - 40), [0, 255, 0, 255]);
    let (w, h) = (SIZE.0 / 4, SIZE.1 / 4);
    let inside = changed(&plain, &drawn, [0, SIZE.1 - h, w, h]);
    assert!(
        inside > (w * h) as usize * 9 / 10,
        "{inside} of {} pixels green",
        w * h
    );
    let outside = changed(&plain, &drawn, [w + 2, 0, SIZE.0 - w - 2, SIZE.1]);
    assert_eq!(outside, 0, "nothing else changed");
}

/// A cube of side 1 standing on the origin, its faces clockwise from outside.
fn cube() -> Draw {
    let mut vertices = Vec::new();
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    for (n, u, w) in faces {
        let corner = |a: f32, b: f32| {
            let p = [0, 1, 2].map(|k| {
                0.5 * n[k] + a * 0.5 * u[k] + b * 0.5 * w[k] + if k == 2 { 0.5 } else { 0.0 }
            });
            Vertex {
                position: p,
                normal: n,
                color: [200, 90, 60, 255],
                uv: [0.0; 2],
            }
        };
        // u x w = n: counter-clockwise from outside in this order; reversed for a clockwise front.
        let q = [
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ];
        vertices.extend([q[0], q[2], q[1], q[0], q[3], q[2]]);
    }
    Draw {
        label: "cube".to_string(),
        vertices,
        lighting: true,
        tex_env: TexEnv::None,
        texture: None,
        blend: Blend::Opaque,
        cull: Cull::BackCw,
        depth_test: true,
        depth_write: true,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    }
}

/// A grid of `side` by `side` cubes, `step` apart, centered on the origin.
struct Cubes {
    side: usize,
    step: f32,
    generation: Rc<Cell<u64>>,
}

impl StaticBatchSource for Cubes {
    fn generation(&self) -> u64 {
        self.generation.get()
    }

    fn batch(&mut self) -> StaticBatch {
        let half = (self.side as f32 - 1.0) * 0.5 * self.step;
        let transforms = (0..self.side * self.side)
            .map(|i| {
                let (x, y) = ((i % self.side) as f32, (i / self.side) as f32);
                let mut m = Mat::from_axis_angle([0.0, 0.0, 1.0], 0.3 * i as f32);
                m.set_column3([x * self.step - half, y * self.step - half + 30.0, 0.0]);
                m
            })
            .collect();
        StaticBatch {
            draws: vec![cube()],
            textures: Vec::new(),
            transforms,
        }
    }
}

struct Batches(Rc<Cell<u64>>);

impl torque_engine::PluginType for Batches {
    const ID: torque_engine::PluginId = torque_engine::PluginId::new("test.static-batches");
}

impl Plugin for Batches {
    fn build(&self, engine: &mut EngineBuilder) {
        let generation = Rc::clone(&self.0);
        engine.render().static_batch(move || Cubes {
            side: 48,
            step: 3.0,
            generation: Rc::clone(&generation),
        });
    }
}

#[test]
fn a_static_batch_of_many_cubes_is_laid_out_in_chunks_and_drawn() {
    let Some(gpu) = gpu() else { return };
    let high = look_at([0.0, -40.0, 30.0], [0.0, 30.0, 0.0]);
    let plain = shot(&mut view(&gpu, None), &gpu, high);
    let generation = Rc::new(Cell::new(0));
    let engine = engine(Batches(Rc::clone(&generation)));
    let mut v = view(&gpu, Some(&engine));
    let drawn = shot(&mut v, &gpu, high);
    save(&drawn, "static_batch");
    // 2,304 cubes over 141 units: a 3 x 3 square of chunks, more where a row of cubes crosses a chunk's edge.
    let cells = v.static_batch_cells(PLUGIN_BATCHES);
    assert!((4..=16).contains(&cells), "{cells} chunks");
    let n = changed(&plain, &drawn, WHOLE);
    assert!(n > 40_000, "{n} pixels changed");
    // A new generation lays the batch out again (the same cubes: the same picture).
    generation.set(1);
    let again = shot(&mut v, &gpu, high);
    assert_eq!(v.static_batch_cells(PLUGIN_BATCHES), cells);
    assert!(changed(&drawn, &again, WHOLE) < 200);
}

/// Sparks going round a ring, faster with each frame's step: an emitter kind of its own.
struct Ring {
    angle: f32,
    center: [f32; 3],
}

impl EmitterKind for Ring {
    fn advance(&mut self, seconds: f32) {
        self.angle += seconds * 2.0;
    }

    fn draw(&mut self, _frame: &EmitterFrame, out: &mut EmitterDraws<'_, '_>) {
        let sprites: Vec<Sprite> = (0..24)
            .map(|i| {
                let a = self.angle + i as f32 * std::f32::consts::TAU / 24.0;
                Sprite {
                    position: [
                        self.center[0] + 3.0 * a.cos(),
                        self.center[1],
                        self.center[2] + 3.0 * a.sin(),
                    ],
                    size: 0.8,
                    spin: a,
                    color: [255, 140, 30, 255],
                }
            })
            .collect();
        out.sprites(&sprites, None, Blend::Additive);
    }
}

struct Kinds;

impl torque_engine::PluginType for Kinds {
    const ID: torque_engine::PluginId = torque_engine::PluginId::new("test.emitter-kinds");
}

impl Plugin for Kinds {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.render().emitter_kind("ring", || Ring {
            angle: 0.0,
            center: [0.0, 0.0, 4.0],
        });
    }
}

struct NoFiles;

impl Assets for NoFiles {
    fn read(&self, _path: &str) -> Option<Vec<u8>> {
        None
    }
}

/// The view's emitter kinds' draws for the frame from `eye`, handed to it as the frame's particles (as the ghosts' effects do).
fn particles(view: &mut WorldView, eye: Eye) {
    let camera = Mat::mul(&Mat::euler_z(eye.yaw), &Mat::euler_x(eye.pitch));
    let frame = EmitterFrame {
        eye: eye.position,
        right: camera.column(0),
        up: camera.column(2),
    };
    let assets = NoFiles;
    let mut set = TextureSet::new(&assets);
    let mut draws = Vec::new();
    view.emitter_draws(&frame, &mut set, &mut draws);
    view.set_particles(&draws, set);
}

#[test]
fn an_emitter_kind_draws_its_particles_beside_the_engine_s() {
    let Some(gpu) = gpu() else { return };
    let plain = shot(&mut view(&gpu, None), &gpu, eye());
    let engine = engine(Kinds);
    let mut v = view(&gpu, Some(&engine));
    assert_eq!(v.emitter_kinds().collect::<Vec<_>>(), ["ring"]);
    v.reset_history();
    let mut drawn = None;
    for _ in 0..FRAMES {
        particles(&mut v, eye());
        drawn = Some(v.render_world(&gpu, SIZE, eye()).unwrap());
    }
    let drawn = drawn.unwrap();
    save(&drawn, "emitter_kind");
    let n = changed(&plain, &drawn, WHOLE);
    assert!(n > 2_000, "{n} pixels changed");
    // The ring's center stays clear: it is a ring.
    let (cx, cy) = project(eye(), [0.0, 0.0, 4.0]);
    let c = changed(&plain, &drawn, [cx as u32 - 10, cy as u32 - 10, 20, 20]);
    assert_eq!(c, 0, "the middle of the ring");
}

#[test]
fn static_batches_past_the_view_s_budget_are_left_out_with_a_warning() {
    let Some(gpu) = gpu() else { return };
    let mut v = view(&gpu, None);
    v.set_static_batch_budget(12 * 150);
    let cubes = |n: usize| StaticBatch {
        draws: vec![cube()],
        textures: Vec::new(),
        transforms: (0..n)
            .map(|i| {
                let mut m = Mat::IDENTITY;
                m.set_column3([i as f32 * 2.0, 0.0, 0.0]);
                m
            })
            .collect(),
    };
    assert_eq!(v.set_static_batch(1, &cubes(100)), None);
    assert_eq!(v.static_batch_triangles(), 1_200);
    // The second has room for 50 of its 100.
    let warning = v
        .set_static_batch(2, &cubes(100))
        .expect("copies were left out");
    assert!(warning.contains("50 of its 100"), "{warning}");
    assert_eq!(v.static_batch_triangles(), 1_800);
    // A full view lays out none of a third; laid out again, the first takes its own room back.
    assert!(v.set_static_batch(3, &cubes(10)).is_some());
    assert_eq!(v.static_batch_cells(3), 0);
    assert_eq!(v.set_static_batch(1, &cubes(100)), None);
    assert_eq!(v.take_warnings().len(), 2);
    assert!(v.take_warnings().is_empty());
}
