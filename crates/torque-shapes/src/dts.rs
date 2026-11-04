//! The reader of a `.dts` (version 24): [`parse`] reads the file the way `TSShape::read` and `assembleShape` do, into the full
//! [`Model`], and [`scan`] is the same walk reduced to counts (it keeps no vertex). The writer that gives the bytes back is
//! `crate::write`.
//!
//! **The file.**
//!
//! ```text
//! u32  version word: the low byte is the version (24), the bits above the exporter's version
//! u32  sizeMemBuffer     the three buffers together, in 32-bit words
//! u32  startU16          where the 16-bit buffer starts, in words
//! u32  startU8           where the 8-bit buffer starts, in words
//! sizeMemBuffer words    [0, startU16) the 32-bit buffer, [startU16, startU8) the 16-bit one, [startU8, end) the 8-bit one
//! u32  numSequences, then the sequences (each: 15 words and eight membership sets)
//! the material list      (module `matlist`)
//! ```
//!
//! Each buffer has its own cursor. Torque only asserts on the bounds and the guard words: this crate bounds each cursor by its
//! section, counts the guards and checks their values (a counter from 0, the
//! 16- and 8-bit ones truncated). Reading order is `assembleShape`'s: counts, bounds, nodes, objects, decals, `.ifl` materials,
//! sub-shapes, default rotations and translations, the keyframe arrays, ground transforms, object states, decal states, triggers,
//! details, meshes, names, each group followed by a guard (a word in each buffer).
//!
//! **Parent meshes.** A mesh whose `parentMesh` is not negative owns no vertex, texture-coordinate or normal bytes: its arrays
//! are the parent's. Its counts are in the file, so a shape can claim millions of vertices in a few bytes:
//! [`Scan::shared_verts`] counts them and the per-shape caps count them too.
//!
//! The version words 19 to 23 and the old format (below 19) are unread ([`ShapeError`]).

use std::sync::Arc;

use crate::matlist::{self, MaterialList};
use crate::shape::{
    Decal, Detail, FileExtras, IflMaterial, IntSet, Mesh, MeshExtra, MeshKind, MeshRender, Model,
    Node, Object, ObjectState, Primitive, Sequence, Shape, SkinData, SortedData, SubShape, Trigger,
};
use crate::{
    Charge, MAX_DTS_BYTES, MAX_MESH_TRIANGLES, MAX_MESH_VERTS, MAX_NAME_BYTES, MAX_SEQUENCES,
    MAX_SHAPE_DETAILS, MAX_SHAPE_MESHES, MAX_SHAPE_NAMES, MAX_SHAPE_NODES, MAX_SHAPE_OBJECTS,
    MAX_SHAPE_PRIMS_TOTAL, MAX_SHAPE_SUBSHAPES, MAX_SHAPE_VERTS_TOTAL, ShapeError, pay, reserved,
    within,
};

/// The newest version Torque reads (`TSShape::smVersion`).
pub const NEWEST_VERSION: u8 = 24;
/// Below this version Torque reads the old format (`... Shape with old version.`).
pub const OLD_BELOW_VERSION: u8 = 19;
/// The bytes before the buffers: the version word and the three sizes.
pub const HEADER_BYTES: usize = 16;
/// The most words of a membership set: Torque's `TSIntegerSet` holds six (a `Sequence` keeps its eight sets in a row).
/// `TSIntegerSet::read` reads `sz` words into the six-word set and asserts `sz <= 6`; this crate refuses more.
pub const MAX_SET_WORDS: usize = 6;
/// The most entries of any keyframe array, trigger list or state list.
pub const MAX_KEYFRAMES: usize = 1 << 20;

/// The 16 bytes before the buffers, checked against the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub version: u8,
    /// Bits 8 to 15 of the version word (Torque ignores them).
    pub version_middle: u8,
    pub exporter: u16,
    pub size_words: u32,
    pub start16: u32,
    pub start8: u32,
}

/// Reads and checks the header of a `.dts` of version 24.
pub fn header(bytes: &[u8]) -> Result<Header, ShapeError> {
    let word: [u8; 4] = bytes
        .get(..4)
        .and_then(|b| b.try_into().ok())
        .ok_or(ShapeError::Short)?;
    let word = u32::from_le_bytes(word);
    let version = (word & 0xff) as u8;
    let exporter = (word >> 16) as u16;
    if version > NEWEST_VERSION {
        return Err(ShapeError::NewerVersion(version));
    }
    if version < OLD_BELOW_VERSION {
        return Err(ShapeError::OldVersion(version));
    }
    if version != NEWEST_VERSION {
        return Err(ShapeError::UnsupportedVersion(version));
    }
    if bytes.len() > MAX_DTS_BYTES {
        return Err(ShapeError::TooLarge {
            bytes: bytes.len(),
            cap: MAX_DTS_BYTES,
        });
    }
    let sizes = bytes
        .get(4..HEADER_BYTES)
        .ok_or(ShapeError::BadHeader("sizes"))?;
    let at = |i: usize| u32::from_le_bytes([sizes[i], sizes[i + 1], sizes[i + 2], sizes[i + 3]]);
    let (size_words, start16, start8) = (at(0), at(4), at(8));
    if start16 > start8 || start8 > size_words {
        return Err(ShapeError::BadHeader("buffer starts"));
    }
    if (HEADER_BYTES as u64) + u64::from(size_words) * 4 > bytes.len() as u64 {
        return Err(ShapeError::BadHeader("buffers longer than the file"));
    }
    Ok(Header {
        version,
        version_middle: ((word >> 8) & 0xff) as u8,
        exporter,
        size_words,
        start16,
        start8,
    })
}

/// The three buffers with their cursors. A read past a buffer's own end is an error, never a read of the next one.
struct Buffers<'a> {
    w32: &'a [u8],
    w16: &'a [u8],
    w8: &'a [u8],
    p32: usize,
    p16: usize,
    p8: usize,
}

