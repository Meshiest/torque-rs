//! Networked ropes that change over time or come as events: reels and taut ropes, lifetimes and fades, one-shot ropes, and predicted
//! ropes, each with a real server and two engine clients over the memory transport.

#[path = "net/rig.rs"]
mod rig;

use rig::{Peer, PeerRopes, Rig, create, off_the_line, rope, two};

#[test]
fn a_reel_crosses_the_network_as_a_target_and_a_time_and_each_client_runs_it() {
    let (mut rig, _) = two();
    let id = create(&mut rig, "Cable");
    rig.run(30);
    let from = rope(&rig.peers[0]).1.length();
    assert!((from - 6.6).abs() < 1e-3, "{from}");
    rig.eval(&format!("ropeNetSetLength({id}, 9, 2);"));
    // Part way: between the two, and the same on both clients.
    rig.run(32);
    let part: Vec<f32> = rig.peers.iter().map(|p| rope(p).1.length()).collect();
    assert!(part.iter().all(|l| *l > from + 0.5 && *l < 8.8), "{part:?}");
    assert!((part[0] - part[1]).abs() < 0.05, "{part:?}");
    assert!(rig.peers.iter().all(|p| rope(p).1.reeling()));
    rig.run(40);
    for peer in &rig.peers {
        let (_, rope) = rope(peer);
        assert_eq!(rope.length(), 9.0);
        assert!(!rope.reeling());
    }
}

#[test]
fn a_rope_shortened_below_the_distance_between_its_ends_is_drawn_taut_on_both_clients() {
    let (mut rig, _) = two();
    let id = create(&mut rig, "Cable");
    rig.run(30);
    assert!(rig.peers.iter().all(|p| !rope(p).1.taut()));
    rig.eval(&format!("ropeNetSetLength({id}, 3, 0.5);"));
    rig.run(40);
    for peer in &rig.peers {
        let (_, rope) = rope(peer);
        assert!(rope.taut());
        assert!(off_the_line(&rope) < 1e-3, "{}", off_the_line(&rope));
        assert_eq!(rope.length(), 3.0);
    }
}

#[test]
fn a_temporary_rope_fades_and_is_deleted_on_the_server_and_both_clients() {
    let (mut rig, _) = two();
    let id = create(&mut rig, "Beam");
    rig.eval(&format!("ropeNetSetLifetime({id}, 2, 1);"));
    rig.run(20);
    for peer in &rig.peers {
        let (_, rope) = rope(peer);
        let left = rope.life_left().expect("a lifetime");
        assert!(left > 1.0 && left <= 2.0, "{left}");
        assert_eq!(rope.fade(), 1.0);
    }
    // Into the last second: fading on both.
    rig.run(26);
    for peer in &rig.peers {
        let fade = rope(peer).1.fade();
        assert!(fade > 0.0 && fade < 0.8, "{fade}");
    }
    rig.run(30);
    assert_eq!(rig.objects_of("Rope"), 0);
    for peer in &rig.peers {
        assert!(peer.rope_ids().is_empty());
    }
    // A rope given a lifetime and then none lives on (the call the first one scheduled finds a rope with no end).
    let id = create(&mut rig, "Cable");
    rig.eval(&format!(
        "ropeNetSetLifetime({id}, 1, 0); ropeNetSetLifetime({id}, 0, 0);"
    ));
    rig.run(60);
    assert_eq!(rig.objects_of("Rope"), 1);
    assert!(rig.peers.iter().all(|p| p.rope_ids().len() == 1));
}

#[test]
fn a_one_shot_rope_is_an_event_with_no_ghost_and_each_client_shows_it_for_its_time() {
    let (mut rig, _) = two();
    let ghosts = rig.peers[0].ghosts();
    let sent = rig.eval("return ropeNetFx(\"\", \"0 0 1\", \"6 0 1\", Beam, 1, 0.3);");
    assert_eq!(sent, "2");
    rig.run(8);
    for peer in &rig.peers {
        assert_eq!(peer.rope_ids().len(), 1, "{:?}", peer.client.client_log);
        let (_, rope) = rope(peer);
        assert!(rope.curve().is_some_and(|c| c.bend == 0.3));
        assert!(rope.life_left().is_some_and(|t| t > 0.5));
        assert_eq!(peer.book(|b| (b.fx_count(), b.ghosted())), (1, 0));
        assert_eq!(peer.ghosts(), ghosts, "no ghost");
    }
    assert_eq!(rig.objects_of("Rope"), 0, "nothing on the server");
    rig.run(40);
    assert!(rig.peers.iter().all(|p| p.rope_ids().is_empty()));
    // One client alone: the connection's object names it.
    let client = rig.eval("return ClientGroup.getObject(0);");
    let sent = rig.eval(&format!(
        "return ropeNetFx({client}, \"0 0 1\", \"6 0 1\", Cable, 1);"
    ));
    assert_eq!(sent, "1");
    rig.run(8);
    let shown: usize = rig.peers.iter().map(|p| p.rope_ids().len()).sum();
    assert_eq!(shown, 1);
}

