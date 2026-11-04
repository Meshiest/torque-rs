//! The model of a shape: what the server keeps ([`Shape`]) and what one parse gives ([`Model`]).
//!
//! These are the types of Torque's `TSShape` after `assembleShape`, as data: [`crate::dts::parse`] fills them and
//! [`crate::write::write_dts`] turns a [`Model`] back into the bytes it came from. The shapes are plain data: no pointers into a
//! memory block, parent meshes share their arrays through [`Arc`], and the reference counts are the only interior state. The
//! derived fields of `TSShape::init` (a node's first object, child and sibling, a detail's polygon count) are kept as the file
//! has them, not recomputed.
//!
//! **What the server keeps.** Nodes with their default rotations and translations, the keyframe arrays (node rotations and
//! translations, uniform, aligned and arbitrary scales, ground transforms, triggers: the simulation's threads and `getMuzzlePoint` need
//! them), objects with their names and mesh indices, sub-shapes, details with their sizes, the names, the sequences (name, flags,
//! duration, priority, ground frames, trigger counts and which nodes and objects they move), the materials with their flags,
//! the bounds, and each mesh's positions and primitives. Texture coordinates and normals are parsed (the writer needs them to
//! re-encode the file) and dropped from the runtime model ([`Shape::for_server`]).

use std::sync::Arc;

use crate::ShapeError;

/// The mesh kinds of the file (`TSMesh::assembleMesh`, the first word of each mesh).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MeshKind {
    /// 0: a standard mesh.
    Standard,
    /// 1: a skin mesh (initial vertices and normals, bones and weights follow).
    Skin,
    /// 2: a decal mesh (this crate does not read it).
    Decal,
    /// 3: a sorted mesh (clusters follow).
    Sorted,
    /// 4: no mesh at all (`NullMeshType`): the type word is the entry and the slot stays empty. Objects have one for a detail
    /// level they do not draw.
    Null,
}

impl MeshKind {
    pub fn from_word(word: u32) -> Option<MeshKind> {
        match word {
            0 => Some(MeshKind::Standard),
            1 => Some(MeshKind::Skin),
            2 => Some(MeshKind::Decal),
            3 => Some(MeshKind::Sorted),
            4 => Some(MeshKind::Null),
            _ => None,
        }
    }

    pub fn word(self) -> u32 {
        match self {
            MeshKind::Standard => 0,
            MeshKind::Skin => 1,
            MeshKind::Decal => 2,
            MeshKind::Sorted => 3,
            MeshKind::Null => 4,
        }
    }
}

/// A node (5 words in the file: name, parent, then the first object, first child and next sibling `init` derives).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Node {
    pub name: i32,
    pub parent: i32,
    pub first_object: i32,
    pub first_child: i32,
    pub next_sibling: i32,
}

/// An object (6 words: name, mesh count, first mesh index, node, next sibling and first decal, the last two derived).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Object {
    pub name: i32,
    pub num_meshes: i32,
    pub start_mesh_index: i32,
    pub node: i32,
    pub next_sibling: i32,
    pub first_decal: i32,
}

/// A sub-shape: the ranges of nodes, objects and decals it owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubShape {
    pub first_node: i32,
    pub first_object: i32,
    pub first_decal: i32,
    pub num_nodes: i32,
    pub num_objects: i32,
    pub num_decals: i32,
}

/// A detail level (7 words): the name, the sub-shape and the object detail it shows, its size and errors, and its polygon count
/// (derived by `init`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Detail {
    pub name: i32,
    pub sub_shape: i32,
    pub object_detail: i32,
    pub size: f32,
    pub average_error: f32,
    pub max_error: f32,
    pub poly_count: i32,
}

/// A rotation as the file stores it: four signed 16-bit components (`Quat16`).
pub type Quat16 = [i16; 4];

/// A trigger of a sequence (two words: the state, the position).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trigger {
    pub state: u32,
    pub pos: f32,
}

/// The most bits a membership set holds: six words (`MAX_TS_SET_DWORDS`), 192 nodes or objects.
pub const SET_BITS: usize = 192;

/// A set of small integers as the file stores it (`TSIntegerSet`): a first word it ignores (Torque's `numInts`; its writer
/// puts 0 there), the count of words, and the words up to the last one that is not zero.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IntSet {
    pub first: u32,
    pub words: Vec<u32>,
}

