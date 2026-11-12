//! The specular lobe's linearly transformed cosines for the area lights (Heitz, Dupuy, Hill and Neubelt 2016, "Real-Time
//! Polygonal-Light Shading with Linearly Transformed Cosines"): for each roughness and view angle, the 3x3 matrix `M` that turns
//! the clamped cosine into the closest fit of the GGX lobe times its cosine, normalized. A polygon's share of the lobe is then the
//! cosine's integral over the polygon transformed by `M`'s inverse: a closed form (Lambert's edge sum), so a rectangle's highlight
//! costs a few edges, not samples.
//!
//! The fit ([`fit_table`]) and the table it made ([`table`], `ltc_ggx.bin`) are this crate's own. The lobe is the forward pass's
//! (`brdf.wgsl`): GGX's distribution with Smith's height-correlated visibility, its Fresnel taken out (F = 1) and kept as two
//! moments, the lobe's directional albedo and the share of it Schlick's `(1 - V.H)^5` weights, so a lookup gives the specular for
//! any F0 and F90 (`F0 (albedo - fresnel) + F90 fresnel`). Each cell minimizes the cubed difference of the two normalized
//! distributions over directions drawn from both (as the paper does) with Nelder and Mead's simplex, from the cell before it in
//! view angle; the matrix's form is the paper's isotropic one, `[X Y Z] [[m11, 0, m13], [0, m22, 0], [0, 0, 1]]` about the lobe's
//! mean direction `Z`.
//!
//! The table is [`LTC_SIZE`] x [`LTC_SIZE`] cells: rows by roughness (perceptual, `i / (LTC_SIZE - 1)`, its GGX alpha at least
//! [`MIN_ALPHA`]), columns by `sqrt(1 - N.V)` (`j / (LTC_SIZE - 1)`, the view angle at most [`MAX_THETA`]), so the cells crowd
//! toward grazing where the lobe changes fastest. A lookup samples between the cell centers (bilinear), as the GPU's filtered
//! texture does. `cargo run -p torque-render-scene --release --example ltc-fit` makes the table again.

use std::f64::consts::PI;
use std::sync::OnceLock;

use glam::{DMat3, DVec3, dvec3};
use rayon::prelude::*;

/// The table's cells a side (roughness down, view angle across).
pub const LTC_SIZE: usize = 64;
/// The smallest GGX alpha fitted (a smoother lobe is a near mirror the cosine's transform cannot reach in half floats).
pub const MIN_ALPHA: f64 = 2e-3;
/// The most grazing view angle fitted (radians, a hundredth of a radian short of the horizon).
pub const MAX_THETA: f64 = std::f64::consts::FRAC_PI_2 - 0.01;
/// One cell's bytes in `ltc_ggx.bin` (six little-endian f32s).
pub const CELL_BYTES: usize = 24;

/// The directions a side drawn from each distribution for the fit's error (stratified, so the error is a fixed function of the
/// matrix and the simplex converges).
const FIT_SAMPLES: usize = 32;
/// The directions a side for the lobe's moments (albedo, Schlick weight, mean direction).
const MOMENT_SAMPLES: usize = 96;
/// The simplex's most steps a cell, and the spread of its values it stops at.
const MAX_STEPS: usize = 600;
const STOP_SPREAD: f64 = 1e-10;

/// One cell: the inverse matrix's four free entries over its middle one (rows `[a 0 b; 0 1 0; c 0 d]` in the shading frame, x
/// toward the eye, z the normal), the lobe's directional albedo with F = 1, and the part of it Schlick's weight takes.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LtcCell {
    pub minv: [f32; 4],
    pub albedo: f32,
    pub fresnel: f32,
}

/// A cell's fitted transform: the free entries of the matrix about the lobe's mean direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LtcFit {
    pub m11: f64,
    pub m22: f64,
    pub m13: f64,
}

/// The roughness (perceptual) and N.V of cell `(i, j)`.
#[must_use]
pub fn cell_params(i: usize, j: usize) -> (f64, f64) {
    let last = (LTC_SIZE - 1) as f64;
    let rough = i as f64 / last;
    let s = j as f64 / last;
    let theta = (1.0 - s * s).clamp(-1.0, 1.0).acos().min(MAX_THETA);
    (rough, theta.cos())
}

/// The GGX alpha a roughness fits with.
#[must_use]
pub fn alpha_of(rough: f64) -> f64 {
    (rough * rough).max(MIN_ALPHA)
}

