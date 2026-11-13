//! The renderer slot: the renderers an engine has, each registered by a plugin under an id, and the one a client's world
//! view draws with, which the game can swap while it runs.
//!
//! | Plugin | Id | Cargo feature | What it adds |
//! |--------|----|---------------|--------------|
//! | [`RendererSlotPlugin`] | `torque.renderer-slot` | - | each client's [`RendererSlot`], shared with its world view, and the console functions `setRenderer(id)` and `getRenderer()` |
//! | [`RendererPlugin`] | `torque.renderer` | `renderer` | the engine's renderer, registered as [`DEFAULT_RENDERER`] (`default`), drawn with what the [`RenderFeaturePlugins`] turn on |
//!
//! Both are in [`DefaultPlugins`](crate::plugins::DefaultPlugins), with the [`RenderFeaturePlugins`] group.
//!
//! **Registering a renderer.** Any plugin adds one under an id ([`EngineRenderers::add_renderer_id`], or
//! [`EngineRenderers::add_renderer`] for a [`NamedRenderer`] type): a function that makes it on the view's GPU, and a
//! [`WorldRenderer`] that draws the view's scene each frame. A client starts on [`DEFAULT_RENDERER`] when its engine has it,
//! else on the first renderer registered. With none the world is not drawn (the GUI is) and the client's console says why.
//!
//! **Hot swapping.** `setRenderer(id)` from a script, [`RendererSlot::switch`] or
//! [`WorldView::set_renderer`](crate::view::WorldView::set_renderer) from Rust: the world view draws with that renderer from
//! its next frame. An id no plugin registered is refused, naming the registered ones. A renderer is made on the view's GPU
//! the first time it is chosen; when that fails the swap is refused with the reason and the renderer drawing stays (choosing
//! the id again tries again). A made renderer is kept while the client lives, so swapping back is immediate and the GPU memory
//! is bounded by the registered renderers. A view follows the slot of the client it draws once it is attached
//! ([`WorldView::attach_to`](crate::view::WorldView::attach_to)); the window and offscreen runners attach it. A swap chosen
//! before then (a script at start-up) is made when the view attaches. Between swaps a frame only compares one counter.
//!
//! **A game's extension.** A world view that draws a game's own render extension (`WorldView<E>` with `E` not `NoExtension`)
//! is drawn by the engine's renderer only: the others draw the engine's plain scene, which carries none of the game's data. Its
//! slot refuses a swap to any other ([`SwitchError::Extension`], naming the extension), and one chosen before the view attached
//! gives way to the engine's own, with the reason on the client's console.
//!
//! **The engine's renderer's features** are plugins of their own ([`RenderFeaturePlugins`]): each turns one of its switches on
//! (`torque_render_hd::HdFeatures` and the water's and particles' looks), so a game drops one by turning its plugin off
//! (`DefaultPlugins.disable::<CloudsPlugin>()`). With all of them on the renderer draws as it always has.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::Rc;

use torque_engine::{ClientStage, EngineBuilder, Plugin, PluginId, VmMut, VmRef};
use torque_render_core::{Gpu, wgpu};
use torque_render_hd::HdFeatures;
use torque_render_hd::ext::PlainScene;
use torque_render_scene::hd::post::{Upscaler, Upscaling};
use torque_render_scene::hd::{HdCamera, HdMode, ImageSettings, Quality};
use torque_ts_vm::vm::Vm;

use crate::render::RenderAdds;

/// The id of the engine's own renderer ([`RendererPlugin`]).
pub const DEFAULT_RENDERER: &str = "default";

/// What a renderer draws a frame from: the view's scene seen through `camera`, into the rectangle `rect` (`[x, y, width,
/// height]`) of `target` (an `Rgba8Unorm` texture).
pub struct RenderFrame<'a> {
    pub scene: &'a PlainScene,
    pub camera: &'a HdCamera,
    pub target: &'a wgpu::TextureView,
    pub rect: [u32; 4],
}

