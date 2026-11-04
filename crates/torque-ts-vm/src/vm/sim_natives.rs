//! The natives that work on objects: the global ones, the SimObject methods and `delete`, the SimSet
//! methods and the datablock functions. Usage strings are TGE's.
//!
//! Follows Torque3D's MIT-licensed `console/simObject.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (c) 2015
//! Faust Logic, Inc.) and `console/simSet.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's
//! own.

use super::{Native, NativeCall, Vm, VmError, lossy};
use crate::sim::{Members, ObjId, SimObject};
use crate::text::atoi;

/// Argument `i`, or the empty string.
fn arg(c: &NativeCall, i: usize) -> &[u8] {
    c.argv.get(i).map_or(&b""[..], Vec::as_slice)
}

/// The object a method native was called on. None for a plain call made outside any method: the
/// TGE crashes there; the methods below give their neutral value.
fn this(vm: &Vm, c: &NativeCall) -> Option<ObjId> {
    c.this_object.filter(|id| vm.objects.contains(*id))
}

pub(super) fn register_globals(vm: &mut Vm) {
    // "0" and "" are never objects; anything else is looked up as `Sim::findObject` does.
    vm.register(
        "isObject",
        2,
        2,
        "isObject(object) - whether the object exists",
        |vm, c| {
            let text = arg(c, 1);
            Native::Bool(text != b"0" && !text.is_empty() && vm.objects.find(text).is_some())
        },
    );
    vm.register(
        "nameToID",
        2,
        2,
        "nameToID(name) - the id of the object of that name",
        |vm, c| Native::Int(vm.objects.find(arg(c, 1)).map_or(-1, |id| id as i32)),
    );
}

/// One non-empty static element as `dump` sorts and prints it.
struct Printed {
    /// The folded field name, then the element: fields sort by name, and an array's
    /// elements in index order (`x[2]` before `x[10]`).
    key: Vec<u8>,
    element: usize,
    name: Vec<u8>,
    value: Vec<u8>,
}

/// The most bytes of a name or of a value that a `dump` line carries. The log keeps `MAX_LOG_LINE` bytes of a
/// line and an escape is never shorter than its byte: the first `MAX_LOG_LINE + 1` bytes of a (possibly huge)
/// value are all a line can show (one more than it keeps, as TGE's printing cuts at a buffer that
/// holds the terminator).
const DUMP_PART_BYTES: usize = super::MAX_LOG_LINE + 1;

/// A `dump` line: `  name = "value"`, the value expanded (`expandEscape`). Only the first
/// [`DUMP_PART_BYTES`] bytes of the name and of the value are copied; the log cuts what follows anyway.
fn field_line(name: &[u8], value: &[u8]) -> Vec<u8> {
    let mut line = b"  ".to_vec();
    line.extend_from_slice(name.get(..DUMP_PART_BYTES).unwrap_or(name));
    line.extend_from_slice(b" = \"");
    line.extend_from_slice(&crate::text::expand_escape(
        value.get(..DUMP_PART_BYTES).unwrap_or(value),
    ));
    line.push(b'"');
    line
}

/// The work of sorting `n` elements by comparisons: `n` times the bits of `n`, the comparisons each element
/// takes (rounded up).
fn sort_work(n: usize) -> u64 {
    (n as u64).saturating_mul(u64::from(usize::BITS - n.leading_zeros()))
}

/// `SimObject::dump`: the non-empty static fields sorted ignoring case
/// (arrays as `name[i]`, each non-empty element), the dynamic fields, and every function of the object's
/// namespace chain with its usage; values expanded (`expandEscape`).
///
/// Charged by the work it does, and the step budget is asked after every field and every line: a static
/// element by the slots it visits and the bytes its value formats to (`fields::get_charged`), the sort by its
/// comparisons, a line by its expanded length (a name and a value are copied only up to [`DUMP_PART_BYTES`]);
/// the namespace walk charges its own entries. A dump the budget cannot pay for stops at the field or line
/// that crossed it with `StepLimit` and prints nothing, so the overshoot is at most one field's formatting or
/// one line, never the whole object. What it prints is charged once more by `print` (the bytes the log keeps).
#[inline(never)]
fn dump(vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
    let lines = dump_lines(vm, id)?;
    for line in lines {
        vm.print(&line);
    }
    Ok(())
}

