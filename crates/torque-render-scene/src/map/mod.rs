//! A map's world as plain draws: the terrain, the interiors, the water blocks, the two replicators and the map's static shapes,
//! from the caller's records of them ([`MapRecords`]) and the map's files (`.ter`, `.dif`, textures, `.dts`) read through
//! [`Assets`]. The looks are the engine's own reading of TGE's, drawn with the fixed-function renderer's state:
//!
//! - **Terrain** ([`terrain`]): the 256 x 256 heightfield, centred on the origin and repeated when the mission repeats it, cut in
//!   chunks of [`terrain::CHUNK`] squares; only the chunks within the visible distance are drawn (the enhanced looks' coarser
//!   far away, `terrain::lod_chunks`). The materials are blended by
//!   their alpha maps as layers: the first opaque, each next one over it with its weight's share in the vertex alpha.
//! - **Sun lighting** ([`light`]): the engine's own relight of the terrain and the interiors (the sun's shadows in a lightmap a
//!   terrain block and atlas pages of the interiors' rectangles), cached by content; each lit surface is drawn unlit and then
//!   multiplied by its lightmap. Until it is done (a live window makes it on a thread of its own), GL lighting from the sun.
//! - **Interiors** ([`interior`]): every instance's surfaces (triangle strips, texgen UVs) in world space, merged into one draw a
//!   material over the whole map. A material whose texture has alpha is drawn blended (and GL-lit).
//! - **Water** ([`water`]): the top of each block's box, its surface texture at its opacity, as a blended plane; in the
//!   enhanced looks also its copies a repeated terrain carries (TGE's tiled fluid), as far as the terrain is drawn.
//! - **Replicators and statics** ([`foliage`], [`statics`]): the foliage as billboards facing the camera within the view
//!   distance; the shape replicas and the `TSStatic`s as their shapes at rest, merged a material per shape file.
//!
//! **Kept between builds.** The parsed files and the merged meshes are kept on the building thread while the map's records and
//! the files' generation do not change ([`crate::textures::files_changed`]). A live window's build sends a mesh's vertices only
//! once ([`MapSent`], which the caller keeps between frames); the renderer keeps the buffers by the draw's key.

pub mod foliage;
pub mod interior;
pub mod light;
pub mod statics;
pub mod terrain;
pub mod water;

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use glam::Vec3;

use crate::textures::TextureSet;
use crate::{Assets, Blend, Cull, Draw, Filter, Sampler, Sun, TexEnv, Texture, Vertex};

/// Whether a map draw whose texture is found nowhere is drawn in pure magenta, unlit and unfogged (so a count of
/// missing-texture pixels can find it); off, such a draw is drawn untextured and lit.
pub static MARK_MISSING: AtomicBool = AtomicBool::new(false);

/// The colour of a missing texture while [`MARK_MISSING`] is set.
pub const MISSING_COLOR: [u8; 4] = [255, 0, 255, 255];

/// The prefix of every map draw's label.
pub const LABEL: &str = "map";

/// How a game reads and dresses its maps: the importer of the map's files (the drawing and its relight read only what it makes,
/// [`mapcore`]), the folders a texture found nowhere is looked for in next, and how a water surface's texture repeats.
#[derive(Clone, Copy)]
pub struct MapProfile {
    pub importer: &'static dyn mapcore::Importer,
    /// Where an interior's material found nowhere beside its `.dif` is looked for next ("": nowhere).
    pub interior_fallback: &'static str,
    /// Where a water block's surface texture found nowhere is looked for by its file name ("": nowhere).
    pub water_fallback: &'static str,
    /// The texture a water block draws when its own is found nowhere ("": none).
    pub water_texture: &'static str,
    /// Repeats of a water surface's texture a unit, in world x and y.
    pub water_loops_per_unit: f32,
    /// A terrain block stands at the height its mission wrote, not at 0 (`TerrainBlock::onAdd`).
    pub terrain_at_mission_height: bool,
}