/// A renderer on the slot other than the engine's own: draws the world view's scene each frame it is chosen.
pub trait WorldRenderer: 'static {
    /// Draws `frame`.
    fn draw(&mut self, frame: RenderFrame<'_>) -> Result<(), String>;
}

/// A renderer type with its id and its constructor: what [`EngineRenderers::add_renderer`] registers.
pub trait NamedRenderer: WorldRenderer + Sized {
    /// The id scripts name it by (`setRenderer(id)`).
    const ID: &'static str;

    /// The renderer on `gpu` (Err: the device cannot draw it).
    fn new(gpu: &Gpu) -> Result<Self, String>;
}

/// Makes a registered renderer on the view's GPU.
pub type MakeRenderer = dyn Fn(&Gpu) -> Result<Box<dyn WorldRenderer>, String>;

/// A renderer registered on the engine: its id and how it is made (None: the engine's own, which the view keeps).
#[derive(Clone)]
pub struct RendererEntry {
    id: String,
    make: Option<Rc<MakeRenderer>>,
}

impl RendererEntry {
    /// Its id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// How it is made (None: the engine's own renderer).
    pub fn make(&self) -> Option<&Rc<MakeRenderer>> {
        self.make.as_ref()
    }
}

impl fmt::Debug for RendererEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RendererEntry")
            .field("id", &self.id)
            .field("default", &self.make.is_none())
            .finish()
    }
}

/// The engine builder's calls that register renderers on the slot.
pub trait EngineRenderers {
    /// Registers the renderer `make` makes under `id` (ASCII letters, digits, `_`, `-`, `.` and `:`). A second renderer
    /// under an id already registered is skipped, with a warning on each client's console.
    fn add_renderer_id(
        &mut self,
        id: &str,
        make: impl Fn(&Gpu) -> Result<Box<dyn WorldRenderer>, String> + 'static,
    ) -> &mut EngineBuilder;

    /// Registers the renderer type `R` under its id.
    fn add_renderer<R: NamedRenderer>(&mut self) -> &mut EngineBuilder;
}

impl EngineRenderers for EngineBuilder {
    fn add_renderer_id(
        &mut self,
        id: &str,
        make: impl Fn(&Gpu) -> Result<Box<dyn WorldRenderer>, String> + 'static,
    ) -> &mut EngineBuilder {
        let make: Rc<MakeRenderer> = Rc::new(make);
        self.push(RendererEntry {
            id: id.to_owned(),
            make: Some(make),
        })
    }

    fn add_renderer<R: NamedRenderer>(&mut self) -> &mut EngineBuilder {
        self.add_renderer_id(R::ID, |gpu| {
            let renderer: Box<dyn WorldRenderer> = Box::new(R::new(gpu)?);
            Ok(renderer)
        })
    }
}

/// What the engine's renderer draws: its switches and the looks of the water and the particles, as the
/// [`RenderFeaturePlugins`] turned them on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderFeatures {
    pub features: HdFeatures,
    /// The water clear, foaming where it is shallow ([`ImageSettings::clear_water`]); off: its texture stays its face.
    pub clear_water: bool,
    /// Smoke and dust lit by the sun, the sky and the lights (the view's particle light rule); off: unlit.
    pub particle_light: bool,
}

impl RenderFeatures {
    /// None of the feature plugins' switches on (the rest as [`HdFeatures::default`]): what the renderer draws with when the
    /// game turned every feature plugin off.
    pub const NONE: RenderFeatures = RenderFeatures {
        features: HdFeatures {
            sun_shadows: false,
            point_shadows: false,
            contact_shadows: false,
            anti_aliasing: false,
            ambient_occlusion: false,
            reflections: false,
            bloom: false,
            volumetrics: false,
            wboit: false,
            grading_lut: false,
            ext_detail: false,
            clouds: false,
            glow_lights: false,
            ..FEATURES_DEFAULT
        },
        clear_water: false,
        particle_light: false,
    };

