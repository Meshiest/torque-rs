//! The per-frame world view.
//!
//! A frame of an example is one [`HdScene`] over plain cells ([`PlainScene`]): the daylight (sun, ambient, fog and the physical
//! sky), a textured floor, and the frame's moving shapes as [`DynamicObject`]s. [`WorldView::draw`] draws it with the enhanced
//! renderer ([`HdWorld`]) into the rectangle the canvas's `GameTSCtrl` gave the world this frame ([`GuiDraws::worlds`]), then
//! the GUI's batches over it with the [`GuiPass`] (no clear between). A canvas without a `GameTSCtrl` drawn draws the GUI alone.
//!
//! The view draws with one renderer of the renderer slot ([`renderers`](crate::renderers)): the engine's own by default, any
//! other a plugin registered when swapped to ([`WorldView::set_renderer`]), or none when the build or the engine has none (the
//! GUI alone). Attached to a client ([`WorldView::attach_to`]), it follows that client's slot (`setRenderer` in its scripts)
//! from its next frame.
//!
//! The view is generic over the renderer's extension ([`HdExtension`]): `WorldView` (the engine's plain cells, [`PlainScene`])
//! by default, `WorldView<E>` for a game's own static geometry ([`WorldView::with_extension`]), whose scene keeps the game's data
//! beside the engine's frame ([`WorldView::scene_mut`]). Only the engine's renderer draws such a view (its slot refuses the
//! others), and a scene without batch cells ([`ViewScene::batch_cells`]) takes no static meshes or batches.
//!
//! The shapes are posed on the CPU: [`WorldView::shape_object`] turns a [`SceneShape`] and its [`ShapeInstance`] (after its
//! threads were advanced and animated) into an object whose vertices are in the world. [`WorldView::set_objects`] hands the
//! frame its objects and keeps the last frame's, which give each object its motion (the anti-aliasing's history follows it).

use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use torque_client::conn::ConnGame;
use torque_gui::GuiDraws;
use torque_render_core::{Gpu, GuiPass, wgpu};
use torque_render_hd::ext::{HdExtension, NoExtension, PlainScene, ViewScene};
use torque_render_hd::{DecalProjection, HdSettings, HdWorld, ParticlePool};
use torque_render_scene::dissolve::Dissolves;
use torque_render_scene::hd::cells::{Batch, Cell, SlotTexture};
use torque_render_scene::hd::clouds::CloudLook;
use torque_render_scene::hd::clusters::{Cluster, MAX_CELL_CLUSTERS};
use torque_render_scene::hd::decals::HdDecal;
use torque_render_scene::hd::distortion::HdDistorter;
use torque_render_scene::hd::environment::ClientDay;
use torque_render_scene::hd::objects::{KeptObject, carry_motion, objects_of};
use torque_render_scene::hd::particle_light::ParticleLight;
use torque_render_scene::hd::scatter::ScatterLook;
use torque_render_scene::hd::view_cull::{ViewCull, is_cut};
use torque_render_scene::hd::{
    DynamicObject, HdCamera, HdDraw, HdScene, HdSpot, Quality, WaterZone, bounding_sphere, material,
};
use torque_render_scene::images::scale_columns;
use torque_render_scene::math::Mat;
use torque_render_scene::moving::ShapeDraws;
use torque_render_scene::particles::SpriteFrame;
use torque_render_scene::shapecache::next_stamp;
use torque_render_scene::shapes::{Loaded, ShapeParts, highest_detail, load, shape_draws};
use torque_render_scene::source::Eye;
use torque_render_scene::textures::TextureSet;
use torque_render_scene::world::ShapeWorld;
use torque_render_scene::{Assets, Blend, Cull, Draw, Filter, Sampler, TexEnv, Texture, Vertex};
use torque_shapes::instance::ShapeInstance;
use torque_shapes::shape::Shape;

#[cfg(feature = "gizmos")]
use crate::gizmos::{GizmoPainter, Gizmos};
use crate::render::{
    EmitterFrame, EmitterKind, EmitterKinds, HdPass, Stage, StaticBatch, StaticBatchSource,
};
use crate::renderers::{DEFAULT_RENDERER, RenderFeatures, RenderFrame, RendererSlot, SwitchError};

/// What the view draws its world with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drawing {
    /// Nothing: no renderer is built in or registered.
    None,
    /// The engine's own renderer (the view's [`HdWorld`]).
    Default,
    /// Another registered renderer, made and kept by the slot: its place among the slot's renderers.
    Other(usize),
}

/// The least turn of the day's sun the view is lit again for: the cosine of a quarter of a degree.
pub const DAY_STEP_COS: f32 = 0.999_990_5;
/// The wind the day's cloud light is laid with.
pub const CLOUD_WIND: [f32; 3] = [1.0, 0.4, 0.0];

/// The field of view a frame is drawn with when its eye names none (degrees, horizontal).
pub const DEFAULT_FOV: u8 = 75;

/// The sun's angular radius the shadows' penumbrae take (degrees: the real sun's), the ambient occlusion's reach (units: about
/// half a figure's width) and the share of it the sun's light takes (a darkening in the sun under feet and where walls meet the
/// floor).
pub const SUN_RADIUS: f32 = 0.27;
pub const AO_RADIUS: f32 = 1.0;
pub const AO_SUN: f32 = 0.5;

/// The largest cell of a sheet of water ([`WorldView::set_water`]).
pub const WATER_CELL: f32 = 32.0;
/// The most cells on a side of a sheet of water.
pub const WATER_MAX_CELLS: usize = 64;

/// A sheet of water ([`WorldView::set_water`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Water<'a> {
    /// Its texture (a path the reader finds) and how many units one repeat of it covers.
    pub texture: &'a str,
    pub tile: f32,
    /// Its tint (gamma bytes; the alpha is the surface's opacity): the renderer's light absorption under it follows it.
    pub color: [u8; 4],
    /// Its surface's height, and the rectangle it covers (x, y).
    pub level: f32,
    pub min: [f32; 2],
    pub max: [f32; 2],
    /// Its bottom's height: a camera between it and the surface, over the rectangle, is under water.
    pub bottom: f32,
    /// How much of its tint's color the view takes under its surface (0..1; 0: none): the share laid over the picture, the
    /// light's absorption along the sight lines under water growing with it (half of it lost over
    /// [`torque_render_hd::water::UNDERWATER_HALF_DISTANCE`] at a half).
    pub underwater: f32,
}

impl Water<'_> {
    /// Whether its height, rectangle and tile are finite numbers, its rectangle's span too.
    pub fn is_finite(&self) -> bool {
        let span = [self.max[0] - self.min[0], self.max[1] - self.min[1]];
        [
            self.level,
            self.tile,
            self.min[0],
            self.min[1],
            self.max[0],
            self.max[1],
            span[0],
            span[1],
        ]
        .iter()
        .all(|v| v.is_finite())
    }
}

/// The most triangles of one part of a static mesh the GPU culls on its own ([`WorldView::set_static_mesh`]).
pub const MESH_CLUSTER_TRIANGLES: u32 = 512;
/// The sampler of a static mesh's untextured surfaces.
const MESH_SAMPLER: Sampler = Sampler {
    repeat: true,
    mag: Filter::Linear,
    min: Filter::Linear,
    mip: Filter::Linear,
    mipmaps: true,
};

/// The key of static mesh `index` among the view's static geometry ([`WorldView::set_static_mesh`]): a cell key of its own,
/// far from any cell of the world's grid.
pub fn static_mesh_key(index: u32) -> [i32; 3] {
    [(index & 0x0fff_ffff) as i32, FAR_KEY, FAR_KEY]
}

/// Where the keys of the view's own static geometry lie, far from any cell of the world's grid.
const FAR_KEY: i32 = -(1 << 28);

/// The most static batches a view holds at once ([`WorldView::set_static_batch`]'s ids are below it).
pub const MAX_STATIC_BATCHES: u32 = 1 << 20;

/// The key of chunk `chunk` of static batch `batch` among the view's static geometry ([`WorldView::set_static_batch`]): beside
/// the static meshes' keys, never one of theirs.
pub fn static_batch_key(batch: u32, chunk: u32) -> [i32; 3] {
    [
        (chunk & 0x0fff_ffff) as i32,
        FAR_KEY,
        FAR_KEY + 1 + (batch % MAX_STATIC_BATCHES) as i32,
    ]
}

/// Why a view refuses static meshes and batches: its scene's geometry takes none ([`ViewScene::batch_cells`]).
const NO_BATCH_CELLS: &str = "this view's scene takes no static batches";

/// The most warnings a view keeps for the console between takes ([`WorldView::take_warnings`]).
pub const MAX_WARNINGS: usize = 16;

/// The static batch ids ([`WorldView::set_static_batch`], below [`MAX_STATIC_BATCHES`]) fall in three ranges: below
/// [`RESTING_BATCHES`] the game's own; from [`RESTING_BATCHES`] to [`PLUGIN_BATCHES`] a ghost scene's resting shapes
/// ([`crate::ghosts::GhostScene`], [`MAX_RESTING_SHAPES`] of them); from [`PLUGIN_BATCHES`] the plugins' sources
/// ([`crate::render::StaticBatchSource`], [`crate::render::MAX_ADDS`] of them).
pub const PLUGIN_BATCHES: u32 = MAX_STATIC_BATCHES - crate::render::MAX_ADDS as u32;

/// The most shapes a ghost scene keeps resting ghosts of as static batches at once: the resting ghosts of a shape past them are
/// drawn every frame, as moving shapes are.
pub const MAX_RESTING_SHAPES: u32 = 256;
/// The first id of a ghost scene's resting shapes' batches (see [`PLUGIN_BATCHES`] for the three ranges).
pub const RESTING_BATCHES: u32 = PLUGIN_BATCHES - MAX_RESTING_SHAPES;

/// A plugin's static batch source and the generation of it the view laid out last.
struct BatchSource {
    source: Box<dyn StaticBatchSource>,
    seen: Option<u64>,
}

/// The sun, the ambient light and the fog of a scene. Colors are gamma-space 0 to 1, as a mission's Sun and Sky give them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Daylight {
    /// Towards the sun (made unit).
    pub towards: [f32; 3],
    pub diffuse: [f32; 3],
    pub ambient: [f32; 3],
    /// The color of the sun's shadows.
    pub shadow: [f32; 3],
    /// The haze toward the horizon, from `fog_start` to `visible` units from the eye (nothing is drawn past `visible`).
    pub fog: [f32; 3],
    pub fog_start: f32,
    pub visible: f32,
}

