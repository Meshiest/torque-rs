//! OpenEXR maps (height and displacement, the single channels a material library ships as floats) read as 16-bit unsigned
//! normalized texels: one channel of the file's first layer that has it (`Y`, else `R`, else its first channel), each value
//! clamped to 0 to 1 and rounded to the nearest of 65,535 steps. The `exr` crate decodes the file.
//!
//! **Every file is untrusted.** The headers are read first and checked before any pixel is decoded: a flat (not deep) image,
//! the layer's sides 1 to the side taken, at most [`MAX_CHANNELS`] channels, and an offset table that fits in the file (a
//! tiny tile size cannot ask for a table larger than the file). Only the one channel is kept, so the memory held is two bytes a
//! texel. A file past [`MAX_FILE_BYTES`], with more than [`MAX_PARTS`] parts or a block the crate would allocate more than
//! [`MAX_BLOCK_BYTES`] for (a tile or scan lines, before it inflates them) is refused, and so is any file on wasm32.

use std::io::Cursor;

use exr::prelude::Vec2;
use exr::prelude::traits::*;

/// The most channels a layer may have.
pub const MAX_CHANNELS: usize = 16;
/// The largest file taken (its headers are parsed whole before any count in them is checked).
pub const MAX_FILE_BYTES: usize = 128 << 20;
/// The most parts (layers) a file may hold.
pub const MAX_PARTS: usize = 8;
/// The most bytes the crate may allocate for one block (a tile, or a run of scan lines) before it inflates it.
pub const MAX_BLOCK_BYTES: u64 = 64 << 20;

/// A single-channel image, texels row after row from the top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct R16 {
    pub width: u32,
    pub height: u32,
    pub texels: Vec<u16>,
}

impl R16 {
    /// Its texels as RGBA8 gray ([`gray8`]), opaque.
    pub fn to_rgba8(&self) -> Vec<u8> {
        self.texels
            .iter()
            .flat_map(|&t| {
                let v = gray8(t);
                [v, v, v, 255]
            })
            .collect()
    }
}

/// A 16-bit texel rounded to 8 bits.
pub const fn gray8(t: u16) -> u8 {
    ((t as u32 * 255 + 32_767) / 65_535) as u8
}

/// Why a file was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExrError {
    #[error("not a readable OpenEXR file: {0}")]
    Read(String),
    #[error("the image is deep, not flat")]
    Deep,
    #[error("{w} x {h} is empty or past the {max} a side taken")]
    Size { w: usize, h: usize, max: u32 },
    #[error("the layer has {0} channels, past the {MAX_CHANNELS} taken, or none")]
    Channels(usize),
    #[error("the offset table of {0} chunks does not fit in the file")]
    Chunks(usize),
    #[error("the file is {0} bytes, past the {MAX_FILE_BYTES} taken")]
    File(usize),
    #[error("the file has {0} parts, past the {MAX_PARTS} taken")]
    Parts(usize),
    #[error("a block of the file takes {0} bytes to decode, past the {MAX_BLOCK_BYTES} taken")]
    Block(u64),
    #[error("OpenEXR files are not read on this target")]
    Unsupported,
}

/// The value of a sample in 16-bit steps.
pub fn to_r16(v: f32) -> u16 {
    if v.is_nan() {
        return 0;
    }
    (v.clamp(0.0, 1.0) * 65_535.0).round() as u16
}

/// The sides of the file's first layer, checked as [`read_r16`] checks them (no pixel decoded).
///
/// # Errors
/// As [`read_r16`]'s header checks.
pub fn size_of(bytes: &[u8], max_side: u32) -> Result<(u32, u32), ExrError> {
    checked(bytes, max_side).map(|(w, h, _)| (w, h))
}

/// The checked first header: its sides and the channel read.
fn checked(bytes: &[u8], max_side: u32) -> Result<(u32, u32, String), ExrError> {
    checked_within(bytes, max_side, MAX_BLOCK_BYTES)
}

