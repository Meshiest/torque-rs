//! A `TerrainBlock` over a legacy `.ter` drawn in the world view, and as a static object of the client's prediction. Its
//! collision and the server's class are in `torque-app-server` ([`install`], [`TERRAIN_TYPE_MASK`]); this module adds what
//! needs the drawing and the client.
//!
//! - **The block** ([`Terrain`]): the server crate's block (it dereferences to it) with the drawing methods.
//! - **On the client** ([`TerrainSet`]): a game's static index ([`torque_client::predict::StaticIndex`]) offers the block to
//!   the prediction as a static object (the client is not sent a TerrainBlock: it reads the same file), so the client's own
//!   Player and the vehicles it runs meet the ground the server's do.
//! - **Drawn** ([`Terrain::draws`]): the block in square chunks of [`CHUNK`] squares, smooth normals from the heightfield, one
//!   texture repeated every `tile` units in x and y (the world view's [`crate::WorldView::set_terrain`]). Or each of its
//!   materials ([`Terrain::layer_draws`], [`crate::WorldView::set_terrain_layers`]): the first opaque, each other one a layer
//!   blended over it where its weight is (its vertex alpha its share of the weights so far, as TGE's blended textures sum
//!   them), each with its own texture and texture coordinates (repeated over x and y, or a host's own, such as a road's along
//!   it).
//!
//! Credit: the placement and the type bits follow `TerrainBlock::onAdd` in TGE 1.4's `terrain/terrData.cc` (Copyright (C)
//! GarageGames.com, Inc.) as Torque3D carries it under the MIT license (`Engine/source/terrain/terrData.cpp`, Copyright (c)
//! 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this crate's own.

use std::ops::Deref;
use std::rc::Rc;

use torque_render_scene::{Blend, Cull, Draw, Filter, Sampler, TexEnv, Vertex};

use torque_client::predict::{GameSource, Object, Source, StaticSet};
use torque_collide::convex::{ConvexSource, ObjectView};
use torque_collide::map::MapShape;
use torque_collide::math::F3;
use torque_collide::{Exhausted, ObjectId};
use torque_maps::ter;

pub use torque_app_server::terrain::{DEFAULT_SQUARE_SIZE, TERRAIN_TYPE_MASK, install};

/// The squares on a side of one drawn chunk.
pub const CHUNK: usize = 32;

/// A terrain block: the server crate's (its heightfield placed as TGE places it) with the drawing methods.
#[derive(Debug, Clone)]
pub struct Terrain(torque_app_server::terrain::Terrain);

impl Deref for Terrain {
    type Target = torque_app_server::terrain::Terrain;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A host's texture coordinates for a triangle's corners (in the world, in drawing order).
pub type TriangleUv = dyn Fn(&[[f32; 3]; 3]) -> [[f32; 2]; 3];

/// A material's texture coordinates as [`Terrain::layer_draws`] makes them.
#[derive(Clone, Copy)]
pub enum LayerUv<'a> {
    /// One repeat every this many units in x and y.
    Tiled(f32),
    /// The host's own for each triangle's corners (in the world, in drawing order).
    Mapped(&'a TriangleUv),
}

impl std::fmt::Debug for LayerUv<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayerUv::Tiled(t) => f.debug_tuple("Tiled").field(t).finish(),
            LayerUv::Mapped(_) => f.write_str("Mapped"),
        }
    }
}

/// How one of the terrain's materials is drawn: its texture (the scene's index, if any) and coordinates.
#[derive(Debug, Clone, Copy)]
pub struct LayerLook<'a> {
    pub texture: Option<usize>,
    pub uv: LayerUv<'a>,
}

/// The sampler a terrain's textures are drawn with.
const SAMPLER: Sampler = Sampler {
    repeat: true,
    mag: Filter::Linear,
    min: Filter::Linear,
    mip: Filter::Linear,
    mipmaps: true,
};

impl Terrain {
    /// The block of the `.ter` file `bytes`, `square_size` units a square.
    pub fn read(bytes: &[u8], square_size: i32) -> Result<Terrain, String> {
        torque_app_server::terrain::Terrain::read(bytes, square_size).map(Terrain)
    }

    fn block(&self) -> &torque_collide::map::TerrainShape {
        self.0.block()
    }

    /// The ground's normal at sample (x, y) of the grid (central differences, the grid clamped at the block's edges).
    fn normal(&self, x: i64, y: i64) -> [f32; 3] {
        let b = self.block();
        let side = ter::SIZE as i64;
        let h = |x: i64, y: i64| b.corner(x.clamp(0, side), y.clamp(0, side))[2];
        let dx = h(x + 1, y) - h(x - 1, y);
        let dy = h(x, y + 1) - h(x, y - 1);
        let n = [-dx, -dy, 2.0 * b.square()];
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
        n.map(|v| v / l)
    }

