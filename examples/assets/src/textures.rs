//! The kit's textures: small RGB images drawn pixel by pixel from a rule, written as 8-bit PNGs.
//!
//! | file | size | look |
//! |---|---|---|
//! | `grid.png` | 64 x 64 | light gray tiles, 16 px each, alternating a shade, with darker grout and a darker border |
//! | `arena_floor.png`, `arena_wall.png` | 64 x 64 | the same tiles in a warm sand and a cool slate (the arena interior's) |
//! | `figure.png` | 64 x 64 | four swatches: skin, shirt (with a hem), trousers (shoes along the bottom), and the eyes' colour |
//! | `pistol.png` | 32 x 32 | dark metal, lighter along the top rows, faint grip lines lower down |
//! | `projectile.png` | 16 x 16 | a white core fading to a pale cyan at the edge |
//! | `spark.png` | 32 x 32 | a soft white dot fading to black at the edge (a particle: drawn added, its black adds nothing) |
//! | `hole.png`, `hole2.png`, `hole3.png` | 64 x 64 | a bolt's hole, drawn multiplied (0.5 gray leaves the surface as it is): a
//! dark, irregular center with short cracks running out of it and a slightly lighter, chipped rim fading into the mid-gray
//! around (three of them, each its own shape) |
//! | `flare_star.png` | 256 x 256 | a muzzle flash seen head on: a hot white core in a soft bloom with uneven petals, smoothed
//! (drawn added: black adds nothing) |
//! | `smoke.png` | 64 x 64, with alpha | a soft, uneven puff of light gray smoke |
//! | `checker.png` | 64 x 64 | the terrain's checkerboard: two squares a side, a light and a dark green |
//! | `track.png` | 32 x 32 | a tire track's tread: gray block prints darker than 0.5, the strip's edges fading to 0.5 |
//! | `dirt.png` | 128 x 256, with alpha | the race track's packed brown dirt along (left to right) and across (top to bottom) it:
//! mottled, fine grit, two darker tire-worn bands streaked along it, looser and lighter at the edges; its alpha the dirt's
//! height (dipping in patches toward the edges) |
//! | `tree_bark.png`, `tree_bark_red.png`, `tree_bark_birch.png`, `tree_pine.png`, `tree_fir.png`, `tree_leaves.png`,
//! `tree_leaves_dark.png`, `tree_leaves_light.png` | 32 x 32 | the trees' bark (brown, streaked up and down; the tall
//! conifers' red-brown in deeper streaks; a birch's pale with short dark marks round it) and foliage (a blue-green and a darker one for the conifers, a leaf green, a deep green and a yellow-green for
//! the leafy trees, softly mottled) |
//! | `buggy.png` | 64 x 64 | the buggy's swatches: orange frame, charcoal panels, trim, seats, lamps, metal, floor, tire, rim and tread |

use std::io;

type Rgb = [u8; 3];
type Rgba = [u8; 4];

/// An image of `width` x `height` whose pixel at column `x`, row `y` (row 0 at the top) is `pixel(x, y)`, as a PNG.
pub fn png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> Rgb) -> io::Result<Vec<u8>> {
    let mut data = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            data.extend(pixel(x, y));
        }
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(io::Error::other)?;
    writer.write_image_data(&data).map_err(io::Error::other)?;
    writer.finish().map_err(io::Error::other)?;
    Ok(out)
}

/// As [`png()`], with an alpha channel.
pub fn png_rgba(width: u32, height: u32, pixel: impl Fn(u32, u32) -> Rgba) -> io::Result<Vec<u8>> {
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            data.extend(pixel(x, y));
        }
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(io::Error::other)?;
    writer.write_image_data(&data).map_err(io::Error::other)?;
    writer.finish().map_err(io::Error::other)?;
    Ok(out)
}

fn shade(c: Rgb, by: i16) -> Rgb {
    c.map(|v| (i16::from(v) + by).clamp(0, 255) as u8)
}

