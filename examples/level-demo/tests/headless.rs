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

/// Esc opens the pause menu over the world; its Leave game goes back to the menu of maps.
fn leave(game: &mut LevelDemo) {
    hold(game, Key::Escape, 64);
    assert!(
        game.flag("$Client::Paused"),
        "Esc did not open the pause menu"
    );
    game.call("leaveGame", &[]).expect("leaveGame");
    game.step(32);
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
        leave(&mut game);
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
        leave(&mut game);
        assert!(!game.in_world(), "{}", map.title);
    }
}

/// The corner readout once frames were drawn 60 a second (the host counts them as the window does): the frame rate alone, since
/// the link to the map's server is in memory and has no round trip to show; Esc opens the pause menu, whose Controls panel
/// lists the keys.
#[test]
fn the_corner_readout_holds_the_frame_rate_and_esc_lists_the_keys() {
    let mut game = LevelDemo::new(GameFiles::bundled().expect("the game folder")).unwrap();
    game.step(64);
    assert!(enter(&mut game, 0), "the first map did not start");
    for i in 0..60 {
        game.host.frame_drawn(f64::from(i) * 1000.0 / 60.0);
        game.step(16);
    }
    game.step(300);
    let text = game
        .call("eval", &["return PlayStats.getValue();"])
        .unwrap();
    assert_eq!(text, "60 fps");
    hold(&mut game, Key::Escape, 64);
    game.call("showControls", &[]).unwrap();
    let shown = game
        .call(
            "eval",
            &["return ControlsPanel.isVisible() SPC ControlsList.getCount();"],
        )
        .unwrap();
    // Three headings, and a key and its action for each of the 11 rows.
    assert_eq!(shown, format!("1 {}", 3 + 2 * 11));
    // A small window: the groups in one column, scrolled with the wheel.
    game.host.window_resized((640, 480));
    game.step(64);
    let small = game
        .call(
            "eval",
            &["return $Pause::ControlsStacked TAB ControlsNote.getValue() TAB ControlsScroll.getExtent();"],
        )
        .unwrap();
    assert!(small.starts_with("1\tMouse wheel: more keys"), "{small:?}");
    // Esc from the panel goes back to the game.
    hold(&mut game, Key::Escape, 64);
    assert!(!game.flag("$Client::Paused"));
    leave(&mut game);
    assert!(!game.in_world());
    assert!(errors(&game).is_empty(), "{:?}", errors(&game));
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
    // The figure first (the map can start before its ghost is drawn), then the camera settled behind it.
    for _ in 0..600 {
        if game.figure().is_some() {
            break;
        }
        frame(&mut game);
    }
    assert!(game.figure().is_some(), "no figure");
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

#[test]
fn tab_moves_the_camera_between_the_eyes_and_behind_and_never_turns_the_view() {
    use torque_example_common::behind::{forward, look};
    let files = GameFiles::bundled().expect("the game folder");
    let mut game = LevelDemo::new(files).expect("the client");
    assert!(enter(&mut game, 0), "the first map did not start");
    let frame = |game: &mut LevelDemo| {
        game.step(16);
        game.update_scene(0.016);
        game.drawn_eye()
    };
    for _ in 0..600 {
        if game.figure().is_some() {
            break;
        }
        frame(&mut game);
    }
    assert!(game.figure().is_some(), "no figure");
    let same = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4);
    let figure_look = |game: &LevelDemo| {
        let f = game.figure().expect("the figure");
        look(f.yaw, f.pitch)
    };
    let apart =
        |a: [f32; 3], b: [f32; 3]| (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt();
    // Turned and tilted by the mouse (to the head's limits, 1.4 radians either way), then held: from the eyes and from behind
    // the figure the view looks the same way, the figure's.
    for (turn, tilt) in [
        ("0", "0"),
        ("300", "150"),
        ("-700", "-270"),
        ("1200", "600"),
        ("-400", "-900"),
    ] {
        game.call("turn", &[turn]).unwrap();
        game.call("look", &[tilt]).unwrap();
        for _ in 0..20 {
            frame(&mut game);
        }
        game.set_first_person(false);
        let behind = frame(&mut game);
        game.set_first_person(true);
        let eyes = frame(&mut game);
        let want = figure_look(&game);
        assert!(
            same(forward(&behind), want) && same(forward(&eyes), want),
            "turn {turn} tilt {tilt}: behind {:?}, eyes {:?}, the figure's {want:?}",
            forward(&behind),
            forward(&eyes)
        );
        assert!(apart(behind.position, eyes.position) > 1.0);
    }
    // Through the ease, either way, held still and with the mouse moving: every frame looks where the figure looks.
    game.call("look", &["300"]).unwrap();
    game.set_first_person(false);
    for _ in 0..30 {
        frame(&mut game);
    }
    for moving in [false, true, false, true] {
        let from = frame(&mut game);
        game.key(Key::Tab, true);
        game.key(Key::Tab, false);
        let mut frames = 0;
        let mut last = from;
        while frames == 0 || game.camera_blending() {
            if moving {
                game.call("turn", &["4"]).unwrap();
                game.call("look", &["-2"]).unwrap();
            }
            let e = frame(&mut game);
            assert!(
                same(forward(&e), figure_look(&game)),
                "frame {frames} of the ease: {:?}, the figure's {:?}",
                forward(&e),
                figure_look(&game)
            );
            last = e;
            frames += 1;
            assert!(frames < 60, "the ease never ends");
        }
        assert!(frames > 5 && apart(from.position, last.position) > 1.0);
    }
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

/// The panel `p` is centered in a window of `size` (to a pixel or two), and each rect of `inner` is inside it.
fn centered(p: [i32; 4], size: (u32, u32), inner: &[(&str, [i32; 4])]) {
    let (w, h) = (size.0 as i32, size.1 as i32);
    assert!(
        (2 * p[0] + p[2] - w).abs() <= 2,
        "across at {size:?}: {p:?}"
    );
    assert!((2 * p[1] + p[3] - h).abs() <= 2, "down at {size:?}: {p:?}");
    for (name, r) in inner {
        let inside = r[0] >= p[0]
            && r[1] >= p[1]
            && r[0] + r[2] <= p[0] + p[2]
            && r[1] + r[3] <= p[1] + p[3];
        assert!(inside, "{name} {r:?} outside the panel {p:?} at {size:?}");
    }
}

#[test]
fn the_menu_stays_centered_at_wide_and_tall_sizes() {
    let files = GameFiles::bundled().expect("the game folder");
    let mut game = LevelDemo::new(files).expect("the client");
    game.step(64);
    for size in [(2560, 1080), (1000, 700), (800, 1400), (2217, 935)] {
        game.client.host.window_resized(size);
        game.step(64);
        let panel = game.client.control_rect("MenuPanel").unwrap();
        let inner: Vec<(&str, [i32; 4])> = ["MenuTitle", "MenuList", "MenuStatus", "MapButton0"]
            .into_iter()
            .map(|n| (n, game.client.control_rect(n).unwrap()))
            .collect();
        centered(panel, size, &inner);
    }
}
