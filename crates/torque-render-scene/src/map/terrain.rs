//! A map's terrain (`TerrainBlock` over a legacy `.ter`; the engine's own reading of TGE's look).
//!
//! **Placement.** The importer's heightfield ([`mapcore::collision::Heightfield`]) at the record's placement
//! ([`mapcore::collision::HeightfieldPlacement`], `maps::legacy::terrain_placement`: at height 0 as `TerrainBlock::onAdd` puts
//! it, or at the mission's height with [`super::MapProfile::terrain_at_mission_height`]): sample (x, y) at
//! its origin plus `(x, y) * squareSize`, at its height plus the object's z; with `RepeatTerrain` the block repeats every `256 * squareSize`
//! metres in x and y (the heights wrap). Each square is cut by the field's [`mapcore::collision::Split`]; the squares
//! `emptySquares` names are holes.
//!
//! **Drawing.** The block is cut into chunks of [`CHUNK`] x [`CHUNK`] squares, one mesh a chunk and material layer, made when a
//! chunk first comes within the visible distance and kept ([`MAX_KEPT_CHUNKS`]). The first material whose texture is found is
//! opaque over every square; each next one is drawn over it (alpha blending, depth equal) on the squares it weighs anything on,
//! with vertex alpha `w_k / (w_0 + ... + w_k)`: the layers then sum to each sample's weights in turn, as TGE's blended texture
//! does. A material whose texture is found nowhere (the name, then the file name beside the `.ter`) is left out, the others'
//! weights filling its squares. The textures repeat every [`TEXTURE_METRES`]. The layers are drawn with the opaque objects,
//! right after their chunk's first.
//!
//! **Lighting.** With the sun's relight ([`super::light`]) the layers are unlit and a last pass a chunk multiplies them by the
//! lightmap of the chunk's tile (a repeated block's tiles without one of their own share its terrain-only lightmap); until
//! then, GL lighting with the heightfield's normals.
//!
//! Credit: follows TGE 1.4's `terrain/terrData.cc` and `terrain/terrRender.cc` (Copyright (C) GarageGames.com, Inc.); the code
//! here is this crate's own.

use std::collections::HashMap;
use std::sync::Arc;

use mapcore::budget::HEIGHTFIELD_SIDE as SIZE;
use mapcore::collision::{Heightfield as Terrain, HeightfieldPlacement};

use super::{Fnv, Mesh, Object, TexRef, find_texture, folder_of};
use crate::textures::TextureSet;
use crate::{Assets, Blend, Cull, TexEnv, Vertex};

/// Squares on a side of one chunk mesh.
pub const CHUNK: usize = 32;
/// The sampler of a terrain lightmap: repeating with the block, linear, level 0 only.
pub const LIGHTMAP_SAMPLER: crate::Sampler = crate::Sampler {
    repeat: true,
    ..super::interior::LIGHTMAP_SAMPLER
};
/// Chunks on a side of a block.
pub const CHUNKS: usize = SIZE / CHUNK;
/// Metres one repeat of a material's texture covers (TGE's blender is not followed).
pub const TEXTURE_METRES: f32 = 32.0;
/// The most chunk meshes kept between builds (all are let go past it; a block has 64 a tile).
pub const MAX_KEPT_CHUNKS: usize = 1024;
/// The most tiles of a repeated block drawn each way from the one under the eye.
pub const MAX_TILE_REACH: i32 = 4;

/// A chunk of a block: its tile and its place in the tile.
type ChunkId = ([i32; 2], [usize; 2]);

/// A lit block's lightmaps.
pub(crate) struct Light {
    pub tiles: Vec<([i32; 2], Arc<crate::Texture>)>,
    pub rest: Option<Arc<crate::Texture>>,
    pub key: u64,
}

impl Light {
    fn of(&self, tile: [i32; 2]) -> Option<&Arc<crate::Texture>> {
        self.tiles
            .iter()
            .find(|(t, _)| *t == tile)
            .map(|(_, l)| l)
            .or(self.rest.as_ref())
    }
}

