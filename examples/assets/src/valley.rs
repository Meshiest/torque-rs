//! The valley: a terrain of a long valley between mountains with a lake in its floor, and woods on its lower slopes, written as
//! a legacy `.ter` (the layout [`crate::terrain`] writes) and forest interiors (the trees of [`crate::trees`]).
//!
//! The block is TGE's default (256 samples a side, 8 m apart, centered on the origin). [`height`] gives the ground: the
//! valley runs west to east along a gentle curve ([`center_y`]), its floor at about [`FLOOR`] m, rising to the east and to a
//! low pass in the west (the sun sets over it); its walls climb to mountains of up to about 300 m, broken by ridges. The lake
//! ([`LAKE_AT`], [`LAKE_RADII`], its outline wavy) is a basin a few meters under [`WATER_LEVEL`], with a sandy shore.
//!
//! Three materials ([`MATERIALS`]): grass everywhere but the steep and the high ground (rock) and the lake's shore and bed
//! (sand). The woods ([`woods`]) stand on the valley's floor and lower slopes in patches, leafy trees low down and conifers
//! higher up, none on the beach, the rock or around the spawn's meadow ([`MEADOW`]); they are cut into squares like the
//! hills' forests ([`crate::trees::chunks_of`]), their files `valley_forest00.dif` on.

use std::sync::OnceLock;

use crate::terrain::{SIDE, sample_xy, smoothstep};
use crate::trees::{Chunk, Kind, Rng, Tree, build_chunks, chunks_of, fbm};

/// The lake's surface, and the floor's height around it.
pub const WATER_LEVEL: f32 = 30.0;
pub const FLOOR: f32 = 32.5;
/// The lake's middle and its radii (x, y).
pub const LAKE_AT: [f32; 2] = [-150.0, 0.0];
pub const LAKE_RADII: [f32; 2] = [240.0, 105.0];
/// The deepest the lake goes under its surface.
pub const LAKE_DEPTH: f32 = 7.0;
/// The meadow kept open around the spawn: its middle and radius.
pub const MEADOW: ([f32; 2], f32) = ([180.0, -30.0], 40.0);
/// The ground's materials, in the file's order (their textures are the kit's `<name>.png`).
pub const MATERIALS: [&str; 3] = ["valley_grass", "valley_rock", "valley_sand"];
/// One repeat of the ground's textures (m).
pub const GROUND_TILE: f32 = 12.0;
/// The terrain's file and the woods' files' prefix.
pub const TERRAIN_FILE: &str = "valley.ter";
pub const FOREST_PREFIX: &str = "valley_forest";
/// The box the woods are placed in (x and y, m), and the most trees.
pub const WOODS_AREA: ([f32; 2], [f32; 2]) = ([-780.0, -620.0], [780.0, 620.0]);
pub const MAX_TREES: usize = 2_000;
/// The height words a meter (`.ter`'s fixed point).
const WORDS_PER_METER: f32 = 32.0;

/// The valley's middle line (its y at `x`).
pub fn center_y(x: f32) -> f32 {
    45.0 * (x / 340.0 + 0.4).sin()
}

/// The valley's floor before the lake: rising to the east and to the pass in the west.
fn floor(x: f32, y: f32) -> f32 {
    FLOOR
        + 30.0 * smoothstep(80.0, 900.0, x)
        + 14.0 * smoothstep(-380.0, -950.0, x)
        + 2.5 * (fbm(x / 90.0, y / 90.0, 301) - 0.5)
}

/// How far out of the lake (x, y) is: below 1 inside its wavy outline.
pub fn lake_distance(x: f32, y: f32) -> f32 {
    let (dx, dy) = (
        (x - LAKE_AT[0]) / LAKE_RADII[0],
        (y - LAKE_AT[1]) / LAKE_RADII[1],
    );
    let r = dx.hypot(dy);
    let wave = 1.0 + 0.16 * (fbm(x / 120.0 + 3.1, y / 120.0 - 1.7, 307) - 0.5);
    r / wave
}

