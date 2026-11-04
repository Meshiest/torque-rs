//! The scene container's core, as TGE has it: the bins every scene object is kept in by its world box, with the overflow list,
//! and the walks `findObjects` and `castRay` make over them ([`bins`]); the boxes, distances and lists of the container's
//! searches ([`query`]); what those read of an object ([`geometry`]); and the transform maths of a scene object
//! ([`transform`]). The arithmetic and the search sort are called through a table a host may replace ([`library`]); the default
//! is plain `f32` and a stable sort. No script engine and no dependency: a host keeps its objects' state and answers
//! [`Geometry`].
//!
//! The container follows Torque3D's MIT-licensed `SceneContainer` (`scene/sceneContainer.cpp`, Copyright (c) 2012 GarageGames,
//! LLC), which is TGE 1.4's `Container`; the code here is this crate's own.

pub mod bins;
pub mod geometry;
pub mod library;
pub mod query;
pub mod transform;

pub use bins::{BinGeometry, Bins};
pub use geometry::{Charge, Exhausted, Geometry, NO_HIT, ray_box, ray_point};
