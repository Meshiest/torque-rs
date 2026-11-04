//! TGE's mip chain: `GBitmap::extrudeMipLevels` (TGE 1.4 `dgl/gBitmap.cc`), which `TextureManager::registerTexture` runs on every
//! texture but the bitmap, terrain and palettised kinds. This follows the Torque Game Engine's `dgl/gBitmap.cc` and
//! `platformWin32/winAsmBlit.cc` (`bitmapExtrudeRGB_mmx`) (TGE 1.4, Copyright (C) GarageGames.com,
//! Inc.; the Torque engine is MIT-licensed as Torque3D, Copyright (c) 2012 GarageGames, LLC); the
//! code here is this crate's own.
//!
//! **The buffer.** `allocateBitmap(w, h, true, format)` lays every level end to end in one allocation, halving each side (floor, at
//! least 1) down to 1 x 1 (its loop is a do-while: a 1 x 1 bitmap has two levels), and fills it with `0xFF`; level 0 is then copied
//! back in. The extrude functions read and write through raw pointers into that one buffer, so this module emulates them over one
//! `Vec<u8>` with the same offsets and in the same order: where TGE's arithmetic strays past a level (an odd width, the width-1
//! path's stride, the MMX loop's fourth lane) the same bytes are read and written here. A read past the END of the allocation (which
//! TGE would make into whatever the heap holds) is 0.
//!
//! **The functions** (by format, as `extrudeMipLevels`' switch picks them):
//! - RGB: `bitmapExtrudeRGB`, a function pointer. The C version averages each 2 x 2 square with rounding (`+ 2 >> 2`, `+ 1 >> 1` for a
//!   one-pixel-wide level); TGE 1.4 Win32's `PlatformBlitInit` swaps in `bitmapExtrudeRGB_mmx` on any CPU with MMX, which truncates
//!   (a right shift by 2 with no bias) and falls back to the C version for a level one pixel high or at most three wide. [`RgbExtrude`]
//!   chooses; [`RgbExtrude::Mmx`] is the default. Which of the two a given
//!   client ends up with depends on its platform initialisation, so both are provided.
//! - RGBA: `bitmapExtrudeRGBA_c` (no MMX version exists in TGE 1.4).
//! - Alpha: no case in the switch: the levels keep `allocateBitmap`'s `0xFF`.
//! - The width-1 path of both C versions advances the source by 6 (RGB) or 8 (RGBA) bytes per row instead of 3 or 4, then by the
//!   stride: kept.
//!
//! `clear_borders` is `extrudeMipLevels(true)` (a `ZeroBorderTexture`): every level above 0 has its outer ring of pixels zeroed (a
//! level under 3 x 3 entirely). **Cap:** [`MAX_MIP_LEVELS`], TGE's `c_maxMipLevels` (`mipLevelOffsets` holds 12; a 4,096-pixel
//! side would make 13 and write past the array).

use crate::{Bitmap, Format, ImageError};

/// `GBitmap::c_maxMipLevels`.
pub const MAX_MIP_LEVELS: usize = 12;

/// Which `bitmapExtrudeRGB` TGE's function pointer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RgbExtrude {
    /// `bitmapExtrudeRGB_c`.
    C,
    /// `bitmapExtrudeRGB_mmx` (TGE 1.4 Win32 with MMX).
    #[default]
    Mmx,
}

/// One level of a chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level {
    pub width: u32,
    pub height: u32,
    /// Its first byte in [`MipChain::bits`] (`mipLevelOffsets`).
    pub offset: usize,
}

/// A bitmap with its mip levels, laid out as `GBitmap` holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MipChain {
    pub format: Format,
    pub levels: Vec<Level>,
    pub bits: Vec<u8>,
}

impl MipChain {
    /// The bytes of level `i`.
    pub fn level(&self, i: usize) -> Option<&[u8]> {
        let l = self.levels.get(i)?;
        let len = l.width as usize * l.height as usize * self.format.bytes_per_pixel();
        self.bits.get(l.offset..l.offset + len)
    }
}

