//! The sky's clouds for review, drawn offscreen: one player hosting alone, the map's day held at a few times, the camera by the
//! buggy looking out over the hills, with the engine's clouds and with the demo's (see [`write_cloud_shots`]).

use std::io;
use std::path::{Path, PathBuf};

use torque::app::shots::{Seat, Shots, run};
use torque::app::window::offscreen_gpu;
use torque_render_core::Gpu;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::source::Eye;

use crate::client::{ClientOptions, NetMode, VehicleDemo};
use crate::files::GameFiles;
use crate::shots::{DAY_SETTLE_MS, GOLDEN_HOUR, NOON, SUNSET, TIMED_FRAMES, WAIT_LIMIT_MS};

/// The afternoon of the pictures looking up and at the clouds' shadows (0 sunrise, 0.25 noon, 0.5 sunset).
pub const AFTERNOON: f32 = 0.36;
pub const SHADOWS: f32 = 0.33;

/// The pictures: the time of day, the name, the eye over the ground at the start (x, y, height), its turn from the ground
/// direction toward the sun (radians, counter-clockwise seen from above) and its pitch (radians, up).
const PICTURES: [(f32, &str, [f32; 3], f32, f32); 5] = [
    (NOON, "clouds-noon", [-6.0, -9.0, 2.2], 1.9, 0.12),
    (GOLDEN_HOUR, "clouds-golden", [-6.0, -9.0, 2.2], 1.0, 0.1),
    (SUNSET, "clouds-sunset", [-6.0, -9.0, 2.2], 0.25, 0.08),
    (AFTERNOON, "clouds-up", [-6.0, -9.0, 2.2], 2.6, 0.75),
    (SHADOWS, "clouds-shadows", [-6.0, -9.0, 40.0], 4.71, -0.12),
];

/// An eye at `at` looking along the ground direction `dir` turned `turn` radians, raised `pitch` radians.
fn looking(at: [f32; 3], dir: [f32; 2], turn: f32, pitch: f32) -> Eye {
    let (sn, cs) = turn.sin_cos();
    let d = [dir[0] * cs - dir[1] * sn, dir[0] * sn + dir[1] * cs];
    let (ps, pc) = pitch.sin_cos();
    let target = [at[0] + d[0] * pc, at[1] + d[1] * pc, at[2] + ps];
    let mut e = look_at(at, target);
    e.fov = None;
    e
}

/// The ground direction toward the sun (from the south-east when it is overhead).
fn toward_sun(game: &VehicleDemo) -> [f32; 2] {
    let s = game
        .sun()
        .filter(|s| s[0].hypot(s[1]) > 0.1)
        .unwrap_or([0.6, -0.8, 0.0]);
    let n = s[0].hypot(s[1]);
    [s[0] / n, s[1] / n]
}

/// The median GPU time (ms) of [`TIMED_FRAMES`] frames from `at` (none without timestamps).
fn gpu_ms(gpu: &Gpu, seat: &mut Seat<VehicleDemo>, at: Eye) -> Result<Option<f64>, String> {
    seat.eye = Some(at);
    let mut ms = Vec::with_capacity(TIMED_FRAMES);
    for _ in 0..TIMED_FRAMES {
        seat.frame(gpu)?;
        ms.extend(seat.view.gpu_ms());
    }
    seat.eye = None;
    ms.sort_by(f64::total_cmp);
    Ok(ms.get(ms.len() / 2).copied())
}

/// The clouds at `size` in `dir`, in `engine/` with the engine's clouds and in `demo/` with the demo's
/// ([`CloudLook::SUNLIT_CUMULUS`](torque_render_scene::hd::clouds::CloudLook::SUNLIT_CUMULUS)): `clouds-noon.png` (midday,
/// looking out across the sun), `clouds-golden.png` (golden hour, the sun to one side), `clouds-sunset.png` (looking into the
/// sunset), `clouds-up.png` (an afternoon's clouds from the ground, looking up away from the sun) and `clouds-shadows.png` (their
/// shadows on the hills, from over them); `gpu-ms.txt` in each the median GPU frame time of each picture. Returns the paths.
pub fn write_cloud_shots(
    dir: &Path,
    size: (u32, u32),
    make_files: impl Fn() -> io::Result<GameFiles>,
) -> Result<Vec<PathBuf>, String> {
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let files = make_files().map_err(|e| e.to_string())?;
    let game = VehicleDemo::new(
        files,
        ClientOptions {
            net: NetMode::Loopback,
            name: "Ann".to_string(),
            port: 0,
            ..ClientOptions::default()
        },
    )?;
    let mut a = Seat::new(game, &gpu, size)?;
    a.game.host_game()?;
    let mut waited = 0;
    while !(a.game.in_world() && a.game.figure().is_some()) {
        if waited >= WAIT_LIMIT_MS {
            return Err("the hosted game did not come".to_string());
        }
        run(&gpu, &mut [&mut a], 100)?;
        waited += 100;
    }
    run(&gpu, &mut [&mut a], 2000)?;
    let ground = a.game.terrain.height(0.0, 0.0).unwrap_or(0.0);
    let demo_look = a.view.scene().frame.cloud_look;
    let mut paths = Vec::new();
    for (folder, look) in [("engine", None), ("demo", demo_look)] {
        a.view.set_cloud_look(look);
        let out = dir.join(folder);
        let mut shots = Shots::new(&out)?;
        let mut report = String::new();
        for (time, name, at, turn, pitch) in PICTURES {
            a.game
                .server
                .as_mut()
                .ok_or("no server")?
                .set_day(time, false)?;
            run(&gpu, &mut [&mut a], DAY_SETTLE_MS)?;
            let at = [at[0], at[1], ground + at[2]];
            let e = looking(at, toward_sun(&a.game), turn, pitch);
            a.eye = Some(e);
            let frame = a.picture(&gpu);
            a.eye = None;
            shots.save(&frame?, name)?;
            report += &format!("{name} gpu ms: {:?}\n", gpu_ms(&gpu, &mut a, e)?);
        }
        let times = out.join("gpu-ms.txt");
        std::fs::write(&times, report).map_err(|e| e.to_string())?;
        paths.extend(shots.paths);
        paths.push(times);
    }
    a.view.set_cloud_look(demo_look);
    Ok(paths)
}
