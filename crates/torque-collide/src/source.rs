//! Collision a plugin contributes: boxes and convex hulls at a pose, each with a type mask, that the server's container and
//! the predicting client's collision world both answer for.
//!
//! A plugin implements [`CollisionSource`] once per side (the server's reads the state the simulation owns, the client's the
//! state its ghosts carry) and registers each as a [`PluginCollision`] on the engine's list
//! (`engine.push(PluginCollision::server(..))`, `PluginCollision::client(..)`). A host turns the entries of its side into
//! [`CollisionSources`], and every collision phase takes a [`ShapeSet`] from them: the shapes as they are when the phase
//! starts. Both sides build the set, its object ids, views and convex builders with this module, so a shape a source gives
//! both sides is the same object to the server's movers and to the client's prediction.
//!
//! - **State.** A source answers for the state it holds when asked. To move a shape, change that state between ticks (the
//!   server's from a tick hook, the client's from its ghost updates); the next phase sees the new pose, and a working set that
//!   kept the shape's convex reads the pose again at each use. Within one phase (one tick's collision, one replayed move) the
//!   shapes do not change.
//! - **Type bits.** A source claims one bit of the free type bits ([`FREE_TYPE_BITS`]) by name: [`CollisionSources`] gives the
//!   bits out in the order of the sorted names of every entry of both sides, so the server and the client agree without
//!   talking. A shape's mask is the bits it asks for from [`SHAPE_TYPE_BITS`] (a static shape's, so the movers collide with it as
//!   they do with one) and its source's bit.
//! - **Ids.** A shape's [`ObjectId`] is [`PLUGIN_ID_BASE`] plus its source's index times [`MAX_SHAPES_PER_SOURCE`] plus its
//!   `key`, so a shape keeps its id from tick to tick whatever order the source lists its shapes in. No object of a host's own
//!   has an id there.
//!
//! **Caps** (each shape or source past one is dropped, never an error): [`MAX_SOURCES`] sources, [`MAX_SHAPES_PER_SOURCE`]
//! shapes of each (a key at or past it is dropped), [`MAX_HULL_VERTS`] vertices and [`MAX_HULL_TRIS`] triangles in a hull.
//!
//! **Work.** A set's work does not depend on the host: it is the shapes the sources list, which [`MAX_SET_SHAPES`] bounds, and
//! no query looks at more than the set holds. It is charged to no host budget (the server's run budget and the client's
//! per-tick budget would run out at different places, and the two sides must not disagree about what is there). A set whose
//! sources list more than [`MAX_SET_SHAPES`] shapes in all (after each source's own cap) holds **none**: every shape of every
//! source is dropped, on the server and on the client alike. The convexes a shape's builder adds are charged to the host as
//! any object's are.
//!
//! **Names.** A name is [`is_source_name`]: it becomes part of a script global, so it may not be a built-in type's. Names are
//! told apart without regard to case (script's globals are), and whether a name is refused depends on the list of sources
//! alone, never on one side's script state, so both sides refuse the same ones.
//!
//! **Hostile input.** A pose with a value that is not finite or past [`MAX_COORD`] (its translation) or [`MAX_POSE_SCALE`] (its
//! rotation and scale elements), a box with a half-extent that is not finite or past [`MAX_HALF_EXTENT`], and a mesh that is not
//! a convex solid ([`PluginHull::new`]) are refused.

use std::fmt;
use std::rc::Rc;

use crate::convex::{
    BOX_CONVEX_TYPE, ConvexBuilder, ConvexKind, ConvexSource, HullClass, HullSlot, ObjectView,
    WorkingSet, view_of,
};
use crate::math::{Box3, F3, Matrix};
use crate::mesh::{Hull, MAX_HULL_TRIS, MAX_HULL_VERTS, MeshPart};
use crate::types::{STATIC, STATIC_SHAPE, STATIC_TS, VEHICLE_BLOCKER};
use crate::{Exhausted, ObjectId, World, work};

