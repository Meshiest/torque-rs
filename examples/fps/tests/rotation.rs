//! The map rotation: with a short round, the round ends on both screens with its scores (the highest first) and the next map's
//! name, then the next map loads under the same connections (no reconnect), the scores start again at 0 and both figures
//! stand on the new map, pistol in hand; twice, so the list wraps around to the first map. With no list, the next map is the
//! one after the running one among `game/missions/`, in order of their names.

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

/// Frames until `done` holds (at most `limit` of 16 ms); whether it did.
fn until(
    a: &mut ArenaGame,
    b: &mut ArenaGame,
    limit: u32,
    done: impl Fn(&ArenaGame, &ArenaGame) -> bool,
) -> bool {
    for _ in 0..limit {
        frame(a, b, 16);
        if done(a, b) {
            return true;
        }
    }
    false
}

fn count(g: &ArenaGame, name: &str) -> i32 {
    g.global(name).parse().unwrap_or(0)
}

/// Whether `g` saw `n` rounds end.
fn ended(g: &ArenaGame, n: i32) -> bool {
    count(g, "$Arena::RoundEnds") == n
}

/// Whether `g` started its `n`th map.
fn on_map(g: &ArenaGame, n: i32) -> bool {
    count(g, "$Arena::Maps") == n
}

/// Both clients stand on the map: in the game, their own figures known with the pistol in hand (the datablocks kept through
/// the ghosts' reset), and each drawing the other.
fn both_play(a: &ArenaGame, b: &ArenaGame) -> bool {
    [a, b].iter().all(|g| {
        g.in_game()
            && g.global("$Arena::OwnGhost") != "-1"
            && g.scene().own_muzzle().is_some()
            && g.scene().others().len() == 1
    })
}

#[test]
fn rounds_end_with_scores_and_the_next_map_loads_in_place() {
    let mut a = client("Ann");
    let mut b = client("Bob");
    a.call("hostGame", &[]).unwrap();
    assert!(until(&mut a, &mut b, 300, |a, _| a.in_game()), "Ann hosts");
    let address = a.hosted_address().unwrap().to_string();
    b.call("connectTo", &[&address]).unwrap();
    assert!(until(&mut a, &mut b, 400, both_play), "Bob joins");
    let connections = |a: &mut ArenaGame| {
        eval(
            a,
            "%l = \"\"; for (%i = 0; %i < ClientGroup.getCount(); %i++) %l = %l SPC ClientGroup.getObject(%i); return trim(%l);",
        )
    };
    let joined = connections(&mut a);

    // A round of two seconds, a second of scores, two maps; Bob leads this round.
    eval(
        &mut a,
        "$Arena::MinRoundSeconds = 1; $Pref::Server::RoundSeconds = 2; $Pref::Server::RoundEndSeconds = 1; \
         $Pref::Server::MapList = \"fixtures/floor.mis fixtures/aim.mis\"; startRound();",
    );
    eval(
        &mut a,
        "for (%i = 0; %i < ClientGroup.getCount(); %i++) { %c = ClientGroup.getObject(%i); %c.score = %c.name $= \"Bob\" ? 3 : 1; }",
    );
    assert!(
        until(&mut a, &mut b, 400, |a, b| ended(a, 1) && ended(b, 1)),
        "the round ends on both screens"
    );
    for g in [&a, &b] {
        assert_eq!(g.global("$Arena::Board"), "Bob\t3\nAnn\t1");
        assert_eq!(g.global("$Arena::RoundShown"), "1", "the scores show");
        assert!(g.global("$Arena::LastChat").contains("Bob wins the round"));
    }
    assert_eq!(eval(&mut a, "return $Arena::RoundOver;"), "1");
    // Asked to end again during the pause, the round keeps the next map's timer.
    eval(&mut a, "endRound();");

    for (round, map) in [(1, "aim"), (2, "floor")] {
        assert!(
            until(&mut a, &mut b, 600, |a, b| on_map(a, round + 1)
                && on_map(b, round + 1)
                && both_play(a, b)),
            "map {round}: both clients play on {map}"
        );
        assert_eq!(
            eval(&mut a, "return $Server::MissionFile;"),
            format!("fixtures/{map}.mis")
        );
        for g in [&a, &b] {
            assert_eq!(g.global("$Arena::Map"), map);
            assert_eq!(count(g, "$Arena::MissionEnds"), round);
            assert_eq!(g.global("$Arena::RoundShown"), "0", "the scores are gone");
        }
        assert_eq!(connections(&mut a), joined, "the same connections");
        assert_eq!(
            eval(
                &mut a,
                "%s = \"\"; for (%i = 0; %i < ClientGroup.getCount(); %i++) %s = %s @ ClientGroup.getObject(%i).score; return %s;"
            ),
            "00",
            "the scores start again"
        );
        assert_eq!(eval(&mut a, "return $Arena::RoundOver;"), "0");
        // Nobody joined again: no "joined the arena" lines, only the round's end and its winner's.
        assert!(!a.global("$Arena::LastChat").contains("joined"));
        if round == 1 {
            assert!(
                until(&mut a, &mut b, 400, |a, _| ended(a, 2)),
                "the second round ends"
            );
            assert!(a.global("$Arena::LastChat").contains("is over"));
        }
    }

    // No list: the map after the running one among the game folder's missions, wrapping around.
    let maps = GameFiles::bundled().unwrap().maps();
    assert!(!maps.is_empty());
    for (i, map) in maps.iter().enumerate() {
        let next = eval(
            &mut a,
            &format!(
                "$Pref::Server::MapList = \"\"; %was = $Server::MissionFile; $Server::MissionFile = \"{map}\"; \
                 %n = nextMission(); $Server::MissionFile = %was; return %n;"
            ),
        );
        assert_eq!(next, maps[(i + 1) % maps.len()], "after {map}");
    }

    // The prefs are held to the shortest round and pause the arena allows.
    let bounded = eval(
        &mut a,
        "$Arena::MinRoundSeconds = 30; $Pref::Server::RoundSeconds = 5; $Pref::Server::RoundEndSeconds = 0; \
         return roundMs() SPC roundEndMs();",
    );
    assert_eq!(bounded, "30000 1000");
}

