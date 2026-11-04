use super::*;

fn cube() -> MeshPart {
    let mut verts = Vec::new();
    for k in 0..8u32 {
        verts.push([(k & 1) as f32, ((k >> 1) & 1) as f32, ((k >> 2) & 1) as f32]);
    }
    // Twelve outward triangles.
    let tris = vec![
        [0, 2, 3],
        [0, 3, 1],
        [4, 5, 7],
        [4, 7, 6],
        [0, 1, 5],
        [0, 5, 4],
        [2, 6, 7],
        [2, 7, 3],
        [0, 4, 6],
        [0, 6, 2],
        [1, 3, 7],
        [1, 7, 5],
    ];
    MeshPart {
        node: IDENTITY,
        verts,
        materials: vec![0; tris.len()],
        tris,
    }
}

#[test]
fn a_cube_accelerator_has_eight_vertices_and_their_faces() {
    let acc = HullAccel::compute(&[cube()]);
    assert_eq!(acc.verts.len(), 8);
    assert_eq!(acc.normals.len(), 12);
    assert_eq!(acc.support([1.0, 1.0, 1.0]), [1.0, 1.0, 1.0]);
    assert_eq!(acc.support([-1.0, -0.5, -2.0]), [0.0, 0.0, 0.0]);
    // A corner touches three sides of two triangles each: the flat test brings in the other triangle of each side.
    let s = &acc.emit[0];
    let nv = s[0] as usize;
    let ne = s[1 + nv] as usize;
    let nf = s[2 + nv + 2 * ne] as usize;
    assert_eq!(nf, 6);
    assert_eq!(s.len(), 3 + nv + 2 * ne + 4 * nf);
}
