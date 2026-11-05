//! Torque interiors (`.dif`) built from brushes: convex solids made in code ([`Brush::cuboid`], [`Brush::from_planes`]) or
//! read from a Quake or Valve 220 `.map` ([`map`]); and from meshes of the model layer ([`mesh`]: glTF, OBJ, any reader of
//! `torque-import`), their polygons drawn as they are, their hulls authored, cut from closed parts or made as slabs under open
//! ones ([`build_polygons`]).
//!
//! [`build()`] makes one interior of a version-0 `.dif` in TGE's form, the form TGE, Torque3D and `torque_maps` all read:
//! hidden faces removed ([`csg`]), surfaces with their texture planes, triangle strips and lightmap rectangles, the brushes as
//! convex hulls with emit strings and polygon lists ([`hull`]), a solid BSP ([`bsp`]) and one zone. [`Built::bytes`] writes it
//! through [`torque_maps::dif::write`]. The engine draws the surfaces and collides with the hulls; the BSP, the emit strings and
//! the polygon lists are written for TGE and Torque3D and checked by this crate's tests.
//!
//! **Materials.** How a model's materials are drawn (nearest-neighbor filtering, see-through glass) is carried as this
//! engine's surface flags ([`materials`]: [`materials::surface_flags`] of the model, [`materials::mark`] on the built `.dif`).
//!
//! **Mission objects.** A water volume, a trigger or a physical zone is not interior geometry in Torque: each is an object of
//! the mission beside the `InteriorInstance`. A `.map`'s brush entities of those classes, and its point entities, are passed
//! through as [`mission::MissionObject`]s, placed with the interior ([`mission::Placement`]) and written as a `.mis` fragment;
//! a mesh named `water*`, `trigger*` or `physical_zone*` too.
//!
//! **Conventions.** Torque's axes (x right, y forward, z up), a `.map`'s too. A face's plane faces out of its brush; polygons
//! and windings run clockwise seen from the front.

pub mod brush;
pub mod bsp;
pub mod build;
pub mod convex;
pub mod csg;
pub mod geom;
pub mod hull;
pub mod lightmap;
pub mod map;
pub mod materials;
pub mod mesh;
pub mod mission;
#[cfg(any(test, feature = "verify"))]
pub mod verify;

pub use brush::{Brush, BrushError, Face, TexGen};
pub use build::{
    BuildError, Built, Options, Polygon, Source, build, build_polygons, is_null_material,
};
pub use geom::{Plane, V3};
