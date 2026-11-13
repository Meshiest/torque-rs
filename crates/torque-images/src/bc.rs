//! Block compression: RGBA8 levels into BC4, BC5 or BC7 blocks and back. The encoders are the `block_compression` crate's
//! Rust version of Intel's ISPC texture compressor (on the CPU, block rows in parallel); the decoders, the same crate's, are what a
//! device without BC support uploads instead (each block's texels as the GPU would sample them).
//!
//! A level of any size is encoded: the texels past its right and bottom edges, up to the next multiple of 4, repeat the edge's
//! last column and row. Blocks are stored row after row, top to bottom, each [`BlockFormat::block_bytes`] long.

use block_compression::{BC7Settings, CompressionVariant, decode, encode};
use rayon::prelude::*;

/// A block-compressed format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockFormat {
    /// One channel (R), 8 bytes a block.
    Bc4,
    /// Two channels (R and G), 16 bytes a block.
    Bc5,
    /// Four channels (RGBA), 16 bytes a block.
    Bc7,
}

/// Why blocks did not encode or decode.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BlockError {
    #[error("a level of {w} x {h} is empty or its {got} bytes are not its {want}")]
    Size { w: u32, h: u32, got: u64, want: u64 },
}

impl BlockFormat {
    /// The bytes of one 4 x 4 block.
    pub const fn block_bytes(self) -> usize {
        match self {
            BlockFormat::Bc4 => 8,
            BlockFormat::Bc5 | BlockFormat::Bc7 => 16,
        }
    }

    /// The bytes of a level of `w` x `h` (whole blocks).
    pub const fn level_bytes(self, w: u32, h: u32) -> u64 {
        w.div_ceil(4) as u64 * h.div_ceil(4) as u64 * self.block_bytes() as u64
    }

    pub const fn name(self) -> &'static str {
        match self {
            BlockFormat::Bc4 => "BC4",
            BlockFormat::Bc5 => "BC5",
            BlockFormat::Bc7 => "BC7",
        }
    }
}

/// Block rows a parallel job encodes.
const ROWS_A_JOB: usize = 4;

fn check(w: u32, h: u32, got: usize, want: u64) -> Result<(), BlockError> {
    if w == 0 || h == 0 || got as u64 != want {
        return Err(BlockError::Size {
            w,
            h,
            got: got as u64,
            want,
        });
    }
    Ok(())
}

/// `rgba` (`w` x `h`, RGBA8, rows top to bottom) as `format`'s blocks. BC4 keeps R, BC5 R and G, BC7 all four (an image whose
/// alpha is 255 everywhere takes BC7's opaque modes).
///
/// # Errors
/// An empty level, or `rgba` not `4 * w * h` bytes.
pub fn encode(format: BlockFormat, rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, BlockError> {
    check(w, h, rgba.len(), 4 * u64::from(w) * u64::from(h))?;
    let variant = match format {
        BlockFormat::Bc4 => CompressionVariant::BC4,
        BlockFormat::Bc5 => CompressionVariant::BC5,
        BlockFormat::Bc7 if rgba.iter().skip(3).step_by(4).all(|&a| a == 255) => {
            CompressionVariant::BC7(BC7Settings::opaque_basic())
        }
        BlockFormat::Bc7 => CompressionVariant::BC7(BC7Settings::alpha_basic()),
    };
    let (bw, bh) = (w.div_ceil(4) as usize, h.div_ceil(4) as usize);
    let row_bytes = bw * format.block_bytes();
    let mut out = vec![0u8; row_bytes * bh];
    out.par_chunks_mut(row_bytes * ROWS_A_JOB)
        .enumerate()
        .for_each(|(job, blocks)| {
            let rows = blocks.len() / row_bytes;
            // This job's texel rows, padded to whole blocks with the edges repeated.
            let (pw, ph) = (4 * bw, 4 * rows);
            let mut padded = vec![0u8; 4 * pw * ph];
            for y in 0..ph {
                let sy = (4 * job * ROWS_A_JOB + y).min(h as usize - 1);
                let src = &rgba[4 * sy * w as usize..4 * (sy + 1) * w as usize];
                let dst = &mut padded[4 * y * pw..4 * (y + 1) * pw];
                dst[..src.len()].copy_from_slice(src);
                let last = [
                    src[src.len() - 4],
                    src[src.len() - 3],
                    src[src.len() - 2],
                    src[src.len() - 1],
                ];
                for p in dst[src.len()..].as_chunks_mut::<4>().0 {
                    *p = last;
                }
            }
            encode::compress_rgba8(
                variant,
                &padded,
                blocks,
                pw as u32,
                ph as u32,
                4 * pw as u32,
            );
        });
    Ok(out)
}

/// `format`'s blocks of a level of `w` x `h` as RGBA8, each texel what a GPU samples: BC4's R with G and B 0 and A 255, BC5's
/// R and G with B 0 and A 255.
///
/// # Errors
/// An empty level, or `blocks` not [`BlockFormat::level_bytes`] long.
pub fn decode(format: BlockFormat, blocks: &[u8], w: u32, h: u32) -> Result<Vec<u8>, BlockError> {
    check(w, h, blocks.len(), format.level_bytes(w, h))?;
    let (w, h) = (w as usize, h as usize);
    let bw = w.div_ceil(4);
    let mut out = vec![0u8; 4 * w * h];
    let row_bytes = bw * format.block_bytes();
    out.par_chunks_mut(4 * w * 4)
        .zip(blocks.par_chunks(row_bytes))
        .for_each(|(texels, row)| {
            let rows = texels.len() / (4 * w);
            for (bx, block) in row.chunks_exact(format.block_bytes()).enumerate() {
                let mut px = [0u8; 64];
                match format {
                    BlockFormat::Bc4 => {
                        let mut r = [0u8; 16];
                        decode::decode_block_bc4(block, &mut r, 4);
                        for (p, v) in px.as_chunks_mut::<4>().0.iter_mut().zip(r) {
                            *p = [v, 0, 0, 255];
                        }
                    }
                    BlockFormat::Bc5 => {
                        let mut rg = [0u8; 32];
                        decode::decode_block_bc5(block, &mut rg, 8);
                        for (p, v) in px
                            .as_chunks_mut::<4>()
                            .0
                            .iter_mut()
                            .zip(rg.as_chunks::<2>().0)
                        {
                            *p = [v[0], v[1], 0, 255];
                        }
                    }
                    BlockFormat::Bc7 => decode::decode_block_bc7(block, &mut px, 16),
                }
                let cols = (w - 4 * bx).min(4);
                for y in 0..rows {
                    let at = 4 * (y * w + 4 * bx);
                    texels[at..at + 4 * cols].copy_from_slice(&px[16 * y..16 * y + 4 * cols]);
                }
            }
        });
    Ok(out)
}

#[cfg(test)]
#[path = "bc_tests.rs"]
mod tests;
