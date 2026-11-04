use super::testkit::{Sample, SampleMesh};
use super::*;

fn free(_: u64) -> bool {
    true
}

#[test]
fn the_sample_scans_with_every_cursor_at_its_end() {
    let bytes = Sample::default().build();
    let scan = scan(&bytes, &mut free).expect("scans");
    assert_eq!(scan.header.version, 24);
    assert_eq!(scan.header.exporter, 1);
    assert_eq!(scan.counts.nodes, 2);
    assert_eq!(scan.counts.names, 4);
    assert_eq!(scan.names[1], b"Collision-1");
    assert_eq!(scan.details.len(), 3);
    assert_eq!(scan.details[1].name, 1);
    assert_eq!(scan.meshes.len(), 1);
    assert_eq!(scan.meshes[0].num_verts, 4);
    assert_eq!(scan.meshes[0].num_indices, 6);
    assert_eq!(scan.sequences.len(), 1);
    assert_eq!(scan.materials.materials.len(), 2);
    assert_eq!(scan.materials.materials[1].name.as_deref(), Some(&b"b"[..]));
    assert_eq!(scan.trailing, 0);
    assert_eq!(scan.guards.wrong, 0);
    assert_eq!(scan.guards.read, 19);
    // The 16-bit and 8-bit sections are padded to a word; the 32-bit one is read to its end.
    assert_eq!(scan.left[0], 0);
    assert!(scan.left[1] <= 1 && scan.left[2] <= 3, "{:?}", scan.left);
    assert_eq!(scan.total_verts, 4);
    assert_eq!(scan.shared_verts, 0);
}

#[test]
fn a_child_mesh_shares_its_parents_vertices_and_owns_no_bytes() {
    let mut sample = Sample::default();
    sample.meshes.push(SampleMesh {
        kind: 0,
        parent: 0,
        verts: 4,
        tverts: 4,
        prims: 1,
        indices: 3,
        skin: [0; 4],
        frames: None,
    });
    let bytes = sample.build();
    let scan = scan(&bytes, &mut free).expect("scans");
    assert_eq!(scan.meshes.len(), 2);
    assert_eq!(scan.meshes[1].parent, 0);
    assert_eq!(scan.total_verts, 8);
    assert_eq!(scan.shared_verts, 4);
    assert_eq!(scan.guards.wrong, 0);
}

#[test]
fn skin_and_sorted_meshes_read_their_extra_arrays() {
    let mut sample = Sample::default();
    sample.meshes.push(SampleMesh {
        kind: 3,
        parent: -1,
        verts: 3,
        tverts: 3,
        prims: 1,
        indices: 3,
        skin: [0; 4],
        frames: None,
    });
    let bytes = sample.build();
    let scan = scan(&bytes, &mut free).expect("scans");
    assert_eq!(scan.meshes[1].kind, MeshKind::Sorted);
    assert_eq!(scan.guards.wrong, 0);
    assert_eq!(scan.guards.read, 22);
}

#[test]
fn a_wrong_guard_is_counted_not_fatal() {
    let mut bytes = Sample::default().build();
    // The first guard is the word after the 19 counts, at body offset 19 * 4.
    bytes[HEADER_BYTES + 19 * 4] = 77;
    let scan = scan(&bytes, &mut free).expect("scans");
    assert_eq!(scan.guards.wrong, 1);
}

#[test]
fn versions_and_headers_are_refused_by_kind() {
    let good = Sample::default().build();
    assert_eq!(scan(&[], &mut free), Err(ShapeError::Short));
    assert_eq!(scan(&[24, 0, 0], &mut free), Err(ShapeError::Short));
    let mut v25 = good.clone();
    v25[0] = 25;
    assert_eq!(scan(&v25, &mut free), Err(ShapeError::NewerVersion(25)));
    let mut v18 = good.clone();
    v18[0] = 18;
    assert_eq!(scan(&v18, &mut free), Err(ShapeError::OldVersion(18)));
    let mut v22 = good.clone();
    v22[0] = 22;
    assert_eq!(
        scan(&v22, &mut free),
        Err(ShapeError::UnsupportedVersion(22))
    );
    // Exporter bits do not matter; the sizes are checked against the file.
    let mut big = good.clone();
    big[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        scan(&big, &mut free),
        Err(ShapeError::BadHeader(_))
    ));
    let mut order = good.clone();
    order[8..12].copy_from_slice(&1000u32.to_le_bytes());
    assert!(matches!(
        scan(&order, &mut free),
        Err(ShapeError::BadHeader(_))
    ));
    assert!(matches!(
        scan(&good[..HEADER_BYTES - 1], &mut free),
        Err(ShapeError::BadHeader(_))
    ));
}

