//! The example without a window: the keys through the action map drive the state machine through idle, walk, run and jump;
//! the figure's pose differs between idle and walk; and frames drawn offscreen show it (skipped with a note on a machine with
//! no GPU adapter).

use torque::app::run::OffscreenRunnerPlugin;
use torque::client::input::Key;
use torque::engine::EngineBuilder;
use torque::shapes::math::mul_p;
use torque_example_character::game::{CharacterGame, CharacterPlugin};
use torque_example_character::motion::State;
use torque_example_character::shots::{FRAME_MS, Step, picture_camera, play};

/// Steps the game `ms` without drawing.
fn run(game: &mut CharacterGame, ms: u32) {
    for _ in 0..ms.div_ceil(FRAME_MS) {
        game.step(FRAME_MS);
    }
}

/// Where the left foot is in the figure's own space.
fn left_foot(game: &CharacterGame) -> [f32; 3] {
    let shape = torque::shapes::shape::Shape::for_server(torque_example_assets::shapes::figure());
    let n = shape.find_name(b"knee_l").unwrap();
    let node = shape.nodes.iter().position(|x| x.name == n).unwrap();
    mul_p(
        &game.figure().instance().node_transforms()[node],
        [0.0, 0.0, -torque_example_assets::shapes::SHIN],
    )
}

#[test]
fn the_keys_walk_run_and_jump_the_figure_and_it_lands() {
    let mut game = CharacterGame::new().unwrap();
    run(&mut game, 500);
    assert_eq!(game.figure().state(), State::Idle);
    let idle_foot = left_foot(&game);

    game.key(Key::W, true);
    run(&mut game, 400);
    assert_eq!(game.figure().state(), State::Walk);
    let walk = game.figure().sequences().walk;
    let thread = game.figure().thread();
    assert_eq!(thread.sequence, Some(walk));
    assert!(
        !thread.transition.in_transition,
        "the blend lasts a quarter second"
    );
    // Over one loop of the walk (any one moment may find the foot passing under the hips).
    let mut moved = 0.0f32;
    for _ in 0..(torque_example_assets::gait::WALK.duration * 1000.0) as u32 / FRAME_MS {
        run(&mut game, FRAME_MS);
        let walk_foot = left_foot(&game);
        moved = (0..3)
            .map(|k| (walk_foot[k] - idle_foot[k]).abs())
            .fold(moved, f32::max);
    }
    assert!(moved > 0.2, "the walk swings the leg: {moved}");
    assert!(
        game.figure().position[1] > 0.3,
        "{:?}",
        game.figure().position
    );

    game.key(Key::ShiftLeft, true);
    run(&mut game, 200);
    let speed = game.figure().ground_speed();
    assert!(
        speed > torque_example_assets::shapes::WALK_SPEED + 0.3,
        "speeding up: {speed}"
    );
    run(&mut game, 600);
    assert_eq!(game.figure().state(), State::Run);

    game.key(Key::Space, true);
    run(&mut game, 100);
    assert_eq!(game.figure().state(), State::Jump);
    assert!(game.figure().position[2] > 0.2);
    run(&mut game, 600);
    assert_eq!(game.figure().state(), State::Run, "landed running");
    assert_eq!(game.figure().position[2], 0.0);
    game.key(Key::Space, false);

    game.key(Key::ShiftLeft, false);
    game.key(Key::W, false);
    run(&mut game, 1500);
    assert_eq!(game.figure().state(), State::Idle);
}

#[test]
fn a_transition_blends_the_pose() {
    let mut game = CharacterGame::new().unwrap();
    run(&mut game, 300);
    game.key(Key::W, true);
    game.step(FRAME_MS);
    game.step(FRAME_MS);
    assert_eq!(game.figure().state(), State::Walk);
    assert!(game.figure().thread().transition.in_transition);
}

