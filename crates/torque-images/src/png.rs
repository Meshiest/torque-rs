//! PNG as TGE 1.4 loads it: `GBitmap::readPNG` (`dgl/bitmapPng.cc`) over libpng 1.2. `readPNG` is the Torque Game Engine's,
//! carried into the MIT-licensed Torque3D as `sReadPNG` in `gfx/bitmap/loaders/bitmapPng.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the chunk and transform rules below follow libpng 1.2.8's behaviour, and no libpng code is carried over.
//!
//! **What `readPNG` asks libpng for**, and so what the bits are:
//! - `png_set_strip_16` for 16-bit samples: the high byte of each (libpng 1.2's `png_do_chop` without the accurate-scale option).
//! - `png_set_expand` whenever the file has a valid `tRNS`, and for every colour type: palette indices become RGB (RGBA with a
//!   `tRNS`), grey of 1, 2 or 4 bits is scaled to 8 (`x 0xff`, `x 0x55`, `x 0x11`), and a `tRNS` colour or grey value becomes an alpha
//!   of 0 (every other pixel 0xff). libpng runs the expansion BEFORE the strip, so a 16-bit `tRNS` value is compared at 16 bits.
//! - `png_set_gray_to_rgb` for grey with alpha only.
//! - Formats: palette and RGB are [`Format::Rgb`], or [`Format::Rgba`] with a `tRNS`; grey with alpha and RGBA are `Rgba`; plain grey
//!   is [`Format::Alpha`] (one byte, uploaded as `GL_ALPHA`).
//! - **No gamma step**: `readPNG` never calls `png_set_gamma`, so `gAMA`, `sRGB`, `iCCP` and `sBIT` change nothing.
//!
//! **Grey with a `tRNS`** is a hole in `readPNG`: the expansion gives grey+alpha (two bytes a pixel, the row stride libpng reports),
//! but the format stays `Alpha`, so the bitmap holds `width * height` bytes while libpng writes rows `2 * width` apart (past the end of
//! the allocation). The texels uploaded are the first `width * height` bytes of that grey+alpha stream; this module
//! returns exactly those (the overflow itself is not modelled). Ordinary files do not take this path.
//!
//! **The chunk walk.** libpng and the `png` crate disagree on ancillary chunks: libpng drops a bad one with a warning (a CRC error, a
//! `tRNS` of the wrong length, a duplicate, one after `IDAT`, one for a colour type with alpha) where the crate fails, and libpng keeps a
//! `tRNS` value's full 16 bits where the crate keeps its low byte. So [`decode`] walks the chunks itself: the critical ones go to the
//! crate unchanged (it checks their CRCs, the order and the zlib stream, Adler-32 included, as libpng and zlib do), every ancillary one
//! is dropped, and `tRNS` is read here by libpng 1.2.8's `png_handle_tRNS` rules.
//!
//! **A `tRNS` whose CRC fails** is dropped by libpng after it has already stored the value (`num_trans`, `trans_values`), so
//! `png_get_valid` says no transparency but `png_set_expand` still adds the alpha channel. A grey or RGB image then comes out of libpng
//! with alpha while `readPNG` picked the format without it, and the bitmap holds the first `width * height * bpp` bytes of the wider
//! stream (the same hole as grey with a `tRNS`). A palette image is stranger: `png_do_expand_palette` finds no transparency table and
//! writes RGB, but the row stride libpng reports (and `readPNG` places the rows at) is that of RGBA, so the bitmap holds each row's
//! `3 * width` bytes followed by `width` bytes of `allocateBitmap`'s `0xFF` fill, cut at `width * height * 3`.
//!
//! **Caps:** [`MAX_PNG_HEIGHT`] is TGE's own (`readPNG`'s row-pointer table holds 2,048 rows and `AssertISV`s above it);
//! [`MAX_PNG_WIDTH`] and [`MAX_PNG_PIXELS`] are this decoder's own. libpng errors (`AssertISV` in TGE) are [`ImageError::Png`].

use std::io::Cursor;

use crate::{Bitmap, Format, ImageError};

