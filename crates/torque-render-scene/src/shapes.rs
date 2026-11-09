//! A ShapeBase object's shape as plain draws: the shape file loaded (kept between builds), its detail level, and each object's
//! meshes with their materials' fixed-function state, tints, hidden objects, IFL frames and point lights. The caller
//! places and poses the shape (an instance of it, the object's transform and scale) and says what the object asks of its
//! drawing ([`ShapeParts`]).
//!
//! **The detail** (TGE 1.4 `TSShapeInstance::selectCurrentDetail`): a shape whose first detail has a non-negative `maxError` and a
//! smallest visible detail uses the error scheme (at [`SCREEN_ERROR`], 0.001, such a shape lands on its smallest visible
//! detail); any other the size scheme (the first detail smaller than the shape's projected radius in pixels).
//!
//! **The objects** (`TSShapeInstance::render`): in object order; besides, an object whose name a host's hidden-name list names
//! is skipped ([`ShapeParts::hidden`]).
//!
//! **Tints** (this crate's, not TGE's): a host may give each object a colour ([`ShapeParts::colors`]) or one colour for every
//! object ([`ShapeParts::color_all`]); a tinted object draws with the tint block below, the colour's alpha the colour's (times
//! the object's visibility, 1 here); one without draws plain.
//!
//! **The GL state** (`Gl`): `TSMesh::initMaterials` first (blending off with `SRC_ALPHA, ONE_MINUS_SRC_ALPHA`, depth
//! writes on, `GL_MODULATE`, texturing off, the material's ambient and diffuse white, no current material), then per primitive
//! whose material changes `TSMesh::setMaterial`:
//! - the material's texture bound (the IFL slot's current frame for an IFL material); `GL_REPEAT` or `GL_CLAMP` per its S
//!   and T flags; linear filtering, or (this engine's own bit, [`shapes::matlist::NEAREST`]) nearest near and far with the mipmap levels still
//!   blended between, as an interior's nearest material (a TGE shape never has the bit: it draws as TGE's);
//! - when the flags changed: `Translucent` turns blending on and depth writes off (else off and on); `Additive` makes the blend
//!   `SRC_ALPHA, ONE`, else `SRC_ALPHA, ONE_MINUS_SRC_ALPHA` (set only when the additive or subtractive bit changed);
//!   `SelfIlluminating` turns lighting off (else on);
//! - the tint block, for a tinted object: a texture without the alpha flag ([`TextureSet::has_alpha`]), or none, draws in
//!   `GL_MODULATE`, the colour ignored unless its alpha is under 1 (then `GL_COLOR_MATERIAL` with `(1, 1, 1, alpha)`, blending on,
//!   depth writes on); a texture with the flag draws in `GL_DECAL` over the colour (`GL_COLOR_MATERIAL` with the colour; blending on
//!   when its alpha is under 1). After the mesh: blending off with `ONE, ZERO` (when the alpha was under 1), the colour white,
//!   `GL_COLOR_MATERIAL` off, `GL_MODULATE`, texturing off.
//!
//! A shape with a translucent material in its drawn detail is drawn with the translucent objects, back to front
//! (`ShapeBase::prepRenderImage`: the whole shape, sorted by its distance).
//!
//! Credit: follows TGE 1.4's `ts/tsShapeInstance.cc`, `ts/tsMesh.cc` (with `TSSkinMesh`) and `game/shapeBase.cc` (Copyright (C)
//! GarageGames.com, Inc.) as Torque3D carries them under the MIT licence (`ts/tsShapeInstance.cpp` and `ts/tsMesh.cpp`,
//! Copyright (c) 2012 GarageGames, LLC; `T3D/shapeBase.cpp`, Copyright (c) 2012 GarageGames, LLC and Copyright (C) 2015 Faust
//! Logic, Inc.). The code here is this crate's own.

use std::collections::BTreeMap;
use std::sync::Arc;

use shapes::dts;
use shapes::instance::ShapeInstance;
use shapes::shape::{MeshExtra, MeshRender, Model, SkinData};

use crate::math::{Mat, unit_or_same};
use crate::textures::TextureSet;
use crate::{Assets, Blend, Cull, Draw, Filter, PointLight, Sampler, TexEnv, Vertex};

/// `TSDrawPrimitive`'s type bits and its material mask.
const PRIM_TYPE_MASK: u32 = 0xc000_0000;
const PRIM_STRIP: u32 = 0x4000_0000;
const PRIM_FAN: u32 = 0x8000_0000;
const PRIM_NO_MATERIAL: u32 = 0x1000_0000;
const PRIM_MATERIAL_MASK: u32 = 0x0fff_ffff;
/// `TSMaterialList`'s flags.
const MAT_S_WRAP: u32 = 0x1;
const MAT_T_WRAP: u32 = 0x2;
const MAT_TRANSLUCENT: u32 = 0x4;
const MAT_ADDITIVE: u32 = 0x8;
const MAT_SUBTRACTIVE: u32 = 0x10;
const MAT_SELF_ILLUMINATING: u32 = 0x20;
const MAT_NO_MIPMAP: u32 = 0x80;
const MAT_IFL: u32 = 0x0800_0000;
/// This engine's own material bit: sampled nearest ([`shapes::matlist::NEAREST`]).
pub const MAT_NEAREST: u32 = shapes::matlist::NEAREST;
/// The screen error the error scheme picks a detail by: TGE's `TSShapeInstance::smScreenError` for the control object and its
/// mount (its default for other shapes is 5.0), here for every shape.
pub const SCREEN_ERROR: f32 = 0.001;
/// An object at or under this visibility is not drawn.
pub const MIN_VISIBILITY: f32 = 0.01;
/// The detail level drawn at `distance` with `pixels_per_meter` pixels to a meter at unit distance.
pub fn select_detail(model: &Model, distance: f32, pixels_per_meter: f32) -> Option<usize> {
    let s = &model.shape;
    let smallest = s.smallest_visible_dl;
    let proj = pixels_per_meter / distance.max(1e-6);
    if smallest >= 0 && s.details.first().is_some_and(|d| d.max_error >= 0.0) {
        let tol = SCREEN_ERROR / proj;
        let prev = 10.0 * s.details.get(smallest as usize)?.average_error * 20.0;
        if prev < tol {
            return Some(smallest as usize);
        }
        for i in (0..=smallest as usize).rev() {
            if 10.0 * s.details[i].average_error < tol {
                return Some(i);
            }
        }
        return Some(0);
    }
    let size = s.radius * proj;
    if size <= s.smallest_visible_size as f32 {
        return None;
    }
    (0..s.details.len()).find(|&i| {
        size > s.details[i].size || i + 1 >= s.details.len() || s.details[i + 1].size < 0.0
    })
}

/// Which detail level a shape is drawn at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailPolicy {
    /// The choice by distance ([`select_detail`]): what the fixed-function frame draws.
    #[default]
    ByDistance,
    /// The most detailed visible level at any distance (the enhanced renderer).
    Highest,
}

/// The first detail level that is drawn at all (a negative size marks a collision or line-of-sight level).
pub fn highest_detail(model: &Model) -> Option<usize> {
    model
        .shape
        .details
        .iter()
        .position(|d| d.size >= 0.0 && d.sub_shape >= 0)
}