/// A loaded terrain block.
pub(crate) struct Block {
    ter: Arc<Terrain>,
    index: usize,
    origin: [f32; 3],
    square: f32,
    repeat: bool,
    empty: Arc<Vec<bool>>,
    /// The materials drawn, in order: (material index, texture name).
    layers: Vec<(usize, String)>,
    chunks: HashMap<ChunkId, Arc<Vec<Mesh>>>,
    /// The `.ter` file's bytes' hash.
    hash: u64,
    /// The block's lightmaps (by tile, and the one for every other tile) and the relight's key, once lit.
    light: Option<Light>,
}

/// Where block `o` lies (the legacy importer's placement of its record's fields): at height 0 (`TerrainBlock::onAdd`), or at
/// the height its mission wrote when the profile says so.
fn placement(o: &Object, profile: &super::MapProfile) -> HeightfieldPlacement {
    let mut p = maps::legacy::terrain_placement(
        o.int("squareSize"),
        o.flag("RepeatTerrain"),
        &o.ints("emptySquares"),
    );
    let z = o.position()[2];
    if profile.terrain_at_mission_height && z.is_finite() {
        p.origin[2] = z;
    }
    p
}

/// The height of block `o` over `ter` at (x, y), None off a block that does not repeat or over a hole.
pub(crate) fn height_at(
    ter: &Terrain,
    o: &Object,
    profile: &super::MapProfile,
    x: f32,
    y: f32,
) -> Option<f32> {
    let p = placement(o, profile);
    sample(ter, p.origin, p.square, p.repeat, &p.holes, x, y).map(|s| s.0)
}

/// The surface of a block at (x, y): its height and the normal of the triangle there.
fn sample(
    ter: &Terrain,
    origin: [f32; 3],
    square: f32,
    repeat: bool,
    empty: &[bool],
    x: f32,
    y: f32,
) -> Option<(f32, [f32; 3])> {
    let gx = (x - origin[0]) / square;
    let gy = (y - origin[1]) / square;
    let span = SIZE as f32;
    if !gx.is_finite() || !gy.is_finite() {
        return None;
    }
    let inside = (0.0..span).contains(&gx) && (0.0..span).contains(&gy);
    if !repeat && !inside {
        return None;
    }
    let (ix, iy) = (gx.floor(), gy.floor());
    let (fx, fy) = (gx - ix, gy - iy);
    let wrap = |v: f32| (v as i64).rem_euclid(SIZE as i64) as usize;
    let (x0, y0) = (wrap(ix), wrap(iy));
    if empty.get(x0 + y0 * SIZE).copied().unwrap_or(true) {
        return None;
    }
    let h = |dx: usize, dy: usize| ter.height(x0 + dx, y0 + dy);
    // z = base + fx * a + fy * b over the triangle (x, y) falls in.
    let (z, a, b) = if ter.main_diagonal(x0, y0) {
        if fx > fy {
            let (a, b) = (h(1, 0) - h(0, 0), h(1, 1) - h(1, 0));
            (h(0, 0) + fx * a + fy * b, a, b)
        } else {
            let (a, b) = (h(1, 1) - h(0, 1), h(0, 1) - h(0, 0));
            (h(0, 0) + fy * b + fx * a, a, b)
        }
    } else if fx + fy < 1.0 {
        let (a, b) = (h(1, 0) - h(0, 0), h(0, 1) - h(0, 0));
        (h(0, 0) + fx * a + fy * b, a, b)
    } else {
        let z = h(1, 1) + (1.0 - fx) * (h(0, 1) - h(1, 1)) + (1.0 - fy) * (h(1, 0) - h(1, 1));
        (z, h(1, 1) - h(0, 1), h(1, 1) - h(1, 0))
    };
    let n = normalize([-a / square, -b / square, 1.0]);
    Some((z + origin[2], n))
}

