//! The tree's rules, one scenario each: refusals, caps, the keep modes, the scale rule, resolve order and the deletes.

mod common;

use common::{World, pose_error};
use torque_collide::math::{IDENTITY, Matrix, mul_p, quat_matrix};
use torque_hierarchy::{
    Caps, Hierarchy, Keep, Local, NodeRef, ObjId, ParentError, ParentRef, Pose,
};

fn id(n: u32) -> ObjId {
    ObjId(n)
}

fn at(x: f32, y: f32, z: f32) -> Matrix {
    let mut m = IDENTITY;
    m[3] = x;
    m[7] = y;
    m[11] = z;
    m
}

/// A host of `n` objects at the origin with scale 1.
fn world(n: u32) -> World {
    let mut w = World::default();
    for i in 0..n {
        w.poses.insert(
            id(i),
            Pose {
                world: IDENTITY,
                scale: [1.0; 3],
            },
        );
    }
    w
}

fn under(h: &mut Hierarchy, host: &World, child: u32, parent: u32) {
    h.set_parent(
        id(child),
        Some(ParentRef::origin(id(parent))),
        Keep::Local,
        host,
    )
    .expect("a link within the caps");
}

fn local_at(x: f32, y: f32, z: f32) -> Local {
    Local {
        translation: [x, y, z],
        ..Local::IDENTITY
    }
}

fn flush(h: &mut Hierarchy, host: &World) -> Vec<ObjId> {
    h.flush(host, usize::MAX);
    let mut moved = Vec::new();
    h.drain_moved(&mut moved);
    moved
}

#[test]
fn self_and_cycle_links_are_refused() {
    let host = world(4);
    let mut h = Hierarchy::new(Caps::default());
    assert_eq!(
        h.set_parent(id(0), Some(ParentRef::origin(id(0))), Keep::World, &host),
        Err(ParentError::SelfParent)
    );
    under(&mut h, &host, 1, 0);
    under(&mut h, &host, 2, 1);
    under(&mut h, &host, 3, 2);
    for (child, parent) in [(0, 3), (0, 2), (1, 3), (0, 1)] {
        let r = h.set_parent(
            id(child),
            Some(ParentRef::origin(id(parent))),
            Keep::World,
            &host,
        );
        assert_eq!(r, Err(ParentError::Cycle), "{child} under {parent}");
    }
    assert_eq!(
        h.check_parent(id(0), Some(ParentRef::origin(id(3)))),
        Err(ParentError::Cycle)
    );
    assert_eq!(h.refused(), 5);
    assert!(
        h.is_ancestor(id(0), id(3)) && !h.is_ancestor(id(3), id(0)) && !h.is_ancestor(id(3), id(3))
    );
    h.audit().expect("a refused link changes nothing");
}

#[test]
fn the_depth_cap_counts_the_moved_subtree() {
    let host = world(8);
    let caps = Caps {
        depth: 3,
        ..Caps::default()
    };
    let mut h = Hierarchy::new(caps);
    // 0 > 1 > 2 > 3 is three links; 4 > 5 is one.
    under(&mut h, &host, 1, 0);
    under(&mut h, &host, 2, 1);
    under(&mut h, &host, 3, 2);
    under(&mut h, &host, 5, 4);
    let link = |h: &mut Hierarchy, child: u32, parent: u32| {
        h.set_parent(
            id(child),
            Some(ParentRef::origin(id(parent))),
            Keep::World,
            &host,
        )
    };
    assert_eq!(
        link(&mut h, 4, 3),
        Err(ParentError::Depth),
        "4 would bring 5 to depth 5"
    );
    assert_eq!(link(&mut h, 6, 3), Err(ParentError::Depth));
    assert!(link(&mut h, 6, 2).is_ok());
    assert_eq!(h.depth(id(6)), 3);
    assert!(
        link(&mut h, 3, 4).is_ok(),
        "a subtree may move to where it fits"
    );
    assert_eq!(h.depth(id(3)), 1);
    h.audit().expect("depths stay consistent");
}

