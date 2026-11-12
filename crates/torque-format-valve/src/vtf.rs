//! VTF textures: a header (`VTF\0`, version 7.0 to 7.5), a small low-resolution image, and the full image's mip levels from the
//! smallest to the largest, each holding every frame, face and depth slice.
//!
//! **Header** (packed): the version, the header's size, width and height (`u16`), the flags, the frame count and first frame,
//! the reflectivity, the bump scale, the image format, the mip count, the low-resolution format and size; 7.2 adds the depth,
//! 7.3 a list of resources (a tag of three bytes, a flag byte and a word: tag `0x30` gives the image data's offset, `0x01` the
//! low-resolution image's). Before 7.3 the image data follows the header and the low-resolution image. An environment map
//! (flag `0x4000`) has 6 faces, or 7 (a sphere map last) in files before 7.5 whose first frame is not `0xffff`.
//!
//! [`Vtf::parse`] reads the header and finds every level's bytes; [`Vtf::decode`] makes RGBA8 levels of the first frame, face
//! and slice from the common formats: DXT1/3/5, ATI1N and ATI2N, the 8-bit RGBA orders, 565, 4444 and 5551, I8, IA88, A8, UV88
//! and the 16-bit float HDR format (clamped to 0..1). The decoded size is capped ([`MAX_PIXELS`]).

use crate::read::{Error, Result, cap, f16, f32_at, i32_at, invalid, slice, u8_at, u16_at, u32_at};

/// The most pixels of a level decoded (the top level of a 4096 x 4096 texture).
pub const MAX_PIXELS: u64 = 1 << 24;
/// The largest side.
pub const MAX_SIDE: u32 = 16384;
/// The largest file read.
pub const MAX_FILE_BYTES: u64 = 128 << 20;

/// Texture flags (the ones a material reader looks at).
pub mod flags {
    pub const POINT_SAMPLE: u32 = 0x1;
    pub const CLAMP_S: u32 = 0x4;
    pub const CLAMP_T: u32 = 0x8;
    pub const NORMAL: u32 = 0x80;
    pub const NO_MIP: u32 = 0x100;
    pub const ONE_BIT_ALPHA: u32 = 0x1000;
    pub const EIGHT_BIT_ALPHA: u32 = 0x2000;
    pub const ENVMAP: u32 = 0x4000;
    pub const SSBUMP: u32 = 0x0800_0000;
}

/// The image formats read (the file's numbers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Rgba8888,
    Abgr8888,
    Rgb888,
    Bgr888,
    Rgb565,
    I8,
    Ia88,
    A8,
    Rgb888Bluescreen,
    Bgr888Bluescreen,
    Argb8888,
    Bgra8888,
    Dxt1,
    Dxt3,
    Dxt5,
    Bgrx8888,
    Bgr565,
    Bgrx5551,
    Bgra4444,
    Dxt1OneBitAlpha,
    Bgra5551,
    Uv88,
    Uvwq8888,
    Rgba16161616F,
    Rgba16161616,
    Uvlx8888,
    Ati2n,
    Ati1n,
    /// A format with no decoder here (the file's number): its levels' sizes are known when `bytes_per_pixel` is.
    Other(i32),
}

impl Format {
    pub fn from_raw(v: i32) -> Format {
        use Format::*;
        match v {
            0 => Rgba8888,
            1 => Abgr8888,
            2 => Rgb888,
            3 => Bgr888,
            4 => Rgb565,
            5 => I8,
            6 => Ia88,
            8 => A8,
            9 => Rgb888Bluescreen,
            10 => Bgr888Bluescreen,
            11 => Argb8888,
            12 => Bgra8888,
            13 => Dxt1,
            14 => Dxt3,
            15 => Dxt5,
            16 => Bgrx8888,
            17 => Bgr565,
            18 => Bgrx5551,
            19 => Bgra4444,
            20 => Dxt1OneBitAlpha,
            21 => Bgra5551,
            22 => Uv88,
            23 => Uvwq8888,
            24 => Rgba16161616F,
            25 => Rgba16161616,
            26 => Uvlx8888,
            37 => Ati2n,
            38 => Ati1n,
            v => Other(v),
        }
    }

