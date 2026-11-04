//! The arena over the loopback address: a host and a second client find each other through the LAN list, one shoots the other,
//! and a chat line goes round. The game plays the test floor (`game/fixtures/floor.mis`): two figures facing each other.

use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;

fn client(name: &str) -> ArenaGame {
    ArenaGame::new(
        GameFiles::bundled().unwrap(),
        ClientOptions {
            net: NetMode::Loopback,
            name: name.to_string(),
            port: 0,
            map: Some("fixtures/floor.mis".to_string()),
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

fn dump(a: &ArenaGame, b: &ArenaGame) {
    for l in &a.server_log {
        println!("S: {l}");
    }
    for l in &a.client_log {
        println!("A: {l}");
    }
    for l in &b.client_log {
        println!("B: {l}");
    }
}

#[test]
fn two_players_meet_shoot_and_chat() {
    let mut a = client("Ann");
    let mut b = client("Bob");
    a.call("hostGame", &[]).unwrap();
    for _ in 0..200 {
        a.step(32);
        if a.in_game() {
            break;
        }
    }
    assert!(a.in_game(), "the host is in its game");
    // The map's Sky reaches the client: the view's distance and the haze's start.
    use torque::client::hud::HudWorld as _;
    assert_eq!(a.with_world(|w| w.view_distances()), Some((1500.0, 400.0)));

    // The second player's LAN list finds the hosted game.
    let port = a.hosted_address().unwrap().port();
    b.set_global("$Arena::Port", &port.to_string());
    b.call("showJoin", &[]).unwrap();
    for _ in 0..100 {
        step_both(&mut a, &mut b, 32);
        if b.global("$Arena::ServerRows") == "1" {
            break;
        }
    }
    assert_eq!(
        b.global("$Arena::ServerRows"),
        "1",
        "the LAN list shows the hosted game"
    );
    assert_eq!(b.global("$ServerInfo::Name"), "Ann's game");
    b.call("joinSelected", &[]).unwrap();
    for _ in 0..200 {
        step_both(&mut a, &mut b, 32);
        if b.in_game() {
            break;
        }
    }
    assert!(b.in_game(), "the second player joined");
    step_both(&mut a, &mut b, 1000);
    assert!(
        a.own_look().is_some() && b.own_look().is_some(),
        "each client knows its own figure"
    );
    // The bolt's trail reaches the other client: its ProjectileData names a ParticleEmitterData it holds, with its particles.
    let trail = b
        .with_world(|w| {
            let (&db, _) = w
                .datablocks
                .iter()
                .find(|(_, d)| d.class == "ProjectileData")?;
            torque_example_fps::scene::bolt_trail(w, db)
        })
        .flatten()
        .expect("the bolt's trail");
    assert_eq!(trail.particle_data.len(), 1);
    assert_eq!(trail.particle_data[0].textures, ["kit/spark"]);
    assert_eq!(
        trail.particle_data[0].lifetime_ms, 64,
        "as the script gave it"
    );

    // One shot from the host at the other figure: a round spent and a hit.
    assert_eq!(a.global("$Arena::Ammo"), "11");
    a.call("fire", &["1"]).unwrap();
    step_both(&mut a, &mut b, 96);
    a.call("fire", &["0"]).unwrap();
    step_both(&mut a, &mut b, 1500);
    if a.global("$Arena::Hits") != "1" {
        dump(&a, &b);
    }
    assert_eq!(a.global("$Arena::Ammo"), "10", "one round fired");
    assert_eq!(a.global("$Arena::Hits"), "1", "the bolt hit");
    let health: i32 = b.global("$Arena::Health").parse().unwrap();
    assert!(health < 100, "the second player was hurt: {health}");

    // A chat line from the host reaches the other client.
    a.call("say", &["hello there"]).unwrap();
    step_both(&mut a, &mut b, 500);
    assert_eq!(b.global("$Arena::LastChat"), "Ann: hello there");
}