#[test]
fn children_descendants_and_total_caps_hold() {
    let host = world(20);
    let caps = Caps {
        depth: 8,
        children: 2,
        descendants: 5,
        total: 7,
    };
    let mut h = Hierarchy::new(caps);
    let link = |h: &mut Hierarchy, child: u32, parent: u32| {
        h.set_parent(
            id(child),
            Some(ParentRef::origin(id(parent))),
            Keep::World,
            &host,
        )
    };
    assert!(link(&mut h, 1, 0).is_ok() && link(&mut h, 2, 0).is_ok());
    assert_eq!(link(&mut h, 3, 0), Err(ParentError::Children));
    assert!(link(&mut h, 3, 1).is_ok() && link(&mut h, 4, 1).is_ok() && link(&mut h, 5, 3).is_ok());
    assert_eq!(h.descendant_count(id(0)), 5);
    assert_eq!(
        link(&mut h, 6, 2),
        Err(ParentError::Subtree),
        "a sixth descendant of 0"
    );
    assert!(link(&mut h, 11, 10).is_ok() && link(&mut h, 12, 10).is_ok());
    assert_eq!(h.parented_count(), 7);
    assert_eq!(link(&mut h, 13, 12), Err(ParentError::Total));
    assert!(
        link(&mut h, 5, 4).is_ok(),
        "moving within a tree adds nothing"
    );
    assert!(link(&mut h, 13, 4).is_err());
}

#[test]
fn keep_world_rewrites_the_local_pose_and_keep_local_moves_the_object() {
    let mut host = world(3);
    host.poses.insert(
        id(0),
        Pose {
            world: at(10.0, 0.0, 0.0),
            scale: [1.0; 3],
        },
    );
    host.poses.insert(
        id(1),
        Pose {
            world: at(12.0, 1.0, 0.0),
            scale: [1.0; 3],
        },
    );
    let mut h = Hierarchy::new(Caps::default());
    let parent = Some(ParentRef::origin(id(0)));
    let pose = h
        .set_parent(id(1), parent, Keep::World, &host)
        .expect("links");
    assert_eq!(pose.world, at(12.0, 1.0, 0.0));
    let local = h.local(id(1)).expect("has a parent");
    assert_eq!(local.translation, [2.0, 1.0, 0.0]);
    assert_eq!(local.rotation, [0.0, 0.0, 0.0, 1.0]);
    // Keeping the local pose under another parent moves the object to the new parent's frame.
    host.poses.insert(
        id(2),
        Pose {
            world: at(-4.0, 0.0, 0.0),
            scale: [1.0; 3],
        },
    );
    let moved = h
        .set_parent(id(1), Some(ParentRef::origin(id(2))), Keep::Local, &host)
        .expect("links");
    assert_eq!(moved.world, at(-2.0, 1.0, 0.0));
    assert_eq!(h.local(id(1)).map(|l| l.translation), Some([2.0, 1.0, 0.0]));
    // Clearing the parent keeping local: the local pose becomes the world pose; keeping world: nothing moves.
    let freed = h
        .set_parent(id(1), None, Keep::Local, &host)
        .expect("clears");
    assert_eq!(freed.world, at(2.0, 1.0, 0.0));
    assert!(
        !h.contains(id(1)) && !h.contains(id(2)),
        "idle objects are let go"
    );
    // A root kept local under a parent takes its own transform as the local pose.
    let mut host = world(2);
    host.poses.insert(
        id(1),
        Pose {
            world: at(3.0, 0.0, 0.0),
            scale: [1.0; 3],
        },
    );
    let mut h = Hierarchy::new(Caps::default());
    let p = h
        .set_parent(id(1), Some(ParentRef::origin(id(0))), Keep::Local, &host)
        .expect("links");
    assert_eq!(p.world, at(3.0, 0.0, 0.0));
}

