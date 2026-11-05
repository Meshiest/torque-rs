//! Frame sequences of the steering and of the tire tracks at speed, a frame every 16 ms of the clock, for review as clips.
//!
//! `cargo run -p torque-example-vehicle-demo --release --example steer-clip -- <dir>` hosts a game, a second player joins and
//! stands beside the buggy while the first drives it. With the buggy standing, the driver steers left (A held a second), holds,
//! then lets go back to the middle; then drives off at full throttle and the frames at speed show the tracks start under the
//! wheels. Each sequence is a folder of numbered pictures in `<dir>`:
//!
//! - `own-eyes/`: the driver's own view from the seated figure's eyes (Tab), the hands on the wheel;
//! - `own-wheel/`: the driver's client, from ahead of the wheel over the dash, looking back at the driver;
//! - `own-chase/`: the driver's client, from the camera behind the buggy, raised to see into the cockpit;
//! - `other-wheel/` and `other-chase/`: the second player's client, the same two views of the buggy the first drives;
//! - `own-tracks/` and `other-tracks/`: at speed, from above and beside the left rear wheel, on each client;
//!
//! and `pause-menu.png`: the pause menu (Esc at the wheel) over the view behind the buggy. Beside each folder, `<folder>.csv`
//! has a line a frame: the frame, where the buggy is drawn and where its left rear wheel's track ends, along its way (m).

use std::path::{Path, PathBuf};

use torque::app::App as _;
use torque::app::shots::{Seat, run, write_png};
use torque::app::window::offscreen_gpu;
use torque::client::input::{Key, MouseButton};
use torque_example_vehicle_demo::client::{ClientOptions, NetMode, VehicleDemo};
use torque_example_vehicle_demo::files::GameFiles;
use torque_render_core::Gpu;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::math::Mat;
use torque_render_scene::mounts::pose_matrix;
use torque_render_scene::source::Eye;

const SIZE: (u32, u32) = (960, 540);
const FRAME_MS: u32 = torque::app::shots::FRAME_MS;
const WAIT_LIMIT_MS: u32 = 20_000;

fn eye(at: [f32; 3], target: [f32; 3]) -> Eye {
    let mut e = look_at(at, target);
    e.fov = None;
    e
}

/// The pose `game` draws vehicle ghost `index` at.
fn pose_of(game: &VehicleDemo, index: u32) -> Option<Mat> {
    let pose = game
        .with_world(|w| w.vehicle_render.get(&index).copied())
        .flatten()?;
    pose_matrix(&pose, None).map(|(m, _)| m)
}

/// The views of a sequence: from ahead of the wheel, from behind and above, low beside the left rear wheel.
#[derive(Clone, Copy, PartialEq)]
enum View {
    Eyes,
    Wheel,
    Chase,
    Tracks,
}

fn view_eye(m: &Mat, view: View) -> Option<Eye> {
    let (wheel, _) = torque_example_assets::buggy::steering_wheel();
    let p = |l: [f32; 3]| m.mul_p(l);
    match view {
        View::Eyes => None,
        View::Wheel => Some(eye(
            p([wheel[0] + 0.1, wheel[1] + 1.0, wheel[2] + 0.55]),
            p(wheel),
        )),
        View::Chase => Some(eye(
            p([0.6, -4.2, 3.3]),
            p([wheel[0], wheel[1], wheel[2] - 0.2]),
        )),
        View::Tracks => Some(eye(p([-3.4, -2.2, 2.6]), p([-1.1, -1.5, 0.0]))),
    }
}

struct Seq {
    dir: PathBuf,
    n: usize,
}

impl Seq {
    fn new(root: &Path, name: &str) -> Result<Seq, String> {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(Seq { dir, n: 0 })
    }

