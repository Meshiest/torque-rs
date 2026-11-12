//! The dissolve, recorded: the kit's figure standing as a statue beside a stack of crates on a plain floor at dusk. The
//! statue turns to sand from the top down (its edge a warm glow, the sand falling), the crates burn away into rising embers,
//! then the statue materializes again from the bottom up under a cold edge and sparks in its own colors. Drawn
//! offscreen through the enhanced renderer at High, its particles through the GPU particle pool (on at High).
//!
//! The frames go to `ffmpeg` (on the path) as `dissolve.mp4` (1920 x 1080, 30 frames a second), with two stills beside it:
//! `dissolve-sand.png` (the statue half sand) and `dissolve-embers.png` (the crates burning into embers).
//!
//! `cargo run -p torque-example-common --release --example dissolve-clip -- <dir> [--still <seconds>]` (default `dissolve-clip`;
//! `--still` draws only the frame at that time, as `still.png`)

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use torque::app::view::{Daylight, SceneShape, WorldView};
use torque::app::window::offscreen_gpu;
use torque_example_common::{KitAssets, kit_path};
use torque_render_scene::dissolve::{Direction, Dissolves, Look};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::{DynamicObject, Quality};
use torque_render_scene::math::Mat;
use torque_render_scene::particles::{
    Emitter, EmitterData, EmitterExtras, ParticleData, ParticleExtras, SpriteFrame,
};
use torque_render_scene::textures::TextureSet;

const SIZE: (u32, u32) = (1920, 1080);
const FPS: u32 = 30;
const LENGTH: f32 = 10.0;
/// The stills' times (seconds).
const SAND_AT: f32 = 2.6;
const EMBERS_AT: f32 = 5.0;
/// The statue's ghost and the crates' first.
const STATUE: u32 = 1;
const CRATES: u32 = 10;

/// A particle datablock's look and motion: its texture, its color fading to `fade` and its size to `end_size` over `life_ms`,
/// its gravity, and whether it is blended by its alpha (else added).
struct Grain {
    texture: &'static str,
    color: [f32; 4],
    fade: [f32; 4],
    size: f32,
    end_size: f32,
    life_ms: i32,
    gravity: f32,
    inv_alpha: bool,
    /// Settles on what it lands on where the pool collides (`collide`, the datablock's field; the pool without collision
    /// leaves it falling).
    collide: bool,
}