impl Default for Daylight {
    /// A clear afternoon: the sun high in the south-west, a blue-gray haze.
    fn default() -> Daylight {
        Daylight {
            towards: [-0.45, -0.55, 0.7],
            diffuse: [0.95, 0.92, 0.85],
            ambient: [0.42, 0.45, 0.52],
            shadow: [0.25, 0.27, 0.32],
            fog: [0.72, 0.8, 0.9],
            fog_start: 60.0,
            visible: 220.0,
        }
    }
}

/// A shape read from an [`Assets`] reader, ready to pose and draw.
#[derive(Clone)]
pub struct SceneShape {
    path: String,
    loaded: Arc<Loaded>,
}

impl SceneShape {
    /// The shape at `path` (its textures are looked up beside it); None when it does not load.
    pub fn load(assets: &dyn Assets, path: &str) -> Option<SceneShape> {
        let mut notes = Vec::new();
        let loaded = load(&|_| None, assets, path, &mut notes)?;
        Some(SceneShape {
            path: path.to_string(),
            loaded,
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn shape(&self) -> &Shape {
        &self.loaded.model.shape
    }

    /// An instance at the shape's rest pose, animated once.
    pub fn instance(&self) -> Option<ShapeInstance> {
        ShapeInstance::new(self.shape(), &mut |_| true).ok()
    }

    /// The shape at its rest pose in its own space (its drawn detail, unscaled, unskinned) as a static batch with no copies
    /// yet: the mesh [`StaticBatch::transforms`] places. None for a shape with nothing to draw.
    pub fn batch(&self, assets: &dyn Assets) -> Option<StaticBatch> {
        let model = &self.loaded.model;
        let detail = model.shape.details.get(self.detail()?).copied()?;
        let instance = self.instance()?;
        let mut set = TextureSet::new(assets);
        let (draws, _) = shape_draws(
            model,
            &instance,
            ShapeParts {
                sub_shape: detail.sub_shape,
                object_detail: detail.object_detail,
                hidden: &[],
                colors: None,
                ifl_frames: &Default::default(),
                hidden_objects: None,
                skin: None,
                color_all: None,
            },
            &Mat::IDENTITY,
            &Mat::IDENTITY,
            true,
            &[],
            &mut set,
            &self.path,
        );
        (!draws.is_empty()).then(|| StaticBatch {
            draws,
            textures: set.into_textures(),
            transforms: Vec::new(),
        })
    }

    /// The index of the node named `name`.
    pub fn node(&self, name: &str) -> Option<usize> {
        let shape = self.shape();
        let n = shape.find_name(name.as_bytes())?;
        shape.nodes.iter().position(|node| node.name == n)
    }

    /// The detail level drawn (the most detailed: the examples' shapes are small).
    pub fn detail(&self) -> Option<usize> {
        highest_detail(&self.loaded.model)
    }
}

/// What a released view keeps of its world for the next one.
enum Carried {
    /// The settings of the world that was released.
    Settings(Box<HdSettings>),
    /// No world was ever made: the first one keeps the renderer's defaults.
    Defaults,
}

/// The enhanced renderer, the GUI pass and the scene they draw. `E` is the extension the renderer draws the world's own static
/// geometry with ([`HdExtension`]): the engine's plain cells by default, else a game's, whose scene ([`ViewScene`]) holds the
/// game's data beside the engine's frame.
pub struct WorldView<E: HdExtension<Scene: ViewScene> = NoExtension> {
    /// The enhanced renderer's GPU world (None: released, [`WorldView::release_gpu`]).
    world: Option<HdWorld<E>>,
    /// What the released world was set to, for the one [`WorldView::restore_gpu`] makes (Some exactly while `world` is None).
    carried: Option<Carried>,
    /// The GUI pass, made at the first [`WorldView::draw`] on a view that starts released
    /// ([`WorldView::released_with_extension`]), else with the view.
    gui: Option<GuiPass>,
    scene: E::Scene,
    /// Whether the floor or the daylight changed since the last frame (else only the objects did).
    statics_changed: bool,
    /// Where the day's sun was when the view was last lit by it ([`WorldView::set_day`]).
    day_sun: Option<[f32; 3]>,
    /// The shadows' color of the last [`Daylight`]: a day whose look names none keeps it.
    daylight_shadow: [f32; 3],
    /// The renderer drawing.
    drawing: Drawing,
    /// The GPU the slot's other renderers are made on.
    gpu: Gpu,
    /// The engine's renderer's quality step and features.
    quality: Quality,
    features: RenderFeatures,
    /// The particle light rule the game asked for ([`WorldView::set_particle_light`]), drawn when the features have lit
    /// particles.
    particle_light: Option<ParticleLight>,
    /// The client's renderer slot the view follows, and the switches it has seen.
    slot: Option<(Rc<RendererSlot>, u64)>,
    /// The client's debug gizmos, drawn over the world.
    #[cfg(feature = "gizmos")]
    gizmos: Option<GizmoPainter>,
    /// The cells of each static batch the view holds, by its id.
    batch_keys: HashMap<u32, (Vec<[i32; 3]>, usize)>,
    /// Warnings for the console of the client the view draws ([`WorldView::take_warnings`]).
    warnings: Vec<String>,
    /// The most triangles the static batches lay out together ([`WorldView::set_static_batch_budget`]).
    batch_budget: usize,
    /// The plugins' static batch sources (laid out under the ids from [`PLUGIN_BATCHES`]).
    batch_sources: Vec<BatchSource>,
    /// The plugins' emitter kinds.
    kinds: EmitterKinds,
    /// What the last frame's moving objects were made of ([`objects_of`]'s kept objects): an object whose builds did not change
    /// is the last frame's again.
    kept: HashMap<u32, KeptObject>,
    /// The effects' clock ([`WorldView::advance_time`]); the scene's time is its wrapped `f32`.
    effect_clock: EffectClock,
    /// Whether the renderer's uploads and the clouds' noise are held to no time ([`WorldView::set_unbudgeted`]): kept for a
    /// world installed later.
    unbudgeted: bool,
}

impl WorldView {
    /// A view on `gpu` with the default [`Daylight`], no floor and no objects, drawn at [`Quality::High`]. The effects' pipelines
    /// start being made on a worker thread ([`HdWorld::warm_effects`]).
    pub fn new(gpu: &Gpu) -> WorldView {
        WorldView::with_extension(gpu)
    }

    /// [`WorldView::new`] without the GPU world: no renderer is built and no warm-up thread starts, and the GUI pass is made at
    /// the first [`draw`](WorldView::draw). See [`WorldView::released_with_extension`].
    pub fn released(gpu: &Gpu) -> WorldView {
        WorldView::released_with_extension(gpu)
    }
}

impl<E: HdExtension<Scene: ViewScene>> WorldView<E> {
    /// [`WorldView::new`] drawing the world's static geometry with the extension `E`, its scene the scene's default with the
    /// view's frame settings. The renderer's modules are composed with `E`'s shaders once a process, so every view and
    /// renderer of a process has the same extension ([`HdWorld::try_from_device`] panics on another).
    pub fn with_extension(gpu: &Gpu) -> WorldView<E> {
        Self::make(gpu, false)
    }

    /// [`with_extension`](Self::with_extension) for a game that draws its world with its own renderer: the view starts as if
    /// [`release_gpu`](Self::release_gpu) had been called on it. No [`HdWorld`] is built, no pipeline-warming thread starts and
    /// the GUI pass is not made until the first [`draw`](Self::draw) (a game that draws its own GUI never makes it), so a
    /// start holds nothing on the GPU that the game would drop at its first frame. The scene, the daylight and the setters
    /// that need no renderer work as on any view. [`restore_gpu`](Self::restore_gpu) or [`adopt_gpu`](Self::adopt_gpu) build
    /// the renderer later, with the engine's default settings; until then the view is released
    /// ([`has_gpu`](Self::has_gpu) is false), `draw` leaves the world to the app, and `draw_world` is an error.
    pub fn released_with_extension(gpu: &Gpu) -> WorldView<E> {
        Self::make(gpu, true)
    }

    fn make(gpu: &Gpu, released: bool) -> WorldView<E> {
        let mut scene = E::Scene::default();
        let frame: &mut HdScene<E::Geometry> = &mut scene;
        // The physical sky in the server sky's place: the scene has no sky of its own.
        frame.atmosphere = true;
        // Sharper penumbrae, a wider occlusion and some of it in the sun.
        frame.sun_radius = SUN_RADIUS;
        frame.ao_radius = AO_RADIUS;
        frame.ao_sun = AO_SUN;
        // The occlusion without grain where it is strong.
        frame.smooth_occlusion = true;
        // Smoke and dust lit by the sun, the sky and the lights; additive glows stay unlit.
        frame.particle_light = Some(ParticleLight::BLENDED);
        let mut view = WorldView {
            world: (!released).then(|| HdWorld::new(gpu)),
            carried: released.then_some(Carried::Defaults),
            gui: (!released).then(|| GuiPass::new(gpu, None)),
            scene,
            statics_changed: true,
            day_sun: None,
            daylight_shadow: Daylight::default().shadow,
            drawing: if cfg!(feature = "renderer") {
                Drawing::Default
            } else {
                Drawing::None
            },
            gpu: gpu.clone(),
            quality: Quality::High,
            features: RenderFeatures::ALL,
            particle_light: Some(ParticleLight::BLENDED),
            slot: None,
            #[cfg(feature = "gizmos")]
            gizmos: None,
            batch_keys: HashMap::new(),
            warnings: Vec::new(),
            batch_budget: crate::render::MAX_VIEW_BATCH_TRIANGLES,
            batch_sources: Vec::new(),
            kinds: EmitterKinds::default(),
            kept: HashMap::new(),
            effect_clock: EffectClock::default(),
            unbudgeted: false,
        };
        view.set_daylight(&Daylight::default());
        // The effects' pipelines made beside the menus and the map's loading, not at the first shot.
        if let Some(world) = &mut view.world {
            force_decals(world);
            world.warm_effects();
        }
        view
    }

    /// The scene as the next frame draws it.
    pub fn scene(&self) -> &E::Scene {
        &self.scene
    }

    /// The scene, for the game's data beside the engine's frame (what the extension reads each frame: its uniform block, its
    /// mode). An edit to the frame's statics here (cells, planes, lights) is not seen until
    /// [`mark_statics_changed`](Self::mark_statics_changed).
    pub fn scene_mut(&mut self) -> &mut E::Scene {
        &mut self.scene
    }

    /// The next frame takes the scene's statics as changed, as after the view's own setters: the renderer compares its cells,
    /// planes and lights again instead of sending only the moving objects (the fast path of a frame where only they moved), so
    /// call it once after an edit to them through [`scene_mut`](Self::scene_mut), not every frame.
    pub fn mark_statics_changed(&mut self) {
        self.statics_changed = true;
    }

    /// The extension's type name when it is a game's, not the engine's ([`NoExtension`]): only the engine's renderer draws such
    /// a view.
    fn game_extension() -> Option<&'static str> {
        (TypeId::of::<E>() != TypeId::of::<NoExtension>()).then(type_name::<E>)
    }

    /// Whether the effects' pipelines are still being made on their worker thread ([`HdWorld::warming`]).
    pub fn warming(&self) -> bool {
        self.world.as_ref().is_some_and(HdWorld::warming)
    }

    /// In a browser: whether the last frame wanted clouds while their noise was still being made ([`HdWorld::clouds_waiting`]):
    /// the sky is not all drawn yet. Never natively.
    pub fn clouds_waiting(&self) -> bool {
        self.world.as_ref().is_some_and(HdWorld::clouds_waiting)
    }

    /// The enhanced renderer the view draws with (its timers and reports).
    ///
    /// # Panics
    /// While the GPU world is released ([`release_gpu`](Self::release_gpu); [`has_gpu`](Self::has_gpu) says).
    pub fn renderer(&mut self) -> &mut HdWorld<E> {
        self.world
            .as_mut()
            .expect("the view's GPU world is released: call restore_gpu first")
    }

    /// Starts the renderer's histories again (a cut: the next frame keeps nothing of the last).
    pub fn reset_history(&mut self) {
        if let Some(world) = &mut self.world {
            world.reset_history();
        }
    }

    /// Whether the view holds its GPU world (false between [`release_gpu`](Self::release_gpu) and
    /// [`restore_gpu`](Self::restore_gpu)).
    pub fn has_gpu(&self) -> bool {
        self.world.is_some()
    }

    /// The bytes of vertices and indices the view's GPU world holds now (0 while it is released): the world's static arena, its
    /// loose draws, its particles and the extension's.
    pub fn gpu_geometry_bytes(&self) -> u64 {
        self.world.as_ref().map_or(0, HdWorld::geometry_bytes)
    }

    /// Drops the enhanced renderer's GPU world (its buffers, textures, targets and pipelines), so the app's frame can draw the
    /// world another way with the memory back, and [`restore_gpu`](Self::restore_gpu) builds it again. Natively it waits for
    /// the device to free the memory; in a browser the buffers are freed when the browser collects them, some time after.
    /// The scene stays as it was (cells, planes, lights, moving objects, particles, the daylight), and so do the quality, the
    /// slot and the GUI pass. Every setting a game made on the renderer through [`renderer`](Self::renderer) is kept for the
    /// restore ([`HdSettings`]). What is not: the passes a game added with [`add_pass`](Self::add_pass) (an attached view takes
    /// its slot's again), and `renderer().ext`, which is made new by the restore, so a game's extension rebuilds the state it
    /// keeps in it. While released, `draw` leaves the world to the app when the engine's renderer was drawing (it clears nothing,
    /// draws the GUI over what the app drew, and returns false), and `draw_world` is an error. A view already released is left
    /// as it is.
    pub fn release_gpu(&mut self) {
        let Some(world) = self.world.take() else {
            return;
        };
        self.carried = Some(Carried::Settings(Box::new(world.settings())));
        // A frame still in flight holds the buffers; the warm-up thread ends with the world.
        let _ = world.finish();
        drop(world);
        let _ = self.gpu.device().poll(wgpu::PollType::wait_indefinitely());
    }

    /// Makes the GPU world again after [`release_gpu`](Self::release_gpu): a new renderer with the settings it had (and a new
    /// `ext`), the effects warming, the slot's passes and the static batch sources made again, every scene cell marked not resident and the
    /// statics changed, so the next frame sends the whole scene. Returns the cells whose meshes the scene had dropped after the
    /// old upload. The anti-aliasing's and the occlusion's histories start over, and the automatic exposure (when on) adapts
    /// again from its start, so the first frames after a restore settle briefly. The meshes dropped cannot be sent again, so the game builds them again (a scene without [`ViewScene::batch_cells`]
    /// counts and clears none: its game marks its own cells). A view that holds its world is left as it is (0).
    ///
    /// The world is built here, on the calling thread, cold: its first frames compile what the renderer needs, a stall on a
    /// window's thread. A game that switches looks and can build the world elsewhere uses [`adopt_gpu`](Self::adopt_gpu).
    pub fn restore_gpu(&mut self) -> usize {
        if self.carried.is_none() {
            return 0;
        }
        let world = HdWorld::new(&self.gpu);
        self.install_world(world)
    }

    /// Takes a GPU world built elsewhere (on a worker thread, warmed there) in place of the view's own: the one it holds is
    /// dropped as [`release_gpu`](Self::release_gpu) drops it (settings kept, the device waited for), then `world` is set up
    /// exactly as [`restore_gpu`](Self::restore_gpu) sets up the one it builds (the released world's settings applied, the
    /// slot's passes and batch sources made again, every scene cell marked not resident, the statics changed), and the same
    /// count of cells whose meshes the scene had dropped is returned. A view whose world was released takes it too.
    ///
    /// Use it to switch looks without building and compiling a world on the window's thread (the release still waits for the
    /// device): build the new world with [`HdWorld::new`] on a worker thread, call
    /// [`HdWorld::warm_effects`] there and let it finish, hand it in on the window's thread. [`restore_gpu`](Self::restore_gpu)
    /// is for a view that released its world and has no other to give.
    ///
    /// `world` must be an `HdWorld<E>` of the same extension as the view's, made on the device of the [`Gpu`] the view was
    /// made on (a buffer or texture of another device is rejected by wgpu at its first use). Whatever was set on `world`
    /// before is replaced by the view's carried settings, so set the game's settings through [`renderer`](Self::renderer)
    /// after, or before the release.
    pub fn adopt_gpu(&mut self, world: HdWorld<E>) -> usize {
        debug_assert!(
            world.device() == self.gpu.device(),
            "adopt_gpu: the world was made on another device"
        );
        self.release_gpu();
        self.install_world(world)
    }

    /// Sets up `world` as the view's after a release (`carried` is Some): [`restore_gpu`](Self::restore_gpu) and
    /// [`adopt_gpu`](Self::adopt_gpu)'s common end.
    fn install_world(&mut self, mut world: HdWorld<E>) -> usize {
        let carried = self
            .carried
            .take()
            .expect("a world is installed only after a release");
        if let Carried::Settings(settings) = carried {
            world.apply_settings(*settings);
        }
        if self.slot.is_some() {
            world.features = self.features.features;
        }
        force_decals(&mut world);
        if self.unbudgeted {
            world.set_unbudgeted();
        }
        world.warm_effects();
        self.world = Some(world);
        if let Some(slot) = self.slot.as_ref().map(|(slot, _)| Rc::clone(slot)) {
            self.take_adds(&slot);
        }
        self.statics_changed = true;
        self.scene.batch_cells().map_or(0, |cells| {
            cells
                .cells
                .values_mut()
                .map(|c| std::mem::take(&mut c.resident) && c.indices.is_empty())
                .filter(|&dropped| dropped)
                .count()
        })
    }

    /// The engine's renderer's quality step.
    pub fn set_quality(&mut self, quality: Quality) {
        self.quality = quality;
        self.scene.set_mode(Some(self.features.mode(quality)));
    }

    /// The id of the renderer drawing (None: the world is not drawn).
    pub fn current_renderer(&self) -> Option<&str> {
        match self.drawing {
            Drawing::None => None,
            Drawing::Default => Some(DEFAULT_RENDERER),
            Drawing::Other(i) => self.slot.as_ref().and_then(|(slot, _)| slot.ids().nth(i)),
        }
    }

    /// Draws with the renderer `id` from the next frame. Attached to a client, through its slot (its scripts' `getRenderer`
    /// sees it); else only the engine's own renderer is known. Err: no renderer has the id (naming the registered ones), or
    /// the renderer could not be made (the one drawing stays).
    pub fn set_renderer(&mut self, id: &str) -> Result<(), String> {
        if let Some((slot, _)) = &self.slot {
            slot.switch(id).map_err(|e| e.to_string())?;
            self.follow_slot();
            return Ok(());
        }
        if !cfg!(feature = "renderer") || id != DEFAULT_RENDERER {
            let registered = if cfg!(feature = "renderer") {
                vec![DEFAULT_RENDERER.to_owned()]
            } else {
                Vec::new()
            };
            return Err(SwitchError::Unknown {
                id: id.to_owned(),
                registered,
            }
            .to_string());
        }
        Ok(())
    }

    /// Follows the renderer slot of the client `host` runs, when its engine has one ([`RendererSlotPlugin`](crate::renderers::RendererSlotPlugin)):
    /// the view draws with the slot's renderer and its features, and with each swap from its next frame. A renderer chosen
    /// before (a script at start-up) that cannot be made on this view's GPU gives way to the engine's own, with a warning on
    /// the client's console.
    pub fn attach_to<C: ConnGame, S: torque_client::host::GameServer>(
        &mut self,
        host: &mut crate::Host<C, S>,
    ) {
        let vm = host.server.vm_mut();
        if let Some(slot) = RendererSlot::of(vm)
            && let Err(e) = self.attach_slot(slot)
        {
            vm.warn(format!("renderer: {e}"));
        }
        #[cfg(feature = "gizmos")]
        if let Some(gizmos) = Gizmos::of(vm) {
            self.attach_gizmos(gizmos);
        }
    }

    /// Draws `gizmos` over the world from the next frame ([`attach_to`](Self::attach_to) does this for the set of the client it
    /// follows), and ages them by each frame it draws.
    #[cfg(feature = "gizmos")]
    pub fn attach_gizmos(&mut self, gizmos: Rc<Gizmos>) {
        self.gizmos = Some(GizmoPainter::new(gizmos));
    }

    /// The debug gizmos of the client the view is attached to (None: it was made without
    /// [`GizmosPlugin`](crate::gizmos::GizmosPlugin), or the view is not attached).
    #[cfg(feature = "gizmos")]
    pub fn gizmos(&self) -> Option<&Rc<Gizmos>> {
        self.gizmos.as_ref().map(GizmoPainter::gizmos)
    }

    /// Every frame ages the gizmos by `seconds` (None: by the time the clock says passed since the last frame). The offscreen
    /// runners set it, so a picture does not depend on how long the machine took.
    #[cfg(feature = "gizmos")]
    pub fn set_gizmo_frame_seconds(&mut self, seconds: Option<f32>) {
        if let Some(painter) = &mut self.gizmos {
            painter.set_frame_seconds(seconds);
        }
    }

    /// Follows `slot` (see [`attach_to`](Self::attach_to)); its renderers are made on this view's GPU. Err: the renderer the
    /// slot had chosen could not be made (the engine's own draws instead, else none).
    pub fn attach_slot(&mut self, slot: Rc<RendererSlot>) -> Result<(), String> {
        self.features = slot.features();
        self.scene.set_mode(Some(self.features.mode(self.quality)));
        if let Some(world) = &mut self.world {
            world.features = self.features.features;
            force_decals(world);
        }
        let lit = self.features.particle_light;
        self.scene.particle_light = self.particle_light.filter(|_| lit);
        self.take_adds(&slot);
        let made = slot
            .attach_gpu(&self.gpu, Self::game_extension())
            .map_err(|e| e.to_string());
        // Any generation but the slot's: the next follow takes its renderer.
        self.slot = Some((Rc::clone(&slot), slot.generation().wrapping_sub(1)));
        self.follow_slot();
        made
    }

    /// What the slot's plugins added to the rendering, made for this view: its passes, static batch sources and emitter kinds
    /// in place of any the view held before.
    fn take_adds(&mut self, slot: &RendererSlot) {
        let adds = slot.adds();
        if let Some(world) = &mut self.world {
            let passes = world.passes_mut();
            passes.clear();
            for e in &adds.passes {
                passes.add(e.stage(), e.make());
            }
        }
        for i in 0..self.batch_sources.len() {
            self.remove_static_batch(PLUGIN_BATCHES + i as u32);
        }
        self.batch_sources.clear();
        if !adds.batches.is_empty() && self.scene.batch_cells().is_none() {
            let names: Vec<&str> = adds.batches.iter().map(|e| e.name()).collect();
            self.warn(format!(
                "the static batch sources {} are left out: {NO_BATCH_CELLS}",
                names.join(", ")
            ));
        } else {
            self.batch_sources = adds
                .batches
                .iter()
                .map(|e| BatchSource {
                    source: e.make(),
                    seen: None,
                })
                .collect();
        }
        self.kinds.kinds = adds
            .kinds
            .iter()
            .map(|e| (e.name().to_owned(), e.make()))
            .collect();
    }

    /// Runs `pass` at `stage` of every frame the engine's renderer draws, after the passes there ([`crate::render`]). A view
    /// attached to a client starts again from the passes its plugins added.
    pub fn add_pass(&mut self, stage: Stage, pass: impl HdPass) {
        let Some(world) = &mut self.world else {
            self.warn("a pass is left out: the view's GPU world is released".to_owned());
            return;
        };
        world.add_pass(stage, pass);
    }

    /// Lays out `source`'s batch from the next frame, and again whenever it changes ([`StaticBatchSource`]). False: the view
    /// holds [`crate::render::MAX_ADDS`] sources, or its scene takes no static batches ([`ViewScene::batch_cells`], with a
    /// warning naming the source).
    pub fn add_static_batch_source<S: StaticBatchSource>(&mut self, source: S) -> bool {
        if self.scene.batch_cells().is_none() {
            self.warn(format!(
                "the static batch source {} is left out: {NO_BATCH_CELLS}",
                type_name::<S>()
            ));
            return false;
        }
        if self.batch_sources.len() >= crate::render::MAX_ADDS {
            return false;
        }
        self.batch_sources.push(BatchSource {
            source: Box::new(source),
            seen: None,
        });
        true
    }

    /// Draws `kind`'s particles into each frame's particles from the next ([`WorldView::emitter_draws`]); a kind named `name`
    /// already is replaced.
    pub fn add_emitter_kind(&mut self, name: &str, kind: impl EmitterKind) {
        let kind: Box<dyn EmitterKind> = Box::new(kind);
        match self.kinds.kinds.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = kind,
            None => self.kinds.kinds.push((name.to_owned(), kind)),
        }
    }

    /// The names of the emitter kinds the view draws, in order.
    pub fn emitter_kinds(&self) -> impl Iterator<Item = &str> + '_ {
        self.kinds.kinds.iter().map(|(n, _)| n.as_str())
    }

