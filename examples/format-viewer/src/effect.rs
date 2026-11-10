//! `effect <out-dir> <particles/x.pcf> [system|--list] [frames] [--time T]`: a particle system and its children simulated
//! (the functions `torque_format_valve::pcf::Sim` knows) and drawn as camera-facing sprites of their materials' textures (a
//! sprite sheet's sequence when the texture has one), one picture a frame at 30 frames a second.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use torque_format_valve::model::INCH;
use torque_format_valve::pcf::{Pcf, Sim};
use torque_format_valve::vmt::{Blend as VmtBlend, texture_path};
use torque_render_scene::hd::{HdDraw, HdVertex, material, oct_encode};
use torque_render_scene::{Blend, Cull};

use crate::Content;
use crate::draw::{Renderer, ground, sampler};

const SIZE: (u32, u32) = (1280, 720);

/// A system's sprites as drawn: its texture, sheet and blend.
struct SpriteLook {
    texture: Option<usize>,
    sheet: Option<torque_format_valve::pcf::Sheet>,
    blend: Blend,
}

/// A soft round sprite: white, its alpha falling off from the middle.
fn puff(r: &mut Renderer) -> usize {
    let mut rgba = Vec::with_capacity(64 * 64 * 4);
    for y in 0..64 {
        for x in 0..64 {
            let (dx, dy) = ((x as f32 - 31.5) / 31.5, (y as f32 - 31.5) / 31.5);
            let a = (1.0 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0).powf(1.5);
            rgba.extend_from_slice(&[255, 255, 255, (a * 255.0) as u8]);
        }
    }
    r.scene.frame.textures.intern(torque_render_scene::Texture {
        path: "viewer-puff".into(),
        width: 64,
        height: 64,
        levels: std::sync::Arc::new(vec![rgba]),
    })
}

fn flag(args: &[String], name: &str) -> Option<f32> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
}

