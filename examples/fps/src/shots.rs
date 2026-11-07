//! The example without a window: two clients in one process over the loopback address, every frame drawn offscreen as the
//! window would draw it, and pictures taken: the main menu, the choice of who may join a hosted game, the list of games with
//! the hosted one, the address box with an error line, the view from the host's eyes with its HUD (rounds, health, chat), the other player holding its pistol, and a bolt in flight (as it leaves the
//! muzzle, then with its trail behind it eight steps away and two steps away), then the arena itself: the floor at the host's
//! feet, the length of the arena from a corner, the host's own shadow on the floor ahead, the view from a spawn point with the
//! other player hidden behind the cover, and the whole arena from above.

use std::io;
use std::path::{Path, PathBuf};

use torque::app::shots::{FRAME_MS, Shots, run, write_png};
use torque_render_core::{Frame, Gpu};
use torque_render_scene::hd::kit::look_at;

use torque::game::scene::lag::STEP_MS;
use torque::script::vm::sim::ObjId;

use crate::client::{ArenaGame, ClientOptions, NetMode};
use crate::files::GameFiles;
use crate::hitbox_view;

/// The longest a step of the script may wait (ms of the clock).
pub const WAIT_LIMIT_MS: u32 = 20_000;

/// One client and what its frames draw into.
type Seat = torque::app::shots::Seat<ArenaGame>;

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
        let game = ArenaGame::new(
            files,
            ClientOptions {
                net: NetMode::Loopback,
                name: name.to_string(),
                port: 0,
                ..ClientOptions::default()
            },
        )?;
        Seat::new(game, &gpu, size)
    };
    let (mut a, mut b) = (seat("Ann")?, seat("Bob")?);
    let mut shots = Shots::new(dir)?;
    let mut save = |frame: &Frame, name: &str| shots.save(frame, name);

    run(&gpu, &mut [&mut a], 200)?;
    save(&a.picture(&gpu)?, "menu")?;

    a.game.call("showHost", &[])?;
    run(&gpu, &mut [&mut a], 64)?;
    save(&a.picture(&gpu)?, "host-choice")?;

    a.game.call("hostGame", &["lan"])?;
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

    // A name where the numbers go: the plain error line under the box.
    b.game
        .call("eval", &["AddressEdit.setValue(\"my-server.example\");"])?;
    b.game.call("connectAddress", &[])?;
    run(&gpu, &mut [&mut a, &mut b], 64)?;
    save(&b.picture(&gpu)?, "join-error")?;
    b.game.call("eval", &["AddressEdit.setValue(\"\");"])?;

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
    a.game.set_view_pitch(1.1);
    run(&gpu, &mut [&mut a, &mut b], 100)?;
    save(&a.picture(&gpu)?, "floor-near")?;
    place(
        &mut a.game,
        "Ann",
        [-18.5, -18.5, 0.1],
        std::f32::consts::FRAC_PI_4,
    )?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    a.game.set_view_pitch(0.2);
    run(&gpu, &mut [&mut a, &mut b], 100)?;
    save(&a.picture(&gpu)?, "arena-far")?;

    // Ann's own shadow on the floor ahead of her, the sun behind her.
    place(
        &mut a.game,
        "Ann",
        [-14.0, 0.0, 0.1],
        std::f32::consts::FRAC_PI_4,
    )?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    a.game.set_view_pitch(0.6);
    run(&gpu, &mut [&mut a, &mut b], 100)?;
    save(&a.picture(&gpu)?, "own-shadow")?;

    // From the first spawn point toward the middle, Bob just behind the ring of cover; then the whole arena from above.
    place(&mut a.game, "Ann", [0.0, -16.0, 0.1], 0.0)?;
    place(&mut a.game, "Bob", [0.0, -5.5, 0.1], std::f32::consts::PI)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    save(&a.picture(&gpu)?, "spawn-view")?;
    a.eye = Some(a.game.scene().overview_eye());
    save(&a.picture(&gpu)?, "overview")?;
    a.eye = None;
    Ok(shots.paths)
}

/// The two clients met in the arena Ann hosts, with a frame of each drawn at `size`.
fn meet(
    gpu: &Gpu,
    size: (u32, u32),
    make_files: &impl Fn() -> io::Result<GameFiles>,
) -> Result<(Seat, Seat), String> {
    let seat = |name: &str| -> Result<Seat, String> {
        let files = make_files().map_err(|e| e.to_string())?;
        let game = ArenaGame::new(
            files,
            ClientOptions {
                net: NetMode::Loopback,
                name: name.to_string(),
                port: 0,
                ..ClientOptions::default()
            },
        )?;
        Seat::new(game, gpu, size)
    };
    let (mut a, mut b) = (seat("Ann")?, seat("Bob")?);
    a.game.call("hostGame", &[])?;
    run_until(gpu, &mut a, &mut b, "the hosted game", |a, _| {
        a.game.in_game()
    })?;
    let port = a.game.hosted_address().ok_or("no hosted address")?.port();
    b.game.set_global("$Arena::Port", &port.to_string());
    b.game.call("showJoin", &[])?;
    run_until(gpu, &mut a, &mut b, "the list of games", |_, b| {
        b.game
            .global("$Arena::ServerRows")
            .parse::<u32>()
            .is_ok_and(|rows| rows > 0)
    })?;
    b.game.call("joinSelected", &[])?;
    run_until(gpu, &mut a, &mut b, "the second player", |a, b| {
        b.game.in_game() && a.game.scene().holders_drawn > 0
    })?;
    run(gpu, &mut [&mut a, &mut b], 1000)?;
    Ok((a, b))
}

