//! The mouse and the keys as the window posts them to the client host: over the menu the pointer hovers a button and the
//! name box takes typed keys, the system cursor stands in for the canvas's (the arena sets no GuiCursor), and in the arena
//! the keys and the mouse reach the play map.

use torque::client::input::{WM_LBUTTONDOWN, WM_LBUTTONUP};
use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;

/// Virtual-key codes of the keys the tests press.
const VK_RETURN: u8 = 0x0d;
const VK_ESCAPE: u8 = 0x1b;

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

fn object(game: &ArenaGame, name: &str) -> u32 {
    game.vm()
        .objects()
        .find(name.as_bytes())
        .unwrap_or_else(|| panic!("{name}"))
}

/// The center of a control placed straight on the menu.
fn center(game: &ArenaGame, name: &str) -> (i32, i32) {
    let id = object(game, name);
    let field = |f: &[u8]| -> Vec<i32> {
        let text = game.vm().objects().field(id, f, b"").unwrap_or_default();
        String::from_utf8_lossy(&text)
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect()
    };
    let (pos, ext) = (field(b"position"), field(b"extent"));
    (pos[0] + ext[0] / 2, pos[1] + ext[1] / 2)
}

fn click(game: &mut ArenaGame, at: (i32, i32)) {
    let lparam = ((at.1 as u32 & 0xffff) << 16) | (at.0 as u32 & 0xffff);
    game.host.mouse_move(at);
    game.host.post(WM_LBUTTONDOWN, 1, lparam);
    game.host.post(WM_LBUTTONUP, 0, lparam);
}

fn press(game: &mut ArenaGame, vk: u8) {
    game.key(vk, true);
    game.key(vk, false);
}

fn over(game: &ArenaGame, name: &str) -> bool {
    let id = object(game, name);
    game.host.gui.with(id, |c| c.mouse_over).unwrap()
}

#[test]
fn the_pointer_hovers_the_menu_buttons_and_the_name_box_takes_keys() {
    let mut g = client();
    let host = center(&g, "HostButton");
    g.host.mouse_move(host);
    assert!(over(&g, "HostButton"), "the pointer over Host hovers it");
    let join = center(&g, "JoinButton");
    g.host.mouse_move(join);
    assert!(!over(&g, "HostButton"), "leaving Host lets it go");
    assert!(over(&g, "JoinButton"));

    let name = center(&g, "NameEdit");
    click(&mut g, name);
    for vk in *b"BOB" {
        press(&mut g, vk);
    }
    g.step(32);
    assert_eq!(
        g.call("eval", &["return NameEdit.getValue();"]).unwrap(),
        "Annbob",
        "the clicked name box takes the keys"
    );
    assert!(g.host.input_errors.is_empty(), "{:?}", g.host.input_errors);
}

#[test]
fn the_system_cursor_stands_in_while_the_canvas_draws_none() {
    let mut g = client();
    g.host.draw_frame();
    assert!(!g.host.wants_mouse());
    assert!(
        g.host.system_cursor(),
        "no GuiCursor is set: the system's cursor is shown"
    );
    g.call("eval", &["Canvas.cursorOff();"]).unwrap();
    g.host.draw_frame();
    assert!(!g.host.system_cursor(), "the cursor off: none");
    g.call("eval", &["Canvas.cursorOn(); Canvas.hideCursor();"])
        .unwrap();
    g.host.draw_frame();
    assert!(!g.host.system_cursor(), "the game hid the cursor: none");

    // A GuiCursor whose bitmap does not load draws nothing: the system's again. One that loads hides it.
    g.call(
        "eval",
        &["Canvas.showCursor(); new GuiCursor(NoCursor) { bitmapName = \"none/here\"; }; Canvas.setCursor(NoCursor);"],
    )
    .unwrap();
    g.host.draw_frame();
    assert!(g.host.system_cursor(), "a cursor that did not load");
    let spark = g
        .files
        .kit
        .paths()
        .into_iter()
        .find(|p| p.contains("spark"));
    let spark = spark.expect("the kit's spark").to_string();
    let set = format!(
        "new GuiCursor(KitCursor) {{ bitmapName = \"{spark}\"; hotSpot = \"1 1\"; }}; Canvas.setCursor(KitCursor);"
    );
    g.call("eval", &[set.as_str()]).unwrap();
    g.host.draw_frame();
    assert!(!g.host.system_cursor(), "the canvas draws the game's own");
}

#[test]
fn in_the_arena_the_keys_and_the_mouse_reach_the_play_map() {
    let mut g = client();
    let host = center(&g, "HostButton");
    click(&mut g, host);
    for _ in 0..200 {
        g.step(32);
        if g.in_game() {
            break;
        }
    }
    assert!(g.in_game(), "the Host button started a game");
    g.host.draw_frame();
    assert!(g.host.wants_mouse(), "the arena holds the mouse");
    assert!(!g.host.system_cursor());

    g.key(b'W', true);
    assert_eq!(g.global("$mvForwardAction"), "1", "W moves forward");
    g.key(b'W', false);
    assert_eq!(g.global("$mvForwardAction"), "0");

    let yaw = g.global("$Arena::LookYaw");
    g.host.mouse_motion(40.0, 0.0);
    assert_ne!(g.global("$Arena::LookYaw"), yaw, "the mouse turns the view");

    let shots = g.global("$mvTriggerCount0");
    click(&mut g, (400, 300));
    assert_ne!(g.global("$mvTriggerCount0"), shots, "a click fires");

    press(&mut g, b'T');
    g.step(32);
    assert_eq!(g.global("$Arena::Chatting"), "1", "T opens the chat box");
    for vk in *b"HI" {
        press(&mut g, vk);
    }
    assert_eq!(
        g.call("eval", &["return ChatEdit.getValue();"]).unwrap(),
        "hi"
    );
    press(&mut g, VK_RETURN);
    assert_eq!(g.global("$Arena::Chatting"), "0", "Return sends the line");

    press(&mut g, VK_ESCAPE);
    g.step(32);
    assert!(!g.in_game(), "Escape leaves the arena");
    assert!(g.host.input_errors.is_empty(), "{:?}", g.host.input_errors);
}
