//! The menus at any window size from 1024x576 to 3840x2160 (and 4:3): each screen's content stays in the window, centered,
//! with nothing overlapping; the join list and address box widen with the window up to a comfortable width and stay centered
//! beyond it, and the buttons keep their size. The pause menu stays in the middle of the view.

use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;

const SIZES: [(i32, i32); 8] = [
    (1024, 576),
    (1024, 768),
    (1280, 720),
    (1600, 1200),
    (1920, 1080),
    (2560, 1080),
    (2560, 1440),
    (3840, 2160),
];

fn client() -> ArenaGame {
    let mut game = ArenaGame::new(
        GameFiles::bundled().unwrap(),
        ClientOptions {
            net: NetMode::Loopback,
            name: "Ann".to_string(),
            port: 0,
            map: Some("fixtures/floor.mis".to_string()),
            ..ClientOptions::default()
        },
    )
    .unwrap();
    game.host.window_resized((800, 600));
    game.step(32);
    game
}

fn resize(game: &mut ArenaGame, size: (i32, i32)) {
    game.host.window_resized((size.0 as u32, size.1 as u32));
    game.step(64);
}

fn rect(game: &mut ArenaGame, name: &str) -> [i32; 4] {
    game.control_rect(name).unwrap()
}

fn overlaps(a: [i32; 4], b: [i32; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

fn inside(inner: [i32; 4], outer: [i32; 4]) -> bool {
    inner[0] >= outer[0]
        && inner[1] >= outer[1]
        && inner[0] + inner[2] <= outer[0] + outer[2]
        && inner[1] + inner[3] <= outer[1] + outer[3]
}

/// The panel is centered in the window (to a pixel or two); every control in `leaves` is in the panel and clear of the others.
fn check(game: &mut ArenaGame, panel: &str, leaves: &[&str], size: (i32, i32)) -> [i32; 4] {
    let window = [0, 0, size.0, size.1];
    let p = rect(game, panel);
    let what = format!("{panel} at {size:?}: {p:?}");
    assert!(inside(p, window), "{what}");
    assert!(
        (2 * p[0] + p[2] - size.0).abs() <= 2,
        "centered across: {what}"
    );
    assert!(
        (2 * p[1] + p[3] - size.1).abs() <= 2,
        "centered down: {what}"
    );
    let rects: Vec<[i32; 4]> = leaves.iter().map(|n| rect(game, n)).collect();
    for (name, r) in leaves.iter().zip(&rects) {
        assert!(inside(*r, p), "{name} {r:?} is outside {what}");
    }
    for (i, a) in rects.iter().enumerate() {
        for (j, b) in rects.iter().enumerate().skip(i + 1) {
            assert!(
                !overlaps(*a, *b),
                "{} {a:?} overlaps {} {b:?} at {size:?}",
                leaves[i],
                leaves[j]
            );
        }
    }
    p
}

const MAIN: &[&str] = &[
    "MainTitle",
    "MainLine",
    "NameLabel",
    "NameEdit",
    "HostButton",
    "JoinButton",
    "QuitButton",
    "HostNote",
    "JoinNote",
    "MainStatus",
];
const HOST: &[&str] = &[
    "HostTitle",
    "HostLine",
    "HostLanButton",
    "HostLanNote",
    "HostLanNote2",
    "HostPublicButton",
    "HostPublicNote",
    "HostPublicNote2",
    "HostBackButton",
    "HostLast",
    "HostStatus",
];
const JOIN: &[&str] = &[
    "JoinTitle",
    "JoinHeaderGame",
    "JoinHeaderPlayers",
    "JoinHeaderPing",
    "JoinHeaderMap",
    "ServerScroll",
    "AddressLabel",
    "AddressEdit",
    "ConnectButton",
    "RefreshButton",
    "JoinSelectedButton",
    "BackButton",
    "JoinStatus",
];

fn check_join(game: &mut ArenaGame, size: (i32, i32)) {
    let p = check(game, "JoinPanel", JOIN, size);
    let want = (size.0 - 64).clamp(640, 1000);
    assert_eq!(p[2], want, "panel width at {size:?}");
    assert!(p[3] <= 760 && p[3] >= 500, "height {} at {size:?}", p[3]);
    // The list and the address box widen with the panel; the buttons keep their size.
    let list = rect(game, "ServerScroll");
    assert_eq!(list[2], p[2], "the list spans the panel at {size:?}");
    let edit = rect(game, "AddressEdit");
    assert_eq!(edit[2], p[2] - 82 - 170 - 10, "the address box at {size:?}");
    for button in ["RefreshButton", "JoinSelectedButton", "BackButton"] {
        let b = rect(game, button);
        assert_eq!((b[2], b[3]), (200, 50), "{button} at {size:?}");
    }
    let connect = rect(game, "ConnectButton");
    assert_eq!((connect[2], connect[3]), (170, 40));
    assert_eq!(
        connect[0] + connect[2],
        p[0] + p[2],
        "Connect keeps the right edge"
    );
    // The list's columns and headers move together.
    let players = rect(game, "JoinHeaderPlayers");
    assert_eq!(
        players[0] - p[0],
        4 + (p[2] * 46 / 100),
        "the Players header at {size:?}"
    );
}

#[test]
fn the_menus_fit_and_center_at_every_window_size() {
    let mut g = client();
    for size in SIZES {
        resize(&mut g, size);
        g.call("showMainMenu", &[]).unwrap();
        g.step(64);
        check(&mut g, "MainPanel", MAIN, size);
        g.call("showHost", &[]).unwrap();
        g.step(64);
        check(&mut g, "HostPanel", HOST, size);
        g.call("showJoin", &[]).unwrap();
        g.step(64);
        check_join(&mut g, size);
    }
    assert!(g.host.input_errors.is_empty(), "{:?}", g.host.input_errors);
}

#[test]
fn a_window_dragged_to_other_sizes_keeps_each_screen_laid_out() {
    let mut g = client();
    for screen in ["Main", "Host", "Join"] {
        let show = match screen {
            "Main" => "showMainMenu",
            "Host" => "showHost",
            _ => "showJoin",
        };
        g.call(show, &[]).unwrap();
        for size in SIZES.iter().chain(SIZES.iter().rev()) {
            resize(&mut g, *size);
            match screen {
                "Main" => {
                    check(&mut g, "MainPanel", MAIN, *size);
                }
                "Host" => {
                    check(&mut g, "HostPanel", HOST, *size);
                }
                _ => check_join(&mut g, *size),
            }
        }
    }
}

#[test]
fn the_pause_menu_stays_in_the_middle_of_the_view() {
    let mut g = client();
    g.call("hostGame", &["lan"]).unwrap();
    for _ in 0..200 {
        g.step(32);
        if g.in_game() {
            break;
        }
    }
    assert!(g.in_game());
    // The host's menu has the Change map entry once the server has sent the maps.
    for _ in 0..200 {
        g.step(32);
        if g.global("$Client::MapCount") != "0" {
            break;
        }
    }
    assert_ne!(
        g.global("$Client::MapCount"),
        "0",
        "the host was sent the maps"
    );
    g.call("pauseGame", &[]).unwrap();
    for size in SIZES {
        resize(&mut g, size);
        let p = check(
            &mut g,
            "PausePanel",
            &[
                "PauseTitle",
                "ResumeButton",
                "ChangeMapButton",
                "ControlsButton",
                "LeaveButton",
                "PauseQuitButton",
                "PauseNote",
            ],
            size,
        );
        assert_eq!((p[2], p[3]), (320, 508), "the pause panel keeps its size");
    }
}

#[test]
fn the_controls_panel_stays_in_the_middle_of_the_view_with_its_keys_in_two_columns() {
    let mut g = client();
    g.call("hostGame", &["lan"]).unwrap();
    for _ in 0..200 {
        g.step(32);
        if g.in_game() {
            break;
        }
    }
    assert!(g.in_game());
    g.call("pauseGame", &[]).unwrap();
    g.call("showControls", &[]).unwrap();
    assert_eq!(
        g.call(
            "eval",
            &["return ControlsPanel.isVisible() SPC PausePanel.isVisible();"]
        )
        .unwrap(),
        "1 0"
    );
    let rows: usize = g
        .call("eval", &["return ControlsList.getCount();"])
        .unwrap()
        .parse()
        .unwrap();
    // Three headings, and a key and its action for each of the 13 rows.
    assert_eq!(rows, 3 + 2 * 13);
    for size in SIZES {
        resize(&mut g, size);
        let p = check(
            &mut g,
            "ControlsPanel",
            &[
                "ControlsTitle",
                "ControlsList",
                "ControlsBackButton",
                "ControlsNote",
            ],
            size,
        );
        assert_eq!(
            (p[2], p[3]),
            (772, 412),
            "the controls panel keeps its size"
        );
    }
    // Back shows the menu again.
    g.call("hideControls", &[]).unwrap();
    assert_eq!(
        g.call(
            "eval",
            &["return ControlsPanel.isVisible() SPC PausePanel.isVisible();"]
        )
        .unwrap(),
        "0 1"
    );
}

/// In a view narrower than the two columns (a small window or a browser's canvas) the groups stack in one column, and where
/// the list is taller than the view it scrolls: the panel stays in the view, centered, its parts clear of each other.
#[test]
fn the_controls_panel_stacks_and_scrolls_in_a_small_view() {
    let mut g = client();
    g.call("hostGame", &["lan"]).unwrap();
    for _ in 0..200 {
        g.step(32);
        if g.in_game() {
            break;
        }
    }
    assert!(g.in_game());
    g.call("pauseGame", &[]).unwrap();
    g.call("showControls", &[]).unwrap();
    for size in [(640, 480), (480, 360), (780, 600), (800, 600), (1280, 720)] {
        resize(&mut g, size);
        let p = check(
            &mut g,
            "ControlsPanel",
            &[
                "ControlsTitle",
                "ControlsScroll",
                "ControlsBackButton",
                "ControlsNote",
            ],
            size,
        );
        let stacked = size.0 < 788;
        assert_eq!(
            g.global("$Pause::ControlsStacked") == "1",
            stacked,
            "{size:?}: {p:?}"
        );
        let scroll = rect(&mut g, "ControlsScroll");
        let list = rect(&mut g, "ControlsList");
        // The list's whole height, or the part that fits beside the title and the Back button when the view is short.
        let short = list[3] > size.1 - 216;
        assert_eq!(list[3] > scroll[3], short, "{size:?}: {scroll:?} {list:?}");
        if size.1 <= 480 {
            assert!(short, "{size:?}: the list scrolls in a short view");
        }
        let note = g
            .call("eval", &["return ControlsNote.getValue();"])
            .unwrap();
        assert_eq!(
            note.starts_with("Mouse wheel: more keys"),
            short,
            "{note:?}"
        );
    }
}

#[test]
fn the_hosts_map_list_stays_in_the_middle_of_the_view_and_lists_the_maps() {
    let mut g = client();
    g.call("hostGame", &["lan"]).unwrap();
    for _ in 0..400 {
        g.step(32);
        if g.in_game() && g.global("$Client::MapCount") != "0" {
            break;
        }
    }
    let maps: usize = g.global("$Client::MapCount").parse().unwrap();
    assert!(maps > 0, "the host was sent the maps");
    g.call("pauseGame", &[]).unwrap();
    g.call("showMapPick", &[]).unwrap();
    for size in SIZES {
        resize(&mut g, size);
        let p = check(
            &mut g,
            "MapPanel",
            &["MapTitle", "MapScroll", "MapLoadButton", "MapBackButton"],
            size,
        );
        assert_eq!((p[2], p[3]), (320, 440), "the map panel keeps its size");
    }
}

#[test]
fn a_browser_pages_main_menu_is_a_centered_column_at_wide_and_tall_sizes() {
    let mut g = client();
    g.set_global("$Arena::JoinOnly", "1");
    g.set_global("$Arena::Solo", "1");
    for size in [(2560, 1080), (1000, 700), (800, 1400), (2217, 935)] {
        resize(&mut g, size);
        g.call("showMainMenu", &[]).unwrap();
        g.step(64);
        check(
            &mut g,
            "MainPanel",
            &[
                "MainTitle",
                "MainLine",
                "NameEdit",
                "HostButton",
                "JoinButton",
                "QuitButton",
                "HostNote",
                "JoinNote",
                "MainStatus",
            ],
            size,
        );
    }
}
