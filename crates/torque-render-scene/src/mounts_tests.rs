use std::collections::HashMap;

use ghost_world::tge::TgeClientSet;
use ghost_world::world::ClientWorld;
use ghosts::shapebase::{Mount, ShapeBody};
use ghosts::shapes::StaticShape;
use ghosts::{F32, TgeClass, TgeRecord, TgeShapeBase, Value, Xf};

use super::*;
use crate::world::tests::{datablock, ghost};

struct Files(HashMap<String, Vec<u8>>);

impl Assets for Files {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.get(&path.to_ascii_lowercase()).cloned()
    }
}

const TURRET: &str = "demo/data/shapes/turret.dts";

/// A world of a StaticShape at (10, 0, 0) whose shape has a `mount0` node, and `rider` mounted on it at node 0.
fn world(rider: TgeRecord, rider_class: TgeClass) -> (ClientWorld<TgeClientSet>, Files) {
    let b = [-1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
    let model = shapes::synth::hull_shape(b, &[("detail32", b)], &["mount0"], &[]);
    let files = Files([(TURRET.to_string(), shapes::write::write_dts(&model))].into());
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        3,
        datablock("StaticShapeData", &[("shapeFile", Value::str(TURRET))]),
    );
    let turret = StaticShape {
        shape: TgeShapeBase {
            game: ghosts::parts::GameBase {
                scale: None,
                datablock: Some(3),
            },
            body: None,
        },
        transform: Some((
            Xf {
                position: [F32::new(10.0), F32::new(0.0), F32::new(0.0)],
                quat: [F32::new(0.0); 3],
                negative_w: false,
            },
            [F32::new(1.0); 3],
        )),
        powered: false,
    };
    w.ghosts.insert(
        1,
        ghost(TgeClass::StaticShape, TgeRecord::StaticShape(turret)),
    );
    w.ghosts.insert(2, ghost(rider_class, rider));
    (w, files)
}

fn mounted_shape_base() -> TgeShapeBase {
    TgeShapeBase {
        game: Default::default(),
        body: Some(ShapeBody {
            mount: Some(Some(Mount { object: 1, node: 0 })),
            ..Default::default()
        }),
    }
}

#[test]
fn a_ghost_rides_its_mounts_node_and_an_unmounted_one_its_pose() {
    let rider = TgeRecord::StaticShape(StaticShape {
        shape: mounted_shape_base(),
        ..Default::default()
    });
    let (w, files) = world(rider, TgeClass::StaticShape);
    let mut models = Models::default();
    let mut notes = Vec::new();
    let m = mounted_transform(&w, &files, &mut models, &mut notes, 2, 0).expect("placed");
    assert_eq!(m.column(3), [10.0, 0.0, 0.0], "{notes:?}");
    // The mount itself rides nothing: drawn where its record puts it.
    assert!(mounted_transform(&w, &files, &mut models, &mut notes, 1, 0).is_none());
    let (at, scale) = pose_matrix(&render_pose(&w, 1).unwrap(), None).unwrap();
    assert_eq!((at.column(3), scale), ([10.0, 0.0, 0.0], [1.0; 3]));
}