    /// The frame drawn, and a line of `<dir>.csv`: the frame, where the buggy (ghost `buggy`) is drawn and where the left rear
    /// wheel's track ends, along the buggy's way (m).
    fn save(&mut self, gpu: &Gpu, seat: &Seat<VehicleDemo>, buggy: u32) -> Result<(), String> {
        let frame = gpu.read_back(&seat.target).map_err(|e| e.to_string())?;
        let path = self.dir.join(format!("{:04}.png", self.n));
        write_png(&frame, &path).map_err(|e| e.to_string())?;
        if let Some((body, front)) = along_way(&seat.game, buggy) {
            use std::io::Write as _;
            let mut csv = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.dir.with_extension("csv"))
                .map_err(|e| e.to_string())?;
            let front = front.map_or(String::new(), |f| format!("{f:.4}"));
            writeln!(csv, "{},{body:.4},{front}", self.n).map_err(|e| e.to_string())?;
        }
        self.n += 1;
        Ok(())
    }
}

/// Where `game` draws ghost `buggy` and where its left rear wheel's track ends (its head's end, or its newest segment's), along
/// the buggy's way on the ground.
fn along_way(game: &VehicleDemo, buggy: u32) -> Option<(f32, Option<f32>)> {
    let m = pose_of(game, buggy)?;
    let f = m.column(1);
    let n = f[0].hypot(f[1]).max(1e-6);
    let along = |p: [f32; 3]| (p[0] * f[0] + p[1] * f[1]) / n;
    let tracks = game.scene().tracks();
    let front = tracks.and_then(|t| {
        t.heads()
            .find(|(k, _)| **k == (buggy, 2))
            .map(|(_, s)| s.line[1])
            .or_else(|| {
                t.segments()
                    .filter(|s| s.wheel == (buggy, 2))
                    .last()
                    .map(|s| s.line[1])
            })
    });
    Some((along(m.column(3)), front.map(along)))
}

fn run_until(
    gpu: &Gpu,
    a: &mut Seat<VehicleDemo>,
    b: &mut Seat<VehicleDemo>,
    what: &str,
    done: impl Fn(&VehicleDemo, &VehicleDemo) -> bool,
) -> Result<(), String> {
    let mut waited = 0;
    while !done(&a.game, &b.game) {
        if waited >= WAIT_LIMIT_MS {
            return Err(format!("{what} did not come"));
        }
        run(gpu, &mut [&mut *a, &mut *b], 100)?;
        waited += 100;
    }
    Ok(())
}

