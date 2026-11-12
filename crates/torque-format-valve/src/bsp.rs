//! VBSP maps (versions 19 to 21): a header of 64 lumps (offset, length, version, and a size when compressed) over the map's data:
//! planes, vertices, edges, faces and their texture placement, the BSP tree's nodes and leaves, the brush models, displacements,
//! the entity text, the game lumps (static props among them), the lightmaps and an embedded zip of the map's own files (the
//! pak lump).
//!
//! [`Bsp::parse`] checks the header and every lump's place; the readers below read one lump each into plain records, and
//! [`Bsp::build`] makes the drawn surfaces: brush faces fanned into triangles and displacements made into their grids, each
//! with texture coordinates, a normal, the displacement's blend alpha and a lightmap sample at each vertex, grouped by
//! material. Lumps compressed with LZMA (later games' maps) are refused, not read. Every count and index is checked.

use std::collections::BTreeMap;

use torque_bytes::crc32;
use torque_resource::zips::ZipMount;

use crate::kv;
use crate::read::{
    Error, Result, cap, f32_at, i16_at, i32_at, invalid, slice, u8_at, u16_at, u32_at, vec3_at,
};

/// The largest map file read.
pub const MAX_FILE_BYTES: u64 = 512 << 20;
/// The most vertices a built map makes, displacements included.
pub const MAX_BUILT_VERTICES: usize = 1 << 24;
/// The most edges of one face.
const MAX_FACE_EDGES: usize = 4096;
/// The largest displacement power (a 17 x 17 grid).
const MAX_DISP_POWER: i32 = 4;

/// Lump numbers.
pub mod lump {
    pub const ENTITIES: usize = 0;
    pub const PLANES: usize = 1;
    pub const TEXDATA: usize = 2;
    pub const VERTEXES: usize = 3;
    pub const NODES: usize = 5;
    pub const TEXINFO: usize = 6;
    pub const FACES: usize = 7;
    pub const LIGHTING: usize = 8;
    pub const WORLDLIGHTS: usize = 15;
    pub const WORLDLIGHTS_HDR: usize = 54;
    pub const LEAF_AMBIENT_INDEX_HDR: usize = 51;
    pub const LEAF_AMBIENT_INDEX: usize = 52;
    pub const LEAF_AMBIENT_LIGHTING_HDR: usize = 55;
    pub const LEAF_AMBIENT_LIGHTING: usize = 56;
    pub const LEAFS: usize = 10;
    pub const EDGES: usize = 12;
    pub const SURFEDGES: usize = 13;
    pub const MODELS: usize = 14;
    pub const LEAFFACES: usize = 16;
    pub const DISPINFO: usize = 26;
    pub const DISP_VERTS: usize = 33;
    pub const GAME_LUMP: usize = 35;
    pub const PAKFILE: usize = 40;
    pub const TEXDATA_STRING_DATA: usize = 43;
    pub const TEXDATA_STRING_TABLE: usize = 44;
    pub const LIGHTING_HDR: usize = 53;
    pub const FACES_HDR: usize = 58;
}

/// Texture flags of a face's texture placement.
pub mod surf {
    pub const SKY2D: i32 = 0x2;
    pub const SKY: i32 = 0x4;
    pub const WARP: i32 = 0x8;
    pub const TRANS: i32 = 0x10;
    pub const TRIGGER: i32 = 0x40;
    pub const NODRAW: i32 = 0x80;
    pub const HINT: i32 = 0x100;
    pub const SKIP: i32 = 0x200;
    pub const BUMPLIGHT: i32 = 0x800;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Lump {
    offset: usize,
    len: usize,
    version: i32,
    compressed: bool,
}

/// A map file (module docs).
#[derive(Debug, Clone)]
pub struct Bsp {
    pub version: i32,
    pub revision: i32,
    bytes: Vec<u8>,
    lumps: Vec<Lump>,
}

/// A face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Face {
    pub plane: u16,
    pub side: u8,
    pub first_edge: i32,
    pub edges: i16,
    pub texinfo: i16,
    pub dispinfo: i16,
    pub styles: [u8; 4],
    pub light_offset: i32,
    pub light_mins: [i32; 2],
    pub light_size: [i32; 2],
}

/// A face's texture placement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexInfo {
    pub texture: [[f32; 4]; 2],
    pub lightmap: [[f32; 4]; 2],
    pub flags: i32,
    pub texdata: i32,
}

/// A texture's name and size.
#[derive(Debug, Clone, PartialEq)]
pub struct TexData {
    pub name: String,
    pub reflectivity: [f32; 3],
    pub width: i32,
    pub height: i32,
}

/// A brush model: model 0 the world, the others brush entities' (`"model" "*N"`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Model {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub origin: [f32; 3],
    pub head_node: i32,
    pub first_face: i32,
    pub faces: i32,
}

/// A displacement's description.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DispInfo {
    pub start: [f32; 3],
    pub first_vert: i32,
    pub power: i32,
    pub face: u16,
}

/// A static prop.
#[derive(Debug, Clone, PartialEq)]
pub struct StaticProp {
    pub model: String,
    pub origin: [f32; 3],
    /// Pitch, yaw, roll in degrees.
    pub angles: [f32; 3],
    pub skin: i32,
    /// How it collides: 0 not at all, 2 its bounding box, 6 its physics model (the engine's solid types).
    pub solid: u8,
    /// The leaves it is in.
    pub leaves: Vec<u16>,
}

/// A leaf of the BSP tree: its area and its faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leaf {
    pub contents: i32,
    pub cluster: i16,
    pub area: u16,
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
    pub first_face: u16,
    pub faces: u16,
}

/// A game lump: its four-letter id, its version and its bytes.
pub type GameLump<'a> = ([u8; 4], u16, &'a [u8]);

/// An entity: its keys and values in order.
pub type Entity = Vec<(String, String)>;

/// The value of `key` in an entity.
pub fn value<'a>(e: &'a Entity, key: &str) -> Option<&'a str> {
    e.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

/// Three numbers of a value (`"1 2 3"`).
pub fn vec3(s: &str) -> Option<[f32; 3]> {
    let v: Vec<f32> = s
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    match v.as_slice() {
        [a, b, c, ..] => Some([*a, *b, *c]),
        _ => None,
    }
}

/// A drawn vertex of a built map, in the map's units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// The texture coordinates (texels over the texture's size).
    pub uv: [f32; 2],
    /// The lightmap's light at the vertex (linear, 1 = full), or none when the face has no lightmap.
    pub light: Option<[f32; 3]>,
    /// Where the vertex is in the lightmap atlas the map was built with ([`Bsp::build_lit`]), in texels (divide by the atlas's
    /// size for texture coordinates); none without an atlas or when the face's lightmap did not fit.
    pub lightmap: Option<[f32; 2]>,
    /// The displacement's blend (0 the first texture, 1 the second); 0 on a brush face.
    pub alpha: f32,
}

/// One material's triangles of a built map.
#[derive(Debug, Clone, PartialEq)]
pub struct MapSurface {
    /// The material's name (as the texture data names it: `concrete/wall01`, `maps/<map>/...` for a patched one).
    pub material: String,
    /// The face texture flags of its faces (the first face's).
    pub flags: i32,
    /// Whether it is a displacement's (drawn with its blend alpha).
    pub displacement: bool,
    pub vertices: Vec<MapVertex>,
    pub indices: Vec<u32>,
}

impl Bsp {
    /// Reads a map's header and checks every lump's place.
    pub fn parse(b: &[u8]) -> Result<Bsp> {
        cap("the map file's size", b.len() as u64, MAX_FILE_BYTES)?;
        if slice(b, 0, 4, "the map signature")? != b"VBSP" {
            return invalid("not a VBSP map (bad signature)");
        }
        let version = i32_at(b, 4, "the map version")?;
        if !(19..=21).contains(&version) {
            return invalid(format!("VBSP version {version} is not read"));
        }
        let mut lumps = Vec::with_capacity(64);
        for i in 0..64 {
            let at = 8 + i * 16;
            let offset = i32_at(b, at, "the lump table")?;
            let len = i32_at(b, at + 4, "the lump table")?;
            let version = i32_at(b, at + 8, "the lump table")?;
            let ucs = i32_at(b, at + 12, "the lump table")?;
            let (Ok(offset), Ok(len)) = (usize::try_from(offset), usize::try_from(len)) else {
                return invalid(format!("lump {i} has a negative place"));
            };
            if offset.checked_add(len).is_none_or(|e| e > b.len()) {
                return Err(Error::Truncated { what: "a lump" });
            }
            lumps.push(Lump {
                offset,
                len,
                version,
                compressed: ucs != 0 && i != lump::GAME_LUMP,
            });
        }
        let revision = i32_at(b, 8 + 64 * 16, "the map revision")?;
        Ok(Bsp {
            version,
            revision,
            bytes: b.to_vec(),
            lumps,
        })
    }

    /// A lump's bytes (an error for a compressed one).
    pub fn lump(&self, i: usize) -> Result<&[u8]> {
        let l = self
            .lumps
            .get(i)
            .ok_or(Error::Invalid(format!("no lump {i}")))?;
        if l.compressed && l.len > 0 {
            return invalid(format!("lump {i} is compressed (not read)"));
        }
        slice(&self.bytes, l.offset, l.len, "a lump")
    }

    /// A lump's version.
    pub fn lump_version(&self, i: usize) -> i32 {
        self.lumps.get(i).map_or(0, |l| l.version)
    }

    /// A lump read as records of `size` bytes each.
    fn records<T>(
        &self,
        i: usize,
        size: usize,
        read: impl Fn(&[u8], usize) -> Result<T>,
    ) -> Result<Vec<T>> {
        let b = self.lump(i)?;
        let n = b.len() / size;
        (0..n).map(|k| read(b, k * size)).collect()
    }

    pub fn vertices(&self) -> Result<Vec<[f32; 3]>> {
        self.records(lump::VERTEXES, 12, |b, at| vec3_at(b, at, "a vertex"))
    }

    pub fn edges(&self) -> Result<Vec<[u16; 2]>> {
        self.records(lump::EDGES, 4, |b, at| {
            Ok([u16_at(b, at, "an edge")?, u16_at(b, at + 2, "an edge")?])
        })
    }

    pub fn surfedges(&self) -> Result<Vec<i32>> {
        self.records(lump::SURFEDGES, 4, |b, at| i32_at(b, at, "a surfedge"))
    }

    pub fn planes(&self) -> Result<Vec<([f32; 3], f32)>> {
        self.records(lump::PLANES, 20, |b, at| {
            Ok((vec3_at(b, at, "a plane")?, f32_at(b, at + 12, "a plane")?))
        })
    }

    /// The faces (the LDR list, or the HDR one when a map has only that).
    pub fn faces(&self) -> Result<Vec<Face>> {
        let i = if self.lump(lump::FACES)?.is_empty() {
            lump::FACES_HDR
        } else {
            lump::FACES
        };
        self.records(i, 56, |b, at| {
            Ok(Face {
                plane: u16_at(b, at, "a face")?,
                side: u8_at(b, at + 2, "a face")?,
                first_edge: i32_at(b, at + 4, "a face")?,
                edges: i16_at(b, at + 8, "a face")?,
                texinfo: i16_at(b, at + 10, "a face")?,
                dispinfo: i16_at(b, at + 12, "a face")?,
                styles: [
                    u8_at(b, at + 16, "a face")?,
                    u8_at(b, at + 17, "a face")?,
                    u8_at(b, at + 18, "a face")?,
                    u8_at(b, at + 19, "a face")?,
                ],
                light_offset: i32_at(b, at + 20, "a face")?,
                light_mins: [i32_at(b, at + 28, "a face")?, i32_at(b, at + 32, "a face")?],
                light_size: [i32_at(b, at + 36, "a face")?, i32_at(b, at + 40, "a face")?],
            })
        })
    }

