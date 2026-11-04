//! The format-neutral map layer.
//!
//! A map format is read by an **importer** (`torque_maps::legacy` for Torque's legacy map files; a BSP importer
//! could follow) into the types here.
//! Hosting, networking, map lists, drawing, the relight, collision and the work budgets read these types only,
//! never a file format: they call an importer through [`Importer`] and get the intermediate back.
//!
//! **The parts** (each a module):
//! - [`geometry`]: a model's drawn triangles by material, each triangle's place in the model's baked lightmap pages, and those
//!   pages ([`geometry::Surfaces`]).
//! - [`collision`]: convex hulls as shared points and planes ([`collision::ConvexSet`]), heightfields with their material layers
//!   ([`collision::Heightfield`]) and their placement, triangle meshes ([`collision::TriMesh`]), each with its contents.
//! - [`material`]: materials (texture layers, flags, blending) and images (decoded, block-compressed or still encoded).
//! - [`scene`]: what a map places and how it looks: instances, the environment (sky, fog, sun, water), spawns, entities with
//!   their outputs, and whether its lightmaps are baked or made by a relight.
//! - [`listing`]: a map as a map list shows it (its `MissionInfo` name and description).
//! - [`source`]: where an importer's base content lives ([`source::AssetSource`]).
//! - [`budget`]: the caps the importers and the consumers share.
//!
//! **What the legacy format fills.** Each side builds the intermediate from the records and files it has: a terrain's
//! [`collision::Heightfield`] and [`collision::HeightfieldPlacement`], an interior's [`geometry::Surfaces`] and [`collision::ConvexSet`], a sky's material
//! list, the files a model names, and a mission's [`listing::Listing`]. Its lightmaps are made by the relight
//! ([`scene::Lighting::Relight`]); it has no entities. The other parts are the contract a BSP importer would fill first.
//!
//! **Hostile input.** Every type here can come from a download. An importer bounds what it makes (its readers' caps, and
//! [`budget`]'s); a consumer still checks every index it follows (a [`collision::HullRange`] names ranges as the file named them,
//! not held to its lists) and charges its work to its own budget.

pub mod budget;
pub mod collision;
pub mod geometry;
pub mod listing;
pub mod material;
pub mod scene;
pub mod source;

use std::fmt;

/// The kinds of file the consumers ask an importer about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileKind {
    /// A heightfield's file (legacy `.ter`).
    Heightfield,
    /// A model of world geometry and its collision (legacy `.dif`).
    Model,
    /// A sky's material list (legacy `.dml`).
    SkyList,
}

/// Why an importer did not make what it was asked for: a line a person can read (the readers' own error, as the
/// console prints it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportError(pub String);

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ImportError {}

/// A map format's readers, as the consumers see them. Each call reads one file's bytes (at most [`Importer::max_bytes`] of its
/// kind: the caller checks the size before reading) and makes its part of the intermediate, or says why not. Implementations
/// never panic on any bytes and bound their work by the file's size.
pub trait Importer: Sync {
    /// The format's name, for messages.
    fn name(&self) -> &'static str;
    /// The largest file of `kind` read.
    fn max_bytes(&self, kind: FileKind) -> usize;
    /// A heightfield file's samples and layers.
    fn heightfield(&self, bytes: &[u8]) -> Result<collision::Heightfield, ImportError>;
    /// A model file's drawn surfaces (its first detail level).
    fn surfaces(&self, bytes: &[u8]) -> Result<geometry::Surfaces, ImportError>;
    /// A model file's convex hulls (its first detail level).
    fn convexes(&self, bytes: &[u8]) -> Result<collision::ConvexSet, ImportError>;
    /// A sky list file's material names, in order.
    fn sky_list(&self, bytes: &[u8]) -> Result<Vec<String>, ImportError>;
    /// The other files a file of `kind` names, as it names them (a model's or heightfield's materials), or why it does not read.
    /// The caller resolves them against its own folders.
    fn named_files(&self, kind: FileKind, bytes: &[u8]) -> Result<Vec<String>, ImportError>;
}
