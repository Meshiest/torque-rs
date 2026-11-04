//! Behaviour chaining: a host adds its own work to a class's Rust behaviour without retyping the one the class already has, as
//! a C++ override calls `Parent::onAdd()` first.
//!
//! `Vm::set_class_behaviour` replaces; [`chain`] keeps what the class had (`parent`) and runs it around the new one (`own`):
//! `on_add`, `preload`, `on_construct` and `on_add_object` parent first (an `on_add` the parent refuses stops there, as `if
//! (!Parent::onAdd()) return false;` does), `on_remove` own first. The VM runs the behaviours of an object's class chain the
//! same way, so a chained behaviour behaves as one registered on a subclass would. No borrow is held across either call: each
//! may run script.

use std::rc::Rc;

use ts_vm::classes::{EngineClass, PreloadError};
use ts_vm::sim::{ObjId, Objects, SimObject};
use ts_vm::vm::{Vm, VmError};

/// A class's behaviour before a [`chain`], and the one chained after it.
struct Chained {
    parent: Rc<dyn EngineClass>,
    own: Rc<dyn EngineClass>,
}

impl EngineClass for Chained {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        if !self.parent.on_add(vm, id)? {
            return Ok(false);
        }
        self.own.on_add(vm, id)
    }

    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        self.own.on_remove(vm, id)?;
        self.parent.on_remove(vm, id)
    }

    fn preload(&self, vm: &mut Vm, id: ObjId) -> Result<(), PreloadError> {
        self.parent.preload(vm, id)?;
        self.own.preload(vm, id)
    }

    fn on_construct(&self, objects: &Objects, object: &mut SimObject) {
        self.parent.on_construct(objects, object);
        self.own.on_construct(objects, object);
    }

    fn on_add_object(&self, vm: &mut Vm, group: ObjId, child: ObjId) -> Result<(), VmError> {
        self.parent.on_add_object(vm, group, child)?;
        self.own.on_add_object(vm, group, child)
    }
}

/// Gives `class` the behaviour `own` after the one it has (see the module docs); a class with none gets `own` alone. False when
/// there is no class of that name.
pub fn chain(vm: &mut Vm, class: &str, own: Rc<dyn EngineClass>) -> bool {
    let objects = vm.objects();
    let parent = objects
        .classes
        .find(class.as_bytes())
        .and_then(|id| objects.classes.get(id))
        .and_then(|def| def.behaviour.clone());
    let behaviour: Rc<dyn EngineClass> = match parent {
        Some(parent) => Rc::new(Chained { parent, own }),
        None => own,
    };
    vm.set_class_behaviour(class, behaviour)
}
