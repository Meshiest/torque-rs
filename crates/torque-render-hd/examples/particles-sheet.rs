//! Ten particle effects (smoke, sparks, fire, dust, explosion, snow, rain, debris, bounce, swirl) drawn one picture each through
//! the GPU particle pool or, with `--cpu`, the CPU path's quads: the pictures a look is judged by.
//!
//! Every effect is a few particle datablocks as the client holds them (the engine's optional fields too: the fire's flipbook
//! blends its frames, the sparks and the rain stretch along their motion) and a script of emitters that runs 160 frames of
//! 16 ms before the picture. The textures are made here.
//!
//! `cargo run -p torque-render-hd --release --example particles-sheet -- --out <dir> [--size WxH] [--quality low|medium|high|ultra]
//! [--cpu] [--only <name>]...` writes `<dir>/<name>.png` for each effect. The pictures are for looking at: a timing run takes
//! the GPU bench lock like any other.

use std::path::PathBuf;

use torque_render_core::Gpu;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_hd::{HdWorld, ParticlePool};
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::hd::particle_light::ParticleLight;
use torque_render_scene::hd::{HdCamera, HdMode, Quality, material};
use torque_render_scene::particles::{
    Emitter, EmitterData, EmitterExtras, ParticleData, ParticleExtras, Rng, SpriteFrame,
};
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Assets, Draw};

const FRAMES: u32 = 160;
const MS: u32 = 16;
const SIDE: u32 = 64;

/// The effects, in the order of the sheet.
const NAMES: [&str; 10] = [
    "smoke",
    "sparks",
    "fire",
    "dust",
    "explosion",
    "snow",
    "rain",
    "debris",
    "bounce",
    "swirl",
];

struct Args {
    out: PathBuf,
    size: (u32, u32),
    quality: Quality,
    cpu: bool,
    only: Vec<String>,
}

fn args() -> Result<Args, String> {
    let mut a = Args {
        out: PathBuf::new(),
        size: (1920, 1080),
        quality: Quality::High,
        cpu: false,
        only: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--out" => a.out = PathBuf::from(value()?),
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').ok_or(format!("--size {v}: WxH"))?;
                a.size = (
                    w.parse().map_err(|e| format!("--size: {e}"))?,
                    h.parse().map_err(|e| format!("--size: {e}"))?,
                );
            }
            "--quality" => {
                let q = value()?;
                a.quality = Quality::ALL
                    .into_iter()
                    .find(|x| x.name() == q)
                    .ok_or(format!("no quality {q}"))?;
            }
            "--cpu" => a.cpu = true,
            "--only" => a.only.push(value()?),
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    if a.out.as_os_str().is_empty() {
        return Err("--out <dir> is needed".into());
    }
    Ok(a)
}

/// A texel's alpha from its distance `d` (0 at the middle, 1 at the edge of a `SIDE` square) for each texture.
fn texel(name: &str, x: u32, y: u32) -> [u8; 4] {
    let (u, v) = (
        (x as f32 + 0.5) / SIDE as f32 * 2.0 - 1.0,
        (y as f32 + 0.5) / SIDE as f32 * 2.0 - 1.0,
    );
    let d = u.hypot(v);
    let soft = (1.0 - d).clamp(0.0, 1.0);
    let a = match name {
        "puff" => soft * soft,
        "spark" => soft.powi(3),
        // A hard-edged chunk.
        "chunk" => {
            if u.abs() < 0.8 && v.abs() < 0.8 {
                1.0
            } else {
                0.0
            }
        }
        // A rain drop: a thin vertical streak, round at its ends.
        "drop" => (1.0 - u.abs() * 3.0).clamp(0.0, 1.0) * (1.0 - v.abs().powi(4)),
        // Four frames of a flame: a teardrop that leans and flickers.
        flame => {
            let k: f32 = flame.trim_start_matches("flame").parse().unwrap_or(0.0);
            let lean = (k * 1.7).sin() * 0.25 * (1.0 - v) * 0.5;
            let width = 0.55 - 0.3 * (v * 0.5 + 0.5) + 0.07 * (k * 2.3 + v * 5.0).sin();
            let edge = ((width - (u - lean).abs()) / width).clamp(0.0, 1.0);
            let ends = (1.0 - v.abs().powi(3)).clamp(0.0, 1.0);
            (edge * ends).powf(0.8)
        }
    };
    [255, 255, 255, (a.clamp(0.0, 1.0) * 255.0) as u8]
}

