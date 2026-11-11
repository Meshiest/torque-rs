//! The office played with others, end to end over the loopback address (`src/session.rs`): a dedicated office server on a real
//! UDP socket, a player who finds it in the list of matches, sees the catalog mod download on the join screen, is asked about
//! the server's scripts, allows them and puts a prop down that a second player sees and picks up; then leaves and joins again
//! with the mod already downloaded. Every client is the window's own, on a loopback socket.
//!
//! They need the realm guest (`just lua-client-web`, or `TORQUE_CLIENT_GUEST`), as `tests/catalog.rs` does;
//! `TORQUE_CLIENT_GUEST_REQUIRED` makes a missing guest a failure (the `catalog-test` recipe sets it).
#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeSet;
use std::path::PathBuf;

use torque::client::input::{Taken, WM_LBUTTONDOWN, WM_LBUTTONUP};
use torque::gui::client_script::{PromptSpot, prompt_spots};
use torque_example_level_demo::catalog::{cards_of, guest_path, load_guest, recent_of, spawn_on};
use torque_example_level_demo::office::{ABILITIES, card_controls, center, status_of};
use torque_example_level_demo::session::{JoinState, MatchSession, join_state, realm_state};
use torque_plugin_client_script::RealmState;
use torque_plugin_props::ClientProps;

fn work(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("level-demo-mp-{name}"))
}

/// The server and its players Ann and Bob, at their menus; None (the test skipped) without the guest unless it is required.
fn session(name: &str) -> Option<MatchSession> {
    let guest = match load_guest(&guest_path()) {
        Ok(guest) => guest,
        Err(why) if std::env::var_os("TORQUE_CLIENT_GUEST_REQUIRED").is_some() => panic!("{why}"),
        Err(why) => {
            eprintln!("skipped: {why}");
            return None;
        }
    };
    let mut s = MatchSession::start(&guest, &work(name), "Office test").expect("the server starts");
    s.add_player("Ann", "ann").expect("Ann");
    s.add_player("Bob", "bob").expect("Bob");
    s.run(10);
    Some(s)
}

/// Runs until player `n`'s join is in the game, keeping every join state seen; whether it got there.
fn until_playing(s: &mut MatchSession, n: usize, seen: &mut BTreeSet<JoinState>) -> bool {
    s.run_until(4_000, |s| {
        seen.insert(join_state(&s.players[n]));
        join_state(&s.players[n]) == JoinState::Playing && s.players[n].in_world()
    })
}

/// The props player `n`'s client holds.
fn props_seen(s: &MatchSession, n: usize) -> usize {
    s.players[n]
        .engine()
        .resource::<ClientProps>()
        .and_then(|book| book.with(|b| b.props().len()))
        .unwrap_or(0)
}

/// What the server's script returns for `code`, as a number (0 when it is none).
fn server_number(s: &mut MatchSession, code: &str) -> i64 {
    s.server_eval(code)
        .expect("the server's script runs")
        .parse()
        .unwrap_or(0)
}

/// The id of the figure of player `name` on the server.
fn figure_of(s: &mut MatchSession, name: &str) -> i64 {
    server_number(
        s,
        &format!(
            "for (%i = 0; %i < ClientGroup.getCount(); %i++) {{ %c = ClientGroup.getObject(%i); \
             if (%c.name $= \"{name}\") return %c.player; }} return 0;"
        ),
    )
}