    /// The block drawn: one draw a chunk of [`CHUNK`] x [`CHUNK`] squares, lit, the scene's texture `texture` (if any) repeated
    /// every `tile` units in x and y, each square cut as its collision is.
    pub fn draws(&self, texture: Option<usize>, tile: f32) -> Vec<Draw> {
        self.chunks(0, &|_| 1.0, texture, LayerUv::Tiled(tile))
    }

    /// Each of the block's materials drawn (module docs), `looks[k]` the look of material `k` (the materials past the looks
    /// are left out): the first as [`Terrain::draws`] makes it with its look, then each other one's chunks blended over it
    /// where its weight is, each a draw of a chunk's squares with any weight of it at a corner: its vertex alpha its share of
    /// the weights of the materials up to it (`w_k / (w_0 + ... + w_k)`), depth tested and not written.
    pub fn layer_draws(&self, looks: &[LayerLook<'_>]) -> Vec<Draw> {
        let mut out = Vec::new();
        for (k, look) in looks.iter().enumerate().take(self.materials().len().max(1)) {
            if k == 0 {
                out.extend(self.chunks(0, &|_| 1.0, look.texture, look.uv));
                continue;
            }
            let share = |[x, y]: [i64; 2]| {
                let sum: u32 = (0..=k).map(|j| u32::from(self.weight(j, x, y))).sum();
                if sum == 0 {
                    0.0
                } else {
                    f32::from(self.weight(k, x, y)) / sum as f32
                }
            };
            out.extend(self.chunks(k, &share, look.texture, look.uv));
        }
        out
    }

    /// The chunk draws of layer `k` (0 opaque, the others blended), `share` a sample's vertex alpha (0 to 1; a square whose
    /// corners are all 0 is left out of a blended layer, and so is a chunk with none).
    fn chunks(
        &self,
        k: usize,
        share: &dyn Fn([i64; 2]) -> f32,
        texture: Option<usize>,
        uv: LayerUv<'_>,
    ) -> Vec<Draw> {
        let b = self.block();
        let blended = k > 0;
        let chunks = ter::SIZE / CHUNK;
        let mut out = Vec::with_capacity(chunks * chunks);
        for cy in 0..chunks {
            for cx in 0..chunks {
                let mut vertices = Vec::with_capacity(if blended { 0 } else { CHUNK * CHUNK * 6 });
                let corner = |x: i64, y: i64| {
                    let p = b.corner(x, y);
                    let alpha = (share([x, y]).clamp(0.0, 1.0) * 255.0).round() as u8;
                    Vertex {
                        position: p,
                        normal: self.normal(x, y),
                        color: [255, 255, 255, alpha],
                        uv: match uv {
                            LayerUv::Tiled(tile) => [p[0] / tile, -p[1] / tile],
                            LayerUv::Mapped(_) => [0.0; 2],
                        },
                    }
                };
                for y in (cy * CHUNK) as i64..((cy + 1) * CHUNK) as i64 {
                    for x in (cx * CHUNK) as i64..((cx + 1) * CHUNK) as i64 {
                        if !b.has_square(x, y) {
                            continue;
                        }
                        let (a, bb, c, d) = (
                            corner(x, y),
                            corner(x + 1, y),
                            corner(x + 1, y + 1),
                            corner(x, y + 1),
                        );
                        if blended && [a, bb, c, d].iter().all(|v| v.color[3] == 0) {
                            continue;
                        }
                        // Clockwise seen from above, each square cut along the diagonal its collision is cut along.
                        let tris = if ter::split45(x as usize, y as usize) {
                            [[a, c, bb], [a, d, c]]
                        } else {
                            [[a, d, bb], [bb, d, c]]
                        };
                        for mut t in tris {
                            if let LayerUv::Mapped(map) = uv {
                                let uvs = map(&t.map(|v| v.position));
                                for (v, uv) in t.iter_mut().zip(uvs) {
                                    v.uv = uv;
                                }
                            }
                            vertices.extend(t);
                        }
                    }
                }
                if blended && vertices.is_empty() {
                    continue;
                }
                out.push(Draw {
                    label: if blended {
                        format!("terrain layer {k} chunk {cx} {cy}")
                    } else {
                        format!("terrain chunk {cx} {cy}")
                    },
                    vertices,
                    lighting: true,
                    tex_env: if texture.is_some() {
                        TexEnv::Modulate
                    } else {
                        TexEnv::None
                    },
                    texture: texture.map(|i| (i, SAMPLER)),
                    blend: if blended { Blend::Alpha } else { Blend::Opaque },
                    cull: Cull::None,
                    depth_test: true,
                    depth_write: !blended,
                    fog: true,
                    lights: Vec::new(),
                    vertex_key: None,
                });
            }
        }
        out
    }
}

// ---- the client

/// The client's id of the `k`th static map object (above any ghost's id).
pub fn static_id(k: u32) -> ObjectId {
    ObjectId(0x7f00_0000 | k)
}

/// A terrain as a static object of the client's container: the container's convexes come from its [`MapShape`]; a ray asks it
/// in its space (the world's: it is placed by the identity).
#[derive(Debug)]
struct TerrainSource(Rc<MapShape>);

impl GameSource for TerrainSource {
    fn convex(&self) -> ConvexSource {
        ConvexSource::Map
    }