    /// Every frame runs the emitter kinds on by `seconds` (None: by the time the clock says passed since the last frame). The
    /// offscreen runners set it, so a picture does not depend on how long the machine took.
    pub fn set_effect_frame_seconds(&mut self, seconds: Option<f32>) {
        self.kinds.fixed = seconds;
    }

    /// Holds what the renderer draws to no time ([`HdWorld::set_unbudgeted`]): its uploads and the clouds' noise are made in
    /// full each frame, as offscreen pictures need. Applied now if the view has its GPU world, and to every world it installs
    /// later ([`restore_gpu`](Self::restore_gpu), [`adopt_gpu`](Self::adopt_gpu)).
    pub fn set_unbudgeted(&mut self) {
        self.unbudgeted = true;
        if let Some(world) = &mut self.world {
            world.set_unbudgeted();
        }
    }

    /// The emitter kinds run on by a frame's step and drawn for `frame` into `draws` (the frame's particles, their textures in
    /// `textures`): what [`GhostScene::effects`](crate::ghosts::GhostScene::effects) adds beside TGE's emitters. Nothing without
    /// a kind.
    pub fn emitter_draws(
        &mut self,
        frame: &EmitterFrame,
        textures: &mut TextureSet<'_>,
        draws: &mut Vec<(f32, Draw)>,
    ) {
        if let Some(warning) = self.kinds.frame(frame, textures, draws) {
            self.warn(warning);
        }
    }