/// An emitter of `g`, ejecting at `velocity` within `cone` degrees of the surface's normal.
fn emitter(g: &Grain, velocity: f32, cone: f32) -> Emitter {
    let p = ParticleData {
        drag: 0.8,
        wind: 0.0,
        gravity: g.gravity,
        inherited_vel: 0.0,
        constant_accel: 0.0,
        lifetime_ms: g.life_ms,
        lifetime_variance_ms: g.life_ms / 3,
        spin_speed: 0.0,
        spin_min: -90.0,
        spin_max: 90.0,
        inv_alpha: g.inv_alpha,
        colors: [g.color, g.fade, g.fade, g.fade],
        sizes: [g.size, g.end_size, g.end_size, g.end_size],
        times: [0.0, 1.0, 1.0, 1.0],
        textures: vec![kit_path(g.texture)],
        animate: false,
        fps: 0,
        extras: ParticleExtras {
            collide: g.collide,
            bounce_elasticity: 0.15,
            bounce_friction: 0.6,
            ..ParticleExtras::default()
        },
    };
    let data = EmitterData {
        period_ms: 10,
        period_variance_ms: 0,
        velocity,
        velocity_variance: velocity * 0.5,
        offset: 0.0,
        theta_min: 0.0,
        theta_max: cone,
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

/// The sand (alpha-blended, falling), the embers (added, rising) and the sparks (added, drifting; tinted by the statue's
/// colors).
fn sand() -> Emitter {
    let g = Grain {
        texture: "smoke.png",
        color: [1.0, 0.9, 0.68, 1.0],
        fade: [0.9, 0.78, 0.55, 0.0],
        size: 0.014,
        end_size: 0.012,
        life_ms: 1700,
        gravity: 0.35,
        inv_alpha: true,
        collide: true,
    };
    emitter(&g, 0.3, 35.0)
}

fn embers() -> Emitter {
    let g = Grain {
        texture: "spark.png",
        color: [1.0, 0.55, 0.15, 1.0],
        fade: [0.8, 0.1, 0.0, 0.0],
        size: 0.022,
        end_size: 0.008,
        life_ms: 1800,
        gravity: -0.25,
        inv_alpha: false,
        collide: false,
    };
    emitter(&g, 0.4, 60.0)
}

fn sparks() -> Emitter {
    let g = Grain {
        texture: "spark.png",
        color: [1.0, 1.0, 1.0, 1.0],
        fade: [0.3, 0.6, 1.0, 0.0],
        size: 0.02,
        end_size: 0.006,
        life_ms: 900,
        gravity: -0.05,
        inv_alpha: false,
        collide: false,
    };
    emitter(&g, 0.25, 80.0)
}

/// One step of the timeline: at `at` seconds ghost `ghost` starts `look` (its particles from `make`).
struct Cue {
    at: f32,
    ghost: u32,
    look: Look,
    make: fn() -> Emitter,
}

fn timeline() -> Vec<Cue> {
    let base = Look {
        emitter: Some(1),
        ..Look::default()
    };
    vec![
        Cue {
            at: 0.8,
            ghost: STATUE,
            look: Look {
                direction: Direction::Out,
                duration_ms: 3600,
                edge_color: [1.0, 0.72, 0.35],
                glow: 7.0,
                edge_share: 0.05,
                cell: 0.09,
                sweep: 0.55,
                density: 60000.0,
                ..base
            },
            make: sand,
        },
        Cue {
            at: 3.4,
            ghost: CRATES,
            look: Look {
                direction: Direction::Out,
                duration_ms: 3200,
                edge_color: [1.0, 0.38, 0.06],
                glow: 12.0,
                edge_share: 0.06,
                cell: 0.16,
                sweep: 0.3,
                density: 6000.0,
                ..base
            },
            make: embers,
        },
        Cue {
            at: 6.0,
            ghost: STATUE,
            look: Look {
                direction: Direction::In,
                duration_ms: 3200,
                edge_color: [0.35, 0.8, 1.0],
                glow: 9.0,
                edge_share: 0.05,
                tint: true,
                cell: 0.09,
                sweep: 0.55,
                density: 6000.0,
                ..base
            },
            make: sparks,
        },
    ]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut dir = PathBuf::from("dissolve-clip");
    let mut still: Option<f32> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--still" => still = args.next().and_then(|s| s.parse().ok()),
            _ => dir = PathBuf::from(a),
        }
    }
    std::fs::create_dir_all(&dir)?;
    let gpu = offscreen_gpu()?;
    let assets = KitAssets::new()?;
    let figure = SceneShape::load(&assets, &kit_path("figure.dts")).ok_or("figure.dts")?;
    let cube = SceneShape::load(&assets, &kit_path("cube.dts")).ok_or("cube.dts")?;
    let detail = figure.detail().ok_or("no detail")?;
    let mut pose = figure.instance().ok_or("no instance")?;
    let _ = pose.animate(figure.shape(), detail, None, &mut |_| true);
    let cube_detail = cube.detail().ok_or("no cube detail")?;
    let mut cube_pose = cube.instance().ok_or("no cube instance")?;
    let _ = cube_pose.animate(cube.shape(), cube_detail, None, &mut |_| true);
    let mut view = WorldView::new(&gpu);
    view.set_quality(Quality::High);
    // Fine sand is many grains: the pool may hold the quality's most (the default is the CPU path's count).
    view.renderer().set_gpu_particle_cap(250_000);
    view.set_plain_floor(400.0, [118, 112, 104]);
    view.set_daylight(&Daylight {
        towards: [0.45, 0.8, 0.4],
        diffuse: [1.0, 0.78, 0.55],
        ambient: [0.22, 0.24, 0.32],
        shadow: [0.12, 0.13, 0.2],
        fog: [0.55, 0.5, 0.52],
        fog_start: 40.0,
        visible: 300.0,
    });
    // The statue faces the camera; the crates stand to its left as the camera sees them, each turned a little more.
    let mut statue_at = Mat::euler_z(0.35);
    statue_at.set_column3([0.0, 0.0, 0.0]);
    let crate_size = cube.shape().radius.max(0.01);
    let crate_scale = 0.38 / crate_size;
    // A crate's height and how far its lowest point is under its origin, from one drawn in place.
    let probe = view.shape_object_with(
        &assets,
        &cube,
        &cube_pose,
        &Mat::IDENTITY,
        [crate_scale; 3],
        CRATES,
        &mut |_| {},
    );
    let zs = probe
        .draws
        .iter()
        .flat_map(|d| d.vertices.iter().map(|v| v.position[2]));
    let (low, high) = zs.fold((f32::MAX, f32::MIN), |(a, b), z| (a.min(z), b.max(z)));
    let crate_place = |k: usize| {
        let mut m = Mat::euler_z(0.3 * k as f32);
        m.set_column3([-1.5 + 0.06 * k as f32, -0.4, (high - low) * k as f32 - low]);
        m
    };
    let mut runner = Dissolves::default();
    let mut made = 0usize;
    let cues = timeline();
    let mut cued = vec![false; cues.len()];
    let dt = 1.0 / FPS as f32;
    let frames = (LENGTH * FPS as f32) as usize;
    let (from, to) = match still {
        Some(t) => (0, ((t * FPS as f32) as usize).min(frames - 1)),
        None => (0, frames - 1),
    };
    let mp4 = dir.join("dissolve.mp4");
    let mut ffmpeg = if still.is_none() {
        let child = Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-s",
            ])
            .arg(format!("{}x{}", SIZE.0, SIZE.1))
            .args([
                "-r",
                &FPS.to_string(),
                "-i",
                "-",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-crf",
                "17",
            ])
            .arg(&mp4)
            .stdin(Stdio::piped())
            .spawn()?;
        Some(child)
    } else {
        None
    };
    let mut pipe = ffmpeg.as_mut().and_then(|f| f.stdin.take());
    let eye_at = |t: f32| {
        // A slow drift around the pair.
        let a = 0.3 + 0.12 * (t / LENGTH);
        [5.2 * a.sin() - 0.4, 5.8 * a.cos(), 2.0]
    };
    for k in from..=to {
        let t = k as f32 * dt;
        let mut objects: Vec<DynamicObject> =
            vec![view.shape_object(&assets, &figure, &pose, &statue_at, STATUE)];
        // The crates are one object (one dissolve).
        let mut stack = view.shape_object_with(
            &assets,
            &cube,
            &cube_pose,
            &crate_place(0),
            [crate_scale; 3],
            CRATES,
            &mut |_| {},
        );
        for c in 1..3 {
            let o = view.shape_object_with(
                &assets,
                &cube,
                &cube_pose,
                &crate_place(c),
                [crate_scale; 3],
                CRATES,
                &mut |_| {},
            );
            stack.draws.extend(o.draws);
        }
        let (c, r) = torque_render_scene::hd::bounding_sphere(&stack.draws);
        (stack.centre, stack.radius) = (c, r);
        objects.push(stack);
        view.set_objects(objects);
        for (i, cue) in cues.iter().enumerate() {
            if !cued[i] && t >= cue.at {
                cued[i] = true;
                let serial = i as u32 + 1;
                let make = cue.make;
                runner.sync(cue.ghost, Some((serial, cue.look, 0)), |_| Some(make()));
            }
        }
        view.run_dissolves(&mut runner, if k == from { 0.0 } else { dt });
        let mut set = TextureSet::new(&assets);
        let mut sprites = SpriteFrame {
            steps: if k == from {
                Vec::new()
            } else {
                vec![(dt * 1000.0).round() as u32]
            },
            ..SpriteFrame::default()
        };
        runner.hand_over(&mut sprites, &mut set);
        view.set_particles_decals_and_sprites(&[], &[], Some(&sprites), set);
        view.advance_time(dt);
        let mut eye = look_at(eye_at(t), [-0.65, -0.15, 1.0]);
        eye.fov = Some(44);
        if k == from {
            view.reset_history();
        }
        let frame = view.render_world(&gpu, SIZE, eye)?;
        made += runner.spawned;
        if k % FPS as usize == 0 {
            eprintln!("{t:.1}s: {made} particles made so far");
        }
        if k < to && still.is_some() {
            continue;
        }
        let rgb: Vec<u8> = frame
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        if let Some(p) = pipe.as_mut() {
            p.write_all(&rgb)?;
        }
        let png = |name: &str| -> Result<(), Box<dyn std::error::Error>> {
            let bytes = torque_example_assets::textures::png(SIZE.0, SIZE.1, |x, y| {
                let d = 3 * (y * SIZE.0 + x) as usize;
                [rgb[d], rgb[d + 1], rgb[d + 2]]
            })?;
            let path = dir.join(name);
            std::fs::write(&path, bytes)?;
            println!("{}", path.display());
            Ok(())
        };
        match still {
            Some(_) => png("still.png")?,
            None if k == (SAND_AT * FPS as f32) as usize => png("dissolve-sand.png")?,
            None if k == (EMBERS_AT * FPS as f32) as usize => png("dissolve-embers.png")?,
            None => {}
        }
    }
    drop(pipe);
    if let Some(mut f) = ffmpeg {
        if !f.wait()?.success() {
            return Err("ffmpeg failed".into());
        }
        println!("{}", mp4.display());
    }
    Ok(())
}
