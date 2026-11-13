//! The animated pose of a shape: Torque's `TSShapeInstance` node transforms and `TSThread`, as data.
//!
//! A [`ShapeInstance`] is what a `ShapeBase` keeps beside its datablock's shape: one matrix per node, the threads that play the
//! shape's sequences, and the dirty flags that say which sub-shapes need their nodes made again. It holds no shape; every call
//! takes the `&Shape` it was made for, so many instances share one parsed shape (the caller keeps the `Arc`). Nothing here reads
//! a file or a script, and every loop is charged to the callback before it runs ([`crate::Charge`]).
//!
//! **What is followed, and where.**
//!
//! | piece | Torque |
//! |---|---|
//! | [`ShapeInstance::new`] | `TSShapeInstance` constructor: every sub-shape is animated once, so nodes start at default pose |
//! | [`ShapeInstance::create_thread`] | `addThread`: none with no sequences; the new thread plays sequence 0 at position 0 |
//! | [`ShapeInstance::set_sequence`] | `TSShapeInstance::setSequence` over `TSThread::setSequence` |
//! | [`ShapeInstance::advance_pos`], [`ShapeInstance::advance_time`] | `TSThread::advancePos` and `advanceTime` |
//! | [`ShapeInstance::animate`] | `animate` (sorts marked threads, then makes the sub-shape's nodes) |
//! | node poses | `animateNodes`, `handleBlendSequence`, the quaternion blend |
//!
//! **Staleness is Torque's.** A thread's sequence, position or direction marks the sub-shapes dirty; matrices change when
//! `animate` runs, and Torque runs it from its collision queries, its eye and muzzle lookups and the client's drawing, not from
//! `playThread`. `getSlotTransform` reads the matrices as they are. So a pose read straight after `playThread` is the
//! pose of the last `animate`.
//!
//! **Arithmetic.** The rotations, blends, casts and the thread sort go through the shape library table
//! ([`crate::library`]); its default is plain `f32` arithmetic in the order Torque's source writes it and a stable sort.
//!
//! **Limits.**
//! - *Transitions* are followed for rotations and translations ([`ShapeInstance::transition_to_sequence`] over the thread's
//!   half, `updateTransitions`, `clearTransition`, the transition branch of `advancePos` and
//!   `handleTransitionNodes` inside `animateNodes`). Their only caller is the client branch of `Player::setActionThread`
//!   (it tests the ghost bit), so a server instance never has a thread in transition. Not followed: the scale nodes' transition
//!   (reference arrays are not kept) and the translation loop's missing lower bound (it
//!   blends the translations of the nodes below the sub-shape too, which only another sub-shape's later pass would read).
//! - *Scale animation* is followed (the thread pass, the defaults, the application, the blend branch of
//!   `handleBlendSequence`). The shape's scale kind is the largest `flags & 7` among its sequences
//!   *now*; Torque caches it at init, so a sequence a `TSShapeConstructor` adds later that raises the kind is seen here and
//!   not there. The pairs (sequence kind, shape kind) that Torque keeps in global scratch arrays (a lower kind in a higher
//!   kind's shape: cases 3, 6 and 7) keep the identity scale here, where Torque would use whatever an earlier shape left.
//! - *Path and trigger tracking* (`makePath`): the position update is followed; the trigger bookkeeping is client-only and left
//!   out.
//! - Shapes over 192 nodes: Torque's node sets are six words on the stack (a longer shape overflows them); here the nodes past
//!   192 take their default pose and no thread moves them.

use std::cmp::Ordering;

use crate::library;
use crate::math::{self, IDENTITY, Matrix, quat16};
use crate::shape::{IntSet, Sequence, Shape};
use crate::swing;
use crate::{Charge, ShapeError, pay};

/// Nodes a six-word set (`TSIntegerSet`) can name.
pub const SET_BITS: usize = 192;
/// Threads one instance may hold (Torque has no limit; a datablock makes a handful).
pub const MAX_THREADS: usize = 256;
/// Work charged for one node of an animate (its matrix, the default pose and the compose).
pub const NODE_WORK: u64 = 64;
/// Work charged for one node a thread animates (an interpolated rotation or translation).
pub const THREAD_NODE_WORK: u64 = 32;

/// `TSShape::Sequence::flags`: the scale kinds, `Blend`, `Cyclic` and `MakePath`.
pub const FLAG_SCALE_KINDS: u32 = 7;
pub const FLAG_BLEND: u32 = 8;
pub const FLAG_CYCLIC: u32 = 16;
pub const FLAG_MAKE_PATH: u32 = 32;

/// `TSShapeInstance` dirty flags: the nodes (`TransformDirty`) and the thread order (`ThreadSortDirty`) are the ones this crate
/// acts on.
pub const DIRTY_TRANSFORM: u32 = 1;
pub const DIRTY_VIS: u32 = 2;
pub const DIRTY_FRAME: u32 = 4;
pub const DIRTY_MAT_FRAME: u32 = 8;
pub const DIRTY_DECAL: u32 = 16;
pub const DIRTY_IFL: u32 = 32;
pub const DIRTY_THREAD_SORT: u32 = 64;
/// `AllDirtyMask` (0x7f).
pub const DIRTY_ALL: u32 = 0x7f;

/// The caller's function for the nodes of [`NodeMasks::callback`]: the node and its local matrix.
pub type NodeCallback<'a> = &'a mut dyn FnMut(usize, &mut Matrix);

/// A six-word set of node indices, as Torque's `TSIntegerSet` holds them.
pub type NodeSet = [u32; 6];

/// Whether `set` has bit `bit` (false past the set).
pub fn set_has(set: &NodeSet, bit: usize) -> bool {
    set.get(bit / 32)
        .is_some_and(|w| w & (1 << (bit % 32)) != 0)
}

fn set_add(set: &mut NodeSet, bit: usize) {
    if let Some(w) = set.get_mut(bit / 32) {
        *w |= 1 << (bit % 32);
    }
}

fn words(set: &IntSet) -> NodeSet {
    let mut out = [0u32; 6];
    for (o, w) in out.iter_mut().zip(&set.words) {
        *o = *w;
    }
    out
}

/// The nodes the caller marks for the Player's own animation: the seven sets `animateNodes` reads.
/// All empty on a server instance until the Player sets some.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeMasks {
    /// Nodes whose rotation is the caller's (the default pose gives them an identity and no thread moves them).
    pub mask_rotation: NodeSet,
    /// Nodes whose x, y or z translation is the caller's.
    pub mask_pos_x: NodeSet,
    pub mask_pos_y: NodeSet,
    pub mask_pos_z: NodeSet,
    /// Nodes the blend sequences leave alone.
    pub disable_blend: NodeSet,
    /// Nodes `animateNodes` does not touch at all (their matrices are set from outside).
    pub hands_off: NodeSet,
    /// Nodes whose matrix the caller's callback sets.
    pub callback: NodeSet,
}

/// A `TSThread`: which sequence it plays, where, and the keyframes that position selects.
#[derive(Debug, Clone, PartialEq)]
pub struct Thread {
    /// The sequence's priority, copied when the sequence is set.
    pub priority: i32,
    /// The sequence, an index into `shape.sequences`.
    pub sequence: Option<usize>,
    /// The position in the sequence, 0 to 1.
    pub pos: f32,
    /// The rate `advance_time` plays at: 1 from the constructor.
    pub time_scale: f32,
    /// The two keyframes the position lies between and the weight of the second.
    pub key_num1: i32,
    pub key_num2: i32,
    pub key_pos: f32,
    /// A blend sequence's thread can be left out of the blend.
    pub blend_disabled: bool,
    /// The sequence's `MakePath` flag.
    pub make_path: bool,
    /// The transition the thread is in.
    pub transition: Transition,
}