    /// Puts `batch` into the view's static geometry as the static batch `id` (below [`MAX_STATIC_BATCHES`]; the plugins' sources
    /// take the ids from [`PLUGIN_BATCHES`]), replacing the one there: cut into chunks ([`StaticBatch::layout`]), each a cell kept
    /// on the GPU and culled there, as [`WorldView::set_static_mesh`] keeps a mesh. An empty batch takes it out. At most
    /// [`MAX_BATCH_TRIANGLES`](crate::render::MAX_BATCH_TRIANGLES) triangles of it are laid out, and no more than the view's
    /// other batches leave of [`MAX_VIEW_BATCH_TRIANGLES`](crate::render::MAX_VIEW_BATCH_TRIANGLES): the copies past them are
    /// left out, in order. Some: copies were left out, the warning (also queued for the console, [`WorldView::take_warnings`]).
    pub fn set_static_batch(&mut self, id: u32, batch: &StaticBatch) -> Option<String> {
        let id = id % MAX_STATIC_BATCHES;
        self.remove_static_batch(id);
        if batch.draws.is_empty() {
            return None;
        }
        if self.scene.batch_cells().is_none() {
            let warning = format!("static batch {id} is left out: {NO_BATCH_CELLS}");
            self.warn(warning.clone());
            return Some(warning);
        }
        let others: usize = self.batch_keys.values().map(|(_, t)| t).sum();
        let budget = self.batch_budget.saturating_sub(others);
        let layout = batch.layout(budget);
        let warning = (layout.left_out > 0).then(|| {
            format!(
                "static batch {id}: {} of its {} copies are left out (a batch lays out at most {} triangles, this view {} over all \
                 its batches; {} were free)",
                layout.left_out,
                batch.transforms.len(),
                crate::render::MAX_BATCH_TRIANGLES,
                self.batch_budget,
                budget,
            )
        });
        if let Some(w) = &warning {
            self.warn(w.clone());
        }
        if layout.chunks.is_empty() {
            return warning;
        }
        let local = self.intern(batch.textures.clone());
        let mut keys = Vec::with_capacity(layout.chunks.len());
        for (n, copies) in layout.chunks.iter().enumerate() {
            let key = static_batch_key(id, n as u32);
            if self.put_static_mesh(key, &local, &batch.chunk_draws(copies)) {
                keys.push(key);
            }
        }
        self.batch_keys.insert(id, (keys, layout.triangles));
        warning
    }

    /// The most triangles the view's static batches lay out together from the next batch laid out: `triangles`, at most
    /// [`MAX_VIEW_BATCH_TRIANGLES`](crate::render::MAX_VIEW_BATCH_TRIANGLES) (the default). The batches laid out already stay.
    pub fn set_static_batch_budget(&mut self, triangles: usize) {
        self.batch_budget = triangles.min(crate::render::MAX_VIEW_BATCH_TRIANGLES);
    }

    /// The triangles the view's static batches lay out, all together.
    pub fn static_batch_triangles(&self) -> usize {
        self.batch_keys.values().map(|(_, t)| t).sum()
    }

    /// The warnings for the console since the last take (a static batch's copies left out, an emitter kind's sprites past the
    /// cap); the window and offscreen runners put them on the client's console each frame ([`WorldView::report_warnings`]).
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// The warnings since the last take on the console of the client `host` runs.
    pub fn report_warnings<C: ConnGame, S: torque_client::host::GameServer>(
        &mut self,
        host: &mut crate::Host<C, S>,
    ) {
        if self.warnings.is_empty() {
            return;
        }
        let vm = host.server.vm_mut();
        for w in self.take_warnings() {
            vm.warn(format!("render: {w}"));
        }
    }