impl IntSet {
    /// How many bits are set.
    pub fn count(&self) -> u64 {
        self.words.iter().map(|w| u64::from(w.count_ones())).sum()
    }

    /// `TSIntegerSet::test`; a bit past the set's six words is not in it (Torque would read past its array).
    pub fn test(&self, bit: usize) -> bool {
        self.words
            .get(bit / 32)
            .is_some_and(|w| w & (1 << (bit % 32)) != 0)
    }

    /// `TSIntegerSet::set`; false (and nothing set) for a bit the six words cannot hold.
    pub fn set(&mut self, bit: usize) -> bool {
        if bit >= SET_BITS {
            return false;
        }
        if self.words.len() <= bit / 32 {
            self.words.resize(bit / 32 + 1, 0);
        }
        self.words[bit / 32] |= 1 << (bit % 32);
        true
    }
}

/// `TSShape::Sequence` flags: which kind of scale the sequence animates.
pub const SEQ_UNIFORM_SCALE: u32 = 1;
pub const SEQ_ALIGNED_SCALE: u32 = 2;
pub const SEQ_ARBITRARY_SCALE: u32 = 4;

/// A sequence (`TSShape::Sequence`, read with its name index): 15 words and the eight membership sets.
#[derive(Debug, Clone, PartialEq)]
pub struct Sequence {
    pub name: i32,
    pub flags: u32,
    pub num_keyframes: i32,
    pub duration: f32,
    pub priority: i32,
    pub first_ground_frame: i32,
    pub num_ground_frames: i32,
    pub base_rotation: i32,
    pub base_translation: i32,
    pub base_scale: i32,
    pub base_object_state: i32,
    pub base_decal_state: i32,
    pub first_trigger: i32,
    pub num_triggers: i32,
    pub tool_begin: f32,
    pub rotation_matters: IntSet,
    pub translation_matters: IntSet,
    pub scale_matters: IntSet,
    pub decal_matters: IntSet,
    pub ifl_matters: IntSet,
    pub vis_matters: IntSet,
    pub frame_matters: IntSet,
    pub mat_frame_matters: IntSet,
}

/// A material of the list: its name (the part after the last `/` or `\`; none for an `.ifl` material, whose name is dropped
/// after reading) and its flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Material {
    pub name: Option<Vec<u8>>,
    pub flags: u32,
}

/// A decal: hangs off an object as an object hangs off a node (`TSShape::Decal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decal {
    pub name: i32,
    pub num_meshes: i32,
    pub start_mesh_index: i32,
    pub object: i32,
    pub next_sibling: i32,
}

/// An `.ifl` material: a material slot that runs through frames (`TSShape::IflMaterial`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IflMaterial {
    pub name: i32,
    pub material_slot: i32,
    pub first_frame: i32,
    pub first_frame_off_time: i32,
    pub num_frames: i32,
}

/// The state of an object in one keyframe (`TSShape::ObjectState`): its visibility and frame indices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectState {
    pub vis: f32,
    pub frame: i32,
    pub mat_frame: i32,
}

/// The primitive of a mesh as the file stores it: the start in the index list, the number of elements and the material word
/// (its low bits are the strip, fan or triangle type, `TSDrawPrimitive`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Primitive {
    pub start: u16,
    pub num_elements: u16,
    pub material: u32,
}

/// What the server keeps of a mesh: the header words, the bounds, and the positions and primitives that collision reads. A mesh
/// with a parent shares the parent's arrays (`Arc`), as Torque's mesh does: `positions` can then hold more
/// than `num_verts` entries, and [`Mesh::verts`] is the first `num_verts` of them.
#[derive(Debug, Clone, PartialEq)]
pub struct Mesh {
    pub kind: MeshKind,
    pub parent: i32,
    pub num_frames: i32,
    pub num_mat_frames: i32,
    pub verts_per_frame: i32,
    pub flags: u32,
    pub bounds: [f32; 6],
    pub center: [f32; 3],
    /// The integer the file holds; Torque turns it into a float by value (`mRadius = (F32)alloc.get32()`).
    pub radius: i32,
    pub num_verts: usize,
    pub num_tverts: usize,
    pub positions: Arc<[[f32; 3]]>,
    pub primitives: Vec<Primitive>,
    pub indices: Vec<u16>,
}

