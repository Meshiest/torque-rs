use super::*;

fn tiny() -> Shape {
    Shape {
        names: vec![b"Collision-1".to_vec(), b"Root".to_vec()],
        ..Shape::empty()
    }
}

fn mesh(positions: Arc<[[f32; 3]]>) -> Mesh {
    Mesh {
        kind: MeshKind::Standard,
        parent: -1,
        num_frames: 1,
        num_mat_frames: 1,
        verts_per_frame: positions.len() as i32,
        flags: 0,
        bounds: [0.0; 6],
        center: [0.0; 3],
        radius: 1,
        num_verts: positions.len(),
        num_tverts: 0,
        positions,
        primitives: Vec::new(),
        indices: Vec::new(),
    }
}

#[test]
fn mesh_kinds_round_trip_and_unknown_words_are_none() {
    for kind in [
        MeshKind::Standard,
        MeshKind::Skin,
        MeshKind::Decal,
        MeshKind::Sorted,
        MeshKind::Null,
    ] {
        assert_eq!(MeshKind::from_word(kind.word()), Some(kind));
    }
    assert_eq!(MeshKind::from_word(5), None);
    assert_eq!(MeshKind::from_word(u32::MAX), None);
}

#[test]
fn find_name_ignores_case_and_returns_the_first() {
    let mut shape = tiny();
    shape.names.push(b"COLLISION-1".to_vec());
    assert_eq!(shape.find_name(b"collision-1"), Some(0));
    assert_eq!(shape.find_name(b"root"), Some(1));
    assert_eq!(shape.find_name(b"missing"), None);
}

#[test]
fn for_server_keeps_the_shape_and_drops_the_render_data() {
    let positions: Arc<[[f32; 3]]> = Arc::from(vec![[1.0, 2.0, 3.0]; 4]);
    let mut shape = tiny();
    shape.meshes.push(Some(mesh(Arc::clone(&positions))));
    let render = MeshRender {
        tverts: Arc::from(vec![[0.0f32; 2]; 4]),
        normals: Arc::from(vec![[0.0f32, 0.0, 1.0]; 4]),
        ..MeshRender::default()
    };
    let model = Model {
        shape: shape.clone(),
        render: vec![Some(render.clone())],
        file: FileExtras::default(),
    };
    let tverts = Arc::clone(&render.tverts);
    assert_eq!(Arc::strong_count(&tverts), 3);
    let kept = Shape::for_server(model);
    // The model's copy of the render data is gone; the shape is the one that went in.
    assert_eq!(Arc::strong_count(&tverts), 2);
    assert_eq!(kept, shape);
    assert_eq!(kept.vertex_total(), 4);
}

#[test]
fn a_child_mesh_reads_the_first_vertices_of_the_shared_array() {
    let positions: Arc<[[f32; 3]]> = Arc::from(vec![[0.0; 3]; 8]);
    let mut child = mesh(Arc::clone(&positions));
    child.num_verts = 3;
    assert_eq!(child.verts().len(), 3);
    child.num_verts = 99;
    assert_eq!(child.verts().len(), 8);
}

#[test]
fn int_sets_test_and_set_within_six_words() {
    let mut set = IntSet::default();
    assert!(!set.test(5));
    assert!(set.set(5));
    assert!(set.set(70));
    assert!(set.test(5) && set.test(70) && !set.test(6));
    assert_eq!(set.words.len(), 3);
    assert!(set.set(191));
    assert!(!set.set(192));
    assert!(!set.test(5000));
}

