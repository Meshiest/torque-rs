//! The showcase's textures, all made here: the particle sprites (served as PNG files to the datablocks' `textureName`s through
//! [`Sprites`]), the stages' floors and the glowing figure's line and pool of light.

use std::f32::consts::TAU;
use std::sync::Arc;

use torque_render_scene::hd::kit::Lcg;
use torque_render_scene::{Assets, Texture};

/// The particle textures by the names the datablocks give them.
pub const SPARK: &str = "fx/spark";
pub const GLOW: &str = "fx/glow";
pub const MOTE: &str = "fx/mote";
pub const FIREFLY: &str = "fx/firefly";
pub const MIST: &str = "fx/mist";
/// A water drop: a tight bright bead with no halo (it is lit, not a light).
pub const DROP: &str = "fx/drop";
/// A glowing line's cross section (a thin bright core in a tight glow), the same core in a smaller glow (a rune's stroke),
/// and a soft disc, as decal textures (the magic circle's figure and its pool of light).
pub const LINE: &str = "fx/line";
pub const GLYPH: &str = "fx/glyph";
pub const DISC: &str = "fx/disc";
/// A lava lake's crust: dark plates glowing faintly, bright cracks between them (a decal texture).
pub const LAVA: &str = "fx/lava";
/// A cherry petal seen at eight tilts, from flat on to nearly edge on and back: a tumbling petal's flipbook.
pub const PETALS: [&str; 8] = [
    "fx/petal0",
    "fx/petal1",
    "fx/petal2",
    "fx/petal3",
    "fx/petal4",
    "fx/petal5",
    "fx/petal6",
    "fx/petal7",
];
/// The smoke sprites: four of the same cloth, so a column is not one shape repeated.
pub const SMOKES: [&str; 4] = ["fx/smoke0", "fx/smoke1", "fx/smoke2", "fx/smoke3"];
pub const SMOKE: &str = SMOKES[0];

/// A texel's alpha at distance `d` from the center (0 at the center, 1 at the edge of the square's inscribed circle).
type Falloff = fn(f32) -> f32;

/// A white sprite of `side` texels whose alpha is `falloff` of the distance from its center, as PNG bytes.
fn sprite(side: u32, falloff: Falloff) -> Vec<u8> {
    let mut texels = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let u = (x as f32 + 0.5) / side as f32 * 2.0 - 1.0;
            let v = (y as f32 + 0.5) / side as f32 * 2.0 - 1.0;
            let a = falloff(u.hypot(v)).clamp(0.0, 1.0);
            texels.extend_from_slice(&[255, 255, 255, (a * 255.0).round() as u8]);
        }
    }
    png(side, side, &texels)
}

pub fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().expect("a png header");
    w.write_image_data(rgba).expect("the png's texels");
    drop(w);
    out
}

/// A hot point: a bright core and a short halo.
fn spark(d: f32) -> f32 {
    (-d * d * 9.0).exp() + 0.25 * (1.0 - d).max(0.0).powi(2)
}

/// A wide soft glow.
fn glow(d: f32) -> f32 {
    (1.0 - d).max(0.0).powf(2.2)
}

/// A small bright orb with a faint rim of light.
fn mote(d: f32) -> f32 {
    (-d * d * 5.0).exp() * (1.0 - d).max(0.0).sqrt()
}

/// A sheet of mist: a soft disk with no shape of its own.
fn mist(d: f32) -> f32 {
    (-d * d * 3.0).exp() * (1.0 - d).max(0.0).powf(1.5)
}

/// A water drop: a tight bead, its edge sharp.
fn bead(d: f32) -> f32 {
    (-d * d * 14.0).exp() * (1.0 - d).max(0.0).sqrt()
}

/// A firefly's light: a tight core in a wide, faint halo.
fn firefly(d: f32) -> f32 {
    (-d * d * 36.0).exp() + 0.16 * (1.0 - d).max(0.0).powi(3)
}

