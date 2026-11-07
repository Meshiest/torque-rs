//! The window client's game half without a window: it runs the example server itself, connects, and its GUI shows the ghosts.
#![cfg(feature = "window")]

use torque::client::platform::window::WindowGame;
use torque_example_minimal::window::ExampleWindow;

#[test]
fn the_window_clients_gui_shows_the_ghosts_it_is_sent() {
    let mut w =
        ExampleWindow::new("127.0.0.1:1".parse().unwrap(), true).expect("the window client");
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(10) && !w.status().starts_with('3') {
        w.step(32);
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
    assert!(w.status().starts_with("3 ghosts"), "{}", w.status());
    let boxes = (0..torque_example_minimal::window::MAX_BOXES)
        .filter_map(|i| w.ghost_box(i))
        .count();
    assert_eq!(boxes, 3, "the crate, the gem and the camera");
    w.finish();
}
