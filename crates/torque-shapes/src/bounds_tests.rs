use super::*;

fn free(_: u64) -> bool {
    true
}

#[test]
fn a_box_of_points_has_its_middle_and_farthest_distance() {
    let c = compute(&[[0.0, 0.0, 0.0], [2.0, 4.0, 0.0], [1.0, 2.0, 0.0]]);
    assert_eq!(c.bounds, [0.0, 0.0, 0.0, 2.0, 4.0, 0.0]);
    assert_eq!(c.center, [1.0, 2.0, 0.0]);
    assert_eq!(c.radius, 5.0f32.sqrt());
    let empty = compute(&[]);
    assert_eq!(empty.bounds, [0.0; 6]);
    assert_eq!(empty.radius, 0.0);
}

#[test]
fn the_shape_figures_follow_from_the_box() {
    let (c, r, t) = shape_derived(&[-3.0, -4.0, 0.0, 3.0, 4.0, 10.0]);
    assert_eq!(c, [0.0, 0.0, 5.0]);
    assert_eq!(r, (9.0f32 + 16.0 + 25.0).sqrt());
    assert_eq!(t, 5.0);
}

#[test]
fn nodes_place_the_geometry_of_a_detail() {
    use crate::shape::{Detail, Node, Object, SubShape};
    let mut shape = Shape::empty();
    shape.nodes = vec![
        Node {
            name: 0,
            parent: -1,
            first_object: 0,
            first_child: 1,
            next_sibling: -1,
        },
        Node {
            name: 1,
            parent: 0,
            first_object: -1,
            first_child: -1,
            next_sibling: -1,
        },
    ];
    shape.default_rotations = vec![[0; 4]; 2];
    shape.default_translations = vec![[1.0, 0.0, 0.0], [0.0, 2.0, 0.0]];
    shape.sub_shapes = vec![SubShape {
        first_node: 0,
        first_object: 0,
        first_decal: 0,
        num_nodes: 2,
        num_objects: 1,
        num_decals: 0,
    }];
    shape.details = vec![Detail {
        name: 0,
        sub_shape: 0,
        object_detail: 0,
        size: 1.0,
        average_error: 0.0,
        max_error: 0.0,
        poly_count: 0,
    }];
    shape.objects = vec![Object {
        name: 0,
        num_meshes: 1,
        start_mesh_index: 0,
        node: 1,
        next_sibling: -1,
        first_decal: -1,
    }];
    let positions: std::sync::Arc<[[f32; 3]]> =
        std::sync::Arc::from(vec![[0.0; 3], [1.0, 1.0, 1.0]]);
    shape.meshes = vec![Some(Mesh {
        kind: crate::shape::MeshKind::Standard,
        parent: -1,
        num_frames: 1,
        num_mat_frames: 1,
        verts_per_frame: 2,
        flags: 0,
        bounds: [0.0; 6],
        center: [0.0; 3],
        radius: 0,
        num_verts: 2,
        num_tverts: 0,
        positions,
        primitives: Vec::new(),
        indices: Vec::new(),
    })];
    // The child node sits at (1, 2, 0): the mesh's box is that plus its own.
    assert_eq!(
        detail_bounds(&shape, 0, &mut free),
        Ok(Some([1.0, 2.0, 0.0, 2.0, 3.0, 1.0]))
    );
    assert_eq!(detail_bounds(&shape, 1, &mut free), Ok(None));
    shape.nodes[1].parent = 5;
    assert_eq!(detail_bounds(&shape, 0, &mut free), Ok(None));
}

/// A hostile input: one 600,000-vertex mesh and 4,096 objects that all point at it. Every object's vertices are walked,
/// so the charge must be per vertex walked: 2.4 billion units, which a small budget refuses before the first is walked.
#[test]
fn a_detail_pays_for_every_vertex_it_walks() {
    use crate::shape::{Detail, Node, Object, SubShape};
    let mut shape = Shape::empty();
    shape.nodes = vec![Node {
        name: 0,
        parent: -1,
        first_object: 0,
        first_child: -1,
        next_sibling: -1,
    }];
    shape.default_rotations = vec![[0; 4]];
    shape.default_translations = vec![[0.0; 3]];
    shape.sub_shapes = vec![SubShape {
        first_node: 0,
        first_object: 0,
        first_decal: 0,
        num_nodes: 1,
        num_objects: 4_096,
        num_decals: 0,
    }];
    shape.details = vec![Detail {
        name: 0,
        sub_shape: 0,
        object_detail: 0,
        size: 1.0,
        average_error: 0.0,
        max_error: 0.0,
        poly_count: 0,
    }];
    let object = Object {
        name: 0,
        num_meshes: 1,
        start_mesh_index: 0,
        node: 0,
        next_sibling: -1,
        first_decal: -1,
    };
    shape.objects = vec![object; 4_096];
    let positions: std::sync::Arc<[[f32; 3]]> = std::sync::Arc::from(vec![[1.0; 3]; 600_000]);
    shape.meshes = vec![Some(Mesh {
        kind: crate::shape::MeshKind::Standard,
        parent: -1,
        num_frames: 1,
        num_mat_frames: 1,
        verts_per_frame: 600_000,
        flags: 0,
        bounds: [0.0; 6],
        center: [0.0; 3],
        radius: 0,
        num_verts: 600_000,
        num_tverts: 0,
        positions,
        primitives: Vec::new(),
        indices: Vec::new(),
    })];
    let walked = || WALKED.with(std::cell::Cell::get);
    let before = walked();
    let mut left = 10_000_000u64;
    let result = detail_bounds(&shape, 0, &mut |u| {
        if u > left {
            return false;
        }
        left -= u;
        true
    });
    assert_eq!(result, Err(ShapeError::OutOfBudget));
    // The refused run walked no more vertices than the budget paid for (not the 2.4 billion it names).
    let refused_walk = walked() - before;
    assert!(refused_walk <= 10_000_000, "{refused_walk}");
    // One object's worth, paid in full, is charged at least its vertices.
    shape.sub_shapes[0].num_objects = 1;
    let before = walked();
    let mut units = 0;
    let bounds = detail_bounds(&shape, 0, &mut |u| {
        units += u;
        true
    });
    assert_eq!(bounds, Ok(Some([1.0; 6])));
    assert!(units >= 600_000, "{units}");
    assert_eq!(walked() - before, 600_000);
}
