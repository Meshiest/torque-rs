use torque_images::{Format, Kind};
use torque_shapes::instance::{ShapeInstance, find_sequence};
use torque_shapes::math::{mul_p, mul_v};
use torque_shapes::shape::{Model, Shape};
use torque_shapes::{bounds, dts, preload, write::write_dts};

use crate::shapes;

fn free(_: u64) -> bool {
    true
}

fn models() -> [(&'static str, Model); 4] {
    [
        ("cube", shapes::cube()),
        ("figure", shapes::figure()),
        ("pistol", shapes::pistol()),
        ("projectile", shapes::projectile()),
    ]
}

fn near(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() < 1e-3)
}

#[test]
fn every_shape_parses_back_to_the_model_it_was_written_from() {
    for (name, model) in models() {
        let bytes = write_dts(&model);
        let back = dts::parse(&bytes, &mut free).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(back.shape, model.shape, "{name}");
        assert_eq!(back.render, model.render, "{name}");
        assert_eq!(back.file.materials, model.file.materials, "{name}");
        assert_eq!(write_dts(&back), bytes, "{name}");
    }
}

#[test]
fn every_bound_is_the_one_the_vertices_give() {
    for (name, model) in models() {
        let report = bounds::check(&model, &mut free).unwrap();
        assert!(report.all_equal(), "{name}: {report:?}");
        assert_eq!(report.detail0_inside, Some(true), "{name}");
    }
}

