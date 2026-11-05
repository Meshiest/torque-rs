//! A clip of the kit's figure walking and running, drawn offscreen for review: it walks at a held-Shift pace, speeds up to a
//! run, runs, slows back to a walk, then walks backward, its action thread driven by the client's animation choice
//! ([`torque::client::player_anim`]) as a ghost's would be. Each frame shows a side view and a three-quarter view side by side
//! over a floor marked every half unit (so a sliding foot shows), with a bar under them: its length the speed, green while
//! the figure walks and orange while it runs.
//!
//! The frames go to `ffmpeg` (on the path) as `walk-clip.mp4`; a strip of a few frames goes beside it as `walk-clip.png`.
//!
//! `cargo run -p torque-example-common --release --example walk-clip -- <dir>` (default `walk-clip`)

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use torque::app::view::{Daylight, SceneShape, WorldView};
use torque::app::window::offscreen_gpu;
use torque::client::player_anim::{PlayerAnimation, PlayerMotion, WALK};
use torque_example_common::{KitAssets, kit_path};
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::math::Mat;

/// One view's size, the frame rate, and the frames the strip takes (seconds into the clip).
const VIEW: (u32, u32) = (640, 480);
const FPS: u32 = 30;
const BAR: u32 = 24;
const STRIP_AT: [f32; 6] = [1.0, 1.15, 3.0, 3.4, 5.0, 8.6];
/// A held-Shift pace (0.35 of a 6 units a second run, the demos' `walkSlowly`) and the run's.
const WALK_PACE: f32 = 2.1;
const RUN_PACE: f32 = 6.0;

