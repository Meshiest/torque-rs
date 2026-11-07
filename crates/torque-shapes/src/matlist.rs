//! The material list at the end of a `.dts` (`TSMaterialList::read`, over `MaterialList::read`), version 24.
//!
//! ```text
//! u8   list version (1; anything else is the text form, which this crate does not read)
//! u32  count
//! count x ( u8 length, length bytes )          the names
//! count x u32                                  flags
//! count x u32 x 3                              reflectance maps, bump maps, detail maps
//! count x f32                                  detail scales
//! count x f32                                  reflection amounts
//! ```
//!
//! Torque strips each name to the part after its last `/` or `\` (older tools wrote paths) and deletes the name of a material
//! flagged [`IFL_MATERIAL`] after reading (its frames are other materials flagged [`IFL_FRAME`]). A name whose first byte is NUL
//! (an empty name) makes `MaterialList::read` fail; `TSMaterialList::read` then reads nothing more and `TSShape::read` ignores the result: the
//! list is the names before it, with no flags, and the shape loads ([`read`] returns them).
//!
//! [`MaterialList::raw`] keeps the list as the file writes it (the names with their paths, the six arrays, and for an early end the
//! claimed count and the empty name's own entry) so that [`write`](fn@write) gives the same bytes. No `.ifl` or `.dml` text is read here: an
//! `.ifl` material's frames are other materials of this list, and a `.dml` is only ever named.

use crate::shape::{Material, RawMaterial, RawMaterials};
use crate::{Charge, MAX_MATERIALS, ShapeError, pay, reserved, within};

/// `TSMaterialList::IflMaterial` (`BIT(27)`): the material is an `.ifl` animation; Torque deletes its name after reading.
pub const IFL_MATERIAL: u32 = 0x0800_0000;
/// `TSMaterialList::IflFrame` (`BIT(28)`): the material is a frame of an `.ifl`.
pub const IFL_FRAME: u32 = 0x1000_0000;
/// This engine's own bit (`BIT(16)`), not TGE's: the material's texture is sampled with nearest-neighbor filtering (pixel
/// art). TGE's flags use bits 0 to 8 and 27 to 31; bits 9 to 26 are unused, its exporters never set them and its reader keeps
/// the word as it is, so a TGE shape never has this bit and TGE ignores it in a shape that does. A material without it
/// draws as TGE's does.
pub const NEAREST: u32 = 0x0001_0000;

/// What one material list holds, and how many bytes it took.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialList {
    /// The runtime materials: stripped names and flags.
    pub materials: Vec<Material>,
    /// The list as the file writes it, for the writer.
    pub raw: RawMaterials,
    pub bytes: usize,
}

/// The name Torque keeps of a raw one: the C string (up to the first NUL), without the part up to its last `/` or `\`.
fn runtime_name(raw: &[u8]) -> Vec<u8> {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    let c = &raw[..end];
    let start = c
        .iter()
        .rposition(|&b| b == b'/' || b == b'\\')
        .map_or(0, |i| i + 1);
    c[start..].to_vec()
}

fn word(bytes: &[u8], at: usize) -> [u8; 4] {
    [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
}

/// Reads the material list that starts `bytes`.
pub fn read(bytes: &[u8], charge: Charge) -> Result<MaterialList, ShapeError> {
    let truncated = |what| ShapeError::Truncated {
        buffer: "material list",
        what,
    };
    let version = *bytes.first().ok_or_else(|| truncated("the version"))?;
    if version != 1 {
        return Err(ShapeError::Bad("material list version"));
    }
    let count_bytes: [u8; 4] = bytes
        .get(1..5)
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| truncated("the count"))?;
    let declared = u32::from_le_bytes(count_bytes);
    let count = within("material count", u64::from(declared), MAX_MATERIALS)?;
    pay(charge, 8 + bytes.len() as u64 / 64 + count as u64)?;
    let mut at = 5usize;
    let mut names: Vec<Vec<u8>> = reserved(count, bytes.len() - at, 1);
    for _ in 0..count {
        let entry = at;
        let len = usize::from(*bytes.get(at).ok_or_else(|| truncated("a name"))?);
        at += 1;
        let raw = bytes.get(at..at + len).ok_or_else(|| truncated("a name"))?;
        at += len;
        // `Stream::readString` fills a C buffer: a name whose first byte is NUL is empty, and an empty name makes
        // `MaterialList::read` return false (TSMaterialList::read then reads nothing more, and TSShape::read ignores
        // the result): the names before it are the list, with no flags, and the shape loads.
        if raw.first().is_none_or(|&b| b == 0) {
            let materials = names
                .iter()
                .map(|n| Material {
                    name: Some(runtime_name(n)),
                    flags: 0,
                })
                .collect();
            return Ok(MaterialList {
                materials,
                raw: RawMaterials::Early {
                    declared,
                    names,
                    terminator: bytes[entry..at].to_vec(),
                },
                bytes: at,
            });
        }
        names.push(raw.to_vec());
    }
    // Six arrays of `count` words follow the names: the flags, the reflectance, bump and detail maps, the detail scales and
    // the reflection amounts.
    let words = count
        .checked_mul(4)
        .and_then(|n| n.checked_mul(6))
        .ok_or(ShapeError::Bad("material count"))?;
    let end = at
        .checked_add(words)
        .filter(|&e| e <= bytes.len())
        .ok_or_else(|| truncated("the flag and map arrays"))?;
    let mut materials = Vec::new();
    let mut raws = Vec::new();
    for (i, name) in names.into_iter().enumerate() {
        let field = |k: usize| word(bytes, at + (k * count + i) * 4);
        let flags = u32::from_le_bytes(field(0));
        materials.push(Material {
            name: (flags & IFL_MATERIAL == 0).then(|| runtime_name(&name)),
            flags,
        });
        raws.push(RawMaterial {
            name,
            flags,
            reflectance_map: u32::from_le_bytes(field(1)),
            bump_map: u32::from_le_bytes(field(2)),
            detail_map: u32::from_le_bytes(field(3)),
            detail_scale: f32::from_le_bytes(field(4)),
            reflection_amount: f32::from_le_bytes(field(5)),
        });
    }
    Ok(MaterialList {
        materials,
        raw: RawMaterials::Complete(raws),
        bytes: end,
    })
}

/// The bytes of a material list as `TSMaterialList::write` makes them.
pub fn write(raw: &RawMaterials, out: &mut Vec<u8>) {
    out.push(1);
    match raw {
        RawMaterials::Complete(list) => {
            out.extend_from_slice(&(list.len() as u32).to_le_bytes());
            for m in list {
                out.push(m.name.len() as u8);
                out.extend_from_slice(&m.name);
            }
            let words = |out: &mut Vec<u8>, f: &dyn Fn(&RawMaterial) -> [u8; 4]| {
                for m in list {
                    out.extend_from_slice(&f(m));
                }
            };
            words(out, &|m| m.flags.to_le_bytes());
            words(out, &|m| m.reflectance_map.to_le_bytes());
            words(out, &|m| m.bump_map.to_le_bytes());
            words(out, &|m| m.detail_map.to_le_bytes());
            words(out, &|m| m.detail_scale.to_le_bytes());
            words(out, &|m| m.reflection_amount.to_le_bytes());
        }
        RawMaterials::Early {
            declared,
            names,
            terminator,
        } => {
            out.extend_from_slice(&declared.to_le_bytes());
            for name in names {
                out.push(name.len() as u8);
                out.extend_from_slice(name);
            }
            out.extend_from_slice(terminator);
        }
    }
}

#[cfg(test)]
#[path = "matlist_tests.rs"]
mod tests;