impl Mesh {
    /// The mesh's vertices (the first `num_verts` of `positions`; all of them when the arrays are somehow shorter).
    pub fn verts(&self) -> &[[f32; 3]] {
        self.positions
            .get(..self.num_verts)
            .unwrap_or(&self.positions)
    }
}

/// The extra arrays of a skin mesh (`TSSkinMesh::assemble`): the initial vertices, normals and encoded normals, the initial bone
/// transforms (16 floats each), and the weights (vertex index, bone index, weight: three arrays of one count) and node indices.
/// A mesh with a parent shares the parent's arrays; the counts are its own.
#[derive(Debug, Clone, PartialEq)]
pub struct SkinData {
    pub num_initial_verts: usize,
    pub num_transforms: usize,
    pub num_weights: usize,
    pub num_nodes: usize,
    pub initial_verts: Arc<[[f32; 3]]>,
    pub initial_normals: Arc<[[f32; 3]]>,
    pub initial_encoded_normals: Arc<[u8]>,
    pub initial_transforms: Arc<[[f32; 16]]>,
    pub vertex_index: Arc<[i32]>,
    pub bone_index: Arc<[i32]>,
    pub weight: Arc<[f32]>,
    pub node_index: Arc<[i32]>,
}

/// The extra arrays of a sorted mesh (`TSSortedMesh::assemble`): the clusters (eight words each: start and end primitive, a
/// normal, `k`, the front and back cluster), four lists of words, and the always-write-depth word.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SortedData {
    pub clusters: Vec<[u32; 8]>,
    pub start_cluster: Vec<u32>,
    pub first_vert: Vec<u32>,
    pub num_verts: Vec<u32>,
    pub first_tvert: Vec<u32>,
    pub always_write_depth: u32,
}

/// What a mesh kind adds to the standard mesh.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum MeshExtra {
    #[default]
    None,
    Skin(SkinData),
    Sorted(SortedData),
}

/// What a parse also gives for the writer and the renderer: the texture coordinates, normals, encoded normals and merge indices of
/// one mesh (shared with the parent's when it has one), and the arrays of a skin or sorted mesh. Dropped from the runtime model.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshRender {
    pub tverts: Arc<[[f32; 2]]>,
    pub normals: Arc<[[f32; 3]]>,
    pub encoded_normals: Arc<[u8]>,
    pub merge_indices: Vec<u16>,
    pub extra: MeshExtra,
}

impl Default for MeshRender {
    fn default() -> MeshRender {
        MeshRender {
            tverts: Arc::from(Vec::new()),
            normals: Arc::from(Vec::new()),
            encoded_normals: Arc::from(Vec::new()),
            merge_indices: Vec::new(),
            extra: MeshExtra::None,
        }
    }
}

/// The shape the server keeps (after `assembleShape` and `init`).
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    /// The low byte of the file's version word (24 for the format this crate reads).
    pub version: u8,
    /// The exporter's version (the bits above).
    pub exporter: u16,
    pub names: Vec<Vec<u8>>,
    pub nodes: Vec<Node>,
    pub objects: Vec<Object>,
    pub decals: Vec<Decal>,
    pub ifl_materials: Vec<IflMaterial>,
    pub sub_shapes: Vec<SubShape>,
    pub details: Vec<Detail>,
    pub default_rotations: Vec<Quat16>,
    pub default_translations: Vec<[f32; 3]>,
    pub node_rotations: Vec<Quat16>,
    pub node_translations: Vec<[f32; 3]>,
    pub node_uniform_scales: Vec<f32>,
    pub node_aligned_scales: Vec<[f32; 3]>,
    pub node_arbitrary_scale_rots: Vec<Quat16>,
    pub node_arbitrary_scale_factors: Vec<[f32; 3]>,
    pub ground_translations: Vec<[f32; 3]>,
    pub ground_rotations: Vec<Quat16>,
    pub object_states: Vec<ObjectState>,
    pub decal_states: Vec<i32>,
    pub triggers: Vec<Trigger>,
    pub sequences: Vec<Sequence>,
    pub materials: Vec<Material>,
    /// One slot per mesh of the file; a mesh this crate does not keep (a skipped detail) is `None`.
    pub meshes: Vec<Option<Mesh>>,
    pub bounds: [f32; 6],
    pub center: [f32; 3],
    pub radius: f32,
    pub tube_radius: f32,
    /// The integer the file holds; Torque turns it into a float by value.
    pub smallest_visible_size: i32,
    pub smallest_visible_dl: i32,
}

