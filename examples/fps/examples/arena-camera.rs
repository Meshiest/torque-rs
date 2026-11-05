//! The camera's motion in the arena, frame by frame: `Tab` out to behind the figure and back standing, walking and mid-turn,
//! `Tab` pressed again mid-switch, and the camera behind the figure backing into the arena's wall while the view sways.
//!
//! `cargo run -p torque-example-fps --release --example arena-camera -- <dir> [fps...] [--render] [--case=<name>]` writes, for
//! each case and frame rate (60 and 144 when none is given), `<dir>/<case>-<fps>.csv` (a line a frame); with `--render`, the
//! 60 fps run of each case also draws every frame offscreen into `<dir>/<case>/NNNN.png`.

use std::path::{Path, PathBuf};

use torque::app::App as _;
use torque::app::shots::write_png;
use torque::app::view::WorldView;
use torque::app::window::offscreen_gpu;
use torque::client::input::Key;
use torque_example_common::camera_log::{CameraFrame, CameraLog, FrameClock};
use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;
use torque_render_core::{Gpu, Offscreen};
use torque_render_scene::source::Eye;

const SIZE: (u32, u32) = (640, 360);
/// The mouse's counts for a radian of turn (the scripts turn 0.003 a count).
const COUNTS_PER_RADIAN: f32 = 1.0 / 0.003;

type Script = fn(&mut ArenaGame, u32, u32) -> Option<&'static str>;

struct Case {
    name: &'static str,
    /// Where the figure is put first ("x y z yaw"), if anywhere but its spawn point.
    place: Option<[&'static str; 4]>,
    ms: u32,
    script: Script,
}

fn now(t: u32, dt: u32, at: u32) -> bool {
    t <= at && at < t + dt
}

fn tabs(game: &mut ArenaGame, t: u32, dt: u32, times: &[u32]) -> Option<&'static str> {
    if times.iter().any(|&at| now(t, dt, at)) {
        game.key(Key::Tab, true);
        game.key(Key::Tab, false);
        return Some(if game.third_person() {
            "tab-out"
        } else {
            "tab-in"
        });
    }
    None
}

fn turn(game: &mut ArenaGame, dt: u32, rate: f32) {
    game.host
        .mouse_motion(rate * dt as f32 / 1000.0 * COUNTS_PER_RADIAN, 0.0);
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "stand",
            place: None,
            ms: 2200,
            script: |g, t, dt| tabs(g, t, dt, &[400, 1300]),
        },
        Case {
            name: "walk",
            place: None,
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
            place: None,
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
            place: None,
            ms: 2600,
            script: |g, t, dt| tabs(g, t, dt, &[400, 500, 1400, 1560]),
        },
        Case {
            name: "wall",
            // Facing north 3.5 units from the south wall (its face at y -20), backing into it while the view sways: the camera
            // behind is pushed in by the wall and slides along it.
            place: Some(["-12", "-16.5", "0.1", "0"]),
            ms: 5000,
            script: |g, t, dt| {
                if t == 0 {
                    g.key(Key::Tab, true);
                    g.key(Key::Tab, false);
                }
                if t >= 600 {
                    g.key(Key::S, true);
                    let s = (t - 600) as f32 / 1000.0;
                    turn(g, dt, 1.2 * (s * std::f32::consts::PI * 0.8).sin());
                }
                None
            },
        },
    ]
}

struct Render<'a> {
    gpu: &'a Gpu,
    view: WorldView,
    target: Offscreen,
    dir: PathBuf,
}

fn frame(
    game: &mut ArenaGame,
    ms: u32,
    render: Option<&mut Render>,
    n: usize,
) -> Result<Eye, String> {
    game.advance(ms);
    match render {
        Some(r) => {
            let (gui, eye) = game.frame(&mut r.view);
            r.view.draw(r.gpu, &r.target.color, &gui, eye)?;
            let picture = r.gpu.read_back(&r.target).map_err(|e| e.to_string())?;
            write_png(&picture, &r.dir.join(format!("{n:04}.png"))).map_err(|e| e.to_string())?;
            Ok(eye)
        }
        None => {
            game.view_frame();
            Ok(game.drawn_eye())
        }
    }
}

fn run_case(case: &Case, fps: f32, gpu: Option<&Gpu>, out: &Path) -> Result<(), String> {
    let mut game = ArenaGame::new(
        GameFiles::bundled().map_err(|e| e.to_string())?,
        ClientOptions {
            net: NetMode::Loopback,
            name: "Ann".to_string(),
            port: 0,
            map: Some("missions/arena.mis".to_string()),
            ..ClientOptions::default()
        },
    )?;
    game.host().surface_size(SIZE);
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
    game.call("hostGame", &[])?;
    let mut waited = 0;
    while !(game.in_game() && game.own_look().is_some_and(|o| o.position.is_some())) {
        let ms = clock.next_ms();
        frame(&mut game, ms, None, 0)?;
        waited += ms;
        if waited > 30_000 {
            return Err("the arena did not start".into());
        }
    }
    if let Some(p) = case.place {
        let server = game.server.as_mut().ok_or("no server")?;
        server.call("placeFigure", &["Ann", p[0], p[1], p[2], p[3]])?;
    }
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
        let drawn = frame(&mut game, ms, render.as_mut(), n)?;
        log.push(CameraFrame {
            t: (t + ms) as f32 / 1000.0,
            dt: ms as f32 / 1000.0,
            drawn,
            target: Some(game.mode_eye()),
            figure: game.own_look().and_then(|o| o.position),
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