pub fn detail_for(
    model: &Model,
    distance: f32,
    pixels_per_meter: f32,
    policy: DetailPolicy,
) -> Option<usize> {
    match policy {
        DetailPolicy::ByDistance => select_detail(model, distance, pixels_per_meter),
        DetailPolicy::Highest => highest_detail(model),
    }
}

pub fn mat3_scale(m: &Mat, s: [f32; 3]) -> Mat {
    let mut r = *m;
    for row in 0..3 {
        for (col, k) in s.iter().enumerate() {
            r.m[row * 4 + col] *= k;
        }
    }
    r
}

/// A shape as the client loads it: the parsed file with the constructor's sequences imported, and the frames of its IFL materials.
pub struct Loaded {
    pub model: Model,
    /// Per IFL material: its name (`name.ifl`), its material slot and the frames' texture paths.
    pub ifls: Vec<(String, usize, Vec<String>)>,
    /// Per mesh: the sphere about its vertices' box in its own space (centre, radius; a build whose vertices stay there,
    /// [`MeshPlace`], is bounded by its meshes' spheres where they are placed).
    pub spheres: Vec<[f32; 4]>,
    /// Whether a mesh is a skin (its vertices are made from several nodes: a build of it stays in world space).
    pub skinned: bool,
    /// Per detail level: the most vertices a build of it makes (every triangle of its meshes, three vertices each), what the
    /// moving shapes' budget counts ([`crate::moving::ShapeBudget`]).
    pub detail_vertices: Vec<usize>,
}

impl Loaded {
    /// The most vertices a build at detail `dl` makes (0 for a level that does not exist).
    pub fn vertices_at(&self, dl: usize) -> usize {
        self.detail_vertices.get(dl).copied().unwrap_or(0)
    }

    /// The next drawn detail level below `dl` (a negative size or sub-shape marks a level that is not drawn), if any.
    pub fn lower_detail(&self, dl: usize) -> Option<usize> {
        let details = &self.model.shape.details;
        (dl + 1..details.len()).find(|&i| details[i].size >= 0.0 && details[i].sub_shape >= 0)
    }

    /// The meshes' spheres when a build of this shape may keep its vertices in their meshes' own space (`on`, and no skin).
    pub fn node_spheres(&self, on: bool) -> Option<&[[f32; 4]]> {
        (on && !self.skinned).then_some(self.spheres.as_slice())
    }
}

/// The sphere about the box of `points` (centre, radius); a zero sphere at the origin for none.
fn sphere_of(points: &[[f32; 3]]) -> [f32; 4] {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    if !(lo[0] <= hi[0] && lo[1] <= hi[1] && lo[2] <= hi[2]) {
        return [0.0; 4];
    }
    let c = [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
    let r = points
        .iter()
        .map(|p| {
            let d = [0, 1, 2].map(|k| p[k] - c[k]);
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        })
        .fold(0.0f32, f32::max);
    [c[0], c[1], c[2], r]
}

/// One drawn mesh of a build whose vertices stay in their meshes' own space (the enhanced renderer's node transforms on the GPU,
/// the enhanced renderer's node places): where its node and the shape's placement put it this frame, how its normals turn, and its
/// sphere in its own space. A build's places are its meshes in the order its vertices name them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshPlace {
    pub now: Mat,
    pub normal: Mat,
    pub sphere: [f32; 4],
}

// A loaded shape goes to the threads that pose and build it.
const _: () = {
    const fn send_sync<T: Send + Sync>() {}
    send_sync::<Loaded>();
};

/// A loaded shape's key: its path ([`shape_key`]), its file's length and hash, and the constructor's sequence names imported into it.
type LoadedKey = (String, usize, u64, Option<Vec<Vec<u8>>>);

/// The most shapes kept loaded between builds (the least recently used dropped first).
pub const MAX_LOADED: usize = 256;
/// The most bytes the shapes kept loaded between builds hold (256 shapes of 16 MiB files were gigabytes): each counts its
/// parsed shape ([`shapes::shape::Shape::approx_bytes`]), its file's length for the render data, and `LOADED_ENTRY_COST`.
pub const MAX_LOADED_BYTES: usize = 256 << 20;
const LOADED_ENTRY_COST: usize = 256;
/// The most shape files whose identity (length and hash) a session remembers (`FILES`).
pub const MAX_KNOWN_FILES: usize = 4096;

