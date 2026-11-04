//! The output side: the IDCT of every block (with `decompress_smooth_data`'s block smoothing when it applies, jdcoefct.c), the
//! upsampling of each component to full size (jdsample.c, with the edge rows `jdmainct.c`'s context pointers duplicate), and the
//! colour conversion (jdcolor.c).
//!
//! libjpeg streams this a row group at a time; this decoder first builds each component's sample plane whole and then reads it with
//! the same rules, which gives the same samples: the upsamplers only ever look at the row above and below (duplicated at the top
//! and bottom of the image) and never at the padding past a component's `downsampled_width`.
//! Based in part on the work of the Independent JPEG Group; see [`super`] for the credits.

use super::idct::islow;
use super::{ColorSpace, Decoder, JpegError};

/// The per-component upsampling method `jinit_upsampler` picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Method {
    FullSize,
    H2V1Fancy,
    H2V1,
    H2V2Fancy,
    H2V2,
    /// `int_upsample`: each sample replicated `h` times across and `v` times down.
    Int {
        h: usize,
        v: usize,
    },
}

/// `jinit_upsampler`'s choice for component `ci` (the IDCT is never scaled, so an input group is `h x v` samples).
pub(super) fn method(d: &Decoder<'_>, ci: usize) -> Result<Method, JpegError> {
    let c = &d.comps[ci];
    let (hi, vi) = (usize::from(c.h), usize::from(c.v));
    let (ho, vo) = (usize::from(d.max_h), usize::from(d.max_v));
    let fancy = c.downsampled_width > 2;
    Ok(if hi == ho && vi == vo {
        Method::FullSize
    } else if hi * 2 == ho && vi == vo {
        if fancy {
            Method::H2V1Fancy
        } else {
            Method::H2V1
        }
    } else if hi * 2 == ho && vi * 2 == vo {
        if fancy {
            Method::H2V2Fancy
        } else {
            Method::H2V2
        }
    } else if ho % hi == 0 && vo % vi == 0 {
        Method::Int {
            h: ho / hi,
            v: vo / vi,
        }
    } else {
        return Err(JpegError::Fatal("JERR_FRACT_SAMPLE_NOTIMPL"));
    })
}

/// One component's samples: `width_in_blocks * 8` by `height_in_blocks * 8`.
struct Plane {
    stride: usize,
    samples: Vec<u8>,
    /// `downsampled_width` and `downsampled_height`.
    dw: usize,
    dh: usize,
}

impl Plane {
    fn at(&self, row: usize, col: usize) -> i32 {
        i32::from(
            self.samples
                .get(row * self.stride + col)
                .copied()
                .unwrap_or(0),
        )
    }
}

/// Positions in natural order of the first five zigzag AC coefficients.
const Q01: usize = 1;
const Q10: usize = 8;
const Q20: usize = 16;
const Q11: usize = 9;
const Q02: usize = 2;

/// `smoothing_ok`: progressive, every component's table latched with the six multipliers non-zero, every DC at least partly known,
/// and some of the first five ACs still inexact. Returns the latched `coef_bits[1..=5]` per component.
fn smoothing(d: &Decoder<'_>) -> Option<Vec<[i32; 6]>> {
    if !d.progressive || d.coef_bits.is_empty() {
        return None;
    }
    let mut useful = false;
    let mut latch = Vec::with_capacity(d.comps.len());
    for (ci, c) in d.comps.iter().enumerate() {
        let q = c.quant?;
        if [0, Q01, Q10, Q20, Q11, Q02].iter().any(|&i| q[i] == 0) {
            return None;
        }
        let bits = d.coef_bits.get(ci)?;
        if bits[0] < 0 {
            return None;
        }
        let mut l = [0i32; 6];
        for k in 1..=5 {
            l[k] = bits[k];
            useful |= bits[k] != 0;
        }
        latch.push(l);
    }
    useful.then_some(latch)
}

