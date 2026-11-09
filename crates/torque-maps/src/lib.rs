//! Torque's legacy map files (interiors for `InteriorInstance`, terrains for `TerrainBlock`), read as data.
//!
//! The layouts follow Torque3D's MIT-licensed source. `Copyright (c) 2012 GarageGames, LLC`: 2.0's `Engine/source/interior/`
//! (`interiorRes.cpp`, `interiorIO.cpp`, `forceField.cpp`, `interiorResObjects.cpp`, `mirrorSubObject.cpp`,
//! `interiorSimpleMesh.cpp`; `interior.cpp` for the drawing and special materials the importer follows) for `.dif`, 3.10.1's
//! `terrain/terrFile.cpp` (`TerrainFile::_loadLegacy`) for `.ter`, and `core/stream/stream.cpp` (`Stream::read(String*)`).
//! `Copyright (c) 2012 GarageGames, LLC` and `Copyright (C) 2015 Faust Logic, Inc.`: `materials/materialList.cpp`
//! (`MaterialList::read`, for `.dml` and inside `.dif`) and `terrain/terrData.cpp` (`TerrainBlock::onAdd`). The code here is
//! written for this crate from those layouts.
//!
//! **What is here:**
//! - [`dif`]: an interior resource (resource version 44) with its interiors (versions 0 to 14), sub-objects, triggers, path
//!   followers, force fields, AI nodes, game entities and vehicle collision ([`dif::parse`]), and [`dif::Interior::check`], the
//!   cross-reference check the collision and drawing code relies on.
//! - [`ter`]: a legacy terrain (versions 1 to 6: 256 by 256 heights, the material map, up to eight names, the alpha maps).
//! - [`dml`]: a text material list (a sky's `.dml`).
//! - [`synth`] and [`fuzz`]: small synthetic files made by a writer, their mutation, and the fuzz that holds the readers to the
//!   rules below.
//! - [`legacy`]: the importer: the files made into the format-neutral map layer ([`torque_mapcore`]), the only form hosting, collision
//!   and drawing read them in.
//!
//! **Rules.** These files come from download sites and from servers (a client downloads them), so they are hostile input. Library
//! code never panics on them: every read is bounded by the bytes that remain, every count is checked against a named cap and
//! against what the remaining bytes could hold before anything is allocated for it, and every vector sized from a file count is
//! reserved through one helper (`reserved`), which records the largest request ([`largest_request`]) and the bytes a parse asked
//! for in all ([`parse_requests`]). Every item of a vector consumes bytes of the file, so a parse never asks for more than a small
//! multiple of the file's size ([`MAX_ALLOC_PER_BYTE`] in each of at most [`MAX_READINGS`] readings). The work is linear in the file's size: nothing recurses and nothing
//! loops over a range a file names except the check's ranges, which are tested, not walked.
//!
//! # Maintaining
//!
//! **Work is tied to the file, not only to its size.** Allocations are counted per byte of input; a pass over a list that a file
//! can make long (a `.dif`'s materials, surfaces, hulls) must be linear in it.
//!
//! **Invariants and their tests.**
//! - Material names of a model are de-duplicated with a set, not by comparing each with every name kept
//!   (`legacy_tests::a_model_with_many_distinct_materials_is_named_in_linear_time`, which has a timing bound;
//!   `material_names_that_differ_only_in_case_are_named_once_and_keep_their_first_spelling` pins the folding).
//! - The fuzz (`tests/fuzz.rs`) runs the importer's `named_files` and `surfaces` over each mutant, reports its longest case and
//!   fails past a bound, with the allocation caps checked as before.

pub mod dif;
pub mod dml;
pub mod fuzz;
pub mod legacy;
pub mod png;
pub mod synth;
pub mod ter;

mod read;

