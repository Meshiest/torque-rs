//! The valley: a terrain of a long valley between mountains with a lake in its floor, and woods on its lower slopes, written as
//! a legacy `.ter` (the layout [`crate::terrain`] writes) and a few trees as interiors (the trees of [`crate::trees`]), each
//! placed many times.
//!
//! The block is TGE's default (256 samples a side, 8 m apart, centered on the origin). [`height`] gives the ground: the
//! valley runs west to east along a gentle curve ([`center_y`]), its floor at about [`FLOOR`] m, rising to the east; west of
//! the lake it sinks under the sea ([`SEA`]), its mountains running out as headlands, all of it under the water before the
//! block's western edge (the sun sets over the sea). Its walls climb through foothills to jagged mountains of up to about
//! 500 m (ridged noise: sharp crests, crags and gullies), and the block's rim is raised on the other sides so no edge shows
//! against the sky. The lake
//! ([`LAKE_AT`], [`LAKE_RADII`], its outline wavy) is a basin a few meters under [`WATER_LEVEL`], with a sandy shore.
//!
//! Three materials ([`MATERIALS`]): grass everywhere but the steep and the high ground (rock) and the lake's shore and bed
//! (sand). The woods ([`instances`]) stand on the valley's floor and lower slopes in patches, leafy trees low down and
//! conifers higher up, none on the beach, the rock or around the spawn's meadow ([`MEADOW`]). They are a few trees and groves
//! built once at the origin ([`VARIANTS`] single trees of each of [`KINDS`], `valley_tree_<kind><n>.dif`, and [`GROVES`]
//! groves of three to five trees of each sort, `valley_grove_<sort><n>.dif`), each place of the woods one of them placed,
//! turned and scaled by its own `InteriorInstance`: the files stay small, and the network sends one object for a grove.

use std::sync::OnceLock;

use crate::terrain::{SIDE, sample_xy, smoothstep};
use crate::trees::{Kind, Rng, fbm};

/// The lake's surface, and the floor's height around it.
pub const WATER_LEVEL: f32 = 30.0;
pub const FLOOR: f32 = 32.5;
/// The lake's middle and its radii (x, y).
pub const LAKE_AT: [f32; 2] = [-150.0, 0.0];
pub const LAKE_RADII: [f32; 2] = [240.0, 105.0];
/// The deepest the lake goes under its surface.
pub const LAKE_DEPTH: f32 = 7.0;
/// Where the valley runs into the sea (x, m: the land from the first to under the water by the second, its middle; the
/// headlands reach [`SEA_HEADLANDS`] of their distance across farther out), and the sea's depth under the water.
pub const SEA: (f32, f32) = (-230.0, -560.0);
pub const SEA_HEADLANDS: f32 = 0.3;
pub const SEA_DEPTH: f32 = 14.0;
/// The meadow kept open around the spawn: its middle and radius.
pub const MEADOW: ([f32; 2], f32) = ([180.0, -30.0], 40.0);
/// The ground's materials, in the file's order (their textures are the kit's `<name>.png`).
pub const MATERIALS: [&str; 3] = ["valley_grass", "valley_rock", "valley_sand"];
/// One repeat of the ground's textures (m).
pub const GROUND_TILE: f32 = 12.0;
/// The terrain's file.
pub const TERRAIN_FILE: &str = "valley.ter";
/// The kinds of single tree in the woods, how many of each are built, and how many groves of each sort (leafy, conifers)
/// (each tree or grove of the woods is one of them).
pub const KINDS: [Kind; 5] = [Kind::Oak, Kind::Birch, Kind::Spruce, Kind::Pine, Kind::Fir];
pub const VARIANTS: usize = 3;
pub const GROVES: usize = 5;
/// A grove's trees: how many, how far from its middle (m), and how far their trunks reach under their feet (the grove is
/// placed as one, at its middle's height: the ground may fall away under a tree at its edge by [`GROVE_SLOPE`] times the
/// radius).
pub const GROVE_TREES: (usize, usize) = (3, 5);
pub const GROVE_RADIUS: (f64, f64) = (3.5, 7.0);
pub const GROVE_SINK: f64 = 4.5;
/// The steepest ground a grove stands on (rise over run).
pub const GROVE_SLOPE: f32 = 0.5;
/// The box the woods are placed in (x and y, m), and the most trees.
pub const WOODS_AREA: ([f32; 2], [f32; 2]) = ([-780.0, -620.0], [780.0, 620.0]);
pub const MAX_PLACES: usize = 1_000;
/// The height words a meter (`.ter`'s fixed point).
const WORDS_PER_METER: f32 = 32.0;

/// The valley's middle line (its y at `x`).
pub fn center_y(x: f32) -> f32 {
    45.0 * (x / 340.0 + 0.4).sin()
}