/// The names over the players at each of `sizes` (the first the clients start at), into `dir` as `names-<w>x<h>.png`: Bob
/// a few steps ahead of Ann, his name over his head in her view (hers is never drawn in it), a chat line each in the box.
/// Returns the paths.
pub fn write_names(
    dir: &Path,
    sizes: &[(u32, u32)],
    make_files: impl Fn() -> io::Result<GameFiles>,
) -> Result<Vec<PathBuf>, String> {
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let first = *sizes.first().ok_or("no sizes")?;
    let (mut a, mut b) = meet(&gpu, first, &make_files)?;
    let mut shots = Shots::new(dir)?;
    place(&mut a.game, "Ann", [4.0, -16.0, 0.1], 0.0)?;
    place(&mut a.game, "Bob", [5.2, -9.5, 0.1], 3.4)?;
    a.game.call("say", &["Ready when you are"])?;
    b.game.call("say", &["Coming for you"])?;
    run(&gpu, &mut [&mut a, &mut b], 1200)?;
    for &size in sizes {
        a.game.host.surface_size(size);
        a.target = gpu.offscreen(size.0, size.1).map_err(|e| e.to_string())?;
        run(&gpu, &mut [&mut a, &mut b], 200)?;
        shots.save(&a.picture(&gpu)?, &format!("names-{}x{}", size.0, size.1))?;
    }
    Ok(shots.paths)
}

/// The HUD for review at `size`, into `dir` with `tag` in each name: Bob's view in play (the corner readout with his round trip
/// to Ann's server), then the pause menu over it and its Controls panel. Returns the paths.
pub fn write_hud(
    dir: &Path,
    size: (u32, u32),
    tag: &str,
    make_files: impl Fn() -> io::Result<GameFiles>,
) -> Result<Vec<PathBuf>, String> {
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let (mut a, mut b) = meet(&gpu, size, &make_files)?;
    let mut shots = Shots::new(dir)?;
    place(&mut a.game, "Ann", [4.6, -11.5, 0.1], 3.6)?;
    place(&mut a.game, "Bob", [4.0, -16.0, 0.1], 0.0)?;
    b.game.call("say", &["Ready when you are"])?;
    run(&gpu, &mut [&mut a, &mut b], 1500)?;
    shots.save(&b.picture(&gpu)?, &format!("fps-play-{tag}"))?;
    b.game.call("pauseKey", &["1"])?;
    run(&gpu, &mut [&mut a, &mut b], 200)?;
    shots.save(&b.picture(&gpu)?, &format!("fps-pause-{tag}"))?;
    b.game.call("showControls", &[])?;
    run(&gpu, &mut [&mut a, &mut b], 100)?;
    shots.save(&b.picture(&gpu)?, &format!("fps-controls-{tag}"))?;
    Ok(shots.paths)
}

/// Ann's camera on Bob: from `offset` (in the world) off his feet, looking at his middle (or at `aim` above his feet).
fn watch_bob(a: &mut Seat, offset: [f32; 3], aim: f32) {
    let scene = a.game.scene();
    let own = scene.own().map_or([0.0; 3], |o| o.position);
    let far = |p: &[f32; 3]| (p[0] - own[0]).powi(2) + (p[1] - own[1]).powi(2);
    let Some(bob) = scene
        .others()
        .into_iter()
        .map(|(position, _)| position)
        .max_by(|p, q| far(p).total_cmp(&far(q)))
    else {
        return;
    };
    let at = [bob[0], bob[1], bob[2] + aim];
    let mut eye = look_at(
        [at[0] + offset[0], at[1] + offset[1], at[2] + offset[2]],
        at,
    );
    eye.fov = Some(60);
    a.eye = Some(eye);
}

/// The main menu, the host choice and the join screen (with a game in its list) at each of `sizes`, written into `dir` as
/// `main-<w>x<h>.png`, `host-<w>x<h>.png` and `join-<w>x<h>.png`. Returns the paths.
pub fn write_menus(
    dir: &Path,
    sizes: &[(u32, u32)],
    make_files: impl Fn() -> io::Result<GameFiles>,
) -> Result<Vec<PathBuf>, String> {
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let first = *sizes.first().ok_or("no sizes")?;
    let seat = |name: &str| -> Result<Seat, String> {
        let game = ArenaGame::new(
            make_files().map_err(|e| e.to_string())?,
            ClientOptions {
                net: NetMode::Loopback,
                name: name.to_string(),
                port: 0,
                ..ClientOptions::default()
            },
        )?;
        Seat::new(game, &gpu, first)
    };
    let (mut a, mut b) = (seat("Ann")?, seat("Bob")?);
    a.game.call("hostGame", &["lan"])?;
    run_until(&gpu, &mut a, &mut b, "the hosted game", |a, _| {
        a.game.in_game()
    })?;
    let port = a.game.hosted_address().ok_or("no hosted address")?.port();
    b.game.set_global("$Arena::Port", &port.to_string());
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for &size in sizes {
        b.game.host.surface_size(size);
        b.target = gpu.offscreen(size.0, size.1).map_err(|e| e.to_string())?;
        for (screen, show) in [
            ("main", "showMainMenu"),
            ("host", "showHost"),
            ("join", "showJoin"),
        ] {
            b.game.call(show, &[])?;
            if screen == "join" {
                run_until(&gpu, &mut a, &mut b, "the list of games", |_, b| {
                    b.game
                        .global("$Arena::ServerRows")
                        .parse::<u32>()
                        .is_ok_and(|n| n > 0)
                })?;
            }
            run(&gpu, &mut [&mut a, &mut b], 64)?;
            let path = dir.join(format!("{screen}-{}x{}.png", size.0, size.1));
            write_png(&b.picture(&gpu)?, &path).map_err(|e| e.to_string())?;
            paths.push(path);
        }
    }
    Ok(paths)
}

