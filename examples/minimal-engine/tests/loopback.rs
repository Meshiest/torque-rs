//! The example server and client in one process over the loopback address: the client connects, is ghosted the mission's objects and
//! its own camera, sees the gem move, and leaves. The same session as the hand-wired example's test, over the engine's helpers.

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use torque::app::client::ClientNet;
use torque::app::plugins::DefaultPlugins;
use torque::app::server::Listen;
use torque::engine::EngineBuilder;
use torque_example_minimal_engine::{ExamplePlugin, client, files, ghosts, server};

fn count(ghosts: &[(&str, [f32; 3])], class: &str) -> usize {
    ghosts.iter().filter(|(c, _)| *c == class).count()
}

#[test]
fn a_client_on_the_loopback_address_sees_the_missions_ghosts() {
    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, ExamplePlugin(files().unwrap())))
        .build()
        .unwrap();
    let mut server = server(
        &engine,
        Listen::Udp {
            address: Ipv4Addr::LOCALHOST,
            port: 0,
            public: false,
        },
    )
    .expect("the server starts");
    let address = server.address().expect("bound");
    let mut client = client(&engine, ClientNet::Loopback).expect("the client");
    client.call("connectTo", &[&address.to_string()]).unwrap();

    let start = Instant::now();
    let mut gem_start = None;
    let mut gem_moved = false;
    while start.elapsed() < Duration::from_secs(10) && !gem_moved {
        server.tick();
        client.step(32);
        if let Some((_, at)) = ghosts(&client).into_iter().find(|(c, _)| *c == "Item") {
            match gem_start {
                None => gem_start = Some(at),
                Some(first) => gem_moved = at != first,
            }
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    assert!(
        gem_moved,
        "the gem's moves arrive: {:?}
{:?}
{:?}",
        ghosts(&client),
        server.drain_log(),
        client.client_log
    );
    let seen = ghosts(&client);
    assert_eq!(count(&seen, "StaticShape"), 1);
    assert_eq!(count(&seen, "Item"), 1);
    assert_eq!(count(&seen, "Camera"), 1, "the client's own camera");
    assert_eq!(client.ghost_count(), 3);

    let (_, crate_at) = seen.iter().find(|(c, _)| *c == "StaticShape").unwrap();
    assert_eq!(*crate_at, [0.0, 0.0, 0.0]);
    // The gem circles the crate at radius 4, 1.5 up.
    let (_, [x, y, z]) = seen.iter().find(|(c, _)| *c == "Item").unwrap();
    assert!(((x * x + y * y).sqrt() - 4.0).abs() < 1e-3 && *z == 1.5);

    let objects = |server: &torque::app::GameServer, class: &[u8]| {
        let objects = server.vm().objects();
        objects
            .iter()
            .filter(|(_, o)| objects.classes.name(o.class()) == class)
            .count()
    };
    assert_eq!(objects(&server, b"GameConnection"), 1);
    client.call("disconnect", &[]).unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    while objects(&server, b"GameConnection") > 0 && Instant::now() < until {
        server.tick();
        client.step(32);
        std::thread::sleep(Duration::from_millis(4));
    }
    assert_eq!(objects(&server, b"GameConnection"), 0);
    // The scripts deleted the camera with the client.
    assert_eq!(objects(&server, b"Camera"), 0);
}
