//! Networked ropes end to end: a real server and engine clients over the memory transport.

#[path = "net/rig.rs"]
mod rig;

use rig::{Rig, create, engine, files, rope, two};
use torque_plugin_rope::{Anchor, Rope, RopeEnd};

#[test]
fn a_rope_made_on_the_server_appears_on_both_clients_with_its_datablock() {
    let (mut rig, _) = two();
    let id = create(&mut rig, "Cable");
    assert!(id > 0);
    rig.run(40);
    for peer in &rig.peers {
        assert_eq!(peer.rope_ids().len(), 1, "{:?}", peer.client.client_log);
        let (_, rope) = rope(peer);
        assert_eq!(rope.anchor(RopeEnd::Start), Anchor::Point([0.0, 0.0, 3.0]));
        assert_eq!(rope.anchor(RopeEnd::End), Anchor::Point([6.0, 0.0, 3.0]));
        // The datablock came with the mission's: the numbers the script wrote.
        assert_eq!(rope.params().segments, 8);
        assert_eq!(rope.params().thickness, 0.05);
        assert_eq!(rope.params().color, [200, 160, 90, 255]);
        assert_eq!(peer.book(|b| b.ghosted()), 1);
    }
    // The points are each client's own: the sags are alike (the same simulation from the same state) but are not on the wire.
    let a = rope(&rig.peers[0]).1;
    let b = rope(&rig.peers[1]).1;
    assert!(
        a.points()
            .zip(b.points())
            .all(|(p, q)| { (0..3).all(|k| (p[k] - q[k]).abs() < 0.05) })
    );
}

#[test]
fn a_look_change_reaches_both_clients_and_touches_nothing_else() {
    // What goes on the wire for one group is in the record's own tests (`net::records`); here the client's side of it.
    let (mut rig, _) = two();
    let id = create(&mut rig, "Cable");
    rig.eval(&format!(
        "ropeNetSetLength({id}, 5.5, 0); ropeNetSetCurve({id}, \"\", \"\", 0.5);"
    ));
    rig.run(40);
    let before: Vec<Rope> = rig.peers.iter().map(|p| rope(p).1).collect();
    assert!(before.iter().all(|r| r.curve().is_some()));
    rig.eval(&format!("ropeNetSetColor({id}, \"255 0 0 255\");"));
    rig.run(6);
    for (peer, before) in rig.peers.iter().zip(&before) {
        let (_, rope) = rope(peer);
        assert_eq!(rope.look().color, Some([255, 0, 0, 255]));
        assert_eq!(rope.anchor(RopeEnd::Start), before.anchor(RopeEnd::Start));
        assert_eq!(rope.anchor(RopeEnd::End), before.anchor(RopeEnd::End));
        assert_eq!(rope.curve(), before.curve());
        assert!((rope.length() - 5.5).abs() < 1e-3);
    }
    // Clearing the look puts the datablock's back.
    rig.eval(&format!("ropeNetClearLook({id});"));
    rig.run(6);
    assert!(rig.peers.iter().all(|p| rope(p).1.look().color.is_none()));
}

#[test]
fn a_texture_the_server_names_is_loaded_by_each_client_and_a_missing_one_draws_plain() {
    let (mut rig, _) = two();
    let id = create(&mut rig, "Cable");
    rig.run(10);
    let texture = |peer: &rig::Peer| {
        let ropes = peer.ropes();
        let world = ropes.lock();
        let (_, rope) = world.ropes().next().expect("a rope");
        rope.appearance(&world.textures()).texture
    };
    assert!(rig.peers.iter().all(|p| texture(p) == 0));
    rig.eval(&format!("ropeNetSetTexture({id}, \"art/stripe.png\");"));
    rig.run(10);
    assert!(rig.peers.iter().all(|p| texture(p) != 0));
    // A file nobody has: the rope is drawn plain and the client says so once.
    rig.eval(&format!("ropeNetSetTexture({id}, \"art/none.png\");"));
    rig.run(10);
    assert!(rig.peers.iter().all(|p| texture(p) == 0));
    let warned = rig.peers[0]
        .client
        .client_log
        .iter()
        .filter(|l| l.contains("art/none.png"))
        .count();
    assert_eq!(warned, 1, "{:?}", rig.peers[0].client.client_log);
}

#[test]
fn deleting_a_rope_removes_it_from_both_clients() {
    let (mut rig, _) = two();
    let id = create(&mut rig, "Cable");
    rig.run(30);
    assert!(rig.peers.iter().all(|p| p.rope_ids().len() == 1));
    assert_eq!(rig.eval(&format!("return ropeNetDelete({id});")), "1");
    rig.run(30);
    for peer in &rig.peers {
        assert!(peer.rope_ids().is_empty());
        assert_eq!(peer.book(|b| b.ghosted()), 0);
    }
    assert_eq!(rig.objects("Rope"), 0);
}

#[test]
fn a_rope_hung_from_a_moving_ghost_follows_it_on_every_client() {
    let (mut rig, _) = two();
    let id = create(&mut rig, "Cable");
    rig.eval(&format!("ropeNetAttach({id}, 1, Mover);"));
    rig.run(10);
    for k in 0..80 {
        rig.eval(&format!("moveMover({});", k as f32 * 0.04));
        rig.step();
    }
    rig.run(12);
    eprintln!(
        "cameras {:?} anchor {:?}",
        rig.peers[0].cameras(),
        rope(&rig.peers[0]).1.anchor(RopeEnd::End)
    );
    rig.peers[0].client.with_world(|w| {
        for (i, g) in &w.records.ghosts {
            eprintln!("ghost {i} {:?} {:?} {:?}", g.class, g.position, g.pose());
        }
    });
    let x = 79.0 * 0.04;
    for peer in &rig.peers {
        let (_, rope) = rope(peer);
        assert!(matches!(rope.anchor(RopeEnd::End), Anchor::Ghost { .. }));
        let end = rope.points().last().unwrap();
        assert!(
            (end[0] - x).abs() < 0.3 && (end[1]).abs() < 0.3 && (end[2] - 1.0).abs() < 0.3,
            "the end is at {end:?}, the mover at {x}"
        );
    }
}

#[test]
fn a_client_without_the_rope_plugin_is_refused_at_the_manifest() {
    let files = files();
    let mut rig = Rig::new(&files);
    rig.join(&files, engine(&files, false), 50_001);
    rig.run(100);
    let peer = &rig.peers[0];
    assert_eq!(peer.ghosts(), 0);
    let why = peer.client.global("$Why");
    assert!(
        why.contains("torque.rope") && why.contains("you do not have it"),
        "{why:?}"
    );
}