/// A shape path as the caches key it: lower case with `\` as `/`, since the asset reader matches names ignoring case; the
/// 300 spellings of one file 300 objects name are one key.
pub fn shape_key(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

fn bytes_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

thread_local! {
    /// The live window builds its control object's frame every frame, and parsing the Player's shape and importing its
    /// constructor's sequences each time took most of 4.5 ms. What a load gives is a function of the key (the
    /// sequence files are read by name: [`clear_loaded`] drops what a session loaded).
    static LOADED: std::cell::RefCell<images::lru::Lru<LoadedKey, Option<Arc<Loaded>>>> =
        std::cell::RefCell::new(images::lru::Lru::new(MAX_LOADED, MAX_LOADED_BYTES));
    /// Each shape file's length and hash by [`shape_key`], read and hashed once a session (a load looked the whole file up and
    /// hashed it before asking [`LOADED`], every frame for every dynamic shape and mounted image). A file not found is not remembered
    /// (it may arrive); [`clear_loaded`] forgets them.
    static FILES: std::cell::RefCell<images::lru::Lru<String, (usize, u64)>> =
        std::cell::RefCell::new(images::lru::Lru::new(MAX_KNOWN_FILES, usize::MAX));
    /// The shape files not found, with the files' generation then ([`crate::textures::files_changed`]): a shape the server names
    /// but never sent was read for every ghost of it every frame.
    static MISSING: std::cell::RefCell<images::lru::Lru<String, u64>> =
        std::cell::RefCell::new(images::lru::Lru::new(MAX_KNOWN_FILES, usize::MAX));
}

/// The shape loader's work since the session started: the shape files read (a file not yet known, or read again to parse), the
/// reads that found no file, and the files parsed (the window's frame breakdown).
static READS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static MISSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PARSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

thread_local! {
    /// The parses the build in progress may still make (None: no limit; [`with_parse_budget`]).
    static PARSES_LEFT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    /// The shapes this thread's builds put off so far ([`put_off_count`]).
    static PUT_OFF: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Puts the parse budget back as it was, however the call it was set for ends.
struct ParseBudgetGuard(Option<usize>);

impl Drop for ParseBudgetGuard {
    fn drop(&mut self) {
        PARSES_LEFT.with(|p| p.set(self.0));
    }
}

/// Runs `f` with at most `parses` shape files parsed by the loads it makes on this thread (None: no limit, as for the control
/// object's shapes inside a frame's budget). A shape past it is put off: not loaded this time, noted and counted
/// ([`put_off_count`]), and loaded by a later build, so a frame that meets hundreds of new or evicted shapes parses a few of
/// them. The budget in place before comes back when `f` returns.
pub fn with_parse_budget<T>(parses: Option<usize>, f: impl FnOnce() -> T) -> T {
    let _restore = ParseBudgetGuard(PARSES_LEFT.with(|p| p.replace(parses)));
    f()
}

/// The shapes this thread's builds put off so far ([`with_parse_budget`]).
pub fn put_off_count() -> u64 {
    PUT_OFF.with(std::cell::Cell::get)
}

/// Whether one more parse fits the budget; `take` uses it up.
fn parse_allowed(take: bool) -> bool {
    PARSES_LEFT.with(|p| match p.get() {
        None => true,
        Some(0) => false,
        Some(n) => {
            if take {
                p.set(Some(n - 1));
            }
            true
        }
    })
}

fn put_off(path: &str, notes: &mut Vec<String>) -> Option<Arc<Loaded>> {
    PUT_OFF.with(|n| n.set(n.get() + 1));
    notes.push(format!("shape {path}: put off to a later frame"));
    None
}

/// The loader's counts so far: (files read, reads that found nothing, files parsed).
pub fn loader_counts() -> (u64, u64, u64) {
    use std::sync::atomic::Ordering::Relaxed;
    (
        READS.load(Relaxed),
        MISSES.load(Relaxed),
        PARSES.load(Relaxed),
    )
}

/// Counts a shape file read and gives it back.
fn counted_read(assets: &dyn Assets, path: &str) -> Option<Vec<u8>> {
    use std::sync::atomic::Ordering::Relaxed;
    READS.fetch_add(1, Relaxed);
    let bytes = assets.read(path);
    if bytes.is_none() {
        MISSES.fetch_add(1, Relaxed);
    }
    bytes
}

/// Forgets the shapes kept loaded between builds and the files' identities (a new connection's files may differ under the same names).
pub fn clear_loaded() {
    LOADED.with(|c| c.borrow_mut().clear());
    FILES.with(|c| c.borrow_mut().clear());
    MISSING.with(|c| c.borrow_mut().clear());
}

/// The shapes kept loaded on this thread and the bytes they hold.
pub fn loaded_stats() -> (usize, usize) {
    LOADED.with(|c| {
        let c = c.borrow();
        (c.len(), c.bytes())
    })
}

/// The shapes one build (a frame's pass) has asked for, by [`shape_key`]: each spelling of a file after the first is a lookup.
#[derive(Default)]
pub struct Models(std::collections::HashMap<String, Option<Arc<Loaded>>>);

impl Models {
    pub fn get(
        &mut self,
        seqs: &dyn Fn(&str) -> Option<Vec<Vec<u8>>>,
        assets: &dyn Assets,
        path: &str,
        notes: &mut Vec<String>,
    ) -> Option<Arc<Loaded>> {
        let key = shape_key(path);
        if let Some(found) = self.0.get(&key) {
            return found.clone();
        }
        let loaded = load(seqs, assets, path, notes);
        self.0.insert(key, loaded.clone());
        loaded
    }
}

/// `load_uncached` through the loaded shapes kept between builds: the file is read only when the session has not seen it or its
/// shape is not kept.
/// `seqs`: the sequences a shape's constructor imports into it, by the shape's path ([`crate::player::import`]).
pub fn load(
    seqs: &dyn Fn(&str) -> Option<Vec<Vec<u8>>>,
    assets: &dyn Assets,
    path: &str,
    notes: &mut Vec<String>,
) -> Option<Arc<Loaded>> {
    let name = shape_key(path);
    let mut fresh = None;
    let ident = match FILES.with(|f| f.borrow_mut().get(&name).copied()) {
        Some(ident) => ident,
        None => {
            let generation = crate::textures::files_generation();
            if MISSING.with(|m| m.borrow_mut().get(&name).copied()) == Some(generation) {
                return None;
            }
            // A file not known yet is parsed (unless its content is kept under another name): not read past the budget.
            if !parse_allowed(false) {
                return put_off(path, notes);
            }
            let Some(bytes) = counted_read(assets, path) else {
                MISSING.with(|m| m.borrow_mut().insert(name.clone(), generation, 1));
                return None;
            };
            let ident = (bytes.len(), bytes_hash(&bytes));
            FILES.with(|f| f.borrow_mut().insert(name.clone(), ident, 1));
            fresh = Some(bytes);
            ident
        }
    };
    let seqs = seqs(path);
    let mut key: LoadedKey = (name, ident.0, ident.1, seqs);
    if let Some(found) = LOADED.with(|c| c.borrow_mut().get(&key).cloned()) {
        return found;
    }
    if !parse_allowed(true) {
        return put_off(path, notes);
    }
    let bytes = match fresh {
        Some(b) => b,

        None => {
            // Read again for the parse: a file that changed under its name since is loaded as it is now, under its new identity.
            let b = counted_read(assets, path)?;
            let now = (b.len(), bytes_hash(&b));
            if now != ident {
                FILES.with(|f| f.borrow_mut().insert(key.0.clone(), now, 1));
                (key.1, key.2) = now;
            }
            b
        }
    };
    let file_len = bytes.len();
    PARSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let loaded = load_uncached(assets, path, bytes, key.3.as_deref(), notes).map(Arc::new);
    let cost = LOADED_ENTRY_COST
        + key.0.len()
        + loaded
            .as_ref()
            .map_or(0, |l| l.model.shape.approx_bytes() + file_len);
    // One shape above the whole budget is not kept (it is loaded again next build); otherwise the least recently used make room.
    LOADED.with(|c| c.borrow_mut().insert(key, loaded.clone(), cost));
    loaded
}

fn load_uncached(
    assets: &dyn Assets,
    path: &str,
    bytes: Vec<u8>,
    seqs: Option<&[Vec<u8>]>,
    notes: &mut Vec<String>,
) -> Option<Loaded> {
    let mut charge = |_: u64| true;
    let bytes = torque_import::native::shape_file(path.as_bytes(), bytes, &mut charge).ok()?;
    let mut model = dts::parse(&bytes, &mut charge).ok()?;
    let dir = path.rsplit_once('/').map_or("", |(d, _)| d).to_string();
    if let Some(seqs) = seqs {
        crate::player::import(&mut model.shape, &dir, seqs, assets, notes);
    }
    let ifls = ifl_lists(&model, path, assets);
    let spheres = model
        .shape
        .meshes
        .iter()
        .map(|m| m.as_ref().map_or([0.0; 4], |m| sphere_of(m.verts())))
        .collect();
    let skinned = model
        .render
        .iter()
        .flatten()
        .any(|r| matches!(r.extra, MeshExtra::Skin(_)));
    let detail_vertices = (0..model.shape.details.len())
        .map(|dl| detail_vertices(&model.shape, dl))
        .collect();
    Some(Loaded {
        model,
        ifls,
        spheres,
        skinned,
        detail_vertices,
    })
}

/// The most vertices a build of detail `dl` makes: three for every triangle of each object's mesh at the level.
fn detail_vertices(s: &shapes::shape::Shape, dl: usize) -> usize {
    let Some(detail) = s.details.get(dl) else {
        return 0;
    };
    let Some(ss) = usize::try_from(detail.sub_shape)
        .ok()
        .and_then(|i| s.sub_shapes.get(i))
    else {
        return 0;
    };
    let first = ss.first_object.max(0) as usize;
    let objects = s
        .objects
        .iter()
        .skip(first)
        .take(ss.num_objects.max(0) as usize);
    objects
        .filter(|o| detail.object_detail >= 0 && detail.object_detail < o.num_meshes)
        .filter_map(|o| {
            let mi = usize::try_from(o.start_mesh_index.checked_add(detail.object_detail)?).ok()?;
            s.meshes.get(mi)?.as_ref()
        })
        .flat_map(|m| &m.primitives)
        .map(|p| {
            let n = usize::from(p.num_elements);
            if p.material & PRIM_TYPE_MASK == 0 {
                n
            } else {
                3 * n.saturating_sub(2)
            }
        })
        .fold(0usize, usize::saturating_add)
}

/// Per IFL material of a shape at `path`: its name (`name.ifl`), its material slot and the frames' texture paths.
pub fn ifl_lists(
    model: &Model,
    path: &str,
    assets: &dyn Assets,
) -> Vec<(String, usize, Vec<String>)> {
    let dir = path.rsplit_once('/').map_or("", |(d, _)| d).to_string();
    let shape = &model.shape;
    let mut ifls = Vec::new();
    for ifl in &shape.ifl_materials {
        let name = usize::try_from(ifl.name)
            .ok()
            .and_then(|n| shape.names.get(n))
            .map(|n| String::from_utf8_lossy(n).to_string())
            .unwrap_or_default();
        let file = if dir.is_empty() {
            name.clone()
        } else {
            format!("{dir}/{name}")
        };
        let frames = crate::player::ifl_frames(assets, &file).unwrap_or_else(|| {
            // The frames the file was exported with: materials of the list.
            let first = ifl.first_frame.max(0) as usize;
            (first..first + ifl.num_frames.max(0) as usize)
                .filter_map(|i| shape.materials.get(i)?.name.clone())
                .map(|n| format!("{dir}/{}", String::from_utf8_lossy(&n)))
                .collect()
        });
        ifls.push((name, ifl.material_slot.max(0) as usize, frames));
    }
    ifls
}

/// What a ghost tells the drawing of its shape.
pub struct ShapeParts<'a> {
    pub sub_shape: i32,
    pub object_detail: i32,
    /// Name indices of the hidden objects.
    pub hidden: &'a [u8],
    pub colors: Option<&'a [Option<[u8; 4]>]>,
    /// The texture of each IFL material slot.
    pub ifl_frames: &'a BTreeMap<usize, String>,
    /// Hidden objects by object index (set by the host).
    pub hidden_objects: Option<&'a [bool]>,
    /// A texture in place of the first material's (a host's skin).
    pub skin: Option<&'a str>,
    /// One tint every object draws with, the per-object tints unused.
    pub color_all: Option<[f32; 4]>,
}

/// GL's state as the shape code leaves it between primitives.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Gl {
    blend: bool,
    blend_func: BlendFunc,
    depth_write: bool,
    texturing: bool,
    lighting: bool,
    color_material: bool,
    decal: bool,
    color: [f32; 4],
    /// The material's ambient and diffuse when `GL_COLOR_MATERIAL` is off.
    material: [f32; 4],
    /// `setMaterial`'s current flags and material.
    flags: u32,
    current: u32,
    /// The bound texture (an index of the frame's set) and its sampler.
    texture: Option<(usize, Sampler)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlendFunc {
    Alpha,
    Additive,
    Subtractive,
    /// `ONE, ZERO`: what the tint block leaves after a translucent tint.
    Replace,
}

/// `v.round() as u8` for `v` in 0..=255 (NaN gives 0), without the C library's `roundf` (a call a primitive on Windows: a sixth of
/// a shape's build). `v - trunc(v)` is exact there, so the halves round as `round` rounds them.
fn round_byte(v: f32) -> u8 {
    let t = v as u8;
    let f = v - f32::from(t);
    t.saturating_add(u8::from(f >= 0.5))
}

impl Gl {
    /// `initMaterials`, with lighting on (the lights were installed).
    fn init() -> Gl {
        Gl {
            blend: false,
            blend_func: BlendFunc::Alpha,
            depth_write: true,
            texturing: false,
            lighting: true,
            color_material: false,
            decal: false,
            color: [1.0; 4],
            material: [1.0; 4],
            flags: MAT_S_WRAP | MAT_T_WRAP,
            current: PRIM_NO_MATERIAL,
            texture: None,
        }
    }

    /// The draw state of the next primitive: its vertices' colour and the fixed-function settings.
    fn snapshot(&self) -> Snap {
        let texture = if self.texturing { self.texture } else { None };
        let env = match (texture.is_some(), self.decal) {
            (false, _) => TexEnv::None,
            (true, true) => TexEnv::Decal,
            (true, false) => TexEnv::Modulate,
        };
        let blend = if !self.blend {
            Blend::Opaque
        } else {
            match self.blend_func {
                BlendFunc::Alpha => Blend::Alpha,
                BlendFunc::Additive => Blend::Additive,
                BlendFunc::Subtractive | BlendFunc::Replace => Blend::Opaque,
            }
        };
        let c = if self.lighting && !self.color_material {
            self.material
        } else {
            self.color
        };
        let byte = |v: f32| round_byte(v.clamp(0.0, 1.0) * 255.0);
        Snap {
            lighting: self.lighting,
            tex_env: env,
            texture,
            blend,
            depth_write: self.depth_write,
            rgba: [byte(c[0]), byte(c[1]), byte(c[2]), byte(c[3])],
        }
    }
}

/// What one primitive is drawn with.
struct Snap {
    lighting: bool,
    tex_env: TexEnv,
    texture: Option<(usize, Sampler)>,
    blend: Blend,
    depth_write: bool,
    rgba: [u8; 4],
}

/// The material word of a primitive as `setMaterial` takes it, its flags, and its texture (an IFL slot's current frame).
fn material_texture(
    model: &Model,
    index: usize,
    parts: &ShapeParts<'_>,
    textures: &mut TextureSet<'_>,
    dir: &str,
) -> (u32, Option<usize>) {
    let s = &model.shape;
    let Some(m) = s.materials.get(index) else {
        return (0, None);
    };
    if index == 0
        && let Some(skin) = parts.skin
    {
        return (m.flags & !MAT_IFL, textures.get(skin));
    }
    if m.flags & MAT_IFL != 0 {
        let tex = parts
            .ifl_frames
            .get(&index)
            .and_then(|path| textures.get(path));
        return (m.flags & !MAT_IFL, tex);
    }
    let tex = m.name.as_ref().and_then(|n| {
        let name = String::from_utf8_lossy(n);
        if dir.is_empty() {
            textures.get(&name)
        } else {
            textures.get(&format!("{dir}/{name}"))
        }
    });
    (m.flags, tex)
}

/// The sampler of a material with `flags`: repeated per its S and T flags, mipmapped unless `NoMipMap`, linear, or with the
/// nearest bit sharp texels near and far (the mipmap levels still blended between, as an interior's nearest material).
pub fn material_sampler(flags: u32) -> Sampler {
    let near = if flags & MAT_NEAREST != 0 {
        Filter::Nearest
    } else {
        Filter::Linear
    };
    Sampler {
        repeat: flags & (MAT_S_WRAP | MAT_T_WRAP) != 0,
        mag: near,
        min: near,
        mip: Filter::Linear,
        mipmaps: flags & MAT_NO_MIPMAP == 0,
    }
}

/// `setMaterial` for the material word `word`.
fn set_material(gl: &mut Gl, word: u32, flags: u32, tex: Option<usize>) {
    if (word | gl.current) & PRIM_NO_MATERIAL != 0 {
        if word & PRIM_NO_MATERIAL != 0 {
            gl.texturing = false;
            gl.blend = false;
            gl.depth_write = true;
            gl.flags &= !MAT_TRANSLUCENT;
            gl.current = word;
            return;
        }
        gl.texturing = true;
    }
    let index = word & PRIM_MATERIAL_MASK;
    // Environment mapping is off (it is not drawn): its bit never counts as a change, nor the nearest bit (TGE has none).
    let changed = (gl.flags ^ flags) & !(0x40 | MAT_NEAREST);
    gl.flags = flags;
    gl.current = index;
    gl.texture = tex.map(|t| (t, material_sampler(flags)));
    if changed == 0 {
        return;
    }
    if flags & MAT_TRANSLUCENT == 0 {
        gl.blend = false;
        gl.depth_write = true;
    } else {
        gl.blend = true;
        gl.depth_write = false;
    }
    if changed & (MAT_ADDITIVE | MAT_SUBTRACTIVE) != 0 {
        gl.blend_func = if flags & MAT_ADDITIVE != 0 {
            BlendFunc::Additive
        } else if flags & MAT_SUBTRACTIVE != 0 {
            BlendFunc::Subtractive
        } else {
            BlendFunc::Alpha
        };
    }
    if changed & MAT_SELF_ILLUMINATING != 0 {
        gl.lighting = flags & MAT_SELF_ILLUMINATING == 0;
    }
}

/// The tint block for the texture `tex` (whether it has the alpha flag) and the colour `c`.
fn color_block(gl: &mut Gl, has_alpha: bool, c: [f32; 4]) {
    if !has_alpha {
        if c[3] >= 1.0 {
            gl.texturing = true;
        } else {
            gl.color_material = true;
            gl.color = [1.0, 1.0, 1.0, c[3]];
            gl.texturing = true;
            gl.blend = true;
            gl.blend_func = BlendFunc::Alpha;
            gl.depth_write = true;
        }
        gl.decal = false;
    } else {
        if c[3] < 1.0 {
            gl.blend = true;
            gl.blend_func = BlendFunc::Alpha;
            gl.depth_write = true;
        }
        gl.color_material = true;
        gl.color = c;
        gl.texturing = true;
        gl.decal = true;
    }
}

/// The end of a mesh drawn with a colour.
fn color_end(gl: &mut Gl, c: [f32; 4]) {
    if c[3] < 1.0 {
        gl.blend = false;
        gl.blend_func = BlendFunc::Replace;
        gl.depth_write = true;
    }
    gl.color = [1.0; 4];
    gl.color_material = false;
    gl.decal = false;
    gl.texturing = false;
}

/// `TSSkinMesh::updateSkin`: each bone's matrix is its node's times its initial transform; each vertex and normal the sum of its
/// weighted bones' images of the initial ones (the normals not renormalised).
fn skin(sk: &SkinData, instance: &ShapeInstance) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let bones: Vec<Mat> = sk
        .node_index
        .iter()
        .zip(sk.initial_transforms.iter())
        .take(sk.num_nodes.min(sk.num_transforms))
        .map(|(&node, initial)| {
            let n = usize::try_from(node)
                .ok()
                .and_then(|i| instance.node_transform(i))
                .map(|m| Mat { m: *m })
                .unwrap_or(Mat::IDENTITY);
            Mat::mul(&n, &Mat { m: *initial })
        })
        .collect();
    let count = sk.num_initial_verts.min(sk.initial_verts.len());
    let mut verts = vec![[0.0f32; 3]; count];
    let mut norms = vec![[0.0f32; 3]; count];
    for i in 0..sk.num_weights.min(sk.vertex_index.len()) {
        let (Some(&v), Some(&b), Some(&w)) = (
            sk.vertex_index.get(i),
            sk.bone_index.get(i),
            sk.weight.get(i),
        ) else {
            continue;
        };
        let (Ok(v), Ok(b)) = (usize::try_from(v), usize::try_from(b)) else {
            continue;
        };
        let (Some(bone), Some(p), Some(slot)) =
            (bones.get(b), sk.initial_verts.get(v), verts.get_mut(v))
        else {
            continue;
        };
        let q = bone.mul_p(*p);
        for k in 0..3 {
            slot[k] += q[k] * w;
        }
        if let (Some(n), Some(slot)) = (sk.initial_normals.get(v), norms.get_mut(v)) {
            let q = bone.mul_v(*n);
            for k in 0..3 {
                slot[k] += q[k] * w;
            }
        }
    }
    (verts, norms)
}