impl<'a> Buffers<'a> {
    fn new(body: &'a [u8], header: &Header) -> Buffers<'a> {
        let (a, b) = (header.start16 as usize * 4, header.start8 as usize * 4);
        Buffers {
            w32: &body[..a],
            w16: &body[a..b],
            w8: &body[b..],
            p32: 0,
            p16: 0,
            p8: 0,
        }
    }

    fn u32(&mut self, what: &'static str) -> Result<u32, ShapeError> {
        let at = self.p32 * 4;
        let b = self.w32.get(at..at + 4).ok_or(ShapeError::Truncated {
            buffer: "32-bit",
            what,
        })?;
        self.p32 += 1;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i32(&mut self, what: &'static str) -> Result<i32, ShapeError> {
        self.u32(what).map(|v| v as i32)
    }

    /// The next `count` words of the 32-bit buffer as bytes (a count from the file, `u64` so that a product cannot wrap).
    fn take32(&mut self, count: u64, what: &'static str) -> Result<&'a [u8], ShapeError> {
        let left = (self.w32.len() / 4 - self.p32) as u64;
        if count > left {
            return Err(ShapeError::Truncated {
                buffer: "32-bit",
                what,
            });
        }
        let at = self.p32 * 4;
        self.p32 += count as usize;
        Ok(&self.w32[at..self.p32 * 4])
    }

    /// The next `count` shorts of the 16-bit buffer as bytes.
    fn take16(&mut self, count: u64, what: &'static str) -> Result<&'a [u8], ShapeError> {
        let left = (self.w16.len() / 2 - self.p16) as u64;
        if count > left {
            return Err(ShapeError::Truncated {
                buffer: "16-bit",
                what,
            });
        }
        let at = self.p16 * 2;
        self.p16 += count as usize;
        Ok(&self.w16[at..self.p16 * 2])
    }

    /// The next `count` bytes of the 8-bit buffer.
    fn take8(&mut self, count: u64, what: &'static str) -> Result<&'a [u8], ShapeError> {
        let left = (self.w8.len() - self.p8) as u64;
        if count > left {
            return Err(ShapeError::Truncated {
                buffer: "8-bit",
                what,
            });
        }
        let at = self.p8;
        self.p8 += count as usize;
        Ok(&self.w8[at..self.p8])
    }

    /// The next NUL-terminated string of the 8-bit buffer, without its NUL.
    fn cstr(&mut self, what: &'static str) -> Result<&'a [u8], ShapeError> {
        let rest = &self.w8[self.p8..];
        let len = rest
            .iter()
            .take(MAX_NAME_BYTES + 1)
            .position(|&b| b == 0)
            .ok_or(ShapeError::Truncated {
                buffer: "8-bit",
                what,
            })?;
        self.p8 += len + 1;
        Ok(&rest[..len])
    }

    /// `TSShapeAlloc::checkGuard`: a word, a short and a byte, from counters that start at 0. Counts and compares.
    fn guard(&mut self, guards: &mut Guards) -> Result<(), ShapeError> {
        let g32 = self.u32("a guard")?;
        let g16 = self.take16(1, "a guard")?;
        let g16 = u16::from_le_bytes([g16[0], g16[1]]);
        let g8 = self.take8(1, "a guard")?[0];
        let want = guards.read;
        if g32 != want || g16 != want as u16 || g8 != want as u8 {
            guards.wrong += 1;
        }
        guards.read += 1;
        Ok(())
    }

    /// The bytes left in the 8-bit section (for the reserve of the name table).
    fn left_bytes8(&self) -> usize {
        self.w8.len() - self.p8
    }

    /// Words, shorts and bytes left in the three sections.
    fn left(&self) -> [usize; 3] {
        [
            self.w32.len() / 4 - self.p32,
            self.w16.len() / 2 - self.p16,
            self.w8.len() - self.p8,
        ]
    }

    /// The bytes left in the three sections, copied (they are padding in well-formed files).
    fn left_over(&self) -> [Vec<u8>; 3] {
        [
            self.w32[self.p32 * 4..].to_vec(),
            self.w16[self.p16 * 2..].to_vec(),
            self.w8[self.p8..].to_vec(),
        ]
    }
}

/// The word `k` of a little-endian chunk.
fn wd(c: &[u8], k: usize) -> u32 {
    u32::from_le_bytes([c[k * 4], c[k * 4 + 1], c[k * 4 + 2], c[k * 4 + 3]])
}

fn fl(c: &[u8], k: usize) -> f32 {
    f32::from_bits(wd(c, k))
}

fn sh(c: &[u8], k: usize) -> i16 {
    i16::from_le_bytes([c[k * 2], c[k * 2 + 1]])
}

fn us(c: &[u8], k: usize) -> u16 {
    u16::from_le_bytes([c[k * 2], c[k * 2 + 1]])
}

/// A vector of the rows of `bytes` (`row` bytes each), reserved through [`reserved`]: the one way a section's bytes become an array.
pub(crate) fn rows<T>(bytes: &[u8], row: usize, f: impl Fn(&[u8]) -> T) -> Vec<T> {
    let mut v = reserved(bytes.len() / row, bytes.len(), row);
    v.extend(bytes.chunks_exact(row).map(f));
    v
}

pub(crate) fn f32x3(bytes: &[u8]) -> Vec<[f32; 3]> {
    rows(bytes, 12, |c| [fl(c, 0), fl(c, 1), fl(c, 2)])
}

pub(crate) fn quats(bytes: &[u8]) -> Vec<[i16; 4]> {
    rows(bytes, 8, |c| [sh(c, 0), sh(c, 1), sh(c, 2), sh(c, 3)])
}

/// The guard words read and how many did not hold the value Torque's counter would write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Guards {
    pub read: u32,
    pub wrong: u32,
}

/// The counts at the head of the 32-bit buffer (version 24 has no skin count).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub nodes: usize,
    pub objects: usize,
    pub decals: usize,
    pub sub_shapes: usize,
    pub ifl_materials: usize,
    pub node_rotations: usize,
    pub node_translations: usize,
    pub uniform_scales: usize,
    pub aligned_scales: usize,
    pub arbitrary_scales: usize,
    pub ground_frames: usize,
    pub object_states: usize,
    pub decal_states: usize,
    pub triggers: usize,
    pub details: usize,
    pub meshes: usize,
    pub names: usize,
    pub smallest_visible_size: i32,
    pub smallest_visible_dl: i32,
}

/// What a mesh's header says (`TSMesh::assemble`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshHeader {
    pub kind: MeshKind,
    pub num_frames: i32,
    pub num_mat_frames: i32,
    pub parent: i32,
    pub radius: i32,
    pub num_verts: usize,
    pub num_tverts: usize,
    /// Primitives, and the index count.
    pub num_prims: usize,
    pub num_indices: usize,
    pub verts_per_frame: i32,
    pub flags: u32,
    /// A skin mesh's counts (`None` for the other kinds).
    pub skin: Option<SkinCounts>,
}

impl MeshHeader {
    /// How many vertices a child of this mesh can share: a skin mesh's lists hold its initial vertices (the reader points the
    /// vertex and normal lists at `initialVerts`), the others' their own.
    pub fn vert_capacity(&self) -> usize {
        self.skin.map_or(self.num_verts, |s| s.verts)
    }
}

/// The counts of a skin mesh's extra arrays (`TSSkinMesh::assemble`): initial vertices (and normals), initial transforms, the
/// vertex, bone and weight arrays (one count for the three) and node indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkinCounts {
    pub verts: usize,
    pub transforms: usize,
    pub weights: usize,
    pub nodes: usize,
}

/// What [`scan`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
    pub header: Header,
    pub counts: Counts,
    pub radius: f32,
    pub tube_radius: f32,
    pub center: [f32; 3],
    pub bounds: [f32; 6],
    pub names: Vec<Vec<u8>>,
    pub details: Vec<Detail>,
    pub sub_shapes: Vec<SubShape>,
    pub meshes: Vec<MeshHeader>,
    pub sequences: Vec<Sequence>,
    pub materials: MaterialList,
    /// Words, shorts and bytes left unread in the three sections when the buffer part ended.
    pub left: [usize; 3],
    pub guards: Guards,
    /// Bytes after the material list (0 for a well-formed version-24 file).
    pub trailing: usize,
    /// Vertices and primitives of all meshes, the ones a parent mesh shares counted again.
    pub total_verts: usize,
    pub total_prims: usize,
    /// Skin transforms, weights and node indices of all skin meshes, shared ones counted again (the initial vertices are in
    /// `total_verts`).
    pub total_skin: usize,
    /// Of those vertices, the ones shared with a parent (no bytes behind them).
    pub shared_verts: usize,
}

fn count(b: &mut Buffers, what: &'static str, cap: usize) -> Result<usize, ShapeError> {
    let v = b.u32(what)?;
    within(what, u64::from(v), cap)
}

fn int_set(r: &mut Reader, what: &'static str) -> Result<IntSet, ShapeError> {
    let first = r.u32(what)?;
    let size = within(what, u64::from(r.u32(what)?), MAX_SET_WORDS)?;
    let mut words = Vec::new();
    for _ in 0..size {
        words.push(r.u32(what)?);
    }
    Ok(IntSet { first, words })
}

/// A little-endian reader over the bytes after the buffers (the sequences, and the whole of a `.dsq`).
pub(crate) struct Reader<'a> {
    pub bytes: &'a [u8],
    pub at: usize,
    pub buffer: &'static str,
}

