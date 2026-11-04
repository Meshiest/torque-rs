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

#[test]
fn a_font_color_set_by_its_name_is_the_arrays_entry() {
    let mut vm = torque_ts_vm::vm::Vm::new();
    let _gui = crate::register(&mut vm);
    let mut make = |code: &[u8]| crate::atoi(&vm.eval(code).unwrap()) as ObjId;
    let named = make(b"return new GuiControlProfile(Named) { fontColor = \"1 2 3 255\"; fontColorHL = \"250 251 252 255\"; };");
    let both = make(b"return new GuiControlProfile(Both) { fontColors[1] = \"9 9 9 255\"; fontColorSEL = \"7 7 7 255\"; };");
    assert_eq!(font_color(&vm, named, 0), [1, 2, 3, 255]);
    assert_eq!(font_color(&vm, named, 1), [250, 251, 252, 255]);
    assert_eq!(font_color(&vm, both, 1), [9, 9, 9, 255]);
    assert_eq!(font_color(&vm, both, 3), [7, 7, 7, 255]);
}
