//! The office's prop catalog end to end, headless: a server whose `catalog` mod it requires of its clients, two clients that download
//! the mod (its pictures included), show the player the consent prompt and run the mod's menu only after Allow, and a Spawn button
//! that asks the server to put a prop down in front of the player. The server's script decides: a known kind, a label of allowed
//! characters and length, a cap of props out a player, a rate limit, and a placement check.
//!
//! Each test plays the menu the way a player does, through the GUI's own input (clicks on the prompt, the label field and the Spawn
//! buttons), and reads what each client shows and holds and what the server holds. They need the realm guest (`just
//! lua-client-web`, or `TORQUE_CLIENT_GUEST`) and run when it is there; `TORQUE_CLIENT_GUEST_REQUIRED` makes a missing guest a
//! failure (the `level-demo-catalog-test` recipe sets it).
#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::sync::OnceLock;

use torque::gui::client_script::PromptSpot;
use torque::mods::Mods;
use torque_client_script_host::RealmGuest;
use torque_example_level_demo::catalog::{ABILITIES, Catalog, SIZE, click, guest_path, load_guest};
use torque_example_level_demo::props::PropKind;
use torque_plugin_client_script::RealmState;

/// The office's corner in the map: the model's floor runs from (0, 0) to (36, 24) in the generator's numbers.
const CORNER: [f32; 2] = [-18.0, -12.0];

/// The compiled guest, once for every test. A guest that is not built fails the test, with the way to build it.
fn guest() -> RealmGuest {
    static GUEST: OnceLock<RealmGuest> = OnceLock::new();
    GUEST
        .get_or_init(|| load_guest(&guest_path()).unwrap_or_else(|why| panic!("{why}")))
        .clone()
}

fn work(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("level-demo-catalog-{name}"))
}

/// The office with two clients in it, each asked about the server's scripts and neither has answered.
fn started(name: &str) -> Catalog {
    let guest = guest();
    let mut c = Catalog::new(&guest, &work(name)).expect("the session");
    c.start_office().expect("the office starts");
    let asked = |c: &mut Catalog| {
        (0..2).all(|n| c.in_world(n) && c.realms[n].state() == RealmState::Asking)
    };
    assert!(
        c.run_until(6_000, asked),
        "both clients were asked: {:?} {:?}",
        (0..2).map(|n| c.realms[n].state()).collect::<Vec<_>>(),
        c.demo.client_log
    );
    c
}

/// Both players allow the menu and have its catalog.
fn both_allow(c: &mut Catalog) {
    for n in 0..2 {
        c.allow(n).expect("allowed");
    }
    assert!(c.run_until(1_000, |c| (0..2).all(|n| !c.cards(n).is_empty())));
}

/// Puts player `name` ("Ann" or "Bob") at (`x`, `y`) of the office's floor (the model's numbers), facing `yaw`.
fn stand(c: &mut Catalog, name: &str, x: f32, y: f32, yaw: f32) {
    let (x, y) = (x + CORNER[0], y + CORNER[1]);
    c.server_eval(&format!(
        "for (%i = 0; %i < ClientGroup.getCount(); %i++) {{ %c = ClientGroup.getObject(%i); \
         if (%c.name $= \"{name}\") %c.player.setTransform(\"{x} {y} 0.2 0 0 1 {yaw}\"); }}"
    ))
    .expect("the server's script runs");
    c.run(12);
}

/// The corridor east of the lobby, facing east; `along` is how far down it.
fn in_corridor(c: &mut Catalog, name: &str, along: f32) {
    stand(c, name, 12.0 + along, 14.0, std::f32::consts::FRAC_PI_2);
}

/// How many props player `name` has out, by the server's count.
fn out(c: &mut Catalog, name: &str) -> u32 {
    c.server_eval(&format!(
        "for (%i = 0; %i < ClientGroup.getCount(); %i++) {{ %c = ClientGroup.getObject(%i); \
         if (%c.name $= \"{name}\") return propCount(%c); }} return -1;"
    ))
    .expect("the server's script runs")
    .parse()
    .unwrap_or(0)
}

fn counter(c: &Catalog, name: &str) -> u32 {
    c.server_global(&format!("$Catalog::{name}"))
        .parse()
        .unwrap_or(0)
}

/// Ticks until client `n`'s status line says `text`.
fn says(c: &mut Catalog, n: usize, text: &str) -> bool {
    c.run_until(600, |c| c.status(n) == text)
}