    pub fn texinfo(&self) -> Result<Vec<TexInfo>> {
        self.records(lump::TEXINFO, 72, |b, at| {
            let v = |o: usize| -> Result<[f32; 4]> {
                Ok([
                    f32_at(b, at + o, "a texinfo")?,
                    f32_at(b, at + o + 4, "a texinfo")?,
                    f32_at(b, at + o + 8, "a texinfo")?,
                    f32_at(b, at + o + 12, "a texinfo")?,
                ])
            };
            Ok(TexInfo {
                texture: [v(0)?, v(16)?],
                lightmap: [v(32)?, v(48)?],
                flags: i32_at(b, at + 64, "a texinfo")?,
                texdata: i32_at(b, at + 68, "a texinfo")?,
            })
        })
    }

    /// The texture data, with their names from the string table.
    pub fn texdata(&self) -> Result<Vec<TexData>> {
        let strings = self.lump(lump::TEXDATA_STRING_DATA)?;
        let table = self.lump(lump::TEXDATA_STRING_TABLE)?;
        self.records(lump::TEXDATA, 32, |b, at| {
            let id = usize::try_from(i32_at(b, at + 12, "a texdata")?).unwrap_or(usize::MAX);
            let name = match id
                .checked_mul(4)
                .map(|o| i32_at(table, o, "the texdata string table"))
            {
                Some(Ok(o)) => crate::read::cstr_at(
                    strings,
                    usize::try_from(o).unwrap_or(usize::MAX),
                    256,
                    "a texture name",
                )
                .unwrap_or_default(),
                _ => String::new(),
            };
            Ok(TexData {
                name,
                reflectivity: vec3_at(b, at, "a texdata")?,
                width: i32_at(b, at + 16, "a texdata")?,
                height: i32_at(b, at + 20, "a texdata")?,
            })
        })
    }

    pub fn models(&self) -> Result<Vec<Model>> {
        self.records(lump::MODELS, 48, |b, at| {
            Ok(Model {
                mins: vec3_at(b, at, "a model")?,
                maxs: vec3_at(b, at + 12, "a model")?,
                origin: vec3_at(b, at + 24, "a model")?,
                head_node: i32_at(b, at + 36, "a model")?,
                first_face: i32_at(b, at + 40, "a model")?,
                faces: i32_at(b, at + 44, "a model")?,
            })
        })
    }

    pub fn dispinfo(&self) -> Result<Vec<DispInfo>> {
        self.records(lump::DISPINFO, 176, |b, at| {
            Ok(DispInfo {
                start: vec3_at(b, at, "a displacement")?,
                first_vert: i32_at(b, at + 12, "a displacement")?,
                power: i32_at(b, at + 20, "a displacement")?,
                face: u16_at(b, at + 36, "a displacement")?,
            })
        })
    }

    /// The displacement vertices: direction, distance, alpha.
    pub fn disp_verts(&self) -> Result<Vec<([f32; 3], f32, f32)>> {
        self.records(lump::DISP_VERTS, 20, |b, at| {
            Ok((
                vec3_at(b, at, "a displacement vertex")?,
                f32_at(b, at + 12, "a displacement vertex")?,
                f32_at(b, at + 16, "a displacement vertex")?,
            ))
        })
    }

    /// The leaves (32 bytes each; 56 in maps whose leaf lump is version 0).
    pub fn leaves(&self) -> Result<Vec<Leaf>> {
        let size = if self.lump_version(lump::LEAFS) == 0 && self.version < 20 {
            56
        } else {
            32
        };
        self.records(lump::LEAFS, size, |b, at| {
            let area_flags = u16_at(b, at + 6, "a leaf")?;
            Ok(Leaf {
                contents: i32_at(b, at, "a leaf")?,
                cluster: i16_at(b, at + 4, "a leaf")?,
                area: area_flags & 0x1ff,
                mins: [
                    i16_at(b, at + 8, "a leaf")?,
                    i16_at(b, at + 10, "a leaf")?,
                    i16_at(b, at + 12, "a leaf")?,
                ],
                maxs: [
                    i16_at(b, at + 14, "a leaf")?,
                    i16_at(b, at + 16, "a leaf")?,
                    i16_at(b, at + 18, "a leaf")?,
                ],
                first_face: u16_at(b, at + 20, "a leaf")?,
                faces: u16_at(b, at + 22, "a leaf")?,
            })
        })
    }

    pub fn leaf_faces(&self) -> Result<Vec<u16>> {
        self.records(lump::LEAFFACES, 2, |b, at| u16_at(b, at, "a leaf face"))
    }

    /// The nodes: plane and children (a negative child `-(leaf + 1)`).
    pub fn nodes(&self) -> Result<Vec<(i32, [i32; 2])>> {
        self.records(lump::NODES, 32, |b, at| {
            Ok((
                i32_at(b, at, "a node")?,
                [i32_at(b, at + 4, "a node")?, i32_at(b, at + 8, "a node")?],
            ))
        })
    }

    /// The leaf a point is in (walking the world's tree), if the tree reaches one.
    pub fn leaf_at(&self, p: [f32; 3]) -> Result<Option<usize>> {
        let Some(world) = self.models()?.first().copied() else {
            return Ok(None);
        };
        let nodes = self.nodes()?;
        let mut steps = MAX_TREE_DEPTH.min(nodes.len() + 1);
        Ok(leaf_in(
            &nodes,
            &self.planes()?,
            world.head_node,
            p,
            &mut steps,
        ))
    }

    /// The entities.
    pub fn entities(&self) -> Result<Vec<Entity>> {
        let text = self.lump(lump::ENTITIES)?;
        let text = text.strip_suffix(&[0]).unwrap_or(text);
        Ok(kv::parse(text)?
            .into_iter()
            .filter_map(|p| match p.value {
                kv::Value::Block(items) => Some(
                    items
                        .into_iter()
                        .filter_map(|q| match q.value {
                            kv::Value::Str(v) => Some((q.key, v)),
                            kv::Value::Block(_) => None,
                        })
                        .collect(),
                ),
                kv::Value::Str(_) => None,
            })
            .collect())
    }