/// The lobe times the cosine (F = 1) toward `l` from `v` (shading frame, z the normal), for GGX alpha `a`.
fn lobe(v: DVec3, l: DVec3, a: f64) -> f64 {
    if l.z <= 0.0 {
        return 0.0;
    }
    let h = (v + l).normalize();
    let a2 = a * a;
    let (nh, nv, nl) = (h.z.max(0.0), v.z.max(1e-6), l.z);
    let d = nh * nh * (a2 - 1.0) + 1.0;
    let dist = a2 / (PI * d * d);
    let gv = nl * (nv * nv * (1.0 - a2) + a2).sqrt();
    let gl = nv * (nl * nl * (1.0 - a2) + a2).sqrt();
    dist * 0.5 / (gv + gl).max(1e-12) * nl
}

/// The density of drawing `l` from `v` by GGX's half vectors (distribution of normals times N.H).
fn lobe_pdf(v: DVec3, l: DVec3, a: f64) -> f64 {
    let h = (v + l).normalize();
    let a2 = a * a;
    let nh = h.z.max(0.0);
    let d = nh * nh * (a2 - 1.0) + 1.0;
    let dist = a2 / (PI * d * d);
    dist * nh / (4.0 * v.dot(h).abs().max(1e-12))
}

/// A direction from `v` through a half vector of GGX's distribution of normals at the stratum `(u1, u2)`.
fn lobe_sample(v: DVec3, a: f64, u1: f64, u2: f64) -> DVec3 {
    let phi = 2.0 * PI * u1;
    let cos_t = ((1.0 - u2) / (1.0 + (a * a - 1.0) * u2)).sqrt();
    let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
    let h = dvec3(sin_t * phi.cos(), sin_t * phi.sin(), cos_t);
    2.0 * v.dot(h) * h - v
}

/// The stratum centers of an `n` x `n` grid.
fn strata(n: usize) -> impl Iterator<Item = (f64, f64)> {
    (0..n * n).map(move |k| {
        (
            ((k % n) as f64 + 0.5) / n as f64,
            ((k / n) as f64 + 0.5) / n as f64,
        )
    })
}

/// The lobe's moments at one cell: its albedo, the part Schlick's weight takes, its mean direction (unit, in the x-z plane).
struct Moments {
    albedo: f64,
    fresnel: f64,
    mean: DVec3,
}

fn moments(v: DVec3, a: f64) -> Moments {
    let (mut albedo, mut fresnel, mut mean) = (0.0, 0.0, DVec3::ZERO);
    for (u1, u2) in strata(MOMENT_SAMPLES) {
        let l = lobe_sample(v, a, u1, u2);
        let pdf = lobe_pdf(v, l, a);
        if pdf <= 0.0 {
            continue;
        }
        let w = lobe(v, l, a) / pdf;
        let vh = v.dot((v + l).normalize()).clamp(0.0, 1.0);
        albedo += w;
        fresnel += w * (1.0 - vh).powi(5);
        mean += w * l;
    }
    let n = (MOMENT_SAMPLES * MOMENT_SAMPLES) as f64;
    let mean = dvec3(mean.x, 0.0, mean.z);
    Moments {
        albedo: albedo / n,
        fresnel: fresnel / n,
        mean: if mean.length() > 0.0 {
            mean.normalize()
        } else {
            DVec3::Z
        },
    }
}

/// The transformed cosine of a fit about the mean direction `z`.
struct Ltc {
    m: DMat3,
    minv: DMat3,
    det_inv: f64,
}

impl Ltc {
    fn new(fit: LtcFit, z: DVec3) -> Ltc {
        let y = DVec3::Y;
        let x = y.cross(z);
        let m = DMat3::from_cols(x, y, z)
            * DMat3::from_cols(
                dvec3(fit.m11.abs().max(1e-7), 0.0, 0.0),
                dvec3(0.0, fit.m22.abs().max(1e-7), 0.0),
                dvec3(fit.m13, 0.0, 1.0),
            );
        let minv = m.inverse();
        Ltc {
            m,
            minv,
            det_inv: minv.determinant().abs(),
        }
    }

    /// The distribution's density toward `l` (unit).
    fn density(&self, l: DVec3) -> f64 {
        let lo = self.minv * l;
        let len = lo.length();
        let cos = lo.z / len;
        if cos <= 0.0 {
            return 0.0;
        }
        cos / PI * self.det_inv / (len * len * len)
    }

    /// A direction of the distribution at the stratum `(u1, u2)` (a cosine-weighted one, transformed).
    fn sample(&self, u1: f64, u2: f64) -> DVec3 {
        let phi = 2.0 * PI * u1;
        let r = u2.sqrt();
        let o = dvec3(r * phi.cos(), r * phi.sin(), (1.0 - u2).max(0.0).sqrt());
        (self.m * o).normalize()
    }
}

