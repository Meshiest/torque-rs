//! The values the engine computes for a datablock when it is added or preloaded, which `SimDataBlockEvent` then packs.
//!
//! **Where they live.** A clamp written back into a member the script can read (a `DebrisData` friction, an `AudioDescription`
//! volume, a `PlayerData` jump delay) is written back into the static field, so `%db.field` reads the clamped value. A value
//! with no field of its own (the image's 31 state records, the emitter's id list) lives in [`DbValues`], a
//! side table keyed by the object's id and emptied when the object goes or `deleteDataBlocks()` runs. The packers' view
//! (the host's datablock packer) reads both.
//!
//! **Behaviours.** [`register`] gives the classes with an add-time or preload step their [`EngineClass`] (the classes that also
//! load a shape get theirs through [`shapes`](crate::shapes), which composes these); the checks themselves are the host's
//! [`DatablockRules`](rules::DatablockRules), TGE's by default. A datablock declared again runs no `onAdd`, only its
//! `preload` (`compiledEval.cpp`'s `OP_ADD_OBJECT` runs `onAdd` once, at the registration, and `DebrisData::onAdd` in
//! `debris.cpp` clamps and warns there), so a redeclare writes no clamp: the fields hold what the script wrote, as a field
//! assignment's do. What a redeclare must rebuild is state computed from the fields into a table of its own (the emitter's
//! particle id list): a class with such state is wrapped ([`rederived`]), and the preload of a redeclare runs its add-time
//! checks again with the warnings held back and every field put back as it was, so only the table changes. A redeclare whose
//! values the checks refuse prints the checks' own warnings and the datablock keeps the values (a first declaration is
//! deleted), so no object is left pointing at a deleted datablock. A client that is sent a changed datablock puts the
//! values in range itself, as its first add did (`Vm::recheck_engine_datablock`, the warnings held); a changed record the
//! checks refuse is dropped (the datablock keeps what it held) where the first add deletes the datablock, and an error line
//! a check prints is said again for each changed record.

pub mod fields;
pub mod image;
pub mod rules;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use ts_vm::classes::{EngineClass, PreloadError};
use ts_vm::sim::ObjId;
use ts_vm::vm::{Vm, VmError};

pub use image::{ImageStates, State};

/// The function that parses a datablock of the host's again.
pub type Prepack = Rc<dyn Fn(&mut Vm, ObjId)>;

#[derive(Default)]
struct Tables {
    /// `ParticleEmitterData`: the raw ids of the `ParticleData` blocks its `particles` string named at add time.
    emitter_ids: HashMap<ObjId, Vec<i32>>,
    /// `ShapeBaseImageData`: the 31 state records (built at add, refreshed at each preload).
    images: HashMap<ObjId, ImageStates>,
    /// What runs before a datablock's pack (a host's datablock that parses again when it is dirty), set by the host.
    prepack: Option<Prepack>,
    /// The host's own tables of datablock state, which forget and clear with these.
    game: Option<Rc<dyn GameTables>>,
    /// The datablocks whose add-time checks ran for the declaration that made them and whose first `preload` is still to
    /// come ([`Rederive`]).
    added: HashSet<ObjId>,
}

/// A host's side tables of datablock state, kept in [`DbValues`] so that they forget an object and clear with the engine's.
pub trait GameTables {
    /// Forgets everything (`deleteDataBlocks()`).
    fn clear(&self);
    /// Forgets one object.
    fn forget(&self, id: ObjId);
    /// The tables themselves, for the host's code.
    fn as_any(&self) -> &dyn std::any::Any;
}

/// The side tables of the computed values. Cheap to clone; clones share the tables.
#[derive(Clone, Default)]
pub struct DbValues(Rc<RefCell<Tables>>);

impl std::fmt::Debug for DbValues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let t = self.0.borrow();
        write!(
            f,
            "DbValues {{ emitters: {}, images: {} }}",
            t.emitter_ids.len(),
            t.images.len()
        )
    }
}

impl DbValues {
    pub fn new() -> DbValues {
        DbValues::default()
    }

    /// Forgets everything (`deleteDataBlocks()` starts the ids again).
    pub fn clear(&self) {
        let prepack = self.0.borrow_mut().prepack.take();
        let game = self.0.borrow_mut().game.take();
        if let Some(g) = &game {
            g.clear();
        }
        *self.0.borrow_mut() = Tables {
            prepack,
            game,
            ..Tables::default()
        };
    }

    /// Forgets one object (it was deleted, or its add failed).
    pub fn forget(&self, id: ObjId) {
        let mut t = self.0.borrow_mut();
        t.emitter_ids.remove(&id);
        t.images.remove(&id);
        t.added.remove(&id);
        if let Some(g) = &t.game {
            g.forget(id);
        }
    }

    /// Gives the host's tables a place beside these.
    pub fn set_game(&self, tables: Rc<dyn GameTables>) {
        self.0.borrow_mut().game = Some(tables);
    }

    /// The host's tables, if it gave any.
    pub fn game(&self) -> Option<Rc<dyn GameTables>> {
        self.0.borrow().game.clone()
    }

    // ---- the add-time checks on a redeclare

    /// The add-time checks of `id` ran for the declaration making it: its first `preload` is not a redeclare.
    fn mark_added(&self, id: ObjId) {
        self.0.borrow_mut().added.insert(id);
    }

