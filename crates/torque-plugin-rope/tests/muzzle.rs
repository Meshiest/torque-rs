//! The muzzle anchors: a rope's end at the muzzle of an image a ghost holds, as the game drew it this frame, and which muzzle that is
//! for the player whose view is first person.

use torque_plugin_rope::{DrawnMuzzle, FrameAnchors, MuzzleAnchors, MuzzleTable};
use torque_render_scene::hd::DynamicObject;
use torque_render_scene::math::Mat;

fn shape(ghost: u32, at: [f32; 3]) -> DynamicObject {
    let mut transform = Mat::IDENTITY;
    transform.set_column3(at);
    DynamicObject {
        ghost,
        transform,
        prev_transform: transform,
        translucent: false,
        centre: at,
        radius: 1.0,
        draws: Vec::new(),
        previous: None,
        stamp: None,
        nodes: Vec::new(),
    }
}

fn muzzle(ghost: u32, slot: usize, at: [f32; 3], first_person: bool) -> DrawnMuzzle {
    DrawnMuzzle {
        ghost,
        slot,
        at,
        dir: [0.0, 1.0, 0.0],
        first_person,
    }
}

#[test]
fn the_first_person_muzzle_is_for_a_first_person_view_and_the_mount_otherwise() {
    let table = MuzzleTable::new();
    let third = muzzle(3, 0, [1.0, 2.0, 1.2], false);
    let first = muzzle(3, 0, [1.1, 2.3, 1.5], true);
    let other = muzzle(8, 0, [9.0, 9.0, 1.0], false);
    // A first-person view: the controlled player's viewmodel muzzle; another player's is its mount's whatever the view.
    table.set([third, first, other], true);
    assert_eq!(table.find(3, 0), Some(first));
    assert_eq!(table.find(8, 0), Some(other));
    // A third-person view: the mount's.
    table.set([third, first, other], false);
    assert_eq!(table.find(3, 0), Some(third));
    // A game that gives only the one it drew: that one, in either view.
    table.set([first], false);
    assert_eq!(table.find(3, 0), Some(first));
    table.set([third], true);
    assert_eq!(table.find(3, 0), Some(third));
    // No muzzle for a slot with no image, or a ghost with none.
    assert_eq!(table.find(3, 1), None);
    assert_eq!(table.find(4, 0), None);
}

#[test]
fn the_anchors_give_the_muzzle_with_its_barrel_and_fall_back_to_the_shape() {
    let table = MuzzleTable::new();
    let anchors = MuzzleAnchors::new(table.clone());
    let player = shape(3, [10.0, 20.0, 0.0]);
    table.set(
        [
            muzzle(3, 0, [10.5, 20.9, 1.4], false),
            muzzle(3, 2, [10.6, 21.0, 1.3], false),
        ],
        false,
    );
    let at = |node: &str| anchors.mount(&player, node);
    let m = at("muzzle").unwrap();
    assert_eq!((m.at, m.dir), ([10.5, 20.9, 1.4], Some([0.0, 1.0, 0.0])));
    assert_eq!(at("muzzle0"), at("muzzle"));
    assert_eq!(at("muzzle2").unwrap().at, [10.6, 21.0, 1.3]);
    assert_eq!(anchors.place(&player, "muzzle"), Some([10.5, 20.9, 1.4]));
    // An image with no muzzle node (no entry): the shape's origin, pointing along its forward.
    let m = at("muzzle1").unwrap();
    assert_eq!((m.at, m.dir), ([10.0, 20.0, 0.0], Some([0.0, 1.0, 0.0])));
    // A ghost that holds nothing the table knows: the same.
    assert_eq!(
        anchors
            .mount(&shape(9, [1.0, 2.0, 3.0]), "muzzle")
            .unwrap()
            .at,
        [1.0, 2.0, 3.0]
    );
    // Another node is the shape's origin with no direction.
    let m = at("hand").unwrap();
    assert_eq!((m.at, m.dir), ([10.0, 20.0, 0.0], None));
    // A ghost whose shape is not drawn (the player's own, in first person) still has its muzzle.
    let ghost = anchors.ghost_mount(3, "muzzle").unwrap();
    assert_eq!(
        (ghost.at, ghost.dir),
        ([10.5, 20.9, 1.4], Some([0.0, 1.0, 0.0]))
    );
    assert_eq!(
        anchors.ghost_mount(3, "muzzle1"),
        None,
        "no muzzle, no mount to give"
    );
    assert_eq!(anchors.ghost_mount(3, "hand"), None);
    // The table is read each frame: a new set is seen at once.
    table.set([muzzle(3, 0, [0.0, 0.0, 0.0], false)], false);
    assert_eq!(at("muzzle").unwrap().at, [0.0; 3]);
}
