//! A game's render extension on a world view (`WorldView<E>`), drawn offscreen: a test extension with a 16-byte frame block and
//! a forward chunk that tints the planes when the block says so. A process composes the renderer's modules with one extension,
//! so every test here draws with the test extension; the engine's own picture to compare with comes from a run of this binary
//! with `TORQUE_RENDER_EXT_PLAIN` set (the one test then draws a plain view and writes its pixels there).

use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::sync::OnceLock;

use torque_app::render::{EngineRender, RenderAdds, StaticBatch, StaticBatchSource};
use torque_app::renderers::{
    DEFAULT_RENDERER, EngineRenderers, RenderFeatures, RenderFrame, RendererEntry, RendererPlugin,
    RendererSlot, SwitchError, WorldRenderer,
};
use torque_app::view::{Daylight, WorldView};
use torque_engine::{Engine, EngineBuilder, Plugin, PluginId, PluginType};
use torque_render_core::Gpu;
use torque_render_hd::ext::{ExtCell, HdExtension, PlainScene, ViewScene};
use torque_render_hd::shaders::{NONE, ShaderExtension};
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::{HdMode, HdScene};
use torque_render_scene::math::Mat;
use torque_render_scene::source::Eye;
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Vertex};

/// Where the plain run writes its picture (set: this run draws the plain view only).
const PLAIN_VAR: &str = "TORQUE_RENDER_EXT_PLAIN";
const SIZE: (u32, u32) = (160, 120);
/// Frames each picture settles for (the anti-aliasing's history).
const FRAMES: u32 = 6;
const FLOOR: [u8; 3] = [120, 160, 90];

fn gpu() -> Option<Gpu> {
    match torque_app::window::offscreen_gpu() {
        Ok(gpu) => Some(gpu),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

/// The test extension: plain cells, the planes tinted by the scene's tint.
struct Tint;

/// The test extension's scene: the engine's plain scene and the tint (None: off).
#[derive(Default)]
struct TintScene {
    plain: PlainScene,
    tint: Option<[f32; 3]>,
}

impl Deref for TintScene {
    type Target = HdScene<StaticCells>;

    fn deref(&self) -> &HdScene<StaticCells> {
        &self.plain.frame
    }
}

impl DerefMut for TintScene {
    fn deref_mut(&mut self) -> &mut HdScene<StaticCells> {
        &mut self.plain.frame
    }
}

/// Its geometry takes none of the view's static meshes and batches.
impl ViewScene for TintScene {
    fn set_mode(&mut self, mode: Option<HdMode>) {
        self.plain.mode = mode;
    }

    fn set_time(&mut self, seconds: f32) {
        self.plain.time = seconds;
    }
}

/// The engine's chunks with the frame block holding the tint and `ext_paint` tinting a plane's paint while the tint is on.
fn tint_shaders() -> ShaderExtension {
    static CHUNKS: OnceLock<ShaderExtension> = OnceLock::new();
    *CHUNKS.get_or_init(|| {
        let common = NONE.common.replace("unused: vec4<f32>,", "tint: vec4<f32>,");
        assert_ne!(common, NONE.common, "the frame block is renamed");
        let start = NONE.forward.find("fn ext_paint(").expect("ext_paint");
        let end = start + NONE.forward[start..].find('}').expect("its end") + 1;
        let forward = format!(
            "{}fn ext_paint(in: VertexOut, paint_in: vec4<f32>, ng: vec3<f32>, v: vec3<f32>) -> vec4<f32> {{
    let plane = ((in.material >> MAT_KIND_SHIFT) & 3u) == KIND_PLANE;
    return select(paint_in, vec4<f32>(paint_in.rgb * frame.ext.tint.rgb, paint_in.a), plane && frame.ext.tint.w > 0.5);
}}{}",
            &NONE.forward[..start],
            &NONE.forward[end..]
        );
        ShaderExtension {
            common: common.leak(),
            forward: forward.leak(),
            shadow: NONE.shadow,
        }
    })
}

impl HdExtension for Tint {
    type Geometry = StaticCells;
    type Scene = TintScene;
    type CellRuns = ();
    type LevelInput = ();
    const FRAME_EXT_BYTES: usize = 16;