/// The most rows: `csgMaxRowPointers = 1 << (c_maxMipLevels - 1)`, 2,048 (`readPNG` asserts fatally above it).
pub const MAX_PNG_HEIGHT: u32 = 2048;
/// The widest image this decoder reads.
pub const MAX_PNG_WIDTH: u32 = 16_384;
/// The most pixels this decoder reads.
pub const MAX_PNG_PIXELS: u64 = 1 << 24;
/// The most bytes of chunk data the walk copies for the crate (the file itself bounds it; this keeps a hostile length field honest).
pub const MAX_PNG_FILE_BYTES: usize = 256 << 20;

const SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

/// Decodes a PNG into the bits `readPNG` leaves.
pub fn decode(bytes: &[u8]) -> Result<Bitmap, ImageError> {
    let walked = walk(bytes)?;
    let raw = decode_raw(&walked.critical, walked.idat_bytes)?;
    expand(&raw, &walked)
}

/// The size `IHDR` gives (see [`crate::size_of`]): the first chunk after the signature, 13 bytes, within the caps.
pub fn header_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..8) != Some(&SIGNATURE[..])
        || be32(bytes, 8)? != 13
        || bytes.get(12..16)? != b"IHDR"
    {
        return None;
    }
    let (w, h) = (be32(bytes, 16)?, be32(bytes, 20)?);
    let ok = w > 0
        && h > 0
        && w <= MAX_PNG_WIDTH
        && h <= MAX_PNG_HEIGHT
        && u64::from(w) * u64::from(h) <= MAX_PNG_PIXELS;
    ok.then_some((w, h))
}

fn err(m: impl Into<String>) -> ImageError {
    ImageError::Png(m.into())
}

/// What the chunk walk kept.
struct Walked {
    /// The signature and the critical chunks, for the crate.
    critical: Vec<u8>,
    color_type: u8,
    bit_depth: u8,
    num_palette: usize,
    /// The transparency libpng holds (`num_trans` and `trans_values`), and whether a `tRNS` passed its CRC (`PNG_INFO_tRNS`).
    trns: Option<Trns>,
    trns_valid: bool,
    /// The bytes of every IDAT chunk together.
    idat_bytes: usize,
}

/// A `tRNS` libpng accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Trns {
    Gray(u16),
    Rgb([u16; 3]),
    /// The palette alphas (`num_trans` of them).
    Palette(Vec<u8>),
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// The chunk walk (see the module docs). libpng reads every chunk to `IEND` (`png_read_end`); bytes after `IEND` are never read.
fn walk(bytes: &[u8]) -> Result<Walked, ImageError> {
    if bytes.len() > MAX_PNG_FILE_BYTES {
        return Err(ImageError::Cap {
            what: "png file bytes",
            value: bytes.len() as u64,
            cap: MAX_PNG_FILE_BYTES as u64,
        });
    }
    if bytes.get(..8) != Some(&SIGNATURE[..]) {
        return Err(err("not a PNG signature"));
    }
    let mut out = Walked {
        critical: SIGNATURE.to_vec(),
        color_type: 0,
        bit_depth: 0,
        num_palette: 0,
        trns: None,
        trns_valid: false,
        idat_bytes: 0,
    };
    let (mut have_ihdr, mut have_plte, mut have_idat) = (false, false, false);
    let mut at = 8usize;
    loop {
        let length = be32(bytes, at).ok_or_else(|| err("the file ended inside a chunk"))?;
        // `png_get_uint_31`: a length above 2^31 - 1 is an error.
        if length > 0x7fff_ffff {
            return Err(err("chunk length out of range"));
        }
        let length = length as usize;
        let name_at = at + 4;
        let name: [u8; 4] = bytes
            .get(name_at..name_at + 4)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| err("the file ended inside a chunk"))?;
        // `png_check_chunk_name`: four ASCII letters.
        if !name.iter().all(|c| c.is_ascii_alphabetic()) {
            return Err(err("invalid chunk type"));
        }
        let data_at = name_at + 4;
        let end = data_at
            .checked_add(length)
            .and_then(|e| e.checked_add(4))
            .ok_or_else(|| err("chunk length out of range"))?;
        let chunk = bytes
            .get(at..end)
            .ok_or_else(|| err("the file ended inside a chunk"))?;
        let data = &chunk[8..8 + length];
        let crc_ok = be32(chunk, 8 + length) == Some(crc32(&chunk[4..8 + length]));
        let critical = name[0] & 0x20 == 0;
        if !have_ihdr && &name != b"IHDR" {
            return Err(err("missing IHDR"));
        }
        match &name {
            b"IHDR" => {
                if have_ihdr {
                    return Err(err("out of place IHDR"));
                }
                have_ihdr = true;
                if let (Some(&depth), Some(&ct)) = (data.get(8), data.get(9)) {
                    out.bit_depth = depth;
                    out.color_type = ct;
                }
            }
            b"PLTE" => {
                have_plte = true;
                out.num_palette = (length / 3).min(256);
            }
            b"IDAT" => {
                have_idat = true;
                out.idat_bytes += length;
            }
            _ => {}
        }
        if critical {
            out.critical.extend_from_slice(chunk);
        } else if &name == b"tRNS"
            && !have_idat
            && !out.trns_valid
            && let Some(t) = trns(&out, have_plte, data)
        {
            out.trns = Some(t);
            out.trns_valid = crc_ok;
        }
        at = end;
        if &name == b"IEND" {
            return Ok(out);
        }
    }
}