impl MapProfile {
    /// TGE's importer ([`maps::legacy::IMPORTER`]), no fallback folders, a water texture repeated each 48 units
    /// (`fluidRender.cc`'s `Q1`), the terrain at height 0.
    pub const TGE: MapProfile = MapProfile {
        importer: &maps::legacy::IMPORTER,
        interior_fallback: "",
        water_fallback: "",
        water_texture: "",
        water_loops_per_unit: 1.0 / 48.0,
        terrain_at_mission_height: false,
    };
}

impl std::fmt::Debug for MapProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapProfile")
            .field("interior_fallback", &self.interior_fallback)
            .field("water_fallback", &self.water_fallback)
            .field("water_texture", &self.water_texture)
            .field("water_loops_per_unit", &self.water_loops_per_unit)
            .field("terrain_at_mission_height", &self.terrain_at_mission_height)
            .finish_non_exhaustive()
    }
}

/// A map's static shapes as the caller draws them: a shape file's draws at rest at its most detailed level (`None`: not
/// loaded), which [`statics`] merges per shape.
pub trait RestShapes {
    fn rest_draws(
        &self,
        assets: &dyn Assets,
        path: &str,
        textures: &mut TextureSet<'_>,
        notes: &mut Vec<String>,
    ) -> Option<Vec<Draw>>;
}

/// No static shapes: none is drawn.
impl RestShapes for () {
    fn rest_draws(
        &self,
        _: &dyn Assets,
        _: &str,
        _: &mut TextureSet<'_>,
        _: &mut Vec<String>,
    ) -> Option<Vec<Draw>> {
        None
    }
}

/// What a map is drawn from: its records, the game's profile and its static shapes.
#[derive(Clone, Copy)]
pub struct MapInput<'a> {
    pub records: &'a MapRecords,
    pub profile: &'a MapProfile,
    pub shapes: &'a dyn RestShapes,
}

/// The most drawn vertices the merged meshes of a map's interiors hold together (their lightmap passes, at most as many again,
/// besides), and of its static shapes apart: past it the rest is not merged (a note says so). A map's files come from its
/// server, so their size is the engine's to bound.
pub const MAX_MERGED_VERTICES: usize = 8_000_000;

/// The texture sampling every map surface uses (repeat, trilinear).
pub const SAMPLER: Sampler = Sampler {
    repeat: true,
    mag: Filter::Linear,
    min: Filter::Linear,
    mip: Filter::Linear,
    mipmaps: true,
};

/// The map's draws of a frame: the opaque ones (drawn with the opaque objects) and the blended ones with their distance from the
/// eye (drawn back to front with the translucent objects).
#[derive(Debug, Default)]
pub struct MapDraws {
    pub opaque: Vec<Draw>,
    pub translucent: Vec<(f32, Draw)>,
}

/// The map meshes a live window's renderer holds (their vertex keys): a mesh is sent once and named by its key after; a key not
/// drawn in a frame is let go, as the renderer lets go of its buffers.
#[derive(Debug, Default, Clone)]
pub struct MapSent {
    keys: HashSet<u64>,
}

/// How a kept mesh names its texture: by the name the build resolved, looked up again each frame.
#[derive(Debug, Clone, PartialEq)]
pub enum TexRef {
    /// No texture on purpose.
    None,
    /// A texture name (`TextureSet::get`).
    Name(String),
    /// A texture name that was found nowhere when the mesh was built.
    Missing(String),
    /// A texture the engine made (a lightmap).
    Generated(Arc<Texture>),
}

/// A mesh kept between builds and the state it is drawn with.
#[derive(Debug, Clone)]
pub struct Mesh {
    pub label: String,
    pub texture: TexRef,
    pub vertices: Arc<Vec<Vertex>>,
    pub lighting: bool,
    pub tex_env: TexEnv,
    pub blend: Blend,
    pub cull: Cull,
    pub depth_write: bool,
    pub sampler: Sampler,
    /// Blended when its texture has alpha (an interior's material), whatever `blend` says.
    pub alpha_from_texture: bool,
    /// Drawn with the opaque objects whatever its blending (a terrain layer or a lightmap pass over the surface drawn before it).
    pub overlay: bool,
    pub centre: [f32; 3],
    pub key: u64,
}