/// Ann hosts a game on `fixtures/floor.mis` and Bob joins it, both playing, with the rotation list `floor`, `aim`.
fn two_players() -> (ArenaGame, ArenaGame) {
    let mut a = client("Ann");
    let mut b = client("Bob");
    a.call("hostGame", &[]).unwrap();
    assert!(until(&mut a, &mut b, 300, |a, _| a.in_game()), "Ann hosts");
    let address = a.hosted_address().unwrap().to_string();
    b.call("connectTo", &[&address]).unwrap();
    assert!(until(&mut a, &mut b, 400, both_play), "Bob joins");
    eval(
        &mut a,
        "$Pref::Server::MapList = \"fixtures/floor.mis fixtures/aim.mis\"; \
         function byName(%n) { for (%i = 0; %i < ClientGroup.getCount(); %i++) \
         if (ClientGroup.getObject(%i).name $= %n) return ClientGroup.getObject(%i); return 0; } \
         sendMapChoices(byName(\"Ann\"));",
    );
    // The host's client is sent the list again.
    assert!(
        until(&mut a, &mut b, 100, |a, _| count(a, "$Client::MapCount")
            == 2),
        "the host was sent the maps"
    );
    (a, b)
}

#[test]
fn a_round_has_five_minutes_and_ten_kills_by_default() {
    let (mut a, _b) = two_players();
    assert_eq!(eval(&mut a, "return $Pref::Server::RoundSeconds;"), "300");
    assert_eq!(eval(&mut a, "return $Pref::Server::ScoreLimit;"), "10");
    assert_eq!(eval(&mut a, "return roundMs();"), "300000");
}

#[test]
fn reaching_the_score_limit_ends_the_round_and_zero_turns_it_off() {
    let (mut a, mut b) = two_players();
    eval(
        &mut a,
        "$T::Log = \"\"; function messageAll(%k, %a, %b) { $T::Log = $T::Log @ %a @ \"|\"; }",
    );
    let knock = |a: &mut ArenaGame| {
        eval(
            a,
            "%ann = byName(\"Ann\"); %bob = byName(\"Bob\"); knockOut(%ann, %bob); return %bob.score;",
        )
    };

    // Short of the limit the round goes on.
    eval(&mut a, "byName(\"Bob\").score = 8;");
    assert_eq!(knock(&mut a), "9");
    assert_eq!(eval(&mut a, "return $Arena::RoundOver;"), "0");

    // With the limit off, a long lead does not end it either.
    eval(
        &mut a,
        "$Pref::Server::ScoreLimit = 0; byName(\"Bob\").score = 50;",
    );
    assert_eq!(knock(&mut a), "51");
    assert_eq!(eval(&mut a, "return $Arena::RoundOver;"), "0");

    // Bob's tenth ends it at once: the chat says who reached it, the knocked-out Ann does not respawn, and both screens show
    // the scores.
    eval(
        &mut a,
        "$Pref::Server::ScoreLimit = 10; byName(\"Bob\").score = 9;",
    );
    assert_eq!(knock(&mut a), "10");
    assert_eq!(eval(&mut a, "return $Arena::RoundOver;"), "1");
    assert_eq!(
        eval(&mut a, "return isEventPending(byName(\"Ann\").respawn);"),
        "0",
        "endRound cancelled the respawn"
    );
    let log = eval(&mut a, "return $T::Log;");
    assert!(log.contains("Bob reached the score limit of 10.|"), "{log}");
    assert!(log.contains("Bob wins the round on"), "{log}");
    assert!(
        until(&mut a, &mut b, 400, |a, b| ended(a, 1) && ended(b, 1)),
        "the round ends on both screens"
    );
}