#[test]
fn every_truncation_of_the_sample_is_an_error_not_a_panic() {
    let good = Sample::default().build();
    for cut in 0..good.len() {
        assert!(scan(&good[..cut], &mut free).is_err(), "cut at {cut}");
    }
}

#[test]
fn hostile_counts_are_refused_before_they_are_used() {
    let good = Sample::default().build();
    for field in 0..19usize {
        let mut bad = good.clone();
        bad[HEADER_BYTES + field * 4..HEADER_BYTES + field * 4 + 4]
            .copy_from_slice(&0x7fff_ffffu32.to_le_bytes());
        // Fields 17 and 18 (the visible size and detail) are plain numbers; every count field must be refused.
        let result = scan(&bad, &mut free);
        if field < 17 {
            assert!(result.is_err(), "field {field}");
        }
    }
}

#[test]
fn a_parent_that_is_not_an_earlier_mesh_or_has_fewer_vertices_is_refused() {
    let mut sample = Sample::default();
    sample.meshes[0].parent = 0;
    assert!(matches!(
        scan(&sample.build(), &mut free),
        Err(ShapeError::Bad(_))
    ));
    let mut sample = Sample::default();
    sample.meshes.push(SampleMesh {
        kind: 0,
        parent: 0,
        verts: 9,
        tverts: 4,
        prims: 0,
        indices: 0,
        skin: [0; 4],
        frames: None,
    });
    assert!(matches!(
        scan(&sample.build(), &mut free),
        Err(ShapeError::Bad(_))
    ));
}

#[test]
fn mesh_cloning_is_stopped_by_the_per_shape_total() {
    // The clones need no bytes, so a parent of 70,000 vertices with 60 children claims 4.2 million vertices, past the
    // per-shape total.
    let mut sample = Sample::default();
    sample.meshes[0].verts = 70_000;
    sample.meshes[0].tverts = 70_000;
    sample.meshes[0].prims = 0;
    sample.meshes[0].indices = 0;
    for _ in 0..60 {
        sample.meshes.push(SampleMesh {
            kind: 0,
            parent: 0,
            verts: 70_000,
            tverts: 70_000,
            prims: 0,
            indices: 0,
            skin: [0; 4],
            frames: None,
        });
    }
    let result = scan(&sample.build(), &mut free);
    assert!(
        matches!(
            result,
            Err(ShapeError::Cap {
                what: "shape vertices",
                ..
            })
        ),
        "{result:?}"
    );
}

#[test]
fn a_null_mesh_is_its_type_word_alone_and_cannot_be_a_parent() {
    let mut sample = Sample::default();
    sample.meshes.push(SampleMesh {
        kind: 4,
        parent: -1,
        verts: 0,
        tverts: 0,
        prims: 0,
        indices: 0,
        skin: [0; 4],
        frames: None,
    });
    let bytes = sample.build();
    let scan = scan(&bytes, &mut free).expect("scans");
    assert_eq!(scan.meshes[1].kind, MeshKind::Null);
    assert_eq!(scan.total_verts, 4);
    assert_eq!(scan.guards.wrong, 0);
    // A child that names the null mesh as its parent has nothing to share.
    sample.meshes.push(SampleMesh {
        kind: 0,
        parent: 1,
        verts: 1,
        tverts: 0,
        prims: 0,
        indices: 0,
        skin: [0; 4],
        frames: None,
    });
    assert!(matches!(scan_result(&sample), Err(ShapeError::Bad(_))));
}

fn scan_result(sample: &Sample) -> Result<Scan, ShapeError> {
    scan(&sample.build(), &mut free)
}

fn skin(verts: u32, parent: i32, counts: [u32; 4]) -> SampleMesh {
    SampleMesh {
        kind: 1,
        parent,
        verts,
        tverts: verts,
        prims: 0,
        indices: 0,
        skin: counts,
        frames: None,
    }
}

fn with_meshes(extra: Vec<SampleMesh>) -> Sample {
    let mut sample = Sample::default();
    sample.meshes.clear();
    sample.meshes.extend(extra);
    sample
}