/// The lobe's directions drawn for one cell's error: each direction, the normalized lobe there and the density it was drawn by.
struct LobeSamples {
    v: DVec3,
    a: f64,
    albedo: f64,
    drawn: Vec<(DVec3, f64, f64)>,
}

impl LobeSamples {
    fn new(v: DVec3, a: f64, albedo: f64) -> LobeSamples {
        let drawn = strata(FIT_SAMPLES)
            .map(|(u1, u2)| {
                let l = lobe_sample(v, a, u1, u2);
                (l, lobe(v, l, a) / albedo, lobe_pdf(v, l, a))
            })
            .collect();
        LobeSamples {
            v,
            a,
            albedo,
            drawn,
        }
    }

    /// The fit's error: the cubed difference of the normalized lobe and the distribution, over directions drawn from both
    /// (each weighted by the two densities' sum, the balance heuristic).
    fn error(&self, ltc: &Ltc) -> f64 {
        let term = |f: f64, d: f64, pdf: f64| {
            let diff = (f - d).abs();
            diff * diff * diff / (pdf + d).max(1e-12)
        };
        let from_lobe: f64 = self
            .drawn
            .iter()
            .map(|&(l, f, pdf)| term(f, ltc.density(l), pdf))
            .sum();
        let from_ltc: f64 = strata(FIT_SAMPLES)
            .map(|(u1, u2)| {
                let l = ltc.sample(u1, u2);
                let f = lobe(self.v, l, self.a) / self.albedo;
                let pdf = if l.z > 0.0 {
                    lobe_pdf(self.v, l, self.a)
                } else {
                    0.0
                };
                term(f, ltc.density(l), pdf)
            })
            .sum();
        (from_lobe + from_ltc) / (2 * FIT_SAMPLES * FIT_SAMPLES) as f64
    }
}

/// Nelder and Mead's simplex minimum of `f` over three values from `start` (each first step `step` of the value, at least
/// `floor`).
fn simplex(f: impl Fn([f64; 3]) -> f64, start: [f64; 3], step: f64, floor: f64) -> [f64; 3] {
    let mut pts = [start; 4];
    for (k, p) in pts.iter_mut().skip(1).enumerate() {
        p[k] += (start[k].abs() * step).max(floor);
    }
    let mut vals = pts.map(&f);
    let combine = |a: [f64; 3], b: [f64; 3], t: f64| [0, 1, 2].map(|k| a[k] + t * (b[k] - a[k]));
    for _ in 0..MAX_STEPS {
        let mut order = [0, 1, 2, 3];
        order.sort_by(|&x, &y| vals[x].total_cmp(&vals[y]));
        pts = order.map(|k| pts[k]);
        vals = order.map(|k| vals[k]);
        if vals[3] - vals[0] <= STOP_SPREAD * vals[0].abs().max(1e-30) {
            break;
        }
        let center = [0, 1, 2].map(|k| (pts[0][k] + pts[1][k] + pts[2][k]) / 3.0);
        let reflected = combine(center, pts[3], -1.0);
        let fr = f(reflected);
        if fr < vals[0] {
            let expanded = combine(center, pts[3], -2.0);
            let fe = f(expanded);
            (pts[3], vals[3]) = if fe < fr {
                (expanded, fe)
            } else {
                (reflected, fr)
            };
        } else if fr < vals[2] {
            (pts[3], vals[3]) = (reflected, fr);
        } else {
            let inside = fr >= vals[3];
            let contracted = combine(center, if inside { pts[3] } else { reflected }, 0.5);
            let fc = f(contracted);
            if fc < vals[3].min(fr) {
                (pts[3], vals[3]) = (contracted, fc);
            } else {
                for k in 1..4 {
                    pts[k] = combine(pts[0], pts[k], 0.5);
                    vals[k] = f(pts[k]);
                }
            }
        }
    }
    let best = (0..4)
        .min_by(|&x, &y| vals[x].total_cmp(&vals[y]))
        .unwrap_or(0);
    pts[best]
}