/// The shape at `path` at rest (no thread run), its most detailed level, in object space: the draws a map's static shapes are
/// merged from ([`crate::map`]), and whether it has translucency. None when the file does not load.
pub fn rest_draws(
    seqs: &dyn Fn(&str) -> Option<Vec<Vec<u8>>>,
    assets: &dyn Assets,
    path: &str,
    textures: &mut TextureSet<'_>,
    notes: &mut Vec<String>,
) -> Option<(Vec<Draw>, bool)> {
    let loaded = load(seqs, assets, path, notes)?;
    let model = &loaded.model;
    let detail = model.shape.details[highest_detail(model)?];
    let mut charge = |_: u64| true;
    let instance = ShapeInstance::new(&model.shape, &mut charge).ok()?;
    // Each IFL material at its first frame.
    let ifl: BTreeMap<usize, String> = loaded
        .ifls
        .iter()
        .filter_map(|(_, slot, frames)| Some((*slot, frames.first()?.clone())))
        .collect();
    let parts = ShapeParts {
        sub_shape: detail.sub_shape,
        object_detail: detail.object_detail,
        hidden: &[],
        colors: None,
        ifl_frames: &ifl,
        hidden_objects: None,
        skin: None,
        color_all: None,
    };
    Some(shape_draws_built(
        model,
        &instance,
        parts,
        &Mat::IDENTITY,
        &Mat::IDENTITY,
        true,
        &[],
        textures,
        path,
    ))
}

