//! A hit's reaction on the level demo's figure, played by its server (common/server.cs's `playerFlinch`), standing and
//! crouched: its drawn head snaps back while its first-person view (the engine's steady eye) does not move. No window and no
//! GPU: the client and the server in one process, the frames placed and not drawn.

use torque::client::input::Key;
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

fn eval_on_server(game: &mut LevelDemo, text: &str) -> String {
    let server = game.server.as_mut().expect("the demo hosts its map");
    let vm = std::borrow::BorrowMut::<torque::app::server::GameServer>::borrow_mut(server).vm_mut();
    String::from_utf8(vm.eval(text.as_bytes()).unwrap()).unwrap()
}

#[test]
fn a_hit_moves_the_drawn_body_but_never_the_first_person_view() {
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
    for (keys, pose) in [(&[][..], "standing"), (&[Key::ControlLeft][..], "crouched")] {
        for k in keys {
            game.key(*k, true);
        }
        eyes(&mut game, 40);
        let before = eyes(&mut game, 4);
        // A shot flying back along -y into its face: its head snaps back (common/server.cs).
        eval_on_server(
            &mut game,
            "playerFlinch(ClientGroup.getObject(0).player, \"0 -1 0\", \"head\");",
        );
        let hit = eyes(&mut game, 30);
        let (view0, drawn0) = before[before.len() - 1];
        let view_moved = hit
            .iter()
            .map(|(v, _)| (0..3).map(|k| (v[k] - view0[k]).abs()).fold(0.0, f32::max))
            .fold(0.0, f32::max);
        let drawn_moved = hit
            .iter()
            .map(|(_, d)| (0..3).map(|k| (d[k] - drawn0[k]).abs()).fold(0.0, f32::max))
            .fold(0.0, f32::max);
        assert!(
            drawn_moved > 0.03,
            "{pose}: the head snaps back ({drawn_moved})"
        );
        assert!(
            view_moved < 0.001,
            "{pose}: the view stays still through the hit ({view_moved})"
        );
        for k in keys {
            game.key(*k, false);
        }
    }
}
