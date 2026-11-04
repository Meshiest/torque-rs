use super::*;
use crate::shape::{IntSet, Node, SubShape};

fn set(bits: &[usize]) -> IntSet {
    let mut s = IntSet::default();
    for b in bits {
        s.set(*b);
    }
    s
}

fn sequence(
    name: i32,
    flags: u32,
    priority: i32,
    keys: i32,
    rot: &[usize],
    tran: &[usize],
) -> Sequence {
    Sequence {
        name,
        flags,
        num_keyframes: keys,
        duration: 2.0,
        priority,
        first_ground_frame: 0,
        num_ground_frames: 0,
        base_rotation: 0,
        base_translation: 0,
        base_scale: 0,
        base_object_state: 0,
        base_decal_state: 0,
        first_trigger: 0,
        num_triggers: 0,
        tool_begin: 0.0,
        rotation_matters: set(rot),
        translation_matters: set(tran),
        scale_matters: IntSet::default(),
        decal_matters: IntSet::default(),
        ifl_matters: IntSet::default(),
        vis_matters: IntSet::default(),
        frame_matters: IntSet::default(),
        mat_frame_matters: IntSet::default(),
    }
}

/// Three nodes in a chain (0 root, 1 under 0, 2 under 1), each translated by 1 along x at rest, and one detail on one sub-shape.
fn chain() -> Shape {
    let mut s = Shape::empty();
    s.names = vec![
        b"root".to_vec(),
        b"mid".to_vec(),
        b"tip".to_vec(),
        b"slide".to_vec(),
        b"turn".to_vec(),
    ];
    let node = |name, parent| Node {
        name,
        parent,
        first_object: -1,
        first_child: -1,
        next_sibling: -1,
    };
    s.nodes = vec![node(0, -1), node(1, 0), node(2, 1)];
    s.sub_shapes = vec![SubShape {
        first_node: 0,
        first_object: 0,
        first_decal: 0,
        num_nodes: 3,
        num_objects: 0,
        num_decals: 0,
    }];
    s.details = vec![crate::shape::Detail {
        name: 0,
        sub_shape: 0,
        object_detail: 0,
        size: 1.0,
        average_error: 0.0,
        max_error: 0.0,
        poly_count: 0,
    }];
    s.default_rotations = vec![[0, 0, 0, 32767]; 3];
    s.default_translations = vec![[0.0; 3], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
    // "slide" moves node 1 from x=1 to x=3 over two keyframes; "turn" turns node 2 a quarter about z.
    s.node_translations = vec![[1.0, 0.0, 0.0], [3.0, 0.0, 0.0]];
    s.node_rotations = vec![[0, 0, 0, 32767], [0, 0, 23170, 23170]];
    s.sequences = vec![
        sequence(3, 0, 5, 2, &[], &[1]),
        sequence(4, 0, 3, 2, &[2], &[]),
    ];
    s
}

fn free() -> impl FnMut(u64) -> bool {
    |_| true
}

/// The chain with a third sequence, "hold", keeping node 1 at x = 1 (the default).
fn chain_with_hold() -> Shape {
    let mut shape = chain();
    shape
        .node_translations
        .extend([[1.0, 0.0, 0.0], [1.0, 0.0, 0.0]]);
    let mut hold = sequence(3, 0, 5, 2, &[], &[1]);
    hold.base_translation = 2;
    shape.sequences.push(hold);
    shape
}

fn x1(inst: &mut ShapeInstance, shape: &Shape) -> f32 {
    inst.animate(shape, 0, None, &mut free()).unwrap();
    inst.node_transforms()[1][3]
}

#[test]
fn a_transition_blends_from_the_pose_it_left_over_its_duration() {
    // `transitionToSequence`, `advancePos`, `handleTransitionNodes`.
    let shape = chain_with_hold();
    let mut inst = ShapeInstance::new(&shape, &mut free()).unwrap();
    let t = inst.create_thread(&shape).unwrap();
    inst.set_sequence(&shape, t, 0, 1.0);
    assert_eq!(x1(&mut inst, &shape), 3.0);
    inst.transition_to_sequence(&shape, t, 2, 0.0, 0.25);
    assert_eq!(inst.transition_count(), 1);
    assert_eq!(
        x1(&mut inst, &shape),
        3.0,
        "the transition starts at the old pose"
    );
    inst.advance_time(&shape, t, 0.125);
    assert!((inst.thread(t).unwrap().get_pos() - 0.5).abs() < 1e-6);
    assert!((x1(&mut inst, &shape) - 2.0).abs() < 1e-5, "half way");
    inst.advance_time(&shape, t, 0.125);
    assert_eq!(inst.transition_count(), 0, "cleared at its end");
    assert_eq!(x1(&mut inst, &shape), 1.0);
    assert_eq!(inst.thread(t).unwrap().sequence, Some(2));
}

#[test]
fn a_transition_run_backwards_goes_back_to_the_sequence_it_left() {
    let shape = chain_with_hold();
    let mut inst = ShapeInstance::new(&shape, &mut free()).unwrap();
    let t = inst.create_thread(&shape).unwrap();
    inst.set_sequence(&shape, t, 0, 1.0);
    inst.animate(&shape, 0, None, &mut free()).unwrap();
    inst.transition_to_sequence(&shape, t, 2, 0.0, 0.25);
    inst.thread_mut(t).unwrap().time_scale = -1.0;
    inst.advance_time(&shape, t, 0.1);
    let th = inst.thread(t).unwrap();
    assert!(!th.transition.in_transition);
    // Back at the old position, then moved by the step scaled from the transition to the sequence (-0.4 * 0.25 / 2).
    assert_eq!(th.sequence, Some(0));
    assert!((th.pos - 0.95).abs() < 1e-6, "{}", th.pos);
    assert!((x1(&mut inst, &shape) - 2.9).abs() < 1e-5);
}

#[test]
fn a_transition_begun_inside_another_starts_from_the_blended_pose() {
    // `updateTransitions` keeps the pose the last animate left, the blend included.
    let shape = chain_with_hold();
    let mut inst = ShapeInstance::new(&shape, &mut free()).unwrap();
    let t = inst.create_thread(&shape).unwrap();
    inst.set_sequence(&shape, t, 0, 1.0);
    inst.animate(&shape, 0, None, &mut free()).unwrap();
    inst.transition_to_sequence(&shape, t, 2, 0.0, 0.25);
    inst.advance_time(&shape, t, 0.125);
    assert!((x1(&mut inst, &shape) - 2.0).abs() < 1e-5);
    inst.transition_to_sequence(&shape, t, 0, 1.0, 0.25);
    assert!((x1(&mut inst, &shape) - 2.0).abs() < 1e-5);
    assert_eq!(inst.thread(t).unwrap().transition.duration, 0.25);
    inst.advance_time(&shape, t, 0.125);
    assert!((x1(&mut inst, &shape) - 2.5).abs() < 1e-5);
}

#[test]
fn a_pose_blend_moves_every_node_from_the_kept_pose_whatever_the_priorities() {
    // An addition of this crate (not part of Torque): a thread set with no transition moves its nodes at once; the blend carries them from the
    // kept pose by its weight, and the kept pose itself is what was drawn (the blend included).
    let shape = chain_with_hold();
    let mut inst = ShapeInstance::new(&shape, &mut free()).unwrap();
    let t = inst.create_thread(&shape).unwrap();
    inst.set_sequence(&shape, t, 0, 1.0);
    assert_eq!(x1(&mut inst, &shape), 3.0);
    let (rotations, translations) = inst.local_pose();
    assert_eq!(translations[1], [3.0, 0.0, 0.0]);
    inst.set_sequence(&shape, t, 2, 0.0);
    assert_eq!(
        x1(&mut inst, &shape),
        1.0,
        "no blend: the new sequence at once"
    );
    for (w, want) in [(0.0, 3.0), (0.25, 2.5), (0.5, 2.0), (1.0, 1.0)] {
        inst.set_pose_blend(Some(PoseBlend {
            rotations: rotations.clone(),
            translations: translations.clone(),
            weight: w,
        }));
        assert!((x1(&mut inst, &shape) - want).abs() < 1e-5, "weight {w}");
        assert!((inst.local_pose().1[1][0] - want).abs() < 1e-5);
    }
    inst.set_pose_blend(None);
    assert_eq!(x1(&mut inst, &shape), 1.0);
    assert!(inst.pose_blend().is_none());
}

#[test]
fn a_new_instance_is_at_the_default_pose() {
    let shape = chain();
    let inst = ShapeInstance::new(&shape, &mut free()).unwrap();
    let m = inst.node_transforms();
    assert_eq!(m[0], IDENTITY);
    assert_eq!((m[1][3], m[2][3]), (1.0, 2.0));
    assert!(inst.dirty(0).is_some_and(|d| d == 0));
}

#[test]
fn a_thread_marks_the_pose_dirty_and_animate_makes_it() {
    let shape = chain();
    let mut inst = ShapeInstance::new(&shape, &mut free()).unwrap();
    let t = inst.create_thread(&shape).unwrap();
    assert_eq!(inst.thread(t).unwrap().sequence, Some(0));
    inst.set_sequence(&shape, t, 0, 1.0);
    // The matrices are stale until animate runs.
    assert_eq!(inst.node_transforms()[1][3], 1.0);
    inst.animate(&shape, 0, None, &mut free()).unwrap();
    assert_eq!(inst.node_transforms()[1][3], 3.0);
    assert_eq!(inst.node_transforms()[2][3], 4.0);
    assert_eq!(inst.dirty(0), Some(0));
}

#[test]
fn position_picks_the_keyframe_pair() {
    let shape = chain();
    let mut inst = ShapeInstance::new(&shape, &mut free()).unwrap();
    let t = inst.create_thread(&shape).unwrap();
    inst.set_sequence(&shape, t, 0, 0.5);
    let th = inst.thread(t).unwrap();
    assert_eq!((th.key_num1, th.key_num2, th.key_pos), (0, 1, 0.5));
    inst.animate(&shape, 0, None, &mut free()).unwrap();
    assert_eq!(inst.node_transforms()[1][3], 2.0);
}

#[test]
fn advance_time_moves_by_the_duration() {
    let shape = chain();
    let mut inst = ShapeInstance::new(&shape, &mut free()).unwrap();
    let t = inst.create_thread(&shape).unwrap();
    inst.set_sequence(&shape, t, 0, 0.0);
    inst.advance_time(&shape, t, 1.0);
    assert_eq!(inst.thread(t).unwrap().pos, 0.5);
    inst.advance_time(&shape, t, 10.0);
    assert_eq!(inst.thread(t).unwrap().pos, 1.0);
    assert_eq!(inst.thread(t).unwrap().key_pos, 0.0);
}

#[test]
fn a_cyclic_sequence_wraps() {
    let mut shape = chain();
    shape.sequences[0].flags = FLAG_CYCLIC;
    let mut inst = ShapeInstance::new(&shape, &mut free()).unwrap();
    let t = inst.create_thread(&shape).unwrap();
    inst.set_sequence(&shape, t, 0, 0.75);
    inst.advance_time(&shape, t, 1.0);
    assert_eq!(inst.thread(t).unwrap().pos, 0.25);
}

#[test]
fn the_higher_priority_thread_decides_a_node() {
    let mut shape = chain();
    // A second sequence that also moves node 1, to x = 10.
    shape
        .node_translations
        .extend([[10.0, 0.0, 0.0], [10.0, 0.0, 0.0]]);
    let mut low = sequence(4, 0, 1, 2, &[], &[1]);
    low.base_translation = 2;
    shape.sequences.push(low);
    let mut inst = ShapeInstance::new(&shape, &mut free()).unwrap();
    let a = inst.create_thread(&shape).unwrap();
    let b = inst.create_thread(&shape).unwrap();
    inst.set_sequence(&shape, a, 0, 1.0);
    inst.set_sequence(&shape, b, 2, 1.0);
    inst.animate(&shape, 0, None, &mut free()).unwrap();
    assert_eq!(
        inst.node_transforms()[1][3],
        3.0,
        "priority 5 beats priority 1"
    );
}

#[test]
fn a_blend_thread_multiplies_onto_the_local_matrix() {
    let mut shape = chain();
    shape.sequences[1].flags = FLAG_BLEND;
    let mut inst = ShapeInstance::new(&shape, &mut free()).unwrap();
    let t = inst.create_thread(&shape).unwrap();
    inst.set_sequence(&shape, t, 1, 1.0);
    inst.animate(&shape, 0, None, &mut free()).unwrap();
    // Node 2 turned a quarter about z on top of its default: its x axis now points along y.
    let m = inst.node_transforms()[2];
    assert!(m[1].abs() > 0.99 && m[0].abs() < 0.01, "{m:?}");
    assert_eq!(m[3], 2.0);
}

#[test]
fn a_uniform_scale_sequence_scales_the_node_and_what_hangs_from_it() {
    let mut shape = chain();
    shape.node_uniform_scales = vec![2.0, 4.0];
    let mut scaling = sequence(3, 1, 5, 2, &[], &[]);
    scaling.scale_matters = set(&[1]);
    shape.sequences.push(scaling);
    let mut inst = ShapeInstance::new(&shape, &mut free()).unwrap();
    let t = inst.create_thread(&shape).unwrap();
    // The flag follows `set_sequence`, as Torque's does: a new thread does not raise it.
    inst.set_sequence(&shape, t, 2, 0.5);
    inst.animate(&shape, 0, None, &mut free()).unwrap();
    let (node1, node2) = (inst.node_transforms()[1], inst.node_transforms()[2]);
    // Node 1 scaled by 3 (the blend of 2 and 4); its translation is not scaled; node 2 hangs from it.
    assert_eq!(
        (node1[0], node1[5], node1[10], node1[3]),
        (3.0, 3.0, 3.0, 1.0)
    );
    assert_eq!((node2[0], node2[3]), (3.0, 4.0));
    // Another sequence on the thread takes the scale away again.
    inst.set_sequence(&shape, t, 0, 1.0);
    inst.animate(&shape, 0, None, &mut free()).unwrap();
    assert_eq!(inst.node_transforms()[1][0], 1.0);
}

#[test]
fn an_arbitrary_scale_sequence_scales_along_its_axes() {
    let mut shape = chain();
    // A quarter turn about z with factors (2, 1, 1): the x factor lands on the y axis of the node.
    shape.node_arbitrary_scale_rots = vec![[0, 0, 23170, 23170]; 2];
    shape.node_arbitrary_scale_factors = vec![[2.0, 1.0, 1.0]; 2];
    let mut scaling = sequence(3, 4, 5, 2, &[], &[]);
    scaling.scale_matters = set(&[1]);
    shape.sequences.push(scaling);
    let mut inst = ShapeInstance::new(&shape, &mut free()).unwrap();
    let t = inst.create_thread(&shape).unwrap();
    inst.set_sequence(&shape, t, 2, 0.0);
    inst.animate(&shape, 0, None, &mut free()).unwrap();
    let m = inst.node_transforms()[1];
    assert!(
        (m[0] - 1.0).abs() < 1e-3 && (m[5] - 2.0).abs() < 1e-3,
        "{m:?}"
    );
}

#[test]
fn a_shape_with_no_sequences_has_no_thread() {
    let mut shape = chain();
    shape.sequences.clear();
    let mut inst = ShapeInstance::new(&shape, &mut free()).unwrap();
    assert!(inst.create_thread(&shape).is_none());
}

#[test]
fn an_unknown_sequence_changes_nothing() {
    let shape = chain();
    let mut inst = ShapeInstance::new(&shape, &mut free()).unwrap();
    let t = inst.create_thread(&shape).unwrap();
    inst.set_sequence(&shape, t, 99, 0.5);
    assert_eq!(inst.thread(t).unwrap().pos, 0.0);
    inst.set_sequence(&shape, t, -1, 0.5);
    assert_eq!(inst.thread(t).unwrap().sequence, Some(0));
}

#[test]
fn the_quaternion_blend_has_the_unit_cases() {
    let id = [0.0, 0.0, 0.0, 1.0];
    // The scale is Torque's polynomial for 1 / sqrt, not the root: at length 1 it is 1.0000371.
    let same = interpolate_quat(id, id, 0.5);
    assert_eq!(same, [0.0, 0.0, 0.0, 1.000_037_1]);
    // The shorter way round: the FIRST quaternion is negated when the dot product is below zero.
    let q = interpolate_quat(id, [0.0, 0.0, 0.0, -1.0], 0.5);
    assert_eq!(q, [0.0, 0.0, 0.0, -1.000_037_1]);
}

#[test]
fn the_thread_sort_is_stable() {
    // Priorities 2, 1, 2, 1, 2: the higher first, equal threads in the order they were in.
    let prio = [2, 1, 2, 1, 2];
    let mut order: Vec<usize> = (0..prio.len()).collect();
    let cmp = |a: &usize, b: &usize| prio[*b] - prio[*a];
    (crate::library::get().sort_threads)(&mut order, &cmp);
    assert_eq!(order, vec![0, 2, 4, 1, 3]);
}

#[test]
fn work_is_charged_and_a_budget_stops_the_animate() {
    let shape = chain();
    let mut spent = 0u64;
    let mut count = |n: u64| {
        spent += n;
        true
    };
    let _ = ShapeInstance::new(&shape, &mut count).unwrap();
    assert!(spent > 0);
    let mut none = |_: u64| false;
    assert_eq!(
        ShapeInstance::new(&shape, &mut none),
        Err(ShapeError::OutOfBudget)
    );
}

#[test]
fn hostile_indices_do_not_panic() {
    let mut shape = chain();
    shape.sequences[0].base_translation = i32::MAX;
    shape.sequences[0].num_keyframes = i32::MAX;
    let mut inst = ShapeInstance::new(&shape, &mut free()).unwrap();
    let t = inst.create_thread(&shape).unwrap();
    inst.set_sequence(&shape, t, 0, f32::NAN);
    inst.advance_time(&shape, t, f32::INFINITY);
    inst.animate(&shape, 0, None, &mut free()).unwrap();
    inst.set_sequence(&shape, t, 0, 1e30);
    inst.animate(&shape, 0, None, &mut free()).unwrap();
}
