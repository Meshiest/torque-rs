//! A deterministic [Rapier](https://rapier.rs) world for torque-rs: rigid bodies stepped at the engine's fixed tick against
//! the map's terrain and interiors, with every value checked and every loop capped, giving the same state bit for bit on
//! native and wasm32.
//!
//! This crate is not a plugin and nothing in the engine's default set depends on it. It is the shared core of two users: a
//! persistent world a server keeps across ticks (props, joints), and a scratch world a solver builds for one tick and drops (a
//! rigid-body solver behind the vehicle crate's `BodySolver` seam).
//!
//! **What is here:**
//! - [`World`]: the bodies by [`BodyId`], the joints between them by [`JointId`] ([`world::joint`]), the map pieces, the step ([`World::step`]: [`TICK`] seconds over a fixed number of
//!   sub-steps), the state read back ([`World::body`]) and written ([`World::set_body`], [`World::apply_impulse`]), and the
//!   words a trace hash folds in ([`World::state_words`]).
//! - [`Shape`]: what a body collides as: a box, a ball, or a convex hull of points (a shape's collision mesh,
//!   [`Shape::from_mesh_hull`]), the hull capped in vertices and replaced by its box when it is degenerate.
//! - [`PartDesc`]: a collider that sits on another body at a pose of its own ([`World::add_part`]): the pair is one rigid body with
//!   both masses, the part's pose in the body's frame changes only when the caller moves it ([`World::move_part`]), and a part
//!   leaves as a body's state with the velocity its body gave its point ([`World::detach_part`]). A part has an id of its own and
//!   is no body ([`World::part`], [`World::part_words`]); removing a body removes its parts, and removing a part wakes its body. Parts are
//!   indexed by body, with their reaches as a multiset, so adding, moving or removing one costs `log n` whatever their number.
//! - [`map`]: the map pieces as colliders, built by region on first use ([`map::CELL`] cells): terrain squares as triangles
//!   cut along the same diagonal as the engine's own terrain collision (not a height field, whose diagonal differs), and
//!   interior hulls as convex polyhedra.
//! - [`Limits`]: the caps (bodies, colliders, hull vertices, contacts a sub-step, cells a query) and the magnitudes values are
//!   held to.
//!
//! **Determinism.** Rapier is built with `enhanced-determinism` (scalar math, `libm` transcendentals, ordered maps; no SIMD
//! lanes wider than four and no threads). The step length and the sub-step count are fixed per world; bodies are kept in a
//! `BTreeMap` by id and every pass over them runs in id order; map cells are built in a fixed order (cell rows, then columns,
//! then the map pieces by id). Nothing reads a clock or a thread's timing. Two worlds given the same calls in the same order
//! hold the same bits, on every target.
//!
//! **Hostile input.** Values reach this crate from scripts, datablocks, ghost records and map files. Every float is checked
//! before Rapier sees it: a non-finite value is refused ([`Error::NonFinite`]), magnitudes are held to [`Limits`], a rotation
//! is normalized (a zero one refused). Native builds also catch a panic at every call into Rapier (the world is then
//! [`Error::Poisoned`] and refuses further work); a wasm32 build aborts on a panic, so the checks are what keep a page alive.
//! Work is charged to the caller's budget (the same `charge` a [`torque_collide::World`] pays into, [`work`] units) before it
//! is done.
//!
//! **Licenses.** Rapier, parry, nalgebra, simba and approx are under the Apache License 2.0. Their notice is [`NOTICE`] and
//! the license's text is [`APACHE_LICENSE`]; both are compiled into any binary that links this crate, so a game can show them
//! (its credits or an `--licenses` flag).

// Comparisons are written so that a NaN takes a fixed side: a negated comparison is the intended form (`!(a >= b)` is "below,
// or unordered"), not a slip.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod guard;
pub mod limits;
pub mod map;
pub mod shape;
pub mod world;

pub use limits::{Config, Limits};
pub use shape::Shape;
pub use torque_collide::Exhausted;
pub use world::joint::{Anchor, JointDesc, JointId, JointKind, JointState, Motor, Spring};
pub use world::part::{PartDesc, PartState};
pub use world::{BodyDesc, BodyId, BodyState, Error, Motion, StepReport, World};

/// The Rapier this crate is built on, for a caller that needs more than this crate's surface (joints, queries). Values given
/// to it directly skip this crate's checks.
pub use rapier3d;

/// One server tick, in seconds (32 ms): the step every [`World`] takes.
pub const TICK: f32 = 0.032;

/// The notice of the Apache-2.0 crates this crate builds on (the crate's `NOTICE` file).
pub const NOTICE: &str = include_str!("../NOTICE");

/// The Apache License 2.0's text (the crate's `LICENSE-APACHE` file).
pub const APACHE_LICENSE: &str = include_str!("../LICENSE-APACHE");

/// Work units charged to the caller's budget, on the scale of [`torque_collide::work`].
pub mod work {
    pub use torque_collide::work::{CANDIDATE, CONVEX, POLY, VERTEX};

    /// A body added, or one walked by a pass over the bodies.
    pub const BODY: u64 = 4;
    /// A body's share of one sub-step (the integration and its constraints' setup).
    pub const BODY_STEP: u64 = 24;
    /// A contact pair's share of one sub-step (the narrow phase and the solver).
    pub const PAIR_STEP: u64 = 16;
    /// A joint's share of one sub-step.
    pub const JOINT_STEP: u64 = 16;
    /// A map cell looked up.
    pub const CELL: u64 = 2;
    /// One point of a convex hull built (the hull's work is about this per point, the cap bounding the count).
    pub const HULL_POINT: u64 = 8;
}
