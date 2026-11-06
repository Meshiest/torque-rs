//! The browser's game without a browser: the client a web build makes (no socket of its own, no list of games) on the files of
//! its pack, played alone at once; the figure walks. Its menu offers only to play alone.

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

#[test]
fn the_web_builds_menu_offers_only_to_play_alone() {
    let pack = web_pack(&GameFiles::bundled().expect("the game folder"));
    let mut game =
        alone(GameFiles::unpack(&pack).expect("the pack reads back")).expect("the game starts");
    game.client.call("showMenu", &[]).unwrap();
    let mut eval = |code: &str| game.client.call("eval", &[code]).unwrap();
    assert_eq!(eval("return AloneButton.isVisible();"), "1");
    for hidden in [
        "HostButton",
        "GamesLabel",
        "GamesScroll",
        "RefreshButton",
        "JoinSelectedButton",
        "AddressLabel",
        "AddressEdit",
        "JoinAddressButton",
    ] {
        assert_eq!(
            eval(&format!("return {hidden}.isVisible();")),
            "0",
            "{hidden}"
        );
    }
    assert!(!eval("return MenuLine.getValue();").contains("network"));
}

#[test]
fn a_page_not_played_alone_also_offers_to_host_and_join_by_room_code() {
    let pack = web_pack(&GameFiles::bundled().expect("the game folder"));
    let mut game =
        alone(GameFiles::unpack(&pack).expect("the pack reads back")).expect("the game starts");
    game.set_global("$World::WebRooms", "1");
    game.client.call("showMenu", &[]).unwrap();
    let mut eval = |code: &str| game.client.call("eval", &[code]).unwrap();
    for shown in [
        "AloneButton",
        "HostButton",
        "AddressLabel",
        "AddressEdit",
        "JoinAddressButton",
    ] {
        assert_eq!(
            eval(&format!("return {shown}.isVisible();")),
            "1",
            "{shown}"
        );
    }
    // The network's list stays hidden: a browser finds games by room code only.
    for hidden in [
        "GamesLabel",
        "GamesScroll",
        "RefreshButton",
        "JoinSelectedButton",
    ] {
        assert_eq!(
            eval(&format!("return {hidden}.isVisible();")),
            "0",
            "{hidden}"
        );
    }
    assert_eq!(eval("return JoinAddressButton.getText();"), "Join room");
    assert!(!eval("return MenuLine.getValue();").contains("network"));
    // Host asks the page to host; a code typed in asks it to join that room, and none asks nothing.
    eval("JoinAddressButton.performClick();");
    assert_eq!(eval("return $World::WebJoinCode;"), "");
    eval("AddressEdit.setValue(\"abcd-efgh\"); joinRoomCode();");
    assert_eq!(eval("return $World::WebJoinCode;"), "abcd-efgh");
    eval("hostOnPage();");
    assert_eq!(eval("return $World::WebHostAsked;"), "1");
    // A hosted game's code shows on the menu.
    eval("$World::RoomCode = \"ABCD-EFGH\"; showMenu();");
    assert!(eval("return MenuStatus.getValue();").contains("ABCD-EFGH"));
}
