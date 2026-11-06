//! The figure's poses in the level demo, in first person: Ctrl crouches and Z lies down, and the steady eye (the engine's own
//! `GhostOptions::steady_eye`) lowers with each pose and rises again, eased rather than snapping. No window and no GPU: the
//! client and the server in one process, the frames placed and not drawn. (One demo a process: the hit's test is its own file.)

use torque::client::input::Key;
use torque::game::sim::player::poses::Pose;
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

/// The view's eye over the figure's feet and the drawn `eye` node's, frame by frame for `frames` frames of 16 ms.
fn eyes(game: &mut LevelDemo, frames: usize) -> Vec<([f32; 3], [f32; 3])> {
    let mut out = Vec::new();
    for _ in 0..frames {
        game.step(16);
        game.update_scene(0.016);
        if let Some(f) = game.figure() {
            let at = |p: [f32; 3]| {
                [
                    p[0] - f.position[0],
                    p[1] - f.position[1],
                    p[2] - f.position[2],
                ]
            };
            out.push((at(f.eye), at(f.drawn_eye)));
        }
    }
    out
}

/// The figure's pose on the server.
fn server_pose(game: &mut LevelDemo) -> Pose {
    let server = game.server.as_mut().expect("the demo hosts its map");
    let vm = std::borrow::BorrowMut::<torque::app::server::GameServer>::borrow_mut(server).vm_mut();
    let text = vm
        .eval(b"return ClientGroup.getObject(0).player.getId();")
        .unwrap();
    let id: torque::script::vm::sim::ObjId = String::from_utf8(text).unwrap().parse().unwrap();
    torque::game::sim::player::state(vm, id).unwrap().pose
}

