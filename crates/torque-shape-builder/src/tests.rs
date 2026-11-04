use torque_shapes::math::{mul_v, quat_matrix, quat16};
use torque_shapes::shape::Model;
use torque_shapes::{bounds, dts, write::write_dts};

use crate::{
    FULL_UV, Face, MeshData, NO_TURN, SequenceDef, ShapeBuilder, Track, material_flags, turn,
};

fn free(_: u64) -> bool {
    true
}

fn near(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() < 1e-3)
}

/// Two nodes, a box and a prism on them, a hull, two details and one looping sequence.
fn sample() -> Model {
    let mut b = ShapeBuilder::new();
    let root = b.node("root", None, [0.0; 3]);
    let arm = b.node_turned(
        "arm",
        Some(root),
        [0.0, 0.0, 1.0],
        turn([1.0, 0.0, 0.0], 30.0),
    );
    let drawn = b.detail("detail1", 1.0);
    let collision = b.detail("Collision-1", -1.0);
    let tex = b.material("tex", material_flags::S_WRAP | material_flags::T_WRAP);
    let mut body = MeshData::new(Some(tex));
    body.add_box([-0.5, -0.25, 0.0], [0.5, 0.25, 1.0], |_| FULL_UV);
    b.object("body", root, drawn, body);
    let mut limb = MeshData::new(Some(tex));
    limb.add_prism(
        &[[-0.1, -0.5], [0.1, -0.5], [0.1, 0.0], [-0.1, 0.0]],
        0.5,
        0.7,
        FULL_UV,
        FULL_UV,
    );
    b.object("limb", arm, drawn, limb);
    b.object(
        "Col-1",
        root,
        collision,
        MeshData::hull_box([-0.5, -0.25, 0.0], [0.5, 0.25, 1.0]),
    );
    b.sequence(SequenceDef {
        name: "swing",
        duration: 1.0,
        cyclic: true,
        priority: 1,
        keyframes: 3,
        rotations: vec![Track {
            node: arm,
            keys: vec![NO_TURN, turn([1.0, 0.0, 0.0], 45.0)],
        }],
        translations: Vec::new(),
        ground: Vec::new(),
    });
    b.build()
}

#[test]
fn a_built_shape_parses_back_to_the_model_it_was_built_from() {
    let model = sample();
    let bytes = write_dts(&model);
    let back = dts::parse(&bytes, &mut free).unwrap();
    assert_eq!(back.shape, model.shape);
    assert_eq!(back.render, model.render);
    assert_eq!(back.file.materials, model.file.materials);
    assert_eq!(write_dts(&back), bytes);
}

#[test]
fn every_bound_is_the_one_the_vertices_give() {
    let report = bounds::check(&sample(), &mut free).unwrap();
    assert!(report.all_equal(), "{report:?}");
    assert_eq!(report.detail0_inside, Some(true));
}

#[test]
fn every_triangle_is_clockwise_seen_from_its_front() {
    let model = sample();
    for (mesh, render) in model.shape.meshes.iter().zip(&model.render) {
        let (Some(mesh), Some(render)) = (mesh, render) else {
            continue;
        };
        let p = &mesh.positions;
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
            // Outward: from the box's center to the triangle; the normals the renderer lights with agree.
            let out = [0, 1, 2].map(|k| (a[k] + b[k] + c[k]) / 3.0 - mesh.center[k]);
            let dot = |v: [f32; 3]| v[0] * cross[0] + v[1] * cross[1] + v[2] * cross[2];
            assert!(dot(out) < 0.0, "{t:?}");
            if render.normals.len() == p.len() && mesh.num_verts == 24 {
                assert!(dot(render.normals[usize::from(t[0])]) < 0.0, "{t:?}");
            }
        }
    }
}

#[test]
fn a_box_has_six_faces_of_two_triangles() {
    let mut m = MeshData::new(Some(0));
    m.add_box([0.0; 3], [1.0; 3], |_| FULL_UV);
    assert_eq!((m.positions.len(), m.triangles()), (24, 12));
    assert_eq!(Face::ALL.len(), 6);
    assert_eq!(MeshData::hull_box([0.0; 3], [1.0; 3]).triangles(), 12);
}

#[test]
fn nodes_are_linked_as_the_shape_init_links_them() {
    let shape = sample().shape;
    assert_eq!(shape.nodes[0].parent, -1);
    assert_eq!(shape.nodes[1].parent, 0);
    assert_eq!(shape.nodes[0].first_child, 1);
    assert_eq!(shape.nodes[1].next_sibling, -1);
    assert_eq!(shape.sequences.len(), 1);
    assert_eq!(shape.sequences[0].num_keyframes, 3);
    // A short track holds its last key.
    assert_eq!(shape.node_rotations.len(), 3);
    assert_eq!(shape.node_rotations[1], shape.node_rotations[2]);
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
    assert_eq!(turn([1.0, 0.0, 0.0], 0.0), NO_TURN);
}

#[test]
fn a_blend_sequence_is_flagged_and_adds_its_turn_to_the_other_threads_pose() {
    use torque_shapes::instance::{FLAG_BLEND, ShapeInstance};
    let mut b = ShapeBuilder::new();
    let root = b.node("root", None, [0.0; 3]);
    let arm = b.node("arm", Some(root), [0.0, 0.0, 1.0]);
    let drawn = b.detail("detail1", 1.0);
    let mut mesh = MeshData::new(None);
    mesh.add_box([-0.1; 3], [0.1; 3], |_| FULL_UV);
    b.object("limb", arm, drawn, mesh);
    let x = [1.0, 0.0, 0.0];
    let def = |name, degrees| SequenceDef {
        name,
        duration: 1.0,
        cyclic: false,
        priority: 0,
        keyframes: 1,
        rotations: vec![Track {
            node: arm,
            keys: vec![turn(x, degrees)],
        }],
        translations: Vec::new(),
        ground: Vec::new(),
    };
    b.sequence(def("raise", 30.0));
    b.blend_sequence(def("nod", 20.0));
    let model = b.build();
    let shape = &model.shape;
    assert_eq!(shape.sequences[0].flags & FLAG_BLEND, 0);
    assert_ne!(shape.sequences[1].flags & FLAG_BLEND, 0);
    let back = dts::parse(&write_dts(&model), &mut free).unwrap();
    assert_eq!(back.shape.sequences, shape.sequences);

    let mut instance = ShapeInstance::new(shape, &mut free).unwrap();
    for seq in 0..2 {
        let t = instance.create_thread(shape).unwrap();
        instance.set_sequence(shape, t, seq, 0.0);
    }
    let _ = instance.animate(shape, 0, None, &mut free);
    // The arm's +y turned 30 by the first thread and 20 more by the blend: 50 degrees up from y toward z.
    let m = instance.node_transform(arm).unwrap();
    let y = mul_v(m, [0.0, 1.0, 0.0]);
    let a = 50f32.to_radians();
    assert!(near(y, [0.0, a.cos(), a.sin()]), "{y:?}");
}
