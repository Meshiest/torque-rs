//! The example without a window: two clients in one process over the loopback address, every frame drawn offscreen as the
//! window would draw it, and pictures taken: the main menu, the list of games with the hosted one, the view from the host's
//! eyes with its HUD (rounds, health, chat), the other player holding its pistol, and a bolt in flight (as it leaves the
//! muzzle, then with its trail behind it eight steps away and two steps away), then the arena itself: the floor at the host's
//! feet, the length of the arena from a corner, the host's own shadow on the floor ahead, the view from a spawn point with the
//! other player hidden behind the cover, and the whole arena from above.

use std::io;
use std::path::{Path, PathBuf};

use torque_example_common::view::WorldView;
use torque_render_core::{Frame, Gpu, Offscreen};
use torque_render_scene::source::Eye;

use crate::client::{ArenaGame, ClientOptions, NetMode};
use crate::files::GameFiles;

/// The clock's step between frames (ms).
pub const FRAME_MS: u32 = 16;
/// The frames drawn from a still scene before its picture (the renderer's history settles).
pub const SETTLE_FRAMES: u32 = 24;
/// The longest a step of the script may wait (ms of the clock).
pub const WAIT_LIMIT_MS: u32 = 20_000;

/// One client and what its frames draw into.
struct Seat {
    game: ArenaGame,
    view: WorldView,
    target: Offscreen,
    /// An eye in place of the game's (none: the game's).
    eye: Option<Eye>,
}

impl Seat {
    fn frame(&mut self, gpu: &Gpu) -> Result<(), String> {
        let (gui, eye) = self.game.frame(&mut self.view);
        let eye = self.eye.unwrap_or(eye);
        self.view.draw(gpu, &self.target.color, &gui, eye)?;
        Ok(())
    }

    /// A picture of the scene as it stands (the clock held), after the renderer settled on it.
    fn picture(&mut self, gpu: &Gpu) -> Result<Frame, String> {
        self.view.reset_history();
        for _ in 0..SETTLE_FRAMES {
            self.frame(gpu)?;
        }
        gpu.read_back(&self.target).map_err(|e| e.to_string())
    }
}

/// Both clients' clocks run `ms`, a frame of each every [`FRAME_MS`].
fn run(gpu: &Gpu, seats: &mut [&mut Seat], ms: u32) -> Result<(), String> {
    for _ in 0..ms.div_ceil(FRAME_MS) {
        for s in seats.iter_mut() {
            s.game.step(FRAME_MS);
            s.game.add_frame_time(FRAME_MS as f32 / 1000.0);
            s.frame(gpu)?;
        }
    }
    Ok(())
}

/// Runs both clocks until `done` holds (at most [`WAIT_LIMIT_MS`]).
fn run_until(
    gpu: &Gpu,
    a: &mut Seat,
    b: &mut Seat,
    what: &str,
    done: impl Fn(&Seat, &Seat) -> bool,
) -> Result<(), String> {
    let mut waited = 0;
    while !done(a, b) {
        if waited > WAIT_LIMIT_MS {
            return Err(format!("timed out waiting for {what}"));
        }
        run(gpu, &mut [&mut *a, &mut *b], FRAME_MS)?;
        waited += FRAME_MS;
    }
    Ok(())
}

