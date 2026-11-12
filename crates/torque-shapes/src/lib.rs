//! Torque shapes: `.dts` shapes and `.dsq` sequence files read as data, with no GPU.
//!
//! This crate has no external dependency. Collision, simulation and rendering can all build on it. Nothing here reads a file by itself:
//! the caller hands it bytes.
//!
//! **What is here:**
//! - [`shape`]: the model a server keeps ([`Shape`]: nodes, objects, decals, `.ifl` materials, sub-shapes, details, names, the
//!   keyframe arrays, object and decal states, triggers, sequences, materials, bounds, mesh positions and primitives) and the
//!   full parse ([`Model`]: the same plus the texture coordinates, normals, skin and sorted arrays the writer needs, and the
//!   [`shape::FileExtras`] that make the writer's output the file's bytes); [`Shape::for_server`] makes the first from the second.
//! - [`dts`]: the reader, [`dts::parse`] (`TSShape::read` and `assembleShape` over the three buffers), and [`dts::scan`], the
//!   same walk reduced to counts.
//! - [`dsq`]: the same for a `.dsq` (`importSequences`), and [`Shape::import_sequences`], which applies one to a shape.
//! - [`write`](mod@write): the writers, [`write::write_dts`] and [`write::write_dsq`], used by the tests and the fuzz (the round trip proves
//!   the reader skips no field).
//! - [`matlist`]: the material list at the end of a `.dts` (`TSMaterialList::read`).
//! - [`bounds`]: the semantic check, bounds, centres and radii recomputed from the parsed vertices.
//! - [`preload`]: what `ShapeBaseData::preload` finds in a shape: its `collision-` details and their `LOS-` details.
//! - [`collide`] and [`math`]: rays and boxes against the meshes of the collision details, as `TSShapeInstance::castRay`,
//!   `TSMesh::castRay` (a hull of planes, not triangles) and `buildPolyList` answer them, and the matrix arithmetic under them.
//! - [`instance`]: the animated pose, as `TSShapeInstance` keeps it: node matrices, `TSThread`, `animateNodes` with the
//!   blend and scale passes. A `ShapeBase` and the player's arm, look and eye chain run on it.
//! - [`library`]: the table the arithmetic of [`math`], [`collide`] and [`instance`] and the thread sort go through, which a
//!   host may replace (as Torque's `mMathFn.h` pointers are).
//! - `synth` and `fuzz` (feature `testing`, for tests): synthetic shapes made by the writer, their mutation, and the fuzz that
//!   holds the reader to the rules below.
//!
//! **Credits.** This crate follows the MIT-licensed Torque3D release of the Torque engine (GarageGames), whose shape code TGE
//! 1.4 shares; it reads TGE 1.4's version-24 files, and where TGE 1.4 differs it follows TGE and names the TGE routine. The
//! files followed:
//! - `Engine/source/ts/tsShape.cpp`, `tsShapeAlloc.cpp`, `tsShapeOldRead.cpp`, `tsMesh.cpp`, `tsCollision.cpp`,
//!   `tsAnimate.cpp`, `tsThread.cpp`, `tsTransform.cpp`, `tsIntegerSet.cpp`, `tsShapeInstance.cpp`, `tsSortedMesh.cpp`,
//!   `tsMaterialList.cpp`; `Engine/source/math/mMatrix.cpp`, `mQuat.cpp`, `mPlane.h`, `mBox.h`, `mMath_C.cpp`, `mMathFn.h`;
//!   `Engine/source/collision/abstractPolyList.cpp`, `earlyOutPolyList.cpp`: `Copyright (c) 2012 GarageGames, LLC`.
//! - `Engine/source/materials/materialList.cpp` and `Engine/source/T3D/shapeBase.cpp` (the preload): `Copyright (c) 2012
//!   GarageGames, LLC` and `Copyright (C) 2015 Faust Logic, Inc.`
//!
//! **Rules.** Library code never panics on hostile input (a `.dts` named by a script is hostile): every count is checked against
//! the bytes that remain and a named cap below before it is used, every product of file counts is a checked multiplication, and
//! what a bound refuses is a [`ShapeError`] value the caller turns into a message. Every vector sized
//! from a file count is reserved through the one helper, `reserved`, which cuts the request to what the remaining bytes could
//! hold and to [`MAX_ALLOC_BYTES`] and records the largest request ([`largest_request`]). Work is charged through a callback
//! ([`Charge`]) before it is done; a run that cannot pay ends with [`ShapeError::OutOfBudget`], not a partial shape. The caps are
//! generous bounds, well above any real shape.

