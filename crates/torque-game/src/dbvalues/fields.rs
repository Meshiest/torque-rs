//! Reading and writing the static fields of a datablock from Rust, element by element (`name[element]`; a `Point3F` or `ColorF`
//! is one element holding its components). A name the class lacks reads as nothing and is not written.

use std::rc::Rc;

use ts_vm::fields::Value;
use ts_vm::sim::ObjId;
use ts_vm::vm::Vm;

fn slot(vm: &Vm, id: ObjId, name: &str, element: usize) -> Option<usize> {
    let objects = vm.objects();
    let class = objects.get(id)?.class();
    objects
        .classes
        .static_field(class, name.as_bytes())?
        .1
        .get(element)
        .copied()
}

/// A copy of the element's value now.
pub fn value(vm: &Vm, id: ObjId, name: &str, element: usize) -> Option<Value> {
    let slot = slot(vm, id, name, element)?;
    vm.objects().get(id)?.slot(slot).cloned()
}

/// Stores `value` in the element (a member write: no validator, no observer).
pub fn put(vm: &mut Vm, id: ObjId, name: &str, element: usize, value: Value) {
    if let Some(slot) = slot(vm, id, name, element)
        && let Some(stored) = vm
            .objects_mut()
            .get_mut(id)
            .and_then(|object| object.slot_mut(slot))
    {
        *stored = value;
    }
}

pub fn f32_of(vm: &Vm, id: ObjId, name: &str, element: usize) -> f32 {
    match value(vm, id, name, element) {
        Some(Value::Float(x)) => x,
        _ => 0.0,
    }
}

pub fn i32_of(vm: &Vm, id: ObjId, name: &str, element: usize) -> i32 {
    match value(vm, id, name, element) {
        Some(Value::Int(x)) => x,
        _ => 0,
    }
}

pub fn bool_of(vm: &Vm, id: ObjId, name: &str, element: usize) -> bool {
    matches!(value(vm, id, name, element), Some(Value::Bool(true)))
}

pub fn vec3_of(vm: &Vm, id: ObjId, name: &str) -> [f32; 3] {
    match value(vm, id, name, 0) {
        Some(Value::Floats3(v)) => v,
        _ => [0.0; 3],
    }
}

pub fn color_of(vm: &Vm, id: ObjId, name: &str) -> [f32; 4] {
    match value(vm, id, name, 0) {
        Some(Value::ColorF(c)) => c,
        _ => [0.0; 4],
    }
}

/// The element's object, if it points at one.
pub fn object_of(vm: &Vm, id: ObjId, name: &str, element: usize) -> Option<ObjId> {
    match value(vm, id, name, element) {
        Some(Value::Object(object)) => object,
        _ => None,
    }
}

/// The element's text, `None` for a NULL pointer: a field never assigned (it still holds the text it started with, the
/// empty one).
pub fn text_of(vm: &Vm, id: ObjId, name: &str, element: usize) -> Option<Rc<[u8]>> {
    let objects = vm.objects();
    let object = objects.get(id)?;
    let slot = slot(vm, id, name, element)?;
    let Some(Value::Text(now)) = object.slot(slot) else {
        return None;
    };
    let unset = objects
        .classes
        .get(object.class())
        .and_then(|c| c.layout.initial.get(slot))
        .is_some_and(|first| match first {
            Value::Text(first) => first.is_empty() && Rc::ptr_eq(now, first),
            _ => false,
        });
    (!unset).then(|| Rc::clone(now))
}

/// An object's name as `%s` of a null pointer shows it.
pub fn shown_name(vm: &Vm, id: ObjId) -> String {
    vm.objects().get(id).and_then(|o| o.name()).map_or_else(
        || "(null)".to_string(),
        |n| String::from_utf8_lossy(n).into_owned(),
    )
}

/// A one-element `Bool` static field of the object, read from the object table.
pub fn bool_in(objects: &ts_vm::sim::Objects, id: ObjId, name: &str) -> bool {
    let Some(object) = objects.get(id) else {
        return false;
    };
    let Some((_, slots)) = objects
        .classes
        .static_field(object.class(), name.as_bytes())
    else {
        return false;
    };
    matches!(
        slots.first().and_then(|&s| object.slot(s)),
        Some(Value::Bool(true))
    )
}

/// Puts the element back to the value a fresh object holds: for a string, a NULL pointer (what a preload writes with `= 0`),
/// which the view and `text_of` read as unset, not as an empty text.
pub fn unset(vm: &mut Vm, id: ObjId, name: &str, element: usize) {
    let initial = {
        let objects = vm.objects();
        let Some(object) = objects.get(id) else {
            return;
        };
        slot(vm, id, name, element).and_then(|s| {
            objects
                .classes
                .get(object.class())
                .and_then(|c| c.layout.initial.get(s))
                .cloned()
        })
    };
    if let Some(initial) = initial {
        put(vm, id, name, element, initial);
    }
}
