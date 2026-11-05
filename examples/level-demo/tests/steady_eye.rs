//! The steady first-person view (the engine's own `GhostOptions::steady_eye`, on in the demo): the figure runs and walks
//! with its bobbing gaits while the eye its view stands at stays level over the ground, and the drawn body's `eye` node
//! (TGE's eye) still bobs. No window and no GPU: the client and the server in one process, the frames placed and not drawn.

use torque::client::input::Key;
use torque_example_level_demo::client::LevelDemo;
use torque_example_level_demo::files::GameFiles;

/// Steps until the client is in the world and holds its figure's ghost; false when it never is.
fn enter(game: &mut LevelDemo, map: usize) -> bool {
    game.start_map(map).expect("the map starts");
    for _ in 0..600 {
        game.step(32);
        if game.in_world() && game.ghost_count() > 1 {
            return true;
        }
    }
    false
}

/// The height over its feet of the view's eye and of the drawn `eye` node, frame by frame for `frames` frames of 16 ms.
fn heights(game: &mut LevelDemo, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut view, mut drawn) = (Vec::new(), Vec::new());
    for _ in 0..frames {
        game.step(16);
        game.update_scene(0.016);
        if let Some(f) = game.figure() {
            view.push(f.eye[2] - f.position[2]);
            drawn.push(f.drawn_eye[2] - f.position[2]);
        }
    }
    (view, drawn)
}

fn spread(v: &[f32]) -> f32 {
    v.iter().copied().fold(f32::MIN, f32::max) - v.iter().copied().fold(f32::MAX, f32::min)
}

#[test]
fn the_first_person_eye_stays_level_while_the_body_bobs() {
    let files = GameFiles::bundled().expect("the game folder");
    let mut game = LevelDemo::new(files).expect("the client");
    assert!(enter(&mut game, 0), "the first map did not start");
    game.set_first_person(true);
    heights(&mut game, 30);
    // Running, then walking (Shift), past the blend into each.
    game.key(Key::W, true);
    heights(&mut game, 40);
    let (view, drawn) = heights(&mut game, 60);
    assert!(
        view.len() > 50,
        "the figure is placed: {} frames",
        view.len()
    );
    assert!(
        spread(&view) < 0.003,
        "the running view bobs {}",
        spread(&view)
    );
    assert!(
        spread(&drawn) > 0.03,
        "the running body bobs {}",
        spread(&drawn)
    );
    game.key(Key::ShiftLeft, true);
    heights(&mut game, 40);
    let (view, drawn) = heights(&mut game, 60);
    assert!(
        spread(&view) < 0.003,
        "the walking view bobs {}",
        spread(&view)
    );
    assert!(
        spread(&drawn) > 0.01,
        "the walking body bobs {}",
        spread(&drawn)
    );
    game.key(Key::ShiftLeft, false);
    game.key(Key::W, false);
}
