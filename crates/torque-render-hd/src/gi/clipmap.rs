//! The voxel clipmap's bookkeeping on the CPU: each level's place about the eye (moved in whole tiles), the tiles waiting to be
//! made (new to a level that moved, or under a changed box), and which tiles a frame makes: the nearest the eye first, at most
//! a budget of tiles with triangles and a larger one of empty tiles (which only clear their slot).

use std::collections::BTreeSet;

use super::sources::Aabb;

/// Voxels a level's side (`gi_common.wgsl`'s `GI_N`).
pub(crate) const N: i32 = 128;
/// Voxels a tile's side (`GI_TILE`) and tiles a level's side (`GI_TILES`).
pub(crate) const TILE: i32 = 8;
pub(crate) const TILES: i32 = N / TILE;
/// Voxels a tile and tiles a level.
pub(crate) const TILE_VOXELS: u64 = (TILE * TILE * TILE) as u64;
pub(crate) const LEVEL_TILES: u64 = (TILES * TILES * TILES) as u64;
/// The most levels (`GI_MAX_LEVELS`).
pub(crate) const MAX_LEVELS: usize = 4;
/// The finest level's voxel (units): a wall half a unit thick is two voxels.
pub(crate) const BASE_SIZE: f32 = 0.25;
/// Empty tiles cleared a frame at most.
pub(crate) const EMPTY_BUDGET: usize = 4096;

/// A tile of a level, by its world tile coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TileKey {
    pub(crate) level: u32,
    pub(crate) tile: [i32; 3],
}

/// A level's voxel side.
pub(crate) fn voxel_size(level: u32) -> f32 {
    BASE_SIZE * (1u32 << level) as f32
}

/// The low corner (voxels, a whole tile) of a level of voxels of `size` about `eye`.
fn origin_for(eye: [f32; 3], size: f32) -> [i32; 3] {
    eye.map(|e| {
        let c = (e / size).floor().clamp(-1e8, 1e8) as i32;
        (c - N / 2).div_euclid(TILE) * TILE
    })
}

/// A tile's world box, grown by a thousandth of a voxel (a triangle on its face is found for it).
pub(crate) fn tile_box(k: TileKey) -> Aabb {
    let s = voxel_size(k.level);
    let side = s * TILE as f32;
    let lo = k.tile.map(|t| t as f32 * side - 1e-3 * s);
    let hi = k.tile.map(|t| (t + 1) as f32 * side + 1e-3 * s);
    [lo, hi]
}

/// The levels' places, the tiles waiting and the tiles known to hold triangles.
pub(crate) struct Clipmap {
    pub(crate) levels: u32,
    /// Each level's low corner (voxels).
    pub(crate) origins: [[i32; 3]; MAX_LEVELS],
    placed: bool,
    pending: BTreeSet<TileKey>,
    /// The tiles made with triangles (the light's round robin walks them).
    pub(crate) solid: BTreeSet<TileKey>,
}

impl Clipmap {
    pub(crate) fn new(levels: u32) -> Clipmap {
        Clipmap {
            levels: levels.min(MAX_LEVELS as u32),
            origins: [[0; 3]; MAX_LEVELS],
            placed: false,
            pending: BTreeSet::new(),
            solid: BTreeSet::new(),
        }
    }

    /// The tiles waiting.
    pub(crate) fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Moves each level to the eye (in whole tiles); a tile a level takes in waits to be made.
    pub(crate) fn place(&mut self, eye: [f32; 3]) {
        for level in 0..self.levels {
            let o = origin_for(eye, voxel_size(level));
            let old = self.origins[level as usize];
            if self.placed && o == old {
                continue;
            }
            let t0 = o.map(|c| c / TILE);
            let old_t0 = old.map(|c| c / TILE);
            for z in 0..TILES {
                for y in 0..TILES {
                    for x in 0..TILES {
                        let t = [t0[0] + x, t0[1] + y, t0[2] + z];
                        let kept = self.placed
                            && (0..3).all(|a| t[a] >= old_t0[a] && t[a] < old_t0[a] + TILES);
                        if !kept {
                            self.pending.insert(TileKey { level, tile: t });
                        }
                    }
                }
            }
            self.origins[level as usize] = o;
        }
        self.placed = true;
        let origins = self.origins;
        let levels = self.levels;
        let holds = |k: &TileKey| {
            k.level < levels
                && (0..3).all(|a| {
                    let t0 = origins[k.level as usize][a] / TILE;
                    k.tile[a] >= t0 && k.tile[a] < t0 + TILES
                })
        };
        self.pending.retain(holds);
        self.solid.retain(holds);
    }

    /// Every tile of every level under the box `b` (grown by a voxel) waits to be made again.
    pub(crate) fn mark(&mut self, b: &Aabb) {
        for level in 0..self.levels {
            let s = voxel_size(level);
            let side = s * TILE as f32;
            let lo = b[0].map(|c| ((c - s) / side).floor() as i32);
            let hi = b[1].map(|c| ((c + s) / side).floor() as i32);
            let t0 = self.origins[level as usize].map(|c| c / TILE);
            let lo: [i32; 3] = std::array::from_fn(|a| lo[a].max(t0[a]));
            let hi: [i32; 3] = std::array::from_fn(|a| hi[a].min(t0[a] + TILES - 1));
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        self.pending.insert(TileKey {
                            level,
                            tile: [x, y, z],
                        });
                    }
                }
            }
        }
    }

    /// Forgets every tile made: all wait again.
    pub(crate) fn reset(&mut self) {
        self.placed = false;
        self.pending.clear();
        self.solid.clear();
    }

    /// The frame's tiles to make, nearest `eye` first, each with its triangles (`triangles_of` a tile's box): at most `budget`
    /// with triangles (None: all) and [`EMPTY_BUDGET`] empty ones (all with no budget).
    pub(crate) fn take(
        &mut self,
        eye: [f32; 3],
        budget: Option<usize>,
        triangles_of: impl Fn(&Aabb) -> Vec<u32>,
    ) -> Vec<(TileKey, Vec<u32>)> {
        let dist = |k: &TileKey| {
            let b = tile_box(*k);
            (0..3)
                .map(|a| {
                    let d = 0.5 * (b[0][a] + b[1][a]) - eye[a];
                    d * d
                })
                .sum::<f32>()
        };
        let mut order: Vec<(f32, TileKey)> = self.pending.iter().map(|k| (dist(k), *k)).collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let (work_budget, empty_budget) = match budget {
            Some(b) => (b, EMPTY_BUDGET),
            None => (usize::MAX, usize::MAX),
        };
        let (mut work, mut empty) = (0, 0);
        let mut out = Vec::new();
        for (_, k) in order {
            if work >= work_budget && empty >= empty_budget {
                break;
            }
            let tris = triangles_of(&tile_box(k));
            if tris.is_empty() {
                if empty >= empty_budget {
                    continue;
                }
                empty += 1;
                self.solid.remove(&k);
            } else {
                if work >= work_budget {
                    continue;
                }
                work += 1;
                self.solid.insert(k);
            }
            self.pending.remove(&k);
            out.push((k, tris));
        }
        out
    }
}
