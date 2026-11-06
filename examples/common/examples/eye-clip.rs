//! A first-person clip of the kit's figure walking and running, drawn offscreen for review, before and after the steady eye:
//! on the left the view stands at the `eye` node of the body as it is drawn (TGE's eye, bobbing with the gait), on the right
//! at the `eye` node of the rest pose carried by the figure's motion (the engine's own `GhostOptions::steady_eye`). The
//! figure walks at the demos' walking pace, then runs, its action thread driven by the client's animation choice; posts stand
//! along the way so the view's motion shows.
//!
//! The frames go to `ffmpeg` (on the path) as `eye-clip.mp4`.
//!
//! `cargo run -p torque-example-common --release --example eye-clip -- <dir>` (default `eye-clip`)

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use torque::app::view::{Daylight, SceneShape, WorldView};
use torque::app::window::offscreen_gpu;
use torque::client::player_anim::{PlayerAnimation, PlayerMotion};
use torque::shapes::instance::ShapeInstance;
use torque::shapes::math::mul_p;
use torque_example_common::{KitAssets, kit_path};
use torque_render_scene::hd::kit::{box_object, look_at};

/// One view's size and the frame rate.
const VIEW: (u32, u32) = (640, 400);
const FPS: u32 = 30;
/// The demos' walking pace and the run's (units a second), and the seconds of each.
const WALK_PACE: f32 = 2.1;
const RUN_PACE: f32 = 6.0;
const WALK_FOR: f32 = 2.5;
const LENGTH: f32 = 5.5;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "eye-clip".to_string()),
    );
    std::fs::create_dir_all(&dir)?;
    let gpu = offscreen_gpu()?;
    let assets = KitAssets::new()?;
    let figure = SceneShape::load(&assets, &kit_path("figure.dts")).ok_or("figure.dts")?;
    let shape = figure.shape();
    let detail = figure.detail().ok_or("no detail")?;
    let eye_node = figure.node("eye").ok_or("no eye")?;
    let mut instance = figure.instance().ok_or("no instance")?;
    let mut anim = PlayerAnimation::new(shape, &mut instance).ok_or("no threads")?;
    // The rest pose: no thread playing.
    let mut rest = ShapeInstance::new(shape, &mut |_| true).map_err(|e| format!("{e:?}"))?;
    let _ = rest.animate(shape, detail, None, &mut |_| true);
    let rest_eye = mul_p(&rest.node_transforms()[eye_node], [0.0; 3]);
    let mut views = [WorldView::new(&gpu), WorldView::new(&gpu)];
    for view in &mut views {
        view.set_plain_floor(400.0, [150, 152, 156]);
        view.set_daylight(&Daylight {
            towards: [0.45, 0.6, 0.66],
            ..Daylight::default()
        });
    }
    let (w, h) = (2 * VIEW.0, VIEW.1);
    let mp4 = dir.join("eye-clip.mp4");
    let mut ffmpeg = Command::new("ffmpeg")
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
        .arg(format!("{w}x{h}"))
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
            "18",
        ])
        .arg(&mp4)
        .stdin(Stdio::piped())
        .spawn()?;
    let mut pipe = ffmpeg.stdin.take().ok_or("no ffmpeg input")?;
    let dt = 1.0 / FPS as f32;
    let mut y = 0.0f32;
    for k in 0..(LENGTH * FPS as f32) as usize {
        let t = k as f32 * dt;
        let speed = if t < WALK_FOR {
            WALK_PACE * (t / 0.4).min(1.0)
        } else {
            WALK_PACE + (RUN_PACE - WALK_PACE) * ((t - WALK_FOR) / 0.6).min(1.0)
        };
        y += speed * dt;
        let motion = PlayerMotion {
            velocity: [0.0, speed, 0.0],
            look: 0.5,
            ..PlayerMotion::default()
        };
        anim.update(shape, &mut instance, &motion, dt);
        let _ = instance.animate(shape, detail, None, &mut |_| true);
        let drawn = mul_p(&instance.node_transforms()[eye_node], [0.0; 3]);
        let mut rgb = vec![0u8; (w * h * 3) as usize];
        for (v, at) in [drawn, rest_eye].into_iter().enumerate() {
            let view = &mut views[v];
            // Posts either side of the way every two units, and tiles every half unit, around the figure.
            let mut objects = Vec::new();
            let near = (y / 0.5).round() as i32;
            for (n, i) in (near - 6..=near + 40).enumerate() {
                let along = i as f32 * 0.5;
                let mut tiles = vec![box_object(
                    10 + 4 * n as u32,
                    [0.0, along, 0.0],
                    [0.05, 0.05, 0.004],
                )];
                if i % 4 == 0 {
                    for (m, x) in [-1.2f32, 1.2].into_iter().enumerate() {
                        tiles.push(box_object(
                            11 + 4 * n as u32 + m as u32,
                            [x, along, 1.0],
                            [0.06, 0.06, 1.0],
                        ));
                    }
                }
                for mut o in tiles {
                    for vtx in o.draws.iter_mut().flat_map(|d| d.vertices.iter_mut()) {
                        vtx.color = [60, 62, 66, 255];
                    }
                    objects.push(o);
                }
            }
            view.set_objects(objects);
            let eye_at = [at[0], at[1] + y, at[2]];
            let mut eye = look_at(eye_at, [eye_at[0], eye_at[1] + 10.0, eye_at[2] - 0.6]);
            eye.fov = Some(75);
            if k == 0 {
                view.reset_history();
            }
            let frame = view.render_world(&gpu, VIEW, eye)?;
            for py in 0..VIEW.1 {
                for px in 0..VIEW.0 {
                    let s = 4 * (py * VIEW.0 + px) as usize;
                    let d = 3 * (py * w + v as u32 * VIEW.0 + px) as usize;
                    rgb[d..d + 3].copy_from_slice(&frame.rgba[s..s + 3]);
                }
            }
        }
        for py in 0..h {
            let d = 3 * (py * w + VIEW.0) as usize;
            rgb[d..d + 3].copy_from_slice(&[30, 30, 30]);
        }
        pipe.write_all(&rgb)?;
    }
    drop(pipe);
    if !ffmpeg.wait()?.success() {
        return Err("ffmpeg failed".into());
    }
    println!("{}", mp4.display());
    Ok(())
}
