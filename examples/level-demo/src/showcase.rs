//! Pictures of the world alone (no menu, HUD or hints), drawn offscreen at the renderer's highest quality: the valley's
//! views ([`write_valley`]) and the pictures the project's front page shows ([`valley_picture`], [`office_picture`]).

use std::path::{Path, PathBuf};

use torque::app::shots::{Seat, Shots};
use torque_render_core::{Frame, Gpu};
use torque_render_scene::hd::Quality;
use torque_render_scene::hd::kit::look_at;

use crate::client::{LevelDemo, with_fov};
use crate::files::GameFiles;
use crate::shots::{START_LIMIT_MS, map_name};

/// The office's corner in the map (the generator's numbers run from it).
const OFFICE_CORNER: [f32; 2] = [-18.0, -12.0];

/// The valley's views: a name, the eye (x, y and its height over the ground) and what it looks at (x, y, z).
pub const VALLEY_VIEWS: [(&str, [f32; 3], [f32; 3]); 6] = [
    ("valley-lake", [400.0, -15.0, 120.0], [-260.0, 0.0, 30.0]),
    ("valley-high", [520.0, -260.0, 160.0], [-420.0, 0.0, 60.0]),
    ("valley-close", [-120.0, -175.0, 9.0], [-170.0, -70.0, 26.0]),
    ("valley-far", [900.0, 120.0, 260.0], [-700.0, -40.0, 80.0]),
    ("valley-start", [200.0, -34.0, 3.5], [-100.0, -10.0, 34.0]),
    ("valley-sea", [-650.0, 20.0, 6.0], [-2400.0, 0.0, 170.0]),
];

/// The eye of a view of [`VALLEY_VIEWS`]: its height over the valley's ground (or over the water, where the ground is under
/// it) made a height.
pub fn valley_eye(at: [f32; 3]) -> [f32; 3] {
    use torque_example_assets::valley::{WATER_LEVEL, height};
    [at[0], at[1], height(at[0], at[1]).max(WATER_LEVEL) + at[2]]
}

/// The game on `files` drawn at `size` with the highest quality, the map `name` started and its figure there.
pub fn seat_on(
    gpu: &Gpu,
    files: GameFiles,
    size: (u32, u32),
    name: &str,
) -> Result<Seat<LevelDemo>, String> {
    let mut seat = Seat::new(LevelDemo::new(files)?, gpu, size)?;
    seat.view.set_quality(Quality::Ultra);
    seat.run(gpu, 200)?;
    let index = seat
        .game
        .maps
        .iter()
        .position(|m| map_name(&m.path) == name)
        .ok_or_else(|| format!("no {name} map"))?;
    seat.game.start_map(index)?;
    let mut waited = 0;
    while !(seat.game.in_world() && seat.game.figure().is_some()) && waited < START_LIMIT_MS {
        seat.run(gpu, 100)?;
        waited += 100;
    }
    if !seat.game.in_world() {
        return Err(format!("{name} did not start"));
    }
    // Every ghost there and the map's name gone from the middle of the view.
    seat.run(gpu, 3000)?;
    Ok(seat)
}

/// A picture of the world from `from` looking at `to`.
pub fn view(
    gpu: &Gpu,
    seat: &mut Seat<LevelDemo>,
    from: [f32; 3],
    to: [f32; 3],
) -> Result<Frame, String> {
    seat.eye = Some(with_fov(look_at(from, to)));
    let frame = seat.world_picture(gpu);
    seat.eye = None;
    frame
}

/// The frames a median GPU time is taken over.
pub const TIMED_FRAMES: usize = 60;

/// The median GPU time (ms) of [`TIMED_FRAMES`] frames of the world from `from` looking at `to` at `quality` (none without
/// timestamps).
pub fn gpu_ms(
    gpu: &Gpu,
    seat: &mut Seat<LevelDemo>,
    from: [f32; 3],
    to: [f32; 3],
    quality: Quality,
) -> Result<Option<f64>, String> {
    seat.view.set_quality(quality);
    seat.eye = Some(with_fov(look_at(from, to)));
    let mut ms = Vec::with_capacity(TIMED_FRAMES);
    for _ in 0..TIMED_FRAMES {
        seat.frame(gpu)?;
        ms.extend(seat.view.gpu_ms());
    }
    seat.eye = None;
    ms.sort_by(f64::total_cmp);
    Ok(ms.get(ms.len() / 2).copied())
}