/// The buffer the extrude functions run over: reads past the end are 0, writes past it are dropped.
struct Mem {
    bits: Vec<u8>,
}

impl Mem {
    fn rd(&self, at: usize) -> u32 {
        u32::from(self.bits.get(at).copied().unwrap_or(0))
    }

    fn wr(&mut self, at: usize, v: u32) {
        if let Some(b) = self.bits.get_mut(at) {
            *b = v as u8;
        }
    }
}

/// `allocateBitmap`'s level layout for `w x h`: (width, height, offset) per level, and the total bytes.
fn layout(w: u32, h: u32, bpp: usize) -> Result<(Vec<Level>, usize), ImageError> {
    let mut levels = vec![Level {
        width: w,
        height: h,
        offset: 0,
    }];
    let (mut cw, mut ch) = (w, h);
    let mut total = w as usize * h as usize * bpp;
    // A do-while: even a 1 x 1 bitmap gets a second (1 x 1) level.
    loop {
        cw = (cw >> 1).max(1);
        ch = (ch >> 1).max(1);
        levels.push(Level {
            width: cw,
            height: ch,
            offset: 0,
        });
        total += cw as usize * ch as usize * bpp;
        if levels.len() > MAX_MIP_LEVELS {
            return Err(ImageError::Cap {
                what: "mip levels",
                value: levels.len() as u64,
                cap: MAX_MIP_LEVELS as u64,
            });
        }
        if cw == 1 && ch == 1 {
            break;
        }
    }
    // `mipLevelOffsets[n]` is the previous level's offset plus its size.
    let mut offset = 0usize;
    for l in &mut levels {
        l.offset = offset;
        offset += l.width as usize * l.height as usize * bpp;
    }
    Ok((levels, total))
}

/// `GBitmap::extrudeMipLevels(clear_borders)` on a decoded bitmap.
pub fn extrude(b: &Bitmap, rgb: RgbExtrude, clear_borders: bool) -> Result<MipChain, ImageError> {
    let bpp = b.format.bytes_per_pixel();
    if b.width == 0 || b.height == 0 || b.bits.len() != b.width as usize * b.height as usize * bpp {
        return Err(ImageError::Invalid("bitmap size does not match its bits"));
    }
    let (levels, total) = layout(b.width, b.height, bpp)?;
    let mut mem = Mem {
        bits: vec![0xff; total],
    };
    mem.bits[..b.bits.len()].copy_from_slice(&b.bits);
    for i in 1..levels.len() {
        let (src, dst) = (levels[i - 1], levels[i]);
        let (sh, sw) = (src.height as usize, src.width as usize);
        match (b.format, rgb) {
            (Format::Rgb, RgbExtrude::C) => extrude_rgb_c(&mut mem, src.offset, dst.offset, sh, sw),
            (Format::Rgb, RgbExtrude::Mmx) => {
                extrude_rgb_mmx(&mut mem, src.offset, dst.offset, sh, sw)
            }
            (Format::Rgba, _) => extrude_rgba_c(&mut mem, src.offset, dst.offset, sh, sw),
            (Format::Alpha, _) => {}
        }
    }
    if clear_borders {
        for l in levels.iter().skip(1) {
            clear_level_borders(&mut mem, l, bpp);
        }
    }
    Ok(MipChain {
        format: b.format,
        levels,
        bits: mem.bits,
    })
}

/// `bitmapExtrudeRGB_c`.
fn extrude_rgb_c(m: &mut Mem, src0: usize, dst0: usize, src_h: usize, src_w: usize) {
    let stride = if src_h != 1 { src_w * 3 } else { 0 };
    let width = (src_w >> 1).max(1);
    let height = (src_h >> 1).max(1);
    let (mut s, mut d) = (src0, dst0);
    if src_w != 1 {
        for _ in 0..height {
            for _ in 0..width {
                for step in [1, 1, 4] {
                    let v =
                        (m.rd(s) + m.rd(s + 3) + m.rd(s + stride) + m.rd(s + stride + 3) + 2) >> 2;
                    m.wr(d, v);
                    d += 1;
                    s += step;
                }
            }
            s += stride;
        }
    } else {
        for _ in 0..height {
            for step in [1, 1, 4] {
                let v = (m.rd(s) + m.rd(s + stride) + 1) >> 1;
                m.wr(d, v);
                d += 1;
                s += step;
            }
            s += stride;
        }
    }
}