#[test]
fn a_one_shot_rope_from_a_hostile_server_is_checked_and_bounded() {
    let (mut rig, _) = two();
    let call = |rig: &mut Rig, args: [&str; 5]| {
        rig.peers[0]
            .client
            .call("clientCmdRopeFx", &args)
            .expect("the call runs")
    };
    let block = rig.eval("return Beam.getId();");
    for (what, args) in [
        ("a point of two words", ["0 0", "1 1 1", &block, "1", "0"]),
        (
            "a point of four words",
            ["0 0 0 0", "1 1 1", &block, "1", "0"],
        ),
        (
            "a point past the world",
            ["1e30 0 0", "1 1 1", &block, "1", "0"],
        ),
        ("no datablock", ["0 0 0", "1 1 1", "NoSuchBlock", "1", "0"]),
        (
            "a datablock of another class",
            ["0 0 0", "1 1 1", "EyeData", "1", "0"],
        ),
        (
            "an object that is no datablock",
            ["0 0 0", "1 1 1", "ServerConnection", "1", "0"],
        ),
        ("no time", ["0 0 0", "1 1 1", &block, "0", "0"]),
        ("a negative time", ["0 0 0", "1 1 1", &block, "-3", "0"]),
        ("an empty point", ["", "1 1 1", &block, "1", "0"]),
    ] {
        assert_eq!(call(&mut rig, args), "0", "{what}");
        assert!(rig.peers[0].rope_ids().is_empty(), "{what}");
    }
    // A time past the most is held to it, a bend past the range to the range.
    assert_eq!(call(&mut rig, ["0 0 0", "1 1 1", &block, "1e9", "99"]), "1");
    let (_, long) = rope(&rig.peers[0]);
    assert!(long.life_left().unwrap() <= 30.0);
    assert_eq!(long.curve().unwrap().bend, 4.0);
    // At most 32 at a time.
    for _ in 0..60 {
        call(&mut rig, ["0 0 0", "1 1 1", &block, "5", "0"]);
    }
    assert_eq!(rig.peers[0].rope_ids().len(), 32);
    assert_eq!(rig.peers[0].book(|b| b.fx_count()), 32);
    // The other client is not touched, and neither is the connection.
    assert!(rig.peers[1].rope_ids().is_empty());
    rig.run(10);
    assert!(rig.peers[0].ghosts() >= 3);
}

#[test]
fn the_server_refuses_to_send_what_it_could_not_make() {
    let (mut rig, _) = two();
    for bad in [
        "ropeNetFx(\"\", \"0 0\", \"6 0 1\", Beam, 1)",
        "ropeNetFx(\"\", \"0 0 1\", \"6 0 1\", NoSuchBlock, 1)",
        "ropeNetFx(\"\", \"0 0 1\", \"6 0 1\", EyeData, 1)",
        "ropeNetFx(\"\", \"0 0 1\", \"6 0 1\", Beam, -1)",
        "ropeNetCreate(\"0 0\", \"6 0 1\", Beam)",
        "ropeNetCreate(\"0 0 1\", \"6 0 1\", EyeData)",
        "ropeNetSetColor(Mover, \"1 2 3\")",
    ] {
        assert_eq!(rig.eval(&format!("return {bad};")), "", "{bad}");
    }
    let id = create(&mut rig, "Cable");
    for bad in [
        format!("ropeNetSetColor({id}, \"red\")"),
        format!("ropeNetSetLength({id}, -2, 1)"),
        format!("ropeNetPin({id}, 2, \"1 1 1\")"),
        format!("ropeNetPin({id}, 0, \"1 1\")"),
        format!("ropeNetAttach({id}, 0, NoSuchObject)"),
        format!("ropeNetSetCurve({id}, \"sideways\", \"\", 0)"),
    ] {
        assert_eq!(rig.eval(&format!("return {bad};")), "", "{bad}");
    }
    // Past the cap on networked ropes the call says no and makes none.
    for _ in 0..300 {
        rig.eval("ropeNetCreate(\"0 0 3\", \"6 0 3\", Cable);");
    }
    assert_eq!(rig.objects_of("Rope"), 256);
}

