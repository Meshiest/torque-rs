//! The convexes (the `Convex` family), the `buildConvex` overrides, and a mover's working set.
//!
//! **Kinds** (`mType`, numbered as Torque's `ConvexType`): [`ConvexKind::Box`] (`BoxConvex`, [`BOX_CONVEX_TYPE`]:
//! PhysicalZone, Item), [`ConvexKind::OrthoBox`] (`OrthoBoxConvex`, also [`BOX_CONVEX_TYPE`]: a Player),
//! [`ConvexKind::Hull`] (one collision detail of an object; its [`HullClass`] gives the type, [`SHAPE_BASE_CONVEX_TYPE`] for
//! the `ShapeBaseConvex` of a StaticShape or a vehicle) and the map pieces ([`crate::map`]).
//!
//! **Classes a game adds.** An object class a game adds overrides `buildConvex` through a [`ConvexBuilder`] that the host
//! returns as [`ConvexSource::Custom`]; it builds with the [`WorkingSet`] calls the Torque classes use ([`WorkingSet::find`],
//! [`WorkingSet::add`], [`WorkingSet::kind_mut`], [`WorkingSet::build_hulls`]) and may make hulls of its own [`HullClass`].
//!
//! **What a convex keeps.** A convex is made once, when it enters a working set, and keeps the centre and size (or the slot and
//! its bounds) it was made with; the object's transform and scale are read per use. So a Player whose object box changes keeps
//! its old box in another mover's working set until that entry leaves it ([`Convex`] holds the made data, [`World::object`] answers
//! the rest at query time).
//!
//! **The working set** ([`WorkingSet`]) is a mover's `mWorking` list: `Convex::addToWorkingList`
//! puts a new entry at its head, and `Convex::updateWorkingList` drops the entries whose bounding box misses
//! the query box or whose object no longer collides, asks the container for objects in the box, and lets each object add its
//! convexes in the container's order (a host may reorder them first: [`World::order_candidates`]).

use std::collections::VecDeque;
use std::rc::Rc;

use crate::map::MapShape;
use crate::math::{
    Box3, F3, IDENTITY, Matrix, affine_inverse, d, matrix_product, mul_box, sqrt_f32,
};
use crate::mesh::Hull;
use crate::poly::PolyList;
use crate::{Exhausted, ObjectId, work};

/// Convexes one working set may hold (cap; a Player among many small objects holds tens).
pub const MAX_WORKING_SET: usize = 4_096;
/// Objects one working-set update may take from the container (cap).
pub const MAX_WORKING_CANDIDATES: usize = 4_096;
/// Collision details of one object a working-set update looks at (cap; TGE's list has no limit).
pub const MAX_HULL_SLOTS: usize = 64;

/// `TSConvexType`.
pub const TS_CONVEX_TYPE: u32 = 0;
/// `BoxConvexType`: both [`ConvexKind::Box`] and [`ConvexKind::OrthoBox`].
pub const BOX_CONVEX_TYPE: u32 = 1;
/// `ShapeBaseConvexType`: the hulls of [`HullClass::SHAPE_BASE`].
pub const SHAPE_BASE_CONVEX_TYPE: u32 = 4;
/// `TSStaticConvexType`.
pub const TS_STATIC_CONVEX_TYPE: u32 = 5;
/// `InteriorMapConvexType`.
pub const INTERIOR_MAP_CONVEX_TYPE: u32 = 6;

/// What a convex needs to know of its object at the time of a query (the `SceneObject` fields it reads).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectView {
    /// `mObjToWorld`.
    pub obj_to_world: Matrix,
    /// `mWorldToObj`.
    pub world_to_obj: Matrix,
    /// `mObjScale`.
    pub scale: F3,
    /// `mObjBox`.
    pub obj_box: Box3,
    /// `mWorldBox`.
    pub world_box: Box3,
    /// The world sphere's radius (`mWorldSphere.radius`).
    pub world_radius: f32,
    /// `mTypeMask`.
    pub type_mask: u32,
    /// The collision-disabled count is not zero.
    pub collision_disabled: bool,
}