/// A thread's transition (`TSThread::transitionData`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Transition {
    /// Whether the thread is in a transition.
    pub in_transition: bool,
    /// The seconds it lasts; rescaled by `updateTransitions` when another starts.
    pub duration: f32,
    /// How far it has run, 0 to 1: the weight of the new pose.
    pub pos: f32,
    /// +1 or -1, the sign of the time scale when it began.
    pub direction: f32,
    /// The rate the new sequence plays at meanwhile: 1, `continuePlay`.
    pub target_scale: f32,
    /// The old sequence's nodes: its rotations, translations and scales, OR'ed over nested transitions.
    pub old_rotation_nodes: NodeSet,
    pub old_translation_nodes: NodeSet,
    pub old_scale_nodes: NodeSet,
    /// The sequence and position it left: a transition run backwards past its start goes back there.
    pub old_sequence: Option<usize>,
    pub old_pos: f32,
    /// The blend eases in and out (smoothstep of the position) instead of running straight: an addition of this crate, not
    /// part of Torque ([`ShapeInstance::transition_to_sequence_eased`]).
    pub eased: bool,
    /// With `eased`, the limbs swing in the shape's space, out to their sides (an addition of this crate,
    /// [`ShapeInstance::transition_to_sequence_swung`]).
    pub swung: bool,
}

impl Thread {
    /// `TSThread::getPos`: the transition's position while in one, else the sequence's.
    pub fn get_pos(&self) -> f32 {
        if self.transition.in_transition {
            self.transition.pos
        } else {
            self.pos
        }
    }
}

/// The handle of a thread of an instance. A destroyed thread's handle is dead; slots are not reused until the instance is dropped,
/// so a stale handle never names another thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThreadId(pub usize);

/// What `TSShapeInstance` keeps of a shape's animation.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeInstance {
    /// One matrix per node (`mNodeTransforms`).
    nodes: Vec<Matrix>,
    /// The thread arena; `None` once destroyed.
    arena: Vec<Option<Thread>>,
    /// `mThreadList`: the order `animate` plays the threads in.
    order: Vec<usize>,
    /// One word of dirty flags per sub-shape.
    dirty: Vec<u32>,
    /// The Player's node marks.
    pub masks: NodeMasks,
    /// `mScaleCurrentlyAnimated`: some thread plays a sequence that animates scale. Kept as Torque keeps it: set by
    /// `set_sequence` and `destroy_thread`, not by `create_thread`.
    scale_animated: bool,
    /// `mTransitionThreads`: the threads in transition, in the order they began.
    transition_threads: Vec<usize>,
    /// `mTransitionRotationNodes`, `mTransitionTranslationNodes` and `mTransitionScaleNodes`.
    transition_rotation_nodes: NodeSet,
    transition_translation_nodes: NodeSet,
    transition_scale_nodes: NodeSet,
    /// `mNodeReferenceRotations` (as `Quat16`) and `mNodeReferenceTranslations`: the pose a transition starts from.
    reference_rotations: Vec<[i16; 4]>,
    reference_translations: Vec<[f32; 3]>,
    /// Left by `animateNodes` in `smNodeCurrentRotations` and `smNodeCurrentTranslations` (globals in Torque, per instance here):
    /// the local pose after the threads and the transitions, which `updateTransitions` keeps as the reference.
    current_rotations: Vec<[f32; 4]>,
    current_translations: Vec<[f32; 3]>,
    /// A blend of every node from a kept local pose toward the threads' ([`PoseBlend`]); an addition of this crate, not part of
    /// Torque.
    pose_blend: Option<PoseBlend>,
}

/// A blend of the local pose (an addition of this crate, not part of Torque): each node's rotation and translation, as threads
/// and transitions give them, are moved from `rotations` and `translations` (a pose [`ShapeInstance::local_pose`] kept) toward their own by `weight`
/// (0 the kept pose, 1 the threads'), before the blend threads and the matrices. Nodes past the kept pose's length are left alone.
#[derive(Debug, Clone, PartialEq)]
pub struct PoseBlend {
    pub rotations: Vec<[f32; 4]>,
    pub translations: Vec<[f32; 3]>,
    pub weight: f32,
}

/// The index of the first sequence named `name` (`TSShape::findSequence`): the name's index in the name table, -1 when
/// there is none, and then the first sequence whose name index equals it (a sequence with no name has -1, and matches).
pub fn find_sequence(shape: &Shape, name: &[u8]) -> Option<usize> {
    let wanted = shape.find_name(name).unwrap_or(-1);
    shape.sequences.iter().position(|s| s.name == wanted)
}

/// A float truncated to an `int`: the `(S32)` cast (through the table).
fn truncate(x: f32) -> i32 {
    (library::get().to_int)(x)
}

/// `TSThread` position clamp (the head of `advancePos`): above 1 is 1, above 0 stays, anything else (0, below, NaN) is 0.
fn clamp01(x: f32) -> f32 {
    if x > 1.0 {
        1.0
    } else if x > 0.0 {
        x
    } else {
        0.0
    }
}

/// `TSTransform::interpolate(QuatF)` (through the table).
pub fn interpolate_quat(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    (library::get().interpolate_quat)(a, b, t)
}

/// `TSTransform::interpolate(QuatF)` as `tsTransform.cpp` writes it: a linear blend with a fast renormalisation. The first
/// quaternion is negated when the two are more than 90 degrees apart (`Dot < 0`); the blend `q1 + t * (q2 - q1)` is scaled by
/// a polynomial for `1 / sqrt(length squared)` (one quadratic below 0.857, another above).
pub fn plain_interpolate_quat(q1: [f32; 4], q2: [f32; 4], interp: f32) -> [f32; 4] {
    let [mut x1, mut y1, mut z1, mut w1] = q1;
    let [x2, y2, z2, w2] = q2;
    let dot = x1 * x2 + y1 * y2 + z1 * z2 + w1 * w2;
    if dot < 0.0 {
        x1 = -x1;
        y1 = -y1;
        z1 = -z1;
        w1 = -w1;
    }
    x1 += interp * (x2 - x1);
    y1 += interp * (y2 - y1);
    z1 += interp * (z2 - z1);
    w1 += interp * (w2 - w1);
    let dist2 = x1 * x1 + y1 * y1 + z1 * z1 + w1 * w1;
    let one_over_l = if dist2 < 0.857 {
        ((0.699368 * dist2) + -1.819985) * dist2 + 2.126369
    } else {
        ((0.454012 * dist2) + -1.403517) * dist2 + 1.949542
    };
    [
        x1 * one_over_l,
        y1 * one_over_l,
        z1 * one_over_l,
        w1 * one_over_l,
    ]
}

/// `TSTransform::interpolate(Point3F)` per component (through the table).
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let lerp = library::get().lerp;
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
}

/// Whether the sequence moves any node, any visibility...: the dirty flags `TSShape::init` computes (`dirtyFlags`).
fn sequence_dirty(s: &Sequence) -> u32 {
    let any = |set: &IntSet| set.words.iter().any(|w| *w != 0);
    let mut flags = 0;
    if any(&s.rotation_matters) || any(&s.translation_matters) || any(&s.scale_matters) {
        flags |= DIRTY_TRANSFORM;
    }
    if any(&s.vis_matters) {
        flags |= DIRTY_VIS;
    }
    if any(&s.frame_matters) {
        flags |= DIRTY_FRAME;
    }
    if any(&s.mat_frame_matters) {
        flags |= DIRTY_MAT_FRAME;
    }
    if any(&s.decal_matters) {
        flags |= DIRTY_DECAL;
    }
    if any(&s.ifl_matters) {
        flags |= DIRTY_IFL;
    }
    flags
}