impl Reader<'_> {
    pub(crate) fn u32(&mut self, what: &'static str) -> Result<u32, ShapeError> {
        let b = self
            .bytes
            .get(self.at..self.at + 4)
            .ok_or(ShapeError::Truncated {
                buffer: self.buffer,
                what,
            })?;
        self.at += 4;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn i32(&mut self, what: &'static str) -> Result<i32, ShapeError> {
        self.u32(what).map(|v| v as i32)
    }

    pub(crate) fn f32(&mut self, what: &'static str) -> Result<f32, ShapeError> {
        self.u32(what).map(f32::from_bits)
    }

    /// The next `count` elements of `size` bytes (a checked product against what remains).
    pub(crate) fn take(
        &mut self,
        count: u64,
        size: u64,
        what: &'static str,
    ) -> Result<&[u8], ShapeError> {
        let bytes = count.checked_mul(size).ok_or(ShapeError::Bad(what))?;
        let left = (self.bytes.len() - self.at) as u64;
        if bytes > left {
            return Err(ShapeError::Truncated {
                buffer: self.buffer,
                what,
            });
        }
        let start = self.at;
        self.at += bytes as usize;
        Ok(&self.bytes[start..self.at])
    }
}

/// `TSShape::Sequence::read`: 14 words after the name (15 with it), the float duration among them, then the eight membership
/// sets (rotation, translation, scale, decal, ifl, vis, frame, material-frame).
pub(crate) fn read_sequence(r: &mut Reader, with_name: bool) -> Result<Sequence, ShapeError> {
    let name = if with_name {
        r.i32("a sequence name")?
    } else {
        -1
    };
    let flags = r.u32("a sequence")?;
    let num_keyframes = r.i32("a sequence")?;
    let duration = r.f32("a sequence")?;
    let priority = r.i32("a sequence")?;
    let first_ground_frame = r.i32("a sequence")?;
    let num_ground_frames = r.i32("a sequence")?;
    let base_rotation = r.i32("a sequence")?;
    let base_translation = r.i32("a sequence")?;
    let base_scale = r.i32("a sequence")?;
    let base_object_state = r.i32("a sequence")?;
    let base_decal_state = r.i32("a sequence")?;
    let first_trigger = r.i32("a sequence")?;
    let num_triggers = r.i32("a sequence")?;
    let tool_begin = r.f32("a sequence")?;
    Ok(Sequence {
        name,
        flags,
        num_keyframes,
        duration,
        priority,
        first_ground_frame,
        num_ground_frames,
        base_rotation,
        base_translation,
        base_scale,
        base_object_state,
        base_decal_state,
        first_trigger,
        num_triggers,
        tool_begin,
        rotation_matters: int_set(r, "a sequence set")?,
        translation_matters: int_set(r, "a sequence set")?,
        scale_matters: int_set(r, "a sequence set")?,
        decal_matters: int_set(r, "a sequence set")?,
        ifl_matters: int_set(r, "a sequence set")?,
        vis_matters: int_set(r, "a sequence set")?,
        frame_matters: int_set(r, "a sequence set")?,
        mat_frame_matters: int_set(r, "a sequence set")?,
    })
}

/// What a parse learned about the file besides the model.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseInfo {
    pub header: Header,
    pub counts: Counts,
    pub guards: Guards,
    /// Words, shorts and bytes left unread in the three sections when the buffer part ended.
    pub left: [usize; 3],
    /// Vertices and primitives of all meshes, the ones a parent mesh shares counted again.
    pub total_verts: usize,
    pub total_prims: usize,
    /// Skin transforms, weights and node indices of all skin meshes, shared ones counted again.
    pub total_skin: usize,
    /// Of the vertices, the ones shared with a parent (no bytes behind them).
    pub shared_verts: usize,
    /// The bytes the material list took.
    pub material_bytes: usize,
}

/// Reads a version-24 `.dts` into the model: `charge` is asked for the file's bytes first and for each mesh after; it
/// returns false when the run cannot pay, which ends the parse with [`ShapeError::OutOfBudget`] (no partial shape).
pub fn parse(bytes: &[u8], charge: Charge) -> Result<Model, ShapeError> {
    parse_with_info(bytes, charge).map(|(model, _)| model)
}

