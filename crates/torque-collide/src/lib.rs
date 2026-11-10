//! The collision library: Torque's convexes, GJK `CollisionState`, `CollisionList` and the polygon lists, following the Torque
//! code, and the math module. Std and a fast hash ([`hash`]) only. The world (the object octree, the bins, the shape meshes) is reached through
//! [`World`], which the host implements.
//!
//! **Credits.** This crate follows the MIT-licensed Torque3D release of the Torque engine (GarageGames), whose code TGE 1.4
//! shares; where TGE 1.4 differs, it follows TGE and names the TGE routine. The files followed:
//! - `Engine/source/collision/convex.cpp`, `boxConvex.cpp`, `gjk.cpp`, `collision.h`, `abstractPolyList.cpp`,
//!   `clippedPolyList.cpp`, `earlyOutPolyList.cpp`, `extrudedPolyList.cpp`; `Engine/source/math/mMatrix.cpp`, `mQuat.cpp`,
//!   `mPlane.h`, `mPlaneTransformer.cpp`, `mMath_C.cpp`, `mMathFn.h`, `mPoint3.h`, `mBox.h`, `mPolyhedron.cpp`;
//!   `Engine/source/ts/tsCollision.cpp`, `tsShape.cpp`: `Copyright (c) 2012 GarageGames, LLC`.
//! - `Engine/source/T3D/objectTypes.h`, `player.cpp`, `projectile.cpp`, `physicalZone.cpp`: `Copyright (c) 2012
//!   GarageGames, LLC` and `Copyright (C) 2015 Faust Logic, Inc.`; `Engine/source/T3D/item.cpp`: `Copyright (c) 2012
//!   GarageGames, LLC`.
//!
//! **What is here:**
//! - [`math`]: `Point3F`/`MatrixF`/`PlaneF` arithmetic, the matrix product, the float-to-int conversion and the
//!   transcendentals.
//! - [`library`]: the math library table the arithmetic goes through, which a host may replace (Torque's `mMathFn.h`
//!   pointers).
//! - [`poly`]: the polygon lists `Player::updatePos`, `findContact` and `step` use, and
//!   `Polyhedron::buildBox`.
//! - [`collision`]: `Collision` and the 64-entry `CollisionList`.
//! - [`convex`]: the convex kinds (box, ortho box, hull), the `buildConvex` builders with the seam for a game's own classes
//!   ([`convex::ConvexBuilder`]), and the mover's working set (`Convex::updateWorkingList`).
//! - [`mesh`]: a collision mesh at a pose as plain arrays, its polygon feed (`TSShapeInstance::buildPolyList`) and its hull
//!   accelerator (`TSShape::computeAccelerator`).
//! - [`map`]: a Torque map's terrain and interiors, their convexes, polygon lists and rays.
//! - [`source`]: collision a plugin contributes (boxes and convex hulls at a pose, with a type bit of their own) that a host's
//!   container and a predicting client's world both answer for.
//! - [`types`]: the object type bits and the masks the classes query with ([`TypeMasks`]).
//! - [`gjk`] and [`feature`]: `GjkCollisionState` (distance, closest points, 16-iteration limit), `ConvexFeature`, the state
//!   list and `getCollisionInfo` (vehicles).
//! - [`hash`]: the hash of the engine's own ids ([`hash::IdMap`]).
//! - [`sat`]: how deep two convex polytopes overlap (the separating axis test), exact while they overlap; this crate's own.
//!
//! **Hostile input.** Positions, scales and boxes come from scripts; meshes from files. Nothing here panics on NaN, infinities,
//! huge or degenerate values: every index is checked, every loop is bounded by a count the crate caps (the `MAX_*` constants), and
//! a query's work is charged to the host's budget ([`World::charge`]) before doing it. Where Torque has a limit it is kept
//! (64 collisions, 16 GJK iterations, 30 faces); where it has none, a named cap stops the query and the list says so. Each
//! comparison is written so that a NaN takes a fixed, documented side (a negated `>=` is "below, or unordered").
//!
//! # Maintaining
//!
//! **Speed.** A mover's sub-step is the server's hottest loop (eight a vehicle tick, one a Player's). Rules a change must keep;
//! the benches that show them are torque-vehicle's `tick` ("buggy on tiles": a floor cut into 0.5-unit boxes, a working set
//! of a few hundred convexes; "buggy beside hulls": 21 hull convexes; and the allocations a tick it prints for each ground):
//! - [`feature::StateList::update`] and [`WorkingSet::update`] allocate nothing once warm: the state list keeps id maps
//!   ([`hash::IdMap`]) for its membership tests, cleared each call, and the working set drops entries in place.
//! - A hull's accelerator is shared ([`mesh::Hull::accel`] is an `Rc`), and a shape's collision slots are one `Rc<[HullSlot]>`
//!   ([`ConvexSource::Shape`]): a host hands out pointers, never copies, per query.
//! - [`source::CollisionSources::snapshot`] lists the plugin shapes again only when a source's
//!   [`source::CollisionSource::revision`] moved (or a source keeps none): a source that keeps one gives it a new
//!   [`source::next_revision`] at every change of what it lists, or phases meet stale shapes.
//! - A plugin shape's [`source::CollisionShape::passes`] lets one mover through it (a held prop's holder):
//!   [`source::ShapeSet`]'s queries and rays skip it only for that mover, every other mover and ray meets it
//!   (`a_shape_lets_only_the_mover_it_names_pass`).
//! - A cache or index changes nothing that is charged or returned: the same [`World::charge`] units at the same points, the same
//!   results in the same order (the trace-hash lines and torque-vehicle's printed rigid trace must not move).

// Comparisons are written so that a NaN takes a fixed side: a negated comparison is the intended form (`!(a >= b)` is "below,
// or unordered"), not a slip.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod collision;
pub mod convex;
pub mod feature;
pub mod gjk;
pub mod hash;
pub mod library;
pub mod map;
pub mod math;
pub mod mesh;
pub mod poly;
pub mod rotation_code;
pub mod sat;
pub mod source;
pub mod types;

pub use collision::{Collision, CollisionList, MAX_COLLISIONS};
pub use convex::{
    Convex, ConvexBuilder, ConvexKind, ConvexSource, HullClass, HullSlot, ObjectView, WorkingSet,
    World,
};
pub use math::{Box3, F3, Matrix, Plane};
pub use poly::{ClippedPolyList, EarlyOutPolyList, ExtrudedPolyList, PolyList, Polyhedron};
pub use types::TypeMasks;

/// An object as the host names it (a `SimObject` id or an index of its own): opaque to this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(pub u32);

/// Work units charged to the host's budget.
pub mod work {
    /// A candidate object the working-set update looks at.
    pub const CANDIDATE: u64 = 4;
    /// A convex built, or one entry of a working set walked.
    pub const CONVEX: u64 = 2;
    /// A point or an index fed to a polygon list.
    pub const VERTEX: u64 = 1;
    /// A polygon fed to a polygon list (its clipping included).
    pub const POLY: u64 = 8;
    /// One GJK iteration (two support calls and the sub-simplex search).
    pub const GJK_ITERATION: u64 = 16;
    /// A vertex, edge or face of a convex feature.
    pub const FEATURE: u64 = 1;
}

/// The query ran out of budget or hit a cap: it stopped where it was, deterministically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exhausted;