impl ShapeInstance {
    /// `TSShapeInstance(shape)`: the matrices, the dirty words, and one `animate` of every detail's sub-shape
    /// (the nodes start at the default pose). Charged per node.
    pub fn new(shape: &Shape, charge: Charge) -> Result<ShapeInstance, ShapeError> {
        pay(charge, 8 + NODE_WORK * shape.nodes.len() as u64)?;
        let mut inst = ShapeInstance {
            nodes: vec![IDENTITY; shape.nodes.len()],
            arena: Vec::new(),
            order: Vec::new(),
            dirty: vec![DIRTY_ALL; shape.sub_shapes.len()],
            masks: NodeMasks::default(),
            scale_animated: false,
            transition_threads: Vec::new(),
            transition_rotation_nodes: [0; 6],
            transition_translation_nodes: [0; 6],
            transition_scale_nodes: [0; 6],
            reference_rotations: Vec::new(),
            reference_translations: Vec::new(),
            current_rotations: vec![[0.0, 0.0, 0.0, 1.0]; shape.nodes.len()],
            current_translations: vec![[0.0; 3]; shape.nodes.len()],
            pose_blend: None,
        };
        for dl in 0..shape.details.len() {
            let ss = shape.details[dl].sub_shape;
            let Ok(ss) = usize::try_from(ss) else {
                continue;
            };
            if inst.dirty.get(ss).is_some_and(|d| d & DIRTY_TRANSFORM != 0) {
                inst.animate(shape, dl, None, charge)?;
                inst.dirty[ss] = 0;
            }
        }
        Ok(inst)
    }

    /// The node matrices as they are now (`mNodeTransforms`).
    pub fn node_transforms(&self) -> &[Matrix] {
        &self.nodes
    }

    /// The matrix of one node, as it is now.
    pub fn node_transform(&self, node: usize) -> Option<&Matrix> {
        self.nodes.get(node)
    }

    /// The local pose the last `animate` made (each node's rotation and translation in its parent's space, after the threads and the
    /// transitions and any [`PoseBlend`], before the blend threads).
    pub fn local_pose(&self) -> (Vec<[f32; 4]>, Vec<[f32; 3]>) {
        (
            self.current_rotations.clone(),
            self.current_translations.clone(),
        )
    }

    /// Sets or clears the [`PoseBlend`]; the nodes are made again at the next `animate`.
    pub fn set_pose_blend(&mut self, blend: Option<PoseBlend>) {
        self.pose_blend = blend;
        self.set_all_dirty();
    }

    /// The [`PoseBlend`] in force.
    pub fn pose_blend(&self) -> Option<&PoseBlend> {
        self.pose_blend.as_ref()
    }

    /// Replaces a node's matrix (a callback or hands-off node sets its own).
    pub fn set_node_transform(&mut self, node: usize, m: Matrix) {
        if let Some(slot) = self.nodes.get_mut(node) {
            *slot = m;
        }
    }

    /// The dirty word of a sub-shape.
    pub fn dirty(&self, sub_shape: usize) -> Option<u32> {
        self.dirty.get(sub_shape).copied()
    }

    /// `setDirty(AllDirtyMask)`: every sub-shape needs its nodes and thread order made again.
    pub fn set_all_dirty(&mut self) {
        for d in &mut self.dirty {
            *d |= DIRTY_ALL;
        }
    }

    /// The thread, if it is alive.
    pub fn thread(&self, id: ThreadId) -> Option<&Thread> {
        self.arena.get(id.0)?.as_ref()
    }

    /// Mutable access to a thread (the caller sets its time scale, as `thread->setTimeScale`).
    pub fn thread_mut(&mut self, id: ThreadId) -> Option<&mut Thread> {
        self.arena.get_mut(id.0)?.as_mut()
    }

    /// The live threads in `mThreadList` order.
    pub fn thread_order(&self) -> Vec<ThreadId> {
        self.order.iter().map(|i| ThreadId(*i)).collect()
    }

    /// How many threads the instance holds.
    pub fn thread_count(&self) -> usize {
        self.order.len()
    }

    /// Whether a thread plays a sequence that animates an object's visibility.
    pub fn animates_visibility(&self, shape: &Shape) -> bool {
        self.order.iter().any(|&i| {
            self.arena[i]
                .as_ref()
                .and_then(|t| t.sequence)
                .and_then(|s| shape.sequences.get(s))
                .is_some_and(|s| s.vis_matters.count() > 0)
        })
    }

    /// Object visibility (`TSShapeInstance::animateVisibility`): its default state (`objectStates[object]`),
    /// unless a thread's sequence animates it, the first such thread in the thread order giving it: the state of its two
    /// keyframes, taken whole at the half way when they differ by nearly 1 (a jump between hidden and shown), else blended by the
    /// key position. A sequence's states are its animated objects' (frame, material frame or visibility) in object order, each
    /// object's keyframes in a row.
    pub fn object_visibility(&self, shape: &Shape) -> Vec<f32> {
        let n = shape.objects.len();
        let mut vis: Vec<f32> = (0..n)
            .map(|i| shape.object_states.get(i).map_or(1.0, |s| s.vis))
            .collect();
        let mut set = vec![false; n];
        for &t in &self.order {
            let Some(th) = self.arena[t].as_ref() else {
                continue;
            };
            let Some(seq) = th.sequence.and_then(|s| shape.sequences.get(s)) else {
                continue;
            };
            if seq.vis_matters.count() == 0 {
                continue;
            }
            let mut j = 0usize;
            for (object, done) in set.iter_mut().enumerate() {
                let matters = seq.frame_matters.test(object)
                    || seq.mat_frame_matters.test(object)
                    || seq.vis_matters.test(object);
                if !matters {
                    continue;
                }
                if !*done && seq.vis_matters.test(object) {
                    let state = |key: i32| {
                        let at = i64::from(seq.base_object_state)
                            + j as i64 * i64::from(seq.num_keyframes)
                            + i64::from(key);
                        usize::try_from(at)
                            .ok()
                            .and_then(|at| shape.object_states.get(at))
                            .map(|s| s.vis)
                    };
                    if let (Some(a), Some(b)) = (state(th.key_num1), state(th.key_num2)) {
                        vis[object] = if (a - b) * (a - b) > 0.99 {
                            if th.key_pos < 0.5 { a } else { b }
                        } else {
                            (1.0 - th.key_pos) * a + th.key_pos * b
                        };
                        *done = true;
                    }
                }
                j += 1;
            }
        }
        vis
    }

    /// `addThread`: a new thread playing sequence 0 at position 0; none for a shape with no sequences (or past
    /// [`MAX_THREADS`]). Every sub-shape is marked dirty.
    pub fn create_thread(&mut self, shape: &Shape) -> Option<ThreadId> {
        if shape.sequences.is_empty() || self.order.len() >= MAX_THREADS {
            return None;
        }
        let mut thread = Thread {
            priority: 0,
            sequence: None,
            pos: 0.0,
            time_scale: 1.0,
            key_num1: 0,
            key_num2: 0,
            key_pos: 0.0,
            blend_disabled: false,
            make_path: false,
            transition: Transition::default(),
        };
        thread.set_sequence(shape, 0, 0.0);
        let id = self.arena.len();
        self.arena.push(Some(thread));
        self.order.push(id);
        self.set_all_dirty();
        Some(ThreadId(id))
    }

    /// `destroyThread`: the thread leaves the list and the sub-shapes are marked dirty.
    pub fn destroy_thread(&mut self, shape: &Shape, id: ThreadId) {
        if self.thread(id).is_none() {
            return;
        }
        self.clear_transition(shape, id);
        self.arena[id.0] = None;
        self.order.retain(|i| *i != id.0);
        self.set_all_dirty();
        self.check_scale_animated(shape);
    }

    /// `TSShapeInstance::setSequence`: the thread plays sequence `seq` from position `pos`, and every sub-shape is dirty.
    /// A sequence the shape does not have leaves the thread as it was (Torque tests the index) but still marks.
    pub fn set_sequence(&mut self, shape: &Shape, id: ThreadId, seq: i32, pos: f32) {
        let Some(thread) = self.arena.get_mut(id.0).and_then(Option::as_mut) else {
            return;
        };
        thread.set_sequence(shape, seq, pos);
        let animates = thread
            .sequence
            .and_then(|s| shape.sequences.get(s))
            .is_some_and(|s| s.flags & FLAG_SCALE_KINDS != 0);
        self.set_all_dirty();
        if !self.scale_animated {
            self.scale_animated = animates;
        } else if !animates {
            self.check_scale_animated(shape);
        }
    }

    /// `checkScaleCurrentlyAnimated`: the flag is true when some thread plays a sequence that animates scale.
    fn check_scale_animated(&mut self, shape: &Shape) {
        self.scale_animated = self.order.iter().any(|i| {
            self.arena[*i]
                .as_ref()
                .and_then(|t| t.sequence)
                .and_then(|s| shape.sequences.get(s))
                .is_some_and(|s| s.flags & FLAG_SCALE_KINDS != 0)
        });
    }