/// The draws of one shape in order, and whether it has translucency.
#[allow(clippy::too_many_arguments)]
pub fn shape_draws(
    model: &Model,
    instance: &ShapeInstance,
    parts: ShapeParts<'_>,
    xform: &Mat,
    rotation: &Mat,
    unscaled: bool,
    point: &[PointLight],
    textures: &mut TextureSet<'_>,
    label: &str,
) -> (Vec<Draw>, bool) {
    let key = crate::shapecache::on().then(|| {
        shape_key_bytes(
            model, instance, &parts, xform, rotation, unscaled, point, false,
        )
    });
    shape_draws_keyed(
        model, instance, parts, xform, rotation, unscaled, point, textures, label, key, None,
    )
}

/// [`shape_draws`] with its cache key made already ([`shape_key_bytes`]; none: the cache is off, the shape built). With `nodes`
/// (the meshes' spheres, [`Loaded::node_spheres`]; the key made for it) a kept build's vertices stay in their meshes' own space
/// and its call carries where its meshes are this frame ([`MeshPlace`]): a build whose nodes moved is the same build. Its draws
/// name no point lights then (the enhanced renderer lights every pixel from the frame's).
#[allow(clippy::too_many_arguments)]
pub fn shape_draws_keyed(
    model: &Model,
    instance: &ShapeInstance,
    parts: ShapeParts<'_>,
    xform: &Mat,
    rotation: &Mat,
    unscaled: bool,
    point: &[PointLight],
    textures: &mut TextureSet<'_>,
    label: &str,
    key: Option<Vec<u8>>,
    nodes: Option<&[[f32; 4]]>,
) -> (Vec<Draw>, bool) {
    let Some(key) = key.filter(|_| crate::shapecache::on()) else {
        let (draws, translucent) = shape_draws_built(
            model, instance, parts, xform, rotation, unscaled, point, textures, label,
        );
        // A build not kept beside kept ones of its ghost: its call says its draws are in the frame's lists, or the ghost's object,
        // made from its calls alone, leaves them out.
        crate::shapecache::unkept(draws.len(), translucent);
        return (draws, translucent);
    };
    let point = if nodes.is_some() { &[][..] } else { point };
    // Kept draws stay in the cache (the call returns none: its `Call` says where they are).
    if let Some(translucent) = crate::shapecache::lookup(label, &key, textures) {
        if let Some(spheres) = nodes {
            crate::shapecache::set_places(mesh_places(
                model, instance, &parts, xform, rotation, spheres,
            ));
        }
        return (Vec::new(), translucent);
    }
    let deferred = textures.deferred;
    let recording = textures.recording.replace(Vec::new());
    let places =
        nodes.map(|spheres| mesh_places(model, instance, &parts, xform, rotation, spheres));
    let (draws, translucent, mut fill) = shape_draws_planned(
        model, instance, parts, xform, rotation, unscaled, point, textures, label,
    );
    fill.node_space = places.is_some();
    let lookups = std::mem::replace(&mut textures.recording, recording).unwrap_or_default();
    let draws = crate::shapecache::keep(
        label,
        key,
        draws,
        translucent,
        lookups,
        textures.deferred != deferred,
        fill,
    );
    if let Some(places) = places {
        crate::shapecache::set_places(places);
    }
    (draws, translucent)
}