/// The motion pictures at `size`, written into `dir`: Bob stepping sideways (left, then right), walking backward and jumping,
/// seen by a camera beside him; Bob looking up, level and down; Bob knocked out from the front (falling backward) and from behind
/// (falling forward), a few frames of each fall and the body lying still; Ann's first-person pistol at rest, then the frames
/// after a shot (the slide back, the kick, the muzzle flash); Bob firing toward the camera; the HUD while reloading; and the
/// pause menu. Returns the paths.
pub fn write_motion(
    dir: &Path,
    size: (u32, u32),
    make_files: impl Fn() -> io::Result<GameFiles>,
) -> Result<Vec<PathBuf>, String> {
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let (mut a, mut b) = meet(&gpu, size, &make_files)?;
    let mut shots = Shots::new(dir)?;
    let mut save = |frame: &Frame, name: &str| shots.save(frame, name);
    use std::f32::consts::PI;

    // Side steps and the walk backward: Bob faces Ann (south); the camera stands in front of him and to his side.
    place(&mut a.game, "Ann", [4.0, -16.0, 0.1], 0.0)?;
    place(&mut a.game, "Bob", [4.6, -11.5, 0.1], PI)?;
    run(&gpu, &mut [&mut a, &mut b], 800)?;
    for (key, name) in [
        ("$mvLeftAction", "strafe-left"),
        ("$mvRightAction", "strafe-right"),
        ("$mvBackwardAction", "walk-back"),
    ] {
        place(&mut a.game, "Bob", [4.6, -11.5, 0.1], PI)?;
        run(&gpu, &mut [&mut a, &mut b], 400)?;
        b.game.set_global(key, "1");
        run(&gpu, &mut [&mut a, &mut b], 350)?;
        for k in 1..=4 {
            watch_bob(&mut a, [-2.6, -3.4, 0.6], 1.0);
            save(&a.picture(&gpu)?, &format!("{name}-{k}"))?;
            run(&gpu, &mut [&mut a, &mut b], 96)?;
        }
        b.game.set_global(key, "0");
    }
    run(&gpu, &mut [&mut a, &mut b], 600)?;

    // A jump: Bob jumps running toward the camera's side, a few frames as he leaves the ground and tucks.
    place(&mut a.game, "Bob", [4.6, -11.5, 0.1], PI)?;
    run(&gpu, &mut [&mut a, &mut b], 400)?;
    b.game.set_global("$mvLeftAction", "1");
    run(&gpu, &mut [&mut a, &mut b], 300)?;
    b.game.call("jump", &["1"])?;
    for k in 1..=4 {
        run(&gpu, &mut [&mut a, &mut b], 96)?;
        watch_bob(&mut a, [-2.6, -3.4, 0.6], 1.0);
        save(&a.picture(&gpu)?, &format!("jump-{k}"))?;
    }
    b.game.call("jump", &["0"])?;
    b.game.set_global("$mvLeftAction", "0");
    run(&gpu, &mut [&mut a, &mut b], 800)?;

    // The look: Bob, side on to the camera, aims up, level and down.
    place(&mut a.game, "Bob", [4.6, -11.5, 0.1], PI)?;
    run(&gpu, &mut [&mut a, &mut b], 400)?;
    for (motion, name) in [
        ("-500", "look-up"),
        ("500", "look-level"),
        ("500", "look-down"),
    ] {
        b.game.call("look", &[motion])?;
        run(&gpu, &mut [&mut a, &mut b], 500)?;
        watch_bob(&mut a, [-4.5, 0.0, 0.4], 1.1);
        save(&a.picture(&gpu)?, name)?;
    }
    b.game.call("look", &["-500"])?;

    // Ann's first-person pistol at rest, then the frames after a shot.
    a.eye = None;
    place(&mut a.game, "Bob", [6.0, 8.0, 0.1], PI)?;
    place(&mut a.game, "Ann", [4.0, -16.0, 0.1], 0.0)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    save(&a.picture(&gpu)?, "fp-rest")?;
    let flashes = a.game.scene().flashes;
    a.game.call("fire", &["1"])?;
    run_until(&gpu, &mut a, &mut b, "the shot's flash", |a, _| {
        a.game.scene().flashes > flashes
    })?;
    save(&a.picture(&gpu)?, "fp-fire-0ms")?;
    for (ms, name) in [
        (16, "fp-fire-16ms"),
        (32, "fp-fire-48ms"),
        (48, "fp-fire-96ms"),
        (112, "fp-fire-208ms"),
    ] {
        run(&gpu, &mut [&mut a, &mut b], ms)?;
        save(&a.picture(&gpu)?, name)?;
    }
    a.game.call("fire", &["0"])?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;

    // Bob fires toward the camera: the flash and the kick in his hand.
    place(&mut a.game, "Bob", [4.6, -11.5, 0.1], PI)?;
    run(&gpu, &mut [&mut a, &mut b], 500)?;
    let flashes = a.game.scene().flashes;
    b.game.call("fire", &["1"])?;
    run_until(&gpu, &mut a, &mut b, "Bob's flash", |a, _| {
        a.game.scene().flashes > flashes
    })?;
    watch_bob(&mut a, [-2.2, -2.6, 0.5], 1.3);
    save(&a.picture(&gpu)?, "third-person-fire")?;
    // His arm at the recoil's height.
    run(&gpu, &mut [&mut a, &mut b], 32)?;
    watch_bob(&mut a, [-2.2, -2.6, 0.5], 1.3);
    save(&a.picture(&gpu)?, "third-person-recoil")?;
    b.game.call("fire", &["0"])?;
    run(&gpu, &mut [&mut a, &mut b], 32)?;
    a.eye = None;
    run(&gpu, &mut [&mut a, &mut b], 600)?;

    // The reload: the HUD while it runs.
    a.game.call("reloadKey", &["1"])?;
    run_until(&gpu, &mut a, &mut b, "the reload", |a, _| {
        a.game.global("$Arena::Reloading") == "1"
    })?;
    run(&gpu, &mut [&mut a, &mut b], 200)?;
    save(&a.picture(&gpu)?, "reload-hud")?;
    run(&gpu, &mut [&mut a, &mut b], 1500)?;

    // Knocked out from the front (Bob faces Ann: he falls backward), then from behind (he faces away: forward).
    for (yaw, name) in [(PI, "death-front"), (0.0, "death-behind")] {
        place(&mut a.game, "Ann", [4.0, -16.0, 0.1], 0.0)?;
        place(&mut a.game, "Bob", [4.0, -11.5, 0.1], yaw)?;
        run(&gpu, &mut [&mut a, &mut b], 500)?;
        let hits: u32 = a.game.global("$Arena::Hits").parse().unwrap_or(0);
        for k in 1..=3 {
            a.game.call("fire", &["1"])?;
            run(&gpu, &mut [&mut a, &mut b], 64)?;
            a.game.call("fire", &["0"])?;
            run_until(&gpu, &mut a, &mut b, "a hit", |a, _| {
                a.game.global("$Arena::Hits").parse::<u32>().unwrap_or(0) >= hits + k
            })?;
            run(&gpu, &mut [&mut a, &mut b], 100)?;
        }
        for (k, ms) in [(1, 0), (2, 200), (3, 200), (4, 250), (5, 600)] {
            run(&gpu, &mut [&mut a, &mut b], ms)?;
            watch_bob(&mut a, [4.6, 1.2, 0.9], 0.6);
            save(&a.picture(&gpu)?, &format!("{name}-{k}"))?;
        }
        a.eye = None;
        // Back on his feet after the knock-out's wait.
        run(&gpu, &mut [&mut a, &mut b], 2000)?;
        a.game.call("reloadKey", &["1"])?;
        run(&gpu, &mut [&mut a, &mut b], 1500)?;
    }

    // The pause menu over Ann's view.
    a.game.call("pauseKey", &["1"])?;
    run(&gpu, &mut [&mut a, &mut b], 200)?;
    save(&a.picture(&gpu)?, "pause-menu")?;
    a.game.call("resumeKey", &["1"])?;
    Ok(shots.paths)
}