/// The effects' textures as files (`fx/<name>.png`).
struct Fx;

impl Assets for Fx {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let name = path.strip_prefix("fx/")?.strip_suffix(".png")?;
        let mut texels = Vec::with_capacity((SIDE * SIDE * 4) as usize);
        for y in 0..SIDE {
            for x in 0..SIDE {
                texels.extend_from_slice(&texel(name, x, y));
            }
        }
        let mut png = Vec::new();
        let mut enc = png::Encoder::new(&mut png, SIDE, SIDE);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(&texels).ok()?;
        drop(w);
        Some(png)
    }
}

/// The emitter's rules, shared by the effects: its particle datablock follows.
struct Rules {
    period_ms: i32,
    velocity: f32,
    variance: f32,
    theta: (f32, f32),
}

fn emitter(r: Rules, p: ParticleData) -> Emitter {
    let data = EmitterData {
        period_ms: r.period_ms,
        period_variance_ms: r.period_ms / 4,
        velocity: r.velocity,
        velocity_variance: r.variance,
        offset: 0.05,
        theta_min: r.theta.0,
        theta_max: r.theta.1,
        phi_ref_vel: 0.0,
        phi_variance: 360.0,
        override_advance: false,
        lifetime_ms: 0,
        use_emitter_sizes: false,
        use_emitter_colors: false,
        particles: vec![1],
        extras: EmitterExtras::default(),
    };
    Emitter::of_data(data, vec![p]).expect("an emitter")
}

/// A particle datablock of `texture`, blended (`inv_alpha`) or added, its colors' rgb with the alpha keys, and its sizes.
#[allow(clippy::too_many_arguments)]
fn particle(
    texture: &[&str],
    inv_alpha: bool,
    rgb: [[f32; 3]; 2],
    alpha: [f32; 4],
    sizes: [f32; 4],
    life_ms: i32,
    gravity: f32,
    drag: f32,
    extras: ParticleExtras,
) -> ParticleData {
    let color = |k: usize, a: f32| {
        let c = rgb[usize::from(k >= 2)];
        [c[0], c[1], c[2], a]
    };
    ParticleData {
        drag,
        wind: 1.0,
        gravity,
        inherited_vel: 0.0,
        constant_accel: 0.0,
        lifetime_ms: life_ms,
        lifetime_variance_ms: life_ms / 5,
        spin_speed: 25.0,
        spin_min: -40.0,
        spin_max: 40.0,
        inv_alpha,
        colors: [
            color(0, alpha[0]),
            color(1, alpha[1]),
            color(2, alpha[2]),
            color(3, alpha[3]),
        ],
        sizes,
        times: [0.0, 0.2, 0.65, 1.0],
        textures: texture.iter().map(|t| format!("fx/{t}")).collect(),
        animate: texture.len() > 1,
        fps: if texture.len() > 1 { 8 } else { 0 },
        extras,
    }
}

/// One emitter of an effect: where it stands, and whether it burst `count` particles at `burst_frame` instead of running.
struct Source {
    emitter: Emitter,
    at: [f32; 3],
    burst: Option<(u32, u32, f32)>,
}

fn continuous(emitter: Emitter, at: [f32; 3]) -> Source {
    Source {
        emitter,
        at,
        burst: None,
    }
}