/// [`parse`], with what it learned about the file besides the model.
pub fn parse_with_info(bytes: &[u8], charge: Charge) -> Result<(Model, ParseInfo), ShapeError> {
    let header = header(bytes)?;
    pay(charge, bytes.len() as u64)?;
    let body_end = HEADER_BYTES + header.size_words as usize * 4;
    let mut b = Buffers::new(&bytes[HEADER_BYTES..body_end], &header);
    let mut guards = Guards::default();

    let counts = Counts {
        nodes: count(&mut b, "node count", MAX_SHAPE_NODES)?,
        objects: count(&mut b, "object count", MAX_SHAPE_OBJECTS)?,
        decals: count(&mut b, "decal count", MAX_SHAPE_OBJECTS)?,
        sub_shapes: count(&mut b, "sub-shape count", MAX_SHAPE_SUBSHAPES)?,
        ifl_materials: count(&mut b, "ifl material count", crate::MAX_MATERIALS)?,
        node_rotations: count(&mut b, "node rotation count", MAX_KEYFRAMES)?,
        node_translations: count(&mut b, "node translation count", MAX_KEYFRAMES)?,
        uniform_scales: count(&mut b, "uniform scale count", MAX_KEYFRAMES)?,
        aligned_scales: count(&mut b, "aligned scale count", MAX_KEYFRAMES)?,
        arbitrary_scales: count(&mut b, "arbitrary scale count", MAX_KEYFRAMES)?,
        ground_frames: count(&mut b, "ground frame count", MAX_KEYFRAMES)?,
        object_states: count(&mut b, "object state count", MAX_KEYFRAMES)?,
        decal_states: count(&mut b, "decal state count", MAX_KEYFRAMES)?,
        triggers: count(&mut b, "trigger count", MAX_KEYFRAMES)?,
        details: count(&mut b, "detail count", MAX_SHAPE_DETAILS)?,
        meshes: count(&mut b, "mesh count", MAX_SHAPE_MESHES)?,
        names: count(&mut b, "name count", MAX_SHAPE_NAMES)?,
        smallest_visible_size: b.i32("smallest visible size")?,
        smallest_visible_dl: b.i32("smallest visible detail")?,
    };
    pay(charge, 64)?;
    b.guard(&mut guards)?;

    let radius = f32::from_bits(b.u32("radius")?);
    let tube_radius = f32::from_bits(b.u32("tube radius")?);
    let center = {
        let c = f32x3(b.take32(3, "center")?);
        c[0]
    };
    let bounds = {
        let c = b.take32(6, "bounds")?;
        [fl(c, 0), fl(c, 1), fl(c, 2), fl(c, 3), fl(c, 4), fl(c, 5)]
    };
    b.guard(&mut guards)?;

    let n = |v: usize| v as u64;
    let nodes = rows(b.take32(n(counts.nodes) * 5, "the nodes")?, 20, |c| Node {
        name: wd(c, 0) as i32,
        parent: wd(c, 1) as i32,
        first_object: wd(c, 2) as i32,
        first_child: wd(c, 3) as i32,
        next_sibling: wd(c, 4) as i32,
    });
    b.guard(&mut guards)?;
    let objects = rows(b.take32(n(counts.objects) * 6, "the objects")?, 24, |c| {
        Object {
            name: wd(c, 0) as i32,
            num_meshes: wd(c, 1) as i32,
            start_mesh_index: wd(c, 2) as i32,
            node: wd(c, 3) as i32,
            next_sibling: wd(c, 4) as i32,
            first_decal: wd(c, 5) as i32,
        }
    });
    b.guard(&mut guards)?;
    let decals = rows(b.take32(n(counts.decals) * 5, "the decals")?, 20, |c| {
        Decal {
            name: wd(c, 0) as i32,
            num_meshes: wd(c, 1) as i32,
            start_mesh_index: wd(c, 2) as i32,
            object: wd(c, 3) as i32,
            next_sibling: wd(c, 4) as i32,
        }
    });
    b.guard(&mut guards)?;
    let ifl_materials = rows(
        b.take32(n(counts.ifl_materials) * 5, "the .ifl materials")?,
        20,
        |c| IflMaterial {
            name: wd(c, 0) as i32,
            material_slot: wd(c, 1) as i32,
            first_frame: wd(c, 2) as i32,
            first_frame_off_time: wd(c, 3) as i32,
            num_frames: wd(c, 4) as i32,
        },
    );
    b.guard(&mut guards)?;
    // Sub-shapes: three arrays of firsts, guard, three of counts, guard.
    let ss = counts.sub_shapes;
    let firsts = rows(b.take32(n(ss) * 3, "a sub-shape")?, 4, |c| wd(c, 0) as i32);
    b.guard(&mut guards)?;
    let nums = rows(b.take32(n(ss) * 3, "a sub-shape")?, 4, |c| wd(c, 0) as i32);
    b.guard(&mut guards)?;
    let mut sub_shapes = reserved(ss, firsts.len() * 4, 4);
    for i in 0..ss {
        sub_shapes.push(SubShape {
            first_node: firsts[i],
            first_object: firsts[ss + i],
            first_decal: firsts[2 * ss + i],
            num_nodes: nums[i],
            num_objects: nums[ss + i],
            num_decals: nums[2 * ss + i],
        });
    }
    // Default rotations (four shorts a node) and translations (three words), then the node keyframes.
    let default_rotations = quats(b.take16(n(counts.nodes) * 4, "the default rotations")?);
    let default_translations = f32x3(b.take32(n(counts.nodes) * 3, "the default translations")?);
    let node_translations =
        f32x3(b.take32(n(counts.node_translations) * 3, "the node translations")?);
    let node_rotations = quats(b.take16(n(counts.node_rotations) * 4, "the node rotations")?);
    b.guard(&mut guards)?;
    let node_uniform_scales = rows(
        b.take32(n(counts.uniform_scales), "the uniform scales")?,
        4,
        |c| fl(c, 0),
    );
    let node_aligned_scales = f32x3(b.take32(n(counts.aligned_scales) * 3, "the aligned scales")?);
    let node_arbitrary_scale_factors = f32x3(b.take32(
        n(counts.arbitrary_scales) * 3,
        "the arbitrary scale factors",
    )?);
    let node_arbitrary_scale_rots = quats(b.take16(
        n(counts.arbitrary_scales) * 4,
        "the arbitrary scale rotations",
    )?);
    b.guard(&mut guards)?;
    let ground_translations =
        f32x3(b.take32(n(counts.ground_frames) * 3, "the ground translations")?);
    let ground_rotations = quats(b.take16(n(counts.ground_frames) * 4, "the ground rotations")?);
    b.guard(&mut guards)?;
    let object_states = rows(
        b.take32(n(counts.object_states) * 3, "the object states")?,
        12,
        |c| ObjectState {
            vis: fl(c, 0),
            frame: wd(c, 1) as i32,
            mat_frame: wd(c, 2) as i32,
        },
    );
    b.guard(&mut guards)?;
    let decal_states = rows(
        b.take32(n(counts.decal_states), "the decal states")?,
        4,
        |c| wd(c, 0) as i32,
    );
    b.guard(&mut guards)?;
    let triggers = rows(b.take32(n(counts.triggers) * 2, "the triggers")?, 8, |c| {
        Trigger {
            state: wd(c, 0),
            pos: fl(c, 1),
        }
    });
    b.guard(&mut guards)?;
    let details = rows(b.take32(n(counts.details) * 7, "the details")?, 28, |c| {
        Detail {
            name: wd(c, 0) as i32,
            sub_shape: wd(c, 1) as i32,
            object_detail: wd(c, 2) as i32,
            size: fl(c, 3),
            average_error: fl(c, 4),
            max_error: fl(c, 5),
            poly_count: wd(c, 6) as i32,
        }
    });
    b.guard(&mut guards)?;
    pay(charge, 64 + counts.nodes as u64 + counts.details as u64)?;

    // Not reserved from the count: a null mesh is four bytes of file and a slot of about two hundred bytes of model in each of
    // these two vectors, so they grow with the meshes really read (at most MAX_SHAPE_MESHES).
    let mut meshes: Vec<Option<Mesh>> = Vec::new();
    let mut render: Vec<Option<MeshRender>> = Vec::new();
    let (mut total_verts, mut total_prims, mut total_skin, mut shared_verts) =
        (0usize, 0usize, 0usize, 0usize);
    for _ in 0..counts.meshes {
        pay(charge, 16)?;
        let word = b.u32("a mesh type")?;
        let kind = MeshKind::from_word(word).ok_or(ShapeError::Bad("mesh type"))?;
        if kind == MeshKind::Decal {
            return Err(ShapeError::Bad("mesh type (decal meshes are not read)"));
        }
        if kind == MeshKind::Null {
            meshes.push(None);
            render.push(None);
            continue;
        }
        let (mesh, r) = read_mesh(&mut b, &mut guards, kind, &meshes, &render)?;
        let skin = match &r.extra {
            MeshExtra::Skin(s) => (
                s.num_initial_verts,
                s.num_transforms + s.num_weights + s.num_nodes,
            ),
            _ => (0, 0),
        };
        total_verts = add_total(
            total_verts,
            mesh.num_verts + skin.0,
            MAX_SHAPE_VERTS_TOTAL,
            "shape vertices",
        )?;
        total_prims = add_total(
            total_prims,
            mesh.primitives.len(),
            MAX_SHAPE_PRIMS_TOTAL,
            "shape primitives",
        )?;
        total_skin = add_total(total_skin, skin.1, MAX_SHAPE_VERTS_TOTAL, "shape skin data")?;
        if mesh.parent >= 0 {
            shared_verts += mesh.num_verts;
        }
        meshes.push(Some(mesh));
        render.push(Some(r));
    }
    b.guard(&mut guards)?;

    let mut names: Vec<Vec<u8>> = reserved(counts.names, b.left_bytes8(), 1);
    for _ in 0..counts.names {
        names.push(b.cstr("a name")?.to_vec());
    }
    b.guard(&mut guards)?;
    let left = b.left();
    let left_over = b.left_over();

    let mut r = Reader {
        bytes,
        at: body_end,
        buffer: "sequences",
    };
    let sequence_count = within(
        "sequence count",
        u64::from(r.u32("the sequence count")?),
        MAX_SEQUENCES,
    )?;
    pay(charge, 8 + 4 * sequence_count as u64)?;
    let mut sequences = reserved(sequence_count, bytes.len() - r.at, SEQUENCE_MIN_BYTES);
    for _ in 0..sequence_count {
        sequences.push(read_sequence(&mut r, true)?);
    }
    let list = matlist::read(&bytes[r.at..], charge)?;
    let trailing = bytes[r.at + list.bytes..].to_vec();

    let shape = Shape {
        version: header.version,
        exporter: header.exporter,
        names,
        nodes,
        objects,
        decals,
        ifl_materials,
        sub_shapes,
        details,
        default_rotations,
        default_translations,
        node_rotations,
        node_translations,
        node_uniform_scales,
        node_aligned_scales,
        node_arbitrary_scale_rots,
        node_arbitrary_scale_factors,
        ground_translations,
        ground_rotations,
        object_states,
        decal_states,
        triggers,
        sequences,
        materials: list.materials,
        meshes,
        bounds,
        center,
        radius,
        tube_radius,
        smallest_visible_size: counts.smallest_visible_size,
        smallest_visible_dl: counts.smallest_visible_dl,
    };
    shape.validate_with(
        |i| match render.get(i).and_then(Option::as_ref).map(|r| &r.extra) {
            Some(MeshExtra::Skin(skin)) => skin
                .num_initial_verts
                .max(shape.meshes[i].as_ref().map_or(0, |m| m.num_verts)),
            _ => shape.meshes[i].as_ref().map_or(0, |m| m.num_verts),
        },
    )?;
    let model = Model {
        shape,
        render,
        file: FileExtras {
            version_middle: header.version_middle,
            materials: list.raw,
            left: left_over,
            trailing,
        },
    };
    let info = ParseInfo {
        header,
        counts,
        guards,
        left,
        total_verts,
        total_prims,
        total_skin,
        shared_verts,
        material_bytes: list.bytes,
    };
    Ok((model, info))
}

