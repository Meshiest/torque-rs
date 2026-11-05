//! Tab's camera: from the own figure's eye it moves behind the figure (shown once out of its head, its pistol in its hand),
//! stops in front of a block behind it, and moves back to the eye (the figure hidden again). The game plays
//! `game/fixtures/aim.mis`: a floor and one block (x 2 to 4, y 7 to 9, 3 high).

use torque::client::input::Key;
use torque_example_fps::client::{
    ArenaGame, CAMERA_DISTANCE, CAMERA_LIFT, CAMERA_SIDE, ClientOptions, NetMode,
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