#[test]
#[ignore = "needs the realm guest: `just lua-client-web`, then run with --include-ignored (`just examples level-demo catalog-test` does)"]
fn a_player_finds_the_office_downloads_the_catalog_allows_it_and_puts_down_a_prop_another_picks_up()
{
    let Some(mut s) = session("join") else {
        return;
    };
    // The list of matches finds the server.
    s.players[0].client.call("showJoin", &[]).unwrap();
    assert!(
        s.run_until(3_000, |s| s.players[0].client.number("$Game::ServerRows")
            > 0.0),
        "the list of matches found the server"
    );
    let row = s.players[0]
        .client
        .call("eval", &["return ServerList.getRowText(0);"])
        .unwrap();
    let cells: Vec<&str> = row.split('\t').collect();
    assert_eq!(cells.len(), 3, "server, players and ping: {row:?}");
    assert_eq!(cells[..2], ["Office test", "0 / 8"], "{row:?}");

    // Joining it: the join screen shows the catalog coming in, then the office.
    s.players[0].client.call("joinSelected", &[]).unwrap();
    let mut seen = BTreeSet::new();
    assert!(
        s.run_until(3_000, |s| {
            seen.insert(join_state(&s.players[0]));
            join_state(&s.players[0]) == JoinState::Downloading
        }),
        "the download started: {seen:?}"
    );
    let ann = &mut s.players[0].client;
    assert_eq!(ann.global("$Join::Mods"), "Prop catalog");
    assert_eq!(ann.global("$Join::Versions"), "1.0.0");
    assert!(
        ann.number("$Join::Files") > 20.0,
        "the menu and its pictures"
    );
    let detail = ann
        .call("eval", &["return JoinScreenDetail.getValue();"])
        .unwrap();
    assert!(
        detail.starts_with("The server's mods: ") && detail.ends_with(" to download."),
        "{detail:?}"
    );
    assert!(until_playing(&mut s, 0, &mut seen), "Ann played: {seen:?}");
    for state in [
        JoinState::Connecting,
        JoinState::Downloading,
        JoinState::Playing,
    ] {
        assert!(
            seen.contains(&state),
            "the screen showed {state:?}: {seen:?}"
        );
    }
    let ann = &s.players[0].client;
    assert_eq!(ann.global("$Join::Received"), ann.global("$Join::Bytes"));

    // Bob joins by the address typed in the box.
    s.join(1).unwrap();
    let mut seen = BTreeSet::new();
    assert!(until_playing(&mut s, 1, &mut seen), "Bob played: {seen:?}");
    assert_eq!(server_number(&mut s, "return ClientGroup.getCount();"), 2);

    // Both are asked; both allow, and have the catalog.
    for n in 0..2 {
        s.allow(n).unwrap();
    }
    assert!(s.run_until(600, |s| {
        (0..2).all(|n| !cards_of(&s.players[n].client).is_empty())
    }));
    assert!(cards_of(&s.players[1].client).contains(&"Chair".to_string()));

    // Ann puts a chair down in the corridor east of the lobby; Bob's client gets it and its line in the recent list.
    s.stand("Ann", -5.0, 2.0, std::f32::consts::FRAC_PI_2)
        .unwrap();
    s.stand("Bob", -8.0, 2.0, std::f32::consts::FRAC_PI_2)
        .unwrap();
    s.run(30);
    let before = props_seen(&s, 1);
    spawn_on(&mut s.players[0].client, "Chair", "Ann's chair").unwrap();
    assert!(
        s.run_until(600, |s| props_seen(s, 1) > before),
        "Bob's client got the chair: {} {:?}",
        props_seen(&s, 1),
        recent_of(&s.players[0].client)
    );
    let ann_client = server_number(
        &mut s,
        "for (%i = 0; %i < ClientGroup.getCount(); %i++) { %c = ClientGroup.getObject(%i); if (%c.name $= \"Ann\") return %c; } return 0;",
    );
    assert_eq!(
        server_number(&mut s, &format!("return propCount({ann_client});")),
        1
    );
    assert!(
        s.run_until(300, |s| recent_of(&s.players[1].client)
            .first()
            .is_some_and(|l| l.contains("Ann") && l.contains("chair"))),
        "Bob's list: {:?}",
        recent_of(&s.players[1].client)
    );

    // Bob takes Ann's place, looks down at the chair and picks it up: holding works for every player.
    s.stand("Ann", -5.0, -2.0, std::f32::consts::FRAC_PI_2)
        .unwrap();
    s.stand("Bob", -5.0, 2.0, std::f32::consts::FRAC_PI_2)
        .unwrap();
    s.players[1].client.set_global("$mvPitch", "0.6");
    s.run(20);
    s.players[1].client.set_global("$mvPitch", "0");
    s.players[1].client.call("grab", &["1"]).unwrap();
    s.run(12);
    let bob = figure_of(&mut s, "Bob");
    let joint = server_number(&mut s, &format!("return {bob}.holdJoint;"));
    let refused = s
        .server_eval("return $World::GrabRefused;")
        .unwrap_or_default();
    assert!(joint != 0, "Bob holds the chair: {refused:?}");
}