/// FNV-1a over words (the meshes' keys and the map's identity).
#[derive(Debug, Clone, Copy)]
pub struct Fnv(pub u64);

impl Fnv {
    pub fn new(tag: &str) -> Fnv {
        let mut h = Fnv(0xcbf2_9ce4_8422_2325);
        h.bytes(tag.as_bytes());
        h
    }
    pub fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
    pub fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
}

/// The classes of a map's objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MapClass {
    TerrainBlock,
    InteriorInstance,
    WaterBlock,
    MissionArea,
    FxFoliageReplicator,
    FxShapeReplicator,
}

impl MapClass {
    /// The class's script name.
    pub fn name(self) -> &'static str {
        match self {
            MapClass::TerrainBlock => "TerrainBlock",
            MapClass::InteriorInstance => "InteriorInstance",
            MapClass::WaterBlock => "WaterBlock",
            MapClass::MissionArea => "MissionArea",
            MapClass::FxFoliageReplicator => "fxFoliageReplicator",
            MapClass::FxShapeReplicator => "fxShapeReplicator",
        }
    }
}

/// One field value of a map object.
#[derive(Debug, Clone, PartialEq)]
pub enum MapValue {
    F32(f32),
    Int(u32),
    Bool(bool),
    Text(Vec<u8>),
    Point([f32; 3]),
    Color([f32; 4]),
    Ints(Vec<u32>),
}

/// A map object's field: its name and its elements (one for a plain field).
#[derive(Debug, Clone, PartialEq)]
pub struct MapField {
    pub name: &'static str,
    pub values: Vec<MapValue>,
}

/// One map object: its class, transform (the position, the quaternion's x, y, z and the sign of its w), scale and fields.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub class: MapClass,
    pub position: [f32; 3],
    pub quat: [f32; 3],
    pub negative_w: bool,
    pub scale: [f32; 3],
    pub fields: Vec<MapField>,
}

impl Object {
    /// An object at `position` with no rotation, `scale` and `fields` (each one element).
    pub fn new(
        class: MapClass,
        position: [f32; 3],
        scale: [f32; 3],
        fields: impl IntoIterator<Item = (&'static str, MapValue)>,
    ) -> Object {
        Object {
            class,
            position,
            quat: [0.0; 3],
            negative_w: false,
            scale,
            fields: fields
                .into_iter()
                .map(|(name, v)| MapField {
                    name,
                    values: vec![v],
                })
                .collect(),
        }
    }
    /// Field `name`'s element `index` (names match ignoring case).
    pub fn get(&self, name: &str, index: usize) -> Option<&MapValue> {
        self.fields
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(name))
            .and_then(|f| f.values.get(index))
    }
    pub fn text(&self, name: &str) -> String {
        match self.get(name, 0) {
            Some(MapValue::Text(t)) => String::from_utf8_lossy(t).replace('\\', "/"),
            _ => String::new(),
        }
    }
    pub fn f32(&self, name: &str) -> f32 {
        match self.get(name, 0) {
            Some(MapValue::F32(x)) => *x,
            Some(MapValue::Int(n)) => *n as i32 as f32,
            _ => 0.0,
        }
    }
    pub fn int(&self, name: &str) -> i32 {
        match self.get(name, 0) {
            Some(MapValue::Int(n)) => *n as i32,
            Some(MapValue::F32(x)) => *x as i32,
            _ => 0,
        }
    }
    pub fn flag(&self, name: &str) -> bool {
        matches!(self.get(name, 0), Some(MapValue::Bool(true)))
    }
    pub fn point(&self, name: &str) -> [f32; 3] {
        match self.get(name, 0) {
            Some(MapValue::Point(p)) => *p,
            _ => [0.0; 3],
        }
    }
    pub fn ints(&self, name: &str) -> Vec<u32> {
        match self.get(name, 0) {
            Some(MapValue::Ints(v)) => v.clone(),
            _ => Vec::new(),
        }
    }
    pub fn position(&self) -> [f32; 3] {
        self.position
    }
    pub fn scale(&self) -> [f32; 3] {
        self.scale
    }
    /// The object's rotation and translation (TGE's `MatrixF`), without the scale.
    pub fn matrix(&self) -> crate::math::Mat {
        let [x, y, z] = self.quat;
        let mut w = (1.0 - (x * x + y * y + z * z)).max(0.0).sqrt();
        if self.negative_w {
            w = -w;
        }
        let mut m = crate::math::Mat::from_quat([x, y, z, w]);
        m.set_column3(self.position());
        m
    }
}

