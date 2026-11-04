//! Reading and writing the static fields of an object from Rust: the host's one way into the slots the scripts
//! read by name (the design note of [`super`]). A field is found by name through the object's class; a name the class
//! lacks reads as nothing and is not written.

use torque_scene::transform::Matrix;
use ts_vm::fields::Value;
use ts_vm::sim::ObjId;
use ts_vm::vm::Vm;

/// The first element's slot of the static field `name` of the object, if its class has it (remembered by the name's address:
/// [`ts_vm::sim::ClassRegistry::static_slot`]).
pub fn slot_of(vm: &Vm, id: ObjId, name: &'static [u8]) -> Option<usize> {
    let objects = vm.objects();
    let class = objects.get(id)?.class();
    objects.classes.static_slot(class, name)
}

/// [`slot_of`] for a name made at run time.
pub fn slot_named(vm: &Vm, id: ObjId, name: &[u8]) -> Option<usize> {
    let objects = vm.objects();
    let class = objects.get(id)?.class();
    objects
        .classes
        .static_field(class, name)?
        .1
        .first()
        .copied()
}

/// A copy of the field's value now.
pub fn value_of(vm: &Vm, id: ObjId, name: &'static [u8]) -> Option<Value> {
    let object = vm.objects().get(id)?;
    let slot = vm.objects().classes.static_slot(object.class(), name)?;
    object.slot(slot).cloned()
}

/// [`value_of`] for a name made at run time.
pub fn value_named(vm: &Vm, id: ObjId, name: &[u8]) -> Option<Value> {
    let slot = slot_named(vm, id, name)?;
    vm.objects().get(id)?.slot(slot).cloned()
}

/// Reads the field's value in place (no copy), with `f`.
pub fn with_value<R>(
    vm: &Vm,
    id: ObjId,
    name: &'static [u8],
    f: impl FnOnce(&Value) -> R,
) -> Option<R> {
    let object = vm.objects().get(id)?;
    let slot = vm.objects().classes.static_slot(object.class(), name)?;
    object.slot(slot).map(f)
}

/// An `int` field's value now (0 for a field the class does not have).
pub fn int_field(vm: &Vm, id: ObjId, name: &'static [u8]) -> i32 {
    with_value(vm, id, name, |v| match v {
        Value::Int(n) => *n,
        _ => 0,
    })
    .unwrap_or(0)
}

/// A `bool` field's value now (false for a field the class does not have).
pub fn bool_field(vm: &Vm, id: ObjId, name: &'static [u8]) -> bool {
    with_value(vm, id, name, |v| matches!(v, Value::Bool(true))).unwrap_or(false)
}

/// The object a datablock-pointer field points at, if it points at one.
pub fn object_field(vm: &Vm, id: ObjId, name: &'static [u8]) -> Option<ObjId> {
    with_value(vm, id, name, |v| match v {
        Value::Object(object) => *object,
        _ => None,
    })
    .flatten()
}

pub fn set_field(vm: &mut Vm, id: ObjId, name: &'static [u8], value: Value) {
    if let Some(slot) = slot_of(vm, id, name)
        && let Some(stored) = vm
            .objects_mut()
            .get_mut(id)
            .and_then(|object| object.slot_mut(slot))
    {
        *stored = value;
    }
}

/// The object's transform, the matrix its `position` and `rotation` fields share, as sixteen floats.
pub fn matrix_of(vm: &Vm, id: ObjId) -> Option<Matrix> {
    with_value(vm, id, b"position", |v| match v {
        Value::Matrix(rows) => Some(flatten(rows)),
        _ => None,
    })
    .flatten()
}

/// Makes `matrix` the object's transform (false for an object with no such field).
pub fn set_matrix(vm: &mut Vm, id: ObjId, matrix: &Matrix) -> bool {
    let Some(slot) = slot_of(vm, id, b"position") else {
        return false;
    };
    let Some(Value::Matrix(rows)) = vm
        .objects_mut()
        .get_mut(id)
        .and_then(|object| object.slot_mut(slot))
    else {
        return false;
    };
    **rows = rows_of(matrix);
    true
}

/// The object's scale (the field `scale`; `1 1 1` for an object without one).
pub fn scale_of(vm: &Vm, id: ObjId) -> [f32; 3] {
    with_value(vm, id, b"scale", |v| match v {
        Value::Floats3(scale) => *scale,
        _ => [1.0; 3],
    })
    .unwrap_or([1.0; 3])
}

pub fn flatten(rows: &[[f32; 4]; 4]) -> Matrix {
    std::array::from_fn(|i| rows[i / 4][i % 4])
}

pub fn rows_of(matrix: &Matrix) -> [[f32; 4]; 4] {
    std::array::from_fn(|r| std::array::from_fn(|c| matrix[4 * r + c]))
}
