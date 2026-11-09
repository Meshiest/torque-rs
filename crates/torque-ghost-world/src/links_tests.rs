use std::collections::BTreeSet;

use ghosts::block::{GhostBlock, LinkField, PoseField};
use hierarchy::{Local, NodeRef, QuantLocal};

use super::*;

fn pose_of(local: &Local, snap: bool) -> PoseField {
    let q = QuantLocal::from_local(local);
    PoseField {
        translation: q.translation,
        dropped: q.rotation.dropped,
        parts: q.rotation.parts,
        scale: None,
        snap,
    }
}

fn at(x: f32) -> Local {
    Local {
        translation: [x, 0.0, 0.0],
        ..Local::IDENTITY
    }
}

fn linked(parent: u32, local: &Local) -> GhostBlock {
    GhostBlock {
        link: Some(LinkField::Parent {
            ghost: parent,
            node: None,
        }),
        pose: Some(pose_of(local, false)),
    }
}

struct Table(BTreeSet<u32>);

impl Table {
    fn of(ids: &[u32]) -> Table {
        Table(ids.iter().copied().collect())
    }

    fn has(&self) -> impl Fn(u32) -> bool + '_ {
        |i| self.0.contains(&i)
    }
}

#[test]
fn a_link_to_a_held_ghost_resolves_at_once_and_a_root_link_takes_it_off() {
    let ghosts = Table::of(&[1, 2]);
    let mut links = ClientLinks::default();
    links.apply(2, &linked(1, &at(3.0)), &ghosts.has(), 0, 0);
    assert_eq!(links.parent(2).map(|p| p.object.0), Some(1));
    assert_eq!(links.local(2, 0, false), Some(at(3.0)));
    let root = GhostBlock {
        link: Some(LinkField::Root),
        pose: None,
    };
    links.apply(2, &root, &ghosts.has(), 0, 0);
    assert_eq!(links.parent(2), None);
    assert_eq!(links.local(2, 0, false), None);
    links.audit().expect("the tree's rules hold");
}

#[test]
fn a_link_to_a_ghost_not_yet_made_waits_then_resolves_or_expires() {
    let mut ghosts = Table::of(&[2, 3]);
    let mut links = ClientLinks::default();
    links.apply(2, &linked(1, &at(1.0)), &ghosts.has(), 100, 0);
    links.apply(3, &linked(9, &at(1.0)), &ghosts.has(), 100, 0);
    assert!(links.is_waiting(2) && links.parent(2).is_none());
    ghosts.0.insert(1);
    links.created(1);
    links.settle(&ghosts.has(), 200);
    assert_eq!(links.parent(2).map(|p| p.object.0), Some(1));
    assert_eq!(links.local(2, 0, false), Some(at(1.0)));
    // Ghost 9 never comes: the link goes after five seconds, counted.
    links.settle(&ghosts.has(), 100 + WAIT_MS);
    assert!(links.is_waiting(3));
    links.settle(&ghosts.has(), 101 + WAIT_MS);
    assert!(!links.is_waiting(3));
    assert_eq!(links.stats().expired, 1);
}

#[test]
fn no_more_than_the_cap_wait() {
    let ids: Vec<u32> = (0..MAX_WAITING as u32 + 10).collect();
    let ghosts = Table::of(&ids);
    let mut links = ClientLinks::default();
    for &i in &ids {
        links.apply(i, &linked(1_000_000, &at(0.0)), &ghosts.has(), 0, 0);
    }
    assert_eq!(links.waiting_count(), MAX_WAITING);
    assert_eq!(links.stats().over_cap, 10);
}

#[test]
fn a_cycle_or_a_self_link_is_refused_and_counted() {
    let ghosts = Table::of(&[1, 2]);
    let mut links = ClientLinks::default();
    links.apply(2, &linked(1, &at(0.0)), &ghosts.has(), 0, 0);
    links.apply(1, &linked(2, &at(0.0)), &ghosts.has(), 0, 0);
    links.apply(1, &linked(1, &at(0.0)), &ghosts.has(), 0, 0);
    assert_eq!(links.parent(1), None);
    assert_eq!(links.stats().refused, 2);
    links.audit().expect("the tree's rules hold");
}