/// `bitmapExtrudeRGBA_c`.
fn extrude_rgba_c(m: &mut Mem, src0: usize, dst0: usize, src_h: usize, src_w: usize) {
    let stride = if src_h != 1 { src_w * 4 } else { 0 };
    let width = (src_w >> 1).max(1);
    let height = (src_h >> 1).max(1);
    let (mut s, mut d) = (src0, dst0);
    if src_w != 1 {
        for _ in 0..height {
            for _ in 0..width {
                for step in [1, 1, 1, 5] {
                    let v =
                        (m.rd(s) + m.rd(s + 4) + m.rd(s + stride) + m.rd(s + stride + 4) + 2) >> 2;
                    m.wr(d, v);
                    d += 1;
                    s += step;
                }
            }
            s += stride;
        }
    } else {
        for _ in 0..height {
            for step in [1, 1, 1, 5] {
                let v = (m.rd(s) + m.rd(s + stride) + 1) >> 1;
                m.wr(d, v);
                d += 1;
                s += step;
            }
            s += stride;
        }
    }
}

/// `bitmapExtrudeRGB_mmx`: four byte lanes at a time (three channels and a fourth the next pixel overwrites), summed as words and
/// shifted right by 2 with no rounding, stored as one 4-byte write.
fn extrude_rgb_mmx(m: &mut Mem, src0: usize, dst0: usize, src_h: usize, src_w: usize) {
    if src_h == 1 || src_w == 1 {
        return extrude_rgb_c(m, src0, dst0, src_h, src_w);
    }
    let width = src_w >> 1;
    let height = src_h >> 1;
    if width <= 1 {
        return extrude_rgb_c(m, src0, dst0, src_h, src_w);
    }
    let src_stride = (width << 1) * 3;
    let dst_stride = width * 3;
    let (mut src, mut dst) = (src0, dst0);
    for _ in 0..height {
        let (mut a, mut b, mut c) = (src, src + src_stride, dst);
        for _ in 0..width {
            let mut lanes = [0u32; 4];
            for (k, lane) in lanes.iter_mut().enumerate() {
                *lane = (m.rd(a + k) + m.rd(a + 3 + k) + m.rd(b + k) + m.rd(b + 3 + k)) >> 2;
            }
            for (k, lane) in lanes.iter().enumerate() {
                // Packing saturates a word to a byte; a sum of four bytes over 4 never exceeds 255.
                m.wr(c + k, (*lane).min(255));
            }
            a += 6;
            b += 6;
            c += 3;
        }
        src += src_stride + src_stride;
        dst += dst_stride;
    }
}

/// The `clearBorders` loop of `extrudeMipLevels` for one level.
fn clear_level_borders(m: &mut Mem, l: &Level, bpp: usize) {
    let (w, h) = (l.width as usize, l.height as usize);
    let zero = |m: &mut Mem, at: usize, len: usize| {
        for i in 0..len {
            m.wr(at + i, 0);
        }
    };
    if h < 3 || w < 3 {
        zero(m, l.offset, w * h * bpp);
        return;
    }
    let row = w * bpp;
    let base = l.offset as isize;
    let end = base + ((h - 1) * row) as isize - bpp as isize;
    zero(m, l.offset, row - bpp);
    let mut bytes = base - bpp as isize;
    while bytes < end {
        bytes += row as isize;
        zero(m, bytes as usize, bpp * 2);
    }
    zero(m, (bytes + 2 * bpp as isize) as usize, row - bpp);
}

#[cfg(test)]
mod tests;