/// One material of the list as the file writes it (`MaterialList::read` over `TSMaterialList::read`): the name with any path,
/// the flags, the three map indices and the two floats. The runtime [`Material`] keeps the stripped name and the flags.
#[derive(Debug, Clone, PartialEq)]
pub struct RawMaterial {
    pub name: Vec<u8>,
    pub flags: u32,
    pub reflectance_map: u32,
    pub bump_map: u32,
    pub detail_map: u32,
    pub detail_scale: f32,
    pub reflection_amount: f32,
}

/// The material list as the file has it. An empty name makes Torque's reader stop (it keeps the names before it and no
/// flags; the shape loads), and this crate keeps the rest of the bytes so that the writer can give them back.
#[derive(Debug, Clone, PartialEq)]
pub enum RawMaterials {
    /// Every name and the six arrays after them were read.
    Complete(Vec<RawMaterial>),
    /// The list ended early: the count the file claimed, the names read before the empty one, and the empty name's own entry
    /// (its length byte and bytes).
    Early {
        declared: u32,
        names: Vec<Vec<u8>>,
        terminator: Vec<u8>,
    },
}

/// What the file holds that neither the runtime shape nor the render data keep, so that the writer, fed the model only, gives the
/// same bytes: the middle bytes of the version word, the material list as written, the bytes the three buffers hold past what
/// was read (the 16- and 8-bit sections end in padding to a whole word), and the bytes after the material list (none in a
/// well-formed file).
#[derive(Debug, Clone, PartialEq)]
pub struct FileExtras {
    /// Bits 8 to 15 of the version word (Torque ignores them).
    pub version_middle: u8,
    pub materials: RawMaterials,
    /// Bytes left unread at the end of the 32-bit, 16-bit and 8-bit sections.
    pub left: [Vec<u8>; 3],
    pub trailing: Vec<u8>,
}

impl Default for FileExtras {
    fn default() -> FileExtras {
        FileExtras {
            version_middle: 0,
            materials: RawMaterials::Complete(Vec::new()),
            left: [Vec::new(), Vec::new(), Vec::new()],
            trailing: Vec::new(),
        }
    }
}

/// One parse: the shape, the render data of each mesh (a slot for each of `shape.meshes`) and the file extras.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub shape: Shape,
    pub render: Vec<Option<MeshRender>>,
    pub file: FileExtras,
}

impl Shape {
    /// A version-24 shape with nothing in it (for the synthetic shapes of the tests and the fuzz).
    pub fn empty() -> Shape {
        Shape {
            version: 24,
            exporter: 1,
            names: Vec::new(),
            nodes: Vec::new(),
            objects: Vec::new(),
            decals: Vec::new(),
            ifl_materials: Vec::new(),
            sub_shapes: Vec::new(),
            details: Vec::new(),
            default_rotations: Vec::new(),
            default_translations: Vec::new(),
            node_rotations: Vec::new(),
            node_translations: Vec::new(),
            node_uniform_scales: Vec::new(),
            node_aligned_scales: Vec::new(),
            node_arbitrary_scale_rots: Vec::new(),
            node_arbitrary_scale_factors: Vec::new(),
            ground_translations: Vec::new(),
            ground_rotations: Vec::new(),
            object_states: Vec::new(),
            decal_states: Vec::new(),
            triggers: Vec::new(),
            sequences: Vec::new(),
            materials: Vec::new(),
            meshes: Vec::new(),
            bounds: [0.0; 6],
            center: [0.0; 3],
            radius: 0.0,
            tube_radius: 0.0,
            smallest_visible_size: 0,
            smallest_visible_dl: 0,
        }
    }

    /// The runtime shape of a parse: the render data is dropped, everything the server can ask is kept.
    pub fn for_server(model: Model) -> Shape {
        let Model { shape, .. } = model;
        shape
    }

    /// The name table's index of `name`, ignoring ASCII case (`TSShape::findName`: names compare without case).
    pub fn find_name(&self, name: &[u8]) -> Option<i32> {
        find_name(&self.names, name)
    }

