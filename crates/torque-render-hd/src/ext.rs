//! What a world's own static geometry brings to the enhanced renderer beside the engine's frame ([`HdExtension`]): its scene
//! and its modes, the tables it keeps beside the cells' meshes (written as cells come and go), its blocks of the uniforms, the
//! buffers in the slots the renderer's groups keep for it ([`ExtSlots`]), its coarser levels of the cells, its extra draws in
//! the prepass, the opaque pass and the sun's cascades, and the moving objects it draws after the anti-aliasing. Every hook
//! but the scene's has a default that adds nothing; [`NoExtension`] is the engine's own (plain cells). A scene a world view fills
//! is a [`ViewScene`] too ([`PlainScene`] is the engine's).

use std::ops::{Deref, DerefMut};

use torque_render_core::wgpu;
use torque_render_scene::hd::cells::{Cell, StaticCells, StaticGeometry};
use torque_render_scene::hd::settings::ImageSettings;
use torque_render_scene::hd::{HdCamera, HdMode, HdScene, HdVertex, Quality, SURFACE_FLOATS};

use crate::shaders::ShaderExtension;
use crate::sky::EXT_FOG_SECTORS;
use crate::water::WaterZone;

/// No host (a light's unused exclusion).
pub const NO_HOST: u32 = u32::MAX;

/// A cell of an extension's static geometry.
pub type ExtCell<E> = Cell<<<E as HdExtension>::Geometry as StaticGeometry>::Ext>;

/// The storage buffers an extension's WGSL ([`crate::shaders::ShaderExtension`]) binds in the slots the renderer's groups keep
/// for it: the extension fills them (again whenever it makes a buffer anew) and every group made after reads them. Each starts
/// as an empty 16-byte buffer.
pub struct ExtSlots {
    /// The forward passes' frame group (0), bindings 14 and 15.
    pub forward: [wgpu::Buffer; 2],
    /// The reflections' group (3), binding 5.
    pub reflect: wgpu::Buffer,
    /// The shadow casters' group (3), bindings 0 and 1.
    pub shadow: [wgpu::Buffer; 2],
}

impl ExtSlots {
    /// Every slot an empty buffer.
    pub fn new(device: &wgpu::Device) -> ExtSlots {
        ExtSlots {
            forward: [0, 1].map(|_| ExtSlots::empty(device)),
            reflect: ExtSlots::empty(device),
            shadow: [0, 1].map(|_| ExtSlots::empty(device)),
        }
    }

    /// A slot's empty buffer (16 bytes of storage).
    pub fn empty(device: &wgpu::Device) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hd extension slot"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
}

/// The surface numbers of the frame uniform: the light's (F0, roughness, the sky SH's luminance and chroma weights on the
/// ambient), the surface block (`common.wgsl`'s `Frame.surf0` to `surf2`), the share of the highlights plain surfaces take (the
/// reflections' uniform), the translucency blended in gamma space, and film grain's amplitude.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surfaces {
    pub light: [f32; 4],
    pub table: [f32; SURFACE_FLOATS],
    pub specular: f32,
    pub gamma_translucency: bool,
    pub grain: f32,
}

impl Surfaces {
    /// The engine's surfaces under `image`'s switches.
    pub fn of(image: &ImageSettings) -> Surfaces {
        let b = |v: bool| if v { 1.0 } else { 0.0 };
        Surfaces {
            light: [0.04, 0.3, 0.7, 0.1],
            table: [
                // surf0: the default roughness, the clear coat's roughness, F0, linear light.
                0.35,
                0.08,
                0.04,
                b(image.linear_light),
                // surf1: shape and plastic roughness, rim, refraction.
                0.6,
                0.35,
                0.0,
                3.0,
                // surf2: plane roughness, stochastic ground, gamma translucency, the clear coat's share of the sky.
                0.7,
                0.0,
                b(!image.linear_blend),
                0.05,
            ],
            specular: 1.0,
            gamma_translucency: !image.linear_blend,
            grain: 0.0,
        }
    }
}