    /// The bytes of a `w` x `h` image (None for a format whose size is not known).
    pub fn image_bytes(self, w: u32, h: u32) -> Option<u64> {
        use Format::*;
        let (w, h) = (u64::from(w.max(1)), u64::from(h.max(1)));
        let blocks = w.div_ceil(4) * h.div_ceil(4);
        Some(match self {
            Dxt1 | Dxt1OneBitAlpha | Ati1n => blocks * 8,
            Dxt3 | Dxt5 | Ati2n => blocks * 16,
            Rgba8888 | Abgr8888 | Argb8888 | Bgra8888 | Bgrx8888 | Uvwq8888 | Uvlx8888 => w * h * 4,
            Rgb888 | Bgr888 | Rgb888Bluescreen | Bgr888Bluescreen => w * h * 3,
            Rgb565 | Bgr565 | Bgrx5551 | Bgra4444 | Bgra5551 | Ia88 | Uv88 => w * h * 2,
            I8 | A8 => w * h,
            Rgba16161616F | Rgba16161616 => w * h * 8,
            Other(7) => w * h,
            Other(27) => w * h * 4,
            Other(28) => w * h * 12,
            Other(29) => w * h * 16,
            Other(_) => return None,
        })
    }
}

/// A texture's header and where its levels are.
#[derive(Debug, Clone, PartialEq)]
pub struct Vtf {
    pub version: (u32, u32),
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub flags: u32,
    pub frames: u32,
    pub faces: u32,
    pub mips: u32,
    pub format: Format,
    pub reflectivity: [f32; 3],
    /// The resources (7.3 and later): each tag, its flag byte and its word (an offset, or the data itself when the flags say
    /// it has no data chunk).
    pub resources: Vec<([u8; 3], u8, u32)>,
    /// The offset of each mip level's first image (level 0 the largest), and the bytes of one image of it.
    levels: Vec<(usize, usize)>,
}