    fn new(_device: &torque_render_core::wgpu::Device) -> Tint {
        Tint
    }

    fn shaders() -> ShaderExtension {
        tint_shaders()
    }

    fn mode(scene: &TintScene) -> Option<HdMode> {
        scene.plain.mode
    }

    fn time(scene: &TintScene) -> f32 {
        scene.plain.time
    }

    fn level_input(_cell: &ExtCell<Tint>) {}

    fn frame_ext(&self, scene: &TintScene) -> Vec<u8> {
        let block = scene.tint.map_or([0.0; 4], |[r, g, b]| [r, g, b, 1.0]);
        block.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
}

/// A plain floor under the default daylight, as both views draw it.
fn floor<E: HdExtension<Scene: ViewScene>>(view: &mut WorldView<E>) {
    view.set_plain_floor(50.0, FLOOR);
    view.set_daylight(&Daylight::default());
    view.renderer().set_unbudgeted();
}

/// The world's pixels after the history settled.
fn picture<E: HdExtension<Scene: ViewScene>>(view: &mut WorldView<E>, gpu: &Gpu) -> Vec<u8> {
    let eye = Eye {
        position: [0.0, -6.0, 4.0],
        pitch: 0.5,
        yaw: 0.0,
        fov: Some(75),
    };
    view.reset_history();
    let mut frame = None;
    for _ in 0..FRAMES {
        frame = Some(
            view.render_world(gpu, SIZE, eye)
                .expect("the world is drawn"),
        );
    }
    frame.expect("a frame").rgb()
}

/// The engine's own view's picture, drawn by a run of this binary with [`PLAIN_VAR`] set.
fn plain_picture() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("render_ext_plain.rgb");
    let _ = std::fs::remove_file(&path);
    let status = Command::new(std::env::current_exe().expect("the test binary"))
        .args([
            "a_tint_extension_differs_when_on_and_matches_the_plain_view_when_off",
            "--exact",
            "--test-threads=1",
        ])
        .env(PLAIN_VAR, &path)
        .status()
        .expect("the plain run starts");
    assert!(status.success(), "the plain run passes");
    std::fs::read(&path).expect("the plain run wrote its picture")
}

#[test]
fn a_tint_extension_differs_when_on_and_matches_the_plain_view_when_off() {
    let Some(gpu) = gpu() else {
        return;
    };
    if let Some(path) = std::env::var_os(PLAIN_VAR) {
        let mut view = WorldView::new(&gpu);
        floor(&mut view);
        std::fs::write(path, picture(&mut view, &gpu)).expect("the picture is written");
        return;
    }
    let plain = plain_picture();
    let mut view = WorldView::<Tint>::with_extension(&gpu);
    floor(&mut view);
    let off = picture(&mut view, &gpu);
    assert_eq!(off.len(), plain.len());
    assert!(
        off == plain,
        "with the tint off the extension draws as the engine's own"
    );

    view.scene_mut().tint = Some([1.0, 0.25, 0.25]);
    let on = picture(&mut view, &gpu);
    let redder = on
        .chunks(3)
        .zip(off.chunks(3))
        .filter(|(a, b)| {
            i32::from(a[1]) + 20 < i32::from(b[1]) && i32::from(a[0]) + 10 >= i32::from(b[0])
        })
        .count();
    assert!(
        redder > on.len() / 3 / 4,
        "the tinted floor fills a quarter of the picture or more ({redder} pixels)"
    );

    view.scene_mut().tint = None;
    assert!(
        picture(&mut view, &gpu) == plain,
        "the tint taken off again"
    );
}

/// A renderer that draws nothing (it is never made here).
struct Other;

impl WorldRenderer for Other {
    fn draw(&mut self, _frame: RenderFrame<'_>) -> Result<(), String> {
        Ok(())
    }
}

/// Registers [`Other`] as `test.other`.
struct OtherPlugin;

impl PluginType for OtherPlugin {
    const ID: PluginId = PluginId::new("test.other-renderer");
}

impl Plugin for OtherPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_renderer_id("test.other", |_| {
            let renderer: Box<dyn WorldRenderer> = Box::new(Other);
            Ok(renderer)
        });
    }
}

/// A static batch source of nothing.
struct Nothing;

