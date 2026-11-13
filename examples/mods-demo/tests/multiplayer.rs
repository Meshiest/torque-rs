//! The game people play, end to end over the loopback address, as the arena's tests run it (`examples/fps`'s `arena.rs` and
//! `rtc.rs`): a soccer server on a real UDP socket with its bots, a player who finds it in the network's list of games, sees
//! the soccer mod download, takes a bot's place and runs with the keys, and joins again with the mod already downloaded; and
//! browsers' clients that download the mod over a game socket (WebSocket) and over a WebRTC data channel beside it.
//!
//! The server runs the Lua game mode, so these tests need the Lua extension (Luau's C++) and are ignored by `cargo test`:
//! `just examples mods-demo test` runs them.

use crate::common;

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::thread;
use std::time::Duration;

use torque::app::serve::{Bind, Transports, web_listen};
use torque::app::server::GameServer;
use torque::client::input::Key;
use torque::client::net::Datagrams;
use torque::client::net::web_rtc::{Link, WebRtcDatagrams};
use torque::client::net::web_socket::WebSocketDatagrams;
use torque::mods::{CacheChoice, Mods};
use torque::server::net::web::WebSetup;
use torque::server::net::web::rtc::RtcSetup;
use torque_example_mods_demo::game::{ClientOptions, JoinState, NetMode, SoccerGame};
use torque_example_mods_demo::server::{ServerOptions, server_engine, start};
use torque_example_mods_demo::{client_builder, client_files};

use common::{lua, work};

/// `ms` of the server's ticks and the clients' steps; with `wall`, at the wall clock's pace too (ICE and DTLS run on it).
fn run(server: &mut GameServer, games: &mut [&mut SoccerGame], ms: u32, wall: bool) {
    for _ in 0..ms / 32 {
        server.tick();
        server.service();
        for game in games.iter_mut() {
            game.step(32);
        }
        if wall {
            thread::sleep(Duration::from_millis(4));
        }
    }
}

/// Runs until `done` holds of `games[0]`, at most `ms`; whether it did. Each join state seen is kept in `seen`.
fn run_until(
    server: &mut GameServer,
    games: &mut [&mut SoccerGame],
    ms: u32,
    seen: &mut BTreeSet<JoinState>,
    done: impl Fn(&SoccerGame) -> bool,
) -> bool {
    for _ in 0..ms / 32 {
        seen.insert(games[0].join_state());
        if done(games[0]) {
            return true;
        }
        run(server, games, 32, true);
    }
    done(games[0])
}

fn options(port: u16) -> ClientOptions {
    ClientOptions {
        net: NetMode::Loopback,
        name: "Ann".to_string(),
        port,
        ..ClientOptions::default()
    }
}

/// A client of the game with none of the mods, its downloads kept in `cache`, on `socket` if it has one.
fn game(dir: &std::path::Path, port: u16, socket: Option<Box<dyn Datagrams>>) -> SoccerGame {
    let files = client_files().unwrap();
    let engine = client_builder(&files, CacheChoice::Folder(dir.join("cache")))
        .unwrap()
        .build()
        .unwrap();
    SoccerGame::from_engine(&engine, &files, options(port), socket).unwrap()
}

fn number(server: &GameServer, name: &str) -> u32 {
    String::from_utf8_lossy(&server.vm().global(name).unwrap_or_default())
        .parse()
        .unwrap_or(0)
}

/// The bytes of the soccer mod's content the server offers.
fn content(server: &GameServer) -> u64 {
    Mods::of(server.vm())
        .and_then(|m| m.content_of("soccer"))
        .expect("the server has the soccer mod")
        .bytes
}

/// Where the client sees its own player.
fn own_position(game: &SoccerGame) -> Option<[f32; 3]> {
    game.client.with_world(|w| w.control.position).flatten()
}