/// Whether object `oi` of a build is drawn at all (`shape_draws_planned` skips a hidden one and one without primitives; a
/// drawn one has a [`Fill`] object, in object order): its mesh index.
fn drawn_mesh(s: &shapes::shape::Shape, parts: &ShapeParts<'_>, oi: usize) -> Option<usize> {
    let obj = s.objects.get(oi)?;
    if parts.object_detail < 0 || parts.object_detail >= obj.num_meshes {
        return None;
    }
    let mi = (obj.start_mesh_index + parts.object_detail) as usize;
    let mesh = s.meshes.get(mi)?.as_ref()?;
    (!object_hidden(parts, obj, oi) && !mesh.primitives.is_empty()).then_some(mi)
}

/// Whether the build leaves object `oi` out: hidden by name (the record's hidden nodes) or by index, its visibility under
/// [`MIN_VISIBILITY`].
fn object_hidden(parts: &ShapeParts<'_>, obj: &shapes::shape::Object, oi: usize) -> bool {
    let hidden = u8::try_from(obj.name).is_ok_and(|n| parts.hidden.contains(&n))
        || parts
            .hidden_objects
            .is_some_and(|h| h.get(oi).copied().unwrap_or(false));
    let visibility: f32 = if hidden { 0.0 } else { 1.0 };
    visibility <= MIN_VISIBILITY
}

/// Where each drawn mesh of a build is this frame ([`MeshPlace`]: its [`Fill`] objects' placement, in their order), for a build
/// whose vertices stay in their meshes' own space.
fn mesh_places(
    model: &Model,
    instance: &ShapeInstance,
    parts: &ShapeParts<'_>,
    xform: &Mat,
    rotation: &Mat,
    spheres: &[[f32; 4]],
) -> Vec<MeshPlace> {
    let s = &model.shape;
    let Some(ss) = usize::try_from(parts.sub_shape)
        .ok()
        .and_then(|i| s.sub_shapes.get(i))
    else {
        return Vec::new();
    };
    let first = ss.first_object.max(0) as usize;
    let mut out = Vec::new();
    for oi in first..first + ss.num_objects.max(0) as usize {
        let Some(mi) = drawn_mesh(s, parts, oi) else {
            continue;
        };
        if model.render.get(mi).is_none_or(Option::is_none) {
            continue;
        }
        let node = s
            .objects
            .get(oi)
            .and_then(|o| usize::try_from(o.node).ok())
            .and_then(|n| instance.node_transform(n))
            .map(|m| Mat { m: *m })
            .unwrap_or(Mat::IDENTITY);
        out.push(MeshPlace {
            now: Mat::mul(xform, &node),
            normal: Mat::mul(rotation, &node),
            sphere: spheres.get(mi).copied().unwrap_or([0.0; 4]),
        });
    }
    out
}

/// Everything `shape_draws_built` reads but the textures' names (their lookups are checked apart): the files' generation, the
/// shape, its nodes' transforms, its parts, its placement and the point lights.
#[allow(clippy::too_many_arguments)]
pub fn shape_key_bytes(
    model: &Model,
    instance: &ShapeInstance,
    parts: &ShapeParts<'_>,
    xform: &Mat,
    rotation: &Mat,
    unscaled: bool,
    point: &[PointLight],
    nodes: bool,
) -> Vec<u8> {
    let mut k = crate::shapecache::Key::default();
    k.u64(crate::textures::files_generation())
        .u64(std::ptr::from_ref(model) as usize as u64)
        .u32(u32::from(nodes));
    // A build in its meshes' own space is the same wherever its nodes and the shape are (and names no point light).
    if !nodes {
        k.u64(instance.node_transforms().len() as u64);
        for m in instance.node_transforms() {
            k.f32s(m);
        }
    }
    k.u32(parts.sub_shape as u32)
        .u32(parts.object_detail as u32)
        .bytes(parts.hidden);
    match parts.colors {
        None => {
            k.u32(0);
        }
        Some(c) => {
            k.u32(1).u64(c.len() as u64);
            for c in c {
                match c {
                    None => k.u32(0),
                    Some(c) => k.u32(1).bytes(c),
                };
            }
        }
    }
    k.u64(parts.ifl_frames.len() as u64);
    for (i, p) in parts.ifl_frames {
        k.u64(*i as u64).bytes(p.as_bytes());
    }
    match parts.hidden_objects {
        None => k.u32(0),
        Some(h) => k
            .u32(1)
            .bytes(&h.iter().map(|b| u8::from(*b)).collect::<Vec<u8>>()),
    };
    match parts.skin {
        None => k.u32(0),
        Some(s) => k.u32(1).bytes(s.as_bytes()),
    };
    match parts.color_all {
        None => k.u32(0),
        Some(c) => k.u32(1).f32s(&c),
    };
    if !nodes {
        k.f32s(&xform.m).f32s(&rotation.m).u32(u32::from(unscaled));
        k.u64(point.len() as u64);
        for p in point {
            k.f32s(&p.position).f32s(&p.diffuse).f32s(&p.attenuation);
        }
    }
    k.0
}