pub fn run(c: &Content, args: &[String]) -> Result<(), String> {
    let usage = "effect <out-dir> <particles/x.pcf> [system|--list] [frames] [--time T]";
    let out = PathBuf::from(args.first().ok_or(usage)?);
    let path = args.get(1).ok_or(usage)?;
    let pcf: Rc<Pcf> = c
        .assets
        .load(&c.files, path)
        .map_err(|e| format!("{path}: {e}"))?;
    let systems = pcf.systems();
    let positional: Vec<&String> = args[2..].iter().filter(|a| !a.starts_with("--")).collect();
    if args.iter().any(|a| a == "--list") || positional.is_empty() {
        for s in &systems {
            println!(
                "{} ({} emitters, {} initializers, {} operators, {} children, {})",
                s.name,
                s.emitters.len(),
                s.initializers.len(),
                s.operators.len(),
                s.children.len(),
                s.material().unwrap_or("no material")
            );
        }
        return Ok(());
    }
    let name = positional[0].as_str();
    let frames: usize = positional.get(1).and_then(|f| f.parse().ok()).unwrap_or(90);
    // The system and its children (down a few levels), each simulated from the same origin.
    let mut names = vec![name.to_string()];
    let mut k = 0;
    while k < names.len() && names.len() < 16 {
        if let Some(s) = pcf.system(&names[k]) {
            for ch in &s.children {
                if !names.iter().any(|n| n.eq_ignore_ascii_case(ch)) {
                    names.push(ch.to_string());
                }
            }
        }
        k += 1;
    }
    let mut sims = Vec::new();
    for (i, n) in names.iter().enumerate() {
        let s = pcf.system(n).ok_or(format!("{path} has no system {n}"))?;
        let known = s
            .emitters
            .iter()
            .chain(&s.initializers)
            .chain(&s.operators)
            .map(|f| f.name)
            .collect::<Vec<_>>();
        println!("{n}: material {:?}, functions {known:?}", s.material());
        // A heat-shimmer system draws the frame behind it bent (a refraction shader): left out here.
        if s.material()
            .and_then(|m| c.material(m))
            .is_some_and(|m| m.shader == "refract")
        {
            println!("  (left out: a refraction material)");
            continue;
        }
        sims.push((
            s.material().map(str::to_string),
            Sim::new(&s, 0x9e37_79b9 + i as u64),
        ));
    }
    let mut r = Renderer::new(SIZE)?;
    r.set_time(flag(args, "--time").unwrap_or(0.49), 230.0);
    let g = ground(&mut r, 20.0, 0.0);
    r.scene.frame.planes = vec![g];
    r.scene.frame.planes_generation += 1;
    let mut looks: HashMap<String, Rc<SpriteLook>> = HashMap::new();
    let mut look_of = |r: &mut Renderer, mat: &Option<String>| -> Rc<SpriteLook> {
        let key = mat.clone().unwrap_or_default();
        if let Some(l) = looks.get(&key) {
            return l.clone();
        }
        let vmt = mat.as_deref().and_then(|m| c.material(m));
        let tex_path = vmt
            .as_ref()
            .and_then(|m| m.get("$basetexture").map(texture_path));
        // A material the game does not have draws as a soft round puff.
        let texture = tex_path
            .as_deref()
            .and_then(|p| r.texture(c, p))
            .or_else(|| {
                println!("  material {key} not found: drawn as a soft puff");
                Some(puff(r))
            });
        let sheet = tex_path
            .as_deref()
            .and_then(|p| c.texture(p))
            .and_then(|t| t.sheet.clone());
        let blend = match vmt.as_ref().map(|m| m.blend()) {
            Some(VmtBlend::Additive) => Blend::Additive,
            _ => Blend::Alpha,
        };
        let l = Rc::new(SpriteLook {
            texture,
            sheet,
            blend,
        });
        looks.insert(key, l.clone());
        l
    };
    // Source places the effect at its origin; it stands a little above the ground here.
    let origin = [0.0f32, 0.0, 0.0];
    let to_world = |p: [f32; 3]| {
        [
            origin[0] - p[1] * INCH,
            origin[1] + p[0] * INCH,
            origin[2] + p[2] * INCH,
        ]
    };
    // A first run to frame the camera on where the particles go.
    let mut probe = sims.clone();
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for _ in 0..frames {
        for (i, (_, s)) in probe.iter_mut().enumerate() {
            s.step(1.0 / 30.0);
            // The camera frames the system itself; its children (smoke, embers) may drift out of the picture.
            if i > 0 {
                continue;
            }
            for p in &s.particles {
                let w = to_world(p.position);
                for k in 0..3 {
                    lo[k] = lo[k].min(w[k] - p.radius * INCH);
                    hi[k] = hi[k].max(w[k] + p.radius * INCH);
                }
            }
        }
    }
    if !lo[0].is_finite() {
        return Err(format!(
            "{name} made no particles the simulation knows how to emit"
        ));
    }
    let center = [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
    let radius = (0..3)
        .map(|k| (hi[k] - lo[k]) * 0.5)
        .map(|d| d * d)
        .sum::<f32>()
        .sqrt()
        .max(0.3);
    let eye = [
        center[0] + radius * 0.9,
        center[1] - radius * 1.9,
        center[2] + radius * 0.35,
    ];
    r.world.reset_history();
    for f in 0..frames {
        let mut draws = Vec::new();
        let right = {
            let d = [center[0] - eye[0], center[1] - eye[1]];
            let l = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-6);
            [d[1] / l, -d[0] / l, 0.0]
        };
        let fwd = {
            let d = [center[0] - eye[0], center[1] - eye[1], center[2] - eye[2]];
            let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
            d.map(|x| x / l)
        };
        let up = [
            right[1] * fwd[2] - right[2] * fwd[1],
            right[2] * fwd[0] - right[0] * fwd[2],
            right[0] * fwd[1] - right[1] * fwd[0],
        ];
        for (mat, sim) in &mut sims {
            sim.step(1.0 / 30.0);
            let look = look_of(&mut r, mat);
            let mut d = HdDraw {
                label: format!("particles {}", mat.as_deref().unwrap_or("")),
                vertices: Vec::new(),
                indices: Vec::new(),
                texture: look.texture.map(|t| (t, sampler())),
                blend: look.blend,
                cull: Cull::None,
                depth_test: true,
                depth_write: false,
            };
            let mut word =
                (material::KIND_SKY << material::KIND_SHIFT) | material::PARTICLE | material::UNLIT;
            if look.texture.is_some() {
                word |= material::tex_env(torque_render_scene::TexEnv::Modulate);
            }
            if look.blend == Blend::Additive {
                word |= material::EMITTER;
            }
            for p in &sim.particles {
                let c = to_world(p.position);
                let s = p.radius * INCH;
                let rect = look
                    .sheet
                    .as_ref()
                    .map_or([0.0, 0.0, 1.0, 1.0], |sh| sh.rect(p.sequence));
                let (sn, cs) = p.rotation.sin_cos();
                let base = d.vertices.len() as u32;
                for (cx, cy, u, v) in [
                    (-1.0f32, -1.0f32, rect[0], rect[3]),
                    (1.0, -1.0, rect[2], rect[3]),
                    (1.0, 1.0, rect[2], rect[1]),
                    (-1.0, 1.0, rect[0], rect[1]),
                ] {
                    let (x, y) = (cx * cs - cy * sn, cx * sn + cy * cs);
                    let pos = [0, 1, 2].map(|k| c[k] + (right[k] * x + up[k] * y) * s);
                    let col = p.color.map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8);
                    d.vertices.push(HdVertex {
                        position: pos,
                        normal: oct_encode([-fwd[0], -fwd[1], -fwd[2]]),
                        uv: [u, v],
                        color: col,
                        material: word,
                        extra: 0,
                    });
                }
                d.indices
                    .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            }
            if !d.indices.is_empty() {
                let dist = (0..3)
                    .map(|k| (center[k] - eye[k]).powi(2))
                    .sum::<f32>()
                    .sqrt();
                draws.push((dist, d));
            }
        }
        r.scene.frame.particles = draws;
        r.scene.frame.particles_generation += 1;
        let file = out.join(format!("frame-{f:04}.png"));
        r.shot(eye, center, if f == 0 { 8 } else { 2 }, &file)?;
    }
    let live: usize = sims.iter().map(|s| s.1.particles.len()).sum();
    println!(
        "wrote {frames} frames of {name} ({live} particles at the end) to {}",
        out.display()
    );
    Ok(())
}