/// The estimate of one AC coefficient from the DC gradient (K.8), as `decompress_smooth_data` computes it in wrapping 32-bit
/// arithmetic.
fn predict(q00: i32, qxx: i32, al: i32, num_factor: i32, gradient: i32) -> i16 {
    let num = num_factor.wrapping_mul(q00).wrapping_mul(gradient);
    let qs7 = qxx.wrapping_shl(7);
    let qs8 = qxx.wrapping_shl(8);
    let mut pred;
    if num >= 0 {
        pred = qs7.wrapping_add(num) / qs8;
        if al > 0 && pred >= (1 << al) {
            pred = (1 << al) - 1;
        }
    } else {
        pred = qs7.wrapping_sub(num) / qs8;
        if al > 0 && pred >= (1 << al) {
            pred = (1 << al) - 1;
        }
        pred = pred.wrapping_neg();
    }
    pred as i16
}

fn build_plane(d: &Decoder<'_>, ci: usize, latch: Option<&[i32; 6]>) -> Plane {
    let c = &d.comps[ci];
    let (wb, hb) = (c.width_in_blocks, c.height_in_blocks);
    let stride = wb * 8;
    let mut samples = vec![0u8; stride * hb * 8];
    let zero = [0u16; 64];
    let quant = c.quant.as_ref().unwrap_or(&zero);
    let block = |r: usize, col: usize| {
        c.coefs
            .get(r * c.blocks_w + col)
            .copied()
            .unwrap_or([0; 64])
    };
    let mut out = [[0u8; 8]; 8];
    for r in 0..hb {
        for col in 0..wb {
            let mut coef = block(r, col);
            if let Some(bits) = latch {
                // The neighbours' DC values; past an edge, the block's own row or column (the sliding registers of the C).
                let pr = r.saturating_sub(1);
                let nr = if r + 1 < hb { r + 1 } else { r };
                let lc = col.saturating_sub(1);
                let rc = if col + 1 < wb { col + 1 } else { col };
                let dc = |rr: usize, cc: usize| i32::from(block(rr, cc)[0]);
                let (dc1, dc2, dc3) = (dc(pr, lc), dc(pr, col), dc(pr, rc));
                let (dc4, dc5, dc6) = (dc(r, lc), dc(r, col), dc(r, rc));
                let (dc7, dc8, dc9) = (dc(nr, lc), dc(nr, col), dc(nr, rc));
                let q = |i: usize| i32::from(quant[i]);
                let q00 = q(0);
                if bits[1] != 0 && coef[Q01] == 0 {
                    coef[Q01] = predict(q00, q(Q01), bits[1], 36, dc4.wrapping_sub(dc6));
                }
                if bits[2] != 0 && coef[Q10] == 0 {
                    coef[Q10] = predict(q00, q(Q10), bits[2], 36, dc2.wrapping_sub(dc8));
                }
                if bits[3] != 0 && coef[Q20] == 0 {
                    let g = dc2.wrapping_add(dc8).wrapping_sub(dc5.wrapping_mul(2));
                    coef[Q20] = predict(q00, q(Q20), bits[3], 9, g);
                }
                if bits[4] != 0 && coef[Q11] == 0 {
                    let g = dc1.wrapping_sub(dc3).wrapping_sub(dc7).wrapping_add(dc9);
                    coef[Q11] = predict(q00, q(Q11), bits[4], 5, g);
                }
                if bits[5] != 0 && coef[Q02] == 0 {
                    let g = dc4.wrapping_add(dc6).wrapping_sub(dc5.wrapping_mul(2));
                    coef[Q02] = predict(q00, q(Q02), bits[5], 9, g);
                }
            }
            islow(&coef, quant, &mut out);
            for (y, line) in out.iter().enumerate() {
                let at = (r * 8 + y) * stride + col * 8;
                if let Some(dst) = samples.get_mut(at..at + 8) {
                    dst.copy_from_slice(line);
                }
            }
        }
    }
    Plane {
        stride,
        samples,
        dw: c.downsampled_width,
        dh: c.downsampled_height,
    }
}