/// A cherry petal of `side` texels at tilt `k` of `n` (the tilt runs through a half turn over the frames, so the flipbook
/// loops): a rounded blade with the notch at its tip, narrowed across as it tilts toward edge on, its edge a little pinker
/// than its heart and its face a touch darker edge on (it takes less light).
fn petal(side: u32, k: usize, n: usize) -> Vec<u8> {
    let tilt = std::f32::consts::PI * k as f32 / n as f32;
    let squash = tilt.cos().abs().max(0.14);
    let mut texels = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let u = ((x as f32 + 0.5) / side as f32 * 2.0 - 1.0) / squash;
            let v = -((y as f32 + 0.5) / side as f32 * 2.0 - 1.0);
            // The blade: an ellipse, fuller toward the tip, with a notch cut at the tip.
            let width = 0.5 + 0.12 * v;
            let body = (u / width).powi(2) + (v / 0.92).powi(2);
            let notch = v > 0.55 && u.abs() < 0.22 * (v - 0.55) / 0.37 + 0.03 * (0.92 - v).max(0.0);
            let inside = (1.0 - body).clamp(0.0, 1.0) * 14.0;
            let a = if notch { 0.0 } else { inside.min(1.0) };
            let edge = body.clamp(0.0, 1.0);
            let shade = 1.0 - 0.18 * (1.0 - squash);
            let r = 255.0 * shade;
            let g = (235.0 - 25.0 * edge) * shade;
            let b = (242.0 - 12.0 * edge) * shade;
            texels.extend_from_slice(&[r as u8, g as u8, b as u8, (a * 255.0) as u8]);
        }
    }
    png(side, side, &texels)
}

/// Value noise on a torus: `cells` random values a side, interpolated smoothly and wrapping, so what it textures tiles.
struct Noise {
    cells: usize,
    grid: Vec<f32>,
}

impl Noise {
    fn new(cells: usize, rng: &mut Lcg) -> Noise {
        Noise {
            cells,
            grid: (0..cells * cells).map(|_| rng.unit()).collect(),
        }
    }

    /// The noise at (`x`, `y`) in 0..1 (wrapping), in 0..1.
    fn at(&self, x: f32, y: f32) -> f32 {
        let n = self.cells;
        let (fx, fy) = (x.rem_euclid(1.0) * n as f32, y.rem_euclid(1.0) * n as f32);
        let (ix, iy) = ((fx as usize).min(n - 1), (fy as usize).min(n - 1));
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
        let at = |i: usize, j: usize| self.grid[(j % n) * n + (i % n)];
        let top = at(ix, iy) * (1.0 - sx) + at(ix + 1, iy) * sx;
        let bottom = at(ix, iy + 1) * (1.0 - sx) + at(ix + 1, iy + 1) * sx;
        top * (1.0 - sy) + bottom * sy
    }
}

/// Noise summed over octaves (`cells` doubling, amplitude halving), in about 0..1.
struct Fbm {
    octaves: Vec<Noise>,
}

impl Fbm {
    fn new(cells: usize, octaves: usize, rng: &mut Lcg) -> Fbm {
        Fbm {
            octaves: (0..octaves).map(|k| Noise::new(cells << k, rng)).collect(),
        }
    }

    fn at(&self, x: f32, y: f32) -> f32 {
        let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
        for n in &self.octaves {
            sum += amp * n.at(x, y);
            norm += amp;
            amp *= 0.5;
        }
        sum / norm
    }
}

