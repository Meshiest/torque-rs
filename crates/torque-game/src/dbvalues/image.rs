//! `ShapeBaseImageData`'s 31 state records: built ONCE, when the datablock is added, from the
//! script arrays as they are then; the sequence, flash-sequence and emitter-node indices are recomputed at every preload.
//! The packer reads the records, never the script arrays, so a later write to an array does not reach the wire.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `T3D/shapeImage.cpp`
//! and `ts/tsShape.cpp` (Copyright (c) 2012 GarageGames, LLC). The code here is this crate's own.

use std::rc::Rc;

use collide::math::{IDENTITY, Matrix};
use shapes::Shape;
use ts_vm::classes::EngineClass;
use ts_vm::sim::ObjId;
use ts_vm::vm::{Vm, VmError};

use super::DbValues;
use super::fields::{bool_of, f32_of, i32_of, object_of, shown_name, text_of};

/// The states of an image.
pub const STATES: usize = 31;

/// The transition arrays, in the order the add resolves them and the wire sends them.
pub const TRANSITIONS: [&str; 11] = [
    "stateTransitionOnNotLoaded",
    "stateTransitionOnLoaded",
    "stateTransitionOnNoAmmo",
    "stateTransitionOnAmmo",
    "stateTransitionOnNoTarget",
    "stateTransitionOnTarget",
    "stateTransitionOnNotWet",
    "stateTransitionOnWet",
    "stateTransitionOnTriggerUp",
    "stateTransitionOnTriggerDown",
    "stateTransitionOnTimeout",
];

/// One state record.
#[derive(Debug, Clone)]
pub struct State {
    /// The name: `None` for a NULL one; an empty name is a pointer to an empty string.
    pub name: Option<Rc<[u8]>>,
    /// The state each transition leads to (-1 for none), in wire order.
    pub trans: [i32; 11],
    pub timeout: f32,
    pub wait_for_timeout: bool,
    pub fire: bool,
    pub eject_shell: bool,
    /// `stateAllowImageChange`.
    pub allow_image_change: bool,
    pub direction: bool,
    pub energy_drain: f32,
    pub loaded_flag: i32,
    pub spin_thread: i32,
    pub recoil: i32,
    /// The sequence index of `stateSequence` in the shape (-1 for none).
    pub sequence: i32,
    pub sequence_vis: i32,
    /// The flash byte: set by a `_vis` sequence, never cleared by the preload.
    pub flash: bool,
    pub ignore_loaded: bool,
    pub sound: Option<ObjId>,
    pub emitter: Option<ObjId>,
    pub emitter_time: f32,
    pub emitter_node: i32,
    /// `stateScript`: the text copied at add (`None` for a NULL one).
    pub script: Option<Rc<[u8]>>,
}

/// The records and the image's flash flag, and the preload's products the state machine and the muzzle chain read.
#[derive(Debug, Clone)]
pub struct ImageStates {
    pub states: Vec<State>,
    pub has_flash: bool,
    /// `useEyeOffset`: `eyeOffset` was not exactly the identity at the last preload.
    pub use_eye_offset: bool,
    /// The shape's `muzzlePoint` node (-1 none), and its matrix in a new instance of the image shape (the default pose: the
    /// instance `setImage` makes on a server is never animated).
    pub muzzle_node: i32,
    pub muzzle_transform: Option<Matrix>,
    /// `mountTransform`: `offset` as it was at the last preload, times the inverse of the `mountPoint` node's default chain.
    pub mount_transform: Matrix,
    /// Counts the add and every preload (the host's cache of the state machine's view follows it).
    pub version: u32,
}

fn until_nul(t: &[u8]) -> &[u8] {
    t.split(|&b| b == 0).next().unwrap_or(&[])
}

/// `lookupState`: the first state whose name equals `name` ignoring case; none named is -1, no match says so and is 0.
fn lookup(vm: &mut Vm, id: ObjId, names: &[Option<Rc<[u8]>>], name: Option<Rc<[u8]>>) -> i32 {
    let Some(name) = name.filter(|n| n.first().is_some_and(|&b| b != 0)) else {
        return -1;
    };
    let want = until_nul(&name);
    for (k, candidate) in names.iter().enumerate() {
        if let Some(candidate) = candidate
            && until_nul(candidate).eq_ignore_ascii_case(want)
        {
            return k as i32;
        }
    }
    let image = shown_name(vm, id);
    vm.error(format!(
        "ShapeBaseImageData:: Could not resolve state \"{}\" for image \"{image}\"",
        String::from_utf8_lossy(want)
    ));
    0
}

