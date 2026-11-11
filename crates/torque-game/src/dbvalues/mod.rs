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
//! `preload`, so the add-time checks are wrapped ([`revalidated`]): the preload of a redeclare runs them again, silently,
//! and the clamps and the emitter's particle list are those a fresh declaration of the same fields would leave. The engine
//! has no such step (`compiledEval.cpp`'s `OP_ADD_OBJECT` runs `onAdd` once, at the registration, and `DebrisData::onAdd`
//! in `debris.cpp` prints its warnings there), so only the warnings follow it: they are printed once, for the declaration that
//! adds the datablock. A redeclare whose values the checks refuse prints the checks' own warnings and the datablock keeps the
//! values (a first declaration is deleted), so no object is left pointing at a deleted datablock.

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
    /// come ([`Revalidate`]).
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

/// An add-time behavior whose checks run again when the datablock is declared again (see the module docs): `on_add` marks
/// the add, the first `preload` after it takes the mark and does nothing, every later `preload` is a redeclare and runs the
/// checks again without their warnings (printed once, at the add, as the engine's `onAdd` does).
struct Revalidate {
    add: Rc<dyn EngineClass>,
    values: DbValues,
}

impl EngineClass for Revalidate {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        self.values.mark_added(id);
        self.add.on_add(vm, id)
    }

    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        self.values.take_added(id);
        self.add.on_remove(vm, id)
    }

    /// The checks again for a redeclare, their warnings held back; a redeclare they refuse (or fail) says them after all, and the
    /// datablock stays as declared.
    fn preload(&self, vm: &mut Vm, id: ObjId) -> Result<(), PreloadError> {
        if self.values.take_added(id) {
            return Ok(());
        }
        let (added, warnings) = vm.holding_warnings(|vm| self.add.on_add(vm, id));
        if !matches!(added, Ok(true)) {
            for line in warnings {
                vm.warn(line);
            }
        }
        added.map_err(PreloadError::Vm)?;
        Ok(())
    }
}

/// `add` wrapped so that its checks run again on a redeclare ([`Revalidate`]); for a class whose add-time behavior is one of
/// another behavior's (the shape classes').
pub fn revalidated(values: &DbValues, add: Rc<dyn EngineClass>) -> Rc<dyn EngineClass> {
    Rc::new(Revalidate {
        add,
        values: values.clone(),
    })
}

/// Gives the classes with an add-time or preload step of their own their behaviour (the shape classes are composed by
/// [`shapes::register`](crate::shapes::register), which calls this first), and the constructor defaults over the class table's.
pub fn register(vm: &mut Vm, values: &DbValues) {
    let rules = rules::rules(vm);
    let simple: [(&str, Rc<dyn EngineClass>); 5] = [
        ("AudioDescription", rules.audio_description()),
        ("AudioProfile", rules.audio_profile()),
        ("DebrisData", rules.debris()),
        ("ParticleData", rules.particle()),
        ("ParticleEmitterData", rules.particle_emitter(values)),
    ];
    for (class, behaviour) in simple {
        vm.set_class_behaviour(class, revalidated(values, behaviour));
    }
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