    /// The game lumps: id, version and bytes.
    pub fn game_lumps(&self) -> Result<Vec<GameLump<'_>>> {
        let b = self.lump(lump::GAME_LUMP)?;
        if b.is_empty() {
            return Ok(Vec::new());
        }
        let n = crate::read::count(
            i32_at(b, 0, "the game lumps")?,
            16,
            b.len().saturating_sub(4),
            "the game lumps",
        )?;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let at = 4 + i * 16;
            let id = u32_at(b, at, "a game lump")?.to_be_bytes();
            let version = u16_at(b, at + 6, "a game lump")?;
            let off = usize::try_from(i32_at(b, at + 8, "a game lump")?).unwrap_or(usize::MAX);
            let len = usize::try_from(i32_at(b, at + 12, "a game lump")?).unwrap_or(usize::MAX);
            // Offsets are from the start of the file.
            out.push((id, version, slice(&self.bytes, off, len, "a game lump")?));
        }
        Ok(out)
    }

    /// The static props (`sprp`, any version: the fields read are in every version's first 36 bytes).
    pub fn static_props(&self) -> Result<Vec<StaticProp>> {
        let Some(&(_, version, b)) = self.game_lumps()?.iter().find(|g| &g.0 == b"sprp") else {
            return Ok(Vec::new());
        };
        if !(4..=13).contains(&version) {
            return invalid(format!("static prop lump version {version} is not read"));
        }
        let mut at = 0usize;
        let names = crate::read::count(
            i32_at(b, at, "the prop names")?,
            128,
            b.len(),
            "the prop names",
        )?;
        at += 4;
        let mut dict = Vec::with_capacity(names);
        for i in 0..names {
            dict.push(crate::read::fixed_str(b, at + i * 128, 128, "a prop name")?);
        }
        at += names * 128;
        let leaves = crate::read::count(
            i32_at(b, at, "the prop leaves")?,
            2,
            b.len().saturating_sub(at + 4),
            "the prop leaves",
        )?;
        at += 4;
        let leaf_list: Vec<u16> = (0..leaves)
            .map(|i| u16_at(b, at + i * 2, "a prop leaf"))
            .collect::<Result<_>>()?;
        at += leaves * 2;
        let n = i32_at(b, at, "the props")?;
        at += 4;
        if n <= 0 {
            return Ok(Vec::new());
        }
        let n = usize::try_from(n).unwrap_or(0);
        let stride = (b.len() - at.min(b.len())) / n;
        if stride < 36 {
            return Err(Error::Truncated {
                what: "the static props",
            });
        }
        let mut props = Vec::with_capacity(n);
        // The props' leaf lists are runs of the one list: copies are paid from its length (twice, and a little more), so props
        // that all name the whole list cannot copy it over and over.
        let mut copies = leaf_list.len().saturating_mul(2).saturating_add(1024);
        for i in 0..n {
            let p = at + i * stride;
            let ty = usize::from(u16_at(b, p + 24, "a prop")?);
            let first = usize::from(u16_at(b, p + 26, "a prop")?);
            let count = usize::from(u16_at(b, p + 28, "a prop")?);
            let count = count.min(leaf_list.len().saturating_sub(first));
            copies = copies.checked_sub(count).ok_or(Error::TooLarge {
                what: "the static props' leaf lists",
                asked: count as u64,
                cap: copies as u64,
            })?;
            props.push(StaticProp {
                model: dict.get(ty).cloned().unwrap_or_default(),
                origin: vec3_at(b, p, "a prop")?,
                angles: vec3_at(b, p + 12, "a prop")?,
                skin: i32_at(b, p + 32, "a prop")?,
                solid: crate::read::u8_at(b, p + 30, "a prop")?,
                leaves: leaf_list
                    .get(first..first + count)
                    .map(<[u16]>::to_vec)
                    .unwrap_or_default(),
            });
        }
        Ok(props)
    }

    /// The pak lump's zip (the map's own materials, cube maps and models), if it has one.
    pub fn pak(&self) -> Result<Option<ZipMount>> {
        let b = self.lump(lump::PAKFILE)?;
        if b.is_empty() {
            return Ok(None);
        }
        ZipMount::open(b.to_vec())
            .map(Some)
            .map_err(|e| Error::Invalid(format!("the pak lump: {e:?}")))
    }

    /// The faces of the world (model 0).
    pub fn world_faces(&self) -> Result<Vec<usize>> {
        let models = self.models()?;
        let Some(m) = models.first() else {
            return Ok(Vec::new());
        };
        self.model_faces(m)
    }

    /// The faces of a brush model (its range clamped to the faces there are).
    pub fn model_faces(&self, m: &Model) -> Result<Vec<usize>> {
        let all = self.faces()?.len();
        let first = usize::try_from(m.first_face).unwrap_or(0).min(all);
        let n = usize::try_from(m.faces).unwrap_or(0);
        Ok((first..first.saturating_add(n).min(all)).collect())
    }

    /// The areas of the faces: each face's area from the leaves that hold it (faces in no leaf: none).
    pub fn face_areas(&self) -> Result<Vec<Option<u16>>> {
        let faces = self.faces()?.len();
        let leaf_faces = self.leaf_faces()?;
        let mut out = vec![None; faces];
        // Leaves whose ranges overlap would look at the same entries again: the looks are capped near the list's length.
        let mut looks = leaf_faces.len().saturating_mul(4).saturating_add(1024);
        for leaf in self.leaves()? {
            let first = usize::from(leaf.first_face).min(leaf_faces.len());
            let last = (first + usize::from(leaf.faces)).min(leaf_faces.len());
            looks = looks.checked_sub(last - first).ok_or(Error::Invalid(
                "the leaves' face ranges overlap too much".into(),
            ))?;
            for &f in &leaf_faces[first..last] {
                if let Some(slot) = out.get_mut(usize::from(f)) {
                    *slot = Some(leaf.area);
                }
            }
        }
        // Faces no leaf lists (displacements): the area of the leaf at their middle.
        if out.iter().any(Option::is_none) {
            let all = self.faces()?;
            let (nodes, planes) = (self.nodes()?, self.planes()?);
            let (verts, edges, surfedges) = (self.vertices()?, self.edges()?, self.surfedges()?);
            let leaves = self.leaves()?;
            let head = self.models()?.first().map_or(-1, |m| m.head_node);
            // Every edge looked at and every node walked is paid from one budget tied to the lumps' sizes: faces whose edge
            // ranges overlap, or a deep or degenerate tree, cannot make the work grow past it.
            let lumps = [lump::FACES, lump::FACES_HDR, lump::NODES, lump::SURFEDGES]
                .iter()
                .map(|&l| self.lumps.get(l).map_or(0, |l| l.len))
                .sum::<usize>();
            let mut steps = lumps.saturating_mul(4).saturating_add(1 << 16);
            for (i, slot) in out.iter_mut().enumerate() {
                if slot.is_some() {
                    continue;
                }
                let Some(f) = all.get(i) else { continue };
                let edges_here = usize::try_from(f.edges).unwrap_or(0).min(MAX_FACE_EDGES);
                steps = steps.checked_sub(edges_here).ok_or(Error::TooLarge {
                    what: "the work of finding the faces' leaves",
                    asked: edges_here as u64,
                    cap: steps as u64,
                })?;
                let mut sum = [0f32; 3];
                let mut n = 0f32;
                for k in 0..usize::try_from(f.edges).unwrap_or(0).min(MAX_FACE_EDGES) {
                    let Some(se) = usize::try_from(f.first_edge)
                        .ok()
                        .and_then(|e| surfedges.get(e + k))
                    else {
                        break;
                    };
                    let Some(e) = edges.get(se.unsigned_abs() as usize) else {
                        break;
                    };
                    let Some(v) = verts.get(usize::from(if *se >= 0 { e[0] } else { e[1] })) else {
                        break;
                    };
                    sum = [sum[0] + v[0], sum[1] + v[1], sum[2] + v[2]];
                    n += 1.0;
                }
                if n > 0.0 {
                    let c = sum.map(|x| x / n);
                    let before = steps;
                    let leaf = leaf_in(&nodes, &planes, head, c, &mut steps);
                    if leaf.is_none() && steps == 0 && before > 0 {
                        return Err(Error::TooLarge {
                            what: "the work of finding the faces' leaves",
                            asked: before as u64 + 1,
                            cap: before as u64,
                        });
                    }
                    *slot = leaf.and_then(|l| leaves.get(l)).map(|l| l.area);
                }
            }
        }
        Ok(out)
    }

    /// The light of a lightmap sample at `index` (in 4-byte samples from the lighting lump's start).
    fn sample(light: &[u8], index: usize) -> Option<[f32; 3]> {
        let at = index.checked_mul(4)?;
        let s = light.get(at..at.checked_add(4)?)?;
        let scale = 2f32.powi(i32::from(s[3] as i8)) / 255.0;
        Some([
            f32::from(s[0]) * scale,
            f32::from(s[1]) * scale,
            f32::from(s[2]) * scale,
        ])
    }

    /// The drawn surfaces of the given faces (module docs): every face whose texture is drawn (not sky, a tool or a trigger),
    /// displacements as their grids; each vertex moved by `offset`. `keep(face)` picks the faces.
    pub fn build(&self, faces: &[usize], offset: [f32; 3]) -> Result<Vec<MapSurface>> {
        self.build_lit(faces, offset, None)
    }

    /// [`Bsp::build`] with each vertex's place in `atlas` ([`Bsp::lightmap_atlas`] of these faces) where its face is there.
    pub fn build_lit(
        &self,
        faces: &[usize],
        offset: [f32; 3],
        atlas: Option<&Atlas>,
    ) -> Result<Vec<MapSurface>> {
        let verts = self.vertices()?;
        let edges = self.edges()?;
        let surfedges = self.surfedges()?;
        let all_faces = self.faces()?;
        let texinfo = self.texinfo()?;
        let texdata = self.texdata()?;
        let dispinfo = self.dispinfo()?;
        let disp_verts = self.disp_verts()?;
        let planes = self.planes()?;
        let light = self.lump(lump::LIGHTING).ok().filter(|l| !l.is_empty());
        let light = match light {
            Some(l) => Some(l),
            None => self.lump(lump::LIGHTING_HDR).ok().filter(|l| !l.is_empty()),
        };
        let mut by_material: BTreeMap<(String, bool), MapSurface> = BTreeMap::new();
        let mut total = 0usize;
        for &fi in faces {
            let Some(f) = all_faces.get(fi) else { continue };
            let Some(ti) = usize::try_from(f.texinfo).ok().and_then(|t| texinfo.get(t)) else {
                continue;
            };
            if ti.flags
                & (surf::SKY | surf::SKY2D | surf::NODRAW | surf::TRIGGER | surf::HINT | surf::SKIP)
                != 0
            {
                continue;
            }
            let Some(td) = usize::try_from(ti.texdata)
                .ok()
                .and_then(|t| texdata.get(t))
            else {
                continue;
            };
            let n = usize::try_from(f.edges).unwrap_or(0);
            if !(3..=MAX_FACE_EDGES).contains(&n) {
                continue;
            }
            // Its corners are paid for before they are read (a displacement's grid after them).
            total += n;
            cap(
                "the map's built vertices",
                total as u64,
                MAX_BUILT_VERTICES as u64,
            )?;
            let mut poly = Vec::with_capacity(n);
            for k in 0..n {
                let se = usize::try_from(f.first_edge)
                    .ok()
                    .and_then(|first| first.checked_add(k))
                    .and_then(|i| surfedges.get(i))
                    .ok_or(Error::Invalid("a face's edge past the list".into()))?;
                let e = edges
                    .get(se.unsigned_abs() as usize)
                    .ok_or(Error::Invalid("a surfedge past the edges".into()))?;
                let v = if *se >= 0 { e[0] } else { e[1] };
                poly.push(
                    *verts
                        .get(usize::from(v))
                        .ok_or(Error::Invalid("an edge past the vertices".into()))?,
                );
            }
            let (tw, th) = (td.width.max(1) as f32, td.height.max(1) as f32);
            let uv = |p: [f32; 3]| -> [f32; 2] {
                let s = &ti.texture;
                [
                    (s[0][0] * p[0] + s[0][1] * p[1] + s[0][2] * p[2] + s[0][3]) / tw,
                    (s[1][0] * p[0] + s[1][1] * p[1] + s[1][2] * p[2] + s[1][3]) / th,
                ]
            };
            let lit = |p: [f32; 3]| -> Option<[f32; 3]> {
                let l = light?;
                if f.light_offset < 0 {
                    return None;
                }
                let m = &ti.lightmap;
                let w = usize::try_from(f.light_size[0]).ok()? + 1;
                let h = usize::try_from(f.light_size[1]).ok()? + 1;
                let s = m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3]
                    - f.light_mins[0] as f32;
                let t = m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3]
                    - f.light_mins[1] as f32;
                let s = (s.round().max(0.0) as usize).min(w - 1);
                let t = (t.round().max(0.0) as usize).min(h - 1);
                Bsp::sample(l, f.light_offset as usize / 4 + t * w + s)
            };
            // A displacement's lightmap is laid over its grid, not projected (the public SDK's `CalcLuxelCoords`: from the start
            // corner, s along the grid's columns to the face's size, t along its rows): luxel (s, t) of grid vertex (row, col).
            let disp_luxel = |row: usize, col: usize, side: usize| -> [f32; 2] {
                let span = (side - 1).max(1) as f32;
                [
                    f.light_size[0].max(0) as f32 * col as f32 / span,
                    f.light_size[1].max(0) as f32 * row as f32 / span,
                ]
            };
            let lit_luxel = |st: [f32; 2]| -> Option<[f32; 3]> {
                let l = light?;
                if f.light_offset < 0 {
                    return None;
                }
                let w = usize::try_from(f.light_size[0]).ok()? + 1;
                let h = usize::try_from(f.light_size[1]).ok()? + 1;
                let s = (st[0].round().max(0.0) as usize).min(w - 1);
                let t = (st[1].round().max(0.0) as usize).min(h - 1);
                Bsp::sample(l, f.light_offset as usize / 4 + t * w + s)
            };
            // The vertex's texel in the atlas: the face's block, the luxel coordinates from the lightmap vectors less the mins,
            // at the texel's center (clamped into the block's padding).
            let placed = atlas.and_then(|a| a.places.get(&fi).copied());
            let atlas_luxel = |st: [f32; 2]| -> Option<[f32; 2]> {
                let (x0, y0) = placed?;
                let s = st[0].clamp(-0.5, f.light_size[0] as f32 + 0.5);
                let t = st[1].clamp(-0.5, f.light_size[1] as f32 + 0.5);
                Some([x0 as f32 + s + 0.5, y0 as f32 + t + 0.5])
            };
            let atlas_at = |p: [f32; 3]| -> Option<[f32; 2]> {
                let m = &ti.lightmap;
                atlas_luxel([
                    m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3]
                        - f.light_mins[0] as f32,
                    m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3]
                        - f.light_mins[1] as f32,
                ])
            };
            let is_disp = f.dispinfo >= 0;
            let key = (td.name.to_ascii_lowercase(), is_disp);
            let surface = by_material.entry(key).or_insert_with(|| MapSurface {
                material: td.name.clone(),
                flags: ti.flags,
                displacement: is_disp,
                vertices: Vec::new(),
                indices: Vec::new(),
            });
            let base = surface.vertices.len() as u32;
            let at = |p: [f32; 3]| [p[0] + offset[0], p[1] + offset[1], p[2] + offset[2]];
            if is_disp {
                let Some(d) = dispinfo.get(f.dispinfo as usize) else {
                    continue;
                };
                if n != 4 || !(1..=MAX_DISP_POWER).contains(&d.power) {
                    continue;
                }
                let side = (1usize << d.power) + 1;
                let first = usize::try_from(d.first_vert).unwrap_or(usize::MAX);
                let Some(dv) = first
                    .checked_add(side * side)
                    .and_then(|end| disp_verts.get(first..end))
                else {
                    continue;
                };
                total += side * side;
                cap(
                    "the map's built vertices",
                    total as u64,
                    MAX_BUILT_VERTICES as u64,
                )?;
                // The corner nearest the start position is the grid's first.
                let start = (0..4)
                    .min_by(|&a, &b| dist2(poly[a], d.start).total_cmp(&dist2(poly[b], d.start)))
                    .unwrap_or(0);
                let c = [0, 1, 2, 3].map(|k| poly[(start + k) % 4]);
                let mut grid = Vec::with_capacity(side * side);
                let span = (side - 1) as f32;
                for i in 0..side {
                    let a = lerp(c[0], c[1], i as f32 / span);
                    let b = lerp(c[3], c[2], i as f32 / span);
                    for j in 0..side {
                        let flat = lerp(a, b, j as f32 / span);
                        let (dir, dist, alpha) = dv[i * side + j];
                        let p = [
                            flat[0] + dir[0] * dist,
                            flat[1] + dir[1] * dist,
                            flat[2] + dir[2] * dist,
                        ];
                        grid.push((p, flat, alpha));
                    }
                }
                for i in 0..side {
                    for j in 0..side {
                        let (p, flat, alpha) = grid[i * side + j];
                        let g =
                            |a: usize, b: usize| grid[a.min(side - 1) * side + b.min(side - 1)].0;
                        let di = sub(g(i + 1, j), g(i.saturating_sub(1), j));
                        let dj = sub(g(i, j + 1), g(i, j.saturating_sub(1)));
                        let nrm = normalize(cross(dj, di));
                        surface.vertices.push(MapVertex {
                            position: at(p),
                            normal: nrm,
                            uv: uv(flat),
                            light: lit_luxel(disp_luxel(i, j, side)),
                            lightmap: atlas_luxel(disp_luxel(i, j, side)),
                            alpha: (alpha / 255.0).clamp(0.0, 1.0),
                        });
                    }
                }
                for i in 0..side - 1 {
                    for j in 0..side - 1 {
                        let q = |a: usize, b: usize| base + (a * side + b) as u32;
                        surface
                            .indices
                            .extend_from_slice(&[q(i, j), q(i + 1, j), q(i + 1, j + 1)]);
                        surface
                            .indices
                            .extend_from_slice(&[q(i, j), q(i + 1, j + 1), q(i, j + 1)]);
                    }
                }
            } else {
                let nrm = match planes.get(usize::from(f.plane)) {
                    Some(&(n, _)) if f.side == 0 => n,
                    Some(&(n, _)) => [-n[0], -n[1], -n[2]],
                    None => normalize(cross(sub(poly[2], poly[0]), sub(poly[1], poly[0]))),
                };
                for &p in &poly {
                    surface.vertices.push(MapVertex {
                        position: at(p),
                        normal: nrm,
                        uv: uv(p),
                        light: lit(p),
                        lightmap: atlas_at(p),
                        alpha: 0.0,
                    });
                }
                for k in 1..n as u32 - 1 {
                    surface
                        .indices
                        .extend_from_slice(&[base, base + k, base + k + 1]);
                }
            }
        }
        Ok(by_material
            .into_values()
            .filter(|s| !s.indices.is_empty())
            .collect())
    }
}