impl Block {
    /// The block's surface at (x, y): its height and normal (None off it or over a hole).
    pub(crate) fn surface(&self, x: f32, y: f32) -> Option<(f32, [f32; 3])> {
        sample(
            &self.ter,
            self.origin,
            self.square,
            self.repeat,
            &self.empty,
            x,
            y,
        )
    }

    /// Reads the block's `.ter` and finds its materials' textures.
    pub(crate) fn load(
        o: &Object,
        profile: &super::MapProfile,
        assets: &dyn Assets,
        textures: &mut TextureSet<'_>,
    ) -> Result<Block, String> {
        let file = o.text("terrainFile");
        let bytes = assets.read(&file).ok_or(format!("{file}: not found"))?;
        let ter = profile
            .importer
            .heightfield(&bytes)
            .map_err(|e| format!("{file}: {e}"))?;
        let folder = folder_of(&file).to_string();
        let mut layers = Vec::new();
        for (k, m) in ter.layers.iter().enumerate() {
            if let Some((_, name)) =
                find_texture(textures, &m.material.replace('\\', "/"), &[&folder])
            {
                layers.push((k, name));
            }
        }
        let place = placement(o, profile);
        let mut h = Fnv::new("ter");
        h.bytes(&bytes);
        Ok(Block {
            ter: Arc::new(ter),
            index: 0,
            origin: place.origin,
            square: place.square,
            repeat: place.repeat,
            empty: place.holes,
            layers,
            chunks: HashMap::new(),
            hash: h.0,
            light: None,
        })
    }

    /// The light's view of the block.
    pub(crate) fn light_input(&self) -> super::light::TerrainIn {
        super::light::TerrainIn {
            ter: Arc::clone(&self.ter),
            origin: self.origin,
            square: self.square,
            repeat: self.repeat,
            empty: Arc::clone(&self.empty),
            hash: self.hash,
        }
    }

    /// Draws the block with its lightmaps from now on.
    pub(crate) fn set_light(&mut self, light: Light) {
        self.light = Some(light);
        self.chunks.clear();
    }

    /// The block's height at sample (gx, gy) of the infinite grid (wrapped).
    fn h(&self, gx: i64, gy: i64) -> f32 {
        let s = SIZE as i64;
        self.ter
            .height(gx.rem_euclid(s) as usize, gy.rem_euclid(s) as usize)
    }

    fn weight(&self, material: usize, gx: i64, gy: i64) -> f32 {
        let s = SIZE as i64;
        let i = gx.rem_euclid(s) as usize + gy.rem_euclid(s) as usize * SIZE;
        f32::from(self.ter.weight(material, i))
    }