/// The type bits no class of the library claims: the bits plugin sources are given, one per source.
pub const FREE_TYPE_BITS: [u32; 8] = [
    1 << 7,
    1 << 8,
    1 << 9,
    1 << 21,
    1 << 28,
    1 << 29,
    1 << 30,
    1 << 31,
];

/// The type bits a shape may ask for beside its source's own: those of a static object.
pub const SHAPE_TYPE_BITS: u32 = STATIC | STATIC_SHAPE | STATIC_TS | VEHICLE_BLOCKER;

/// Sources a host takes (cap: one per free type bit).
pub const MAX_SOURCES: usize = FREE_TYPE_BITS.len();
/// Shapes one source has in a set (cap), and the keys its shapes may use (`0..MAX_SHAPES_PER_SOURCE`).
pub const MAX_SHAPES_PER_SOURCE: usize = 1024;
/// The first id of a plugin shape. Past it, up to [`MAX_SOURCES`] times [`MAX_SHAPES_PER_SOURCE`] ids.
pub const PLUGIN_ID_BASE: u32 = 0xE000_0000;
/// The most shapes a set holds (cap): sources whose lists add up to more leave the set empty.
pub const MAX_SET_SHAPES: usize = 4096;
/// The largest element of a pose's rotation and scale part (a pose past it is refused).
pub const MAX_POSE_SCALE: f32 = 1.0e3;
/// The largest coordinate of a pose's translation (a pose past it is refused).
pub const MAX_COORD: f32 = 1.0e7;
/// The smallest and the largest half-extent of a box (a box outside them is refused, or raised to the smallest).
pub const MIN_HALF_EXTENT: f32 = 1.0e-3;
pub const MAX_HALF_EXTENT: f32 = 1.0e5;
/// The longest a source's name may be.
pub const MAX_NAME: usize = 32;

/// Which side an entry is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    /// The server's container.
    Server,
    /// The predicting client's collision world.
    Client,
}

/// A plugin's collision, as one side answers for it ([`CollisionSource::shapes`]).
pub trait CollisionSource: fmt::Debug {
    /// Appends the shapes the source has now to `out` (a set takes at most the first [`MAX_SHAPES_PER_SOURCE`] of them).
    fn shapes(&self, out: &mut Vec<CollisionShape>);
}

/// What a source puts on the engine's list: its name (which claims its type bit), the side it answers for and the source.
#[derive(Debug, Clone)]
pub struct PluginCollision {
    pub name: String,
    pub side: Side,
    pub source: Rc<dyn CollisionSource>,
}

impl PluginCollision {
    /// A source for the server's container, claiming the type bit `name` is given (ASCII letters, digits and `_`, at most
    /// [`MAX_NAME`] of them).
    pub fn server(name: impl Into<String>, source: Rc<dyn CollisionSource>) -> PluginCollision {
        PluginCollision {
            name: name.into(),
            side: Side::Server,
            source,
        }
    }

    /// A source for the predicting client's collision world: give it the name of the server's, for the bits to agree.
    pub fn client(name: impl Into<String>, source: Rc<dyn CollisionSource>) -> PluginCollision {
        PluginCollision {
            name: name.into(),
            side: Side::Client,
            source,
        }
    }
}

/// One shape of a source.
#[derive(Debug, Clone)]
pub struct CollisionShape {
    /// Its identity within the source (below [`MAX_SHAPES_PER_SOURCE`]): the same key is the same object from tick to tick. Of
    /// two shapes with one key, the first listed is taken.
    pub key: u32,
    /// Where it is: a row-major 4x4 with the translation in elements 3, 7 and 11, as `collide`'s matrices.
    pub pose: Matrix,
    pub kind: ShapeKind,
    /// The type bits it asks for ([`SHAPE_TYPE_BITS`] is all it gets), beside its source's own.
    pub type_mask: u32,
}

