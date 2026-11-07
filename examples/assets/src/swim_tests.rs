//! The swimming sequences: seamless loops, hands and feet that reach where their keys put them with the knees and elbows bent
//! the way they bend, no part passing into another, the strokes' ground speeds, the treading eye out of the water, the
//! breaststroke near level under the surface, and a raised arm over a stroke that goes on.

use torque_shapes::instance::{ShapeInstance, find_sequence};
use torque_shapes::math::{mat_inverse, mul_p};
use torque_shapes::preload;
use torque_shapes::shape::Shape;

use super::{
    Beat, SWIM_BACK_SPEED, SWIM_SIDE_SPEED, SWIM_SPEED, SWIM_WATER, key_of, looped, loops,
    stroke_beats,
};
use crate::poses::solve;
use crate::shapes::{self, GRIP, Rot};

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

fn far(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Every node's origin and each hand's grip.
fn points(shape: &Shape, inst: &ShapeInstance) -> Vec<[f32; 3]> {
    let mut out: Vec<[f32; 3]> = inst
        .node_transforms()
        .iter()
        .map(|m| [m[3], m[7], m[11]])
        .collect();
    for elbow in ["elbow_l", "elbow_r"] {
        out.push(at(shape, inst, elbow, [0.0, 0.0, -GRIP]));
    }
    out
}

const STROKES: [&str; 4] = ["swim_root", "swim_forward", "swim_backward", "swim_side"];

#[test]
fn each_swimming_loop_is_seamless() {
    let shape = figure();
    for (name, _, beat) in loops() {
        // The beats come round to where they started.
        let (a, b) = (beat(0.0).to_array(), beat(1.0 - 1e-5).to_array());
        let off = a
            .iter()
            .zip(&b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f32::max);
        assert!(off < 0.01, "{name}: the loop ends {off} from its start");
        // Through the shape: no step across the seam larger than the loop's others.
        let m = 200;
        let pts: Vec<Vec<[f32; 3]>> = (0..m)
            .map(|j| points(&shape, &posed(&shape, &[(name, j as f32 / m as f32)])))
            .collect();
        let step = |i: usize, j: usize| {
            pts[i]
                .iter()
                .zip(&pts[j])
                .map(|(p, q)| far(*p, *q))
                .fold(0.0, f32::max)
        };
        let inner = (0..m - 1).map(|j| step(j, j + 1)).fold(0.0, f32::max);
        let seam = step(m - 1, 0);
        assert!(
            inner < 0.08,
            "{name}: a part moves {inner} in a 200th of the loop"
        );
        assert!(
            seam < 1.5 * inner + 0.002,
            "{name}: the seam steps {seam}, the loop's largest step {inner}"
        );
    }
}

/// The turn about x of a hinge's rotation (degrees).
fn hinge_degrees(r: &Rot) -> f32 {
    r[2][1].atan2(r[1][1]).to_degrees()
}

#[test]
fn every_hand_and_foot_reaches_its_place_and_the_joints_bend_their_own_way() {
    let shape = figure();
    for (name, n, beat) in loops() {
        for k in 0..n {
            let key = key_of(&beat(k as f32 / n as f32));
            let inst = posed(&shape, &[(name, k as f32 / n as f32)]);
            for (side, s) in ["l", "r"].into_iter().enumerate() {
                let grip = at(&shape, &inst, &format!("elbow_{s}"), [0.0, 0.0, -GRIP]);
                let ankle = at(&shape, &inst, &format!("ankle_{s}"), [0.0; 3]);
                let (g, a) = (
                    far(grip, key.arms[side].grip),
                    far(ankle, key.feet[side].ankle),
                );
                assert!(
                    g < 0.005,
                    "{name} key {k}: the {s} hand is {g} from its place"
                );
                assert!(
                    a < 0.005,
                    "{name} key {k}: the {s} ankle is {a} from its place"
                );
            }
            let t = solve(&key);
            for side in 0..2 {
                let knee = hinge_degrees(&t.legs[side][1]);
                let elbow = hinge_degrees(&t.arms[side][1]);
                assert!(
                    knee < 0.5,
                    "{name} key {k}: knee {side} bends {knee} the wrong way"
                );
                assert!(
                    elbow > -0.5,
                    "{name} key {k}: elbow {side} bends {elbow} the wrong way"
                );
            }
        }
    }
}

/// A hitbox: the part's name, its node's transform and its box on the node.
type Part = (String, [f32; 16], [f32; 3], [f32; 3]);

/// The figure's hitboxes at the pose `threads` give.
fn parts(shape: &Shape, threads: &[(&str, f32)]) -> Vec<Part> {
    let facts = preload::facts(shape, &mut free).unwrap();
    let los = facts.slots.los[0] as usize;
    let inst = posed(shape, threads);
    let mut out = Vec::new();
    for o in &shape.objects {
        let Some(Some(mesh)) = shape.meshes.get(o.start_mesh_index as usize + los) else {
            continue;
        };
        let name = String::from_utf8(shape.names[o.name as usize].clone()).unwrap();
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in mesh.positions.iter() {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        out.push((name, inst.node_transforms()[o.node as usize], lo, hi));
    }
    out
}

/// How far the deepest corner of box `a` is inside box `b` (0 when none is).
fn depth(a: &Part, b: &Part) -> f32 {
    let into_b = mat_inverse(&b.1);
    (0..8)
        .map(|c| {
            let corner = [0, 1, 2].map(|k| if c >> k & 1 == 0 { a.2[k] } else { a.3[k] });
            let p = mul_p(&into_b, mul_p(&a.1, corner));
            (0..3)
                .map(|k| (p[k] - b.2[k]).min(b.3[k] - p[k]))
                .fold(f32::MAX, f32::min)
                .max(0.0)
        })
        .fold(0.0, f32::max)
}

/// How far a part may pass into another (the kit's 3 cm: a tilted shin's corner may dip as far under the floor).
const OVERLAP: f32 = 0.03;

#[test]
fn no_part_passes_into_another_while_swimming() {
    let shape = figure();
    let mut poses: Vec<Vec<(&str, f32)>> = Vec::new();
    for name in STROKES {
        for k in 0..32 {
            let p = k as f32 / 32.0;
            poses.push(vec![(name, p)]);
            let arm = if name == "swim_forward" {
                "swim_forward_point"
            } else {
                "swim_point"
            };
            poses.push(vec![(name, p), (arm, 0.5)]);
            let hold = if name == "swim_forward" {
                "swim_forward_hold"
            } else {
                "swim_hold"
            };
            poses.push(vec![(name, p), (hold, 0.5)]);
        }
    }
    let arms = ["lforearm", "lhand", "rforearm", "rhand"];
    let core = ["torso", "head", "lleg", "rleg"];
    let left = ["lleg", "lshin", "lfoot"];
    let right = ["rleg", "rshin", "rfoot"];
    for threads in poses {
        let all = parts(&shape, &threads);
        let get = |n: &str| all.iter().find(|p| p.0 == n).unwrap();
        let mut pairs: Vec<(&str, &str)> = Vec::new();
        for a in arms {
            pairs.extend(core.iter().map(|c| (a, *c)));
        }
        for l in left {
            pairs.extend(right.iter().map(|r| (l, *r)));
        }
        for (a, b) in pairs {
            let d = depth(get(a), get(b)).max(depth(get(b), get(a)));
            assert!(d < OVERLAP, "{threads:?}: {a} is {d} into {b}");
        }
    }
}

#[test]
fn the_strokes_carry_the_underwater_speeds() {
    let shape = figure();
    for (name, dir, speed) in [
        ("swim_forward", [0.0, 1.0], SWIM_SPEED),
        ("swim_backward", [0.0, -1.0], SWIM_BACK_SPEED),
        ("swim_side", [-1.0, 0.0], SWIM_SIDE_SPEED),
    ] {
        let s = &shape.sequences[find_sequence(&shape, name.as_bytes()).unwrap()];
        assert_ne!(s.flags & 32, 0, "{name} makes a path");
        let last = (s.first_ground_frame + s.num_ground_frames - 1) as usize;
        let g = shape.ground_translations[last];
        let along = (g[0] * dir[0] + g[1] * dir[1]) / s.duration;
        assert!((along - speed).abs() < 1e-3, "{name}: {along} a second");
        assert!(
            (g[0] * dir[1] - g[1] * dir[0]).abs() < 1e-4,
            "{name} goes straight"
        );
    }
}

#[test]
fn treading_the_eye_is_out_of_the_water_the_hands_under_and_the_feet_over_the_floor() {
    let shape = figure();
    let eye = at(
        &shape,
        &posed(&shape, &[("swim_root", 0.0)]),
        "eye",
        [0.0; 3],
    )[2] - SWIM_WATER;
    assert!(
        (0.1..0.2).contains(&eye),
        "the treading eye is {eye} out of the water"
    );
    for name in ["swim_root", "swim_backward", "swim_side"] {
        for k in 0..32 {
            let p = k as f32 / 32.0;
            let inst = posed(&shape, &[(name, p)]);
            let eye = at(&shape, &inst, "eye", [0.0; 3])[2];
            assert!(eye > SWIM_WATER + 0.06, "{name} at {p}: the eye at {eye}");
            for elbow in ["elbow_l", "elbow_r"] {
                let hand = at(&shape, &inst, elbow, [0.0, 0.0, -GRIP])[2];
                assert!(hand < SWIM_WATER - 0.1, "{name} at {p}: a hand at {hand}");
            }
            // Treading in water no deeper than it swims in, the feet stand on the floor rather than in it.
            for part in parts(&shape, &[(name, p)]) {
                let low = lowest(&part);
                assert!(
                    low > -0.03,
                    "{name} at {p}: {} {low} under the feet's origin",
                    part.0
                );
            }
        }
    }
}

/// The lowest corner of a part's hitbox.
fn lowest((_, m, lo, hi): &Part) -> f32 {
    (0..8)
        .map(|c| {
            let corner = [0, 1, 2].map(|k| if c >> k & 1 == 0 { lo[k] } else { hi[k] });
            mul_p(m, corner)[2]
        })
        .fold(f32::MAX, f32::min)
}

/// The angle (degrees) the torso leans from upright.
fn lean(shape: &Shape, inst: &ShapeInstance) -> f32 {
    let m = inst.node_transforms()[node(shape, "torso")];
    m[10].clamp(-1.0, 1.0).acos().to_degrees()
}

#[test]
fn the_breaststroke_lies_near_level_under_the_surface_and_comes_up_to_breathe() {
    let shape = figure();
    let n = 52;
    let mut sum = 0.0;
    for k in 0..n {
        let p = k as f32 / n as f32;
        let inst = posed(&shape, &[("swim_forward", p)]);
        let l = lean(&shape, &inst);
        assert!(
            (58.0..86.0).contains(&l),
            "at {p} the body leans {l} degrees"
        );
        sum += l;
        // The hips stay just under the surface.
        let hips = at(&shape, &inst, "torso", [0.0; 3])[2];
        assert!(
            (SWIM_WATER - 0.35..SWIM_WATER - 0.1).contains(&hips),
            "at {p} the hips are at {hips}"
        );
    }
    let mean = sum / n as f32;
    assert!(
        (70.0..80.0).contains(&mean),
        "the body leans {mean} degrees on average"
    );
    let eye = |p: f32| {
        at(
            &shape,
            &posed(&shape, &[("swim_forward", p)]),
            "eye",
            [0.0; 3],
        )[2]
    };
    assert!(
        eye(0.32) > SWIM_WATER + 0.1,
        "the breath: the eye at {}",
        eye(0.32)
    );
    assert!(
        eye(0.8) < SWIM_WATER,
        "the glide: the face down, the eye at {}",
        eye(0.8)
    );
    // The hands wide at the sweep and together under the chin at the breath.
    let hands = |p: f32| {
        let i = posed(&shape, &[("swim_forward", p)]);
        far(
            at(&shape, &i, "elbow_l", [0.0, 0.0, -GRIP]),
            at(&shape, &i, "elbow_r", [0.0, 0.0, -GRIP]),
        )
    };
    assert!(
        hands(0.12) > 0.9,
        "the sweep: the hands {} apart",
        hands(0.12)
    );
    assert!(
        hands(0.32) < 0.3,
        "under the chin: the hands {} apart",
        hands(0.32)
    );
    // The heels drawn up to the seat before the kick, and the legs long in the glide.
    let heel = |p: f32| {
        let i = posed(&shape, &[("swim_forward", p)]);
        far(
            at(&shape, &i, "ankle_l", [0.0; 3]),
            at(&shape, &i, "torso", [0.0; 3]),
        )
    };
    assert!(
        heel(0.44) < 0.55 && heel(0.8) > 0.8,
        "the heels: {} {}",
        heel(0.44),
        heel(0.8)
    );
    // The beats are the looped curve's: a beat's own phase gives the beat.
    let beats = stroke_beats();
    for (phase, beat) in &beats {
        let b: Beat = looped(&beats, *phase);
        let off = b
            .to_array()
            .iter()
            .zip(beat.to_array())
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f32::max);
        assert!(off < 1e-4, "the curve misses the beat at {phase} by {off}");
    }
}

/// The unit direction from `a` to `b`.
fn toward(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    let d = far(a, b);
    [0, 1, 2].map(|k| (b[k] - a[k]) / d)
}

#[test]
fn the_swimming_arm_actions_raise_the_right_arm_ahead_and_the_rest_swims_on() {
    let shape = figure();
    let mut checked = 0;
    for name in STROKES {
        for (point, hold) in [
            ("swim_point", "swim_hold"),
            ("swim_forward_point", "swim_forward_hold"),
        ] {
            // Each action's own version (`swim_forward_point`) for the breaststroke, the pose's for the rest.
            if (name == "swim_forward") != point.starts_with("swim_forward") {
                continue;
            }
            for k in 0..8 {
                let p = k as f32 / 8.0;
                let swim = posed(&shape, &[(name, p)]);
                for arm in [point, hold] {
                    let raised = posed(&shape, &[(name, p), (arm, 0.5)]);
                    // The forearm points ahead, level (the look at 0.5).
                    let dir = toward(
                        at(&shape, &raised, "elbow_r", [0.0; 3]),
                        at(&shape, &raised, "elbow_r", [0.0, 0.0, -GRIP]),
                    );
                    assert!(
                        dir[1] > 0.85,
                        "{name} at {p} with {arm}: the forearm points {dir:?}"
                    );
                    // The left arm and the legs keep the stroke.
                    for n in [
                        "elbow_l", "arm_l", "knee_l", "knee_r", "ankle_l", "ankle_r", "head",
                    ] {
                        let d = far(
                            at(&shape, &swim, n, [0.0; 3]),
                            at(&shape, &raised, n, [0.0; 3]),
                        );
                        assert!(d < 1e-4, "{name} at {p} with {arm}: {n} moved {d}");
                    }
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 4 * 8 * 2);
}
