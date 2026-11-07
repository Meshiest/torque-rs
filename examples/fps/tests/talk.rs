//! What the players say and the names over them, over the loopback address: a chat line from one client shows in the other's
//! chat box, cleaned of markup and control bytes and cut to its length (never inside a character); a client that sends too
//! many lines at once has the rest dropped and is told once; a name is cleaned the same way. The view's names show the other
//! player's name over its head, never the player's own, and none through a wall.

use torque::client::input::{Key, MouseButton};
use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;

fn client(name: &str, map: Option<&str>) -> ArenaGame {
    ArenaGame::new(
        GameFiles::bundled().unwrap(),
        ClientOptions {
            net: NetMode::Loopback,
            name: name.to_string(),
            port: 0,
            map: map.map(str::to_string),
            ..ClientOptions::default()
        },
    )
    .unwrap()
}

fn step_both(a: &mut ArenaGame, b: &mut ArenaGame, ms: u32) {
    for _ in 0..ms / 32 {
        a.step(32);
        b.step(32);
    }
}

/// Ann hosts (on `map`), the client named `second` joins; both in the game, a second later.
fn two(second: &str, map: Option<&str>) -> (ArenaGame, ArenaGame) {
    let mut a = client("Ann", map);
    let mut b = client(second, map);
    a.call("hostGame", &[]).unwrap();
    for _ in 0..300 {
        step_both(&mut a, &mut b, 32);
        if a.in_game() {
            break;
        }
    }
    assert!(a.in_game(), "the host is in its game");
    let address = a.hosted_address().unwrap().to_string();
    b.call("connectTo", &[&address]).unwrap();
    for _ in 0..300 {
        step_both(&mut a, &mut b, 32);
        if b.in_game() {
            break;
        }
    }
    assert!(b.in_game(), "the second player joined");
    step_both(&mut a, &mut b, 1000);
    (a, b)
}

fn count(g: &ArenaGame) -> u32 {
    g.global("$Chat::Count").parse().unwrap_or(0)
}

/// The names the view's HUD has this tick: each name and whether a wall hides it.
fn names(g: &ArenaGame) -> Vec<(String, bool)> {
    g.host
        .gui
        .0
        .borrow()
        .hud
        .names
        .iter()
        .map(|n| (String::from_utf8_lossy(&n.name).into_owned(), n.hidden))
        .collect()
}

#[test]
fn chat_lines_and_names_are_cleaned_cut_and_limited() {
    // A name with a color tag, control bytes (one hiding a tag) and more than 24 bytes: the tags and the bytes go, the rest
    // is cut.
    let (mut a, mut b) = two(
        "<color:ff0000>Bo\u{1}b<\u{7f}br> the very long-named player",
        Some("fixtures/floor.mis"),
    );
    let bob = "Bob the very long-named";
    assert_eq!(bob.len(), 23, "24 bytes, the space at the end trimmed");
    assert_eq!(a.global("$Chat::Last"), format!("{bob} joined the arena."));

    // A line with markup and control bytes: shown as plain text in the other's box, with the name the server gave.
    b.call(
        "say",
        &["<color:00ff00><font:Arial:40>Server:\u{7} you win<br>"],
    )
    .unwrap();
    step_both(&mut a, &mut b, 300);
    assert_eq!(a.global("$Chat::Last"), format!("{bob}: Server: you win"));
    assert_eq!(
        a.call(
            "eval",
            &["return ChatLines.getLineTag(ChatLines.getNumLines() - 1);"]
        )
        .unwrap(),
        "0",
        "a player's line, never the game's"
    );

    // A control byte inside a tag does not hide it: the bytes go first, then the tag.
    b.call(
        "say",
        &["<\u{7f}color:ff0000>Server: hi<\u{7f}br><\u{1}br>"],
    )
    .unwrap();
    step_both(&mut a, &mut b, 300);
    assert_eq!(a.global("$Chat::Last"), format!("{bob}: Server: hi"));

    // Too long: cut to 120 bytes; a cut inside a character moves back to its start.
    b.call("say", &[&"x".repeat(200)]).unwrap();
    step_both(&mut a, &mut b, 300);
    assert_eq!(
        a.global("$Chat::Last"),
        format!("{bob}: {}", "x".repeat(120))
    );
    let wide = format!("a{}", "é".repeat(60));
    assert_eq!(wide.len(), 121);
    b.call("say", &[&wide]).unwrap();
    step_both(&mut a, &mut b, 300);
    assert_eq!(
        a.global("$Chat::Last"),
        format!("{bob}: a{}", "é".repeat(59)),
        "119 bytes, the last whole character kept"
    );
    // Nothing but markup and spaces: no line at all.
    let before = count(&a);
    b.call("say", &["  <br><spush>  "]).unwrap();
    step_both(&mut a, &mut b, 1500);
    assert_eq!(count(&a), before, "an empty line is not sent on");

    // Eight lines at once: four go round (the burst), the rest are dropped and the sender told once.
    step_both(&mut a, &mut b, 8000);
    let (a0, b0) = (count(&a), count(&b));
    for i in 0..8 {
        b.call("say", &[&format!("line {i}")]).unwrap();
    }
    step_both(&mut a, &mut b, 300);
    assert_eq!(count(&a) - a0, 4, "four lines reached the other player");
    assert_eq!(a.global("$Chat::Last"), format!("{bob}: line 3"));
    assert_eq!(
        count(&b) - b0,
        5,
        "the sender sees its four and one warning"
    );
    assert_eq!(
        b.global("$Chat::Last"),
        "Too many messages. Wait a moment, then try again."
    );
    // A moment later one more line goes through.
    step_both(&mut a, &mut b, 1600);
    b.call("say", &["later"]).unwrap();
    step_both(&mut a, &mut b, 300);
    assert_eq!(a.global("$Chat::Last"), format!("{bob}: later"));
    assert_eq!(count(&a) - a0, 5);

    // The other player's name over its figure is the cleaned one.
    let seen = names(&a);
    assert_eq!(seen, [(bob.to_string(), false)], "{seen:?}");
}