/// A level decoded: RGBA8, `width * height * 4` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Level {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Vtf {
    /// Reads a texture's header and checks every level's bytes are in the file.
    pub fn parse(b: &[u8]) -> Result<Vtf> {
        cap("the VTF file's size", b.len() as u64, MAX_FILE_BYTES)?;
        if slice(b, 0, 4, "the VTF signature")? != b"VTF\0" {
            return invalid("not a VTF texture (bad signature)");
        }
        let major = u32_at(b, 4, "the VTF version")?;
        let minor = u32_at(b, 8, "the VTF version")?;
        if major != 7 || minor > 5 {
            return invalid(format!("VTF version {major}.{minor} is not read"));
        }
        let header_size = u32_at(b, 12, "the VTF header")? as usize;
        let width = u32::from(u16_at(b, 16, "the VTF header")?);
        let height = u32::from(u16_at(b, 18, "the VTF header")?);
        let flags = u32_at(b, 20, "the VTF header")?;
        let frames = u32::from(u16_at(b, 24, "the VTF header")?).max(1);
        let first_frame = u16_at(b, 26, "the VTF header")?;
        let reflectivity = [
            f32_at(b, 32, "the VTF header")?,
            f32_at(b, 36, "the VTF header")?,
            f32_at(b, 40, "the VTF header")?,
        ];
        let format = Format::from_raw(i32_at(b, 52, "the VTF header")?);
        let mips = u32::from(u8_at(b, 56, "the VTF header")?).max(1);
        let low_format = Format::from_raw(i32_at(b, 57, "the VTF header")?);
        let low_w = u32::from(u8_at(b, 61, "the VTF header")?);
        let low_h = u32::from(u8_at(b, 62, "the VTF header")?);
        let depth = if minor >= 2 {
            u32::from(u16_at(b, 63, "the VTF header")?).max(1)
        } else {
            1
        };
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return invalid(format!("a VTF of {width} x {height}"));
        }
        if mips > 16 {
            return invalid(format!("a VTF with {mips} mip levels"));
        }
        let faces = if flags & flags::ENVMAP != 0 {
            if minor < 5 && first_frame != 0xffff {
                7
            } else {
                6
            }
        } else {
            1
        };
        let mut data = None;
        let mut resources = Vec::new();
        if minor >= 3 {
            let n = u32_at(b, 68, "the VTF resources")?;
            if n > 32 {
                return invalid(format!("a VTF with {n} resources"));
            }
            for i in 0..n as usize {
                let at = 80 + i * 8;
                let tag = slice(b, at, 3, "a VTF resource")?;
                let word = u32_at(b, at + 4, "a VTF resource")?;
                if tag == [0x30, 0, 0] {
                    data = Some(word as usize);
                }
                resources.push((
                    [tag[0], tag[1], tag[2]],
                    u8_at(b, at + 3, "a VTF resource")?,
                    word,
                ));
            }
        }
        let data = match data {
            Some(d) => d,
            None => {
                let low = if low_w == 0 || low_h == 0 {
                    0
                } else {
                    low_format.image_bytes(low_w, low_h).unwrap_or(0)
                };
                header_size
                    .checked_add(low as usize)
                    .ok_or(Error::Truncated {
                        what: "the VTF image data",
                    })?
            }
        };
        let Some(_) = format.image_bytes(1, 1) else {
            return invalid(format!("VTF image format {format:?} is not read"));
        };
        let per_image = |level: u32| -> u64 {
            format
                .image_bytes((width >> level).max(1), (height >> level).max(1))
                .unwrap_or(0)
                * u64::from((depth >> level).max(1))
        };
        // The levels lie smallest first; each holds frames x faces x slices images.
        let copies = u64::from(frames) * u64::from(faces);
        let mut levels = vec![(0usize, 0usize); mips as usize];
        let mut at = data as u64;
        for level in (0..mips).rev() {
            let one = per_image(level);
            let slice_bytes = format
                .image_bytes((width >> level).max(1), (height >> level).max(1))
                .unwrap_or(0);
            levels[level as usize] = (at as usize, slice_bytes as usize);
            at = at
                .checked_add(one.checked_mul(copies).ok_or(Error::Truncated {
                    what: "a VTF level",
                })?)
                .ok_or(Error::Truncated {
                    what: "a VTF level",
                })?;
        }
        if at > b.len() as u64 {
            return Err(Error::Truncated {
                what: "the VTF image data",
            });
        }
        Ok(Vtf {
            version: (major, minor),
            width,
            height,
            depth,
            flags,
            frames,
            faces,
            mips,
            format,
            reflectivity,
            resources,
            levels,
        })
    }

    /// A resource's data chunk (the length-prefixed bytes of `tag`; not for the image data, tag `0x30`).
    pub fn resource<'a>(&self, b: &'a [u8], tag: [u8; 3]) -> Option<&'a [u8]> {
        let &(_, flags, at) = self.resources.iter().find(|r| r.0 == tag)?;
        if flags & 0x02 != 0 {
            return None;
        }
        let len = u32_at(b, at as usize, "a VTF resource").ok()? as usize;
        slice(b, (at as usize).checked_add(4)?, len, "a VTF resource").ok()
    }

    /// The decoded levels of the first frame, face and slice, the largest first, skipping levels while the top one is over
    /// [`MAX_PIXELS`] or wider than `max_side` (`0`: no limit).
    pub fn decode(&self, b: &[u8], max_side: u32) -> Result<Vec<Level>> {
        let mut out = Vec::new();
        for (i, &(at, len)) in self.levels.iter().enumerate() {
            let w = (self.width >> i).max(1);
            let h = (self.height >> i).max(1);
            if out.is_empty()
                && (u64::from(w) * u64::from(h) > MAX_PIXELS
                    || (max_side != 0 && w.max(h) > max_side))
            {
                continue;
            }
            let src = slice(b, at, len, "a VTF level")?;
            out.push(Level {
                width: w,
                height: h,
                rgba: decode_image(self.format, src, w, h)?,
            });
        }
        if out.is_empty() {
            return Err(Error::TooLarge {
                what: "the VTF's smallest level",
                asked: u64::from(self.width) * u64::from(self.height),
                cap: MAX_PIXELS,
            });
        }
        Ok(out)
    }
}