#[test]
fn a_parents_scale_scales_the_childs_translation_only() {
    let mut host = world(2);
    host.poses.insert(
        id(0),
        Pose {
            world: at(1.0, 0.0, 0.0),
            scale: [2.0, 3.0, 4.0],
        },
    );
    let mut h = Hierarchy::new(Caps::default());
    under(&mut h, &host, 1, 0);
    let turn = [0.0, 0.0, 0.5f32.sqrt(), 0.5f32.sqrt()];
    let local = Local {
        translation: [1.0, 1.0, 1.0],
        rotation: turn,
        scale: [5.0, 5.0, 5.0],
    };
    h.set_local(id(1), local).expect("valid");
    flush(&mut h, &host);
    let world = h.world(id(1)).expect("held");
    assert_eq!([world[3], world[7], world[11]], [3.0, 3.0, 4.0]);
    let rotation = quat_matrix(turn);
    let rows = [0, 1, 2, 4, 5, 6, 8, 9, 10];
    assert!(
        rows.iter().all(|i| (world[*i] - rotation[*i]).abs() < 1e-6),
        "the parent's scale does not shear the child"
    );
    assert_eq!(h.pose(id(1)).map(|p| p.scale), Some([5.0; 3]));
    // Keeping world under a scaled parent divides the translation back.
    host.poses.insert(
        id(1),
        Pose {
            world: at(3.0, 3.0, 4.0),
            scale: [1.0; 3],
        },
    );
    let mut h = Hierarchy::new(Caps::default());
    h.set_parent(id(1), Some(ParentRef::origin(id(0))), Keep::World, &host)
        .expect("links");
    let t = h.local(id(1)).expect("has a parent").translation;
    assert!(
        t.iter()
            .zip([1.0, 1.0, 1.0])
            .all(|(a, b)| (a - b).abs() < 1e-6),
        "{t:?}"
    );
}

#[test]
fn a_node_parent_composes_the_hosts_node_matrix() {
    let host = world(2);
    let mut h = Hierarchy::new(Caps::default());
    let on_node = ParentRef {
        object: id(0),
        node: NodeRef::Node(2),
    };
    h.set_parent(id(1), Some(on_node), Keep::Local, &host)
        .expect("links");
    flush(&mut h, &host);
    let node = torque_hierarchy::Host::node_matrix(&host, id(0), 2).expect("node 2 exists");
    assert_eq!(
        h.world(id(1)).map(|w| (w[3], w[7], w[11])),
        Some((node[3], node[7], node[11]))
    );
    let missing = ParentRef {
        object: id(0),
        node: NodeRef::Node(9),
    };
    assert_eq!(
        h.set_parent(id(1), Some(missing), Keep::World, &host),
        Err(ParentError::BadNode)
    );
    assert_eq!(h.parent(id(1)), Some(on_node));
}