    /// Every feature plugin's switches on: the renderer as it always draws ([`HdFeatures::default`], clear water, lit
    /// particles). A world view without an engine draws with it.
    pub const ALL: RenderFeatures = RenderFeatures {
        features: FEATURES_DEFAULT,
        clear_water: true,
        particle_light: true,
    };

    /// The renderer's mode at `quality` with these features.
    pub fn mode(&self, quality: Quality) -> HdMode {
        HdMode {
            image: ImageSettings {
                clear_water: self.clear_water,
                ..ImageSettings::DEFAULT
            },
            quality,
        }
    }
}

/// The engine's renderer's prefs (the client's script engine has them; [`WorldView::read_prefs`](crate::view::WorldView::read_prefs)
/// reads them).
#[derive(torque_engine::Prefs)]
#[prefs(prefix = "$Pref::Render")]
pub struct RenderPrefs {
    /// The upscaler High and Ultra draw with: `off` (every pixel of the window, the default) or `temporal` (a smaller frame
    /// brought up to the window's size by the temporal upscaler). Any other word is off.
    #[pref(default = "off")]
    pub upscaler: String,
    /// The share of the window's pixels a side the upscaler draws: 0 for the quality's own (0.8 at High, 0.83 at Ultra), else
    /// held to 0.5 to 1.
    #[pref(default = 0.0, range = 0.0..=1.0)]
    pub upscale_scale: f32,
}

impl RenderPrefs {
    /// The renderer's upscaling setting these prefs ask for.
    #[must_use]
    pub fn upscaling(&self) -> Upscaling {
        Upscaling {
            upscaler: Upscaler::named(&self.upscaler).unwrap_or_default(),
            scale: (self.upscale_scale > 0.0).then_some(self.upscale_scale),
        }
    }
}

/// [`HdFeatures::default`] as a constant.
const FEATURES_DEFAULT: HdFeatures = HdFeatures {
    sun_shadows: true,
    point_shadows: true,
    contact_shadows: true,
    anti_aliasing: true,
    ambient_occlusion: true,
    reflections: true,
    bloom: true,
    volumetrics: true,
    wboit: true,
    grading_lut: true,
    coloured_shadows: true,
    ext_detail: true,
    clouds: true,
    opt_in_clouds: false,
    cloud_layers: u32::MAX,
    glow_lights: true,
    cloud_sun: true,
    fog_sky_mean: true,
    cloud_from_eye: true,
    cloud_cube: true,
    gpu_particles: torque_render_hd::ParticlePool::Auto,
    froxel_fog: true,
    projected_decals: torque_render_hd::DecalProjection::Auto,
};

/// Why a swap of renderers was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchError {
    /// No renderer has the id.
    Unknown {
        /// The id asked for.
        id: String,
        /// The ids registered, in the engine's order.
        registered: Vec<String>,
    },
    /// The renderer could not be made on the view's GPU.
    Failed {
        /// Its id.
        id: String,
        /// Why.
        why: String,
    },
    /// The attached view draws a game's extension, which only the engine's renderer draws ([`DEFAULT_RENDERER`]).
    Extension {
        /// The id asked for.
        id: String,
        /// The extension's type name.
        extension: &'static str,
    },
}

impl fmt::Display for SwitchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SwitchError::Unknown { id, registered } => {
                write!(f, "no renderer {id:?} (registered: ")?;
                if registered.is_empty() {
                    f.write_str("none")?;
                } else {
                    f.write_str(&registered.join(", "))?;
                }
                f.write_str(")")
            }
            SwitchError::Failed { id, why } => {
                write!(f, "the renderer {id:?} could not be made: {why}")
            }
            SwitchError::Extension { id, extension } => write!(
                f,
                "the renderer {id:?} cannot draw this view: its world has the game extension {extension}, which only \
                 the engine's renderer ({DEFAULT_RENDERER:?}) draws"
            ),
        }
    }
}