/// The lines of [`dump`], built under the step budget.
#[inline(never)]
fn dump_lines(vm: &Vm, id: ObjId) -> Result<Vec<Vec<u8>>, VmError> {
    let Some(object) = vm.objects.get(id) else {
        return Ok(Vec::new());
    };
    let objects = &vm.objects;
    let names = |id: ObjId| objects.get(id).and_then(|o| o.name().map(<[u8]>::to_vec));
    let mut charge = |units: u64| vm.charge_work(units);
    let pay = |units: u64| {
        if vm.charge_work(units) {
            Ok(())
        } else {
            Err(VmError::StepLimit)
        }
    };
    let mut statics: Vec<Printed> = Vec::new();
    if let Some(class) = objects.classes.get(object.class()) {
        pay(class.layout.fields.len() as u64)?;
        for (field, slots) in &class.layout.fields {
            if matches!(field.ty, crate::fields::FieldType::Deprecated) {
                continue;
            }
            pay(slots.len() as u64)?;
            for (k, slot) in slots.iter().enumerate() {
                let Some(value) = object.slot(*slot) else {
                    continue;
                };
                let text = crate::fields::get_charged(
                    objects.classes.field_rules,
                    field.ty,
                    value,
                    &names,
                    &mut charge,
                );
                vm.ensure_budget()?;
                if text.is_empty() {
                    continue;
                }
                let mut name = field.name.clone();
                if field.count > 1 {
                    name.extend_from_slice(format!("[{k}]").as_bytes());
                }
                statics.push(Printed {
                    key: crate::text::fold(&field.name),
                    element: k,
                    name,
                    value: text,
                });
            }
        }
    }
    pay(sort_work(statics.len()))?;
    statics.sort_by(|a, b| (&a.key, a.element).cmp(&(&b.key, b.element)));
    let mut lines: Vec<Vec<u8>> = vec![b"Member Fields:".to_vec()];
    for printed in &statics {
        let line = field_line(&printed.name, &printed.value);
        pay(line.len() as u64)?;
        lines.push(line);
    }
    lines.push(b"Tagged Fields:".to_vec());
    pay(object.fields.len() as u64)?;
    for (name, value) in object.fields.iter() {
        let line = field_line(name, value);
        pay(line.len() as u64)?;
        lines.push(line);
    }
    lines.push(b"Methods:".to_vec());
    for (name, usage) in vm.namespaces.chain_functions(object.namespace()) {
        let mut line = b"  ".to_vec();
        line.extend_from_slice(name.get(..DUMP_PART_BYTES).unwrap_or(&name));
        line.extend_from_slice(b"() - ");
        line.extend_from_slice(usage.as_bytes());
        pay(line.len() as u64)?;
        lines.push(line);
    }
    Ok(lines)
}

pub(super) fn register_object_methods(vm: &mut Vm) {
    vm.register_in(
        "SimObject",
        "dump",
        2,
        2,
        "object.dump() - prints the object's fields and methods",
        |vm, c| match this(vm, c).map(|id| dump(vm, id)) {
            Some(Err(error)) => Native::Fail(error),
            _ => Native::Void,
        },
    );
    vm.register_in(
        "SimObject",
        "getId",
        2,
        2,
        "object.getId() - the object's id",
        |vm, c| Native::Int(this(vm, c).map_or(0, |id| id as i32)),
    );
    vm.register_in(
        "SimObject",
        "getName",
        2,
        2,
        "object.getName() - the object's name",
        |vm, c| {
            let name = this(vm, c)
                .and_then(|id| vm.objects.get(id))
                .and_then(SimObject::name);
            Native::Str(name.unwrap_or_default().to_vec())
        },
    );
    vm.register_in(
        "SimObject",
        "setName",
        3,
        3,
        "object.setName(name) - renames the object",
        |vm, c| {
            if let Some(id) = this(vm, c) {
                // `assignName`: the name through the string table (the first spelling of it the table holds).
                let name = arg(c, 2);
                let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
                let name = vm.strings.intern(&name[..end]);
                vm.objects.set_name(id, &name);
            }
            Native::Void
        },
    );
    vm.register_in(
        "SimObject",
        "getClassName",
        2,
        2,
        "object.getClassName() - the object's class",
        |vm, c| {
            let class = this(vm, c)
                .and_then(|id| vm.objects.get(id))
                .map(SimObject::class);
            Native::Str(
                class
                    .map(|k| vm.objects.classes.name(k).to_vec())
                    .unwrap_or_default(),
            )
        },
    );
    vm.register_in(
        "SimObject",
        "getGroup",
        2,
        2,
        "object.getGroup() - the group that holds the object",
        |vm, c| {
            let group = this(vm, c)
                .and_then(|id| vm.objects.get(id))
                .and_then(SimObject::group);
            Native::Int(group.map_or(-1, |g| g as i32))
        },
    );
    // The type mask of scene objects; 0 for everything this crate knows.
    vm.register_in(
        "SimObject",
        "getType",
        2,
        2,
        "object.getType() - the object's type mask",
        |_, _| Native::Int(0),
    );
}