/// Reads and decodes a texture: its header and its levels.
pub fn read(b: &[u8], max_side: u32) -> Result<(Vtf, Vec<Level>)> {
    let v = Vtf::parse(b)?;
    let levels = v.decode(b, max_side)?;
    Ok((v, levels))
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 31) as u32;
    let g = ((c >> 5) & 63) as u32;
    let bl = (c & 31) as u32;
    [
        ((r * 527 + 23) >> 6) as u8,
        ((g * 259 + 33) >> 6) as u8,
        ((bl * 527 + 23) >> 6) as u8,
    ]
}

/// One BC1 color block into `out` (16 RGBA texels in rows); `four` forces the four-color mode (DXT3/5's color blocks).
fn bc1_block(blk: &[u8], four: bool, out: &mut [[u8; 4]; 16]) {
    let c0 = u16::from_le_bytes([blk[0], blk[1]]);
    let c1 = u16::from_le_bytes([blk[2], blk[3]]);
    let (a, b) = (rgb565(c0), rgb565(c1));
    let mix = |wa: u32, wb: u32, d: u32| -> [u8; 4] {
        [
            ((u32::from(a[0]) * wa + u32::from(b[0]) * wb) / d) as u8,
            ((u32::from(a[1]) * wa + u32::from(b[1]) * wb) / d) as u8,
            ((u32::from(a[2]) * wa + u32::from(b[2]) * wb) / d) as u8,
            255,
        ]
    };
    let palette = if four || c0 > c1 {
        [
            [a[0], a[1], a[2], 255],
            [b[0], b[1], b[2], 255],
            mix(2, 1, 3),
            mix(1, 2, 3),
        ]
    } else {
        [
            [a[0], a[1], a[2], 255],
            [b[0], b[1], b[2], 255],
            mix(1, 1, 2),
            [0, 0, 0, 0],
        ]
    };
    let bits = u32::from_le_bytes([blk[4], blk[5], blk[6], blk[7]]);
    for (i, t) in out.iter_mut().enumerate() {
        *t = palette[((bits >> (2 * i)) & 3) as usize];
    }
}