#[test]
fn a_skin_mesh_records_its_counts_and_a_skin_child_shares_its_parents() {
    let sample = with_meshes(vec![skin(4, -1, [4, 2, 4, 2]), skin(4, 0, [4, 2, 3, 2])]);
    let scan = scan(&sample.build(), &mut free).expect("scans");
    let owner = scan.meshes[0].skin.expect("counts");
    assert_eq!(
        (owner.verts, owner.transforms, owner.weights, owner.nodes),
        (4, 2, 4, 2)
    );
    assert_eq!(scan.meshes[1].skin.map(|s| s.weights), Some(3));
    assert_eq!(scan.guards.wrong, 0);
    // The initial vertices count toward the shape's vertices (owned or shared), the rest toward its skin data.
    assert_eq!(scan.total_verts, 4 + 4 + 4 + 4);
    assert_eq!(scan.total_skin, 8 + 7);
}

#[test]
fn a_skin_child_needs_a_skin_parent_and_counts_within_the_parents() {
    // A standard parent never fills the skin lists (the reader sets them for skin meshes only).
    let mut standard = with_meshes(vec![skin(4, -1, [4, 2, 4, 2])]);
    standard.meshes[0].kind = 0;
    standard.meshes.push(skin(4, 0, [4, 2, 4, 2]));
    assert!(matches!(scan_result(&standard), Err(ShapeError::Bad(_))));
    for over in 0..4 {
        let mut counts = [4, 2, 4, 2];
        counts[over] += 1;
        let sample = with_meshes(vec![skin(4, -1, [4, 2, 4, 2]), skin(4, 0, counts)]);
        assert!(
            matches!(scan_result(&sample), Err(ShapeError::Bad(_))),
            "count {over}"
        );
    }
}

#[test]
fn skin_cloning_is_stopped_by_the_per_shape_totals() {
    let mut meshes = vec![skin(70_000, -1, [70_000, 1, 70_000, 1])];
    for _ in 0..60 {
        meshes.push(skin(70_000, 0, [70_000, 1, 70_000, 1]));
    }
    let result = scan_result(&with_meshes(meshes));
    assert!(matches!(result, Err(ShapeError::Cap { .. })), "{result:?}");
}

#[test]
fn frame_counts_are_checked_products_against_the_vertex_counts() {
    let frames = |f: u32, m: u32, v: u32| {
        let mut sample = Sample::default();
        sample.meshes[0].frames = Some((f, m, v));
        scan_result(&sample)
    };
    // Four vertices and four texture vertices: one frame of four, two of two, one material frame of four.
    assert!(frames(1, 1, 4).is_ok());
    assert!(frames(2, 1, 2).is_ok());
    assert!(frames(1, 1, 5).is_err());
    assert!(frames(3, 1, 2).is_err());
    assert!(frames(1, 3, 2).is_err());
    // Negative counts and products that overflow every width.
    assert!(frames(u32::MAX, 1, 1).is_err());
    assert!(frames(1, 1, u32::MAX).is_err());
    assert!(frames(0x7fff_ffff, 0x7fff_ffff, 0x7fff_ffff).is_err());
}

#[test]
fn capacity_is_bounded_by_the_bytes_that_remain() {
    assert_eq!(crate::capacity(16_384, 100, 4), 25);
    assert_eq!(crate::capacity(3, 100, 4), 3);
    assert_eq!(crate::capacity(5, 0, 4), 0);
    assert_eq!(crate::capacity(5, 10, 0), 5);
}

#[test]
fn a_material_list_that_ends_early_still_loads_the_shape() {
    let sample = Sample {
        materials: vec!["a", "", "c"],
        ..Sample::default()
    };
    let scan = scan_result(&sample).expect("loads");
    assert_eq!(scan.materials.materials.len(), 1);
    assert_eq!(scan.materials.materials[0].flags, 0);
    assert!(scan.trailing > 0);
}

#[test]
fn a_decal_mesh_and_an_unknown_mesh_type_are_refused() {
    for kind in [2u32, 5, 9] {
        let mut sample = Sample::default();
        sample.meshes[0].kind = kind;
        assert!(matches!(
            scan(&sample.build(), &mut free),
            Err(ShapeError::Bad(_))
        ));
    }
}

#[test]
fn a_budget_that_cannot_pay_ends_the_scan() {
    let bytes = Sample::default().build();
    assert_eq!(scan(&bytes, &mut |_| false), Err(ShapeError::OutOfBudget));
    // Paying the first call only: the work after it refuses.
    let mut paid = 0;
    let result = scan(&bytes, &mut |_| {
        paid += 1;
        paid <= 1
    });
    assert_eq!(result, Err(ShapeError::OutOfBudget));
}