/// Ticks until client `n`'s status line starts with `text`.
fn says_start(c: &mut Catalog, n: usize, text: &str) -> bool {
    c.run_until(600, |c| c.status(n).starts_with(text))
}

/// What the world holds: the server's props, and what each client has of them as ghosts.
fn ghosts(c: &Catalog) -> [usize; 2] {
    [c.demo.client.ghost_count(), c.other.ghost_count()]
}

#[test]
#[ignore = "needs the realm guest: `just lua-client-web`, then run with --include-ignored (`just examples level-demo catalog-test` does)"]
fn joining_asks_the_player_and_both_clients_download_the_mod_with_its_pictures() {
    let mut c = started("join");
    // Both are asked, and have the mod's files already: downloaded before the prompt, read by nothing until Allow.
    for n in 0..2 {
        let client = c.client(n);
        let mods = Mods::of(client.vm()).expect("the client has mods");
        assert!(mods.is_server_file("client/main.lua"), "client {n}");
        assert!(mods.is_untrusted("client/main.lua"), "and holds it as data");
        for kind in PropKind::ALL {
            let icon = format!("icons/{}.png", kind.word());
            assert!(mods.is_server_file(&icon), "client {n} downloaded {icon}");
        }
        for file in ["server.cs", "mod.toml"] {
            assert!(!mods.is_server_file(file), "client {n} has {file}");
        }
    }
    // A press on Allow as the prompt appears does nothing; until the player answers nothing of the realm exists.
    for n in 0..2 {
        super_press(&mut c, n);
    }
    c.run(400);
    for n in 0..2 {
        assert_eq!(c.realms[n].state(), RealmState::Asking, "client {n}");
        assert_eq!(c.realms[n].stats().opened, 0, "no realm exists");
        assert!(!c.has_menu(n), "nothing of the menu is drawn");
    }
    both_allow(&mut c);
    for n in 0..2 {
        assert_eq!(c.realms[n].stats().opened, 1);
        let pictures = c.pictures(n);
        assert!(!pictures.is_empty(), "client {n} shows cards");
        assert!(
            pictures
                .iter()
                .all(|p| p.starts_with("mods/catalog/icons/") && p.ends_with(".png")),
            "{pictures:?}"
        );
    }
    assert!(
        c.run_until(600, |c| c.cards(0) == c.cards(1)),
        "{:?} / {:?}",
        c.cards(0),
        c.cards(1)
    );
    assert!(
        c.cards(0).contains(&"Chair".to_string()),
        "{:?}",
        c.cards(0)
    );
}

fn super_press(c: &mut Catalog, n: usize) {
    torque_example_level_demo::catalog::press_prompt(c.client(n), ABILITIES, PromptSpot::Allow);
    c.run(2);
}

#[test]
#[ignore = "needs the realm guest: `just lua-client-web`, then run with --include-ignored (`just examples level-demo catalog-test` does)"]
fn one_client_spawns_a_chair_and_both_see_the_new_prop() {
    let mut c = started("chair");
    both_allow(&mut c);
    in_corridor(&mut c, "Bob", 4.0);
    let before = ghosts(&c);
    c.spawn(1, "Chair", "").expect("the form");
    assert!(
        c.run_until(600, |c| ghosts(c)
            .iter()
            .zip(before)
            .all(|(g, b)| *g == b + 1)),
        "both clients ghost the new prop: {before:?} -> {:?}",
        ghosts(&c)
    );
    assert_eq!(out(&mut c, "Bob"), 1);
    assert_eq!(out(&mut c, "Ann"), 0);
    assert_eq!(counter(&c, "Spawned"), 1);
    assert_eq!(c.status(0), "", "the other player is told nothing");
    assert_eq!(
        c.status(1),
        "",
        "and the one who asked is told only of a refusal"
    );
}

