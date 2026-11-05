//! The browser's game without a browser: the client a web build makes (no socket of its own, no list of games) on the files of
//! its pack, played alone at once; the figure walks.

use torque::client::input::Key;
use torque_example_vehicle_demo::files::{GameFiles, web_pack};
use torque_example_vehicle_demo::web::alone;

#[test]
fn the_web_builds_game_plays_alone_on_its_pack_and_the_figure_walks() {
    let pack = web_pack(&GameFiles::bundled().expect("the game folder"));
    let files = GameFiles::unpack(&pack).expect("the pack reads back");
    let mut game = alone(files).expect("the game starts");
    assert!(game.local(), "an in-memory link, no socket");
    let mut steps = 0;
    while !(game.in_world() && game.figure().is_some()) {
        assert!(steps < 320, "not in the world after ten seconds");
        game.step(32);
        game.update_scene(0.032);
        steps += 1;
    }
    let from = game.figure().unwrap().position;
    game.key(Key::W, true);
    for _ in 0..60 {
        game.step(32);
        game.update_scene(0.032);
    }
    game.key(Key::W, false);
    let to = game.figure().unwrap().position;
    let moved = (to[0] - from[0]).hypot(to[1] - from[1]);
    assert!(moved > 1.0, "{from:?} to {to:?}");
}