#[test]
#[ignore = "builds the Lua extension (Luau's C++)"]
fn a_player_finds_the_server_downloads_the_mod_takes_a_bots_place_and_plays() {
    let dir = work("udp");
    let options = ServerOptions {
        port: 0,
        name: "Soccer in the park".to_string(),
        ..ServerOptions::default()
    };
    let engine = server_engine(&lua(&dir)).unwrap();
    let mut server = start(&engine, &options, options.udp()).unwrap();
    let port = server.address().unwrap().port();
    let mut ann = game(&dir, port, None);
    run(&mut server, &mut [&mut ann], 320, false);
    assert_eq!(
        number(&server, "$Soccer::Bots"),
        2,
        "two bots play an empty server"
    );
    assert_eq!(
        ann.join_state(),
        JoinState::Menu,
        "the client starts at its menu"
    );

    // The list of games finds the server and shows its players and bots.
    ann.client.call("showJoin", &[]).unwrap();
    let mut seen = BTreeSet::new();
    let listed = run_until(&mut server, &mut [&mut ann], 5_000, &mut seen, |g| {
        g.client.number("$Game::ServerRows") > 0.0
    });
    assert!(listed, "the list of games found the server");
    let row = ann
        .client
        .call("eval", &["return ServerList.getRowText(0);"])
        .unwrap();
    let cells: Vec<&str> = row.split('\t').collect();
    assert_eq!(cells[..3], ["Soccer in the park", "0 / 8", "2"], "{row:?}");

    // Joining it: the download screen shows the mod, its files and its bytes, then the match.
    ann.client.call("joinSelected", &[]).unwrap();
    let mut seen = BTreeSet::new();
    let downloading = run_until(&mut server, &mut [&mut ann], 10_000, &mut seen, |g| {
        g.join_state() == JoinState::Downloading
    });
    assert!(downloading, "the download started: {seen:?}");
    let bytes = content(&server);
    // The offer carries the mod's display name, not its id.
    assert_eq!(ann.client.global("$Join::Mods"), "Soccer");
    assert_eq!(ann.client.global("$Join::Versions"), "1.0.0");
    assert_eq!(ann.client.global("$Join::Bytes"), bytes.to_string());
    assert!(ann.client.number("$Join::Files") > 10.0);
    let detail = ann
        .client
        .call("eval", &["return JoinScreenDetail.getValue();"])
        .unwrap();
    assert!(
        detail.contains("files,") && detail.contains("KB to download"),
        "{detail:?}"
    );
    let playing = run_until(
        &mut server,
        &mut [&mut ann],
        30_000,
        &mut seen,
        SoccerGame::playing,
    );
    assert!(
        playing,
        "the client joined the match: {seen:?} {:?}",
        ann.client.client_log
    );
    for state in [
        JoinState::Connecting,
        JoinState::Downloading,
        JoinState::Joining,
        JoinState::Playing,
    ] {
        assert!(
            seen.contains(&state),
            "the screen showed {state:?}: {seen:?}"
        );
    }
    assert_eq!(ann.client.global("$Join::Received"), bytes.to_string());
    run(&mut server, &mut [&mut ann], 320, false);
    assert_eq!(
        number(&server, "$Soccer::Bots"),
        1,
        "a bot left for the player"
    );
    assert_eq!(number(&server, "$Server::PlayerCount"), 1);
    let team = ann.client.global("$Match::Team");
    assert!(team == "Blue" || team == "Red", "{team:?}");

    // The keys: D runs toward the goal the player attacks (the camera puts it on the screen's right).
    let attack = ann.client.number("$Match::Attack");
    let in_play = |s: &GameServer| {
        String::from_utf8_lossy(&s.vm().global("$Soccer::Phase").unwrap_or_default()) == "play"
    };
    for _ in 0..200 {
        if in_play(&server) {
            break;
        }
        run(&mut server, &mut [&mut ann], 32, false);
    }
    let before = own_position(&ann).expect("the client holds its player");
    ann.client.key(Key::D, true);
    run(&mut server, &mut [&mut ann], 640, false);
    ann.client.key(Key::D, false);
    let after = own_position(&ann).unwrap();
    assert!(
        (after[1] - before[1]) * attack > 1.0,
        "the player ran toward the goal it attacks: {before:?} to {after:?}, attack {attack}"
    );

    // Leaving gives the place back to a bot; a second join downloads nothing.
    ann.client.call("leaveMatch", &[]).unwrap();
    run(&mut server, &mut [&mut ann], 640, false);
    assert_eq!(
        number(&server, "$Soccer::Bots"),
        2,
        "a bot took the place back"
    );
    ann.join(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)).to_string())
        .unwrap();
    let mut seen = BTreeSet::new();
    let again = run_until(
        &mut server,
        &mut [&mut ann],
        15_000,
        &mut seen,
        SoccerGame::playing,
    );
    assert!(again, "the second join: {seen:?}");
    assert!(
        seen.contains(&JoinState::Cached),
        "the screen said nothing was downloaded: {seen:?}"
    );
    assert!(!seen.contains(&JoinState::Downloading), "{seen:?}");
    assert_eq!(
        ann.client.call("getModDownloadProgress", &[]).unwrap(),
        "0 0 0 0"
    );
}

/// A browser's game socket to `server`'s web transport, made as a page makes one: with a data channel beside it when
/// `channel` (a page's link, `WebRtcDatagrams`), else the plain socket.
fn web_socket(
    server: &mut GameServer,
    channel: bool,
) -> (
    Box<dyn Datagrams>,
    Option<torque::client::net::web_rtc::LinkWatch>,
) {
    let addr = server.address().unwrap();
    let SocketAddr::V4(v4) = addr else {
        panic!("an IPv4 server");
    };
    let h = thread::spawn(move || WebSocketDatagrams::connect(addr, v4));
    while !h.is_finished() {
        server.service();
        thread::sleep(Duration::from_millis(1));
    }
    let socket = h.join().unwrap().unwrap();
    if !channel {
        return (Box::new(socket), None);
    }
    let socket = WebRtcDatagrams::new(socket);
    let watch = socket.watch();
    (Box::new(socket), Some(watch))
}

