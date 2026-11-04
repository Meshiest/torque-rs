//! The kit's textures: small RGB images drawn pixel by pixel from a rule, written as 8-bit PNGs.
//!
//! | file | size | look |
//! |---|---|---|
//! | `grid.png` | 64 x 64 | light grey tiles, 16 px each, alternating a shade, with darker grout and a darker border |
//! | `figure.png` | 64 x 64 | four swatches: skin, shirt (with a hem), trousers (shoes along the bottom), and the eyes' colour |
//! | `pistol.png` | 32 x 32 | dark metal, lighter along the top rows, faint grip lines lower down |
//! | `projectile.png` | 16 x 16 | a hot core fading to orange at the edge |

use std::io;

type Rgb = [u8; 3];

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
        [mix(255.0, 255.0), mix(240.0, 120.0), mix(170.0, 30.0)]
    })
}