/// The map objects and static shapes of a world, in the order the caller keeps them.
#[derive(Debug, Default, Clone)]
pub struct MapRecords {
    pub objects: Vec<Object>,
    /// Each `TSStatic`: its shape, matrix and scale.
    pub statics: Vec<(String, [f32; 16], [f32; 3])>,
}

/// What the map's records and files make: the identity they were made for and the parts.
struct Built {
    key: u64,
    generation: u64,
    /// A texture was still being decoded (a live window's [`TextureSet::aside`]): made again at the next build.
    pending: bool,
    terrains: Vec<terrain::Block>,
    meshes: Vec<Mesh>,
    foliage: Vec<foliage::Set>,
    notes: Vec<String>,
    /// What the relight reads, its key, and the key the meshes were lit with (None: GL-lit).
    light: light::Inputs,
    light_key: u64,
    lit_key: Option<u64>,
}

thread_local! {
    static BUILT: RefCell<Option<Built>> = const { RefCell::new(None) };
    /// The enhanced renderer's build (GL-lit, no relight), kept apart from the fixed-function one's.
    static BUILT_HD: RefCell<Option<Built>> = const { RefCell::new(None) };
}

/// The records' identity: a hash of every object's class, transform, scale and field values in order, and of the statics.
pub fn identity(r: &MapRecords) -> u64 {
    let mut h = Fnv::new("map records");
    for o in &r.objects {
        h.bytes(o.class.name().as_bytes());
        for p in o.position.iter().chain(&o.quat) {
            h.u32(p.to_bits());
        }
        h.u32(u32::from(o.negative_w));
        for s in &o.scale {
            h.u32(s.to_bits());
        }
        for v in o.fields.iter().flat_map(|f| &f.values) {
            match v {
                MapValue::F32(x) => h.u32(x.to_bits()),
                MapValue::Int(n) => h.u32(*n),
                MapValue::Bool(b) => h.u32(u32::from(*b)),
                MapValue::Text(t) => {
                    h.u64(t.len() as u64);
                    h.bytes(t);
                }
                MapValue::Point(p) => p.iter().for_each(|x| h.u32(x.to_bits())),
                MapValue::Color(c) => c.iter().for_each(|x| h.u32(x.to_bits())),
                MapValue::Ints(list) => {
                    h.u64(list.len() as u64);
                    list.iter().for_each(|&x| h.u32(x));
                }
            }
        }
    }
    for (shape, m, s) in &r.statics {
        h.bytes(shape.as_bytes());
        m.iter().chain(s).for_each(|x| h.u32(x.to_bits()));
    }
    h.0
}

/// Lets go of the map built on this thread (the checks' second load builds it again).
pub fn forget_built() {
    BUILT.with(|cell| *cell.borrow_mut() = None);
    BUILT_HD.with(|cell| *cell.borrow_mut() = None);
}

/// Where a map mesh of the enhanced renderer goes ([`enhanced`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdPart {
    /// An interior's, a static shape's or a shape replica's surfaces (merged a material over the map).
    Surface,
    /// One layer of the terrain chunk with this key: a chunk's layers in order, the first opaque, each next blended over it.
    Chunk(u64),
    /// The top of a water block.
    Water,
    /// The billboards of the foliage replicator with this key, built for this eye.
    Foliage(u64),
}