#[test]
fn a_vehicle_the_client_runs_is_drawn_where_it_runs_it() {
    let rider = TgeRecord::StaticShape(StaticShape::default());
    let (mut w, _) = world(rider, TgeClass::StaticShape);
    let run = Pose::Quat {
        position: [1.0, 2.0, 3.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
    };
    w.vehicle_render.insert(1, run);
    assert_eq!(render_pose(&w, 1), Some(run));
}

#[test]
fn a_mount_chain_past_the_depth_is_not_followed() {
    // The turret rides itself: a cycle stops at once.
    let rider = TgeRecord::StaticShape(StaticShape::default());
    let (mut w, files) = world(rider, TgeClass::StaticShape);
    if let Some(TgeRecord::StaticShape(t)) = w.ghosts.get_mut(&1).and_then(|g| g.record.as_mut()) {
        t.shape.body = mounted_shape_base().body;
    }
    let mut models = Models::default();
    assert!(mounted_transform(&w, &files, &mut models, &mut Vec::new(), 1, 0).is_none());
}

/// A mount point the mount's shape has no node for: the rider sits at the mount's render transform, unscaled (TGE's).
#[test]
fn a_bare_mount_point_gives_the_mounts_render_transform_unscaled() {
    let mut base = mounted_shape_base();
    if let Some(body) = base.body.as_mut() {
        body.mount = Some(Some(Mount { object: 1, node: 5 }));
    }
    let rider = TgeRecord::StaticShape(StaticShape {
        shape: base,
        ..Default::default()
    });
    let (mut w, files) = world(rider, TgeClass::StaticShape);
    if let Some(TgeRecord::StaticShape(t)) = w.ghosts.get_mut(&1).and_then(|g| g.record.as_mut()) {
        t.shape.game.scale = Some([F32::new(2.0); 3]);
        if let Some((_, s)) = t.transform.as_mut() {
            *s = [F32::new(2.0); 3];
        }
    }
    assert_eq!(w.ghosts[&1].scale(), Some([2.0; 3]));
    let mut models = Models::default();
    let mut notes = Vec::new();
    let m = mounted_transform(&w, &files, &mut models, &mut notes, 2, 0).expect("placed");
    assert_eq!(m.column(0), [1.0, 0.0, 0.0], "{notes:?}");
    assert_eq!(m.column(3), [10.0, 0.0, 0.0]);
}

/// A chain of `links` scene-hierarchy links whose ghosts' records carry no pose, each ghost also mounted on its parent: no
/// link can be placed. The deepest ghost is `links + 1`.
fn unplaceable_chain(links: u32) -> ClientWorld<TgeClientSet> {
    use ghosts::block::{GhostBlock, LinkField};
    let mut w = ClientWorld::<TgeClientSet>::default();
    for index in 1..=links + 1 {
        let mut base = mounted_shape_base();
        if let Some(body) = base.body.as_mut() {
            body.mount = Some(Some(Mount {
                object: index - 1,
                node: 0,
            }));
        }
        let record = TgeRecord::StaticShape(StaticShape {
            shape: base,
            ..Default::default()
        });
        w.ghosts.insert(index, ghost(TgeClass::StaticShape, record));
    }
    for index in 2..=links + 1 {
        let block = GhostBlock {
            link: Some(LinkField::Parent {
                ghost: index - 1,
                node: None,
            }),
            pose: None,
        };
        let held = |i: u32| (1..=links + 1).contains(&i);
        w.links.apply(index, &block, &held, 0, 0);
    }
    assert_eq!(w.links.linked_count(), links as usize);
    w
}

#[test]
fn a_chain_of_unplaceable_links_costs_one_placement_a_ghost() {
    for links in [8, 16, hierarchy::HARD_DEPTH as u32] {
        let w = unplaceable_chain(links);
        let deepest = links + 1;
        let mut carried = Carried::default();
        let at = carried_transform(
            &w,
            &Files(HashMap::new()),
            &mut Models::default(),
            &mut Vec::new(),
            &mut carried,
            deepest,
            0,
        );
        // A ghost with a link follows its link alone: no parent can be placed, so it is drawn at its own pose.
        assert!(at.is_none());
        assert_eq!(carried.worked(), deepest as usize, "{links} links");
        // The rest of the frame's ghosts cost nothing more.
        for index in 1..=deepest {
            let _ = carried_transform(
                &w,
                &Files(HashMap::new()),
                &mut Models::default(),
                &mut Vec::new(),
                &mut carried,
                index,
                0,
            );
        }
        assert_eq!(carried.worked(), deepest as usize, "{links} links");
    }
}

#[test]
fn a_loop_of_mounts_places_each_ghost_as_alone_whichever_is_asked_first() {
    // The turret (1) rides its rider (2), which rides the turret.
    let rider = TgeRecord::StaticShape(StaticShape {
        shape: mounted_shape_base(),
        ..Default::default()
    });
    let (mut w, files) = world(rider, TgeClass::StaticShape);
    if let Some(TgeRecord::StaticShape(t)) = w.ghosts.get_mut(&1).and_then(|g| g.record.as_mut()) {
        let mut base = mounted_shape_base();
        if let Some(body) = base.body.as_mut() {
            body.mount = Some(Some(Mount { object: 2, node: 0 }));
        }
        t.shape.body = base.body;
    }
    let alone = |index| {
        mounted_transform(
            &w,
            &files,
            &mut Models::default(),
            &mut Vec::new(),
            index,
            0,
        )
    };
    let mut carried = Carried::default();
    let mut shared = |index| {
        carried_transform(
            &w,
            &files,
            &mut Models::default(),
            &mut Vec::new(),
            &mut carried,
            index,
            0,
        )
    };
    let (first, second) = (shared(1), shared(2));
    assert_eq!((first, second), (alone(1), alone(2)));
}

#[test]
fn an_items_rotation_block_turns_about_minus_z_when_its_flag_is_set() {
    let item = |rotation| {
        let pose = Pose::Item {
            position: [1.0, 2.0, 3.0],
            rotation,
        };
        pose_matrix(&pose, None).expect("an Item's matrix").0
    };
    let up = item(Some((false, 1.0)));
    let down = item(Some((true, 1.0)));
    assert!(up.column(0)[1].abs() > 0.5);
    assert!((up.column(0)[1] + down.column(0)[1]).abs() < 1e-6);
    assert!((up.column(0)[0] - down.column(0)[0]).abs() < 1e-6);
    assert_eq!(down.column(0), item(Some((false, -1.0))).column(0));
}