    /// Queues `warning` for the console (at most [`MAX_WARNINGS`] wait).
    pub(crate) fn warn(&mut self, warning: String) {
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(warning);
        }
    }

    /// Takes the static batch `id` out of the view (nothing if there is none).
    pub fn remove_static_batch(&mut self, id: u32) {
        let Some((keys, _)) = self.batch_keys.remove(&(id % MAX_STATIC_BATCHES)) else {
            return;
        };
        for key in keys {
            self.remove_static_mesh(key);
        }
    }

    /// The cells the static batch `id` is laid out in (0: the view holds no such batch).
    pub fn static_batch_cells(&self, id: u32) -> usize {
        self.batch_keys
            .get(&(id % MAX_STATIC_BATCHES))
            .map_or(0, |(keys, _)| keys.len())
    }

    /// The plugins' static batches that changed, laid out again (a compare a source otherwise).
    fn follow_batch_sources(&mut self) {
        for i in 0..self.batch_sources.len() {
            let b = &mut self.batch_sources[i];
            let generation = b.source.generation();
            if b.seen == Some(generation) {
                continue;
            }
            b.seen = Some(generation);
            let batch = b.source.batch();
            self.set_static_batch(PLUGIN_BATCHES + i as u32, &batch);
        }
    }

    /// The slot's renderer, when it was swapped since the last frame (one compare otherwise). The engine's own renderer starts
    /// its histories again when it comes back.
    fn follow_slot(&mut self) {
        let Some((slot, seen)) = &mut self.slot else {
            return;
        };
        let generation = slot.generation();
        if *seen == generation {
            return;
        }
        *seen = generation;
        let was = self.drawing;
        self.drawing = match (slot.current_index(), slot.current_entry()) {
            (Some(index), Some(e)) if e.make().is_some() => Drawing::Other(index),
            (_, Some(_)) => Drawing::Default,
            _ => Drawing::None,
        };
        if self.drawing == Drawing::Default && was != Drawing::Default {
            self.reset_history();
        }
    }

    /// How the volumetric clouds look (None: the engine's, [`CloudLook::DEFAULT`]).
    pub fn set_cloud_look(&mut self, look: Option<CloudLook>) {
        self.scene.cloud_look = look;
    }

    pub fn set_daylight(&mut self, d: &Daylight) {
        let length = d
            .towards
            .iter()
            .map(|v| v * v)
            .sum::<f32>()
            .sqrt()
            .max(1e-6);
        let s = &mut self.scene;
        s.sun.towards = d.towards.map(|v| v / length);
        s.sun.diffuse = d.diffuse;
        s.sun.ambient = d.ambient;
        s.sun.shadow = d.shadow;
        s.fog.color = d.fog;
        s.fog.start = d.fog_start;
        s.fog.end = d.visible;
        s.clear = d.fog;
        s.fog_distance = d.fog_start;
        s.visible_distance = d.visible;
        self.statics_changed = true;
        // A day lights the view again over this daylight.
        self.day_sun = None;
        self.daylight_shadow = d.shadow;
    }

    /// Lights the view by `day` under `look` (a server's TimeOfDay and ScatterSky): the sun or the moon, the ambient light, the
    /// fog's color and the clear, the clouds' light and the physical sky made for the sun, over the [`Daylight`]'s distances.
    /// A ScatterSky carries no shadow color: a look without one keeps the [`Daylight`]'s (else the shadows are black, by day and
    /// under the moon).
    /// The view's statics are made again only when the sun has moved [`DAY_STEP_COS`] or more since it was last lit (the sky
    /// and the terrain's light go up again then, not every frame). False when nothing changed (no day, or too small a move).
    pub fn set_day(&mut self, day: &ClientDay, look: &ScatterLook) -> bool {
        let (Some(d), Some(light)) = (day.day.as_ref(), day.light(look)) else {
            return false;
        };
        let towards = d.towards();
        if let Some(last) = self.day_sun {
            let dot: f32 = (0..3).map(|k| last[k] * towards[k]).sum();
            if dot > DAY_STEP_COS {
                return false;
            }
        }
        let s = &mut self.scene;
        s.sun = light.sun();
        if s.sun.shadow == [0.0; 3] {
            s.sun.shadow = self.daylight_shadow;
        }
        s.fog = light.fog(s.fog);
        s.clear = light.fog;
        s.atmosphere_sun = Some(towards);
        s.clouds = Some(light.clouds(CLOUD_WIND));
        self.day_sun = Some(towards);
        self.statics_changed = true;
        true
    }

    /// The frame's spot lights (the scene's lights go up again when they changed).
    pub fn set_spots(&mut self, spots: Vec<HdSpot>) {
        if self.scene.spots != spots {
            self.scene.spots = spots;
            self.scene.lights_generation = self.scene.lights_generation.wrapping_add(1);
        }
    }

    /// A square floor at height 0, `half` units from the origin each way, textured with `texture` (a path the reader finds)
    /// repeated every `tile` units.
    pub fn set_floor(&mut self, assets: &dyn Assets, texture: &str, half: f32, tile: f32) {
        let mut set = TextureSet::new(assets);
        let found = set.get(texture);
        let local = self.intern(set.into_textures());
        self.put_floor(floor_draw(found, [255; 3], half, tile), &local);
    }

    /// A square floor of one color (a backdrop for looking at a shape).
    pub fn set_plain_floor(&mut self, half: f32, color: [u8; 3]) {
        self.put_floor(floor_draw(None, color, half, 1.0), &[]);
    }

    /// A terrain as the ground in place of the floor, textured with `texture` (a path the reader finds) repeated every `tile`
    /// units ([`super::terrain::Terrain::draws`]).
    pub fn set_terrain(
        &mut self,
        assets: &dyn Assets,
        terrain: &super::terrain::Terrain,
        texture: &str,
        tile: f32,
    ) {
        let mut set = TextureSet::new(assets);
        let found = set.get(texture);
        let local = self.intern(set.into_textures());
        self.scene.planes = terrain
            .draws(found, tile)
            .iter()
            .map(|d| HdDraw::from_fixed(d, material::KIND_PLANE, &local))
            .collect();
        self.scene.planes_generation = self.scene.planes_generation.wrapping_add(1);
        self.statics_changed = true;
    }

    /// A terrain as the ground in place of the floor with each of its materials drawn ([`super::terrain::Terrain::layer_draws`]):
    /// `layers[k]` is material `k`'s texture (a path the reader finds) and texture coordinates. The first is opaque, the others
    /// are the renderer's terrain layers over it ([`material::LAYER`]: their edges sharpened, broken up by their textures'
    /// alpha).
    pub fn set_terrain_layers(
        &mut self,
        assets: &dyn Assets,
        terrain: &super::terrain::Terrain,
        layers: &[(&str, super::terrain::LayerUv<'_>)],
    ) {
        let mut set = TextureSet::new(assets);
        let looks: Vec<super::terrain::LayerLook<'_>> = layers
            .iter()
            .map(|&(texture, uv)| super::terrain::LayerLook {
                texture: set.get(texture),
                uv,
            })
            .collect();
        let local = self.intern(set.into_textures());
        self.scene.planes = terrain
            .layer_draws(&looks)
            .iter()
            .map(|d| {
                let hd = HdDraw::from_fixed(d, material::KIND_PLANE, &local);
                if d.blend == Blend::Opaque {
                    hd
                } else {
                    hd.with_material(material::LAYER)
                }
            })
            .collect();
        self.scene.planes_generation = self.scene.planes_generation.wrapping_add(1);
        self.statics_changed = true;
    }

    /// A sheet of water: the plane at height `level` over the rectangle `min`..`max` (x, y), tinted `color` (its alpha the
    /// surface's opacity) with `texture` (a path the reader finds) repeated every `tile` units, drawn by the renderer as water
    /// (its waves, refraction, reflection and the sun's glint). It is cut into cells of at most [`WATER_CELL`] units so the
    /// fog and the light are interpolated over it. Replaces the water there was.
    /// A sheet whose height, rectangle or tile is not finite numbers (a map's file is not trusted) is not drawn: the water
    /// there was is taken away.
    /// A camera under its surface takes its tint and absorption, split at the waterline when it straddles it
    /// ([`underwater_zone`]).
    pub fn set_water(&mut self, assets: &dyn Assets, water: &Water<'_>) {
        self.scene.water_zone = underwater_zone(water);
        if !water.is_finite() {
            self.scene.water.clear();
            self.scene.planes_generation = self.scene.planes_generation.wrapping_add(1);
            self.statics_changed = true;
            return;
        }
        let mut set = TextureSet::new(assets);
        let found = set.get(water.texture);
        let local = self.intern(set.into_textures());
        let draw = water_draw(found, water);
        self.scene.water = vec![HdDraw::from_fixed(&draw, material::KIND_PLANE, &local)];
        self.scene.planes_generation = self.scene.planes_generation.wrapping_add(1);
        self.statics_changed = true;
    }

    /// Whether the ground's textures are varied against visible tiling (stochastic tiling of every terrain layer and a
    /// large-scale tint: [`torque_render_scene::hd::HdScene::ground_variation`]); off by default.
    pub fn set_ground_variation(&mut self, on: bool) {
        self.scene.ground_variation = on;
    }

    /// The volumetric clouds' own clock in seconds ([`torque_render_scene::hd::HdScene::cloud_time`]: their drift, change of
    /// shape and shadows), or None for the effect time. A game advances it as fast as it wants the clouds to run.
    pub fn set_cloud_time(&mut self, time: Option<f32>) {
        self.scene.cloud_time = time;
    }

    /// The effects' clock in seconds: the water's waves and ripples, the sky's cloud layers (and the volumetric clouds without
    /// a clock of their own) and the stars' twinkle run on it; the image's grain does not (its pattern stays still). It starts
    /// at 0 and stands still until a game runs it ([`WorldView::advance_time`]).
    pub fn time(&self) -> f32 {
        E::time(&self.scene)
    }

    /// Runs the effects' clock on by `seconds`, a frame's time ([`EffectClock`]).
    pub fn advance_time(&mut self, seconds: f32) {
        self.effect_clock.advance(seconds);
        self.scene.set_time(self.effect_clock.gpu());
    }

    /// No ground and no water: the floor, the terrain and the water taken away.
    pub fn clear_ground(&mut self) {
        self.scene.planes.clear();
        self.scene.water.clear();
        self.scene.water_zone = None;
        self.scene.planes_generation = self.scene.planes_generation.wrapping_add(1);
        self.statics_changed = true;
    }

    /// Forgets the day's light ([`WorldView::set_day`]): the sky made for its sun and its clouds' light. The view is lit by
    /// the next [`Daylight`] it is given (a world without a day after one with it).
    pub fn clear_day(&mut self) {
        self.scene.atmosphere_sun = None;
        self.scene.clouds = None;
        self.day_sun = None;
        self.statics_changed = true;
    }

    fn put_floor(&mut self, floor: Draw, local: &[usize]) {
        self.scene.planes = vec![HdDraw::from_fixed(&floor, material::KIND_PLANE, local)];
        self.scene.planes_generation = self.scene.planes_generation.wrapping_add(1);
        self.statics_changed = true;
    }

    /// The textures of a [`TextureSet`] in the scene's table: the table's index of each of the set's.
    fn intern(&mut self, textures: Vec<Texture>) -> Vec<usize> {
        textures
            .into_iter()
            .map(|t| self.scene.textures.intern(t))
            .collect()
    }

    /// `shape` posed as `instance` is now (animate it first), placed by `transform` (unscaled), as the moving object `id`.
    pub fn shape_object(
        &mut self,
        assets: &dyn Assets,
        shape: &SceneShape,
        instance: &ShapeInstance,
        transform: &Mat,
        id: u32,
    ) -> DynamicObject {
        self.shape_object_with(
            assets,
            shape,
            instance,
            transform,
            [1.0; 3],
            id,
            &mut |_| {},
        )
    }

    /// [`WorldView::shape_object`] scaled by `scale` (along the shape's own axes, its normals turned by `transform` alone), each
    /// of its draws handed to `edit` before it joins the object (in the world: a caller may set its texture coordinates or
    /// colors from where the vertices are).
    #[allow(clippy::too_many_arguments)]
    pub fn shape_object_with(
        &mut self,
        assets: &dyn Assets,
        shape: &SceneShape,
        instance: &ShapeInstance,
        transform: &Mat,
        scale: [f32; 3],
        id: u32,
        edit: &mut dyn FnMut(&mut Draw),
    ) -> DynamicObject {
        let model = &shape.loaded.model;
        let detail = shape
            .detail()
            .and_then(|d| model.shape.details.get(d).copied());
        let unscaled = scale == [1.0; 3];
        let placed = scale_columns(transform, scale);
        let mut set = TextureSet::new(assets);
        let (mut draws, translucent) = match detail {
            Some(detail) => shape_draws(
                model,
                instance,
                ShapeParts {
                    sub_shape: detail.sub_shape,
                    object_detail: detail.object_detail,
                    hidden: &[],
                    colors: None,
                    ifl_frames: &Default::default(),
                    hidden_objects: None,
                    skin: None,
                    color_all: None,
                },
                &placed,
                transform,
                unscaled,
                &[],
                &mut set,
                &shape.path,
            ),
            None => (Vec::new(), false),
        };
        draws.iter_mut().for_each(edit);
        let local = self.intern(set.into_textures());
        let draws: Vec<HdDraw> = draws
            .iter()
            .map(|d| HdDraw::from_fixed(d, material::KIND_SHAPE, &local))
            .collect();
        let (center, radius) = bounding_sphere(&draws);
        DynamicObject {
            ghost: id,
            transform: placed,
            prev_transform: placed,
            translucent,
            centre: center,
            radius,
            draws,
            previous: None,
            stamp: None,
            nodes: Vec::new(),
            takes_decals: false,
        }
    }

    /// Draws already in the world (their textures indices into `set`) as the moving object `id`, which no animation moves (a
    /// host's own geometry; geometry that stays put is cheaper as a static mesh, [`WorldView::set_static_mesh`]).
    pub fn mesh_object(&mut self, set: TextureSet, draws: &[Draw], id: u32) -> DynamicObject {
        let local = self.intern(set.into_textures());
        mesh_object_of(&local, draws, id)
    }

    /// Puts `draws` (in the world, their textures in `set`) into the view's static geometry as the mesh `key` (a key of
    /// [`static_mesh_key`]'s), replacing the one there: the renderer uploads it once and keeps it on the GPU, culls it on the
    /// GPU by its parts' boxes and casts its shadows into the cached cascades, so a frame costs nothing for it on the CPU.
    /// Each draw is drawn as an opaque or alpha-blended (`blend` not opaque), back-face culled (clockwise front) surface.
    pub fn set_static_mesh(&mut self, key: [i32; 3], set: TextureSet, draws: &[Draw]) {
        let local = self.intern(set.into_textures());
        self.put_static_mesh(key, &local, draws);
    }

    /// [`WorldView::set_static_mesh`] with moving objects whose draws share `set`'s textures (each `(draws, id)` one object as
    /// [`WorldView::mesh_object`] makes it, for the frames to draw): a static mesh's see-through parts the translucent pass
    /// sorts one by one.
    pub fn set_static_mesh_with_objects(
        &mut self,
        key: [i32; 3],
        set: TextureSet,
        draws: &[Draw],
        objects: &[(Vec<Draw>, u32)],
    ) -> Vec<DynamicObject> {
        let local = self.intern(set.into_textures());
        self.put_static_mesh(key, &local, draws);
        objects
            .iter()
            .map(|(draws, id)| mesh_object_of(&local, draws, *id))
            .collect()
    }

    /// Puts the static mesh `key` into the scene's batch cells; whether it holds a cell now (false: nothing to draw, or the
    /// scene takes no static geometry of the view's, with a warning).
    fn put_static_mesh(&mut self, key: [i32; 3], local: &[usize], draws: &[Draw]) -> bool {
        let Some(cells) = self.scene.batch_cells() else {
            self.warn(format!("static mesh {key:?} is left out: {NO_BATCH_CELLS}"));
            return false;
        };
        let draws: Vec<HdDraw> = draws
            .iter()
            .map(|d| HdDraw::from_fixed(d, material::KIND_SHAPE, local))
            .collect();
        let mut slot_of = |texture: Option<(usize, Sampler)>| -> u32 {
            let want = SlotTexture {
                texture: texture.map(|t| t.0),
                sampler: texture.map_or(MESH_SAMPLER, |t| t.1),
            };
            if let Some((&k, _)) = cells.slots.iter().find(|(_, s)| **s == want) {
                return k;
            }
            let k = cells.slots.keys().next_back().map_or(0, |k| k + 1);
            cells.slots.insert(k, want);
            k
        };
        // One batch a slot and blending, the opaque ones first.
        let mut groups: Vec<(bool, u32, Vec<&HdDraw>)> = Vec::new();
        for d in &draws {
            let (translucent, slot) = (d.blend != Blend::Opaque, slot_of(d.texture));
            match groups
                .iter_mut()
                .find(|g| g.0 == translucent && g.1 == slot)
            {
                Some(g) => g.2.push(d),
                None => groups.push((translucent, slot, vec![d])),
            }
        }
        groups.sort_by_key(|g| (g.0, g.1));
        let mut cell = Cell {
            key,
            bounds: [[f32::INFINITY; 3], [f32::NEG_INFINITY; 3]],
            vertices: Vec::new(),
            indices: Vec::new(),
            batches: Vec::new(),
            clusters: Vec::new(),
            generation: next_stamp(),
            resident: false,
            ext: (),
        };
        for (translucent, slot, group) in &groups {
            let first = cell.indices.len() as u32;
            for d in group {
                let base = cell.vertices.len() as u32;
                cell.vertices.extend_from_slice(&d.vertices);
                cell.indices.extend(d.indices.iter().map(|i| base + i));
            }
            let count = cell.indices.len() as u32 - first;
            cell.batches.push(Batch {
                slot: *slot,
                translucent: *translucent,
                first,
                count,
            });
            // The GPU culls the batch in runs of nearby triangles (the meshes come in the order they were built in).
            let mut at = first;
            while at < first + count {
                let n = (first + count - at).min(3 * MESH_CLUSTER_TRIANGLES);
                let mut bounds = [[f32::INFINITY; 3], [f32::NEG_INFINITY; 3]];
                for &i in &cell.indices[at as usize..(at + n) as usize] {
                    let p = cell.vertices[i as usize].position;
                    for k in 0..3 {
                        bounds[0][k] = bounds[0][k].min(p[k]);
                        bounds[1][k] = bounds[1][k].max(p[k]);
                    }
                }
                cell.clusters.push(Cluster {
                    slot: *slot,
                    translucent: *translucent,
                    first: at,
                    count: n,
                    bounds,
                });
                at += n;
            }
        }
        if cell.clusters.len() > MAX_CELL_CLUSTERS {
            // One a batch, its box the mesh's.
            cell.clusters.clear();
        }
        for v in &cell.vertices {
            for k in 0..3 {
                cell.bounds[0][k] = cell.bounds[0][k].min(v.position[k]);
                cell.bounds[1][k] = cell.bounds[1][k].max(v.position[k]);
            }
        }
        let held = !cell.indices.is_empty();
        if held {
            cells.cells.insert(key, cell);
        } else {
            cells.cells.remove(&key);
        }
        self.statics_changed = true;
        held
    }

    /// Takes the static mesh `key` out of the view (nothing if there is none).
    pub fn remove_static_mesh(&mut self, key: [i32; 3]) {
        if let Some(cells) = self.scene.batch_cells()
            && cells.cells.remove(&key).is_some()
        {
            self.statics_changed = true;
        }
    }

    /// The objects of a frame's moving shapes ([`torque_render_scene::moving::draws_culled`] over `world`, their textures in
    /// `textures`): one a ghost, each moving from where the last frame's object of its ghost was.
    pub fn ghost_objects<W: ShapeWorld + ?Sized>(
        &mut self,
        world: &W,
        shaped: &ShapeDraws,
        textures: TextureSet,
    ) -> Vec<DynamicObject> {
        let local = self.intern(textures.into_textures());
        objects_of(
            world,
            shaped,
            &local,
            &mut self.scene.objects,
            Some(&mut self.kept),
        )
    }

    /// What a frame drawn by this view can show of `world`'s moving shapes ([`ViewCull`]): from `eye` (the eye the host is about
    /// to draw from, the boom's or the orbit's in a third person or an orbit), the world's camera and the eye the last frame was
    /// drawn from, their shadows, the water's reflection and the light capture included. None (nothing left out): no frame drawn
    /// yet, a cut ([`is_cut`]: the eye jumped from the last frame's), or another renderer than the engine's drawing (its frame is
    /// not the one the cull knows).
    pub fn ghost_cull<W: ShapeWorld + ?Sized>(
        &self,
        world: &W,
        eye: Option<&Eye>,
    ) -> Option<ViewCull> {
        if self.drawing != Drawing::Default {
            return None;
        }
        let records = world.records();
        let frame: &HdScene<E::Geometry> = &self.scene;
        let last = frame.eye?;
        let camera = records.eye();
        if eye.copied().or(camera).is_some_and(|e| is_cut(&last, &e)) {
            return None;
        }
        let fov = records.fov_deg();
        let eyes: Vec<(Eye, f32)> = eye
            .map(|e| (*e, e.fov.map_or(fov, f32::from)))
            .into_iter()
            .chain(camera.map(|e| (e, fov)))
            .chain(Some((last, f32::from(frame.fov))))
            .collect();
        ViewCull::new(
            frame,
            &eyes,
            self.world.as_ref().is_some_and(HdWorld::capture_on),
            self.world
                .as_ref()
                .and_then(HdWorld::shadow_range)
                .map(|r| r.end),
        )
    }

    /// The last frame's moving objects (a host takes the draws of one it hands out again as they were).
    pub(crate) fn objects_mut(&mut self) -> &mut [DynamicObject] {
        &mut self.scene.objects
    }

    /// The frame's moving objects. Each takes its motion from the last frame's object of the same id (its transform then and where
    /// its vertices were), unless it brings its own ([`carry_motion`]).
    pub fn set_objects(&mut self, mut objects: Vec<DynamicObject>) {
        carry_motion(&mut objects, &self.scene.objects);
        self.scene.objects = objects;
    }

    /// The frame's dissolving objects ([`HdScene::dissolves`]): `dissolves` run `dt` seconds on over the objects the view holds
    /// (each dissolve finds its object by its ghost; a host that runs it before [`WorldView::set_objects`] samples and emits
    /// from the last frame's objects, a frame behind its clip).
    pub fn run_dissolves(&mut self, dissolves: &mut Dissolves, dt: f32) {
        let s = &mut *self.scene;
        s.dissolves = dissolves.frame(&s.objects, &s.textures.textures, dt);
    }

    /// The frame's distorters (surfaces that bend the picture behind them, [`torque_render_scene::hd::distortion`]), their maps'
    /// indices into `textures`, in place of the last frame's. Every frame hands its own (none: an empty list).
    pub fn set_distorters(&mut self, distorters: Vec<HdDistorter>, textures: TextureSet) {
        self.scene.set_distorters(distorters, textures);
    }

    /// The frame's projected decals ([`torque_render_scene::decals::DecalManager::boxes`], their textures indices into
    /// `textures`): drawn where the renderer projects them ([`WorldView::projected_decals`]). Every frame hands its own.
    pub fn set_decals(&mut self, decals: &[HdDecal], textures: TextureSet) {
        self.scene.set_decals(decals, textures);
    }

    /// Whether the frames project the decals ([`HdWorld::projected_decals_at`], at the view's quality: Medium and up) instead of
    /// drawing the CPU path's triangles. `TORQUE_PROJECTED_DECALS=1` in the environment projects them for the run at every
    /// quality and `TORQUE_PROJECTED_DECALS=0` at none (the two paths' pictures compared; set on the world as the view takes
    /// it). False with no renderer.
    pub fn projected_decals(&self) -> bool {
        self.world
            .as_ref()
            .is_some_and(|w| w.projected_decals_at(self.quality))
    }

    /// More distorters this frame, after those set ([`WorldView::set_distorters`]).
    pub fn add_distorters(&mut self, distorters: Vec<HdDistorter>, textures: TextureSet) {
        self.scene.add_distorters(distorters, textures);
    }

    /// The frame's particles: each draw at its eye distance, its texture an index into `textures` (the set the emitters drew
    /// with). Every frame hands its own (none: an empty list).
    pub fn set_particles(&mut self, draws: &[(f32, Draw)], textures: TextureSet) {
        self.scene.set_particles(draws, textures);
    }

    /// Waits for the GPU and returns the last frame's GPU time in ms (`None` without timestamps).
    pub fn gpu_ms(&mut self) -> Option<f64> {
        self.world.as_mut()?.gpu_ms()
    }

    /// Which particle draws the enhanced renderer lights (none: all unlit, as TGE draws them; [`ParticleLight::BLENDED`]: smoke
    /// and dust lit, glows not), from the next frame's particles on.
    pub fn set_particle_light(&mut self, rule: Option<ParticleLight>) {
        self.particle_light = rule;
        let lit = self.features.particle_light;
        self.scene.particle_light = rule.filter(|_| lit);
    }

    /// Whether the frames hand their emitters' particles to the renderer's GPU particle pool ([`HdWorld::gpu_particles_at`], at
    /// the view's quality) instead of drawing them on the CPU. `TORQUE_GPU_PARTICLES=1` in the environment turns the pool on for the
    /// run at every quality and `TORQUE_GPU_PARTICLES=0` off (the two paths' pictures compared); else it runs at Medium and up.
    pub fn gpu_particles(&mut self) -> bool {
        let Some(world) = &mut self.world else {
            return false;
        };
        match gpu_particles_forced() {
            Some(true) => world.features.gpu_particles = ParticlePool::On,
            Some(false) => world.features.gpu_particles = ParticlePool::Off,
            None => {}
        }
        world.gpu_particles_at(self.quality)
    }

    /// [`WorldView::set_particles_and_decals`] with the frame's hand-over to the GPU particle pool (its looks' textures indices
    /// into the same `textures`; None: nothing handed over).
    pub fn set_particles_decals_and_sprites(
        &mut self,
        particles: &[(f32, Draw)],
        decals: &[(f32, Draw)],
        sprites: Option<&SpriteFrame>,
        textures: TextureSet,
    ) {
        self.scene
            .set_particles_decals_and_sprites(particles, decals, sprites, textures);
    }

    /// The frame's particles and decals (the decals' draws lie on surfaces: not faded into them as the particles are), their
    /// textures indices into `textures`.
    pub fn set_particles_and_decals(
        &mut self,
        particles: &[(f32, Draw)],
        decals: &[(f32, Draw)],
        textures: TextureSet,
    ) {
        self.scene
            .set_particles_and_decals(particles, decals, textures);
    }

    /// Draws a frame into `target` (an `Rgba8Unorm` texture of the canvas's size): the scene seen from `eye` in the world
    /// rectangle of `gui` (the first `GameTSCtrl` drawn), then `gui` over it. Returns whether the world was drawn.
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        gui: &GuiDraws,
        eye: Eye,
    ) -> Result<bool, String> {
        let texture = target.texture();
        let (width, height) = (texture.width(), texture.height());
        self.follow_slot();
        // The engine's renderer drawing with its world released: the app drew the look itself, and the GUI goes over it.
        let own_look = self.drawing == Drawing::Default && self.world.is_none();
        let world = gui
            .worlds
            .first()
            .filter(|_| self.drawing != Drawing::None && !own_look)
            .and_then(|w| {
                let [x, y, w, h] = w.rect;
                let x0 = u32::try_from(x).ok()?.min(width);
                let y0 = u32::try_from(y).ok()?.min(height);
                let w = u32::try_from(w).ok()?.min(width - x0);
                let h = u32::try_from(h).ok()?.min(height - y0);
                (w > 0 && h > 0).then_some([x0, y0, w, h])
            });
        if let Some(rect) = world {
            self.draw_world(target, rect, eye)?;
        }
        let clear = (world.is_none() && !own_look).then_some([0.0, 0.0, 0.0, 1.0]);
        self.gui
            .get_or_insert_with(|| GuiPass::new(gpu, None))
            .draw(gpu, gui, target, clear);
        Ok(world.is_some())
    }

    /// Draws the scene seen from `eye` into the rectangle `[x, y, width, height]` of `target` (an `Rgba8Unorm` texture).
    pub fn draw_world(
        &mut self,
        target: &wgpu::TextureView,
        [x, y, w, h]: [u32; 4],
        eye: Eye,
    ) -> Result<(), String> {
        self.follow_slot();
        if self.drawing == Drawing::None {
            return Err("no renderer: the build or the engine has none".to_string());
        }
        if !self.batch_sources.is_empty() {
            self.follow_batch_sources();
        }
        let fov = eye.fov.unwrap_or(DEFAULT_FOV);
        let s = &mut self.scene;
        s.eye = Some(eye);
        s.fov = fov;
        s.size = (w, h);
        s.generation += 1;
        if !std::mem::take(&mut self.statics_changed) {
            // Only the objects moved: the renderer sends the draws again and keeps the rest.
            s.control_generation += 1;
        }
        let camera = HdCamera::new(eye, fov, s.visible_distance, w, h);
        #[cfg_attr(not(feature = "gizmos"), allow(unused_variables))]
        let own = if let (Drawing::Other(i), Some((slot, _))) = (self.drawing, &self.slot) {
            // The slot refuses other renderers on a game extension's view, so the scene is the engine's here.
            let scene = (&self.scene as &dyn Any).downcast_ref::<PlainScene>();
            debug_assert!(
                scene.is_some(),
                "another renderer on a game extension's view"
            );
            let scene = scene.ok_or("the renderer cannot draw this view's scene")?;
            slot.draw(
                i,
                RenderFrame {
                    scene,
                    camera: &camera,
                    target,
                    rect: [x, y, w, h],
                },
            )?;
            false
        } else {
            let world = self
                .world
                .as_mut()
                .ok_or("the view's GPU world is released")?;
            world.upload(&self.scene);
            let format = target.texture().format();
            world
                .draw(&self.scene, &camera, Some((target, format, [x, y])))
                .map_err(|e| format!("the world: {e}"))?;
            true
        };
        // The debug gizmos over the finished world; the engine's renderer has a depth to hide them behind.
        #[cfg(feature = "gizmos")]
        if let Some(painter) = &mut self.gizmos {
            let depth = own
                .then(|| self.world.as_ref().and_then(HdWorld::depth_view))
                .flatten();
            painter.paint(&self.gpu, target, [x, y, w, h], &camera, depth);
        }
        Ok(())
    }

    /// The scene alone seen from `eye`, drawn offscreen at `size` and read back.
    pub fn render_world(
        &mut self,
        gpu: &Gpu,
        size: (u32, u32),
        eye: Eye,
    ) -> Result<torque_render_core::Frame, String> {
        let target = gpu.offscreen(size.0, size.1).map_err(|e| e.to_string())?;
        self.draw_world(&target.color, [0, 0, size.0, size.1], eye)?;
        gpu.read_back(&target).map_err(|e| e.to_string())
    }
}