/// `png_handle_tRNS` (libpng 1.2.8): the value it stores, or `None` where it warns and drops the chunk first. The CRC is checked
/// after the value is stored (the caller keeps both).
fn trns(w: &Walked, have_plte: bool, data: &[u8]) -> Option<Trns> {
    let u16_at = |i: usize| u16::from_be_bytes([data[i], data[i + 1]]);
    match w.color_type {
        0 if data.len() == 2 => Some(Trns::Gray(u16_at(0))),
        2 if data.len() == 6 => Some(Trns::Rgb([u16_at(0), u16_at(2), u16_at(4)])),
        3 => {
            // Without a PLTE first, `num_palette` is 0 and every length is "incorrect".
            let num_palette = if have_plte { w.num_palette } else { 0 };
            if data.len() > num_palette || data.is_empty() {
                return None;
            }
            Some(Trns::Palette(data.to_vec()))
        }
        _ => None,
    }
}

/// CRC-32 of a chunk's name and data (the PNG polynomial).
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The raw rows as the file stores them (big-endian samples, sub-byte samples packed), deinterlaced.
struct Raw {
    width: u32,
    height: u32,
    stride: usize,
    data: Vec<u8>,
}

/// Deflate expands at most 1,032 to 1: an image whose rows need more than that of its IDAT bytes cannot decode (libpng: "Not enough
/// image data"), so it is refused before the row buffer is allocated.
const DEFLATE_MAX_RATIO: usize = 1100;

fn decode_raw(critical: &[u8], idat_bytes: usize) -> Result<Raw, ImageError> {
    let mut options = ::png::DecodeOptions::default();
    // libpng's zlib checks the Adler-32 at the end of the stream; libpng reads the stream to its end (`png_read_finish_row`).
    options.set_ignore_adler32(false);
    let mut decoder = ::png::Decoder::new_with_options(Cursor::new(critical), options);
    decoder.set_limits(::png::Limits {
        bytes: (MAX_PNG_PIXELS as usize) * 8 + (1 << 20),
    });
    decoder.set_transformations(::png::Transformations::IDENTITY);
    let header = decoder.read_header_info().map_err(|e| err(e.to_string()))?;
    let (width, height) = (header.width, header.height);
    if height > MAX_PNG_HEIGHT {
        return Err(ImageError::Cap {
            what: "png height",
            value: u64::from(height),
            cap: u64::from(MAX_PNG_HEIGHT),
        });
    }
    if width > MAX_PNG_WIDTH {
        return Err(ImageError::Cap {
            what: "png width",
            value: u64::from(width),
            cap: u64::from(MAX_PNG_WIDTH),
        });
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_PNG_PIXELS {
        return Err(ImageError::Cap {
            what: "png pixels",
            value: pixels,
            cap: MAX_PNG_PIXELS,
        });
    }
    let mut reader = decoder.read_info().map_err(|e| err(e.to_string()))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| err("image too large"))?;
    if size
        > idat_bytes
            .saturating_mul(DEFLATE_MAX_RATIO)
            .saturating_add(1024)
    {
        return Err(err("not enough image data"));
    }
    let stride = reader
        .output_line_size(width)
        .ok_or_else(|| err("image too large"))?;
    let mut data = vec![0u8; size];
    reader
        .next_frame(&mut data)
        .map_err(|e| err(e.to_string()))?;
    reader.finish().map_err(|e| err(e.to_string()))?;
    Ok(Raw {
        width,
        height,
        stride,
        data,
    })
}

