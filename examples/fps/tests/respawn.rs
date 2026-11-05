//! A knocked-out figure falls, then its new figure stands: Ann knocks Bob out (`game/fixtures/floor.mis`), both clients draw his
//! fall, and once he respawns both draw him standing (`root` or a moving action, not a death) with his arm and head threads
//! running, and the server poses his hitboxes standing too, over several knock-outs (a new figure may take the ghost index
//! of an old one).

use torque::game::sim::player;

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

fn frame(a: &mut ArenaGame, b: &mut ArenaGame, ms: u32) {
    for g in [a, b] {
        g.step(ms);
        g.add_frame_time(ms as f32 / 1000.0);
        g.view_frame();
    }
}

/// Runs `text` on Ann's server.
fn eval(a: &mut ArenaGame, text: &str) -> String {
    let vm = a.server.as_mut().unwrap().vm_mut();
    String::from_utf8(vm.eval(text.as_bytes()).unwrap()).unwrap()
}

/// The action the server's figure of `name` plays (its hitboxes' pose).
fn server_action(a: &mut ArenaGame, name: &str) -> u32 {
    let id: u32 = eval(a, &format!("return figureOf(\"{name}\");"))
        .parse()
        .unwrap();
    let vm = a.server.as_mut().unwrap().vm_mut();
    player::state(vm, id).unwrap().action_anim
}

/// Every figure's drawn action on `g`'s screen, its own included.
fn actions(g: &ArenaGame) -> Vec<String> {
    g.scene()
        .player_actions()
        .into_iter()
        .map(|(_, a)| a)
        .collect()
}

#[test]
fn a_respawned_figure_stands_on_both_screens() {
    let mut a = client("Ann");
    let mut b = client("Bob");
    a.call("hostGame", &[]).unwrap();
    for _ in 0..300 {
        frame(&mut a, &mut b, 16);
        if a.in_game() {
            break;
        }
    }
    let address = a.hosted_address().unwrap().to_string();
    b.call("connectTo", &[&address]).unwrap();
    for _ in 0..300 {
        frame(&mut a, &mut b, 16);
        if b.in_game() && a.scene().others().len() == 1 && b.scene().others().len() == 1 {
            break;
        }
    }
    for _ in 0..60 {
        frame(&mut a, &mut b, 16);
    }
    let standing = |g: &ArenaGame, who: &str, when: &str| {
        let drawn = actions(g);
        eprintln!("{who} {when}: {:?}", g.scene().player_actions());
        for name in &drawn {
            assert!(
                ["root", "run", "back", "side"].contains(&name.as_str()),
                "{who} draws a figure in {name:?} {when}: {drawn:?}"
            );
        }
        let threads = g.scene().player_threads();
        assert!(
            threads.iter().all(|&(_, arm, head)| arm && head),
            "{who}'s figures' arm and head threads run {when}: {threads:?}"
        );
    };
    standing(&a, "Ann", "at the start");
    standing(&b, "Bob", "at the start");
    for round in 0..4 {
        // Ann shoots until Bob falls.
        let mut fell = false;
        for _ in 0..8 {
            a.call("fire", &["1"]).unwrap();
            for i in 0..40 {
                if i == 4 {
                    a.call("fire", &["0"]).unwrap();
                }
                frame(&mut a, &mut b, 16);
            }
            if a.scene().others_arms() == [(true, None)] {
                fell = true;
                break;
            }
        }
        assert!(fell, "round {round}: Ann sees Bob fall");
        assert!(
            server_action(&mut a, "Bob") >= 8,
            "the server's figure lies in its death"
        );
        // He lies there until the arena respawns him (2.5 s after the knock-out), then stands again.
        for _ in 0..200 {
            frame(&mut a, &mut b, 16);
        }
        // Back at the spawn facing Ann (the arena's next spawn point may be hers).
        eval(&mut a, "placeFigure(\"Bob\", 0, 6, 0.1, 3.14159);");
        for _ in 0..30 {
            frame(&mut a, &mut b, 16);
        }
        let when = format!("after respawn {round}");
        let action = server_action(&mut a, "Bob");
        assert!(
            action < 4,
            "the server poses Bob's hitboxes standing {when}: action {action}"
        );
        standing(&a, "Ann", &when);
        standing(&b, "Bob", &when);
        assert_eq!(
            a.scene().others_arms(),
            [(false, Some("hold".to_string()))],
            "Ann sees Bob's arm hold the pistol {when}"
        );
        assert_eq!(
            b.scene().others_arms(),
            [(false, Some("hold".to_string()))],
            "Bob sees Ann's arm hold the pistol {when}"
        );
    }
}