/// A map mesh for the enhanced renderer: its draw state (no vertices: they are `vertices`, shared with the kept mesh), its key
/// (the same key, the same vertices and state), its centre and where it goes.
#[derive(Debug, Clone)]
pub struct HdMesh {
    pub draw: Draw,
    pub vertices: Arc<Vec<Vertex>>,
    pub key: u64,
    pub centre: [f32; 3],
    pub part: HdPart,
}

/// The map's meshes for the enhanced renderer seen from `eye` with `visible` the visible distance (it lights, shadows and
/// reflects them). The same files and placement as the fixed-function [`draws`], built apart and never relit (no lightmap
/// pass): every surface GL-lit, so the enhanced renderer's own sun, shadows and ambient light it. The bounds are the
/// fixed-function build's (the readers' caps, the kept chunks, the replicas, the billboards); the terrain is drawn at levels of
/// detail (`terrain::lod_chunks`) and a repeated terrain carries the water with it (`water::tiles`), so `visible` may reach
/// far past the fixed-function frame's.
pub fn enhanced(
    map: MapInput<'_>,
    assets: &dyn Assets,
    eye: [f32; 3],
    visible: f32,
    textures: &mut TextureSet<'_>,
    notes: &mut Vec<String>,
) -> Vec<HdMesh> {
    let recs = map.records;
    if recs.objects.is_empty() {
        return Vec::new();
    }
    let key = identity(recs);
    let generation = crate::textures::files_generation();
    BUILT_HD.with(|cell| {
        let mut slot = cell.borrow_mut();
        let again = !slot
            .as_ref()
            .is_some_and(|b| b.key == key && b.generation == generation && !b.pending);
        if again {
            let deferred = textures.deferred;
            let sun = Sun {
                towards: [0.0, 0.0, 1.0],
                diffuse: [0.0; 3],
                ambient: [0.0; 3],
            };
            let mut made = build(map, assets, key, generation, sun, true, false, textures);
            made.pending = textures.deferred != deferred;
            *slot = Some(made);
        }
        let Some(built) = slot.as_mut() else {
            return Vec::new();
        };
        notes.extend(built.notes.iter().cloned());
        let mut out = Vec::new();
        let mut push = |m: &Mesh, part: HdPart, textures: &mut TextureSet<'_>| {
            let Some((draw, recolour)) = resolve(m, textures) else {
                return;
            };
            let (vertices, key) = if recolour {
                (
                    Arc::new(recoloured(&m.vertices)),
                    m.key ^ 0x6d69_7373_696e_6721,
                )
            } else {
                (Arc::clone(&m.vertices), m.key)
            };
            out.push(HdMesh {
                draw,
                vertices,
                key,
                centre: m.centre,
                part,
            });
        };
        for m in &built.meshes {
            let part = if m.label.starts_with(water::LABEL) {
                HdPart::Water
            } else {
                HdPart::Surface
            };
            push(m, part, textures);
        }
        // The water a repeated terrain carries with it (TGE's tiled fluid), as far as the terrain is drawn.
        let period = built
            .terrains
            .iter()
            .find_map(terrain::Block::repeat_period);
        if let Some(period) = period {
            for (mesh_key, corners) in water::tops(&recs.objects, key) {
                let Some(m) = built.meshes.iter().find(|m| m.key == mesh_key) else {
                    continue;
                };
                if let Some(t) = water::tiles(
                    m,
                    corners,
                    period,
                    map.profile.water_loops_per_unit,
                    eye,
                    visible,
                    terrain::MAX_LOD_TILE_REACH,
                ) {
                    push(&t, HdPart::Water, textures);
                }
            }
        }
        for chunk in terrain::lod_chunks(&mut built.terrains, eye, visible, key) {
            let Some(first) = chunk.first() else {
                continue;
            };
            let id = first.key;
            for m in chunk.iter() {
                push(m, HdPart::Chunk(id), textures);
            }
        }
        for (n, set) in built.foliage.iter().enumerate() {
            if let Some(d) = foliage::draw(set, eye, textures) {
                let mut h = Fnv::new("map hd foliage");
                h.u64(key);
                h.u64(n as u64);
                let id = h.0;
                eye.iter().for_each(|c| h.u32(c.to_bits()));
                let mut d = d;
                let vertices = Arc::new(std::mem::take(&mut d.vertices));
                out.push(HdMesh {
                    draw: d,
                    vertices,
                    key: h.0,
                    centre: set.centre,
                    part: HdPart::Foliage(id),
                });
            }
        }
        out
    })
}

