//! The camera behind the figure in its poses: crouched and prone it rides lower with the figure's eye, and lying down while
//! looking up its boom stops above the floor instead of going through it. No window and no GPU: the client and the server in
//! one process, the frames placed and not drawn. (One demo a process.)

use torque::client::input::Key;
use torque::game::sim::player::poses::Pose;
use torque_example_level_demo::client::LevelDemo;
use torque_example_level_demo::files::GameFiles;

const FLAT_PATH: &str = "missions/zz-flat-test.mis";

/// The game with a test map whose floor (its top at 0) is an interior as well as a cube: the headless frames place the
/// interiors for the camera's line of sight (the cubes only as they are drawn).
fn files() -> GameFiles {
    use torque_interior_builder::mission::{Placement, fragment};
    use torque_interior_builder::{Brush, Options, TexGen, V3, build};
    let texgen = |n| TexGen::world(n, 1.0);
    let floor = build(
        &[Brush::cuboid(
            V3::new(-20.0, -20.0, -1.0),
            V3::new(20.0, 20.0, 0.0),
            "floor",
            texgen,
        )],
        &Options::default(),
    )
    .expect("the floor builds");
    let mut files = GameFiles::bundled().expect("the game folder");
    files
        .kit
        .insert("flat-floor.dif", floor.bytes().expect("the floor's bytes"));
    let map = format!(
        "// Flat test\n// A floor.\nnew SimGroup(MissionGroup)\n{{\n   new ScriptObject(MapInfo) {{ title = \"Flat test\"; \
         spawn = \"0 0 0.2\"; spawnYaw = 0; }};\n   new StaticShape() {{ dataBlock = WorldCubeData; position = \"0 0 -0.5\"; \
         scale = \"40 40 1\"; }};\n{}\n}};\n",
        fragment("", "kit/flat-floor.dif", &Placement::default(), &[])
    );
    files.insert(FLAT_PATH, map.as_bytes());
    files
}

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

/// `n` frames of 16 ms: the camera drawn after the last, and the figure's feet and eye.
fn frames(game: &mut LevelDemo, n: usize) -> ([f32; 3], [f32; 3], [f32; 3]) {
    for _ in 0..n {
        game.step(16);
        game.update_scene(0.016);
    }
    let camera = game.drawn_eye().position;
    let f = game.figure().expect("the own figure");
    (camera, f.position, f.eye)
}

#[test]
fn the_camera_behind_rides_lower_with_the_pose_and_stays_above_the_floor() {
    let mut game = LevelDemo::new(files()).expect("the client");
    let index = game
        .maps
        .iter()
        .position(|m| m.path == FLAT_PATH)
        .expect("listed");
    assert!(enter(&mut game, index), "the test map did not start");
    assert!(!game.first_person(), "the camera behind the figure");
    let (stand, feet, _) = frames(&mut game, 60);
    assert!(feet[2].abs() < 0.1, "on the floor: {feet:?}");

    game.key(Key::ControlLeft, true);
    let (crouch, _, crouch_eye) = frames(&mut game, 60);
    assert_eq!(server_pose(&mut game), Pose::Crouch);
    game.key(Key::Z, true);
    game.key(Key::Z, false);
    let (prone, _, prone_eye) = frames(&mut game, 90);
    assert_eq!(server_pose(&mut game), Pose::Prone);
    assert!(
        stand[2] > crouch[2] + 0.2 && crouch[2] > prone[2] + 0.2,
        "the camera rides lower with the figure: {stand:?} {crouch:?} {prone:?} (eyes {crouch_eye:?} {prone_eye:?})"
    );
    // Lying down and looking up as far as the head goes: the boom swings under the figure, into the floor, and stops above it.
    game.call("look", &["-600"]).unwrap();
    let (up, feet, eye) = frames(&mut game, 60);
    assert!(
        up[2] > 0.05 && up[2] < eye[2] + 0.5,
        "the camera above the floor, low behind the figure: {up:?} (feet {feet:?}, eye {eye:?})"
    );
    let along = ((up[0] - feet[0]).powi(2) + (up[1] - feet[1]).powi(2)).sqrt();
    assert!(along > 0.3, "still behind the figure: {up:?} {feet:?}");
    // Level again (the head stopped at its limit, 1.4 radians up).
    game.call("look", &["467"]).unwrap();
    game.key(Key::Z, true);
    game.key(Key::Z, false);
    game.key(Key::ControlLeft, false);
    let (back, _, _) = frames(&mut game, 120);
    assert_eq!(server_pose(&mut game), Pose::Stand);
    assert!(
        (back[2] - stand[2]).abs() < 0.05,
        "back up: {back:?} {stand:?}"
    );
}