/// What a shape is.
#[derive(Debug, Clone)]
pub enum ShapeKind {
    /// A box `-half..half` around the pose's origin, turned by the pose.
    Box { half: F3 },
    /// A convex mesh at the pose (made once with [`PluginHull::new`]).
    Hull(Rc<PluginHull>),
}

/// A convex mesh a source makes once and gives at many poses: its hull and the bounds of its vertices.
#[derive(Debug, Clone)]
pub struct PluginHull {
    hull: Rc<Hull>,
    bounds: Box3,
}

impl PluginHull {
    /// A hull of the convex mesh `verts` and `tris`, its triangles wound as Torque's collision meshes are (clockwise seen from
    /// outside: the cross product of a triangle's first two edges points into the mesh). `None` for a mesh with no triangle,
    /// past [`MAX_HULL_VERTS`] or [`MAX_HULL_TRIS`], a vertex that is not finite or past [`MAX_COORD`], an index out of range, or
    /// one that is not a convex solid: a degenerate triangle (a repeated index or no area), a vertex no triangle uses, a vertex
    /// outside a triangle's plane (not convex, or wound the other way round), or all of it flat.
    pub fn new(verts: &[F3], tris: &[[u32; 3]]) -> Option<PluginHull> {
        if tris.is_empty() || verts.len() > MAX_HULL_VERTS || tris.len() > MAX_HULL_TRIS {
            return None;
        }
        if verts.iter().flatten().any(|v| !(v.abs() <= MAX_COORD)) {
            return None;
        }
        if tris.iter().flatten().any(|&i| i as usize >= verts.len()) {
            return None;
        }
        if !is_convex_solid(verts, tris) {
            return None;
        }
        let mut bounds = Box3::new([f32::MAX; 3], [f32::MIN; 3]);
        for v in verts {
            bounds.min = std::array::from_fn(|k| bounds.min[k].min(v[k]));
            bounds.max = std::array::from_fn(|k| bounds.max[k].max(v[k]));
        }
        let part = MeshPart {
            node: crate::math::IDENTITY,
            verts: verts.to_vec(),
            tris: tris.to_vec(),
            materials: vec![0; tris.len()],
        };
        Some(PluginHull {
            hull: Rc::new(Hull::new(None, vec![part])),
            bounds,
        })
    }

    /// A hull of the box `-half..half` (twelve triangles).
    pub fn cuboid(half: F3) -> Option<PluginHull> {
        let [x, y, z] = half;
        let verts = [
            [-x, -y, -z],
            [x, -y, -z],
            [x, y, -z],
            [-x, y, -z],
            [-x, -y, z],
            [x, -y, z],
            [x, y, z],
            [-x, y, z],
        ];
        let tris = [
            [0, 1, 2],
            [0, 2, 3],
            [4, 6, 5],
            [4, 7, 6],
            [0, 5, 1],
            [0, 4, 5],
            [1, 6, 2],
            [1, 5, 6],
            [2, 7, 3],
            [2, 6, 7],
            [3, 4, 0],
            [3, 7, 4],
        ];
        PluginHull::new(&verts, &tris)
    }

    /// The bounds of the vertices.
    pub fn bounds(&self) -> Box3 {
        self.bounds
    }
}

