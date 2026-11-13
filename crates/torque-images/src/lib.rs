//! Image decoders for the renderer. No GPU: bytes in, the bits of a TGE `GBitmap` out, as TGE's loaders leave them, so that a
//! texture uploaded from here is texel-equal to the one TGE uploads.
//!
//! - [`png`]: `GBitmap::readPNG` over libpng 1.2: the `png` crate decodes the raw rows and this module applies the transforms
//!   `readPNG` asks for (`png_set_strip_16`, `png_set_expand`, `png_set_gray_to_rgb`), in libpng's order, with **no gamma step**
//!   (`png_set_gamma` is never called).
//! - [`jpeg`]: `GBitmap::readJPEG` over libjpeg 6b (`6b  27-Mar-1998`): our own Rust implementation of the decoder with the
//!   defaults `jpeg_read_header` sets (the slow integer IDCT, fancy upsampling, block smoothing, no colour quantisation).
//! - [`mip`]: `GBitmap::extrudeMipLevels`, the box filter TGE builds mip levels with on the CPU, pointer arithmetic and all.
//! - [`ktx2`], [`bc`], [`exr`]: the physically based materials' files, none of them TGE's: KTX 2.0 textures (stored or
//!   Zstandard levels), BC4, BC5 and BC7 blocks both ways on the CPU, and OpenEXR maps read as 16-bit texels.
//!
//! **The formats** are TGE's `GBitmap::BitmapFormat` as the loaders pick them: [`Format::Alpha`] (one byte, uploaded as `GL_ALPHA`) for
//! a grey PNG and a greyscale JPEG, [`Format::Rgb`] and [`Format::Rgba`]. [`Bitmap::to_rgba8`] is the texel GL builds from each.
//!
//! **Every decoder reads hostile bytes.** It returns a value or an [`ImageError`] and never panics; every size it allocates for is
//! checked against a named cap first. Where TGE would stop with a fatal assert (libpng's error callback is `AssertISV`, libjpeg's
//! `error_exit` calls `exit`), the decoder returns an error.
//!
//! **Credits.** The loaders follow the Torque Game Engine's `GBitmap` code (TGE 1.4: `dgl/gBitmap.cc`, `dgl/bitmapPng.cc`,
//! `dgl/bitmapJpeg.cc`, `dgl/gTexManager.cc`, `platformWin32/winAsmBlit.cc`, each Copyright (C) GarageGames.com, Inc.; the Torque
//! engine is MIT-licensed as Torque3D, Copyright (c) 2012 GarageGames, LLC). Of
//! these, Torque3D carries the bitmap and its PNG loader on in `gfx/bitmap/gBitmap.cpp` and `gfx/bitmap/loaders/bitmapPng.cpp`;
//! it has no JPEG loader and no MMX mip path. The code here is this crate's own. This software is based in part on the work of the Independent JPEG Group: [`jpeg`] is written from the
//! algorithms of libjpeg 6b. The PNG chunk rules follow libpng 1.2.8's behaviour; no libpng code is carried over.
//!
//! # Maintaining
//!
//! **Work is tied to the file's size.** Every decoder allocates and loops for what its input can pay for, so a few bytes never ask
//! for a large image or a long decode. PNG: the rows may need at most 1,100 times the IDAT bytes. JPEG ([`jpeg::Limits`]): a frame
//! holds at most `JPEG_SAMPLES_FLOOR` plus `samples_per_byte` samples a file byte, the scans together visit at most
//! `JPEG_SAMPLES_FLOOR` plus `block_visits_per_byte` blocks a file byte (both charged to the file's bytes, nothing else), and a scan
//! whose entropy decoder has no input left stops (a restart interval keeps it running while input remains, and is charged). The
//! coefficient arrays grow a block row at a time as the scans reach them. A new decoder or a new cap gets the same tie and a test
//! that a header with no data behind it is refused.
//!
//! **Invariants and their tests.**
//! - A frame header with no data behind it is refused by the sample tie, in milliseconds
//!   (`jpeg::tests::a_frame_header_with_no_data_behind_it_is_refused`), and `header_size` agrees with the decoder.
//! - Empty scans stop at once, and scans repeated past the visit allowance are refused
//!   (`scans_with_no_data_stop_at_once`, `the_scans_together_may_visit_only_a_bounded_number_of_blocks`).
//! - An honest, large, flat progressive file still decodes (`an_honest_large_flat_progressive_file_decodes`): a cap change must keep it.
//! - The fuzz runs under the default [`jpeg::Limits`], reports its slowest case and fails past a bound
//!   (`fuzz::hostile_images_never_panic`).
//! - Coefficient memory follows the data decoded: a frame with no data behind it allocates at most its first block row
//!   (`jpeg::tests::coefficient_memory_follows_the_rows_the_scans_reach`); the sample tie still bounds a full decode (about 8 KiB
//!   of memory a file byte at the cap).
//! - With restart markers on and no input left, the rest of a scan is skipped (`restart_scans_with_no_input_left_stop_at_once`).
//! - A KTX 2.0 file is checked whole from its header before any level is allocated or decoded ([`ktx2::header`]: every range
//!   in the file, each level its format's size, the levels and key/value data within [`ktx2::Limits`]), and a Zstandard level
//!   decodes into exactly its declared length with the decoder's window held to it
//!   (`a_zstandard_level_of_the_wrong_length_is_refused`); written and read again it is the same bytes
//!   (`every_format_writes_and_reads_back_byte_for_byte`). An OpenEXR file's headers are checked (flat, sides, channels, an
//!   offset table that fits) before a pixel is decoded, and one channel is kept. Both readers' fuzz is `texture_fuzz`
//!   (`TORQUE_FUZZ_TEXTURES`; `just texture-verify` runs 300,000 cases each). A new format goes in [`ktx2::Ktx2Format`]
//!   with its descriptor, size and the fuzz's seeds; [`bc::decode`] gives what a GPU samples, since a device without BC
//!   textures uploads it.