impl std::error::Error for SwitchError {}

/// One client's renderers: those its engine registered, the one drawing, the ones made so far and the engine's renderer's
/// features. Its natives swap it; the world view attached to it follows from its next frame. A renderer is made on the view's
/// GPU the first time it is chosen and kept while the client lives. A client's script engine keeps the slot as an extension.
pub struct RendererSlot {
    entries: Vec<RendererEntry>,
    current: Cell<Option<usize>>,
    features: RenderFeatures,
    /// Counts the swaps: a view follows when it changed.
    generation: Cell<u64>,
    /// The attached view's GPU (None until a view attaches: a swap before then is made at the attach).
    gpu: RefCell<Option<Gpu>>,
    /// The game extension the attached view draws (its type name): the slot then refuses every renderer but the engine's.
    extension: Cell<Option<&'static str>>,
    /// The renderers made so far, by entry.
    made: RefCell<Vec<Option<Box<dyn WorldRenderer>>>>,
    /// The passes, static batches and emitter kinds the engine's plugins added ([`crate::render`]).
    adds: RenderAdds,
}

impl fmt::Debug for RendererSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RendererSlot")
            .field("entries", &self.entries)
            .field("current", &self.current.get())
            .field("generation", &self.generation.get())
            .finish_non_exhaustive()
    }
}

impl RendererSlot {
    /// A slot with the renderers `entries` (in the engine's order; a repeated or invalid id is left out, and each one left out
    /// is in the second value) and the engine's renderer's `features`, drawing with [`DEFAULT_RENDERER`] when it is one of
    /// them, else with the first.
    pub fn new(entries: &[RendererEntry], features: RenderFeatures) -> (RendererSlot, Vec<String>) {
        let mut kept: Vec<RendererEntry> = Vec::new();
        let mut warnings = Vec::new();
        for e in entries {
            if !PluginId::is_valid(&e.id) {
                warnings.push(format!("a renderer's id {:?} is not valid: left out", e.id));
            } else if kept.iter().any(|k| k.id == e.id) {
                warnings.push(format!(
                    "a second renderer registered as {:?}: the first is kept",
                    e.id
                ));
            } else {
                kept.push(e.clone());
            }
        }
        let current = kept
            .iter()
            .position(|e| e.id == DEFAULT_RENDERER)
            .or((!kept.is_empty()).then_some(0));
        let made = RefCell::new(kept.iter().map(|_| None).collect());
        let slot = RendererSlot {
            entries: kept,
            current: Cell::new(current),
            features,
            generation: Cell::new(0),
            gpu: RefCell::new(None),
            extension: Cell::new(None),
            made,
            adds: RenderAdds::default(),
        };
        (slot, warnings)
    }

    /// The slot with what the engine's plugins added to the rendering ([`crate::render`]): the world view attached to it makes
    /// its own of each.
    pub fn with_adds(mut self, adds: RenderAdds) -> RendererSlot {
        self.adds = adds;
        self
    }

    /// What the engine's plugins added to the rendering.
    pub fn adds(&self) -> &RenderAdds {
        &self.adds
    }

