//! The mouse and the keys as the window posts them to the client host: over the menu the pointer hovers a button and the
//! name box takes typed keys, the system cursor stands in for the canvas's (the arena sets no GuiCursor), in the arena the
//! keys and the mouse reach the play map, and Escape opens the pause menu over the view.

use torque::client::host::CursorShape;
use torque::client::input::{Key, WM_LBUTTONDOWN, WM_LBUTTONUP};
use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;

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
fn center(game: &mut ArenaGame, name: &str) -> (i32, i32) {
    let [x, y, w, h] = game.control_rect(name).unwrap();
    (x + w / 2, y + h / 2)
}

fn click(game: &mut ArenaGame, at: (i32, i32)) {
    let lparam = ((at.1 as u32 & 0xffff) << 16) | (at.0 as u32 & 0xffff);
    game.host.mouse_move(at);
    game.host.post(WM_LBUTTONDOWN, 1, lparam);
    game.host.post(WM_LBUTTONUP, 0, lparam);
}

fn press(game: &mut ArenaGame, key: Key) {
    game.key(key, true);
    game.key(key, false);
}

fn over(game: &ArenaGame, name: &str) -> bool {
    let id = object(game, name);
    game.host.gui.with(id, |c| c.mouse_over).unwrap()
}

#[test]
fn the_pointer_hovers_the_menu_buttons_and_the_name_box_takes_keys() {
    let mut g = client();
    let host = center(&mut g, "HostButton");
    g.host.mouse_move(host);
    assert!(over(&g, "HostButton"), "the pointer over Host hovers it");
    let join = center(&mut g, "JoinButton");
    g.host.mouse_move(join);
    assert!(!over(&g, "HostButton"), "leaving Host lets it go");
    assert!(over(&g, "JoinButton"));

    let name = center(&mut g, "NameEdit");
    click(&mut g, name);
    for key in [Key::B, Key::O, Key::B] {
        press(&mut g, key);
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
fn the_menu_draws_its_text_with_the_system_fonts() {
    if torque::fonts::system::SystemFonts::new().is_none() {
        // CI's images have fonts (the test job installs DejaVu); elsewhere a system with none skips.
        assert!(std::env::var_os("CI").is_none(), "CI has no font files");
        eprintln!("this system has no font files: skipped");
        return;
    }
    let mut g = client();
    let draws = g.host.draw_frame();
    assert_eq!(
        draws.report.no_font, 0,
        "text drawn with no font: {:?}",
        draws.report.missing_fonts
    );
    let glyphs: usize = draws
        .batches
        .iter()
        .filter(|b| {
            b.texture
                .as_ref()
                .is_some_and(|t| t.name.starts_with("font:"))
        })
        .map(|b| b.quads.len())
        .sum();
    assert!(glyphs > 20, "the menu's glyphs drawn: {glyphs}");
}

/// `key` pressed with Ctrl held.
fn ctrl(game: &mut ArenaGame, key: Key) {
    game.key(Key::ControlLeft, true);
    press(game, key);
    game.key(Key::ControlLeft, false);
}

fn name_value(game: &mut ArenaGame) -> String {
    game.call("eval", &["return NameEdit.getValue();"]).unwrap()
}

#[test]
fn the_name_box_takes_the_desktop_editing_keys() {
    let mut g = client();
    let name = center(&mut g, "NameEdit");
    click(&mut g, name);
    for key in [Key::B, Key::O, Key::B, Key::Space, Key::J, Key::O] {
        press(&mut g, key);
    }
    assert_eq!(name_value(&mut g), "Annbob jo");
    ctrl(&mut g, Key::Backspace);
    assert_eq!(
        name_value(&mut g),
        "Annbob ",
        "Ctrl+Backspace deletes a word"
    );
    ctrl(&mut g, Key::Z);
    assert_eq!(name_value(&mut g), "Annbob jo", "Ctrl+Z undoes it");
    ctrl(&mut g, Key::A);
    press(&mut g, Key::X);
    assert_eq!(
        name_value(&mut g),
        "x",
        "Ctrl+A selects all, typing replaces it"
    );
    ctrl(&mut g, Key::Z);
    assert_eq!(name_value(&mut g), "Annbob jo");
    ctrl(&mut g, Key::Y);
    assert_eq!(name_value(&mut g), "x", "Ctrl+Y redoes");
    assert!(g.host.input_errors.is_empty(), "{:?}", g.host.input_errors);
}

#[test]
fn the_system_cursor_is_a_pointer_over_buttons_and_a_text_cursor_over_the_name_box() {
    let mut g = client();
    g.host.draw_frame();
    assert!(g.host.system_cursor());
    let host = center(&mut g, "HostButton");
    g.host.mouse_move(host);
    assert_eq!(g.host.cursor_shape(), CursorShape::Pointer, "a button");
    let name = center(&mut g, "NameEdit");
    g.host.mouse_move(name);
    assert_eq!(g.host.cursor_shape(), CursorShape::Text, "the name box");
    g.host.mouse_move((1, 1));
    assert_eq!(g.host.cursor_shape(), CursorShape::Arrow, "the bare panel");
    g.host.mouse_move(host);
    assert_eq!(g.host.cursor_shape(), CursorShape::Pointer);
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
    let host = center(&mut g, "HostButton");
    click(&mut g, host);
    g.step(32);
    let lan = center(&mut g, "HostLanButton");
    click(&mut g, lan);
    for _ in 0..200 {
        g.step(32);
        if g.in_game() {
            break;
        }
    }
    assert!(g.in_game(), "the Host and LAN only buttons started a game");
    g.host.draw_frame();
    assert!(g.host.wants_mouse(), "the arena holds the mouse");
    assert!(!g.host.system_cursor());

    g.key(Key::W, true);
    assert_eq!(g.global("$mvForwardAction"), "0.35", "W walks forward");
    g.key(Key::W, false);
    assert_eq!(g.global("$mvForwardAction"), "0");

    let yaw = g.global("$Arena::LookYaw");
    g.host.mouse_motion(40.0, 0.0);
    assert_ne!(g.global("$Arena::LookYaw"), yaw, "the mouse turns the view");

    let shots = g.global("$mvTriggerCount0");
    click(&mut g, (400, 300));
    assert_ne!(g.global("$mvTriggerCount0"), shots, "a click fires");

    press(&mut g, Key::T);
    g.step(32);
    assert_eq!(g.global("$Arena::Chatting"), "1", "T opens the chat box");
    for key in [Key::H, Key::I] {
        press(&mut g, key);
    }
    assert_eq!(
        g.call("eval", &["return ChatEdit.getValue();"]).unwrap(),
        "hi"
    );
    press(&mut g, Key::Enter);
    assert_eq!(g.global("$Arena::Chatting"), "0", "Return sends the line");

    press(&mut g, Key::Escape);
    g.step(32);
    assert_eq!(g.global("$Client::Paused"), "1", "Escape pauses");
    assert!(g.in_game(), "and stays in the arena");
    assert!(g.host.input_errors.is_empty(), "{:?}", g.host.input_errors);
}

/// A client in the arena it hosts.
fn in_arena() -> ArenaGame {
    let mut g = client();
    let host = center(&mut g, "HostButton");
    click(&mut g, host);
    g.step(32);
    let lan = center(&mut g, "HostLanButton");
    click(&mut g, lan);
    for _ in 0..200 {
        g.step(32);
        if g.in_game() {
            break;
        }
    }
    assert!(g.in_game(), "the Host and LAN only buttons started a game");
    g.host.draw_frame();
    g
}

/// The center of a control in the pause panel.
fn in_panel(game: &mut ArenaGame, name: &str) -> (i32, i32) {
    center(game, name)
}

#[test]
fn escape_pauses_over_the_view_and_resume_or_escape_goes_back() {
    let mut g = in_arena();
    press(&mut g, Key::Escape);
    g.step(32);
    g.host.draw_frame();
    assert_eq!(g.global("$Client::Paused"), "1");
    assert!(
        g.host.system_cursor(),
        "the cursor is on over the pause menu"
    );
    assert!(!g.host.wants_mouse(), "the mouse is free");
    // The figure does not move while paused: W goes nowhere.
    g.key(Key::W, true);
    assert_eq!(
        g.global("$mvForwardAction"),
        "0",
        "W does not reach the play map"
    );
    g.key(Key::W, false);
    let resume = in_panel(&mut g, "ResumeButton");
    g.host.mouse_move(resume);
    assert!(over(&g, "ResumeButton"), "the pointer hovers Resume");

    // Resume: back to the game, the keys on the figure again.
    click(&mut g, resume);
    g.step(32);
    g.host.draw_frame();
    assert_eq!(g.global("$Client::Paused"), "0", "Resume goes back");
    assert!(g.host.wants_mouse(), "the arena holds the mouse again");
    g.key(Key::W, true);
    assert_eq!(g.global("$mvForwardAction"), "0.35", "W walks again");
    g.key(Key::W, false);

    // Escape twice: paused, then back.
    press(&mut g, Key::Escape);
    g.step(32);
    assert_eq!(g.global("$Client::Paused"), "1");
    press(&mut g, Key::Escape);
    g.step(32);
    assert_eq!(g.global("$Client::Paused"), "0", "Escape again goes back");
    g.key(Key::W, true);
    assert_eq!(g.global("$mvForwardAction"), "0.35");
    g.key(Key::W, false);

    // T still opens the chat when not paused.
    press(&mut g, Key::T);
    g.step(32);
    assert_eq!(g.global("$Arena::Chatting"), "1");
    press(&mut g, Key::Escape);
    assert_eq!(g.global("$Arena::Chatting"), "0", "Escape closes the chat");
    assert_eq!(g.global("$Client::Paused"), "0", "without pausing");
    assert!(g.host.input_errors.is_empty(), "{:?}", g.host.input_errors);
}

#[test]
fn leave_game_from_the_pause_menu_returns_to_the_main_menu() {
    let mut g = in_arena();
    press(&mut g, Key::Escape);
    g.step(32);
    g.host.draw_frame();
    let leave = in_panel(&mut g, "LeaveButton");
    click(&mut g, leave);
    g.step(32);
    assert!(!g.in_game(), "Leave game leaves the arena");
    assert_eq!(g.global("$Client::Paused"), "0");
    assert_eq!(
        g.call("eval", &["return Canvas.getContent().getName();"])
            .unwrap(),
        "MainMenuGui"
    );
    g.host.draw_frame();
    assert!(g.host.system_cursor(), "the menu's cursor");
    assert!(g.host.input_errors.is_empty(), "{:?}", g.host.input_errors);
}

#[test]
fn a_page_played_alone_offers_to_play_alone_or_play_by_link() {
    let mut g = client();
    g.set_global("$Arena::JoinOnly", "1");
    g.set_global("$Arena::Solo", "1");
    g.call("showMainMenu", &[]).unwrap();
    let mut eval = |code: &str| g.call("eval", &[code]).unwrap();
    // Play alone, host with an invite link, or join with one: no game server's host or join.
    for (button, text, command) in [
        ("JoinButton", "Play alone", "joinPage();"),
        ("HostButton", "Host with a link", "hostWithLink();"),
        (
            "QuitButton",
            "Join with Link",
            "$Arena::PageAsk = \"join\";",
        ),
    ] {
        assert_eq!(
            eval(&format!("return {button}.isVisible();")),
            "1",
            "{button}"
        );
        assert_eq!(
            eval(&format!("return {button}.getText();")),
            text,
            "{button}"
        );
        assert_eq!(
            eval(&format!("return {button}.command;")),
            command,
            "{button}"
        );
    }
    assert!(!eval("return MainLine.getValue();").contains("network"));
}

#[test]
fn host_with_a_link_joins_the_pages_game_and_an_invited_page_offers_to_play_alone_instead() {
    let mut g = client();
    g.set_global("$Arena::JoinOnly", "1");
    g.set_global("$Arena::Solo", "1");
    g.call("showMainMenu", &[]).unwrap();
    g.call("eval", &["hostWithLink();"]).unwrap();
    assert_eq!(
        g.global("$Arena::JoinAsked"),
        "1",
        "into the page's own game"
    );
    assert_eq!(g.global("$Arena::PageAsk"), "host", "with the panel");

    let mut g = client();
    g.set_global("$Arena::JoinOnly", "1");
    g.set_global("$Arena::Invited", "1");
    g.call("showMainMenu", &[]).unwrap();
    let mut eval = |code: &str| g.call("eval", &[code]).unwrap();
    assert_eq!(eval("return QuitButton.isVisible();"), "1");
    assert_eq!(eval("return QuitButton.getText();"), "Play alone instead");
    assert_eq!(
        eval("return QuitButton.command;"),
        "$Arena::PageAsk = \"back\";"
    );
    assert_eq!(eval("return HostButton.isVisible();"), "0");
}
