//! `phy <out-dir> <model.mdl>...`: the models side by side on a checkered ground, each with its collision hulls drawn over it
//! as wireframes (one color per convex piece), and a line of counts for each: the source of its hulls, the pieces, the fallbacks.

use std::path::PathBuf;

use torque_format_valve::collision::{HullSource, ModelCollision};
use torque_format_valve::load_model;
use torque_format_valve::model::{Choice, INCH};
use torque_modelcore::math;
use torque_render_hd::gizmos::GizmoSegment;

use crate::Content;
use crate::draw::{Renderer, bounds, ground, object};

const SIZE: (u32, u32) = (1600, 800);

const COLORS: [[u8; 4]; 6] = [
    [255, 64, 64, 255],
    [64, 255, 96, 255],
    [255, 220, 40, 255],
    [80, 200, 255, 255],
    [255, 100, 255, 255],
    [255, 160, 60, 255],
];

/// A model-space point (Source inches, Source axes) in the world's meters (a quarter turn about z), then moved by `at`.
fn to_world(p: [f32; 3], at: [f32; 3]) -> [f32; 3] {
    [
        -p[1] * INCH + at[0],
        p[0] * INCH + at[1],
        p[2] * INCH + at[2],
    ]
}

pub fn run(c: &Content, args: &[String]) -> Result<(), String> {
    let out = PathBuf::from(args.first().ok_or("phy <out-dir> <model.mdl>...")?);
    let paths = &args[1..];
    if paths.is_empty() {
        return Err("phy <out-dir> <model.mdl>...".into());
    }
    let mut r = Renderer::new(SIZE)?;
    let mut draws = Vec::new();
    let mut lines: Vec<GizmoSegment> = Vec::new();
    let mut cursor = 0.0f32;
    let mut height = 0.0f32;
    for path in paths {
        let m = load_model(&c.assets, &c.files, path, &Choice::default(), &mut |_| {
            false
        })
        .map_err(|e| format!("{path}: {e}"))?;
        let col = ModelCollision::load(&c.assets, &c.files, path).map_err(|e| e.to_string())?;
        let rest = r.model_draws(c, &m, None, 0.0, &math::IDENTITY, "model");
        let (lo, hi) = bounds(&rest);
        // The model's own box, then moved so its low corner sits at the cursor on the ground, centered in y.
        let at = [cursor - lo[0], -0.5 * (lo[1] + hi[1]), -lo[2]];
        let place = math::trs(at, [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        draws.extend(r.model_draws(c, &m, None, 0.0, &place, "model"));
        let mut edges = 0;
        for (k, h) in col.hulls.iter().enumerate() {
            let color = COLORS[k % COLORS.len()];
            for e in h.edges() {
                let a = to_world(h.points[e[0] as usize], at);
                let b = to_world(h.points[e[1] as usize], at);
                lines.push(GizmoSegment {
                    a,
                    b,
                    color,
                    depth_test: false,
                });
                edges += 1;
            }
        }
        let source = match col.source {
            HullSource::Phy => "physics file",
            HullSource::MdlBox => "model hull box (fallback)",
            HullSource::RenderHull => "render mesh hull (fallback)",
        };
        println!(
            "{path}: {source}, {} hulls, {edges} edges drawn",
            col.hulls.len()
        );
        for n in &col.notes {
            println!("  note: {n}");
        }
        cursor += (hi[0] - lo[0]).max(0.3) + 0.5;
        height = height.max(hi[2] - lo[2]);
    }
    let (lo, hi) = bounds(&draws);
    let center = [0.5 * (lo[0] + hi[0]), 0.0, 0.5 * height];
    let half = 0.5 * (hi[0] - lo[0]).max(1.0);
    let g = ground(&mut r, (half * 3.0).max(4.0), 0.0);
    r.scene.frame.planes = vec![g];
    r.scene.frame.planes_generation += 1;
    r.scene.frame.objects = vec![object(1, draws)];
    let eye = [center[0], -(half * 1.25 + height * 1.1), height * 1.25];
    let file = out.join("hulls.png");
    r.shot_lines(eye, center, 24, &file, &lines)?;
    println!("wrote {}", file.display());
    Ok(())
}
