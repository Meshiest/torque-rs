//! The GUI pass on the GPU (skipped with a note when the machine has no adapter): fills, `GL_MODULATE`, the alpha blend and the one that multiplies the destination,
//! nearest sampling of texel centres, the scissor of a batch's clip, against GL 1.x's formulas.

use std::sync::Arc;

use torque_gui::GuiDraws;
use torque_gui::draw::{GuiBatch, GuiBlend, GuiQuad};
use torque_gui::textures::GuiTexture;
use torque_render_core::{Gpu, GuiPass};

fn quad(rect: [f32; 4], uv: [f32; 4], color: [u8; 4]) -> GuiQuad {
    GuiQuad { rect, uv, color }
}

fn batch(
    clip: [i32; 4],
    texture: Option<Arc<GuiTexture>>,
    blend: GuiBlend,
    quads: Vec<GuiQuad>,
) -> GuiBatch {
    GuiBatch {
        clip,
        texture,
        blend,
        quads,
    }
}

#[test]
fn fills_textures_blends_and_clips_as_gl_does() {
    let r = match Gpu::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let mut pass = GuiPass::new(&r, None);
    // A 2 x 2 texture: red, green / blue, half-transparent white.
    let tex = Arc::new(GuiTexture {
        id: u64::MAX - 7,
        name: "test".into(),
        bitmap_size: (2, 2),
        size: (2, 2),
        rgba: vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 128,
        ],
        nearest: true,
        clamp: true,
    });
    let full = [0, 0, 8, 4];
    let draws = GuiDraws {
        size: (8, 4),
        batches: vec![
            // Grey everywhere.
            batch(
                full,
                None,
                GuiBlend::Alpha,
                vec![quad([0.0, 0.0, 8.0, 4.0], [0.0; 4], [100, 100, 100, 255])],
            ),
            // The texture 1:1 at (0,0), modulated by white: the texels, the half-transparent one blended over the grey.
            batch(
                full,
                Some(tex.clone()),
                GuiBlend::Alpha,
                vec![quad(
                    [0.0, 0.0, 2.0, 2.0],
                    [0.0, 0.0, 1.0, 1.0],
                    [255, 255, 255, 255],
                )],
            ),
            // A half-alpha black fill clipped to column 4 only.
            batch(
                [4, 0, 1, 4],
                None,
                GuiBlend::Alpha,
                vec![quad([3.0, 0.0, 8.0, 4.0], [0.0; 4], [0, 0, 0, 128])],
            ),
            // Multiply: DST_COLOR, ONE_MINUS_SRC_ALPHA with an opaque half grey: dst * 0.5.
            batch(
                full,
                None,
                GuiBlend::Multiply,
                vec![quad([6.0, 0.0, 8.0, 4.0], [0.0; 4], [128, 128, 128, 255])],
            ),
        ],
        ..Default::default()
    };
    let target = r.offscreen(8, 4).unwrap();
    pass.draw(&r, &draws, &target.color, Some([0.0, 0.0, 0.0, 1.0]));
    let f = r.read_back(&target).unwrap();
    let px = |x: usize, y: usize| {
        let i = (y * 8 + x) * 4;
        [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2]]
    };
    assert_eq!(px(0, 0), [255, 0, 0]);
    assert_eq!(px(1, 0), [0, 255, 0]);
    assert_eq!(px(0, 1), [0, 0, 255]);
    // 255 * 128/255 + 100 * (1 - 128/255) = 128 + 49.8.
    let w = px(1, 1);
    assert!(w.iter().all(|&c| (177..=178).contains(&c)), "{w:?}");
    // Only column 4 darkened: 100 * (1 - 128/255) = 49.8.
    assert!((49..=50).contains(&px(4, 2)[0]), "{:?}", px(4, 2));
    assert_eq!(px(3, 2), [100, 100, 100]);
    assert_eq!(px(5, 2), [100, 100, 100]);
    // Multiply: 100 * 128/255 + 100 * 0 = 50.2.
    assert!((50..=51).contains(&px(7, 3)[0]), "{:?}", px(7, 3));
    assert_eq!(pass.textures(), 1);
}