#[test]
#[ignore = "builds the Lua extension (Luau's C++)"]
fn browsers_download_the_mod_over_a_game_socket_and_over_a_data_channel() {
    let dir = work("web");
    let options = ServerOptions {
        port: 0,
        ..ServerOptions::default()
    };
    let bind = Bind {
        address: Ipv4Addr::LOCALHOST,
        port: 0,
        public: false,
    };
    let setup = WebSetup {
        rtc: Some(RtcSetup::default()),
        ..WebSetup::default()
    };
    let engine = server_engine(&lua(&dir)).unwrap();
    let mut server = start(&engine, &options, web_listen(Transports::Both, bind, setup)).unwrap();
    let address = match server.address().unwrap() {
        SocketAddr::V4(v4) => v4.to_string(),
        SocketAddr::V6(_) => panic!("an IPv4 server"),
    };
    let bytes = content(&server);

    // A page on its game socket alone.
    let (socket, _) = web_socket(&mut server, false);
    let mut bob = game(&dir.join("bob"), 0, Some(socket));
    bob.join(&address).unwrap();
    let mut seen = BTreeSet::new();
    let playing = run_until(
        &mut server,
        &mut [&mut bob],
        30_000,
        &mut seen,
        SoccerGame::playing,
    );
    assert!(
        playing,
        "the game socket's client joined: {seen:?} {:?}",
        bob.client.client_log
    );
    assert!(seen.contains(&JoinState::Downloading), "{seen:?}");
    assert_eq!(bob.client.global("$Join::Received"), bytes.to_string());

    // A page whose data channel opens before it joins: the mod comes on the channel.
    let (socket, watch) = web_socket(&mut server, true);
    let watch = watch.expect("a data channel's link");
    let mut cid = game(&dir.join("cid"), 0, Some(socket));
    for _ in 0..300 {
        if watch.borrow().link == Link::Channel {
            break;
        }
        run(&mut server, &mut [&mut cid, &mut bob], 32, true);
    }
    assert_eq!(
        watch.borrow().link,
        Link::Channel,
        "the data channel opened"
    );
    let before = watch.borrow().messages_in;
    cid.join(&address).unwrap();
    let mut seen = BTreeSet::new();
    let playing = run_until(
        &mut server,
        &mut [&mut cid, &mut bob],
        30_000,
        &mut seen,
        SoccerGame::playing,
    );
    assert!(
        playing,
        "the data channel's client joined: {seen:?} {:?}",
        cid.client.client_log
    );
    assert!(seen.contains(&JoinState::Downloading), "{seen:?}");
    assert_eq!(cid.client.global("$Join::Received"), bytes.to_string());
    let w = watch.borrow();
    assert_eq!(w.link, Link::Channel, "still on the channel");
    // The download alone is more than 60 datagrams of about a kilobyte each.
    assert!(
        w.messages_in > before + 60,
        "the server's packets came on the channel: {w:?}"
    );
    drop(w);
    run(&mut server, &mut [&mut cid, &mut bob], 320, true);
    assert_eq!(
        number(&server, "$Soccer::Bots"),
        0,
        "two people: no bot plays"
    );
}

#[test]
#[ignore = "builds the Lua extension (Luau's C++)"]
fn two_bots_play_a_match_on_an_empty_server() {
    let dir = work("bots");
    let options = ServerOptions {
        port: 0,
        ..ServerOptions::default()
    };
    let engine = server_engine(&lua(&dir)).unwrap();
    let mut server = start(&engine, &options, options.udp()).unwrap();
    // A minute of the match at most (32 ms ticks).
    for _ in 0..1_900 {
        server.tick();
        if number(&server, "$Soccer::Goals") >= 1 {
            break;
        }
    }
    assert_eq!(number(&server, "$Soccer::Bots"), 2);
    assert!(
        number(&server, "$Soccer::Kicks") > 2,
        "the bots kicked the ball: {}",
        number(&server, "$Soccer::Kicks")
    );
    assert!(
        number(&server, "$Soccer::Goals") >= 1,
        "the bots scored: kicks {}",
        number(&server, "$Soccer::Kicks")
    );
    let console: Vec<String> = server
        .vm_mut()
        .drain_log()
        .into_iter()
        .map(|l| String::from_utf8_lossy(&l.text).into_owned())
        .collect();
    assert!(
        !console.iter().any(|l| l.contains("Unknown command")),
        "{console:?}"
    );
}
