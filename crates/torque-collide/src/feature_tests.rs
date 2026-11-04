use super::*;

/// A feature of `n` vertices, `n` edges and `n` faces, all at the origin (every pair within tolerance, nothing inside).
fn big(n: usize) -> ConvexFeature {
    let mut cf = ConvexFeature::default();
    for i in 0..n as u32 {
        cf.verts.push([0.0; 3]);
        cf.edges.push([i, i]);
        cf.faces.push(FeatureFace {
            normal: [0.0, 0.0, 1.0],
            vertex: [i, i, i],
        });
    }
    cf
}

#[test]
fn collide_pays_for_its_work_and_stops_when_refused() {
    let (a, b) = (big(255), big(255));
    // Refused at once: nothing is tested, the list stays empty.
    let mut list = CollisionList::default();
    assert_eq!(
        a.collide(&b, &mut list, 1.0, &mut |_| false),
        Err(Exhausted)
    );
    assert_eq!(list.count, 0);
    // A budget of 10 vertex tests: the call stops after spending it, whatever the features' size.
    let (mut spent, mut calls) = (0u64, 0u32);
    let mut list = CollisionList::default();
    let per = work::FEATURE * (b.faces.len() as u64 + 1);
    let r = a.collide(&b, &mut list, -1.0, &mut |u| {
        calls += 1;
        if spent + u > per * 10 {
            return false;
        }
        spent += u;
        true
    });
    assert_eq!(r, Err(Exhausted));
    assert_eq!(calls, 11);
    // Unrefused, the work charged grows with the features (vertices x faces, then edge pairs x faces).
    let mut total = 0u64;
    let mut list = CollisionList::default();
    let small = (big(4), big(4));
    assert!(
        small
            .0
            .collide(&small.1, &mut list, -1.0, &mut |u| {
                total += u;
                true
            })
            .is_ok()
    );
    assert_eq!(total, work::FEATURE * (4 * 5 + 4 * 5 + 16 * 9));
}

#[test]
fn crossing_edges_meet_at_their_closest_points() {
    // Two unit-length edges crossing 1 apart in z: x from -1 to 1 at z = 0, y from -1 to 1 at z = 1.
    let (sq, i1, i2) = sqr_distance_edges(
        [-1.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, -1.0, 1.0],
        [0.0, 1.0, 1.0],
    );
    assert_eq!(sq, 1.0);
    assert_eq!(i1, [0.0, 0.0, 0.0]);
    assert_eq!(i2, [0.0, 0.0, 1.0]);
    // Parallel edges are not edge contacts.
    let (sq, _, _) =
        sqr_distance_edges([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]);
    assert_eq!(sq, 1e10);
}

#[test]
fn short_edges_have_a_truncated_determinant() {
    // Edges of length 0.5: a00 * a11 = 0.0625, which truncates to 0: they count as parallel.
    let (sq, _, _) = sqr_distance_edges(
        [-0.25, 0.0, 0.0],
        [0.25, 0.0, 0.0],
        [0.0, -0.25, 0.1],
        [0.0, 0.25, 0.1],
    );
    assert_eq!(sq, 1e10);
}

#[test]
fn a_box_feature_toward_a_corner_is_that_corner() {
    let cf = ConvexFeature::of_box(
        [0.0; 3],
        [1.0; 3],
        ObjectId(1),
        &crate::math::IDENTITY,
        [1.0, 1.0, 1.0],
    );
    assert_eq!(cf.verts, vec![[1.0, 1.0, 1.0]]);
    // Toward +z only: the top face.
    let cf = ConvexFeature::of_box(
        [0.0; 3],
        [1.0; 3],
        ObjectId(1),
        &crate::math::IDENTITY,
        [0.0, 0.0, 1.0],
    );
    assert_eq!(cf.verts.len(), 4);
    assert_eq!(cf.faces.len(), 2);
    assert_eq!(cf.faces[0].normal, [0.0, 0.0, 1.0]);
}