    /// Whether `id`'s add-time checks ran for the declaration that is preloading it now (true once per add).
    fn take_added(&self, id: ObjId) -> bool {
        self.0.borrow_mut().added.remove(&id)
    }

    // ---- the emitter's id list

    pub fn set_emitter_ids(&self, id: ObjId, ids: Vec<i32>) {
        self.0.borrow_mut().emitter_ids.insert(id, ids);
    }

    /// The raw ids of the emitter's particles (empty for an object that has none).
    pub fn emitter_ids(&self, id: ObjId) -> Vec<i32> {
        self.0
            .borrow()
            .emitter_ids
            .get(&id)
            .cloned()
            .unwrap_or_default()
    }

    // ---- the image's states

    pub fn set_image(&self, id: ObjId, states: ImageStates) {
        self.0.borrow_mut().images.insert(id, states);
    }

    /// Runs `f` on the image's states, if the object has any.
    pub fn with_image<R>(&self, id: ObjId, f: impl FnOnce(&ImageStates) -> R) -> Option<R> {
        self.0.borrow().images.get(&id).map(f)
    }

    pub fn with_image_mut<R>(&self, id: ObjId, f: impl FnOnce(&mut ImageStates) -> R) -> Option<R> {
        self.0.borrow_mut().images.get_mut(&id).map(f)
    }

    /// Gives the pack what runs before it (the host's re-preload of a dirty datablock).
    pub fn set_prepack(&self, f: Prepack) {
        self.0.borrow_mut().prepack = Some(f);
    }

    /// Before the pack of datablock `id`: the host's prepack, if it set one.
    pub fn before_pack(&self, vm: &mut Vm, id: ObjId) {
        let prepack = self.0.borrow().prepack.clone();
        if let Some(prepack) = prepack {
            prepack(vm, id);
        }
    }
}

/// An add-time behavior whose derived state is rebuilt when the datablock is declared again (see the module docs): `on_add`
/// marks the add, the first `preload` after it takes the mark and does nothing, every later `preload` is a redeclare and runs the
/// checks again with their warnings held and every field put back, so only the state the checks compute into a table of its own
/// follows the redeclare.
struct Rederive {
    add: Rc<dyn EngineClass>,
    values: DbValues,
}

impl EngineClass for Rederive {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        self.values.mark_added(id);
        self.add.on_add(vm, id)
    }

    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        self.values.take_added(id);
        self.add.on_remove(vm, id)
    }

    /// The checks again for a redeclare, their warnings held back and the fields they wrote put back; a redeclare they refuse (or
    /// fail) says them after all, and the datablock stays as declared.
    fn preload(&self, vm: &mut Vm, id: ObjId) -> Result<(), PreloadError> {
        if self.values.take_added(id) {
            return Ok(());
        }
        let fields = vm.static_values(id);
        let (added, warnings) = vm.holding_warnings(|vm| self.add.on_add(vm, id));
        vm.restore_static_values(id, fields);
        if !matches!(added, Ok(true)) {
            for line in warnings {
                vm.warn(line);
            }
        }
        added.map_err(PreloadError::Vm)?;
        Ok(())
    }
}

/// `add` wrapped so that its checks rebuild the state they compute on a redeclare ([`Rederive`]); for a class whose add-time
/// behavior computes such state (the emitter's particle list).
pub fn rederived(values: &DbValues, add: Rc<dyn EngineClass>) -> Rc<dyn EngineClass> {
    Rc::new(Rederive {
        add,
        values: values.clone(),
    })
}

/// Gives the classes with an add-time or preload step of their own their behaviour (the shape classes are composed by
/// [`shapes::register`](crate::shapes::register), which calls this first), and the constructor defaults over the class table's.
pub fn register(vm: &mut Vm, values: &DbValues) {
    let rules = rules::rules(vm);
    let simple: [(&str, Rc<dyn EngineClass>); 4] = [
        ("AudioDescription", rules.audio_description()),
        ("AudioProfile", rules.audio_profile()),
        ("DebrisData", rules.debris()),
        ("ParticleData", rules.particle()),
    ];
    for (class, behaviour) in simple {
        vm.set_class_behaviour(class, behaviour);
    }
    vm.set_class_behaviour(
        "ParticleEmitterData",
        rederived(values, rules.particle_emitter(values)),
    );
    rules.constructor_defaults(vm);
    // `%particle.reload()` and `%emitter.reload()`: the add-time checks again, their result dropped; the emitter's id list is
    // rebuilt when the checks got that far.
    let particle_rules = Rc::clone(&rules);
    register_reload(vm, "ParticleData", move |vm, id| {
        particle_rules.particle_checks(vm, id);
    });
    let values = values.clone();
    register_reload(vm, "ParticleEmitterData", move |vm, id| {
        let has_ids = !values.emitter_ids(id).is_empty();
        if let (_, Some(ids)) = rules.emitter_checks(vm, id, has_ids) {
            values.set_emitter_ids(id, ids);
        }
    });
}

/// Replaces the table's stub of the console method `reload` of `class`.
fn register_reload(vm: &mut Vm, class: &str, f: impl Fn(&mut Vm, ObjId) + 'static) {
    let Some((class, spec)) = crate::methods::spec(vm, class, "reload") else {
        return;
    };
    vm.register_in(
        class,
        spec.name,
        spec.min,
        spec.max,
        spec.usage,
        move |vm, call| {
            if let Some(id) = call.this_object {
                f(vm, id);
            }
            ts_vm::vm::Native::Void
        },
    );
}
