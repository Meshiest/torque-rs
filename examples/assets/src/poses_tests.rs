//! The crouched and prone sequences and the hit reactions: feet and hands that stand stay put, the poses fit their boxes,
//! a change of pose keeps the feet where they stand, and the reactions rise sharply and come back to rest.

use torque_shapes::instance::{ShapeInstance, find_sequence};
use torque_shapes::math::mul_p;
use torque_shapes::preload;
use torque_shapes::shape::Shape;

use crate::poses::{CRAWL_LOOP, CRAWL_SPEED, CROUCH_SIDE_SPEED, SIDE_LOOP};
use crate::shapes::{self, FOOT_HEEL, FOOT_TOE, GRIP, SOLE};

fn free(_: u64) -> bool {
    true
}

fn figure() -> Shape {
    Shape::for_server(shapes::figure())
}

fn node(shape: &Shape, name: &str) -> usize {
    let n = shape.find_name(name.as_bytes()).unwrap();
    shape.nodes.iter().position(|x| x.name == n).unwrap()
}

/// An instance with `threads` (sequence and position) playing, in order, animated.
fn posed(shape: &Shape, threads: &[(&str, f32)]) -> ShapeInstance {
    let mut inst = ShapeInstance::new(shape, &mut free).unwrap();
    for (sequence, pos) in threads {
        let t = inst.create_thread(shape).unwrap();
        let seq = find_sequence(shape, sequence.as_bytes()).unwrap();
        inst.set_sequence(shape, t, seq as i32, *pos);
    }
    inst.animate(shape, 0, None, &mut free).unwrap();
    inst
}

fn at(shape: &Shape, inst: &ShapeInstance, name: &str, local: [f32; 3]) -> [f32; 3] {
    mul_p(&inst.node_transforms()[node(shape, name)], local)
}

/// The contacts a pose stands on, each a node and a point on it: the feet's heel and toe edges, the hands' grips.
fn contacts() -> Vec<(&'static str, [f32; 3])> {
    let mut out = Vec::new();
    for ankle in ["ankle_l", "ankle_r"] {
        out.push((ankle, [0.0, FOOT_HEEL, -SOLE]));
        out.push((ankle, [0.0, FOOT_TOE, -SOLE]));
    }
    for elbow in ["elbow_l", "elbow_r"] {
        out.push((elbow, [0.0, 0.0, -GRIP]));
    }
    out
}

/// Over one loop of `name` (moving `dir` at `speed`), each contact on the ground for a step at both ends (within `down` of
/// its lowest) moves with the ground at most `slip` of the ground's speed; and how far each one lifts.
fn slide(
    shape: &Shape,
    name: &str,
    dir: [f32; 3],
    speed: f32,
    duration: f32,
    down: f32,
) -> (f32, f32) {
    let n = 240;
    let pts: Vec<Vec<[f32; 3]>> = (0..=n)
        .map(|k| {
            let p = k as f32 / n as f32;
            let inst = posed(shape, &[(name, p)]);
            contacts()
                .iter()
                .map(|(nd, l)| {
                    let q = at(shape, &inst, nd, *l);
                    let travel = speed * duration * p;
                    [q[0] + dir[0] * travel, q[1] + dir[1] * travel, q[2]]
                })
                .collect()
        })
        .collect();
    let (mut worst, mut lifted) = (0.0f32, 0.0f32);
    for c in 0..contacts().len() {
        let low = pts.iter().map(|p| p[c][2]).fold(f32::MAX, f32::min);
        let high = pts.iter().map(|p| p[c][2]).fold(f32::MIN, f32::max);
        // Only contacts that touch down count (a held-up hand never stands).
        if low > 0.12 {
            continue;
        }
        lifted = lifted.max(high - low);
        // A step inside a stance (the contact down before and after it too), so the instant of landing or leaving is not
        // counted.
        let on = |k: usize| pts[k % (n + 1)][c][2] < low + down;
        for k in 1..n - 1 {
            let (a, b) = (pts[k][c], pts[k + 1][c]);
            if on(k - 1) && on(k) && on(k + 1) && on(k + 2) {
                let moved = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
                let r = moved * n as f32 / duration / speed;
                if r > worst && std::env::var("POSE_DEBUG").is_ok() {
                    eprintln!("{name} contact {c} at {k}: {r} z {} low {low}", a[2]);
                }
                worst = worst.max(r);
            }
        }
    }
    (worst, lifted)
}

