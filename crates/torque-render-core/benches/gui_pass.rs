//! The GUI pass's vertex encoding of a busy frame's draw list (no GPU).

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use torque_gui::GuiDraws;
use torque_gui::draw::{GuiBatch, GuiBlend, GuiQuad};
use torque_render_core::gui_pass::vertex_bytes;

/// 3,000 quads in 300 batches.
fn draws() -> GuiDraws {
    let mut d = GuiDraws::default();
    for b in 0..300 {
        let quads = (0..10)
            .map(|k| {
                let x = (b * 10 + k) as f32;
                GuiQuad {
                    rect: [x, 4.0, x + 6.0, 14.0],
                    uv: [0.0, 0.0, 0.75, 0.625],
                    color: [255, 255, 255, 255],
                }
            })
            .collect();
        d.batches.push(GuiBatch {
            clip: [0, 0, 1280, 720],
            texture: None,
            blend: GuiBlend::Alpha,
            quads,
        });
    }
    d
}

fn gui_pass(c: &mut Criterion) {
    let d = draws();
    let mut bytes = Vec::new();
    c.bench_function("gui pass vertices 3k quads", |b| {
        b.iter(|| {
            vertex_bytes(&d, &mut bytes);
            black_box(bytes.len())
        })
    });
}

criterion_group!(benches, gui_pass);
criterion_main!(benches);