/// The widest atlas side.
pub const MAX_ATLAS_SIDE: u32 = 4096;
/// The most texels one map's lightmap atlas holds (its texture budget: 16 Mi texels, 4096 x 4096).
pub const MAX_ATLAS_TEXELS: usize = 1 << 24;
/// The largest lightmap of one face read (luxels a side; the format's own faces stay far below).
const MAX_FACE_LUXELS: i32 = 1024;
/// The texel (at its center) of the atlas's fully lit block: light 1.
pub const FULL_LIGHT_TEXEL: [f32; 2] = [2.0, 2.0];
/// The texel of the atlas's block of light [`GAIN_LIGHT`]: a surface lit by its vertex colors takes it, its colors the light over
/// that gain (colors stop at 1; light goes past it).
pub const GAIN_TEXEL: [f32; 2] = [6.0, 2.0];
pub const GAIN_LIGHT: f32 = 4.0;

/// A map's lightmaps packed in one texture (linear light, 1 = full; `width` x `height` texels, rows top down), and where each
/// face's lightmap is: its block's first sample (the block has one texel of padding around it, a copy of its edge, so bilinear
/// sampling never reads a neighbor). The first two 4 x 4 blocks are reserved: light 1 ([`FULL_LIGHT_TEXEL`]) and
/// [`GAIN_LIGHT`] ([`GAIN_TEXEL`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Atlas {
    pub width: u32,
    pub height: u32,
    pub texels: Vec<[f32; 3]>,
    places: std::collections::HashMap<usize, (u32, u32)>,
    /// Faces whose lightmap did not fit (past [`MAX_ATLAS_TEXELS`]).
    pub left_out: usize,
}

impl Atlas {
    /// Whether face `face` has a block.
    pub fn has(&self, face: usize) -> bool {
        self.places.contains_key(&face)
    }
}

/// A light cube: the light arriving from +x, -x, +y, -y, +z and -z (linear).
pub type LightCube = [[f32; 3]; 6];

/// A light the map compiler kept for lighting models at run time (its direct light is not in the ambient cubes, which hold the
/// light bounced off surfaces and the sky's ambient light).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldLight {
    /// 0 surface, 1 point, 2 spot, 3 sky (a direction), 4 point with the old falloff, 5 the sky's ambient light.
    pub kind: i32,
    pub origin: [f32; 3],
    /// Linear light, the lightmaps' scale.
    pub intensity: [f32; 3],
    /// A spot's or the sky's direction (the way its light goes).
    pub normal: [f32; 3],
    pub stop_dot: f32,
    pub stop_dot2: f32,
    pub radius: f32,
    /// Falloff: 1 / (constant + linear d + quadratic d^2).
    pub attenuation: [f32; 3],
}

/// The most world lights read.
pub const MAX_WORLD_LIGHTS: usize = 8192;
/// The longest march a visibility test takes, in steps of [`TRACE_STEP`] units.
const MAX_TRACE_STEPS: usize = 4096;
const TRACE_STEP: f32 = 12.0;

/// The deepest tree walk one visibility sample takes.
pub const MAX_TREE_DEPTH: usize = 1024;
/// The most visibility answers a tracer keeps.
pub const MAX_SEEN: usize = 1 << 20;
/// The tree nodes all of one map's lighting traces may visit together (past it the traces stop: ambient light only).
pub const MAX_TRACE_WORK: u64 = 1 << 26;

/// A map's tree, kept for visibility tests: points marched along a line, each looked up in the tree. Every node visited is
/// paid from one budget for the whole map ([`MAX_TRACE_WORK`]); once it is spent every test answers "not seen".
#[derive(Debug)]
pub struct Tracer {
    nodes: Vec<(i32, [i32; 2])>,
    planes: Vec<([f32; 3], f32)>,
    contents: Vec<i32>,
    head: i32,
    mins: [f32; 3],
    maxs: [f32; 3],
    work: std::cell::Cell<u64>,
    /// Visibility already found: (light, the leaf of the point it was asked from).
    seen: std::cell::RefCell<std::collections::HashMap<(usize, usize), bool>>,
    /// The most answers kept ([`MAX_SEEN`]).
    max_seen: usize,
}

impl Tracer {
    /// Whether the budget is spent.
    pub fn exhausted(&self) -> bool {
        self.work.get() == 0
    }

    /// The leaf at `p`, paid for (none outside the tree, past the depth cap, or with the budget spent).
    fn leaf(&self, p: [f32; 3]) -> Option<usize> {
        let mut steps = MAX_TREE_DEPTH.min(self.nodes.len() + 1);
        let before = steps;
        let leaf = leaf_in(&self.nodes, &self.planes, self.head, p, &mut steps);
        let used = (before - steps) as u64 + 1;
        let left = self.work.get();
        self.work.set(left.saturating_sub(used));
        if left < used { None } else { leaf }
    }

    /// Whether `p` is in a solid leaf (or outside the tree, or the budget is spent).
    pub fn solid(&self, p: [f32; 3]) -> bool {
        match self.leaf(p) {
            Some(l) => self.contents.get(l).is_none_or(|c| c & 1 != 0),
            None => true,
        }
    }

    /// Whether nothing solid lies between `a` and `b` (sampled every few units).
    pub fn clear(&self, a: [f32; 3], b: [f32; 3]) -> bool {
        let d = sub(b, a);
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if !len.is_finite() {
            return false;
        }
        let steps = ((len / TRACE_STEP) as usize).min(MAX_TRACE_STEPS);
        (1..steps).all(|k| !self.exhausted() && !self.solid(lerp(a, b, k as f32 / steps as f32)))
    }

    /// Whether the sky is seen from `p` along `dir` (unit): the march meets solid space once and stays in it out to the world's
    /// bounds (the outer shell, where a map's sky brushes are). A solid that the march leaves again (a roof with air above it)
    /// blocks the sky. An approximation of the compiler's test (it asks whether the face the ray meets is sky).
    pub fn sky_seen(&self, p: [f32; 3], dir: [f32; 3]) -> bool {
        let outside = |q: [f32; 3]| (0..3).any(|k| q[k] < self.mins[k] || q[k] > self.maxs[k]);
        let mut in_solid = false;
        for k in 1..MAX_TRACE_STEPS {
            if self.exhausted() {
                return false;
            }
            let q = [0, 1, 2].map(|a| p[a] + dir[a] * TRACE_STEP * k as f32);
            if outside(q) {
                return true;
            }
            let solid = self.solid(q);
            if in_solid && !solid {
                return false;
            }
            in_solid |= solid;
        }
        false
    }

    /// `test` for light `light` seen from `p`, kept per (light, leaf of `p`): points in one leaf share the answer.
    fn cached(&self, light: usize, p: [f32; 3], test: impl FnOnce() -> bool) -> bool {
        let Some(leaf) = self.leaf(p) else {
            return false;
        };
        if let Some(&v) = self.seen.borrow().get(&(light, leaf)) {
            return v;
        }
        let v = test();
        // At most MAX_SEEN answers kept; past it each is found again (still paid from the budget).
        let mut seen = self.seen.borrow_mut();
        if seen.len() < self.max_seen {
            seen.insert((light, leaf), v);
        }
        v
    }
}

impl Bsp {
    /// The world lights (the LDR list, else the HDR one).
    pub fn world_lights(&self) -> Result<Vec<WorldLight>> {
        let i = if self.lump(lump::WORLDLIGHTS)?.is_empty() {
            lump::WORLDLIGHTS_HDR
        } else {
            lump::WORLDLIGHTS
        };
        let b = self.lump(i)?;
        if b.len() % 88 != 0 {
            return Ok(Vec::new());
        }
        cap(
            "the world lights",
            (b.len() / 88) as u64,
            MAX_WORLD_LIGHTS as u64,
        )?;
        self.records(i, 88, |b, at| {
            Ok(WorldLight {
                origin: vec3_at(b, at, "a world light")?,
                intensity: vec3_at(b, at + 12, "a world light")?,
                normal: vec3_at(b, at + 24, "a world light")?,
                kind: i32_at(b, at + 40, "a world light")?,
                stop_dot: f32_at(b, at + 48, "a world light")?,
                stop_dot2: f32_at(b, at + 52, "a world light")?,
                radius: f32_at(b, at + 60, "a world light")?,
                attenuation: vec3_at(b, at + 64, "a world light")?,
            })
        })
    }