    /// `TSThread::advancePos`: the position moves by `delta` of the sequence and keeps to its bounds; the sub-shapes are
    /// marked dirty when the move is more than 0.00001 (their nodes too while the thread is in a transition). In a transition the
    /// transition runs first, `direction * delta` of it: past its end it is cleared, before its start it is cleared and the thread
    /// goes back to the sequence and position it left; the sequence then moves by `delta * target_scale * duration` over its own
    /// duration.
    pub fn advance_pos(&mut self, shape: &Shape, id: ThreadId, delta: f32) {
        let Some(thread) = self.arena.get_mut(id.0).and_then(Option::as_mut) else {
            return;
        };
        if !thread.transition.in_transition {
            if thread.advance_pos(shape, delta) {
                let flags = thread
                    .sequence
                    .and_then(|s| shape.sequences.get(s))
                    .map_or(0, sequence_dirty);
                for d in &mut self.dirty {
                    *d |= flags;
                }
            }
            return;
        }
        thread.pos = clamp01(thread.pos);
        if delta.abs() > 0.00001 {
            let flags = thread
                .sequence
                .and_then(|s| shape.sequences.get(s))
                .map_or(0, sequence_dirty)
                | DIRTY_TRANSFORM;
            for d in &mut self.dirty {
                *d |= flags;
            }
        }
        let t = (library::get().mul_add)(thread.transition.direction, delta, thread.transition.pos);
        thread.transition.pos = t;
        // Torque's two compares: a NaN position ends nothing (`!(0.0..1.0).contains` would end it).
        #[allow(clippy::manual_range_contains)]
        let outside = t < 0.0 || t >= 1.0;
        if outside {
            self.clear_transition(shape, id);
            let back = self
                .thread(id)
                .filter(|th| th.transition.pos < 0.0)
                .and_then(|th| Some((th.transition.old_sequence?, th.transition.old_pos)));
            if let Some((seq, pos)) = back {
                self.set_sequence(shape, id, seq as i32, pos);
            }
        }
        let Some(thread) = self.arena.get_mut(id.0).and_then(Option::as_mut) else {
            return;
        };
        let duration = thread
            .sequence
            .and_then(|s| shape.sequences.get(s))
            .map_or(0.0, |s| s.duration);
        let delta = if duration > 0.0 {
            ((thread.transition.target_scale * thread.transition.duration) / duration) * delta
        } else {
            0.0
        };
        thread.advance_pos(shape, delta);
    }

    /// `TSThread::advanceTime`: `delta` seconds at the thread's time scale, over the transition's duration while in one,
    /// else the sequence's.
    pub fn advance_time(&mut self, shape: &Shape, id: ThreadId, seconds: f32) {
        let Some(thread) = self.thread(id) else {
            return;
        };
        let duration = if thread.transition.in_transition {
            thread.transition.duration
        } else {
            thread
                .sequence
                .and_then(|s| shape.sequences.get(s))
                .map_or(0.0, |s| s.duration)
        };
        let delta = if duration == 0.0 {
            0.0
        } else {
            (library::get().mul_div)(thread.time_scale, seconds, duration)
        };
        self.advance_pos(shape, id, delta);
    }

    /// `animateNodeSubtrees(true)`: every sub-shape's nodes made again now (a transition starts from the pose it leaves).
    fn animate_all_nodes(&mut self, shape: &Shape) {
        for d in &mut self.dirty {
            *d |= DIRTY_TRANSFORM;
        }
        let mut charge = |_: u64| true;
        for ss in 0..shape.sub_shapes.len().min(self.dirty.len()) {
            if self.dirty[ss] & DIRTY_TRANSFORM != 0 {
                let _ = self.animate_nodes(shape, ss, None, &mut charge);
                self.dirty[ss] &= !DIRTY_TRANSFORM;
            }
        }
    }

    /// [`Self::transition_to_sequence`] with a blend that eases in and out (the engine's own, [`Transition::eased`]): the new
    /// pose's weight is the smoothstep of the transition's position, so the body starts and settles gently.
    pub fn transition_to_sequence_eased(
        &mut self,
        shape: &Shape,
        id: ThreadId,
        seq: usize,
        pos: f32,
        duration: f32,
    ) {
        self.transition_to_sequence(shape, id, seq, pos, duration);
        if let Some(th) = self.arena.get_mut(id.0).and_then(Option::as_mut)
            && th.transition.in_transition
        {
            th.transition.eased = true;
        }
    }

    /// [`Self::transition_to_sequence_eased`] with the limbs swinging in the shape's space (the engine's own, [`swing`]): each
    /// arm and leg swings from where it pointed to where it points, out to its own side on the way, and turns about itself
    /// meanwhile, however its parent turns. For a change of the whole body's pose (out of the water: the body lying
    /// face down comes upright as the arms come down from over the head), where blending each limb under its parent would
    /// swing it through the body.
    pub fn transition_to_sequence_swung(
        &mut self,
        shape: &Shape,
        id: ThreadId,
        seq: usize,
        pos: f32,
        duration: f32,
    ) {
        self.transition_to_sequence_eased(shape, id, seq, pos, duration);
        if let Some(th) = self.arena.get_mut(id.0).and_then(Option::as_mut)
            && th.transition.in_transition
        {
            th.transition.swung = true;
        }
    }

    /// `TSShapeInstance::transitionToSequence` (the thread's half included): the thread moves to sequence `seq` at `pos`
    /// over `duration` seconds, blending from the pose the instance has now, and goes on playing meanwhile (`continuePlay`: the
    /// target scale 1). A sequence the shape does not have changes nothing.
    pub fn transition_to_sequence(
        &mut self,
        shape: &Shape,
        id: ThreadId,
        seq: usize,
        pos: f32,
        duration: f32,
    ) {
        let Some(s) = shape.sequences.get(seq) else {
            return;
        };
        if self.thread(id).is_none() {
            return;
        }
        self.animate_all_nodes(shape);
        let Some(thread) = self.arena.get_mut(id.0).and_then(Option::as_mut) else {
            return;
        };
        let old = thread.sequence.and_then(|i| shape.sequences.get(i));
        let matters = |f: fn(&Sequence) -> &IntSet| old.map_or([0; 6], |o| words(f(o)));
        let (r, t, sc) = (
            matters(|q| &q.rotation_matters),
            matters(|q| &q.translation_matters),
            matters(|q| &q.scale_matters),
        );
        let tr = &mut thread.transition;
        if tr.in_transition {
            for w in 0..6 {
                tr.old_rotation_nodes[w] |= r[w];
                tr.old_translation_nodes[w] |= t[w];
                tr.old_scale_nodes[w] |= sc[w];
            }
        } else {
            tr.old_rotation_nodes = r;
            tr.old_translation_nodes = t;
            tr.old_scale_nodes = sc;
        }
        tr.duration = duration;
        tr.old_sequence = thread.sequence;
        tr.pos = 0.0;
        tr.old_pos = thread.pos;
        tr.direction = if thread.time_scale > 0.0 { 1.0 } else { -1.0 };
        tr.target_scale = 1.0;
        tr.in_transition = true;
        tr.eased = false;
        tr.swung = false;
        thread.set_sequence(shape, seq as i32, pos);
        let old = thread.transition;
        self.set_all_dirty();
        let animates = s.flags & FLAG_SCALE_KINDS != 0;
        if !self.scale_animated {
            self.scale_animated = animates;
        } else if !animates {
            self.check_scale_animated(shape);
        }
        let (new_r, new_t, new_s) = (
            words(&s.rotation_matters),
            words(&s.translation_matters),
            words(&s.scale_matters),
        );
        for w in 0..6 {
            self.transition_rotation_nodes[w] |= old.old_rotation_nodes[w] | new_r[w];
            self.transition_translation_nodes[w] |= old.old_translation_nodes[w] | new_t[w];
            self.transition_scale_nodes[w] |= old.old_scale_nodes[w] | new_s[w];
        }
        if !self.transition_threads.contains(&id.0) {
            self.transition_threads.push(id.0);
        }
        self.update_transitions(shape);
    }

