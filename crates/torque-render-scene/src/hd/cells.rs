//! The static geometry's cells: meshes of 32-unit cells of the world, each a vertex and index list cut into batches (one a
//! texture slot) and clusters (what the GPU culls and draws), with an extension's own data beside the mesh ([`Cell::ext`]).
//!
//! What builds the cells is the world's own: a [`StaticGeometry`] provider holds them and each slot's texture, and the renderer
//! walks them through it without knowing what they were built from ([`StaticCells`] is a plain set of them).

use std::collections::BTreeMap;

use crate::Sampler;
use crate::hd::HdVertex;

/// A cell's side (units).
pub const CELL_SIZE: f32 = 32.0;

/// One texture slot's batch in a cell: indices `first .. first + count` of the cell's index list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Batch {
    pub slot: u32,
    pub translucent: bool,
    pub first: u32,
    pub count: u32,
}

/// A cell: its key (the cell's grid coordinates), box, vertices, indices and batches (opaque first, by slot), the generation
/// it was built in, and the extension's data `X`.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell<X = ()> {
    pub key: [i32; 3],
    pub bounds: [[f32; 3]; 2],
    pub vertices: Vec<HdVertex>,
    pub indices: Vec<u32>,
    pub batches: Vec<Batch>,
    /// The batches cut into the clusters the GPU culls and draws ([`crate::hd::clusters`]; each batch's quads are ordered by
    /// sub-box for them).
    pub clusters: Vec<crate::hd::clusters::Cluster>,
    pub generation: u64,
    /// The GPU holds this cell's meshes and its CPU vertices and indices may have been dropped after upload: an empty mesh then
    /// is not a removed cell.
    pub resident: bool,
    /// The extension's own data of the cell (what it built beside the mesh).
    pub ext: X,
}

/// The bytes a cell's mesh takes on the GPU.
pub fn mesh_bytes<X>(c: &Cell<X>) -> u64 {
    (c.vertices.len() * crate::hd::HD_VERTEX_BYTES + c.indices.len() * 4) as u64
}

/// The cell a position falls in.
pub fn cell_key(p: [f32; 3]) -> [i32; 3] {
    p.map(|v| (v / CELL_SIZE).floor().clamp(-1e6, 1e6) as i32)
}

/// A texture slot's texture (an index into the frame's [`TextureTable`](crate::hd::TextureTable)) and sampler.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotTexture {
    pub texture: Option<usize>,
    pub sampler: Sampler,
}

/// The static geometry a frame draws, as its provider keeps it: the cells by key (each with the provider's own data `Ext`
/// beside its mesh) and each batch slot's texture.
pub trait StaticGeometry {
    /// What the provider keeps beside each cell's mesh.
    type Ext;

    /// The cells by key ([`cell_key`]).
    fn cells(&self) -> &BTreeMap<[i32; 3], Cell<Self::Ext>>;

    /// Each batch slot's texture.
    fn slots(&self) -> &BTreeMap<u32, SlotTexture>;

    /// Whether changed cells wait for a later build (the frame draws the cells it holds meanwhile).
    fn pending(&self) -> bool {
        false
    }
}

/// A plain set of cells and slots, built by whoever fills them.
#[derive(Debug, Clone, PartialEq)]
pub struct StaticCells<X = ()> {
    pub cells: BTreeMap<[i32; 3], Cell<X>>,
    pub slots: BTreeMap<u32, SlotTexture>,
}

impl<X> Default for StaticCells<X> {
    fn default() -> StaticCells<X> {
        StaticCells {
            cells: BTreeMap::new(),
            slots: BTreeMap::new(),
        }
    }
}

impl<X> StaticGeometry for StaticCells<X> {
    type Ext = X;

    fn cells(&self) -> &BTreeMap<[i32; 3], Cell<X>> {
        &self.cells
    }

    fn slots(&self) -> &BTreeMap<u32, SlotTexture> {
        &self.slots
    }
}