/// The speed along the figure's facing (negative: backward) `t` seconds in.
fn speed_at(t: f32) -> f32 {
    let ramp = |t: f32, t0: f32, t1: f32, a: f32, b: f32| {
        a + (b - a) * ((t - t0) / (t1 - t0)).clamp(0.0, 1.0)
    };
    match t {
        t if t < 0.4 => ramp(t, 0.0, 0.4, 0.0, WALK_PACE),
        t if t < 2.6 => WALK_PACE,
        t if t < 3.6 => ramp(t, 2.6, 3.6, WALK_PACE, RUN_PACE),
        t if t < 5.4 => RUN_PACE,
        t if t < 6.4 => ramp(t, 5.4, 6.4, RUN_PACE, WALK_PACE),
        t if t < 7.6 => WALK_PACE,
        t if t < 8.0 => ramp(t, 7.6, 8.0, WALK_PACE, 0.0),
        t if t < 8.4 => ramp(t, 8.0, 8.4, 0.0, -1.4),
        t if t < 9.6 => -1.4,
        _ => ramp(t, 9.6, 9.9, -1.4, 0.0),
    }
}
const LENGTH: f32 = 10.2;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "walk-clip".to_string()),
    );
    std::fs::create_dir_all(&dir)?;
    let gpu = offscreen_gpu()?;
    let assets = KitAssets::new()?;
    let figure = SceneShape::load(&assets, &kit_path("figure.dts")).ok_or("figure.dts")?;
    let shape = figure.shape();
    let detail = figure.detail().ok_or("no detail")?;
    let mut instance = figure.instance().ok_or("no instance")?;
    let mut anim = PlayerAnimation::new(shape, &mut instance).ok_or("no threads")?;
    let walk = anim
        .actions()
        .iter()
        .position(|a| a.name == WALK)
        .ok_or("the figure has no walk")?;
    let mut views = [WorldView::new(&gpu), WorldView::new(&gpu)];
    for view in &mut views {
        view.set_plain_floor(400.0, [150, 152, 156]);
        view.set_daylight(&Daylight {
            towards: [0.45, 0.6, 0.66],
            ..Daylight::default()
        });
    }
    let (w, h) = (2 * VIEW.0, VIEW.1 + BAR);
    let mp4 = dir.join("walk-clip.mp4");
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
    let frames = (LENGTH * FPS as f32) as usize;
    let (mut y, mut strip) = (0.0f32, Vec::new());
    for k in 0..frames {
        let t = k as f32 * dt;
        let speed = speed_at(t);
        y += speed * dt;
        let motion = PlayerMotion {
            velocity: [0.0, speed, 0.0],
            // The head level (0 looks fully up, 1 fully down).
            look: 0.5,
            ..PlayerMotion::default()
        };
        anim.update(shape, &mut instance, &motion, dt);
        let _ = instance.animate(shape, detail, None, &mut |_| true);
        let mut place = Mat::IDENTITY;
        place.set_column3([0.0, y, 0.0]);
        let mut rgb = vec![0u8; (w * h * 3) as usize];
        // The side view from the figure's right, and the three-quarter view from its front right, both following it.
        for (v, (side, rise, ahead)) in [(1.0f32, 0.12f32, 0.0f32), (0.7, 0.3, 0.7)]
            .into_iter()
            .enumerate()
        {
            let view = &mut views[v];
            let mut objects = vec![view.shape_object(&assets, &figure, &instance, &place, 1)];
            // Dark tiles every half unit along the path and a row to the side.
            let near = (y / 0.5).round() as i32;
            for (n, i) in (near - 14..=near + 14).enumerate() {
                for (m, x) in [-0.45f32, 0.45].into_iter().enumerate() {
                    let mut o = box_object(
                        10 + 2 * n as u32 + m as u32,
                        [x, i as f32 * 0.5, 0.0],
                        [0.05, 0.05, 0.004],
                    );
                    for vtx in o.draws.iter_mut().flat_map(|d| d.vertices.iter_mut()) {
                        vtx.color = [60, 62, 66, 255];
                    }
                    objects.push(o);
                }
            }
            view.set_objects(objects);
            let center = [0.0, y, 1.0];
            let distance = 4.2;
            let flat = (1.0 - rise * rise).sqrt();
            let along = (1.0 - side * side).sqrt() * ahead.signum();
            let eye_at = [
                center[0] + distance * side * flat,
                center[1] + distance * along * flat,
                center[2] + distance * rise,
            ];
            let mut eye = look_at(eye_at, center);
            eye.fov = Some(45);
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
        // The bar: the speed out of the run's, green walking, orange running, gray otherwise; a mark at the walk's pace.
        let color = match anim.action().0 {
            a if a == walk => [70, 190, 90],
            1 => [235, 140, 40],
            _ => [140, 140, 140],
        };
        let filled = (speed.abs() / RUN_PACE * w as f32) as u32;
        let pace = (WALK_PACE / RUN_PACE * w as f32) as u32;
        for py in VIEW.1..h {
            for px in 0..w {
                let d = 3 * (py * w + px) as usize;
                let c = if px == pace || px == VIEW.0 && py < VIEW.1 + 4 {
                    [255, 255, 255]
                } else if px < filled {
                    color
                } else {
                    [30, 30, 30]
                };
                rgb[d..d + 3].copy_from_slice(&c);
            }
        }
        for py in 0..VIEW.1 {
            let d = 3 * (py * w + VIEW.0) as usize;
            rgb[d..d + 3].copy_from_slice(&[30, 30, 30]);
        }
        pipe.write_all(&rgb)?;
        if STRIP_AT
            .iter()
            .any(|&s| (s * FPS as f32).round() as usize == k)
        {
            strip.push(rgb);
        }
    }
    drop(pipe);
    if !ffmpeg.wait()?.success() {
        return Err("ffmpeg failed".into());
    }
    println!("{}", mp4.display());
    // The strip: the chosen frames stacked.
    let png = dir.join("walk-clip.png");
    let rows = strip.len() as u32;
    let bytes = torque_example_assets::textures::png(w, h * rows, |x, y| {
        let f = &strip[(y / h) as usize];
        let d = 3 * ((y % h) * w + x) as usize;
        [f[d], f[d + 1], f[d + 2]]
    })?;
    std::fs::write(&png, bytes)?;
    println!("{}", png.display());
    Ok(())
}