    /// `updateTransitions`: the pose `animateNodes` last left becomes the reference of the transition nodes (a rotation
    /// kept as a `Quat16`: each component times 32767, truncated), and every transition still running starts again from it over what
    /// was left of it (`duration *= 1 - pos`, `pos = 0`).
    fn update_transitions(&mut self, shape: &Shape) {
        if self.transition_threads.is_empty() {
            return;
        }
        let n = shape.nodes.len();
        self.reference_rotations.resize(n, [0, 0, 0, 32767]);
        self.reference_translations.resize(n, [0.0; 3]);
        for i in 0..n.min(SET_BITS) {
            if set_has(&self.transition_rotation_nodes, i) {
                let q = self
                    .current_rotations
                    .get(i)
                    .copied()
                    .unwrap_or([0.0, 0.0, 0.0, 1.0]);
                self.reference_rotations[i] = q.map(|c| truncate(c * 32767.0) as i16);
            }
            if set_has(&self.transition_translation_nodes, i) {
                self.reference_translations[i] = self
                    .current_translations
                    .get(i)
                    .copied()
                    .unwrap_or([0.0; 3]);
            }
        }
        for t in &self.transition_threads {
            if let Some(th) = self.arena.get_mut(*t).and_then(Option::as_mut)
                && th.transition.in_transition
            {
                let p = th.transition.pos;
                th.transition.pos = 0.0;
                th.transition.duration *= 1.0 - p;
            }
        }
    }

    /// `clearTransition`: the thread leaves its transition and the list (the pose made again first when others stay in
    /// theirs); the instance's transition nodes become those of the threads still in one, which start again from the pose as it is.
    pub fn clear_transition(&mut self, shape: &Shape, id: ThreadId) {
        if !self.thread(id).is_some_and(|t| t.transition.in_transition) {
            return;
        }
        if self.transition_threads.len() > 1 {
            self.animate_all_nodes(shape);
        }
        if let Some(th) = self.arena.get_mut(id.0).and_then(Option::as_mut) {
            th.transition.in_transition = false;
        }
        self.transition_threads.retain(|t| *t != id.0);
        self.transition_rotation_nodes = [0; 6];
        self.transition_translation_nodes = [0; 6];
        self.transition_scale_nodes = [0; 6];
        for t in &self.transition_threads {
            let Some(th) = self.arena.get(*t).and_then(Option::as_ref) else {
                continue;
            };
            let s = th.sequence.and_then(|i| shape.sequences.get(i));
            let m = |f: fn(&Sequence) -> &IntSet| s.map_or([0; 6], |q| words(f(q)));
            let (r, tr, sc) = (
                m(|q| &q.rotation_matters),
                m(|q| &q.translation_matters),
                m(|q| &q.scale_matters),
            );
            for w in 0..6 {
                self.transition_rotation_nodes[w] |= th.transition.old_rotation_nodes[w] | r[w];
                self.transition_translation_nodes[w] |=
                    th.transition.old_translation_nodes[w] | tr[w];
                self.transition_scale_nodes[w] |= th.transition.old_scale_nodes[w] | sc[w];
            }
        }
        for d in &mut self.dirty {
            *d |= DIRTY_THREAD_SORT;
        }
        self.update_transitions(shape);
    }

    /// How many threads are in a transition.
    pub fn transition_count(&self) -> usize {
        self.transition_threads.len()
    }

    /// `handleTransitionNodes`: each transition node's local rotation and translation moves from the reference toward the
    /// pose the threads gave it by the transition's position: the transition of the thread that set the node when it is in one, else
    /// the first in the list whose old or new sequence moves the node.
    fn handle_transition_nodes(
        &self,
        shape: &Shape,
        a: usize,
        b: usize,
        set_by: (&[Option<usize>], &[Option<usize>]),
        rotations: &mut [[f32; 4]],
        translations: &mut [[f32; 3]],
    ) {
        let in_transition = |t: Option<usize>| {
            t.filter(|t| {
                self.arena
                    .get(*t)
                    .and_then(Option::as_ref)
                    .is_some_and(|th| th.transition.in_transition)
            })
        };
        // The transition's weight for node `n` (smoothstepped when it eases) and whether its limbs swing.
        let pick = |n: usize, set: Option<usize>, rot: bool| -> Option<(f32, bool)> {
            let t = in_transition(set).or_else(|| {
                self.transition_threads.iter().copied().find(|t| {
                    let Some(th) = self.arena.get(*t).and_then(Option::as_ref) else {
                        return false;
                    };
                    let s = th.sequence.and_then(|i| shape.sequences.get(i));
                    if rot {
                        set_has(&th.transition.old_rotation_nodes, n)
                            || s.is_some_and(|s| s.rotation_matters.test(n))
                    } else {
                        set_has(&th.transition.old_translation_nodes, n)
                            || s.is_some_and(|s| s.translation_matters.test(n))
                    }
                })
            })?;
            self.arena.get(t).and_then(Option::as_ref).map(|th| {
                let p = th.transition.pos;
                if th.transition.eased {
                    let u = p.clamp(0.0, 1.0);
                    (u * u * (3.0 - 2.0 * u), th.transition.swung)
                } else {
                    (p, false)
                }
            })
        };
        let reference = |n: usize| {
            quat16(
                self.reference_rotations
                    .get(n)
                    .copied()
                    .unwrap_or([0, 0, 0, 32767]),
            )
        };
        let swung = self.transition_threads.iter().any(|t| {
            self.arena
                .get(*t)
                .and_then(Option::as_ref)
                .is_some_and(|th| th.transition.in_transition && th.transition.swung)
        });
        if swung {
            // Only the first `SET_BITS` nodes can be in a transition; the nodes past them (and so their children) are left.
            let end = b.min(SET_BITS).max(a);
            let mut weights = [None; SET_BITS];
            for (n, w) in (a..end).zip(weights.iter_mut()) {
                if set_has(&self.transition_rotation_nodes, n) {
                    *w = pick(n, set_by.0.get(n).copied().flatten(), true);
                }
            }
            swing::blend(
                shape,
                a,
                &weights[..end - a],
                reference,
                &mut rotations[a..end],
            );
        } else {
            for n in members_range(&self.transition_rotation_nodes, a, b) {
                if let Some((p, _)) = pick(n, set_by.0.get(n).copied().flatten(), true) {
                    rotations[n] = interpolate_quat(reference(n), rotations[n], p);
                }
            }
        }
        for n in members_range(&self.transition_translation_nodes, a, b) {
            let Some((p, _)) = pick(n, set_by.1.get(n).copied().flatten(), false) else {
                continue;
            };
            let r = self
                .reference_translations
                .get(n)
                .copied()
                .unwrap_or([0.0; 3]);
            translations[n] = lerp3(r, translations[n], p);
        }
    }

    /// `animate` for a detail level: the thread list is sorted when the sub-shape is marked for it, and the nodes are made
    /// again when it is marked for them. The flags clear. Charged per node and per thread-node.
    pub fn animate(
        &mut self,
        shape: &Shape,
        detail: usize,
        callback: Option<NodeCallback<'_>>,
        charge: Charge,
    ) -> Result<(), ShapeError> {
        let Some(ss) = shape
            .details
            .get(detail)
            .and_then(|d| usize::try_from(d.sub_shape).ok())
        else {
            return Ok(());
        };
        let Some(flags) = self.dirty.get(ss).copied() else {
            return Ok(());
        };
        if flags & DIRTY_THREAD_SORT != 0 {
            self.sort_threads(shape);
        }
        if flags & DIRTY_TRANSFORM != 0 {
            self.animate_nodes(shape, ss, callback, charge)?;
        }
        self.dirty[ss] = 0;
        Ok(())
    }

