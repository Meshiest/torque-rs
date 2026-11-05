//! Model readers and writers: files into the model layer (`torque-modelcore`) and the model layer into Torque shapes.
//!
//! **What is here:**
//! - [`gltf`] (feature `gltf`): the glTF 2.0 reader ([`gltf::read`]: `.glb`, or `.gltf` with its side files through a
//!   resolver) and writer ([`gltf::write_glb`]), which keeps what a shape needs and glTF has no word for (details, material
//!   flags, sequence flags, ground frames) in `extras`, so a shape written out and read back is the same shape.
//! - [`obj`]: the Wavefront OBJ reader ([`obj::read`], `.mtl` files through a resolver): static meshes, groups as meshes.
//! - [`bbmodel`] (feature `gltf`, for its JSON parser and image headers): the Blockbench reader ([`bbmodel::read`]): cubes,
//!   meshes, the outliner's groups as nodes, per-face coordinates and the textures the file carries.
//! - [`dts`]: the model layer as a shape ([`dts::to_dts`], through `torque-shape-builder`) and a shape as the model layer
//!   ([`dts::from_dts`]).
//! - [`native`] (feature `gltf`): what the engine calls when a datablock's shape file is a `.glb` or `.gltf`: the shape's
//!   `.dts` bytes, so everything that reads a `.dts` (drawing, animation, collision, the server's preload) reads it unchanged.
//!
//! **Frames.** Readers turn every file into Torque's frame (x right, y forward, z up, meters). glTF is y up: a glTF point
//! `(x, y, z)` is Torque's `(x, -z, y)`, the inverse of the axis change Blender's glTF exporter makes, so a model built in
//! Blender facing +y (Torque's way) comes out as it was built. Triangle fronts turn from glTF's counter-clockwise to the shape's
//! clockwise. OBJ takes the same turn, and its texture v the other way up.
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

#[cfg(feature = "gltf")]
pub mod bbmodel;
pub mod caps;
pub mod dts;
#[cfg(feature = "gltf")]
pub mod gltf;
#[cfg(feature = "gltf")]
pub mod native;
pub mod obj;

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

/// A box hull (`Col-1` in `Collision-1`) around every drawn mesh at rest, on the first root node, for a model with no
/// collision mesh.
pub fn add_box_hull(model: &mut modelcore::Model) {
    let has_collision = model.meshes.iter().any(|m| {
        m.detail
            .and_then(|d| model.details.get(d))
            .is_some_and(|d| modelcore::naming::role(&d.name) == modelcore::naming::Role::Collision)
    });
    let Some(root) = model.nodes.iter().position(|n| n.parent.is_none()) else {
        return;
    };
    if has_collision || model.check().is_err() {
        return;
    }
    let world = model.rest_world();
    let to_root = modelcore::math::inverse(&world[root]);
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for (i, m) in model.meshes.iter().enumerate() {
        let drawn = m
            .detail
            .and_then(|d| model.details.get(d))
            .is_some_and(|d| d.size >= 0.0);
        if !drawn {
            continue;
        }
        for p in model.placed_positions(i, &world) {
            let p = modelcore::math::point(&to_root, p);
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
    }
    if min[0] > max[0] {
        return;
    }
    let hull = torque_shape_builder::MeshData::hull_box(min, max);
    model.details.push(modelcore::Detail {
        name: "Collision-1".into(),
        size: -1.0,
    });
    model
        .notes
        .push("no collision mesh: a box hull was added".into());
    model.meshes.push(modelcore::Mesh {
        name: "Col-1".into(),
        node: root,
        detail: Some(model.details.len() - 1),
        positions: hull.positions,
        normals: hull.normals,
        uv0: hull.tverts,
        primitives: vec![modelcore::Primitive {
            material: None,
            indices: hull.indices.iter().map(|&i| u32::from(i)).collect(),
        }],
        ..modelcore::Mesh::default()
    });
}