/// Sample `x` of a row of one-channel samples of `depth` bits (big-endian, packed from the high bit).
fn packed(row: &[u8], x: usize, depth: u8) -> u16 {
    match depth {
        1 | 2 | 4 => {
            let bit = x * usize::from(depth);
            let byte = row.get(bit / 8).copied().unwrap_or(0);
            let shift = 8 - usize::from(depth) - bit % 8;
            u16::from((byte >> shift) & ((1u8 << depth) - 1))
        }
        8 => u16::from(row.get(x).copied().unwrap_or(0)),
        _ => {
            let i = x * 2;
            u16::from_be_bytes([
                row.get(i).copied().unwrap_or(0),
                row.get(i + 1).copied().unwrap_or(0),
            ])
        }
    }
}

/// A 16-bit sample of an n-channel row: channel `c` of pixel `x`.
fn wide(row: &[u8], x: usize, channels: usize, c: usize) -> u16 {
    let i = (x * channels + c) * 2;
    u16::from_be_bytes([
        row.get(i).copied().unwrap_or(0),
        row.get(i + 1).copied().unwrap_or(0),
    ])
}

/// A grey sample of fewer than 8 bits scaled to 8 (`png_do_expand`), an 8-bit one as it is, a 16-bit one's high byte.
fn gray8(v: u16, depth: u8) -> u8 {
    match depth {
        1 => (v & 1) as u8 * 0xff,
        2 => (v & 3) as u8 * 0x55,
        4 => (v & 0xf) as u8 * 0x11,
        8 => v as u8,
        _ => (v >> 8) as u8,
    }
}