pub mod bounds;
pub mod collide;
pub mod dsq;
pub mod dts;
#[cfg(any(test, feature = "testing"))]
pub mod fuzz;
pub mod instance;
pub mod library;
pub mod math;
pub mod matlist;
pub mod preload;
pub mod shape;
pub mod swing;
#[cfg(any(test, feature = "testing"))]
pub mod synth;
pub mod write;

pub use shape::{Model, Shape};

/// A budget callback: asked to pay `units` of work before the work is done; false when it cannot (the run stops).
pub use torque_bytes::Charge;
pub(crate) use torque_bytes::pay;

/// The largest `.dts` read.
pub const MAX_DTS_BYTES: usize = 16 << 20;
/// The largest `.dsq` read.
pub const MAX_DSQ_BYTES: usize = 1 << 20;
/// Nodes in one shape.
pub const MAX_SHAPE_NODES: usize = 4_096;
/// Objects in one shape.
pub const MAX_SHAPE_OBJECTS: usize = 4_096;
/// Details in one shape.
pub const MAX_SHAPE_DETAILS: usize = 256;
/// Meshes in one shape.
pub const MAX_SHAPE_MESHES: usize = 16_384;
/// Names in one shape.
pub const MAX_SHAPE_NAMES: usize = 16_384;
/// Sub-shapes in one shape.
pub const MAX_SHAPE_SUBSHAPES: usize = 256;
/// Sequences in one shape, imports included.
pub const MAX_SEQUENCES: usize = 1_024;
/// Materials in one shape's list.
pub const MAX_MATERIALS: usize = 1_024;
/// Vertices of one mesh.
pub const MAX_MESH_VERTS: usize = 1 << 20;
/// Triangles (or primitive elements) of one mesh.
pub const MAX_MESH_TRIANGLES: usize = 1 << 20;
/// Vertices of one shape, the ones shared with a parent mesh counted again.
pub const MAX_SHAPE_VERTS_TOTAL: usize = 4_194_304;
/// Primitives of one shape, the shared ones counted again.
pub const MAX_SHAPE_PRIMS_TOTAL: usize = 4_194_304;
/// The kept set of shapes, in bytes of model (a bound for callers that cache shapes).
pub const MAX_KEPT_SHAPE_BYTES: usize = 256 << 20;
/// The longest name of a shape's name table, in bytes (names are C strings in the 8-bit buffer; the crate bounds each one so
/// that a name cannot be the whole file).
pub const MAX_NAME_BYTES: usize = 4_096;