/// The valley's floor before the lake: rising to the east and to the pass in the west.
fn floor(x: f32, y: f32) -> f32 {
    FLOOR + 30.0 * smoothstep(80.0, 900.0, x) + 2.5 * (fbm(x / 90.0, y / 90.0, 301) - 0.5)
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

/// Ridged noise at (x, y) (in lattice cells): `octaves` octaves of [`fbm`]'s noise folded into sharp crests (one less the
/// distance from the middle, squared), each twice as fine and half as strong as the last, the finer ones weighted by the
/// crests under them (sharper peaks, rougher crests), in [0, 1).
pub fn ridged(x: f32, y: f32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut weight, mut amp, mut f, mut prev) = (0.0, 0.0, 1.0, 1.0, 1.0);
    for o in 0..octaves {
        let n = 1.0 - (2.0 * fbm(x * f, y * f, seed.wrapping_add(7 * o)) - 1.0).abs();
        let crest = n * n * (0.5 + 0.5 * prev);
        sum += crest * amp;
        weight += amp;
        prev = n;
        amp *= 0.5;
        f *= 2.0;
    }
    sum / weight
}

/// The ground's height at world (x, y) (m), the block centered on the origin.
pub fn height(x: f32, y: f32) -> f32 {
    let across = (y - center_y(x)).abs();
    let half = 200.0 + 45.0 * (x / 260.0).sin();
    // Foothills along the valley's sides, then the mountains behind them: lower toward the pass in the west, their crests and
    // gullies from ridged noise; and the block's rim raised so no edge shows against the sky.
    let foot = smoothstep(0.45 * half, 1.3 * half, across);
    let hills = foot * (14.0 + 45.0 * ridged(x / 220.0 + 5.3, y / 200.0, 2, 331));
    let tall = 420.0 - 230.0 * smoothstep(-300.0, -950.0, x);
    let range = smoothstep(1.0 * half, 2.9 * half, across).powf(1.4);
    let massif = 0.35 + 0.9 * fbm(x / 300.0, y / 260.0 - 2.2, 337);
    let peaks = ridged(x / 330.0, y / 290.0, 4, 311);
    let crags = 120.0 * ridged(x / 120.0 + 7.7, y / 105.0 + 1.9, 3, 341) * range;
    let rim =
        260.0 * smoothstep(780.0, 1010.0, x.abs().max(y.abs())) * smoothstep(-650.0, -300.0, x);
    let land = floor(x, y) + hills + tall * range * (0.45 * massif + 0.75 * peaks) + crags + rim;
    // The lake: a basin under the water, its shore sloping up to the floor.
    let d = lake_distance(x, y);
    let shore = smoothstep(0.8, 1.25, d);
    let bed = WATER_LEVEL - 0.8 - LAKE_DEPTH * (1.0 - d).clamp(0.0, 1.0).sqrt();
    let ground = bed + (land - bed) * shore;
    // The sea: west of the lake the valley sinks under the water, the mountains on either side running out as headlands
    // (farther out than the middle), all of it under the sea before the block's western edge.
    let sea = smoothstep(SEA.0, SEA.1, x + SEA_HEADLANDS * across.min(700.0));
    let sea_floor = WATER_LEVEL - SEA_DEPTH;
    (ground + (sea_floor - ground) * sea).min(ground)
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

/// One place of the woods: a tree or a grove's interior, its foot (on the ground), its turn about the up axis (degrees) and
/// its scale.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub file: String,
    pub foot: [f32; 3],
    pub turn: f32,
    pub scale: f32,
    /// How many trees it holds.
    pub trees: usize,
}

/// The file of variant `n` of single tree kind `kind`.
pub fn tree_file(kind: Kind, n: usize) -> String {
    format!(
        "valley_tree_{}{n}.dif",
        format!("{kind:?}").to_ascii_lowercase()
    )
}

/// The file of grove `n` (conifers, or leafy trees).
pub fn grove_file(conifers: bool, n: usize) -> String {
    format!(
        "valley_grove_{}{n}.dif",
        if conifers { "conifer" } else { "leafy" }
    )
}