    /// The meshes of chunk `c` of tile `t`.
    fn chunk(&self, t: [i32; 2], c: [usize; 2], key: u64) -> Vec<Mesh> {
        let s = SIZE as i64;
        let x0 = i64::from(t[0]) * s + (c[0] * CHUNK) as i64;
        let y0 = i64::from(t[1]) * s + (c[1] * CHUNK) as i64;
        let sq = self.square;
        let vertex = |gx: i64, gy: i64, alpha: u8| {
            let p = [
                self.origin[0] + gx as f32 * sq,
                self.origin[1] + gy as f32 * sq,
                self.origin[2] + self.h(gx, gy),
            ];
            let n = normalize([
                (self.h(gx - 1, gy) - self.h(gx + 1, gy)) / (2.0 * sq),
                (self.h(gx, gy - 1) - self.h(gx, gy + 1)) / (2.0 * sq),
                1.0,
            ]);
            Vertex {
                position: p,
                normal: n,
                color: [255, 255, 255, alpha],
                uv: [
                    gx as f32 * sq / TEXTURE_METRES,
                    gy as f32 * sq / TEXTURE_METRES,
                ],
            }
        };
        let mut out = Vec::new();
        let lit = self.light.as_ref().and_then(|l| {
            l.of(if self.repeat { t } else { [0, 0] })
                .map(|m| (m, l.key))
        });
        let light_key = lit.map_or(0, |l| l.1);
        let centre = [
            self.origin[0] + (x0 as f32 + CHUNK as f32 / 2.0) * sq,
            self.origin[1] + (y0 as f32 + CHUNK as f32 / 2.0) * sq,
            self.origin[2] + self.h(x0 + CHUNK as i64 / 2, y0 + CHUNK as i64 / 2),
        ];
        for (li, (material, name)) in self.layers.iter().enumerate() {
            let earlier: Vec<usize> = self.layers[..=li].iter().map(|(m, _)| *m).collect();
            // The layer's share of the weights so far at a sample (the first layer is opaque).
            let share = |gx: i64, gy: i64| -> u8 {
                if li == 0 {
                    return 255;
                }
                let w = self.weight(*material, gx, gy);
                let total: f32 = earlier.iter().map(|&m| self.weight(m, gx, gy)).sum();
                if total <= 0.0 {
                    0
                } else {
                    (w / total * 255.0).round().clamp(0.0, 255.0) as u8
                }
            };
            let mut vertices = Vec::new();
            for y in y0..y0 + CHUNK as i64 {
                for x in x0..x0 + CHUNK as i64 {
                    let (lx, ly) = (x.rem_euclid(s) as usize, y.rem_euclid(s) as usize);
                    if self.empty[lx + ly * SIZE] {
                        continue;
                    }
                    let corners = [(x, y), (x + 1, y), (x + 1, y + 1), (x, y + 1)];
                    let a = corners.map(|(gx, gy)| share(gx, gy));
                    if li > 0 && a.iter().all(|&v| v == 0) {
                        continue;
                    }
                    let v = [0, 1, 2, 3].map(|k| vertex(corners[k].0, corners[k].1, a[k]));
                    // Clockwise seen from above (the fixed-function renderer's front).
                    if self.ter.main_diagonal(lx, ly) {
                        vertices.extend_from_slice(&[v[0], v[2], v[1], v[0], v[3], v[2]]);
                    } else {
                        vertices.extend_from_slice(&[v[0], v[3], v[1], v[1], v[3], v[2]]);
                    }
                }
            }
            let mut h = Fnv::new("map terrain chunk");
            h.u64(key);
            h.u64(light_key);
            h.u64(self.index as u64);
            h.u64(t[0] as u32 as u64);
            h.u64(t[1] as u32 as u64);
            h.u64(c[0] as u64);
            h.u64(c[1] as u64);
            h.u64(li as u64);
            out.push(Mesh {
                label: format!("terrain {name} {} {} {} {}", t[0], t[1], c[0], c[1]),
                texture: TexRef::Name(name.clone()),
                vertices: Arc::new(vertices),
                lighting: lit.is_none(),
                tex_env: TexEnv::Modulate,
                blend: if li == 0 { Blend::Opaque } else { Blend::Alpha },
                cull: Cull::BackCw,
                depth_write: li == 0,
                sampler: super::SAMPLER,
                alpha_from_texture: false,
                overlay: true,
                centre,
                key: h.0,
            });
        }
        if let Some((tex, lk)) = lit
            && !self.layers.is_empty()
        {
            let mut vertices = Vec::new();
            let lm = |gx: i64, gy: i64| {
                let mut v = vertex(gx, gy, 255);
                v.uv = [gx as f32 / s as f32, gy as f32 / s as f32];
                v
            };
            for y in y0..y0 + CHUNK as i64 {
                for x in x0..x0 + CHUNK as i64 {
                    let (lx, ly) = (x.rem_euclid(s) as usize, y.rem_euclid(s) as usize);
                    if self.empty[lx + ly * SIZE] {
                        continue;
                    }
                    let v = [(x, y), (x + 1, y), (x + 1, y + 1), (x, y + 1)].map(|(a, b)| lm(a, b));
                    if self.ter.main_diagonal(lx, ly) {
                        vertices.extend_from_slice(&[v[0], v[2], v[1], v[0], v[3], v[2]]);
                    } else {
                        vertices.extend_from_slice(&[v[0], v[3], v[1], v[1], v[3], v[2]]);
                    }
                }
            }
            let mut h = Fnv::new("map terrain lightmap");
            h.u64(key);
            h.u64(lk);
            h.u64(self.index as u64);
            h.u64(t[0] as u32 as u64);
            h.u64(t[1] as u32 as u64);
            h.u64(c[0] as u64);
            h.u64(c[1] as u64);
            out.push(Mesh {
                label: format!("terrain lightmap {} {} {} {}", t[0], t[1], c[0], c[1]),
                texture: TexRef::Generated(Arc::clone(tex)),
                vertices: Arc::new(vertices),
                lighting: false,
                tex_env: TexEnv::Replace,
                blend: Blend::Multiply,
                cull: Cull::BackCw,
                depth_write: false,
                sampler: LIGHTMAP_SAMPLER,
                alpha_from_texture: false,
                overlay: true,
                centre,
                key: h.0,
            });
        }
        out
    }
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 0.0 {
        v.map(|c| c / l)
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// The chunk meshes within `visible` of `eye` over every block (made on first use and kept), the opaque layer of a chunk first.
pub(crate) fn visible_chunks(
    blocks: &mut [Block],
    eye: [f32; 3],
    visible: f32,
    key: u64,
) -> Vec<Arc<Vec<Mesh>>> {
    let mut out = Vec::new();
    for (index, b) in blocks.iter_mut().enumerate() {
        b.index = index;
        if b.layers.is_empty() {
            continue;
        }
        let span = SIZE as f32 * b.square;
        let chunk_span = CHUNK as f32 * b.square;
        let reach = visible.max(0.0);
        let tile = |v: f32, o: f32| ((v - o) / span).floor() as i32;
        let (tx, ty) = (tile(eye[0], b.origin[0]), tile(eye[1], b.origin[1]));
        let tiles: Vec<[i32; 2]> = if b.repeat {
            let r = ((reach / span).ceil() as i32 + 1).min(MAX_TILE_REACH);
            (-r..=r)
                .flat_map(|dy| (-r..=r).map(move |dx| [tx + dx, ty + dy]))
                .collect()
        } else {
            vec![[0, 0]]
        };
        // The kept chunks never pass the cap: a full cache is let go, and no chunk past the cap is made this build (a map's
        // `squareSize` and repeat set how many chunks lie in reach, so the work a build does is held here, not by the map).
        if b.chunks.len() >= MAX_KEPT_CHUNKS {
            b.chunks.clear();
        }
        for t in tiles {
            for cy in 0..CHUNKS {
                for cx in 0..CHUNKS {
                    let x0 = b.origin[0] + t[0] as f32 * span + cx as f32 * chunk_span;
                    let y0 = b.origin[1] + t[1] as f32 * span + cy as f32 * chunk_span;
                    let dx = (x0 - eye[0]).max(eye[0] - (x0 + chunk_span)).max(0.0);
                    let dy = (y0 - eye[1]).max(eye[1] - (y0 + chunk_span)).max(0.0);
                    if dx * dx + dy * dy > reach * reach {
                        continue;
                    }
                    let id = (t, [cx, cy]);
                    if !b.chunks.contains_key(&id) {
                        if b.chunks.len() >= MAX_KEPT_CHUNKS {
                            continue;
                        }
                        let made = Arc::new(b.chunk(t, [cx, cy], key));
                        b.chunks.insert(id, made);
                    }
                    if let Some(m) = b.chunks.get(&id) {
                        out.push(Arc::clone(m));
                    }
                }
            }
        }
    }
    out
}