/// The map's draws for a frame seen from `eye` with `visible` the visible distance, lit by `sun`. `sent`: a live window's record
/// of the meshes its renderer holds (None: every mesh's vertices are sent, as a build compared byte for byte wants; the relight
/// is then made before the frame instead of on a thread of its own).
#[allow(clippy::too_many_arguments)]
pub fn draws(
    map: MapInput<'_>,
    assets: &dyn Assets,
    eye: [f32; 3],
    visible: f32,
    sun: Sun,
    textures: &mut TextureSet<'_>,
    notes: &mut Vec<String>,
    sent: Option<&mut MapSent>,
) -> MapDraws {
    let recs = map.records;
    if recs.objects.is_empty() {
        return MapDraws::default();
    }
    let key = identity(recs);
    let generation = crate::textures::files_generation();
    let blocking = sent.is_none();
    BUILT.with(|cell| {
        let mut slot = cell.borrow_mut();
        let mut again = !slot
            .as_ref()
            .is_some_and(|b| b.key == key && b.generation == generation && !b.pending);
        // The same map: a new sun asks for its relight, and lightmaps that came since are drawn (built again once).
        if !again && let Some(b) = slot.as_mut() {
            if b.light.sun != sun {
                b.light.sun = sun;
                b.light_key = b.light.key();
            }
            if b.lit_key != Some(b.light_key)
                && light::ENABLED.load(Ordering::Relaxed)
                && (light::ready(b.light_key)
                    || light::get(&b.light, b.light_key, blocking).is_some())
            {
                again = true;
            }
        }
        if again {
            let deferred = textures.deferred;
            let mut made = build(map, assets, key, generation, sun, blocking, true, textures);
            made.pending = textures.deferred != deferred;
            *slot = Some(made);
        }
        let Some(built) = slot.as_mut() else {
            return MapDraws::default();
        };
        notes.extend(built.notes.iter().cloned());
        let mut out = MapDraws::default();
        let mut meshes: Vec<&Mesh> = built.meshes.iter().collect();
        let chunks = terrain::visible_chunks(&mut built.terrains, eye, visible, key);
        meshes.extend(chunks.iter().flat_map(|m| m.iter()));
        let mut keys = HashSet::new();
        let held = sent.as_ref().map(|s| &s.keys);
        for m in meshes {
            emit(m, textures, held, &mut keys, eye, &mut out);
        }
        for set in &built.foliage {
            if let Some(d) = foliage::draw(set, eye, textures) {
                let distance = Vec3::from(set.centre).distance(eye.into());
                out.translucent.push((distance, d));
            }
        }
        if let Some(s) = sent {
            s.keys = keys;
        }
        out
    })
}