/// The effects' clock starts over here (seconds: about 68 minutes; a power of two, so the wrap is exact). The wrap is not
/// seamless: the water's waves (`omega * t`), the sky's twinkle and the clouds' fallback drift jump once per wrap, and the
/// frame at the wrap shows one frame of spurious motion vectors. 4,096 s keeps the `f32` the shaders get within 0.25 ms of
/// the clock.
pub const EFFECT_TIME_WRAP: f64 = 4_096.0;

/// The effects' clock: exact seconds in an `f64`, wrapped at [`EFFECT_TIME_WRAP`]. Only the wrapped value is narrowed for the
/// GPU, so a frame's step is not rounded to the `f32` grid at every addition (which drifts the clock's rate when the steps are
/// small against that grid, as at a high refresh rate).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EffectClock {
    seconds: f64,
}

impl EffectClock {
    /// The clock run on by `seconds`, a frame's time: a step that is not a positive finite number leaves it.
    pub fn advance(&mut self, seconds: f32) {
        if seconds.is_finite() && seconds > 0.0 {
            self.seconds = (self.seconds + f64::from(seconds)) % EFFECT_TIME_WRAP;
        }
    }

    /// The clock as the shaders take it, in `[0, EFFECT_TIME_WRAP)`.
    pub fn gpu(self) -> f32 {
        self.seconds as f32
    }
}

