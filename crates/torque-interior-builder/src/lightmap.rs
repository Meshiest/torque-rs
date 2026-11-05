//! Lightmap rectangles: each surface's texels on the world axes across its main axis, packed into pages.
//!
//! A surface's lightmap planes read two world axes, each scaled by a power of two (the only form a `.dif` stores,
//! `writeLMapTexGen`): one texel every [`crate::Options::lightmap_texel`] units, a texel of border round the surface. Pages are
//! [`PAGE`] texels a side (a version-0 rectangle's offsets and sizes are bytes); a rectangle is at most [`MAX_RECT`] a side
//! (a larger surface is cut first, [`crate::build()`]). Rectangles are packed in rows, tallest first.

use crate::brush::main_axis;
use crate::geom::V3;

/// A page's side in texels.
pub const PAGE: u32 = 256;
/// A rectangle's largest side in texels.
pub const MAX_RECT: u32 = 254;

/// The two world axes a surface facing along `n` is mapped on.
pub fn axes(n: V3) -> (usize, usize) {
    match main_axis(n) {
        2 => (0, 1),
        0 => (1, 2),
        _ => (0, 2),
    }
}

/// A surface's rectangle before packing: its texel origin on the two axes and its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub origin: [i64; 2],
    pub size: [u32; 2],
}

/// The rectangle of a polygon mapped on `axes` at one texel every `texel` units, with a texel of border.
pub fn rect(poly: &[V3], axes: (usize, usize), texel: f64) -> Rect {
    let (a, b) = axes;
    let span = |k: usize| {
        let lo = poly
            .iter()
            .map(|p| p[k] / texel)
            .fold(f64::INFINITY, f64::min);
        let hi = poly
            .iter()
            .map(|p| p[k] / texel)
            .fold(f64::NEG_INFINITY, f64::max);
        let start = lo.floor() as i64 - 1;
        let end = hi.ceil() as i64 + 1;
        (start, (end - start).max(1) as u32)
    };
    let (sa, wa) = span(a);
    let (sb, wb) = span(b);
    Rect {
        origin: [sa, sb],
        size: [wa, wb],
    }
}

/// Where rectangles went: each one's page and offset, and the pages' count.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Packing {
    pub places: Vec<(u32, [u32; 2])>,
    pub pages: u32,
}

/// Packs `sizes` (each at most [`MAX_RECT`] a side) into [`PAGE`]-square pages in rows, tallest first.
pub fn pack(sizes: &[[u32; 2]]) -> Packing {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| {
        (
            std::cmp::Reverse(sizes[i][1]),
            std::cmp::Reverse(sizes[i][0]),
            i,
        )
    });
    let mut places = vec![(0, [0, 0]); sizes.len()];
    let (mut page, mut x, mut y, mut row) = (0u32, 0u32, 0u32, 0u32);
    let mut used = false;
    for i in order {
        let [w, h] = sizes[i];
        if x + w > PAGE {
            x = 0;
            y += row;
            row = 0;
        }
        if y + h > PAGE {
            page += 1;
            x = 0;
            y = 0;
            row = 0;
        }
        places[i] = (page, [x, y]);
        used = true;
        x += w;
        row = row.max(h);
    }
    Packing {
        places,
        pages: if used { page + 1 } else { 0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_never_overlap_and_stay_on_their_pages() {
        let sizes: Vec<[u32; 2]> = (0..300)
            .map(|i| [1 + (i * 37 % 200) as u32, 1 + (i * 53 % 120) as u32])
            .collect();
        let p = pack(&sizes);
        for (i, &(page, [x, y])) in p.places.iter().enumerate() {
            assert!(page < p.pages);
            assert!(x + sizes[i][0] <= PAGE && y + sizes[i][1] <= PAGE);
            for (j, &(pj, [xj, yj])) in p.places.iter().enumerate().skip(i + 1) {
                let apart = page != pj
                    || x + sizes[i][0] <= xj
                    || xj + sizes[j][0] <= x
                    || y + sizes[i][1] <= yj
                    || yj + sizes[j][1] <= y;
                assert!(apart, "{i} and {j}");
            }
        }
    }

    #[test]
    fn a_rectangle_covers_its_polygon_with_a_border() {
        let poly = [[0.2, 0.0, 0.0], [3.9, 0.0, 0.0], [3.9, 2.1, 0.0]];
        let r = rect(&poly, (0, 1), 0.5);
        assert_eq!(r.origin, [-1, -1]);
        assert_eq!(r.size, [10, 7]);
    }
}
