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
    g.call("pauseGame", &[]).unwrap();
    for size in SIZES {
        resize(&mut g, size);
        let p = check(
            &mut g,
            "PausePanel",
            &[
                "PauseTitle",
                "ResumeButton",
                "LeaveButton",
                "PauseQuitButton",
                "PauseNote",
            ],
            size,
        );
        assert_eq!((p[2], p[3]), (320, 356), "the pause panel keeps its size");
    }
}
