//! The camera's motion in the level demo, frame by frame: `Tab` in and out standing, walking, running and mid-turn, `Tab`
//! pressed again mid-switch, and the camera behind the figure walking past the office's walls and glass.
//!
//! `cargo run -p torque-example-level-demo --release --example level-camera -- <dir> [fps...] [--render]` writes, for each case
//! and frame rate (60 and 144 when none is given), `<dir>/<case>-<fps>.csv` (a line a frame, [`CameraLog::csv`]); with
//! `--render`, the 60 fps run of each case also draws every frame offscreen into `<dir>/<case>/NNNN.png`.

use std::path::{Path, PathBuf};

use torque::app::App as _;
use torque::app::shots::write_png;
use torque::app::view::WorldView;
use torque::app::window::offscreen_gpu;
use torque::client::input::Key;
use torque_example_common::camera_log::{CameraFrame, CameraLog, FrameClock};
use torque_example_level_demo::client::LevelDemo;
use torque_example_level_demo::files::GameFiles;
use torque_render_core::{Gpu, Offscreen};

const SIZE: (u32, u32) = (640, 360);
/// The office's corner in the map (the generator's numbers run from it).
const OFFICE_CORNER: [f32; 2] = [-18.0, -12.0];
/// The mouse's counts for a radian of turn (the scripts turn 0.003 a count).
const COUNTS_PER_RADIAN: f32 = 1.0 / 0.003;

/// What a case does at its time `t` (ms since it started) over a frame of `dt` ms: keys and mouse, and a mark for the log.
type Script = fn(&mut LevelDemo, u32, u32) -> Option<&'static str>;

struct Case {
    name: &'static str,
    /// The map (its script's name) and, in the office, the spawn point (the office's numbers) and yaw.
    map: &'static str,
    spawn: Option<([f32; 2], f32)>,
    ms: u32,
    script: Script,
}

/// Whether time `at` falls in the frame from `t` over `dt`.
fn now(t: u32, dt: u32, at: u32) -> bool {
    t <= at && at < t + dt
}

fn tap(game: &mut LevelDemo, key: Key) {
    game.key(key, true);
    game.key(key, false);
}

/// `Tab` at each of `times`.
fn tabs(game: &mut LevelDemo, t: u32, dt: u32, times: &[u32]) -> Option<&'static str> {
    if times.iter().any(|&at| now(t, dt, at)) {
        tap(game, Key::Tab);
        return Some(if game.first_person() {
            "tab-in"
        } else {
            "tab-out"
        });
    }
    None
}

/// The mouse turning the view `rate` radians a second over the frame.
fn turn(game: &mut LevelDemo, dt: u32, rate: f32) {
    game.host
        .mouse_motion(rate * dt as f32 / 1000.0 * COUNTS_PER_RADIAN, 0.0);
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "stand",
            map: "plaza",
            spawn: None,
            ms: 2200,
            script: |g, t, dt| tabs(g, t, dt, &[400, 1300]),
        },
        Case {
            name: "walk",
            map: "plaza",
            spawn: None,
            ms: 2400,
            script: |g, t, dt| {
                if t == 0 {
                    g.key(Key::ShiftLeft, true);
                    g.key(Key::W, true);
                }
                tabs(g, t, dt, &[700, 1600])
            },
        },
        Case {
            name: "run",
            map: "plaza",
            spawn: None,
            ms: 2400,
            script: |g, t, dt| {
                if t == 0 {
                    g.key(Key::W, true);
                }
                tabs(g, t, dt, &[700, 1600])
            },
        },
        Case {
            name: "turn",
            map: "plaza",
            spawn: None,
            ms: 2400,
            script: |g, t, dt| {
                if t == 0 {
                    g.key(Key::W, true);
                }
                if (300..2100).contains(&t) {
                    turn(g, dt, 1.5);
                }
                tabs(g, t, dt, &[700, 1600])
            },
        },
        Case {
            name: "repress",
            map: "plaza",
            spawn: None,
            ms: 2600,
            script: |g, t, dt| tabs(g, t, dt, &[400, 500, 1400, 1560]),
        },
        Case {
            name: "office-corridor",
            map: "office",
            spawn: Some(([12.6, 13.9], std::f32::consts::FRAC_PI_2)),
            ms: 5000,
            script: |g, t, dt| {
                if t == 0 {
                    g.key(Key::ShiftLeft, true);
                    g.key(Key::W, true);
                }
                // Looking left and right as it walks: the line of sight sweeps over the meeting room's glass.
                let s = t as f32 / 1000.0;
                turn(g, dt, 0.9 * (s * std::f32::consts::PI).cos());
                None
            },
        },
        Case {
            name: "office-glass",
            // In the corridor facing south, the meeting room's glass wall behind: the view turns slowly one way and back, so the
            // camera behind sweeps through the glass, its posts and the corridor's walls.
            map: "office",
            spawn: Some(([12.0, 13.6], std::f32::consts::PI)),
            ms: 5000,
            script: |g, t, dt| {
                let s = t as f32 / 1000.0;
                turn(g, dt, 0.8 * (s * std::f32::consts::PI * 0.4).cos());
                None
            },
        },
        Case {
            name: "office-aisle",
            map: "office",
            spawn: Some(([12.6, 6.5], std::f32::consts::FRAC_PI_2)),
            ms: 5000,
            script: |g, t, dt| {
                if t == 0 {
                    g.key(Key::ShiftLeft, true);
                    g.key(Key::W, true);
                }
                let s = t as f32 / 1000.0;
                turn(g, dt, 1.2 * (s * std::f32::consts::PI * 0.8).sin());
                None
            },
        },
    ]
}

