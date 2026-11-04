//! The backdrop blur on the GPU (skipped with a note when the machine has no adapter) against [`blur::reference`], the CPU's,
//! over synthetic scenes (stripes, a checker, one-pixel lines: hard edges a blur must spread): single backdrops of every level
//! count, at the target's edges, odd sizes, corners, tints and opacities; nested backdrops with opaque fills between them (each
//! blurs what was drawn before it). And the levels and placement of hostile rectangles and radii.

use torque_gui::GuiDraws;
use torque_gui::backdrop::BackdropDraw;
use torque_gui::draw::{ExtraDraw, GuiBatch, GuiBlend, GuiQuad};
use torque_render_core::blur::{self, Blur};
use torque_render_core::{Frame, Gpu, GuiPass};

/// A step after the base scene: a backdrop, or an opaque fill.
#[derive(Debug, Clone, Copy)]
enum Step {
    Backdrop(BackdropDraw),
    Fill([i32; 4], [u8; 3]),
}

/// One case: its name, the target size and the steps.
#[derive(Debug, Clone)]
struct Case {
    name: String,
    size: (u32, u32),
    steps: Vec<Step>,
}

/// A case's result: the mean and largest absolute difference (0-255 units) over the backdrops' areas, RGB, and the pixels compared.
#[derive(Debug, Clone)]
struct Judged {
    name: String,
    mae: f64,
    max: u8,
    pixels: u64,
}

fn quad(r: [i32; 4], c: [u8; 4]) -> GuiQuad {
    GuiQuad {
        rect: [
            r[0] as f32,
            r[1] as f32,
            (r[0] + r[2]) as f32,
            (r[1] + r[3]) as f32,
        ],
        uv: [0.0; 4],
        color: c,
    }
}

/// The base scene: a dark ground, coloured stripes, a checker of 16-pixel squares and one-pixel lines.
fn base_quads(size: (u32, u32)) -> Vec<GuiQuad> {
    let (w, h) = (size.0 as i32, size.1 as i32);
    let mut q = vec![quad([0, 0, w, h], [20, 23, 28, 255])];
    let colors = [
        [230, 72, 77],
        [76, 141, 255],
        [48, 164, 108],
        [250, 200, 40],
        [240, 240, 240],
    ];
    let mut x = 0;
    let mut i = 0;
    while x < w / 2 {
        let c = colors[i % colors.len()];
        q.push(quad([x, 0, 23, h], [c[0], c[1], c[2], 255]));
        x += 37;
        i += 1;
    }
    let mut y = 0;
    while y < h {
        let mut x = w / 2;
        while x < w {
            if ((x / 16) + (y / 16)) % 2 == 0 {
                q.push(quad([x, y, 16, 16], [255, 255, 255, 255]));
            }
            x += 16;
        }
        y += 16;
    }
    for k in 0..(h / 6) {
        q.push(quad([w / 4, k * 6, w / 2, 1], [0, 0, 0, 255]));
    }
    q
}

fn backdrop(rect: [i32; 4], radius: u32, corner: u32, tint: [u8; 4], opacity: f32) -> BackdropDraw {
    BackdropDraw {
        at_batch: 0,
        rect,
        clip: [i32::MIN / 4, i32::MIN / 4, i32::MAX / 2, i32::MAX / 2],
        radius,
        tint,
        corner,
        opacity,
    }
}

/// The 12 single-backdrop cases and the 3 nested ones.
fn cases() -> (Vec<Case>, Vec<Case>) {
    let s = (320, 240);
    let tint = [28, 32, 39, 170];
    let one = |name: &str, size: (u32, u32), b: BackdropDraw| Case {
        name: name.to_string(),
        size,
        steps: vec![Step::Backdrop(b)],
    };
    let mut clipped = backdrop([40, 40, 200, 150], 16, 6, tint, 1.0);
    clipped.clip = [60, 50, 120, 90];
    let single = vec![
        one(
            "radius 2 (1 level)",
            s,
            backdrop([40, 30, 200, 150], 2, 0, [0; 4], 1.0),
        ),
        one(
            "radius 8 (2 levels)",
            s,
            backdrop([40, 30, 200, 150], 8, 6, tint, 1.0),
        ),
        one(
            "radius 16 (3 levels)",
            s,
            backdrop([40, 30, 200, 150], 16, 6, tint, 1.0),
        ),
        one(
            "radius 32 (4 levels)",
            s,
            backdrop([40, 30, 200, 150], 32, 12, tint, 1.0),
        ),
        one(
            "radius 64 (5 levels)",
            s,
            backdrop([40, 30, 200, 150], 64, 12, tint, 1.0),
        ),
        one(
            "full target",
            s,
            backdrop([0, 0, 320, 240], 64, 0, [0; 4], 1.0),
        ),
        one(
            "past the corner",
            s,
            backdrop([-30, -20, 120, 90], 16, 8, tint, 1.0),
        ),
        one(
            "odd size and place",
            (317, 233),
            backdrop([13, 7, 201, 151], 32, 9, tint, 1.0),
        ),
        one("1x1", s, backdrop([100, 100, 1, 1], 8, 0, [0; 4], 1.0)),
        one(
            "opacity 0.5",
            s,
            backdrop([40, 30, 200, 150], 16, 6, tint, 0.5),
        ),
        one(
            "tint only (radius 0)",
            s,
            backdrop([40, 30, 200, 150], 0, 6, tint, 1.0),
        ),
        one("clipped", s, clipped),
    ];
    let mut nested = Vec::new();
    for depth in 2..=4 {
        let mut steps = Vec::new();
        for k in 0..depth {
            let d = 24 * k;
            steps.push(Step::Backdrop(backdrop(
                [30 + d, 20 + d, 240 - 2 * d, 190 - 2 * d],
                8 << k.min(3),
                6,
                tint,
                1.0,
            )));
            steps.push(Step::Fill([36 + d, 26 + d, 60, 6], [76, 141, 255]));
        }
        nested.push(Case {
            name: format!("{depth} nested"),
            size: s,
            steps,
        });
    }
    (single, nested)
}