/// Turns Ann's view `yaw` radians from where it looks, with the next move (as the mouse would).
fn turn_by(a: &mut Seat, yaw: f32) {
    let waiting = a.game.vm().global_f32("$mvYaw").unwrap_or(0.0);
    a.game.set_global("$mvYaw", &(waiting + yaw).to_string());
    let look = a.game.vm().global_f32("$Arena::LookYaw").unwrap_or(0.0);
    a.game
        .set_global("$Arena::LookYaw", &(look + yaw).to_string());
}

/// One shot of Ann's: the trigger down for a few frames, then up.
fn shoot(gpu: &Gpu, a: &mut Seat, b: &mut Seat) -> Result<(), String> {
    a.game.call("fire", &["1"])?;
    run(gpu, &mut [&mut *a, &mut *b], 64)?;
    a.game.call("fire", &["0"])?;
    run(gpu, &mut [&mut *a, &mut *b], 500)
}

/// The polish pictures at `size`, written into `dir`: Ann's first-person frames of a bolt fired into open space and of one
/// fired at a wall (a frame every [`FRAME_MS`] from the shot's flash), the holes bolts leave in a wall and in the floor, a hit
/// as the shooter sees it (the hit mark and the sparks) and as the one hit sees it (the hurt edge and the mark of where it came
/// from), and aiming down the sights: Ann's pistol at the hip, on its way up and at the sights, a bolt fired from there, and
/// Bob seen holding his pistol, then raising it to his eye. Returns the paths.
pub fn write_polish(
    dir: &Path,
    size: (u32, u32),
    make_files: impl Fn() -> io::Result<GameFiles>,
) -> Result<Vec<PathBuf>, String> {
    use std::f32::consts::PI;
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let (mut a, mut b) = meet(&gpu, size, &make_files)?;
    let mut shots = Shots::new(dir)?;
    let mut save = |frame: &Frame, name: &str| shots.save(frame, name);
    place(&mut a.game, "Bob", [-12.0, 12.0, 0.1], PI)?;

    // A bolt into open space (a little above level, over the far wall), then one at the cover ahead: frame by frame.
    for (at, pitch, name) in [
        ([4.0, -16.0, 0.1], -0.2, "bolt-open"),
        ([0.0, -18.0, 0.1], 0.05, "bolt-wall"),
    ] {
        place(&mut a.game, "Ann", at, 0.0)?;
        run(&gpu, &mut [&mut a, &mut b], 600)?;
        a.game.set_view_pitch(pitch);
        run(&gpu, &mut [&mut a, &mut b], 300)?;
        let flashes = a.game.scene().flashes;
        a.game.call("fire", &["1"])?;
        run_until(&gpu, &mut a, &mut b, "the shot's flash", |a, _| {
            a.game.scene().flashes > flashes
        })?;
        for k in 0..8 {
            save(&a.picture(&gpu)?, &format!("{name}-{k}"))?;
            run(&gpu, &mut [&mut a, &mut b], FRAME_MS)?;
        }
        a.game.call("fire", &["0"])?;
        run(&gpu, &mut [&mut a, &mut b], 600)?;
    }

    // Holes in a wall: a few shots at the cover's face three steps ahead; then in the floor.
    place(&mut a.game, "Ann", [0.0, -14.0, 0.1], 0.0)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    for (yaw, pitch) in [
        (0.0, -0.05),
        (0.12, 0.0),
        (-0.22, 0.05),
        (0.15, -0.17),
        (-0.2, -0.1),
    ] {
        turn_by(&mut a, yaw);
        a.game.set_view_pitch(pitch);
        run(&gpu, &mut [&mut a, &mut b], 100)?;
        shoot(&gpu, &mut a, &mut b)?;
    }
    a.game.set_view_pitch(0.0);
    turn_by(&mut a, 0.15);
    run(&gpu, &mut [&mut a, &mut b], 300)?;
    save(&a.picture(&gpu)?, "decals-wall")?;
    place(&mut a.game, "Ann", [-14.0, 0.0, 0.1], 0.0)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    a.game.set_view_pitch(0.9);
    run(&gpu, &mut [&mut a, &mut b], 200)?;
    for yaw in [0.0, 0.2, -0.35, 0.25] {
        turn_by(&mut a, yaw);
        run(&gpu, &mut [&mut a, &mut b], 100)?;
        shoot(&gpu, &mut a, &mut b)?;
    }
    turn_by(&mut a, -0.05);
    run(&gpu, &mut [&mut a, &mut b], 300)?;
    save(&a.picture(&gpu)?, "decals-floor")?;
    a.game.set_view_pitch(0.0);

    // A hit: Ann shoots Bob, who stands side on to her; Ann sees the mark and the sparks, Bob the hurt edge and where it came
    // from.
    place(&mut a.game, "Ann", [4.0, -16.0, 0.1], 0.0)?;
    place(&mut a.game, "Bob", [4.0, -11.0, 0.1], PI / 2.0)?;
    run(&gpu, &mut [&mut a, &mut b], 800)?;
    let hits: u32 = a.game.global("$Arena::Hits").parse().unwrap_or(0);
    a.game.call("fire", &["1"])?;
    run_until(&gpu, &mut a, &mut b, "a hit", |a, _| {
        a.game.global("$Arena::Hits").parse::<u32>().unwrap_or(0) > hits
    })?;
    run(&gpu, &mut [&mut a, &mut b], 32)?;
    save(&a.picture(&gpu)?, "hit-shooter-1")?;
    save(&b.picture(&gpu)?, "hit-victim-1")?;
    a.game.call("fire", &["0"])?;
    run(&gpu, &mut [&mut a, &mut b], 150)?;
    save(&a.picture(&gpu)?, "hit-shooter-2")?;
    save(&b.picture(&gpu)?, "hit-victim-2")?;
    run(&gpu, &mut [&mut a, &mut b], 2500)?;

    // Aiming down the sights: at the hip, on the way up, at the sights, a bolt fired from there.
    place(&mut a.game, "Bob", [-12.0, 12.0, 0.1], PI)?;
    place(&mut a.game, "Ann", [4.0, -16.0, 0.1], 0.0)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    save(&a.picture(&gpu)?, "aim-hip")?;
    a.game.call("aim", &["1"])?;
    run(&gpu, &mut [&mut a, &mut b], 80)?;
    save(&a.picture(&gpu)?, "aim-rising")?;
    run(&gpu, &mut [&mut a, &mut b], 400)?;
    save(&a.picture(&gpu)?, "aim-sights")?;
    let flashes = a.game.scene().flashes;
    a.game.call("fire", &["1"])?;
    run_until(&gpu, &mut a, &mut b, "the shot's flash", |a, _| {
        a.game.scene().flashes > flashes
    })?;
    for k in 0..4 {
        run(&gpu, &mut [&mut a, &mut b], FRAME_MS)?;
        save(&a.picture(&gpu)?, &format!("aim-fire-{k}"))?;
    }
    a.game.call("fire", &["0"])?;
    a.game.call("aim", &["0"])?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;

    // Bob holding his pistol, then raising it to his eye.
    place(&mut a.game, "Bob", [4.6, -11.5, 0.1], PI)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    watch_bob(&mut a, [-2.6, -3.4, 0.6], 1.3);
    save(&a.picture(&gpu)?, "aim-third-hold")?;
    b.game.call("aim", &["1"])?;
    run(&gpu, &mut [&mut a, &mut b], 500)?;
    watch_bob(&mut a, [-2.6, -3.4, 0.6], 1.3);
    save(&a.picture(&gpu)?, "aim-third-raised")?;
    watch_bob(&mut a, [-3.6, 0.4, 0.4], 1.3);
    save(&a.picture(&gpu)?, "aim-third-side")?;
    b.game.call("aim", &["0"])?;
    a.eye = None;
    Ok(shots.paths)
}

