//! A browser on a WebRTC data channel and a desktop on UDP in one game, over the loopback address: a dedicated server on
//! `--transport both --webrtc`, a client over UDP and a client whose game socket opens a data channel (as a browser's
//! does). Each sees the other's figure, chat goes both ways and shots land both ways while the browser's datagrams go on
//! the channel. When the channel cannot open (every datagram of it dropped), the browser plays over its game socket.

use std::net::{Ipv4Addr, SocketAddr};
use std::thread;
use std::time::Duration;

use torque::app::server::GameServer;
use torque::client::net::web_rtc::{Link, LinkWatch, WebRtcDatagrams};
use torque::client::net::web_socket::WebSocketDatagrams;
use torque::server::net::web::WebSetup;
use torque::server::net::web::rtc::RtcSetup;
use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;
use torque_example_fps::server::{ArenaServer, ServerOptions, Transports};

fn server(rtc: RtcSetup) -> GameServer {
    ArenaServer::start_dedicated_on(
        &GameFiles::bundled().unwrap(),
        &ServerOptions {
            address: Ipv4Addr::LOCALHOST,
            port: 0,
            name: "Arena".to_string(),
            mission: "fixtures/floor.mis".to_string(),
            public: false,
        },
        Transports::Both,
        WebSetup {
            rtc: Some(rtc),
            ..WebSetup::default()
        },
    )
    .unwrap()
}

fn options(name: &str) -> ClientOptions {
    ClientOptions {
        net: NetMode::Loopback,
        name: name.to_string(),
        port: 0,
        ..ClientOptions::default()
    }
}

fn udp_client(server: &GameServer, name: &str) -> ArenaGame {
    let mut c = ArenaGame::new(GameFiles::bundled().unwrap(), options(name)).unwrap();
    let addr = server.address().unwrap().to_string();
    c.call("connectTo", &[&addr]).unwrap();
    c
}

/// A browser's client: a game socket that opens a data channel when the server offers one.
fn rtc_client(server: &mut GameServer, name: &str, open_timeout_ms: u64) -> (ArenaGame, LinkWatch) {
    let addr = server.address().unwrap();
    let SocketAddr::V4(v4) = addr else {
        panic!("an IPv4 server");
    };
    let h = thread::spawn(move || WebSocketDatagrams::connect(addr, v4));
    while !h.is_finished() {
        server.service();
        thread::sleep(Duration::from_millis(1));
    }
    let mut socket = WebRtcDatagrams::new(h.join().unwrap().unwrap());
    socket.set_open_timeout(open_timeout_ms);
    let watch = socket.watch();
    let mut c = ArenaGame::with_socket(
        GameFiles::bundled().unwrap(),
        options(name),
        Box::new(socket),
    )
    .unwrap();
    c.call("connectTo", &[&v4.to_string()]).unwrap();
    (c, watch)
}

/// `ms` of the server's ticks and the clients' steps, at the wall clock's pace (ICE and DTLS run on it).
fn run(server: &mut GameServer, clients: &mut [&mut ArenaGame], ms: u32) {
    for _ in 0..ms / 32 {
        server.tick();
        server.service();
        for c in clients.iter_mut() {
            c.step(32);
        }
        thread::sleep(Duration::from_millis(4));
    }
}

fn players(game: &ArenaGame) -> usize {
    game.with_world(|w| {
        w.records
            .ghosts
            .values()
            .filter(|g| g.class == Some(torque::ghosts::TgeClass::Player))
            .count()
    })
    .unwrap_or(0)
}

fn shoot(server: &mut GameServer, from: &mut ArenaGame, other: &mut ArenaGame) {
    from.call("fire", &["1"]).unwrap();
    run(server, &mut [from, other], 96);
    from.call("fire", &["0"]).unwrap();
    run(server, &mut [from, other], 1500);
}

