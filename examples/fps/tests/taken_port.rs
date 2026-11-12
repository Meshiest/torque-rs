//! A game hosted for one client plays out the same whether or not the default port is taken: the game asks for no fixed port, so
//! what holds it changes nothing the scripts, the packets or the figures do. Over the in-process network (every datagram there at
//! the next read) the two runs compare exactly.

use std::net::Ipv4Addr;

use torque::app::transports::{
    InstantNet, InstantNetClientTransportPlugin, InstantNetServerTransportPlugin,
};
use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;
use torque_example_fps::plugin::{ArenaPlugin, arena_engine};
use torque_example_fps::server::DEFAULT_PORT;

/// A client over `net` that asks for the default port, as a game made with default options does.
fn client(name: &str, net: &InstantNet) -> ArenaGame {
    let files = GameFiles::bundled().unwrap();
    let mut engine = arena_engine(ArenaPlugin::new(&files));
    engine.add_plugins((
        InstantNetClientTransportPlugin,
        InstantNetServerTransportPlugin,
    ));
    let engine = engine.build().unwrap();
    let options = ClientOptions {
        net: NetMode::Loopback,
        name: name.to_string(),
        port: DEFAULT_PORT,
        map: Some("fixtures/aim.mis".to_string()),
        instant_net: Some(net.clone()),
        ..ClientOptions::default()
    };
    ArenaGame::from_engine(&engine, files, options, None).unwrap()
}

/// 16 ms of both clocks and a frame of each.
fn frame(a: &mut ArenaGame, b: &mut ArenaGame) {
    for g in [a, b] {
        g.step(16);
        g.add_frame_time(0.016);
        g.view_frame();
    }
}

/// Everything about a hosted game with a second player that a port could change: where it listens, the packets the server sent,
/// the figures, the lines the server printed, and what the scripts' random numbers give on both sides.
fn trace(taken: bool) -> Vec<String> {
    let net = InstantNet::default();
    // The default port held on the in-process network the game runs on.
    let _held = taken.then(|| net.listen(Ipv4Addr::LOCALHOST, DEFAULT_PORT).unwrap());

    let (mut a, mut b) = (client("Ann", &net), client("Bob", &net));
    a.call("hostGame", &[]).unwrap();
    for _ in 0..300 {
        frame(&mut a, &mut b);
        if a.in_game() {
            break;
        }
    }
    assert!(a.in_game(), "the game started");
    let address = a.hosted_address().unwrap();
    b.call("connectTo", &[&address.to_string()]).unwrap();
    for _ in 0..300 {
        frame(&mut a, &mut b);
        if b.in_game() && b.scene().others().len() == 1 {
            break;
        }
    }
    assert_eq!(b.scene().others().len(), 1, "Bob sees Ann");
    for _ in 0..200 {
        frame(&mut a, &mut b);
    }

    let server = a.server.as_mut().unwrap();
    let packets = server.shared().unwrap().borrow().counters.packets_sent;
    let draw = |game: &mut ArenaGame| game.call("eval", &["return getRandom(1000000);"]).unwrap();
    let mut lines = vec![
        format!("address {address}"),
        format!("packets {packets}"),
        format!("ann random {}", draw(&mut a)),
        format!("bob random {}", draw(&mut b)),
        format!("ann look {:?}", a.own_look().map(|l| format!("{l:?}"))),
        format!("bob look {:?}", b.own_look().map(|l| format!("{l:?}"))),
        format!("natives {}", a.vm().native_listing().len()),
    ];
    lines.extend(a.server_log.iter().cloned());
    lines
}

#[test]
fn a_game_hosted_while_the_default_port_is_taken_plays_as_when_it_is_free() {
    let free = trace(false);
    let taken = trace(true);
    assert_eq!(free, taken);
    assert!(free.len() > 10, "the traces hold the server's lines");
}