/// The ground's height at world (x, y) (m), the block centered on the origin.
pub fn height(x: f32, y: f32) -> f32 {
    let across = (y - center_y(x)).abs();
    let half = 210.0 + 50.0 * (x / 260.0).sin();
    // Foothills along the valley's sides, then the mountains behind them: lower toward the pass in the west, ridged.
    let foot = smoothstep(0.45 * half, 1.4 * half, across);
    let hills = foot * (18.0 + 40.0 * fbm(x / 160.0 + 5.3, y / 140.0, 331));
    let tall = 300.0 - 170.0 * smoothstep(-300.0, -900.0, x);
    let range = smoothstep(1.1 * half, 3.6 * half, across);
    let massif = 0.3 + 1.0 * fbm(x / 260.0, y / 240.0 - 2.2, 337);
    let ridges = 1.0 - (2.0 * fbm(x / 190.0, y / 150.0, 311) - 1.0).abs();
    let gullies = 1.0 - (2.0 * fbm(x / 85.0 + 7.7, y / 70.0 + 1.9, 341) - 1.0).abs();
    let crags = (60.0 * ridges + 40.0 * gullies) * range;
    let land = floor(x, y) + hills + tall * range * massif + crags;
    // The lake: a basin under the water, its shore sloping up to the floor.
    let d = lake_distance(x, y);
    let shore = smoothstep(0.8, 1.25, d);
    let bed = WATER_LEVEL - 0.8 - LAKE_DEPTH * (1.0 - d).clamp(0.0, 1.0).sqrt();
    bed + (land - bed) * shore
}

/// How steep the ground is at (x, y) (rise over run).
pub fn slope(x: f32, y: f32) -> f32 {
    let h = height;
    (h(x + 4.0, y) - h(x - 4.0, y)).hypot(h(x, y + 4.0) - h(x, y - 4.0)) / 8.0
}

/// The materials' shares at (x, y), summing to 1: grass, rock, sand.
pub fn shares(x: f32, y: f32) -> [f32; 3] {
    let h = height(x, y);
    let s = slope(x, y);
    let n = fbm(x / 40.0, y / 40.0, 313) - 0.5;
    let rock =
        (smoothstep(0.55, 0.85, s + 0.2 * n) + smoothstep(200.0, 260.0, h + 50.0 * n)).min(1.0);
    let sand = smoothstep(1.12, 1.0, lake_distance(x, y) + 0.06 * n) * (1.0 - rock);
    [1.0 - rock - sand, rock, sand]
}

/// The valley as a `.ter` (module docs).
pub fn terrain() -> Vec<u8> {
    let samples = SIDE * SIDE;
    let mut out = Vec::with_capacity(1 + samples * 6 + 32);
    out.push(2);
    let mut weights = [vec![0u8; samples], vec![0u8; samples], vec![0u8; samples]];
    for j in 0..SIDE {
        for i in 0..SIDE {
            let [x, y] = sample_xy(i, j);
            let word = (height(x, y) * WORDS_PER_METER)
                .round()
                .clamp(0.0, f32::from(u16::MAX)) as u16;
            out.extend(word.to_le_bytes());
            let s = shares(x, y);
            let rock = (s[1] * 255.0).round() as u8;
            let sand = ((s[2] * 255.0).round() as u8).min(255 - rock);
            let k = i + j * SIDE;
            weights[0][k] = 255 - rock - sand;
            weights[1][k] = rock;
            weights[2][k] = sand;
        }
    }
    out.extend(std::iter::repeat_n(0u8, samples));
    for name in MATERIALS {
        out.push(name.len() as u8);
        out.extend(name.as_bytes());
    }
    out.extend([0u8; 8 - MATERIALS.len()]);
    for w in weights {
        out.extend(w);
    }
    out
}

/// Whether a tree's foot may stand at (x, y), and how thick the woods are there (0 to 1).
pub fn woods_density(x: f32, y: f32) -> f32 {
    let h = height(x, y);
    let beach = lake_distance(x, y) < 1.2;
    let meadow = (x - MEADOW.0[0]).hypot(y - MEADOW.0[1]) < MEADOW.1;
    if beach || meadow || !(WATER_LEVEL + 1.5..=210.0).contains(&h) || slope(x, y) > 0.75 {
        return 0.0;
    }
    let patch = fbm(x / 130.0, y / 130.0, 317);
    // Thicker up the lower slopes, thinning toward the tree line.
    let up = smoothstep(FLOOR + 4.0, FLOOR + 40.0, h) * (1.0 - smoothstep(160.0, 210.0, h));
    smoothstep(0.42, 0.56, patch + 0.15 * up)
}

