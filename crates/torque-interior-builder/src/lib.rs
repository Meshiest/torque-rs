//! Torque interiors (`.dif`) built from brushes: convex solids made in code ([`Brush::cuboid`], [`Brush::from_planes`]) or
//! read from a Quake or Valve 220 `.map` ([`map`]).
//!
//! [`build()`] makes one interior of a version-0 `.dif` in TGE's form, the form TGE, Torque3D and `torque_maps` all read:
//! hidden faces removed ([`csg`]), surfaces with their texture planes, triangle strips and lightmap rectangles, the brushes as
//! convex hulls with emit strings and polygon lists ([`hull`]), a solid BSP ([`bsp`]) and one zone. [`Built::bytes`] writes it
//! through [`torque_maps::dif::write`]. The engine draws the surfaces and collides with the hulls; the BSP, the emit strings and
//! the polygon lists are written for TGE and Torque3D and checked by this crate's tests.
//!
//! **Mission objects.** A water volume, a trigger or a physical zone is not interior geometry in Torque: each is an object of
//! the mission beside the `InteriorInstance`. A `.map`'s brush entities of those classes, and its point entities, are passed
//! through as [`mission::MissionObject`]s, placed with the interior ([`mission::Placement`]) and written as a `.mis` fragment.
//!
//! **Conventions.** Torque's axes (x right, y forward, z up), a `.map`'s too. A face's plane faces out of its brush; polygons
//! and windings run clockwise seen from the front.

pub mod brush;
pub mod bsp;
pub mod build;
pub mod csg;
pub mod geom;
pub mod hull;
pub mod lightmap;
pub mod map;
pub mod mission;
#[cfg(any(test, feature = "verify"))]
pub mod verify;

pub use brush::{Brush, BrushError, Face, TexGen};
pub use build::{BuildError, Built, Options, build, is_null_material};
pub use geom::{Plane, V3};
