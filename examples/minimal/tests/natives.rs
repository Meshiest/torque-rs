//! The natives the minimal example's script engines have, compared with the snapshot files in `tests/natives/`. A change to what
//! scripts can call shows as a diff here; `TORQUE_BLESS=1 cargo test -p torque-example-minimal --features window --test
//! natives` rewrites them.

use std::path::{Path, PathBuf};

use torque::script::vm::listing::check_listing;

fn listing(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/natives")
        .join(name)
}

#[test]
fn the_server_has_these_natives() {
    let vm = torque_example_minimal::game::load(torque_example_minimal::game::bundled_root())
        .expect("the bundled game loads");
    check_listing(&listing("server.txt"), &vm.native_listing());
}

/// The console client (`min-client`) has no script engine; the window client (feature `window`) has the GUI's.
#[cfg(feature = "window")]
#[test]
fn the_window_client_has_these_natives() {
    let window =
        torque_example_minimal::window::ExampleWindow::new("127.0.0.1:1".parse().unwrap(), true)
            .expect("the window client");
    check_listing(&listing("client.txt"), &window.vm().native_listing());
}