/// A kept mesh's draw state for this frame, its vertices left out: the texture looked up again (None: still being decoded, the
/// mesh waits for a later build), and whether its vertices are drawn in [`MISSING_COLOR`] (a texture found nowhere while
/// [`MARK_MISSING`] is set).
fn resolve(m: &Mesh, textures: &mut TextureSet<'_>) -> Option<(Draw, bool)> {
    if m.vertices.is_empty() {
        return None;
    }
    let mark = MARK_MISSING.load(Ordering::Relaxed);
    let deferred = textures.deferred;
    let found = match &m.texture {
        TexRef::None => Some(None),
        TexRef::Name(n) => textures.get(n).map(Some),
        TexRef::Missing(_) => None,
        TexRef::Generated(t) => textures.generated(t).map(Some),
    };
    // A texture still being decoded: the mesh waits for a later build (never drawn untextured meanwhile).
    if found.is_none() && textures.deferred != deferred {
        return None;
    }
    let mut d = Draw {
        label: format!("{LABEL} {}", m.label),
        vertices: Vec::new(),
        lighting: m.lighting,
        tex_env: m.tex_env,
        texture: None,
        blend: m.blend,
        cull: m.cull,
        depth_test: true,
        depth_write: m.depth_write,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    };
    let mut recolour = false;
    match found {
        Some(Some(t)) => {
            d.texture = Some((t, m.sampler));
            if m.alpha_from_texture && textures.has_alpha(t) {
                d.blend = Blend::Alpha;
                d.depth_write = false;
            }
        }
        Some(None) => d.tex_env = TexEnv::None,
        None => {
            d.tex_env = TexEnv::None;
            d.label.push_str(MISSING_SUFFIX);
            if mark {
                d.lighting = false;
                d.fog = false;
                recolour = true;
            }
        }
    }
    Some((d, recolour))
}

/// The label's end of a map draw whose texture is found nowhere.
pub const MISSING_SUFFIX: &str = " (missing texture)";

/// `vertices` in [`MISSING_COLOR`].
fn recoloured(vertices: &[Vertex]) -> Vec<Vertex> {
    vertices
        .iter()
        .map(|v| Vertex {
            color: MISSING_COLOR,
            ..*v
        })
        .collect()
}

/// A kept mesh as this frame's draw.
fn emit(
    m: &Mesh,
    textures: &mut TextureSet<'_>,
    held: Option<&HashSet<u64>>,
    keys: &mut HashSet<u64>,
    eye: [f32; 3],
    out: &mut MapDraws,
) {
    let Some((mut d, recolour)) = resolve(m, textures) else {
        return;
    };
    if recolour {
        d.vertices = recoloured(&m.vertices);
    } else if let Some(held) = held {
        d.vertex_key = Some(m.key);
        keys.insert(m.key);
        if !held.contains(&m.key) {
            d.vertices = m.vertices.as_ref().clone();
        }
    } else {
        d.vertices = m.vertices.as_ref().clone();
    }
    if d.blend == Blend::Opaque || m.overlay {
        out.opaque.push(d);
    } else {
        out.translucent
            .push((Vec3::from(m.centre).distance(eye.into()), d));
    }
}

/// Reads the map's files, asks for its relight and makes its meshes.
#[allow(clippy::too_many_arguments)]
fn build(
    map: MapInput<'_>,
    assets: &dyn Assets,
    key: u64,
    generation: u64,
    sun: Sun,
    blocking: bool,
    relight: bool,
    textures: &mut TextureSet<'_>,
) -> Built {
    let (recs, profile) = (map.records, map.profile);
    let mut notes = Vec::new();
    let mut terrains = Vec::new();
    for o in recs
        .objects
        .iter()
        .filter(|o| o.class == MapClass::TerrainBlock)
    {
        match terrain::Block::load(o, profile, assets, textures) {
            Ok(b) => terrains.push(b),
            Err(e) => notes.push(format!("map terrain: {e}")),
        }
    }
    let instances = interior::instances(&recs.objects, profile, assets, &mut notes);
    let materials = interior::materials(&instances, profile, textures);
    // The enhanced renderer lights the map themselves: no relight is asked for (nor its inputs read).
    let inputs = light::Inputs {
        sun,
        terrains: if relight {
            terrains.iter().map(terrain::Block::light_input).collect()
        } else {
            Vec::new()
        },
        instances: if relight {
            interior::light_inputs(&instances, &materials)
        } else {
            Vec::new()
        },
    };
    let light_key = if relight { inputs.key() } else { 0 };
    let lit = if relight {
        light::get(&inputs, light_key, blocking)
    } else {
        None
    };
    if let Some(l) = &lit {
        for (index, b) in terrains.iter_mut().enumerate() {
            let mut light = terrain::Light {
                tiles: Vec::new(),
                rest: None,
                key: l.key,
            };
            for (m, t) in l.plan.terrains.iter().zip(&l.terrains) {
                if m.block != index {
                    continue;
                }
                if m.interiors {
                    light.tiles.push((m.tile, Arc::clone(t)));
                } else {
                    light.rest = Some(Arc::clone(t));
                }
            }
            if !light.tiles.is_empty() || light.rest.is_some() {
                b.set_light(light);
            }
        }
    }
    let mut meshes = interior::meshes(&instances, &materials, key, lit.as_deref(), &mut notes);
    meshes.extend(water::meshes(&recs.objects, profile, textures, key));
    let ground = foliage::interior_ground(&recs.objects, profile, assets);
    let placed = foliage::shape_replicas(&recs.objects, &terrains, &ground);
    meshes.extend(statics::meshes(
        map.shapes,
        assets,
        &recs.statics,
        &placed,
        textures,
        key,
        &mut notes,
        MAX_MERGED_VERTICES,
    ));
    let foliage = foliage::sets(&recs.objects, &terrains, &ground, textures);
    Built {
        key,
        generation,
        pending: false,
        terrains,
        meshes,
        foliage,
        notes,
        lit_key: lit.as_ref().map(|l| l.key),
        light: inputs,
        light_key,
    }
}