    fn cast_ray(&self, start: F3, end: F3, _first: bool) -> Option<(f32, F3)> {
        self.0.cast_ray(start, end, &mut |_| true).ok().flatten()
    }
}

/// The map objects a game's static index offers the client's prediction (module docs): every one whose box a query reaches.
#[derive(Debug, Clone, Default)]
pub struct TerrainSet {
    objects: Vec<(ObjectId, Object)>,
}

impl TerrainSet {
    /// The set of `terrains`.
    pub fn new(terrains: &[Terrain]) -> TerrainSet {
        let objects = terrains
            .iter()
            .enumerate()
            .map(|(k, t)| {
                let shape = t.shape();
                (
                    static_id(k as u32),
                    Object {
                        view: t.view(),
                        source: Source::Game(Rc::new(TerrainSource(Rc::clone(&shape)))),
                        shape: None,
                        map: Some(shape),
                    },
                )
            })
            .collect();
        TerrainSet { objects }
    }

    /// Whether the set holds no object.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

impl StaticSet for TerrainSet {
    fn near(&self, lo: F3, hi: F3, work: &mut u64, budget: u64) -> Result<Vec<u32>, Exhausted> {
        let mut out = Vec::new();
        for (i, (_, o)) in self.objects.iter().enumerate() {
            *work = work.saturating_add(1);
            if *work > budget {
                return Err(Exhausted);
            }
            let w = &o.view.world_box;
            if (0..3).all(|k| lo[k] <= w.max[k] && hi[k] >= w.min[k]) {
                out.push(i as u32);
            }
        }
        Ok(out)
    }

    fn get(&self, i: u32) -> &(ObjectId, Object) {
        &self.objects[i as usize]
    }

    fn find(&self, id: ObjectId) -> Option<u32> {
        self.objects
            .iter()
            .position(|(o, _)| *o == id)
            .map(|i| i as u32)
    }

    fn set_view(&mut self, i: u32, view: ObjectView) {
        if let Some((_, o)) = self.objects.get_mut(i as usize) {
            o.view = view;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat version-2 `.ter` of `grass` and `dirt`, the dirt weighing `w` at samples (10..=12, 20..=21) and the grass the rest.
    fn two_materials(w: u8) -> Vec<u8> {
        let mut out = vec![2u8];
        out.extend(std::iter::repeat_n([0u8, 2], ter::SAMPLES).flatten());
        out.extend(std::iter::repeat_n(0u8, ter::SAMPLES));
        for name in [&b"grass"[..], b"dirt"] {
            out.push(name.len() as u8);
            out.extend(name);
        }
        out.extend([0u8; 6]);
        let dirt: Vec<u8> = (0..ter::SAMPLES)
            .map(|i| {
                let (x, y) = (i % ter::SIZE, i / ter::SIZE);
                if (10..=12).contains(&x) && (20..=21).contains(&y) {
                    w
                } else {
                    0
                }
            })
            .collect();
        out.extend(dirt.iter().map(|&d| 255 - d));
        out.extend(dirt);
        out
    }

    #[test]
    fn a_second_material_is_blended_over_the_first_only_where_it_weighs() {
        let t = Terrain::read(&two_materials(204), 8).unwrap();
        assert_eq!(t.materials(), ["grass", "dirt"]);
        let uv = |c: &[[f32; 3]; 3]| c.map(|p| [p[0], p[1]]);
        let looks = [
            LayerLook {
                texture: Some(3),
                uv: LayerUv::Tiled(8.0),
            },
            LayerLook {
                texture: Some(4),
                uv: LayerUv::Mapped(&uv),
            },
        ];
        let draws = t.layer_draws(&looks);
        // The first material as the one-texture ground is.
        let plain = t.draws(Some(3), 8.0);
        assert_eq!(&draws[..plain.len()], &plain[..]);
        // Then one blended chunk: the squares with a corner the dirt weighs, its share in the alpha.
        let layer = &draws[plain.len()..];
        assert_eq!(layer.len(), 1);
        let d = &layer[0];
        assert_eq!(
            (d.blend, d.depth_write, d.texture.map(|t| t.0)),
            (Blend::Alpha, false, Some(4))
        );
        // Samples 9..=13 by 19..=22 hold the squares 9..=12 by 19..=21.
        assert_eq!(d.vertices.len(), 4 * 3 * 6);
        let at = |v: &Vertex| {
            let o = (ter::SIZE / 2) as f32 * 8.0;
            (
                ((v.position[0] + o) / 8.0).round() as usize,
                ((v.position[1] + o) / 8.0).round() as usize,
            )
        };
        for v in &d.vertices {
            let (x, y) = at(v);
            let inside = (10..=12).contains(&x) && (20..=21).contains(&y);
            assert_eq!(v.color[3], if inside { 204 } else { 0 }, "{x} {y}");
            assert_eq!(
                v.uv,
                [v.position[0], v.position[1]],
                "the host's coordinates"
            );
        }
    }
}