/// A floor's two triangles at height 0, `half` each way from the origin, tinted `color`, with texture `texture` of the frame's
/// set (if any) repeated every `tile` units.
fn floor_draw(texture: Option<usize>, color: [u8; 3], half: f32, tile: f32) -> Draw {
    let corner = |x: f32, y: f32| Vertex {
        position: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        color: [color[0], color[1], color[2], 255],
        uv: [x / tile, y / tile],
    };
    let (a, b, c, d) = (
        corner(-half, -half),
        corner(half, -half),
        corner(half, half),
        corner(-half, half),
    );
    let sampler = Sampler {
        repeat: true,
        mag: Filter::Linear,
        min: Filter::Linear,
        mip: Filter::Linear,
        mipmaps: true,
    };
    Draw {
        label: "floor".to_string(),
        vertices: vec![a, b, c, a, c, d],
        lighting: true,
        tex_env: if texture.is_some() {
            TexEnv::Modulate
        } else {
            TexEnv::None
        },
        texture: texture.map(|i| (i, sampler)),
        blend: Blend::Opaque,
        cull: Cull::None,
        depth_test: true,
        depth_write: true,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    }
}

/// The water zone a camera is under water in ([`torque_render_scene::hd::WaterZone`]): a sheet's box from its bottom up to its
/// surface over its rectangle, tinted with its color at its `underwater` share. None for a sheet that is not finite numbers,
/// has no height or rectangle, or takes no tint.
pub fn underwater_zone(w: &Water<'_>) -> Option<WaterZone> {
    let share = w.underwater.clamp(0.0, 1.0);
    let flat = (0..2).any(|k| w.min[k] >= w.max[k]);
    let finite = w.is_finite() && w.bottom.is_finite() && share.is_finite();
    if !finite || share <= 0.0 || w.bottom >= w.level || flat {
        return None;
    }
    let c = w.color.map(|b| f32::from(b) / 255.0);
    Some(WaterZone {
        min: [w.min[0], w.min[1], w.bottom],
        max: [w.max[0], w.max[1], w.level],
        tint: [c[0], c[1], c[2], share],
    })
}