/// One pass: a fresh game, the driver's client drawn from `own` and the other's from `other`, the frames of the steering
/// (and with `tracks`, of the drive at speed) saved.
fn pass(
    gpu: &Gpu,
    root: &Path,
    own: (View, &str),
    other: (View, &str),
    tracks: bool,
) -> Result<(), String> {
    let seat = |name: &str| -> Result<Seat<VehicleDemo>, String> {
        let files = GameFiles::bundled().map_err(|e| e.to_string())?;
        let game = VehicleDemo::new(
            files,
            ClientOptions {
                net: NetMode::Loopback,
                name: name.to_string(),
                port: 0,
                ..ClientOptions::default()
            },
        )?;
        Seat::new(game, gpu, SIZE)
    };
    let (mut a, mut b) = (seat("Ann")?, seat("Bob")?);
    run(gpu, &mut [&mut a], 200)?;
    a.game.host_game()?;
    run_until(gpu, &mut a, &mut b, "the hosted game", |a, _| {
        a.in_world() && a.figure().is_some()
    })?;
    let address = a.game.hosted_address().ok_or("no hosted address")?;
    b.game.join(&address.to_string())?;
    run_until(gpu, &mut a, &mut b, "the second player", |a, b| {
        b.in_world() && b.figure().is_some() && a.ghost_count() > 3
    })?;
    run(gpu, &mut [&mut a, &mut b], 3000)?;
    a.game.host.mouse_motion(0.0, 110.0);
    run(gpu, &mut [&mut a, &mut b], 300)?;
    a.game
        .host
        .mouse_button(MouseButton::Left, true, (480, 270));
    a.game
        .host
        .mouse_button(MouseButton::Left, false, (480, 270));
    run_until(gpu, &mut a, &mut b, "the driver's seat", |a, _| a.driving())?;
    a.game.host.mouse_motion(0.0, -110.0);
    run(gpu, &mut [&mut a, &mut b], 1500)?;
    let ga = a.game.buggy_ghost().ok_or("no buggy")?;
    let gb = b.game.buggy_ghost().ok_or("no buggy on the other client")?;
    a.game.set_first_person(own.0 == View::Eyes);

    let mut seq_a = Seq::new(root, own.1)?;
    let mut seq_b = Seq::new(root, other.1)?;
    // Each frame: both clocks a frame on, each drawn from its view of the buggy as it draws it this frame.
    let frames = |a: &mut Seat<VehicleDemo>,
                  b: &mut Seat<VehicleDemo>,
                  ms: u32,
                  seqs: Option<(&mut Seq, &mut Seq)>,
                  views: (View, View)|
     -> Result<(), String> {
        let mut seqs = seqs;
        for _ in 0..ms.div_ceil(FRAME_MS) {
            for (s, g, v) in [(&mut *a, ga, views.0), (&mut *b, gb, views.1)] {
                s.game.advance(FRAME_MS);
                // The eye on the buggy where this frame draws it: the frame's prediction first (the client's ticks pose it at
                // the tick's time; the frame draws it between ticks), as the frame itself does again (the same time: no change).
                s.game.client.predict_frame();
                s.eye = pose_of(&s.game, g).and_then(|m| view_eye(&m, v));
                s.frame(gpu)?;
            }
            if let Some((sa, sb)) = seqs.as_mut() {
                sa.save(gpu, a, ga)?;
                sb.save(gpu, b, gb)?;
            }
        }
        Ok(())
    };
    let views = (own.0, other.0);
    frames(&mut a, &mut b, 400, None, views)?;
    frames(&mut a, &mut b, 300, Some((&mut seq_a, &mut seq_b)), views)?;
    a.game.key(Key::A, true);
    frames(&mut a, &mut b, 1100, Some((&mut seq_a, &mut seq_b)), views)?;
    a.game.key(Key::A, false);
    frames(&mut a, &mut b, 900, Some((&mut seq_a, &mut seq_b)), views)?;
    if !tracks {
        return Ok(());
    }
    a.game.set_first_person(false);
    a.game.key(Key::W, true);
    frames(&mut a, &mut b, 3500, None, (View::Tracks, View::Tracks))?;
    let mut ta = Seq::new(root, "own-tracks")?;
    let mut tb = Seq::new(root, "other-tracks")?;
    frames(
        &mut a,
        &mut b,
        1200,
        Some((&mut ta, &mut tb)),
        (View::Tracks, View::Tracks),
    )?;
    a.game.key(Key::W, false);
    // The pause menu over the driving view behind the buggy (Esc at the wheel).
    a.game.key(Key::Space, true);
    run(gpu, &mut [&mut a, &mut b], 1500)?;
    a.game.key(Key::Space, false);
    a.eye = None;
    a.game.key(Key::Escape, true);
    a.game.key(Key::Escape, false);
    run(gpu, &mut [&mut a, &mut b], 300)?;
    let frame = a.picture(gpu)?;
    write_png(&frame, &root.join("pause-menu.png")).map_err(|e| e.to_string())?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).ok_or("usage: steer-clip <dir>")?;
    let root = PathBuf::from(dir);
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    pass(
        &gpu,
        &root,
        (View::Wheel, "own-wheel"),
        (View::Wheel, "other-wheel"),
        true,
    )?;
    pass(
        &gpu,
        &root,
        (View::Eyes, "own-eyes"),
        (View::Chase, "other-chase"),
        false,
    )?;
    pass(
        &gpu,
        &root,
        (View::Chase, "own-chase"),
        (View::Chase, "other-chase-2"),
        false,
    )?;
    println!("{}", root.display());
    Ok(())
}