/// Whether the triangles make a convex solid wound clockwise seen from outside: each has an area, every vertex is used and lies on
/// the inner side of (or on, within a tolerance of the mesh's size) every triangle's plane, and each plane has a vertex clearly
/// inside, so the solid is not flat.
fn is_convex_solid(verts: &[F3], tris: &[[u32; 3]]) -> bool {
    let p = |i: u32| verts[i as usize].map(f64::from);
    let mut used = vec![false; verts.len()];
    let mut extent = 0.0f64;
    for v in verts {
        for c in v {
            extent = extent.max(f64::from(c.abs()));
        }
    }
    let tolerance = 1.0e-5 * extent.max(1.0);
    for t in tris {
        if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
            return false;
        }
        for &i in t {
            used[i as usize] = true;
        }
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        // Twice the area, against the mesh's own size: a sliver is no face.
        if !(len > 1.0e-6 * extent.max(1.0).powi(2)) {
            return false;
        }
        let mut deepest = 0.0f64;
        for v in verts {
            // The distance inside the plane (the normal points inward).
            let d = ((f64::from(v[0]) - a[0]) * n[0]
                + (f64::from(v[1]) - a[1]) * n[1]
                + (f64::from(v[2]) - a[2]) * n[2])
                / len;
            if d < -tolerance {
                return false;
            }
            deepest = deepest.max(d);
        }
        if deepest <= tolerance {
            return false;
        }
    }
    used.iter().all(|&u| u)
}

/// The sources of one side with the bit and the index each was given.
#[derive(Debug, Clone, Default)]
pub struct CollisionSources {
    bound: Vec<Bound>,
}

#[derive(Debug, Clone)]
struct Bound {
    name: String,
    bit: u32,
    index: u32,
    source: Rc<dyn CollisionSource>,
}

impl CollisionSources {
    /// The sources of `side` among `entries` (the engine's list), and the warnings for what was refused: a name that is not
    /// valid, a second source of one name on the side, and a name past the [`MAX_SOURCES`] first (sorted) names.
    ///
    /// The bits and indexes come from the sorted unique names of the entries of both sides, so a name has the same bit on
    /// the server and the client whichever of them registered it.
    pub fn from_entries(
        entries: &[PluginCollision],
        side: Side,
    ) -> (CollisionSources, Vec<String>) {
        let mut warnings = Vec::new();
        for e in entries.iter().filter(|e| e.side == side) {
            if !is_source_name(&e.name) {
                warnings.push(format!(
                    "collision source `{}` refused: a name is 1 to {MAX_NAME} ASCII letters, digits or `_`, and not a built-in type's",
                    e.name
                ));
            }
        }
        let names = source_names(entries.iter().map(|e| e.name.as_str()));
        let mut bound: Vec<Bound> = Vec::new();
        for e in entries.iter().filter(|e| e.side == side) {
            let Some(index) = names.iter().position(|n| n.eq_ignore_ascii_case(&e.name)) else {
                continue;
            };
            if index >= MAX_SOURCES {
                warnings.push(format!(
                    "collision source `{}` refused: at most {MAX_SOURCES} sources",
                    e.name
                ));
                continue;
            }
            if bound.iter().any(|b| b.index as usize == index) {
                warnings.push(format!(
                    "collision source `{}` refused: the {side:?} has one of that name (names do not differ by case)",
                    e.name
                ));
                continue;
            }
            bound.push(Bound {
                name: names[index].to_owned(),
                bit: FREE_TYPE_BITS[index],
                index: index as u32,
                source: Rc::clone(&e.source),
            });
        }
        bound.sort_by_key(|b| b.index);
        (CollisionSources { bound }, warnings)
    }

    /// Whether there is no source.
    pub fn is_empty(&self) -> bool {
        self.bound.is_empty()
    }

    /// Each source's name and type bit, in index order.
    pub fn bits(&self) -> impl Iterator<Item = (&str, u32)> {
        self.bound.iter().map(|b| (b.name.as_str(), b.bit))
    }

    /// The type bit of the source called `name`.
    pub fn bit(&self, name: &str) -> Option<u32> {
        self.bound
            .iter()
            .find(|b| b.name.eq_ignore_ascii_case(name))
            .map(|b| b.bit)
    }