/// The draw list of a case: the base batch, then each step (a backdrop before the next batch, a fill a batch of its own).
fn draws_of(c: &Case, base_only: bool) -> GuiDraws {
    let clip = [0, 0, c.size.0 as i32, c.size.1 as i32];
    let batch = |quads| GuiBatch {
        clip,
        texture: None,
        blend: GuiBlend::Alpha,
        quads,
    };
    let mut d = GuiDraws {
        size: c.size,
        batches: vec![batch(base_quads(c.size))],
        ..GuiDraws::default()
    };
    if base_only {
        return d;
    }
    for s in &c.steps {
        match *s {
            Step::Backdrop(mut b) => {
                b.at_batch = d.batches.len();
                d.extras.push(ExtraDraw {
                    at_batch: b.at_batch,
                    item: std::rc::Rc::new(b),
                });
            }
            Step::Fill(r, rgb) => d
                .batches
                .push(batch(vec![quad(r, [rgb[0], rgb[1], rgb[2], 255])])),
        }
    }
    d
}

fn frame(renderer: &Gpu, pass: &mut GuiPass, d: &GuiDraws) -> Result<Frame, String> {
    let t = renderer
        .offscreen(d.size.0, d.size.1)
        .map_err(|e| e.to_string())?;
    pass.draw(renderer, d, &t.color, Some([0.0, 0.0, 0.0, 1.0]));
    renderer.read_back(&t).map_err(|e| e.to_string())
}

/// Draws a case on the GPU and with the CPU reference (from the GPU's own base frame), and compares them over the backdrops'
/// areas.
fn judge(renderer: &Gpu, pass: &mut GuiPass, c: &Case) -> Result<Judged, String> {
    let base = frame(renderer, pass, &draws_of(c, true))?;
    let gpu = frame(renderer, pass, &draws_of(c, false))?;
    let mut cpu = base.rgba.clone();
    let w = c.size.0 as usize;
    let mut mask = vec![false; (c.size.0 * c.size.1) as usize];
    for s in &c.steps {
        match *s {
            Step::Backdrop(b) => {
                blur::reference(&mut cpu, c.size, &b, true);
                if let Some((area, _)) = blur::placement(&b, c.size) {
                    for y in area[1]..area[3] {
                        for x in area[0]..area[2] {
                            mask[y as usize * w + x as usize] = true;
                        }
                    }
                }
            }
            Step::Fill(r, rgb) => {
                for y in r[1].max(0)..(r[1] + r[3]).min(c.size.1 as i32) {
                    for x in r[0].max(0)..(r[0] + r[2]).min(c.size.0 as i32) {
                        let i = (y as usize * w + x as usize) * 4;
                        cpu[i..i + 3].copy_from_slice(&rgb);
                        cpu[i + 3] = 255;
                    }
                }
            }
        }
    }
    let (mut sum, mut max, mut n) = (0u64, 0u8, 0u64);
    for (p, m) in mask.iter().enumerate() {
        if !m {
            continue;
        }
        n += 1;
        for k in 0..3 {
            let d = gpu.rgba[p * 4 + k].abs_diff(cpu[p * 4 + k]);
            sum += u64::from(d);
            max = max.max(d);
        }
    }
    Ok(Judged {
        name: c.name.clone(),
        mae: if n == 0 {
            0.0
        } else {
            sum as f64 / (3 * n) as f64
        },
        max,
        pixels: n,
    })
}

#[test]
fn the_gpu_blur_matches_the_cpu_reference() {
    let r = match Gpu::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let mut pass = GuiPass::new(&r, Some(Box::new(Blur::new(&r))));
    let (single, nested) = cases();
    for c in single.iter().chain(&nested) {
        let j = judge(&r, &mut pass, c).expect("drawn");
        eprintln!(
            "{}: MAE {:.4} max {} over {} px",
            j.name, j.mae, j.max, j.pixels
        );
        assert!(j.mae <= 0.5, "{j:?}");
    }
}

#[test]
fn levels_and_placement_are_capped_and_cut_to_the_target() {
    assert_eq!(blur::levels(0), 0);
    assert_eq!(blur::levels(1), 1);
    assert_eq!(blur::levels(4), 1);
    assert_eq!(blur::levels(8), 2);
    assert_eq!(blur::levels(64), 5);
    assert_eq!(blur::levels(u32::MAX), 5);
    let mut b = backdrop(
        [-1_000_000, -1_000_000, i32::MAX, i32::MAX],
        u32::MAX,
        u32::MAX,
        [0; 4],
        f32::NAN,
    );
    let (area, grab) = blur::placement(&b, (100, 50)).expect("inside");
    assert_eq!(area, [0, 0, 100, 50]);
    assert_eq!(grab, [0, 0, 100, 50]);
    // From the far left, a width that ends before the target is empty.
    b.rect = [i32::MIN, 0, i32::MAX, 10];
    assert!(blur::placement(&b, (100, 50)).is_none());
    b.rect = [200, 0, 10, 10];
    assert!(blur::placement(&b, (100, 50)).is_none());
    b.rect = [10, 10, -5, 10];
    assert!(blur::placement(&b, (100, 50)).is_none());
}
