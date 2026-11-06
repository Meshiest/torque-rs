//! A clip of aiming down the sights, drawn offscreen: two clients over the loopback address face each other; both raise their
//! pistols to the sights, lower them, then raise and lower them again at once (the press let go mid-way). Each frame shows
//! Ann's own view (left) beside Bob seen from his side (right), at 60 frames a second.
//!
//! The frames go to `ffmpeg` (on the path) as `ads-clip.mp4` in the folder given; a strip of the first raise goes beside it as
//! `ads-strip.png`.
//!
//! `cargo run -p torque-example-fps --release --example ads_clip -- <dir>`

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use torque::app::App;
use torque::app::shots::{FRAME_MS, Seat, png_rgb, run};
use torque::app::view::WorldView;
use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;
use torque_render_core::Gpu;
use torque_render_scene::hd::kit::look_at;

type S = Seat<ArenaGame>;

const W: u32 = 640;
const H: u32 = 360;

fn until(
    gpu: &Gpu,
    a: &mut S,
    b: &mut S,
    what: &str,
    done: impl Fn(&S, &S) -> bool,
) -> Result<(), String> {
    let mut waited = 0;
    while !done(a, b) {
        if waited > 30_000 {
            return Err(format!("timed out waiting for {what}"));
        }
        run(gpu, &mut [&mut *a, &mut *b], FRAME_MS)?;
        waited += FRAME_MS;
    }
    Ok(())
}

fn place(a: &mut S, name: &str, at: [f32; 3], yaw: f32) -> Result<(), String> {
    let server = a.game.server.as_mut().ok_or("no hosted server")?;
    let [x, y, z] = at.map(|v| v.to_string());
    server.call("placeFigure", &[name, &x, &y, &z, &yaw.to_string()])?;
    Ok(())
}

/// Ann's own view and Bob from his side, side by side, as RGB rows.
fn frame(
    gpu: &Gpu,
    a: &mut S,
    b: &mut S,
    side: &mut WorldView,
    side_target: &torque_render_core::Offscreen,
) -> Result<Vec<u8>, String> {
    a.game.advance(FRAME_MS);
    b.game.advance(FRAME_MS);
    a.frame(gpu)?;
    b.frame(gpu)?;
    let own = gpu.read_back(&a.target).map_err(|e| e.to_string())?;
    let (gui, _) = a.game.frame(side);
    let bob = [4.0, -11.0, 0.1];
    let mut eye = look_at([bob[0] + 2.6, bob[1] - 3.2, 1.9], [bob[0], bob[1], 1.3]);
    eye.fov = Some(50);
    side.draw(gpu, &side_target.color, &gui, eye)?;
    let other = gpu.read_back(side_target).map_err(|e| e.to_string())?;
    let mut rgb = Vec::with_capacity((W * 2 * H * 3) as usize);
    for y in 0..H {
        for f in [&own, &other] {
            let row = &f.rgba[(4 * y * W) as usize..(4 * (y + 1) * W) as usize];
            rgb.extend(
                row.as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [p[0], p[1], p[2]]),
            );
        }
    }
    Ok(rgb)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "ads-clip".into()));
    std::fs::create_dir_all(&dir)?;
    let gpu = torque::app::window::offscreen_gpu()?;
    let seat = |name: &str| -> Result<S, String> {
        let files = GameFiles::bundled().map_err(|e| e.to_string())?;
        let game = ArenaGame::new(
            files,
            ClientOptions {
                net: NetMode::Loopback,
                name: name.to_string(),
                port: 0,
                ..ClientOptions::default()
            },
        )?;
        Seat::new(game, &gpu, (W, H))
    };
    let (mut a, mut b) = (seat("Ann")?, seat("Bob")?);
    run(&gpu, &mut [&mut a], 200)?;
    a.game.call("hostGame", &["lan"])?;
    until(&gpu, &mut a, &mut b, "the hosted game", |a, _| {
        a.game.in_game()
    })?;
    let port = a.game.hosted_address().ok_or("no hosted address")?.port();
    b.game.set_global("$Arena::Port", &port.to_string());
    b.game.call("showJoin", &[])?;
    until(&gpu, &mut a, &mut b, "the list of games", |_, b| {
        b.game
            .global("$Arena::ServerRows")
            .parse::<u32>()
            .is_ok_and(|rows| rows > 0)
    })?;
    b.game.call("joinSelected", &[])?;
    until(&gpu, &mut a, &mut b, "the second player", |a, b| {
        b.game.in_game() && a.game.scene().holders_drawn > 0
    })?;
    place(&mut a, "Ann", [4.0, -16.0, 0.1], 0.0)?;
    place(&mut a, "Bob", [4.0, -11.0, 0.1], std::f32::consts::PI)?;
    run(&gpu, &mut [&mut a, &mut b], 1500)?;

    let mut side = a.game.view(&gpu);
    let side_target = gpu.offscreen(W, H)?;
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
        .arg(format!("{}x{H}", 2 * W))
        .args([
            "-r", "60", "-i", "-", "-pix_fmt", "yuv420p", "-c:v", "libx264", "-crf", "18",
        ])
        .arg(dir.join("ads-clip.mp4"))
        .stdin(Stdio::piped())
        .spawn()?;
    let mut pipe = ffmpeg.stdin.take().ok_or("no ffmpeg input")?;
    // (ms from the start, aim down or up) for both players.
    let presses = [(300, true), (900, false), (1500, true), (1580, false)];
    let mut strip: Vec<Vec<u8>> = Vec::new();
    let mut t = 0;
    while t < 2200 {
        for (at, down) in presses {
            if at == t || (at > t && at < t + FRAME_MS) {
                let v = if down { "1" } else { "0" };
                a.game.call("aim", &[v])?;
                b.game.call("aim", &[v])?;
            }
        }
        let rgb = frame(&gpu, &mut a, &mut b, &mut side, &side_target)?;
        // Every other frame of the first raise.
        if (288..288 + 16 * FRAME_MS).contains(&t) && (t - 288) % (2 * FRAME_MS) == 0 {
            strip.push(rgb.clone());
        }
        pipe.write_all(&rgb)?;
        t += FRAME_MS;
    }
    drop(pipe);
    ffmpeg.wait()?;
    // The strip: the frames' left halves (Ann's view) stacked side by side, at half size.
    let (sw, sh) = (W / 2, H / 2);
    let n = strip.len() as u32;
    let png = png_rgb(sw * n, sh * 2, |x, y| {
        let f = &strip[(x / sw) as usize];
        let (fx, fy) = (x % sw, y % sh);
        let half = if y < sh { 0 } else { W };
        let i = (3 * ((2 * fy) * 2 * W + half + 2 * fx)) as usize;
        [f[i], f[i + 1], f[i + 2]]
    })?;
    std::fs::write(dir.join("ads-strip.png"), png)?;
    println!("{}", dir.join("ads-clip.mp4").display());
    Ok(())
}
