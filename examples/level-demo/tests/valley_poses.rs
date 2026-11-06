//! The valley's terrain and water with the figure's poses: it crouches and lies down on the terrain, and the camera behind it,
//! turned all the way round and looking up, stays above the terrain and the water. No window and no GPU: the client and the
//! server in one process, the frames placed and not drawn. (One demo a process.)

use torque::client::input::Key;
use torque::game::sim::player::poses::Pose;
use torque_example_level_demo::client::LevelDemo;
use torque_example_level_demo::files::GameFiles;

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

fn server_pose(game: &mut LevelDemo) -> Pose {
    let server = game.server.as_mut().expect("the demo hosts its map");
    let vm = std::borrow::BorrowMut::<torque::app::server::GameServer>::borrow_mut(server).vm_mut();
    let text = vm
        .eval(b"return ClientGroup.getObject(0).player.getId();")
        .unwrap();
    let id: torque::script::vm::sim::ObjId = String::from_utf8(text).unwrap().parse().unwrap();
    torque::game::sim::player::state(vm, id).unwrap().pose
}

/// `n` frames of 16 ms; the camera drawn after the last.
fn frames(game: &mut LevelDemo, n: usize) -> [f32; 3] {
    for _ in 0..n {
        game.step(16);
        game.update_scene(0.016);
    }
    game.drawn_eye().position
}

/// How far the camera at `at` is above the terrain or water under it (none where there is neither).
fn above(game: &LevelDemo, at: [f32; 3]) -> Option<f32> {
    game.ground.surface(at[0], at[1]).map(|h| at[2] - h)
}

#[test]
fn on_the_valley_the_figure_lies_down_and_the_camera_stays_above_the_ground_and_water() {
    let mut game =
        LevelDemo::new(GameFiles::bundled().expect("the game folder")).expect("the client");
    let index = game
        .maps
        .iter()
        .position(|m| m.path.ends_with("valley.mis"))
        .expect("the valley is listed");
    assert!(enter(&mut game, index), "the valley did not start");
    frames(&mut game, 60);
    assert!(game.ground.terrain.is_some() && game.ground.water.is_some());
    let feet = game.figure().expect("the own figure").position;
    let ground = game
        .ground
        .height(feet[0], feet[1])
        .expect("on the terrain");
    assert!(
        (feet[2] - ground).abs() < 0.3,
        "standing on the terrain: {feet:?} over {ground}"
    );

    game.key(Key::ControlLeft, true);
    frames(&mut game, 60);
    assert_eq!(server_pose(&mut game), Pose::Crouch);
    game.key(Key::Z, true);
    game.key(Key::Z, false);
    frames(&mut game, 90);
    assert_eq!(server_pose(&mut game), Pose::Prone);
    let f = game.figure().unwrap();
    let ground = game.ground.height(f.position[0], f.position[1]).unwrap();
    assert!(
        (f.position[2] - ground).abs() < 0.3 && f.eye[2] - ground < 0.9,
        "lying on the terrain: {:?} (eye {:?}) over {ground}",
        f.position,
        f.eye
    );
    // Looking up and turned all the way round: the boom swings low over the terrain and the water and never under them.
    game.call("look", &["-300"]).unwrap();
    let mut lowest = f32::MAX;
    for _ in 0..24 {
        game.call("turn", &["87"]).unwrap();
        let at = frames(&mut game, 8);
        if let Some(h) = above(&game, at) {
            lowest = lowest.min(h);
        }
    }
    assert!(
        lowest > 0.25,
        "the camera came {lowest} above the ground or water"
    );
}
