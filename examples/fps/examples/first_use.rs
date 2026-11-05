//! The frame times of the arena's first-time effects, drawn offscreen: two clients over the loopback address (Ann hosts, Bob
//! joins), then Ann's first shot, a second one, a shot at a wall, shots at Bob until he falls, and a walk. Each frame's CPU
//! time (the clock's step, the frame's scene, its draw, the wait for the GPU) is printed per event beside the median of the
//! quiet frames before them, with the time the map took to load.
//!
//! `cargo run -p torque-example-fps --release --example first_use [-- <width> <height>]`

use std::time::Instant;

use torque::app::App;
use torque::app::shots::{FRAME_MS, Seat, run};
use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;
use torque_render_core::Gpu;

type S = Seat<ArenaGame>;

/// The frames after an event's start whose slowest is its time.
const FIRST_FRAMES: usize = 6;

/// One frame's CPU times (ms): the clock's step, the frame's scene, its draw, the wait for the GPU.
#[derive(Clone, Copy, Default)]
struct Times {
    step: f64,
    scene: f64,
    draw: f64,
    gpu: f64,
    /// The render pipelines the frame made, and the draw's slowest stretch between the renderer's marks.
    made: u64,
    mark: (&'static str, f64),
}

impl Times {
    fn total(&self) -> f64 {
        self.step + self.scene + self.draw + self.gpu
    }
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// One frame of `seat`, timed.
fn timed(gpu: &Gpu, seat: &mut S) -> Result<Times, String> {
    let t = Instant::now();
    seat.game.advance(FRAME_MS);
    let step = ms(t);
    let t = Instant::now();
    let (gui, eye) = seat.game.frame(&mut seat.view);
    let scene = ms(t);
    let made = seat.view.renderer().pipelines_made;
    seat.view.renderer().take_cpu_passes();
    let t = Instant::now();
    seat.view.draw(gpu, &seat.target.color, &gui, eye)?;
    let draw = ms(t);
    let made = seat.view.renderer().pipelines_made - made;
    let mark = seat
        .view
        .renderer()
        .take_cpu_passes()
        .into_iter()
        .filter(|(name, _)| !name.starts_with("passes") && !name.starts_with("draws"))
        .max_by(|x, y| x.1.total_cmp(&y.1))
        .unwrap_or(("", 0.0));
    let t = Instant::now();
    gpu.finish().map_err(|e| e.to_string())?;
    let gpu_ms = ms(t);
    Ok(Times {
        step,
        scene,
        draw,
        gpu: gpu_ms,
        made,
        mark,
    })
}

/// The frames of both seats while `ms` of the clock runs, each labeled `what`.
struct Log {
    rows: Vec<(String, Times, Times)>,
}

impl Log {
    fn run(&mut self, gpu: &Gpu, a: &mut S, b: &mut S, what: &str, ms: u32) -> Result<(), String> {
        for _ in 0..ms.div_ceil(FRAME_MS) {
            let ta = timed(gpu, a)?;
            let tb = timed(gpu, b)?;
            self.rows.push((what.to_string(), ta, tb));
        }
        Ok(())
    }

    /// Frames labeled `before` until Ann's scene has a new bolt hole, then `after` up to `ms` in all.
    fn run_to_hole(
        &mut self,
        gpu: &Gpu,
        a: &mut S,
        b: &mut S,
        before: &str,
        after: &str,
        ms: u32,
    ) -> Result<(), String> {
        let holes = a.game.scene().decals.len();
        let mut what = before;
        for _ in 0..ms.div_ceil(FRAME_MS) {
            let ta = timed(gpu, a)?;
            let tb = timed(gpu, b)?;
            if a.game.scene().decals.len() > holes {
                what = after;
            }
            self.rows.push((what.to_string(), ta, tb));
        }
        Ok(())
    }
}

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

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let width = args.next().and_then(|s| s.parse().ok()).unwrap_or(1280);
    let height = args.next().and_then(|s| s.parse().ok()).unwrap_or(720);
    let gpu = torque::app::window::offscreen_gpu()?;
    println!("adapter: {}", gpu.adapter());
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
        let mut seat = Seat::new(game, &gpu, (width, height))?;
        seat.view.renderer().cpu_marks = true;
        Ok(seat)
    };
    let (mut a, mut b) = (seat("Ann")?, seat("Bob")?);
    run(&gpu, &mut [&mut a], 200)?;

    let load = Instant::now();
    a.game.call("hostGame", &["lan"])?;
    until(&gpu, &mut a, &mut b, "the hosted game", |a, _| {
        a.game.in_game() && a.game.scene().own().is_some()
    })?;
    let host_ms = ms(load);
    let port = a.game.hosted_address().ok_or("no hosted address")?.port();
    b.game.set_global("$Arena::Port", &port.to_string());
    b.game.call("showJoin", &[])?;
    until(&gpu, &mut a, &mut b, "the list of games", |_, b| {
        b.game
            .global("$Arena::ServerRows")
            .parse::<u32>()
            .is_ok_and(|rows| rows > 0)
    })?;
    let join = Instant::now();
    b.game.call("joinSelected", &[])?;
    until(&gpu, &mut a, &mut b, "the second player", |a, b| {
        b.game.in_game() && a.game.scene().holders_drawn > 0
    })?;
    let join_ms = ms(join);
    println!("load: host {host_ms:.0} ms, join {join_ms:.0} ms");