/// The header of a mesh as [`scan`] reports it.
fn mesh_header(mesh: Option<&Mesh>, render: Option<&MeshRender>) -> MeshHeader {
    let (Some(m), Some(r)) = (mesh, render) else {
        return null_mesh();
    };
    MeshHeader {
        kind: m.kind,
        num_frames: m.num_frames,
        num_mat_frames: m.num_mat_frames,
        parent: m.parent,
        radius: m.radius,
        num_verts: m.num_verts,
        num_tverts: m.num_tverts,
        num_prims: m.primitives.len(),
        num_indices: m.indices.len(),
        verts_per_frame: m.verts_per_frame,
        flags: m.flags,
        skin: match &r.extra {
            MeshExtra::Skin(s) => Some(SkinCounts {
                verts: s.num_initial_verts,
                transforms: s.num_transforms,
                weights: s.num_weights,
                nodes: s.num_nodes,
            }),
            _ => None,
        },
    }
}

/// Walks a version-24 `.dts` and reports what it saw: [`parse`] with the model reduced to counts.
pub fn scan(bytes: &[u8], charge: Charge) -> Result<Scan, ShapeError> {
    let (model, info) = parse_with_info(bytes, charge)?;
    let Model {
        shape,
        render,
        file,
    } = model;
    let meshes = shape
        .meshes
        .iter()
        .zip(&render)
        .map(|(m, r)| mesh_header(m.as_ref(), r.as_ref()))
        .collect();
    let materials = MaterialList {
        materials: shape.materials,
        raw: file.materials,
        bytes: info.material_bytes,
    };
    Ok(Scan {
        header: info.header,
        counts: info.counts,
        radius: shape.radius,
        tube_radius: shape.tube_radius,
        center: shape.center,
        bounds: shape.bounds,
        names: shape.names,
        details: shape.details,
        sub_shapes: shape.sub_shapes,
        meshes,
        sequences: shape.sequences,
        materials,
        left: info.left,
        guards: info.guards,
        trailing: file.trailing.len(),
        total_verts: info.total_verts,
        total_prims: info.total_prims,
        total_skin: info.total_skin,
        shared_verts: info.shared_verts,
    })
}

/// Adds `n` to a per-shape total and refuses a total over `cap`.
fn add_total(total: usize, n: usize, cap: usize, what: &'static str) -> Result<usize, ShapeError> {
    total
        .checked_add(n)
        .filter(|&v| v <= cap)
        .ok_or(ShapeError::Cap {
            what,
            value: (total as u64) + n as u64,
            cap: cap as u64,
        })
}

/// The least bytes a sequence takes in a `.dts`: 15 words and eight sets of two words.
pub(crate) const SEQUENCE_MIN_BYTES: usize = 15 * 4 + 8 * 8;

/// The entry of a `NullMeshType` mesh: nothing follows its type word.
fn null_mesh() -> MeshHeader {
    MeshHeader {
        kind: MeshKind::Null,
        num_frames: 0,
        num_mat_frames: 0,
        parent: -1,
        radius: 0,
        num_verts: 0,
        num_tverts: 0,
        num_prims: 0,
        num_indices: 0,
        verts_per_frame: 0,
        flags: 0,
        skin: None,
    }
}

/// The arrays a child mesh shares with its parent (`getSharedData32` over `smVertsList`, `smTVertsList`, `smNormsList`): a skin
/// parent offers its initial vertices and normals (the reader points the lists at them), any other its own.
struct Shared {
    positions: Arc<[[f32; 3]]>,
    tverts: Arc<[[f32; 2]]>,
    normals: Arc<[[f32; 3]]>,
    encoded: Arc<[u8]>,
}

fn shared_of(m: &Mesh, r: &MeshRender) -> Shared {
    match &r.extra {
        MeshExtra::Skin(s) => Shared {
            positions: Arc::clone(&s.initial_verts),
            tverts: Arc::clone(&r.tverts),
            normals: Arc::clone(&s.initial_normals),
            encoded: Arc::clone(&r.encoded_normals),
        },
        _ => Shared {
            positions: Arc::clone(&m.positions),
            tverts: Arc::clone(&r.tverts),
            normals: Arc::clone(&r.normals),
            encoded: Arc::clone(&r.encoded_normals),
        },
    }
}