/// The fit of cell `(i, j)` from `start` (the cell before it in view angle; none: the lobe's own alpha, isotropic), and the cell
/// as the table holds it.
#[must_use]
pub fn fit_cell(i: usize, j: usize, start: Option<LtcFit>) -> (LtcFit, LtcCell) {
    let (rough, nv) = cell_params(i, j);
    let a = alpha_of(rough);
    let v = dvec3((1.0 - nv * nv).max(0.0).sqrt(), 0.0, nv);
    let m = moments(v, a);
    let samples = LobeSamples::new(v, a, m.albedo);
    let start = start.unwrap_or(LtcFit {
        m11: a,
        m22: a,
        m13: 0.0,
    });
    let p = simplex(
        |p| {
            samples.error(&Ltc::new(
                LtcFit {
                    m11: p[0],
                    m22: p[1],
                    m13: p[2],
                },
                m.mean,
            ))
        },
        [start.m11, start.m22, start.m13],
        0.05,
        1e-4,
    );
    let fit = LtcFit {
        m11: p[0].abs(),
        m22: p[1].abs(),
        m13: p[2],
    };
    let minv = Ltc::new(fit, m.mean).minv;
    // Rows [a 0 b; 0 e 0; c 0 d] over e: the polygon's form factor does not change with the inverse's scale.
    let e = minv.y_axis.y;
    let cell = LtcCell {
        minv: [
            (minv.x_axis.x / e) as f32,
            (minv.z_axis.x / e) as f32,
            (minv.x_axis.z / e) as f32,
            (minv.z_axis.z / e) as f32,
        ],
        albedo: m.albedo as f32,
        fresnel: m.fresnel as f32,
    };
    (fit, cell)
}

/// One roughness's row of the table, each view angle fitted from the one before it.
#[must_use]
pub fn fit_row(i: usize) -> Vec<LtcCell> {
    let mut start = None;
    (0..LTC_SIZE)
        .map(|j| {
            let (fit, cell) = fit_cell(i, j, start);
            start = Some(fit);
            cell
        })
        .collect()
}

/// The whole table (rows in parallel on the current rayon pool), row-major by roughness.
#[must_use]
pub fn fit_table() -> Vec<LtcCell> {
    (0..LTC_SIZE)
        .into_par_iter()
        .map(fit_row)
        .collect::<Vec<_>>()
        .concat()
}

/// The cells as `ltc_ggx.bin` holds them.
#[must_use]
pub fn to_bytes(cells: &[LtcCell]) -> Vec<u8> {
    cells
        .iter()
        .flat_map(|c| {
            [
                c.minv[0], c.minv[1], c.minv[2], c.minv[3], c.albedo, c.fresnel,
            ]
        })
        .flat_map(f32::to_le_bytes)
        .collect()
}

/// The cells of `ltc_ggx.bin`'s bytes (a partial cell at the end is left out).
#[must_use]
pub fn from_bytes(bytes: &[u8]) -> Vec<LtcCell> {
    bytes
        .as_chunks::<CELL_BYTES>()
        .0
        .iter()
        .map(|c| {
            let f: [f32; 6] = std::array::from_fn(|k| {
                f32::from_le_bytes([c[4 * k], c[4 * k + 1], c[4 * k + 2], c[4 * k + 3]])
            });
            LtcCell {
                minv: [f[0], f[1], f[2], f[3]],
                albedo: f[4],
                fresnel: f[5],
            }
        })
        .collect()
}

/// The fitted table ([`LTC_SIZE`]² cells, row-major by roughness).
#[must_use]
pub fn table() -> &'static [LtcCell] {
    static TABLE: OnceLock<Vec<LtcCell>> = OnceLock::new();
    TABLE.get_or_init(|| from_bytes(include_bytes!("ltc_ggx.bin")))
}

/// The table between its cell centers at roughness `rough` and N.V `nv` (bilinear, as the GPU's filtered lookup).
#[must_use]
pub fn lookup(rough: f32, nv: f32) -> LtcCell {
    let cells = table();
    let last = (LTC_SIZE - 1) as f32;
    let x = rough.clamp(0.0, 1.0) * last;
    let y = (1.0 - nv.clamp(0.0, 1.0)).sqrt() * last;
    let (i0, j0) = (x.floor() as usize, y.floor() as usize);
    let (i1, j1) = ((i0 + 1).min(LTC_SIZE - 1), (j0 + 1).min(LTC_SIZE - 1));
    let (ti, tj) = (x - i0 as f32, y - j0 as f32);
    let at = |i: usize, j: usize| cells[i * LTC_SIZE + j];
    let mix = |a: LtcCell, b: LtcCell, t: f32| LtcCell {
        minv: [0, 1, 2, 3].map(|k| a.minv[k] + t * (b.minv[k] - a.minv[k])),
        albedo: a.albedo + t * (b.albedo - a.albedo),
        fresnel: a.fresnel + t * (b.fresnel - a.fresnel),
    };
    mix(
        mix(at(i0, j0), at(i0, j1), tj),
        mix(at(i1, j0), at(i1, j1), tj),
        ti,
    )
}

#[cfg(test)]
#[path = "ltc_tests.rs"]
mod tests;