fn mesh(kind: u32, parent: i32, verts: u32, skin: [u32; 4]) -> SampleMesh {
    SampleMesh {
        kind,
        parent,
        verts,
        tverts: verts,
        prims: 2,
        indices: if verts == 0 { 0 } else { 5 },
        skin,
        frames: None,
    }
}

#[test]
fn the_writer_gives_the_testkits_files_back() {
    let mut samples = vec![Sample::default()];
    let mut with = |meshes: Vec<SampleMesh>| {
        samples.push(Sample {
            meshes,
            ..Sample::default()
        });
    };
    with(vec![mesh(0, -1, 4, [0; 4]), mesh(0, 0, 3, [0; 4])]);
    with(vec![
        mesh(1, -1, 4, [4, 2, 4, 2]),
        mesh(1, 0, 3, [3, 2, 1, 2]),
    ]);
    with(vec![
        mesh(3, -1, 5, [0; 4]),
        mesh(4, -1, 0, [0; 4]),
        mesh(0, -1, 0, [0; 4]),
    ]);
    with(Vec::new());
    samples.push(Sample {
        materials: vec!["a", "", "c"],
        ..Sample::default()
    });
    samples.push(Sample {
        exporter: 124,
        nodes: 5,
        materials: Vec::new(),
        ..Sample::default()
    });
    for (i, sample) in samples.iter().enumerate() {
        let bytes = sample.build();
        let model = parse(&bytes, &mut free).expect("parses");
        assert_eq!(crate::write::write_dts(&model), bytes, "sample {i}");
    }
}

#[test]
fn what_the_file_holds_past_the_data_is_given_back_too() {
    use crate::write::write_dts;
    let mut bytes = Sample::default().build();
    // Bits 8 to 15 of the version word, which Torque ignores.
    bytes[1] = 0x5a;
    let model = parse(&bytes, &mut free).expect("parses");
    assert_eq!(model.file.version_middle, 0x5a);
    assert_eq!(write_dts(&model), bytes);
    // Padding with something in it, and a byte after the material list.
    let mut model = parse(&Sample::default().build(), &mut free).expect("parses");
    model.file.left = [vec![9, 8, 7, 6], vec![1, 2, 3, 4], vec![5, 5, 5, 5]];
    model.file.trailing = vec![0xaa, 0xbb];
    let bytes = write_dts(&model);
    let again = parse(&bytes, &mut free).expect("parses");
    assert!(again.file.left[0].starts_with(&[9, 8, 7, 6]));
    assert!(again.file.left[1].starts_with(&[1, 2, 3, 4]));
    assert!(again.file.left[2].starts_with(&[5, 5, 5, 5]));
    assert_eq!(again.file.trailing, [0xaa, 0xbb]);
    assert_eq!(write_dts(&again), bytes);
}

#[test]
fn a_parsed_sample_has_the_arrays_it_was_built_with() {
    let sample = Sample {
        meshes: vec![mesh(1, -1, 4, [4, 2, 4, 2]), mesh(0, 0, 3, [0; 4])],
        ..Sample::default()
    };
    let model = parse(&sample.build(), &mut free).expect("parses");
    let shape = &model.shape;
    assert_eq!(shape.nodes.len(), 2);
    assert_eq!(shape.names[2], b"LOS-9");
    assert_eq!(shape.details[1].name, 1);
    let Some(m) = &shape.meshes[0] else { panic!() };
    assert_eq!((m.kind, m.num_verts, m.num_tverts), (MeshKind::Skin, 4, 4));
    assert_eq!((m.primitives.len(), m.indices.len()), (2, 5));
    let r0 = model.render[0].as_ref().expect("render");
    let MeshExtra::Skin(skin) = &r0.extra else {
        panic!()
    };
    assert_eq!(
        (skin.num_initial_verts, skin.initial_transforms.len()),
        (4, 2)
    );
    assert_eq!((skin.vertex_index.len(), skin.node_index.len()), (4, 2));
    // The child shares the parent's initial vertices and owns no bytes.
    let child = shape.meshes[1].as_ref().expect("child");
    assert_eq!((child.parent, child.num_verts), (0, 3));
    assert!(std::sync::Arc::ptr_eq(
        &child.positions,
        &skin.initial_verts
    ));
    assert_eq!(child.verts().len(), 3);
    assert_eq!(shape.materials.len(), 2);
    assert_eq!(shape.sequences.len(), 1);
    assert_eq!(shape.sequences[0].rotation_matters.words, vec![5]);
}