/// [`shape_draws`] built.
#[allow(clippy::too_many_arguments)]
fn shape_draws_built(
    model: &Model,
    instance: &ShapeInstance,
    parts: ShapeParts<'_>,
    xform: &Mat,
    rotation: &Mat,
    unscaled: bool,
    point: &[PointLight],
    textures: &mut TextureSet<'_>,
    label: &str,
) -> (Vec<Draw>, bool) {
    let (mut draws, translucent, fill) = shape_draws_planned(
        model, instance, parts, xform, rotation, unscaled, point, textures, label,
    );
    let t = web_time::Instant::now();
    fill.view().run(&mut draws, None);
    INLINE_FILL.with(|f| f.set(f.get() + t.elapsed().as_nanos() as u64));
    (draws, translucent)
}

thread_local! {
    /// The vertices filled inside builds without the cache on this thread since the last [`take_inline_fill`] (ns).
    static INLINE_FILL: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The time this thread's builds without the cache spent filling their vertices since the last call (ns), set back to 0.
pub fn take_inline_fill() -> u64 {
    INLINE_FILL.with(|f| f.replace(0))
}

/// One object's vertices for a [`Fill`]: its mesh's positions and normals (or a skin's, made at the build), its texture
/// coordinates, and how it is placed (`m`) and turned (`n_xform`; a scaled shape's normals renormalised).
pub struct FillObject {
    positions: Points,
    normals: Points,
    tverts: Arc<[[f32; 2]]>,
    m: Mat,
    n_xform: Mat,
    unscaled: bool,
}

/// Points of a mesh: the first `n` of its shared array (all of them when it is shorter), or a skin's own.
enum Points {
    Shared(Arc<[[f32; 3]]>, usize),
    Own(Vec<[f32; 3]>),
}

impl Points {
    fn slice(&self) -> &[[f32; 3]] {
        match self {
            Points::Shared(p, n) => p.get(..*n).unwrap_or(p),
            Points::Own(v) => v,
        }
    }
}

/// One primitive's part of a draw: its object (in `Fill::objects`), its kind and indices, and its vertices' colour.
pub struct Segment {
    object: usize,
    kind: u32,
    indices: Vec<u16>,
    rgba: [u8; 4],
}

/// How a planned build's draws get their vertices (`shape_draws_planned`): the objects, and each draw's segments in order.
/// Filling is pure (the same vertices on any thread): a control sync fills its kept builds together over a few threads
/// ([`crate::shapecache`]).
#[derive(Default)]
pub struct Fill {
    objects: Vec<FillObject>,
    draws: Vec<Vec<Segment>>,
    /// The vertices stay in their meshes' own space (each names its object: [`MeshPlace`]).
    pub node_space: bool,
}

impl Fill {
    /// The fill with its arrays borrowed (for another thread).
    pub fn view(&self) -> FillView<'_> {
        FillView {
            objects: self
                .objects
                .iter()
                .map(|o| ObjectView {
                    positions: o.positions.slice(),
                    normals: o.normals.slice(),
                    tverts: &o.tverts,
                    m: o.m,
                    n_xform: o.n_xform,
                    unscaled: o.unscaled,
                })
                .collect(),
            draws: &self.draws,
            node_space: self.node_space,
        }
    }
}

struct ObjectView<'a> {
    positions: &'a [[f32; 3]],
    normals: &'a [[f32; 3]],
    tverts: &'a [[f32; 2]],
    m: Mat,
    n_xform: Mat,
    unscaled: bool,
}

/// A [`Fill`] whose arrays are borrowed slices (sent to the thread that fills its draws).
pub struct FillView<'a> {
    objects: Vec<ObjectView<'a>>,
    draws: &'a [Vec<Segment>],
    node_space: bool,
}

/// The triangles of a primitive of `kind` over `idx`, as GL turns strips and fans into them.
fn each_triangle(kind: u32, idx: &[u16], mut f: impl FnMut([u16; 3])) {
    match kind {
        PRIM_STRIP => {
            for i in 2..idx.len() {
                if i % 2 == 0 {
                    f([idx[i - 2], idx[i - 1], idx[i]]);
                } else {
                    f([idx[i - 1], idx[i - 2], idx[i]]);
                }
            }
        }
        PRIM_FAN => {
            for i in 2..idx.len() {
                f([idx[0], idx[i - 1], idx[i]]);
            }
        }
        _ => {
            for t in idx.as_chunks::<3>().0 {
                f([t[0], t[1], t[2]]);
            }
        }
    }
}

impl FillView<'_> {
    /// The vertices this fill makes at most (its work).
    pub fn work(&self) -> usize {
        self.draws
            .iter()
            .flatten()
            .map(|g| 3 * g.indices.len())
            .sum()
    }

    /// `draws` (the planned build's, in order) given their vertices. In its meshes' own space ([`Fill::node_space`]) the vertices
    /// are the meshes' own and `nodes` gets each draw's vertices' objects (their [`MeshPlace`]s).
    pub fn run(&self, draws: &mut [Draw], mut nodes: Option<&mut Vec<Vec<u16>>>) {
        if let Some(n) = nodes.as_deref_mut() {
            n.clear();
            n.resize(draws.len(), Vec::new());
        }
        // Each object's vertices placed once (a vertex is in several triangles): where it is and its normal. GL lights the
        // normal as the mesh has it (no GL_NORMALIZE): a foliage mesh may have normals of length 0 to 7e-7, which only the
        // ambient term lights. A scaled shape's normals are renormalised (GL's inverse transpose would shrink or grow them).
        let placed: Vec<Vec<([f32; 3], [f32; 3])>> = self
            .objects
            .iter()
            .map(|o| {
                if self.node_space {
                    return o
                        .positions
                        .iter()
                        .copied()
                        .zip(o.normals.iter().copied())
                        .collect();
                }
                o.positions
                    .iter()
                    .zip(o.normals)
                    .map(|(pos, n)| {
                        let normal = if o.unscaled {
                            o.n_xform.mul_v(*n)
                        } else {
                            unit_or_same(o.n_xform.mul_v(*n).into()).to_array()
                        };
                        (o.m.mul_p(*pos), normal)
                    })
                    .collect()
            })
            .collect();
        for (k, (d, segments)) in draws.iter_mut().zip(self.draws).enumerate() {
            d.vertices
                .reserve(segments.iter().map(|g| 3 * g.indices.len()).sum());
            let mut named = nodes.as_deref_mut().and_then(|n| n.get_mut(k));
            for g in segments {
                let (Some(points), Some(o)) = (placed.get(g.object), self.objects.get(g.object))
                else {
                    continue;
                };
                let object = u16::try_from(g.object).unwrap_or(u16::MAX);
                each_triangle(g.kind, &g.indices, |tri| {
                    for i in tri {
                        let i = i as usize;
                        let Some(&(position, normal)) = points.get(i) else {
                            continue;
                        };
                        d.vertices.push(Vertex {
                            position,
                            normal,
                            color: g.rgba,
                            uv: o.tverts.get(i).copied().unwrap_or([0.0; 2]),
                        });
                        if let Some(n) = named.as_deref_mut() {
                            n.push(object);
                        }
                    }
                });
            }
        }
    }
}