#[test]
fn the_chat_frees_the_mouse_and_typing_moves_nothing() {
    let (mut a, mut b) = two("Bob", Some("fixtures/floor.mis"));
    assert!(a.host.wants_mouse(), "the arena holds the mouse");
    a.key(Key::W, true);
    assert_eq!(a.global("$mvForwardAction"), "0.35", "W walks");
    let press = |g: &mut ArenaGame, key: Key| {
        g.key(key, true);
        g.key(key, false);
    };
    press(&mut a, Key::T);
    step_both(&mut a, &mut b, 32);
    assert_eq!(a.global("$Chat::Open"), "1", "T opens the chat");
    assert!(!a.host.wants_mouse(), "the mouse is free while it is open");
    assert_eq!(a.global("$mvForwardAction"), "0", "the held W was let go");
    // W types a letter; it does not walk.
    press(&mut a, Key::W);
    assert_eq!(a.global("$mvForwardAction"), "0");
    assert_eq!(
        a.call("eval", &["return ChatEdit.getValue();"]).unwrap(),
        "w"
    );
    // Escape closes it (not the pause menu): the mouse held again, W walks again.
    press(&mut a, Key::Escape);
    step_both(&mut a, &mut b, 32);
    assert_eq!(a.global("$Chat::Open"), "0");
    assert_ne!(a.global("$Client::Paused"), "1");
    assert!(a.host.wants_mouse());
    a.key(Key::W, true);
    assert_eq!(a.global("$mvForwardAction"), "0.35");
    a.key(Key::W, false);
    // Enter sends the line.
    press(&mut a, Key::T);
    step_both(&mut a, &mut b, 32);
    for key in [Key::H, Key::I] {
        press(&mut a, key);
    }
    press(&mut a, Key::Enter);
    step_both(&mut a, &mut b, 300);
    assert_eq!(a.global("$Chat::Open"), "0");
    assert_eq!(b.global("$Chat::Last"), "Ann: hi");
    // A click on the view while it is open fires nothing, and Esc still closes it.
    press(&mut a, Key::T);
    step_both(&mut a, &mut b, 32);
    let shots = a.global("$mvTriggerCount0");
    a.host.mouse_button(MouseButton::Left, true, (400, 300));
    a.host.mouse_button(MouseButton::Left, false, (400, 300));
    assert_eq!(a.global("$mvTriggerCount0"), shots, "no shot");
    press(&mut a, Key::Escape);
    step_both(&mut a, &mut b, 32);
    assert_eq!(a.global("$Chat::Open"), "0");
    assert_ne!(a.global("$Client::Paused"), "1");
    assert!(a.host.input_errors.is_empty(), "{:?}", a.host.input_errors);
}

