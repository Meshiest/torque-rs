//! A respawn far from the spawn point cuts the camera there: behind the figure lying down and at its eyes standing, it is
//! never drawn on the way between, nor anywhere else while the new figure is on its way. No window and no GPU: the client and
//! the server in one process, the frames placed and not drawn. (One demo a process.)

use torque::client::input::Key;
use torque_example_level_demo::client::LevelDemo;
use torque_example_level_demo::files::GameFiles;

const FLAT_PATH: &str = "missions/zz-flat-test.mis";

/// The game with a test map whose floor (its top at 0) is an interior as well as a cube.
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

/// A frame of 16 ms: the eye drawn.
fn frame(game: &mut LevelDemo) -> [f32; 3] {
    game.step(16);
    game.update_scene(0.016);
    game.drawn_eye().position
}

#[test]
fn a_respawn_far_off_cuts_the_camera_behind_and_at_the_eyes() {
    let mut game = LevelDemo::new(files()).expect("the client");
    let index = game
        .maps
        .iter()
        .position(|m| m.path == FLAT_PATH)
        .expect("listed");
    game.start_map(index).expect("the map starts");
    for _ in 0..600 {
        game.step(32);
        if game.in_world() && game.ghost_count() > 1 {
            break;
        }
    }
    assert!(game.in_world(), "the test map did not start");
    for first in [false, true] {
        game.key(Key::Z, true);
        game.key(Key::Z, false);
        game.set_first_person(first);
        let server = game.server.as_mut().expect("the demo hosts its map");
        let vm =
            std::borrow::BorrowMut::<torque::app::server::GameServer>::borrow_mut(server).vm_mut();
        vm.eval(b"ClientGroup.getObject(0).player.setTransform(\"12 12 0.2 0 0 1 0\");")
            .unwrap();
        for _ in 0..60 {
            frame(&mut game);
        }
        let before = frame(&mut game);
        game.call("restart", &["1"]).unwrap();
        let eyes: Vec<[f32; 3]> = (0..90).map(|_| frame(&mut game)).collect();
        let after = *eyes.last().unwrap();
        // Across the floor: the new figure may go on into its pose at the spawn point, its eye lowered there.
        let across = |p: [f32; 3], q: [f32; 3]| (p[0] - q[0]).hypot(p[1] - q[1]);
        assert!(across(before, after) > 10.0, "{before:?} to {after:?}");
        for (i, p) in eyes.iter().enumerate() {
            let gap = across(*p, before).min(across(*p, after));
            assert!(
                gap < 1.0,
                "first person {first}: frame {i} drew the eye at {p:?}, {gap} m from {before:?} and {after:?}: {:?}",
                &eyes[i.saturating_sub(3)..(i + 10).min(eyes.len())]
            );
        }
    }
}
