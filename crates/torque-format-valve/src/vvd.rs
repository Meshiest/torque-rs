//! VVD vertex files: a model's vertices (position, normal, UV, up to three bone weights) for its detail levels.
//!
//! **Layout.** A 64-byte header (`IDSV`, version 4, the model's checksum, the detail levels and each one's vertex count, the
//! fix-up table, the vertex and tangent data's offsets), then 48-byte vertices: three weights, three bone indices and their count,
//! the position, the normal and the UV. When the fix-up table is not empty, a detail level's list is the concatenation of the
//! fix-ups whose level is at least that level (each a run of vertices from the stored list).

use crate::read::{Error, Result, cap, count, f32_at, i32_at, invalid, slice, u8_at, vec3_at};

/// The largest vertex file read.
pub const MAX_FILE_BYTES: u64 = 64 << 20;
const HEADER_BYTES: usize = 64;
const VERTEX_BYTES: usize = 48;

/// One vertex.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub weights: [f32; 3],
    pub bones: [u8; 3],
    pub bone_count: u8,
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// A vertex file: the top detail level's vertices, fix-ups applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Vvd {
    pub checksum: i32,
    pub lods: i32,
    pub vertices: Vec<Vertex>,
}

impl Vvd {
    /// Reads a vertex file (its top detail level).
    pub fn parse(b: &[u8]) -> Result<Vvd> {
        cap("the VVD file's size", b.len() as u64, MAX_FILE_BYTES)?;
        if slice(b, 0, 4, "the VVD signature")? != b"IDSV" {
            return invalid("not a VVD vertex file (bad signature)");
        }
        let version = i32_at(b, 4, "the VVD header")?;
        if version != 4 {
            return invalid(format!("VVD version {version} is not read"));
        }
        if b.len() < HEADER_BYTES {
            return Err(Error::Truncated {
                what: "the VVD header",
            });
        }
        let checksum = i32_at(b, 8, "the VVD header")?;
        let lods = i32_at(b, 12, "the VVD header")?;
        let lod0 = i32_at(b, 16, "the VVD header")?;
        let fixups = i32_at(b, 48, "the VVD header")?;
        let fixup_at = usize::try_from(i32_at(b, 52, "the VVD header")?).unwrap_or(usize::MAX);
        let data = usize::try_from(i32_at(b, 56, "the VVD header")?).unwrap_or(usize::MAX);
        let room = b.len().saturating_sub(data);
        let stored = count(lod0, VERTEX_BYTES, room, "the VVD vertices")?;
        let read_vertex = |i: usize| -> Result<Vertex> {
            let at = data + i * VERTEX_BYTES;
            Ok(Vertex {
                weights: [
                    f32_at(b, at, "a vertex")?,
                    f32_at(b, at + 4, "a vertex")?,
                    f32_at(b, at + 8, "a vertex")?,
                ],
                bones: [
                    u8_at(b, at + 12, "a vertex")?,
                    u8_at(b, at + 13, "a vertex")?,
                    u8_at(b, at + 14, "a vertex")?,
                ],
                bone_count: u8_at(b, at + 15, "a vertex")?,
                position: vec3_at(b, at + 16, "a vertex")?,
                normal: vec3_at(b, at + 28, "a vertex")?,
                uv: [
                    f32_at(b, at + 40, "a vertex")?,
                    f32_at(b, at + 44, "a vertex")?,
                ],
            })
        };
        let mut vertices = Vec::with_capacity(stored);
        if fixups <= 0 {
            for i in 0..stored {
                vertices.push(read_vertex(i)?);
            }
        } else {
            let n = count(
                fixups,
                12,
                b.len().saturating_sub(fixup_at),
                "the VVD fix-ups",
            )?;
            for f in 0..n {
                let at = fixup_at + f * 12;
                let lod = i32_at(b, at, "a fix-up")?;
                let src = usize::try_from(i32_at(b, at + 4, "a fix-up")?)
                    .map_err(|_| Error::Invalid("a negative fix-up".into()))?;
                let len = usize::try_from(i32_at(b, at + 8, "a fix-up")?)
                    .map_err(|_| Error::Invalid("a negative fix-up".into()))?;
                if lod < 0 {
                    continue;
                }
                if src.checked_add(len).is_none_or(|e| e > stored) || vertices.len() + len > stored
                {
                    return invalid("a VVD fix-up past the vertices");
                }
                for i in src..src + len {
                    vertices.push(read_vertex(i)?);
                }
            }
        }
        Ok(Vvd {
            checksum,
            lods,
            vertices,
        })
    }
}