/// Why a shape or a sequence file is not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShapeError {
    /// Fewer than the four bytes of the version word.
    Short,
    /// The file is longer than the cap for its kind.
    TooLarge { bytes: usize, cap: usize },
    /// The low byte of the version word is above 24.
    NewerVersion(u8),
    /// A version below 19: Torque reads it with `readOldShape`; this crate keeps the version word only.
    OldVersion(u8),
    /// A version from 19 to 23: Torque reads them, this crate does not (yet).
    UnsupportedVersion(u8),
    /// A header size is not consistent with the file (the sections do not fit, `startU16 > startU8`, ...).
    BadHeader(&'static str),
    /// A section ran out of words, shorts or bytes: the buffer, and what was being read.
    Truncated {
        buffer: &'static str,
        what: &'static str,
    },
    /// A count is above its cap.
    Cap {
        what: &'static str,
        value: u64,
        cap: u64,
    },
    /// A field is outside what the structure allows (an index out of range, a mesh type this crate does not read, ...).
    Bad(&'static str),
    /// The budget callback refused the work.
    OutOfBudget,
}

impl std::fmt::Display for ShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShapeError::Short => write!(f, "shorter than a version word"),
            ShapeError::TooLarge { bytes, cap } => write!(f, "{bytes} bytes (cap {cap})"),
            ShapeError::NewerVersion(v) => write!(f, "version {v} is newer than 24"),
            ShapeError::OldVersion(v) => write!(f, "old version {v}"),
            ShapeError::UnsupportedVersion(v) => write!(f, "version {v} is not read"),
            ShapeError::BadHeader(what) => write!(f, "bad header: {what}"),
            ShapeError::Truncated { buffer, what } => {
                write!(f, "the {buffer} buffer ends inside {what}")
            }
            ShapeError::Cap { what, value, cap } => {
                write!(f, "{what} {value} is over the cap {cap}")
            }
            ShapeError::Bad(what) => write!(f, "bad {what}"),
            ShapeError::OutOfBudget => write!(f, "out of budget"),
        }
    }
}

impl std::error::Error for ShapeError {}

impl From<torque_bytes::OutOfBudget> for ShapeError {
    fn from(_: torque_bytes::OutOfBudget) -> ShapeError {
        ShapeError::OutOfBudget
    }
}

/// The most bytes one request through `reserved` asks the allocator for (the readers never ask for more, whatever a count in a
/// file claims; a vector still grows past it, a push at a time, as items are really read from bytes the file holds).
pub const MAX_ALLOC_BYTES: usize = 64 << 20;

/// The capacity to reserve for `count` items a file claims: at most the items the `bytes_left` could hold at `min_item` bytes
/// each, so that a count in a header never sizes an allocation the bytes cannot back.
pub(crate) fn capacity(count: usize, bytes_left: usize, min_item: usize) -> usize {
    count.min(bytes_left / min_item.max(1))
}

static LARGEST_REQUEST: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

thread_local! {
    static REQUEST_MARK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The largest request this thread has made through `reserved` since the last call, in bytes (and resets it): the fuzz asks
/// after each parse and holds the answer to the size of the file that was parsed.
pub fn take_request_mark() -> usize {
    REQUEST_MARK.with(|m| m.replace(0))
}

/// The largest allocation (in bytes) any `reserved` call in this process has asked for; the fuzz asserts it stays within
/// [`MAX_ALLOC_BYTES`] (a counting `GlobalAlloc` would need `unsafe`, which the workspace forbids).
pub fn largest_request() -> usize {
    LARGEST_REQUEST.load(std::sync::atomic::Ordering::Relaxed)
}

/// A vector for `count` items a file claims, sized by [`capacity`] and by [`MAX_ALLOC_BYTES`]: the only way the readers of this
/// crate reserve room for a count that comes from a file (a unit test fails on any other `with_capacity` in their source). The
/// request it makes is recorded for [`largest_request`].
pub(crate) fn reserved<T>(count: usize, bytes_left: usize, min_item: usize) -> Vec<T> {
    let item = std::mem::size_of::<T>().max(1);
    let n = capacity(count, bytes_left, min_item).min(MAX_ALLOC_BYTES / item);
    LARGEST_REQUEST.fetch_max(n * item, std::sync::atomic::Ordering::Relaxed);
    REQUEST_MARK.with(|m| m.set(m.get().max(n * item)));
    Vec::with_capacity(n)
}

/// A cap check: `value` against `cap`, as a [`ShapeError::Cap`].
pub(crate) fn within(what: &'static str, value: u64, cap: usize) -> Result<usize, ShapeError> {
    if value > cap as u64 {
        Err(ShapeError::Cap {
            what,
            value,
            cap: cap as u64,
        })
    } else {
        Ok(value as usize)
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