#[test]
fn the_crouched_walk_and_the_crawl_keep_what_stands_on_the_ground_still() {
    let shape = figure();
    let g = crate::gait::CROUCH;
    for (name, dir, speed, duration) in [
        ("crouch_forward", [0.0, 1.0, 0.0], g.speed, g.duration),
        ("crouch_backward", [0.0, -1.0, 0.0], g.speed, g.duration),
        (
            "crouch_side",
            [-1.0, 0.0, 0.0],
            CROUCH_SIDE_SPEED,
            SIDE_LOOP,
        ),
        ("prone_forward", [0.0, 1.0, 0.0], CRAWL_SPEED, CRAWL_LOOP),
        ("prone_backward", [0.0, -1.0, 0.0], CRAWL_SPEED, CRAWL_LOOP),
    ] {
        let (worst, lifted) = slide(&shape, name, dir, speed, duration, 0.001);
        assert!(
            worst < 0.06,
            "{name}: a contact on the ground slides at {worst} of the ground's speed"
        );
        assert!(lifted > 0.04, "{name}: a step lifts {lifted}");
    }
}

#[test]
fn the_crouched_and_prone_sequences_carry_their_ground_speed_and_stand_on_the_floor() {
    let shape = figure();
    for (name, speed) in [
        ("crouch_forward", crate::poses::CROUCH_SPEED),
        ("crouch_backward", crate::poses::CROUCH_SPEED),
        ("crouch_side", CROUCH_SIDE_SPEED),
        ("prone_forward", CRAWL_SPEED),
        ("prone_backward", CRAWL_SPEED),
    ] {
        let s = &shape.sequences[find_sequence(&shape, name.as_bytes()).unwrap()];
        assert_ne!(s.flags & 32, 0, "{name} makes a path");
        let last = (s.first_ground_frame + s.num_ground_frames - 1) as usize;
        let g = shape.ground_translations[last];
        let travel = (g[0].powi(2) + g[1].powi(2)).sqrt();
        assert!(
            (travel / s.duration - speed).abs() < 1e-3,
            "{name}: {} a second",
            travel / s.duration
        );
    }
    // Nothing sinks into the floor at any moment of any pose.
    for name in [
        "crouch_root",
        "crouch_forward",
        "crouch_side",
        "prone_root",
        "prone_forward",
    ] {
        for k in 0..32 {
            let inst = posed(&shape, &[(name, k as f32 / 32.0)]);
            for (nd, l) in contacts() {
                let z = at(&shape, &inst, nd, l)[2];
                assert!(z > -0.01, "{name} at {k}: {nd} {z} under the floor");
            }
        }
    }
}

/// The figure's hitboxes at the pose `threads` give: each corner of each part's box.
fn hitbox_corners(shape: &Shape, threads: &[(&str, f32)]) -> Vec<(String, [f32; 3])> {
    let facts = preload::facts(shape, &mut free).unwrap();
    let los = facts.slots.los[0] as usize;
    let inst = posed(shape, threads);
    let mut out = Vec::new();
    for o in &shape.objects {
        let Some(Some(mesh)) = shape.meshes.get(o.start_mesh_index as usize + los) else {
            continue;
        };
        let name = String::from_utf8(shape.names[o.name as usize].clone()).unwrap();
        let m = inst.node_transforms()[o.node as usize];
        out.extend(mesh.positions.iter().map(|c| (name.clone(), mul_p(&m, *c))));
    }
    out
}