    /// The shapes the sources have now, in id order: each source's first [`MAX_SHAPES_PER_SOURCE`] that passed the checks; none
    /// at all when the sources list more than [`MAX_SET_SHAPES`] (see the module's note on work).
    pub fn snapshot(&self) -> ShapeSet {
        let mut entries: Vec<Entry> = Vec::new();
        let mut listed = Vec::new();
        let mut total = 0usize;
        for b in &self.bound {
            listed.clear();
            b.source.shapes(&mut listed);
            listed.truncate(MAX_SHAPES_PER_SOURCE);
            total += listed.len();
            if total > MAX_SET_SHAPES {
                return ShapeSet::default();
            }
            let mut mine: Vec<Entry> = listed.drain(..).filter_map(|s| Entry::new(b, s)).collect();
            // The first listed of two shapes with one key is the one kept.
            mine.sort_by_key(|e| e.id);
            mine.dedup_by_key(|e| e.id);
            entries.append(&mut mine);
        }
        ShapeSet { entries }
    }
}

/// The valid names among `names`, each once without regard to case (the first by order of spelling kept), in the order the type
/// bits and ids are given. What a manifest lists and [`CollisionSources`] numbers.
pub fn source_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let mut v: Vec<&str> = names.into_iter().filter(|n| is_source_name(n)).collect();
    v.sort_unstable_by(|a, b| {
        a.to_ascii_lowercase()
            .cmp(&b.to_ascii_lowercase())
            .then(a.cmp(b))
    });
    v.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    v
}

/// The names of the library's own type bits (`StaticShape`, `Player`, ...): no source may take one, since script sees a source's
/// bit as `$TypeMasks::<Name>ObjectType`, which would then name a built-in mask.
pub const RESERVED_NAMES: [&str; 24] = [
    "Static",
    "Environment",
    "Terrain",
    "Interior",
    "Water",
    "Trigger",
    "Marker",
    "GameBase",
    "ShapeBase",
    "Camera",
    "StaticShape",
    "Player",
    "Item",
    "Vehicle",
    "VehicleBlocker",
    "Projectile",
    "Explosion",
    "Corpse",
    "Debris",
    "PhysicalZone",
    "StaticTS",
    "AI",
    "StaticRendered",
    "DamagableItem",
];

/// Whether `name` may name a source: 1 to [`MAX_NAME`] ASCII letters, digits and `_`, and not a [reserved](RESERVED_NAMES) name
/// in any case (script's names do not tell cases apart).
pub fn is_source_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        && !RESERVED_NAMES.iter().any(|r| r.eq_ignore_ascii_case(name))
}

/// A shape in a set.
#[derive(Debug, Clone)]
struct Entry {
    id: ObjectId,
    view: ObjectView,
    builder: Rc<ShapeBuilder>,
    hull: Option<Rc<Hull>>,
}

impl Entry {
    fn new(b: &Bound, s: CollisionShape) -> Option<Entry> {
        if s.key as usize >= MAX_SHAPES_PER_SOURCE {
            return None;
        }
        let translation = [3, 7, 11];
        let linear = [0, 1, 2, 4, 5, 6, 8, 9, 10];
        if s.pose.iter().any(|v| !v.is_finite())
            || translation.iter().any(|&i| s.pose[i].abs() > MAX_COORD)
            || linear.iter().any(|&i| s.pose[i].abs() > MAX_POSE_SCALE)
        {
            return None;
        }
        let mask = (s.type_mask & SHAPE_TYPE_BITS) | b.bit;
        let (obj_box, hull) = match &s.kind {
            ShapeKind::Box { half } => {
                if half
                    .iter()
                    .any(|h| !h.is_finite() || h.abs() > MAX_HALF_EXTENT)
                {
                    return None;
                }
                let h = half.map(|h| h.abs().max(MIN_HALF_EXTENT));
                (Box3::new(h.map(|h| -h), h), None)
            }
            ShapeKind::Hull(h) => (h.bounds, Some(Rc::clone(&h.hull))),
        };
        let view = view_of(s.pose, obj_box, [1.0; 3], mask);
        Some(Entry {
            id: ObjectId(PLUGIN_ID_BASE + b.index * MAX_SHAPES_PER_SOURCE as u32 + s.key),
            view,
            builder: Rc::new(ShapeBuilder {
                half: hull.is_none().then_some(obj_box.max),
                bounds: obj_box,
            }),
            hull,
        })
    }
}