/// The valley's views ([`VALLEY_VIEWS`]) at `size`, written into `dir`, and `gpu-ms.txt`: each view's median GPU frame time
/// at the High quality (a window's) and at Ultra. Returns the paths.
pub fn write_valley(
    gpu: &Gpu,
    files: GameFiles,
    dir: &Path,
    size: (u32, u32),
) -> Result<Vec<PathBuf>, String> {
    let mut seat = seat_on(gpu, files, size, "valley")?;
    let mut shots = Shots::new(dir)?;
    let mut report = String::new();
    for (name, from, to) in VALLEY_VIEWS {
        let from = valley_eye(from);
        shots.save(&view(gpu, &mut seat, from, to)?, name)?;
        let high = gpu_ms(gpu, &mut seat, from, to, Quality::High)?;
        let ultra = gpu_ms(gpu, &mut seat, from, to, Quality::Ultra)?;
        report += &format!("{name} gpu ms: high {high:?}, ultra {ultra:?}\n");
    }
    let times = dir.join("gpu-ms.txt");
    std::fs::write(&times, report).map_err(|e| e.to_string())?;
    shots.paths.push(times);
    Ok(shots.paths)
}

/// The valley clip's view: from over the eastern shore (its height over the ground) up the valley into the sky.
pub const CLIP_VIEW: ([f32; 3], [f32; 3]) = ([300.0, -15.0, 45.0], [-500.0, 0.0, 280.0]);

/// The valley's sky moving: `seconds` of the clock from [`CLIP_VIEW`], the world alone drawn every frame and every `every`th
/// frame written into `dir` as `0000.png` on (for a clip at `1000 / (FRAME_MS * every)` frames a second), then the last frame
/// as `last.png` and again held still and settled as `held.png` (the moving clouds' sharpness against the still ones'). Returns
/// the paths.
pub fn write_valley_clip(
    gpu: &Gpu,
    files: GameFiles,
    dir: &Path,
    size: (u32, u32),
    seconds: f32,
    every: u32,
) -> Result<Vec<PathBuf>, String> {
    use torque::app::App as _;
    use torque::app::shots::{FRAME_MS, write_png};
    let mut seat = seat_on(gpu, files, size, "valley")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (from, to) = CLIP_VIEW;
    let eye = with_fov(look_at(valley_eye(from), to));
    let rect = [0, 0, seat.target.width, seat.target.height];
    let frames = (seconds * 1000.0 / FRAME_MS as f32) as u32;
    let mut paths = Vec::new();
    for k in 0..frames {
        seat.game.advance(FRAME_MS);
        let _ = seat.game.frame(&mut seat.view);
        seat.view.draw_world(&seat.target.color, rect, eye)?;
        if k % every.max(1) == 0 {
            let frame = gpu.read_back(&seat.target).map_err(|e| e.to_string())?;
            let path = dir.join(format!("{:04}.png", paths.len()));
            write_png(&frame, &path).map_err(|e| e.to_string())?;
            paths.push(path);
        }
    }
    // The last frame again, the clock held and the renderer settled: the clouds without their motion (their sharpness to
    // compare the moving frames' with).
    let frame = gpu.read_back(&seat.target).map_err(|e| e.to_string())?;
    let last = dir.join("last.png");
    write_png(&frame, &last).map_err(|e| e.to_string())?;
    seat.eye = Some(eye);
    let held = seat.world_picture(gpu)?;
    seat.eye = None;
    let path = dir.join("held.png");
    write_png(&held, &path).map_err(|e| e.to_string())?;
    paths.extend([last, path]);
    Ok(paths)
}

/// How much more of the clock runs before the front page's valley is pictured (ms): the moment its clouds' reflections read
/// best.
pub const SEA_PICTURE_MS: u32 = 6_000;

/// The front page's valley: low over the sea, the clouds racing over it reflected in the water, the sun's glint running to the
/// horizon between the far ranges.
pub fn valley_picture(gpu: &Gpu, files: GameFiles, size: (u32, u32)) -> Result<Frame, String> {
    let mut seat = seat_on(gpu, files, size, "valley")?;
    seat.run(gpu, SEA_PICTURE_MS)?;
    let (_, from, to) = VALLEY_VIEWS[5];
    view(gpu, &mut seat, valley_eye(from), to)
}

/// The front page's office: the open-plan desks under their lights, the glass of the meeting room beyond.
pub fn office_picture(gpu: &Gpu, files: GameFiles, size: (u32, u32)) -> Result<Frame, String> {
    let mut seat = seat_on(gpu, files, size, "office")?;
    let at = |x: f32, y: f32, z: f32| [x + OFFICE_CORNER[0], y + OFFICE_CORNER[1], z];
    view(gpu, &mut seat, at(12.8, 6.4, 1.7), at(30.0, 4.0, 0.8))
}
