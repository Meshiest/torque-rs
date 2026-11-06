//! Tab's camera: from the own figure's eye it moves behind the figure (shown once out of its head, its pistol in its hand),
//! stops in front of a block behind it, and moves back to the eye (the figure hidden again). The game plays
//! `game/fixtures/aim.mis`: a floor and one block (x 2 to 4, y 7 to 9, 3 high).

use torque::client::input::Key;
use torque_example_fps::client::{
    ArenaGame, CAMERA_DISTANCE, CAMERA_LIFT, CAMERA_NEAREST, CAMERA_SIDE, ClientOptions, NetMode,
};
use torque_example_fps::files::GameFiles;
use torque_render_scene::source::Eye;

fn host() -> ArenaGame {
    let mut a = ArenaGame::new(
        GameFiles::bundled().unwrap(),
        ClientOptions {
            net: NetMode::Loopback,
            name: "Ann".to_string(),
            port: 0,
            map: Some("fixtures/aim.mis".to_string()),
            ..ClientOptions::default()
        },
    )
    .unwrap();
    a.call("hostGame", &[]).unwrap();
    for _ in 0..300 {
        frame(&mut a);
        if a.in_game() && a.own_look().is_some_and(|o| o.position.is_some()) {
            break;
        }
    }
    assert!(a.in_game(), "in the game");
    frames(&mut a, 30);
    a
}

/// 16 ms of the clock and a frame after it, as the window runs them: the eye it draws.
fn frame(a: &mut ArenaGame) -> Eye {
    a.step(16);
    a.add_frame_time(0.016);
    a.view_frame();
    a.drawn_eye()
}

fn frames(a: &mut ArenaGame, n: usize) -> Eye {
    let mut e = frame(a);
    for _ in 1..n {
        e = frame(a);
    }
    e
}

fn tab(a: &mut ArenaGame) {
    a.key(Key::Tab, true);
    a.key(Key::Tab, false);
}

fn apart(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt()
}

fn hidden(a: &ArenaGame) -> bool {
    a.with_world(|w| w.hide_control).unwrap()
}

#[test]
fn tab_eases_the_camera_behind_the_figure_keeps_it_off_a_block_and_back() {
    let mut a = host();
    let eye = frames(&mut a, 2);
    assert!(!a.third_person());
    assert_eq!(eye.position, a.eye().position, "at the eye");
    assert!(hidden(&a), "the figure hidden in the first person");

    tab(&mut a);
    let first = frame(&mut a);
    assert!(a.camera_blending(), "eased, not cut");
    assert!(apart(first.position, a.eye().position) < CAMERA_DISTANCE * 0.5);
    // Shown once the camera is out of the head, early in the move.
    let mut out = 1;
    while hidden(&a) {
        frame(&mut a);
        out += 1;
        assert!(out < 12, "still hidden {out} frames into the move");
    }
    assert!(a.camera_blending());
    let behind = frames(&mut a, 40);
    assert!(a.third_person() && !a.camera_blending());
    let whole = (CAMERA_DISTANCE.powi(2) + CAMERA_SIDE.powi(2) + CAMERA_LIFT.powi(2)).sqrt();
    let back = apart(behind.position, a.eye().position);
    assert!(
        (back - whole).abs() < 0.3,
        "{back} behind the eye, want about {whole}"
    );
    assert!(!hidden(&a));

    // Two units north of the block, facing north: the camera stops in front of the block's north face (y 9).
    let server = a.server.as_mut().unwrap();
    server
        .call("placeFigure", &["Ann", "3", "11", "0.1", "0"])
        .unwrap();
    let stopped = frames(&mut a, 40);
    let head = a.eye().position;
    assert!(head[1] > 10.5, "the figure moved: {head:?}");
    assert!(
        stopped.position[1] > 9.1 && stopped.position[1] < head[1] - 1.0,
        "the camera at {:?}, the eye at {head:?}",
        stopped.position
    );
    assert!(!hidden(&a));

    tab(&mut a);
    let back_home = frames(&mut a, 40);
    assert!(!a.third_person() && !a.camera_blending());
    assert_eq!(back_home.position, a.eye().position);
    assert!(hidden(&a), "hidden once the camera is at the eye");
}