/// The sample `(x, y)` of the full-size image for one component, by its method.
fn upsampled(p: &Plane, m: Method, x: usize, y: usize) -> u8 {
    match m {
        Method::FullSize => p.at(y, x) as u8,
        Method::H2V1 => p.at(y, x / 2) as u8,
        Method::H2V2 => p.at(y / 2, x / 2) as u8,
        Method::Int { h, v } => p.at(y / v, x / h) as u8,
        Method::H2V1Fancy => {
            let i = x / 2;
            let here = p.at(y, i) * 3;
            if x.is_multiple_of(2) {
                if i == 0 {
                    p.at(y, 0) as u8
                } else {
                    ((here + p.at(y, i - 1) + 1) >> 2) as u8
                }
            } else if i + 1 >= p.dw {
                p.at(y, i) as u8
            } else {
                ((here + p.at(y, i + 1) + 2) >> 2) as u8
            }
        }
        Method::H2V2Fancy => {
            let row = y / 2;
            // The next-nearest row: above for an even output row, below for an odd one; the image's first and last rows stand
            // in for the missing ones.
            let other = if y.is_multiple_of(2) {
                row.saturating_sub(1)
            } else if row + 1 >= p.dh {
                p.dh.saturating_sub(1)
            } else {
                row + 1
            };
            let colsum = |i: usize| p.at(row, i) * 3 + p.at(other, i);
            let i = x / 2;
            let this = colsum(i);
            if x.is_multiple_of(2) {
                if i == 0 {
                    ((this * 4 + 8) >> 4) as u8
                } else {
                    ((this * 3 + colsum(i - 1) + 8) >> 4) as u8
                }
            } else if i + 1 >= p.dw {
                ((this * 4 + 7) >> 4) as u8
            } else {
                ((this * 3 + colsum(i + 1) + 7) >> 4) as u8
            }
        }
    }
}

/// libjpeg's "simple" range-limit table for the colour converter: below 0 is 0, above 255 is 255 over the range the converter can
/// reach (`y + Cr_r_tab[cr]` and its kin stay within -256..=639).
fn simple_limit(x: i32) -> u8 {
    x.clamp(0, 255) as u8
}

/// `build_ycc_rgb_table`'s entries for one chroma sample.
fn fix(x: f64) -> i32 {
    (x * 65536.0 + 0.5) as i32
}

/// The image's bits: one byte (greyscale) or three (RGB) per pixel.
pub(super) fn render(d: &mut Decoder<'_>, cs: ColorSpace) -> Result<Vec<u8>, JpegError> {
    let latch = if d.progressive { smoothing(d) } else { None };
    d.info.smoothed = latch.is_some();
    let mut planes = Vec::with_capacity(d.comps.len());
    let mut methods = Vec::with_capacity(d.comps.len());
    for ci in 0..d.comps.len() {
        planes.push(build_plane(d, ci, latch.as_ref().and_then(|l| l.get(ci))));
        methods.push(method(d, ci)?);
    }
    let (w, h) = (d.width as usize, d.height as usize);
    let mut out = Vec::with_capacity(w * h * if cs == ColorSpace::Gray { 1 } else { 3 });
    match cs {
        ColorSpace::Gray => {
            for y in 0..h {
                for x in 0..w {
                    out.push(upsampled(&planes[0], methods[0], x, y));
                }
            }
        }
        ColorSpace::Rgb => {
            for y in 0..h {
                for x in 0..w {
                    for k in 0..3 {
                        out.push(upsampled(&planes[k], methods[k], x, y));
                    }
                }
            }
        }
        ColorSpace::Ycc => {
            let (cr_r, cb_b, cr_g, cb_g) = (fix(1.40200), fix(1.77200), fix(0.71414), fix(0.34414));
            for y in 0..h {
                for x in 0..w {
                    let yy = i32::from(upsampled(&planes[0], methods[0], x, y));
                    let cb = i32::from(upsampled(&planes[1], methods[1], x, y)) - 128;
                    let cr = i32::from(upsampled(&planes[2], methods[2], x, y)) - 128;
                    let r = (cr_r * cr + (1 << 15)) >> 16;
                    let b = (cb_b * cb + (1 << 15)) >> 16;
                    let g = ((-cb_g) * cb + (1 << 15) + (-cr_g) * cr) >> 16;
                    out.push(simple_limit(yy + r));
                    out.push(simple_limit(yy + g));
                    out.push(simple_limit(yy + b));
                }
            }
        }
    }
    Ok(out)
}
