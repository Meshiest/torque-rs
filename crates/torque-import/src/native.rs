//! Native glTF shapes: a datablock's shape file may be a `.glb` or `.gltf`. [`shape_file`] gives the `.dts` bytes the engine's
//! shape readers take, so a glTF is drawn, animated, collided and preloaded exactly as the `.dts` [`crate::dts::to_dts`] makes of
//! it. A `.gltf` loads when its buffers are `data:` URIs (side files are for the converter); textures are found by name beside
//! the shape, as a `.dts`'s are.

use crate::{Charge, ImportError, Options, dts, gltf};

/// Whether a shape path names a glTF (`.glb` or `.gltf`, case ignored).
pub fn is_gltf_path(path: &[u8]) -> bool {
    let ends = |ext: &[u8]| {
        path.len() >= ext.len() && path[path.len() - ext.len()..].eq_ignore_ascii_case(ext)
    };
    ends(b".glb") || ends(b".gltf")
}

/// The `.dts` bytes of the glTF `bytes`, paid for through `charge`.
pub fn dts_bytes(bytes: &[u8], charge: Charge) -> Result<Vec<u8>, ImportError> {
    let model = gltf::read(bytes, &mut |_| None, &Options::default(), charge)?;
    let shape = dts::to_dts(&model)?;
    let out = torque_shapes::write::write_dts(&shape);
    if out.len() > torque_shapes::MAX_DTS_BYTES {
        return Err(ImportError::TooLarge {
            what: "the shape made of the glTF",
            bytes: out.len() as u64,
            cap: torque_shapes::MAX_DTS_BYTES as u64,
        });
    }
    Ok(out)
}

/// The bytes of the shape file at `path`: unchanged unless `path` names a glTF ([`is_gltf_path`]), then its `.dts` bytes.
pub fn shape_file(path: &[u8], bytes: Vec<u8>, charge: Charge) -> Result<Vec<u8>, ImportError> {
    if is_gltf_path(path) {
        dts_bytes(&bytes, charge)
    } else {
        Ok(bytes)
    }
}
