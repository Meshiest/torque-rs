//! The sky's irradiance as L2 spherical harmonics (the ambient's shape).
//!
//! The sky is what the fixed-function frame draws behind everything (the sky box, the clouds, the fog bans and
//! anything below the horizon): [`sky_shape`] casts [`DIRECTIONS`] fixed rays from the eye through those draws in their order
//! (each hit sampled from a small mip level of its texture, the texture environment applied, alpha blended over what is behind),
//! takes the radiance to linear, projects it on the nine real SH basis functions and convolves it with the clamped cosine
//! (Ramamoorthi and Hanrahan 2001). The result is normalised so that its mean over the horizontal normals is 1: the shader
//! multiplies the frame's ambient by this shape (only its variation is used: the ambient stays calibrated to the sun's).

use crate::hd::{TextureTable, srgb_to_linear};
use crate::{Blend, Draw, TexEnv};

/// The rays cast (a Fibonacci sphere).
pub const DIRECTIONS: usize = 1024;
/// The mip level sampled is the first no wider than this (the sky's radiance, not its detail).
pub const SAMPLE_WIDTH: u32 = 64;
/// `Y_0^0`.
const Y00: f32 = 0.282_094_8;

/// The SH of a constant 1 (the shape without a sky).
pub fn uniform() -> [[f32; 3]; 9] {
    let mut c = [[0.0; 3]; 9];
    c[0] = [1.0 / Y00; 3];
    c
}

/// The nine real SH basis functions at unit direction `d`.
pub fn basis(d: [f32; 3]) -> [f32; 9] {
    let [x, y, z] = d;
    [
        Y00,
        0.488_602_5 * y,
        0.488_602_5 * z,
        0.488_602_5 * x,
        1.092_548_4 * x * y,
        1.092_548_4 * y * z,
        0.315_391_57 * (3.0 * z * z - 1.0),
        1.092_548_4 * x * z,
        0.546_274_2 * (x * x - y * y),
    ]
}

/// The SH's value at `d`.
pub fn eval(c: &[[f32; 3]; 9], d: [f32; 3]) -> [f32; 3] {
    let y = basis(d);
    let mut out = [0.0; 3];
    for i in 0..9 {
        for k in 0..3 {
            out[k] += c[i][k] * y[i];
        }
    }
    out
}