/// The game's files with the office's spawn point moved (the office's numbers) when `spawn` says.
fn files_for(spawn: Option<([f32; 2], f32)>) -> Result<GameFiles, String> {
    let mut files = GameFiles::bundled().map_err(|e| e.to_string())?;
    if let Some((at, yaw)) = spawn {
        let path = "levels/office.mis";
        let text =
            String::from_utf8_lossy(files.file(path).ok_or("no office template")?).into_owned();
        let spawn = format!(
            "spawn = \"{} {} 0.2\"; spawnYaw = {yaw};",
            at[0] + OFFICE_CORNER[0],
            at[1] + OFFICE_CORNER[1]
        );
        let changed = text.replace("spawn = \"-12.5 -15 0.2\"; spawnYaw = 0;", &spawn);
        if changed == text {
            return Err("the office's spawn line changed".into());
        }
        files.insert(path, changed.as_bytes());
    }
    Ok(files)
}

/// What the frames draw into, when they are drawn.
struct Render<'a> {
    gpu: &'a Gpu,
    view: WorldView,
    target: Offscreen,
    dir: PathBuf,
}

/// One frame: the clock's `ms`, then the frame's motion and the eye it draws (drawn into `render` when there is one).
fn frame(
    game: &mut LevelDemo,
    ms: u32,
    render: Option<&mut Render>,
    n: usize,
) -> Result<(f32, torque_render_scene::source::Eye), String> {
    game.advance(ms);
    let eye = match render {
        Some(r) => {
            let (gui, eye) = game.frame(&mut r.view);
            r.view.draw(r.gpu, &r.target.color, &gui, eye)?;
            let picture = r.gpu.read_back(&r.target).map_err(|e| e.to_string())?;
            write_png(&picture, &r.dir.join(format!("{n:04}.png"))).map_err(|e| e.to_string())?;
            eye
        }
        None => {
            let dt = game.take_frame_time();
            game.update_scene(dt);
            game.drawn_eye()
        }
    };
    Ok((ms as f32 / 1000.0, eye))
}

fn run_case(case: &Case, fps: f32, gpu: Option<&Gpu>, out: &Path) -> Result<(), String> {
    let files = files_for(case.spawn)?;
    let mut game = LevelDemo::new(files)?;
    game.host().surface_size(SIZE);
    let index = game
        .maps
        .iter()
        .position(|m| m.path.ends_with(&format!("{}.mis", case.map)))
        .ok_or("no such map")?;
    let mut render = match gpu {
        Some(gpu) => {
            let dir = out.join(case.name);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            Some(Render {
                gpu,
                view: game.view(gpu),
                target: gpu.offscreen(SIZE.0, SIZE.1).map_err(|e| e.to_string())?,
                dir,
            })
        }
        None => None,
    };
    let mut clock = FrameClock::new(fps);
    game.advance(200);
    game.start_map(index)?;
    let mut waited = 0;
    while !(game.in_world() && game.figure().is_some()) {
        let ms = clock.next_ms();
        frame(&mut game, ms, None, 0)?;
        waited += ms;
        if waited > 30_000 {
            return Err(format!("{} did not start", case.map));
        }
    }
    // Settled: the ghosts in, the map's name gone (drawn, so the renderer's history is warm when the clip starts).
    let settle = clock.now_ms() + 2500;
    while clock.now_ms() < settle {
        let ms = clock.next_ms();
        frame(&mut game, ms, render.as_mut(), 0)?;
    }
    let mut log = CameraLog::default();
    let start = clock.now_ms();
    let mut n = 0;
    while clock.now_ms() - start < u64::from(case.ms) {
        let t = (clock.now_ms() - start) as u32;
        let ms = clock.next_ms();
        let mark = (case.script)(&mut game, t, ms).unwrap_or("");
        let (dt, drawn) = frame(&mut game, ms, render.as_mut(), n)?;
        log.push(CameraFrame {
            t: (t + ms) as f32 / 1000.0,
            dt,
            drawn,
            target: Some(game.eye()),
            figure: game.figure().map(|f| f.position),
            mark: mark.to_string(),
        });
        n += 1;
    }
    log.write_csv(&out.join(format!("{}-{}.csv", case.name, fps as u32)))
        .map_err(|e| e.to_string())
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().ok_or("give the output folder")?);
    let rest: Vec<String> = args.collect();
    let draw = rest.iter().any(|a| a == "--render");
    let only: Vec<&str> = rest
        .iter()
        .filter_map(|a| a.strip_prefix("--case="))
        .collect();
    let mut rates: Vec<f32> = rest.iter().filter_map(|a| a.parse().ok()).collect();
    if rates.is_empty() {
        rates = vec![60.0, 144.0];
    }
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let gpu = if draw {
        Some(offscreen_gpu().map_err(|e| e.to_string())?)
    } else {
        None
    };
    for case in cases() {
        if !only.is_empty() && !only.contains(&case.name) {
            continue;
        }
        for &fps in &rates {
            let render = if fps == 60.0 { gpu.as_ref() } else { None };
            run_case(&case, fps, render, &out)?;
            println!("{} at {fps} fps", case.name);
        }
    }
    Ok(())
}
