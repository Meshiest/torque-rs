//! Pictures of the projected decals: a floor with two walls meeting in a corner, a pillar and a ball, shot full of bullet holes
//! and scorch marks (multiplied over the surfaces) with paint splats (blended by their alpha), and a figure (a moving shape)
//! standing in a scorch, which takes none of it. Drawn at High: `wide` (the whole corner), `closeup` (a few holes at the
//! corner), `wide-cpu` (the same decals through the CPU path's clipped triangles, to compare) and `figure`.
//!
//! `cargo run -p torque-render-hd --release --example decals-sheet -- <dir> [--size WxH] [--count N]` writes
//! `<dir>/decals-<name>.png`. The textures are made in code (`decal_kit`).

#[path = "decal_kit/mod.rs"]
mod decal_kit;

use std::path::{Path, PathBuf};

use decal_kit::{DecalTextures, Surface, decal};
use torque_render_core::{Frame, Gpu};
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_hd::{DecalProjection, HdWorld};
use torque_render_scene::Blend;
use torque_render_scene::decals::{DecalData, DecalManager, Triangle};
use torque_render_scene::hd::kit::{Lcg, box_object, look_at};
use torque_render_scene::hd::{HdCamera, HdMode, Quality};
use torque_render_scene::source::Eye;
use torque_render_scene::textures::TextureSet;

/// The corner's surfaces: the floor, the walls along x = -4 and y = 6, a pillar and a ball.
fn surfaces() -> Vec<Surface> {
    vec![
        Surface::block([0.0, 2.0, -0.5], [14.0, 14.0, 0.5], [138, 128, 112, 255]),
        Surface::block([-4.5, 2.0, 2.0], [0.5, 8.0, 2.0], [176, 168, 150, 255]),
        Surface::block([2.0, 6.5, 2.0], [6.0, 0.5, 2.0], [160, 158, 150, 255]),
        Surface::block([1.6, 3.4, 1.2], [0.35, 0.35, 1.2], [120, 130, 140, 255]),
        Surface::ball([-1.6, 3.2, 0.7], 0.7, [150, 120, 100, 255]),
    ]
}

/// Where the figure stands (its center), in a scorch on the floor.
const FIGURE: [f32; 3] = [-0.4, 1.2, 0.9];

/// A hit: the decal, where, the surface's normal and the turn.
type Hit = (DecalData, [f32; 3], [f32; 3], f32);

/// `count` hits over the walls, the floor (most near the corner), the pillar's front and the ball.
fn hits(count: usize) -> Vec<Hit> {
    let mut rng = Lcg::new(11);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let r = rng.unit();
        let size = 0.18 + 0.3 * rng.unit() * rng.unit();
        let (name, blend) = match i % 23 {
            0 | 11 => ("scorch", Blend::Modulate2x),
            5 => ("splat", Blend::Alpha),
            _ => ("hole", Blend::Modulate2x),
        };
        let mut d = decal(name, [size, size], blend);
        if name != "hole" {
            d.size = [size * 3.0, size * 3.0];
        }
        let (u, v) = (rng.unit(), rng.unit());
        let (at, n) = if r < 0.35 {
            // The wall along x = -4, more of them low and near the corner.
            ([-4.0, 6.0 - 7.0 * u * u, 3.6 * v * v], [1.0, 0.0, 0.0])
        } else if r < 0.7 {
            // The wall along y = 6.
            ([-4.0 + 9.0 * u * u, 6.0, 3.6 * v * v], [0.0, -1.0, 0.0])
        } else if r < 0.9 {
            (
                [-4.0 + 7.0 * u * u, 6.0 - 6.0 * v * v, 0.0],
                [0.0, 0.0, 1.0],
            )
        } else if r < 0.96 {
            ([1.6 - 0.3 + 0.6 * u, 3.05, 0.2 + 2.0 * v], [0.0, -1.0, 0.0])
        } else {
            let (a, b) = (std::f32::consts::TAU * u, 0.9 * v - 0.2);
            let n = [b.cos() * a.cos(), -b.cos() * a.sin().abs(), b.sin()];
            ([-1.6 + 0.7 * n[0], 3.2 + 0.7 * n[1], 0.7 + 0.7 * n[2]], n)
        };
        out.push((d, at, n, rng.unit() * std::f32::consts::TAU));
    }
    out
}