    /// The registered renderers' ids, in the engine's order.
    pub fn ids(&self) -> impl ExactSizeIterator<Item = &str> + '_ {
        self.entries.iter().map(|e| e.id.as_str())
    }

    /// Whether a renderer has the id `id`.
    pub fn has(&self, id: &str) -> bool {
        self.entries.iter().any(|e| e.id == id)
    }

    /// The renderer drawing (None: the engine has none).
    pub fn current(&self) -> Option<&str> {
        self.current_entry().map(RendererEntry::id)
    }

    /// The entry of the renderer drawing.
    pub fn current_entry(&self) -> Option<&RendererEntry> {
        self.entries.get(self.current.get()?)
    }

    /// What the engine's renderer draws with.
    pub fn features(&self) -> RenderFeatures {
        self.features
    }

    /// Draws with the renderer `id` from the view's next frame, made now if a view is attached and it was never made. Err: no
    /// renderer has the id, it could not be made (the renderer drawing stays; choosing it again tries again), or it is not the
    /// engine's and the attached view draws a game extension.
    pub fn switch(&self, id: &str) -> Result<(), SwitchError> {
        let Some(index) = self.entries.iter().position(|e| e.id == id) else {
            return Err(SwitchError::Unknown {
                id: id.to_owned(),
                registered: self.ids().map(str::to_owned).collect(),
            });
        };
        if self.current.get() == Some(index) {
            return Ok(());
        }
        self.refuse_on_extension(index)?;
        self.make(index)?;
        self.current.set(Some(index));
        self.generation.set(self.generation.get() + 1);
        Ok(())
    }

    /// [`switch`](Self::switch) to the renderer type `R`.
    pub fn switch_to<R: NamedRenderer>(&self) -> Result<(), SwitchError> {
        self.switch(R::ID)
    }

    /// Counts the swaps.
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// The slot of the client whose script engine is `vm` (None: its engine has no [`RendererSlotPlugin`]).
    pub fn of(vm: &Vm) -> Option<Rc<RendererSlot>> {
        vm.extension::<RendererSlot>()
    }

    /// Makes the renderer of entry `index` on the attached view's GPU, unless it is the engine's own, already made or no view
    /// is attached yet.
    fn make(&self, index: usize) -> Result<(), SwitchError> {
        let entry = &self.entries[index];
        let Some(make) = entry.make() else {
            return Ok(());
        };
        if self.made.borrow()[index].is_some() {
            return Ok(());
        }
        let Some(gpu) = self.gpu.borrow().clone() else {
            return Ok(());
        };
        let renderer = make(&gpu).map_err(|why| SwitchError::Failed {
            id: entry.id.clone(),
            why,
        })?;
        self.made.borrow_mut()[index] = Some(renderer);
        Ok(())
    }

    /// Err when the attached view draws a game extension and entry `index` is not the engine's renderer.
    fn refuse_on_extension(&self, index: usize) -> Result<(), SwitchError> {
        let entry = &self.entries[index];
        match self.extension.get() {
            Some(extension) if entry.make().is_some() => Err(SwitchError::Extension {
                id: entry.id.clone(),
                extension,
            }),
            _ => Ok(()),
        }
    }

    /// A view on `gpu` attached, drawing the game extension `extension` (its type name; None: the engine's plain cells): the
    /// renderers are made on it. The renderer chosen before, if it cannot be made or the view's extension refuses it, gives
    /// way to [`DEFAULT_RENDERER`] (else to none); the error is returned for the view to report.
    pub(crate) fn attach_gpu(
        &self,
        gpu: &Gpu,
        extension: Option<&'static str>,
    ) -> Result<(), SwitchError> {
        *self.gpu.borrow_mut() = Some(gpu.clone());
        self.extension.set(extension);
        let Some(index) = self.current.get() else {
            return Ok(());
        };
        let Err(e) = self
            .refuse_on_extension(index)
            .and_then(|()| self.make(index))
        else {
            return Ok(());
        };
        let fallback = self
            .entries
            .iter()
            .position(|e| e.id == DEFAULT_RENDERER && e.make().is_none());
        self.current.set(fallback);
        self.generation.set(self.generation.get() + 1);
        Err(e)
    }

    /// Draws `frame` with the made renderer of entry `index`.
    pub(crate) fn draw(&self, index: usize, frame: RenderFrame<'_>) -> Result<(), String> {
        match self.made.borrow_mut().get_mut(index) {
            Some(Some(renderer)) => renderer.draw(frame),
            _ => Err(format!(
                "the renderer {:?} was not made",
                self.entries.get(index).map_or("", RendererEntry::id)
            )),
        }
    }

    /// The place of the renderer drawing among the entries.
    pub(crate) fn current_index(&self) -> Option<usize> {
        self.current.get()
    }
}