    /// What the shape takes in memory, counted from its arrays (the figure `MAX_KEPT_SHAPE_BYTES` bounds): every table at its
    /// length times its item, the names, the meshes' own positions, primitives and indices (a mesh that shares its parent's
    /// arrays counts only itself), and each sequence's membership sets. An upper-bound estimate, not an allocator reading.
    pub fn approx_bytes(&self) -> usize {
        use std::mem::size_of;
        fn table<T>(v: &[T]) -> usize {
            v.len().saturating_mul(size_of::<T>())
        }
        let mut total = size_of::<Shape>();
        total = total.saturating_add(
            self.names
                .iter()
                .map(|n| n.len() + size_of::<Vec<u8>>())
                .sum(),
        );
        for part in [
            table(&self.nodes),
            table(&self.objects),
            table(&self.decals),
            table(&self.ifl_materials),
            table(&self.sub_shapes),
            table(&self.details),
            table(&self.default_rotations),
            table(&self.default_translations),
            table(&self.node_rotations),
            table(&self.node_translations),
            table(&self.node_uniform_scales),
            table(&self.node_aligned_scales),
            table(&self.node_arbitrary_scale_rots),
            table(&self.node_arbitrary_scale_factors),
            table(&self.ground_translations),
            table(&self.ground_rotations),
            table(&self.object_states),
            table(&self.decal_states),
            table(&self.triggers),
            table(&self.sequences),
            table(&self.materials),
            table(&self.meshes),
        ] {
            total = total.saturating_add(part);
        }
        for q in &self.sequences {
            for set in [
                &q.rotation_matters,
                &q.translation_matters,
                &q.scale_matters,
                &q.decal_matters,
                &q.ifl_matters,
                &q.vis_matters,
                &q.frame_matters,
                &q.mat_frame_matters,
            ] {
                total = total.saturating_add(table(&set.words));
            }
        }
        for m in self.materials.iter().filter_map(|m| m.name.as_ref()) {
            total = total.saturating_add(m.len());
        }
        for m in self.meshes.iter().flatten() {
            total = total
                .saturating_add(table(&m.primitives))
                .saturating_add(table(&m.indices));
            if m.parent < 0 {
                total = total.saturating_add(table(&m.positions));
            }
        }
        total
    }

    /// Every vertex position of every mesh, the ones shared with a parent counted again (the figure the per-shape cap bounds).
    pub fn vertex_total(&self) -> usize {
        self.meshes.iter().flatten().map(|m| m.num_verts).sum()
    }
}

/// The lengths of the keyframe arrays the sequences of a shape index (see [`check_sequence`]).
#[derive(Debug, Clone, Copy)]
pub struct SeqLens {
    pub rotations: usize,
    pub translations: usize,
    pub uniform: usize,
    pub aligned: usize,
    pub arbitrary: usize,
    pub ground: usize,
    pub triggers: usize,
    pub object_states: usize,
    pub decal_states: usize,
}

/// `base + per_key * keys` entries fit in an array of `len`: Torque indexes a sequence's keyframes as `base + (bits set before
/// the node) * numKeyframes + keyframe` without a check. A sequence that reads none of the array (no bit set, or no keyframe) never
/// uses its base, which exporters then write as -1: any base fits.
fn fits(base: i32, per_key: u64, keys: i32, len: usize) -> bool {
    if keys >= 0 && (per_key == 0 || keys == 0) {
        return true;
    }
    base >= 0 && keys >= 0 && (base as u64) + per_key * (keys as u64) <= len as u64
}

/// Whether every keyframe index a sequence can reach is inside the arrays it indexes (Torque reads whatever
/// memory the index lands on). `states` also checks the object and decal states (a `.dsq` brings none: its base state indices
/// are the shape's own, so an import does not ask).
pub fn check_sequence(s: &Sequence, l: &SeqLens, states: bool) -> Result<(), ShapeError> {
    let bad = ShapeError::Bad("sequence (an index past the keyframes it reads)");
    let k = s.num_keyframes;
    let ok = fits(s.base_rotation, s.rotation_matters.count(), k, l.rotations)
        && fits(
            s.base_translation,
            s.translation_matters.count(),
            k,
            l.translations,
        )
        && match s.flags {
            f if f & SEQ_UNIFORM_SCALE != 0 => {
                fits(s.base_scale, s.scale_matters.count(), k, l.uniform)
            }
            f if f & SEQ_ALIGNED_SCALE != 0 => {
                fits(s.base_scale, s.scale_matters.count(), k, l.aligned)
            }
            f if f & SEQ_ARBITRARY_SCALE != 0 => {
                fits(s.base_scale, s.scale_matters.count(), k, l.arbitrary)
            }
            _ => true,
        }
        && fits(s.first_ground_frame, 1, s.num_ground_frames, l.ground)
        && fits(s.first_trigger, 1, s.num_triggers, l.triggers);
    if !ok {
        return Err(bad);
    }
    if states {
        let objects: u64 = (0..6)
            .map(|i| {
                let w = |set: &IntSet| set.words.get(i).copied().unwrap_or(0);
                u64::from(
                    (w(&s.vis_matters) | w(&s.frame_matters) | w(&s.mat_frame_matters))
                        .count_ones(),
                )
            })
            .sum();
        if !fits(s.base_object_state, objects, k, l.object_states)
            || !fits(
                s.base_decal_state,
                s.decal_matters.count(),
                k,
                l.decal_states,
            )
        {
            return Err(bad);
        }
    }
    Ok(())
}