/// Whether `id` is in the range plugin shapes have.
pub fn is_plugin_id(id: ObjectId) -> bool {
    (PLUGIN_ID_BASE..PLUGIN_ID_BASE + (MAX_SOURCES * MAX_SHAPES_PER_SOURCE) as u32).contains(&id.0)
}

/// The shapes of a host's plugin sources for one collision phase: what a [`World`] answers for the ids in the plugin range.
#[derive(Debug, Clone, Default)]
pub struct ShapeSet {
    entries: Vec<Entry>,
}

impl ShapeSet {
    /// Shapes in the set.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn entry(&self, id: ObjectId) -> Option<&Entry> {
        let at = self.entries.binary_search_by_key(&id, |e| e.id).ok()?;
        self.entries.get(at)
    }

    /// [`World::object`] for a shape.
    pub fn object(&self, id: ObjectId) -> Option<ObjectView> {
        self.entry(id).map(|e| e.view)
    }

    /// [`World::find_objects`] over the shapes: those whose boxes meet `query` and whose masks meet `mask`, in id order. At most
    /// `limit` ids go into `out` in all. Charged to no budget: the set is bounded ([`MAX_SET_SHAPES`]).
    pub fn find(
        &self,
        query: &Box3,
        mask: u32,
        exclude: Option<ObjectId>,
        limit: usize,
        out: &mut Vec<ObjectId>,
    ) {
        for e in &self.entries {
            if out.len() >= limit {
                return;
            }
            if Some(e.id) == exclude
                || e.view.type_mask & mask == 0
                || e.view.collision_disabled
                || !e.view.world_box.overlaps(query)
            {
                continue;
            }
            out.push(e.id);
        }
    }

    /// [`World::convex_source`] for a shape.
    pub fn convex_source(&self, id: ObjectId) -> ConvexSource {
        match self.entry(id) {
            Some(e) => ConvexSource::Custom(Rc::clone(&e.builder) as Rc<dyn ConvexBuilder>),
            None => ConvexSource::None,
        }
    }

    /// [`World::hull`] for a shape: its one slot.
    pub fn hull(&self, id: ObjectId, slot: u32) -> Option<Rc<Hull>> {
        if slot != 0 {
            return None;
        }
        self.entry(id)?.hull.clone()
    }
}

/// A shape's `buildConvex`: a box convex of its half-extents, or one hull convex over its one slot.
#[derive(Debug)]
struct ShapeBuilder {
    /// The half-extents of a box shape.
    half: Option<F3>,
    bounds: Box3,
}

impl ConvexBuilder for ShapeBuilder {
    fn build_convex(
        &self,
        set: &mut WorkingSet,
        world: &mut dyn World,
        id: ObjectId,
        view: &ObjectView,
        query: &Box3,
    ) -> Result<(), Exhausted> {
        if let Some(size) = self.half {
            if !query.overlaps(&view.world_box) {
                return Ok(());
            }
            let kind = ConvexKind::Box {
                center: [0.0; 3],
                size,
            };
            // A box the source resized keeps its convex, the size moved on.
            if let Some(at) = set.find(BOX_CONVEX_TYPE, id, None) {
                if let Some(k) = set.kind_mut(at) {
                    *k = kind;
                }
                return Ok(());
            }
            if !world.charge(work::CONVEX) {
                return Err(Exhausted);
            }
            set.add(id, kind);
            return Ok(());
        }
        let slots = [HullSlot {
            slot: 0,
            present: true,
            bounds: self.bounds,
        }];
        set.build_hulls(world, id, view, query, &slots, HullClass::SHAPE_BASE, true)
    }
}

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;