/// The woods, placed once: candidates on a jittered grid over [`WOODS_AREA`], each kept as likely as the [`woods_density`]
/// there, leafy trees low down and conifers higher up.
pub fn woods() -> &'static [Tree] {
    static WOODS: OnceLock<Vec<Tree>> = OnceLock::new();
    WOODS.get_or_init(|| {
        const STEP: f32 = 10.0;
        let mut rng = Rng(7_999_975);
        let mut out = Vec::new();
        let (lo, hi) = WOODS_AREA;
        let (nx, ny) = (
            ((hi[0] - lo[0]) / STEP) as usize,
            ((hi[1] - lo[1]) / STEP) as usize,
        );
        for j in 0..ny {
            for i in 0..nx {
                // Jittered within the middle of the cell, so two trees stand at least 40% of a step apart.
                let x = lo[0] + (i as f32 + 0.2 + 0.6 * rng.unit() as f32) * STEP;
                let y = lo[1] + (j as f32 + 0.2 + 0.6 * rng.unit() as f32) * STEP;
                let keep = rng.unit() as f32;
                let dens = woods_density(x, y);
                if dens <= 0.0 || keep > dens * 0.17 || out.len() >= MAX_TREES {
                    continue;
                }
                let h = height(x, y);
                let high = f64::from(smoothstep(50.0, 95.0, h));
                let kind = rng.pick(&[
                    (Kind::Oak, 0.45 * (1.0 - high)),
                    (Kind::Birch, 0.25 * (1.0 - high)),
                    (Kind::Spruce, 0.15 + 0.25 * high),
                    (Kind::Pine, 0.1 + 0.3 * high),
                    (Kind::Fir, 0.05 + 0.3 * high),
                ]);
                let size = (0.65 + 0.4 * dens) * rng.range(0.85, 1.15) as f32;
                out.push(Tree {
                    kind,
                    foot: [x, y, h],
                    size,
                    seed: rng.next(),
                });
            }
        }
        out
    })
}

/// The woods' squares (module docs).
pub fn chunks() -> &'static [Chunk] {
    static CHUNKS: OnceLock<Vec<Chunk>> = OnceLock::new();
    CHUNKS.get_or_init(|| chunks_of(woods(), FOREST_PREFIX))
}

/// Every square of the woods as a `.dif` with its file name, built once.
pub fn forest_difs() -> std::io::Result<Vec<(&'static str, Vec<u8>)>> {
    static DIFS: OnceLock<Result<Vec<Vec<u8>>, String>> = OnceLock::new();
    let difs = DIFS.get_or_init(|| build_chunks(woods(), chunks()));
    let difs = difs
        .as_ref()
        .map_err(|e| std::io::Error::other(e.clone()))?;
    Ok(chunks()
        .iter()
        .zip(difs)
        .map(|(c, d)| (c.file.as_str(), d.clone()))
        .collect())
}

/// The valley's files (its terrain, its woods and its textures), by name in the kit's folder. Not in [`crate::all`]: a game
/// with the valley adds them (they take a moment to build).
pub fn files() -> std::io::Result<Vec<crate::Asset>> {
    use crate::textures;
    let mut out = vec![
        crate::Asset {
            name: TERRAIN_FILE,
            bytes: terrain(),
        },
        crate::Asset {
            name: "valley_grass.png",
            bytes: textures::valley_grass()?,
        },
        crate::Asset {
            name: "valley_rock.png",
            bytes: textures::valley_rock()?,
        },
        crate::Asset {
            name: "valley_sand.png",
            bytes: textures::valley_sand()?,
        },
        crate::Asset {
            name: "valley_water.png",
            bytes: textures::valley_water()?,
        },
    ];
    out.extend(
        forest_difs()?
            .into_iter()
            .map(|(name, bytes)| crate::Asset { name, bytes }),
    );
    Ok(out)
}