#[test]
fn every_triangle_is_clockwise_seen_from_its_front() {
    for (name, model) in models() {
        for (mesh, render) in model.shape.meshes.iter().zip(&model.render) {
            let (Some(mesh), Some(render)) = (mesh, render) else {
                continue;
            };
            let p = &mesh.positions;
            let centre = mesh.center;
            for t in mesh.indices.chunks_exact(3) {
                let [a, b, c] = [0, 1, 2].map(|i| p[usize::from(t[i])]);
                let (e1, e2) = (
                    [0, 1, 2].map(|k| b[k] - a[k]),
                    [0, 1, 2].map(|k| c[k] - a[k]),
                );
                let cross = [
                    e1[1] * e2[2] - e1[2] * e2[1],
                    e1[2] * e2[0] - e1[0] * e2[2],
                    e1[0] * e2[1] - e1[1] * e2[0],
                ];
                // Outward: from the box's centre to the triangle; the normals the renderer lights with agree.
                let out = [0, 1, 2].map(|k| (a[k] + b[k] + c[k]) / 3.0 - centre[k]);
                let dot = |v: [f32; 3]| v[0] * cross[0] + v[1] * cross[1] + v[2] * cross[2];
                assert!(dot(out) < 0.0, "{name}: {t:?}");
                if render.normals.len() == p.len() && mesh.num_verts == 24 {
                    assert!(
                        dot(render.normals[usize::from(t[0])]) < 0.0,
                        "{name}: {t:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_cube_has_a_collision_hull_and_a_line_of_sight_detail() {
    let shape = Shape::for_server(shapes::cube());
    let facts = preload::facts(&shape, &mut free).unwrap();
    assert_eq!(facts.slots.hulls(), 1);
    assert_eq!(facts.slots.own_los(), 1);
    assert!(facts.all_inside());
    assert_eq!(facts.bounds[0], [-0.5, -0.5, -0.5, 0.5, 0.5, 0.5]);
    assert_eq!(shape.bounds, [-0.5, -0.5, -0.5, 0.5, 0.5, 0.5]);
}

#[test]
fn the_figure_has_the_nodes_a_player_looks_up() {
    let shape = Shape::for_server(shapes::figure());
    let facts = preload::facts(&shape, &mut free).unwrap();
    let node = |name: &str| {
        let n = shape.find_name(name.as_bytes()).unwrap();
        shape.nodes.iter().position(|x| x.name == n).unwrap() as i32
    };
    assert_eq!(facts.eye_node, node("eye"));
    assert_eq!(facts.camera_node, node("cam"));
    assert_eq!(facts.mount_nodes[0], node("mount0"));
    assert_eq!(
        facts.slots.hulls(),
        0,
        "a player collides by its box, not a hull"
    );
    assert!((shape.bounds[5] - 2.01).abs() < 1e-4, "{:?}", shape.bounds);
}

/// Where the point `local` of node `name` is when `sequence` is at `pos`.
fn posed(shape: &Shape, sequence: &str, pos: f32, name: &str, local: [f32; 3]) -> [f32; 3] {
    let mut inst = ShapeInstance::new(shape, &mut free).unwrap();
    let t = inst.create_thread(shape).unwrap();
    let seq = find_sequence(shape, sequence.as_bytes()).unwrap();
    inst.set_sequence(shape, t, seq as i32, pos);
    inst.animate(shape, 0, None, &mut free).unwrap();
    let n = shape.find_name(name.as_bytes()).unwrap();
    let node = shape.nodes.iter().position(|x| x.name == n).unwrap();
    mul_p(&inst.node_transforms()[node], local)
}

#[test]
fn walking_and_running_swing_the_legs_against_each_other() {
    let shape = Shape::for_server(shapes::figure());
    let foot = [0.0, 0.0, -shapes::SHIN];
    for (seq, reach) in [("walk", 0.35), ("run", 0.5)] {
        let (l, r) = (
            posed(&shape, seq, 0.25, "knee_l", foot),
            posed(&shape, seq, 0.25, "knee_r", foot),
        );
        assert!(l[1] > reach && r[1] < -reach, "{seq}: {l:?} {r:?}");
        let (l, r) = (
            posed(&shape, seq, 0.75, "knee_l", foot),
            posed(&shape, seq, 0.75, "knee_r", foot),
        );
        assert!(l[1] < -reach && r[1] > reach, "{seq}: {l:?} {r:?}");
        // The arm swings against its own side's leg.
        let elbow = posed(&shape, seq, 0.25, "elbow_l", [0.0; 3]);
        assert!(elbow[1] < -0.1, "{seq}: {elbow:?}");
    }
    let head = posed(&shape, "run", 0.0, "head", [0.0; 3]);
    assert!(head[1] > 0.1, "the run leans forward: {head:?}");
    let still = posed(&shape, "walk", 0.0, "head", [0.0; 3]);
    assert!(near(still, [0.0, 0.0, 1.65]), "{still:?}");
}

/// The angle (degrees) between a limb's upper and lower part at a joint: 0 straight.
fn bend(shape: &Shape, seq: &str, pos: f32, upper: &str, joint: &str, lower: f32) -> f32 {
    let top = posed(shape, seq, pos, upper, [0.0; 3]);
    let mid = posed(shape, seq, pos, joint, [0.0; 3]);
    let end = posed(shape, seq, pos, joint, [0.0, 0.0, -lower]);
    let a = [0, 1, 2].map(|k| mid[k] - top[k]);
    let b = [0, 1, 2].map(|k| end[k] - mid[k]);
    let dot = (0..3).map(|k| a[k] * b[k]).sum::<f32>();
    let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (dot / (len(a) * len(b)))
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

#[test]
fn the_knees_bend_as_the_legs_come_through_and_the_elbows_bend_running() {
    let shape = Shape::for_server(shapes::figure());
    let shin = shapes::SHIN;
    for seq in ["walk", "run"] {
        // A quarter in: the left leg forward (its foot about to land), the right behind (coming through).
        let front = bend(&shape, seq, 0.25, "leg_l", "knee_l", shin);
        let back = bend(&shape, seq, 0.25, "leg_r", "knee_r", shin);
        assert!(back > front + 30.0, "{seq}: {front} {back}");
        // The knee bends the shin backward, never forward.
        let knee = posed(&shape, seq, 0.25, "knee_r", [0.0; 3]);
        let foot = posed(&shape, seq, 0.25, "knee_r", [0.0, 0.0, -shin]);
        let thigh = posed(&shape, seq, 0.25, "leg_r", [0.0; 3]);
        let ahead =
            (knee[1] - thigh[1]) * (foot[2] - knee[2]) - (knee[2] - thigh[2]) * (foot[1] - knee[1]);
        assert!(ahead < 0.0, "{seq}: {thigh:?} {knee:?} {foot:?}");
    }
    let still = bend(&shape, "walk", 0.25, "leg_l", "knee_l", shin);
    assert!(still < 2.0, "the landing leg is straight: {still}");
    let walking = bend(&shape, "walk", 0.0, "arm_r", "elbow_r", shapes::FOREARM);
    let running = bend(&shape, "run", 0.0, "arm_r", "elbow_r", shapes::FOREARM);
    assert!(running > 60.0 && walking < 30.0, "{walking} {running}");
}

#[test]
fn the_hands_reach_mid_thigh_and_the_hand_holds_the_mount() {
    let shape = Shape::for_server(shapes::figure());
    let fingers = posed(&shape, "root", 0.0, "elbow_l", [0.0, 0.0, -shapes::FOREARM]);
    let mid_thigh = 0.95 - 0.5 * shapes::THIGH;
    assert!((fingers[2] - mid_thigh).abs() < 0.08, "{fingers:?}");
    let mount = posed(&shape, "root", 0.0, "mount0", [0.0; 3]);
    let wrist = posed(
        &shape,
        "root",
        0.0,
        "elbow_r",
        [0.0, 0.0, -shapes::FOREARM + 0.11],
    );
    assert!(mount[2] < wrist[2] && mount[2] > fingers[2], "{mount:?}");
}

#[test]
fn the_jump_swings_the_arms_forward_tucks_the_knees_and_holds_its_last_key() {
    let shape = Shape::for_server(shapes::figure());
    for side in ["l", "r"] {
        let hand = posed(
            &shape,
            "jump",
            1.0,
            &format!("elbow_{side}"),
            [0.0, 0.0, -shapes::FOREARM],
        );
        let shoulder = posed(&shape, "jump", 1.0, &format!("arm_{side}"), [0.0; 3]);
        // In front of the figure, at about shoulder height: not overhead.
        assert!(hand[1] > shoulder[1] + 0.4, "{side}: {hand:?} {shoulder:?}");
        assert!(
            (hand[2] - shoulder[2]).abs() < 0.2,
            "{side}: {hand:?} {shoulder:?}"
        );
    }
    for side in ["knee_l", "knee_r"] {
        let foot = posed(&shape, "jump", 1.0, side, [0.0, 0.0, -shapes::SHIN]);
        assert!(foot[2] > 0.3, "tucked up: {side} {foot:?}");
    }
    let seq = |name: &str| &shape.sequences[find_sequence(&shape, name.as_bytes()).unwrap()];
    assert_eq!(seq("jump").flags & 16, 0, "played once");
    for name in ["root", "idle", "walk", "run"] {
        assert_eq!(seq(name).flags & 16, 16, "{name} loops");
    }
    assert_eq!(seq("root").rotation_matters, seq("idle").rotation_matters);
}

#[test]
fn the_eyes_are_boxes_on_the_face() {
    let model = shapes::figure();
    let shape = &model.shape;
    for name in ["pupil_l", "pupil_r"] {
        let n = shape.find_name(name.as_bytes()).unwrap();
        assert!(shape.objects.iter().any(|o| o.name == n), "{name}");
    }
}

#[test]
fn holding_raises_the_right_arm_and_the_held_item_points_along_the_forearm() {
    let shape = Shape::for_server(shapes::figure());
    let mut inst = ShapeInstance::new(&shape, &mut free).unwrap();
    for (name, pos) in [("walk", 0.25), ("hold", 0.5)] {
        let t = inst.create_thread(&shape).unwrap();
        let seq = find_sequence(&shape, name.as_bytes()).unwrap();
        inst.set_sequence(&shape, t, seq as i32, pos);
    }
    inst.animate(&shape, 0, None, &mut free).unwrap();
    let node = |name: &str| {
        let n = shape.find_name(name.as_bytes()).unwrap();
        shape.nodes.iter().position(|x| x.name == n).unwrap()
    };
    let hand = &inst.node_transforms()[node("mount0")];
    let elbow = &inst.node_transforms()[node("elbow_r")];
    let ahead = mul_v(hand, [0.0, 1.0, 0.0]);
    let up = mul_v(hand, [0.0, 0.0, 1.0]);
    assert!(ahead[1] > 0.99 && up[2] > 0.99, "{ahead:?} {up:?}");
    // The forearm points the same way, and the hand is in front of the shoulder at its height.
    let forearm = mul_v(elbow, [0.0, 0.0, -1.0]);
    assert!(forearm[1] > 0.99, "{forearm:?}");
    let grip = mul_p(hand, [0.0; 3]);
    assert!(grip[1] > 0.6 && (grip[2] - 1.5).abs() < 0.15, "{grip:?}");
    // The left arm still walks.
    let left = mul_p(&inst.node_transforms()[node("elbow_l")], [0.0; 3]);
    assert!(left[1] < -0.1, "{left:?}");
}

#[test]
fn walk_and_run_carry_their_ground_speed() {
    let shape = Shape::for_server(shapes::figure());
    for (name, speed) in [("walk", shapes::WALK_SPEED), ("run", shapes::RUN_SPEED)] {
        let s = &shape.sequences[find_sequence(&shape, name.as_bytes()).unwrap()];
        let last = (s.first_ground_frame + s.num_ground_frames - 1) as usize;
        let travel = shape.ground_translations[last][1];
        assert!(
            (travel / s.duration - speed).abs() < 1e-4,
            "{name}: {travel}"
        );
    }
}

#[test]
fn the_pistol_fires_its_slide_back_and_has_its_image_nodes() {
    let shape = Shape::for_server(shapes::pistol());
    for node in ["mountPoint", "muzzlePoint", "ejectPoint"] {
        let n = shape.find_name(node.as_bytes()).unwrap();
        assert!(shape.nodes.iter().any(|x| x.name == n), "{node}");
    }
    let back = posed(&shape, "fire", 0.5, "slide", [0.0; 3]);
    assert!(near(back, [0.0, -shapes::RECOIL, 0.0]), "{back:?}");
    let home = posed(&shape, "fire", 1.0, "slide", [0.0; 3]);
    assert!(near(home, [0.0; 3]), "{home:?}");
    // The muzzle point is on the barrel's front face, the front of the whole pistol.
    let muzzle = posed(&shape, "fire", 0.0, "muzzlePoint", [0.0; 3]);
    assert!(
        near(muzzle, [0.0, shapes::MUZZLE_Y, shapes::BORE_Z]),
        "{muzzle:?}"
    );
    assert!(
        (shape.bounds[4] - shapes::MUZZLE_Y).abs() < 1e-6,
        "{:?}",
        shape.bounds
    );
    // The hand's point is inside the grip, below the slide.
    let mount = posed(&shape, "fire", 0.0, "mountPoint", [0.0; 3]);
    assert!(
        mount[2] < -0.02 && mount[2] > -0.09 && mount[1].abs() < 0.03,
        "{mount:?}"
    );
}

#[test]
fn the_textures_decode_with_the_engines_reader() {
    let cases: [(&str, Vec<u8>, u32); 6] = [
        ("grid", crate::textures::grid().unwrap(), 64),
        ("figure", crate::textures::figure().unwrap(), 64),
        ("pistol", crate::textures::pistol().unwrap(), 32),
        ("projectile", crate::textures::projectile().unwrap(), 16),
        ("spark", crate::textures::spark().unwrap(), 32),
        ("hole", crate::textures::hole().unwrap(), 16),
    ];
    for (name, bytes, side) in cases {
        let image =
            torque_images::decode(Kind::Png, &bytes).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(
            (image.width, image.height, image.format),
            (side, side, Format::Rgb),
            "{name}"
        );
    }
    let grid = torque_images::decode(Kind::Png, &crate::textures::grid().unwrap()).unwrap();
    let px = |x: usize, y: usize| &grid.bits[(y * 64 + x) * 3..][..3];
    assert!(px(0, 0)[0] < px(8, 8)[0], "a darker border");
}

#[test]
fn the_kit_is_the_same_on_every_run_and_every_file_is_named_once() {
    let (a, b) = (crate::all().unwrap(), crate::all().unwrap());
    assert_eq!(a, b);
    let mut names: Vec<_> = a.iter().map(|x| x.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 10);
    // Every material names a texture of the kit.
    for (_, model) in models() {
        for m in &model.shape.materials {
            let file = format!(
                "{}.png",
                String::from_utf8_lossy(m.name.as_deref().unwrap())
            );
            assert!(names.contains(&file.as_str()), "{file}");
        }
    }
}

/// Where the point `local` of node `name` is with `threads` (sequence and position) playing, in order.
fn posed_with(shape: &Shape, threads: &[(&str, f32)], name: &str, local: [f32; 3]) -> [f32; 3] {
    let mut inst = ShapeInstance::new(shape, &mut free).unwrap();
    for (sequence, pos) in threads {
        let t = inst.create_thread(shape).unwrap();
        let seq = find_sequence(shape, sequence.as_bytes()).unwrap();
        inst.set_sequence(shape, t, seq as i32, *pos);
    }
    inst.animate(shape, 0, None, &mut free).unwrap();
    let n = shape.find_name(name.as_bytes()).unwrap();
    let node = shape.nodes.iter().position(|x| x.name == n).unwrap();
    mul_p(&inst.node_transforms()[node], local)
}

#[test]
fn back_and_side_carry_their_ground_speed_backward_and_to_the_left() {
    let shape = Shape::for_server(shapes::figure());
    for (name, speed, axis) in [
        ("back", -shapes::BACK_SPEED, 1),
        ("side", -shapes::SIDE_SPEED, 0),
    ] {
        let s = &shape.sequences[find_sequence(&shape, name.as_bytes()).unwrap()];
        assert!(s.flags & 16 != 0, "{name} loops");
        let last = (s.first_ground_frame + s.num_ground_frames - 1) as usize;
        let travel = shape.ground_translations[last];
        assert!(
            (travel[axis] / s.duration - speed).abs() < 1e-4 && travel[1 - axis].abs() < 1e-6,
            "{name}: {travel:?}"
        );
    }
}

#[test]
fn walking_backward_lifts_each_foot_as_it_comes_back() {
    let shape = Shape::for_server(shapes::figure());
    let foot = [0.0, 0.0, -shapes::SHIN];
    let rest = posed(&shape, "root", 0.0, "knee_l", foot)[2];
    // The left leg is forward at the loop's start and swings back under the hip by a quarter.
    let (start, quarter) = (
        posed(&shape, "back", 0.0, "knee_l", foot),
        posed(&shape, "back", 0.25, "knee_l", foot),
    );
    assert!(
        quarter[1] < start[1],
        "the left leg swings back: {start:?} {quarter:?}"
    );
    let lifted = (0..8)
        .map(|k| posed(&shape, "back", k as f32 / 8.0, "knee_l", foot)[2] - rest)
        .fold(0.0f32, f32::max);
    assert!(lifted > 0.08, "the left foot lifts: {lifted}");
}

#[test]
fn the_side_step_swings_the_left_foot_out_then_closes_the_right_in() {
    let shape = Shape::for_server(shapes::figure());
    let foot = [0.0, 0.0, -shapes::SHIN];
    let rest_r = posed(&shape, "root", 0.0, "knee_r", foot);
    let rest_l = posed(&shape, "root", 0.0, "knee_l", foot);
    let out_l = posed(&shape, "side", 0.25, "knee_l", foot);
    assert!(
        out_l[0] < rest_l[0] - 0.1 && out_l[2] > rest_l[2] + 0.05,
        "the left foot lifts and swings out: {out_l:?}"
    );
    let down_r = posed(&shape, "side", 0.25, "knee_r", foot);
    assert!(
        down_r[2] < rest_r[2] + 0.02,
        "the right stays down: {down_r:?}"
    );
    let in_r = posed(&shape, "side", 0.75, "knee_r", foot);
    assert!(in_r[2] > rest_r[2] + 0.05, "then the right lifts: {in_r:?}");
}

#[test]
fn the_deaths_fall_backward_and_forward_and_lie_flat_on_the_floor() {
    let shape = Shape::for_server(shapes::figure());
    for (name, way) in [("death1", -1.0f32), ("death2", 1.0)] {
        let s = &shape.sequences[find_sequence(&shape, name.as_bytes()).unwrap()];
        assert_eq!(s.flags & 16, 0, "{name} plays once");
        let head = posed(&shape, name, 1.0, "head", [0.0, 0.0, 0.2]);
        assert!(
            head[1] * way > 1.4 && head[2] < 0.4,
            "{name}: the head lies on the floor that way: {head:?}"
        );
        // Lying still, every corner of the head, the torso and the feet is on or above the floor, and none far off it.
        let corners = |min: [f32; 3], max: [f32; 3]| {
            (0..8).map(move |i| {
                [
                    if i & 1 == 0 { min[0] } else { max[0] },
                    if i & 2 == 0 { min[1] } else { max[1] },
                    if i & 4 == 0 { min[2] } else { max[2] },
                ]
            })
        };
        let foot = (
            [-0.095, -0.11, -shapes::SHIN],
            [0.095, 0.17, -shapes::SHIN + 0.08],
        );
        let mut lowest = f32::MAX;
        for (node, min, max) in [
            ("head", [-0.16, -0.16, 0.02], [0.16, 0.16, 0.36]),
            ("torso", [-0.25, -0.13, 0.0], [0.25, 0.13, 0.7]),
            ("knee_l", foot.0, foot.1),
            ("knee_r", foot.0, foot.1),
        ] {
            for c in corners(min, max) {
                lowest = lowest.min(posed(&shape, name, 1.0, node, c)[2]);
            }
        }
        assert!(
            (-0.02..0.08).contains(&lowest),
            "{name}: the lowest corner {lowest}"
        );
        // Standing at the start.
        let start = posed(&shape, name, 0.0, "head", [0.0; 3]);
        assert!(near(start, [0.0, 0.0, 1.65]), "{name}: {start:?}");
    }
}

#[test]
fn the_look_bends_the_head_torso_and_aiming_arm_with_the_pitch() {
    let shape = Shape::for_server(shapes::figure());
    let s = &shape.sequences[find_sequence(&shape, b"head").unwrap()];
    assert_ne!(s.flags & 8, 0, "head is a blend sequence");
    for (pos, pitch) in [
        (0.0, shapes::LOOK_RANGE),
        (0.5, 0.0),
        (1.0, -shapes::LOOK_RANGE),
    ] {
        assert_eq!(shapes::look_pitch(pos), pitch);
        // With a gait, the hold and the look playing, the held item points along the aim.
        let threads = [("walk", 0.25), ("hold", pos), ("head", pos)];
        let at = posed_with(&shape, &threads, "mount0", [0.0; 3]);
        let ahead = posed_with(&shape, &threads, "mount0", [0.0, 1.0, 0.0]);
        let d = [0, 1, 2].map(|k| ahead[k] - at[k]);
        let got = d[2].atan2(d[1]).to_degrees();
        assert!(
            (got - pitch).abs() < 3.0,
            "at {pos}: aims {got}, not {pitch}"
        );
        // The face turns the same way, by less.
        let eye = posed_with(&shape, &threads, "head", [0.0, 1.0, 0.2]);
        let center = posed_with(&shape, &threads, "head", [0.0, 0.0, 0.2]);
        let face = (eye[2] - center[2]).atan2(eye[1] - center[1]).to_degrees();
        let share = shapes::LOOK_TORSO + shapes::LOOK_HEAD;
        assert!(
            (face - share * pitch).abs() < 3.0,
            "at {pos}: the face {face}"
        );
    }
    // The blend adds to the run's lean: the head is further forward looking down than level.
    let level = posed_with(&shape, &[("run", 0.0), ("head", 0.5)], "head", [0.0; 3]);
    let down = posed_with(&shape, &[("run", 0.0), ("head", 1.0)], "head", [0.0; 3]);
    let run = posed(&shape, "run", 0.0, "head", [0.0; 3]);
    assert!(near(level, run), "{level:?} {run:?}");
    assert!(down[1] > level[1] + 0.1, "{down:?}");
}

#[test]
fn the_whole_top_of_the_pistol_runs_back_and_the_barrel_stays() {
    let shape = Shape::for_server(shapes::pistol());
    let front = [0.0, shapes::MUZZLE_Y, 0.05];
    let back = posed(&shape, "fire", 0.5, "slide", front);
    assert!(
        near(back, [0.0, shapes::MUZZLE_Y - shapes::RECOIL, 0.05]),
        "the slide's front face runs back with it: {back:?}"
    );
    let n = |name: &str| shape.find_name(name.as_bytes()).unwrap();
    let node_of = |object: &str| {
        shape
            .objects
            .iter()
            .find(|o| o.name == n(object))
            .unwrap()
            .node
    };
    let slide = shape
        .nodes
        .iter()
        .position(|x| x.name == n("slide"))
        .unwrap() as i32;
    for part in ["slide", "front_sight", "rear_sight"] {
        assert_eq!(node_of(part), slide, "{part} is on the slide");
    }
    assert_ne!(node_of("barrel"), slide, "the barrel is fixed");
}