    /// The tree and the world's bounds, for visibility tests.
    pub fn tracer(&self) -> Result<Tracer> {
        let world = self.models()?.first().copied();
        Ok(Tracer {
            nodes: self.nodes()?,
            planes: self.planes()?,
            contents: self.leaves()?.iter().map(|l| l.contents).collect(),
            head: world.map_or(-1, |m| m.head_node),
            mins: world.map_or([0.0; 3], |m| m.mins),
            maxs: world.map_or([0.0; 3], |m| m.maxs),
            work: std::cell::Cell::new(MAX_TRACE_WORK),
            seen: std::cell::RefCell::new(std::collections::HashMap::new()),
            max_seen: MAX_SEEN,
        })
    }
}

/// The direct lights that reach `p`: each its direction towards the light (unit) and its light there (before the surface's
/// facing), the sky's light where the sky is seen along it, the points' and spots' within their reach and in sight, as the
/// map compiler leaves them for models; lights under `min`, and lights with a field that is not finite or a direction of no
/// length, are left out. Each light's visibility is found once a leaf ([`Tracer`]); none once the tracer's budget is spent.
pub fn direct_lights(
    tracer: &Tracer,
    lights: &[WorldLight],
    p: [f32; 3],
    min: f32,
) -> Vec<([f32; 3], [f32; 3])> {
    let mut out = Vec::new();
    if p.iter().any(|v| !v.is_finite()) {
        return out;
    }
    for (index, l) in lights.iter().enumerate() {
        if tracer.exhausted() {
            break;
        }
        let fields = [l.origin, l.intensity, l.normal, l.attenuation];
        if fields.iter().flatten().any(|v| !v.is_finite())
            || ![l.stop_dot, l.stop_dot2, l.radius]
                .iter()
                .all(|v| v.is_finite())
        {
            continue;
        }
        match l.kind {
            3 => {
                let n = l.normal;
                if n[0] * n[0] + n[1] * n[1] + n[2] * n[2] < 0.25 {
                    continue;
                }
                let dir = normalize([-n[0], -n[1], -n[2]]);
                if tracer.cached(index, p, || tracer.sky_seen(p, dir)) {
                    out.push((dir, l.intensity));
                }
            }
            1 | 2 | 4 => {
                let d = sub(l.origin, p);
                let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if dist < 1e-3 || (l.radius > 0.0 && dist > l.radius) {
                    continue;
                }
                let [c, li, q] = l.attenuation;
                let fall = 1.0 / (c + li * dist + q * dist * dist).max(1e-6);
                let dir = [d[0] / dist, d[1] / dist, d[2] / dist];
                let mut scale = fall;
                if l.kind == 2 {
                    let dot = -(dir[0] * l.normal[0] + dir[1] * l.normal[1] + dir[2] * l.normal[2]);
                    if dot <= l.stop_dot2 {
                        continue;
                    }
                    if dot < l.stop_dot {
                        scale *= (dot - l.stop_dot2) / (l.stop_dot - l.stop_dot2).max(1e-6);
                    }
                }
                let light = l.intensity.map(|v| v * scale);
                if !light.iter().all(|v| v.is_finite())
                    || light.iter().fold(0.0f32, |m, &v| m.max(v)) < min
                    || !tracer.cached(index, p, || tracer.clear(p, l.origin))
                {
                    continue;
                }
                out.push((dir, light));
            }
            _ => {}
        }
    }
    if tracer.exhausted() {
        // Spent part-way: this point's lights would differ from its neighbors'; ambient light only.
        out.clear();
    }
    out
}

/// The light of a cube along a normal (the squared components weight the three faces it looks at).
pub fn cube_light(cube: &LightCube, n: [f32; 3]) -> [f32; 3] {
    let mut out = [0f32; 3];
    for (axis, &c) in n.iter().enumerate() {
        let face = &cube[axis * 2 + usize::from(c < 0.0)];
        for k in 0..3 {
            out[k] += face[k] * c * c;
        }
    }
    out
}

impl Bsp {
    /// The faces' lightmaps (style 0; a bump-lit face's first, unbumped map) packed into an atlas, tallest first in shelves.
    /// Faces without a lightmap, or past the texture budget, have no block.
    pub fn lightmap_atlas(&self, faces: &[usize]) -> Result<Atlas> {
        let all = self.faces()?;
        let light = match self.lump(lump::LIGHTING) {
            Ok(l) if !l.is_empty() => l,
            _ => self.lump(lump::LIGHTING_HDR)?,
        };
        // Each face's block: its samples' size, plus the padding.
        let mut blocks: Vec<(usize, u32, u32)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for &fi in faces {
            let Some(f) = all.get(fi) else { continue };
            if f.light_offset < 0 || !seen.insert(fi) {
                continue;
            }
            let (w, h) = (f.light_size[0], f.light_size[1]);
            if !(0..MAX_FACE_LUXELS).contains(&w) || !(0..MAX_FACE_LUXELS).contains(&h) {
                continue;
            }
            blocks.push((fi, w as u32 + 1, h as u32 + 1));
        }
        blocks.sort_by_key(|b| (std::cmp::Reverse(b.2), std::cmp::Reverse(b.1)));
        let area: u64 = blocks
            .iter()
            .map(|b| u64::from(b.1 + 2) * u64::from(b.2 + 2))
            .sum();
        let width = if area * 5 / 4 < 1024 * 1024 {
            1024
        } else if area * 5 / 4 < 2048 * 2048 {
            2048
        } else {
            MAX_ATLAS_SIDE
        };
        let rows_cap = (MAX_ATLAS_TEXELS / width as usize) as u32;
        let mut atlas = Atlas {
            width,
            height: 0,
            texels: Vec::new(),
            places: std::collections::HashMap::new(),
            left_out: 0,
        };
        // The reserved blocks on the first shelf.
        let (mut x, mut y, mut shelf) = (8u32, 0u32, 4u32);
        let grow = |atlas: &mut Atlas, rows: u32| {
            if rows > atlas.height {
                atlas
                    .texels
                    .resize(rows as usize * width as usize, [0.0; 3]);
                atlas.height = rows;
            }
        };
        grow(&mut atlas, 4);
        for (bx, value) in [(0usize, 1.0f32), (4, GAIN_LIGHT)] {
            for ty in 0..4 {
                for tx in 0..4 {
                    atlas.texels[ty * width as usize + bx + tx] = [value; 3];
                }
            }
        }
        for (fi, w, h) in blocks {
            let (pw, ph) = (w + 2, h + 2);
            if pw > width {
                atlas.left_out += 1;
                continue;
            }
            if x + pw > width {
                x = 0;
                y += shelf;
                shelf = 0;
            }
            if y + ph > rows_cap {
                atlas.left_out += 1;
                continue;
            }
            shelf = shelf.max(ph);
            grow(&mut atlas, y + ph);
            let Some(f) = all.get(fi) else { continue };
            let start = f.light_offset as usize / 4;
            let (wu, hu) = (w as usize, h as usize);
            for ty in 0..ph as usize {
                for tx in 0..pw as usize {
                    // The padding repeats the nearest edge sample.
                    let sx = tx.saturating_sub(1).min(wu - 1);
                    let sy = ty.saturating_sub(1).min(hu - 1);
                    let c = Bsp::sample(light, start + sy * wu + sx).unwrap_or([0.0; 3]);
                    atlas.texels[(y as usize + ty) * width as usize + x as usize + tx] = c;
                }
            }
            atlas.places.insert(fi, (x + 1, y + 1));
            x += pw;
        }
        // A height of whole blocks of four (textures prefer it).
        let rounded = atlas.height.div_ceil(4) * 4;
        grow(&mut atlas, rounded);
        Ok(atlas)
    }

    /// The light cube of the ambient sample nearest `p` in the leaf that holds it (the LDR samples, else the HDR ones; in maps
    /// without the index, the leaf's one cube), in the lightmaps' scale (1 = full); none
    /// for a point outside the tree, a leaf without samples, or a map without the per-leaf ambient lumps.
    pub fn ambient_at(&self, p: [f32; 3]) -> Result<Option<LightCube>> {
        Ok(self.ambient()?.at(p))
    }

    /// The ambient light cubes read once (the tree, the leaves and the lumps that hold them), for many [`Ambient::at`]
    /// lookups.
    pub fn ambient(&self) -> Result<Ambient<'_>> {
        let leaves = self.leaves()?;
        let ldr = (
            self.lump(lump::LEAF_AMBIENT_INDEX),
            self.lump(lump::LEAF_AMBIENT_LIGHTING),
        );
        let (index, samples, per_leaf) = match ldr {
            (Ok(i), Ok(s)) if !i.is_empty() && !s.is_empty() => (i, s, false),
            // Earlier maps of the same version have no index: one light cube (24 bytes) a leaf, for the leaf's whole box.
            (Ok(i), Ok(s)) if i.is_empty() && s.len() == leaves.len() * 24 => (i, s, true),
            _ => (
                self.lump(lump::LEAF_AMBIENT_INDEX_HDR)?,
                self.lump(lump::LEAF_AMBIENT_LIGHTING_HDR)?,
                false,
            ),
        };
        Ok(Ambient {
            head: self.models()?.first().map(|m| m.head_node),
            nodes: self.nodes()?,
            planes: self.planes()?,
            leaves,
            index,
            samples,
            per_leaf,
        })
    }
}

/// A map's ambient light cubes, read once ([`Bsp::ambient`]).
#[derive(Debug, Clone)]
pub struct Ambient<'a> {
    head: Option<i32>,
    nodes: Vec<(i32, [i32; 2])>,
    planes: Vec<([f32; 3], f32)>,
    leaves: Vec<Leaf>,
    index: &'a [u8],
    samples: &'a [u8],
    per_leaf: bool,
}

impl Ambient<'_> {
    /// [`Bsp::ambient_at`] from what was read.
    pub fn at(&self, p: [f32; 3]) -> Option<LightCube> {
        let mut steps = MAX_TREE_DEPTH.min(self.nodes.len() + 1);
        let leaf = leaf_in(&self.nodes, &self.planes, self.head?, p, &mut steps)?;
        let l = self.leaves.get(leaf)?;
        let (index, samples) = (self.index, self.samples);
        if self.per_leaf {
            let c = slice(samples, leaf.checked_mul(24)?, 24, "a leaf's light cube").ok()?;
            let mut cube = [[0f32; 3]; 6];
            for (face, v) in cube.iter_mut().enumerate() {
                *v = Bsp::sample(c, face).unwrap_or([0.0; 3]).map(|x| x * 255.0);
            }
            return Some(cube);
        }
        let count = u16_at(index, leaf.checked_mul(4)?, "an ambient index").ok()?;
        let first = usize::from(u16_at(index, leaf * 4 + 2, "an ambient index").ok()?);
        let mut best: Option<(f32, LightCube)> = None;
        for k in first..first + usize::from(count) {
            let Ok(s) = slice(samples, k * 28, 28, "an ambient sample") else {
                break;
            };
            let frac = [s[24], s[25], s[26]];
            let pos: [f32; 3] = [0, 1, 2].map(|a| {
                f32::from(l.mins[a])
                    + f32::from(frac[a]) / 255.0 * (f32::from(l.maxs[a]) - f32::from(l.mins[a]))
            });
            let d = dist2(pos, p);
            if best.as_ref().is_none_or(|b| d < b.0) {
                // The ambient samples read as the lightmaps' encoding come out 255 times darker than the lightmap of an
                // up-facing floor beside them (checked on maps of two games): they hold `rgb x 2^exponent` without the
                // lightmaps' division by 255, so it is undone here.
                let mut cube = [[0f32; 3]; 6];
                for (face, c) in cube.iter_mut().enumerate() {
                    *c = Bsp::sample(s, face).unwrap_or([0.0; 3]).map(|v| v * 255.0);
                }
                best = Some((d, cube));
            }
        }
        best.map(|b| b.1)
    }
}

