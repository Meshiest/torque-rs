//! Model readers and writers: files into the model layer (`torque-modelcore`) and the model layer into Torque shapes.
//!
//! **What is here:**
//! - [`gltf`] (feature `gltf`): the glTF 2.0 reader ([`gltf::read`]: `.glb`, or `.gltf` with its side files through a
//!   resolver) and writer ([`gltf::write_glb`]), which keeps what a shape needs and glTF has no word for (details, material
//!   flags, sequence flags, ground frames) in `extras`, so a shape written out and read back is the same shape.
//! - [`dts`]: the model layer as a shape ([`dts::to_dts`], through `torque-shape-builder`) and a shape as the model layer
//!   ([`dts::from_dts`]).
//! - [`native`] (feature `gltf`): what the engine calls when a datablock's shape file is a `.glb` or `.gltf`: the shape's
//!   `.dts` bytes, so everything that reads a `.dts` (drawing, animation, collision, the server's preload) reads it unchanged.
//!
//! **Frames.** Readers turn every file into Torque's frame (x right, y forward, z up, meters). glTF is y up: a glTF point
//! `(x, y, z)` is Torque's `(x, -z, y)`, the inverse of the axis change Blender's glTF exporter makes, so a model built in
//! Blender facing +y (Torque's way) comes out as it was built. Triangle fronts turn from glTF's counter-clockwise to the shape's
//! clockwise.
//!
//! **Hostile input.** A glTF can come from a server. Every count, size, offset and stride a file gives is checked against the
//! bytes there and a named cap ([`caps`]) before it is used; work is paid through a [`Charge`] callback before it is done; URIs
//! are relative paths only (no scheme, no absolute path, no `..`), `data:` URIs are size-capped, images are never decoded
//! (their headers' sizes are checked). What a bound refuses is an [`ImportError`], never a panic.
//!
//! **Credits.** The detail and collision naming of meshes (`box2`, `Col-1`, `LOSCol-9`, by `torque_modelcore::naming`), the
//! object of a mesh's levels and the key count of a sampled sequence (`duration * rate + 0.5`, plus one) follow Torque3D's
//! shape loader, `TSShapeLoader::generateObjects` and `generateSequences` (`Engine/source/ts/loader/tsShapeLoader.cpp`:
//! Copyright (c) 2012 GarageGames, LLC; MIT). The code here is this crate's own.

pub mod caps;
pub mod dts;
#[cfg(feature = "gltf")]
pub mod gltf;
#[cfg(feature = "gltf")]
pub mod native;

pub use torque_modelcore as modelcore;
pub use torque_shapes::Charge;

/// Why a file is not read or a model not written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    /// The file or one of its parts is larger than its cap.
    TooLarge {
        what: &'static str,
        bytes: u64,
        cap: u64,
    },
    /// A count is above its cap.
    Cap {
        what: &'static str,
        count: u64,
        cap: u64,
    },
    /// The bytes are not what the format says.
    Malformed(String),
    /// A part of the format this crate does not read (a sparse accessor, a non-base64 data URI).
    Unsupported(String),
    /// A URI that is not a relative path (a scheme, an absolute path, `..`).
    Uri(String),
    /// A side file the resolver did not give.
    Missing(String),
    /// The work budget ran out.
    OutOfBudget,
    /// The model fails `Model::check`, or a shape could not be made of it.
    Invalid(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::TooLarge { what, bytes, cap } => {
                write!(f, "{what} is {bytes} bytes, over the cap of {cap}")
            }
            ImportError::Cap { what, count, cap } => {
                write!(f, "{count} {what}, over the cap of {cap}")
            }
            ImportError::Malformed(s) => write!(f, "malformed: {s}"),
            ImportError::Unsupported(s) => write!(f, "not supported: {s}"),
            ImportError::Uri(s) => write!(f, "refused URI: {s}"),
            ImportError::Missing(s) => write!(f, "missing file: {s}"),
            ImportError::OutOfBudget => write!(f, "out of work budget"),
            ImportError::Invalid(s) => write!(f, "invalid model: {s}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<modelcore::Invalid> for ImportError {
    fn from(e: modelcore::Invalid) -> ImportError {
        ImportError::Invalid(e.0)
    }
}

/// `Err(ImportError::Cap)` when `count` is above `cap`.
pub(crate) fn cap(what: &'static str, count: usize, cap: usize) -> Result<(), ImportError> {
    if count > cap {
        Err(ImportError::Cap {
            what,
            count: count as u64,
            cap: cap as u64,
        })
    } else {
        Ok(())
    }
}

/// Pays `units` before the work.
pub(crate) fn pay(charge: &mut Charge, units: u64) -> Result<(), ImportError> {
    if charge(units) {
        Ok(())
    } else {
        Err(ImportError::OutOfBudget)
    }
}

/// How a reader turns a file into Torque's frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    /// The file's up axis: `'y'` (glTF's) or `'z'` (already Torque's).
    pub up: char,
    /// Meters a file unit.
    pub scale: f32,
    /// Keys a second an animation is sampled at when the file does not give its keys.
    pub rate: f32,
    /// Adds a box hull (`Collision-1`) around the drawn meshes of a model that has no collision mesh. Ignored for a file this
    /// crate wrote (it says exactly what the shape has).
    pub box_hull: bool,
    /// The largest JSON part read ([`caps::MAX_JSON_BYTES`]; less for a file a server can send).
    pub max_json_bytes: usize,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            up: 'y',
            scale: 1.0,
            rate: 30.0,
            box_hull: true,
            max_json_bytes: caps::MAX_JSON_BYTES,
        }
    }
}
