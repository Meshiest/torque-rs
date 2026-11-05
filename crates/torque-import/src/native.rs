//! Native glTF shapes: a datablock's shape file may be a `.glb` or `.gltf`. [`shape_file`] gives the `.dts` bytes the engine's
//! shape readers take, so a glTF is drawn, animated, collided and preloaded exactly as the `.dts` [`crate::dts::to_dts`] makes of
//! it. A `.gltf` loads when its buffers are `data:` URIs (side files are for the converter); textures are found by name beside
//! the shape, as a `.dts`'s are.

use crate::{Charge, ImportError, Options, caps, dts, gltf};

/// Whether a shape path names a glTF (`.glb` or `.gltf`, case ignored).
pub fn is_gltf_path(path: &[u8]) -> bool {
    let ends = |ext: &[u8]| {
        path.len() >= ext.len() && path[path.len() - ext.len()..].eq_ignore_ascii_case(ext)
    };
    ends(b".glb") || ends(b".gltf")
}

/// The work units the shape writing is charged a vertex, index and sampled value of the model.
const WRITE_WORK_PER_ELEMENT: u64 = 4;

/// The `.dts` bytes of the glTF `bytes` (at most [`caps::MAX_NATIVE_FILE_BYTES`], its JSON at most
/// [`caps::MAX_NATIVE_JSON_BYTES`]: a server sends these), paid for through `charge`: the read, then the shape made of it.
pub fn dts_bytes(bytes: &[u8], charge: Charge) -> Result<Vec<u8>, ImportError> {
    if bytes.len() > caps::MAX_NATIVE_FILE_BYTES {
        return Err(ImportError::TooLarge {
            what: "a glTF shape file",
            bytes: bytes.len() as u64,
            cap: caps::MAX_NATIVE_FILE_BYTES as u64,
        });
    }
    let options = Options {
        max_json_bytes: caps::MAX_NATIVE_JSON_BYTES,
        ..Options::default()
    };
    let mut charge = charge;
    let model = gltf::read(bytes, &mut |_, _| None, &options, &mut charge)?;
    let elements: usize = model
        .meshes
        .iter()
        .map(|m| m.positions.len() + m.primitives.iter().map(|p| p.indices.len()).sum::<usize>())
        .chain(
            model
                .animations
                .iter()
                .map(|a| a.keys.saturating_mul(a.tracks.len() + 1)),
        )
        .fold(0, usize::saturating_add);
    if !charge((elements as u64).saturating_mul(WRITE_WORK_PER_ELEMENT)) {
        return Err(ImportError::OutOfBudget);
    }
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