/// The cube's tiles.
pub fn grid() -> io::Result<Vec<u8>> {
    grid_tinted([255; 3])
}

/// The arena interior's floor: the tiles in a warm sand.
pub const SAND: Rgb = [214, 198, 170];
/// The arena interior's walls and cover: the tiles in a cool slate.
pub const SLATE: Rgb = [170, 182, 201];

/// The cube's tiles multiplied by `tint` (as the cubes are drawn tinted).
pub fn grid_tinted(tint: Rgb) -> io::Result<Vec<u8>> {
    let base: Rgb = [176, 181, 189];
    let tinted = |c: Rgb| [0, 1, 2].map(|k| (u16::from(c[k]) * u16::from(tint[k]) / 255) as u8);
    png(64, 64, |x, y| tinted(grid_tile(base, x, y)))
}

/// The tile pattern's pixel at `x`, `y`.
fn grid_tile(base: Rgb, x: u32, y: u32) -> Rgb {
    if x < 2 || y < 2 || x >= 62 || y >= 62 {
        shade(base, -64)
    } else if x.is_multiple_of(16) || y.is_multiple_of(16) {
        shade(base, -40)
    } else if (x / 16 + y / 16).is_multiple_of(2) {
        shade(base, 10)
    } else {
        base
    }
}

/// The figure's four swatches.
pub fn figure() -> io::Result<Vec<u8>> {
    let skin: Rgb = [226, 186, 150];
    let shirt: Rgb = [52, 104, 196];
    let trousers: Rgb = [48, 52, 68];
    png(64, 64, |x, y| {
        let qy = y % 32;
        match (x < 32, y < 32) {
            (true, true) => skin,
            (false, true) if qy >= 28 => shade(shirt, -30),
            (false, true) => shirt,
            (true, false) if qy >= 29 => shade(trousers, -26),
            (true, false) => trousers,
            (false, false) => [30, 30, 36],
        }
    })
}

/// The pistol's metal.
pub fn pistol() -> io::Result<Vec<u8>> {
    let metal: Rgb = [74, 78, 86];
    png(32, 32, |_, y| {
        if y < 4 {
            shade(metal, 36)
        } else if y >= 16 && y % 3 == 0 {
            shade(metal, -14)
        } else {
            metal
        }
    })
}

/// The projectile's glow.
pub fn projectile() -> io::Result<Vec<u8>> {
    png(16, 16, |x, y| {
        let (dx, dy) = (x as f32 - 7.5, y as f32 - 7.5);
        let t = ((dx * dx + dy * dy).sqrt() / 10.6).min(1.0);
        let mix = |a: f32, b: f32| (a + (b - a) * t) as u8;
        [mix(255.0, 170.0), mix(255.0, 230.0), mix(255.0, 255.0)]
    })
}

/// A trail particle's soft dot.
pub fn spark() -> io::Result<Vec<u8>> {
    png(32, 32, |x, y| {
        let (dx, dy) = (x as f32 - 15.5, y as f32 - 15.5);
        let t = (1.0 - (dx * dx + dy * dy).sqrt() / 15.5).max(0.0);
        let v = (255.0 * t * t).round() as u8;
        [v, v, v]
    })
}

/// A small, repeatable random sequence (a seed's own).
struct Seq(u32);