#[test]
fn an_out_of_range_pose_is_refused_and_the_last_one_stands() {
    let ghosts = Table::of(&[1, 2]);
    let mut links = ClientLinks::default();
    links.apply(2, &linked(1, &at(2.0)), &ghosts.has(), 0, 0);
    let mut bad = pose_of(&at(5.0), false);
    bad.translation[0] = 16_000_000;
    links.apply(
        2,
        &GhostBlock {
            link: None,
            pose: Some(bad),
        },
        &ghosts.has(),
        0,
        0,
    );
    assert_eq!(links.stats().bad_poses, 1);
    assert_eq!(links.local(2, 0, false), Some(at(2.0)));
}

#[test]
fn a_new_pose_blends_over_a_tick_unless_snapped() {
    let ghosts = Table::of(&[1, 2]);
    let mut links = ClientLinks::default();
    links.apply(2, &linked(1, &at(0.0)), &ghosts.has(), 0, 1_000);
    let moved = |snap| GhostBlock {
        link: None,
        pose: Some(pose_of(&at(4.0), snap)),
    };
    links.apply(2, &moved(false), &ghosts.has(), 0, 1_000);
    let half = links.local(2, 1_000 + BLEND_MS / 2, true).expect("a pose");
    assert!((half.translation[0] - 2.0).abs() < 1e-3, "{half:?}");
    assert_eq!(links.local(2, 1_000 + BLEND_MS, true), Some(at(4.0)));
    // Not animating: the last pose sent.
    assert_eq!(links.local(2, 1_000, false), Some(at(4.0)));
    links.apply(2, &moved(true), &ghosts.has(), 0, 2_000);
    assert_eq!(links.local(2, 2_000, true), Some(at(4.0)));
}

#[test]
fn a_parent_that_goes_leaves_its_children_where_they_were() {
    let mut ghosts = Table::of(&[1, 2, 3]);
    let mut links = ClientLinks::default();
    links.apply(2, &linked(1, &at(1.0)), &ghosts.has(), 0, 0);
    links.apply(3, &linked(2, &at(1.0)), &ghosts.has(), 0, 0);
    let mut held = IDENTITY;
    held[3] = 7.0;
    ghosts.0.remove(&2);
    links.deleted(2, [(3, (held, [1.0; 3]))], &ghosts.has());
    assert_eq!(links.parent(3), None);
    assert_eq!(links.held(3), Some((held, [1.0; 3])));
    assert_eq!(links.children(1).count(), 0);
    // Its next update lets go of the held pose.
    links.apply(3, &GhostBlock::default(), &ghosts.has(), 0, 0);
    assert_eq!(links.held(3), None);
    links.audit().expect("the tree's rules hold");
}

#[test]
fn a_node_link_keeps_its_node() {
    let ghosts = Table::of(&[1, 2]);
    let mut links = ClientLinks::default();
    let block = GhostBlock {
        link: Some(LinkField::Parent {
            ghost: 1,
            node: Some(4),
        }),
        pose: Some(pose_of(&at(0.5), false)),
    };
    links.apply(2, &block, &ghosts.has(), 0, 0);
    assert_eq!(links.parent(2).map(|p| p.node), Some(NodeRef::Node(4)));
}

#[test]
fn a_link_waiting_for_a_ghost_that_went_does_not_take_the_next_ghost_at_its_index() {
    let mut ghosts = Table::of(&[2]);
    let mut links = ClientLinks::default();
    links.apply(2, &linked(9, &at(1.0)), &ghosts.has(), 100, 0);
    assert!(links.is_waiting(2));
    // Ghost 9 comes and goes before the waiting links are tried, then its index is given to another object, all within
    // the wait.
    ghosts.0.insert(9);
    links.created(9);
    ghosts.0.remove(&9);
    links.deleted(9, [], &ghosts.has());
    ghosts.0.insert(9);
    links.created(9);
    links.settle(&ghosts.has(), 200);
    assert!(!links.is_waiting(2));
    assert_eq!(links.parent(2), None);
    links.audit().expect("the tree's rules hold");
}