#[test]
fn resolve_is_parents_first_and_a_dirty_root_resolves_its_subtree_once() {
    let host = world(8);
    let mut h = Hierarchy::new(Caps::default());
    // 0 > (1 > (3, 4), 2 > 5), and 6 > 7 apart.
    for (child, parent) in [(1, 0), (2, 0), (3, 1), (4, 1), (5, 2), (7, 6)] {
        under(&mut h, &host, child, parent);
        h.set_local(id(child), local_at(1.0, 0.0, 0.0))
            .expect("valid");
    }
    flush(&mut h, &host);
    assert_eq!(h.world(id(5)).map(|w| w[3]), Some(2.0));
    // Dirty a deep node and its ancestor and an unrelated root: each node once, parents before children.
    h.mark_dirty(id(1));
    h.mark_dirty(id(0));
    h.mark_dirty(id(1));
    h.mark_dirty(id(6));
    assert_eq!(h.dirty_count(), 3);
    h.set_world(id(0), at(10.0, 0.0, 0.0), &host)
        .expect("moves a root");
    let stats = h.flush(&host, usize::MAX);
    assert_eq!(
        (stats.resolved, stats.carried),
        (6, 0),
        "5 below 0 and 1 below 6"
    );
    let mut moved = Vec::new();
    h.drain_moved(&mut moved);
    assert_eq!(moved.len(), 6);
    let position = |o: ObjId| moved.iter().position(|m| *m == o).expect("moved");
    for (child, parent) in [(1, 0), (2, 0), (3, 1), (4, 1), (5, 2)] {
        if parent != 0 {
            assert!(
                position(id(parent)) < position(id(child)),
                "{parent} before {child}"
            );
        }
        assert!(h.parent(id(child)).is_some_and(|p| p.object == id(parent)));
    }
    assert_eq!(h.world(id(5)).map(|w| w[3]), Some(12.0));
    assert_eq!(h.world(id(3)).map(|w| w[3]), Some(12.0));
    assert_eq!(h.dirty_count(), 0);
    let again = h.flush(&host, usize::MAX);
    assert_eq!(again.resolved, 0);
}

#[test]
fn a_budget_carries_the_remaining_dirty_roots() {
    let host = world(12);
    let mut h = Hierarchy::new(Caps::default());
    for root in [0u32, 4, 8] {
        for k in 1..4 {
            under(&mut h, &host, root + k, root);
        }
    }
    flush(&mut h, &host);
    for root in [0u32, 4, 8] {
        h.set_world(id(root), at(root as f32, 0.0, 0.0), &host)
            .expect("moves");
    }
    let first = h.flush(&host, 3);
    assert_eq!(
        (first.resolved, first.carried),
        (3, 2),
        "a root's subtree is resolved whole"
    );
    let second = h.flush(&host, usize::MAX);
    assert_eq!((second.resolved, second.carried), (6, 0));
    assert_eq!(h.world(id(11)).map(|w| w[3]), Some(8.0));
}

#[test]
fn a_plain_delete_detaches_keeping_world_and_takes_flagged_children() {
    let mut host = world(8);
    host.poses.insert(
        id(0),
        Pose {
            world: at(5.0, 0.0, 0.0),
            scale: [1.0; 3],
        },
    );
    let mut h = Hierarchy::new(Caps::default());
    // 0 > (1, 2 > 3, 4 > 5), 2 and 3 flagged; 5 has a child 6 that is not.
    for (child, parent) in [(1, 0), (2, 0), (3, 2), (4, 0), (5, 4), (6, 5)] {
        under(&mut h, &host, child, parent);
        h.set_local(id(child), local_at(1.0, 0.0, 0.0))
            .expect("valid");
    }
    h.set_delete_with_parent(id(2), true).expect("flags");
    h.set_delete_with_parent(id(3), true).expect("flags");
    flush(&mut h, &host);
    let before: Vec<Matrix> = [1, 4, 5]
        .iter()
        .map(|c| h.world(id(*c)).expect("held"))
        .collect();
    let mut out = Vec::new();
    h.delete(id(0), &host, &mut out);
    assert_eq!(out, [id(3), id(2)], "flagged children go, children first");
    assert!(!h.contains(id(0)) && !h.contains(id(2)) && !h.contains(id(3)));
    assert!(!h.delete_with_parent(id(2)));
    assert!(
        !h.contains(id(1)),
        "a child with no children of its own is let go"
    );
    assert_eq!(h.parent(id(4)), None);
    assert_eq!(h.parent(id(5)).map(|p| p.object), Some(id(4)));
    let moved = flush(&mut h, &host);
    assert!(moved.is_empty(), "keeping world needs no recompute below");
    assert_eq!(
        pose_error(&h.world(id(4)).expect("held"), &before[1]),
        (0.0, 0.0)
    );
    assert_eq!(
        pose_error(&h.world(id(5)).expect("held"), &before[2]),
        (0.0, 0.0)
    );
    h.audit().expect("consistent");
}