/// The `n`-th of [`DIRECTIONS`] directions.
pub fn direction(n: usize) -> [f32; 3] {
    let i = n as f32 + 0.5;
    let z = 1.0 - 2.0 * i / DIRECTIONS as f32;
    let r = (1.0 - z * z).max(0.0).sqrt();
    let phi = i * 2.399_963_1;
    [r * phi.cos(), r * phi.sin(), z]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The barycentric `(u, v)` where a ray from the origin along `d` meets the triangle, if it does (Moller and Trumbore 1997).
fn hit(d: [f32; 3], p: [[f32; 3]; 3]) -> Option<(f32, f32)> {
    let e1 = sub(p[1], p[0]);
    let e2 = sub(p[2], p[0]);
    let h = cross(d, e2);
    let a = dot(e1, h);
    if a.abs() < 1e-12 {
        return None;
    }
    let f = 1.0 / a;
    let s = [-p[0][0], -p[0][1], -p[0][2]];
    let u = f * dot(s, h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = f * dot(d, q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    (f * dot(e2, q) > 0.0).then_some((u, v))
}

/// A texel of the table's texture `t` at `uv` (repeat wrap, nearest), from its first level no wider than `width`.
fn texel(table: &TextureTable, t: usize, uv: [f32; 2], width: u32) -> [f32; 4] {
    let Some(tex) = table.textures.get(t) else {
        return [1.0; 4];
    };
    let mut level = 0;
    while (tex.width >> level).max(1) > width && level + 1 < tex.levels.len() {
        level += 1;
    }
    let (w, h) = ((tex.width >> level).max(1), (tex.height >> level).max(1));
    let Some(bytes) = tex.levels.get(level) else {
        return [1.0; 4];
    };
    let x = ((uv[0].rem_euclid(1.0) * w as f32) as u32).min(w - 1);
    let y = ((uv[1].rem_euclid(1.0) * h as f32) as u32).min(h - 1);
    let i = ((y * w + x) * 4) as usize;
    match bytes.get(i..i + 4) {
        Some(p) => [0, 1, 2, 3].map(|k| f32::from(p[k]) / 255.0),
        None => [1.0; 4],
    }
}

/// The colour the sky draws show along `d` from the eye (gamma 0..1): the clear, then each draw over it in order.
pub fn radiance(
    sky: &[Draw],
    table: &TextureTable,
    local: &[usize],
    eye: [f32; 3],
    clear: [f32; 3],
    d: [f32; 3],
) -> [f32; 3] {
    radiance_at(sky, table, local, eye, clear, d, SAMPLE_WIDTH)
}

/// [`radiance`] with the textures sampled from their first level no wider than `width`.
pub fn radiance_at(
    sky: &[Draw],
    table: &TextureTable,
    local: &[usize],
    eye: [f32; 3],
    clear: [f32; 3],
    d: [f32; 3],
    width: u32,
) -> [f32; 3] {
    let mut c = clear;
    for draw in sky {
        if matches!(
            draw.blend,
            Blend::Additive | Blend::One | Blend::Multiply | Blend::Modulate2x
        ) {
            continue;
        }
        for t in draw.vertices.chunks_exact(3) {
            let p = [0, 1, 2].map(|k| sub(t[k].position, eye));
            let Some((u, v)) = hit(d, p) else { continue };
            let w = [1.0 - u - v, u, v];
            let mut col = [0.0f32; 4];
            let mut uv = [0.0f32; 2];
            for (wk, tk) in w.iter().zip(t) {
                for (c, b) in col.iter_mut().zip(tk.color) {
                    *c += wk * f32::from(b) / 255.0;
                }
                uv[0] += wk * tk.uv[0];
                uv[1] += wk * tk.uv[1];
            }
            if let Some((i, _)) = draw.texture
                && let Some(&g) = local.get(i)
            {
                let tx = texel(table, g, uv, width);
                col = match draw.tex_env {
                    TexEnv::Modulate => [
                        col[0] * tx[0],
                        col[1] * tx[1],
                        col[2] * tx[2],
                        col[3] * tx[3],
                    ],
                    TexEnv::Decal => {
                        let a = tx[3];
                        [
                            col[0] * (1.0 - a) + tx[0] * a,
                            col[1] * (1.0 - a) + tx[1] * a,
                            col[2] * (1.0 - a) + tx[2] * a,
                            col[3],
                        ]
                    }
                    TexEnv::Replace => tx,
                    TexEnv::None => col,
                };
            }
            let a = if draw.blend == Blend::Alpha {
                col[3].clamp(0.0, 1.0)
            } else {
                1.0
            };
            for ch in 0..3 {
                c[ch] = c[ch] * (1.0 - a) + col[ch].clamp(0.0, 1.0) * a;
            }
            break;
        }
    }
    c
}

/// The sky's irradiance shape: L2 SH (RGB) of the cosine-convolved linear radiance, divided by its mean luminance over the horizontal
/// normals (the [`uniform`] shape when there is no sky or it is black). `eye` is the point the draws were built around.
pub fn sky_shape(
    sky: &[Draw],
    table: &TextureTable,
    local: &[usize],
    eye: [f32; 3],
    clear: [f32; 3],
) -> [[f32; 3]; 9] {
    if sky.is_empty() {
        return uniform();
    }
    let mut c = [[0.0f32; 3]; 9];
    for n in 0..DIRECTIONS {
        let d = direction(n);
        let l = radiance(sky, table, local, eye, clear, d).map(srgb_to_linear);
        let y = basis(d);
        for i in 0..9 {
            for k in 0..3 {
                c[i][k] += l[k] * y[i];
            }
        }
    }
    let w = 4.0 * std::f32::consts::PI / DIRECTIONS as f32;
    // The clamped cosine's bands: pi, 2 pi / 3, pi / 4.
    let band = [
        std::f32::consts::PI,
        2.0 * std::f32::consts::PI / 3.0,
        std::f32::consts::FRAC_PI_4,
    ];
    for (i, ci) in c.iter_mut().enumerate() {
        let l = match i {
            0 => 0,
            1..=3 => 1,
            _ => 2,
        };
        for v in ci.iter_mut() {
            *v *= w * band[l];
        }
    }
    let mut mean = 0.0;
    for k in 0..16 {
        let a = k as f32 * std::f32::consts::TAU / 16.0;
        let e = eval(&c, [a.cos(), a.sin(), 0.0]);
        mean += (0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2]) / 16.0;
    }
    if mean.is_nan() || mean <= 1e-6 || !mean.is_finite() {
        return uniform();
    }
    c.map(|ci| ci.map(|v| v / mean))
}

#[cfg(test)]
#[path = "sh_tests.rs"]
mod tests;