/// The renderer slot: gives each client a [`RendererSlot`] over the renderers the engine's plugins registered and the
/// engine's renderer's features, and the console functions `setRenderer(id)` (true when it swapped or already drew with
/// that renderer; false, with a console line saying why, when no renderer has the id or it could not be made) and
/// `getRenderer()` (the drawing renderer's id, empty with none). [`RendererPlugin`] requires it.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.renderer-slot")]
pub struct RendererSlotPlugin;

impl Plugin for RendererSlotPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.client(ClientStage::Natives, |vm, engine| {
            let features = engine
                .resource::<RenderFeatures>()
                .as_deref()
                .copied()
                .unwrap_or(RenderFeatures::NONE);
            let (slot, warnings) = RendererSlot::new(engine.list::<RendererEntry>(), features);
            for warning in warnings {
                vm.warn(warning);
            }
            let adds = RenderAdds::of_engine(engine).kept(|w| vm.warn(w));
            let slot = slot.with_adds(adds);
            if slot.current().is_none() {
                vm.warn("the engine has no renderer: the world is not drawn".to_string());
            }
            vm.set_extension(Rc::new(slot));
            Ok(())
        });
        engine
            .client_script()
            .natives(torque_engine::natives![set_renderer, get_renderer]);
    }
}

/// Swaps the client's world view to the renderer registered as `id`: true when it swapped or already drew with it;
/// false, with a console line saying why, when no renderer has the id or it could not be made.
#[torque_engine::function(name = "setRenderer", usage = "setRenderer(id);")]
fn set_renderer(mut vm: VmMut, id: Cow<str>) -> bool {
    let Some(slot) = RendererSlot::of(&vm) else {
        vm.error("setRenderer: this client has no renderer slot".to_string());
        return false;
    };
    match slot.switch(id.trim()) {
        Ok(()) => true,
        Err(e @ SwitchError::Failed { .. }) => {
            vm.warn(format!("setRenderer: {e}"));
            false
        }
        Err(e) => {
            vm.error(format!("setRenderer: {e}"));
            false
        }
    }
}

/// The id of the renderer the client's world view draws with; empty with none.
#[torque_engine::function(name = "getRenderer", usage = "getRenderer();")]
fn get_renderer(vm: VmRef) -> String {
    RendererSlot::of(&vm)
        .and_then(|slot| slot.current().map(str::to_string))
        .unwrap_or_default()
}

/// The engine's feature set, made with no feature on if it is not there yet.
#[cfg(feature = "renderer")]
fn feature_set(engine: &mut EngineBuilder) -> &mut RenderFeatures {
    if engine.resource::<RenderFeatures>().is_none() {
        engine.insert_resource(RenderFeatures::NONE);
    }
    engine
        .resource_mut::<RenderFeatures>()
        .expect("the feature set was just inserted")
}

/// The engine's renderer on the slot, as [`DEFAULT_RENDERER`], drawn with the switches the [`RenderFeaturePlugins`] turn
/// on. In [`DefaultPlugins`](crate::plugins::DefaultPlugins); built with torque-app's `renderer` feature.
#[cfg(feature = "renderer")]
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.renderer")]
pub struct RendererPlugin;

#[cfg(feature = "renderer")]
impl Plugin for RendererPlugin {
    fn requires(&self, requires: &mut torque_engine::Requires) {
        requires.plugin::<RendererSlotPlugin>();
    }

    fn build(&self, engine: &mut EngineBuilder) {
        engine.push(RendererEntry {
            id: DEFAULT_RENDERER.to_owned(),
            make: None,
        });
        feature_set(engine);
        // The projected decals' cap (`GhostScene::read_prefs` reads it) and the upscaler (`WorldView::read_prefs`).
        engine
            .client_script()
            .prefs::<crate::ghosts::DecalPrefs>()
            .prefs::<RenderPrefs>();
    }
}