impl Seq {
    fn f(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A bolt's hole (a decal drawn multiplied: 0.5 gray leaves the surface as it is), shaped by `seed`: a dark, irregular center
/// (its edge a few waves about a circle), short cracks running out of it, and a slightly lighter, chipped rim, every edge
/// fading softly into the mid-gray around.
pub fn hole_of(seed: u32) -> io::Result<Vec<u8>> {
    let mut r = Seq(seed.wrapping_mul(2_654_435_761).wrapping_add(17));
    let phases = [r.f(), r.f(), r.f()].map(|v| v * std::f32::consts::TAU);
    let waves = [1.0 + r.f() * 0.6, 0.6 + r.f() * 0.4, 0.4 + r.f() * 0.3];
    let core = 8.0 + r.f() * 2.5;
    let cracks: Vec<(f32, f32, f32)> = (0..5 + (r.f() * 3.0) as usize)
        .map(|_| {
            (
                r.f() * std::f32::consts::TAU,
                4.0 + r.f() * 10.0,
                (r.f() - 0.5) * 0.6,
            )
        })
        .collect();
    png(64, 64, move |x, y| {
        let (dx, dy) = (x as f32 - 31.5, y as f32 - 31.5);
        let d = (dx * dx + dy * dy).sqrt();
        let a = dy.atan2(dx);
        let edge = core
            + waves[0] * (3.0 * a + phases[0]).sin()
            + waves[1] * (5.0 * a + phases[1]).sin()
            + waves[2] * (8.0 * a + phases[2]).sin();
        // The hole: dark, a little lighter toward its edge.
        let hole = 1.0 - smoothstep(edge - 1.0, edge + 0.6, d);
        // The chipped rim: dusty, fading out over a few pixels past the edge.
        let rim =
            smoothstep(edge - 1.5, edge + 0.5, d) * (1.0 - smoothstep(edge + 2.0, edge + 8.0, d));
        // The cracks: thin dark lines out of the edge, bending a little, thinner and fainter as they go.
        let mut crack: f32 = 0.0;
        for &(angle, length, bend) in &cracks {
            let along = d - edge + 1.0;
            if along < 0.0 || along > length {
                continue;
            }
            let t = along / length;
            let dir = angle + bend * t;
            let off = (a - dir + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            let across = (off * d).abs();
            let width = 0.9 * (1.0 - t) + 0.3;
            crack = crack.max((1.0 - smoothstep(width * 0.5, width, across)) * (1.0 - t * t));
        }
        let depth = (d / edge).clamp(0.0, 1.0);
        let dark = [22.0 + 26.0 * depth; 3];
        let dust = [150.0, 149.0, 147.0];
        let crack_c = [40.0, 40.0, 40.0];
        let alpha_hole = hole * 0.95;
        let alpha_rim = rim * 0.6;
        let alpha_crack = crack * 0.7;
        let alpha = (alpha_hole + alpha_rim + alpha_crack).min(1.0);
        if alpha <= 0.0 {
            return [128, 128, 128];
        }
        [0, 1, 2].map(|k| {
            let c = (dark[k] * alpha_hole + dust[k] * alpha_rim + crack_c[k] * alpha_crack)
                / (alpha_hole + alpha_rim + alpha_crack);
            (128.0 + (c - 128.0) * alpha).round() as u8
        })
    })
}

/// The first bolt hole ([`hole_of`]).
pub fn hole() -> io::Result<Vec<u8>> {
    hole_of(1)
}

/// The side of [`flare_star`]'s square, in pixels.
pub const FLARE_STAR_SIDE: u32 = 256;

/// A muzzle flash seen head on: a hot white core in a soft bloom whose edge swells into a few uneven petals (no spikes),
/// black around (drawn added). Each pixel averages 4 x 4 samples, so the edge is smooth.
pub fn flare_star() -> io::Result<Vec<u8>> {
    const SAMPLES: u32 = 4;
    let side = FLARE_STAR_SIDE as f32;
    png(FLARE_STAR_SIDE, FLARE_STAR_SIDE, |x, y| {
        let mut sum = 0.0;
        for sy in 0..SAMPLES {
            for sx in 0..SAMPLES {
                let px = x as f32 + (sx as f32 + 0.5) / SAMPLES as f32;
                let py = y as f32 + (sy as f32 + 0.5) / SAMPLES as f32;
                sum += star_light((px / side) * 2.0 - 1.0, (py / side) * 2.0 - 1.0);
            }
        }
        let light = sum / (SAMPLES * SAMPLES) as f32;
        // A soft tone curve: the core blooms instead of clipping flat.
        let v = 1.0 - (-light * 1.6).exp();
        let r = (x as f32 / side * 2.0 - 1.0).hypot(y as f32 / side * 2.0 - 1.0);
        let warm = smoothstep(0.2, 0.9, r);
        [
            255.0 * v,
            (255.0 - 12.0 * warm) * v,
            (252.0 - 50.0 * warm) * v,
        ]
        .map(|c| c.clamp(0.0, 255.0) as u8)
    })
}

/// The flash's light at (`x`, `y`), each from -1 to 1 across the square (0 at the middle): a core, and a bloom whose reach
/// varies with the angle (five uneven petals).
fn star_light(x: f32, y: f32) -> f32 {
    let r = x.hypot(y);
    let a = y.atan2(x);
    let petals = 1.0 + 0.22 * (5.0 * a + 0.4).cos() + 0.1 * (3.0 * a - 1.1).cos();
    let reach = r / petals;
    let core = (-r * r * 70.0).exp() * 2.0;
    let bloom = (-reach * reach * 9.0).exp() * 0.9;
    let halo = (-reach * reach * 3.0).exp() * 0.2 * (1.0 - r).max(0.0);
    core + bloom + halo
}

/// A soft, uneven puff of light gray smoke (alpha soft to its edge, a few lumps in it).
pub fn smoke() -> io::Result<Vec<u8>> {
    png_rgba(64, 64, |x, y| {
        let (dx, dy) = (x as f32 - 31.5, y as f32 - 31.5);
        let d = (dx * dx + dy * dy).sqrt() / 32.0;
        let a = dy.atan2(dx);
        let lumps = 0.08 * (3.0 * a + 0.7).sin() + 0.06 * (5.0 * a + 2.1).sin();
        let edge = 0.85 + lumps;
        let puff = 1.0 - smoothstep(0.2, edge, d);
        let inner = 0.85 + 0.15 * ((x as f32 * 0.37).sin() * (y as f32 * 0.29).cos());
        let alpha = (puff * puff * inner).clamp(0.0, 1.0);
        let v = (205.0 + 20.0 * (1.0 - d)).min(235.0) as u8;
        [v, v, v.saturating_add(4), (alpha * 255.0).round() as u8]
    })
}

/// The two greens of the terrain's checkerboard.
pub const CHECKER_LIGHT: Rgb = [112, 178, 76];
pub const CHECKER_DARK: Rgb = [58, 118, 50];

/// The terrain's checkerboard: 2 x 2 squares of 32 px, light and dark green, a faint speckle in each so the ground reads as a
/// surface up close (one repeat of the texture is two squares a side).
pub fn checker() -> io::Result<Vec<u8>> {
    png(64, 64, |x, y| {
        let base = if (x / 32 + y / 32) % 2 == 0 {
            CHECKER_LIGHT
        } else {
            CHECKER_DARK
        };
        let n = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)) % 7;
        shade(base, n as i16 - 3)
    })
}

/// A tire track's tread, multiplied over the ground (0.5 gray leaves it as it is): two rows of block prints across the strip,
/// darker in the middle of each print, the edges of the strip fading out; one repeat a strip's width along it.
pub fn track() -> io::Result<Vec<u8>> {
    png(32, 32, |x, y| {
        let across = (x as f32 + 0.5) / 32.0;
        let edge = (1.0 - ((across - 0.5).abs() * 2.0).powi(4)).max(0.0);
        // Two rows of blocks (left and right half), the right row a half repeat on.
        let row = if x < 16 { y } else { (y + 16) % 32 };
        let print = if (row % 16) < 10 && (x % 16) > 1 && (x % 16) < 14 {
            0.5
        } else {
            0.68
        };
        let v = 0.5 + (print - 0.5 - 0.5) * 0.5 * edge;
        let g = (v.clamp(0.0, 1.0) * 255.0) as u8;
        [g, g, g]
    })
}

/// The buggy's swatches: a 4 x 4 grid of 16 px cells (see `buggy.rs`): the frame's orange paint, the charcoal body panels,
/// the trim, the seats; the lamps, the taillights, bare metal, the floor's tread plate; the tire, the rim and the tread blocks
/// (darker, with grooves). The rest is the trim.
pub fn buggy() -> io::Result<Vec<u8>> {
    let paint: Rgb = [236, 128, 30];
    let trim: Rgb = [36, 38, 44];
    png(64, 64, |x, y| {
        let (cx, cy, u, v) = (x / 16, y / 16, x % 16, y % 16);
        match (cx, cy) {
            (0, 0) => paint,
            (1, 0) => [58, 64, 74],
            (3, 0) => [150, 34, 30],
            (0, 1) => [255, 248, 214],
            (1, 1) => [214, 28, 24],
            (2, 1) => [170, 174, 180],
            (3, 1) if (u + v) % 6 == 0 => [110, 114, 120],
            (3, 1) => [84, 88, 94],
            (0, 2) => [44, 44, 48],
            (1, 2) => [206, 208, 212],
            (2, 2) if u % 5 == 0 => [16, 16, 18],
            (2, 2) => [32, 32, 36],
            _ => trim,
        }
    })
}

/// A hash of lattice point (x, y) and `seed` in [0, 1).
fn lattice(x: u32, y: u32, seed: u32) -> f32 {
    let mut h =
        x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xff_ffff) as f32 / 0x100_0000 as f32
}