use std::cell::Cell;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The largest `.dif` read.
pub const MAX_DIF_BYTES: usize = 64 << 20;
/// The largest `.ter` read (a legacy file is under 0.9 MB plus its two scripts).
pub const MAX_TER_BYTES: usize = 16 << 20;
/// The largest `.dml` read.
pub const MAX_DML_BYTES: usize = 64 << 10;
/// The most items of any one vector in a file.
pub const MAX_ITEMS: usize = 1 << 24;
/// The most interiors of a resource: detail levels and sub-objects each.
pub const MAX_INTERIORS: usize = 256;
/// The most entries of the small lists of a resource (triggers, path followers, force fields, AI nodes, game entities) and of an
/// interior's sub-objects and static meshes.
pub const MAX_ENTRIES: usize = 4_096;
/// The largest single allocation a reader asks for.
pub const MAX_ALLOC_BYTES: usize = 256 << 20;
/// The most bytes one reading asks for per byte of its file: every item consumes at least one byte and no item kept is more than
/// this many times its size in the file (a 2-byte dictionary entry is kept as two empty strings of 24 bytes each, a 6-byte plane
/// as 16 bytes, a 1-byte bool as 1).
pub const MAX_ALLOC_PER_BYTE: usize = 32;
/// The most readings of one file's bytes in a parse: a `.dif` is read again from the start with the border word
/// (`dif::parse_with_border_pad`), and each interior again from its start the plain way (`dif::parse`). What a failed reading asked for is freed before the next.
pub const MAX_READINGS: usize = 4;

/// What a reader refused, with what it was reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapError {
    /// The file ended inside a field.
    Short { what: &'static str, at: usize },
    /// The file is larger than its kind's cap.
    TooBig { len: usize, cap: usize },
    /// A count above its cap, or above what the remaining bytes could hold.
    Count {
        what: &'static str,
        value: u64,
        cap: u64,
    },
    /// A version the readers do not know.
    Version { what: &'static str, found: u32 },
    /// A value that is not valid where it is (a bad key, a bad encoding, a nonzero reserved word).
    Bad {
        what: &'static str,
        value: u64,
        at: usize,
    },
    /// A reference outside the vector it names (the check).
    Index {
        what: &'static str,
        value: u64,
        len: u64,
    },
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MapError::Short { what, at } => write!(f, "file ends inside {what} at byte {at}"),
            MapError::TooBig { len, cap } => {
                write!(f, "file of {len} bytes is over the cap of {cap}")
            }
            MapError::Count { what, value, cap } => write!(f, "{what} count {value} is over {cap}"),
            MapError::Version { what, found } => write!(f, "{what} version {found} is not read"),
            MapError::Bad { what, value, at } => write!(f, "bad {what} {value} at byte {at}"),
            MapError::Index { what, value, len } => {
                write!(f, "{what} {value} is outside its {len} entries")
            }
        }
    }
}

impl std::error::Error for MapError {}

static LARGEST_REQUEST: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static PARSE_REQUESTS: Cell<usize> = const { Cell::new(0) };
}

/// The largest single allocation any reader has asked for in this process, in bytes. The fuzz holds it under
/// [`MAX_ALLOC_BYTES`] (a counting `GlobalAlloc` would need `unsafe`, which the workspace forbids).
pub fn largest_request() -> usize {
    LARGEST_REQUEST.load(Ordering::Relaxed)
}

/// The bytes the readers on this thread have asked for since the last [`reset_parse_requests`].
pub fn parse_requests() -> usize {
    PARSE_REQUESTS.with(Cell::get)
}

/// Starts a new count for [`parse_requests`].
pub fn reset_parse_requests() {
    PARSE_REQUESTS.with(|c| c.set(0));
}

/// Records a request of `bytes` (a vector reserved, or bytes copied out of a file).
pub(crate) fn record(bytes: usize) {
    LARGEST_REQUEST.fetch_max(bytes, Ordering::Relaxed);
    PARSE_REQUESTS.with(|c| c.set(c.get().saturating_add(bytes)));
}

/// A vector for `count` items a file claims, each at least `min_item` bytes of the `bytes_left`: the only way the readers of this
/// crate reserve room for a count that comes from a file. The count is cut to what the bytes could hold and to
/// [`MAX_ALLOC_BYTES`], and the request is recorded.
pub(crate) fn reserved<T>(count: usize, bytes_left: usize, min_item: usize) -> Vec<T> {
    let item = std::mem::size_of::<T>().max(1);
    let n = count
        .min(bytes_left / min_item.max(1))
        .min(MAX_ALLOC_BYTES / item);
    record(n * item);
    Vec::with_capacity(n)
}

/// A file's size against its kind's cap.
pub(crate) fn size_ok(len: usize, cap: usize) -> Result<(), MapError> {
    if len > cap {
        Err(MapError::TooBig { len, cap })
    } else {
        Ok(())
    }
}

/// A name from a file as text: bytes 0x80 and above are Latin-1 (each maps to the code point of its value), so no name is lost.
pub fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