/// A sheet of water's draw: its rectangle cut into cells of at most [`WATER_CELL`] units, blended, both sides drawn, no depth
/// written (the renderer draws a blended plane as water).
fn water_draw(texture: Option<usize>, w: &Water<'_>) -> Draw {
    let cells = |len: f32| ((len.abs() / WATER_CELL).ceil() as usize).clamp(1, WATER_MAX_CELLS);
    let size = [w.max[0] - w.min[0], w.max[1] - w.min[1]];
    let (nx, ny) = (cells(size[0]), cells(size[1]));
    let tile = w.tile.max(1e-3);
    let point = |i: usize, j: usize| {
        let x = w.min[0] + size[0] * i as f32 / nx as f32;
        let y = w.min[1] + size[1] * j as f32 / ny as f32;
        Vertex {
            position: [x, y, w.level],
            normal: [0.0, 0.0, 1.0],
            color: w.color,
            uv: [x / tile, y / tile],
        }
    };
    let mut vertices = Vec::with_capacity(nx * ny * 6);
    for j in 0..ny {
        for i in 0..nx {
            let q = [
                point(i, j),
                point(i + 1, j),
                point(i + 1, j + 1),
                point(i, j + 1),
            ];
            vertices.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
        }
    }
    Draw {
        label: "water".to_string(),
        vertices,
        lighting: true,
        tex_env: if texture.is_some() {
            TexEnv::Modulate
        } else {
            TexEnv::None
        },
        texture: texture.map(|i| (i, MESH_SAMPLER)),
        blend: Blend::Alpha,
        cull: Cull::None,
        depth_test: true,
        depth_write: false,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    }
}

/// A static mesh object of `draws` (their textures indices into `local`, the frame's table): translucent when a draw blends.
fn mesh_object_of(local: &[usize], draws: &[Draw], id: u32) -> DynamicObject {
    let translucent = draws.iter().any(|d| d.blend != Blend::Opaque);
    let draws: Vec<HdDraw> = draws
        .iter()
        .map(|d| HdDraw::from_fixed(d, material::KIND_SHAPE, local))
        .collect();
    let (centre, radius) = bounding_sphere(&draws);
    DynamicObject {
        ghost: id,
        transform: Mat::IDENTITY,
        prev_transform: Mat::IDENTITY,
        translucent,
        centre,
        radius,
        draws,
        previous: None,
        stamp: None,
        nodes: Vec::new(),
        takes_decals: false,
    }
}

/// Whether the environment projects the decals (`TORQUE_PROJECTED_DECALS=1`) or not (`0`) for the run, read once.
fn projected_decals_forced() -> Option<DecalProjection> {
    static FORCED: std::sync::OnceLock<Option<DecalProjection>> = std::sync::OnceLock::new();
    *FORCED.get_or_init(|| match std::env::var_os("TORQUE_PROJECTED_DECALS") {
        Some(v) if v == "1" => Some(DecalProjection::On),
        Some(v) if v == "0" => Some(DecalProjection::Off),
        _ => None,
    })
}

/// `world`'s decal projection as the environment forces it for the run ([`projected_decals_forced`]); as it is otherwise.
fn force_decals<E: HdExtension>(world: &mut HdWorld<E>) {
    if let Some(forced) = projected_decals_forced() {
        world.features.projected_decals = forced;
    }
}

/// Whether the environment turns the GPU particle pool on (`TORQUE_GPU_PARTICLES=1`) or off (`0`) for the run, read once.
fn gpu_particles_forced() -> Option<bool> {
    static FORCED: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    *FORCED.get_or_init(|| match std::env::var_os("TORQUE_GPU_PARTICLES") {
        Some(v) if v == "1" => Some(true),
        Some(v) if v == "0" => Some(false),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_effect_clock_runs_wraps_and_ignores_steps_that_are_not_forward() {
        let mut clock = EffectClock::default();
        clock.advance(1.5);
        assert_eq!(clock.gpu(), 1.5);
        clock.advance(-1.0);
        clock.advance(f32::NAN);
        clock.advance(f32::INFINITY);
        assert_eq!(clock.gpu(), 1.5);
        clock.advance(EFFECT_TIME_WRAP as f32 - 1.0);
        assert!((0.0..1.0).contains(&clock.gpu()), "{}", clock.gpu());
    }

    /// Two hours of frames, across the wrap: the clock the shaders take still runs at the frame rate (its advance over a second
    /// of frames within 1% of a second), at 60 Hz and at a high refresh rate whose step is small against the `f32` grid near
    /// the wrap.
    #[test]
    fn the_effect_clock_keeps_its_rate_over_two_hours_of_frames() {
        for fps in [60u32, 144, 240] {
            let step = 1.0 / fps as f32;
            let mut clock = EffectClock::default();
            let (mut worst, mut last) = (0.0_f64, clock.gpu());
            for frame in 1..=(2 * 3600 * fps) {
                clock.advance(step);
                if frame % fps == 0 {
                    let now = clock.gpu();
                    let ran = (f64::from(now) - f64::from(last)).rem_euclid(EFFECT_TIME_WRAP);
                    worst = worst.max((ran - 1.0).abs());
                    last = now;
                }
            }
            assert!(
                worst < 0.01,
                "{fps} Hz: a second of frames ran the clock {worst} off 1 s"
            );
        }
    }

    /// A sheet of water from a hostile map: a height, a corner or a tile that is not a finite number (or a span that overflows)
    /// is refused; a plain one is drawn as cells of its rectangle.
    #[test]
    fn water_with_numbers_that_are_not_finite_is_refused() {
        let plain = Water {
            texture: "kit/water.png",
            tile: 24.0,
            color: [64, 112, 120, 200],
            level: 30.0,
            min: [-100.0, -50.0],
            max: [100.0, 50.0],
            bottom: 0.0,
            underwater: 0.3,
        };
        assert!(plain.is_finite());
        let d = water_draw(None, &plain);
        assert_eq!(d.vertices.len(), 7 * 4 * 6);
        assert!(d.vertices.iter().all(|v| v.position[2] == 30.0));
        for hostile in [
            Water {
                level: f32::NAN,
                ..plain
            },
            Water {
                tile: f32::INFINITY,
                ..plain
            },
            Water {
                min: [f32::NEG_INFINITY, 0.0],
                ..plain
            },
            Water {
                max: [0.0, f32::NAN],
                ..plain
            },
            Water {
                min: [-3e38, 0.0],
                max: [3e38, 1.0],
                ..plain
            },
        ] {
            assert!(!hostile.is_finite(), "{hostile:?}");
            assert_eq!(underwater_zone(&hostile), None, "{hostile:?}");
        }
    }

    /// A sheet of water is a zone the view is under water in: its box from its bottom up to its surface over its rectangle,
    /// tinted its color at its underwater share; none without a share, a depth or a finite bottom.
    #[test]
    fn a_sheet_of_water_yields_its_underwater_zone() {
        let lake = Water {
            texture: "kit/water.png",
            tile: 24.0,
            color: [51, 102, 153, 200],
            level: 30.0,
            min: [-100.0, -50.0],
            max: [100.0, 50.0],
            bottom: 0.0,
            underwater: 0.3,
        };
        let zone = underwater_zone(&lake).expect("a zone");
        assert_eq!(zone.min, [-100.0, -50.0, 0.0]);
        assert_eq!(zone.max, [100.0, 50.0, 30.0]);
        assert_eq!(zone.tint, [0.2, 0.4, 0.6, 0.3]);
        assert!(zone.holds([0.0, 0.0, 29.9]) && !zone.holds([0.0, 0.0, 30.0]));
        assert!(!zone.holds([0.0, 60.0, 10.0]));
        assert_eq!(
            underwater_zone(&Water {
                underwater: 2.0,
                ..lake
            })
            .map(|z| z.tint[3]),
            Some(1.0)
        );
        for none in [
            Water {
                underwater: 0.0,
                ..lake
            },
            Water {
                underwater: f32::NAN,
                ..lake
            },
            Water {
                bottom: 30.0,
                ..lake
            },
            Water {
                bottom: f32::NEG_INFINITY,
                ..lake
            },
            Water {
                max: [-100.0, 50.0],
                ..lake
            },
        ] {
            assert_eq!(underwater_zone(&none), None, "{none:?}");
        }
    }
}
