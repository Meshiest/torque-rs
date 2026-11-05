use crate::math::{self, Mat4};
use crate::naming::{MeshDetail, Role, Volume, is_named_point, mesh_detail, role, trailing_number};
use crate::{Animation, Mesh, Model, Node, Primitive, Skin};

fn near(a: &[f32], b: &[f32], tol: f32) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
}

#[test]
fn trs_decomposes_back_and_inverts() {
    let q = math::normalize_quat([0.3, -0.2, 0.5, 0.8]);
    let m = math::trs([1.0, 2.0, -3.0], q, [2.0, 0.5, 1.5]);
    let (t, q2, s) = math::decompose(&m);
    assert!(near(&t, &[1.0, 2.0, -3.0], 1e-6));
    assert!(near(&s, &[2.0, 0.5, 1.5], 1e-5));
    let same = near(&q2, &q, 1e-5) || near(&q2.map(|c| -c), &q, 1e-5);
    assert!(same, "{q2:?} vs {q:?}");
    let id = math::mul(&m, &math::inverse(&m));
    assert!(near(&id, &math::IDENTITY, 1e-5), "{id:?}");
}

#[test]
fn a_quarter_turn_about_z_takes_x_to_y() {
    let h = std::f32::consts::FRAC_1_SQRT_2;
    let m = math::rotation([0.0, 0.0, h, h]);
    assert!(near(
        &math::vector(&m, [1.0, 0.0, 0.0]),
        &[0.0, 1.0, 0.0],
        1e-6
    ));
    let half = math::slerp(math::NO_TURN, [0.0, 0.0, h, h], 0.5);
    let eighth = (std::f32::consts::PI / 8.0).sin();
    assert!(near(
        &half,
        &[0.0, 0.0, eighth, (std::f32::consts::PI / 8.0).cos()],
        1e-6
    ));
}

fn two_nodes() -> Model {
    let mut m = Model {
        nodes: vec![
            Node::new("root", None, [0.0, 0.0, 1.0]),
            Node::new("arm", Some(0), [1.0, 0.0, 0.0]),
        ],
        ..Model::default()
    };
    m.nodes[0].rotation = [
        0.0,
        0.0,
        std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
    ];
    m.meshes.push(Mesh {
        name: "box".into(),
        node: 1,
        positions: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        primitives: vec![Primitive {
            material: None,
            indices: vec![0, 1, 2],
        }],
        ..Mesh::default()
    });
    m
}

#[test]
fn rest_positions_follow_the_tree() {
    let m = two_nodes();
    m.check().unwrap();
    let p = m.rest_positions(0);
    // The arm sits at (0, 1, 1) turned a quarter about z: its x is the model's y.
    assert!(near(&p[0], &[0.0, 2.0, 1.0], 1e-6), "{p:?}");
}

#[test]
fn a_skin_places_vertices_by_its_joints() {
    let mut m = two_nodes();
    let world = m.rest_world();
    let mesh = &mut m.meshes[0];
    mesh.skin = Some(Skin {
        joints: vec![1],
        inverse_binds: vec![math::inverse(&world[1])],
    });
    mesh.positions = vec![[5.0, 6.0, 7.0]; 3];
    mesh.joints = vec![[0; 4]; 3];
    mesh.weights = vec![[1.0, 0.0, 0.0, 0.0]; 3];
    m.check().unwrap();
    // At rest the joint's transform cancels its inverse bind: the vertices stay where they are.
    assert!(near(&m.rest_positions(0)[0], &[5.0, 6.0, 7.0], 1e-5));
}

#[test]
fn check_refuses_cycles_and_bad_indices() {
    let mut m = two_nodes();
    m.nodes[0].parent = Some(1);
    assert!(m.check().is_err());
    // The world walk ends on a cycle too.
    assert_eq!(m.rest_world().len(), 2);
    let mut m = two_nodes();
    m.meshes[0].primitives[0].indices = vec![0, 1, 3];
    assert!(m.check().is_err());
    let mut m = two_nodes();
    m.animations.push(Animation {
        keys: 2,
        tracks: vec![crate::Track {
            node: 0,
            rotations: vec![math::NO_TURN],
            ..Default::default()
        }],
        ..Default::default()
    });
    assert!(m.check().is_err());
}

#[test]
fn key_times_span_the_duration() {
    let mut a = Animation {
        duration: 2.0,
        keys: 5,
        ..Default::default()
    };
    assert_eq!(a.key_time(4), 2.0);
    a.cyclic = true;
    assert_eq!(a.key_time(4), 1.6);
    a.keys = 1;
    a.cyclic = false;
    assert_eq!(a.key_time(0), 0.0);
}

#[test]
fn names_say_what_a_node_is() {
    assert_eq!(trailing_number("box 32"), ("box", Some(32)));
    assert_eq!(trailing_number("Col-1"), ("Col", Some(-1)));
    assert_eq!(trailing_number("arm_l"), ("arm_l", None));
    assert_eq!(trailing_number("leg_2"), ("leg", Some(-2)));
    let d = |o: &str, d: &str, size| MeshDetail {
        object: o.into(),
        detail: d.into(),
        size,
    };
    assert_eq!(mesh_detail("box"), d("box", "detail2", 2));
    assert_eq!(mesh_detail("box8"), d("box", "detail8", 8));
    assert_eq!(mesh_detail("Col-1"), d("Col", "Collision-1", -1));
    assert_eq!(mesh_detail("Collision3"), d("Collision", "Collision-3", -3));
    assert_eq!(mesh_detail("LOSCol-9"), d("LOSCol", "LOS-9", -9));
    assert_eq!(role("Col-1"), Role::Collision);
    assert_eq!(role("hull_wall"), Role::Collision);
    assert_eq!(role("col_floor"), Role::Collision);
    assert_eq!(role("LOS-9"), Role::Los);
    assert_eq!(role("water_pool"), Role::Volume(Volume::Water));
    assert_eq!(role("physical_zone2"), Role::Volume(Volume::PhysicalZone));
    assert_eq!(role("trigger"), Role::Volume(Volume::Trigger));
    assert_eq!(role("color"), Role::Visible);
    assert!(is_named_point("mount31") && is_named_point("Eye") && !is_named_point("mount32"));
}

#[test]
fn pose_local_takes_keys_and_rest_elsewhere() {
    let m = two_nodes();
    let a = Animation {
        duration: 1.0,
        keys: 2,
        tracks: vec![crate::Track {
            node: 1,
            translations: vec![[0.0; 3], [3.0, 0.0, 0.0]],
            ..Default::default()
        }],
        ..Default::default()
    };
    let local: Vec<Mat4> = m.pose_local(&a, 1);
    assert_eq!(local[1][3], 3.0);
    assert!(near(&local[0], &m.nodes[0].local(), 0.0));
}