/// What the extension's extra draws are prepared with for the main view: the pipelines' layouts and modules (the forward
/// pass's, `clustered` or the brute-force loop, and the prepass's), the camera, the cells within the near detail's far distance
/// of the eye (in key order) and their boxes, and the near detail's distances ([`HdExtension::near_detail`]).
pub struct ExtraPrepare<'a> {
    pub device: &'a wgpu::Device,
    pub forward_layout: &'a wgpu::PipelineLayout,
    pub prepass_layout: &'a wgpu::PipelineLayout,
    pub forward: &'a wgpu::ShaderModule,
    pub prepass: &'a wgpu::ShaderModule,
    pub clustered: bool,
    pub camera: &'a HdCamera,
    pub near: Vec<[i32; 3]>,
    pub cells: &'a dyn Fn([i32; 3]) -> Option<[[f32; 3]; 2]>,
    pub reach: [f32; 2],
}

/// What the extra draws drew into a pass: the draw calls, the items drawn, and of them those of the ground.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExtraDrawn {
    pub calls: usize,
    pub drawn: usize,
    pub ground: usize,
}

/// The extension's extra casters in the sun's fine cascades, drawn a cell at a time with the pipeline the shadows made from
/// [`HdExtension::extra_casters`].
pub trait ExtraCasters {
    /// Binds the casters' shared buffers into `pass`; false when there is nothing to draw.
    fn bind(&self, pass: &mut wgpu::RenderPass<'_>) -> bool;
    /// Whether the cell `key` has casters.
    fn holds(&self, key: [i32; 3]) -> bool;
    /// Draws the cell `key`'s casters (after [`ExtraCasters::bind`]).
    fn cast(&self, pass: &mut wgpu::RenderPass<'_>, key: [i32; 3]);
}

/// The extra casters of a frame: the casters, the near detail's distance (the cascades' casters reach no farther from the eye),
/// the coarsest cascade texel that holds them, and the cells near enough to cast them.
pub struct ShadowExtras<'a> {
    pub casters: &'a dyn ExtraCasters,
    pub reach: f32,
    pub texel: f32,
    pub cells: Vec<[i32; 3]>,
}

/// What a world's static geometry brings to the enhanced renderer beside the engine's frame.
pub trait HdExtension: Sized + 'static {
    /// The static geometry's provider (its cells carry the extension's data).
    type Geometry: StaticGeometry<Ext: Send + 'static>;
    /// The scene the renderer draws: the engine's frame and whatever the extension keeps beside it.
    type Scene: Deref<Target = HdScene<Self::Geometry>>;
    /// A cell's runs in the extension's own tables ([`HdExtension::put_cell`]).
    type CellRuns: Copy + Default;
    /// What a coarser level is built from: a cell's share ([`HdExtension::level_input`]).
    type LevelInput: Send + 'static;
    /// The bytes of the frame uniform's extension block (the chunk's `FrameExt`).
    const FRAME_EXT_BYTES: usize;

    /// The extension's GPU state on `device`.
    fn new(device: &wgpu::Device) -> Self;

    /// The WGSL chunks the renderer's modules are composed with.
    fn shaders() -> ShaderExtension;

    /// How `scene` is drawn (None: another renderer draws it; the frame then takes [`HdExtension::fallback_mode`]).
    fn mode(scene: &Self::Scene) -> Option<HdMode>;

    /// The mode a scene without one is drawn in.
    fn fallback_mode() -> HdMode {
        HdMode::new(Quality::High)
    }

    /// The effects' time of `scene` (seconds).
    fn time(scene: &Self::Scene) -> f32;

    /// The renderer's switch of the extension's fine surface detail and extra draws (each frame, before anything is drawn).
    fn set_detail(&mut self, _on: bool) {}

    /// A cell going on the GPU: its runs in the extension's tables.
    fn put_cell(&mut self, _cell: &ExtCell<Self>) -> Self::CellRuns {
        Self::CellRuns::default()
    }

    /// A cell's runs given back.
    fn free_cell(&mut self, _runs: Self::CellRuns) {}

    /// A cell's vertex as the GPU holds it, where the extension rewrites it (its index into the tables at the cell's `runs`).
    fn vertex(&self, _runs: &Self::CellRuns, _v: &HdVertex) -> Option<HdVertex> {
        None
    }

    /// The cell `key` taken off the GPU (or about to be put on again).
    fn cell_out(&mut self, _key: [i32; 3]) {}

    /// A cell put on the GPU.
    fn cell_in(&mut self, _cell: &ExtCell<Self>) {}

