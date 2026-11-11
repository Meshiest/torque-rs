//! Posed hitboxes: a Player whose datablock says `hitboxes = true` is hit by its parts at its pose. A ray past the arm misses,
//! one through the arm hits it, and the action, arm (look) and script threads move the parts. Without the field, or with a
//! shape that has no LOS meshes, the upright box answers as before.

use crate::figure;

use figure::{across, eval, game_server, ray, spawn};
use torque_server::server::TICK_MS;

#[test]
fn a_ray_past_the_arm_misses_and_one_through_it_hits_the_arm() {
    let mut s = game_server();
    let t = spawn(&mut s, "Target", "Posed", "0 0 0");
    assert_eq!(
        across(&mut s, 0.7, 1.3).0,
        0,
        "past the arm, inside the box"
    );
    assert_eq!(
        across(&mut s, 0.35, 1.3).0,
        0,
        "between the torso and the arm"
    );
    let (hit, point) = across(&mut s, 0.5, 1.3);
    assert_eq!(hit, t, "through the arm");
    assert_eq!(
        eval(
            &mut s,
            &format!("return Target.getDamageLocation(\"{point}\");")
        ),
        "rarm"
    );
    let (hit, point) = across(&mut s, 0.0, 2.0);
    assert_eq!(hit, t);
    assert_eq!(
        eval(
            &mut s,
            &format!("return Target.getDamageLocation(\"{point}\");")
        ),
        "head"
    );
    assert_eq!(
        eval(&mut s, "return Target.getDamageLocation(\"0 0 0.5\");"),
        "torso",
        "a point no ray hit: the part nearest it"
    );
}

#[test]
fn without_the_field_or_without_los_meshes_the_box_answers() {
    let mut s = game_server();
    let boxed = spawn(&mut s, "Boxed1", "Boxed", "0 0 0");
    assert_eq!(across(&mut s, 0.7, 1.3).0, boxed, "TGE's box");
    assert_eq!(
        eval(&mut s, "return Boxed1.getDamageLocation(\"0.7 -1.5 1.3\");"),
        ""
    );
    eval(&mut s, "Boxed1.delete();");
    let plain = spawn(&mut s, "Plain", "NoParts", "0 0 0");
    assert_eq!(across(&mut s, 1.2, 1.3).0, plain, "no LOS meshes: the box");
}

#[test]
fn crouching_looking_and_leaning_move_the_parts() {
    let mut s = game_server();
    let t = spawn(&mut s, "Target", "Posed", "0 0 0");
    // Crouched (the action thread): the head drops from 1.8..2.2 to 1.3..1.7.
    assert_eq!(across(&mut s, 0.0, 2.0).0, t);
    eval(&mut s, "Target.setActionThread(\"crouch\", true);");
    // The crouch blends in over a quarter second, as a client draws it.
    assert_eq!(across(&mut s, 0.0, 2.0).0, t, "not yet");
    for _ in 0..8 {
        // Held where it stands (no floor).
        eval(
            &mut s,
            "Target.setTransform(\"0 0 0 0 0 1 0\"); Target.setVelocity(\"0 0 0\");",
        );
        s.run(TICK_MS, |_| false);
    }
    assert_eq!(across(&mut s, 0.0, 2.0).0, 0, "the head is lower");
    let (hit, point) = across(&mut s, 0.0, 1.5);
    assert_eq!(hit, t);
    assert_eq!(
        eval(
            &mut s,
            &format!("return Target.getDamageLocation(\"{point}\");")
        ),
        "head"
    );
    eval(&mut s, "Target.delete();");

    // Looking down all the way (the arm thread at the end of `look`): the arm swings forward to the shoulder's height.
    let t = spawn(&mut s, "Looker", "Posed", "0 0 0");
    assert_eq!(across(&mut s, 0.5, 1.2).0, t, "the arm hangs");
    assert_eq!(
        ray(&mut s, "0.5 0.4 4", "0.5 0.4 -1").0,
        0,
        "nothing in front of the shoulder"
    );
    torque_game::sim::player::state_mut(&mut s.vm, t)
        .unwrap()
        .head[0] = 1.5;
    for _ in 0..2 {
        s.run(TICK_MS, |_| false);
    }
    assert_eq!(across(&mut s, 0.5, 1.2).0, 0, "the arm is up");
    let (hit, _) = ray(&mut s, "0.5 0.4 4", "0.5 0.4 -1");
    assert_eq!(hit, t, "the arm reaches forward");
    eval(&mut s, "Looker.delete();");

    // Leaning (a script thread): the head moves off the upright line toward +x.
    let t = spawn(&mut s, "Leaner", "Posed", "0 0 0");
    eval(&mut s, "Leaner.playThread(0, \"lean\");");
    assert_eq!(across(&mut s, 0.0, 2.0).0, 0, "no head where it stood");
    let (hit, point) = across(&mut s, 1.0, 1.73);
    assert_eq!(hit, t);
    assert_eq!(
        eval(
            &mut s,
            &format!("return Leaner.getDamageLocation(\"{point}\");")
        ),
        "head"
    );
}