/// A smoke sprite of `side` texels: wisps, not a ball. Ridged noise read through a domain warp gives fibers and curling
/// sheets, thinned toward a lumpy edge; mostly transparent, and only a little darker where it is thickest (campfire smoke
/// is a thin, pale, translucent thing). Each `seed` is its own sprite.
fn wisp(side: u32, seed: u64) -> Vec<u8> {
    let n = side as usize;
    let mut rng = Lcg::new(seed);
    let fibers = Fbm::new(3, 5, &mut rng);
    let warp = Fbm::new(2, 3, &mut rng);
    let sheets = Fbm::new(2, 2, &mut rng);
    let rim = Noise::new(5, &mut rng);
    let mut texels = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let (u, v) = ((x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32);
            let (cx, cy) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
            let d = cx.hypot(cy);
            let angle = cy.atan2(cx) / TAU;
            let edge = 0.55 + 0.4 * rim.at(angle, 0.61);
            let body = ((edge - d) / (0.7 * edge)).clamp(0.0, 1.0);
            let body = body * body * (3.0 - 2.0 * body);
            // The warp bends the fibers into curls; the ridges of the noise are the fibers.
            let (wx, wy) = (warp.at(u, v) - 0.5, warp.at(v + 0.4, u + 0.6) - 0.5);
            let (px, py) = (u + 0.5 * wx, v + 0.5 * wy);
            let f = fibers.at(px, py);
            let ridged = (1.0 - (2.0 * f - 1.0).abs()).powf(2.2);
            let sheet = (sheets.at(px + 0.2, py + 0.7) - 0.25).max(0.0) * 1.4;
            let density = (body * (0.25 * sheet + 1.1 * ridged * (0.4 + sheet))).clamp(0.0, 1.0);
            let shade = 1.0 - 0.3 * density;
            let c = (shade * 255.0) as u8;
            texels.extend_from_slice(&[c, c, c, (density * 255.0) as u8]);
        }
    }
    png(side, side, &texels)
}

/// The particle textures as files.
pub struct Sprites {
    files: Vec<(&'static str, Vec<u8>)>,
    /// The ones named at run time (the digital rain's glyph atlas).
    made: Vec<(String, Vec<u8>)>,
}

impl Sprites {
    pub fn new() -> Sprites {
        let mut files = vec![
            (SPARK, sprite(32, spark)),
            (GLOW, sprite(64, glow)),
            (MOTE, sprite(32, mote)),
            (FIREFLY, sprite(64, firefly)),
            (MIST, sprite(64, mist)),
            (DROP, sprite(32, bead)),
            (LINE, texture_png(&line())),
            (GLYPH, texture_png(&glyph())),
            (DISC, texture_png(&disc())),
            (LAVA, texture_png(&lava())),
        ];
        files.extend(
            SMOKES
                .iter()
                .enumerate()
                .map(|(k, name)| (*name, wisp(128, 23 + k as u64))),
        );
        files.extend(
            PETALS
                .iter()
                .enumerate()
                .map(|(k, name)| (*name, petal(64, k, PETALS.len()))),
        );
        Sprites {
            files,
            made: crate::effects::glyph_sprites(),
        }
    }
}

impl Assets for Sprites {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let stem = path.strip_suffix(".png")?;
        self.files
            .iter()
            .find(|(name, _)| stem.eq_ignore_ascii_case(name))
            .map(|(_, bytes)| bytes.clone())
            .or_else(|| {
                self.made
                    .iter()
                    .find(|(name, _)| stem.eq_ignore_ascii_case(name))
                    .map(|(_, bytes)| bytes.clone())
            })
    }
}

/// A made texture's first level as PNG bytes (served to the datablocks and decals as a file).
fn texture_png(t: &Texture) -> Vec<u8> {
    png(t.width, t.height, &t.levels[0])
}

fn texture(name: &str, side: usize, rgba: Vec<u8>) -> Texture {
    Texture {
        path: name.into(),
        width: side as u32,
        height: side as u32,
        levels: Arc::new(vec![rgba]),
    }
}

/// The concrete floor: dark slabs with faint seams, tiled every 4 units.
pub fn concrete() -> Texture {
    const N: usize = 256;
    let mut rng = Lcg::new(41);
    let speckle: Vec<f32> = (0..N * N).map(|_| rng.unit()).collect();
    let mut rgba = Vec::with_capacity(N * N * 4);
    for y in 0..N {
        for x in 0..N {
            let seam = x % (N / 2) < 2 || y % (N / 2) < 2;
            let s = speckle[y * N + x];
            let v = if seam { 60.0 } else { 110.0 + 30.0 * s };
            let c = v as u8;
            rgba.extend_from_slice(&[c, c, (v * 1.03).min(255.0) as u8, 255]);
        }
    }
    texture("showcase concrete", N, rgba)
}