/// The effect `name`: its sources, and the camera (eye, target).
fn effect(name: &str) -> (Vec<Source>, [f32; 3], [f32; 3]) {
    let x = ParticleExtras::default;
    let eye = [0.0, -7.0, 2.2];
    let look = [0.0, 0.0, 1.5];
    match name {
        "smoke" => {
            let p = particle(
                &["puff"],
                true,
                [[0.62, 0.6, 0.57], [0.4, 0.4, 0.4]],
                [0.0, 0.55, 0.35, 0.0],
                [0.5, 0.9, 1.5, 2.2],
                3600,
                -0.06,
                0.5,
                x(),
            );
            let r = Rules {
                period_ms: 22,
                velocity: 0.9,
                variance: 0.3,
                theta: (0.0, 18.0),
            };
            (vec![continuous(emitter(r, p), [0.0, 0.0, 0.2])], eye, look)
        }
        "sparks" => {
            let extras = ParticleExtras {
                size_by_velocity: 0.035,
                ..x()
            };
            let p = particle(
                &["spark"],
                false,
                [[1.0, 0.85, 0.5], [0.9, 0.3, 0.05]],
                [1.0, 0.9, 0.6, 0.0],
                [0.07, 0.06, 0.05, 0.03],
                900,
                1.0,
                0.1,
                extras,
            );
            let r = Rules {
                period_ms: 3,
                velocity: 4.0,
                variance: 2.0,
                theta: (0.0, 70.0),
            };
            (vec![continuous(emitter(r, p), [0.0, 0.0, 0.5])], eye, look)
        }
        "bounce" => {
            // Sparks that land on the floor and bounce off it, the depth buffer their ground.
            let extras = ParticleExtras {
                size_by_velocity: 0.03,
                collide: true,
                bounce_elasticity: 0.55,
                bounce_friction: 0.3,
                ..x()
            };
            let p = particle(
                &["spark"],
                false,
                [[1.0, 0.85, 0.5], [0.9, 0.3, 0.05]],
                [1.0, 0.95, 0.8, 0.0],
                [0.07, 0.06, 0.05, 0.04],
                2600,
                1.0,
                0.05,
                extras,
            );
            let r = Rules {
                period_ms: 6,
                velocity: 4.5,
                variance: 1.5,
                theta: (10.0, 55.0),
            };
            (
                vec![continuous(emitter(r, p), [0.0, 0.0, 0.4])],
                [3.5, -5.0, 1.0],
                [0.0, 0.0, 0.5],
            )
        }
        "swirl" => {
            // The smoke again, in smaller puffs carried through curling air.
            let extras = ParticleExtras {
                turbulence: 0.6,
                turbulence_scale: 1.8,
                ..x()
            };
            let p = particle(
                &["puff"],
                true,
                [[0.62, 0.6, 0.57], [0.4, 0.4, 0.4]],
                [0.0, 0.5, 0.3, 0.0],
                [0.25, 0.5, 0.8, 1.1],
                4200,
                -0.06,
                0.5,
                extras,
            );
            let r = Rules {
                period_ms: 10,
                velocity: 1.4,
                variance: 0.3,
                theta: (0.0, 12.0),
            };
            (
                vec![continuous(emitter(r, p), [0.0, 0.0, 0.2])],
                [0.0, -8.0, 2.6],
                [0.0, 0.0, 2.4],
            )
        }
        "fire" => {
            let extras = ParticleExtras {
                flipbook_blend: true,
                ..x()
            };
            let p = particle(
                &["flame0", "flame1", "flame2", "flame3"],
                false,
                [[1.0, 0.75, 0.3], [0.9, 0.2, 0.05]],
                [0.0, 0.9, 0.6, 0.0],
                [0.5, 0.8, 0.7, 0.3],
                900,
                -0.9,
                0.4,
                extras,
            );
            let r = Rules {
                period_ms: 25,
                velocity: 0.6,
                variance: 0.2,
                theta: (0.0, 10.0),
            };
            (vec![continuous(emitter(r, p), [0.0, 0.0, 0.3])], eye, look)
        }
        "dust" => {
            let p = particle(
                &["puff"],
                true,
                [[0.72, 0.62, 0.46], [0.6, 0.5, 0.38]],
                [0.0, 0.45, 0.25, 0.0],
                [0.4, 0.8, 1.2, 1.6],
                1900,
                -0.02,
                1.5,
                x(),
            );
            let r = Rules {
                period_ms: 12,
                velocity: 1.3,
                variance: 0.8,
                theta: (62.0, 90.0),
            };
            (vec![continuous(emitter(r, p), [0.0, 0.0, 0.1])], eye, look)
        }
        "explosion" => {
            let smoke = particle(
                &["puff"],
                true,
                [[0.35, 0.3, 0.27], [0.12, 0.12, 0.12]],
                [0.0, 0.8, 0.6, 0.0],
                [0.4, 0.8, 1.2, 1.5],
                3000,
                -0.1,
                0.9,
                x(),
            );
            let flash = particle(
                &["puff"],
                false,
                [[1.0, 0.8, 0.4], [0.9, 0.3, 0.05]],
                [1.0, 0.9, 0.4, 0.0],
                [0.4, 0.7, 0.9, 0.6],
                900,
                -0.2,
                0.8,
                x(),
            );
            let r = |velocity| Rules {
                period_ms: 1000,
                velocity,
                variance: velocity * 0.4,
                theta: (0.0, 90.0),
            };
            let at = [0.0, 0.0, 1.0];
            (
                vec![
                    Source {
                        burst: Some((110, 140, 0.4)),
                        ..continuous(emitter(r(2.0), smoke), at)
                    },
                    Source {
                        burst: Some((110, 120, 0.3)),
                        ..continuous(emitter(r(3.2), flash), at)
                    },
                ],
                [0.0, -9.0, 2.4],
                look,
            )
        }
        "snow" => {
            let p = particle(
                &["puff"],
                true,
                [[1.0, 1.0, 1.0], [0.9, 0.93, 1.0]],
                [0.0, 0.9, 0.9, 0.0],
                [0.07, 0.07, 0.07, 0.07],
                6000,
                0.12,
                0.6,
                x(),
            );
            let sources = (0..48)
                .map(|k| {
                    let r = Rules {
                        period_ms: 60,
                        velocity: 0.3,
                        variance: 0.2,
                        theta: (0.0, 90.0),
                    };
                    let at = [
                        (k % 12) as f32 * 1.2 - 6.6,
                        (k / 12) as f32 * 2.0 - 1.0,
                        6.0,
                    ];
                    continuous(emitter(r, p.clone()), at)
                })
                .collect();
            (sources, [0.0, -9.0, 2.5], [0.0, 0.0, 2.5])
        }
        "rain" => {
            let extras = ParticleExtras {
                size_by_velocity: 0.045,
                ..x()
            };
            let p = particle(
                &["drop"],
                true,
                [[0.75, 0.8, 0.9], [0.75, 0.8, 0.9]],
                [0.35, 0.5, 0.5, 0.35],
                [0.07, 0.07, 0.07, 0.07],
                500,
                2.0,
                0.0,
                extras,
            );
            let sources = (0..60)
                .map(|k| {
                    let r = Rules {
                        period_ms: 8,
                        velocity: 0.3,
                        variance: 0.2,
                        theta: (0.0, 90.0),
                    };
                    let at = [
                        (k % 15) as f32 * 1.2 - 8.4,
                        (k / 15) as f32 * 1.6 - 1.0,
                        6.5,
                    ];
                    continuous(emitter(r, p.clone()), at)
                })
                .collect();
            (sources, [0.0, -9.0, 2.5], [0.0, 0.0, 2.5])
        }
        _ => {
            let p = particle(
                &["chunk"],
                true,
                [[0.34, 0.27, 0.2], [0.22, 0.18, 0.14]],
                [1.0, 1.0, 1.0, 0.0],
                [0.14, 0.12, 0.12, 0.1],
                1600,
                1.0,
                0.1,
                x(),
            );
            let r = Rules {
                period_ms: 1000,
                velocity: 4.5,
                variance: 2.0,
                theta: (0.0, 80.0),
            };
            (
                vec![Source {
                    burst: Some((110, 90, 0.2)),
                    ..continuous(emitter(r, p), [0.0, 0.0, 0.3])
                }],
                eye,
                look,
            )
        }
    }
}