/// The muzzle's smoke after a shot, in the sun and in the cover's shadow, from Ann's eyes: `smoke-sun-unlit.png`,
/// `smoke-sun-lit.png`, `smoke-shade-unlit.png` and `smoke-shade-lit.png` (unlit as TGE draws particles, and lit by
/// [`ParticleLight::BLENDED`](torque_render_scene::hd::particle_light::ParticleLight::BLENDED)) at `size` in `dir`, each with a close crop of the muzzle (`-crop`). Returns the paths.
pub fn write_lit_smoke(
    dir: &Path,
    size: (u32, u32),
    make_files: impl Fn() -> io::Result<GameFiles>,
) -> Result<Vec<PathBuf>, String> {
    use torque_render_scene::hd::particle_light::ParticleLight;
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let (mut a, mut b) = meet(&gpu, size, &make_files)?;
    let mut shots = Shots::new(dir)?;
    place(&mut a.game, "Bob", [-12.0, 12.0, 0.1], std::f32::consts::PI)?;
    // The shade: just out of the cover (x -0.5..0.5, y -11..-7, 2.4 high) on its side away from the sun, facing across it.
    let sun = a.view.scene().sun.towards;
    let n = sun[0].hypot(sun[1]).max(1e-3);
    let g = [-sun[0] / n, -sun[1] / n];
    let t = [
        (0.5 + 0.45) / g[0].abs().max(1e-3),
        (2.0 + 0.45) / g[1].abs().max(1e-3),
    ];
    let t = t[0].min(t[1]);
    let shade = [g[0] * t, -9.0 + g[1] * t, 0.1];
    for (spot, at, yaw) in [
        ("sun", [4.0, -16.0, 0.1], 0.0),
        (
            "shade",
            shade,
            g[0].atan2(g[1]) + std::f32::consts::FRAC_PI_2,
        ),
    ] {
        place(&mut a.game, "Ann", at, yaw)?;
        run(&gpu, &mut [&mut a, &mut b], 600)?;
        a.game.set_view_pitch(-0.12);
        run(&gpu, &mut [&mut a, &mut b], 300)?;
        let flashes = a.game.scene().flashes;
        a.game.call("fire", &["1"])?;
        run_until(&gpu, &mut a, &mut b, "the shot's flash", |a, _| {
            a.game.scene().flashes > flashes
        })?;
        run(&gpu, &mut [&mut a, &mut b], 200)?;
        for (look, rule) in [("unlit", None), ("lit", Some(ParticleLight::BLENDED))] {
            a.view.set_particle_light(rule);
            let frame = a.picture(&gpu)?;
            shots.save(&frame, &format!("smoke-{spot}-{look}"))?;
            shots.save(
                &zoomed(&frame, size.0 / 3, size.1 / 3, 3),
                &format!("smoke-{spot}-{look}-crop"),
            )?;
        }
        a.view.set_particle_light(None);
        a.game.call("fire", &["0"])?;
        run(&gpu, &mut [&mut a, &mut b], 1500)?;
    }
    Ok(shots.paths)
}

/// Turns Ann's view to look at `target` (with the next move, as the mouse would).
fn look_at_point(a: &mut Seat, target: [f32; 3]) {
    let Some(own) = a.game.scene().own() else {
        return;
    };
    let d = [0, 1, 2].map(|k| target[k] - own.eye[k]);
    let yaw = d[0].atan2(d[1]);
    let pitch = -d[2].atan2(d[0].hypot(d[1]));
    let mut turn = yaw - own.yaw;
    while turn > std::f32::consts::PI {
        turn -= std::f32::consts::TAU;
    }
    while turn < -std::f32::consts::PI {
        turn += std::f32::consts::TAU;
    }
    turn_by(a, turn);
    a.game.set_view_pitch(pitch);
}