/// The woods, placed once: places on a jittered grid over [`WOODS_AREA`], each kept as likely as the [`woods_density`] there;
/// a grove where the ground is gentle enough and clear of the shore, else a single tree; leafy low down, conifers higher up.
/// Each place is one interior's instance, so the network sends few objects for many trees.
pub fn instances() -> &'static [Instance] {
    static PLACES: OnceLock<Vec<Instance>> = OnceLock::new();
    PLACES.get_or_init(|| {
        const STEP: f32 = 15.0;
        let mut rng = Rng(7_999_975);
        let mut out = Vec::new();
        let (lo, hi) = WOODS_AREA;
        let (nx, ny) = (
            ((hi[0] - lo[0]) / STEP) as usize,
            ((hi[1] - lo[1]) / STEP) as usize,
        );
        for j in 0..ny {
            for i in 0..nx {
                // Jittered within the middle of the cell, so two places stand at least 40% of a step apart.
                let x = lo[0] + (i as f32 + 0.2 + 0.6 * rng.unit() as f32) * STEP;
                let y = lo[1] + (j as f32 + 0.2 + 0.6 * rng.unit() as f32) * STEP;
                let keep = rng.unit() as f32;
                let dens = woods_density(x, y);
                if dens <= 0.0 || keep > dens * 0.2 || out.len() >= MAX_PLACES {
                    continue;
                }
                let h = height(x, y);
                let high = smoothstep(50.0, 95.0, h);
                let conifers = f64::from(0.25 + 0.6 * high) > rng.unit();
                let turn = rng.range(0.0, 360.0) as f32;
                let scale = (0.8 + 0.35 * dens) * rng.range(0.9, 1.1) as f32;
                let grove = slope(x, y) < GROVE_SLOPE && lake_distance(x, y) > 1.35;
                let pick = (rng.next() % 1000) as usize;
                let (file, trees) = if grove {
                    let n = pick % GROVES;
                    (grove_file(conifers, n), grove_size(conifers, n))
                } else {
                    let kind = if conifers {
                        [Kind::Spruce, Kind::Pine, Kind::Fir][pick % 3]
                    } else {
                        [Kind::Oak, Kind::Birch][pick % 2]
                    };
                    (tree_file(kind, (pick / 7) % VARIANTS), 1)
                };
                out.push(Instance {
                    file,
                    foot: [x, y, h],
                    turn,
                    scale,
                    trees,
                });
            }
        }
        out
    })
}

/// Grove `n`'s trees (conifers, or leafy): their kinds, feet (about its middle, at height 0), sizes and seeds.
fn grove(conifers: bool, n: usize) -> Vec<(Kind, [f64; 3], f64, u64)> {
    let mut rng = Rng(0x5eed + n as u64 * 97 + u64::from(conifers) * 13);
    let count = GROVE_TREES.0 + (rng.next() as usize) % (GROVE_TREES.1 - GROVE_TREES.0 + 1);
    let start = rng.range(0.0, std::f64::consts::TAU);
    (0..count)
        .map(|k| {
            let kind = if conifers {
                rng.pick(&[(Kind::Spruce, 0.45), (Kind::Pine, 0.3), (Kind::Fir, 0.25)])
            } else {
                rng.pick(&[(Kind::Oak, 0.6), (Kind::Birch, 0.4)])
            };
            // The first in the middle, the others round it.
            let (r, a) = if k == 0 {
                (0.0, 0.0)
            } else {
                (
                    rng.range(GROVE_RADIUS.0, GROVE_RADIUS.1),
                    start
                        + std::f64::consts::TAU * k as f64 / (count - 1) as f64
                        + rng.range(-0.4, 0.4),
                )
            };
            (
                kind,
                [r * a.cos(), r * a.sin(), 0.0],
                rng.range(0.7, 1.05),
                rng.next(),
            )
        })
        .collect()
}

/// How many trees grove `n` holds.
fn grove_size(conifers: bool, n: usize) -> usize {
    grove(conifers, n).len()
}

fn interior(brushes: &[torque_interior_builder::Brush]) -> std::io::Result<Vec<u8>> {
    torque_interior_builder::build(brushes, &crate::trees::options())
        .and_then(|b| b.bytes())
        .map_err(std::io::Error::other)
}

/// Each variant's `.dif` with its file name: [`VARIANTS`] grown trees of each of [`KINDS`] and [`GROVES`] groves of each
/// sort, their feet at the origin.
pub fn tree_difs() -> std::io::Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    for (k, &kind) in KINDS.iter().enumerate() {
        for n in 0..VARIANTS {
            let seed = 0x5eed_0000 + (k * 16 + n) as u64;
            let brushes = crate::trees::tree(kind, [0.0, 0.0, 0.0], 1.0, seed);
            out.push((tree_file(kind, n), interior(&brushes)?));
        }
    }
    for conifers in [false, true] {
        for n in 0..GROVES {
            let brushes: Vec<_> = grove(conifers, n)
                .into_iter()
                .flat_map(|(kind, foot, size, seed)| {
                    crate::trees::tree_sunk(kind, foot, size, seed, GROVE_SINK)
                })
                .collect();
            out.push((grove_file(conifers, n), interior(&brushes)?));
        }
    }
    Ok(out)
}

/// The valley's files (its terrain, its woods and its textures), by name in the kit's folder. Not in [`crate::all`]: a game
/// with the valley adds them (they take a moment to build).
pub fn files() -> std::io::Result<Vec<(String, Vec<u8>)>> {
    use crate::textures;
    let mut out = vec![
        (TERRAIN_FILE.to_string(), terrain()),
        ("valley_grass.png".to_string(), textures::valley_grass()?),
        ("valley_rock.png".to_string(), textures::valley_rock()?),
        ("valley_sand.png".to_string(), textures::valley_sand()?),
        ("valley_water.png".to_string(), textures::valley_water()?),
    ];
    out.extend(tree_difs()?);
    Ok(out)
}