#[test]
#[ignore = "needs the realm guest: `just lua-client-web`, then run with --include-ignored (`just examples level-demo catalog-test` does)"]
fn leaving_and_joining_again_downloads_nothing_and_the_answer_is_remembered() {
    let Some(mut s) = session("again") else {
        return;
    };
    s.join(0).unwrap();
    let mut seen = BTreeSet::new();
    assert!(until_playing(&mut s, 0, &mut seen), "Ann played: {seen:?}");
    assert!(seen.contains(&JoinState::Downloading), "{seen:?}");
    s.allow(0).unwrap();

    // The Esc menu's Leave: back at the menu, out of the server, the catalog taken down.
    s.players[0].client.call("pauseGame", &[]).unwrap();
    s.players[0].client.call("leaveGame", &[]).unwrap();
    s.run(30);
    assert_eq!(
        server_number(&mut s, "return ClientGroup.getCount();"),
        0,
        "Ann left the server"
    );
    assert_eq!(join_state(&s.players[0]), JoinState::Menu);
    assert!(!s.players[0].in_world());
    assert_eq!(realm_state(&s.players[0]), RealmState::Idle);
    assert!(cards_of(&s.players[0].client).is_empty());

    // Joining again: the mod is in the cache, so nothing is downloaded, and the player's Allow is remembered for the server.
    s.join(0).unwrap();
    let mut seen = BTreeSet::new();
    assert!(
        until_playing(&mut s, 0, &mut seen),
        "Ann played again: {seen:?}"
    );
    assert!(
        seen.contains(&JoinState::Cached),
        "nothing to download: {seen:?}"
    );
    assert!(!seen.contains(&JoinState::Downloading), "{seen:?}");
    assert!(
        s.run_until(600, |s| !cards_of(&s.players[0].client).is_empty()),
        "the catalog runs without a question: {:?}",
        realm_state(&s.players[0])
    );
    assert_eq!(
        s.players[0]
            .scripts
            .as_ref()
            .unwrap()
            .realms
            .stats()
            .prompts,
        1,
        "asked once"
    );

    // A map played alone after it: no join screen, no catalog asked about.
    s.players[0].client.call("leaveGame", &[]).unwrap();
    s.run(20);
    s.players[0].client.set_global("$World::Pick", "0");
    assert!(
        s.run_until(1_500, |s| s.players[0].in_world()),
        "the map played alone"
    );
    assert_eq!(join_state(&s.players[0]), JoinState::Menu);
    s.run(100);
    assert_eq!(
        realm_state(&s.players[0]),
        RealmState::Idle,
        "no server scripts offered"
    );
}

/// The window's left button as its messages (`WM_LBUTTONDOWN`, `WM_LBUTTONUP`) at `at`, through the host's routing: who took the
/// press.
fn window_click(s: &mut MatchSession, n: usize, at: (i32, i32)) -> Taken {
    let l = ((at.1 as u32 & 0xffff) << 16) | (at.0 as u32 & 0xffff);
    let host = &mut s.players[n].client.host;
    host.mouse_move(at);
    let taken = host.post(WM_LBUTTONDOWN, 1, l);
    host.post(WM_LBUTTONUP, 0, l);
    taken
}

#[test]
#[ignore = "needs the realm guest: `just lua-client-web`, then run with --include-ignored (`just examples level-demo catalog-test` does)"]
fn the_windows_clicks_reach_the_prompt_and_the_catalog_and_with_the_cursor_off_the_play_map() {
    let Some(mut s) = session("clicks") else {
        return;
    };
    s.join(0).unwrap();
    let mut seen = BTreeSet::new();
    assert!(until_playing(&mut s, 0, &mut seen), "{seen:?}");
    assert!(s.run_until(3_000, |s| realm_state(&s.players[0]) == RealmState::Asking));
    s.run(24);
    // The routing a window's click takes (the global map, the canvas, the other maps): the canvas, with the prompt's cursor on.
    let size = s.players[0].client.host.gui.0.borrow().canvas_size;
    let (_, allow) = prompt_spots(size, ABILITIES)
        .into_iter()
        .find(|(spot, _)| *spot == PromptSpot::Allow)
        .expect("the prompt's Allow");
    assert_eq!(window_click(&mut s, 0, center(allow)), Taken::Canvas);
    assert!(
        s.run_until(300, |s| !cards_of(&s.players[0].client).is_empty()),
        "the catalog started"
    );
    // A card, clicked the same way, asks the server.
    let chair = card_controls(&s.players[0].client)
        .find(|c| c.text == "Chair")
        .expect("the Chair card");
    s.stand("Ann", -5.0, 2.0, std::f32::consts::FRAC_PI_2)
        .unwrap();
    assert_eq!(window_click(&mut s, 0, center(chair.rect)), Taken::Canvas);
    assert!(
        s.run_until(300, |s| recent_of(&s.players[0].client)
            .first()
            .is_some_and(|l| l.starts_with("Ann put down a chair"))),
        "{:?} {:?}",
        status_of(&s.players[0].client),
        recent_of(&s.players[0].client)
    );
    // With the cursor off (the catalog hidden, the game's mouse look), a click is the play map's (it picks up or drops).
    s.players[0]
        .client
        .call("eval", &["Canvas.cursorOff();"])
        .unwrap();
    s.run(5);
    let play_map = s.players[0]
        .client
        .vm()
        .objects()
        .find(b"PlayMap")
        .expect("PlayMap");
    assert_eq!(window_click(&mut s, 0, center(allow)), Taken::Map(play_map));
}
