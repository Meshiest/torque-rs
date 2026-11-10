//! VTX mesh files (`.dx90.vtx`, `.dx80.vtx`, `.sw.vtx`): a model's triangles, laid out for the hardware, per body part, model,
//! detail level and mesh.
//!
//! **Layout** (packed, version 7): a 36-byte header (version, the vertex cache size, bone limits, the model's checksum, the
//! detail levels, the material replacements, the body parts); body parts (8 bytes) hold models (8), models hold detail levels
//! (12), levels hold meshes (9), meshes hold strip groups (25 bytes; 33 in later games, which add a topology list), each a list
//! of 9-byte vertices (their place in the mesh's vertex list among them) and of 16-bit indices into it, cut into strips. The
//! strips here are triangle lists covering the group's indices, so a group's indices are read as one triangle list.
//!
//! [`Vtx::parse`] keeps the top detail level of every model: per mesh, triangles as indices into that mesh's vertices.

use crate::read::{Error, Result, cap, count, i32_at, invalid, u16_at};

/// The largest mesh file read.
pub const MAX_FILE_BYTES: u64 = 64 << 20;
/// The most indices one file gives, all strip groups together (and never more than the file's bytes hold, two a index).
pub const MAX_TOTAL_INDICES: usize = 1 << 24;
/// The most vertices one file gives, all strip groups together (and never more than the file's bytes hold, nine a vertex).
pub const MAX_TOTAL_VERTICES: usize = 1 << 22;
/// The most structures (body parts, models, meshes, strip groups) one file lists, all together.
const MAX_STRUCTURES: usize = 1 << 20;

/// What a file may still give: groups whose offsets overlap read the same bytes again, so the totals are kept across the whole
/// file, not per group.
struct Budget {
    indices: usize,
    vertices: usize,
    structures: usize,
}

impl Budget {
    fn take(left: &mut usize, n: usize, what: &'static str, cap: usize) -> Result<()> {
        *left = left.checked_sub(n).ok_or(Error::TooLarge {
            what,
            asked: n as u64,
            cap: cap as u64,
        })?;
        Ok(())
    }
}

/// The top detail level's triangles, per body part, model and mesh: each a list of mesh-vertex indices, three a triangle.
#[derive(Debug, Clone, PartialEq)]
pub struct Vtx {
    pub checksum: i32,
    pub body_parts: Vec<Vec<Vec<Vec<u32>>>>,
}

/// A count at `at` and an offset after it, relative to the structure at `base`, checked to fit `stride`-byte items.
fn list(
    b: &[u8],
    base: usize,
    at: usize,
    stride: usize,
    what: &'static str,
) -> Result<(usize, usize)> {
    let n = i32_at(b, at, what)?;
    let off = i32_at(b, at + 4, what)?;
    if n == 0 {
        return Ok((0, 0));
    }
    let start = crate::read::rel(base, off, what)?;
    Ok((
        count(n, stride, b.len().saturating_sub(start), what)?,
        start,
    ))
}

impl Vtx {
    /// Reads a mesh file.
    pub fn parse(b: &[u8]) -> Result<Vtx> {
        cap("the VTX file's size", b.len() as u64, MAX_FILE_BYTES)?;
        let version = i32_at(b, 0, "the VTX header")?;
        if version != 7 {
            return invalid(format!("VTX version {version} is not read"));
        }
        let checksum = i32_at(b, 16, "the VTX header")?;
        // Strip groups are 25 bytes in the older layout and 33 in the newer; the newer is taken when the older does not read.
        match Vtx::read_parts(b, 25) {
            Ok(body_parts) => Ok(Vtx {
                checksum,
                body_parts,
            }),
            Err(first) => Vtx::read_parts(b, 33)
                .map(|body_parts| Vtx {
                    checksum,
                    body_parts,
                })
                .map_err(|_| first),
        }
    }

    fn read_parts(b: &[u8], group_bytes: usize) -> Result<Vec<Vec<Vec<Vec<u32>>>>> {
        let max_indices = MAX_TOTAL_INDICES.min(b.len() / 2);
        let max_vertices = MAX_TOTAL_VERTICES.min(b.len() / 9);
        let mut budget = Budget {
            indices: max_indices,
            vertices: max_vertices,
            structures: MAX_STRUCTURES,
        };
        let (nparts, poff) = list(b, 0, 28, 8, "the VTX body parts")?;
        Budget::take(
            &mut budget.structures,
            nparts,
            "the VTX body parts",
            MAX_STRUCTURES,
        )?;
        let mut parts = Vec::with_capacity(nparts);
        for p in 0..nparts {
            let (nmodels, moff) = list(b, poff + p * 8, poff + p * 8, 8, "a VTX body part")?;
            Budget::take(
                &mut budget.structures,
                nmodels,
                "the VTX models",
                MAX_STRUCTURES,
            )?;
            let mut models = Vec::with_capacity(nmodels);
            for m in 0..nmodels {
                let (nlods, loff) = list(b, moff + m * 8, moff + m * 8, 12, "a VTX model")?;
                if nlods == 0 {
                    models.push(Vec::new());
                    continue;
                }
                // The top detail level only.
                let (nmeshes, meshoff) = list(b, loff, loff, 9, "a VTX detail level")?;
                Budget::take(
                    &mut budget.structures,
                    nmeshes,
                    "the VTX meshes",
                    MAX_STRUCTURES,
                )?;
                let mut meshes = Vec::with_capacity(nmeshes);
                for k in 0..nmeshes {
                    let (ngroups, goff) = list(
                        b,
                        meshoff + k * 9,
                        meshoff + k * 9,
                        group_bytes,
                        "a VTX mesh",
                    )?;
                    Budget::take(
                        &mut budget.structures,
                        ngroups,
                        "the VTX strip groups",
                        MAX_STRUCTURES,
                    )?;
                    let mut tris = Vec::new();
                    for g in 0..ngroups {
                        let at = goff + g * group_bytes;
                        let (nverts, voff) = list(b, at, at, 9, "a VTX strip group's vertices")?;
                        let (nidx, ioff) = list(b, at, at + 8, 2, "a VTX strip group's indices")?;
                        Budget::take(
                            &mut budget.indices,
                            nidx,
                            "the VTX file's indices",
                            max_indices,
                        )?;
                        Budget::take(
                            &mut budget.vertices,
                            nverts,
                            "the VTX file's vertices",
                            max_vertices,
                        )?;
                        let mut verts = Vec::with_capacity(nverts);
                        for v in 0..nverts {
                            verts.push(u32::from(u16_at(b, voff + v * 9 + 4, "a VTX vertex")?));
                        }
                        for t in 0..nidx / 3 {
                            let mut tri = [0u32; 3];
                            for (c, slot) in tri.iter_mut().enumerate() {
                                let i =
                                    usize::from(u16_at(b, ioff + (t * 3 + c) * 2, "a VTX index")?);
                                *slot = *verts.get(i).ok_or(Error::Invalid(
                                    "a VTX index past its vertices".into(),
                                ))?;
                            }
                            tris.extend_from_slice(&tri);
                        }
                    }
                    meshes.push(tris);
                }
                models.push(meshes);
            }
            parts.push(models);
        }
        Ok(parts)
    }
}