#[test]
fn behind_a_crouched_or_prone_figure_the_camera_rides_lower_and_stays_above_the_floor() {
    let mut a = host();
    tab(&mut a);
    let stand = frames(&mut a, 60).position;
    a.call("crouchKey", &["1"]).unwrap();
    let crouch = frames(&mut a, 60).position;
    let crouch_eye = a.eye().position;
    a.call("proneKey", &["1"]).unwrap();
    let prone = frames(&mut a, 120).position;
    let prone_eye = a.eye().position;
    assert!(
        stand[2] > crouch[2] + 0.2 && crouch[2] > prone[2] + 0.2,
        "the camera rides lower with the eye: {stand:?} {crouch:?} {prone:?} (eyes {crouch_eye:?} {prone_eye:?})"
    );
    let back = apart(prone, prone_eye);
    assert!(back > CAMERA_DISTANCE * 0.8, "{back} behind the eye");
    // Looking up: the boom swings down behind the figure toward the floor (its top at 0) and stops above it, behind the
    // figure while there is room for the camera there; with none (the head right up), the view is the eye's (`CAMERA_NEAREST`).
    for _ in 0..8 {
        a.host.mouse_motion(0.0, -20.0);
        frame(&mut a);
    }
    let up = frames(&mut a, 60).position;
    let eye = a.eye().position;
    assert!(
        up[2] > 0.05 && up[2] < eye[2] + 0.1 && apart(up, eye) > CAMERA_NEAREST,
        "the camera above the floor, low behind the figure: {up:?} (eye {eye:?})"
    );
    for _ in 0..30 {
        a.host.mouse_motion(0.0, -20.0);
        let at = frame(&mut a).position;
        assert!(at[2] > 0.05, "never under the floor: {at:?}");
    }
}

/// The drawn eyes from now until the camera settles at the new place, after `teleport` (none farther than `near` from either
/// the eye before it or the one it settles at: a cut, not a move through what lies between).
fn cut_on(a: &mut ArenaGame, teleport: impl FnOnce(&mut ArenaGame), label: &str) {
    let before = frames(a, 60).position;
    teleport(a);
    let mut drawn = Vec::new();
    for _ in 0..90 {
        drawn.push(frame(a).position);
    }
    let after = *drawn.last().unwrap();
    assert!(
        apart(before, after) > 10.0,
        "{label}: the figure went far ({before:?} to {after:?})"
    );
    let near = 1.0;
    for (i, p) in drawn.iter().enumerate() {
        let gap = apart(*p, before).min(apart(*p, after));
        assert!(
            gap < near,
            "{label}: frame {i} drew the eye at {p:?}, {gap} m from both {before:?} and {after:?}: {drawn:?}"
        );
    }
}

fn place_far(a: &mut ArenaGame) {
    let server = a.server.as_mut().unwrap();
    server
        .call("placeFigure", &["Ann", "20", "20", "0.1", "0"])
        .unwrap();
}

fn respawn(a: &mut ArenaGame) {
    let vm = a.server.as_mut().unwrap().vm_mut();
    vm.eval(b"ClientGroup.getObject(0).spawnFigure();").unwrap();
}

#[test]
fn a_teleport_or_a_respawn_cuts_the_camera_in_either_person() {
    for behind in [false, true] {
        let person = if behind {
            "third person"
        } else {
            "first person"
        };
        let mut a = host();
        if behind {
            tab(&mut a);
        }
        frames(&mut a, 60);
        assert_eq!(a.third_person(), behind);
        cut_on(&mut a, place_far, &format!("{person}, placed far"));
        cut_on(&mut a, respawn, &format!("{person}, respawned"));
        cut_on(&mut a, place_far, &format!("{person}, placed far again"));
    }
}
