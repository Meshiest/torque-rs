//! The per-frame world view.
//!
//! A frame of an example is one [`HdScene`] over plain cells ([`PlainScene`]): the daylight (sun, ambient, fog and the physical
//! sky), a textured floor, and the frame's moving shapes as [`DynamicObject`]s. [`WorldView::draw`] draws it with the enhanced
//! renderer ([`HdWorld`]) into the rectangle the canvas's `GameTSCtrl` gave the world this frame ([`GuiDraws::worlds`]), then
//! the GUI's batches over it with the [`GuiPass`] (no clear between). A canvas without a `GameTSCtrl` drawn draws the GUI alone.
//!
//! The shapes are posed on the CPU: [`WorldView::shape_object`] turns a [`SceneShape`] and its [`ShapeInstance`] (after its
//! threads were advanced and animated) into an object whose vertices are in the world. [`WorldView::set_objects`] hands the
//! frame its objects and keeps the last frame's, which give each object its motion (the anti-aliasing's history follows it).

use std::collections::HashMap;
use std::sync::Arc;

use torque_gui::GuiDraws;
use torque_render_core::{Gpu, GuiPass, wgpu};
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_scene::hd::cells::{Batch, Cell, SlotTexture, StaticCells};
use torque_render_scene::hd::clouds::CloudLook;
use torque_render_scene::hd::clusters::{Cluster, MAX_CELL_CLUSTERS};
use torque_render_scene::hd::environment::ClientDay;
use torque_render_scene::hd::objects::objects_of;
use torque_render_scene::hd::particle_light::ParticleLight;
use torque_render_scene::hd::scatter::ScatterLook;
use torque_render_scene::hd::{
    DynamicObject, HdCamera, HdDraw, HdMode, HdScene, HdSpot, Quality, bounding_sphere, material,
};
use torque_render_scene::images::scale_columns;
use torque_render_scene::math::Mat;
use torque_render_scene::moving::ShapeDraws;
use torque_render_scene::shapecache::next_stamp;
use torque_render_scene::shapes::{Loaded, ShapeParts, highest_detail, load, shape_draws};
use torque_render_scene::source::Eye;
use torque_render_scene::textures::TextureSet;
use torque_render_scene::world::ShapeWorld;
use torque_render_scene::{Assets, Blend, Cull, Draw, Filter, Sampler, TexEnv, Texture, Vertex};
use torque_shapes::instance::ShapeInstance;
use torque_shapes::shape::Shape;

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
    const FAR: i32 = -(1 << 28);
    [(index & 0x0fff_ffff) as i32, FAR, FAR]
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

/// The enhanced renderer, the GUI pass and the scene they draw.
pub struct WorldView {
    world: HdWorld<NoExtension>,
    gui: GuiPass,
    scene: PlainScene,
    /// Whether the floor or the daylight changed since the last frame (else only the objects did).
    statics_changed: bool,
    /// Where the day's sun was when the view was last lit by it ([`WorldView::set_day`]).
    day_sun: Option<[f32; 3]>,
    /// The shadows' color of the last [`Daylight`]: a day whose look names none keeps it.
    daylight_shadow: [f32; 3],
}

impl WorldView {
    /// A view on `gpu` with the default [`Daylight`], no floor and no objects, drawn at [`Quality::High`]. The effects' pipelines
    /// start being made on a worker thread ([`HdWorld::warm_effects`]).
    pub fn new(gpu: &Gpu) -> WorldView {
        let mut frame = HdScene::new(StaticCells::default());
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
        let scene = PlainScene {
            frame,
            ..PlainScene::default()
        };
        let mut view = WorldView {
            world: HdWorld::new(gpu),
            gui: GuiPass::new(gpu, None),
            scene,
            statics_changed: true,
            day_sun: None,
            daylight_shadow: Daylight::default().shadow,
        };
        view.set_daylight(&Daylight::default());
        // The effects' pipelines made beside the menus and the map's loading, not at the first shot.
        view.world.warm_effects();
        view
    }