/// A small valid shape: one node, one object with one mesh of four vertices and a triangle, one sequence that rotates the node.
fn valid_shape() -> Shape {
    let mut s = tiny();
    s.nodes = vec![Node {
        name: 1,
        parent: -1,
        first_object: -1,
        first_child: -1,
        next_sibling: -1,
    }];
    s.objects = vec![Object {
        name: 1,
        num_meshes: 1,
        start_mesh_index: 0,
        node: 0,
        next_sibling: -1,
        first_decal: -1,
    }];
    let mut m = mesh(Arc::from(vec![[0.0; 3]; 4]));
    m.primitives = vec![Primitive {
        start: 0,
        num_elements: 3,
        material: 0,
    }];
    m.indices = vec![0, 1, 3];
    s.meshes = vec![Some(m)];
    s.node_rotations = vec![[0; 4]; 2];
    s.node_translations = vec![[0.0; 3]; 2];
    s.triggers = vec![Trigger { state: 1, pos: 0.5 }];
    let mut q = crate::dsq::parse(&crate::dsq::tests_support::one_sequence(), &mut |_| true)
        .expect("parses")
        .sequences
        .remove(0)
        .sequence;
    q.name = 1;
    q.flags = 0;
    q.num_keyframes = 2;
    q.base_rotation = 0;
    q.base_translation = 0;
    q.first_ground_frame = 0;
    q.num_ground_frames = 0;
    q.first_trigger = 0;
    q.num_triggers = 1;
    q.base_object_state = 0;
    q.base_decal_state = 0;
    q.rotation_matters = IntSet {
        first: 0,
        words: vec![1],
    };
    q.translation_matters = IntSet::default();
    q.scale_matters = IntSet::default();
    q.vis_matters = IntSet::default();
    q.frame_matters = IntSet::default();
    q.mat_frame_matters = IntSet::default();
    q.decal_matters = IntSet::default();
    s.sequences = vec![q];
    s
}

#[test]
fn a_shape_whose_indices_point_inside_it_validates() {
    assert_eq!(valid_shape().validate(), Ok(()));
}

#[test]
fn every_index_that_points_outside_is_refused() {
    type Breaker = fn(&mut Shape);
    let breakers: Vec<(&str, Breaker)> = vec![
        ("parent", |s| s.nodes[0].parent = 5),
        ("own parent", |s| s.nodes[0].parent = 0),
        ("negative parent", |s| s.nodes[0].parent = -2),
        ("object node", |s| s.objects[0].node = 1),
        ("object meshes", |s| s.objects[0].num_meshes = 2),
        ("object start", |s| s.objects[0].start_mesh_index = -1),
        ("object start far", |s| {
            s.objects[0].start_mesh_index = i32::MAX
        }),
        ("primitive", |s| {
            s.meshes[0].as_mut().unwrap().primitives[0].num_elements = 4
        }),
        ("primitive start", |s| {
            s.meshes[0].as_mut().unwrap().primitives[0].start = u16::MAX
        }),
        ("index", |s| s.meshes[0].as_mut().unwrap().indices[2] = 4),
        ("base rotation", |s| s.sequences[0].base_rotation = 1),
        ("base rotation max", |s| {
            s.sequences[0].base_rotation = i32::MAX
        }),
        ("keyframes", |s| s.sequences[0].num_keyframes = 3),
        ("negative keyframes", |s| s.sequences[0].num_keyframes = -1),
        ("trigger", |s| s.sequences[0].first_trigger = 1),
        ("triggers", |s| s.sequences[0].num_triggers = 2),
        ("ground", |s| s.sequences[0].num_ground_frames = 1),
        ("translation", |s| {
            s.sequences[0].translation_matters.words = vec![0b111];
            s.sequences[0].base_translation = 0;
        }),
        ("object state", |s| {
            s.sequences[0].vis_matters.words = vec![1]
        }),
        ("decal state", |s| {
            s.sequences[0].decal_matters.words = vec![1]
        }),
        ("scale", |s| {
            s.sequences[0].flags = SEQ_UNIFORM_SCALE;
            s.sequences[0].scale_matters.words = vec![1];
        }),
    ];
    for (what, breaker) in breakers {
        let mut s = valid_shape();
        breaker(&mut s);
        assert!(
            matches!(s.validate(), Err(ShapeError::Bad(_))),
            "{what} was accepted"
        );
    }
}

#[test]
fn the_estimate_of_a_shape_grows_with_its_tables_and_counts_shared_vertices_once() {
    let small = crate::dts::parse(&crate::synth::cloning_shape(1_000, 0), &mut |_| true)
        .unwrap()
        .shape;
    let cloned = crate::dts::parse(&crate::synth::cloning_shape(1_000, 40), &mut |_| true)
        .unwrap()
        .shape;
    assert!(small.approx_bytes() >= 12_000);
    // Forty children share the parent's 12,000 bytes of positions: they add their own headers only.
    let per_child = (cloned.approx_bytes() - small.approx_bytes()) / 40;
    assert!(per_child < 1_000, "{per_child}");
    let mut grown = small.clone();
    grown.names.push(vec![b'x'; 5_000]);
    assert!(grown.approx_bytes() >= small.approx_bytes() + 5_000);
}