pub mod bc;
pub mod exr;
pub mod jpeg;
pub mod ktx2;
pub mod lru;
pub mod mip;
pub mod png;

#[cfg(test)]
mod fuzz;
#[cfg(test)]
mod testenc;
#[cfg(test)]
mod texture_fuzz;

use std::fmt;

/// TGE's `GBitmap::BitmapFormat`, the three the image loaders produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// One byte per pixel, uploaded as `GL_ALPHA` (`getSourceDestByteFormat`).
    Alpha,
    Rgb,
    Rgba,
}

impl Format {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Format::Alpha => 1,
            Format::Rgb => 3,
            Format::Rgba => 4,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Format::Alpha => "alpha",
            Format::Rgb => "rgb",
            Format::Rgba => "rgba",
        }
    }
}

/// A decoded image: the bits of mip level 0 of a `GBitmap`, rows top to bottom, no padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub format: Format,
    pub bits: Vec<u8>,
}

impl Bitmap {
    /// The texel GL builds from each pixel of the uploaded format (the GL 1.x conversion of a base internal format to RGBA):
    /// `GL_ALPHA` is `(0, 0, 0, A)`, `GL_RGB` is `(R, G, B, 255)`, `GL_RGBA` as stored.
    ///
    /// **Open question:** what a `GL_ALPHA` texture shows depends on the texture environment (under `GL_MODULATE` the colour
    /// is the fragment's and only the alpha comes from the texture); this view is the texel, not what a draw makes of it.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let pixels = self.bits.len() / self.format.bytes_per_pixel();
        let mut out = Vec::with_capacity(pixels * 4);
        match self.format {
            Format::Alpha => {
                for &a in &self.bits {
                    out.extend_from_slice(&[0, 0, 0, a]);
                }
            }
            Format::Rgb => {
                for p in self.bits.as_chunks::<3>().0 {
                    out.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
            }
            Format::Rgba => out.extend_from_slice(&self.bits),
        }
        out
    }
}

