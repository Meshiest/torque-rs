//! `model <out-dir> <model.mdl> [sequence|--list] [frames] [--skin N] [--yaw D] [--cycles N]`: a model on a checkered ground, from four sides
//! at rest, or `frames` pictures of one sequence (`frame-0000.png`, ...; 60 by default) for a clip.

use std::path::PathBuf;

use torque_format_valve::model::Choice;
use torque_format_valve::{load_model, mdl::anim_flags};
use torque_modelcore::math;

use crate::Content;
use crate::draw::{Renderer, bounds, ground, object};

const SIZE: (u32, u32) = (1280, 720);

fn flag(args: &[String], name: &str) -> Option<f32> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
}

pub fn run(c: &Content, args: &[String]) -> Result<(), String> {
    let out = PathBuf::from(
        args.first()
            .ok_or("model <out-dir> <model.mdl> [sequence] [frames]")?,
    );
    let path = args
        .get(1)
        .ok_or("model <out-dir> <model.mdl> [sequence] [frames]")?;
    let positional: Vec<&String> = args[2..].iter().filter(|a| !a.starts_with("--")).collect();
    let seq = positional
        .first()
        .map(|s| s.as_str())
        .filter(|s| s.parse::<f32>().is_err());
    let frames = positional
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(60usize);
    let choice = Choice {
        skin: flag(args, "--skin").unwrap_or(0.0) as usize,
        body: Vec::new(),
    };
    let yaw = flag(args, "--yaw").unwrap_or(30.0);
    let cycles = flag(args, "--cycles").unwrap_or(1.0);
    let t0 = std::time::Instant::now();
    let m = load_model(&c.assets, &c.files, path, &choice, &mut |s| {
        s.flags & anim_flags::DELTA == 0
    })
    .map_err(|e| format!("{path}: {e}"))?;
    let verts: usize = m.model.meshes.iter().map(|x| x.positions.len()).sum();
    let tris: usize = m.model.meshes.iter().map(|x| x.triangles()).sum();
    println!(
        "{path}: {} bones, {} meshes, {verts} vertices, {tris} triangles, {} materials, {} sequences, read in {:.0} ms",
        m.bone_nodes.len(),
        m.model.meshes.len(),
        m.model.materials.len(),
        m.model.animations.len(),
        t0.elapsed().as_secs_f32() * 1000.0
    );
    for (mat, paths) in m.model.materials.iter().zip(&m.material_paths) {
        let found = paths.iter().find(|p| c.files.len_of(p).is_some());
        println!(
            "  material {}: {}",
            mat.name,
            found.map_or_else(|| format!("not found in {paths:?}"), |p| p.clone())
        );
    }
    for n in &m.model.notes {
        println!("  note: {n}");
    }
    if args.iter().any(|a| a == "--list") {
        for a in &m.model.animations {
            println!(
                "  {} ({} keys, {:.2} s{})",
                a.name,
                a.keys,
                a.duration,
                if a.cyclic { ", loops" } else { "" }
            );
        }
        return Ok(());
    }
    let mut r = Renderer::new(SIZE)?;
    let anim = seq.and_then(|s| {
        m.model
            .animations
            .iter()
            .position(|a| a.name.eq_ignore_ascii_case(s))
    });
    if seq.is_some() && anim.is_none() {
        return Err(format!(
            "{path} has no sequence {}",
            seq.unwrap_or_default()
        ));
    }
    let rest = r.model_draws(c, &m, anim, 0.0, &math::IDENTITY, "model");
    let (lo, hi) = bounds(&rest);
    let center = [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
    let radius = (0..3)
        .map(|k| (hi[k] - lo[k]) * 0.5)
        .map(|d| d * d)
        .sum::<f32>()
        .sqrt()
        .max(0.05);
    let g = ground(&mut r, (radius * 6.0).max(4.0), lo[2]);
    r.scene.frame.planes = vec![g];
    r.scene.frame.planes_generation += 1;
    let view = |angle: f32| -> [f32; 3] {
        let a = angle.to_radians();
        let d = radius * 2.3;
        [
            center[0] + d * a.sin(),
            center[1] - d * a.cos(),
            center[2] + radius * 0.6,
        ]
    };
    match anim {
        None => {
            r.scene.frame.objects = vec![object(1, rest)];
            for (i, a) in [yaw, yaw + 90.0, yaw + 180.0, yaw + 270.0]
                .into_iter()
                .enumerate()
            {
                r.world.reset_history();
                let file = out.join(format!("view-{i}.png"));
                r.shot(view(a), center, 24, &file)?;
                println!("wrote {}", file.display());
            }
        }
        Some(a) => {
            let dur = m.model.animations[a].duration;
            let eye = view(yaw);
            r.world.reset_history();
            for f in 0..frames {
                let t = dur * cycles * f as f32 / frames as f32;
                let draws = r.model_draws(c, &m, Some(a), t, &math::IDENTITY, "model");
                r.scene.frame.objects = vec![object(1, draws)];
                let file = out.join(format!("frame-{f:04}.png"));
                r.shot(eye, center, if f == 0 { 16 } else { 3 }, &file)?;
            }
            println!(
                "wrote {frames} frames of {} ({dur:.2} s) to {}",
                m.model.animations[a].name,
                out.display()
            );
        }
    }
    Ok(())
}
