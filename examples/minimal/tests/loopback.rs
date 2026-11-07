//! The example server and client in one process over the loopback address: the client connects, is ghosted the mission's
//! objects and its own camera, sees the gem move, and leaves.

use std::time::{Duration, Instant};

use torque_example_minimal::client::{ClientNote, ExampleClient};
use torque_example_minimal::server::{DedicatedServer, ServerNote, ServerOptions};

fn class_count(client: &ExampleClient, class: &str) -> usize {
    client.seen().iter().filter(|g| g.class == class).count()
}

#[test]
fn a_client_on_the_loopback_address_sees_the_missions_ghosts() {
    let mut server =
        DedicatedServer::start(&ServerOptions::loopback(0)).expect("the server starts");
    assert_eq!(server.ghosted_count(), 2, "the crate and the gem");
    let address = server.local_addr().expect("bound");
    let mut client = ExampleClient::connect(address).expect("the client's socket");

    let start = Instant::now();
    let mut gem_start = None;
    let mut gem_moved = false;
    while start.elapsed() < Duration::from_secs(10) && !gem_moved {
        let now = start.elapsed().as_millis() as u32;
        server.tick(now);
        client.tick(now);
        if let Some(gem) = client.seen().into_iter().find(|g| g.class == "Item") {
            let position = gem.position.expect("an item's record carries its position");
            match gem_start {
                None => gem_start = Some(position),
                Some(first) => gem_moved = position != first && gem.updates > 0,
            }
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    assert!(client.connected());
    let notes = client.take_notes();
    assert!(notes.contains(&ClientNote::GhostingStarted), "{notes:?}");
    assert!(gem_moved, "the gem's moves arrive: {:?}", client.seen());
    assert_eq!(class_count(&client, "StaticShape"), 1);
    assert_eq!(class_count(&client, "Item"), 1);
    assert_eq!(class_count(&client, "Camera"), 1, "the client's own camera");

    let crate_ghost = client
        .seen()
        .into_iter()
        .find(|g| g.class == "StaticShape")
        .unwrap();
    assert_eq!(crate_ghost.position, Some([0.0, 0.0, 0.0]));
    assert!(crate_ghost.datablock.is_some());
    // The gem circles the crate at radius 4, 1.5 up.
    let gem = client
        .seen()
        .into_iter()
        .find(|g| g.class == "Item")
        .unwrap();
    let [x, y, z] = gem.position.unwrap();
    assert!(
        ((x * x + y * y).sqrt() - 4.0).abs() < 1e-3 && z == 1.5,
        "{gem}"
    );

    let notes = server.take_notes();
    assert!(
        notes
            .iter()
            .any(|n| matches!(n, ServerNote::Connected { .. }))
    );
    assert!(
        notes
            .iter()
            .any(|n| matches!(n, ServerNote::Ghosting { .. }))
    );
    assert_eq!(server.client_count(), 1);

    client.disconnect();
    let now = start.elapsed().as_millis() as u32;
    let until = Instant::now() + Duration::from_secs(2);
    while server.client_count() > 0 && Instant::now() < until {
        server.tick(now + 1);
        std::thread::sleep(Duration::from_millis(4));
    }
    assert_eq!(server.client_count(), 0);
    assert!(
        server
            .take_notes()
            .iter()
            .any(|n| matches!(n, ServerNote::Left { .. })),
    );
    // The scripts deleted the camera with the client.
    let cameras = server
        .vm()
        .objects()
        .iter()
        .filter(|(_, o)| server.vm().objects().classes.name(o.class()) == b"Camera")
        .count();
    assert_eq!(cameras, 0);
}
