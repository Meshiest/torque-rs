//! A model's drawn geometry: its triangles by material in the model's own space, and where each triangle lies in the model's
//! baked lightmap pages (the rectangles the relight replaces, or a format's own bake).

use std::collections::BTreeMap;

use crate::material::EncodedImage;

/// A triangle corner in the model's space: position, normal, texture UV.
pub type Corner = ([f32; 3], [f32; 3], [f32; 2]);

/// No lightmap rectangle ([`LightmapTri::rect`]).
pub const NO_RECT: u32 = u32::MAX;

/// A triangle's lightmap: its rectangle (an index into [`Surfaces::rects`], or [`NO_RECT`]) and its corners' texel coordinates in
/// that rectangle's page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightmapTri {
    pub rect: u32,
    pub texel: [[f32; 2]; 3],
}

/// A lightmap rectangle of a model's page and the polygons drawn from it (several when the bake shared it between coplanar
/// surfaces).
#[derive(Debug, Clone, PartialEq)]
pub struct LightRect {
    /// The page's index in [`Surfaces::lightmaps`].
    pub lightmap: u32,
    /// Its corner and size in the page, in texels.
    pub offset: [u32; 2],
    pub size: [u32; 2],
    /// A point's texel coordinates: `p[axes[k]] * scale[k] + add[k]`.
    pub axes: [usize; 2],
    pub scale: [f32; 2],
    pub add: [f32; 2],
    /// The first polygon's plane (`normal . p + d = 0`), facing its front.
    pub normal: [f32; 3],
    pub d: f32,
    /// Each polygon's points, in its own order.
    pub polys: Vec<Vec<[f32; 3]>>,
}

/// A model's drawn triangles in its own space, by material name: three [`Corner`]s a triangle, and each triangle's lightmap in
/// `lightmap_tris` (the same order); the lightmap rectangles and the model's baked pages (still encoded).
///
/// A material named here is drawn: an importer leaves out its format's surfaces that are never drawn (editor-only materials).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Surfaces {
    pub by_material: BTreeMap<String, Vec<Corner>>,
    pub lightmap_tris: BTreeMap<String, Vec<LightmapTri>>,
    pub rects: Vec<LightRect>,
    pub lightmaps: Vec<EncodedImage>,
    /// The model's box in its own space (min, max), as its file gives it.
    pub bounds: ([f32; 3], [f32; 3]),
    /// The file's identity (a hash of its bytes), set by whoever read it: the consumers' caches key on it.
    pub content: u64,
}

/// An importer's space to the world's: `world = scale * (axes * p)`, with `axes` a signed permutation (row `k` names the source
/// axis of world axis `k` and its sign). The legacy format is the identity; BSP is a uniform scale with no swap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub scale: f32,
    pub axes: [(usize, f32); 3],
}

impl Transform {
    /// No change.
    pub const IDENTITY: Transform = Transform {
        scale: 1.0,
        axes: [(0, 1.0), (1, 1.0), (2, 1.0)],
    };

    /// A point in the world.
    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        self.axes.map(|(a, s)| p[a.min(2)] * s * self.scale)
    }

    /// A direction in the world (not scaled).
    pub fn direction(&self, v: [f32; 3]) -> [f32; 3] {
        self.axes.map(|(a, s)| v[a.min(2)] * s)
    }
}

#[cfg(test)]
#[path = "geometry_tests.rs"]
mod tests;