/// Smooth value noise in [0, 1) at (x, y) (in lattice cells), repeating every `period` cells each way.
fn noise(x: f32, y: f32, period: [u32; 2], seed: u32) -> f32 {
    let (fx, fy) = (x.floor(), y.floor());
    let (tx, ty) = (x - fx, y - fy);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let cell = |dx: i64, dy: i64| {
        let px = (fx as i64 + dx).rem_euclid(i64::from(period[0])) as u32;
        let py = (fy as i64 + dy).rem_euclid(i64::from(period[1])) as u32;
        lattice(px, py, seed)
    };
    let top = cell(0, 0) + (cell(1, 0) - cell(0, 0)) * sx;
    let bottom = cell(0, 1) + (cell(1, 1) - cell(0, 1)) * sx;
    top + (bottom - top) * sy
}

/// Value noise over an image of `size` pixels, `cells` lattice cells across it each way at the coarsest of `octaves` (each
/// twice as fine and half as strong), repeating with the image: in [0, 1).
fn fbm(x: u32, y: u32, size: [u32; 2], cells: [u32; 2], octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut weight, mut amp) = (0.0, 0.0, 1.0);
    for o in 0..octaves {
        let c = [cells[0] << o, cells[1] << o];
        let u = (x as f32 + 0.5) * c[0] as f32 / size[0] as f32;
        let v = (y as f32 + 0.5) * c[1] as f32 / size[1] as f32;
        sum += noise(u, v, c, seed.wrapping_add(o)) * amp;
        weight += amp;
        amp *= 0.5;
    }
    sum / weight
}