/// Ann on UDP in the game, then Bob over `bob`'s links, both holding both figures.
fn two_players(s: &mut GameServer, open_timeout_ms: u64) -> (ArenaGame, ArenaGame, LinkWatch) {
    let mut ann = udp_client(s, "Ann");
    for _ in 0..200 {
        run(s, &mut [&mut ann], 32);
        if ann.in_game() {
            break;
        }
    }
    assert!(ann.in_game(), "the desktop player joined over UDP");
    let (mut bob, watch) = rtc_client(s, "Bob", open_timeout_ms);
    for _ in 0..300 {
        run(s, &mut [&mut ann, &mut bob], 32);
        if bob.in_game() {
            break;
        }
    }
    assert!(bob.in_game(), "the browser player joined");
    run(s, &mut [&mut ann, &mut bob], 1000);
    assert_eq!(players(&ann), 2, "the desktop sees both figures");
    assert_eq!(players(&bob), 2, "the browser sees both figures");
    (ann, bob, watch)
}

#[test]
fn a_browser_on_a_data_channel_and_a_desktop_play_one_game() {
    let mut s = server(RtcSetup::default());
    let (mut ann, mut bob, watch) = two_players(&mut s, 10_000);
    for _ in 0..100 {
        if watch.borrow().link == Link::Channel {
            break;
        }
        run(&mut s, &mut [&mut ann, &mut bob], 32);
    }
    assert_eq!(
        watch.borrow().link,
        Link::Channel,
        "the browser's datagrams go on the channel"
    );
    let before = watch.borrow().messages_in;

    bob.call("say", &["over the channel"]).unwrap();
    run(&mut s, &mut [&mut ann, &mut bob], 500);
    assert_eq!(ann.global("$Chat::Last"), "Bob: over the channel");
    ann.call("say", &["back"]).unwrap();
    run(&mut s, &mut [&mut ann, &mut bob], 500);
    assert_eq!(bob.global("$Chat::Last"), "Ann: back");

    shoot(&mut s, &mut ann, &mut bob);
    assert_eq!(
        ann.global("$Arena::Hits"),
        "1",
        "the desktop's bolt hit the browser's figure"
    );
    shoot(&mut s, &mut bob, &mut ann);
    assert_eq!(
        bob.global("$Arena::Hits"),
        "1",
        "the browser's bolt hit the desktop's figure"
    );

    let w = watch.borrow();
    assert_eq!(w.link, Link::Channel, "still on the channel");
    assert!(
        w.messages_in > before + 50,
        "the server's packets came on the channel: {w:?}"
    );
    assert!(
        w.messages_out > 50,
        "the browser's packets went on the channel: {w:?}"
    );
    let status = s.transport_status();
    assert!(status.iter().any(|l| l.contains(" open,")), "{status:?}");
}

#[test]
fn a_browser_whose_channel_cannot_open_plays_over_its_socket() {
    // Every datagram of the channels dropped, as when UDP is blocked.
    let mut s = server(RtcSetup {
        loss_percent: 100,
        ..RtcSetup::default()
    });
    let (mut ann, mut bob, watch) = two_players(&mut s, 1500);
    // The channel's open time is the wall clock's, the game's is stepped: step until the wall clock passes it (at most 10 s),
    // however fast the steps run.
    let began = std::time::Instant::now();
    while !watch.borrow().failed && began.elapsed().as_secs() < 10 {
        run(&mut s, &mut [&mut ann, &mut bob], 100);
    }
    {
        let w = watch.borrow();
        assert!(
            w.tried && w.failed,
            "the channel was tried and given up: {w:?}"
        );
        assert_eq!(w.link, Link::Socket);
        assert_eq!(w.messages_out, 0);
    }
    bob.call("say", &["over the socket"]).unwrap();
    run(&mut s, &mut [&mut ann, &mut bob], 500);
    assert_eq!(ann.global("$Chat::Last"), "Bob: over the socket");
    shoot(&mut s, &mut bob, &mut ann);
    assert_eq!(bob.global("$Arena::Hits"), "1");
}