    /// The comparison: threads of non-blend sequences first, then by priority, the higher first.
    fn sort_threads(&mut self, shape: &Shape) {
        let key = |i: &usize| -> (bool, i32) {
            let t = self.arena[*i].as_ref();
            let blend = t
                .and_then(|t| t.sequence)
                .and_then(|s| shape.sequences.get(s))
                .is_some_and(|s| s.flags & FLAG_BLEND != 0);
            (blend, t.map_or(0, |t| t.priority))
        };
        let cmp = |a: &usize, b: &usize| -> i32 {
            let ((ba, pa), (bb, pb)) = (key(a), key(b));
            if ba == bb {
                match pa.cmp(&pb) {
                    Ordering::Less => 1,
                    Ordering::Equal => 0,
                    Ordering::Greater => -1,
                }
            } else {
                i32::from(ba) * 2 - 1
            }
        };
        let mut order = std::mem::take(&mut self.order);
        (library::get().sort_threads)(&mut order, &cmp);
        self.order = order;
    }

    /// `animateNodes(ss)`.
    fn animate_nodes(
        &mut self,
        shape: &Shape,
        ss: usize,
        callback: Option<NodeCallback<'_>>,
        charge: Charge,
    ) -> Result<(), ShapeError> {
        let Some(sub) = shape.sub_shapes.get(ss) else {
            return Ok(());
        };
        let node_count = shape.nodes.len();
        if node_count == 0 {
            return Ok(());
        }
        let a = usize::try_from(sub.first_node).unwrap_or(0).min(node_count);
        let b = a
            .saturating_add(usize::try_from(sub.num_nodes).unwrap_or(0))
            .min(node_count);
        pay(
            charge,
            16 + NODE_WORK * (b - a) as u64
                + THREAD_NODE_WORK * (self.order.len() as u64) * (b - a) as u64,
        )?;
        let masks = self.masks.clone();
        // The nodes the non-blend threads leave to the default pose, and where the blend threads begin.
        let mut rot_default = full_set(node_count);
        let mut tran_default = full_set(node_count);
        let mut scale_default = full_set(node_count);
        let order = self.order.clone();
        let mut first_blend = order.len();
        for (i, id) in order.iter().enumerate() {
            let Some(seq) = self.arena[*id]
                .as_ref()
                .and_then(|t| t.sequence)
                .and_then(|s| shape.sequences.get(s))
            else {
                continue;
            };
            if seq.flags & FLAG_BLEND != 0 {
                first_blend = i;
                break;
            }
            let (r, t) = (
                words(&seq.rotation_matters),
                words(&seq.translation_matters),
            );
            let sc = words(&seq.scale_matters);
            for w in 0..6 {
                rot_default[w] &= !r[w];
                tran_default[w] &= !t[w];
                scale_default[w] &= !sc[w];
            }
        }
        let pos_masks: NodeSet = std::array::from_fn(|w| {
            masks.mask_pos_x[w] | masks.mask_pos_y[w] | masks.mask_pos_z[w]
        });
        // rot_init: the nodes that get the default rotation now; tran_init likewise (the masked ones too).
        let rot_init: NodeSet = std::array::from_fn(|w| {
            (!masks.callback[w] & rot_default[w] & !masks.hands_off[w]) | masks.mask_rotation[w]
        });
        let tran_init: NodeSet = std::array::from_fn(|w| {
            ((pos_masks[w] | tran_default[w]) & !masks.callback[w]) & !masks.hands_off[w]
        });
        let mut rotations: Vec<[f32; 4]> = vec![[0.0, 0.0, 0.0, 1.0]; node_count];
        // `smRotationThreads`, `smTranslationThreads`: the thread that set each node (none for the default pose).
        let mut rot_by: Vec<Option<usize>> = vec![None; node_count];
        let mut tran_by: Vec<Option<usize>> = vec![None; node_count];
        let mut translations: Vec<[f32; 3]> = vec![[0.0; 3]; node_count];
        // Torque's scratch arrays hold whatever the last animate left; a node no rule below sets keeps its matrix.
        for n in a..b {
            // Nodes past the set bits take the default pose.
            let in_set = n < SET_BITS;
            if !in_set || set_has(&rot_init, n) {
                rotations[n] = quat16(
                    shape
                        .default_rotations
                        .get(n)
                        .copied()
                        .unwrap_or([0, 0, 0, 32767]),
                );
            }
            if !in_set || set_has(&tran_init, n) {
                translations[n] = shape
                    .default_translations
                    .get(n)
                    .copied()
                    .unwrap_or([0.0; 3]);
            }
        }
        let mut rot_done: NodeSet =
            std::array::from_fn(|w| rot_init[w] | masks.callback[w] | masks.hands_off[w]);
        let mut tran_done: NodeSet = std::array::from_fn(|w| {
            (!(masks.hands_off[w] | pos_masks[w]) & tran_init[w])
                | masks.callback[w]
                | masks.hands_off[w]
        });
        // Scale, only while some thread plays a sequence that animates it. The scratch arrays start at the
        // identity scale; Torque's are globals that keep what the last shape left in them.
        let kind = scale_kind(shape);
        let mut scales = Scales::new(node_count);
        let mut scale_done: NodeSet = [0; 6];
        if self.scale_animated {
            scale_done = std::array::from_fn(|w| {
                (scale_default[w] & !masks.callback[w] & !masks.hands_off[w])
                    | masks.hands_off[w]
                    | masks.callback[w]
            });
        }
        // The non-blend threads, highest priority first: the first to move a node decides it.
        for id in order.iter().take(first_blend) {
            let Some(thread) = self.arena[*id].as_ref() else {
                continue;
            };
            let Some(seq) = thread.sequence.and_then(|s| shape.sequences.get(s)) else {
                continue;
            };
            let k = i64::from(seq.num_keyframes);
            // Rotations: the sequence's nodes in order; the n-th member reads keyframes of the n-th track.
            for (track, n) in members(&seq.rotation_matters, b).into_iter().enumerate() {
                let track = track as i64;
                if n >= a && n < SET_BITS && !set_has(&rot_done, n) {
                    let base = k * track + i64::from(seq.base_rotation);
                    let q1 = rotation_key(shape, i64::from(thread.key_num1) + base);
                    let q2 = rotation_key(shape, i64::from(thread.key_num2) + base);
                    rotations[n] = interpolate_quat(q1, q2, thread.key_pos);
                    rot_by[n] = Some(*id);
                    set_add(&mut rot_done, n);
                }
            }
            for (track, n) in members(&seq.translation_matters, b).into_iter().enumerate() {
                let track = track as i64;
                if n >= a && n < SET_BITS && !set_has(&tran_done, n) {
                    let base = k * track + i64::from(seq.base_translation);
                    let p1 = translation_key(shape, i64::from(thread.key_num1) + base);
                    let p2 = translation_key(shape, i64::from(thread.key_num2) + base);
                    let v = lerp3(p1, p2, thread.key_pos);
                    if !set_has(&pos_masks, n) {
                        translations[n] = v;
                    } else {
                        if !set_has(&masks.mask_pos_x, n) {
                            translations[n][0] = v[0];
                        }
                        if !set_has(&masks.mask_pos_y, n) {
                            translations[n][1] = v[1];
                        }
                        if !set_has(&masks.mask_pos_z, n) {
                            translations[n][2] = v[2];
                        }
                    }
                    tran_by[n] = Some(*id);
                    set_add(&mut tran_done, n);
                }
            }
            if self.scale_animated {
                for (track, n) in members(&seq.scale_matters, b).into_iter().enumerate() {
                    let track = track as i64;
                    if n >= a && n < SET_BITS && !set_has(&scale_done, n) {
                        scales.thread_scale(shape, kind, seq, thread, k * track, n);
                        set_add(&mut scale_done, n);
                    }
                }
            }
        }
        // The transitions: from the reference pose toward the threads' by each transition's position.
        if !self.transition_threads.is_empty() {
            self.handle_transition_nodes(
                shape,
                a,
                b,
                (&rot_by, &tran_by),
                &mut rotations,
                &mut translations,
            );
        }
        // The whole-pose blend (not part of Torque).
        if let Some(pb) = &self.pose_blend {
            let w = pb.weight.clamp(0.0, 1.0);
            for n in a..b {
                if n < SET_BITS && set_has(&masks.hands_off, n) {
                    continue;
                }
                if let Some(r) = pb.rotations.get(n) {
                    rotations[n] = interpolate_quat(*r, rotations[n], w);
                }
                if let Some(t) = pb.translations.get(n) {
                    translations[n] = lerp3(*t, translations[n], w);
                }
            }
        }
        // What the scratch arrays hold now, for the next transition's reference.
        self.current_rotations
            .resize(node_count, [0.0, 0.0, 0.0, 1.0]);
        self.current_translations.resize(node_count, [0.0; 3]);
        self.current_rotations[a..b].copy_from_slice(&rotations[a..b]);
        self.current_translations[a..b].copy_from_slice(&translations[a..b]);
        // The matrices: a rotation and a translation for every node the Player has not taken over.
        for n in a..b {
            if n < SET_BITS && set_has(&masks.hands_off, n) {
                continue;
            }
            let mut m = math::quat_matrix(rotations[n]);
            m[3] = translations[n][0];
            m[7] = translations[n][1];
            m[11] = translations[n][2];
            self.nodes[n] = m;
        }
        if self.scale_animated {
            for (n, node) in self.nodes.iter_mut().enumerate().take(b).skip(a) {
                if n < SET_BITS && set_has(&masks.hands_off, n) {
                    continue;
                }
                scales.apply(kind, node, n);
            }
        }
        // The nodes the callback sets.
        if let Some(cb) = callback {
            for n in members_range(&masks.callback, a, b) {
                cb(n, &mut self.nodes[n]);
            }
        }
        // The blend threads multiply their own pose onto the local matrices.
        for id in order.iter().skip(first_blend) {
            let Some(thread) = self.arena[*id].clone() else {
                continue;
            };
            if thread.blend_disabled {
                continue;
            }
            self.blend(shape, &thread, a, b);
        }
        // Compose: each node under its parent's final matrix, in node order.
        for n in a..b {
            if let Ok(p) = usize::try_from(shape.nodes[n].parent)
                && p < node_count
            {
                let local = self.nodes[n];
                self.nodes[n] = math::mat_mul(&self.nodes[p], &local);
            }
        }
        Ok(())
    }