/// One BC4 (DXT5 alpha) block: 16 values.
fn bc4_block(blk: &[u8]) -> [u8; 16] {
    let (a0, a1) = (u32::from(blk[0]), u32::from(blk[1]));
    let mut pal = [0u8; 8];
    pal[0] = a0 as u8;
    pal[1] = a1 as u8;
    if a0 > a1 {
        for i in 1..7u32 {
            pal[i as usize + 1] = (((7 - i) * a0 + i * a1) / 7) as u8;
        }
    } else {
        for i in 1..5u32 {
            pal[i as usize + 1] = (((5 - i) * a0 + i * a1) / 5) as u8;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits = 0u64;
    for (k, &byte) in blk[2..8].iter().enumerate() {
        bits |= u64::from(byte) << (8 * k);
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = pal[((bits >> (3 * i)) & 7) as usize];
    }
    out
}

/// Decodes one image of `format` (`src` exactly its bytes) into RGBA8.
pub fn decode_image(format: Format, src: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    use Format::*;
    let (wu, hu) = (w as usize, h as usize);
    let need = format.image_bytes(w, h).unwrap_or(u64::MAX);
    if (src.len() as u64) < need {
        return Err(Error::Truncated {
            what: "a VTF image",
        });
    }
    cap(
        "a VTF level's pixels",
        u64::from(w) * u64::from(h),
        MAX_PIXELS,
    )?;
    let mut out = vec![0u8; wu * hu * 4];
    let block_bytes = match format {
        Dxt1 | Dxt1OneBitAlpha | Ati1n => Some(8),
        Dxt3 | Dxt5 | Ati2n => Some(16),
        _ => None,
    };
    if let Some(bb) = block_bytes {
        let bw = wu.div_ceil(4);
        let bh = hu.div_ceil(4);
        for by in 0..bh {
            for bx in 0..bw {
                let at = (by * bw + bx) * bb;
                let blk = &src[at..at + bb];
                let mut texels = [[0u8; 4]; 16];
                match format {
                    Dxt1 | Dxt1OneBitAlpha => bc1_block(blk, false, &mut texels),
                    Dxt3 => {
                        bc1_block(&blk[8..], true, &mut texels);
                        for (i, t) in texels.iter_mut().enumerate() {
                            let nib = (blk[i / 2] >> (4 * (i % 2))) & 0xf;
                            t[3] = nib * 17;
                        }
                    }
                    Dxt5 => {
                        bc1_block(&blk[8..], true, &mut texels);
                        let a = bc4_block(&blk[..8]);
                        for (t, a) in texels.iter_mut().zip(a) {
                            t[3] = a;
                        }
                    }
                    Ati1n => {
                        let r = bc4_block(blk);
                        for (t, r) in texels.iter_mut().zip(r) {
                            *t = [r, r, r, 255];
                        }
                    }
                    _ => {
                        // ATI2N: two BC4 blocks, X then Y of a normal; Z made from them.
                        let x = bc4_block(&blk[..8]);
                        let y = bc4_block(&blk[8..]);
                        for (i, t) in texels.iter_mut().enumerate() {
                            let nx = f32::from(x[i]) / 127.5 - 1.0;
                            let ny = f32::from(y[i]) / 127.5 - 1.0;
                            let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
                            *t = [x[i], y[i], ((nz * 0.5 + 0.5) * 255.0) as u8, 255];
                        }
                    }
                }
                for (i, t) in texels.iter().enumerate() {
                    let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                    if x < wu && y < hu {
                        let o = (y * wu + x) * 4;
                        out[o..o + 4].copy_from_slice(t);
                    }
                }
            }
        }
        return Ok(out);
    }
    for (i, px) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let p: [u8; 4] = match format {
            Rgba8888 => [src[i * 4], src[i * 4 + 1], src[i * 4 + 2], src[i * 4 + 3]],
            Abgr8888 => [src[i * 4 + 3], src[i * 4 + 2], src[i * 4 + 1], src[i * 4]],
            Argb8888 => [src[i * 4 + 1], src[i * 4 + 2], src[i * 4 + 3], src[i * 4]],
            Bgra8888 => [src[i * 4 + 2], src[i * 4 + 1], src[i * 4], src[i * 4 + 3]],
            Bgrx8888 => [src[i * 4 + 2], src[i * 4 + 1], src[i * 4], 255],
            Uvwq8888 | Uvlx8888 => [src[i * 4], src[i * 4 + 1], src[i * 4 + 2], 255],
            Rgb888 => [src[i * 3], src[i * 3 + 1], src[i * 3 + 2], 255],
            Bgr888 => [src[i * 3 + 2], src[i * 3 + 1], src[i * 3], 255],
            Rgb888Bluescreen | Bgr888Bluescreen => {
                let (r, g, b) = if format == Rgb888Bluescreen {
                    (src[i * 3], src[i * 3 + 1], src[i * 3 + 2])
                } else {
                    (src[i * 3 + 2], src[i * 3 + 1], src[i * 3])
                };
                if (r, g, b) == (0, 0, 255) {
                    [0, 0, 0, 0]
                } else {
                    [r, g, b, 255]
                }
            }
            Rgb565 => {
                let c = rgb565(u16::from_le_bytes([src[i * 2], src[i * 2 + 1]]));
                [c[0], c[1], c[2], 255]
            }
            Bgr565 => {
                let c = rgb565(u16::from_le_bytes([src[i * 2], src[i * 2 + 1]]));
                [c[2], c[1], c[0], 255]
            }
            Bgra4444 => {
                let v = u16::from_le_bytes([src[i * 2], src[i * 2 + 1]]);
                let n = |s: u16| (((v >> s) & 0xf) as u8) * 17;
                [n(8), n(4), n(0), n(12)]
            }
            Bgrx5551 | Bgra5551 => {
                let v = u16::from_le_bytes([src[i * 2], src[i * 2 + 1]]);
                let n = |s: u16| ((u32::from((v >> s) & 31) * 527 + 23) >> 6) as u8;
                let a = if format == Bgra5551 && v & 0x8000 == 0 {
                    0
                } else {
                    255
                };
                [n(10), n(5), n(0), a]
            }
            I8 => [src[i], src[i], src[i], 255],
            Ia88 => [src[i * 2], src[i * 2], src[i * 2], src[i * 2 + 1]],
            A8 => [255, 255, 255, src[i]],
            Uv88 => [src[i * 2], src[i * 2 + 1], 255, 255],
            Rgba16161616F => {
                let c = |k: usize| {
                    let h = u16::from_le_bytes([src[i * 8 + 2 * k], src[i * 8 + 2 * k + 1]]);
                    (f16(h).clamp(0.0, 1.0) * 255.0 + 0.5) as u8
                };
                [c(0), c(1), c(2), c(3)]
            }
            Rgba16161616 => {
                let c = |k: usize| src[i * 8 + 2 * k + 1];
                [c(0), c(1), c(2), c(3)]
            }
            _ => return invalid(format!("VTF image format {format:?} is not decoded")),
        };
        px.copy_from_slice(&p);
    }
    Ok(out)
}

/// A VTF file of `w` x `h` with the given levels' bytes (the largest first, one image each), for tests and fuzz seeds.
#[cfg(test)]
pub fn write(format: i32, w: u16, h: u16, minor: u32, levels: &[Vec<u8>]) -> Vec<u8> {
    let header = if minor >= 3 { 80 + 8 } else { 80 };
    let mut out = vec![0u8; header];
    out[..4].copy_from_slice(b"VTF\0");
    out[4..8].copy_from_slice(&7u32.to_le_bytes());
    out[8..12].copy_from_slice(&minor.to_le_bytes());
    out[12..16].copy_from_slice(&(header as u32).to_le_bytes());
    out[16..18].copy_from_slice(&w.to_le_bytes());
    out[18..20].copy_from_slice(&h.to_le_bytes());
    out[24..26].copy_from_slice(&1u16.to_le_bytes());
    out[26..28].copy_from_slice(&0xffffu16.to_le_bytes());
    out[52..56].copy_from_slice(&format.to_le_bytes());
    out[56] = levels.len() as u8;
    out[57..61].copy_from_slice(&(-1i32).to_le_bytes());
    out[63..65].copy_from_slice(&1u16.to_le_bytes());
    if minor >= 3 {
        out[68..72].copy_from_slice(&1u32.to_le_bytes());
        out[80..83].copy_from_slice(&[0x30, 0, 0]);
        out[84..88].copy_from_slice(&(header as u32).to_le_bytes());
    }
    for l in levels.iter().rev() {
        out.extend_from_slice(l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_rgba_and_bgr() {
        let top: Vec<u8> = (0..2 * 2 * 4).map(|i| i as u8).collect();
        let small = vec![9, 8, 7, 6];
        for minor in [1, 2, 3, 4, 5] {
            let f = write(0, 2, 2, minor, &[top.clone(), small.clone()]);
            let (v, levels) = read(&f, 0).unwrap();
            assert_eq!((v.width, v.height, v.mips), (2, 2, 2));
            assert_eq!(levels[0].rgba, top);
            assert_eq!(levels[1].rgba, small);
        }
        let f = write(3, 1, 1, 2, &[vec![1, 2, 3]]);
        assert_eq!(read(&f, 0).unwrap().1[0].rgba, vec![3, 2, 1, 255]);
    }

    #[test]
    fn decodes_dxt1() {
        // Color 0 pure red, color 1 pure blue, every texel index 1 (blue) on a 4x4 block.
        let blk = vec![0x00, 0xf8, 0x1f, 0x00, 0x55, 0x55, 0x55, 0x55];
        let f = write(13, 4, 4, 2, &[blk]);
        let (_, levels) = read(&f, 0).unwrap();
        assert_eq!(&levels[0].rgba[..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn refuses_truncated_and_huge() {
        let f = write(0, 64, 64, 2, &[vec![0; 16]]);
        assert!(Vtf::parse(&f).is_err());
        let mut f = write(0, 1, 1, 2, &[vec![0; 4]]);
        f[56] = 200;
        assert!(Vtf::parse(&f).is_err());
    }
}