pub(super) fn register_set_methods(vm: &mut Vm) {
    vm.register_in(
        "SimObject",
        "delete",
        2,
        2,
        "object.delete() - deletes the object",
        |vm, c| {
            let Some(id) = this(vm, c) else {
                return Native::Void;
            };
            match vm.delete_object(id) {
                Ok(()) => Native::Void,
                Err(e) => Native::Fail(e),
            }
        },
    );
    vm.register_in(
        "SimSet",
        "add",
        3,
        0,
        "set.add(objects...) - adds objects to the set",
        |vm, c| {
            let Some(set) = this(vm, c) else {
                return Native::Void;
            };
            for text in c.argv.iter().skip(2) {
                match vm.objects.find(text) {
                    // A set may hold itself: TGE has no check.
                    Some(id) => {
                        vm.objects.add(set, id);
                    }
                    None => vm.print(
                        format!("Set::add: Object \"{}\" doesn't exist", lossy(text)).as_bytes(),
                    ),
                }
            }
            Native::Void
        },
    );
    vm.register_in(
        "SimSet",
        "remove",
        3,
        0,
        "set.remove(objects...) - removes objects from the set",
        |vm, c| {
            let Some(set) = this(vm, c) else {
                return Native::Void;
            };
            for text in c.argv.iter().skip(2) {
                let removed = vm
                    .objects
                    .find(text)
                    .is_some_and(|id| vm.objects.is_member(set, id) && vm.objects.remove(set, id));
                if !removed {
                    vm.print(
                        format!(
                            "Set::remove: Object \"{}\" does not exist in set",
                            lossy(text)
                        )
                        .as_bytes(),
                    );
                }
            }
            Native::Void
        },
    );
    vm.register_in(
        "SimSet",
        "clear",
        2,
        2,
        "set.clear() - removes every object from the set",
        |vm, c| {
            if let Some(set) = this(vm, c) {
                vm.objects.clear(set);
            }
            Native::Void
        },
    );
    vm.register_in(
        "SimSet",
        "getCount",
        2,
        2,
        "set.getCount() - how many objects the set holds",
        |vm, c| {
            let count = this(vm, c)
                .and_then(|set| vm.objects.members(set))
                .map_or(0, Members::len);
            Native::Int(i32::try_from(count).unwrap_or(i32::MAX))
        },
    );
    vm.register_in(
        "SimSet",
        "getObject",
        3,
        3,
        "set.getObject(index) - the object at that index",
        |vm, c| {
            let Some(set) = this(vm, c) else {
                return Native::Int(-1);
            };
            let index = atoi(arg(c, 2));
            let members = vm.objects.members(set);
            match usize::try_from(index).ok().and_then(|i| members?.get(i)) {
                Some(id) => Native::Int(id as i32),
                None => {
                    vm.print(b"Set::getObject index out of range.");
                    Native::Int(-1)
                }
            }
        },
    );
    vm.register_in(
        "SimSet",
        "isMember",
        3,
        3,
        "set.isMember(object) - whether the set holds the object",
        |vm, c| {
            let Some(set) = this(vm, c) else {
                return Native::Bool(false);
            };
            match vm.objects.find(arg(c, 2)) {
                Some(id) => Native::Bool(vm.objects.is_member(set, id)),
                None => {
                    let line = vm.messages().is_member_no_object(&lossy(arg(c, 2)));
                    vm.print(line.as_bytes());
                    Native::Bool(false)
                }
            }
        },
    );
    vm.register_in(
        "SimSet",
        "bringToFront",
        3,
        3,
        "set.bringToFront(object) - moves the object to the front",
        |vm, c| move_member(vm, c, true),
    );
    vm.register_in(
        "SimSet",
        "pushToBack",
        3,
        3,
        "set.pushToBack(object) - moves the object to the back",
        |vm, c| move_member(vm, c, false),
    );
    vm.register_in(
        "SimSet",
        "listObjects",
        2,
        2,
        "set.listObjects() - prints the set's objects",
        |vm, c| {
            let Some(set) = this(vm, c) else {
                return Native::Void;
            };
            let members: Vec<ObjId> = vm
                .objects
                .members(set)
                .map(|m| m.as_slice().to_vec())
                .unwrap_or_default();
            // One log line per member: charged like the other loops over a script-built set.
            vm.objects.add_work(members.len() as u64);
            for id in members {
                let Some(object) = vm.objects.get(id) else {
                    continue;
                };
                let class = lossy(vm.objects.classes.name(object.class()));
                let mark = if vm.objects.is_set(id) { "(g)" } else { "" };
                let line = match object.name() {
                    Some(name) => format!("   {},\"{}\": {class} {mark}", id as i32, lossy(name)),
                    None => format!("   {}: {class} {mark}", id as i32),
                };
                vm.print(line.as_bytes());
            }
            Native::Void
        },
    );
}

/// `bringToFront` / `pushToBack`: an unknown object or a non-member changes nothing.
fn move_member(vm: &mut Vm, c: &NativeCall, to_front: bool) -> Native {
    if let (Some(set), Some(id)) = (this(vm, c), vm.objects.find(arg(c, 2))) {
        vm.objects.move_member(set, id, to_front);
    }
    Native::Void
}

#[cfg(test)]
#[path = "sim_natives_tests.rs"]
mod tests;