/// `ShapeBaseImageData::onAdd` (after `GameBaseData::onAdd`): the records, built from the script arrays now. The sequence
/// and node indices start at -1.
pub struct ImageDataAdd {
    pub values: DbValues,
}

impl EngineClass for ImageDataAdd {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        let names: Vec<Option<Rc<[u8]>>> = (0..STATES)
            .map(|i| text_of(vm, id, "stateName", i))
            .collect();
        let mut states = Vec::with_capacity(STATES);
        for (i, name) in names.iter().enumerate() {
            let mut trans = [-1i32; 11];
            for (k, field) in TRANSITIONS.iter().enumerate() {
                let target = text_of(vm, id, field, i);
                trans[k] = lookup(vm, id, &names, target);
            }
            states.push(State {
                name: name.clone(),
                trans,
                timeout: f32_of(vm, id, "stateTimeoutValue", i),
                wait_for_timeout: bool_of(vm, id, "stateWaitForTimeout", i),
                fire: bool_of(vm, id, "stateFire", i),
                eject_shell: bool_of(vm, id, "stateEjectShell", i),
                direction: bool_of(vm, id, "stateDirection", i),
                energy_drain: f32_of(vm, id, "stateEnergyDrain", i),
                loaded_flag: i32_of(vm, id, "stateLoadedFlag", i),
                spin_thread: i32_of(vm, id, "stateSpinThread", i),
                recoil: i32_of(vm, id, "stateRecoil", i),
                sequence: -1,
                sequence_vis: -1,
                flash: false,
                ignore_loaded: false,
                sound: object_of(vm, id, "stateSound", i),
                emitter: object_of(vm, id, "stateEmitter", i),
                emitter_time: f32_of(vm, id, "stateEmitterTime", i),
                emitter_node: -1,
                allow_image_change: bool_of(vm, id, "stateAllowImageChange", i),
                script: text_of(vm, id, "stateScript", i),
            });
        }
        self.values.set_image(
            id,
            ImageStates {
                states,
                has_flash: false,
                use_eye_offset: false,
                muzzle_node: -1,
                muzzle_transform: None,
                mount_transform: IDENTITY,
                version: 0,
            },
        );
        Ok(true)
    }

    fn on_remove(&self, _vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        self.values.forget(id);
        Ok(())
    }
}

/// `TSShape::findSequence`: the index of the first sequence whose name is the name table's entry for `name`; -1 for none.
fn find_sequence(shape: &Shape, name: &[u8]) -> i32 {
    match shape.find_name(name) {
        Some(n) => shape
            .sequences
            .iter()
            .position(|s| s.name == n)
            .map_or(-1, |p| p as i32),
        None => -1,
    }
}

/// `TSShape::findNode`.
fn find_node(shape: &Shape, name: &[u8]) -> i32 {
    match shape.find_name(name) {
        Some(n) => shape
            .nodes
            .iter()
            .position(|node| node.name == n)
            .map_or(-1, |p| p as i32),
        None => -1,
    }
}

/// What the preload reads of the script arrays for one state.
struct Reads {
    sequence: Option<Rc<[u8]>>,
    random_flash: bool,
    ignore_loaded: bool,
    node: Option<Rc<[u8]>>,
}