impl Shape {
    /// The lengths of this shape's keyframe arrays.
    pub fn seq_lens(&self) -> SeqLens {
        SeqLens {
            rotations: self.node_rotations.len(),
            translations: self.node_translations.len(),
            uniform: self.node_uniform_scales.len(),
            aligned: self.node_aligned_scales.len(),
            arbitrary: self.node_arbitrary_scale_factors.len(),
            ground: self.ground_translations.len(),
            triggers: self.triggers.len(),
            object_states: self.object_states.len(),
            decal_states: self.decal_states.len(),
        }
    }

    /// Whether the indices the model keeps point inside it (Torque trusts them): a node's parent, an object's
    /// node and meshes, a primitive's range of the index list, each index against the vertices, and every keyframe index of
    /// each sequence ([`check_sequence`]). Parse ends with it, so a shape that is returned can be read without a bounds check
    /// at these places.
    ///
    /// A skin mesh's indices address its initial vertices, which the runtime shape does not keep: this checks them against the
    /// mesh's own vertices, and [`Shape::validate_with`] takes the count to use.
    pub fn validate(&self) -> Result<(), ShapeError> {
        self.validate_with(|i| self.meshes[i].as_ref().map_or(0, |m| m.num_verts))
    }

    /// [`Shape::validate`], with `indexed(i)` the number of vertices the indices of mesh `i` may address.
    pub fn validate_with(&self, indexed: impl Fn(usize) -> usize) -> Result<(), ShapeError> {
        let bad = |what: &'static str| Err(ShapeError::Bad(what));
        for (i, n) in self.nodes.iter().enumerate() {
            let ok = n.parent == -1
                || (n.parent >= 0
                    && (n.parent as usize) < self.nodes.len()
                    && n.parent as usize != i);
            if !ok {
                return bad("node (a parent outside the nodes)");
            }
        }
        for o in &self.objects {
            if o.node != -1 && !(o.node >= 0 && (o.node as usize) < self.nodes.len()) {
                return bad("object (a node outside the nodes)");
            }
            if o.num_meshes < 0
                || (o.num_meshes > 0
                    && !(o.start_mesh_index >= 0
                        && (o.start_mesh_index as i64) + (o.num_meshes as i64)
                            <= self.meshes.len() as i64))
            {
                return bad("object (meshes outside the mesh list)");
            }
        }
        for (i, m) in self.meshes.iter().enumerate() {
            let Some(m) = m else {
                continue;
            };
            let held = indexed(i);
            for p in &m.primitives {
                if usize::from(p.start) + usize::from(p.num_elements) > m.indices.len() {
                    return bad("mesh (a primitive past the index list)");
                }
            }
            if m.indices.iter().any(|&x| usize::from(x) >= held) {
                return bad("mesh (an index past the vertices)");
            }
        }
        let lens = self.seq_lens();
        for q in &self.sequences {
            check_sequence(q, &lens, true)?;
        }
        Ok(())
    }
}

/// `TSShape::findName` over a name table: the first index whose name equals `name` ignoring ASCII case.
pub fn find_name(names: &[Vec<u8>], name: &[u8]) -> Option<i32> {
    names
        .iter()
        .position(|n| n.eq_ignore_ascii_case(name))
        .and_then(|i| i32::try_from(i).ok())
}

#[cfg(test)]
#[path = "shape_tests.rs"]
mod tests;