    /// `handleBlendSequence`: a blend thread's rotations, translations and scales, as a matrix per node, multiplied onto the
    /// node's local matrix (`node * blend`).
    fn blend(&mut self, shape: &Shape, thread: &Thread, a: usize, b: usize) {
        let Some(seq) = thread.sequence.and_then(|s| shape.sequences.get(s)) else {
            return;
        };
        let k = i64::from(seq.num_keyframes);
        let (rot, tran) = (
            words(&seq.rotation_matters),
            words(&seq.translation_matters),
        );
        let scale = words(&seq.scale_matters);
        let all: NodeSet = std::array::from_fn(|w| rot[w] | tran[w] | scale[w]);
        let (mut rot_track, mut tran_track, mut scale_track) = (0i64, 0i64, 0i64);
        let kind = scale_kind(shape);
        // Torque compares the FIRST node the sequence moves with sub-shape's first node (the saved first bit, read each
        // pass, not the node in hand): a sequence that starts below the sub-shape skips all of its nodes.
        let members = members_all(&all, b);
        let skip_all = members.first().is_some_and(|first| *first < a);
        for n in members {
            if skip_all || (n < SET_BITS && set_has(&self.masks.disable_blend, n)) {
                rot_track += i64::from(set_has(&rot, n));
                tran_track += i64::from(set_has(&tran, n));
                scale_track += i64::from(set_has(&scale, n));
                continue;
            }
            let mut m = IDENTITY;
            if set_has(&rot, n) {
                let base = k * rot_track + i64::from(seq.base_rotation);
                let q1 = rotation_key(shape, i64::from(thread.key_num1) + base);
                let q2 = rotation_key(shape, i64::from(thread.key_num2) + base);
                m = math::quat_matrix(interpolate_quat(q1, q2, thread.key_pos));
                rot_track += 1;
            }
            if set_has(&tran, n) {
                let base = k * tran_track + i64::from(seq.base_translation);
                let p1 = translation_key(shape, i64::from(thread.key_num1) + base);
                let p2 = translation_key(shape, i64::from(thread.key_num2) + base);
                let v = lerp3(p1, p2, thread.key_pos);
                m[3] = v[0];
                m[7] = v[1];
                m[11] = v[2];
                tran_track += 1;
            }
            if set_has(&scale, n) {
                let base = k * scale_track + i64::from(seq.base_scale);
                let (i1, i2) = (
                    i64::from(thread.key_num1) + base,
                    i64::from(thread.key_num2) + base,
                );
                // A uniform sequence, else the shape's own kind: aligned, else arbitrary (the blend's branches).
                if seq.flags & 1 != 0 {
                    let s = lerp1(
                        uniform_key(shape, i1),
                        uniform_key(shape, i2),
                        thread.key_pos,
                    );
                    scale_matrix(&mut m, [s, s, s]);
                } else if kind & 2 != 0 {
                    let v = lerp3(
                        aligned_key(shape, i1),
                        aligned_key(shape, i2),
                        thread.key_pos,
                    );
                    scale_matrix(&mut m, v);
                } else {
                    let (q1, s1) = arbitrary_key(shape, i1);
                    let (q2, s2) = arbitrary_key(shape, i2);
                    let q = interpolate_quat(q1, q2, thread.key_pos);
                    let s = lerp3(s1, s2, thread.key_pos);
                    arbitrary_scale(&mut m, q, s);
                }
                scale_track += 1;
            }
            if let Some(node) = self.nodes.get_mut(n) {
                let old = *node;
                *node = math::mat_mul(&old, &m);
            }
        }
    }
}

/// The shape's scale kind (`TSShape::mScaleKind`, bits 0 to 2): the largest `flags & 7` over the sequences that animate scale.
fn scale_kind(shape: &Shape) -> u32 {
    shape
        .sequences
        .iter()
        .map(|s| s.flags & FLAG_SCALE_KINDS)
        .max()
        .unwrap_or(0)
}

/// `TSTransform::interpolate(F32)` (through the table): the scale blend of the thread pass and `handleBlendSequence`.
fn lerp1(a: f32, b: f32, t: f32) -> f32 {
    (library::get().lerp)(a, b, t)
}

fn uniform_key(shape: &Shape, index: i64) -> f32 {
    usize::try_from(index)
        .ok()
        .and_then(|i| shape.node_uniform_scales.get(i))
        .copied()
        .unwrap_or(1.0)
}

fn aligned_key(shape: &Shape, index: i64) -> [f32; 3] {
    usize::try_from(index)
        .ok()
        .and_then(|i| shape.node_aligned_scales.get(i))
        .copied()
        .unwrap_or([1.0; 3])
}

/// An arbitrary scale key: its rotation (as a float quaternion) and its factors.
fn arbitrary_key(shape: &Shape, index: i64) -> ([f32; 4], [f32; 3]) {
    let i = usize::try_from(index).ok();
    let q = i
        .and_then(|i| shape.node_arbitrary_scale_rots.get(i))
        .map_or([0.0, 0.0, 0.0, 1.0], |q| quat16(*q));
    let s = i
        .and_then(|i| shape.node_arbitrary_scale_factors.get(i))
        .copied()
        .unwrap_or([1.0; 3]);
    (q, s)
}

/// `m_matF_scale`: the first three columns of every row times the factors, each a float product.
fn scale_matrix(m: &mut Matrix, p: [f32; 3]) {
    for row in 0..4 {
        for (col, factor) in p.iter().enumerate() {
            m[row * 4 + col] *= factor;
        }
    }
}

/// `m = m * (R scaled by s) * R^-1`, R the matrix of the rotation `q`: a scale along arbitrary axes.
fn arbitrary_scale(m: &mut Matrix, q: [f32; 4], s: [f32; 3]) {
    let r = math::quat_matrix(q);
    let inverse = math::mat_inverse(&r);
    let mut scaled = r;
    scale_matrix(&mut scaled, s);
    let s_matrix = math::mat_mul(&scaled, &inverse);
    let old = *m;
    *m = math::mat_mul(&old, &s_matrix);
}