/// Writes `frame` as a PNG.
pub fn write_png(frame: &Frame, path: &Path) -> io::Result<()> {
    let width = frame.width;
    let bytes = torque_example_assets::textures::png(frame.width, frame.height, |x, y| {
        let i = 4 * (y * width + x) as usize;
        [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
    })?;
    std::fs::write(path, bytes)
}

/// Places a client's figure on the server: feet at `at`, facing `yaw`.
fn place(host: &mut ArenaGame, name: &str, at: [f32; 3], yaw: f32) -> Result<(), String> {
    let server = host.server.as_mut().ok_or("no hosted server")?;
    let [x, y, z] = at.map(|v| v.to_string());
    server.call("placeFigure", &[name, &x, &y, &z, &yaw.to_string()])?;
    Ok(())
}

/// The pictures at `size`, written into `dir` (see the module docs). Returns the paths.
pub fn write_all(
    dir: &Path,
    size: (u32, u32),
    make_files: impl Fn() -> io::Result<GameFiles>,
) -> Result<Vec<PathBuf>, String> {
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let seat = |name: &str| -> Result<Seat, String> {
        let files = make_files().map_err(|e| e.to_string())?;
        let mut game = ArenaGame::new(
            files,
            ClientOptions {
                net: NetMode::Loopback,
                name: name.to_string(),
                port: 0,
                ..ClientOptions::default()
            },
        )?;
        game.host.surface_size(size);
        Ok(Seat {
            view: game.view(&gpu),
            target: gpu.offscreen(size.0, size.1).map_err(|e| e.to_string())?,
            eye: None,
            game,
        })
    };
    let (mut a, mut b) = (seat("Ann")?, seat("Bob")?);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    let mut save = |frame: &Frame, name: &str| -> Result<(), String> {
        let path = dir.join(format!("{name}.png"));
        write_png(frame, &path).map_err(|e| e.to_string())?;
        paths.push(path);
        Ok(())
    };

    run(&gpu, &mut [&mut a], 200)?;
    save(&a.picture(&gpu)?, "menu")?;

    a.game.call("hostGame", &[])?;
    run_until(&gpu, &mut a, &mut b, "the hosted game", |a, _| {
        a.game.in_game()
    })?;
    let port = a.game.hosted_address().ok_or("no hosted address")?.port();
    b.game.set_global("$Arena::Port", &port.to_string());
    b.game.call("showJoin", &[])?;
    run_until(&gpu, &mut a, &mut b, "the list of games", |_, b| {
        b.game
            .global("$Arena::ServerRows")
            .parse::<u32>()
            .is_ok_and(|rows| rows > 0)
    })?;
    save(&b.picture(&gpu)?, "lan-list")?;

    b.game.call("joinSelected", &[])?;
    run_until(&gpu, &mut a, &mut b, "the second player", |a, b| {
        b.game.in_game() && a.game.scene().holders_drawn > 0
    })?;
    run(&gpu, &mut [&mut a, &mut b], 1000)?;

    // Face to face, a few steps apart: Bob holds his pistol in Ann's view.
    place(&mut a.game, "Ann", [4.0, -16.0, 0.1], 0.0)?;
    place(&mut a.game, "Bob", [4.6, -11.5, 0.1], 3.6)?;
    a.game.call("say", &["Ready when you are"])?;
    b.game.call("say", &["Bring it"])?;
    run(&gpu, &mut [&mut a, &mut b], 1200)?;
    save(&a.picture(&gpu)?, "other-player")?;

    // A round fired: the HUD counts one fewer.
    a.game.call("fire", &["1"])?;
    run(&gpu, &mut [&mut a, &mut b], 64)?;
    a.game.call("fire", &["0"])?;
    run(&gpu, &mut [&mut a, &mut b], 1200)?;
    save(&a.picture(&gpu)?, "first-person")?;

    // Bob to the side, firing across Ann's view: the bolt in flight.
    place(&mut a.game, "Ann", [2.0, -18.0, 0.1], 0.0)?;
    place(
        &mut a.game,
        "Bob",
        [1.2, -10.0, 0.1],
        std::f32::consts::FRAC_PI_2,
    )?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    b.game.call("fire", &["1"])?;
    run_until(&gpu, &mut a, &mut b, "a bolt in flight", |a, _| {
        a.game.scene().bolts_drawn > 0
    })?;
    save(&a.picture(&gpu)?, "bolt")?;
    b.game.call("fire", &["0"])?;

    // The same bolt farther on, its trail behind it, eight steps from Ann; then Ann two steps from its line, and another.
    run_until(&gpu, &mut a, &mut b, "the bolt mid-view", |a, _| {
        a.game
            .scene()
            .bolt_positions()
            .iter()
            .any(|p| (6.0..7.5).contains(&p[0]))
    })?;
    save(&a.picture(&gpu)?, "bolt-cube-mid")?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    place(&mut a.game, "Ann", [8.0, -12.0, 0.1], 0.0)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    b.game.call("fire", &["1"])?;
    run_until(&gpu, &mut a, &mut b, "a bolt close by", |a, _| {
        a.game
            .scene()
            .bolt_positions()
            .iter()
            .any(|p| (8.3..9.5).contains(&p[0]))
    })?;
    save(&a.picture(&gpu)?, "bolt-cube-near")?;
    b.game.call("fire", &["0"])?;

    // The floor close up, looking down at Ann's feet; then the whole arena from a corner, the floor reaching far off.
    place(&mut a.game, "Ann", [-14.0, 0.0, 0.1], 0.0)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    a.game.set_global("$Arena::LookPitch", "1.1");
    save(&a.picture(&gpu)?, "floor-near")?;
    place(
        &mut a.game,
        "Ann",
        [-18.5, -18.5, 0.1],
        std::f32::consts::FRAC_PI_4,
    )?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    a.game.set_global("$Arena::LookPitch", "0.2");
    save(&a.picture(&gpu)?, "arena-far")?;

    // Ann's own shadow on the floor ahead of her, the sun behind her.
    place(
        &mut a.game,
        "Ann",
        [-14.0, 0.0, 0.1],
        std::f32::consts::FRAC_PI_4,
    )?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    a.game.set_global("$Arena::LookPitch", "0.6");
    save(&a.picture(&gpu)?, "own-shadow")?;

    // From the first spawn point toward the middle, Bob just behind the ring of cover; then the whole arena from above.
    place(&mut a.game, "Ann", [0.0, -16.0, 0.1], 0.0)?;
    place(&mut a.game, "Bob", [0.0, -5.5, 0.1], std::f32::consts::PI)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    save(&a.picture(&gpu)?, "spawn-view")?;
    a.eye = Some(a.game.scene().overview_eye());
    save(&a.picture(&gpu)?, "overview")?;
    a.eye = None;
    Ok(paths)
}
