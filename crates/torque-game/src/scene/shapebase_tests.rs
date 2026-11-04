use std::rc::Rc;

use shapes::instance::{ShapeInstance, ThreadId};
use shapes::synth::hull_shape;
use ts_vm::vm::Vm;

use super::{ActionTable, ShapeBody, TGE_ACTIONS, player_actions, player_threads};
use crate::shapes::KeptShape;
use crate::sim_classes::{PlayerKind, SimClasses, TorqueClasses, TorquePlayerKind};

const BOX: [f32; 6] = [-1.0, -1.0, -1.0, 1.0, 1.0, 1.0];

/// Without a host's Player class the action table is TGE's: root, the three moving actions with their directions, fall, the
/// two jumps and land; fall is 4, land 7, no crouching actions, 520 actions in all.
#[test]
fn the_default_action_table_is_tges() {
    let t = TorqueClasses.action_table();
    assert!(std::ptr::eq(t, &TGE_ACTIONS));
    assert_eq!(
        t.fixed,
        [
            "root",
            "run",
            "back",
            "side",
            "fall",
            "jump",
            "standjump",
            "land"
        ]
    );
    assert_eq!(t.dirs.len(), t.fixed.len());
    assert_eq!(
        t.dirs[..4],
        [
            [0.0; 3],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [-1.0, 0.0, 0.0]
        ]
    );
    assert!(t.dirs[4..].iter().all(|d| *d == [0.0; 3]));
    assert_eq!((t.fall, t.land, t.crouch, t.max), (4, 7, None, 520));
    assert_eq!((t.jump, t.stand_jump, t.none), (5, 6, 511));
    assert_eq!(t.fixed[t.jump as usize], "jump");
    assert_eq!(t.fixed[t.stand_jump as usize], "standjump");
    assert_eq!(t.numbers(), ::sim::player::TGE_ACTION_NUMBERS);
    assert_eq!(t.fixed[t.fall as usize], "fall");
    assert_eq!(t.fixed[t.land as usize], "land");
}

/// The action list: the fixed actions by name (-1 where the shape has no such sequence), then every other sequence of the
/// shape in its order, named as the shape names it.
#[test]
fn the_action_list_puts_the_fixed_actions_first() {
    let m = hull_shape(
        BOX,
        &[("Collision-1", BOX)],
        &[],
        &["look", "land", "run", "wave"],
    );
    let actions = player_actions(&m.shape, &TGE_ACTIONS);
    let names: Vec<&[u8]> = actions.iter().map(|a| a.name.as_slice()).collect();
    let expected: [&[u8]; 10] = [
        b"root",
        b"run",
        b"back",
        b"side",
        b"fall",
        b"jump",
        b"standjump",
        b"land",
        b"look",
        b"wave",
    ];
    assert_eq!(names, expected);
    let sequences: Vec<i32> = actions.iter().map(|a| a.sequence).collect();
    assert_eq!(sequences, [-1, 2, -1, -1, -1, -1, -1, 1, 0, 3]);
}

/// A table keeps no more actions than its size.
#[test]
fn the_action_list_stops_at_the_tables_size() {
    let small = ActionTable {
        max: 9,
        ..TGE_ACTIONS
    };
    let m = hull_shape(
        BOX,
        &[("Collision-1", BOX)],
        &[],
        &["look", "wave", "dance"],
    );
    let actions = player_actions(&m.shape, &small);
    assert_eq!(actions.len(), 9);
    assert_eq!(actions[8].name, b"look");
}

/// A body of a box shape with the given sequences.
fn body_with(sequences: &[&str]) -> ShapeBody {
    let m = hull_shape(BOX, &[("Collision-1", BOX)], &[], sequences);
    let kept = Rc::new(KeptShape::holding(m.shape));
    let shape = kept.shape.as_ref().expect("held");
    let instance = ShapeInstance::new(shape, &mut |_| true).expect("an instance");
    ShapeBody {
        kept,
        instance,
        damage: None,
        visibility: None,
    }
}

/// TGE's Player makes the arm thread (the shape has a `look` action), the `head` and `headside` threads and the action thread,
/// and no thread of its own: the shape's other sequences make none. No action is chosen yet; the arm plays `look`.
#[test]
fn tges_player_makes_the_arm_head_headside_and_action_threads() {
    let mut body = body_with(&["look", "head", "headside", "wave"]);
    let vm = Vm::new();
    let class = |b: &mut ShapeBody| TorqueClasses.player_class_threads(&vm, 1, b);
    let p = player_threads(&mut body, None, &TGE_ACTIONS, class).expect("threads");
    assert!(
        p.arm.is_some() && p.head.is_some() && p.head_side.is_some() && p.action_thread.is_some()
    );
    assert!(p.class_threads.is_empty());
    assert_eq!(body.instance.thread_count(), 4);
    assert_eq!(p.action, 511);
    assert_eq!(p.actions[p.arm_action as usize].name, b"look");
}

/// A class's own threads are made after `headside` and before the action thread, and kept by their numbers.
#[test]
fn a_class_makes_its_threads_before_the_action_thread() {
    let mut body = body_with(&["head", "headside", "wave"]);
    let class = |b: &mut ShapeBody| {
        let shape = Rc::clone(&b.kept);
        vec![
            None,
            b.instance
                .create_thread(shape.shape.as_ref().expect("held")),
        ]
    };
    let p = player_threads(&mut body, None, &TGE_ACTIONS, class).expect("threads");
    let id = |t: Option<ThreadId>| t.expect("made").0;
    assert_eq!(p.class_threads.len(), 2);
    assert!(p.class_threads[0].is_none());
    let own = id(p.class_threads[1]);
    assert!(id(p.head_side) < own && own < id(p.action_thread));
}

/// TGE's Player sizes its box from `boundingBox` and has no threads of its own to step.
#[test]
fn tges_player_box_is_the_bounding_box_and_it_steps_no_threads_of_its_own() {
    let vm = Vm::new();
    assert_eq!(TorqueClasses.player_box_field(&vm, 1), b"boundingBox");
    let mut body = body_with(&["wave"]);
    assert_eq!(
        TorquePlayerKind.advance_class_threads(&mut body, &[], 0.032),
        0
    );
}