/// How many vertices a child of this mesh can share (see [`MeshHeader::vert_capacity`]).
fn vert_capacity(m: &Mesh, r: &MeshRender) -> usize {
    match &r.extra {
        MeshExtra::Skin(s) => s.num_initial_verts,
        _ => m.num_verts,
    }
}

/// One mesh after its type word (`TSMesh::assemble`, `TSSkinMesh`, `TSSortedMesh`). A mesh with a
/// parent owns no vertex, texture-coordinate or normal data; the parent must be an earlier mesh.
fn read_mesh(
    b: &mut Buffers,
    guards: &mut Guards,
    kind: MeshKind,
    earlier: &[Option<Mesh>],
    earlier_render: &[Option<MeshRender>],
) -> Result<(Mesh, MeshRender), ShapeError> {
    b.guard(guards)?;
    let num_frames = b.i32("a mesh")?;
    let num_mat_frames = b.i32("a mesh")?;
    let parent = b.i32("a mesh parent")?;
    let parent_pair = if parent >= 0 {
        let index = parent as usize;
        match (
            earlier.get(index).and_then(Option::as_ref),
            earlier_render.get(index).and_then(Option::as_ref),
        ) {
            (Some(m), Some(r)) => Some((m, r)),
            _ => return Err(ShapeError::Bad("parent mesh (not an earlier mesh)")),
        }
    } else {
        None
    };
    // Bounds (6), centre (3) and the radius (an integer Torque turns into a float).
    let head = b.take32(9, "a mesh")?;
    let bounds = [
        fl(head, 0),
        fl(head, 1),
        fl(head, 2),
        fl(head, 3),
        fl(head, 4),
        fl(head, 5),
    ];
    let center = [fl(head, 6), fl(head, 7), fl(head, 8)];
    let radius = b.i32("a mesh radius")?;
    let owns = parent < 0;

    let num_verts = within("mesh vertices", u64::from(b.u32("a mesh")?), MAX_MESH_VERTS)?;
    if kind == MeshKind::Skin
        && parent_pair.is_some_and(|(_, r)| !matches!(r.extra, MeshExtra::Skin(_)))
    {
        // The reader fills the transform, weight and node lists for skin meshes only: a standard parent has none to share.
        return Err(ShapeError::Bad("skin mesh (the parent is not a skin mesh)"));
    }
    if let Some((pm, pr)) = parent_pair
        && num_verts > vert_capacity(pm, pr)
    {
        return Err(ShapeError::Bad("mesh vertices (more than the parent's)"));
    }
    let shared = parent_pair.map(|(m, r)| shared_of(m, r));
    let mut positions: Arc<[[f32; 3]]> = Arc::from(Vec::new());
    if owns {
        positions = Arc::from(f32x3(b.take32(3 * num_verts as u64, "the vertices")?));
    } else if let Some(s) = &shared {
        positions = Arc::clone(&s.positions);
    }
    let num_tverts = within(
        "mesh texture vertices",
        u64::from(b.u32("a mesh")?),
        MAX_MESH_VERTS,
    )?;
    if let Some((pm, _)) = parent_pair
        && num_tverts > pm.num_tverts
    {
        return Err(ShapeError::Bad("texture vertices (more than the parent's)"));
    }
    let (tverts, normals, encoded_normals);
    if owns {
        tverts = Arc::from(rows(
            b.take32(2 * num_tverts as u64, "the texture vertices")?,
            8,
            |c| [fl(c, 0), fl(c, 1)],
        ));
        // Normals (three words each), then the encoded normals this crate does not use (one byte each): version 24 stores both.
        normals = Arc::from(f32x3(b.take32(3 * num_verts as u64, "the normals")?));
        encoded_normals = Arc::from(b.take8(num_verts as u64, "the encoded normals")?.to_vec());
    } else if let Some(s) = &shared {
        tverts = Arc::clone(&s.tverts);
        normals = Arc::clone(&s.normals);
        encoded_normals = Arc::clone(&s.encoded);
    } else {
        tverts = Arc::from(Vec::new());
        normals = Arc::from(Vec::new());
        encoded_normals = Arc::from(Vec::new());
    }
    let num_prims = within(
        "mesh primitives",
        u64::from(b.u32("a mesh")?),
        MAX_MESH_TRIANGLES,
    )?;
    let prim16 = b.take16(2 * num_prims as u64, "the primitives")?;
    let prim32 = b.take32(num_prims as u64, "the primitive materials")?;
    let mut primitives: Vec<Primitive> = reserved(num_prims, prim32.len(), 4);
    for i in 0..num_prims {
        primitives.push(Primitive {
            start: us(prim16, 2 * i),
            num_elements: us(prim16, 2 * i + 1),
            material: wd(prim32, i),
        });
    }
    let num_indices = within(
        "mesh indices",
        u64::from(b.u32("a mesh")?),
        MAX_MESH_TRIANGLES.saturating_mul(3),
    )?;
    let indices = rows(b.take16(num_indices as u64, "the indices")?, 2, |c| {
        us(c, 0)
    });
    let merge = within(
        "merge indices",
        u64::from(b.u32("a mesh")?),
        MAX_MESH_TRIANGLES.saturating_mul(3),
    )?;
    let merge_indices = rows(b.take16(merge as u64, "the merge indices")?, 2, |c| {
        us(c, 0)
    });
    let verts_per_frame = b.i32("a mesh")?;
    let flags = b.u32("a mesh")?;
    b.guard(guards)?;

    let mut extra = MeshExtra::None;
    match kind {
        MeshKind::Standard | MeshKind::Decal | MeshKind::Null => {}
        MeshKind::Skin => {
            let skin = read_skin(b, guards, owns, parent_pair)?;
            extra = MeshExtra::Skin(skin);
        }
        MeshKind::Sorted => {
            let clusters = within(
                "clusters",
                u64::from(b.u32("a sorted mesh")?),
                MAX_MESH_VERTS,
            )?;
            let clusters = rows(b.take32(8 * clusters as u64, "the clusters")?, 32, |c| {
                [
                    wd(c, 0),
                    wd(c, 1),
                    wd(c, 2),
                    wd(c, 3),
                    wd(c, 4),
                    wd(c, 5),
                    wd(c, 6),
                    wd(c, 7),
                ]
            });
            let mut lists: [Vec<u32>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
            for (what, list) in [
                "start clusters",
                "first vertices",
                "vertex counts",
                "first texture vertices",
            ]
            .into_iter()
            .zip(lists.iter_mut())
            {
                let n = within(what, u64::from(b.u32("a sorted mesh")?), MAX_MESH_VERTS)?;
                *list = rows(b.take32(n as u64, what)?, 4, |c| wd(c, 0));
            }
            let [start_cluster, first_vert, num_verts_list, first_tvert] = lists;
            let always_write_depth = b.u32("always write depth")?;
            b.guard(guards)?;
            extra = MeshExtra::Sorted(SortedData {
                clusters,
                start_cluster,
                first_vert,
                num_verts: num_verts_list,
                first_tvert,
                always_write_depth,
            });
        }
    }
    // A skin mesh keeps its vertices in `initialVerts` (the base mesh has none): they are what its frames index.
    let held = match &extra {
        MeshExtra::Skin(s) => s.num_initial_verts.max(num_verts),
        _ => num_verts,
    };
    check_frames(
        num_frames,
        num_mat_frames,
        verts_per_frame,
        held,
        num_tverts,
    )?;
    let mesh = Mesh {
        kind,
        parent,
        num_frames,
        num_mat_frames,
        verts_per_frame,
        flags,
        bounds,
        center,
        radius,
        num_verts,
        num_tverts,
        positions,
        primitives,
        indices,
    };
    let render = MeshRender {
        tverts,
        normals,
        encoded_normals,
        merge_indices,
        extra,
    };
    Ok((mesh, render))
}

/// The arrays of a skin mesh after its standard part (`TSSkinMesh::assemble`).
fn read_skin(
    b: &mut Buffers,
    guards: &mut Guards,
    owns: bool,
    parent: Option<(&Mesh, &MeshRender)>,
) -> Result<SkinData, ShapeError> {
    let parent_skin = parent.and_then(|(_, r)| match &r.extra {
        MeshExtra::Skin(s) => Some(s),
        _ => None,
    });
    let sz = within("skin vertices", u64::from(b.u32("a skin")?), MAX_MESH_VERTS)?;
    let (initial_verts, initial_normals, initial_encoded_normals);
    if owns {
        initial_verts = Arc::from(f32x3(b.take32(3 * sz as u64, "the initial vertices")?));
        initial_normals = Arc::from(f32x3(b.take32(3 * sz as u64, "the initial normals")?));
        initial_encoded_normals = Arc::from(b.take8(sz as u64, "the encoded normals")?.to_vec());
    } else if let Some(p) = parent_skin {
        initial_verts = Arc::clone(&p.initial_verts);
        initial_normals = Arc::clone(&p.initial_normals);
        initial_encoded_normals = Arc::clone(&p.initial_encoded_normals);
    } else {
        initial_verts = Arc::from(Vec::new());
        initial_normals = Arc::from(Vec::new());
        initial_encoded_normals = Arc::from(Vec::new());
    }
    let transforms = within(
        "skin transforms",
        u64::from(b.u32("a skin")?),
        MAX_MESH_VERTS,
    )?;
    let initial_transforms: Arc<[[f32; 16]]> = if owns {
        Arc::from(rows(
            b.take32(16 * transforms as u64, "the initial transforms")?,
            64,
            |c| std::array::from_fn(|k| fl(c, k)),
        ))
    } else {
        parent_skin.map_or_else(
            || Arc::from(Vec::new()),
            |p| Arc::clone(&p.initial_transforms),
        )
    };
    let weights = within("skin weights", u64::from(b.u32("a skin")?), MAX_MESH_VERTS)?;
    let vertex_index: Arc<[i32]>;
    let bone_index: Arc<[i32]>;
    let weight: Arc<[f32]>;
    if owns {
        // Vertex index, bone index and weight arrays, one count for the three.
        let w = weights as u64;
        vertex_index = Arc::from(rows(b.take32(w, "the skin vertex indices")?, 4, |c| {
            wd(c, 0) as i32
        }));
        bone_index = Arc::from(rows(b.take32(w, "the skin bone indices")?, 4, |c| {
            wd(c, 0) as i32
        }));
        weight = Arc::from(rows(b.take32(w, "the skin weights")?, 4, |c| fl(c, 0)));
    } else if let Some(p) = parent_skin {
        vertex_index = Arc::clone(&p.vertex_index);
        bone_index = Arc::clone(&p.bone_index);
        weight = Arc::clone(&p.weight);
    } else {
        vertex_index = Arc::from(Vec::new());
        bone_index = Arc::from(Vec::new());
        weight = Arc::from(Vec::new());
    }
    let nodes = within("skin nodes", u64::from(b.u32("a skin")?), MAX_MESH_VERTS)?;
    let node_index: Arc<[i32]> = if owns {
        Arc::from(rows(
            b.take32(nodes as u64, "the skin node indices")?,
            4,
            |c| wd(c, 0) as i32,
        ))
    } else {
        parent_skin.map_or_else(|| Arc::from(Vec::new()), |p| Arc::clone(&p.node_index))
    };
    // A child shares the parent's arrays: it can claim no more than the parent holds (Torque reads them without a bound).
    if let Some(p) = parent_skin
        && (sz > p.num_initial_verts
            || transforms > p.num_transforms
            || weights > p.num_weights
            || nodes > p.num_nodes)
    {
        return Err(ShapeError::Bad("skin mesh (more than the parent's arrays)"));
    }
    b.guard(guards)?;
    Ok(SkinData {
        num_initial_verts: sz,
        num_transforms: transforms,
        num_weights: weights,
        num_nodes: nodes,
        initial_verts,
        initial_normals,
        initial_encoded_normals,
        initial_transforms,
        vertex_index,
        bone_index,
        weight,
        node_index,
    })
}

/// The frame counts of a mesh (`TSMesh::assemble` reads them as signed words): none negative, and the products with the vertices
/// per frame checked and within what the mesh holds: `numFrames * vertsPerFrame` vertices and `numMatFrames * vertsPerFrame`
/// texture vertices (Torque indexes by them without a check).
fn check_frames(
    frames: i32,
    mat_frames: i32,
    per_frame: i32,
    num_verts: usize,
    num_tverts: usize,
) -> Result<(), ShapeError> {
    let bad = ShapeError::Bad("mesh frame counts");
    let (Ok(f), Ok(m), Ok(v)) = (
        u64::try_from(frames),
        u64::try_from(mat_frames),
        u64::try_from(per_frame),
    ) else {
        return Err(bad);
    };
    // Each is below 2^31, so a pair multiplies without overflow; the triple is checked.
    f.checked_mul(m)
        .and_then(|fm| fm.checked_mul(v))
        .ok_or(bad.clone())?;
    if f * v > num_verts as u64 || m * v > num_tverts as u64 {
        return Err(bad);
    }
    Ok(())
}

/// A synthetic version-24 file builder for the tests of this crate: the same layout, built from a
/// description, with guards that count as Torque's do.
#[cfg(test)]
pub(crate) mod testkit {
    use super::HEADER_BYTES;

    /// One mesh of the sample: its type, parent and counts.
    #[derive(Clone)]
    pub struct SampleMesh {
        pub kind: u32,
        pub parent: i32,
        pub verts: u32,
        pub tverts: u32,
        pub prims: u32,
        pub indices: u32,
        /// A skin mesh's counts: initial vertices, transforms, weights, node indices.
        pub skin: [u32; 4],
        /// (frames, material frames, vertices per frame) when not (1, 1, vertices).
        pub frames: Option<(u32, u32, u32)>,
    }

    pub struct Sample {
        pub exporter: u16,
        pub nodes: u32,
        pub names: Vec<&'static str>,
        /// (name index, sub-shape, object detail)
        pub details: Vec<(i32, i32, i32)>,
        pub meshes: Vec<SampleMesh>,
        pub sequences: u32,
        pub materials: Vec<&'static str>,
    }

    impl Default for Sample {
        fn default() -> Sample {
            Sample {
                exporter: 1,
                nodes: 2,
                names: vec!["root", "Collision-1", "LOS-9", "detail2"],
                details: vec![(3, 0, 0), (1, 0, 0), (2, 0, 0)],
                meshes: vec![SampleMesh {
                    kind: 0,
                    parent: -1,
                    verts: 4,
                    tverts: 4,
                    prims: 1,
                    indices: 6,
                    skin: [0; 4],
                    frames: None,
                }],
                sequences: 1,
                materials: vec!["a", "dir/b"],
            }
        }
    }

    struct Out {
        w32: Vec<u32>,
        w16: Vec<u16>,
        w8: Vec<u8>,
        guard: u32,
    }

    impl Out {
        fn guard(&mut self) {
            self.w32.push(self.guard);
            self.w16.push(self.guard as u16);
            self.w8.push(self.guard as u8);
            self.guard += 1;
        }
    }

    impl Sample {
        pub fn build(&self) -> Vec<u8> {
            let mut o = Out {
                w32: Vec::new(),
                w16: Vec::new(),
                w8: Vec::new(),
                guard: 0,
            };
            let ss = 1u32;
            let counts = [
                self.nodes,
                0,
                0,
                ss,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                self.details.len() as u32,
                self.meshes.len() as u32,
                self.names.len() as u32,
                100,
                0,
            ];
            o.w32.extend_from_slice(&counts);
            o.guard();
            o.w32.extend_from_slice(&[0x3f80_0000; 2]);
            o.w32.extend_from_slice(&[0; 3]);
            o.w32.extend_from_slice(&[0; 6]);
            o.guard();
            for _ in 0..self.nodes {
                // Name 0, no parent, and the links init derives (none).
                o.w32.extend_from_slice(&[0, u32::MAX, 0, 0, 0]);
            }
            o.guard();
            o.guard(); // objects
            o.guard(); // decals
            o.guard(); // ifl
            o.w32.extend_from_slice(&[0, 0, 0]);
            o.guard();
            o.w32.extend_from_slice(&[self.nodes, 0, 0]);
            o.guard();
            for _ in 0..self.nodes * 4 {
                o.w16.push(0);
            }
            for _ in 0..self.nodes * 3 {
                o.w32.push(0);
            }
            o.guard(); // node keyframes
            o.guard(); // scales
            o.guard(); // ground
            o.guard(); // object states
            o.guard(); // decal states
            o.guard(); // triggers
            for d in &self.details {
                o.w32.extend_from_slice(&[
                    d.0 as u32,
                    d.1 as u32,
                    d.2 as u32,
                    0x4120_0000,
                    0,
                    0,
                    0,
                ]);
            }
            o.guard();
            for m in &self.meshes {
                o.w32.push(m.kind);
                if m.kind == 4 {
                    continue;
                }
                o.guard();
                let (f, mf, vpf) = m.frames.unwrap_or((1, 1, m.verts));
                o.w32.extend_from_slice(&[f, mf, m.parent as u32]);
                o.w32.extend_from_slice(&[0; 9]);
                o.w32.push(1); // radius
                o.w32.push(m.verts);
                if m.parent < 0 {
                    o.w32
                        .extend(std::iter::repeat_n(0u32, m.verts as usize * 3));
                }
                o.w32.push(m.tverts);
                if m.parent < 0 {
                    o.w32
                        .extend(std::iter::repeat_n(0u32, m.tverts as usize * 2));
                    o.w32
                        .extend(std::iter::repeat_n(0u32, m.verts as usize * 3));
                    o.w8.extend(std::iter::repeat_n(0u8, m.verts as usize));
                }
                o.w32.push(m.prims);
                o.w16
                    .extend(std::iter::repeat_n(0u16, m.prims as usize * 2));
                o.w32.extend(std::iter::repeat_n(0u32, m.prims as usize));
                o.w32.push(m.indices);
                o.w16.extend(std::iter::repeat_n(0u16, m.indices as usize));
                o.w32.push(0); // merge indices
                o.w32.push(vpf); // vertsPerFrame
                o.w32.push(0); // flags
                o.guard();
                if m.kind == 3 {
                    o.w32.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
                    o.guard();
                }
                if m.kind == 1 {
                    let [sz, tr, wt, nd] = m.skin;
                    let own = m.parent < 0;
                    o.w32.push(sz);
                    if own {
                        o.w32.extend(std::iter::repeat_n(0u32, sz as usize * 6));
                        o.w8.extend(std::iter::repeat_n(0u8, sz as usize));
                    }
                    o.w32.push(tr);
                    if own {
                        o.w32.extend(std::iter::repeat_n(0u32, tr as usize * 16));
                    }
                    o.w32.push(wt);
                    if own {
                        o.w32.extend(std::iter::repeat_n(0u32, wt as usize * 3));
                    }
                    o.w32.push(nd);
                    if own {
                        o.w32.extend(std::iter::repeat_n(0u32, nd as usize));
                    }
                    o.guard();
                }
            }
            o.guard();
            for name in &self.names {
                o.w8.extend_from_slice(name.as_bytes());
                o.w8.push(0);
            }
            o.guard();

            // Assemble: pad the 16-bit buffer to a word and the 8-bit buffer to a word.
            if o.w16.len() % 2 == 1 {
                o.w16.push(0);
            }
            while !o.w8.len().is_multiple_of(4) {
                o.w8.push(0);
            }
            let start16 = o.w32.len() as u32;
            let start8 = start16 + (o.w16.len() / 2) as u32;
            let size = start8 + (o.w8.len() / 4) as u32;
            let mut out = Vec::with_capacity(HEADER_BYTES + size as usize * 4);
            out.extend_from_slice(&(24u32 | (u32::from(self.exporter) << 16)).to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&start16.to_le_bytes());
            out.extend_from_slice(&start8.to_le_bytes());
            for w in &o.w32 {
                out.extend_from_slice(&w.to_le_bytes());
            }
            for w in &o.w16 {
                out.extend_from_slice(&w.to_le_bytes());
            }
            out.extend_from_slice(&o.w8);
            out.extend_from_slice(&self.sequences.to_le_bytes());
            for i in 0..self.sequences {
                out.extend_from_slice(&(i as i32).to_le_bytes());
                for _ in 0..14 {
                    out.extend_from_slice(&0u32.to_le_bytes());
                }
                for _ in 0..8 {
                    out.extend_from_slice(&0u32.to_le_bytes());
                    out.extend_from_slice(&1u32.to_le_bytes());
                    out.extend_from_slice(&5u32.to_le_bytes());
                }
            }
            out.push(1);
            out.extend_from_slice(&(self.materials.len() as u32).to_le_bytes());
            for m in &self.materials {
                out.push(m.len() as u8);
                out.extend_from_slice(m.as_bytes());
            }
            out.extend(std::iter::repeat_n(0u8, self.materials.len() * 24));
            out
        }
    }
}

#[cfg(test)]
#[path = "dts_tests.rs"]
mod tests;