#[test]
fn each_pose_fits_its_box_and_lowers_the_eye() {
    let shape = figure();
    // The demos' `crouchBoundingBox` and `proneBoundingBox` (width, depth, height); a tilted shin's corner may dip a little
    // under the floor at the ankle.
    // The crouched walk's knees and shins come up to 8 cm past the front as they swing.
    for (name, size, past) in [
        ("crouch_root", [1.0f32, 1.0, 1.6], 0.0),
        ("crouch_forward", [1.0, 1.0, 1.6], 0.08),
        ("prone_root", [2.0, 2.0, 0.8], 0.0),
    ] {
        for k in 0..8 {
            for (part, p) in hitbox_corners(&shape, &[(name, k as f32 / 8.0)]) {
                assert!(
                    p[0].abs() <= 0.5 * size[0] + past
                        && p[1].abs() <= 0.5 * size[1] + past
                        && (-0.03..=size[2]).contains(&p[2]),
                    "{name} at {k}: {part} at {p:?} outside {size:?}"
                );
            }
        }
    }
    let eye = |threads: &[(&str, f32)]| at(&shape, &posed(&shape, threads), "eye", [0.0; 3]);
    let (stand, crouch, prone) = (
        eye(&[]),
        eye(&[("crouch_root", 0.0)]),
        eye(&[("prone_root", 0.0)]),
    );
    assert!(
        stand[2] > 1.8 && (1.2..1.5).contains(&crouch[2]) && (0.4..0.75).contains(&prone[2]),
        "eye heights: {} {} {}",
        stand[2],
        crouch[2],
        prone[2]
    );
}

#[test]
fn a_crouch_lowers_the_body_over_feet_that_stay_where_they_stand() {
    let shape = figure();
    let mut inst = ShapeInstance::new(&shape, &mut free).unwrap();
    let t = inst.create_thread(&shape).unwrap();
    let root = find_sequence(&shape, b"root").unwrap();
    let crouch = find_sequence(&shape, b"crouch_root").unwrap();
    inst.set_sequence(&shape, t, root as i32, 0.0);
    inst.animate(&shape, 0, None, &mut free).unwrap();
    let feet = |inst: &ShapeInstance| {
        ["ankle_l", "ankle_r"].map(|a| at(&shape, inst, a, [0.0, 0.0, -SOLE]))
    };
    let start = feet(&inst);
    let hips0 = at(&shape, &inst, "torso", [0.0; 3])[2];
    inst.transition_to_sequence(&shape, t, crouch, 0.0, 0.3);
    let (mut worst, mut last_hips, mut smooth) = (0.0f32, hips0, true);
    let steps = 30;
    for _ in 0..steps {
        inst.advance_time(&shape, t, 0.3 / steps as f32);
        inst.animate(&shape, 0, None, &mut free).unwrap();
        let now = feet(&inst);
        for side in 0..2 {
            let d = ((now[side][0] - start[side][0]).powi(2)
                + (now[side][1] - start[side][1]).powi(2))
            .sqrt();
            worst = worst.max(d);
        }
        let hips = at(&shape, &inst, "torso", [0.0; 3])[2];
        smooth &= hips <= last_hips + 1e-4;
        last_hips = hips;
    }
    assert!(
        worst < 0.1,
        "the feet move {worst} along the ground as the figure crouches"
    );
    assert!(smooth, "the hips only go down");
    assert!(
        hips0 - last_hips > 0.3,
        "the hips lower {}",
        hips0 - last_hips
    );
}