/// The scratch arrays of the scale pass: one value per node, in the shape's scale kind (`smNodeCurrentUniformScales`, `...Aligned...`,
/// `...Arbitrary...`: a rotation and three factors).
struct Scales {
    uniform: Vec<f32>,
    aligned: Vec<[f32; 3]>,
    arbitrary: Vec<([f32; 4], [f32; 3])>,
}

impl Scales {
    fn new(nodes: usize) -> Scales {
        Scales {
            uniform: vec![1.0; nodes],
            aligned: vec![[1.0; 3]; nodes],
            arbitrary: vec![([0.0, 0.0, 0.0, 1.0], [1.0; 3]); nodes],
        }
    }

    /// The thread pass for one node: its scale key pair, blended, goes into the array named by (sequence kind, shape kind).
    /// The pairs Torque leaves untouched (a lower kind in a higher kind's shape) keep the identity scale here.
    fn thread_scale(
        &mut self,
        shape: &Shape,
        kind: u32,
        seq: &Sequence,
        thread: &Thread,
        offset: i64,
        n: usize,
    ) {
        let base = offset + i64::from(seq.base_scale);
        let (i1, i2) = (
            i64::from(thread.key_num1) + base,
            i64::from(thread.key_num2) + base,
        );
        let t = thread.key_pos;
        let seq_kind = if seq.flags & 2 != 0 {
            1
        } else if seq.flags & 4 != 0 {
            2
        } else {
            0
        };
        let mut case = seq_kind;
        if kind & 2 != 0 {
            case += 3;
        }
        if kind & 4 != 0 {
            case += 6;
        }
        match case {
            // The uniform blend, kept as a float (case 0) or as three equal factors.
            0..=2 => {
                let u = lerp1(uniform_key(shape, i1), uniform_key(shape, i2), t);
                match case {
                    0 => self.uniform[n] = u,
                    1 => self.aligned[n] = [u; 3],
                    _ => self.arbitrary[n] = ([0.0, 0.0, 0.0, 1.0], [u; 3]),
                }
            }
            // The aligned blend (cases 4 and 5).
            4 | 5 => {
                let v = lerp3(aligned_key(shape, i1), aligned_key(shape, i2), t);
                if case == 4 {
                    self.aligned[n] = v;
                } else {
                    self.arbitrary[n] = ([0.0, 0.0, 0.0, 1.0], v);
                }
            }
            // The arbitrary blend (case 8): the rotation by the quaternion blend, the factors componentwise.
            8 => {
                let (q1, s1) = arbitrary_key(shape, i1);
                let (q2, s2) = arbitrary_key(shape, i2);
                self.arbitrary[n] = (interpolate_quat(q1, q2, t), lerp3(s1, s2, t));
            }
            _ => {}
        }
    }

    /// Applying the scale for one node: the matrix scaled by the node's entry, by the shape's kind.
    fn apply(&self, kind: u32, m: &mut Matrix, n: usize) {
        if kind & 1 != 0 {
            let u = self.uniform[n];
            scale_matrix(m, [u, u, u]);
        } else if kind & 2 != 0 {
            scale_matrix(m, self.aligned[n]);
        } else {
            let (q, s) = self.arbitrary[n];
            arbitrary_scale(m, q, s);
        }
    }
}

/// The nodes `0..count` as a set (the first [`SET_BITS`] of them).
fn full_set(count: usize) -> NodeSet {
    let mut set = [0u32; 6];
    for n in 0..count.min(SET_BITS) {
        set_add(&mut set, n);
    }
    set
}

fn rotation_key(shape: &Shape, index: i64) -> [f32; 4] {
    usize::try_from(index)
        .ok()
        .and_then(|i| shape.node_rotations.get(i))
        .map_or([0.0, 0.0, 0.0, 1.0], |q| quat16(*q))
}

fn translation_key(shape: &Shape, index: i64) -> [f32; 3] {
    usize::try_from(index)
        .ok()
        .and_then(|i| shape.node_translations.get(i))
        .copied()
        .unwrap_or([0.0; 3])
}

/// The members of a sequence's set below `limit`, ascending.
fn members(set: &IntSet, limit: usize) -> Vec<usize> {
    members_all(&words(set), limit)
}

fn members_all(set: &NodeSet, limit: usize) -> Vec<usize> {
    (0..limit.min(SET_BITS))
        .filter(|n| set_has(set, *n))
        .collect()
}

/// The members of a node set in `[from, to)`.
fn members_range(set: &NodeSet, from: usize, to: usize) -> Vec<usize> {
    (from..to.min(SET_BITS))
        .filter(|n| set_has(set, *n))
        .collect()
}

impl Thread {
    /// `TSThread::setSequence`: the sequence, its priority and path flag, the position (a cyclic sequence's near 1 is
    /// `0.99999`), and the keyframes the position selects. An index the shape does not have changes nothing.
    fn set_sequence(&mut self, shape: &Shape, seq: i32, pos: f32) {
        let Some((index, s)) = usize::try_from(seq)
            .ok()
            .and_then(|i| shape.sequences.get(i).map(|s| (i, s)))
        else {
            return;
        };
        self.sequence = Some(index);
        self.priority = s.priority;
        self.pos = pos;
        self.make_path = s.flags & FLAG_MAKE_PATH != 0;
        let cyclic = s.flags & FLAG_CYCLIC != 0;
        let mut pos = pos;
        if pos > 0.99999 && cyclic {
            self.pos = 0.99999;
            pos = 0.99999;
        }
        self.keys_for(s, pos, cyclic);
    }

    /// The keyframe pair and weight for a position (the tail shared by `setSequence` and `advancePos`).
    fn keys_for(&mut self, s: &Sequence, pos: f32, cyclic: bool) {
        let n = s.num_keyframes;
        if cyclic {
            let f = (n as f32) * pos;
            let i = truncate(f);
            self.key_pos = f - (i as f32);
            self.key_num1 = i;
            self.key_num2 = if i == n.wrapping_sub(1) {
                0
            } else {
                i.wrapping_add(1)
            };
        } else if pos == 1.0 {
            self.key_pos = 0.0;
            self.key_num1 = n.wrapping_sub(1);
            self.key_num2 = n.wrapping_sub(1);
        } else {
            let f = (n.wrapping_sub(1) as f32) * pos;
            let i = truncate(f);
            self.key_pos = f - (i as f32);
            self.key_num1 = i;
            self.key_num2 = i.wrapping_add(1);
        }
    }

    /// `TSThread::advancePos` without a transition: true when the move was big enough to mark the sub-shapes dirty
    /// (`|delta| > 0.00001`).
    fn advance_pos(&mut self, shape: &Shape, delta: f32) -> bool {
        self.pos = clamp01(self.pos);
        let marks = delta.abs() > 0.00001;
        let Some(s) = self.sequence.and_then(|i| shape.sequences.get(i)) else {
            return marks;
        };
        let cyclic = s.flags & FLAG_CYCLIC != 0;
        let old = self.pos;
        let moved = old + delta;
        let local;
        if !self.make_path {
            if !cyclic {
                local = clamp01(moved);
            } else {
                local = wrap(moved, truncate(moved));
            }
            self.pos = local;
        } else if !cyclic {
            local = clamp01(moved);
            self.pos = local;
        } else {
            // A path wraps with the floor, not the truncation.
            let mut whole = truncate(moved);
            if moved < 0.0 {
                whole = whole.wrapping_sub(1);
            }
            local = wrap(moved, whole);
            self.pos = local;
        }
        self.keys_for(s, local, cyclic);
        marks
    }
}

/// `x - (float)whole`, then into `[0, 1)` by one step either way (the cyclic wrap of `advancePos`).
fn wrap(x: f32, whole: i32) -> f32 {
    let mut t = x - whole as f32;
    if t < 0.0 {
        t += 1.0;
    }
    if t >= 1.0 {
        t -= 1.0;
    }
    t
}

#[cfg(test)]
#[path = "instance_tests.rs"]
mod tests;
