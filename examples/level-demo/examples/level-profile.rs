//! Where a frame's time goes in the level demo, offscreen: the office (the front page's view) and the valley's views held
//! still, each drawn at High and at Ultra for `--frames` frames (60 by default) after the renderer settled, then the GPU time
//! between the frame's pass marks as medians over the frames, the CPU steps' means, and the passes and draw calls by kind
//! (`torque_render_hd::profile`).
//!
//! `cargo run -p torque-example-level-demo --release --example level-profile -- [--frames N] [--scene office|valley]
//! [--quality high|ultra] [--view <valley view>] [--size WxH]`. The machine's load moves the means; the medians stand.

use torque::app::shots::Seat;
use torque::app::window::offscreen_gpu;
use torque_example_level_demo::client::{LevelDemo, with_fov};
use torque_example_level_demo::files::GameFiles;
use torque_example_level_demo::showcase::{VALLEY_VIEWS, seat_on, valley_eye};
use torque_render_core::Gpu;
use torque_render_hd::profile::Profile;
use torque_render_scene::hd::Quality;
use torque_render_scene::hd::kit::look_at;

/// The pictures' size unless `--size` says otherwise.
const SIZE: (u32, u32) = (1280, 720);
/// The office's corner in the map (the generator's numbers run from it).
const OFFICE_CORNER: [f32; 2] = [-18.0, -12.0];
/// The frames drawn before the measured ones (the histories and the pipelines settled).
const SETTLE: usize = 30;

struct Args {
    frames: usize,
    scene: Option<String>,
    quality: Option<Quality>,
    view: Option<String>,
    size: (u32, u32),
}

fn args() -> Result<Args, String> {
    let mut a = Args {
        frames: 60,
        scene: None,
        quality: None,
        view: None,
        size: SIZE,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--frames" => a.frames = value()?.parse().map_err(|e| format!("{e}"))?,
            "--scene" => a.scene = Some(value()?),
            "--view" => a.view = Some(value()?),
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').ok_or("--size needs WxH")?;
                a.size = (
                    w.parse().map_err(|e| format!("{e}"))?,
                    h.parse().map_err(|e| format!("{e}"))?,
                );
            }
            "--quality" => {
                let v = value()?;
                a.quality = Some(Quality::named(&v).ok_or_else(|| format!("no quality {v}"))?);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

/// The view from `from` to `to` held for `frames` frames at `quality`, profiled.
fn profile(
    gpu: &Gpu,
    seat: &mut Seat<LevelDemo>,
    from: [f32; 3],
    to: [f32; 3],
    quality: Quality,
    frames: usize,
) -> Result<Profile, String> {
    seat.view.set_quality(quality);
    seat.eye = Some(with_fov(look_at(from, to)));
    seat.view.reset_history();
    for _ in 0..SETTLE {
        seat.frame(gpu)?;
    }
    let mut p = Profile::begin(seat.view.renderer());
    for _ in 0..frames {
        seat.frame(gpu)?;
        p.frame(seat.view.renderer());
    }
    seat.view.renderer().profile_passes = false;
    seat.eye = None;
    Ok(p)
}

fn main() -> Result<(), String> {
    let args = args()?;
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let qualities: Vec<Quality> = match args.quality {
        Some(q) => vec![q],
        None => vec![Quality::High, Quality::Ultra],
    };
    let wants = |scene: &str| args.scene.as_deref().is_none_or(|s| s == scene);
    if wants("office") {
        let files = GameFiles::bundled().map_err(|e| e.to_string())?;
        let mut seat = seat_on(&gpu, files, args.size, "office")?;
        let at = |x: f32, y: f32, z: f32| [x + OFFICE_CORNER[0], y + OFFICE_CORNER[1], z];
        for &q in &qualities {
            let p = profile(
                &gpu,
                &mut seat,
                at(12.8, 6.4, 1.7),
                at(30.0, 4.0, 0.8),
                q,
                args.frames,
            )?;
            println!("office {} {}x{}", q.name(), args.size.0, args.size.1);
            print!("{}", p.report(0.05));
        }
    }
    if wants("valley") {
        let files = GameFiles::bundled().map_err(|e| e.to_string())?;
        let mut seat = seat_on(&gpu, files, args.size, "valley")?;
        for (name, from, to) in VALLEY_VIEWS {
            if args.view.as_deref().is_some_and(|v| v != name) {
                continue;
            }
            let from = valley_eye(from);
            for &q in &qualities {
                let p = profile(&gpu, &mut seat, from, to, q, args.frames)?;
                println!("{name} {} {}x{}", q.name(), args.size.0, args.size.1);
                print!("{}", p.report(0.05));
            }
        }
    }
    Ok(())
}