#[test]
fn the_host_changes_the_map_and_both_clients_stay_connected() {
    let (mut a, mut b) = two_players();
    // Only the host's client was sent the maps (the pause menu's Change map entry).
    assert_eq!(count(&a, "$Client::MapCount"), 2);
    assert_eq!(count(&b, "$Client::MapCount"), 0);
    let connections = |a: &mut ArenaGame| {
        eval(
            a,
            "%l = \"\"; for (%i = 0; %i < ClientGroup.getCount(); %i++) %l = %l SPC ClientGroup.getObject(%i); return trim(%l);",
        )
    };
    let joined = connections(&mut a);
    eval(&mut a, "byName(\"Bob\").score = 4;");

    // Asked twice at once: the second is held back by the rate limit.
    a.call("changeMap", &["fixtures/aim.mis"]).unwrap();
    a.call("changeMap", &["fixtures/floor.mis"]).unwrap();
    assert!(
        until(&mut a, &mut b, 600, |a, b| on_map(a, 2)
            && on_map(b, 2)
            && both_play(a, b)),
        "both clients play on the chosen map"
    );
    assert_eq!(
        eval(&mut a, "return $Server::MissionFile;"),
        "fixtures/aim.mis"
    );
    for g in [&a, &b] {
        assert_eq!(g.global("$Arena::Map"), "aim");
    }
    assert_eq!(connections(&mut a), joined, "the same connections");
    assert_eq!(
        eval(&mut a, "return byName(\"Bob\").score;"),
        "0",
        "the scores start again"
    );
    assert_eq!(eval(&mut a, "return $Arena::RoundOver;"), "0");
    assert_eq!(
        count(&a, "$Client::MapCount"),
        2,
        "sent again for the new map"
    );
    let log = a.server_log.join("\n");
    assert!(log.contains("too soon after the last"), "{log}");
}

#[test]
fn a_remote_clients_map_change_and_a_map_off_the_list_are_refused() {
    let (mut a, mut b) = two_players();
    a.server_log.clear();
    let sequence = eval(&mut a, "return $Server::MissionSequence;");
    let quiet = |a: &mut ArenaGame, b: &mut ArenaGame| {
        for _ in 0..60 {
            frame(a, b, 16);
        }
        assert_eq!(eval(a, "return $Server::MissionSequence;"), sequence);
        assert_eq!(
            eval(a, "return $Server::MissionFile;"),
            "fixtures/floor.mis"
        );
        std::mem::take(&mut a.server_log).join("\n")
    };

    // Bob, who joined over the network, is not the host: a listed map is refused too. A guessed key does not help.
    b.call("changeMap", &["fixtures/aim.mis"]).unwrap();
    let log = quiet(&mut a, &mut b);
    assert!(log.contains("not the host"), "{log}");
    assert!(!log.contains("Map change to"), "{log}");
    let address = a.hosted_address().unwrap().to_string();
    b.call("connectTo", &[&address, "0123456789abcdef0123456789abcdef"])
        .unwrap();
    assert!(until(&mut a, &mut b, 600, both_play), "Bob joins again");
    a.server_log.clear();
    b.call("changeMap", &["fixtures/aim.mis"]).unwrap();
    let log = quiet(&mut a, &mut b);
    assert!(log.contains("not the host"), "{log}");

    // The host asks for maps that are not in the server's list, in the ways a client might.
    for name in [
        "fixtures/nothere.mis",
        "../fixtures/aim.mis",
        "missions/arena.mis",
        "",
    ] {
        a.call("changeMap", &[name]).unwrap();
        let log = quiet(&mut a, &mut b);
        assert!(log.contains("not in the map list"), "{name:?}: {log}");
    }
}
