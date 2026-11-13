//! The networked state hashed: a scripted run of a server and two clients, every tick's rope points of both clients folded into one
//! value, twice, must give the same value. A rope is cosmetic, so nothing here is a prediction the server corrects; what the check
//! pins is that a client's rope is a function of what it was sent and when (no clock, no hash order, no random number reaches it).
//!
//! The rope's crate draws with the renderer, which does not build for wasm32, so this does not join `torque-trace-hash`'s native =
//! wasm comparison (`just trace-hash`).

use crate::rig;

use rig::{PeerRopes, Rig, RigRopes, engine, files};
use torque_trace_hash::hash::TraceHasher;

/// Runs the scenario and gives the hash of every tick's points, and how many values it hashed.
fn run() -> (u64, u64) {
    let files = files();
    let mut rig = Rig::new(&files, &[]);
    rig.join(&files, engine(&files, true), 50_001, &[]);
    rig.join(&files, engine(&files, true), 50_002, &[]);
    assert!(rig.run_until(400, |r| r.peers.iter().all(|p| p.ghosts() >= 3)));
    let mut hasher = TraceHasher::new();
    let id: i32 = rig
        .eval("return ropeNetCreate(\"0 0 3\", \"6 0 3\", Cable);")
        .parse()
        .unwrap();
    let beam: i32 = rig
        .eval("return ropeNetCreate(\"0 1 3\", \"6 1 3\", Beam);")
        .parse()
        .unwrap();
    for tick in 0..300u32 {
        match tick {
            20 => {
                rig.eval(&format!("ropeNetAttach({id}, 1, Mover);"));
            }
            60 => {
                rig.eval(&format!("ropeNetSetLength({id}, 9, 2);"));
            }
            100 => {
                rig.eval(&format!("ropeNetSetCurve({beam}, \"0 0 1\", \"\", 0.5);"));
            }
            140 => {
                rig.eval(&format!("ropeNetSetLength({id}, 3, 1);"));
            }
            200 => {
                rig.eval(&format!("ropeNetSetLifetime({beam}, 2, 1);"));
            }
            _ => {}
        }
        if (20..120).contains(&tick) {
            rig.move_mover((tick - 20) as f32 * 0.03);
        }
        rig.step();
        for peer in &rig.peers {
            let ropes = peer.ropes();
            let world = ropes.lock();
            hasher.u32(world.len() as u32);
            for (id, rope) in world.ropes() {
                hasher.u32(id);
                hasher.f32(rope.fade());
                hasher.f32(rope.length());
                for p in rope.points() {
                    hasher.floats(&p);
                }
            }
        }
    }
    (hasher.finish(), hasher.count())
}

#[test]
fn a_run_of_the_server_and_two_clients_hashes_the_same_twice() {
    let (first, values) = run();
    let (second, _) = run();
    assert!(values > 10_000, "{values} values hashed");
    assert_eq!(first, second);
}