    let mut log = Log { rows: Vec::new() };
    place(&mut a, "Ann", [2.0, -18.0, 0.1], 0.0)?;
    place(&mut a, "Bob", [-14.0, 14.0, 0.1], 0.0)?;
    log.run(&gpu, &mut a, &mut b, "quiet", 3000)?;

    // Shots into the arena's length (the bolt flies on until it hits the far wall).
    a.game.call("fire", &["1"])?;
    log.run(&gpu, &mut a, &mut b, "first shot", 64)?;
    a.game.call("fire", &["0"])?;
    log.run_to_hole(&gpu, &mut a, &mut b, "first shot", "first hole", 1400)?;
    a.game.call("fire", &["1"])?;
    log.run(&gpu, &mut a, &mut b, "second shot", 64)?;
    a.game.call("fire", &["0"])?;
    log.run_to_hole(&gpu, &mut a, &mut b, "second shot", "second hole", 1400)?;

    // Shots at Bob a few steps away until he falls.
    place(&mut a, "Ann", [4.0, -16.0, 0.1], 0.0)?;
    place(&mut a, "Bob", [4.0, -11.0, 0.1], std::f32::consts::PI)?;
    log.run(&gpu, &mut a, &mut b, "quiet", 1000)?;
    for what in ["first hit", "second hit", "first fall"] {
        a.game.call("fire", &["1"])?;
        log.run(&gpu, &mut a, &mut b, what, 64)?;
        a.game.call("fire", &["0"])?;
        log.run(&gpu, &mut a, &mut b, what, 1000)?;
    }
    log.run(&gpu, &mut a, &mut b, "first fall", 2000)?;

    // A walk forward.
    a.game.set_global("$mvForwardAction", "1");
    log.run(&gpu, &mut a, &mut b, "first walk", 1500)?;
    a.game.set_global("$mvForwardAction", "0");
    log.run(&gpu, &mut a, &mut b, "quiet", 500)?;

    let quiet: Vec<&(String, Times, Times)> = log.rows.iter().filter(|r| r.0 == "quiet").collect();
    let med_a = median(quiet.iter().map(|r| r.1.total()).collect());
    let med_b = median(quiet.iter().map(|r| r.2.total()).collect());
    let p95 = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() * 95 / 100]
    };
    println!(
        "quiet frames: {} (Ann median {med_a:.2} ms, p95 {:.2}; Bob median {med_b:.2} ms, p95 {:.2})",
        quiet.len(),
        p95(quiet.iter().map(|r| r.1.total()).collect()),
        p95(quiet.iter().map(|r| r.2.total()).collect()),
    );
    println!(
        "{:<12} {:>8} {:>8} {:>6} {:>6} {:>6} {:>6} {:>8} {:>8} {:>8}  pipelines made",
        "event", "Ann", "over", "step", "scene", "draw", "gpu", "Bob", "over", "window"
    );
    let mut events: Vec<&str> = Vec::new();
    for r in &log.rows {
        if r.0 != "quiet" && !events.contains(&r.0.as_str()) {
            events.push(&r.0);
        }
    }
    // Each event's first frames (where its first-time work lands): the slowest of them, Ann's and Bob's, and the slowest
    // frame of its whole window.
    for e in events {
        let rows: Vec<_> = log.rows.iter().filter(|r| r.0 == e).collect();
        let first = &rows[..rows.len().min(FIRST_FRAMES)];
        let worst = first
            .iter()
            .max_by(|x, y| x.1.total().total_cmp(&y.1.total()))
            .ok_or("no rows")?;
        let worst_b = first.iter().map(|r| r.2.total()).fold(0.0, f64::max);
        let window = rows.iter().map(|r| r.1.total()).fold(0.0, f64::max);
        let t = worst.1;
        println!(
            "{:<12} {:>8.2} {:>8.2} {:>6.2} {:>6.2} {:>6.2} {:>6.2} {:>8.2} {:>8.2} {:>8.2}  {}",
            e,
            t.total(),
            t.total() - med_a,
            t.step,
            t.scene,
            t.draw,
            t.gpu,
            worst_b,
            worst_b - med_b,
            window,
            rows.iter().map(|r| r.1.made + r.2.made).sum::<u64>()
        );
    }
    if std::env::var_os("FIRST_USE_ALL").is_some() {
        for (i, r) in log.rows.iter().enumerate() {
            println!(
                "{i:4} {:<12} {:7.2} [{:.2} {:.2} {:.2} {:.2}] {} {} {:.2} | {:7.2} {}",
                r.0,
                r.1.total(),
                r.1.step,
                r.1.scene,
                r.1.draw,
                r.1.gpu,
                r.1.made,
                r.1.mark.0,
                r.1.mark.1,
                r.2.total(),
                r.2.made
            );
        }
    }
    Ok(())
}