/// A crop of `frame` around its center, `w` by `h` pixels, scaled up `zoom` times (nearest), for a close look.
fn zoomed(frame: &Frame, w: u32, h: u32, zoom: u32) -> Frame {
    let (x0, y0) = ((frame.width - w) / 2, (frame.height - h) / 2);
    let (zw, zh) = (w * zoom, h * zoom);
    let mut rgba = Vec::with_capacity((zw * zh * 4) as usize);
    for y in 0..zh {
        for x in 0..zw {
            let i = 4 * ((y0 + y / zoom) * frame.width + x0 + x / zoom) as usize;
            rgba.extend_from_slice(&frame.rgba[i..i + 4]);
        }
    }
    Frame {
        width: zw,
        height: zh,
        rgba,
    }
}

/// The second round of polish pictures at `size`, written into `dir`'s `ribbon-flash`, `decals2` and `sights` folders: Ann's
/// first-person frames 0, 16, 32, 64, 128, 256 and 512 ms after a shot (the flares and the smoke); Bob's bolt and its ribbon
/// seen from the side, into the open sky and into a wall; bolt holes in the middle of a wall, at a block's edge, in the
/// floor and on a wall in shadow, close and from a few steps off; and Ann aimed down her sights, with a close crop of them. Returns the paths.
pub fn write_polish2(
    dir: &Path,
    size: (u32, u32),
    make_files: impl Fn() -> io::Result<GameFiles>,
) -> Result<Vec<PathBuf>, String> {
    use std::f32::consts::PI;
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let (mut a, mut b) = meet(&gpu, size, &make_files)?;
    let mut paths = Vec::new();
    let mut save = |frame: &Frame, sub: &str, name: &str| -> Result<(), String> {
        let folder = dir.join(sub);
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let path = folder.join(format!("{name}.png"));
        write_png(frame, &path).map_err(|e| e.to_string())?;
        paths.push(path);
        Ok(())
    };
    place(&mut a.game, "Bob", [-12.0, 12.0, 0.1], PI)?;

    // The flares and the smoke: Ann fires into the open, a little above level.
    place(&mut a.game, "Ann", [4.0, -16.0, 0.1], 0.0)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    a.game.set_view_pitch(-0.12);
    run(&gpu, &mut [&mut a, &mut b], 300)?;
    let flashes = a.game.scene().flashes;
    a.game.call("fire", &["1"])?;
    run_until(&gpu, &mut a, &mut b, "the shot's flash", |a, _| {
        a.game.scene().flashes > flashes
    })?;
    let mut at = 0;
    for ms in [0, 16, 32, 64, 128, 256, 512] {
        run(&gpu, &mut [&mut a, &mut b], ms - at)?;
        at = ms;
        save(
            &a.picture(&gpu)?,
            "ribbon-flash",
            &format!("fp-fire-{ms:03}ms"),
        )?;
    }
    a.game.call("fire", &["0"])?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;

    // Bob's bolt from the side: into the open sky (aimed up over the far wall), then into the wall ahead of him.
    for (bob, pitch, camera, look, name) in [
        (
            [-6.0, -10.0, 0.1],
            -0.25,
            [2.0, -17.0, 2.5],
            [2.0, -10.0, 3.2],
            "ribbon-sky",
        ),
        (
            [10.0, -10.0, 0.1],
            0.0,
            [16.0, -15.5, 2.2],
            [17.0, -10.0, 1.8],
            "ribbon-wall",
        ),
    ] {
        place(&mut a.game, "Bob", bob, PI / 2.0)?;
        run(&gpu, &mut [&mut a, &mut b], 600)?;
        b.game.set_view_pitch(pitch);
        run(&gpu, &mut [&mut a, &mut b], 300)?;
        let mut eye = look_at(camera, look);
        eye.fov = Some(70);
        a.eye = Some(eye);
        b.game.call("fire", &["1"])?;
        run_until(&gpu, &mut a, &mut b, "a bolt in flight", |a, _| {
            a.game.scene().bolts_drawn > 0
        })?;
        b.game.call("fire", &["0"])?;
        for k in 1..=4 {
            run(&gpu, &mut [&mut a, &mut b], 32)?;
            save(&a.picture(&gpu)?, "ribbon-flash", &format!("{name}-{k}"))?;
        }
        a.eye = None;
        b.game.set_view_pitch(0.0);
        run(&gpu, &mut [&mut a, &mut b], 600)?;
    }
    place(&mut a.game, "Bob", [-12.0, 12.0, 0.1], PI)?;

    // Bolt holes: on the cover's face south of the middle (x -0.5..0.5, its face at y = -11, its top at z = 2.4): in the
    // middle, at its right edge and at its top edge; then in the floor.
    place(&mut a.game, "Ann", [0.3, -13.4, 0.1], 0.0)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    for target in [
        [0.0, -11.0, 1.4],
        [-0.2, -11.0, 1.0],
        [0.47, -11.0, 1.7],
        [0.49, -11.0, 1.2],
        [0.1, -11.0, 2.38],
        [-0.3, -11.0, 2.36],
    ] {
        look_at_point(&mut a, target);
        run(&gpu, &mut [&mut a, &mut b], 150)?;
        shoot(&gpu, &mut a, &mut b)?;
    }
    look_at_point(&mut a, [0.1, -11.0, 1.7]);
    run(&gpu, &mut [&mut a, &mut b], 300)?;
    save(&a.picture(&gpu)?, "decals2", "wall-close")?;
    place(&mut a.game, "Ann", [3.5, -18.0, 0.1], 0.0)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    look_at_point(&mut a, [0.0, -11.0, 1.6]);
    run(&gpu, &mut [&mut a, &mut b], 300)?;
    save(&a.picture(&gpu)?, "decals2", "wall-play")?;
    // The block's edge seen from the side: the holes stop at it.
    place(&mut a.game, "Ann", [2.2, -12.4, 0.1], 0.0)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    look_at_point(&mut a, [0.45, -11.0, 1.8]);
    run(&gpu, &mut [&mut a, &mut b], 300)?;
    save(&a.picture(&gpu)?, "decals2", "edge-close")?;
    place(&mut a.game, "Ann", [-14.0, 0.0, 0.1], 0.0)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    for target in [[-14.0, 1.6, 0.0], [-13.6, 1.9, 0.0], [-14.4, 1.5, 0.0]] {
        look_at_point(&mut a, target);
        run(&gpu, &mut [&mut a, &mut b], 150)?;
        shoot(&gpu, &mut a, &mut b)?;
    }
    look_at_point(&mut a, [-14.0, 1.7, 0.0]);
    run(&gpu, &mut [&mut a, &mut b], 300)?;
    save(&a.picture(&gpu)?, "decals2", "floor-close")?;
    place(&mut a.game, "Ann", [-14.0, -4.0, 0.1], 0.0)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    look_at_point(&mut a, [-14.0, 1.7, 0.0]);
    run(&gpu, &mut [&mut a, &mut b], 300)?;
    save(&a.picture(&gpu)?, "decals2", "floor-play")?;
    // The same cover's east face (x = 0.5, y -11..-7), in the sun's shadow.
    a.game.call("reloadKey", &["1"])?;
    place(&mut a.game, "Ann", [2.6, -10.2, 0.1], 0.0)?;
    run(&gpu, &mut [&mut a, &mut b], 2000)?;
    for target in [[0.5, -9.0, 1.4], [0.5, -9.6, 1.0], [0.5, -8.4, 1.8]] {
        look_at_point(&mut a, target);
        run(&gpu, &mut [&mut a, &mut b], 150)?;
        shoot(&gpu, &mut a, &mut b)?;
    }
    look_at_point(&mut a, [0.5, -9.0, 1.4]);
    run(&gpu, &mut [&mut a, &mut b], 300)?;
    save(&a.picture(&gpu)?, "decals2", "shadow-close")?;
    a.game.set_view_pitch(0.0);
    run(&gpu, &mut [&mut a, &mut b], 2500)?;

    // Aimed down the sights, and a close crop of them.
    place(&mut a.game, "Ann", [4.0, -16.0, 0.1], 0.0)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    a.game.call("aim", &["1"])?;
    run(&gpu, &mut [&mut a, &mut b], 500)?;
    let frame = a.picture(&gpu)?;
    let tag = format!("{}x{}", size.0, size.1);
    save(&frame, "sights", &format!("aimed-{tag}"))?;
    save(
        &zoomed(&frame, size.0 / 8, size.1 / 8, 4),
        "sights",
        &format!("aimed-{tag}-crop"),
    )?;
    a.game.call("aim", &["0"])?;
    Ok(paths)
}