#[test]
#[ignore = "needs the realm guest: `just lua-client-web`, then run with --include-ignored (`just examples level-demo catalog-test` does)"]
fn a_valid_label_shows_in_both_recent_lists_and_stays_on_the_prop() {
    let mut c = started("label");
    both_allow(&mut c);
    in_corridor(&mut c, "Bob", 4.0);
    c.spawn(1, "Chair", "Bob's chair").expect("the form");
    let both = |c: &mut Catalog| {
        (0..2).all(|n| {
            c.recent(n).first().map(String::as_str) == Some("Bob put down a chair: Bob's chair")
        })
    };
    assert!(
        c.run_until(600, both),
        "{:?} / {:?}",
        c.recent(0),
        c.recent(1)
    );
    // The label is on the prop, as a field of the server's object.
    let label = c
        .server_eval(
            "for (%i = 0; %i < ClientGroup.getCount(); %i++) { %c = ClientGroup.getObject(%i); \
             if (%c.name $= \"Bob\") return getWord(%c.catalogProps, 0).catalogLabel; } return \"\";",
        )
        .unwrap();
    assert_eq!(label, "Bob's chair");
    // The form was cleared when the server took the spawn.
    assert!(c.run_until(600, |c| {
        c.shown(1).iter().any(|s| {
            s.kind == torque_client_script_format::ui::Kind::TextField && s.text.is_empty()
        })
    }));
    // A second player's spawn comes after, newest first.
    in_corridor(&mut c, "Ann", 10.0);
    c.spawn(0, "Plant", "").expect("the form");
    assert!(c.run_until(600, |c| c.recent(0).len() == 2 && c.recent(1).len() == 2));
    assert_eq!(
        c.recent(1),
        ["Ann put down a plant", "Bob put down a chair: Bob's chair"]
    );
    assert_eq!(c.recent(0), c.recent(1));
}

#[test]
#[ignore = "needs the realm guest: `just lua-client-web`, then run with --include-ignored (`just examples level-demo catalog-test` does)"]
fn a_bad_label_an_unknown_kind_and_a_blocked_place_are_refused_with_their_reasons_and_nothing_spawns()
 {
    let mut c = started("refused");
    both_allow(&mut c);
    in_corridor(&mut c, "Bob", 4.0);
    let before = (ghosts(&c), out(&mut c, "Bob"), c.recent(1));

    // A label over the server's 32 characters (the field itself takes 40).
    let long = "A label that is much too long for it";
    assert!(long.len() > 32 && long.len() <= 40);
    c.spawn(1, "Chair", long).expect("the form");
    assert!(says(
        &mut c,
        1,
        "That label is too long. Use 32 characters or fewer."
    ));
    // A character the list leaves out.
    c.run(200);
    c.spawn(1, "Chair", "<b>mine</b>").expect("the form");
    assert!(says(
        &mut c,
        1,
        "Labels can use letters, numbers, spaces and . , ! ? ' - only."
    ));
    // A kind nobody offered, as a modified client would send it.
    c.run(200);
    c.server_eval(
        "for (%i = 0; %i < ClientGroup.getCount(); %i++) { %c = ClientGroup.getObject(%i); \
         if (%c.name $= \"Bob\") clientScriptOnMessage(%c, \"spawn|sofa_bed|\"); }",
    )
    .unwrap();
    assert!(says(&mut c, 1, "There is no such prop."));
    // A place with a wall in the way: facing the corridor's north wall from a hand away.
    c.run(200);
    stand(&mut c, "Bob", 19.0, 14.4, 0.0);
    c.spawn(1, "Chair", "").expect("the form");
    assert!(says_start(
        &mut c,
        1,
        "There is no room for that in front of you"
    ));

    c.run(60);
    assert_eq!(counter(&c, "Spawned"), 0);
    assert_eq!(counter(&c, "Refused"), 4);
    assert_eq!((ghosts(&c), out(&mut c, "Bob"), c.recent(1)), before);
    assert_eq!(c.recent(0), before.2);
}