/// The scene: a floor, a back wall, a sun, and the usual rule that lights blended particles.
fn scene(size: (u32, u32), quality: Quality, eye: [f32; 3], target: [f32; 3]) -> PlainScene {
    let mut s = PlainScene {
        mode: Some(HdMode::new(quality)),
        ..PlainScene::default()
    };
    s.frame.sun.towards = [-0.45, -0.35, 0.82];
    s.frame.sun.diffuse = [0.85, 0.8, 0.72];
    s.frame.sun.ambient = [0.32, 0.34, 0.4];
    let plane = |label: &str, at: [f32; 3], half: [f32; 3]| {
        let mut d = box_object(0, at, half).draws.remove(0);
        d.label = label.into();
        for v in &mut d.vertices {
            v.material = material::KIND_PLANE << material::KIND_SHIFT;
        }
        d
    };
    s.frame.planes = vec![
        plane("floor", [0.0, 0.0, -0.5], [30.0, 30.0, 0.5]),
        plane("wall", [0.0, 7.0, 4.0], [30.0, 0.5, 5.0]),
    ];
    s.frame.planes_generation += 1;
    s.frame.eye = Some(look_at(eye, target));
    s.frame.fov = 60;
    s.frame.size = size;
    s.frame.particle_light = Some(ParticleLight::BLENDED);
    s
}

