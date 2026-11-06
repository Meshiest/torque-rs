//! The example without a window or a GPU: the client and the server joined in one process, the keys posted as a window
//! posts them, and the figure's place read from the server's simulation and from the client's ghost of it.

use torque::client::input::Key;
use torque_example_level_demo::client::LevelDemo;
use torque_example_level_demo::files::GameFiles;

/// A test map: a floor and one wall four units ahead of the spawn point, which faces it.
const WALL_MAP: &str = "// Wall test
// A floor and one wall ahead.
new SimGroup(MissionGroup)
{
   new ScriptObject(MapInfo) { title = \"Wall test\"; spawn = \"0 0 0.2\"; spawnYaw = 0; };
   new StaticShape() { dataBlock = WorldCubeData; position = \"0 0 -0.5\"; scale = \"20 20 1\"; };
   new StaticShape(TestWall) { dataBlock = WorldCubeData; position = \"0 4.5 1.5\"; scale = \"8 1 3\"; };
};
";

/// The map's path in the game folder.
const WALL_PATH: &str = "missions/zz-wall-test.mis";

/// Steps until the client is in the world and holds its figure's ghost; false when it never is.
fn enter(game: &mut LevelDemo, map: usize) -> bool {
    game.start_map(map).expect("the map starts");
    for _ in 0..600 {
        game.step(32);
        // The figure is drawn from its ghost: a frame needs no GPU to place it.
        if game.in_world() && game.ghost_count() > 1 {
            return true;
        }
    }
    false
}

fn server_position(game: &LevelDemo) -> [f32; 3] {
    game.server
        .as_ref()
        .and_then(|s| s.figure_position())
        .expect("the server's figure")
}

/// Holds key `vk` down for `ms` of the clock.
fn hold(game: &mut LevelDemo, key: Key, ms: u32) {
    game.key(key, true);
    for _ in 0..ms / 32 {
        game.step(32);
    }
    game.key(key, false);
}

fn errors(game: &LevelDemo) -> Vec<&String> {
    game.server_log
        .iter()
        .filter(|l| {
            l.contains("Unknown command") || l.contains("Unable to find") || l.contains("error")
        })
        .collect()
}

#[test]
fn every_map_loads_and_its_figure_spawns_and_walks() {
    let files = GameFiles::bundled().expect("the game folder");
    let mut game = LevelDemo::new(files).expect("the client");
    game.step(64);
    for (index, map) in game.maps.clone().iter().enumerate() {
        assert!(enter(&mut game, index), "{} did not start", map.title);
        for _ in 0..30 {
            game.step(32);
        }
        let start = server_position(&game);
        hold(&mut game, Key::W, 1200);
        let end = server_position(&game);
        let moved = ((end[0] - start[0]).powi(2) + (end[1] - start[1]).powi(2)).sqrt();
        assert!(
            moved > 1.0,
            "{}: the figure moved {moved} from {start:?}",
            map.title
        );
        assert!(
            end[2] > -1.0,
            "{}: the figure fell through to {end:?}",
            map.title
        );
        assert!(
            errors(&game).is_empty(),
            "{}: {:?}",
            map.title,
            errors(&game)
        );
        // Leaving goes back to the menu and stops the server.
        hold(&mut game, Key::Escape, 64);
        assert!(!game.in_world() && game.server.is_none(), "{}", map.title);
    }
}

/// Every map on the menu starts from its number key, as a player picks it.
#[test]
fn every_map_starts_from_its_number_key() {
    const DIGITS: [Key; 9] = [
        Key::Digit1,
        Key::Digit2,
        Key::Digit3,
        Key::Digit4,
        Key::Digit5,
        Key::Digit6,
        Key::Digit7,
        Key::Digit8,
        Key::Digit9,
    ];
    let mut game = LevelDemo::new(GameFiles::bundled().expect("the game folder")).unwrap();
    game.step(64);
    let maps = game.maps.clone();
    assert!(maps.len() <= DIGITS.len(), "more maps than number keys");
    for (index, map) in maps.iter().enumerate() {
        game.key(DIGITS[index], true);
        game.key(DIGITS[index], false);
        let mut started = false;
        for _ in 0..600 {
            game.step(32);
            if game.in_world() && game.ghost_count() > 1 {
                started = true;
                break;
            }
        }
        assert!(
            started,
            "{} did not start from key {}: {:?}",
            map.title,
            index + 1,
            game.server_log
        );
        assert_eq!(game.playing, Some(index), "{}", map.title);
        hold(&mut game, Key::Escape, 64);
        assert!(!game.in_world(), "{}", map.title);
    }
}

#[test]
fn the_maps_sky_reaches_the_client_with_its_far_fog() {
    let mut game = LevelDemo::new(GameFiles::bundled().expect("the game folder")).unwrap();
    assert!(enter(&mut game, 0));
    let mut sky = None;
    for _ in 0..100 {
        sky = game.sky_daylight();
        if sky.is_some() {
            break;
        }
        game.step(32);
    }
    let sky = sky.unwrap_or_else(|| panic!("the Sky is ghosted: {:#?}", game.server_log));
    assert_eq!((sky.fog_start, sky.visible), (1500.0, 3000.0));
}

#[test]
fn a_figure_that_walks_into_a_cube_stops_at_it_and_jumps() {
    let mut files = GameFiles::bundled().expect("the game folder");
    files.insert(WALL_PATH, WALL_MAP.as_bytes());
    let mut game = LevelDemo::new(files).expect("the client");
    let map = game
        .maps
        .iter()
        .position(|m| m.path == WALL_PATH)
        .expect("the test map is listed");
    assert!(enter(&mut game, map), "the test map did not start");
    for _ in 0..30 {
        game.step(32);
    }
    let start = server_position(&game);
    assert!(start[2].abs() < 0.1, "standing on the floor: {start:?}");
    // Two seconds forward: far enough to cross the four units, but the wall's near face is at y 4 and the figure's box is a
    // unit deep.
    hold(&mut game, Key::W, 2000);
    let stopped = server_position(&game);
    assert!(
        stopped[1] > 3.0 && stopped[1] < 3.6,
        "stopped by the wall: {stopped:?}"
    );
    assert!(stopped[2].abs() < 0.1, "still on the floor: {stopped:?}");
    // The client's ghost of the figure is where the server's figure is.
    for _ in 0..10 {
        game.step(32);
    }
    let ghost = game
        .ghost_figure_position()
        .expect("the client's ghost of the figure");
    let at = server_position(&game);
    let apart = (0..3).map(|i| (ghost[i] - at[i]).abs()).fold(0.0, f32::max);
    assert!(apart < 0.05, "the ghost at {ghost:?}, the figure at {at:?}");
    // A jump lifts the figure off the floor and it lands again.
    game.key(Key::Space, true);
    let mut highest = 0.0f32;
    for _ in 0..40 {
        game.step(32);
        highest = highest.max(server_position(&game)[2]);
    }
    game.key(Key::Space, false);
    for _ in 0..60 {
        game.step(32);
    }
    assert!(highest > 0.5, "the jump reached {highest}");
    assert!(server_position(&game)[2].abs() < 0.1, "landed again");
    assert!(errors(&game).is_empty(), "{:?}", errors(&game));
}

/// Space pressed and let go in the same frame (no step between) jumps once, and the jump is not held after it: the figure lands
/// and stays on the floor.
#[test]
fn a_tap_of_space_inside_one_frame_jumps_once_and_lets_go() {
    let mut files = GameFiles::bundled().expect("the game folder");
    files.insert(WALL_PATH, WALL_MAP.as_bytes());
    let mut game = LevelDemo::new(files).expect("the client");
    let map = game
        .maps
        .iter()
        .position(|m| m.path == WALL_PATH)
        .expect("the test map is listed");
    assert!(enter(&mut game, map), "the test map did not start");
    for _ in 0..30 {
        game.step(32);
    }
    assert!(server_position(&game)[2].abs() < 0.1, "standing first");
    for tap in 0..2 {
        game.key(Key::Space, true);
        game.key(Key::Space, false);
        let (mut takeoffs, mut aloft, mut highest) = (0, false, 0.0f32);
        // Three seconds: a held jump would leave the floor again and again in that time (its delay is 10 ticks).
        for _ in 0..94 {
            game.step(32);
            let z = server_position(&game)[2];
            highest = highest.max(z);
            if !aloft && z > 0.3 {
                takeoffs += 1;
            }
            aloft = z > 0.3;
        }
        assert!(highest > 0.5, "tap {tap}: the jump reached {highest}");
        assert_eq!(takeoffs, 1, "tap {tap}: one jump for one tap");
        assert!(
            server_position(&game)[2].abs() < 0.1,
            "tap {tap}: landed and not jumping again"
        );
    }
}

#[test]
fn tab_eases_the_camera_to_the_eyes_and_back_and_hides_the_figure_only_there() {
    use torque::app::camera_blend::CLOSE;
    // The figure is hidden while the camera is within CLOSE of its eyes; the frame's flag is the frame before's distance.
    let files = GameFiles::bundled().expect("the game folder");
    let mut game = LevelDemo::new(files).expect("the client");
    assert!(enter(&mut game, 0), "the first map did not start");
    let frame = |game: &mut LevelDemo| {
        game.step(16);
        game.update_scene(0.016);
        game.drawn_eye()
    };
    // The own figure placed (the camera behind it), then a moment more.
    for _ in 0..600 {
        frame(&mut game);
        if game.figure().is_some() {
            break;
        }
    }
    assert!(game.figure().is_some(), "no own figure");
    for _ in 0..30 {
        frame(&mut game);
    }
    let hidden = |g: &LevelDemo| g.with_world(|w| w.hide_control).unwrap();
    let apart =
        |a: [f32; 3], b: [f32; 3]| (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt();
    let behind = frame(&mut game);
    assert!(!game.first_person() && !hidden(&game));
    game.key(Key::Tab, true);
    game.key(Key::Tab, false);
    let mut last = behind;
    let mut biggest = 0.0f32;
    let mut frames = 0;
    let mut hidden_frames = 0;
    while frames == 0 || game.camera_blending() {
        let e = frame(&mut game);
        let left = apart(e.position, game.eye().position);
        // Hidden only once the camera is close to the eyes (the frame before's distance decides: the camera only closes in).
        assert!(
            !hidden(&game) || left < CLOSE,
            "hidden {left} from the eyes"
        );
        hidden_frames += usize::from(hidden(&game));
        biggest = biggest.max(apart(last.position, e.position));
        last = e;
        frames += 1;
        assert!(frames < 60);
    }
    assert!(hidden(&game) && hidden_frames > 1);
    let eyes = game.eye();
    assert_eq!(last, eyes);
    // No frame's step more than the spring's fastest share of the way: 0.27 of its rate times the way, a second.
    let whole = apart(behind.position, eyes.position);
    assert!(whole > 3.0, "{whole} from behind to the eyes");
    assert!(
        biggest < whole * 0.13 + 0.05,
        "a step of {biggest} over {whole}"
    );
    // Back: shown once the camera is out of the head, early in the move.
    game.key(Key::Tab, true);
    game.key(Key::Tab, false);
    let mut frames = 0;
    while hidden(&game) {
        frame(&mut game);
        frames += 1;
        assert!(frames < 12, "still hidden after {frames} frames");
    }
    assert!(game.camera_blending());
}

/// A test map: a floor, and an interior over the spawn point (which faces +y) with a ceiling 2.6 units up and a wall 5 units
/// behind; the camera behind the figure would be 6.5 units back and above the ceiling without them.
#[test]
fn the_camera_behind_the_figure_stays_under_an_interior_ceiling_and_in_front_of_its_wall() {
    use torque_interior_builder::mission::{Placement, fragment};
    use torque_interior_builder::{Brush, Options, TexGen, V3, build};
    let texgen = |n| TexGen::world(n, 1.0);
    let room = build(
        &[
            Brush::cuboid(
                V3::new(-6.0, -8.0, 2.6),
                V3::new(6.0, 8.0, 2.9),
                "roof",
                texgen,
            ),
            Brush::cuboid(
                V3::new(-6.0, -5.4, 0.0),
                V3::new(6.0, -5.0, 2.6),
                "wall",
                texgen,
            ),
        ],
        &Options::default(),
    )
    .expect("the room builds");
    let mut files = GameFiles::bundled().expect("the game folder");
    files
        .kit
        .insert("camera-room.dif", room.bytes().expect("the room's bytes"));
    let map = format!(
        "// Camera room test\n// A ceiling and a wall behind.\nnew SimGroup(MissionGroup)\n{{\n   new ScriptObject(MapInfo) {{ \
         title = \"Camera room test\"; spawn = \"0 0 0.2\"; spawnYaw = 0; }};\n   new StaticShape() {{ dataBlock = WorldCubeData; \
         position = \"0 0 -0.5\"; scale = \"20 20 1\"; }};\n{}\n}};\n",
        fragment("", "kit/camera-room.dif", &Placement::default(), &[])
    );
    let path = "missions/zz-camera-room-test.mis";
    files.insert(path, map.as_bytes());
    let mut game = LevelDemo::new(files).expect("the client");
    let index = game
        .maps
        .iter()
        .position(|m| m.path == path)
        .expect("listed");
    assert!(enter(&mut game, index), "the test map did not start");
    for _ in 0..30 {
        game.step(32);
        game.update_scene(0.032);
    }
    assert!(!game.first_person());
    let eye = game.drawn_eye().position;
    let figure = server_position(&game);
    assert!(figure[2].abs() < 0.1, "the figure on the floor: {figure:?}");
    assert!(
        eye[2] < 2.6 && eye[1] > -5.0 && eye[1] < figure[1],
        "the camera at {eye:?} behind the figure at {figure:?}"
    );
}