impl ObjectView {
    /// The view of an axis-aligned box `min..max` in the world: no rotation, unit scale, the object box centered on the origin,
    /// a world radius of 1 and the collision enabled.
    #[must_use]
    pub fn from_box(min: F3, max: F3, type_mask: u32) -> ObjectView {
        let center: F3 = std::array::from_fn(|i| (min[i] + max[i]) * 0.5);
        let half: F3 = std::array::from_fn(|i| ((d(max[i]) - d(min[i])) * 0.5) as f32);
        let mut obj_to_world = IDENTITY;
        let mut world_to_obj = IDENTITY;
        for (row, c) in center.into_iter().enumerate() {
            obj_to_world[row * 4 + 3] = c;
            world_to_obj[row * 4 + 3] = -c;
        }
        ObjectView {
            obj_to_world,
            world_to_obj,
            scale: [1.0; 3],
            obj_box: Box3::new(half.map(|h| -h), half),
            world_box: Box3::new(min, max),
            world_radius: 1.0,
            type_mask,
            collision_disabled: false,
        }
    }

    /// The translation (`getPosition`: elements 3, 7, 11 of the transform).
    pub fn position(&self) -> F3 {
        [
            self.obj_to_world[3],
            self.obj_to_world[7],
            self.obj_to_world[11],
        ]
    }
}

/// An object's view at `m` with object box `obj_box` and `scale` (`resetWorldBox`'s world box and radius).
pub fn view_of(m: Matrix, obj_box: Box3, scale: F3, type_mask: u32) -> ObjectView {
    let scaled = Box3::new(
        [0, 1, 2].map(|i| (f64::from(obj_box.min[i]) * f64::from(scale[i])) as f32),
        [0, 1, 2].map(|i| (f64::from(obj_box.max[i]) * f64::from(scale[i])) as f32),
    );
    let mut w = mul_box(&m, &scaled);
    if !(w.max[0] >= w.min[0] && w.max[1] >= w.min[1] && w.max[2] >= w.min[2]) {
        w = Box3::new([-1.0; 3], [1.0; 3]);
    }
    let c = |i: usize| ((f64::from(w.max[i]) + f64::from(w.min[i])) * 0.5) as f32;
    let center = [c(0), c(1), c(2)];
    let dd = |i: usize| (f64::from(w.max[i]) - f64::from(center[i])) as f32;
    let (dx, dy, dz) = (dd(0), dd(1), dd(2));
    let sq = ((f64::from(dx) * f64::from(dx) + f64::from(dy) * f64::from(dy))
        + f64::from(dz) * f64::from(dz)) as f32;
    ObjectView {
        obj_to_world: m,
        world_to_obj: affine_inverse(&m),
        scale,
        obj_box,
        world_box: w,
        world_radius: sqrt_f32(sq),
        type_mask,
        collision_disabled: false,
    }
}

/// One collision slot of a shape datablock: its index, the detail's bounds (unscaled), and whether the slot names a
/// detail (`-1` does not).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HullSlot {
    pub slot: u32,
    pub present: bool,
    pub bounds: Box3,
}

impl HullSlot {
    /// The first `count` slots of a shape whose collision entries name the details `details` (`-1` for none) with the boxes
    /// `bounds` (`[min.x, min.y, min.z, max.x, max.y, max.z]`): a slot past either list is absent (a shape lists only the
    /// collision details it has, as TGE's lists grow by one per detail).
    pub fn of_shape(count: usize, details: &[i32], bounds: &[[f32; 6]]) -> Rc<[HullSlot]> {
        (0..count)
            .map(|i| {
                let b = bounds.get(i).copied().unwrap_or_default();
                HullSlot {
                    slot: i as u32,
                    present: details.get(i).is_some_and(|&d| d >= 0),
                    bounds: Box3 {
                        min: [b[0], b[1], b[2]],
                        max: [b[3], b[4], b[5]],
                    },
                }
            })
            .collect()
    }
}

/// A class's `buildConvex` override, for an object class a game adds (Torque's virtual on `SceneObject`). The host resolves
/// the object's class in [`World::convex_source`] and returns the builder with the per-object data it reads.
///
/// The builder runs where the Torque classes' builders run, once per candidate of [`WorkingSet::update`] (or a direct
/// [`WorkingSet::build_convex`]); it charges its own work ([`World::charge`]) and adds through the [`WorkingSet`] calls.
pub trait ConvexBuilder: std::fmt::Debug {
    /// Adds the object's convexes that meet `query` (in the world's space) to `set`; `view` is the object's view now.
    fn build_convex(
        &self,
        set: &mut WorkingSet,
        world: &mut dyn World,
        id: ObjectId,
        view: &ObjectView,
        query: &Box3,
    ) -> Result<(), Exhausted>;
}