fn expand(raw: &Raw, w: &Walked) -> Result<Bitmap, ImageError> {
    let (width, height) = (raw.width as usize, raw.height as usize);
    let depth = w.bit_depth;
    let rows = (0..height).map(|y| {
        let at = y * raw.stride;
        raw.data.get(at..at + raw.stride).unwrap_or(&[])
    });
    let mut bits = Vec::new();
    let format = match w.color_type {
        // Grey.
        0 => match &w.trns {
            None => {
                bits.reserve(width * height);
                for row in rows {
                    for x in 0..width {
                        bits.push(gray8(packed(row, x, depth), depth));
                    }
                }
                Format::Alpha
            }
            Some(t) => {
                let key = match t {
                    Trns::Gray(g) => *g,
                    _ => return Err(err("tRNS of another colour type")),
                };
                // `png_do_expand` (1.2.8) scales the key like a sample, unmasked (`gray * 0x11` for 4 bits, in 16 bits), and compares
                // it with the scaled byte: a key with bits above the depth never matches. At 8 bits the byte meets the 16-bit key, at
                // 16 bits both words. (Later 1.2.x releases mask the key to the depth first; this follows 1.2.8.)
                let scaled_key = match depth {
                    1 => key.wrapping_mul(0xff),
                    2 => key.wrapping_mul(0x55),
                    4 => key.wrapping_mul(0x11),
                    _ => key,
                };
                let mut ga = Vec::with_capacity(width * height * 2);
                for row in rows {
                    for x in 0..width {
                        let v = packed(row, x, depth);
                        let g = gray8(v, depth);
                        let sample = if depth == 16 { v } else { u16::from(g) };
                        ga.push(g);
                        ga.push(if sample == scaled_key { 0 } else { 0xff });
                    }
                }
                // The bitmap holds width * height bytes; libpng's rows land 2 * width apart (module docs).
                ga.truncate(width * height);
                bits = ga;
                Format::Alpha
            }
        },
        // RGB.
        2 => {
            let key = match &w.trns {
                Some(Trns::Rgb(k)) => Some(*k),
                Some(_) => return Err(err("tRNS of another colour type")),
                None => None,
            };
            bits.reserve(width * height * if key.is_some() { 4 } else { 3 });
            for row in rows {
                for x in 0..width {
                    let s = |c: usize| -> u16 {
                        if depth == 16 {
                            wide(row, x, 3, c)
                        } else {
                            u16::from(row.get(x * 3 + c).copied().unwrap_or(0))
                        }
                    };
                    let px = [s(0), s(1), s(2)];
                    for v in px {
                        bits.push(if depth == 16 { (v >> 8) as u8 } else { v as u8 });
                    }
                    if let Some(k) = key {
                        bits.push(if px == k { 0 } else { 0xff });
                    }
                }
            }
            match (key.is_some(), w.trns_valid) {
                (true, true) => Format::Rgba,
                (true, false) => {
                    // libpng expanded to RGBA; the bitmap holds the first width * height * 3 bytes (module docs).
                    bits.truncate(width * height * 3);
                    Format::Rgb
                }
                _ => Format::Rgb,
            }
        }
        // Palette.
        3 => {
            let palette = palette_of(&w.critical);
            let alphas = match &w.trns {
                Some(Trns::Palette(a)) => Some(a.as_slice()),
                Some(_) => return Err(err("tRNS of another colour type")),
                None => None,
            };
            bits.reserve(width * height * if alphas.is_some() { 4 } else { 3 });
            for row in rows {
                for x in 0..width {
                    let i = usize::from(packed(row, x, depth));
                    // libpng 1.2.1+ keeps a 256-entry palette, zeroed past `num_palette`.
                    let rgb = if i < w.num_palette {
                        palette.get(i * 3..i * 3 + 3).unwrap_or(&[0, 0, 0])
                    } else {
                        &[0, 0, 0]
                    };
                    bits.extend_from_slice(rgb);
                    match alphas {
                        Some(a) if w.trns_valid => bits.push(a.get(i).copied().unwrap_or(0xff)),
                        _ => {}
                    }
                }
            }
            match (alphas.is_some(), w.trns_valid) {
                (true, true) => Format::Rgba,
                (true, false) => {
                    // RGB rows at the RGBA stride over the 0xFF fill (module docs).
                    let mut laid = vec![0xffu8; width * height * 3];
                    for (y, row) in bits.chunks(width * 3).enumerate() {
                        let at = y * width * 4;
                        if at >= laid.len() {
                            break;
                        }
                        let n = row.len().min(laid.len() - at);
                        laid[at..at + n].copy_from_slice(&row[..n]);
                    }
                    bits = laid;
                    Format::Rgb
                }
                _ => Format::Rgb,
            }
        }
        // Grey with alpha: `png_set_gray_to_rgb` after the strip.
        4 => {
            bits.reserve(width * height * 4);
            for row in rows {
                for x in 0..width {
                    let (g, a) = if depth == 16 {
                        (
                            (wide(row, x, 2, 0) >> 8) as u8,
                            (wide(row, x, 2, 1) >> 8) as u8,
                        )
                    } else {
                        (
                            row.get(x * 2).copied().unwrap_or(0),
                            row.get(x * 2 + 1).copied().unwrap_or(0),
                        )
                    };
                    bits.extend_from_slice(&[g, g, g, a]);
                }
            }
            Format::Rgba
        }
        // RGBA.
        6 => {
            bits.reserve(width * height * 4);
            for row in rows {
                for x in 0..width {
                    for c in 0..4 {
                        bits.push(if depth == 16 {
                            (wide(row, x, 4, c) >> 8) as u8
                        } else {
                            row.get(x * 4 + c).copied().unwrap_or(0)
                        });
                    }
                }
            }
            Format::Rgba
        }
        _ => return Err(err("invalid colour type")),
    };
    Ok(Bitmap {
        width: raw.width,
        height: raw.height,
        format,
        bits,
    })
}

/// The palette entries of the (first) `PLTE` chunk of the critical stream.
fn palette_of(critical: &[u8]) -> Vec<u8> {
    let mut at = 8usize;
    while let Some(length) = be32(critical, at) {
        let length = length as usize;
        let name = critical.get(at + 4..at + 8);
        let data = critical.get(at + 8..at + 8 + length);
        if name == Some(b"PLTE") {
            return data
                .map(|d| d[..d.len() / 3 * 3].to_vec())
                .unwrap_or_default();
        }
        at = at + 12 + length;
    }
    Vec::new()
}

#[cfg(test)]
mod tests;