#[test]
fn the_hit_reactions_rise_sharply_and_come_back_to_rest() {
    let shape = figure();
    let rest = posed(&shape, &[("root", 0.0)]);
    let head = |inst: &ShapeInstance| at(&shape, inst, "head", [0.0, 0.0, 0.3]);
    let chest = |inst: &ShapeInstance| at(&shape, inst, "torso", [0.0, 0.0, 0.6]);
    let (h0, c0) = (head(&rest), chest(&rest));
    // Where each reaction pushes the head and the chest at its peak: away from the side hit.
    for (name, away) in [
        ("flinch_front", [0.0, -1.0]),
        ("flinch_back", [0.0, 1.0]),
        ("flinch_left", [1.0, 0.0]),
        ("flinch_right", [-1.0, 0.0]),
        ("flinch_head_front", [0.0, -1.0]),
        ("flinch_head_back", [0.0, 1.0]),
        ("flinch_head_left", [1.0, 0.0]),
        ("flinch_head_right", [-1.0, 0.0]),
    ] {
        let seq = find_sequence(&shape, name.as_bytes()).unwrap();
        assert!(
            shape.sequences[seq].flags & torque_shapes::instance::FLAG_BLEND != 0,
            "{name} blends"
        );
        for pos in [0.0, 1.0] {
            let i = posed(&shape, &[("root", 0.0), (name, pos)]);
            let moved = (0..3)
                .map(|k| (head(&i)[k] - h0[k]).abs())
                .fold(0.0, f32::max);
            assert!(moved < 1e-3, "{name} at {pos}: off rest by {moved}");
        }
        let peak = (1..10)
            .map(|k| {
                let i = posed(&shape, &[("root", 0.0), (name, k as f32 / 10.0)]);
                let (h, c) = (head(&i), chest(&i));
                let along =
                    |p: [f32; 3], q: [f32; 3]| (p[0] - q[0]) * away[0] + (p[1] - q[1]) * away[1];
                (along(h, h0), along(c, c0), k)
            })
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap();
        assert!(
            peak.0 > 0.05,
            "{name}: the head moves {} away from the hit",
            peak.0
        );
        assert!(peak.2 <= 4, "{name}: peaks late, at {}", peak.2);
        if name.starts_with("flinch_head") {
            assert!(
                peak.1 < 0.5 * peak.0,
                "{name}: the head snaps, the chest barely ({peak:?})"
            );
        } else {
            assert!(
                peak.1 > 0.04,
                "{name}: the chest is punched away ({peak:?})"
            );
        }
    }
    // Over a crouch or a crawl a reaction still adds to the pose.
    for pose in ["crouch_root", "prone_forward"] {
        let base = head(&posed(&shape, &[(pose, 0.25)]));
        let hit = head(&posed(&shape, &[(pose, 0.25), ("flinch_head_front", 0.15)]));
        let d = (0..3).map(|k| (hit[k] - base[k]).abs()).fold(0.0, f32::max);
        assert!(d > 0.03, "{pose}: the head snaps {d}");
    }
}

#[test]
fn lying_down_the_pistol_arm_points_ahead_along_the_ground() {
    let shape = figure();
    let i = posed(&shape, &[("prone_root", 0.0), ("prone_hold", 0.5)]);
    let elbow = at(&shape, &i, "elbow_r", [0.0; 3]);
    let hand = at(&shape, &i, "elbow_r", [0.0, 0.0, -GRIP]);
    let d = [hand[0] - elbow[0], hand[1] - elbow[1], hand[2] - elbow[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    assert!(d[1] / len > 0.9, "the forearm points ahead: {d:?}");
    assert!(hand[2] > 0.0, "above the ground: {hand:?}");
}

/// Every drawn vertex of the figure at the pose `threads` give (detail 0).
fn drawn(shape: &Shape, threads: &[(&str, f32)]) -> Vec<(String, [f32; 3])> {
    let inst = posed(shape, threads);
    let mut out = Vec::new();
    for o in &shape.objects {
        let Some(Some(mesh)) = shape.meshes.get(o.start_mesh_index as usize) else {
            continue;
        };
        let name = String::from_utf8(shape.names[o.name as usize].clone()).unwrap();
        let m = inst.node_transforms()[o.node as usize];
        out.extend(mesh.positions.iter().map(|c| (name.clone(), mul_p(&m, *c))));
    }
    out
}

fn origins(shape: &Shape, threads: &[(&str, f32)], skip: &[&str]) -> Vec<(String, [f32; 3])> {
    let inst = posed(shape, threads);
    shape
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let name = String::from_utf8(shape.names[n.name as usize].clone()).unwrap();
            let m = inst.node_transforms()[i];
            (name, [m[3], m[7], m[11]])
        })
        .filter(|(name, _)| !skip.contains(&name.as_str()))
        .collect()
}

