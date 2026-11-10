//! The natives the character example's client script engine has, compared with the snapshot in `tests/natives/client.txt`. A
//! change to what scripts can call shows as a diff here; `TORQUE_BLESS=1 cargo test -p torque-example-character --test
//! natives` rewrites it. The example has no server.

use std::path::Path;

use torque::script::vm::listing::check_listing;
use torque_example_character::game::CharacterGame;

#[test]
fn the_client_has_these_natives() {
    let game = CharacterGame::new().unwrap();
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/natives/client.txt");
    check_listing(&file, &game.vm().native_listing());
}