/// Ann aimed down her sights at `size`, and a close crop of them, into `dir`'s `sights` folder. Returns the paths.
pub fn write_sights(
    dir: &Path,
    size: (u32, u32),
    make_files: impl Fn() -> io::Result<GameFiles>,
) -> Result<Vec<PathBuf>, String> {
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let (mut a, mut b) = meet(&gpu, size, &make_files)?;
    place(&mut a.game, "Bob", [-12.0, 12.0, 0.1], std::f32::consts::PI)?;
    place(&mut a.game, "Ann", [4.0, -16.0, 0.1], 0.0)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    a.game.call("aim", &["1"])?;
    run(&gpu, &mut [&mut a, &mut b], 500)?;
    let folder = dir.join("sights");
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let frame = a.picture(&gpu)?;
    let tag = format!("{}x{}", size.0, size.1);
    let mut paths = Vec::new();
    for (f, name) in [
        (frame.clone(), format!("aimed-{tag}")),
        (
            zoomed(&frame, size.0 / 8, size.1 / 8, 4),
            format!("aimed-{tag}-crop"),
        ),
    ] {
        let path = folder.join(format!("{name}.png"));
        write_png(&f, &path).map_err(|e| e.to_string())?;
        paths.push(path);
    }
    Ok(paths)
}

/// Bob's figure on the server Ann hosts.
fn bob_figure(a: &mut Seat) -> Result<ObjId, String> {
    let server = a.game.server.as_mut().ok_or("no hosted server")?;
    server
        .call("figureOf", &["Bob"])?
        .parse()
        .map_err(|_| "Bob has no figure".to_string())
}

/// Bob's figure where Ann's last frame drew it (the other figure farthest from her own).
fn bob_drawn(a: &Seat) -> Option<[f32; 3]> {
    let scene = a.game.scene();
    let own = scene.own().map_or([0.0; 3], |o| o.position);
    let far = |p: &[f32; 3]| (p[0] - own[0]).powi(2) + (p[1] - own[1]).powi(2);
    scene
        .others()
        .into_iter()
        .map(|(position, _)| position)
        .max_by(|p, q| far(p).total_cmp(&far(q)))
}

/// Three numbers of a script's answer.
fn floats3(text: &str) -> [f32; 3] {
    let mut v = text.split_whitespace().map(|w| w.parse().unwrap_or(0.0));
    std::array::from_fn(|_| v.next().unwrap_or(0.0))
}

/// A picture of Ann's view with Bob's hitboxes drawn over it ([`hitbox_view`]), seen from `offset` off his feet. The boxes are
/// the server's last step run on along Bob's velocity to the time the client draws him at: how far along the velocity the drawn
/// figure is ahead of the server's (at most two steps; one step when he moves only up or down). Returns that lead (ms) and how
/// far the drawn figure is from the boxes' figure then.
fn hitbox_picture(
    gpu: &Gpu,
    a: &mut Seat,
    shots: &mut Shots,
    offset: [f32; 3],
    name: &str,
) -> Result<(f32, [f32; 3]), String> {
    watch_bob(a, offset, 1.0);
    let eye = a.eye.ok_or("no eye")?;
    let mut frame = a.picture(gpu)?;
    let drawn = bob_drawn(a).ok_or("Bob is not drawn")?;
    let bob = bob_figure(a)?;
    let server = a.game.server.as_mut().ok_or("no hosted server")?;
    let mut ask = |what: &str| -> Result<[f32; 3], String> {
        let text = server
            .vm_mut()
            .eval(format!("return {bob}.{what}();").as_bytes())
            .map_err(|e| e.to_string())?;
        Ok(floats3(&String::from_utf8_lossy(&text)))
    };
    let (at, velocity) = (ask("getPosition")?, ask("getVelocity")?);
    let ahead: [f32; 3] = std::array::from_fn(|k| drawn[k] - at[k]);
    let level = velocity[0] * velocity[0] + velocity[1] * velocity[1];
    let step = STEP_MS as f32 / 1000.0;
    let lead = if level > 0.25 {
        ((ahead[0] * velocity[0] + ahead[1] * velocity[1]) / level).clamp(0.0, 2.0 * step)
    } else if velocity[2].abs() > 0.5 {
        step
    } else {
        0.0
    };
    let by: [f32; 3] = std::array::from_fn(|k| velocity[k] * lead);
    let mut boxes = hitbox_view::part_boxes(server, bob);
    if boxes.is_empty() {
        return Err("Bob has no hitboxes".to_string());
    }
    for corner in boxes.iter_mut().flat_map(|b| b.corners.iter_mut()) {
        for k in 0..3 {
            corner[k] += by[k];
        }
    }
    hitbox_view::draw(&mut frame, eye, &boxes);
    shots.save(&frame, name)?;
    Ok((lead * 1000.0, std::array::from_fn(|k| ahead[k] - by[k])))
}