    /// The tables' changes to the GPU (after the cells of an upload or of the levels went in).
    fn flush_cells(&mut self, _device: &wgpu::Device, _queue: &wgpu::Queue) {}

    /// After an upload's cells: what the extension keeps from them (`cells`: a cell's box on the GPU), with the boxes whose
    /// cached shadows are drawn again added to `dirty`.
    fn cells_done(
        &mut self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _scene: &Self::Scene,
        _cells: &dyn Fn([i32; 3]) -> Option<[[f32; 3]; 2]>,
        _dirty: &mut Vec<[[f32; 3]; 2]>,
    ) {
    }

    /// Whether the clusters of texture slot `slot` are drawn only near the eye ([`crate::arena::CLUSTER_NEAR`]).
    fn near_slot(_slot: u32) -> bool {
        false
    }

    /// Whether a cell vertex's material word reflects (the reflections run when a cell holding one is in sight).
    fn reflective(_material: u32) -> bool {
        false
    }

    /// The static geometry's owners (a vertex's `extra` word) a light at `position` is mounted in, left out of its own shadow
    /// cube (the rest [`NO_HOST`]).
    fn light_hosts(_scene: &Self::Scene, _position: [f32; 3]) -> [u32; 4] {
        [NO_HOST; 4]
    }

    /// A cell's share of the coarser levels' input.
    fn level_input(cell: &ExtCell<Self>) -> Self::LevelInput;

    /// The coarser `level`'s (1 or 2) mesh of `block` from its cells' inputs, where it pays (run on the levels' worker).
    fn build_level(
        _level: u32,
        _block: [i32; 3],
        _inputs: Vec<Self::LevelInput>,
    ) -> Option<ExtCell<Self>> {
        None
    }

    /// The distances past which levels 1 and 2 are drawn from `camera` at `quality`.
    fn level_distances(_camera: &HdCamera, _quality: Quality) -> [f32; 2] {
        [f32::INFINITY; 2]
    }

    /// Whether the moving object of `ghost` is drawn after the anti-aliasing (where the frame has a TAA).
    fn drawn_late(_ghost: u32) -> bool {
        false
    }

    /// Whether the moving object of `ghost` stays in a light capture's faces however near their eye it is.
    fn kept_near_capture(_ghost: u32) -> bool {
        false
    }

    /// Whether the light capture is drawn whatever is in sight.
    fn capture_always(_scene: &Self::Scene) -> bool {
        false
    }

    /// The light capture's faces the moving objects were kept for (a face not in it is drawn again in full the next frame).
    fn capture_cover(_scene: &Self::Scene) -> [bool; 6] {
        [true; 6]
    }

    /// The water zone the eye may be under: the scene's ([`torque_render_scene::hd::HdScene::water_zone`]).
    fn water_zone(scene: &Self::Scene) -> Option<WaterZone> {
        scene.water_zone
    }

    /// Whether the static geometry fades into the fog before the visible distance in some heading.
    fn fog_fades(_scene: &Self::Scene) -> bool {
        false
    }

    /// Where the static geometry is wholly fogged in each heading ([`crate::sky`]'s extension fog ends; 0: with the rest).
    fn fog_ends(_scene: &Self::Scene) -> [f32; EXT_FOG_SECTORS] {
        [0.0; EXT_FOG_SECTORS]
    }

    /// The surface numbers of `scene`'s frame.
    fn surfaces(&self, scene: &Self::Scene) -> Surfaces {
        Surfaces::of(&Self::mode(scene).unwrap_or_else(Self::fallback_mode).image)
    }

    /// The frame uniform's extension block ([`HdExtension::FRAME_EXT_BYTES`]).
    fn frame_ext(&self, _scene: &Self::Scene) -> Vec<u8> {
        vec![0; Self::FRAME_EXT_BYTES]
    }

    /// The reflections' uniform's extension block (`ReflExt`: x and y the roughness of the reflective classes 1 and 2).
    fn refl_ext(&self, _scene: &Self::Scene) -> [f32; 4] {
        [0.0; 4]
    }

    /// The extension's buffers into the renderer's slots (each frame, before anything is drawn).
    fn fill_slots(&mut self, _slots: &mut ExtSlots) {}