#[test]
#[ignore = "needs the realm guest: `just lua-client-web`, then run with --include-ignored (`just examples level-demo catalog-test` does)"]
fn the_cap_and_the_rate_limit_refuse_with_their_reasons() {
    let mut c = started("limits");
    both_allow(&mut c);
    // Four props out is the cap: each is put down in a clear place of the corridor, a few seconds apart.
    for (i, along) in [3.0, 5.5, 8.0, 10.5].into_iter().enumerate() {
        in_corridor(&mut c, "Bob", along);
        c.spawn(1, "Bin", "").expect("the form");
        assert!(
            c.run_until(600, |c| counter(c, "Spawned") as usize > i),
            "spawn {i}: {}",
            c.status(1)
        );
        c.run(180);
    }
    assert_eq!(out(&mut c, "Bob"), 4);
    in_corridor(&mut c, "Bob", 13.0);
    c.spawn(1, "Bin", "").expect("the form");
    assert!(says(
        &mut c,
        1,
        "You already have 4 props out. Take them back first."
    ));
    assert_eq!(out(&mut c, "Bob"), 4);
    // Taking them back frees the cap.
    c.run(180);
    c.press(1, "Clear my props").expect("the button");
    assert!(c.run_until(600, |c| out(c, "Bob") == 0));

    // A flood: Spawn pressed eight times in a quarter of a second. The server looks at the first three and refuses the rest
    // without looking (the others of those three found no room, the first prop being in the way).
    c.run(180);
    in_corridor(&mut c, "Ann", 20.0);
    let slowed = counter(&c, "Slowed");
    for _ in 0..8 {
        c.spawn(0, "Bin", "").expect("the form");
        c.run(1);
    }
    c.run(60);
    assert_eq!(counter(&c, "Slowed") - slowed, 5);
    assert_eq!(out(&mut c, "Ann"), 1, "only the first fitted");
    assert_eq!(c.status(0), "Slow down. Wait a few seconds and try again.");
    // The limit is the flooder's own: the other player is not slowed.
    in_corridor(&mut c, "Bob", 26.0);
    let before = counter(&c, "Spawned");
    c.spawn(1, "Bin", "").expect("the form");
    assert!(c.run_until(600, |c| counter(c, "Spawned") == before + 1));
}

#[test]
#[ignore = "needs the realm guest: `just lua-client-web`, then run with --include-ignored (`just examples level-demo catalog-test` does)"]
fn a_client_that_never_clicks_allow_gets_no_menu_and_cannot_spawn() {
    let mut c = started("silent");
    // Ann allows; Bob's player leaves the prompt alone.
    c.allow(0).expect("allowed");
    assert!(c.run_until(600, |c| !c.cards(0).is_empty()));
    in_corridor(&mut c, "Ann", 4.0);
    c.spawn(0, "Chair", "").expect("the form");
    assert!(c.run_until(600, |c| counter(c, "Spawned") == 1));
    let before = (
        counter(&c, "Spawned"),
        counter(&c, "Refused"),
        counter(&c, "Slowed"),
    );
    // Many ticks, clicks where the menu would be on Bob's screen, and Ann's next spawn: none of it reaches a realm.
    let (w, h) = (SIZE.0 as i32, SIZE.1 as i32);
    for _ in 0..20 {
        click(c.client(1), (w / 4, h / 2));
        c.run(10);
    }
    assert_eq!(c.realms[1].state(), RealmState::Asking);
    assert_eq!(c.realms[1].stats().opened, 0, "no realm was ever made");
    assert!(!c.other.host.gui.client_script_active());
    assert!(!c.has_menu(1), "no menu on that screen");
    assert!(c.cards(1).is_empty() && c.recent(1).is_empty());
    assert_eq!(
        c.server_eval(
            "for (%i = 0; %i < ClientGroup.getCount(); %i++) { %c = ClientGroup.getObject(%i); \
             if (%c.name $= \"Bob\") return %c.catalogHere @ %c.catalogTimes @ %c.catalogProps; } return \"x\";"
        )
        .unwrap(),
        "",
        "the server never heard from it"
    );
    // It spawns nothing, and what the server holds is only what Ann did.
    assert_eq!(out(&mut c, "Bob"), 0);
    assert_eq!(out(&mut c, "Ann"), 1);
    assert_eq!(
        (
            counter(&c, "Spawned"),
            counter(&c, "Refused"),
            counter(&c, "Slowed")
        ),
        before
    );
}

