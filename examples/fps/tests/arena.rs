//! The arena over the loopback address: a host and a second client find each other through the LAN list, one shoots the other,
//! and a chat line goes round; the host reloads, by the key and by emptying the magazine, and knocks the other out, who falls
//! over backward (shot from the front) and stays down. The game plays the test floor (`game/fixtures/floor.mis`): two figures
//! facing each other.

use torque::client::player_anim::{ServerAction, action_list};
use torque::shapes::shape::Shape;
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

/// One shot: the trigger down, a moment, up, then `ms` of both clocks.
fn shoot(a: &mut ArenaGame, b: &mut ArenaGame, ms: u32) {
    a.call("fire", &["1"]).unwrap();
    step_both(a, b, 96);
    a.call("fire", &["0"]).unwrap();
    step_both(a, b, ms);
}

/// The server's action of each Player ghost `game` holds (the ones the server set past the table: a death).
fn player_actions(game: &ArenaGame) -> Vec<ServerAction> {
    game.with_world(|w| {
        w.records
            .ghosts
            .values()
            .filter_map(|g| ServerAction::of_record(g.record.as_ref()?))
            .collect()
    })
    .unwrap_or_default()
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
        trail.particle_data[0].lifetime_ms, 160,
        "as the script gave it"
    );

    // The pistol's fire state names the muzzle flash's emitter, which the client holds with its particles.
    let flash = b
        .with_world(|w| {
            let (&db, _) = w
                .datablocks
                .iter()
                .find(|(_, d)| d.class == "ShapeBaseImageData")?;
            torque_example_fps::scene::fire_emitter(w, db).map(|(e, _)| e)
        })
        .flatten()
        .expect("the muzzle flash");
    assert_eq!(flash.particle_data[0].lifetime_ms, 640);

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
    assert_eq!(b.global("$Chat::Last"), "Ann: hello there");

    // R reloads a magazine that is not full: the HUD says so while it runs, then the magazine is full again.
    a.call("reloadKey", &["1"]).unwrap();
    step_both(&mut a, &mut b, 300);
    assert_eq!(a.global("$Arena::Reloading"), "1", "reloading");
    assert_eq!(
        a.call("eval", &["return ReloadHud.getValue();"]).unwrap(),
        "Reloading..."
    );
    step_both(&mut a, &mut b, 1500);
    assert_eq!(a.global("$Arena::Ammo"), "11", "a full magazine");
    assert_eq!(a.global("$Arena::Reloading"), "0");
    assert_eq!(
        a.call("eval", &["return ReloadHud.getValue();"]).unwrap(),
        ""
    );
    // A full magazine does not reload.
    a.call("reloadKey", &["1"]).unwrap();
    step_both(&mut a, &mut b, 300);
    assert_eq!(a.global("$Arena::Reloads"), "1");

    // Two more hits knock the other out: it falls backward (shot from the front) and holds there.
    assert!(player_actions(&b).iter().all(|x| x.action < 8));
    shoot(&mut a, &mut b, 600);
    shoot(&mut a, &mut b, 1200);
    if a.global("$Arena::Hits") != "3" {
        dump(&a, &b);
    }
    assert_eq!(a.global("$Arena::Hits"), "3", "three hits");
    let figure = Shape::for_server(torque_example_assets::shapes::figure());
    let actions = action_list(&figure);
    let named = |x: &ServerAction| actions[x.action as usize].name.clone();
    for (who, game) in [("the shooter", &a), ("the one shot", &b)] {
        let deaths: Vec<_> = player_actions(game)
            .iter()
            .filter(|x| x.action >= 8)
            .map(|x| (named(x), x.hold))
            .collect();
        assert_eq!(deaths, [(b"death1".to_vec(), true)], "{who} sees the fall");
    }

    // Emptying the magazine reloads it by itself.
    let left: u32 = a.global("$Arena::Ammo").parse().unwrap();
    for _ in 0..left {
        shoot(&mut a, &mut b, 200);
    }
    assert_eq!(a.global("$Arena::Reloads"), "2", "empty: a reload");
    step_both(&mut a, &mut b, 1500);
    assert_eq!(a.global("$Arena::Ammo"), "11");
}
