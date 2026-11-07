//! The window client's game half without a window: it runs the example server itself, connects, and its GUI shows the ghosts.
#![cfg(feature = "window")]

use std::time::{Duration, Instant};

use torque::app::App;
use torque::app::plugins::DefaultPlugins;
use torque::engine::EngineBuilder;
use torque_example_minimal_engine::window::ExampleGame;
use torque_example_minimal_engine::{ExamplePlugin, MAX_BOXES, files};

/// What the client's scripts answer for `code`.
fn eval(game: &mut ExampleGame, code: &str) -> String {
    let answer = game
        .client
        .vm_mut()
        .eval(code.as_bytes())
        .expect("the script");
    String::from_utf8_lossy(&answer).into_owned()
}

#[test]
fn the_window_clients_gui_shows_the_ghosts_it_is_sent() {
    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, ExamplePlugin(files().unwrap())))
        .build()
        .unwrap();
    let mut game = ExampleGame::new(&engine, None).expect("the window client");
    let status = "return ExampleStatus.getValue();";
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) && !eval(&mut game, status).starts_with('3') {
        game.advance(32);
        std::thread::sleep(Duration::from_millis(4));
    }
    assert_eq!(eval(&mut game, status), "3 ghosts");
    let visible = format!(
        "%n = 0; for (%i = 0; %i < {MAX_BOXES}; %i++) if ((\"ExampleBox\" @ %i).isVisible()) %n++; return %n;"
    );
    assert_eq!(
        eval(&mut game, &visible),
        "3",
        "the crate, the gem and the camera"
    );
}