#[test]
#[ignore = "needs the realm guest: `just lua-client-web`, then run with --include-ignored (`just examples level-demo catalog-test` does)"]
fn escape_closes_the_menu_q_opens_it_and_shift_escape_ends_the_realm() {
    use torque::client::input::Key;
    let mut c = started("keys");
    c.allow(0).expect("allowed");
    assert!(c.run_until(600, |c| !c.cards(0).is_empty()));
    // Escape: the engine takes the cursor back and tells the menu, which closes. The game's own menu does not open, and the
    // realm goes on.
    c.press_key(0, Key::Escape);
    assert!(
        c.run_until(200, |c| c.cards(0).is_empty()),
        "the catalog is closed"
    );
    assert_eq!(c.realms[0].state(), RealmState::Running);
    let paused = c.demo.client.global("$Client::Paused");
    assert!(
        paused.is_empty() || paused == "0",
        "no game menu: {paused:?}"
    );
    // Q: the game's own key goes to the server, whose script tells the menu to open, and the menu has the cursor again.
    c.press_key(0, Key::Q);
    assert!(
        c.run_until(400, |c| !c.cards(0).is_empty()),
        "the catalog is open"
    );
    c.run(5);
    assert!(c.demo.client.host.gui.client_script_holds().cursor);
    // Q again closes it, and the cursor goes with it.
    c.press_key(0, Key::Q);
    assert!(
        c.run_until(400, |c| c.cards(0).is_empty()),
        "the catalog is closed again"
    );
    c.run(5);
    assert!(!c.demo.client.host.gui.client_script_holds().cursor);
    // The key opens it once more, within the moment the game's key is good for.
    c.press_key(0, Key::Q);
    assert!(c.run_until(400, |c| !c.cards(0).is_empty()));
    // The key does nothing for a player whose menu is not up.
    c.press_key(1, Key::Q);
    c.run(30);
    assert!(!c.has_menu(1));
    // Shift+Escape turns server scripts off for the connection.
    let client = c.client(0);
    client.key(Key::ShiftLeft, true);
    client.key(Key::Escape, true);
    client.key(Key::Escape, false);
    client.key(Key::ShiftLeft, false);
    assert!(c.run_until(200, |c| c.realms[0].state() != RealmState::Running));
    assert!(!c.has_menu(0), "nothing of the menu is left");
}

#[test]
#[ignore = "needs the realm guest: `just lua-client-web`, then run with --include-ignored (`just examples level-demo catalog-test` does)"]
fn hostile_messages_are_refused_or_taken_as_what_they_are_and_the_server_goes_on() {
    let mut c = started("hostile");
    both_allow(&mut c);
    // What a modified client could send as its message: the handler gets the text of it. A tab and a backslash in a label, a
    // table (it arrives as JSON text), a string of 4,000 characters, and an empty label (fine: no label).
    let send = |c: &mut Catalog, text: &str| {
        c.server_eval(&format!(
            "for (%i = 0; %i < ClientGroup.getCount(); %i++) {{ %c = ClientGroup.getObject(%i); \
             if (%c.name $= \"Bob\") clientScriptOnMessage(%c, {text}); }}"
        ))
        .expect("the server's script runs");
    };
    c.server_eval("$Test::Huge = \"\"; for (%i = 0; %i < 400; %i++) $Test::Huge = $Test::Huge @ \"abcdefghij\";")
        .expect("the server's script runs");
    let huge = "\"spawn|chair|\" @ $Test::Huge";
    let cases: [(&str, Option<&str>); 6] = [
        ("\"spawn|chair|a\tb\"", Some("Labels can use")),
        ("\"spawn|chair|a\\x5cb\"", Some("Labels can use")),
        (
            "\"{\\\"spawn\\\":{\\\"kind\\\":\\\"chair\\\"}}\"",
            Some("The server did not understand that."),
        ),
        (huge, Some("That label is too long")),
        ("\"spawn|chair|\"", None),
        ("\"\"", Some("The server did not understand that.")),
    ];
    for (n, (text, reason)) in cases.into_iter().enumerate() {
        c.run(200);
        in_corridor(&mut c, "Bob", 4.0 + 2.5 * n as f32);
        let before = counter(&c, "Spawned");
        send(&mut c, text);
        match reason {
            Some(reason) => {
                assert!(says_start(&mut c, 1, reason), "case {n}: {}", c.status(1));
                assert_eq!(counter(&c, "Spawned"), before, "case {n}");
            }
            None => assert!(
                c.run_until(600, |c| counter(c, "Spawned") == before + 1),
                "an empty label is a spawn with none"
            ),
        }
    }
    // A flood of them is held to the allowance, and nothing else moved.
    c.run(200);
    for _ in 0..6 {
        send(&mut c, "\"hello\"");
    }
    c.run(60);
    assert!(counter(&c, "Slowed") >= 3, "{}", counter(&c, "Slowed"));
    assert_eq!(out(&mut c, "Bob"), 1);
    assert_eq!(out(&mut c, "Ann"), 0);
}
