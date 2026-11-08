//! Networked ropes drawn offscreen by the clients that were sent them: the same rope seen by two clients, a taut rope, and a
//! temporary rope fading out. `TORQUE_ROPE_PICTURES=<folder>` also writes the pictures there as PNGs.

#[path = "pictures/common.rs"]
mod common;
#[path = "net/rig.rs"]
mod rig;

use common::{SIZE, WHOLE, changed, gpu, save, shot_from, view};
use rig::{Rig, engine, files, rope};
use torque_render_core::Frame;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::source::Eye;

fn front() -> Eye {
    look_at([0.0, -10.5, 3.4], [0.0, 0.0, 1.6])
}

fn side() -> Eye {
    look_at([9.0, -6.5, 3.0], [0.0, 0.0, 1.8])
}

/// A server and two clients, whose engines draw.
fn rig() -> Rig {
    let files = files();
    let mut rig = Rig::new(&files);
    rig.join(&files, engine(&files, true), 50_001);
    rig.join(&files, engine(&files, true), 50_002);
    assert!(rig.run_until(400, |r| r.peers.iter().all(|p| p.ghosts() >= 3)));
    rig
}

/// A rope between the posts of `view`, 8 apart, as the server makes it.
fn make(rig: &mut Rig, block: &str) -> i32 {
    let id = rig.eval(&format!(
        "return ropeNetCreate(\"-4 0 3\", \"4 0 3\", {block});"
    ));
    id.parse().expect("a rope id")
}

#[test]
fn the_same_networked_rope_is_drawn_by_two_clients() {
    let Some(gpu) = gpu() else { return };
    let mut rig = rig();
    let mut views: Vec<_> = rig.peers.iter().map(|p| view(&gpu, &p.engine)).collect();
    let bare: Vec<_> = views
        .iter_mut()
        .map(|v| shot_from(v, &gpu, front))
        .collect();
    make(&mut rig, "Thick");
    rig.run(160);
    let seen: Vec<_> = views
        .iter_mut()
        .map(|v| shot_from(v, &gpu, front))
        .collect();
    let aside: Vec<_> = views.iter_mut().map(|v| shot_from(v, &gpu, side)).collect();
    save(&seen[0], "client-a-front");
    save(&seen[1], "client-b-front");
    save(&aside[0], "client-a-side");
    save(&aside[1], "client-b-side");
    for (k, (drawn, empty)) in seen.iter().zip(&bare).enumerate() {
        assert!(
            changed(empty, drawn, WHOLE, 40) > 400,
            "client {k} draws the rope"
        );
    }
    // Each client simulated the rope itself from the same state: the two pictures agree to within a few pixels of sag.
    let apart = changed(&seen[0], &seen[1], WHOLE, 40);
    let rope_pixels = changed(&bare[0], &seen[0], WHOLE, 40);
    assert!(
        apart * 10 < rope_pixels,
        "{apart} pixels differ between the clients, the rope is {rope_pixels}"
    );
    let _ = (SIZE, rope(&rig.peers[0]));
}

#[test]
fn a_rope_shortened_past_its_ends_is_drawn_taut_and_a_longer_one_sags() {
    let Some(gpu) = gpu() else { return };
    let mut rig = rig();
    let mut v = view(&gpu, &rig.peers[0].engine);
    let id = make(&mut rig, "Thick");
    rig.eval(&format!("ropeNetSetLength({id}, 9, 0);"));
    rig.run(200);
    let slack = shot_from(&mut v, &gpu, front);
    save(&slack, "slack");
    rig.eval(&format!("ropeNetSetLength({id}, 6, 1);"));
    rig.run(80);
    let taut = shot_from(&mut v, &gpu, front);
    save(&taut, "taut");
    assert!(rope(&rig.peers[0]).1.taut());
    // The sag is gone: the rope's pixels below the line between the posts differ.
    let (_, y) = common::project([0.0, 0.0, 2.0]);
    let below = [0, y, SIZE.0, SIZE.1 / 2 - 20];
    assert!(changed(&slack, &taut, below, 40) > 300);
}

#[test]
fn a_temporary_rope_fades_out_in_the_pictures() {
    let Some(gpu) = gpu() else { return };
    let mut rig = rig();
    let mut v = view(&gpu, &rig.peers[1].engine);
    let tube = make(&mut rig, "Thick");
    let glow = rig.eval("return ropeNetCreate(\"-4 0 1.7\", \"4 0 1.7\", Glow);");
    let bare = shot_from(&mut v, &gpu, front);
    rig.eval(&format!(
        "ropeNetSetLifetime({tube}, 5, 3); ropeNetSetLifetime({glow}, 5, 3);"
    ));
    // A second in: whole. The fade is the last 3 of 5 seconds: pictures at 2.6, 3.6 and 4.7 s, then it is gone.
    rig.run(32);
    let whole = shot_from(&mut v, &gpu, front);
    save(&whole, "fade-0");
    rig.run(50);
    let early = shot_from(&mut v, &gpu, front);
    save(&early, "fade-1");
    rig.run(32);
    let late = shot_from(&mut v, &gpu, front);
    save(&late, "fade-2");
    rig.run(34);
    let last = shot_from(&mut v, &gpu, front);
    save(&last, "fade-3");
    rig.run(40);
    let gone = shot_from(&mut v, &gpu, front);
    save(&gone, "fade-4");
    let drawn = |a: &Frame| changed(&bare, a, WHOLE, 24);
    let counts = [
        drawn(&whole),
        drawn(&early),
        drawn(&late),
        drawn(&last),
        drawn(&gone),
    ];
    assert!(counts[0] > 1000, "{counts:?}");
    assert!(
        counts.windows(2).all(|w| w[0] > w[1]),
        "less and less of it: {counts:?}"
    );
    assert!(counts[4] < 200, "{counts:?}");
}