/// The relight's reading of the terrain under the map's roofs (the plan's check, `light::roofs`): for each of the `count`
/// largest interiors standing on the terrain with a roof over their middle, the terrain lightmap's luminance there, an open
/// texel's with the same normal, and the ambient's. Empty before the map on this thread is lit.
pub fn roof_readings(count: usize) -> Vec<[f32; 3]> {
    BUILT.with(|cell| {
        let slot = cell.borrow();
        let Some(b) = slot.as_ref() else {
            return Vec::new();
        };
        let Some(lit) = light::kept(b.light_key) else {
            return Vec::new();
        };
        light::roofs(&lit, &b.light, count)
    })
}

/// The height of the map's terrain under (x, y) (the first terrain block's, None off it or over a hole): a view can stand on it.
pub fn terrain_height(map: MapInput<'_>, assets: &dyn Assets, x: f32, y: f32) -> Option<f32> {
    let o = map
        .records
        .objects
        .iter()
        .find(|o| o.class == MapClass::TerrainBlock)?;
    let bytes = assets.read(&o.text("terrainFile"))?;
    let ter = map.profile.importer.heightfield(&bytes).ok()?;
    terrain::height_at(&ter, o, map.profile, x, y)
}

/// Each interior instance's world bounds (min, max): a view can look at them.
pub fn interior_bounds(map: MapInput<'_>, assets: &dyn Assets) -> Vec<([f32; 3], [f32; 3])> {
    interior::bounds(&map.records.objects, map.profile, assets)
}

/// A texture's lookup with the engine's fallbacks for a map's files: the name as written (with the resource manager's search up
/// the folders), else the same file name in each of `folders` in turn (a map may name files in other downloads' folders: a map's
/// own folder, then the profile's fallback folders). The first lookup's note stays.
pub fn find_texture(
    textures: &mut TextureSet<'_>,
    name: &str,
    folders: &[&str],
) -> Option<(usize, String)> {
    if name.is_empty() {
        return None;
    }
    if let Some(t) = textures.get(name) {
        return Some((t, name.to_string()));
    }
    let file = name.rsplit('/').next().unwrap_or(name);
    for folder in folders.iter().filter(|f| !f.is_empty()) {
        let local = format!("{}/{file}", folder.trim_end_matches('/'));
        if local.eq_ignore_ascii_case(name) {
            continue;
        }
        if let Some(t) = textures.get(&local) {
            return Some((t, local));
        }
    }
    None
}

/// A path's folder (`a/b/c.dif` -> `a/b`).
pub fn folder_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}