/// A client makes its own rope of the datablock `block` (an id: a client has no names for datablocks) at once and tags it: its
/// local id and the predict id.
fn predict(peer: &mut Peer, block: &str) -> (u32, u32) {
    let local = peer
        .client
        .call("ropeCreate", &["0 0 2", "4 0 2", block])
        .unwrap();
    let id = peer.client.call("ropePredict", &[&local]).unwrap();
    (local.parse().unwrap(), id.parse().unwrap())
}

#[test]
fn a_predicted_rope_is_adopted_by_the_servers_ghost_and_there_is_exactly_one() {
    let (mut rig, _) = two();
    let block = rig.eval("return Beam.getId();");
    let (local, id) = predict(&mut rig.peers[0], &block);
    assert!(id >= 1 && local > 0);
    assert_eq!(rig.peers[0].rope_ids(), [local], "made at once");
    rig.peers[0]
        .client
        .call("eval", &[&format!("commandToServer('FireBeam', {id});")])
        .unwrap();
    rig.run(30);
    assert_eq!(rig.objects_of("Rope"), 1);
    // The firing client keeps the rope it made; the other client has the server's own.
    assert_eq!(rig.peers[0].rope_ids(), [local]);
    assert_eq!(rig.peers[0].book(|b| (b.ghosted(), b.waiting())), (1, 0));
    assert_eq!(rig.peers[1].rope_ids().len(), 1);
    assert_eq!(rig.peers[1].book(|b| (b.ghosted(), b.waiting())), (1, 0));
    // Not faded out: the adopted rope lasts as long as the ghost does.
    rig.run(100);
    assert_eq!(rig.peers[0].rope_ids(), [local]);
    assert_eq!(rope(&rig.peers[0]).1.fade(), 1.0);
    // The server deletes it: both clients lose theirs.
    rig.eval("ropeNetDelete($Fired);");
    rig.run(20);
    assert!(rig.peers.iter().all(|p| p.rope_ids().is_empty()));
}

#[test]
fn a_predicted_rope_the_server_never_confirms_fades_out_and_goes() {
    let (mut rig, _) = two();
    let block = rig.eval("return Beam.getId();");
    let (local, _) = predict(&mut rig.peers[1], &block);
    rig.run(40);
    assert_eq!(rig.peers[1].rope_ids(), [local]);
    assert_eq!(rope(&rig.peers[1]).1.fade(), 1.0, "still waiting");
    // Two seconds on it starts to fade; a quarter second later it is gone.
    rig.run(24);
    let now = rope(&rig.peers[1]).1;
    assert!(now.life_left().is_some(), "fading");
    assert!(now.fade() < 1.0);
    assert_eq!(rig.peers[1].book(|b| b.waiting()), 0);
    rig.run(12);
    assert!(rig.peers[1].rope_ids().is_empty());
    assert!(
        rig.peers[0].rope_ids().is_empty(),
        "no other client had one"
    );
}

#[test]
fn the_ids_a_client_picks_are_bounded_and_cannot_touch_another_clients_ropes() {
    let (mut rig, _) = two();
    let block = rig.eval("return Beam.getId();");
    // Both clients pick the first id; the server makes a rope for the first alone.
    let (local_a, id_a) = predict(&mut rig.peers[0], &block);
    let (local_b, id_b) = predict(&mut rig.peers[1], &block);
    assert_eq!(id_a, id_b);
    rig.peers[0]
        .client
        .call("eval", &[&format!("commandToServer('FireBeam', {id_a});")])
        .unwrap();
    rig.run(30);
    // The first adopted its own. The second did not take the first's ghost for its own: it holds its prediction (until it fades)
    // and the server's rope, so it has two.
    assert_eq!(rig.peers[0].rope_ids(), [local_a]);
    assert_eq!(rig.peers[1].rope_ids().len(), 2);
    assert!(rig.peers[1].rope_ids().contains(&local_b));
    assert_eq!(rig.peers[1].book(|b| b.waiting()), 1);
    // An id nobody holds is nothing: the server's rope is made as any other would be.
    rig.peers[0]
        .client
        .call("eval", &["commandToServer('FireBeam', 200);"])
        .unwrap();
    rig.run(20);
    assert_eq!(rig.peers[0].rope_ids().len(), 2);
    // Sixteen wait at most, each with an id of its own; a rope that is not there takes none.
    let peer = &mut rig.peers[0];
    let mut ids = Vec::new();
    for _ in 0..20 {
        ids.push(predict(peer, &block).1);
    }
    let held: Vec<u32> = ids.iter().copied().filter(|id| *id != 0).collect();
    let distinct: std::collections::BTreeSet<_> = held.iter().collect();
    assert_eq!((held.len(), distinct.len()), (16, 16));
    assert!(ids.iter().all(|id| *id <= 255));
    assert_eq!(peer.client.call("ropePredict", &["999999"]).unwrap(), "0");
}

