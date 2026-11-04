//! The matrix and quaternion maths of a scene object's transform text (`getTransform`, `setTransform`, a camera's orbit): TGE's
//! (`ts_vm::geometry`) unless the host names its own rounding ([`set_geometry`], a `Vm` extension), so that the scene's numbers
//! are those of the host's vector functions.

use std::rc::Rc;

use ts_vm::geometry::{Mat4, V3};
use ts_vm::vm::Vm;

/// The four conversions.
#[derive(Clone, Copy)]
pub struct Geometry {
    pub matrix_from_quat: fn([f32; 4]) -> Mat4,
    pub quat_from_axis_angle: fn(V3, f32) -> [f32; 4],
    pub axis_angle_from_quat: fn([f32; 4]) -> (V3, f32),
    pub quat_from_matrix: fn(&Mat4) -> [f32; 4],
}

/// TGE's (`QuatF`, `AngAxisF`, `MatrixF`).
pub const TGE: Geometry = Geometry {
    matrix_from_quat: ts_vm::geometry::matrix_from_quat,
    quat_from_axis_angle: ts_vm::geometry::quat_from_axis_angle,
    axis_angle_from_quat: ts_vm::geometry::axis_angle_from_quat,
    quat_from_matrix: ts_vm::geometry::quat_from_matrix,
};

struct Installed(&'static Geometry);

/// The host's maths for `vm` from now on.
pub fn set_geometry(vm: &mut Vm, geometry: &'static Geometry) {
    vm.set_extension(Rc::new(Installed(geometry)));
}

/// The maths `vm`'s game named, else TGE's.
pub fn of(vm: &Vm) -> &'static Geometry {
    vm.extension::<Installed>().map_or(&TGE, |g| g.0)
}