/// A wet street at night: dark asphalt, its aggregate a fine speckle with a few darker patches, tiled every 4 units.
pub fn grass() -> Texture {
    const N: usize = 256;
    let mut rng = Lcg::new(43);
    let tufts = Fbm::new(6, 4, &mut rng);
    let patches = Fbm::new(2, 2, &mut rng);
    let mut rgba = Vec::with_capacity(N * N * 4);
    for y in 0..N {
        for x in 0..N {
            let (u, v) = (x as f32 / N as f32, y as f32 / N as f32);
            let t = tufts.at(u, v);
            let p = patches.at(u, v);
            let fine = rng.unit();
            // Green where the tufts are, toward brown earth between them.
            let green = (0.35 + 0.65 * t) * (0.7 + 0.3 * p);
            let r = 30.0 + 40.0 * (1.0 - green) + 12.0 * fine;
            let g = 44.0 + 50.0 * green + 14.0 * fine;
            let b = 18.0 + 16.0 * (1.0 - green) + 8.0 * fine;
            rgba.extend_from_slice(&[r as u8, g as u8, b as u8, 255]);
        }
    }
    texture("showcase grass", N, rgba)
}

/// A lava field's ground: dark basalt, rough and lumpy from noise, with no seams, tiled every 4 units.
pub fn basalt() -> Texture {
    const N: usize = 256;
    let mut rng = Lcg::new(59);
    let lumps = Fbm::new(4, 5, &mut rng);
    let mut rgba = Vec::with_capacity(N * N * 4);
    for y in 0..N {
        for x in 0..N {
            let (u, v) = (x as f32 / N as f32, y as f32 / N as f32);
            let l = lumps.at(u, v);
            let fine = rng.unit();
            let s = 0.45 + 0.55 * l;
            let r = 34.0 * s + 8.0 * fine;
            let g = 31.0 * s + 8.0 * fine;
            let b = 32.0 * s + 8.0 * fine;
            rgba.extend_from_slice(&[r as u8, g as u8, b as u8, 255]);
        }
    }
    texture("showcase basalt", N, rgba)
}

/// A barn's floor: weathered boards along y, each its own shade, grain along them and a dark seam between, tiled every 4
/// units.
pub fn planks() -> Texture {
    const N: usize = 256;
    const BOARDS: usize = 6;
    let mut rng = Lcg::new(47);
    let grain = Fbm::new(2, 5, &mut rng);
    let shades: Vec<f32> = (0..BOARDS).map(|_| rng.unit()).collect();
    let mut rgba = Vec::with_capacity(N * N * 4);
    for y in 0..N {
        for x in 0..N {
            let (u, v) = (x as f32 / N as f32, y as f32 / N as f32);
            let board = (u * BOARDS as f32) as usize % BOARDS;
            let across = (u * BOARDS as f32).fract();
            let seam = across < 0.03 || across > 0.97;
            // The grain runs along the boards: the noise is stretched along v and read with the board's own offset.
            let g = grain.at(u * 6.0 + shades[board] * 3.0, v * 0.35 + shades[board]);
            let streak = (1.0 - (2.0 * g - 1.0).abs()).powf(1.6);
            let fine = rng.unit();
            let base = 0.55 + 0.3 * shades[board];
            let v = if seam {
                0.25
            } else {
                base * (0.78 + 0.22 * streak) + 0.05 * fine
            };
            let r = (150.0 * v) as u8;
            let gch = (118.0 * v) as u8;
            let b = (82.0 * v) as u8;
            rgba.extend_from_slice(&[r, gch, b, 255]);
        }
    }
    texture("showcase planks", N, rgba)
}

/// A glowing line's cross section, across the texture's height: a thin bright core (a twelfth of the ribbon's width) in a
/// tight soft glow. A ribbon's texture, constant along its length.
pub fn line() -> Texture {
    line_profile("showcase line", 0.35)
}

/// The same core in a smaller glow: a rune's stroke.
pub fn glyph() -> Texture {
    line_profile("showcase glyph", 0.22)
}