/// The hitbox pictures at `size`, written into `dir`: Bob's hitboxes as the server poses them, drawn over him standing (from
/// the front and the side), running, stepping to the side, looking up and down, jumping, aiming down the sights and falling
/// knocked out. Returns the paths.
pub fn write_hitboxes(
    dir: &Path,
    size: (u32, u32),
    make_files: impl Fn() -> io::Result<GameFiles>,
) -> Result<Vec<PathBuf>, String> {
    use std::f32::consts::PI;
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let (mut a, mut b) = meet(&gpu, size, &make_files)?;
    let mut shots = Shots::new(dir)?;
    let mut offsets = Vec::new();
    let front = [-1.8, -4.4, 0.2];
    let side = [-4.8, 0.0, 0.2];
    place(&mut a.game, "Ann", [4.0, -16.0, 0.1], 0.0)?;

    place(&mut a.game, "Bob", [4.6, -11.5, 0.1], PI)?;
    run(&gpu, &mut [&mut a, &mut b], 800)?;
    let off = hitbox_picture(&gpu, &mut a, &mut shots, front, "stand-front")?;
    offsets.push(("stand-front".to_string(), off));
    let off = hitbox_picture(&gpu, &mut a, &mut shots, side, "stand-side")?;
    offsets.push(("stand-side".to_string(), off));

    for (key, name) in [("$mvForwardAction", "run"), ("$mvLeftAction", "side-step")] {
        place(&mut a.game, "Bob", [4.6, -2.0, 0.1], PI)?;
        run(&gpu, &mut [&mut a, &mut b], 500)?;
        b.game.set_global(key, "1");
        run(&gpu, &mut [&mut a, &mut b], 400)?;
        for k in 1..=2 {
            let off = hitbox_picture(&gpu, &mut a, &mut shots, side, &format!("{name}-{k}"))?;
            offsets.push((format!("{name}-{k}"), off));
            run(&gpu, &mut [&mut a, &mut b], 160)?;
        }
        b.game.set_global(key, "0");
    }

    place(&mut a.game, "Bob", [4.6, -11.5, 0.1], PI)?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    for (motion, name) in [("-500", "look-up"), ("1000", "look-down")] {
        b.game.call("look", &[motion])?;
        run(&gpu, &mut [&mut a, &mut b], 500)?;
        let off = hitbox_picture(&gpu, &mut a, &mut shots, side, name)?;
        offsets.push((String::from(name), off));
    }
    b.game.call("look", &["-500"])?;

    place(&mut a.game, "Bob", [4.6, -6.0, 0.1], PI)?;
    run(&gpu, &mut [&mut a, &mut b], 500)?;
    b.game.set_global("$mvForwardAction", "1");
    run(&gpu, &mut [&mut a, &mut b], 300)?;
    b.game.call("jump", &["1"])?;
    for k in 1..=2 {
        run(&gpu, &mut [&mut a, &mut b], 128)?;
        let off = hitbox_picture(&gpu, &mut a, &mut shots, side, &format!("jump-{k}"))?;
        offsets.push((format!("jump-{k}"), off));
    }
    b.game.call("jump", &["0"])?;
    b.game.set_global("$mvForwardAction", "0");
    run(&gpu, &mut [&mut a, &mut b], 800)?;

    place(&mut a.game, "Bob", [4.6, -11.5, 0.1], PI)?;
    b.game.call("aim", &["1"])?;
    run(&gpu, &mut [&mut a, &mut b], 600)?;
    let off = hitbox_picture(&gpu, &mut a, &mut shots, front, "aim-front")?;
    offsets.push(("aim-front".to_string(), off));
    let off = hitbox_picture(&gpu, &mut a, &mut shots, side, "aim-side")?;
    offsets.push(("aim-side".to_string(), off));
    b.game.call("aim", &["0"])?;
    run(&gpu, &mut [&mut a, &mut b], 400)?;

    // Knocked out from the front: he falls backward.
    let server = a.game.server.as_mut().ok_or("no hosted server")?;
    server
        .vm_mut()
        .eval(b"ArenaFigure.hurt(figureOf(\"Bob\"), \"\", 100, \"\");")
        .map_err(|e| e.to_string())?;
    for (k, ms) in [(1, 160), (2, 160), (3, 600)] {
        run(&gpu, &mut [&mut a, &mut b], ms)?;
        let off = hitbox_picture(&gpu, &mut a, &mut shots, side, &format!("fall-{k}"))?;
        offsets.push((format!("fall-{k}"), off));
    }
    a.eye = None;
    // A line a picture: the boxes' lead (ms), and how far the drawn figure is from the boxes' (x, y, z).
    let lines: String = offsets
        .iter()
        .map(|(name, (lead, d))| format!("{name} {lead:.0} {:.3} {:.3} {:.3}\n", d[0], d[1], d[2]))
        .collect();
    let path = dir.join("offsets.txt");
    std::fs::write(&path, lines).map_err(|e| e.to_string())?;
    shots.paths.push(path);
    Ok(shots.paths)
}