fn scaled(c: Rgb, by: f32) -> Rgb {
    c.map(|v| (f32::from(v) * by).round().clamp(0.0, 255.0) as u8)
}

/// The race track's packed dirt.
pub const DIRT: Rgb = [150, 116, 84];
/// The dirt's size: along the track (one repeat [`crate::track::DIRT_REPEAT`] m) and across it (one repeat
/// [`crate::track::DIRT_ACROSS`] m, the centerline in the middle row).
pub const DIRT_SIZE: [u32; 2] = [128, 256];

/// The race track's dirt, along the track left to right and across it top to bottom: packed brown dirt, mottled, with fine
/// grit; two darker bands where the tires wear it (streaked along the track) either side of a lighter middle, looser and
/// lighter toward the edges. Its alpha is the dirt's height for the terrain layer's edge: 255 across the middle, dipping in
/// patches toward the edges so the grass breaks into the dirt's edge unevenly.
pub fn dirt() -> io::Result<Vec<u8>> {
    let [w, h] = DIRT_SIZE;
    let across = crate::track::DIRT_ACROSS;
    png_rgba(w, h, |x, y| {
        let side = ((y as f32 + 0.5) / h as f32 - 0.5) * across;
        let d = side.abs();
        let mottle = fbm(x, y, DIRT_SIZE, [4, 8], 4, 11) - 0.5;
        let grit = lattice(x, y, 29) - 0.5;
        let streak = fbm(x, y, DIRT_SIZE, [2, 64], 2, 37);
        // The tire bands: about 2.3 m either side of the middle, 1.8 m wide, darker where the streaks run.
        let band = (1.0 - ((d - 2.3) / 0.9).powi(2)).max(0.0);
        let wear = band * (0.55 + 0.45 * streak);
        let middle = (1.0 - (d / 1.2).powi(2)).max(0.0);
        let loose = crate::terrain::smoothstep(4.5, 6.5, d);
        let by = 1.0 + 0.10 * mottle + 0.07 * grit * (1.0 + loose) - 0.12 * wear
            + 0.04 * middle
            + 0.05 * loose;
        let c = scaled(DIRT, by);
        // Tufts: fine patches (a third of a meter and finer) where the grass reaches in, deeper toward the edge.
        let tufts = fbm(x, y, DIRT_SIZE, [24, 48], 3, 53);
        let dip =
            crate::terrain::smoothstep(4.5, 7.0, d) * crate::terrain::smoothstep(0.35, 0.75, tufts);
        let alpha = (255.0 * (1.0 - 0.8 * dip)).round() as u8;
        [c[0], c[1], c[2], alpha]
    })
}