struct Shot {
    name: &'static str,
    eye: Eye,
    cpu: bool,
    figure: bool,
}

fn draw(gpu: &Gpu, size: (u32, u32), hits: &[Hit], shot: &Shot) -> Result<Frame, String> {
    let surfaces = surfaces();
    let mut s = PlainScene {
        mode: Some(HdMode::new(Quality::High)),
        ..PlainScene::default()
    };
    s.frame.sun.towards = [0.45, -0.55, 0.7];
    s.frame.sun.diffuse = [0.85, 0.82, 0.76];
    s.frame.sun.ambient = [0.38, 0.4, 0.45];
    s.frame.planes = surfaces.iter().map(|f| f.draw.clone()).collect();
    s.frame.planes_generation += 1;
    if shot.figure {
        s.frame.objects = vec![box_object(9, FIGURE, [0.3, 0.22, 0.9])];
    }
    s.frame.eye = Some(shot.eye);
    s.frame.fov = 70;
    s.frame.size = size;
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.features.projected_decals = if shot.cpu {
        DecalProjection::Off
    } else {
        DecalProjection::On
    };
    let triangles: Vec<Triangle> = surfaces
        .iter()
        .flat_map(|f| f.triangles.iter().copied())
        .collect();
    let mut m = DecalManager::new(hits.len(), 1.0e9, 1.0);
    if !shot.cpu {
        m.set_projected(Some(hits.len()));
    }
    for (d, at, n, angle) in hits {
        if shot.cpu {
            m.add_clipped(d, *at, *n, *angle, &triangles);
        } else {
            m.add_box(d, *at, *n, *angle);
        }
    }
    if shot.figure {
        m.add_box(
            &decal("scorch", [2.2, 2.2], Blend::Modulate2x),
            [FIGURE[0], FIGURE[1], 0.0],
            [0.0, 0.0, 1.0],
            0.5,
        );
    }
    let mut set = TextureSet::new(&DecalTextures);
    if shot.cpu {
        let draws = m.draws(shot.eye.position, &mut set);
        s.frame.set_particles_and_decals(&[], &draws, set);
    } else {
        let boxes = m.boxes(&mut set);
        s.frame.set_decals(&boxes, set);
    }
    let camera = HdCamera::new(shot.eye, 70.0f32, 200.0, size.0, size.1);
    s.frame.generation += 1;
    world.upload(&s);
    // The TAA's history settled.
    for _ in 0..24 {
        world.draw(&s, &camera, None)?;
    }
    world.read_ldr()
}

fn save(f: &Frame, path: &Path) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), f.width, f.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&f.rgba).map_err(|e| e.to_string())
}

fn main() -> Result<(), String> {
    let mut it = std::env::args().skip(1);
    let mut dir: Option<PathBuf> = None;
    let mut size = (1920, 1080);
    let mut count = 600;
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').ok_or(format!("--size {v}: WxH"))?;
                size = (
                    w.parse().map_err(|e| format!("--size: {e}"))?,
                    h.parse().map_err(|e| format!("--size: {e}"))?,
                );
            }
            "--count" => count = value()?.parse().map_err(|e| format!("--count: {e}"))?,
            _ if dir.is_none() => dir = Some(PathBuf::from(arg)),
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    let dir = dir.ok_or("the folder to write into")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let hits = hits(count);
    let wide = look_at([4.5, -3.5, 3.2], [-2.0, 4.5, 0.8]);
    let shots = [
        Shot {
            name: "wide",
            eye: wide,
            cpu: false,
            figure: false,
        },
        Shot {
            name: "closeup",
            eye: look_at([-2.6, 4.4, 0.9], [-4.0, 6.0, 0.4]),
            cpu: false,
            figure: false,
        },
        Shot {
            name: "wide-cpu",
            eye: wide,
            cpu: true,
            figure: false,
        },
        Shot {
            name: "figure",
            eye: look_at([1.9, -1.6, 1.8], [FIGURE[0], FIGURE[1], 0.6]),
            cpu: false,
            figure: true,
        },
    ];
    for shot in &shots {
        let f = draw(&gpu, size, &hits, shot)?;
        let path = dir.join(format!("decals-{}.png", shot.name));
        save(&f, &path)?;
        println!("{}", path.display());
    }
    Ok(())
}
