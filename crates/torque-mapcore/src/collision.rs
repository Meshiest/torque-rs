//! What a map collides by: convex hulls, heightfields and triangle meshes, each with its contents.

use crate::budget::{HEIGHTFIELD_SAMPLES, HEIGHTFIELD_SIDE, MAX_SQUARE_SIZE};

/// What a piece of collision is to a mover, a ray and a placed object (bits; a piece may be several).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Contents(pub u32);

impl Contents {
    /// Stops players, items, rays and placed objects.
    pub const SOLID: Contents = Contents(1);
    /// Stops players only.
    pub const PLAYER_CLIP: Contents = Contents(2);
    /// Water (a volume a player swims in).
    pub const WATER: Contents = Contents(4);
    /// A ladder (climbed).
    pub const LADDER: Contents = Contents(8);
    /// A trigger volume (entered, not collided with).
    pub const TRIGGER: Contents = Contents(16);

    pub fn has(self, other: Contents) -> bool {
        self.0 & other.0 == other.0
    }
}

/// A plane `normal . p + d = 0`, as a file gives it (not normalised, not oriented).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub normal: [f32; 3],
    pub d: f32,
}

/// A hull's plane: an index into [`ConvexSet::planes`], negated when `flipped`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaneRef {
    pub index: u32,
    pub flipped: bool,
}

/// One hull: its points are `ConvexSet::point_refs[point_start..point_start + point_count]`, its planes
/// `ConvexSet::plane_refs[plane_start..plane_end]`. The ranges are as the file names them, **not** held to the lists: a consumer
/// checks every one (and charges the whole named length before it refuses an over-long one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HullRange {
    pub point_start: u32,
    pub point_count: u32,
    pub plane_start: u32,
    pub plane_end: u32,
    pub contents: Contents,
}

/// A model's convex hulls over shared points and planes (legacy `.dif`'s hulls; BSP: its brushes, whose points the
/// consumer makes from the planes). The planes face any way: a consumer orients each by the hull's points.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConvexSet {
    pub points: Vec<[f32; 3]>,
    pub planes: Vec<Plane>,
    pub point_refs: Vec<u32>,
    pub plane_refs: Vec<PlaneRef>,
    pub hulls: Vec<HullRange>,
}

/// How a heightfield's squares are cut into two triangles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    /// Square (x, y) is cut from its (x, y) corner to its (x + 1, y + 1) corner when `x ^ y` is even, else from (x + 1, y) to
    /// (x, y + 1) (Torque's grid alternates them).
    Alternating,
}

impl Split {
    /// Whether square (x, y) is cut along the diagonal from its (x, y) corner to its (x + 1, y + 1) corner.
    pub fn main_diagonal(self, x: usize, y: usize) -> bool {
        match self {
            Split::Alternating => (x ^ y) & 1 == 0,
        }
    }
}

/// One material layer of a heightfield: its material as the file names it, and its weight at every sample (0-255, x fastest).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    pub material: String,
    pub weights: Vec<u8>,
}

/// A heightfield: [`HEIGHTFIELD_SIDE`] samples a side, x fastest, wrapping, its heights in the importer's units, its layers in
/// the file's order (a layer may name no material).
#[derive(Debug, Clone, PartialEq)]
pub struct Heightfield {
    pub heights: Vec<f32>,
    pub layers: Vec<Layer>,
    pub split: Split,
}

impl Heightfield {
    /// The height at sample (x, y), the grid wrapping; 0 where the field holds no height.
    pub fn height(&self, x: usize, y: usize) -> f32 {
        let i = (x % HEIGHTFIELD_SIDE) + (y % HEIGHTFIELD_SIDE) * HEIGHTFIELD_SIDE;
        self.heights.get(i).copied().unwrap_or(0.0)
    }

    /// Layer `layer`'s weight at sample index `i` (0 for a layer or sample that is not there).
    pub fn weight(&self, layer: usize, i: usize) -> u8 {
        self.layers
            .get(layer)
            .and_then(|l| l.weights.get(i))
            .copied()
            .unwrap_or(0)
    }

    /// Whether square (x, y) is cut along the diagonal from its (x, y) corner to its (x + 1, y + 1) corner ([`Split`]).
    pub fn main_diagonal(&self, x: usize, y: usize) -> bool {
        self.split.main_diagonal(x, y)
    }
}

/// Where a heightfield lies in the world: sample (0, 0) at `origin`, `square` units a square, repeated every
/// `HEIGHTFIELD_SIDE * square` in x and y when `repeat`, and the squares `holes` names (x fastest) left out.
#[derive(Debug, Clone, PartialEq)]
pub struct HeightfieldPlacement {
    pub origin: [f32; 3],
    /// At least 1 and at most [`MAX_SQUARE_SIZE`].
    pub square: f32,
    pub repeat: bool,
    /// [`HEIGHTFIELD_SAMPLES`] flags.
    pub holes: std::sync::Arc<Vec<bool>>,
}

impl HeightfieldPlacement {
    /// A placement from an importer's values, its square held to 1..[`MAX_SQUARE_SIZE`] and a non-finite height made 0; `holes`
    /// is cut or padded to [`HEIGHTFIELD_SAMPLES`].
    pub fn new(
        origin_xy: [f32; 2],
        z: f32,
        square: i32,
        repeat: bool,
        mut holes: Vec<bool>,
    ) -> HeightfieldPlacement {
        holes.resize(HEIGHTFIELD_SAMPLES, false);
        HeightfieldPlacement {
            origin: [
                origin_xy[0],
                origin_xy[1],
                if z.is_finite() { z } else { 0.0 },
            ],
            square: square.clamp(1, MAX_SQUARE_SIZE) as f32,
            repeat,
            holes: std::sync::Arc::new(holes),
        }
    }
}

/// Triangles that collide (a BSP's displacements; terrain cuts): points, and three indices a triangle, each triangle's
/// contents in `contents` (the same order).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TriMesh {
    pub points: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub contents: Vec<Contents>,
}

#[cfg(test)]
#[path = "collision_tests.rs"]
mod tests;
