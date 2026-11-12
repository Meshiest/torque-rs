//! What a `Vm` of the Torque3D dialect has beyond the runtime's natives: the compiler, and the console methods of
//! the internal names the dialect's `new` and `->` use (Torque3D's `SimObject::getInternalName`,
//! `setInternalName` in `simObject.cpp`, MIT, Copyright (c) 2012 GarageGames, LLC and Copyright (c) 2015 Faust Logic,
//! Inc.; `SimSet::findObjectByInternalName` in `simSet.cpp`, MIT, Copyright (c) 2012 GarageGames, LLC).

use super::Torque3dCompiler;
use super::value::Value;
use crate::vm::{Native, Vm};

/// Installs the Torque3D dialect's compiler and natives in `vm` ([`Vm::new`] does it for its executor).
pub fn install(vm: &mut Vm) {
    vm.set_compiler(Torque3dCompiler);
    vm.register_in(
        "SimObject",
        "getInternalName",
        2,
        2,
        "obj.getInternalName()",
        |vm, c| {
            let name = c
                .this(vm)
                .and_then(|id| vm.objects().get(id))
                .and_then(|o| o.internal_name().map(<[u8]>::to_vec));
            Native::Str(name.unwrap_or_default())
        },
    );
    vm.register_in(
        "SimObject",
        "setInternalName",
        3,
        3,
        "obj.setInternalName(newInternalName)",
        |vm, c| {
            let name = c.arg(2).to_vec();
            if let Some(object) = c.this(vm).and_then(|id| vm.objects_mut().get_mut(id)) {
                object.set_internal_name(&name);
            }
            Native::Void
        },
    );
    vm.register_in(
        "SimSet",
        "findObjectByInternalName",
        3,
        4,
        "set.findObjectByInternalName(internalName, [searchChildren = false])",
        |vm, c| {
            let recurse = Value::text(c.arg(3)).truth();
            let found = c
                .this(vm)
                .filter(|set| vm.objects().is_set(*set))
                .and_then(|set| vm.objects().find_internal(set, c.arg(2), recurse));
            Native::Int(found.map_or(0, |id| id as i32))
        },
    );
}
