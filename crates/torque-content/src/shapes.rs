//! Shapes from model files: glTF (`.glb`, `.gltf`), Wavefront OBJ (`.obj`) and Blockbench (`.bbmodel`), each made into the
//! `.dts` bytes every shape reader takes ([`ImportedShape`]).
//!
//! - [`GltfFormatPlugin`] reads as a datablock's glTF shape file is read today ([`torque_import::native::dts_bytes`]), so the
//!   bytes are the same: files up to [`caps::MAX_NATIVE_FILE_BYTES`], their JSON up to [`caps::MAX_NATIVE_JSON_BYTES`], from any
//!   source (the reader is hardened and fuzzed). A `.gltf` loads when its buffers are `data:` URIs.
//! - [`ObjFormatPlugin`] and [`BlockbenchFormatPlugin`] read as `torque-convert` does (the reader with its default
//!   [`Options`], then [`dts::to_dts`] and [`write_dts`]), files up to [`caps::MAX_FILE_BYTES`], from trusted sources only.
//!   An OBJ's `.mtl` files are not read (its materials keep their names).
//!
//! Every load's work is paid from [`MAX_IMPORT_WORK`] units ([`ImportError::OutOfBudget`] past it).

use torque_engine::assets::{LoadCx, Spec};
use torque_engine::{EngineBuilder, Plugin, PluginId};
#[cfg(feature = "gltf")]
use torque_import::bbmodel;
use torque_import::modelcore::Model;
use torque_import::{ImportError, Options, caps, dts, obj};
use torque_shapes::write::write_dts;

use crate::{MAX_IMPORT_WORK, budget};

/// A shape a model file made: the `.dts` bytes every shape reader takes (`torque_shapes::dts::parse` reads them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedShape {
    /// The shape's `.dts` bytes.
    pub dts: Vec<u8>,
}

/// The `.dts` bytes of `model` (checked, at most [`torque_shapes::MAX_DTS_BYTES`]).
fn shape_of(model: &Model) -> Result<ImportedShape, ImportError> {
    let dts = write_dts(&dts::to_dts(model)?);
    if dts.len() > torque_shapes::MAX_DTS_BYTES {
        return Err(ImportError::TooLarge {
            what: "the shape made of the model",
            bytes: dts.len() as u64,
            cap: torque_shapes::MAX_DTS_BYTES as u64,
        });
    }
    Ok(ImportedShape { dts })
}

/// The shape of a glTF file's bytes, as a datablock's shape file is read ([`torque_import::native::dts_bytes`]).
#[cfg(feature = "gltf")]
pub fn read_gltf(bytes: &[u8]) -> Result<ImportedShape, ImportError> {
    let mut charge = budget(MAX_IMPORT_WORK);
    torque_import::native::dts_bytes(bytes, &mut charge).map(|dts| ImportedShape { dts })
}

/// The shape of an OBJ file's bytes (its `.mtl` files not read).
pub fn read_obj(bytes: &[u8]) -> Result<ImportedShape, ImportError> {
    let mut charge = budget(MAX_IMPORT_WORK);
    let model = obj::read(bytes, &mut |_, _| None, &Options::default(), &mut charge)?;
    shape_of(&model)
}

/// The shape of a Blockbench file's bytes.
#[cfg(feature = "gltf")]
pub fn read_bbmodel(bytes: &[u8]) -> Result<ImportedShape, ImportError> {
    let mut charge = budget(MAX_IMPORT_WORK);
    let model = bbmodel::read(bytes, &Options::default(), &mut charge)?;
    shape_of(&model)
}

/// The loader's message of an import error.
fn message(e: ImportError) -> String {
    e.to_string()
}

/// glTF shapes: `.glb` and `.gltf` as [`ImportedShape`], from any source (module docs).
#[cfg(feature = "gltf")]
#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "torque.format-gltf")]
pub struct GltfFormatPlugin;

#[cfg(feature = "gltf")]
impl GltfFormatPlugin {
    /// What the loader takes.
    pub const SPEC: Spec = Spec::untrusted(caps::MAX_NATIVE_FILE_BYTES as u64);
}

#[cfg(feature = "gltf")]
impl Plugin for GltfFormatPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .assets()
            .loader(&["glb", "gltf"], Self::SPEC, |bytes, _: &LoadCx<'_>| {
                read_gltf(bytes).map_err(message)
            });
    }
}

/// OBJ shapes: `.obj` as [`ImportedShape`], from trusted sources only (module docs).
#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "torque.format-obj")]
pub struct ObjFormatPlugin;

impl ObjFormatPlugin {
    /// What the loader takes.
    pub const SPEC: Spec = Spec::trusted(caps::MAX_FILE_BYTES as u64);
}

impl Plugin for ObjFormatPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .assets()
            .loader(&["obj"], Self::SPEC, |bytes, _: &LoadCx<'_>| {
                read_obj(bytes).map_err(message)
            });
    }
}

/// Blockbench shapes: `.bbmodel` as [`ImportedShape`], from trusted sources only (module docs).
#[cfg(feature = "gltf")]
#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "torque.format-blockbench")]
pub struct BlockbenchFormatPlugin;

#[cfg(feature = "gltf")]
impl BlockbenchFormatPlugin {
    /// What the loader takes.
    pub const SPEC: Spec = Spec::trusted(caps::MAX_FILE_BYTES as u64);
}

#[cfg(feature = "gltf")]
impl Plugin for BlockbenchFormatPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .assets()
            .loader(&["bbmodel"], Self::SPEC, |bytes, _: &LoadCx<'_>| {
                read_bbmodel(bytes).map_err(message)
            });
    }
}