/// Declares the feature plugins: each one's type, id and what it turns on in the engine's [`RenderFeatures`].
#[cfg(feature = "renderer")]
macro_rules! feature_plugins {
    ($($(#[$doc:meta])* $name:ident = $id:literal, |$set:ident| $on:expr;)*) => {
        $(
            $(#[$doc])*
            #[derive(Clone, Copy, Debug, Default)]
            pub struct $name;

            impl torque_engine::PluginType for $name {
                const ID: torque_engine::PluginId = torque_engine::PluginId::new($id);
            }

            impl Plugin for $name {
                fn build(&self, engine: &mut EngineBuilder) {
                    let $set = feature_set(engine);
                    $on;
                }
            }
        )*

        /// The engine's renderer's features, each a plugin that turns its switches on; in this order in
        /// [`DefaultPlugins`](crate::plugins::DefaultPlugins). A game drops one with `disable::<P>()`.
        #[derive(Clone, Copy, Debug, Default)]
        pub struct RenderFeaturePlugins;

        impl torque_engine::PluginGroup for RenderFeaturePlugins {
            fn build(self) -> torque_engine::PluginGroupBuilder {
                torque_engine::PluginGroupBuilder::new()$(.add($name))*
            }
        }
    };
}

#[cfg(feature = "renderer")]
feature_plugins! {
    /// The sun's cascaded shadows, the lights' shadows and the contact shadows (`torque.render-shadows`).
    ShadowsPlugin = "torque.render-shadows", |set| {
        set.features.sun_shadows = true;
        set.features.point_shadows = true;
        set.features.contact_shadows = true;
    };
    /// The quality's anti-aliasing: the TAA with its jitter, or the FXAA-style filter (`torque.render-anti-aliasing`).
    AntiAliasingPlugin = "torque.render-anti-aliasing", |set| set.features.anti_aliasing = true;
    /// The ambient occlusion (`torque.render-ambient-occlusion`).
    AmbientOcclusionPlugin = "torque.render-ambient-occlusion", |set| set.features.ambient_occlusion = true;
    /// The reflections: SSR, the light capture, the planar reflection and the water drawn as water; off, reflective
    /// surfaces reflect the sky cube and the water is blended (`torque.render-reflections`).
    ReflectionsPlugin = "torque.render-reflections", |set| set.features.reflections = true;
    /// Clear water: absorbing toward its color on the way to the floor, scattering, foam where it is shallow; off, its
    /// texture stays its face (`torque.render-clear-water`).
    ClearWaterPlugin = "torque.render-clear-water", |set| set.clear_water = true;
    /// The bloom (`torque.render-bloom`).
    BloomPlugin = "torque.render-bloom", |set| set.features.bloom = true;
    /// The volumetric light (`torque.render-volumetrics`).
    VolumetricsPlugin = "torque.render-volumetrics", |set| set.features.volumetrics = true;
    /// The volumetric clouds where the quality has them; off, the server's flat cloud layers (`torque.render-clouds`).
    CloudsPlugin = "torque.render-clouds", |set| set.features.clouds = true;
    /// Weighted blended transparency behind an exact front layer; off, the translucent draws sorted and blended in turn
    /// (`torque.render-transparency`).
    TransparencyPlugin = "torque.render-transparency", |set| set.features.wboit = true;
    /// The tonemap through the grading table; off, the analytic curves (`torque.render-grading`).
    GradingPlugin = "torque.render-grading", |set| set.features.grading_lut = true;
    /// The glowing surfaces' lights (`torque.render-glow-lights`).
    GlowLightsPlugin = "torque.render-glow-lights", |set| set.features.glow_lights = true;
    /// The extension's fine surface detail and its extra draws (`torque.render-surface-detail`).
    SurfaceDetailPlugin = "torque.render-surface-detail", |set| set.features.ext_detail = true;
    /// Smoke and dust lit by the sun, the sky and the lights (`torque.render-particle-light`).
    ParticleLightPlugin = "torque.render-particle-light", |set| set.particle_light = true;
}