/// The effect `name` for `FRAMES` frames, its picture at `size`.
fn draw(gpu: &Gpu, a: &Args, name: &str) -> Result<torque_render_core::Frame, String> {
    let (mut sources, eye, target) = effect(name);
    let mut s = scene(a.size, a.quality, eye, target);
    if name == "bounce" {
        // A dim room: the sparks are the light.
        s.frame.sun.diffuse = [0.1, 0.1, 0.12];
        s.frame.sun.ambient = [0.06, 0.06, 0.08];
    }
    let camera = HdCamera::new(
        s.frame.eye.expect("the eye"),
        f32::from(s.frame.fov),
        s.frame.visible_distance,
        a.size.0,
        a.size.1,
    );
    let right = [camera.view[0], camera.view[4], camera.view[8]];
    let up = [camera.view[1], camera.view[5], camera.view[9]];
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.features.gpu_particles = if a.cpu {
        ParticlePool::Off
    } else {
        ParticlePool::Auto
    };
    let pool = world.gpu_particles_at(a.quality);
    if !a.cpu && !pool {
        return Err("this device does not run the GPU particle pool at this quality".into());
    }
    let assets = Fx;
    let mut rng = Rng::new(0x5eed);
    let mut live = 0usize;
    s.frame.generation += 1;
    world.upload(&s);
    for k in 0..FRAMES {
        let mut set = TextureSet::new(&assets);
        let mut sprites = SpriteFrame {
            steps: vec![MS],
            ..SpriteFrame::default()
        };
        let mut draws: Vec<(f32, Draw)> = Vec::new();
        for src in &mut sources {
            let e = &mut src.emitter;
            e.advance(MS, &mut live);
            match src.burst {
                Some((at, count, radius)) if k == at => {
                    e.emit_burst(src.at, [0.0, 0.0, 1.0], radius, count, &mut rng, &mut live);
                }
                Some(_) => {}
                None => e.emit_path(src.at, [0.0, 0.0, 1.0], [0.0; 3], MS, &mut rng, &mut live),
            }
            if pool {
                e.hand_over(&mut sprites, &mut set, name, &mut live);
            } else if let Some(d) = e.draw(right, up, &mut set, name) {
                draws.push((8.0, d));
            }
        }
        s.frame
            .set_particles_decals_and_sprites(&draws, &[], pool.then_some(&sprites), set);
        s.time = k as f32 * MS as f32 / 1000.0;
        s.frame.generation += 1;
        s.frame.control_generation += 1;
        world.upload(&s);
        world.draw(&s, &camera, None)?;
    }
    world.read_ldr()
}

fn save(path: &std::path::Path, f: &torque_render_core::Frame) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), f.width, f.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&f.rgba).map_err(|e| e.to_string())
}

fn main() -> Result<(), String> {
    let a = args()?;
    std::fs::create_dir_all(&a.out).map_err(|e| e.to_string())?;
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    for name in NAMES {
        if !a.only.is_empty() && !a.only.iter().any(|o| o == name) {
            continue;
        }
        let frame = draw(&gpu, &a, name)?;
        let path = a.out.join(format!("{name}.png"));
        save(&path, &frame)?;
        println!("{}", path.display());
    }
    Ok(())
}