    /// The near detail's distances (whole up to, gone by) when the frame draws it: the extra draws and the near clusters.
    fn near_detail(&self, _scene: &Self::Scene) -> Option<[f32; 2]> {
        None
    }

    /// The extra draws of the main view made ready (None: the near detail is off).
    fn prepare_extras(&mut self, _scene: &Self::Scene, _cx: Option<ExtraPrepare<'_>>) {}

    /// Whether the main view has extra draws this frame.
    fn has_extras(&self) -> bool {
        false
    }

    /// The extra draws into the prepass (`prepass`) or the opaque pass (`clustered`'s pipelines).
    fn draw_extras(
        &self,
        _pass: &mut wgpu::RenderPass<'_>,
        _prepass: bool,
        _clustered: bool,
    ) -> ExtraDrawn {
        ExtraDrawn::default()
    }

    /// The extra items the extension holds (the reports).
    fn extras_held(&self) -> usize {
        0
    }

    /// The extra casters' vertex entry in the shadow module and their vertex buffers (None: none).
    fn extra_casters() -> Option<(&'static str, Vec<wgpu::VertexBufferLayout<'static>>)> {
        None
    }

    /// The extra casters and the coarsest cascade texel that holds them.
    fn casters(&self) -> Option<(&dyn ExtraCasters, f32)> {
        None
    }

    /// The longest shadow the extra casters cast across a face at `quality` (the sun's light word).
    fn caster_reach(_quality: Quality) -> f32 {
        0.0
    }

    /// The extension's CPU memory by part (the reports).
    fn cpu_memory(&self) -> Vec<(&'static str, u64)> {
        Vec::new()
    }

    /// The extension's GPU memory of geometry (bytes).
    fn gpu_bytes(&self) -> u64 {
        0
    }
}

/// The engine's frame over plain cells, drawn at `mode` (None: not by the enhanced renderer) with effects at `time`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlainScene {
    pub frame: HdScene<StaticCells>,
    pub mode: Option<HdMode>,
    pub time: f32,
}

impl Default for PlainScene {
    fn default() -> PlainScene {
        PlainScene {
            frame: HdScene::new(StaticCells::default()),
            mode: Some(HdMode::default()),
            time: 0.0,
        }
    }
}

impl Deref for PlainScene {
    type Target = HdScene<StaticCells>;

    fn deref(&self) -> &HdScene<StaticCells> {
        &self.frame
    }
}

impl DerefMut for PlainScene {
    fn deref_mut(&mut self) -> &mut HdScene<StaticCells> {
        &mut self.frame
    }
}

/// The scene a world view fills: the engine's frame by deref, and the parts the view sets beside it. An extension whose scene
/// is one can be drawn by a world view (`torque_app::view::WorldView<E>`); its own data stays in the scene beside the frame,
/// filled by the game.
pub trait ViewScene: Default + DerefMut + 'static {
    /// How the scene is drawn from the next frame (None: not by the enhanced renderer).
    fn set_mode(&mut self, mode: Option<HdMode>);

    /// The effects' time (seconds).
    fn set_time(&mut self, seconds: f32);

    /// The cells the view's static meshes and batches go in (None: this scene's geometry takes none, and the view refuses
    /// them).
    fn batch_cells(&mut self) -> Option<&mut StaticCells> {
        None
    }
}

impl ViewScene for PlainScene {
    fn set_mode(&mut self, mode: Option<HdMode>) {
        self.mode = mode;
    }

    fn set_time(&mut self, seconds: f32) {
        self.time = seconds;
    }

    fn batch_cells(&mut self) -> Option<&mut StaticCells> {
        Some(&mut self.frame.cells)
    }
}

/// The engine's own extension: plain cells, no tables, no extra draws.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoExtension;

impl HdExtension for NoExtension {
    type Geometry = StaticCells;
    type Scene = PlainScene;
    type CellRuns = ();
    type LevelInput = ();
    const FRAME_EXT_BYTES: usize = 16;

    fn new(_device: &wgpu::Device) -> NoExtension {
        NoExtension
    }

    fn shaders() -> ShaderExtension {
        crate::shaders::NONE
    }

    fn mode(scene: &PlainScene) -> Option<HdMode> {
        scene.mode
    }

    fn time(scene: &PlainScene) -> f32 {
        scene.time
    }

    fn level_input(_cell: &ExtCell<NoExtension>) {}
}
