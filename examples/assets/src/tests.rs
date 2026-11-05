use torque_images::{Format, Kind};
use torque_shapes::instance::{ShapeInstance, find_sequence};
use torque_shapes::math::{mul_p, mul_v};
use torque_shapes::shape::{Model, Shape};
use torque_shapes::{bounds, dts, preload, write::write_dts};

use crate::shapes;

fn free(_: u64) -> bool {
    true
}

fn models() -> [(&'static str, Model); 6] {
    [
        ("cube", shapes::cube()),
        ("figure", shapes::figure()),
        ("pistol", shapes::pistol()),
        ("projectile", shapes::projectile()),
        ("buggy", crate::buggy::buggy()),
        ("buggy_wheel", crate::buggy::wheel()),
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
                // The buggy's meshes are many parts each: their faces agree with their own normals instead.
                if name.starts_with("buggy") {
                    if render.normals.len() == p.len() {
                        for &i in t {
                            assert!(dot(render.normals[usize::from(i)]) < 0.0, "{name}: {t:?}");
                        }
                    }
                    continue;
                }
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
fn the_figures_hitboxes_are_a_line_of_sight_detail_inside_its_box() {
    let shape = Shape::for_server(shapes::figure());
    let facts = preload::facts(&shape, &mut free).unwrap();
    assert_eq!(facts.slots.own_los(), 1);
    let los = facts.slots.los[0] as usize;
    assert!(shape.details[los].size < 0.0, "never drawn");
    let inst = ShapeInstance::new(&shape, &mut free).unwrap();
    let mut parts = Vec::new();
    for o in &shape.objects {
        let Some(Some(mesh)) = shape.meshes.get(o.start_mesh_index as usize + los) else {
            continue;
        };
        let name = String::from_utf8(shape.names[o.name as usize].clone()).unwrap();
        let hull = torque_shapes::collide::Hull::build_relative(mesh, &mut free).unwrap();
        assert_eq!(hull.planes(), 6, "{name}: a closed box");
        let node = inst.node_transforms()[o.node as usize];
        for c in mesh.positions.iter() {
            let p = mul_p(&node, *c);
            assert!(
                p[0].abs() <= 0.5 && p[1].abs() <= 0.5 && (0.0..=2.0).contains(&p[2]),
                "{name} at {p:?}"
            );
        }
        parts.push(name);
    }
    for name in [
        "head", "torso", "larm", "rforearm", "lhand", "rleg", "lshin", "rfoot",
    ] {
        assert!(parts.iter().any(|p| p == name), "{name} in {parts:?}");
    }
    assert_eq!(parts.len(), 14);
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
        (
            "flare_star",
            crate::textures::flare_star().unwrap(),
            crate::textures::FLARE_STAR_SIDE,
        ),
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
    // The holes, drawn multiplied: a mid-gray corner (no change), a dark middle.
    for (name, bytes) in [
        ("hole", crate::textures::hole_of(1).unwrap()),
        ("hole2", crate::textures::hole_of(2).unwrap()),
    ] {
        let image =
            torque_images::decode(Kind::Png, &bytes).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(
            (image.width, image.height, image.format),
            (64, 64, Format::Rgb),
            "{name}"
        );
        let px = |x: usize, y: usize| &image.bits[(y * 64 + x) * 3..][..3];
        assert_eq!(px(0, 0), [128, 128, 128], "{name}: a neutral corner");
        assert!(px(32, 32).iter().all(|&c| c < 40), "{name}: a dark middle");
    }
    // The smoke, with an alpha channel: soft edges, clear corners.
    {
        let (name, bytes) = ("smoke", crate::textures::smoke().unwrap());
        let image =
            torque_images::decode(Kind::Png, &bytes).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(
            (image.width, image.height, image.format),
            (64, 64, Format::Rgba),
            "{name}"
        );
        let alpha = |x: usize, y: usize| image.bits[(y * 64 + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0, "{name}: a clear corner");
        assert!(alpha(32, 32) > 150, "{name}: a solid middle");
    }
    assert_ne!(
        crate::textures::hole_of(1).unwrap(),
        crate::textures::hole_of(2).unwrap(),
        "each hole its own"
    );
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
    assert_eq!(names.len(), 28 + 10 + crate::trees::chunks().len());
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
    let arena =
        torque_interior_builder::build(&crate::interiors::arena(), &Default::default()).unwrap();
    for m in &arena.dif.details[0].materials {
        let file = format!("{m}.png");
        assert!(names.contains(&file.as_str()), "{file}");
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
fn aiming_raises_the_pistol_to_eye_level_in_front_of_the_face_along_the_aim() {
    let shape = Shape::for_server(shapes::figure());
    for (pos, pitch) in [
        (0.25, shapes::look_pitch(0.25)),
        (0.5, 0.0),
        (0.75, shapes::look_pitch(0.75)),
    ] {
        let threads = [("root", 0.0), ("aim", pos), ("head", pos)];
        let at = posed_with(&shape, &threads, "mount0", [0.0; 3]);
        let ahead = posed_with(&shape, &threads, "mount0", [0.0, 1.0, 0.0]);
        let d = [0, 1, 2].map(|k| ahead[k] - at[k]);
        let got = d[2].atan2(d[1]).to_degrees();
        assert!(
            (got - pitch).abs() < 3.0,
            "at {pos}: aims {got}, not {pitch}"
        );
        assert!(d[0].abs() < 0.05, "at {pos}: points straight ahead {d:?}");
    }
    let held = posed_with(&shape, &[("root", 0.0), ("hold", 0.5)], "mount0", [0.0; 3]);
    let aimed = posed_with(&shape, &[("root", 0.0), ("aim", 0.5)], "mount0", [0.0; 3]);
    assert!(aimed[2] > held[2] + 0.1, "higher: {aimed:?} {held:?}");
    assert!(
        aimed[0] < held[0] - 0.1,
        "nearer the middle: {aimed:?} {held:?}"
    );
    assert!(aimed[1] > 0.5, "in front: {aimed:?}");
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

#[test]
fn the_recoil_kicks_the_held_arm_up_and_back_and_settles() {
    let shape = shapes::figure();
    let shape = &shape.shape;
    let s = &shape.sequences[find_sequence(shape, b"light_recoil").unwrap()];
    assert_ne!(s.flags & 8, 0, "light_recoil is a blend sequence");
    assert_eq!(s.duration, shapes::RECOIL_SECONDS);
    // Over the arm held up, the hand rises at the kick's height and is back where it was at the end.
    let hand = |pos: f32| {
        posed_with(
            shape,
            &[("hold", 0.5), ("light_recoil", pos)],
            "elbow_r",
            [0.0, 0.0, -0.3],
        )
    };
    let held = posed(shape, "hold", 0.5, "elbow_r", [0.0, 0.0, -0.3]);
    let kicked = hand(0.04 / shapes::RECOIL_SECONDS);
    assert!(kicked[2] > held[2] + 0.02, "{kicked:?} over {held:?}");
    let end = hand(1.0);
    assert!(
        (0..3).all(|k| (end[k] - held[k]).abs() < 1e-4),
        "{end:?} back at {held:?}"
    );
}

/// The share of the builder's full counts the arena's check takes (`DIF_VERIFY_SHARE` sets it: 1 for the full counts).
fn verify_share() -> f64 {
    std::env::var("DIF_VERIFY_SHARE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01)
}

#[test]
fn the_arena_interior_passes_the_builders_checks() {
    use torque_interior_builder::verify::{Counts, run};
    let brushes = crate::interiors::arena();
    let r = run(
        &brushes,
        &torque_interior_builder::Options::default(),
        Counts::scaled(verify_share()),
        7,
    )
    .unwrap();
    println!("arena ({} brushes): {}", brushes.len(), r.line());
    assert!(r.clean(), "{}", r.line());
    assert_eq!(r.hulls, brushes.len());
}

#[test]
fn the_buggy_has_its_hubs_two_seats_and_a_body_hull() {
    let shape = Shape::for_server(crate::buggy::buggy());
    let facts = preload::facts(&shape, &mut free).unwrap();
    let node = |name: &str| {
        let n = shape.find_name(name.as_bytes()).unwrap();
        shape.nodes.iter().position(|x| x.name == n).unwrap() as i32
    };
    for i in 0..4 {
        node(&format!("hub{i}"));
    }
    assert_eq!(facts.mount_nodes[0], node("mount0"));
    assert_eq!(facts.mount_nodes[1], node("mount1"));
    assert_eq!(facts.mount_nodes[2], node("mount2"));
    assert_eq!(facts.eye_node, node("eye"));
    assert_eq!(facts.slots.hulls(), 1);
    assert_eq!(facts.slots.own_los(), 1);
    // The headlight between the roof's lamps, just ahead of them; their lit faces inside them until `lights_on` plays.
    use crate::buggy::{HEADLIGHT, LIGHTS_ON, ROOF_LAMP_DEPTH, ROOF_LAMP_Y, ROOF_LAMP_Z};
    assert!(HEADLIGHT[1] > ROOF_LAMP_Y && (HEADLIGHT[2] - ROOF_LAMP_Z).abs() < 1e-6);
    let rest = shape.default_translations[node("lamps_lit") as usize];
    assert!(
        rest[1] < ROOF_LAMP_Y && rest[1] > ROOF_LAMP_Y - ROOF_LAMP_DEPTH,
        "{rest:?}"
    );
    let out = posed(&shape, LIGHTS_ON, 0.0, "lamps_lit", [0.0; 3]);
    assert!(out[1] < ROOF_LAMP_Y, "{out:?}");
    let lit = posed(&shape, LIGHTS_ON, 1.0, "lamps_lit", [0.0; 3]);
    assert!(
        lit[1] > ROOF_LAMP_Y && lit[1] < ROOF_LAMP_Y + 0.02,
        "{lit:?}"
    );
}

#[test]
fn the_seated_hands_hold_a_round_wheel_before_the_chest_and_the_feet_rest_on_the_floor() {
    use crate::buggy::{SEAT_HIP, SEATS, grips, steering_wheel};
    let [l, r] = grips();
    // Level with each other, in front of the chest, at its height.
    assert!(
        (l[1] - r[1]).abs() < 1e-3 && (l[2] - r[2]).abs() < 1e-3,
        "{l:?} {r:?}"
    );
    assert!(l[1] > 0.3 && l[1] < 0.6, "{l:?}");
    assert!(l[2] > SEAT_HIP + 0.1 && l[2] < SEAT_HIP + 0.55, "{l:?}");
    let (_, radius) = steering_wheel();
    assert!(
        (0.13..0.23).contains(&radius),
        "the wheel's radius {radius}"
    );
    // The soles above the floor pan's top.
    let shape = Shape::for_server(shapes::figure());
    for knee in ["knee_l", "knee_r"] {
        let sole = posed(&shape, "sit", 0.0, knee, [0.0, 0.0, -shapes::SHIN]);
        assert!(SEATS[0][2] + sole[2] > 0.24, "{knee}: {sole:?}");
    }
}

#[test]
fn a_riding_figure_rests_its_hands_on_its_thighs() {
    let shape = Shape::for_server(shapes::figure());
    for elbow in ["elbow_l", "elbow_r"] {
        let hand = posed(&shape, "ride", 0.0, elbow, [0.0, 0.0, -shapes::GRIP]);
        assert!((0.2..0.45).contains(&hand[1]), "{elbow}: {hand:?}");
        assert!((1.02..1.25).contains(&hand[2]), "{elbow}: {hand:?}");
        assert!((0.12..0.28).contains(&hand[0].abs()), "{elbow}: {hand:?}");
    }
}

#[test]
fn the_steering_sequence_turns_the_wheel_about_its_column_clockwise_for_the_right() {
    use crate::buggy::{MAX_STEERING, WHEEL_LOCK, steering_wheel, wheel_turn};
    let shape = Shape::for_server(crate::buggy::buggy());
    let (center, radius) = steering_wheel();
    // The rim's top in the wheel's own space (the wheel faces back up the column; its up is across the column).
    let (s, c) = crate::buggy::COLUMN_DROP.to_radians().sin_cos();
    let up = [0.0, s, c];
    let top = up.map(|v| v * radius);
    let at = |place: f32| posed(&shape, "steering", place, "steering_wheel", top);
    let straight = at(0.5);
    assert!(
        (straight[0] - center[0]).abs() < 1e-3,
        "{straight:?} {center:?}"
    );
    // Full right: the thread at 0.5 - lock / 2, the top swung right by the wheel's lock.
    assert!((wheel_turn(MAX_STEERING) - WHEEL_LOCK).abs() < 1e-3);
    let right = at(0.5 - MAX_STEERING * 0.5);
    let left = at(0.5 + MAX_STEERING * 0.5);
    let side = |p: [f32; 3]| p[0] - center[0];
    let expect = radius * WHEEL_LOCK.to_radians().sin();
    assert!(
        (side(right) - expect).abs() < 0.02,
        "{right:?} for {expect}"
    );
    assert!((side(left) + expect).abs() < 0.02, "{left:?} for {expect}");
}

#[test]
fn the_seated_hands_stay_on_the_rim_as_the_steering_turns_the_wheel() {
    use crate::buggy::{COLUMN_DROP, MAX_STEERING, WHEEL_LOCK, grips};
    let shape = Shape::for_server(shapes::figure());
    let [l, r] = grips();
    let center = [0, 1, 2].map(|k| 0.5 * (l[k] + r[k]));
    let (s, c) = COLUMN_DROP.to_radians().sin_cos();
    let along = [0.0f32, c, -s];
    // Every place the steering takes, between the keys too: from full right to full left in small steps.
    for k in 0..=110 {
        let place = 0.5 - MAX_STEERING * 0.5 + MAX_STEERING * k as f32 / 110.0;
        let turn = ((1.0 - 2.0 * place) / MAX_STEERING * WHEEL_LOCK).to_radians();
        for (grip, elbow) in [(l, "elbow_l"), (r, "elbow_r")] {
            // The grip turned about the column through the wheel's middle (Rodrigues).
            let v = [0, 1, 2].map(|k| grip[k] - center[k]);
            let (sn, cs) = turn.sin_cos();
            let cr = [
                along[1] * v[2] - along[2] * v[1],
                along[2] * v[0] - along[0] * v[2],
                along[0] * v[1] - along[1] * v[0],
            ];
            let d: f32 = (0..3).map(|k| along[k] * v[k]).sum();
            let want =
                [0, 1, 2].map(|k| center[k] + v[k] * cs + cr[k] * sn + along[k] * d * (1.0 - cs));
            let hand = posed_with(
                &shape,
                &[("sit", 0.0), ("steering", place)],
                elbow,
                [0.0, 0.0, -shapes::GRIP],
            );
            let apart = (0..3)
                .map(|k| (hand[k] - want[k]).powi(2))
                .sum::<f32>()
                .sqrt();
            assert!(apart < 0.01, "{elbow} at {place}: {hand:?} for {want:?}");
        }
    }
}

#[test]
fn every_shape_through_a_glb_makes_the_same_dts() {
    for (name, model) in models() {
        let glb = crate::glb_of(&model).unwrap_or_else(|e| panic!("{name}: {e}"));
        let read =
            torque_import::gltf::read(&glb, &mut |_, _| None, &Default::default(), &mut free)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        let back = torque_import::dts::to_dts(&read).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            write_dts(&back) == write_dts(&model),
            "{name}: the .dts made through the .glb differs"
        );
        // The engine's own path: a datablock's `.glb` gives the same bytes.
        let native = torque_import::native::shape_file(b"kit/x.glb", glb, &mut free).unwrap();
        assert!(native == write_dts(&model), "{name}");
    }
}

#[test]
fn the_keep_and_the_yard_pass_the_builders_checks() {
    use torque_interior_builder::verify::{Counts, run};
    for (name, brushes) in [
        ("keep", crate::interiors::keep()),
        ("yard", crate::interiors::yard()),
    ] {
        let r = run(
            &brushes,
            &torque_interior_builder::Options::default(),
            Counts::scaled(verify_share()),
            7,
        )
        .unwrap();
        println!("{name} ({} brushes): {}", brushes.len(), r.line());
        assert!(r.clean(), "{name}: {}", r.line());
        assert_eq!(r.hulls, brushes.len());
    }
}

#[test]
fn the_track_is_a_closed_loop_round_the_field_that_never_comes_near_itself() {
    let t = crate::track::track();
    let p = t.points();
    assert!((1800.0..2400.0).contains(&t.length()), "{}", t.length());
    // Closed: the last point runs into the first.
    let (a, b) = (p[0], p[p.len() - 1]);
    assert!((a[0] - b[0]).hypot(a[1] - b[1]) < 2.0 * crate::track::STEP);
    // Clear of the start field by its width and more, and of itself wherever it is far along the loop.
    let r = crate::terrain::FIELD_RADIUS + crate::track::WIDTH;
    assert!(p.iter().all(|q| q[0].hypot(q[1]) > r));
    let step = 4;
    for i in (0..p.len()).step_by(step) {
        let near = t.nearest(p[i][0], p[i][1]).unwrap();
        assert!(near.distance < 0.01, "{i}: on the centerline");
        for j in (0..p.len()).step_by(step) {
            let along = (i as f32 - j as f32).abs() * crate::track::STEP;
            let along = along.min(t.length() - along);
            if along > 100.0 {
                let d = (p[i][0] - p[j][0]).hypot(p[i][1] - p[j][1]);
                assert!(d > 50.0, "{i} {j}: {d}");
            }
        }
    }
}

#[test]
fn the_dirt_covers_the_track_and_leaves_the_field_green() {
    use crate::track::{HALF_WIDTH, dirt_share, track};
    let t = track();
    for k in 0..200 {
        let (p, h) = t.at(t.length() * k as f32 / 200.0);
        let across = [h[1], -h[0]];
        let at = |d: f32| dirt_share(p[0] + across[0] * d, p[1] + across[1] * d);
        assert!(at(0.0) > 0.9, "{k}: the middle");
        assert!((at(HALF_WIDTH) - 0.5).abs() < 0.05, "{k}: half at the edge");
        assert_eq!(at(HALF_WIDTH + 6.0), 0.0, "{k}: none beside it");
    }
    assert_eq!(dirt_share(0.0, 0.0), 0.0);
    // The terrain's two weights sum to 255 everywhere.
    let ter = crate::terrain::hills();
    let n = crate::terrain::SIDE * crate::terrain::SIDE;
    let alpha = &ter[ter.len() - 2 * n..];
    assert!((0..n).all(|i| u16::from(alpha[i]) + u16::from(alpha[n + i]) == 255));
    assert!(alpha[n..].contains(&255) && alpha[n..].iter().filter(|&&a| a > 0).count() < n / 20);
}

#[test]
fn the_ground_is_shaped_along_the_track_and_left_as_it_was_elsewhere() {
    use crate::terrain::{height, raw_height};
    use crate::track::{BANK_MAX, track};
    assert_eq!(height(0.0, 0.0), raw_height(0.0, 0.0));
    assert_eq!(height(0.0, 95.0), raw_height(0.0, 95.0), "the hill's top");
    assert_eq!(height(600.0, 600.0), raw_height(600.0, 600.0));
    let t = track();
    let (mut lift, mut bank) = ([0.0f32; 2], 0.0f32);
    for k in 0..400 {
        let s = t.length() * k as f32 / 400.0;
        let (p, h) = t.at(s);
        let across = [h[1], -h[0]];
        let z = |d: f32| height(p[0] + across[0] * d, p[1] + across[1] * d);
        // Across the track a straight line, level or banked no steeper than the cap.
        let (left, right) = (z(0.0) - z(-4.0), z(4.0) - z(0.0));
        assert!((left - right).abs() < 0.05, "{k}: {left} {right}");
        assert!(right.abs() <= 4.0 * BANK_MAX + 0.02, "{k}: {right}");
        lift = [lift[0].min(t.lift(s)), lift[1].max(t.lift(s))];
        bank = bank.max(t.bank_at(s).abs());
    }
    assert!(lift[0] < -3.9 && lift[1] > 6.0, "{lift:?}");
    assert!(bank > 0.06, "{bank}");
    // The start straight and the way over the round hill keep the hills' shape (the track leans with them, at most
    // LEAN_MAX).
    for s in [0.0, 60.0, 150.0, t.length() - 20.0] {
        assert_eq!(t.lift(s), 0.0, "{s}");
        assert!(t.bank_at(s).abs() <= crate::track::LEAN_MAX, "{s}");
    }
}

#[test]
fn the_track_is_smooth_enough_to_drive_and_its_crests_lift_the_buggy() {
    use crate::terrain::{SIDE, SQUARE, height, sample_xy};
    use crate::track::{HALF_WIDTH, track};
    let t = track();
    // The terrain as the engine has it: the samples' heights, between them on the triangles' planes (bilinear is close).
    let samples: Vec<f32> = (0..SIDE * SIDE)
        .map(|k| {
            let [x, y] = sample_xy(k % SIDE, k / SIDE);
            height(x, y)
        })
        .collect();
    let origin = sample_xy(0, 0);
    let ground = |x: f32, y: f32| {
        let (u, v) = ((x - origin[0]) / SQUARE, (y - origin[1]) / SQUARE);
        let (i, j) = (u.floor() as usize, v.floor() as usize);
        let (a, b) = (u - i as f32, v - j as f32);
        let h = |i: usize, j: usize| samples[i + j * SIDE];
        (h(i, j) * (1.0 - a) + h(i + 1, j) * a) * (1.0 - b)
            + (h(i, j + 1) * (1.0 - a) + h(i + 1, j + 1) * a) * b
    };
    // The grade along the centerline, over 4 m, and the change of grade (a lip) between one 4 m and the next.
    let (mut steepest, mut lip, mut crest) = (0.0f32, 0.0f32, 0.0f32);
    let n = (t.length() / 4.0) as usize;
    let z: Vec<f32> = (0..n)
        .map(|k| {
            let (p, _) = t.at(k as f32 * 4.0);
            ground(p[0], p[1])
        })
        .collect();
    for k in 0..n {
        let g0 = (z[(k + 1) % n] - z[k]) / 4.0;
        let g1 = (z[(k + 2) % n] - z[(k + 1) % n]) / 4.0;
        steepest = steepest.max(g0.abs());
        lip = lip.max((g1 - g0).abs());
        crest = crest.max(g0 - g1);
    }
    println!(
        "track: steepest grade {steepest:.3}, largest change of grade over 4 m {lip:.3}, sharpest crest {crest:.3}"
    );
    assert!(steepest < 0.3, "{steepest}");
    assert!(lip < 0.15, "{lip}");
    // A crest bends the road down by a few hundredths of a grade every 4 m: at 20 m/s that is more than gravity holds.
    assert!(crest > 0.03, "{crest}");
    // Beside the shaped track, out to the landforms' reach, no seam or step between neighboring samples: no sharper bend
    // than the track's shoulders have always had (the round hill's steep side, unshaped, bends more).
    let mut worst = 0.0f32;
    for j in 1..SIDE - 1 {
        for i in 1..SIDE - 1 {
            let [x, y] = sample_xy(i, j);
            if t.nearest(x, y)
                .is_none_or(|n| n.distance < HALF_WIDTH || t.shaped(n.along) == 0.0)
            {
                continue;
            }
            let h = |i: usize, j: usize| samples[i + j * SIDE];
            let bend = (h(i + 1, j) - 2.0 * h(i, j) + h(i - 1, j))
                .abs()
                .max((h(i, j + 1) - 2.0 * h(i, j) + h(i, j - 1)).abs());
            worst = worst.max(bend);
        }
    }
    println!("beside the shaped track: largest bend between samples {worst:.2} m");
    assert!(worst < 3.5, "{worst}");
}

#[test]
fn the_dirts_uv_runs_along_the_track_and_over_the_loops_start() {
    use crate::track::{DIRT_ACROSS, dirt_uv, track};
    let t = track();
    let (p, h) = t.at(10.0);
    let q = t.at(14.0).0;
    let uv = dirt_uv(&[
        [p[0], p[1], 0.0],
        [q[0], q[1], 0.0],
        [p[0] + h[1], p[1] - h[0], 0.0],
    ]);
    assert!((uv[0][1] - 0.5).abs() < 0.01, "the middle across");
    assert!(uv[1][0] > uv[0][0], "on along it");
    assert!(
        (uv[2][1] - (0.5 + 1.0 / DIRT_ACROSS)).abs() < 0.01,
        "a meter to the right"
    );
    // A triangle over the loop's start keeps its coordinates together.
    let (a, b) = (t.at(t.length() - 2.0).0, t.at(2.0).0);
    let uv = dirt_uv(&[
        [a[0], a[1], 0.0],
        [b[0], b[1], 0.0],
        [a[0], a[1] + 1.0, 0.0],
    ]);
    assert!((uv[1][0] - uv[0][0]).abs() < 1.0, "{uv:?}");
}

#[test]
fn the_forest_stands_in_dense_patches_off_the_track_the_field_and_the_way_north() {
    use crate::track::{HALF_WIDTH, track};
    use crate::trees::{SPACING, TRACK_CLEAR, allowed, chunks, forest};
    let f = forest();
    println!("forest: {} trees in {} squares", f.len(), chunks().len());
    assert!((2000..6000).contains(&f.len()), "{}", f.len());
    let t = track();
    for tree in f {
        let [x, y, z] = tree.foot;
        assert!(allowed(x, y), "{:?}", tree.foot);
        assert_eq!(z, crate::terrain::height(x, y));
        assert!(
            t.nearest(x, y)
                .is_none_or(|n| n.distance >= HALF_WIDTH + TRACK_CLEAR)
        );
    }
    // Spaced: no two feet nearer than a core's spacing (sorted along x, so only near neighbors are compared).
    let mut feet: Vec<[f32; 3]> = f.iter().map(|t| t.foot).collect();
    feet.sort_by(|a, b| a[0].total_cmp(&b[0]));
    for (i, a) in feet.iter().enumerate() {
        for b in feet[i + 1..].iter().take_while(|b| b[0] - a[0] < SPACING.0) {
            assert!((a[0] - b[0]).hypot(a[1] - b[1]) >= SPACING.0 - 1e-3);
        }
    }
    // Dense cores and open meadows: the trees within 20 m of the densest spot, and the share of 20 m squares between 40 and
    // 120 m off the track with no tree at all.
    let crowd = |x: f32, y: f32, r: f32| {
        feet.iter()
            .filter(|b| (b[0] - x).hypot(b[1] - y) < r)
            .count()
    };
    let densest = feet
        .iter()
        .step_by(7)
        .map(|a| crowd(a[0], a[1], 20.0))
        .max()
        .unwrap();
    let (mut squares, mut open) = (0, 0);
    for j in -25..20 {
        for i in -25..25 {
            let (x, y) = (i as f32 * 20.0 + 10.0, j as f32 * 20.0 + 10.0);
            let near = t.nearest(x, y).map(|n| n.distance);
            if near.is_some_and(|d| d >= 40.0) {
                squares += 1;
                open += usize::from(crowd(x, y, 10.0) == 0);
            }
        }
    }
    println!(
        "densest: {densest} trees within 20 m; {open} of {squares} squares near the track open"
    );
    assert!(densest >= 30, "{densest}");
    assert!(
        open * 4 > squares && open * 4 < squares * 3,
        "{open} {squares}"
    );
    // Every tree in one square, each square its own file.
    let mut seen: Vec<usize> = chunks()
        .iter()
        .flat_map(|c| c.trees.iter().copied())
        .collect();
    seen.sort_unstable();
    assert!(seen.iter().copied().eq(0..f.len()));
    assert!(!allowed(0.0, 0.0) && !allowed(0.0, 100.0));
    let p = t.points()[100];
    assert!(!allowed(p[0], p[1]));
}

#[test]
fn only_trunks_reach_down_to_a_buggy_and_the_tall_conifers_tower() {
    use crate::trees::{BARK, BARK_BIRCH, BARK_RED, CLEARANCE, Kind, tree};
    for kind in Kind::ALL {
        let mut tallest = 0.0f64;
        for seed in 0..24u64 {
            for size in [0.6, 0.85, 1.0, 1.15] {
                let brushes = tree(kind, [0.0, 0.0, 0.0], size, seed);
                let mut trunks = 0;
                for b in &brushes {
                    let (lo, hi) = b.bounds().unwrap();
                    tallest = tallest.max(hi[2]);
                    let material = b.faces[0].material.as_str();
                    if [BARK, BARK_RED, BARK_BIRCH].contains(&material) {
                        trunks += 1;
                        // Not wider than a tall trunk's flared foot.
                        assert!(hi[0] - lo[0] < 5.0, "{kind:?}");
                    } else {
                        // A canopy's lowest reach (a leaning tree's dips a little).
                        assert!(
                            lo[2] > CLEARANCE - 0.35,
                            "{kind:?} {seed} {size}: {}",
                            lo[2]
                        );
                    }
                }
                assert!((1..=2).contains(&trunks), "{kind:?}");
            }
        }
        let grown = kind.height();
        println!("{kind:?}: tallest {tallest:.1} m");
        assert!(
            tallest > grown && tallest < grown * 1.5,
            "{kind:?}: {tallest}"
        );
    }
    assert!(
        Kind::Redwood.height() > 15.0 && Kind::Fir.height() > 12.0,
        "several buggies tall"
    );
}

#[test]
fn the_forest_builds_and_stays_within_its_triangle_budget() {
    let (mut triangles, mut most) = (0, 0);
    let mut names: Vec<String> = Vec::new();
    for k in 0..crate::trees::chunks().len() {
        let built = torque_interior_builder::build(
            &crate::trees::chunk_brushes(k),
            &crate::trees::options(),
        )
        .unwrap();
        triangles += built
            .surfaces
            .iter()
            .map(|(_, p)| p.len() - 2)
            .sum::<usize>();
        let d = &built.dif.details[0];
        most = most.max(d.bsp_nodes.len());
        names.extend(d.materials.iter().cloned());
    }
    println!("forest: {triangles} triangles, at most {most} BSP nodes an interior");
    assert!(triangles < 400_000, "{triangles}");
    names.sort();
    names.dedup();
    let kit: Vec<_> = crate::all().unwrap().iter().map(|a| a.name).collect();
    for m in names {
        assert!(kit.contains(&format!("{m}.png").as_str()), "{m}");
    }
}

#[test]
fn the_tree_lineup_passes_the_builders_checks() {
    use torque_interior_builder::verify::{Counts, run};
    let brushes = crate::trees::lineup();
    let r = run(
        &brushes,
        &crate::trees::options(),
        Counts::scaled(verify_share()),
        11,
    )
    .unwrap();
    println!("tree lineup ({} brushes): {}", brushes.len(), r.line());
    assert!(r.clean(), "{}", r.line());
    assert_eq!(r.hulls, brushes.len());
}