#[test]
fn frames_drawn_offscreen_show_the_figure_move() {
    let gpu = match torque::app::window::offscreen_gpu() {
        Ok(gpu) => gpu,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let mut game = CharacterGame::new().unwrap();
    picture_camera(&mut game);
    let mut view = game.view(&gpu);
    let script = [
        Step::Wait(160),
        Step::Shot("idle"),
        Step::Wait(160),
        Step::Shot("idle again"),
        Step::Key(Key::D, true),
        Step::Wait(320),
        Step::Shot("walk"),
    ];
    let shots = play(&mut game, &gpu, &mut view, (320, 180), &script).unwrap();
    assert_eq!(game.figure().state(), State::Walk);
    let differ = |a: usize, b: usize| {
        shots[a]
            .1
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(shots[b].1.rgba.as_chunks::<4>().0)
            .filter(|(p, q)| (0..3).any(|k| p[k].abs_diff(q[k]) > 24))
            .count()
    };
    let still = differ(0, 1);
    let moving = differ(1, 2);
    assert!(moving > 200, "walking changes the picture: {moving} pixels");
    assert!(
        moving > 4 * still.max(1),
        "standing changes it little: {still} vs {moving}"
    );
    // A sky above and a lit floor below: neither black.
    let rgba = &shots[2].1.rgba;
    let lum = |y: u32| {
        let i = 4 * (y * 320 + 160) as usize;
        u32::from(rgba[i]) + u32::from(rgba[i + 1]) + u32::from(rgba[i + 2])
    };
    assert!(lum(40) > 150 && lum(170) > 150, "{} {}", lum(40), lum(170));
}

#[test]
fn main_cs_sets_a_far_view() {
    let game = CharacterGame::new().unwrap();
    let d = game.daylight();
    assert_eq!((d.fog_start, d.visible), (1500.0, 3000.0));
    assert!(torque_example_character::game::FLOOR_HALF > d.visible);
}

#[test]
fn the_pistol_key_puts_the_pistol_in_the_raised_hand() {
    let mut game = CharacterGame::new().unwrap();
    run(&mut game, 100);
    assert!(game.held_pistol().is_none());
    game.key(Key::F, true);
    game.key(Key::F, false);
    run(&mut game, 300);
    assert!(game.figure().holding() && game.figure().hold_weight() == 1.0);
    let held = game.held_pistol().unwrap();
    // The barrel points the way the figure faces (+y), level, at about shoulder height in front of it.
    let ahead = held.mul_v([0.0, 1.0, 0.0]);
    assert!(ahead[1] > 0.98, "{ahead:?}");
    let muzzle = held.mul_p([0.0, 0.205, 0.033]);
    assert!(
        muzzle[1] > 0.8 && (muzzle[2] - 1.55).abs() < 0.2,
        "{muzzle:?}"
    );
    // Walking keeps the arm up.
    game.key(Key::W, true);
    run(&mut game, 500);
    assert_eq!(game.figure().state(), State::Walk);
    let ahead = game.held_pistol().unwrap().mul_v([0.0, 1.0, 0.0]);
    assert!(ahead[1] > 0.98, "{ahead:?}");
    game.key(Key::W, false);
    game.key(Key::F, true);
    game.key(Key::F, false);
    run(&mut game, 50);
    assert!(
        game.figure().holding() && game.figure().hold_weight() < 1.0,
        "the arm goes down over a moment"
    );
    run(&mut game, 300);
    assert!(!game.figure().holding() && game.held_pistol().is_none());
}

/// The corner readout once frames were drawn 60 a second (the host counts them as the window does): the frame rate.
#[test]
fn the_corner_readout_holds_the_frame_rate() {
    let mut game = CharacterGame::new().unwrap();
    for i in 0..60 {
        game.host.frame_drawn(f64::from(i) * 1000.0 / 60.0);
        game.step(16);
    }
    game.step(300);
    let text = game
        .call("eval", &["return CharacterStats.getValue();"])
        .unwrap();
    assert_eq!(text, "60 fps");
}

#[test]
fn the_offscreen_runner_plugin_draws_the_game_to_a_picture() {
    if let Err(e) = torque::app::window::offscreen_gpu() {
        eprintln!("skipped: {e}");
        return;
    }
    let picture = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("runner/character.png");
    let _ = std::fs::remove_file(&picture);
    EngineBuilder::new()
        .add_plugins((
            OffscreenRunnerPlugin::new((320, 180), 300).picture(&picture),
            CharacterPlugin,
        ))
        .run()
        .expect("the offscreen run");
    let png = std::fs::read(&picture).expect("the picture was written");
    assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    let size = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
    assert_eq!((size(16), size(20)), (320, 180));
    // A drawn scene (sky, floor, figure) does not compress to a flat color's few hundred bytes.
    assert!(png.len() > 3000, "{} bytes", png.len());
}