#[test]
fn a_recursive_delete_removes_the_subtree_children_first() {
    let host = world(4_200);
    let mut h = Hierarchy::new(Caps::default());
    // A 4,096-node tree under object 0: eight children each with eight, and so on.
    for i in 1..4_096u32 {
        under(&mut h, &host, i, (i - 1) / 8);
    }
    assert_eq!(h.descendant_count(id(0)), 4_095);
    under(&mut h, &host, 4_100, 1);
    let parent_of = |i: u32| if i == 4_100 { 1 } else { (i - 1) / 8 };
    let below_1 = |mut i: u32| loop {
        if i == 1 {
            break true;
        }
        if i == 0 {
            break false;
        }
        i = parent_of(i);
    };
    let mut expected: Vec<u32> = (1..4_096).filter(|i| below_1(*i)).collect();
    expected.push(4_100);
    let mut out = Vec::new();
    h.delete_recursive(id(1), &mut out);
    let mut got: Vec<u32> = out.iter().map(|o| o.0).collect();
    got.sort_unstable();
    expected.sort_unstable();
    assert_eq!(got, expected);
    let position = |o: u32| out.iter().position(|m| m.0 == o).expect("deleted");
    for o in &out {
        if o.0 != 1 {
            assert!(
                position(o.0) < position(parent_of(o.0)),
                "{o:?} is listed after its parent"
            );
        }
    }
    assert_eq!(out.last(), Some(&id(1)));
    assert!(out.iter().all(|o| !h.contains(*o)));
    assert_eq!(h.parented_count(), 4_096 - out.len());
    h.audit().expect("consistent");
    let mut rest = Vec::new();
    h.delete_recursive(id(0), &mut rest);
    assert_eq!(rest.last(), Some(&id(0)));
    assert!(
        h.is_empty() && h.parented_count() == 0,
        "0 objects and 0 dangling links remain"
    );
}

#[test]
fn a_link_with_a_bad_pose_changes_nothing() {
    let host = world(3);
    let mut h = Hierarchy::new(Caps::default());
    under(&mut h, &host, 1, 0);
    let bad = [
        Local {
            translation: [f32::NAN, 0.0, 0.0],
            ..Local::IDENTITY
        },
        Local {
            translation: [20_000.0, 0.0, 0.0],
            ..Local::IDENTITY
        },
        Local {
            scale: [0.0, 1.0, 1.0],
            ..Local::IDENTITY
        },
        Local {
            rotation: [0.0; 4],
            ..Local::IDENTITY
        },
    ];
    let errors = [
        ParentError::NotFinite,
        ParentError::OutOfRange,
        ParentError::OutOfRange,
        ParentError::OutOfRange,
    ];
    for (local, want) in bad.into_iter().zip(errors) {
        assert_eq!(
            h.link(id(2), ParentRef::origin(id(1)), local, &host),
            Err(want)
        );
        assert_eq!(h.set_local(id(1), local), Err(want));
    }
    assert!(!h.contains(id(2)));
    assert_eq!(
        h.set_local(id(2), Local::IDENTITY),
        Err(ParentError::NotParented)
    );
    let pose = h
        .link(
            id(2),
            ParentRef::origin(id(1)),
            local_at(0.0, 4.0, 0.0),
            &host,
        )
        .expect("a good link");
    assert_eq!(pose.world, at(0.0, 4.0, 0.0));
    let unknown = ObjId(77);
    assert_eq!(
        h.set_parent(unknown, Some(ParentRef::origin(id(0))), Keep::World, &host),
        Err(ParentError::Unknown)
    );
    h.audit().expect("consistent");
}