fn line_profile(name: &str, glow_strength: f32) -> Texture {
    const N: usize = 256;
    let mut rng = Lcg::new(47);
    let mut rgba = Vec::with_capacity(N * N * 4);
    for y in 0..N {
        let d = ((y as f32 + 0.5) / N as f32 - 0.5).abs() * 2.0;
        let core = (-(d / 0.08).powi(2)).exp();
        let glow = glow_strength * (-(d / 0.4).powi(2)).exp();
        // The core white (the line's own color), the glow a little bluer than the line.
        let r = (255.0 * (0.78 + 0.22 * core)) as u8;
        let g = (255.0 * (0.86 + 0.14 * core)) as u8;
        for _ in 0..N {
            // Dithered to its texels, so the glow's slope has no bands.
            let a = ((core + glow) * 255.0 + rng.unit() - 0.5).round();
            rgba.extend_from_slice(&[r, g, 255, a.clamp(0.0, 255.0) as u8]);
        }
    }
    texture(name, N, rgba)
}

/// A lava lake's crust: cellular plates (the nearest of scattered points, wrapping, their edges bent by a warp) with the
/// cracks between them glowing white-yellow, the plates a deep red that shows through, and the glow fading into the plates
/// from each crack.
pub fn lava() -> Texture {
    const N: usize = 256;
    const CELLS: usize = 40;
    let mut rng = Lcg::new(53);
    let points: Vec<(f32, f32)> = (0..CELLS).map(|_| (rng.unit(), rng.unit())).collect();
    let heat = Fbm::new(3, 3, &mut rng);
    let warp = Fbm::new(3, 3, &mut rng);
    let mut rgba = Vec::with_capacity(N * N * 4);
    for y in 0..N {
        for x in 0..N {
            let (u, v) = ((x as f32 + 0.5) / N as f32, (y as f32 + 0.5) / N as f32);
            // The plates' edges wander: the point is read through a warp before the cells are found.
            let (wu, wv) = (
                u + 0.06 * (warp.at(u, v) - 0.5),
                v + 0.06 * (warp.at(v + 0.3, u + 0.7) - 0.5),
            );
            // The two nearest points on the torus: a crack where they are as near as each other.
            let (mut d1, mut d2) = (f32::MAX, f32::MAX);
            for &(px, py) in &points {
                let dx = (wu - px + 1.5).rem_euclid(1.0) - 0.5;
                let dy = (wv - py + 1.5).rem_euclid(1.0) - 0.5;
                let d = dx.hypot(dy);
                if d < d1 {
                    d2 = d1;
                    d1 = d;
                } else if d < d2 {
                    d2 = d;
                }
            }
            let edge = d2 - d1;
            let h = heat.at(u, v);
            let crack = (1.0 - edge / (0.014 + 0.012 * h)).clamp(0.0, 1.0);
            let core = crack.powf(2.0);
            let fringe = (1.0 - edge / 0.09).clamp(0.0, 1.0).powf(1.6) * (0.5 + 0.5 * h);
            // White-yellow in the crack's core, orange about it, a deep red glowing through the plates.
            let plate = 0.5 + 0.5 * h;
            let r = (0.5 * plate + 0.5 * (core + 0.7 * fringe).min(1.0)).min(1.0);
            let g = 0.06 * plate + 0.8 * core + 0.25 * fringe;
            let b = 0.55 * core * core;
            let a = (0.22 * plate + 0.4 * fringe + 0.9 * core).min(1.0);
            rgba.extend_from_slice(&[
                (r * 255.0) as u8,
                (g * 255.0).min(255.0) as u8,
                (b * 255.0).min(255.0) as u8,
                (a * 255.0) as u8,
            ]);
        }
    }
    texture("showcase lava", N, rgba)
}

/// A soft disc: a pool of light on the floor.
pub fn disc() -> Texture {
    const N: usize = 128;
    let mut rgba = Vec::with_capacity(N * N * 4);
    for y in 0..N {
        for x in 0..N {
            let u = (x as f32 + 0.5) / N as f32 * 2.0 - 1.0;
            let v = (y as f32 + 0.5) / N as f32 * 2.0 - 1.0;
            let d = u.hypot(v);
            let a = (1.0 - d).max(0.0).powf(1.6);
            rgba.extend_from_slice(&[255, 255, 255, (a * 255.0) as u8]);
        }
    }
    texture("showcase disc", N, rgba)
}