impl StaticBatchSource for Nothing {
    fn generation(&self) -> u64 {
        0
    }

    fn batch(&mut self) -> StaticBatch {
        StaticBatch {
            draws: Vec::new(),
            textures: Vec::new(),
            transforms: Vec::new(),
        }
    }
}

/// Adds [`Nothing`] as a static batch source.
struct NothingPlugin;

impl PluginType for NothingPlugin {
    const ID: PluginId = PluginId::new("test.nothing-batch");
}

impl Plugin for NothingPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.render().static_batch(|| Nothing);
    }
}

fn engine() -> Engine {
    EngineBuilder::new()
        .add_plugins((
            RendererPlugin,
            torque_app::renderers::RenderFeaturePlugins,
            OtherPlugin,
            NothingPlugin,
        ))
        .build()
        .expect("the engine is built")
}

/// The slot a client of `engine` gets, with what its plugins added.
fn slot(engine: &Engine) -> Rc<RendererSlot> {
    let (slot, warnings) = RendererSlot::new(engine.list::<RendererEntry>(), RenderFeatures::ALL);
    assert!(warnings.is_empty(), "{warnings:?}");
    let adds = RenderAdds::of_engine(engine).kept(|w| panic!("{w}"));
    Rc::new(slot.with_adds(adds))
}

#[test]
fn another_renderer_on_a_game_extension_s_view_is_refused_with_the_reason() {
    let Some(gpu) = gpu() else {
        return;
    };
    let engine = engine();

    // Chosen before the view attached (a script at start-up): the engine's renderer draws instead, and the attach says why.
    let early = slot(&engine);
    early.switch("test.other").expect("no view is attached yet");
    let mut view = WorldView::<Tint>::with_extension(&gpu);
    let refused = view.attach_slot(Rc::clone(&early)).unwrap_err();
    assert!(refused.contains("game extension"), "{refused}");
    assert!(refused.contains("render_ext::Tint"), "{refused}");
    assert_eq!(view.current_renderer(), Some(DEFAULT_RENDERER));
    assert_eq!(early.current(), Some(DEFAULT_RENDERER));

    // A swap once attached is refused and the engine's renderer stays.
    let attached = slot(&engine);
    let mut view = WorldView::<Tint>::with_extension(&gpu);
    view.attach_slot(Rc::clone(&attached))
        .expect("the engine's renderer draws it");
    assert!(matches!(
        attached.switch("test.other"),
        Err(SwitchError::Extension { ref id, .. }) if id == "test.other"
    ));
    let refused = view.set_renderer("test.other").unwrap_err();
    assert!(refused.contains("only the engine's renderer"), "{refused}");
    assert_eq!(view.current_renderer(), Some(DEFAULT_RENDERER));
    view.set_renderer(DEFAULT_RENDERER)
        .expect("the engine's own is taken");
}

#[test]
fn a_scene_without_batch_cells_refuses_static_batches_with_a_warning_naming_them() {
    let Some(gpu) = gpu() else {
        return;
    };
    let engine = engine();
    let mut view = WorldView::<Tint>::with_extension(&gpu);
    view.attach_slot(slot(&engine))
        .expect("the engine's renderer draws it");
    let warnings = view.take_warnings();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("render_ext::Nothing") && w.contains("takes no static batches")),
        "{warnings:?}"
    );

    assert!(!view.add_static_batch_source(Nothing));
    let warnings = view.take_warnings();
    assert!(
        warnings.iter().any(|w| w.contains("render_ext::Nothing")),
        "{warnings:?}"
    );

    let corner = |x: f32, y: f32| Vertex {
        position: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        color: [255; 4],
        uv: [0.0; 2],
    };
    let triangle = Draw {
        label: "triangle".to_string(),
        vertices: vec![corner(0.0, 0.0), corner(1.0, 0.0), corner(0.0, 1.0)],
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
    };
    let batch = StaticBatch {
        draws: vec![triangle],
        textures: Vec::new(),
        transforms: vec![Mat::IDENTITY],
    };
    let warning = view.set_static_batch(1, &batch).expect("refused");
    assert!(warning.contains("takes no static batches"), "{warning}");
    assert_eq!(view.static_batch_cells(1), 0);
}
