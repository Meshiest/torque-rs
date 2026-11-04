//! The kit's textures: small RGB images drawn pixel by pixel from a rule, written as 8-bit PNGs.
//!
//! | file | size | look |
//! |---|---|---|
//! | `grid.png` | 64 x 64 | light gray tiles, 16 px each, alternating a shade, with darker grout and a darker border |
//! | `figure.png` | 64 x 64 | four swatches: skin, shirt (with a hem), trousers (shoes along the bottom), and the eyes' colour |
//! | `pistol.png` | 32 x 32 | dark metal, lighter along the top rows, faint grip lines lower down |
//! | `projectile.png` | 16 x 16 | a white core fading to a pale cyan at the edge |
//! | `spark.png` | 32 x 32 | a soft white dot fading to black at the edge (a particle: drawn added, its black adds nothing) |
//! | `hole.png`, `hole2.png`, `hole3.png` | 64 x 64, with alpha | a bolt's hole: a dark, irregular center with short cracks
//! running out of it and a lighter, chipped and dusty rim fading into the surface (three of them, each its own shape) |
//! | `flare_star.png` | 256 x 256 | a muzzle flash seen head on: a hot white core in a soft bloom with uneven petals, smoothed
//! (drawn added: black adds nothing) |
//! | `smoke.png` | 64 x 64, with alpha | a soft, uneven puff of light gray smoke |

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

/// As [`png`], with an alpha channel.
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
    let base: Rgb = [176, 181, 189];
    png(64, 64, |x, y| {
        if x < 2 || y < 2 || x >= 62 || y >= 62 {
            shade(base, -64)
        } else if x % 16 == 0 || y % 16 == 0 {
            shade(base, -40)
        } else if (x / 16 + y / 16) % 2 == 0 {
            shade(base, 10)
        } else {
            base
        }
    })
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

/// A bolt's hole (a decal), shaped by `seed`: a dark, irregular center (its edge a few waves about a circle), short cracks
/// running out of it, and a lighter, chipped and dusty rim fading into the surface; the alpha soft at every edge.
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
    png_rgba(64, 64, move |x, y| {
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
        let dark = [
            10.0 + 18.0 * depth,
            10.0 + 17.0 * depth,
            12.0 + 16.0 * depth,
        ];
        let dust = [205.0, 198.0, 186.0];
        let crack_c = [34.0, 33.0, 32.0];
        let alpha_hole = hole * 0.95;
        let alpha_rim = rim * 0.6;
        let alpha_crack = crack * 0.7;
        let alpha = (alpha_hole + alpha_rim + alpha_crack).min(1.0);
        if alpha <= 0.0 {
            return [128, 128, 128, 0];
        }
        let c = [0, 1, 2].map(|k| {
            ((dark[k] * alpha_hole + dust[k] * alpha_rim + crack_c[k] * alpha_crack)
                / (alpha_hole + alpha_rim + alpha_crack)) as u8
        });
        [c[0], c[1], c[2], (alpha * 255.0).round() as u8]
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