/// The most luxel reads one sun-agreement check makes.
pub const MAX_CHECK_READS: u64 = 1 << 26;

/// The eight ways a face's lightmap could be read against its luxel grid: `(transposed, flip s, flip t)`.
pub const ORIENTATIONS: [(bool, bool, bool); 8] = [
    (false, false, false),
    (false, true, false),
    (false, false, true),
    (false, true, true),
    (true, false, false),
    (true, true, false),
    (true, false, true),
    (true, true, true),
];

/// How well a map's lightmaps agree with its sun: for luxels of faces whose lightmap has contrast, whether the luxel is bright
/// (over the middle of its face's range) against whether the sun is seen from the luxel's place, read in each of
/// [`ORIENTATIONS`]; brush faces and displacements apart. `(agreeing, compared)` per orientation (transposed readings compare
/// square lightmaps only).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SunAgreement {
    pub faces: [(u64, u64); 8],
    pub displacements: [(u64, u64); 8],
}

impl Bsp {
    /// The [`SunAgreement`] of up to `max_luxels` luxels, the sun taken from the map's sky light (none without one). Each
    /// luxel's place: a brush face's from its lightmap vectors (the plane point at the luxel's center), a displacement's from
    /// its grid at the luxel's place across the base face.
    pub fn sun_agreement(&self, max_luxels: usize) -> Result<Option<SunAgreement>> {
        let Some(sun) = self.world_lights()?.into_iter().find(|l| l.kind == 3) else {
            return Ok(None);
        };
        let to_sun = normalize([-sun.normal[0], -sun.normal[1], -sun.normal[2]]);
        let tracer = self.tracer()?;
        // A check, not a load: a larger budget (a hundred times the load's).
        tracer.work.set(MAX_TRACE_WORK.saturating_mul(100));
        let faces = self.faces()?;
        let texinfo = self.texinfo()?;
        let planes = self.planes()?;
        let (verts, edges, surfedges) = (self.vertices()?, self.edges()?, self.surfedges()?);
        let dispinfo = self.dispinfo()?;
        let disp_verts = self.disp_verts()?;
        let light = match self.lump(lump::LIGHTING) {
            Ok(l) if !l.is_empty() => l,
            _ => self.lump(lump::LIGHTING_HDR)?,
        };
        let in_range = |f: &&Face| {
            f.light_offset >= 0
                && (0..MAX_FACE_LUXELS).contains(&f.light_size[0])
                && (0..MAX_FACE_LUXELS).contains(&f.light_size[1])
        };
        let total: u64 = faces
            .iter()
            .filter(in_range)
            .map(|f| (f.light_size[0] as u64 + 1) * (f.light_size[1] as u64 + 1))
            .sum();
        let stride = (total / max_luxels.max(1) as u64).max(1) as usize;
        // Every luxel read is paid from one budget (the faces' range scans and the comparisons together).
        let mut reads = MAX_CHECK_READS;
        let mut out = SunAgreement::default();
        let mut counter = 0usize;
        for f in &faces {
            if f.light_offset < 0 || tracer.exhausted() {
                continue;
            }
            let Some(ti) = usize::try_from(f.texinfo).ok().and_then(|t| texinfo.get(t)) else {
                continue;
            };
            if ti.flags & (surf::SKY | surf::SKY2D | surf::NODRAW) != 0 {
                continue;
            }
            let (w, h) = (f.light_size[0], f.light_size[1]);
            if !(0..MAX_FACE_LUXELS).contains(&w)
                || !(0..MAX_FACE_LUXELS).contains(&h)
                || w * h == 0
            {
                continue;
            }
            let (wu, hu) = (w as usize + 1, h as usize + 1);
            let start = f.light_offset as usize / 4;
            let value = |x: usize, y: usize| -> f32 {
                Bsp::sample(light, start + y * wu + x).map_or(0.0, |c| c[0] + c[1] + c[2])
            };
            let face_reads = (wu * hu) as u64 * 9;
            if reads < face_reads {
                break;
            }
            reads -= face_reads;
            let (mut lo, mut hi) = (f32::INFINITY, 0f32);
            for y in 0..hu {
                for x in 0..wu {
                    let v = value(x, y);
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            // Only faces with a shadow edge on them say anything.
            if !(hi > 0.05 && hi > 3.0 * lo.max(1e-4)) {
                continue;
            }
            let Some(&(normal, dist)) = planes.get(usize::from(f.plane)) else {
                continue;
            };
            let n_out = if f.side == 0 {
                normal
            } else {
                normal.map(|v| -v)
            };
            // A brush face turned from the sun is dark wherever it is: only faces the sun reaches say anything.
            if f.dispinfo < 0
                && n_out[0] * to_sun[0] + n_out[1] * to_sun[1] + n_out[2] * to_sun[2] < 0.2
            {
                continue;
            }
            let m = &ti.lightmap;
            // A displacement: its base corners (rotated to start at its start position) and grid.
            let disp = if f.dispinfo >= 0 {
                let Some(d) = dispinfo.get(f.dispinfo as usize) else {
                    continue;
                };
                if f.edges != 4 || !(1..=MAX_DISP_POWER).contains(&d.power) {
                    continue;
                }
                let mut poly = Vec::new();
                for k in 0..4 {
                    let Some(se) = usize::try_from(f.first_edge)
                        .ok()
                        .and_then(|e| surfedges.get(e + k))
                    else {
                        break;
                    };
                    let Some(e) = edges.get(se.unsigned_abs() as usize) else {
                        break;
                    };
                    let Some(v) = verts.get(usize::from(if *se >= 0 { e[0] } else { e[1] })) else {
                        break;
                    };
                    poly.push(*v);
                }
                if poly.len() != 4 {
                    continue;
                }
                let side = (1usize << d.power) + 1;
                let first = usize::try_from(d.first_vert).unwrap_or(usize::MAX);
                let Some(dv) = first
                    .checked_add(side * side)
                    .and_then(|e| disp_verts.get(first..e))
                else {
                    continue;
                };
                let start = (0..4)
                    .min_by(|&a, &b| dist2(poly[a], d.start).total_cmp(&dist2(poly[b], d.start)))
                    .unwrap_or(0);
                let c = [0, 1, 2, 3].map(|k| poly[(start + k) % 4]);
                Some((c, side, dv))
            } else {
                None
            };
            for y in 0..hu {
                for x in 0..wu {
                    counter += 1;
                    if !counter.is_multiple_of(stride) {
                        continue;
                    }
                    let (sx, ty) = (x as f32, y as f32);
                    let place = match &disp {
                        None => solve3(
                            [
                                [m[0][0], m[0][1], m[0][2]],
                                [m[1][0], m[1][1], m[1][2]],
                                normal,
                            ],
                            [
                                sx + f.light_mins[0] as f32 - m[0][3],
                                ty + f.light_mins[1] as f32 - m[1][3],
                                dist,
                            ],
                        ),
                        Some((c, side, dv)) => {
                            // The luxel's grid place (the lightmap laid over the grid: s along its columns, t along
                            // its rows), then the displaced grid there.
                            let span = (*side - 1) as f32;
                            let j = ((sx / w.max(1) as f32 * span).round() as usize).min(side - 1);
                            let i = ((ty / h.max(1) as f32 * span).round() as usize).min(side - 1);
                            let ra = lerp(c[0], c[1], i as f32 / span);
                            let rb = lerp(c[3], c[2], i as f32 / span);
                            let flat = lerp(ra, rb, j as f32 / span);
                            let (dir, dd, _) = dv[i * side + j];
                            Some([
                                flat[0] + dir[0] * dd,
                                flat[1] + dir[1] * dd,
                                flat[2] + dir[2] * dd,
                            ])
                        }
                    };
                    let Some(place) = place else { continue };
                    let from = [0, 1, 2].map(|k| place[k] + n_out[k] * 2.0 + to_sun[k] * 2.0);
                    let lit_by_sun = tracer.sky_seen(from, to_sun);
                    for (o, &(transposed, fs, ft)) in ORIENTATIONS.iter().enumerate() {
                        if transposed && w != h {
                            continue;
                        }
                        let (mut rx, mut ry) = if transposed { (y, x) } else { (x, y) };
                        if fs {
                            rx = wu - 1 - rx;
                        }
                        if ft {
                            ry = hu - 1 - ry;
                        }
                        let bright = (value(rx, ry) - lo) / (hi - lo) > 0.5;
                        let slot = if disp.is_some() {
                            &mut out.displacements[o]
                        } else {
                            &mut out.faces[o]
                        };
                        slot.1 += 1;
                        if bright == lit_by_sun {
                            slot.0 += 1;
                        }
                    }
                }
            }
        }
        Ok(Some(out))
    }
}

/// The solution of a 3 x 3 system (rows `m`), none when it has none.
fn solve3(m: [[f32; 3]; 3], r: [f32; 3]) -> Option<[f32; 3]> {
    let det = |a: [[f32; 3]; 3]| {
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    };
    let d = det(m);
    if d.abs() < 1e-9 || !d.is_finite() {
        return None;
    }
    let col = |k: usize| {
        let mut a = m;
        for row in 0..3 {
            a[row][k] = r[row];
        }
        det(a) / d
    };
    Some([col(0), col(1), col(2)])
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = sub(a, b);
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-12 && l.is_finite() {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// A small map (tests and fuzz seeds): a square floor face, a displacement of power 2 beside it, a brush model, two entities,
/// a static prop, a lighting lump and a one-file pak lump.
pub fn write_test_map() -> Vec<u8> {
    test_map(false)
}

/// [`write_test_map`] with a sun (a sky world light) and two ambient samples in its leaf.
pub fn write_lit_test_map() -> Vec<u8> {
    test_map(true)
}

fn test_map(lit: bool) -> Vec<u8> {
    let mut lumps: Vec<Vec<u8>> = vec![Vec::new(); 64];
    let f = |v: &mut Vec<u8>, x: f32| v.extend_from_slice(&x.to_le_bytes());
    let i = |v: &mut Vec<u8>, x: i32| v.extend_from_slice(&x.to_le_bytes());
    let h = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    lumps[lump::ENTITIES] =
        b"{\n\"classname\" \"worldspawn\"\n}\n{\n\"classname\" \"func_wall\"\n\"model\" \"*1\"\n\"origin\" \"0 0 8\"\n}\n\0".to_vec();
    // Vertices: the floor's square, then the displacement's square beside it.
    let square = |x0: f32| {
        [
            [x0, 0.0, 0.0],
            [x0, 64.0, 0.0],
            [x0 + 64.0, 64.0, 0.0],
            [x0 + 64.0, 0.0, 0.0],
        ]
    };
    for p in square(0.0).iter().chain(square(64.0).iter()) {
        for c in p {
            f(&mut lumps[lump::VERTEXES], *c);
        }
    }
    // Edges and surfedges: two loops of four.
    h(&mut lumps[lump::EDGES], 0);
    h(&mut lumps[lump::EDGES], 0);
    for q in 0..2u16 {
        for k in 0..4u16 {
            h(&mut lumps[lump::EDGES], q * 4 + k);
            h(&mut lumps[lump::EDGES], q * 4 + (k + 1) % 4);
            i(&mut lumps[lump::SURFEDGES], i32::from(q * 4 + k + 1));
        }
    }
    // Planes.
    for c in [0.0f32, 0.0, 1.0, 0.0] {
        f(&mut lumps[lump::PLANES], c);
    }
    i(&mut lumps[lump::PLANES], 2);
    // Texture placement and data.
    for (s, t) in [([1.0f32, 0.0, 0.0, 0.0], [0.0f32, 1.0, 0.0, 0.0])] {
        for c in s.iter().chain(t.iter()) {
            f(&mut lumps[lump::TEXINFO], *c);
        }
        for c in [0.25f32, 0.0, 0.0, 0.0, 0.0, 0.25, 0.0, 0.0] {
            f(&mut lumps[lump::TEXINFO], c);
        }
        i(&mut lumps[lump::TEXINFO], 0);
        i(&mut lumps[lump::TEXINFO], 0);
    }
    for c in [0.5f32, 0.5, 0.5] {
        f(&mut lumps[lump::TEXDATA], c);
    }
    for v in [0, 64, 64, 64, 64] {
        i(&mut lumps[lump::TEXDATA], v);
    }
    lumps[lump::TEXDATA_STRING_DATA] = b"DEV/FLOOR\0".to_vec();
    i(&mut lumps[lump::TEXDATA_STRING_TABLE], 0);
    // Faces: the floor (lit), the displacement.
    for (q, disp) in [(0i32, -1i16), (1, 0)] {
        let face = &mut lumps[lump::FACES];
        h(face, 0);
        face.extend_from_slice(&[0, 0]);
        i(face, q * 4);
        face.extend_from_slice(&4i16.to_le_bytes());
        face.extend_from_slice(&0i16.to_le_bytes());
        face.extend_from_slice(&disp.to_le_bytes());
        face.extend_from_slice(&(-1i16).to_le_bytes());
        face.extend_from_slice(&[0, 255, 255, 255]);
        i(face, 0);
        f(face, 4096.0);
        for v in [0, 0, 16, 16, 0] {
            i(face, v);
        }
        face.extend_from_slice(&[0u8; 8]);
    }
    // Lighting: 17 x 17 samples of mid gray.
    for _ in 0..17 * 17 {
        lumps[lump::LIGHTING].extend_from_slice(&[128, 128, 128, 0]);
    }
    // The displacement.
    let mut d = Vec::new();
    for c in [64.0f32, 0.0, 0.0] {
        f(&mut d, c);
    }
    i(&mut d, 0);
    i(&mut d, 0);
    i(&mut d, 2);
    d.resize(36, 0);
    h(&mut d, 1);
    d.resize(176, 0);
    lumps[lump::DISPINFO] = d;
    for k in 0..25 {
        for c in [0.0f32, 0.0, 1.0, (k % 5) as f32 * 4.0, (k * 11) as f32] {
            f(&mut lumps[lump::DISP_VERTS], c);
        }
    }
    // Models: the world (both faces), the brush entity (the floor again).
    for (first, n) in [(0, 2), (0, 1)] {
        for c in [0.0f32, 0.0, 0.0, 128.0, 64.0, 16.0, 0.0, 0.0, 0.0] {
            f(&mut lumps[lump::MODELS], c);
        }
        i(&mut lumps[lump::MODELS], -1);
        i(&mut lumps[lump::MODELS], first);
        i(&mut lumps[lump::MODELS], n);
    }
    // One leaf (area 1) holding both faces.
    let leaf = &mut lumps[lump::LEAFS];
    i(leaf, 0);
    leaf.extend_from_slice(&0i16.to_le_bytes());
    h(leaf, 1);
    leaf.extend_from_slice(&[0u8; 12]);
    h(leaf, 0);
    h(leaf, 2);
    leaf.extend_from_slice(&[0u8; 8]);
    h(&mut lumps[lump::LEAFFACES], 0);
    h(&mut lumps[lump::LEAFFACES], 1);
    // The static prop lump (version 6, 64-byte props), after the game lump directory.
    let mut sprp = Vec::new();
    i(&mut sprp, 1);
    let mut name = b"models/props/box.mdl".to_vec();
    name.resize(128, 0);
    sprp.extend_from_slice(&name);
    i(&mut sprp, 1);
    h(&mut sprp, 0);
    i(&mut sprp, 1);
    let mut prop = Vec::new();
    for c in [32.0f32, 32.0, 0.0, 0.0, 90.0, 0.0] {
        f(&mut prop, c);
    }
    h(&mut prop, 0);
    h(&mut prop, 0);
    h(&mut prop, 1);
    prop.extend_from_slice(&[6, 0]);
    i(&mut prop, 0);
    prop.resize(64, 0);
    sprp.extend_from_slice(&prop);
    // The pak lump: a stored zip of one file.
    let pak = stored_zip(
        "materials/dev/floor.vmt",
        b"LightmappedGeneric { $basetexture dev/floor }",
    );
    lumps[lump::PAKFILE] = pak;
    if lit {
        let mut sun = Vec::new();
        for c in [0.0f32, 0.0, 0.0, 1.0, 1.0, 1.0, 0.3, 0.2, -1.0] {
            f(&mut sun, c);
        }
        sun.resize(40, 0);
        i(&mut sun, 3);
        sun.resize(88, 0);
        lumps[lump::WORLDLIGHTS] = sun;
        h(&mut lumps[lump::LEAF_AMBIENT_INDEX], 2);
        h(&mut lumps[lump::LEAF_AMBIENT_INDEX], 0);
        for k in 0..2u8 {
            let mut sample = [64u8, 64, 64, 0].repeat(6);
            sample.extend_from_slice(&[k * 255, 128, 0, 0]);
            lumps[lump::LEAF_AMBIENT_LIGHTING].extend_from_slice(&sample);
        }
    }
    layout(lumps, &sprp)
}

/// A map file of `lumps` (the leaf lump marked version 1) and a game lump holding `sprp` as its static props.
pub fn layout(mut lumps: Vec<Vec<u8>>, sprp: &[u8]) -> Vec<u8> {
    lumps.resize(64, Vec::new());
    let i = |v: &mut Vec<u8>, x: i32| v.extend_from_slice(&x.to_le_bytes());
    let h = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    // The header, then each lump; the game lump's directory points at the static props after it.
    let header = 8 + 64 * 16 + 4;
    let mut out = vec![0u8; header];
    out[..4].copy_from_slice(b"VBSP");
    out[4..8].copy_from_slice(&20i32.to_le_bytes());
    for (k, l) in lumps.iter_mut().enumerate() {
        if k == lump::GAME_LUMP {
            continue;
        }
        let at = out.len();
        out.extend_from_slice(l);
        out[8 + k * 16..12 + k * 16].copy_from_slice(&(at as i32).to_le_bytes());
        out[12 + k * 16..16 + k * 16].copy_from_slice(&(l.len() as i32).to_le_bytes());
        if k == lump::LEAFS {
            out[16 + k * 16..20 + k * 16].copy_from_slice(&1i32.to_le_bytes());
        }
    }
    let gl = out.len();
    let sprp_at = gl + 4 + 16;
    let mut g = Vec::new();
    i(&mut g, 1);
    g.extend_from_slice(&u32::from_be_bytes(*b"sprp").to_le_bytes());
    h(&mut g, 0);
    h(&mut g, 6);
    i(&mut g, sprp_at as i32);
    i(&mut g, sprp.len() as i32);
    g.extend_from_slice(sprp);
    out.extend_from_slice(&g);
    out[8 + lump::GAME_LUMP * 16..12 + lump::GAME_LUMP * 16]
        .copy_from_slice(&(gl as i32).to_le_bytes());
    out[12 + lump::GAME_LUMP * 16..16 + lump::GAME_LUMP * 16]
        .copy_from_slice(&(g.len() as i32).to_le_bytes());
    out
}

/// The leaf of the tree from node `head` that holds `p`; each node visited is paid from `steps` (None when they run out, or
/// when the walk leaves the tree).
fn leaf_in(
    nodes: &[(i32, [i32; 2])],
    planes: &[([f32; 3], f32)],
    head: i32,
    p: [f32; 3],
    steps: &mut usize,
) -> Option<usize> {
    let mut n = head;
    for _ in 0..=nodes.len() {
        if n < 0 {
            return Some((-(n + 1)) as usize);
        }
        *steps = steps.checked_sub(1)?;
        let &(plane, children) = nodes.get(n as usize)?;
        let &(normal, dist) = usize::try_from(plane).ok().and_then(|p| planes.get(p))?;
        let d = normal[0] * p[0] + normal[1] * p[1] + normal[2] * p[2] - dist;
        n = if d >= 0.0 { children[0] } else { children[1] };
    }
    None
}

/// A zip of one stored (uncompressed) file.
pub fn stored_zip(name: &str, data: &[u8]) -> Vec<u8> {
    let crc = crc32(data);
    let mut z = Vec::new();
    let put16 = |z: &mut Vec<u8>, v: u16| z.extend_from_slice(&v.to_le_bytes());
    let put32 = |z: &mut Vec<u8>, v: u32| z.extend_from_slice(&v.to_le_bytes());
    put32(&mut z, 0x0403_4b50);
    put16(&mut z, 10);
    put16(&mut z, 0);
    put16(&mut z, 0);
    put32(&mut z, 0);
    put32(&mut z, crc);
    put32(&mut z, data.len() as u32);
    put32(&mut z, data.len() as u32);
    put16(&mut z, name.len() as u16);
    put16(&mut z, 0);
    z.extend_from_slice(name.as_bytes());
    z.extend_from_slice(data);
    let cd = z.len();
    put32(&mut z, 0x0201_4b50);
    put16(&mut z, 20);
    put16(&mut z, 10);
    put16(&mut z, 0);
    put16(&mut z, 0);
    put32(&mut z, 0);
    put32(&mut z, crc);
    put32(&mut z, data.len() as u32);
    put32(&mut z, data.len() as u32);
    put16(&mut z, name.len() as u16);
    put16(&mut z, 0);
    put16(&mut z, 0);
    put16(&mut z, 0);
    put16(&mut z, 0);
    put32(&mut z, 0);
    put32(&mut z, 0);
    z.extend_from_slice(name.as_bytes());
    let cd_len = z.len() - cd;
    put32(&mut z, 0x0605_4b50);
    put16(&mut z, 0);
    put16(&mut z, 0);
    put16(&mut z, 1);
    put16(&mut z, 1);
    put32(&mut z, cd_len as u32);
    put32(&mut z, cd as u32);
    put16(&mut z, 0);
    z
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_builds_the_test_map() {
        let b = write_test_map();
        let m = Bsp::parse(&b).unwrap();
        assert_eq!(m.version, 20);
        let ents = m.entities().unwrap();
        assert_eq!(ents.len(), 2);
        assert_eq!(value(&ents[1], "model"), Some("*1"));
        let props = m.static_props().unwrap();
        assert_eq!(props.len(), 1);
        assert_eq!(props[0].model, "models/props/box.mdl");
        assert_eq!(props[0].angles, [0.0, 90.0, 0.0]);
        assert_eq!(props[0].leaves, vec![0]);
        let surfaces = m.build(&m.world_faces().unwrap(), [0.0; 3]).unwrap();
        assert_eq!(surfaces.len(), 2);
        let floor = surfaces.iter().find(|s| !s.displacement).unwrap();
        assert_eq!((floor.vertices.len(), floor.indices.len()), (4, 6));
        assert!(floor.vertices[0].light.is_some());
        let disp = surfaces.iter().find(|s| s.displacement).unwrap();
        assert_eq!((disp.vertices.len(), disp.indices.len()), (25, 16 * 6));
        // The last row is raised 16 units along z and fully blended.
        assert!((disp.vertices[24].position[2] - 16.0).abs() < 1e-4);
        assert!(disp.vertices.iter().any(|v| v.alpha >= 1.0));
        assert_eq!(m.face_areas().unwrap(), vec![Some(1), Some(1)]);
        assert_eq!(m.leaf_at([1.0, 1.0, 1.0]).unwrap(), Some(0));
        let pak = m.pak().unwrap().unwrap();
        assert!(pak.read(b"materials/dev/floor.vmt").is_some());
    }

    #[test]
    fn packs_the_lightmaps_into_an_atlas() {
        let m = Bsp::parse(&write_test_map()).unwrap();
        let faces = m.world_faces().unwrap();
        let atlas = m.lightmap_atlas(&faces).unwrap();
        assert!(atlas.has(0) && atlas.has(1));
        assert_eq!(atlas.width, 1024);
        assert_eq!(atlas.height % 4, 0);
        // Mid gray: 128 x 2^0 / 255, and the padding copies the edge.
        let at = |x: usize, y: usize| atlas.texels[y * 1024 + x];
        assert!((at(1, 1)[0] - 1.0).abs() < 1e-6, "the full-light block");
        assert!((at(5, 1)[0] - GAIN_LIGHT).abs() < 1e-6, "the gain block");
        let (x0, y0) = atlas.places[&0];
        let v = at(x0 as usize, y0 as usize)[0];
        assert!((v - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(at(x0 as usize - 1, y0 as usize - 1)[0], v);
        let surfaces = m.build_lit(&faces, [0.0; 3], Some(&atlas)).unwrap();
        let floor = surfaces.iter().find(|s| !s.displacement).unwrap();
        // The floor's corner at the origin maps to luxel (0, 0): the block's first texel's center.
        let corner = floor
            .vertices
            .iter()
            .find(|v| v.position == [0.0, 0.0, 0.0])
            .unwrap();
        assert_eq!(corner.lightmap, Some([x0 as f32 + 0.5, y0 as f32 + 0.5]));
        // A displacement's lightmap lies over its grid from its start corner: s along the columns, t along the rows, to the
        // face's size (16 x 16 here), whatever its lightmap vectors project.
        let disp = surfaces.iter().find(|s| s.displacement).unwrap();
        let (dx, dy) = atlas.places[&1];
        let at = |row: usize, col: usize| disp.vertices[row * 5 + col].lightmap.unwrap();
        assert_eq!(at(0, 0), [dx as f32 + 0.5, dy as f32 + 0.5]);
        assert_eq!(at(0, 4), [dx as f32 + 16.5, dy as f32 + 0.5]);
        assert_eq!(at(4, 0), [dx as f32 + 0.5, dy as f32 + 16.5]);
        assert_eq!(at(2, 1), [dx as f32 + 4.5, dy as f32 + 8.5]);
    }

    #[test]
    fn a_light_cube_lights_by_the_normal() {
        let mut cube = [[0f32; 3]; 6];
        cube[4] = [1.0, 2.0, 3.0];
        cube[1] = [4.0, 4.0, 4.0];
        assert_eq!(cube_light(&cube, [0.0, 0.0, 1.0]), [1.0, 2.0, 3.0]);
        assert_eq!(cube_light(&cube, [-1.0, 0.0, 0.0]), [4.0, 4.0, 4.0]);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let c = cube_light(&cube, [-h, 0.0, h]);
        assert!((c[0] - 2.5).abs() < 1e-5);
    }

    #[test]
    fn a_deep_tree_under_many_unlisted_faces_is_refused() {
        // 20,000 faces in no leaf, each over the same 4,096 edges, above a chain of 20,000 nodes: the leaf fallback would walk
        // about 480 million steps.
        let mut lumps = vec![Vec::new(); 64];
        for c in [0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            lumps[lump::VERTEXES].extend_from_slice(&c.to_le_bytes());
        }
        lumps[lump::EDGES] = [0u16, 1, 1, 2]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        lumps[lump::SURFEDGES] = (0..4096).flat_map(|_| 1i32.to_le_bytes()).collect();
        let mut face = [0u8; 56];
        face[8..10].copy_from_slice(&4096i16.to_le_bytes());
        face[12..14].copy_from_slice(&(-1i16).to_le_bytes());
        lumps[lump::FACES] = face.repeat(20_000);
        let mut plane = Vec::new();
        for c in [0f32, 0.0, 1.0, -1.0e9] {
            plane.extend_from_slice(&c.to_le_bytes());
        }
        plane.extend_from_slice(&2i32.to_le_bytes());
        lumps[lump::PLANES] = plane;
        let n = 20_000i32;
        for k in 0..n {
            let next = if k + 1 < n { k + 1 } else { -1 };
            let mut node = vec![0u8; 32];
            node[4..8].copy_from_slice(&next.to_le_bytes());
            node[8..12].copy_from_slice(&next.to_le_bytes());
            lumps[lump::NODES].extend_from_slice(&node);
        }
        lumps[lump::LEAFS] = vec![0u8; 32];
        lumps[lump::MODELS] = vec![0u8; 48];
        let b = layout(lumps, &[]);
        let m = Bsp::parse(&b).unwrap();
        let t = std::time::Instant::now();
        let e = m.face_areas().unwrap_err();
        assert!(e.to_string().contains("over the cap"), "{e}");
        assert!(t.elapsed().as_secs() < 10);
    }

    #[test]
    fn lighting_traces_stay_in_their_budget() {
        // A chain of 20,000 nodes (every point walks the whole chain), 8,192 point lights all in reach, 1,000 points.
        let mut lumps = vec![Vec::new(); 64];
        let mut plane = Vec::new();
        for c in [0f32, 0.0, 1.0, -1.0e9] {
            plane.extend_from_slice(&c.to_le_bytes());
        }
        plane.extend_from_slice(&2i32.to_le_bytes());
        lumps[lump::PLANES] = plane;
        let n = 20_000i32;
        for k in 0..n {
            let next = if k + 1 < n { k + 1 } else { -1 };
            let mut node = vec![0u8; 32];
            node[4..8].copy_from_slice(&next.to_le_bytes());
            node[8..12].copy_from_slice(&next.to_le_bytes());
            lumps[lump::NODES].extend_from_slice(&node);
        }
        lumps[lump::LEAFS] = vec![0u8; 32];
        let mut model = vec![0u8; 48];
        for (k, v) in [-4096f32, -4096.0, -4096.0, 4096.0, 4096.0, 4096.0]
            .iter()
            .enumerate()
        {
            model[k * 4..k * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        lumps[lump::MODELS] = model;
        let m = Bsp::parse(&layout(lumps, &[])).unwrap();
        let tracer = m.tracer().unwrap();
        let lights: Vec<WorldLight> = (0..8192)
            .map(|k| WorldLight {
                kind: 1,
                origin: [k as f32, 2000.0, 100.0],
                intensity: [1e6; 3],
                normal: [0.0; 3],
                stop_dot: 0.0,
                stop_dot2: 0.0,
                radius: 0.0,
                attenuation: [1.0, 0.0, 0.0],
            })
            .chain([WorldLight {
                kind: 3,
                origin: [0.0; 3],
                intensity: [f32::NAN; 3],
                normal: [0.0, 0.0, -1.0],
                stop_dot: 0.0,
                stop_dot2: 0.0,
                radius: 0.0,
                attenuation: [0.0; 3],
            }])
            .collect();
        // The kept answers stop at their cap.
        let mut small = m.tracer().unwrap();
        small.max_seen = 10;
        let _ = direct_lights(&small, &lights[..100], [1.0, 2.0, 3.0], 0.01);
        assert!(small.seen.borrow().len() <= 10);
        let t = std::time::Instant::now();
        for k in 0..1000 {
            let _ = direct_lights(&tracer, &lights, [k as f32, -(k as f32), 10.0], 0.01);
        }
        assert!(t.elapsed().as_secs() < 30, "{:?}", t.elapsed());
        // The leaf is empty: every light is seen (cached per leaf), or the budget ran out first.
        assert!(tracer.exhausted() || tracer.seen.borrow().len() <= 8193);
    }

    #[test]
    fn reads_the_sun_and_the_ambient_samples() {
        let m = Bsp::parse(&write_lit_test_map()).unwrap();
        assert_eq!(m.world_lights().unwrap()[0].kind, 3);
        let a = m.ambient().unwrap();
        let cube = a.at([1.0, 2.0, 3.0]).unwrap();
        assert!(cube.iter().all(|c| c[0] > 0.0));
        assert_eq!(m.ambient_at([1.0, 2.0, 3.0]).unwrap(), Some(cube));
        let _ = m.sun_agreement(64).unwrap();
    }

    #[test]
    fn static_props_cannot_copy_their_leaves_over_and_over() {
        // 4,000 props each naming the whole list of 4,000 leaves: 16 million copies from a lump of a few hundred kB.
        let mut sprp = Vec::new();
        sprp.extend_from_slice(&1i32.to_le_bytes());
        sprp.extend_from_slice(&[0u8; 128]);
        sprp.extend_from_slice(&4000i32.to_le_bytes());
        sprp.extend((0..4000u16).flat_map(|l| l.to_le_bytes()));
        sprp.extend_from_slice(&4000i32.to_le_bytes());
        let mut prop = vec![0u8; 64];
        prop[28..30].copy_from_slice(&4000u16.to_le_bytes());
        for _ in 0..4000 {
            sprp.extend_from_slice(&prop);
        }
        let mut lumps = vec![Vec::new(); 64];
        lumps[lump::MODELS] = vec![0u8; 48];
        let m = Bsp::parse(&layout(lumps, &sprp)).unwrap();
        let e = m.static_props().unwrap_err();
        assert!(e.to_string().contains("leaf lists"), "{e}");
    }

    #[test]
    fn a_face_s_corners_are_paid_before_they_are_read() {
        assert_eq!(Bsp::sample(&[1, 2, 3, 0], usize::MAX / 4), None);
        assert_eq!(Bsp::sample(&[255, 0, 0, 0], 0).map(|c| c[0]), Some(1.0));
    }

    #[test]
    fn refuses_bad_headers() {
        assert!(Bsp::parse(b"VBSP").is_err());
        let mut b = write_test_map();
        b[8 + 3 * 16..12 + 3 * 16].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(Bsp::parse(&b).is_err());
        let mut b = write_test_map();
        b[4..8].copy_from_slice(&29i32.to_le_bytes());
        assert!(Bsp::parse(&b).is_err());
    }
}