/// Which loader a file goes to. TGE picks it by the file's extension (the resource manager's registered extensions; `GBitmap::load`
/// tries the name as given, then `.jpg`, `.png`, `.gif` and `.bmp`), not by its bytes.
///
/// This crate decodes two of TGE's bitmap formats, PNG and JPEG; it has no GIF, BMP or `.bm8` decoder, so such a file is no
/// image here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Png,
    Jpeg,
}

impl Kind {
    /// The loader for a file name (ignoring case): `.png` and `.jpg`, the extensions TGE registers for these loaders (it never
    /// reads a `.jpeg`).
    pub fn of_name(name: &str) -> Option<Kind> {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".png") {
            Some(Kind::Png)
        } else if lower.ends_with(".jpg") {
            Some(Kind::Jpeg)
        } else {
            None
        }
    }
}

/// The width and height an image's header gives, without decoding it (GuiMLTextCtrl's `<bitmap:>` layout asks only the size):
/// a PNG's `IHDR` (first after the signature), a JPEG's frame header (`SOF0`, `SOF1`, `SOF2`, `SOF9` or `SOF10`, the ones the decoder
/// takes). `None` where [`decode`] would refuse the header: a size of 0, past the decoder's caps ([`png::MAX_PNG_WIDTH`],
/// [`png::MAX_PNG_HEIGHT`], [`png::MAX_PNG_PIXELS`]; [`jpeg::MAX_JPEG_DIMENSION`], [`jpeg::MAX_JPEG_PIXELS`]), another frame type, or
/// no frame header. A file whose header passes and whose body does not decode gives a size here and no texture at the draw.
pub fn size_of(kind: Kind, bytes: &[u8]) -> Option<(u32, u32)> {
    match kind {
        Kind::Png => png::header_size(bytes),
        Kind::Jpeg => jpeg::header_size(bytes),
    }
}

/// Decodes `bytes` with the loader for `kind`.
pub fn decode(kind: Kind, bytes: &[u8]) -> Result<Bitmap, ImageError> {
    match kind {
        Kind::Png => png::decode(bytes),
        Kind::Jpeg => jpeg::decode(bytes).map_err(ImageError::Jpeg),
    }
}

/// Why an image did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageError {
    /// The PNG loader refused the file (libpng would have called its error function: a fatal assert in TGE).
    Png(String),
    /// The JPEG loader refused the file.
    Jpeg(jpeg::JpegError),
    /// An argument the function cannot take (a bitmap whose bits do not match its size).
    Invalid(&'static str),
    /// A size is above one of the named caps.
    Cap {
        what: &'static str,
        value: u64,
        cap: u64,
    },
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::Png(m) => write!(f, "png: {m}"),
            ImageError::Jpeg(e) => write!(f, "jpeg: {e}"),
            ImageError::Invalid(m) => write!(f, "invalid: {m}"),
            ImageError::Cap { what, value, cap } => {
                write!(f, "{what}: {value} is above the limit of {cap}")
            }
        }
    }
}

impl std::error::Error for ImageError {}

#[cfg(test)]
mod kind_tests {
    use super::Kind;

    #[test]
    fn the_loader_is_picked_by_the_extensions_tge_registers() {
        assert_eq!(Kind::of_name("a/b.PNG"), Some(Kind::Png));
        assert_eq!(Kind::of_name("a/b.jpg"), Some(Kind::Jpeg));
        for name in [
            "a/b.jpeg",
            "a/b.gif",
            "a/b.bmp",
            "a/b.bm8",
            "a/b",
            "a/b.png.txt",
        ] {
            assert_eq!(Kind::of_name(name), None, "{name}");
        }
    }
}