/// What the crate allocates to decode one block of `h` before inflating it: the block's area (a tile cut to the layer, or the
/// layer's width by a run of scan lines) at the channels' bytes a pixel.
fn block_bytes(h: &exr::meta::header::Header) -> u64 {
    let (w, ht) = (h.layer_size.width() as u64, h.layer_size.height() as u64);
    let area = match h.blocks {
        exr::meta::BlockDescription::ScanLines => {
            w.saturating_mul(ht.min(h.compression.scan_lines_per_block() as u64))
        }
        exr::meta::BlockDescription::Tiles(t) => w
            .min(t.tile_size.width() as u64)
            .saturating_mul(ht.min(t.tile_size.height() as u64)),
    };
    area.saturating_mul(h.channels.bytes_per_pixel as u64)
}

/// [`checked`] with the most bytes a block may need given.
fn checked_within(
    bytes: &[u8],
    max_side: u32,
    max_block: u64,
) -> Result<(u32, u32, String), ExrError> {
    // The crate's chunk-count arithmetic can overflow a 32-bit usize.
    if cfg!(target_arch = "wasm32") {
        return Err(ExrError::Unsupported);
    }
    if bytes.len() > MAX_FILE_BYTES {
        return Err(ExrError::File(bytes.len()));
    }
    let meta = exr::meta::MetaData::read_from_buffered(bytes, false)
        .map_err(|e| ExrError::Read(e.to_string()))?;
    if meta.headers.len() > MAX_PARTS {
        return Err(ExrError::Parts(meta.headers.len()));
    }
    let header = meta
        .headers
        .first()
        .ok_or_else(|| ExrError::Read("no header".to_string()))?;
    // Every part's offset table is in the file: eight bytes a chunk.
    let chunks: usize = meta
        .headers
        .iter()
        .map(|h| h.chunk_count)
        .fold(0, usize::saturating_add);
    if chunks.saturating_mul(8) > bytes.len() {
        return Err(ExrError::Chunks(chunks));
    }
    if meta.headers.iter().any(|h| h.deep) {
        return Err(ExrError::Deep);
    }
    for h in &meta.headers {
        let (w, ht) = (h.layer_size.width(), h.layer_size.height());
        if w == 0 || ht == 0 || w > max_side as usize || ht > max_side as usize {
            return Err(ExrError::Size {
                w,
                h: ht,
                max: max_side,
            });
        }
        let n = h.channels.list.len();
        if n == 0 || n > MAX_CHANNELS {
            return Err(ExrError::Channels(n));
        }
        let block = block_bytes(h);
        if block > max_block {
            return Err(ExrError::Block(block));
        }
    }
    let names: Vec<String> = header
        .channels
        .list
        .iter()
        .map(|c| c.name.to_string())
        .collect();
    let name = ["Y", "R"]
        .into_iter()
        .find(|n| names.iter().any(|c| c == n))
        .map(str::to_string)
        .or_else(|| names.first().cloned())
        .ok_or(ExrError::Channels(0))?;
    Ok((
        header.layer_size.width() as u32,
        header.layer_size.height() as u32,
        name,
    ))
}

/// The file's map as [`R16`] texels, the file's sides at most `max_side`.
///
/// # Errors
/// A file that does not parse or decode, a deep image, sides past `max_side`, too many channels, or an offset table larger
/// than the file.
pub fn read_r16(bytes: &[u8], max_side: u32) -> Result<R16, ExrError> {
    let (w, h, name) = checked(bytes, max_side)?;
    let image = read()
        .no_deep_data()
        .largest_resolution_level()
        .specific_channels()
        .required(name.as_str())
        .collect_pixels(
            move |size, _| (size.width(), vec![0u16; size.area()]),
            |(width, texels): &mut (usize, Vec<u16>), p: Vec2<usize>, (v,): (f32,)| {
                if let Some(t) = texels.get_mut(p.y() * *width + p.x()) {
                    *t = to_r16(v);
                }
            },
        )
        .first_valid_layer()
        .all_attributes()
        .non_parallel()
        .from_buffered(Cursor::new(bytes))
        .map_err(|e| ExrError::Read(e.to_string()))?;
    let layer = image.layer_data;
    let (lw, lh) = (layer.size.width(), layer.size.height());
    let (_, texels) = layer.channel_data.pixels;
    if (lw, lh) != (w as usize, h as usize) || texels.len() != lw * lh {
        return Err(ExrError::Read(
            "the layer read is not the header's size".to_string(),
        ));
    }
    Ok(R16 {
        width: w,
        height: h,
        texels,
    })
}

#[cfg(test)]
#[path = "exr_tests.rs"]
pub(crate) mod tests;