    /// The scene as the next frame draws it.
    pub fn scene(&self) -> &PlainScene {
        &self.scene
    }

    /// The enhanced renderer the view draws with (its timers and reports).
    pub fn renderer(&mut self) -> &mut HdWorld<NoExtension> {
        &mut self.world
    }

    /// Starts the renderer's histories again (a cut: the next frame keeps nothing of the last).
    pub fn reset_history(&mut self) {
        self.world.reset_history();
    }

    /// The renderer's quality step.
    pub fn set_quality(&mut self, quality: Quality) {
        self.scene.mode = Some(HdMode::new(quality));
    }

    /// How the volumetric clouds look (None: the engine's, [`CloudLook::DEFAULT`]).
    pub fn set_cloud_look(&mut self, look: Option<CloudLook>) {
        self.scene.frame.cloud_look = look;
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

    fn put_static_mesh(&mut self, key: [i32; 3], local: &[usize], draws: &[Draw]) {
        let draws: Vec<HdDraw> = draws
            .iter()
            .map(|d| HdDraw::from_fixed(d, material::KIND_SHAPE, local))
            .collect();
        let cells = &mut self.scene.frame.cells;
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
        if cell.indices.is_empty() {
            cells.cells.remove(&key);
        } else {
            cells.cells.insert(key, cell);
        }
        self.statics_changed = true;
    }

    /// Takes the static mesh `key` out of the view (nothing if there is none).
    pub fn remove_static_mesh(&mut self, key: [i32; 3]) {
        if self.scene.frame.cells.cells.remove(&key).is_some() {
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
        objects_of(world, shaped, &local, &mut self.scene.objects, None)
    }

    /// The frame's moving objects. Each takes its motion from the last frame's object of the same id: its transform then and
    /// where its vertices were.
    pub fn set_objects(&mut self, mut objects: Vec<DynamicObject>) {
        let before: HashMap<u32, &DynamicObject> =
            self.scene.objects.iter().map(|b| (b.ghost, b)).collect();
        for o in &mut objects {
            if let Some(before) = before.get(&o.ghost) {
                o.prev_transform = before.transform;
                o.previous = o.previous_from(before);
            }
        }
        self.scene.objects = objects;
    }

    /// The frame's particles: each draw at its eye distance, its texture an index into `textures` (the set the emitters drew
    /// with). Every frame hands its own (none: an empty list).
    pub fn set_particles(&mut self, draws: &[(f32, Draw)], textures: TextureSet) {
        self.scene.set_particles(draws, textures);
    }

    /// Waits for the GPU and returns the last frame's GPU time in ms (`None` without timestamps).
    pub fn gpu_ms(&mut self) -> Option<f64> {
        self.world.gpu_ms()
    }

    /// Which particle draws the enhanced renderer lights (none: all unlit, as TGE draws them; [`ParticleLight::BLENDED`]: smoke
    /// and dust lit, glows not), from the next frame's particles on.
    pub fn set_particle_light(&mut self, rule: Option<ParticleLight>) {
        self.scene.particle_light = rule;
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
        let world = gui.worlds.first().and_then(|w| {
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
        let clear = world.is_none().then_some([0.0, 0.0, 0.0, 1.0]);
        self.gui.draw(gpu, gui, target, clear);
        Ok(world.is_some())
    }

    /// Draws the scene seen from `eye` into the rectangle `[x, y, width, height]` of `target` (an `Rgba8Unorm` texture).
    pub fn draw_world(
        &mut self,
        target: &wgpu::TextureView,
        [x, y, w, h]: [u32; 4],
        eye: Eye,
    ) -> Result<(), String> {
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
        self.world.upload(&self.scene);
        let format = target.texture().format();
        self.world
            .draw(&self.scene, &camera, Some((target, format, [x, y])))
            .map_err(|e| format!("the world: {e}"))
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
    }
}
