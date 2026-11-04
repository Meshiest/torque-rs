use torque_images::{Format, Kind};
use torque_shapes::instance::{ShapeInstance, find_sequence};
use torque_shapes::math::{mul_p, mul_v, quat_matrix, quat16};
use torque_shapes::shape::{Model, Shape};
use torque_shapes::{bounds, dts, preload, write::write_dts};

use crate::builder::turn;
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
fn a_turn_is_right_handed_through_torques_matrix() {
    let about = |axis, v| mul_v(&quat_matrix(quat16(turn(axis, 90.0))), v);
    assert!(near(
        about([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        [0.0, 0.0, 1.0]
    ));
    assert!(near(
        about([0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        [0.0, 1.0, 0.0]
    ));
    // A hanging limb turned forward about x reaches forward.
    assert!(near(
        about([1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        [0.0, 1.0, 0.0]
    ));
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
    let foot = [0.0, 0.0, -0.95];
    for (seq, reach) in [("walk", 0.35), ("run", 0.6)] {
        let (l, r) = (
            posed(&shape, seq, 0.25, "leg_l", foot),
            posed(&shape, seq, 0.25, "leg_r", foot),
        );
        assert!(l[1] > reach && r[1] < -reach, "{seq}: {l:?} {r:?}");
        let (l, r) = (
            posed(&shape, seq, 0.75, "leg_l", foot),
            posed(&shape, seq, 0.75, "leg_r", foot),
        );
        assert!(l[1] < -reach && r[1] > reach, "{seq}: {l:?} {r:?}");
        // The arm swings against its own side's leg.
        let hand = posed(&shape, seq, 0.25, "arm_l", [0.0, 0.0, -0.66]);
        assert!(hand[1] < -0.1, "{seq}: {hand:?}");
    }
    let head = posed(&shape, "run", 0.0, "head", [0.0; 3]);
    assert!(head[1] > 0.1, "the run leans forward: {head:?}");
    let still = posed(&shape, "walk", 0.0, "head", [0.0; 3]);
    assert!(near(still, [0.0, 0.0, 1.65]), "{still:?}");
}

#[test]
fn the_jump_raises_the_arms_and_holds_its_last_key() {
    let shape = Shape::for_server(shapes::figure());
    let hand = posed(&shape, "jump", 1.0, "arm_r", [0.0, 0.0, -0.66]);
    let shoulder = posed(&shape, "jump", 1.0, "arm_r", [0.0; 3]);
    assert!(hand[2] > shoulder[2] + 0.4, "{hand:?} {shoulder:?}");
    let seq = |name: &str| &shape.sequences[find_sequence(&shape, name.as_bytes()).unwrap()];
    assert_eq!(seq("jump").flags & 16, 0, "played once");
    for name in ["root", "idle", "walk", "run"] {
        assert_eq!(seq(name).flags & 16, 16, "{name} loops");
    }
    assert_eq!(seq("root").base_rotation + 8, seq("idle").base_rotation);
    assert_eq!(seq("root").rotation_matters, seq("idle").rotation_matters);
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
    assert!(near(back, [0.0, -0.06, 0.075]), "{back:?}");
    let home = posed(&shape, "fire", 1.0, "slide", [0.0; 3]);
    assert!(near(home, [0.0, 0.0, 0.075]), "{home:?}");
    let muzzle = posed(&shape, "fire", 0.0, "muzzlePoint", [0.0; 3]);
    assert!(muzzle[1] > 0.24, "past the barrel's end: {muzzle:?}");
}

#[test]
fn the_textures_decode_with_the_engines_reader() {
    let cases: [(&str, Vec<u8>, u32); 4] = [
        ("grid", crate::textures::grid().unwrap(), 64),
        ("figure", crate::textures::figure().unwrap(), 64),
        ("pistol", crate::textures::pistol().unwrap(), 32),
        ("projectile", crate::textures::projectile().unwrap(), 16),
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
    assert_eq!(names.len(), 8);
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
