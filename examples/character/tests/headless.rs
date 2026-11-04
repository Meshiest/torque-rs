//! The example without a window: the keys through the action map drive the state machine through idle, walk, run and jump;
//! the figure's pose differs between idle and walk; and frames drawn offscreen show it (skipped with a note on a machine with
//! no GPU adapter).

use torque::shapes::math::mul_p;
use torque_example_character::game::{CharacterGame, keys};
use torque_example_character::motion::State;
use torque_example_character::shots::{FRAME_MS, Step, picture_camera, play};
use torque_render_core::Gpu;

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

    game.key(keys::W, true);
    run(&mut game, 400);
    assert_eq!(game.figure().state(), State::Walk);
    let walk = game.figure().sequences().walk;
    let thread = game.figure().thread();
    assert_eq!(thread.sequence, Some(walk));
    assert!(
        !thread.transition.in_transition,
        "the blend lasts a quarter second"
    );
    let walk_foot = left_foot(&game);
    let moved = (0..3)
        .map(|k| (walk_foot[k] - idle_foot[k]).abs())
        .fold(0.0, f32::max);
    assert!(
        moved > 0.05,
        "the walk swings the leg: {idle_foot:?} {walk_foot:?}"
    );
    assert!(
        game.figure().position[1] > 0.3,
        "{:?}",
        game.figure().position
    );

    game.key(keys::SHIFT, true);
    run(&mut game, 200);
    let speed = game.figure().ground_speed();
    assert!(speed > 1.6, "speeding up: {speed}");
    run(&mut game, 600);
    assert_eq!(game.figure().state(), State::Run);

    game.key(keys::SPACE, true);
    run(&mut game, 100);
    assert_eq!(game.figure().state(), State::Jump);
    assert!(game.figure().position[2] > 0.2);
    run(&mut game, 600);
    assert_eq!(game.figure().state(), State::Run, "landed running");
    assert_eq!(game.figure().position[2], 0.0);
    game.key(keys::SPACE, false);

    game.key(keys::SHIFT, false);
    game.key(keys::W, false);
    run(&mut game, 1500);
    assert_eq!(game.figure().state(), State::Idle);
}

#[test]
fn a_transition_blends_the_pose() {
    let mut game = CharacterGame::new().unwrap();
    run(&mut game, 300);
    game.key(keys::W, true);
    game.step(FRAME_MS);
    game.step(FRAME_MS);
    assert_eq!(game.figure().state(), State::Walk);
    assert!(game.figure().thread().transition.in_transition);
}

#[test]
fn frames_drawn_offscreen_show_the_figure_move() {
    let gpu = match Gpu::new() {
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
        Step::Key(keys::D, true),
        Step::Wait(320),
        Step::Shot("walk"),
    ];
    let shots = play(&mut game, &gpu, &mut view, (320, 180), &script).unwrap();
    assert_eq!(game.figure().state(), State::Walk);
    let differ = |a: usize, b: usize| {
        shots[a]
            .1
            .rgba
            .chunks_exact(4)
            .zip(shots[b].1.rgba.chunks_exact(4))
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
fn the_pistol_key_puts_the_pistol_in_the_raised_hand() {
    let mut game = CharacterGame::new().unwrap();
    run(&mut game, 100);
    assert!(game.held_pistol().is_none());
    game.key(keys::F, true);
    game.key(keys::F, false);
    run(&mut game, 100);
    assert!(game.figure().holding());
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
    game.key(keys::W, true);
    run(&mut game, 500);
    assert_eq!(game.figure().state(), State::Walk);
    let ahead = game.held_pistol().unwrap().mul_v([0.0, 1.0, 0.0]);
    assert!(ahead[1] > 0.98, "{ahead:?}");
    game.key(keys::W, false);
    game.key(keys::F, true);
    game.key(keys::F, false);
    run(&mut game, 50);
    assert!(!game.figure().holding() && game.held_pistol().is_none());
}