/// The trees' colors: bark (brown, and a birch's pale bark and its dark marks), the conifers' needles (a blue-green and a
/// darker one), and the leafy trees' three greens.
pub const BARK: Rgb = [134, 98, 68];
pub const BARK_RED: Rgb = [146, 80, 54];
pub const BARK_BIRCH: Rgb = [222, 216, 200];
pub const BIRCH_MARK: Rgb = [62, 56, 52];
pub const PINE: Rgb = [44, 96, 64];
pub const FIR: Rgb = [30, 74, 58];
pub const LEAVES: Rgb = [82, 140, 50];
pub const LEAVES_DARK: Rgb = [56, 108, 44];
pub const LEAVES_LIGHT: Rgb = [140, 168, 62];

/// A tree's bark: brown, streaked up and down (the trunks' sides take the texture's rows upright).
pub fn bark() -> io::Result<Vec<u8>> {
    png(32, 32, |x, y| {
        let streak = fbm(x, y, [32, 32], [8, 1], 2, 71) - 0.5;
        let grain = lattice(x, y, 73) - 0.5;
        scaled(BARK, 1.0 + 0.22 * streak + 0.06 * grain)
    })
}

/// The tall conifers' bark: red-brown, in deep fibrous streaks up and down.
pub fn red_bark() -> io::Result<Vec<u8>> {
    png(32, 32, |x, y| {
        let streak = fbm(x, y, [32, 32], [16, 1], 2, 89) - 0.5;
        let grain = lattice(x, y, 97) - 0.5;
        scaled(BARK_RED, 1.0 + 0.32 * streak + 0.06 * grain)
    })
}

/// A birch's bark: pale, faintly streaked, with short dark marks running round the trunk.
pub fn birch_bark() -> io::Result<Vec<u8>> {
    png(32, 32, |x, y| {
        let streak = fbm(x, y, [32, 32], [8, 1], 2, 79) - 0.5;
        let marks = fbm(x, y, [32, 32], [2, 8], 2, 83);
        let c = scaled(BARK_BIRCH, 1.0 + 0.08 * streak);
        let m = smoothstep(0.62, 0.7, marks);
        [0, 1, 2]
            .map(|k| (f32::from(c[k]) * (1.0 - m) + f32::from(BIRCH_MARK[k]) * m).round() as u8)
    })
}