/// [`shape_draws`]'s draws and translucency with no vertices yet, and the [`Fill`] that gives them theirs.
#[allow(clippy::too_many_arguments)]
fn shape_draws_planned(
    model: &Model,
    instance: &ShapeInstance,
    parts: ShapeParts<'_>,
    xform: &Mat,
    rotation: &Mat,
    unscaled: bool,
    point: &[PointLight],
    textures: &mut TextureSet<'_>,
    label: &str,
) -> (Vec<Draw>, bool, Fill) {
    let mut fill = Fill::default();
    let s = &model.shape;
    let Some(ss) = usize::try_from(parts.sub_shape)
        .ok()
        .and_then(|i| s.sub_shapes.get(i))
    else {
        return (Vec::new(), false, fill);
    };
    // The colour of each draw's last vertex (None: it has none yet), for the primitives that join it.
    let mut last_color: Vec<Option<[u8; 4]>> = Vec::new();
    let dir = shape_dir(label).to_string();
    let first = ss.first_object.max(0) as usize;
    let objects = first..first + ss.num_objects.max(0) as usize;
    let mut out: Vec<Draw> = Vec::new();
    let mut translucent = false;
    let mut gl = Gl::init();
    for oi in objects {
        let Some(obj) = s.objects.get(oi) else {
            continue;
        };
        if parts.object_detail < 0 || parts.object_detail >= obj.num_meshes {
            continue;
        }
        let mi = (obj.start_mesh_index + parts.object_detail) as usize;
        let (Some(Some(mesh)), Some(Some(render))) = (s.meshes.get(mi), model.render.get(mi))
        else {
            continue;
        };
        // TSShapeInstance::hasTranslucency looks at every mesh of the detail, hidden or not.
        for p in &mesh.primitives {
            if p.material & PRIM_NO_MATERIAL == 0
                && s.materials
                    .get((p.material & PRIM_MATERIAL_MASK) as usize)
                    .is_some_and(|m| m.flags & MAT_TRANSLUCENT != 0)
            {
                translucent = true;
            }
        }
        if object_hidden(&parts, obj, oi) || mesh.primitives.is_empty() {
            continue;
        }
        let visibility: f32 = 1.0;
        let color = parts.color_all.or_else(|| {
            parts
                .colors
                .and_then(|c| c.get(oi).copied().flatten())
                .map(|c| {
                    let mut f = c.map(|b| f32::from(b) / 255.0);
                    if visibility <= 1.0 {
                        f[3] *= visibility;
                    }
                    f
                })
        });
        // An object with no node (a skin: its vertices are already in the shape's space) draws untransformed.
        let node = usize::try_from(obj.node)
            .ok()
            .and_then(|n| instance.node_transform(n))
            .map(|m| Mat { m: *m })
            .unwrap_or(Mat::IDENTITY);
        let m = Mat::mul(xform, &node);
        let n_xform = Mat::mul(rotation, &node);
        let MeshRender {
            tverts,
            normals,
            extra,
            ..
        } = render;
        let (positions, normals) = match extra {
            MeshExtra::Skin(sk) => {
                let (v, n) = skin(sk, instance);
                (Points::Own(v), Points::Own(n))
            }
            _ => (
                Points::Shared(Arc::clone(&mesh.positions), mesh.num_verts),
                Points::Shared(Arc::clone(normals), usize::MAX),
            ),
        };
        // Whether a corner index has a vertex (and a normal).
        let points = positions.slice().len().min(normals.slice().len());
        let object = fill.objects.len();
        fill.objects.push(FillObject {
            positions,
            normals,
            tverts: Arc::clone(tverts),
            m,
            n_xform,
            unscaled,
        });
        // The tint block for the first primitive's texture, before any setMaterial.
        if let Some(c) = color {
            let index = (mesh.primitives[0].material & PRIM_MATERIAL_MASK) as usize;
            let (_, tex) = material_texture(model, index, &parts, textures, &dir);
            color_block(&mut gl, tex.is_some_and(|t| textures.has_alpha(t)), c);
        }
        for p in &mesh.primitives {
            if (p.material ^ gl.current) & 0x1fff_ffff != 0 {
                let index = (p.material & PRIM_MATERIAL_MASK) as usize;
                let (flags, tex) = material_texture(model, index, &parts, textures, &dir);
                set_material(&mut gl, p.material, flags, tex);
                if let Some(c) = color {
                    color_block(&mut gl, tex.is_some_and(|t| textures.has_alpha(t)), c);
                }
            }
            let start = p.start as usize;
            let end = start + p.num_elements as usize;
            let Some(idx) = mesh.indices.get(start..end) else {
                continue;
            };
            let Snap {
                lighting,
                tex_env,
                texture,
                blend,
                depth_write,
                rgba,
            } = gl.snapshot();
            // Consecutive primitives with one state share a draw: this one's vertices go straight into the last draw then.
            let joins = out.last().is_some_and(|last| {
                last.lighting == lighting
                    && last.tex_env == tex_env
                    && last.texture == texture
                    && last.blend == blend
                    && last.depth_write == depth_write
                    && last_color.last().copied().flatten() == Some(rgba)
            });
            let kind = p.material & PRIM_TYPE_MASK;
            // Whether the primitive gives its draw a vertex (the next one's join reads its colour).
            let mut gives = false;
            each_triangle(kind, idx, |t| {
                gives |= t.iter().any(|&i| (i as usize) < points);
            });
            let segment = Segment {
                object,
                kind,
                indices: idx.to_vec(),
                rgba,
            };
            if joins {
                if let (Some(segments), Some(c)) = (fill.draws.last_mut(), last_color.last_mut()) {
                    segments.push(segment);
                    if gives {
                        *c = Some(rgba);
                    }
                }
                continue;
            }
            fill.draws.push(vec![segment]);
            last_color.push(gives.then_some(rgba));
            out.push(Draw {
                label: format!("shape {label} object {oi}"),
                vertices: Vec::new(),
                lighting,
                tex_env,
                texture,
                blend,
                cull: Cull::BackCw,
                depth_test: true,
                depth_write,
                fog: false,
                lights: point.to_vec(),
                vertex_key: None,
            });
        }
        if let Some(c) = color {
            color_end(&mut gl, c);
        }
    }
    (out, translucent, fill)
}

/// The folder of the shape file named at the end of a draw label.
fn shape_dir(label: &str) -> &str {
    let path = label.rsplit(' ').next().unwrap_or("");
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

#[cfg(test)]
#[path = "shapes_tests.rs"]
mod tests;
