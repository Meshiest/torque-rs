//! Flipbook atlases: a particle's `animateTexture` frames laid out in one texture, so a look is one bucket (one texture, one
//! indirect draw) whatever its frame count, and the fragment can blend the frame in view with the next one.
//!
//! The frames fill a grid row by row (as many columns as the square root of the count asks for), each resampled to one cell
//! size when they differ (nearest, from the level of each frame's chain nearest the cell). Each mip level of the atlas is the
//! 2 x 2 box of the one above it, and the chain stops where a cell is one texel across, so a level never mixes two frames'
//! texels unless a frame's side is odd.

use super::sprites::MAX_FRAMES;
use crate::Texture;

/// The longest side of an atlas (texels): the cell shrinks by halves until the grid fits.
pub const MAX_SIDE: u32 = 4096;

/// A built atlas: its texture and the grid its frames fill.
#[derive(Debug, Clone)]
pub struct Atlas {
    pub texture: Texture,
    pub cols: u32,
    pub rows: u32,
}

/// The grid for `n` frames: columns and rows (the grid as square as it can be, columns first).
pub fn grid(n: usize) -> (u32, u32) {
    let n = n.max(1) as u32;
    let cols = (n as f32).sqrt().ceil() as u32;
    (cols, n.div_ceil(cols))
}

/// The atlas of `frames` (two to [`MAX_FRAMES`] textures, in order), or None for a count outside that or a frame with no texels.
pub fn build(frames: &[&Texture]) -> Option<Atlas> {
    if !(2..=MAX_FRAMES).contains(&frames.len()) || frames.iter().any(|t| t.levels.is_empty()) {
        return None;
    }
    let (cols, rows) = grid(frames.len());
    let mut cell = (
        frames.iter().map(|t| t.width).max()?.max(1),
        frames.iter().map(|t| t.height).max()?.max(1),
    );
    while (cols * cell.0 > MAX_SIDE || rows * cell.1 > MAX_SIDE) && cell.0.max(cell.1) > 1 {
        cell = ((cell.0 / 2).max(1), (cell.1 / 2).max(1));
    }
    let (width, height) = (cols * cell.0, rows * cell.1);
    let mut level0 = vec![0u8; (4 * width * height) as usize];
    for (k, t) in frames.iter().enumerate() {
        let (cx, cy) = ((k as u32 % cols) * cell.0, (k as u32 / cols) * cell.1);
        paste(&mut level0, width, (cx, cy), cell, t);
    }
    let mut levels = vec![level0];
    let (mut w, mut h) = (width, height);
    let (mut cw, mut ch) = cell;
    while cw > 1 && ch > 1 && w > 1 && h > 1 {
        let next = halve(levels.last()?, w, h);
        (w, h) = (w / 2, h / 2);
        (cw, ch) = (cw / 2, ch / 2);
        levels.push(next);
    }
    Some(Atlas {
        texture: Texture {
            format: crate::TexelFormat::Rgba8,
            path: format!("flipbook {}x{} of {}", cell.0, cell.1, frames.len()),
            width,
            height,
            levels: std::sync::Arc::new(levels),
        },
        cols,
        rows,
    })
}

/// `t` resampled to `cell` texels into the grid image `out` (`width` texels across) at `at`, nearest, from the level of its
/// chain nearest the cell's size (a frame of the cell's own size copies its level 0).
fn paste(out: &mut [u8], width: u32, at: (u32, u32), cell: (u32, u32), t: &Texture) {
    let level = (0..t.levels.len())
        .take_while(|&i| (t.width >> i).max(1) >= cell.0 && (t.height >> i).max(1) >= cell.1)
        .last()
        .unwrap_or(0);
    let (sw, sh) = ((t.width >> level).max(1), (t.height >> level).max(1));
    let src = &t.levels[level];
    if src.len() < (4 * sw * sh) as usize {
        return;
    }
    for y in 0..cell.1 {
        let sy = (u64::from(y) * u64::from(sh) / u64::from(cell.1)) as u32;
        for x in 0..cell.0 {
            let sx = (u64::from(x) * u64::from(sw) / u64::from(cell.0)) as u32;
            let from = 4 * (sy * sw + sx) as usize;
            let to = 4 * ((at.1 + y) * width + at.0 + x) as usize;
            out[to..to + 4].copy_from_slice(&src[from..from + 4]);
        }
    }
}

/// The 2 x 2 box of an RGBA8 image `w` x `h` texels (an odd side loses its last row or column).
fn halve(src: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (nw, nh) = (w / 2, h / 2);
    let mut out = Vec::with_capacity((4 * nw * nh) as usize);
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..4 {
                let at = |dx: u32, dy: u32| {
                    u32::from(src[4 * ((2 * y + dy) * w + 2 * x + dx) as usize + c])
                };
                out.push(((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8);
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "atlas_tests.rs"]
mod tests;
