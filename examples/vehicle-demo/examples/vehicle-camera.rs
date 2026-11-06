//! The camera's motion in the vehicle demo, frame by frame: `Tab` between the camera behind the buggy and the driver's eyes
//! while it drives (straight, and turning), `Tab` pressed again mid-switch, and `Tab` on foot.
//!
//! `cargo run -p torque-example-vehicle-demo --release --example vehicle-camera -- <dir> [fps...] [--render] [--case=<name>]`
//! writes, for each case and frame rate (60 and 144 when none is given), `<dir>/<case>-<fps>.csv` (a line a frame); with
//! `--render`, the 60 fps run of each case also draws every frame offscreen into `<dir>/<case>/NNNN.png`.

use std::path::{Path, PathBuf};

use torque::app::App as _;
use torque::app::shots::write_png;
use torque::app::view::WorldView;
use torque::app::window::offscreen_gpu;
use torque::client::input::{Key, MouseButton};
use torque_example_common::camera_log::{CameraFrame, CameraLog, FrameClock};
use torque_example_vehicle_demo::client::{ClientOptions, NetMode, VehicleDemo};
use torque_example_vehicle_demo::files::GameFiles;
use torque_render_core::{Gpu, Offscreen};
use torque_render_scene::source::Eye;

const SIZE: (u32, u32) = (640, 360);

type Script = fn(&mut VehicleDemo, u32, u32) -> Option<&'static str>;

struct Case {
    name: &'static str,
    /// Whether the figure gets in the buggy first.
    seated: bool,
    ms: u32,
    script: Script,
}

fn now(t: u32, dt: u32, at: u32) -> bool {
    t <= at && at < t + dt
}

fn tabs(game: &mut VehicleDemo, t: u32, dt: u32, times: &[u32]) -> Option<&'static str> {
    if times.iter().any(|&at| now(t, dt, at)) {
        game.key(Key::Tab, true);
        game.key(Key::Tab, false);
        return Some(if game.first_person() {
            "tab-in"
        } else {
            "tab-out"
        });
    }
    None
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "drive",
            seated: true,
            ms: 4200,
            script: |g, t, dt| {
                if t == 0 {
                    g.key(Key::W, true);
                }
                tabs(g, t, dt, &[1500, 2700])
            },
        },
        Case {
            name: "drive-turn",
            seated: true,
            ms: 4200,
            script: |g, t, dt| {
                if t == 0 {
                    g.key(Key::W, true);
                }
                if now(t, dt, 1000) {
                    g.key(Key::A, true);
                }
                tabs(g, t, dt, &[1500, 2700])
            },
        },
        Case {
            name: "drive-repress",
            seated: true,
            ms: 4200,
            script: |g, t, dt| {
                if t == 0 {
                    g.key(Key::W, true);
                }
                tabs(g, t, dt, &[1500, 1620, 2700, 2800])
            },
        },
        Case {
            name: "foot",
            seated: false,
            ms: 2200,
            script: |g, t, dt| tabs(g, t, dt, &[400, 1300]),
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
    game: &mut VehicleDemo,
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
            let dt = game.take_frame_time();
            game.update_scene(dt);
            Ok(game.drawn_eye(dt))
        }
    }
}

fn run_for(
    game: &mut VehicleDemo,
    clock: &mut FrameClock,
    render: &mut Option<Render>,
    ms: u32,
) -> Result<(), String> {
    let until = clock.now_ms() + u64::from(ms);
    while clock.now_ms() < until {
        let step = clock.next_ms();
        frame(game, step, render.as_mut(), 0)?;
    }
    Ok(())
}

fn run_case(case: &Case, fps: f32, gpu: Option<&Gpu>, out: &Path) -> Result<(), String> {
    let mut game = VehicleDemo::new(
        GameFiles::bundled().map_err(|e| e.to_string())?,
        ClientOptions {
            net: NetMode::Loopback,
            name: "Ann".to_string(),
            port: 0,
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
    game.set_global("$World::AloneAsked", "1");
    let mut waited = 0;
    while !(game.in_world() && game.buggy_ghost().is_some()) {
        let ms = clock.next_ms();
        frame(&mut game, ms, None, 0)?;
        waited += ms;
        if waited > 30_000 {
            return Err("the world did not start".into());
        }
    }
    run_for(&mut game, &mut clock, &mut None, 2000)?;
    if case.seated {
        game.host.mouse_motion(0.0, 110.0);
        run_for(&mut game, &mut clock, &mut None, 300)?;
        let middle = (SIZE.0 as i32 / 2, SIZE.1 as i32 / 2);
        game.host.mouse_button(MouseButton::Left, true, middle);
        game.host.mouse_button(MouseButton::Left, false, middle);
        run_for(&mut game, &mut clock, &mut None, 600)?;
        if !game.driving() {
            return Err("the figure did not get in".into());
        }
        // The look back level behind the buggy.
        game.host.mouse_motion(0.0, -110.0);
    }
    run_for(&mut game, &mut clock, &mut render, 2000)?;
    let mut log = CameraLog::default();
    let start = clock.now_ms();
    let mut n = 0;
    while clock.now_ms() - start < u64::from(case.ms) {
        let t = (clock.now_ms() - start) as u32;
        let ms = clock.next_ms();
        let mark = (case.script)(&mut game, t, ms).unwrap_or("");
        let drawn = frame(&mut game, ms, render.as_mut(), n)?;
        let figure = if game.seated() {
            game.buggy_pose().map(|m| m.column(3))
        } else {
            game.figure().map(|f| f.position)
        };
        log.push(CameraFrame {
            t: (t + ms) as f32 / 1000.0,
            dt: ms as f32 / 1000.0,
            drawn,
            target: None,
            figure,
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
