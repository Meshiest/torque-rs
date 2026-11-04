//! The reader of a legacy `.ter` terrain (versions 1 to 6).
//!
//! The layout follows Torque3D 3.10.1's MIT-licensed `TerrainFile::_loadLegacy` (`terrain/terrFile.cpp`), Copyright (c) 2012
//! GarageGames, LLC; the code here is this crate's own.
//!
//! ```text
//! u8   version (1 to 6)
//! u16  heights, 256 x 256 (x fastest), each a fixed-point height in 1/32 m
//! u8   material map, 256 x 256: the low three bits the base material slot, the high bits flags
//! 8 x  material names (a length byte, or 255 and a 16-bit length, then the bytes); an empty name is an unused slot
//! from version 2: an alpha map (256 x 256 bytes) for each material named, in slot order
//! from version 3: u32 length and the texture script, u32 length and the heightfield script
//! from version 5: two edge terrain names (a length byte and the bytes)
//! ```
//!
//! A version-1 file has no alpha maps: each square is all its base material's ([`Terrain::alpha`]). The base slot indexes the
//! named materials in order, the empty names skipped (TGE and Torque3D compact the names as they read them), not the file's
//! eight name slots.

use crate::read::Reader;
use crate::{MAX_TER_BYTES, MapError, size_ok};

/// The squares on a side.
pub const SIZE: usize = 256;
/// The samples of one map.
pub const SAMPLES: usize = SIZE * SIZE;
/// The material slots.
pub const SLOTS: usize = 8;
/// The newest legacy version (7 on is Torque3D's own format, not read).
pub const NEWEST_VERSION: u8 = 6;
/// A height word's value of one metre.
pub const HEIGHT_PER_METRE: f32 = 32.0;
/// The bits of a material map byte that name the base slot.
pub const SLOT_MASK: u8 = 0x7;

/// A named material: its slot among the file's eight names and, from version 2, its alpha map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Material {
    pub slot: u8,
    pub name: String,
    /// 256 x 256 weights; None in a version-1 file.
    pub alpha: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terrain {
    pub version: u8,
    /// 256 x 256 raw height words, x fastest.
    pub heights: Vec<u16>,
    /// 256 x 256 bytes: base slot and flags.
    pub material_map: Vec<u8>,
    pub materials: Vec<Material>,
    pub texture_script: Vec<u8>,
    pub heightfield_script: Vec<u8>,
    pub edge_terrains: Option<[String; 2]>,
    /// Bytes after the last field (Torque ignores them).
    pub trailing: usize,
}

impl Terrain {
    /// The height at a sample in metres; the grid wraps (as Torque's does).
    pub fn height(&self, x: usize, y: usize) -> f32 {
        let i = (x % SIZE) + (y % SIZE) * SIZE;
        self.heights
            .get(i)
            .map_or(0.0, |&h| h as f32 / HEIGHT_PER_METRE)
    }

    /// The weight of `material` (an index into [`Terrain::materials`]) at sample `i`: its alpha map, or in a version-1 file 255
    /// where the base slot is `material` (the named materials' index, not their slot).
    pub fn alpha(&self, material: usize, i: usize) -> u8 {
        let Some(m) = self.materials.get(material) else {
            return 0;
        };
        match &m.alpha {
            Some(a) => a.get(i).copied().unwrap_or(0),
            None => {
                if self.material_map.get(i).map(|b| usize::from(b & SLOT_MASK)) == Some(material) {
                    255
                } else {
                    0
                }
            }
        }
    }

    /// The material each sample shows most (Torque3D's conversion to a layer map): the first of the greatest weights, 0 when all
    /// are 0, at most the last material.
    pub fn dominant(&self, i: usize) -> usize {
        let mut layer = 0;
        let mut best = 0;
        for k in 0..self.materials.len() {
            let a = self.alpha(k, i);
            if a > best {
                layer = k;
                best = a;
            }
        }
        layer.min(self.materials.len().saturating_sub(1))
    }
}

/// Where a block's sample (0, 0) lies in x and y for `square_size` metres a square: centred on the origin whatever position
/// its mission wrote, as `TerrainBlock::onAdd` places it (`setPosition(-squareSize * 128, -squareSize * 128, 0)`). The block
/// repeats every `256 * square_size` metres when the mission repeats it.
pub fn block_origin(square_size: f32) -> [f32; 2] {
    let h = -square_size * (SIZE / 2) as f32;
    [h, h]
}

/// Whether square (x, y) is cut along the diagonal from its (x, y) corner to its (x + 1, y + 1) corner (else from (x + 1, y) to
/// (x, y + 1)): every other square, as Torque3D's grid map alternates them. Drawing and collision use the same cut.
pub fn split45(x: usize, y: usize) -> bool {
    (x ^ y) & 1 == 0
}

/// The empty squares (holes) a mission's `emptySquares` runs name, as a 256 x 256 map (x fastest): each run is
/// `x | y << 8 | count << 16`, `count` squares from (x, y) along x (at least one), cut at the row's end.
pub fn empty_squares(runs: &[u32]) -> Vec<bool> {
    let mut out: Vec<bool> = std::iter::repeat_n(false, SAMPLES).collect();
    for &run in runs {
        let x = (run & 0xff) as usize;
        let y = ((run >> 8) & 0xff) as usize;
        let count = ((run >> 16) as usize).max(1);
        for k in x..(x + count).min(SIZE) {
            out[k + y * SIZE] = true;
        }
    }
    out
}

/// Reads a whole legacy `.ter`.
pub fn parse(bytes: &[u8]) -> Result<Terrain, MapError> {
    size_ok(bytes.len(), MAX_TER_BYTES)?;
    let mut r = Reader::new(bytes);
    let version = r.u8("terrain version")?;
    if !(1..=NEWEST_VERSION).contains(&version) {
        return Err(MapError::Version {
            what: "terrain",
            found: version as u32,
        });
    }
    r.fits("heights", SAMPLES as u64, 2, SAMPLES)?;
    let heights = r.items(SAMPLES, 2, &mut |r| r.u16("height"))?;
    let material_map = r.bytes_copy(SAMPLES, "material map")?;
    let mut named = Vec::new();
    for slot in 0..SLOTS as u8 {
        let name = r.long_string("material name")?;
        if !name.is_empty() {
            named.push((slot, name));
        }
    }
    let mut materials = Vec::new();
    for (slot, name) in named {
        let alpha = if version == 1 {
            None
        } else {
            Some(r.bytes_copy(SAMPLES, "alpha map")?)
        };
        materials.push(Material { slot, name, alpha });
    }
    let (texture_script, heightfield_script) = if version >= 3 {
        let n = r.count("texture script", 1, MAX_TER_BYTES)?;
        let a = r.bytes_copy(n, "texture script")?;
        let n = r.count("heightfield script", 1, MAX_TER_BYTES)?;
        (a, r.bytes_copy(n, "heightfield script")?)
    } else {
        (Vec::new(), Vec::new())
    };
    let edge_terrains = if version >= 5 {
        Some([r.string("edge terrain")?, r.string("edge terrain")?])
    } else {
        None
    };
    Ok(Terrain {
        version,
        heights,
        material_map,
        materials,
        texture_script,
        heightfield_script,
        edge_terrains,
        trailing: r.left(),
    })
}

#[cfg(test)]
#[path = "ter_tests.rs"]
mod tests;