#[test]
fn moving_a_child_in_the_world_sets_its_local_pose() {
    let mut host = world(2);
    host.poses.insert(
        id(0),
        Pose {
            world: at(0.0, 0.0, 5.0),
            scale: [1.0; 3],
        },
    );
    let mut h = Hierarchy::new(Caps::default());
    under(&mut h, &host, 1, 0);
    assert_eq!(h.set_world(id(1), at(1.0, 2.0, 8.0), &host), Ok(true));
    assert_eq!(h.local(id(1)).map(|l| l.translation), Some([1.0, 2.0, 3.0]));
    assert_eq!(h.set_world(id(9), IDENTITY, &host), Ok(false));
    let moved = mul_p(&h.world(id(0)).expect("held"), [1.0, 0.0, 0.0]);
    assert_eq!(moved, [1.0, 0.0, 5.0]);
}

#[test]
fn a_childs_scale_is_rounded_to_the_wire_form_only_when_locals_are_quantized() {
    let host = world(2);
    let scale = [1.234_567_8, 0.333_333_3, 7.654_321];
    for quantize in [false, true] {
        let mut h = Hierarchy::new(Caps::default()).quantize_locals(quantize);
        under(&mut h, &host, 1, 0);
        assert_eq!(h.set_scale(id(1), scale), Ok(true));
        let got = h.local(id(1)).expect("has a parent").scale;
        if quantize {
            let wire = torque_hierarchy::quantize(&Local {
                scale,
                ..Local::IDENTITY
            })
            .scale;
            assert_eq!(got, wire);
            assert_ne!(got, scale);
            let again = torque_hierarchy::quantize(&Local {
                scale: got,
                ..Local::IDENTITY
            })
            .scale;
            assert_eq!(again, got, "a rounded scale stays put");
        } else {
            assert_eq!(got, scale);
        }
        // A root's scale is not on the wire.
        assert_eq!(h.set_scale(id(0), scale), Ok(true));
        assert_eq!(h.pose(id(0)).map(|p| p.scale), Some(scale));
    }
}

#[test]
fn the_moved_list_names_an_object_once_after_a_delete_and_a_rebuild() {
    let host = world(2);
    let mut h = Hierarchy::new(Caps::default());
    let mut out = Vec::new();
    // Far more rebuilds than the list's cap allows, never drained.
    for _ in 0..1_000 {
        under(&mut h, &host, 1, 0);
        h.set_local(id(1), local_at(1.0, 0.0, 0.0)).expect("valid");
        h.flush(&host, usize::MAX);
        h.delete(id(1), &host, &mut out);
    }
    under(&mut h, &host, 1, 0);
    h.set_local(id(1), local_at(1.0, 0.0, 0.0)).expect("valid");
    h.flush(&host, usize::MAX);
    let mut moved = Vec::new();
    h.drain_moved(&mut moved);
    assert_eq!(moved, [id(1)]);
    h.drain_moved(&mut moved);
    assert_eq!(moved.len(), 1, "a drained list is empty");
}

#[test]
fn a_subtree_that_got_shorter_fits_where_it_did_not() {
    let host = world(12);
    let caps = Caps {
        depth: 3,
        ..Caps::default()
    };
    let mut h = Hierarchy::new(caps);
    // 10 > 11 is two deep; 0 > 1 > 2 is a chain of two links.
    under(&mut h, &host, 11, 10);
    under(&mut h, &host, 1, 0);
    under(&mut h, &host, 2, 1);
    let onto_11 = Some(ParentRef::origin(id(11)));
    assert_eq!(
        h.set_parent(id(0), onto_11, Keep::World, &host),
        Err(ParentError::Depth)
    );
    h.set_parent(id(2), None, Keep::World, &host)
        .expect("detaches");
    h.set_parent(id(0), onto_11, Keep::World, &host)
        .expect("0 > 1 now fits under 11");
    assert_eq!(h.depth(id(1)), 3);
    h.audit().expect("heights stay consistent");
}
