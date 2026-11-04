use super::*;

fn bitmap(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> torque_images::Bitmap {
    let mut bits = Vec::new();
    for y in 0..h {
        for x in 0..w {
            bits.extend_from_slice(&f(x, y));
        }
    }
    torque_images::Bitmap {
        width: w,
        height: h,
        format: torque_images::Format::Rgb,
        bits,
    }
}

#[test]
fn the_array_cuts_runs_between_separator_pixels() {
    // A 7 x 4 bitmap: row 0 a separator (red), a 3 x 2 image at (0,1) closed below by a separator, a separator column at 3, a
    // 3 x 3 image at (4,1).
    let b = bitmap(7, 4, |x, y| {
        if y == 0 || x == 3 || (y == 3 && x < 3) {
            [255, 0, 0]
        } else {
            [0, 0, 255]
        }
    });
    assert_eq!(construct_from(&b), vec![[0, 1, 3, 2], [4, 1, 3, 3]]);
}