fn far(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[test]
fn the_way_down_kneels_puts_a_hand_down_and_lies_flat_and_the_way_up_is_it_reversed() {
    use crate::poses::{PRONE_DOWN_SECONDS, PRONE_UP_SECONDS};
    let shape = figure();
    let seq = |name: &str| &shape.sequences[find_sequence(&shape, name.as_bytes()).unwrap()];
    assert!((seq("prone_down").duration - PRONE_DOWN_SECONDS).abs() < 1e-6);
    assert!((seq("prone_up").duration - PRONE_UP_SECONDS).abs() < 1e-6);
    assert_eq!(seq("prone_down").priority, 2, "over a held pistol's arm");
    // The way down starts where the crouch is and ends where prone is (the right arm reaches ahead, a held pistol's way).
    let arm = ["arm_r", "elbow_r", "mount0"];
    for (a, b) in [
        (("crouch_root", 0.0), ("prone_down", 0.0)),
        (("prone_root", 0.0), ("prone_down", 1.0)),
        (("prone_root", 0.0), ("prone_up", 0.0)),
        (("crouch_root", 0.0), ("prone_up", 1.0)),
    ] {
        for ((n, p), (_, q)) in
            origins(&shape, &[a], &arm)
                .into_iter()
                .zip(origins(&shape, &[b], &arm))
        {
            assert!(
                far(p, q) < 0.01,
                "{a:?} and {b:?} differ at {n} by {}",
                far(p, q)
            );
        }
    }
    // Up is down played backward.
    for k in 0..=8 {
        let t = k as f32 / 8.0;
        for ((n, p), (_, q)) in origins(&shape, &[("prone_up", t)], &[])
            .into_iter()
            .zip(origins(&shape, &[("prone_down", 1.0 - t)], &[]))
        {
            assert!(
                far(p, q) < 0.04,
                "up at {t} is not down at {}: {n}",
                1.0 - t
            );
        }
    }
    let n = 96;
    let hand = |t: f32| {
        at(
            &shape,
            &posed(&shape, &[("prone_down", t)]),
            "elbow_l",
            [0.0, 0.0, -GRIP],
        )
    };
    let planted = hand(0.6);
    let (mut lowest, mut slide, mut step) = (f32::MAX, 0.0f32, 0.0f32);
    let mut last: Option<Vec<(String, [f32; 3])>> = None;
    for k in 0..=n {
        let t = k as f32 / n as f32;
        let points = drawn(&shape, &[("prone_down", t)]);
        lowest = points.iter().map(|p| p.1[2]).fold(lowest, f32::min);
        if t >= 0.6 {
            slide = slide.max(far(hand(t), planted));
        }
        // Nothing jumps between two moments a hundredth of the way apart.
        if let Some(l) = &last {
            step = l
                .iter()
                .zip(&points)
                .map(|(a, b)| far(a.1, b.1))
                .fold(step, f32::max);
        }
        last = Some(points);
        // The head looks ahead: its face turned no more than 35 degrees down or 15 up.
        let i = posed(&shape, &[("prone_down", t)]);
        let m = i.node_transforms()[node(&shape, "head")];
        let ahead = [m[1], m[5], m[9]];
        let pitch = ahead[2].atan2(ahead[1]).to_degrees();
        assert!(
            (-35.0..15.0).contains(&pitch),
            "at {t} the head looks {pitch} degrees"
        );
    }
    assert!(lowest > -0.012, "a part sinks {lowest} into the floor");
    assert!(slide < 0.015, "the planted hand slides {slide}");
    assert!(step < 0.06, "a part jumps {step} in a hundredth of the way");
    // The right knee touches the floor first, then both, with the left hand (each shin's end at the knee).
    let shin_low = |t: f32, knee: &str| {
        let i = posed(&shape, &[("prone_down", t)]);
        [-0.09f32, 0.09]
            .iter()
            .flat_map(|&x| [-0.1f32, 0.1].map(move |y| [x, y]))
            .flat_map(|[x, y]| [0.0f32, -0.15].map(move |z| [x, y, z]))
            .map(|c| at(&shape, &i, knee, c)[2])
            .fold(f32::MAX, f32::min)
    };
    assert!(shin_low(0.35, "knee_r") < 0.02 && shin_low(0.35, "knee_l") > 0.1);
    assert!(shin_low(0.55, "knee_r") < 0.02 && shin_low(0.55, "knee_l") < 0.02);
    assert!(
        hand(0.55)[2] < 0.15,
        "the left hand is down: {:?}",
        hand(0.55)
    );
}