/// The largest change of height from one frame to the next.
fn largest_step(heights: &[f32]) -> f32 {
    heights
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn the_first_person_eye_lowers_with_each_pose_eased_and_rises_again() {
    let files = GameFiles::bundled().expect("the game folder");
    let mut game = LevelDemo::new(files).expect("the client");
    assert!(enter(&mut game, 0), "the first map did not start");
    game.set_first_person(true);
    // Until the figure is placed in first person, and a moment more.
    for _ in 0..600 {
        if !eyes(&mut game, 1).is_empty() {
            break;
        }
    }
    eyes(&mut game, 30);
    let height = |e: &[([f32; 3], [f32; 3])]| e.iter().map(|x| x.0[2]).collect::<Vec<f32>>();
    let stand = height(&eyes(&mut game, 10));
    let standing = stand[stand.len() - 1];

    game.key(Key::ControlLeft, true);
    let down = height(&eyes(&mut game, 40));
    assert_eq!(server_pose(&mut game), Pose::Crouch);
    let crouched = down[down.len() - 1];
    assert!(
        standing - crouched > 0.3 && (1.2..1.6).contains(&crouched),
        "the eye crouches: {standing} to {crouched}"
    );
    // Eased: it moves a little at a time, never jumping, and it moves only down.
    let step = largest_step(&down);
    assert!(step < 0.08, "the crouch moves the eye {step} in one frame");
    assert!(down.windows(2).all(|w| w[1] <= w[0] + 1e-4), "{down:?}");
    // Lying down (Z with Ctrl still held), then getting up again.
    game.key(Key::Z, true);
    game.key(Key::Z, false);
    let flat = height(&eyes(&mut game, 60));
    assert_eq!(server_pose(&mut game), Pose::Prone);
    let prone = flat[flat.len() - 1];
    assert!((0.35..0.8).contains(&prone), "the eye lies down: {prone}");
    assert!(largest_step(&flat) < 0.08, "{flat:?}");
    game.key(Key::Z, true);
    game.key(Key::Z, false);
    game.key(Key::ControlLeft, false);
    let up = height(&eyes(&mut game, 90));
    assert_eq!(server_pose(&mut game), Pose::Stand);
    assert!(
        (up[up.len() - 1] - standing).abs() < 0.01,
        "back at {} from {standing}",
        up[up.len() - 1]
    );
    assert!(largest_step(&up) < 0.08, "{up:?}");
    assert!(
        up.windows(2).all(|w| w[1] >= w[0] - 1e-4),
        "it only rises: {up:?}"
    );
}

#[test]
fn space_while_lying_down_gets_up_and_does_not_jump() {
    let files = GameFiles::bundled().expect("the game folder");
    let mut game = LevelDemo::new(files).expect("the client");
    assert!(enter(&mut game, 0), "the first map did not start");
    eyes(&mut game, 60);
    game.key(Key::Z, true);
    game.key(Key::Z, false);
    eyes(&mut game, 60);
    assert_eq!(server_pose(&mut game), Pose::Prone);
    let lying = game.ghost_figure_position().expect("the figure")[2];
    game.key(Key::Space, true);
    game.key(Key::Space, false);
    let mut highest = lying;
    for _ in 0..90 {
        eyes(&mut game, 1);
        highest = highest.max(game.ghost_figure_position().expect("the figure")[2]);
    }
    assert_eq!(server_pose(&mut game), Pose::Stand);
    assert!(
        highest - lying < 0.05,
        "the figure jumped: {lying} to {highest}"
    );
    // A second tap, standing, jumps again.
    game.key(Key::Space, true);
    game.key(Key::Space, false);
    let mut highest = lying;
    for _ in 0..40 {
        eyes(&mut game, 1);
        highest = highest.max(game.ghost_figure_position().expect("the figure")[2]);
    }
    assert!(
        highest - lying > 0.3,
        "standing, Space jumps: {lying} to {highest}"
    );
}

/// Presses and lets go of `key`, then steps `frames` frames.
fn tap(game: &mut LevelDemo, key: Key, frames: usize) {
    game.key(key, true);
    game.key(key, false);
    eyes(game, frames);
}

#[test]
fn c_toggles_the_crouch_and_crouch_keys_lying_down_get_up_to_a_crouch() {
    let files = GameFiles::bundled().expect("the game folder");
    let mut game = LevelDemo::new(files).expect("the client");
    assert!(enter(&mut game, 0), "the first map did not start");
    eyes(&mut game, 60);
    tap(&mut game, Key::C, 60);
    assert_eq!(server_pose(&mut game), Pose::Crouch, "C crouches");
    tap(&mut game, Key::C, 60);
    assert_eq!(server_pose(&mut game), Pose::Stand, "C again stands");
    tap(&mut game, Key::C, 60);
    tap(&mut game, Key::Space, 60);
    assert_eq!(
        server_pose(&mut game),
        Pose::Stand,
        "Space stands from C's crouch"
    );
    tap(&mut game, Key::Z, 90);
    assert_eq!(server_pose(&mut game), Pose::Prone);
    tap(&mut game, Key::C, 90);
    assert_eq!(
        server_pose(&mut game),
        Pose::Crouch,
        "C lying down gets up to a crouch"
    );
    tap(&mut game, Key::Z, 90);
    assert_eq!(
        server_pose(&mut game),
        Pose::Prone,
        "Z from C's crouch lies down"
    );
    game.key(Key::ControlLeft, true);
    eyes(&mut game, 90);
    assert_eq!(
        server_pose(&mut game),
        Pose::Crouch,
        "Ctrl lying down gets up to a crouch"
    );
    game.key(Key::ControlLeft, false);
    eyes(&mut game, 90);
    assert_eq!(server_pose(&mut game), Pose::Stand, "Ctrl let go stands");
}

#[test]
fn a_restart_lying_down_starts_standing() {
    let files = GameFiles::bundled().expect("the game folder");
    let mut game = LevelDemo::new(files).expect("the client");
    assert!(enter(&mut game, 0), "the first map did not start");
    eyes(&mut game, 60);
    tap(&mut game, Key::Z, 90);
    assert_eq!(server_pose(&mut game), Pose::Prone);
    tap(&mut game, Key::R, 120);
    assert_eq!(
        server_pose(&mut game),
        Pose::Stand,
        "the restarted figure stands"
    );
}
