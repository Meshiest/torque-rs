use super::*;
use std::sync::Arc;

/// A `w` x `h` texture of one color with a full mip chain of it.
fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Texture {
    let mut levels = Vec::new();
    let (mut lw, mut lh) = (w, h);
    loop {
        levels.push(rgba.repeat((lw * lh) as usize));
        if lw == 1 && lh == 1 {
            break;
        }
        (lw, lh) = ((lw / 2).max(1), (lh / 2).max(1));
    }
    Texture {
        format: crate::TexelFormat::Rgba8,
        path: "solid".into(),
        width: w,
        height: h,
        levels: Arc::new(levels),
    }
}

fn texel(a: &Atlas, level: usize, x: u32, y: u32) -> [u8; 4] {
    let w = (a.texture.width >> level).max(1);
    let at = 4 * (y * w + x) as usize;
    a.texture.levels[level][at..at + 4].try_into().unwrap()
}

#[test]
fn the_grid_is_as_square_as_the_count_allows() {
    let cells: Vec<(u32, u32)> = [1, 2, 4, 5, 9, 10, 16, 32].map(grid).to_vec();
    assert_eq!(
        cells,
        [
            (1, 1),
            (2, 1),
            (2, 2),
            (3, 2),
            (3, 3),
            (4, 3),
            (4, 4),
            (6, 6)
        ]
    );
}

#[test]
fn frames_fill_the_cells_in_order_and_each_level_is_the_box_of_the_one_above() {
    let colors = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]];
    let frames: Vec<Texture> = colors.iter().map(|&c| solid(8, 8, c)).collect();
    let refs: Vec<&Texture> = frames.iter().collect();
    let a = build(&refs).expect("an atlas");
    assert_eq!((a.cols, a.rows), (2, 2));
    assert_eq!((a.texture.width, a.texture.height), (16, 16));
    assert_eq!(texel(&a, 0, 3, 3), colors[0]);
    assert_eq!(texel(&a, 0, 11, 3), colors[1]);
    assert_eq!(texel(&a, 0, 3, 11), colors[2]);
    // The fourth cell holds nothing.
    assert_eq!(texel(&a, 0, 11, 11), [0; 4]);
    // The chain stops where a cell is one texel across, and no level mixes two frames' colors.
    assert_eq!(a.texture.levels.len(), 4);
    assert_eq!(texel(&a, 3, 0, 0), colors[0]);
    assert_eq!(texel(&a, 3, 1, 0), colors[1]);
    assert_eq!(texel(&a, 3, 0, 1), colors[2]);
}

#[test]
fn frames_of_other_sizes_are_resampled_to_the_largest() {
    let small = solid(4, 4, [10, 20, 30, 255]);
    let big = solid(16, 16, [200, 100, 50, 255]);
    let a = build(&[&small, &big]).expect("an atlas");
    assert_eq!((a.texture.width, a.texture.height), (32, 16));
    assert_eq!(texel(&a, 0, 15, 15), [10, 20, 30, 255]);
    assert_eq!(texel(&a, 0, 16, 0), [200, 100, 50, 255]);
}

#[test]
fn a_grid_past_the_side_limit_shrinks_its_cells() {
    let frames: Vec<Texture> = (0..16).map(|_| solid(2048, 8, [1, 2, 3, 255])).collect();
    let refs: Vec<&Texture> = frames.iter().collect();
    let a = build(&refs).expect("an atlas");
    assert!(a.texture.width <= MAX_SIDE && a.texture.height <= MAX_SIDE);
    assert_eq!(texel(&a, 0, 5, 1), [1, 2, 3, 255]);
}

#[test]
fn no_atlas_for_one_frame_too_many_or_a_frame_with_no_texels() {
    let t = solid(4, 4, [0; 4]);
    assert!(build(&[&t]).is_none());
    assert!(build(&vec![&t; MAX_FRAMES + 1]).is_none());
    let empty = Texture {
        format: crate::TexelFormat::Rgba8,
        path: "e".into(),
        width: 4,
        height: 4,
        levels: Arc::new(Vec::new()),
    };
    assert!(build(&[&t, &empty]).is_none());
}