/// The records' part of `ShapeBaseImageData::preload` once the shape is loaded: `shape` is `None` for a shape the engine
/// keeps header-only (versions below 24), which has no sequences and no nodes here.
///
/// Per state: the sequence index (an empty `stateSequence` keeps the old one modulo the sequence count; -1 stays -1), the
/// `_vis` sequence (only with `stateSequenceRandomFlash` and a name; found sets the flash byte and the image's `hasFlash`; the
/// modulo path for an old index sets them too), the copy of `stateIgnoreLoadedForReady`, and the emitter node (a name finds the
/// node, -1 is the `muzzlePoint` node, and a first node whose NAME index equals the result is replaced by `muzzlePoint` (a name
/// index is compared with a node index).
pub fn preload(values: &DbValues, vm: &Vm, id: ObjId, shape: Option<&Shape>) {
    let sequences = shape.map_or(0, |s| s.sequences.len() as i32);
    let muzzle = shape.map_or(-1, |s| find_node(s, b"muzzlePoint"));
    let reads: Vec<Reads> = (0..STATES)
        .map(|i| Reads {
            sequence: text_of(vm, id, "stateSequence", i),
            random_flash: bool_of(vm, id, "stateSequenceRandomFlash", i),
            ignore_loaded: bool_of(vm, id, "stateIgnoreLoadedForReady", i),
            node: text_of(vm, id, "stateEmitterNode", i),
        })
        .collect();
    let products = preload_products(vm, id, shape);
    values.with_image_mut(id, |image| {
        image.version = image.version.wrapping_add(1);
        (
            image.use_eye_offset,
            image.muzzle_node,
            image.muzzle_transform,
            image.mount_transform,
        ) = products;
        image.has_flash = false;
        for (state, read) in image.states.iter_mut().zip(reads) {
            let name = read
                .sequence
                .as_deref()
                .map(until_nul)
                .filter(|n| !n.is_empty());
            match name {
                None => {
                    if sequences > 0 && state.sequence != -1 {
                        state.sequence %= sequences;
                    } else {
                        state.sequence = -1;
                    }
                }
                Some(n) => state.sequence = shape.map_or(-1, |s| find_sequence(s, n)),
            }
            match name {
                Some(n) if read.random_flash => {
                    // strncpy 100 bytes, then "_vis" into the 128-byte buffer.
                    let mut buffer: Vec<u8> = n.iter().copied().take(100).collect();
                    buffer.extend_from_slice(b"_vis");
                    state.sequence_vis = shape.map_or(-1, |s| find_sequence(s, &buffer));
                }
                _ => {
                    if sequences > 0 && state.sequence_vis != -1 {
                        state.sequence_vis %= sequences;
                    } else {
                        state.sequence_vis = -1;
                    }
                }
            }
            if state.sequence_vis != -1 {
                state.flash = true;
                image.has_flash = true;
            }
            state.ignore_loaded = read.ignore_loaded;
            if let Some(n) = read
                .node
                .as_deref()
                .map(until_nul)
                .filter(|n| !n.is_empty())
            {
                state.emitter_node = shape.map_or(-1, |s| find_node(s, n));
            }
            if state.emitter_node == -1 {
                state.emitter_node = muzzle;
            }
            if let Some(s) = shape
                && s.nodes
                    .iter()
                    .position(|node| node.name == state.emitter_node)
                    == Some(0)
            {
                state.emitter_node = muzzle;
            }
        }
    });
}

fn matrix_field(vm: &Vm, id: ObjId, name: &[u8]) -> Matrix {
    match crate::scene::access::value_named(vm, id, name) {
        Some(ts_vm::fields::Value::Matrix(m)) => flat(&m),
        _ => IDENTITY,
    }
}

/// A matrix field's rows as 16 floats (row-major, the translation in elements 3, 7 and 11).
pub fn flat(rows: &[[f32; 4]; 4]) -> Matrix {
    std::array::from_fn(|i| rows[i / 4][i % 4])
}

/// Every element compared with the identity's (an ordered `==`, so `-0.0` passes and a NaN does not).
fn is_identity(m: &Matrix) -> bool {
    m.iter().zip(IDENTITY.iter()).all(|(a, b)| a == b)
}

/// The preload's products: `useEyeOffset`, the `muzzlePoint` node and its default-pose matrix, and the
/// mount transform. The mount transform is `offset` copied, and when the shape has a `mountPoint` node, times the
/// inverse (`m_matF_inverse`) of the product, from that node up to the root, of each node's rest matrix (the shorts over
/// 32767, `QuatF::setMatrix`, the default translation): `total = total * node` in that order, as TGE multiplies it.
fn preload_products(
    vm: &Vm,
    id: ObjId,
    shape: Option<&Shape>,
) -> (bool, i32, Option<Matrix>, Matrix) {
    let use_eye = !is_identity(&matrix_field(vm, id, b"eyeOffset"));
    let mut mount = matrix_field(vm, id, b"offset");
    let Some(shape) = shape else {
        return (use_eye, -1, None, mount);
    };
    let muzzle = find_node(shape, b"muzzlePoint");
    let charge = &mut |units: u64| vm.charge_work(units);
    let muzzle_m = usize::try_from(muzzle).ok().and_then(|n| {
        shapes::instance::ShapeInstance::new(shape, charge)
            .ok()?
            .node_transform(n)
            .copied()
    });
    let mp = find_node(shape, b"mountPoint");
    if let Ok(mut node) = usize::try_from(mp) {
        let mut total = IDENTITY;
        // The parent walk is bounded by the node count (a well-formed shape's chain is shorter).
        for _ in 0..=shape.nodes.len() {
            let (Some(q), Some(t)) = (
                shape.default_rotations.get(node),
                shape.default_translations.get(node),
            ) else {
                break;
            };
            total = shapes::math::mat_mul(&total, &shapes::math::node_matrix(*q, *t));
            match shape.nodes.get(node).map(|n| n.parent) {
                Some(p) if p >= 0 => node = p as usize,
                _ => break,
            }
        }
        let inv = collide::math::inverse(&total);
        mount = shapes::math::mat_mul(&mount, &inv);
    }
    (use_eye, muzzle, muzzle_m, mount)
}