#[test]
fn a_disconnect_takes_the_networked_ropes_away_and_a_reconnect_does_not_leak() {
    let (mut rig, _) = two();
    create(&mut rig, "Cable");
    rig.run(30);
    assert!(rig.peers.iter().all(|p| p.rope_ids().len() == 1));
    // A predicted rope the client made by itself is not the server's and stays until its own time is up.
    rig.peers[0].client.call("disconnect", &[]).unwrap();
    rig.run(10);
    assert!(
        rig.peers[0].rope_ids().is_empty(),
        "the ropes of a server it left"
    );
    assert_eq!(rig.peers[0].book(|b| b.ghosted()), 0);
    assert_eq!(
        rig.peers[1].rope_ids().len(),
        1,
        "the other client is still there"
    );
    // It comes back: one rope again, and the one it had before is not still in its world.
    let address = rig::SERVER.to_string();
    rig.peers[0].client.call("connectTo", &[&address]).unwrap();
    assert!(rig.run_until(400, |r| r.peers[0].rope_ids().len() == 1));
    rig.run(40);
    assert_eq!(rig.peers[0].rope_ids().len(), 1);
    assert_eq!(rig.peers[0].book(|b| b.ghosted()), 1);
    // Twice more, with the rope changing in between.
    for _ in 0..2 {
        rig.peers[0].client.call("disconnect", &[]).unwrap();
        rig.run(10);
        assert!(rig.peers[0].rope_ids().is_empty());
        rig.peers[0].client.call("connectTo", &[&address]).unwrap();
        assert!(rig.run_until(400, |r| r.peers[0].rope_ids().len() == 1));
        rig.run(20);
        assert_eq!(rig.peers[0].rope_ids().len(), 1);
    }
}

#[test]
fn a_loop_of_one_shot_ropes_is_sent_each_client_at_most_32_a_second() {
    let (mut rig, _) = two();
    rig.server.drain_log();
    let mut sent = 0;
    for _ in 0..100 {
        let n = rig.eval("return ropeNetFx(\"\", \"0 0 1\", \"6 0 1\", Beam, 1);");
        sent += n.parse::<i32>().unwrap();
    }
    // Two clients, 32 each; the rest were dropped, with one line for each client.
    assert_eq!(sent, 64);
    let log = rig.server.drain_log();
    let warnings = log
        .iter()
        .filter(|l| l.contains("ropeNetFx: more than 32"))
        .count();
    assert_eq!(warnings, 2, "{log:?}");
    rig.run(8);
    for peer in &rig.peers {
        assert_eq!(peer.rope_ids().len(), 32);
    }
    // A second later the budget is whole again.
    rig.run(40);
    let n = rig.eval("return ropeNetFx(\"\", \"0 0 1\", \"6 0 1\", Beam, 1);");
    assert_eq!(n, "2");
}

#[test]
fn a_new_lifetime_takes_the_place_of_the_one_before() {
    let (mut rig, _) = two();
    let id = create(&mut rig, "Cable");
    rig.eval(&format!("ropeNetSetLifetime({id}, 1, 0);"));
    let first = rig.eval(&format!("return {id}.lifeEvent;"));
    assert!(first.parse::<i32>().unwrap() > 0, "{first:?}");
    assert_eq!(rig.eval(&format!("return isEventPending({first});")), "1");
    // Each call leaves one scheduled call, and the one before is gone.
    for _ in 0..5 {
        rig.eval(&format!("ropeNetSetLifetime({id}, 10, 1);"));
    }
    assert_eq!(rig.eval(&format!("return isEventPending({first});")), "0");
    let last = rig.eval(&format!("return {id}.lifeEvent;"));
    assert_eq!(rig.eval(&format!("return isEventPending({last});")), "1");
    rig.run(60);
    assert_eq!(
        rig.objects_of("Rope"),
        1,
        "the first call's end passed and the rope is still here"
    );
    // No lifetime takes the call back.
    rig.eval(&format!("ropeNetSetLifetime({id}, 0, 0);"));
    assert_eq!(rig.eval(&format!("return isEventPending({last});")), "0");
    assert_eq!(rig.eval(&format!("return {id}.lifeEvent;")), "0");
}