#[test]
fn the_view_names_the_other_player_not_its_own_and_none_through_a_wall_from_the_drawn_camera() {
    let (mut a, mut b) = two("Bob", None);
    let place = |a: &mut ArenaGame, name: &str, at: [f32; 3], yaw: f32| {
        let [x, y, z] = at.map(|v| v.to_string());
        a.server
            .as_mut()
            .unwrap()
            .call("placeFigure", &[name, &x, &y, &z, &yaw.to_string()])
            .unwrap();
    };
    // Face to face in the open, a few steps apart: each sees the other's name, not its own.
    place(&mut a, "Ann", [4.0, -16.0, 0.1], 0.0);
    place(&mut a, "Bob", [4.6, -11.5, 0.1], 3.6);
    step_both(&mut a, &mut b, 1000);
    assert_eq!(names(&a), [("Bob".to_string(), false)]);
    assert_eq!(names(&b), [("Ann".to_string(), false)]);
    // Bob behind the ring of cover: his name is hidden from Ann.
    place(&mut a, "Ann", [0.0, -16.0, 0.1], 0.0);
    place(&mut a, "Bob", [0.0, -5.5, 0.1], std::f32::consts::PI);
    step_both(&mut a, &mut b, 1000);
    assert_eq!(names(&a), [("Bob".to_string(), true)], "behind the wall");
    // A camera of the game's own high above Ann (as one behind her would be): the names are tested from it, as they are
    // drawn from it, and Bob's shows over the cover.
    let above = torque_render_scene::source::Eye {
        position: [0.0, -16.0, 10.0],
        pitch: 0.63,
        yaw: 0.0,
        fov: None,
    };
    a.host.client.borrow_mut().names_eye = Some(above);
    step_both(&mut a, &mut b, 100);
    assert_eq!(names(&a), [("Bob".to_string(), false)], "seen from above");
    let camera = a.host.gui.0.borrow().hud.camera.unwrap();
    assert_eq!(camera.position, above.position);
    // Back at her eyes: hidden again.
    a.host.client.borrow_mut().names_eye = None;
    step_both(&mut a, &mut b, 100);
    assert_eq!(names(&a), [("Bob".to_string(), true)]);
}

/// Ann hosts on the floor; Bob and Mallory join from the same address (this computer).
fn three() -> (ArenaGame, ArenaGame, ArenaGame) {
    let (mut a, mut b) = two("Bob", Some("fixtures/floor.mis"));
    let mut c = client(
        "<\u{7f}color:ff0000>Mallory<\u{7f}br>",
        Some("fixtures/floor.mis"),
    );
    let address = a.hosted_address().unwrap().to_string();
    c.call("connectTo", &[&address]).unwrap();
    for _ in 0..300 {
        steps(&mut a, &mut b, &mut c, 32);
        if c.in_game() {
            break;
        }
    }
    assert!(c.in_game(), "the third player joined");
    steps(&mut a, &mut b, &mut c, 1000);
    (a, b, c)
}

fn steps(a: &mut ArenaGame, b: &mut ArenaGame, c: &mut ArenaGame, ms: u32) {
    for _ in 0..ms / 32 {
        a.step(32);
        b.step(32);
        c.step(32);
    }
}

#[test]
fn connections_from_one_address_share_the_allowance_and_a_reconnect_does_not_refill_it() {
    let (mut a, mut b, mut c) = three();
    // The name's tag, hidden by control bytes, is stripped too.
    assert_eq!(a.global("$Chat::Last"), "Mallory joined the arena.");
    // Slow the allowance's refill, so the test's own seconds add nothing to it.
    a.server
        .as_mut()
        .unwrap()
        .call("eval", &["$Server::ChatRefillMs = 600000;"])
        .unwrap();
    let before = count(&a);
    for i in 0..3 {
        b.call("say", &[&format!("bob {i}")]).unwrap();
        c.call("say", &[&format!("mallory {i}")]).unwrap();
    }
    steps(&mut a, &mut b, &mut c, 300);
    assert_eq!(count(&a) - before, 4, "four lines between the two");
    assert_eq!(
        c.global("$Chat::Last"),
        "Too many messages. Wait a moment, then try again."
    );

    // Mallory leaves and comes back: still no lines left.
    c.call("disconnect", &[]).unwrap();
    steps(&mut a, &mut b, &mut c, 500);
    let address = a.hosted_address().unwrap().to_string();
    c.call("connectTo", &[&address]).unwrap();
    for _ in 0..300 {
        steps(&mut a, &mut b, &mut c, 32);
        if c.in_game() {
            break;
        }
    }
    assert!(c.in_game(), "Mallory is back");
    steps(&mut a, &mut b, &mut c, 1000);
    let before = count(&a);
    for i in 0..4 {
        c.call("say", &[&format!("again {i}")]).unwrap();
    }
    steps(&mut a, &mut b, &mut c, 300);
    assert_eq!(count(&a), before, "the reconnect started no new allowance");
    // The host's own connection has an allowance of its own.
    a.call("say", &["host"]).unwrap();
    steps(&mut a, &mut b, &mut c, 300);
    assert_eq!(a.global("$Chat::Last"), "Ann: host");
}