/// Foliage of `color`, softly mottled (`seed` its own pattern) so each flat facet reads as a surface without hiding it.
pub fn foliage(color: Rgb, seed: u32) -> io::Result<Vec<u8>> {
    png(32, 32, |x, y| {
        let mottle = fbm(x, y, [32, 32], [4, 4], 3, seed) - 0.5;
        let grain = lattice(x, y, seed + 7) - 0.5;
        scaled(color, 1.0 + 0.14 * mottle + 0.05 * grain)
    })
}

/// The valley's ground: a meadow's grass, the mountains' rock, the lake's shore sand, and the lake's water.
pub const GRASS: Rgb = [96, 128, 58];
pub const ROCK: Rgb = [128, 120, 110];
pub const SHORE_SAND: Rgb = [196, 178, 140];
pub const WATER: Rgb = [92, 128, 132];
/// The valley's ground textures' size (one repeat [`crate::valley::GROUND_TILE`] m).
pub const GROUND_SIZE: u32 = 128;

/// The valley's grass: a meadow green, patchy in larger drifts (drier, yellower) and fine blades of grain.
pub fn valley_grass() -> io::Result<Vec<u8>> {
    let size = [GROUND_SIZE; 2];
    png(GROUND_SIZE, GROUND_SIZE, |x, y| {
        let drift = fbm(x, y, size, [2, 2], 3, 211);
        let mottle = fbm(x, y, size, [8, 8], 3, 223) - 0.5;
        let grain = lattice(x, y, 227) - 0.5;
        let dry = smoothstep(0.5, 0.75, drift);
        let c = [0, 1, 2].map(|k| {
            let green = f32::from(GRASS[k]);
            let straw = f32::from([150u8, 150, 76][k]);
            (green + (straw - green) * 0.45 * dry) as u8
        });
        scaled(c, 1.0 + 0.16 * mottle + 0.10 * grain)
    })
}

/// The valley's rock: gray stone in broad bands and cracks; its alpha its height (high on the bands, low in the cracks), so it
/// breaks into the grass unevenly.
pub fn valley_rock() -> io::Result<Vec<u8>> {
    let size = [GROUND_SIZE; 2];
    png_rgba(GROUND_SIZE, GROUND_SIZE, |x, y| {
        let bands = fbm(x, y, size, [2, 6], 4, 233);
        let crack = 1.0 - (2.0 * fbm(x, y, size, [6, 6], 3, 239) - 1.0).abs();
        let grain = lattice(x, y, 241) - 0.5;
        let by = 0.82 + 0.35 * bands - 0.22 * smoothstep(0.8, 0.95, crack) + 0.08 * grain;
        let c = scaled(ROCK, by);
        let alpha = (255.0 * (0.25 + 0.75 * bands)).round() as u8;
        [c[0], c[1], c[2], alpha]
    })
}

/// The lake's shore: pale sand, rippled and speckled; its alpha its height in soft patches.
pub fn valley_sand() -> io::Result<Vec<u8>> {
    let size = [GROUND_SIZE; 2];
    png_rgba(GROUND_SIZE, GROUND_SIZE, |x, y| {
        let ripple = fbm(x, y, size, [3, 24], 2, 251) - 0.5;
        let patch = fbm(x, y, size, [4, 4], 3, 257);
        let grain = lattice(x, y, 263) - 0.5;
        let c = scaled(SHORE_SAND, 1.0 + 0.08 * ripple + 0.10 * grain);
        let alpha = (255.0 * (0.3 + 0.7 * patch)).round() as u8;
        [c[0], c[1], c[2], alpha]
    })
}

/// The lake's surface: a soft blue-green with slow ripples (the water's bump and its light detail).
pub fn valley_water() -> io::Result<Vec<u8>> {
    let size = [GROUND_SIZE; 2];
    png(GROUND_SIZE, GROUND_SIZE, |x, y| {
        let ripple = fbm(x, y, size, [6, 4], 4, 269) - 0.5;
        scaled(WATER, 1.0 + 0.25 * ripple)
    })
}
