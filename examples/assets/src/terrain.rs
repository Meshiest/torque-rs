//! The hills: a terrain of rolling green hills with a flat field in the middle, written as a legacy `.ter` (version 2, the
//! format a `TerrainBlock`'s `terrainFile` names).
//!
//! The block is TGE's default: [`SIDE`] samples a side, [`SQUARE`] m apart, centered on the origin (`TerrainBlock::onAdd`
//! puts sample (0, 0) at `-128 * squareSize` in x and y), so it covers 2,048 m a side. [`height`] gives the ground at any
//! point: a flat field [`FIELD_RADIUS`] m round the origin at [`BASE`] m, gentle rolling hills beyond it, a single round hill
//! [`HILL_AT`] north of the field to drive over (steep on the field's side), and higher hills toward the edges. One material, `checker` (its texture is
//! `checker.png`), weighs everything.
//!
//! ```text
//! u8   version 2
//! u16  heights, 256 x 256 (x fastest), each in 1/32 m
//! u8   material map, 256 x 256 (the base material slot: 0)
//! 8 x  material names (a length byte and the bytes; empty: an unused slot)
//! u8   an alpha map, 256 x 256, for the one material named
//! ```
//!
//! Credit: the layout is the legacy one Torque3D reads in `TerrainFile::_loadLegacy` (`terrain/terrFile.cpp`, MIT, Copyright
//! (c) 2012 GarageGames, LLC). The code here is the examples' own.

/// Samples a side, and the meters between them (TGE's `squareSize` default).
pub const SIDE: usize = 256;
pub const SQUARE: f32 = 8.0;
/// The field's height and its radius (flat inside it).
pub const BASE: f32 = 20.0;
pub const FIELD_RADIUS: f32 = 40.0;
/// The round hill north of the field: its middle, height, its width across and on its north side (standard deviations, m),
/// and its width on the south side, toward the field (narrower: the steep side).
pub const HILL_AT: [f32; 2] = [0.0, 95.0];
pub const HILL_HEIGHT: f32 = 9.0;
pub const HILL_WIDTH: f32 = 24.0;
pub const HILL_STEEP_WIDTH: f32 = 14.0;
/// The height words a meter (`.ter`'s fixed point).
const WORDS_PER_METER: f32 = 32.0;

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The ground's height at world (x, y) (m), the block centered on the origin.
pub fn height(x: f32, y: f32) -> f32 {
    let r = (x * x + y * y).sqrt();
    let rolling = 4.5 * (x / 61.0 + 0.7).sin() * (y / 53.0 + 1.3).sin()
        + 2.6 * ((x + y) / 37.0).sin()
        + 1.6 * ((x - 1.7 * y) / 29.0 + 2.0).sin();
    let far =
        16.0 * smoothstep(260.0, 700.0, r) * ((x / 170.0).sin() * (y / 190.0 + 0.4).cos() + 0.9);
    let (dx, dy) = (x - HILL_AT[0], y - HILL_AT[1]);
    let wy = if dy < 0.0 {
        HILL_STEEP_WIDTH
    } else {
        HILL_WIDTH
    };
    let hill = HILL_HEIGHT
        * (-(dx * dx) / (2.0 * HILL_WIDTH * HILL_WIDTH) - (dy * dy) / (2.0 * wy * wy)).exp();
    let field = smoothstep(FIELD_RADIUS, FIELD_RADIUS + 70.0, r);
    BASE + field * (rolling + far) + hill
}

/// Where sample (i, j) lies in x and y.
pub fn sample_xy(i: usize, j: usize) -> [f32; 2] {
    let origin = -SQUARE * (SIDE / 2) as f32;
    [origin + i as f32 * SQUARE, origin + j as f32 * SQUARE]
}

/// The hills as a `.ter` (module docs).
pub fn hills() -> Vec<u8> {
    let samples = SIDE * SIDE;
    let mut out = Vec::with_capacity(1 + samples * 4 + 16);
    out.push(2);
    for j in 0..SIDE {
        for i in 0..SIDE {
            let [x, y] = sample_xy(i, j);
            let word = (height(x, y) * WORDS_PER_METER)
                .round()
                .clamp(0.0, f32::from(u16::MAX)) as u16;
            out.extend(word.to_le_bytes());
        }
    }
    out.extend(std::iter::repeat_n(0u8, samples));
    out.push(7);
    out.extend(b"checker");
    out.extend([0u8; 7]);
    out.extend(std::iter::repeat_n(255u8, samples));
    out
}