/// Which `buildConvex` an object has, with the data it reads.
#[derive(Debug, Clone)]
pub enum ConvexSource {
    /// No override that adds anything (the `SceneObject` default).
    None,
    /// `Player::buildConvex`: `has_shape` is the Player's shape flag.
    Player { has_shape: bool },
    /// `ShapeBase::buildConvex`: `has_shape` is the object's shape flag; the datablock's slots.
    Shape {
        has_shape: bool,
        slots: Rc<[HullSlot]>,
    },
    /// `PhysicalZone::buildConvex`.
    Zone,
    /// `Item::buildConvex`: `has_shape` is the item's shape flag.
    Item { has_shape: bool },
    /// A Torque map's terrain or interior: its pieces come from [`World::map_shape`] ([`crate::map`]).
    Map,
    /// A class a game adds: its own `buildConvex`.
    Custom(Rc<dyn ConvexBuilder>),
}

impl PartialEq for ConvexSource {
    /// As a derive would compare, with two [`ConvexSource::Custom`] equal when they share the builder.
    fn eq(&self, other: &ConvexSource) -> bool {
        use ConvexSource as S;
        match (self, other) {
            (S::None, S::None) | (S::Zone, S::Zone) | (S::Map, S::Map) => true,
            (S::Player { has_shape: a }, S::Player { has_shape: b }) => a == b,
            (S::Item { has_shape: a }, S::Item { has_shape: b }) => a == b,
            (
                S::Shape {
                    has_shape: a,
                    slots: sa,
                },
                S::Shape {
                    has_shape: b,
                    slots: sb,
                },
            ) => a == b && sa == sb,
            (S::Custom(a), S::Custom(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// What a collision-detail hull convex is ([`ConvexKind::Hull`]): its `mType`, and whether a slot whose mesh is gone feeds its
/// bounds to a polygon list as a box (otherwise it feeds nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HullClass {
    pub type_id: u32,
    pub missing_mesh_as_box: bool,
}

impl HullClass {
    /// `ShapeBaseConvex`.
    pub const SHAPE_BASE: HullClass = HullClass {
        type_id: SHAPE_BASE_CONVEX_TYPE,
        missing_mesh_as_box: false,
    };
}

/// The world a collision query runs in, implemented by the host from the container (the octree, then the bins).
pub trait World {
    /// The object's view, or `None` once it is gone.
    fn object(&self, id: ObjectId) -> Option<ObjectView>;
    /// `Container::findObjects` over `query` and `mask`, in the host's order (Torque's container walks its bins newest
    /// first), skipping objects whose collision is disabled and `exclude` (the mover, disabled around the call). It
    /// pushes at most `limit` ids and stops there (the caller notes the overflow when `out` reaches `limit`): a query over a huge
    /// box must not grow `out` without bound before the caller's cap applies.
    fn find_objects(
        &mut self,
        query: &Box3,
        mask: u32,
        exclude: Option<ObjectId>,
        limit: usize,
        out: &mut Vec<ObjectId>,
    ) -> Result<(), Exhausted>;
    /// Which `buildConvex` the object has.
    fn convex_source(&self, id: ObjectId) -> ConvexSource;
    /// The mesh of collision slot `slot` of the object at its current pose (a ShapeBase's animated).
    fn hull(&self, id: ObjectId, slot: u32) -> Option<Rc<Hull>>;
    /// Pays `units` of work before it is done; false when the budget is spent.
    fn charge(&mut self, units: u64) -> bool;
    /// The order `Convex::updateWorkingList` builds the candidates in: `ids` are the container's finds (each object still
    /// there), in the container's order, which Torque keeps. A host may reorder them; the default leaves them as they are.
    fn order_candidates(&mut self, ids: &mut [ObjectId]) {
        let _ = ids;
    }
    /// A Torque map object's collision; worlds without map objects keep the default (none).
    fn map_shape(&self, id: ObjectId) -> Option<Rc<MapShape>> {
        let _ = id;
        None
    }
}

/// The kinds of convex.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConvexKind {
    /// `BoxConvex`: a box `center +- size` in the object's space, moved by its full transform.
    Box { center: F3, size: F3 },
    /// `OrthoBoxConvex`: the same box moved by the object's translation only.
    OrthoBox { center: F3, size: F3 },
    /// A hull: collision slot `slot` of the object, `bounds` its detail's bounds; `class` says what made it
    /// ([`HullClass::SHAPE_BASE`] for `ShapeBaseConvex`).
    Hull {
        class: HullClass,
        slot: u32,
        bounds: Box3,
    },
    /// A map terrain's triangle `half` of square (`x`, `y`) ([`crate::map`]): in the world's space.
    TerrainTri { x: i32, y: i32, half: u8 },
    /// A map interior's convex hull `hull` ([`crate::map`]): in the world's space.
    InteriorHull { hull: u32 },
}

/// A convex of an object in a working set. `id` names this convex (its identity, like a pointer): two ortho boxes of one
/// object are different convexes, and a collision state follows the convex, not the object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Convex {
    pub id: u64,
    pub object: ObjectId,
    pub kind: ConvexKind,
}

impl Convex {
    /// `mType`: [`BOX_CONVEX_TYPE`] for both boxes, the class's type for a hull.
    pub fn type_id(&self) -> u32 {
        match self.kind {
            ConvexKind::Box { .. } | ConvexKind::OrthoBox { .. } => BOX_CONVEX_TYPE,
            ConvexKind::Hull { class, .. } => class.type_id,
            ConvexKind::TerrainTri { .. } => crate::map::TERRAIN_CONVEX_TYPE,
            ConvexKind::InteriorHull { .. } => crate::map::INTERIOR_CONVEX_TYPE,
        }
    }

    /// Whether this is a map piece.
    pub fn is_map(&self) -> bool {
        matches!(
            self.kind,
            ConvexKind::TerrainTri { .. } | ConvexKind::InteriorHull { .. }
        )
    }

    /// A map piece's shape.
    fn map(&self, world: &dyn World) -> Option<Rc<MapShape>> {
        if self.is_map() {
            world.map_shape(self.object)
        } else {
            None
        }
    }

    fn slot(&self) -> Option<u32> {
        match self.kind {
            ConvexKind::Hull { slot, .. } => Some(slot),
            _ => None,
        }
    }

    /// `getTransform`: the object's transform for a box; its translation alone for an ortho box;
    /// for a hull the object's transform times the slot's node transform when it has one ([`matrix_product`]).
    pub fn transform(&self, world: &dyn World) -> Option<Matrix> {
        let view = world.object(self.object)?;
        Some(match self.kind {
            ConvexKind::Box { .. } => view.obj_to_world,
            ConvexKind::OrthoBox { .. } => {
                let mut m = IDENTITY;
                m[3] = view.obj_to_world[3];
                m[7] = view.obj_to_world[7];
                m[11] = view.obj_to_world[11];
                m
            }
            ConvexKind::Hull { slot, .. } => {
                match world.hull(self.object, slot).and_then(|h| h.node) {
                    Some(node) => matrix_product(&view.obj_to_world, &node),
                    None => view.obj_to_world,
                }
            }
            ConvexKind::TerrainTri { .. } | ConvexKind::InteriorHull { .. } => IDENTITY,
        })
    }

    /// `getScale`: the object's scale (a map piece's is a unit scale: it is in the world's space).
    pub fn scale(&self, world: &dyn World) -> Option<F3> {
        let view = world.object(self.object)?;
        Some(if self.is_map() { [1.0; 3] } else { view.scale })
    }

    /// `getBoundingBox()`: the object's world box for both boxes; for a hull its bounds at the object's
    /// transform and scale.
    pub fn bounding_box(&self, world: &dyn World) -> Option<Box3> {
        let view = world.object(self.object)?;
        Some(match self.kind {
            ConvexKind::Box { .. } | ConvexKind::OrthoBox { .. } => view.world_box,
            ConvexKind::Hull { bounds, .. } => {
                scaled_box_at(&bounds, &view.obj_to_world, view.scale)
            }
            ConvexKind::TerrainTri { .. } | ConvexKind::InteriorHull { .. } => {
                self.map(world)?.piece_box(&self.kind)?
            }
        })
    }

    /// `getBoundingBox(mat, scale)`: the object's box or the hull's bounds, each corner times the
    /// scale (floats), moved by `mat` ([`mul_box`]).
    pub fn bounding_box_at(&self, world: &dyn World, mat: &Matrix, scale: F3) -> Option<Box3> {
        let b = match self.kind {
            ConvexKind::Box { .. } | ConvexKind::OrthoBox { .. } => {
                world.object(self.object)?.obj_box
            }
            ConvexKind::Hull { bounds, .. } => bounds,
            // A map piece is in the world's space wherever it is asked about.
            ConvexKind::TerrainTri { .. } | ConvexKind::InteriorHull { .. } => {
                return self.map(world)?.piece_box(&self.kind);
            }
        };
        Some(scaled_box_at(&b, mat, scale))
    }

    /// `support` in the convex's own space: for a box `center + size` with each size component negated where
    /// `v` is negative (a NaN is not negative); for a hull the accelerator vertex with the largest dot.
    pub fn support(&self, world: &dyn World, v: F3) -> F3 {
        match self.kind {
            ConvexKind::Box { center, size } | ConvexKind::OrthoBox { center, size } => {
                let pick = |c: f32, s: f32, x: f32| c + if x < 0.0 { -s } else { s };
                [
                    pick(center[0], size[0], v[0]),
                    pick(center[1], size[1], v[1]),
                    pick(center[2], size[2], v[2]),
                ]
            }
            ConvexKind::Hull { slot, .. } => world
                .hull(self.object, slot)
                .and_then(|h| h.accel.as_ref().map(|a| a.support(v)))
                .unwrap_or([0.0; 3]),
            ConvexKind::TerrainTri { .. } | ConvexKind::InteriorHull { .. } => self
                .map(world)
                .map_or([0.0; 3], |m| m.support(&self.kind, v)),
        }
    }

    /// `getPolyList`: the list's transform and scale set from the convex and its object, then the box's eight corners
    /// and six faces ([`box_poly_list`]) or the slot's meshes ([`crate::mesh::feed_hull`]).
    pub fn poly_list(
        &self,
        world: &mut dyn World,
        list: &mut dyn PolyList,
    ) -> Result<(), Exhausted> {
        let Some(view) = world.object(self.object) else {
            return Ok(());
        };
        match self.kind {
            ConvexKind::Box { center, size } | ConvexKind::OrthoBox { center, size } => {
                let transform = self.transform(world).unwrap_or(IDENTITY);
                if !world.charge(work::POLY * 6 + work::VERTEX * 8) {
                    return Err(Exhausted);
                }
                list.set_transform(&transform, view.scale);
                list.set_object(Some(self.object));
                box_poly_list(list, center, size);
                Ok(())
            }
            ConvexKind::Hull {
                class,
                slot,
                bounds,
            } => {
                list.set_transform(&view.obj_to_world, view.scale);
                list.set_object(Some(self.object));
                match world.hull(self.object, slot) {
                    Some(h) => crate::mesh::feed_hull(world, list, &h),
                    None if class.missing_mesh_as_box => {
                        // With no shape instance: the bounds as a box.
                        if !world.charge(work::POLY * 6 + work::VERTEX * 8) {
                            return Err(Exhausted);
                        }
                        list.add_box(&bounds);
                        Ok(())
                    }
                    None => Ok(()),
                }
            }
            ConvexKind::TerrainTri { .. } | ConvexKind::InteriorHull { .. } => {
                let Some(m) = self.map(world) else {
                    return Ok(());
                };
                m.poly_list(&self.kind, self.object, list, &mut |u| world.charge(u))
            }
        }
    }
}

/// A box's corners times `scale`, moved by `m`: how a hull's bounds are placed in the world.
pub fn scaled_box_at(b: &Box3, m: &Matrix, scale: F3) -> Box3 {
    let s = |x: f32, k: f32| x * k;
    let scaled = Box3::new(
        [
            s(b.min[0], scale[0]),
            s(b.min[1], scale[1]),
            s(b.min[2], scale[2]),
        ],
        [
            s(b.max[0], scale[0]),
            s(b.max[1], scale[1]),
            s(b.max[2], scale[2]),
        ],
    );
    mul_box(m, &scaled)
}

/// `BoxConvex::getPolyList`'s body: the corners `center -+ size` in the order of x fastest, then y, then z (each sum a
/// float), and the six faces of `sFace`, each `begin(0, i)`, four vertices, the plane of the first three, `end`.
pub fn box_poly_list(list: &mut dyn PolyList, c: F3, s: F3) {
    let f = |a: f32, b: f32, plus: bool| {
        if plus { a + b } else { a - b }
    };
    let mut base = 0u32;
    for k in 0..8u32 {
        let p = [
            f(c[0], s[0], k & 1 != 0),
            f(c[1], s[1], k & 2 != 0),
            f(c[2], s[2], k & 4 != 0),
        ];
        let i = list.add_point(p);
        if k == 0 {
            base = i;
        }
    }
    for (i, face) in BOX_CONVEX_FACES.iter().enumerate() {
        list.begin(0, i as u32);
        let v = face.map(|x| base.wrapping_add(x));
        for &x in &v {
            list.vertex(x);
        }
        list.plane_points(v[0], v[1], v[2]);
        list.end();
    }
}

/// `sFace`: the corners of each face of a box convex, corner `k` being `center + (+-x, +-y, +-z)` with bit 0 for x.
pub const BOX_CONVEX_FACES: [[u32; 4]; 6] = [
    [0, 4, 5, 1],
    [0, 2, 6, 4],
    [3, 7, 6, 2],
    [3, 1, 5, 7],
    [0, 1, 3, 2],
    [4, 6, 7, 5],
];

/// `sCorner`: for each corner, its three neighbours and the three faces between them.
pub const BOX_CONVEX_CORNERS: [[u32; 6]; 8] = [
    [1, 2, 4, 4, 0, 1],
    [0, 3, 5, 4, 0, 3],
    [0, 3, 6, 4, 1, 2],
    [1, 2, 7, 4, 3, 2],
    [0, 5, 6, 0, 1, 5],
    [1, 4, 7, 0, 3, 5],
    [2, 4, 7, 1, 2, 5],
    [3, 5, 6, 3, 2, 5],
];

/// `sFace`'s axis and flip: the axis of each face's normal and whether it points down the axis.
pub const BOX_CONVEX_FACE_AXES: [(usize, bool); 6] = [
    (1, true),
    (0, true),
    (1, false),
    (0, false),
    (2, true),
    (2, false),
];

/// The query box in an object's space as `buildConvex` makes it: moved by `mWorldToObj` ([`mul_box`]) and each corner divided by
/// the object's scale.
pub fn query_in_object(view: &ObjectView, query: &Box3) -> Box3 {
    let b = mul_box(&view.world_to_obj, query);
    let q = |x: f32, s: f32| x / s;
    let s = view.scale;
    Box3::new(
        [q(b.min[0], s[0]), q(b.min[1], s[1]), q(b.min[2], s[2])],
        [q(b.max[0], s[0]), q(b.max[1], s[1]), q(b.max[2], s[2])],
    )
}

/// A mover's working set (`mWorking`): the convexes of the objects near it, the newest first.
#[derive(Debug, Clone, Default)]
pub struct WorkingSet {
    entries: VecDeque<Convex>,
    next_id: u64,
    found: Vec<ObjectId>,
    order: Vec<ObjectId>,
    /// A cap stopped the last update (the set holds what was built before it).
    pub overflowed: bool,
}

impl WorkingSet {
    pub fn new() -> WorkingSet {
        WorkingSet::default()
    }

    /// The entries in list order (head first).
    pub fn iter(&self) -> impl Iterator<Item = &Convex> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `Convex::clearWorkingList`: everything dropped (the mover is deleted or warped).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Drops the entries of an object (its `onRemove` nukes its convexes from every list).
    pub fn remove_object(&mut self, id: ObjectId) {
        self.entries.retain(|c| c.object != id);
    }

    /// `addToWorkingList`: at the head, with a fresh id. False (and [`overflowed`](Self::overflowed) set) when the set holds
    /// [`MAX_WORKING_SET`] already.
    pub fn add(&mut self, object: ObjectId, kind: ConvexKind) -> bool {
        if self.entries.len() >= MAX_WORKING_SET {
            self.overflowed = true;
            return false;
        }
        self.next_id = self.next_id.wrapping_add(1);
        self.entries.push_front(Convex {
            id: self.next_id,
            object,
            kind,
        });
        true
    }

    /// The entries as a slice-like copy, head first (for the state list).
    pub fn to_vec(&self) -> Vec<Convex> {
        self.entries.iter().copied().collect()
    }

    /// The first entry (head first) with this type and object, as each `buildConvex` looks for its own before adding (the hulls
    /// match the slot too).
    pub fn find(&self, type_id: u32, object: ObjectId, slot: Option<u32>) -> Option<usize> {
        self.entries.iter().position(|c| {
            c.type_id() == type_id && c.object == object && (slot.is_none() || c.slot() == slot)
        })
    }

    /// Entry `index`'s kind (an index from [`find`](Self::find)), for a builder that moves a convex it made earlier.
    pub fn kind_mut(&mut self, index: usize) -> Option<&mut ConvexKind> {
        self.entries.get_mut(index).map(|c| &mut c.kind)
    }

    /// `Convex::updateWorkingList` for the mover `owner` with the query box `query` and mask `mask`.
    pub fn update(
        &mut self,
        world: &mut dyn World,
        owner: ObjectId,
        query: &Box3,
        mask: u32,
    ) -> Result<(), Exhausted> {
        self.overflowed = false;
        // Drop what no longer overlaps or no longer collides (a gone object too: its convexes were nuked with it).
        let mut spent = false;
        self.entries.retain(|c| {
            // Once the budget is spent the rest stay as they are and the update stops after this walk.
            if spent || !world.charge(work::CONVEX) {
                spent = true;
                return true;
            }
            match (world.object(c.object), c.bounding_box(world)) {
                (Some(view), Some(bb)) => {
                    let missed = query.max[0] < bb.min[0]
                        || query.max[1] < bb.min[1]
                        || query.max[2] < bb.min[2]
                        || bb.max[0] < query.min[0]
                        || bb.max[1] < query.min[1]
                        || bb.max[2] < query.min[2];
                    !missed && !view.collision_disabled
                }
                _ => false,
            }
        });
        if spent {
            return Err(Exhausted);
        }
        self.found.clear();
        world.find_objects(
            query,
            mask,
            Some(owner),
            MAX_WORKING_CANDIDATES + 1,
            &mut self.found,
        )?;
        if self.found.len() > MAX_WORKING_CANDIDATES {
            self.found.truncate(MAX_WORKING_CANDIDATES);
            self.overflowed = true;
        }
        self.order.clear();
        for &id in &self.found {
            if world.object(id).is_some() {
                self.order.push(id);
            }
        }
        let mut order = std::mem::take(&mut self.order);
        world.order_candidates(&mut order);
        let mut result = Ok(());
        for &id in &order {
            if !world.charge(work::CANDIDATE) {
                result = Err(Exhausted);
                break;
            }
            if let Err(e) = self.build_convex(world, id, query) {
                result = Err(e);
                break;
            }
        }
        self.order = order;
        result
    }

    /// The object's `buildConvex(box, convex)` for this working set.
    pub fn build_convex(
        &mut self,
        world: &mut dyn World,
        id: ObjectId,
        query: &Box3,
    ) -> Result<(), Exhausted> {
        let Some(view) = world.object(id) else {
            return Ok(());
        };
        match world.convex_source(id) {
            ConvexSource::None => Ok(()),
            ConvexSource::Player { has_shape } => {
                if !has_shape {
                    return Ok(());
                }
                self.build_player(world, id, &view, query)
            }
            ConvexSource::Shape { has_shape, slots } => {
                if !has_shape {
                    return Ok(());
                }
                self.build_hulls(world, id, &view, query, &slots, HullClass::SHAPE_BASE, true)
            }
            ConvexSource::Zone => self.build_zone(world, id, &view, query),
            ConvexSource::Item { has_shape } => {
                if !has_shape {
                    return Ok(());
                }
                self.build_item(world, id, &view, query)
            }
            ConvexSource::Map => self.build_map(world, id, query),
            ConvexSource::Custom(b) => b.build_convex(self, world, id, &view, query),
        }
    }

    /// A map object's pieces that meet the query ([`crate::map`]): each one not in the set yet is added, in the
    /// shape's order. Charged a convex a piece looked at.
    fn build_map(
        &mut self,
        world: &mut dyn World,
        id: ObjectId,
        query: &Box3,
    ) -> Result<(), Exhausted> {
        let Some(shape) = world.map_shape(id) else {
            return Ok(());
        };
        let mut kinds = Vec::new();
        let mut capped = false;
        shape.pieces(query, &mut |u| world.charge(u), &mut kinds, &mut capped)?;
        if capped {
            self.overflowed = true;
        }
        for kind in kinds {
            if self
                .entries
                .iter()
                .any(|c| c.object == id && c.kind == kind)
            {
                continue;
            }
            if !self.add(id, kind) {
                break;
            }
        }
        Ok(())
    }

    /// `Player::buildConvex`: when the query (in the object's space) meets the object box and the Player has no box here yet,
    /// an ortho box of its object box: `center = f32((min + max) * 0.5)`, `size = f32(f32(max - min) * 0.5)`.
    fn build_player(
        &mut self,
        world: &mut dyn World,
        id: ObjectId,
        view: &ObjectView,
        query: &Box3,
    ) -> Result<(), Exhausted> {
        let q = query_in_object(view, query);
        if !q.overlaps(&view.obj_box) || self.find(BOX_CONVEX_TYPE, id, None).is_some() {
            return Ok(());
        }
        if !world.charge(work::CONVEX) {
            return Err(Exhausted);
        }
        let b = view.obj_box;
        let c = |lo: f32, hi: f32| (lo + hi) * 0.5;
        let s = |lo: f32, hi: f32| (hi - lo) * 0.5;
        let center = [
            c(b.min[0], b.max[0]),
            c(b.min[1], b.max[1]),
            c(b.min[2], b.max[2]),
        ];
        let size = [
            s(b.min[0], b.max[0]),
            s(b.min[1], b.max[1]),
            s(b.min[2], b.max[2]),
        ];
        self.add(id, ConvexKind::OrthoBox { center, size });
        Ok(())
    }

    /// The hull loop of `ShapeBase::buildConvex`, for any hull class: with `test_obj_box`, nothing unless the query (in the
    /// object's space) meets the object box; then for each slot that names a detail, its bounds times the scale (floats) at the
    /// object's transform; when that meets the query and no hull of the class and slot is here yet, a hull is added.
    #[allow(clippy::too_many_arguments)]
    pub fn build_hulls(
        &mut self,
        world: &mut dyn World,
        id: ObjectId,
        view: &ObjectView,
        query: &Box3,
        slots: &[HullSlot],
        class: HullClass,
        test_obj_box: bool,
    ) -> Result<(), Exhausted> {
        if test_obj_box {
            let q = query_in_object(view, query);
            if !q.overlaps(&view.obj_box) {
                return Ok(());
            }
        }
        for s in slots.iter().take(MAX_HULL_SLOTS) {
            if !s.present {
                continue;
            }
            if !world.charge(work::CONVEX) {
                return Err(Exhausted);
            }
            let wb = scaled_box_at(&s.bounds, &view.obj_to_world, view.scale);
            if !query.overlaps(&wb) || self.find(class.type_id, id, Some(s.slot)).is_some() {
                continue;
            }
            let kind = ConvexKind::Hull {
                class,
                slot: s.slot,
                bounds: s.bounds,
            };
            if !self.add(id, kind) {
                break;
            }
        }
        Ok(())
    }

    /// `Item::buildConvex`: when the query meets the item's WORLD box (no move to its space) and it has no box here
    /// yet, a box convex of its object box, as the zone's.
    fn build_item(
        &mut self,
        world: &mut dyn World,
        id: ObjectId,
        view: &ObjectView,
        query: &Box3,
    ) -> Result<(), Exhausted> {
        if !query.overlaps(&view.world_box) || self.find(BOX_CONVEX_TYPE, id, None).is_some() {
            return Ok(());
        }
        if !world.charge(work::CONVEX) {
            return Err(Exhausted);
        }
        let b = view.obj_box;
        let c = |lo: f32, hi: f32| (lo + hi) * 0.5;
        let s = |lo: f32, hi: f32| (hi - lo) * 0.5;
        let center = [
            c(b.min[0], b.max[0]),
            c(b.min[1], b.max[1]),
            c(b.min[2], b.max[2]),
        ];
        let size = [
            s(b.min[0], b.max[0]),
            s(b.min[1], b.max[1]),
            s(b.min[2], b.max[2]),
        ];
        self.add(id, ConvexKind::Box { center, size });
        Ok(())
    }

    /// `PhysicalZone::buildConvex`: a box convex of the object box (centre and half-size, floats).
    fn build_zone(
        &mut self,
        world: &mut dyn World,
        id: ObjectId,
        view: &ObjectView,
        query: &Box3,
    ) -> Result<(), Exhausted> {
        let q = query_in_object(view, query);
        if !q.overlaps(&view.obj_box) || self.find(BOX_CONVEX_TYPE, id, None).is_some() {
            return Ok(());
        }
        if !world.charge(work::CONVEX) {
            return Err(Exhausted);
        }
        let b = view.obj_box;
        let c = |lo: f32, hi: f32| (lo + hi) * 0.5;
        let s = |lo: f32, hi: f32| (hi - lo) * 0.5;
        let center = [
            c(b.min[0], b.max[0]),
            c(b.min[1], b.max[1]),
            c(b.min[2], b.max[2]),
        ];
        let size = [
            s(b.min[0], b.max[0]),
            s(b.min[1], b.max[1]),
            s(b.min[2], b.max[2]),
        ];
        self.add(id, ConvexKind::Box { center, size });
        Ok(())
    }
}

#[cfg(test)]
#[path = "convex_tests.rs"]
mod tests;
